//! CPU mesh evaluation: mesh_bench <session> <output directory> [ground truth directory] [occupancy PLY].
use anyhow::Context;
use orbiters_reconstruct::{
    camera::Camera,
    mesh::{self, Mesh},
    ply, prepare,
};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};
#[derive(Deserialize)]
struct View {
    depth: String,
    camera_to_world: [f32; 12],
    projection: [f32; 4],
    width: u32,
    height: u32,
}
fn png(path: &Path, c: &Camera, d: &[f32]) -> anyhow::Result<()> {
    let rgb: Vec<u8> = d
        .iter()
        .flat_map(|&z| {
            if z.is_finite() {
                let v = (255. * (1. - z / 40.).clamp(0., 1.)) as u8;
                [v, v, v]
            } else {
                [100, 0, 100]
            }
        })
        .collect();
    let mut e = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path)?), c.width, c.height);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header()?.write_image_data(&rgb)?;
    Ok(())
}
fn overlay(path: &Path, c: &Camera, d: &[f32], g: &orbiters_reconstruct::gaussians::Gaussians) -> anyhow::Result<()> {
    let w = c.width as usize;
    let h = c.height as usize;
    let mut rgb = vec![0u8; w * h * 9];
    let mut points = vec![f32::INFINITY; w * h];
    for y in 0..h {
        for x in 0..w {
            let z = d[y * w + x];
            let color = if z.is_finite() {
                let v = (255. * (1. - z / 40.).clamp(0., 1.)) as u8;
                [v, v, v]
            } else {
                [100, 0, 100]
            };
            for panel in [0, 2] {
                let k = (y * w * 3 + panel * w + x) * 3;
                rgb[k..k + 3].copy_from_slice(&color);
            }
        }
    }
    for (i, &p) in g.means.iter().enumerate() {
        let Some((x, y, z)) = c.project(p) else { continue };
        let x = x.round() as i32;
        let y = y.round() as i32;
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
            continue;
        }
        let at = y as usize * w + x as usize;
        if z >= points[at] {
            continue;
        }
        points[at] = z;
        let color = g.colors[i].map(|v| (255. * v.clamp(0., 1.)) as u8);
        let k = (y as usize * w * 3 + w + x as usize) * 3;
        rgb[k..k + 3].copy_from_slice(&color);
        if !d[at].is_finite() || (z - d[at]).abs() < 0.15 {
            let k = (y as usize * w * 3 + 2 * w + x as usize) * 3;
            rgb[k..k + 3].copy_from_slice(&color);
        }
    }
    let mut e = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path)?), c.width * 3, c.height);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header()?.write_image_data(&rgb)?;
    Ok(())
}
fn readback(file: &Path, m: &Mesh) -> anyhow::Result<f32> {
    let b = std::fs::read(file)?;
    let n = u32::from_le_bytes(b[12..16].try_into()?) as usize;
    let start = 28 + n;
    let mut maximum = 0.0f32;
    for (i, p) in m.positions.iter().enumerate() {
        let k = start + i * 12;
        let mut decoded: [f32; 3] = std::array::from_fn(|c| f32::from_le_bytes(b[k + c * 4..k + c * 4 + 4].try_into().unwrap()));
        decoded[0] = -decoded[0];
        for c in 0..3 {
            maximum = maximum.max((decoded[c] - p[c]).abs());
        }
    }
    anyhow::ensure!(maximum == 0., "exported coordinates do not round-trip");
    Ok(maximum)
}
// This deliberately supports the single, identity-node GLB emitted by splat-transform,
// rejecting unsupported layouts instead of silently scoring a subset or ignoring a transform.
fn collision_glb(path: &Path) -> anyhow::Result<Mesh> {
    let b = std::fs::read(path)?;
    let word = |at: usize| -> anyhow::Result<u32> { Ok(u32::from_le_bytes(b.get(at..at + 4).context("truncated GLB word")?.try_into()?)) };
    anyhow::ensure!(b.get(..4) == Some(b"glTF") && word(4)? == 2 && word(8)? as usize == b.len(), "GLB header");
    let n = word(12)? as usize;
    anyhow::ensure!(word(16)? == 0x4e4f534a, "JSON chunk required");
    let j: serde_json::Value = serde_json::from_slice(b.get(20..20 + n).context("truncated JSON")?)?;
    let binary = 28 + n;
    anyhow::ensure!(word(24 + n)? == 0x004e4942 && binary + word(20 + n)? as usize == b.len(), "BIN chunk required");
    anyhow::ensure!(
        j["meshes"].as_array().is_some_and(|v| v.len() == 1)
            && j["meshes"][0]["primitives"].as_array().is_some_and(|v| v.len() == 1)
            && j["nodes"].as_array().is_some_and(|v| v.len() == 1)
            && j["nodes"][0]["mesh"] == 0,
        "only a single mesh/primitive/node is supported"
    );
    for field in ["matrix", "translation", "rotation", "scale", "children"] {
        anyhow::ensure!(j["nodes"][0][field].is_null(), "transformed or hierarchical GLB is unsupported");
    }
    let primitive = &j["meshes"][0]["primitives"][0];
    anyhow::ensure!(primitive["mode"].is_null() || primitive["mode"] == 4, "triangle list required");
    let pa = &j["accessors"][primitive["attributes"]["POSITION"].as_u64().context("position accessor")? as usize];
    let ia = &j["accessors"][primitive["indices"].as_u64().context("index accessor")? as usize];
    let data = |a: &serde_json::Value, element: usize| -> anyhow::Result<(&[u8], usize, usize)> {
        anyhow::ensure!(a["sparse"].is_null(), "sparse accessors unsupported");
        let v = &j["bufferViews"][a["bufferView"].as_u64().context("buffer view")? as usize];
        anyhow::ensure!(v["buffer"] == 0, "embedded buffer required");
        let start = v["byteOffset"].as_u64().unwrap_or(0) as usize;
        let length = v["byteLength"].as_u64().context("buffer length")? as usize;
        let start = binary.checked_add(start).context("buffer offset overflow")?;
        let end = start.checked_add(length).context("buffer length overflow")?;
        let bytes = b.get(start..end).context("buffer view out of range")?;
        let offset = a["byteOffset"].as_u64().unwrap_or(0) as usize;
        let count = a["count"].as_u64().context("accessor count")? as usize;
        let stride = v["byteStride"].as_u64().unwrap_or(element as u64) as usize;
        let extent = count.saturating_sub(1).checked_mul(stride).and_then(|n| n.checked_add(element)).context("accessor overflow")?;
        anyhow::ensure!(
            count > 0 && stride >= element && offset.checked_add(extent).is_some_and(|end| end <= bytes.len()),
            "accessor out of range"
        );
        Ok((&bytes[offset..], count, stride))
    };
    anyhow::ensure!(pa["componentType"] == 5126 && pa["type"] == "VEC3", "float positions required");
    let (bytes, count, stride) = data(pa, 12)?;
    let positions: Vec<[f32; 3]> = (0..count)
        .map(|i| std::array::from_fn(|c| -f32::from_le_bytes(bytes[i * stride + c * 4..i * stride + c * 4 + 4].try_into().unwrap())))
        .collect();
    anyhow::ensure!(positions.iter().flatten().all(|v| v.is_finite()), "nonfinite position");
    // N4N0 PLY stores [x,y,-z]. PlayCanvas labels PLY with a 180-degree Z rotation,
    // and writeVoxel bakes it into engine space: [-x,-y,-z]. Negate all axes back.
    let ty = ia["componentType"].as_u64().context("index component type")?;
    anyhow::ensure!((ty == 5125 || ty == 5123) && ia["type"] == "SCALAR", "unsigned indices required");
    let element = if ty == 5125 { 4 } else { 2 };
    let (bytes, count, stride) = data(ia, element)?;
    anyhow::ensure!(count % 3 == 0, "triangle index count");
    let indices: Vec<_> = (0..count)
        .map(|i| {
            if ty == 5125 {
                u32::from_le_bytes(bytes[i * stride..i * stride + 4].try_into().unwrap())
            } else {
                u16::from_le_bytes(bytes[i * stride..i * stride + 2].try_into().unwrap()) as u32
            }
        })
        .collect();
    anyhow::ensure!(indices.iter().all(|&i| (i as usize) < positions.len()), "index out of range");
    Ok(Mesh { positions, triangles: indices.chunks_exact(3).map(|t| [t[0], t[2], t[1]]).collect() })
}

