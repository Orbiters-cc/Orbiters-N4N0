//! Standard exports of a reconstruction, for other tools: glTF 2.0 binary (GLB) with the ratified
//! `KHR_gaussian_splatting` extension (three.js, Spark, Cesium, glTF importers), Niantic's SPZ (Spark, PlayCanvas'
//! SuperSplat and splat-transform, Unity and VRChat splat importers) in version 3, which every SPZ reader opens (the
//! desktop's Export), and version 4 (the command line), and the reference 3DGS PLY layout alone (every splat tool).
//! Degree-zero colour only: what the trainer learns.
//!
//! Each file is in its own format's frame, so that a capture sits in every reader the way it was taken ([`Frame`]):
//! glTF's is Unity's world with X negated, SPZ's is Unity's world with Z negated (the PLY's frame); the two differ by
//! half a turn about Y. Writing the GLB in the PLY's frame, as this exporter first did, brought a capture back into
//! Unity (glTFast, UnityGLTF) turned 180° about Y, the splats ahead of the capture behind it, and showed its back in
//! Blender's front view: measured with an asymmetric scene through three.js 0.187's loader and Niantic's SPZ reader,
//! which converts our SPZ to its Unity frame (RUF) exactly and to its glTF frame (LUF) exactly onto the GLB now.
//!
//! Colours are the trainer's display sRGB, blended in that space (`srgb_rec709_display`). They are not bounded: N4N0's
//! rasteriser blends whatever the optimiser learns, and many splats darken what lies behind them with a negative
//! channel (of the non-sky splats: Home 31%, 25% by opacity; Plaza 11%; Needle 17%). `KHR_gaussian_splatting` clamps
//! each splat's colour at 0 before blending, three.js' RGBA8 at [0, 1], so those tools brighten such places whatever
//! the file says. Rendered by N4N0 with the same clamps, held-out PSNR falls 0.27 dB on Home, 0.36 on Plaza and 0.15 on
//! Needle at 0, and 0.65, 0.44 and 0.30 dB at [0, 1] (the clamp the VRChat tree's and web preview's colour bytes apply
//! too). The GLB keeps the learned floats anyway: a clamp written into the file changes nothing in a conforming
//! renderer and only loses range in a float one; matching needs a trainer that learns colours the standard can draw.
//! The sky shell's discs and the separate sky map are left out: other tools draw no sky map.
use crate::{
    gaussians::{Gaussians, sigmoid},
    ply,
};
use anyhow::{Context, ensure};
use orbiters_i18n::tr;
use serde_json::json;
use std::path::Path;

/// A right-handed frame reached from Unity's world (left-handed, +X right, +Y up, +Z forward) by negating one axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Frame {
    /// glTF's: right-handed, +Y up, an asset's front facing +Z (glTF 2.0 § 3.4; LUF in the SPZ library's terms). Unity's
    /// glTF importers (glTFast, UnityGLTF) convert from it by negating X, so a GLB in it lands in a Unity world where
    /// it was captured; Blender's importer turns glTF's +Z to its −Y, the front view.
    Gltf,
    /// SPZ's stored frame RUB (+X right, +Y up, +Z back: three.js' and OpenGL's), the PLY's and web `.splat`'s frame.
    /// The SPZ library converts it to Unity's (its RUF) by negating Z.
    Rub,
}

impl Frame {
    pub fn position(self, [x, y, z]: [f32; 3]) -> [f32; 3] {
        match self {
            Frame::Gltf => [-x, y, z],
            Frame::Rub => [x, y, -z],
        }
    }

    /// The rotation (w, x, y, z), a Unity-world R, seen through this frame's mirror S: S R S. A quaternion's vector
    /// part is an axis (a pseudovector), so it becomes −S v: (w, x, −y, −z) through X's mirror, (w, −x, −y, z) through
    /// Z's. Normalised, in glTF's and SPZ's (x, y, z, w) order.
    pub fn rotation(self, q: [f32; 4]) -> [f32; 4] {
        let n = q.iter().map(|c| c * c).sum::<f32>().sqrt();
        let [w, x, y, z] = q.map(|c| c / n);
        match self {
            Frame::Gltf => [x, -y, -z, w],
            Frame::Rub => [-x, -y, z, w],
        }
    }
}

