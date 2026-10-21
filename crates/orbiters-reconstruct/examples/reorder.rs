//! Lab: rewrites a splat PLY in another order, to measure how memory order affects the kernels (kbench --ply).
//!
//! ```text
//! cargo run --release --example reorder -- <in.ply> <out.ply> morton|shuffle
//! ```

use anyhow::{bail, Context as _};
use orbiters_reconstruct::gaussians::Gaussians;
use orbiters_reconstruct::ply;

/// Spreads the low 10 bits of `v` to every third bit.
fn spread(v: u32) -> u32 {
    let mut x = v & 0x3ff;
    x = (x | (x << 16)) & 0x0300_00ff;
    x = (x | (x << 8)) & 0x0300_f00f;
    x = (x | (x << 4)) & 0x030c_30c3;
    (x | (x << 2)) & 0x0924_9249
}

/// Morton codes of the centres in their 1st–99th percentile box (the far sky shell clamps to its faces).
pub fn morton_codes(g: &Gaussians) -> Vec<u32> {
    let mut lo = [0f32; 3];
    let mut hi = [0f32; 3];
    for k in 0..3 {
        let mut v: Vec<f32> = g.means.iter().map(|m| m[k]).collect();
        v.sort_by(f32::total_cmp);
        lo[k] = v[v.len() / 100];
        hi[k] = v[v.len() * 99 / 100].max(lo[k] + 1e-3);
    }
    g.means
        .iter()
        .map(|m| {
            let q = |k: usize| (((m[k] - lo[k]) / (hi[k] - lo[k])).clamp(0.0, 1.0) * 1023.0) as u32;
            spread(q(0)) | (spread(q(1)) << 1) | (spread(q(2)) << 2)
        })
        .collect()
}

fn permute(g: &Gaussians, order: &[usize]) -> Gaussians {
    let mut out = Gaussians::default();
    for &i in order {
        out.push(g.means[i], g.log_scales[i], g.rotations[i], g.opacity_logits[i], g.colors[i]);
    }
    out
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (Some(input), Some(output), Some(mode)) = (a.first(), a.get(1), a.get(2)) else { bail!("usage: reorder <in.ply> <out.ply> morton|shuffle") };
    let g = ply::read(std::path::Path::new(input)).context("reading")?;
    let mut order: Vec<usize> = (0..g.len()).collect();
    match mode.as_str() {
        "morton" => {
            let codes = morton_codes(&g);
            order.sort_by_key(|&i| codes[i]);
        }
        "shuffle" => fastrand::Rng::with_seed(3).shuffle(&mut order),
        other => bail!("unknown order {other}"),
    }
    ply::write(std::path::Path::new(output), &permute(&g, &order))?;
    println!("{} splats written in {mode} order", g.len());
    Ok(())
}
