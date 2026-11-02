//! Degree-zero interchange exports. Both files use right-handed, Y-up coordinates (Unity Z negated).
//! The trained values are display sRGB, blended in that space. Sky discs and the separate sky map are omitted.
//! SPZ v4 uses five independent degree-zero Zstandard streams; no draft glTF compression extension is advertised.
use crate::{
    gaussians::{Gaussians, sigmoid},
    ply,
};
use anyhow::{Context, ensure};
use serde_json::json;
use std::path::Path;

fn indices(g: &Gaussians) -> anyhow::Result<Vec<usize>> {
    ensure!(
        g.means.len() == g.rotations.len() && g.means.len() == g.log_scales.len() && g.means.len() == g.colors.len() && g.means.len() == g.opacity_logits.len(),
        "inconsistent Gaussian arrays"
    );
    let ids: Vec<_> = (0..g.len()).filter(|&i| !g.sky.get(i).copied().unwrap_or(false)).collect();
    ensure!(!ids.is_empty() && ids.len() <= u32::MAX as usize, "export requires 1..2^32-1 non-sky splats");
    for &i in &ids {
        ensure!(
            g.means[i]
                .iter()
                .chain(&g.rotations[i])
                .chain(&g.log_scales[i])
                .chain(&g.colors[i])
                .chain(std::iter::once(&g.opacity_logits[i]))
                .all(|v| v.is_finite()),
            "nonfinite splat {i}"
        );
        ensure!(g.rotations[i].iter().map(|x| x * x).sum::<f32>().is_finite() && g.rotations[i].iter().any(|x| x.abs() > 1e-12), "invalid rotation {i}");
        ensure!(g.colors[i].iter().all(|c| ((c - 0.5) / ply::SH_C0).is_finite()), "SH0 overflow at splat {i}");
        ensure!(g.log_scales[i].iter().all(|x| x.exp().is_finite() && x.exp() > 0.0), "invalid scale {i}");
    }
    Ok(ids)
}
fn rotation(g: &Gaussians, i: usize) -> [f32; 4] {
    let [w, x, y, z] = g.rotations[i];
    let n = (w * w + x * x + y * y + z * z).sqrt();
    [-x / n, -y / n, z / n, w / n]
}
pub fn gltf(g: &Gaussians) -> anyhow::Result<Vec<u8>> {
    let ids = indices(g)?;
    let mut bin = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut attrs = serde_json::Map::new();
    for (a, name, width) in [
        (0, "POSITION", 3),
        (1, "KHR_gaussian_splatting:ROTATION", 4),
        (2, "KHR_gaussian_splatting:SCALE", 3),
        (3, "KHR_gaussian_splatting:OPACITY", 1),
        (4, "KHR_gaussian_splatting:SH_DEGREE_0_COEF_0", 3),
    ] {
        let offset = bin.len();
        let mut min = vec![f32::INFINITY; width];
        let mut max = vec![f32::NEG_INFINITY; width];
        for &i in &ids {
            let values: Vec<f32> = match a {
                0 => vec![g.means[i][0], g.means[i][1], -g.means[i][2]],
                1 => rotation(g, i).to_vec(),
                2 => g.log_scales[i].map(f32::exp).to_vec(),
                3 => vec![sigmoid(g.opacity_logits[i])],
                _ => g.colors[i].map(|c| (c - 0.5) / ply::SH_C0).to_vec(),
            };
            for (k, v) in values.into_iter().enumerate() {
                min[k] = min[k].min(v);
                max[k] = max[k].max(v);
                bin.extend(v.to_le_bytes());
            }
        }
        views.push(json!({"buffer":0,"byteOffset":offset,"byteLength":bin.len()-offset,"target":34962}));
        let mut acc = json!({"bufferView":a,"componentType":5126,"count":ids.len(),"type":match width {1=>"SCALAR",3=>"VEC3",_=>"VEC4"}});
        if a == 0 {
            acc["min"] = json!(min);
            acc["max"] = json!(max);
        }
        accessors.push(acc);
        attrs.insert(name.into(), json!(a));
    }
    let doc = json!({"asset":{"version":"2.0","generator":"Orbiters - N4N0"},"extensionsUsed":["KHR_gaussian_splatting"],"extensionsRequired":["KHR_gaussian_splatting"],"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],"meshes":[{"primitives":[{"mode":0,"attributes":attrs,"extensions":{"KHR_gaussian_splatting":{"kernel":"ellipse","colorSpace":"srgb_rec709_display","projection":"perspective","sortingMethod":"cameraDistance"}}}]}],"buffers":[{"byteLength":bin.len()}],"bufferViews":views,"accessors":accessors});
    let mut json = serde_json::to_vec(&doc)?;
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    let len = 28usize.checked_add(json.len()).and_then(|x| x.checked_add(bin.len())).context("GLB too large")?;
    ensure!(len <= u32::MAX as usize, "GLB exceeds 4 GB");
    let mut out = Vec::with_capacity(len);
    for v in [0x46546c67, 2, len as u32, json.len() as u32, 0x4e4f534a] {
        out.extend(v.to_le_bytes());
    }
    out.extend(json);
    out.extend((bin.len() as u32).to_le_bytes());
    out.extend(0x004e4942u32.to_le_bytes());
    out.extend(bin);
    Ok(out)
}
/// SPZ's sign/magnitude smallest-three encoding, in xyzw order. The omitted component is positive.
pub fn pack_rotation(q: [f32; 4]) -> u32 {
    let n = q.iter().map(|x| x * x).sum::<f32>().sqrt();
    let q = q.map(|v| v / n);
    let largest = (0..4).max_by(|&a, &b| q[a].abs().total_cmp(&q[b].abs())).unwrap();
    let mut bits = largest as u32;
    for i in 0..4 {
        if i != largest {
            let sign = ((q[i] < 0.0) ^ (q[largest] < 0.0)) as u32;
            let mag = (q[i].abs() * std::f32::consts::SQRT_2 * 511.0).round() as u32;
            bits = (bits << 10) | (sign << 9) | mag.min(511);
        }
    }
    bits
}
pub fn unpack_rotation(bits: u32) -> [f32; 4] {
    let largest = (bits >> 30) as usize;
    let mut q = [0.; 4];
    let mut shift = 20;
    for i in 0..4 {
        if i != largest {
            let v = (bits >> shift) & 1023;
            q[i] = (v & 511) as f32 / (511.0 * std::f32::consts::SQRT_2) * if v & 512 != 0 { -1.0 } else { 1.0 };
            shift -= 10;
        }
    }
    q[largest] = (1.0 - q.iter().map(|x| x * x).sum::<f32>()).max(0.).sqrt();
    q
}
fn byte(x: f32) -> u8 {
    x.round().clamp(0., 255.) as u8
}
pub fn spz(g: &Gaussians) -> anyhow::Result<Vec<u8>> {
    let ids = indices(g)?;
    let mut streams: [Vec<u8>; 5] = Default::default();
    let (mut clipped_scales, mut clipped_colors) = (0usize, 0usize);
    for &i in &ids {
        for p in [g.means[i][0], g.means[i][1], -g.means[i][2]] {
            let v = (p * 4096.).round();
            ensure!((-8388608.0..=8388607.0).contains(&v), "SPZ position outside signed 24-bit range at splat {i}");
            streams[0].extend_from_slice(&(v as i32).to_le_bytes()[..3]);
        }
        streams[1].push(byte(sigmoid(g.opacity_logits[i]) * 255.));
        for c in g.colors[i] {
            let encoded = (c - 0.5) / ply::SH_C0 * (0.15 * 255.) + 127.5;
            clipped_colors += usize::from(!(0.0..=255.0).contains(&encoded));
            streams[2].push(byte(encoded));
        }
        for s in g.log_scales[i] {
            clipped_scales += usize::from(!(-10.0..=5.9375).contains(&s));
            streams[3].push(byte((s + 10.) * 16.));
        }
        streams[4].extend(pack_rotation(rotation(g, i)).to_le_bytes());
    }
    if clipped_scales > 0 || clipped_colors > 0 {
        eprintln!(
            "SPZ range saturation: {clipped_scales} scale components and {clipped_colors} colour components out of {} each; GLB preserves their full range",
            ids.len() * 3
        );
    }
    let mut out = Vec::new();
    out.extend(b"NGSP");
    out.extend(4u32.to_le_bytes());
    out.extend((ids.len() as u32).to_le_bytes());
    out.extend([0, 12, 0, 5]);
    out.extend(32u32.to_le_bytes());
    out.extend([0; 12]);
    out.resize(112, 0);
    // Compress one stream at a time: this bounds compressor workspace to one stream.
    for (i, s) in streams.iter().enumerate() {
        let c = zstd::bulk::compress(s, 3)?;
        out[32 + i * 16..40 + i * 16].copy_from_slice(&(c.len() as u64).to_le_bytes());
        out[40 + i * 16..48 + i * 16].copy_from_slice(&(s.len() as u64).to_le_bytes());
        out.extend(c);
    }
    Ok(out)
}
fn cli(args: &[String], encode: fn(&Gaussians) -> anyhow::Result<Vec<u8>>) -> anyhow::Result<()> {
    ensure!(args.len() == 2, "usage: --export-gltf <splats.ply> <out.glb> or --export-spz <splats.ply> <out.spz>");
    let g = ply::read(Path::new(&args[0]))?;
    let bytes = encode(&g)?;
    // Encode and validate before opening the destination. No GPU or sky image is loaded.
    let out = Path::new(&args[1]);
    if out.exists() {
        ensure!(std::fs::canonicalize(out)? != std::fs::canonicalize(&args[0])?, "input and output must differ");
    }
    let tmp = out.with_extension(format!("{}.part", out.extension().and_then(|x| x.to_str()).unwrap_or("export")));
    if tmp.exists() {
        ensure!(std::fs::canonicalize(&tmp)? != std::fs::canonicalize(&args[0])?, "input and temporary output must differ");
    }
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, out)?;
    Ok(())
}
pub fn gltf_cli(args: &[String]) -> anyhow::Result<()> {
    cli(args, gltf)
}
pub fn spz_cli(args: &[String]) -> anyhow::Result<()> {
    cli(args, spz)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Gaussians {
        let mut g = Gaussians::default();
        g.push([1., 2., 3.], [-2., -3., -4.], [0.9, 0.1, -0.2, 0.3], 0.4, [0.2, 0.5, 0.8]);
        g
    }
    #[test]
    fn glb_attributes_roundtrip() {
        let g = sample();
        let b = gltf(&g).unwrap();
        let n = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
        let j: serde_json::Value = serde_json::from_slice(&b[20..20 + n]).unwrap();
        assert_eq!(j["accessors"][0]["count"], 1);
        assert_eq!(j["meshes"][0]["primitives"][0]["extensions"]["KHR_gaussian_splatting"]["colorSpace"], "srgb_rec709_display");
        let f: Vec<_> = b[28 + n..].chunks_exact(4).map(|x| f32::from_le_bytes(x.try_into().unwrap())).collect();
        assert_eq!(&f[..3], &[1., 2., -3.]);
        for k in 0..3 {
            assert!((f[7 + k].ln() - g.log_scales[0][k]).abs() < 1e-6);
            assert!((f[11 + k] * ply::SH_C0 + 0.5 - g.colors[0][k]).abs() < 1e-6);
        }
    }
    #[test]
    fn spz_streams_roundtrip() {
        let g = sample();
        let b = spz(&g).unwrap();
        let mut pos = 112;
        let mut s = Vec::new();
        for i in 0..5 {
            let n = u64::from_le_bytes(b[32 + i * 16..40 + i * 16].try_into().unwrap()) as usize;
            let size = u64::from_le_bytes(b[40 + i * 16..48 + i * 16].try_into().unwrap()) as usize;
            s.push(zstd::bulk::decompress(&b[pos..pos + n], size).unwrap());
            pos += n;
        }
        assert_eq!(pos, b.len());
        assert_eq!(s.iter().map(Vec::len).collect::<Vec<_>>(), [9, 1, 3, 3, 4]);
        for k in 0..3 {
            let p = &s[0][k * 3..k * 3 + 3];
            let v = ((p[0] as i32) | ((p[1] as i32) << 8) | ((p[2] as i32) << 16)) << 8 >> 8;
            assert!((v as f32 / 4096. - [1., 2., -3.][k]).abs() < 0.00013);
            assert!((s[3][k] as f32 / 16. - 10. - g.log_scales[0][k]).abs() <= 0.03125);
            assert!(((s[2][k] as f32 / 255. - 0.5) / 0.15 * ply::SH_C0 + 0.5 - g.colors[0][k]).abs() < 0.004);
        }
        let q = unpack_rotation(u32::from_le_bytes(s[4][..].try_into().unwrap()));
        assert!(q.iter().zip(rotation(&g, 0)).map(|(a, b)| a * b).sum::<f32>().abs() > 0.99999);
    }
    #[test]
    fn rotations_cover_all_omitted_components_and_signs() {
        let mut rng = fastrand::Rng::with_seed(72);
        for _ in 0..10000 {
            let q = std::array::from_fn(|_| rng.f32() * 2.0 - 1.0);
            let n = q.iter().map(|v| v * v).sum::<f32>().sqrt();
            let decoded = unpack_rotation(pack_rotation(q));
            let dot = q.iter().zip(decoded).map(|(a, b)| a * b / n).sum::<f32>().abs();
            assert!(dot > 0.99999, "{dot}");
        }
    }
    #[test]
    fn rejects_invalid_and_omits_sky() {
        let mut g = sample();
        g.sky[0] = true;
        assert!(gltf(&g).is_err());
        g.sky[0] = false;
        g.means[0][0] = f32::NAN;
        assert!(gltf(&g).is_err());
        g.means[0][0] = 2048.;
        assert!(spz(&g).is_err());
    }
    #[test]
    fn temporary_output_cannot_overwrite_input() {
        let dir = std::env::temp_dir().join(format!("orbiters-export-{}-{}", std::process::id(), fastrand::u64(..)));
        std::fs::create_dir(&dir).unwrap();
        let input = dir.join("model.glb.part");
        let output = dir.join("model.glb");
        ply::write(&input, &sample()).unwrap();
        let before = std::fs::read(&input).unwrap();
        let args = [input.to_string_lossy().into_owned(), output.to_string_lossy().into_owned()];
        let result = gltf_cli(&args);
        assert!(result.unwrap_err().to_string().contains("input and temporary output must differ"));
        assert_eq!(std::fs::read(&input).unwrap(), before);
        assert!(!output.exists());
        std::fs::remove_file(input).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