/// The splats an export holds: every one but the sky shell's, each checked to be a finite Gaussian with a rotation,
/// positive linear scales and its 3D filter fused (a PLY's are: crate::gaussians::Gaussians::fuse_filter), so that a
/// file never carries a value its readers would choke on.
fn exported(g: &Gaussians) -> anyhow::Result<Vec<usize>> {
    let n = g.len();
    ensure!([g.rotations.len(), g.log_scales.len(), g.colors.len(), g.opacity_logits.len()].iter().all(|&l| l == n), "{}", tr("inconsistent Gaussian arrays"));
    let ids: Vec<_> = (0..n).filter(|&i| !g.sky.get(i).copied().unwrap_or(false)).collect();
    ensure!(!ids.is_empty() && ids.len() <= u32::MAX as usize, "{}", tr("export requires 1..2^32-1 non-sky splats"));
    for &i in &ids {
        let values = g.means[i].iter().chain(&g.rotations[i]).chain(&g.log_scales[i]).chain(&g.colors[i]).chain([&g.opacity_logits[i]]);
        ensure!(values.copied().all(f32::is_finite), "{}", tr!("nonfinite splat {index}", index = i));
        ensure!(g.rotations[i].iter().map(|x| x * x).sum::<f32>().is_finite() && g.rotations[i].iter().any(|x| x.abs() > 1e-12), "{}", tr!("invalid rotation {index}", index = i));
        ensure!(g.colors[i].iter().all(|c| ((c - 0.5) / ply::SH_C0).is_finite()), "{}", tr!("SH0 overflow at splat {index}", index = i));
        ensure!(g.log_scales[i].iter().all(|x| x.exp().is_finite() && x.exp() > 0.0), "{}", tr!("invalid scale {index}", index = i));
        ensure!(g.filter(i) == 0.0, "{}", tr!("splat {index} has an unfused 3D filter", index = i));
    }
    Ok(ids)
}

/// The degree-zero spherical harmonic of a display colour (colour = 0.5 + C0 · f_dc).
fn sh0(c: [f32; 3]) -> [f32; 3] {
    c.map(|c| (c - 0.5) / ply::SH_C0)
}

/// The splats as a GLB in glTF's frame: one POINTS primitive with float `POSITION` and the extension's `ROTATION`
/// (unit, x y z w), `SCALE` (linear), `OPACITY` (sigmoid) and `SH_DEGREE_0_COEF_0`, the ellipse kernel with
/// perspective projection and camera-distance sorting (the defaults, written out). Every value keeps its float range:
/// this is the lossless export. The extension is required, so a loader without it refuses the file instead of drawing
/// a million uncoloured points (no `COLOR_0` fallback is written).
pub fn gltf(g: &Gaussians) -> anyhow::Result<Vec<u8>> {
    let ids = exported(g)?;
    let mut bin = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut attributes = serde_json::Map::new();
    let semantics = [
        ("POSITION", 3),
        ("KHR_gaussian_splatting:ROTATION", 4),
        ("KHR_gaussian_splatting:SCALE", 3),
        ("KHR_gaussian_splatting:OPACITY", 1),
        ("KHR_gaussian_splatting:SH_DEGREE_0_COEF_0", 3),
    ];
    for (a, (name, width)) in semantics.into_iter().enumerate() {
        let offset = bin.len();
        let mut min = vec![f32::INFINITY; width];
        let mut max = vec![f32::NEG_INFINITY; width];
        bin.reserve(ids.len() * width * 4);
        for &i in &ids {
            // A fixed array a splat, of which the attribute's first `width` values (RECONSTRUCT-14l: no vector each).
            let mut values = [0.0f32; 4];
            match a {
                0 => values[..3].copy_from_slice(&Frame::Gltf.position(g.means[i])),
                1 => values.copy_from_slice(&Frame::Gltf.rotation(g.rotations[i])),
                2 => values[..3].copy_from_slice(&g.log_scales[i].map(f32::exp)),
                3 => values[0] = sigmoid(g.opacity_logits[i]),
                _ => values[..3].copy_from_slice(&sh0(g.colors[i])),
            }
            for (k, &v) in values[..width].iter().enumerate() {
                min[k] = min[k].min(v);
                max[k] = max[k].max(v);
                bin.extend(v.to_le_bytes());
            }
        }
        views.push(json!({"buffer": 0, "byteOffset": offset, "byteLength": bin.len() - offset, "target": 34962}));
        let kind = match width {
            1 => "SCALAR",
            3 => "VEC3",
            _ => "VEC4",
        };
        let mut accessor = json!({"bufferView": a, "componentType": 5126, "count": ids.len(), "type": kind});
        // glTF requires POSITION's bounds.
        if a == 0 {
            accessor["min"] = json!(min);
            accessor["max"] = json!(max);
        }
        accessors.push(accessor);
        attributes.insert(name.into(), json!(a));
    }
    let splatting = json!({"kernel": "ellipse", "colorSpace": "srgb_rec709_display", "projection": "perspective", "sortingMethod": "cameraDistance"});
    let doc = json!({
        "asset": {"version": "2.0", "generator": "Orbiters - N4N0"},
        "extensionsUsed": ["KHR_gaussian_splatting"],
        "extensionsRequired": ["KHR_gaussian_splatting"],
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{"primitives": [{"mode": 0, "attributes": attributes, "extensions": {"KHR_gaussian_splatting": splatting}}]}],
        "buffers": [{"byteLength": bin.len()}],
        "bufferViews": views,
        "accessors": accessors,
    });
    let mut json = serde_json::to_vec(&doc)?;
    // Chunks are 4-byte aligned: JSON pads with spaces; the binary chunk is all floats already.
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let len = 28usize.checked_add(json.len()).and_then(|x| x.checked_add(bin.len())).with_context(|| tr("GLB too large"))?;
    ensure!(len <= u32::MAX as usize, "{}", tr("GLB exceeds 4 GB"));
    let mut out = Vec::with_capacity(len);
    // Header (magic "glTF", version 2, length), then the JSON chunk's length and type "JSON".
    for v in [0x4654_6c67, 2, len as u32, json.len() as u32, 0x4e4f_534a] {
        out.extend(v.to_le_bytes());
    }
    out.extend(json);
    out.extend((bin.len() as u32).to_le_bytes());
    out.extend(0x004e_4942u32.to_le_bytes());
    out.extend(bin);
    Ok(out)
}