fn occupancy(path: &Path, size: f32) -> anyhow::Result<Mesh> {
    if path.extension().and_then(|s| s.to_str()) == Some("glb") {
        return collision_glb(path);
    }
    let g = ply::read(path)?;
    let mut occupied = HashSet::new();
    for (i, p) in g.means.iter().enumerate() {
        if g.sky[i] {
            continue;
        }
        let opacity = 1.0 / (1.0 + (-g.opacity_logits[i]).exp());
        if opacity < 0.1 {
            continue;
        }
        let r = orbiters_reconstruct::gaussians::rotation_matrix(g.rotations[i]);
        let scale = g.log_scales[i].map(f32::exp);
        let cutoff = (2.0 * (opacity / 0.1).ln()).sqrt();
        let extent: [f32; 3] = std::array::from_fn(|a| (0..3).map(|b| (r[a][b] * scale[b]).powi(2)).sum::<f32>().sqrt() * cutoff + size);
        let lo: [i32; 3] = std::array::from_fn(|a| ((p[a] - extent[a]) / size).floor() as i32);
        let hi: [i32; 3] = std::array::from_fn(|a| ((p[a] + extent[a]) / size).ceil() as i32);
        anyhow::ensure!(
            (0..3).map(|a| (hi[a] - lo[a] + 1) as i64).product::<i64>() < 10_000_000,
            "occupancy splat exceeds 10M candidate nodes"
        );
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    let k = [x, y, z];
                    let d: [f32; 3] = std::array::from_fn(|a| k[a] as f32 * size - p[a]);
                    // Expand each Gaussian axis by half a voxel so sub-voxel thin discs are represented.
                    let d2 = (0..3).map(|b| ((0..3).map(|a| r[a][b] * d[a]).sum::<f32>() / (scale[b] + size * 0.5)).powi(2)).sum::<f32>();
                    if d2 <= cutoff * cutoff {
                        occupied.insert(k);
                    }
                }
            }
        }
        anyhow::ensure!(occupied.len() < 12_000_000, "occupancy exceeds 12M nodes");
    }
    let mut f = HashMap::new();
    for &k in &occupied {
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    let q = [k[0] + x, k[1] + y, k[2] + z];
                    f.insert(q, if occupied.contains(&q) { -0.5 } else { 0.5 });
                }
            }
        }
    }
    Ok(mesh::marching_cubes(&f, size))
}
fn main() -> anyhow::Result<()> {
    let a: Vec<_> = std::env::args().skip(1).collect();
    anyhow::ensure!(a.len() >= 2, "session output [gt] [ply]");
    let out = Path::new(&a[1]);
    std::fs::create_dir_all(out)?;
    let load = std::time::Instant::now();
    let p = prepare::prepare(Path::new(&a[0]), &prepare::Options { width: 640, aberration: false, ..Default::default() }, |_| {})?;
    let loading = load.elapsed().as_secs_f64();
    for s in p.describe() {
        eprintln!("{s}");
    }
    let views: Vec<View> = if a.len() > 2 {
        std::fs::read_to_string(Path::new(&a[2]).join("views.jsonl"))?.lines().map(serde_json::from_str).collect::<Result<_, _>>()?
    } else {
        vec![]
    };
    let sizes: Vec<f32> = if let Ok(v) = std::env::var("MESH_BENCH_VOXEL") { vec![v.parse()?] } else { vec![0.02, 0.05, 0.1] };
    let mut failed = false;
    let first_mode = if std::env::var("MESH_BENCH_MODE").as_deref() == Ok("occupancy") { 1 } else { 0 };
    for mode in first_mode..if a.len() > 3 { 2 } else { 1 } {
        for &size in &sizes {
            let name = format!("{}-{:.0}cm", if mode == 0 { "depth" } else { "occupancy-opacity" }, size * 100.);
            let arm = (|| -> anyhow::Result<()> {
                let start = std::time::Instant::now();
                let mut m = if mode == 0 { mesh::fuse(&p.data, size)? } else { occupancy(Path::new(&a[3]), size)? };
                let raw = m.triangles.len();
                m.clean(16);
                m.decimate(100_000, size);
                let components = m.clean(16);
                let file = out.join(format!("{name}.glb"));
                m.write(&file)?;
                let roundtrip_error = readback(&file, &m)?;
                m.write(&out.join(format!("{name}.obj")))?;
                let elapsed = start.elapsed().as_secs_f64();
                let mut per_view = Vec::new();
                let mut pooled_errors = Vec::new();
                let mut total_true = 0usize;
                let mut total_within = 0usize;
                for (i, v) in views.iter().enumerate() {
                    let c = Camera::from_strip(&v.camera_to_world, &v.projection, v.width, v.height, 1.);
                    let rendered = m.depth(&c);
                    let bytes = std::fs::read(Path::new(&a[2]).join(&v.depth))?;
                    let truth: Vec<_> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
                    anyhow::ensure!(truth.len() == rendered.len(), "truth dimensions");
                    let mut errors = Vec::new();
                    let mut valid = 0;
                    let mut within = 0;
                    for (&gt, &d) in truth.iter().zip(&rendered) {
                        if gt > 0. && gt.is_finite() {
                            valid += 1;
                            if d.is_finite() {
                                let e = (d - gt).abs() / gt;
                                if e <= 0.05 {
                                    within += 1
                                }
                                errors.push(e);
                            }
                        }
                    }
                    total_true += valid;
                    total_within += within;
                    pooled_errors.extend_from_slice(&errors);
                    errors.sort_unstable_by(f32::total_cmp);
                    per_view.push(serde_json::json!({"median_relative":errors.get(errors.len()/2),"within5_all":within as f64/valid.max(1) as f64,"coverage":errors.len() as f64/valid.max(1) as f64}));
                    if i % 35 == 0 {
                        png(&out.join(format!("{name}-gt-{i}.png")), &c, &rendered)?;
                    }
                }
                for (i, v) in p.data.views.iter().step_by((p.data.views.len() / 3).max(1)).take(3).enumerate() {
                    let c = v.camera;
                    let depth = m.depth(&c);
                    png(&out.join(format!("{name}-capture-{i}.png")), &c, &depth)?;
                    if i == 0 && a.len() > 3 && Path::new(&a[3]).extension().and_then(|s| s.to_str()) == Some("ply") {
                        let g = ply::read(Path::new(&a[3]))?;
                        overlay(&out.join(format!("{name}-splat-overlay.png")), &c, &depth, &g)?;
                    }
                }
                let path: Vec<_> = p.data.views.iter().map(|v| v.camera.position()).collect();
                let (supported, same_floor) = m.floor_support(&path);
                pooled_errors.sort_unstable_by(f32::total_cmp);
                let mean = |key: &str| {
                    let values: Vec<_> = per_view.iter().filter_map(|v| v[key].as_f64()).collect();
                    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
                };
                let bounds_min: [f32; 3] = std::array::from_fn(|i| m.positions.iter().map(|p| p[i]).fold(f32::INFINITY, f32::min));
                let bounds_max: [f32; 3] = std::array::from_fn(|i| m.positions.iter().map(|p| p[i]).fold(f32::NEG_INFINITY, f32::max));
                let result = serde_json::json!({"gltf_roundtrip_max_error":roundtrip_error,"pooled_median_relative":pooled_errors.get(pooled_errors.len()/2),"pooled_coverage":pooled_errors.len() as f64/total_true.max(1) as f64,"pooled_within5_all":total_within as f64/total_true.max(1) as f64,"bounds_min":bounds_min,"bounds_max":bounds_max,"name":name,"loading_seconds":loading,"mesh_seconds":elapsed,"pre_cleanup_triangles":raw,"triangles":m.triangles.len(),"bytes":std::fs::metadata(file)?.len(),"components":components,"single_floor_support_share":same_floor as f64/path.len().max(1) as f64,"path_support_share":supported as f64/p.data.views.len().max(1) as f64,"views":per_view.len(),"mean_median_relative":mean("median_relative"),"mean_within5_all":mean("within5_all"),"mean_coverage":mean("coverage"),"per_view":per_view});
                std::fs::write(out.join(format!("{name}.json")), serde_json::to_vec_pretty(&result)?)?;
                println!(
                    "{}",
                    serde_json::json!({"name":name,"seconds":elapsed,"triangles":m.triangles.len(),"coverage":result["mean_coverage"],"median":result["mean_median_relative"]})
                );
                Ok(())
            })();
            if let Err(error) = arm {
                failed = true;
                let failure = serde_json::json!({"name":name,"error":format!("{error:#}")});
                eprintln!("{failure}");
                std::fs::write(out.join(format!("{name}.error.json")), serde_json::to_vec_pretty(&failure)?)?;
            }
        }
    }
    anyhow::ensure!(!failed, "one or more mesh arms failed; see error JSON");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mesh_playcanvas_engine_coordinates_restore_native_world() {
        let m = Mesh { positions: vec![[1., 2., 3.], [2., 2., 3.], [1., 3., 3.]], triangles: vec![[0, 1, 2]] };
        let path = std::env::temp_dir().join(format!("orbiters-playcanvas-{}.glb", std::process::id()));
        // Mesh::write reflects X and winding. Pre-reflect Y/Z so the fixture is a valid
        // PlayCanvas-engine GLB, including correct accessor bounds, not just patched bytes.
        let fixture = Mesh {
            positions: m.positions.iter().map(|p| [p[0], -p[1], -p[2]]).collect(),
            triangles: m.triangles.clone(),
        };
        fixture.write(&path).unwrap();
        let restored = collision_glb(&path).unwrap();
        assert_eq!(restored.positions, m.positions);
        assert_eq!(restored.triangles, m.triangles);
        std::fs::write(&path, b"bad").unwrap();
        assert!(collision_glb(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
