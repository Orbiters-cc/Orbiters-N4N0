//! Lab only: quantise an existing tuned v1 tree, preserving every node and its contiguous splat range.
//! Usage: compact_tree <pc.n4tree> <output folder>. Decoded leaf PLYs feed the existing eval/offpath scorers.
use orbiters_reconstruct::{
    export,
    gaussians::{Gaussians, logit},
    ply,
};
use std::{path::Path, time::Instant};
fn u32at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn f32at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn main() -> anyhow::Result<()> {
    let a: Vec<_> = std::env::args().collect();
    anyhow::ensure!(a.len() == 3, "compact_tree <tree> <out>");
    let b = std::fs::read(&a[1])?;
    anyhow::ensure!(b.len() >= 32 && &b[..8] == b"N4N0TREE" && u32at(&b, 8) == 1, "expected v1 tree");
    let nodes = u32at(&b, 12) as usize;
    let count = u32at(&b, 16) as usize;
    anyhow::ensure!(b.len() == 32 * (1 + nodes + count), "tree length");
    let out = Path::new(&a[2]);
    std::fs::create_dir_all(out)?;
    let mut parent = vec![false; nodes];
    for n in 0..nodes {
        let p = u32at(&b, 32 + n * 32 + 20);
        if p != u32::MAX {
            parent[p as usize] = true;
        }
    }
    let mut rows = Vec::new();
    for stride in [32usize, 20, 16] {
        let begin = Instant::now();
        let mut packed = Vec::new();
        let mut boxes = Vec::new();
        for n in 0..nodes {
            let nb = 32 + n * 32;
            let start = u32at(&b, nb + 24) as usize;
            let len = u32at(&b, nb + 28) as usize;
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            for i in start..start + len {
                for k in 0..3 {
                    let v = f32at(&b, 32 * (1 + nodes + i) + 4 * k);
                    lo[k] = lo[k].min(v);
                    hi[k] = hi[k].max(v);
                }
            }
            boxes.push((start, len, lo, hi));
            for i in start..start + len {
                let r = &b[32 * (1 + nodes + i)..32 * (2 + nodes + i)];
                if stride == 32 {
                    packed.extend_from_slice(r);
                    continue;
                }
                for k in 0..3 {
                    let t = if hi[k] > lo[k] { (f32at(r, 4 * k) - lo[k]) / (hi[k] - lo[k]) } else { 0. };
                    packed.extend(((t * 65535.).round() as u16).to_le_bytes());
                }
                for k in 0..3 {
                    let s = f32at(r, 12 + k * 4).ln();
                    if stride == 20 {
                        packed.extend(half::f16::from_f32(s).to_le_bytes());
                    } else {
                        packed.push(((s + 10.) * 16.).round().clamp(0., 255.) as u8);
                    }
                }
                let q = [r[29], r[30], r[31], r[28]].map(|v| (v as f32 - 128.) / 128.);
                let bits = export::pack_rotation(q);
                if stride == 20 {
                    packed.extend(bits.to_le_bytes());
                } else {
                    let mut small = bits >> 30;
                    for shift in [20, 10, 0] {
                        let v = (bits >> shift) & 1023;
                        let mag = ((v & 511) as f32 * 63. / 511.).round() as u32;
                        small = (small << 7) | ((v >> 9) << 6) | mag;
                    }
                    packed.extend_from_slice(&small.to_le_bytes()[..3]);
                }
                packed.extend_from_slice(&r[24..28]);
            }
        }
        let encode_ms = begin.elapsed().as_secs_f64() * 1000.;
        let begin = Instant::now();
        let mut g = Gaussians::default();
        let mut max_pos = 0f32;
        let mut max_scale = 0f32;
        for (n, &(start, len, lo, hi)) in boxes.iter().enumerate() {
            for i in start..start + len {
                let r = &packed[i * stride..(i + 1) * stride];
                let orig = &b[32 * (1 + nodes + i)..32 * (2 + nodes + i)];
                let (p, s, q, c) = if stride == 32 {
                    (
                        [f32at(r, 0), f32at(r, 4), f32at(r, 8)],
                        [f32at(r, 12).ln(), f32at(r, 16).ln(), f32at(r, 20).ln()],
                        [r[29], r[30], r[31], r[28]].map(|v| (v as f32 - 128.) / 128.),
                        &r[24..28],
                    )
                } else {
                    let p = std::array::from_fn(|k| lo[k] + u16::from_le_bytes(r[2 * k..2 * k + 2].try_into().unwrap()) as f32 / 65535. * (hi[k] - lo[k]));
                    let s = std::array::from_fn(|k| {
                        if stride == 20 { half::f16::from_le_bytes(r[6 + 2 * k..8 + 2 * k].try_into().unwrap()).to_f32() } else { r[6 + k] as f32 / 16. - 10. }
                    });
                    let bits = if stride == 20 {
                        u32at(r, 12)
                    } else {
                        let small = u32::from_le_bytes([r[9], r[10], r[11], 0]);
                        let mut bits = small >> 21;
                        for sh in [14, 7, 0] {
                            let v = (small >> sh) & 127;
                            bits = (bits << 10) | ((v >> 6) << 9) | ((v & 63) as f32 * 511. / 63.).round() as u32;
                        }
                        bits
                    };
                    (p, s, export::unpack_rotation(bits), &r[stride - 4..])
                };
                for k in 0..3 {
                    max_pos = max_pos.max((p[k] - f32at(orig, k * 4)).abs());
                    max_scale = max_scale.max((s[k].exp() / f32at(orig, 12 + k * 4) - 1.).abs());
                }
                if !parent[n] {
                    g.push([p[0], p[1], -p[2]], s, [q[3], -q[0], -q[1], q[2]], logit(c[3] as f32 / 255.), [c[0], c[1], c[2]].map(|x| x as f32 / 255.));
                }
            }
        }
        let decode_ms = begin.elapsed().as_secs_f64() * 1000.;
        std::fs::write(out.join(format!("records{stride}.bin")), &packed)?;
        ply::write(&out.join(format!("leaves{stride}.ply")), &g)?;
        // The lab sidecar stores exact per-node bounds, separately from existing traversal spheres.
        std::fs::write(out.join("bounds.json"), serde_json::to_vec(&boxes)?)?;
        rows.push(serde_json::json!({"stride":stride,"records":count,"nodes":nodes,"leaves":g.len(),"bytes":packed.len(),"bounds_bytes":nodes*24,"encode_ms":encode_ms,"decode_ms":decode_ms,"max_position_axis_m":max_pos,"max_scale_relative":max_scale}));
    }
    let j = serde_json::to_string_pretty(&rows)?;
    println!("{j}");
    std::fs::write(out.join("compact.json"), j)?;
    Ok(())
}