/// SPZ's smallest-three quaternion (x y z w order): the index of the largest component in the top 2 bits, then the
/// other three as a sign bit and 9 bits of magnitude over [0, 1/√2], the omitted one made positive (q and −q are the
/// same rotation). The reference encoder's rounding (load-spz.cc packQuaternionSmallestThree).
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

/// The quaternion (x y z w) of [`pack_rotation`]'s bits; the omitted component is what makes it unit.
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

/// SPZ header flag: the splats were trained with Mip-Splatting's filters, so a reader applies its 2D filter's opacity
/// compensation (SPZ README, "antialiased"). N4N0's are (crate::cpu::FILTER_2D with compensation c2, the 3D filter
/// fused into the PLY); without the flag a reader draws faint sub-pixel splats, far detail mostly, up to tens of times
/// too strong (a 0.01 px² splat under a 0.3 px² dilation keeps its full opacity instead of 1/31 of it).
const SPZ_ANTIALIASED: u8 = 0x1;

/// How many values SPZ's 8-bit quantisation saturated, of `components` each (three a splat): log scales outside
/// [−10, 5.94] (splats thinner than 45 µm, which only get wider) and degree-zero colours outside about [−0.44, 1.44]
/// (learned colours brighter or darker than SPZ stores: the module doc). [`gltf`] and [`ply`] keep them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Saturated {
    pub scales: usize,
    pub colors: usize,
    pub components: usize,
}

/// The exported splats as SPZ's five attribute streams in RUB, with the reference encoder's quantisation
/// (load-spz.cc packGaussians): positions (signed 24-bit fixed point, 12 fractional bits), opacities (sigmoid · 255),
/// colours (f_dc · 0.15 · 255 + 127.5), log scales ((s + 10) · 16) and rotations ([`pack_rotation`]); degree zero has no
/// SH stream. Versions 3 and 4 hold these same bytes ([`spz3`], [`spz4`]). Positions past ±2048 m are refused (the
/// reference wraps them silently; N4N0's worlds reach about 210 m once the sky shell is left out). Colours outside
/// [−0.44, 1.44] and log scales outside [−10, 5.94] saturate, as in the reference ([`Saturated`]).
fn spz_streams(g: &Gaussians) -> anyhow::Result<(u32, [Vec<u8>; 5], Saturated)> {
    let ids = exported(g)?;
    let mut streams: [Vec<u8>; 5] = Default::default();
    let mut saturated = Saturated { components: ids.len() * 3, ..Default::default() };
    for &i in &ids {
        for p in Frame::Rub.position(g.means[i]) {
            let v = (p * 4096.).round();
            ensure!((-8388608.0..=8388607.0).contains(&v), "{}", tr!("SPZ position outside signed 24-bit range at splat {index}", index = i));
            streams[0].extend_from_slice(&(v as i32).to_le_bytes()[..3]);
        }
        streams[1].push(byte(sigmoid(g.opacity_logits[i]) * 255.));
        for c in sh0(g.colors[i]) {
            let encoded = c * (0.15 * 255.) + 127.5;
            saturated.colors += usize::from(!(0.0..=255.0).contains(&encoded));
            streams[2].push(byte(encoded));
        }
        for s in g.log_scales[i] {
            saturated.scales += usize::from(!(-10.0..=5.9375).contains(&s));
            streams[3].push(byte((s + 10.) * 16.));
        }
        streams[4].extend(pack_rotation(Frame::Rub.rotation(g.rotations[i])).to_le_bytes());
    }
    Ok((ids.len() as u32, streams, saturated))
}

/// The splats as SPZ version 4 in RUB, the reference encoder's layout (load-spz.cc saveSpz): a 32-byte plain header
/// (magic NGSP, version, count, SH degree 0, 12 fractional position bits, flags, five streams, the table of contents at
/// byte 32), 16 bytes of table per stream (compressed and uncompressed size), then [`spz_streams`]' five streams, each
/// its own Zstandard stream. Lossy: [`gltf`] and [`ply`] keep the full range. Version 4 (May 2026) needs an updated
/// reader; [`spz3`] opens in every one.
pub fn spz4(g: &Gaussians) -> anyhow::Result<(Vec<u8>, Saturated)> {
    let (count, streams, saturated) = spz_streams(g)?;
    let mut out = Vec::new();
    out.extend(b"NGSP");
    out.extend(4u32.to_le_bytes());
    out.extend(count.to_le_bytes());
    out.extend([0, 12, SPZ_ANTIALIASED, 5]);
    out.extend(32u32.to_le_bytes());
    out.extend([0; 12]);
    out.resize(32 + 5 * 16, 0);
    // Compress one stream at a time: this bounds compressor workspace to one stream.
    for (i, s) in streams.iter().enumerate() {
        let c = zstd::bulk::compress(s, 3)?;
        out[32 + i * 16..40 + i * 16].copy_from_slice(&(c.len() as u64).to_le_bytes());
        out[40 + i * 16..48 + i * 16].copy_from_slice(&(s.len() as u64).to_le_bytes());
        out.extend(c);
    }
    Ok((out, saturated))
}

/// [`spz4`], what the command line writes, saying what saturated.
pub fn spz(g: &Gaussians) -> anyhow::Result<Vec<u8>> {
    let (bytes, s) = spz4(g)?;
    if s.scales > 0 || s.colors > 0 {
        eprintln!(
            "SPZ range saturation: {} scale components and {} colour components out of {} each; GLB preserves their full range",
            s.scales, s.colors, s.components
        );
    }
    Ok(bytes)
}

/// The splats as SPZ version 3, what the desktop's Export writes: the reference's legacy container (load-spz.cc
/// serializePackedGaussians, compressGzipped), one gzip stream of a 16-byte header (magic NGSP, version 3, count, SH
/// degree 0, 12 fractional position bits, flags, a reserved zero) and [`spz_streams`]' five streams one after the other,
/// the bytes [`spz4`] compresses one by one. Every SPZ reader opens version 3 (Niantic's library reads 1 to 3 through
/// its legacy path), while version 4, out since May 2026, opens only in readers built on the newest library ("Old
/// loaders cannot read SPZ 4 files", Niantic): a VRChat splat importer or web viewer made before it refuses the file.
/// Rotations are the smallest three since version 3, so nothing is lost against version 4, and the single gzip stream
/// costs little: Home's 731,087 splats take 12.89 MB, 0.45% more than version 4's 12.83 MB. `progress` hears how much is
/// compressed (0 to 1) and stops the export with an error.
pub fn spz3(g: &Gaussians, progress: &mut dyn FnMut(f32) -> anyhow::Result<()>) -> anyhow::Result<(Vec<u8>, Saturated)> {
    use std::io::Write as _;
    let (count, streams, saturated) = spz_streams(g)?;
    let mut header = Vec::with_capacity(16);
    header.extend(b"NGSP");
    header.extend(3u32.to_le_bytes());
    header.extend(count.to_le_bytes());
    header.extend([0, 12, SPZ_ANTIALIASED, 0]);
    let total = 16 + streams.iter().map(Vec::len).sum::<usize>();
    let mut gz = flate2::write::GzEncoder::new(Vec::with_capacity(total), flate2::Compression::default());
    let mut done = 0;
    for part in std::iter::once(&header).chain(&streams) {
        for chunk in part.chunks(1 << 20) {
            gz.write_all(chunk)?;
            done += chunk.len();
            progress(done as f32 / total as f32)?;
        }
    }
    Ok((gz.finish()?, saturated))
}

/// The reference 3DGS layout's properties: what [`ply`] writes, as crate::ply writes them first.
const PLY_PROPERTIES: [&str; 17] =
    ["x", "y", "z", "nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1", "scale_2", "rot_0", "rot_1", "rot_2", "rot_3"];

/// The splats as a binary little-endian PLY of the reference 3DGS layout alone (`x y z nx ny nz f_dc_0..2 opacity
/// scale_0..2 rot_0..3`: degree-zero colour, opacity as a logit, log scales, the rotation w x y z), which every splat tool
/// reads: in RUB like N4N0's own PLY (crate::ply) and [`spz3`], every value the float trained (lossless, as [`gltf`]),
/// without the sky shell and without N4N0's own properties (crate::ply::EXTRAS), which other tools would carry along
/// unread.
pub fn ply(g: &Gaussians) -> anyhow::Result<Vec<u8>> {
    use std::io::Write as _;
    let ids = exported(g)?;
    let mut out = Vec::with_capacity(512 + ids.len() * PLY_PROPERTIES.len() * 4);
    out.extend_from_slice(b"ply\nformat binary_little_endian 1.0\ncomment Orbiters - N4N0 export (right-handed, +Y up: Unity's world with Z negated)\n");
    writeln!(out, "element vertex {}", ids.len())?;
    for p in PLY_PROPERTIES {
        writeln!(out, "property float {p}")?;
    }
    out.extend_from_slice(b"end_header\n");
    for &i in &ids {
        let [x, y, z, w] = Frame::Rub.rotation(g.rotations[i]);
        let values = Frame::Rub
            .position(g.means[i])
            .into_iter()
            .chain([0.0; 3])
            .chain(sh0(g.colors[i]))
            .chain([g.opacity_logits[i]])
            .chain(g.log_scales[i])
            .chain([w, x, y, z]);
        for v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    Ok(out)
}

/// `<splats.ply> <out>`: encodes (and so validates) everything before touching the destination, then writes beside it
/// and renames, so a failed export never leaves a truncated file under the destination's name nor overwrites the PLY
/// it reads. No GPU or sky map is loaded. What the 3D view hid or deleted (crate::edit, the edit beside the PLY) is
/// left out, as from every other output; a damaged or outdated edit refuses the export instead of exporting it all.
fn cli(args: &[String], encode: fn(&Gaussians) -> anyhow::Result<Vec<u8>>) -> anyhow::Result<()> {
    ensure!(args.len() == 2, "usage: --export-gltf <splats.ply> <out.glb> or --export-spz <splats.ply> <out.spz>");
    let (g, _, _) = crate::edit::read_edited(Path::new(&args[0])).with_context(|| format!("reading {}", args[0]))?; // i18n: not shown (a command-line tool run from a terminal: the app never starts it)
    let bytes = encode(&g)?;
    let out = Path::new(&args[1]);
    if out.exists() {
        ensure!(std::fs::canonicalize(out)? != std::fs::canonicalize(&args[0])?, "input and output must differ"); // i18n: not shown (a command-line tool run from a terminal: the app never starts it)
    }
    let tmp = out.with_extension(format!("{}.part", out.extension().and_then(|x| x.to_str()).unwrap_or("export")));
    if tmp.exists() {
        ensure!(std::fs::canonicalize(&tmp)? != std::fs::canonicalize(&args[0])?, "input and temporary output must differ"); // i18n: not shown (a command-line tool run from a terminal: the app never starts it)
    }
    if let Err(e) = std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, out)) {
        std::fs::remove_file(&tmp).ok();
        return Err(e).with_context(|| format!("writing {}", out.display())); // i18n: not shown (a command-line tool run from a terminal: the app never starts it)
    }
    Ok(())
}

/// `--export-gltf <splats.ply> <out.glb>`: [`gltf`].
pub fn gltf_cli(args: &[String]) -> anyhow::Result<()> {
    cli(args, gltf)
}

/// `--export-spz <splats.ply> <out.spz>`: [`spz`].
pub fn spz_cli(args: &[String]) -> anyhow::Result<()> {
    cli(args, spz)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gaussians::rotation_matrix;

    /// RECONSTRUCT-14l: the GLB's values are byte for byte those of the per-splat vectors it used to build.
    #[test]
    fn the_glb_holds_the_same_bytes_without_a_vector_a_splat() {
        let mut g = Gaussians::default();
        for i in 0..40 {
            let f = i as f32;
            g.push([f * 0.3, -f, 2.0 + f * 0.01], [-2.0 + f * 0.01, -3.0, -1.5], [0.9, 0.1 * f, -0.3, 0.2], f * 0.1 - 2.0, [0.2, 0.01 * f, 0.9]);
        }
        let ids = exported(&g).unwrap();
        let mut expected = Vec::new();
        for a in 0..5 {
            for &i in &ids {
                let values: Vec<f32> = match a {
                    0 => Frame::Gltf.position(g.means[i]).to_vec(),
                    1 => Frame::Gltf.rotation(g.rotations[i]).to_vec(),
                    2 => g.log_scales[i].map(f32::exp).to_vec(),
                    3 => vec![sigmoid(g.opacity_logits[i])],
                    _ => sh0(g.colors[i]).to_vec(),
                };
                for v in values {
                    expected.extend(v.to_le_bytes());
                }
            }
        }
        let glb = gltf(&g).unwrap();
        assert!(glb.windows(expected.len()).any(|w| w == expected.as_slice()), "the binary chunk changed");
    }
    use std::collections::HashMap;

    fn sample() -> Gaussians {
        let mut g = Gaussians::default();
        g.push([1., 2., 3.], [-2., -3., -4.], [0.9, 0.1, -0.2, 0.3], 0.4, [0.2, 0.5, 0.8]);
        g
    }

    /// Unity-world splats no mirror or turn leaves alone: long ones on the right (+X), above (+Y) and ahead (+Z),
    /// and an anisotropic one turned about all three axes.
    fn asymmetric() -> Gaussians {
        let mut g = Gaussians::default();
        let l = [0.4f32.ln(), 0.05f32.ln(), 0.05f32.ln()];
        g.push([2., 0., 0.], l, [1., 0., 0., 0.], 3., [1., 0., 0.]);
        g.push([0., 3., 0.], [l[1], l[0], l[1]], [1., 0., 0., 0.], 3., [0., 1., 0.]);
        g.push([0., 0., 5.], [l[1], l[1], l[0]], [1., 0., 0., 0.], 3., [0., 0., 1.]);
        g.push([1., 2., 4.], [0.6f32.ln(), 0.1f32.ln(), 0.03f32.ln()], [0.9, 0.2, 0.3, -0.25], 3., [0.9, 0.8, 0.2]);
        g
    }

    fn covariance(log_scale: [f32; 3], q: [f32; 4]) -> [[f32; 3]; 3] {
        let r = rotation_matrix(q);
        let s = log_scale.map(|s| (2. * s).exp());
        std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| r[i][k] * s[k] * r[j][k]).sum()))
    }

    fn assert_same_covariance(a: [[f32; 3]; 3], b: [[f32; 3]; 3], tolerance: f32) {
        for i in 0..3 {
            for j in 0..3 {
                assert!((a[i][j] - b[i][j]).abs() <= tolerance, "{a:?} vs {b:?}");
            }
        }
    }

    /// The GLB's JSON and its attributes' floats, found as a loader finds them: through accessors and buffer views.
    fn read_glb(b: &[u8]) -> (serde_json::Value, HashMap<String, Vec<f32>>) {
        assert_eq!(&b[..4], b"glTF");
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize, b.len());
        let n = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
        let j: serde_json::Value = serde_json::from_slice(&b[20..20 + n]).unwrap();
        let bin = &b[28 + n..];
        let mut attributes = HashMap::new();
        for (name, a) in j["meshes"][0]["primitives"][0]["attributes"].as_object().unwrap() {
            let accessor = &j["accessors"][a.as_u64().unwrap() as usize];
            let view = &j["bufferViews"][accessor["bufferView"].as_u64().unwrap() as usize];
            let (o, l) = (view["byteOffset"].as_u64().unwrap() as usize, view["byteLength"].as_u64().unwrap() as usize);
            attributes.insert(name.clone(), bin[o..o + l].chunks_exact(4).map(|x| f32::from_le_bytes(x.try_into().unwrap())).collect());
        }
        (j, attributes)
    }

    /// The SPZ's header and its five decompressed streams.
    fn read_spz(b: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut pos = 32 + 5 * 16;
        let mut streams = Vec::new();
        for i in 0..5 {
            let n = u64::from_le_bytes(b[32 + i * 16..40 + i * 16].try_into().unwrap()) as usize;
            let size = u64::from_le_bytes(b[40 + i * 16..48 + i * 16].try_into().unwrap()) as usize;
            streams.push(zstd::bulk::decompress(&b[pos..pos + n], size).unwrap());
            pos += n;
        }
        assert_eq!(pos, b.len());
        (b[..32].to_vec(), streams)
    }

    #[test]
    fn glb_attributes_roundtrip() {
        let g = sample();
        let (j, a) = read_glb(&gltf(&g).unwrap());
        assert_eq!(j["accessors"][0]["count"], 1);
        assert_eq!(j["meshes"][0]["primitives"][0]["extensions"]["KHR_gaussian_splatting"]["colorSpace"], "srgb_rec709_display");
        assert_eq!(a["POSITION"], [-1., 2., 3.]);
        for k in 0..3 {
            assert!((a["KHR_gaussian_splatting:SCALE"][k].ln() - g.log_scales[0][k]).abs() < 1e-6);
            assert!((a["KHR_gaussian_splatting:SH_DEGREE_0_COEF_0"][k] * ply::SH_C0 + 0.5 - g.colors[0][k]).abs() < 1e-6);
        }
        assert!((a["KHR_gaussian_splatting:OPACITY"][0] - sigmoid(0.4)).abs() < 1e-7);
    }

    /// Imported into Unity as glTFast and UnityGLTF import (X negated, the quaternion (x, −y, −z, w)), the GLB is the
    /// capture itself: same places, same shapes. The splat ahead of the capture is at glTF's front (+Z) and the one on
    /// its right at an asset's right (−X), so Blender's front view looks at what the capture looked at.
    #[test]
    fn glb_comes_back_into_unity_as_captured() {
        let g = asymmetric();
        let (_, a) = read_glb(&gltf(&g).unwrap());
        let (p, q, s) = (&a["POSITION"], &a["KHR_gaussian_splatting:ROTATION"], &a["KHR_gaussian_splatting:SCALE"]);
        for i in 0..g.len() {
            assert_eq!([-p[3 * i], p[3 * i + 1], p[3 * i + 2]], g.means[i]);
            let unity = [q[4 * i + 3], q[4 * i], -q[4 * i + 1], -q[4 * i + 2]];
            let back = covariance(std::array::from_fn(|k| s[3 * i + k].ln()), unity);
            assert_same_covariance(back, covariance(g.log_scales[i], g.rotations[i]), 1e-6);
        }
        assert!(p[2 * 3 + 2] > 0. && p[0] < 0., "ahead at +Z, right at -X");
    }

    /// Converted to Unity's frame as the SPZ library converts its RUB to RUF (Z negated, the quaternion's x and y
    /// negated), the SPZ is the capture within its quantisation; it is also the GLB turned half a turn about Y (X and
    /// Z negated), the library's RUB to LUF. Its header says the splats were trained with Mip-Splatting's filters.
    #[test]
    fn spz_comes_back_into_unity_as_captured() {
        let g = asymmetric();
        let (header, s) = read_spz(&spz(&g).unwrap());
        assert_eq!(header[14], SPZ_ANTIALIASED);
        let (_, a) = read_glb(&gltf(&g).unwrap());
        for i in 0..g.len() {
            let p: [f32; 3] = std::array::from_fn(|k| {
                let b = &s[0][(3 * i + k) * 3..(3 * i + k) * 3 + 3];
                ((b[0] as i32 | (b[1] as i32) << 8 | (b[2] as i32) << 16) << 8 >> 8) as f32 / 4096.
            });
            let unity = [p[0], p[1], -p[2]];
            assert!(unity.iter().zip(g.means[i]).all(|(a, b)| (a - b).abs() <= 0.5 / 4096.), "{unity:?}");
            assert!((0..3).all(|k| (a["POSITION"][3 * i + k] - p[k] * [-1., 1., -1.][k]).abs() <= 0.5 / 4096.));
            let q = unpack_rotation(u32::from_le_bytes(s[4][4 * i..4 * i + 4].try_into().unwrap()));
            let log_scale = std::array::from_fn(|k| s[3][3 * i + k] as f32 / 16. - 10.);
            let back = covariance(log_scale, [q[3], -q[0], -q[1], q[2]]);
            // Log scales are 1/16 steps: a variance within e^(1/16) - 1 of the largest axis's.
            assert_same_covariance(back, covariance(g.log_scales[i], g.rotations[i]), 0.065 * 0.6 * 0.6);
        }
    }

    #[test]
    fn spz_streams_roundtrip() {
        let g = sample();
        let (_, s) = read_spz(&spz(&g).unwrap());
        assert_eq!(s.iter().map(Vec::len).collect::<Vec<_>>(), [9, 1, 3, 3, 4]);
        for k in 0..3 {
            let p = &s[0][k * 3..k * 3 + 3];
            let v = ((p[0] as i32) | ((p[1] as i32) << 8) | ((p[2] as i32) << 16)) << 8 >> 8;
            assert!((v as f32 / 4096. - [1., 2., -3.][k]).abs() < 0.00013);
            assert!((s[3][k] as f32 / 16. - 10. - g.log_scales[0][k]).abs() <= 0.03125);
            assert!(((s[2][k] as f32 / 255. - 0.5) / 0.15 * ply::SH_C0 + 0.5 - g.colors[0][k]).abs() < 0.004);
        }
        let q = unpack_rotation(u32::from_le_bytes(s[4][..].try_into().unwrap()));
        assert!(q.iter().zip(Frame::Rub.rotation(g.rotations[0])).map(|(a, b)| a * b).sum::<f32>().abs() > 0.99999);
    }

    /// Version 3 is version 4's streams in the legacy container: one gzip stream of a 16-byte header (NGSP, 3, the count,
    /// degree 0, 12 fractional bits, antialiased, reserved) and the five streams one after the other. Its progress
    /// reaches 1, and an error from it stops the encoding.
    #[test]
    fn spz3_holds_version_4s_streams_in_one_gzip_stream() {
        use std::io::Read as _;
        let g = asymmetric();
        let mut heard = Vec::new();
        let (v3, s3) = spz3(&g, &mut |f| {
            heard.push(f);
            Ok(())
        })
        .unwrap();
        let (v4, s4) = spz4(&g).unwrap();
        assert_eq!(s3, s4);
        assert_eq!(v3[..2], [0x1f, 0x8b], "a gzip stream");
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(&v3[..]).read_to_end(&mut raw).unwrap();
        assert_eq!(raw[..4], *b"NGSP");
        assert_eq!(u32::from_le_bytes(raw[4..8].try_into().unwrap()), 3);
        assert_eq!(u32::from_le_bytes(raw[8..12].try_into().unwrap()), g.len() as u32);
        assert_eq!(raw[12..16], [0, 12, SPZ_ANTIALIASED, 0]);
        assert_eq!(raw[16..], read_spz(&v4).1.concat()[..]);
        assert_eq!(heard.last(), Some(&1.0));
        assert!(heard.windows(2).all(|w| w[0] <= w[1]));
        assert!(spz3(&g, &mut |_| anyhow::bail!("stopped")).is_err());
    }

    /// The PLY export is the reference layout alone, which crate::ply reads back as the very splats (positions,
    /// scales, opacities and shapes as trained, colours to float precision), without the sky shell.
    #[test]
    fn ply_export_reads_back_as_the_splats_without_the_sky() {
        let mut g = asymmetric();
        g.push([0., 300., 0.], [3.; 3], [1., 0., 0., 0.], 2., [0.4, 0.6, 0.9]);
        *g.sky.last_mut().unwrap() = true;
        let bytes = ply(&g).unwrap();
        let end = bytes.windows(11).position(|w| w == b"end_header\n").unwrap() + 11;
        let header = std::str::from_utf8(&bytes[..end]).unwrap();
        assert!(header.contains("element vertex 4\n") && !header.contains("sky"), "{header}");
        assert_eq!(header.matches("property float").count(), 17);
        assert_eq!(bytes.len(), end + 4 * 17 * 4);
        let back = crate::ply::parse(&bytes).unwrap();
        assert_eq!(back.len(), 4);
        for i in 0..4 {
            assert_eq!(back.means[i], g.means[i]);
            assert_eq!(back.log_scales[i], g.log_scales[i]);
            assert_eq!(back.opacity_logits[i], g.opacity_logits[i]);
            assert!(back.colors[i].iter().zip(g.colors[i]).all(|(a, b)| (a - b).abs() < 1e-6));
            assert_same_covariance(covariance(back.log_scales[i], back.rotations[i]), covariance(g.log_scales[i], g.rotations[i]), 1e-6);
            assert!(!back.sky[i]);
        }
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
        assert!(spz3(&g, &mut |_| Ok(())).is_err());
        g.means[0][0] = 1.;
        g.filter_3d[0] = 1e-4;
        assert!(gltf(&g).is_err());
    }

    /// What the 3D view hid or deleted never leaves in an export (crate::edit), as from every other output.
    #[test]
    fn exports_leave_out_what_the_3d_view_removed() {
        let dir = std::env::temp_dir().join(format!("orbiters-export-edit-{}-{}", std::process::id(), fastrand::u64(..)));
        std::fs::create_dir(&dir).unwrap();
        let mut g = Gaussians::default();
        for i in 0..6 {
            g.push([i as f32, 0., 1.], [-3.; 3], [1., 0., 0., 0.], 1., [0.5; 3]);
        }
        let input = dir.join(orbiters_core::reconstruction::PLY_FILE);
        ply::write(&input, &g).unwrap();
        let (_, identity, _) = crate::edit::read_for_view(&input).unwrap();
        let mut edit = crate::edit::Edit::new(6, identity);
        edit.marks[1] = crate::edit::HIDDEN;
        edit.marks[4] = crate::edit::DELETED;
        edit.save(&dir).unwrap();
        let output = dir.join("model.spz");
        spz_cli(&[input.to_string_lossy().into_owned(), output.to_string_lossy().into_owned()]).unwrap();
        let bytes = std::fs::read(&output).unwrap();
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 4, "four of six splats exported");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Every format of a real reconstruction, timed, for other readers to check (a lab run, not by default):
    /// `ORBITERS_EXPORT_PLY=<session>/reconstruction/splats.ply ORBITERS_EXPORT_OUT=<folder> cargo test --release -p
    /// orbiters-reconstruct export_a_real_reconstruction -- --ignored --nocapture` writes there what its edits keep as
    /// model.spz (version 3), model-v4.spz, model.glb and model.ply, and prints their sizes and times.
    #[test]
    #[ignore]
    fn export_a_real_reconstruction() {
        let ply_path = std::env::var("ORBITERS_EXPORT_PLY").expect("ORBITERS_EXPORT_PLY");
        let out = std::path::PathBuf::from(std::env::var("ORBITERS_EXPORT_OUT").expect("ORBITERS_EXPORT_OUT"));
        std::fs::create_dir_all(&out).unwrap();
        let start = std::time::Instant::now();
        let (g, removed, _) = crate::edit::read_edited(Path::new(&ply_path)).unwrap();
        let kept = (0..g.len()).filter(|&i| !g.sky[i]).count();
        println!("read {} splats ({removed} removed by edits, {kept} without the sky) in {:.2} s", g.len(), start.elapsed().as_secs_f32());
        let encoders: [(&str, Box<dyn Fn(&Gaussians) -> anyhow::Result<(Vec<u8>, Option<Saturated>)>>); 4] = [
            ("model.spz", Box::new(|g| spz3(g, &mut |_| Ok(())).map(|(b, s)| (b, Some(s))))),
            ("model-v4.spz", Box::new(|g| spz4(g).map(|(b, s)| (b, Some(s))))),
            ("model.glb", Box::new(|g| gltf(g).map(|b| (b, None)))),
            ("model.ply", Box::new(|g| ply(g).map(|b| (b, None)))),
        ];
        for (name, encode) in encoders {
            let start = std::time::Instant::now();
            let (bytes, saturated) = encode(&g).unwrap();
            let took = start.elapsed().as_secs_f32();
            std::fs::write(out.join(name), &bytes).unwrap();
            println!("{name}: {} bytes, {:.2} bytes a splat, {took:.2} s, saturated {saturated:?}", bytes.len(), bytes.len() as f64 / kept as f64);
        }
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
