//! Collider evaluation (orbiters_reconstruct::mesh):
//!
//! ```text
//! mesh_bench <session> <output directory> [--gt DIR] [--splats PLY] [--voxel 0.04,0.05] [--radius M] [--no-fill]
//!            [--quest] [--label NAME] [--reference NAME=FILE.glb]...
//! ```
//!
//! Builds the collider at each voxel size, writes its GLB and scores the GLB as Unity imports it (X reflected back):
//! - with a ground truth (`offpath`'s views.jsonl and little-endian eye-depth files): median relative depth error over
//!   covered true pixels, coverage (covered / valid true pixels) and within-5 % (correct / valid true pixels, holes
//!   count as failures), pooled over all pixels, over the pixels whose true surface lies in the reachable region, and
//!   over those the capture observed (a voxel with weight there);
//! - the walked path: the first experiment's floor probes under the keyframe cameras (one connected floor) and a
//!   capsule walk (orbiters_reconstruct::mesh::walk);
//! - geometry: boundary and non-manifold edges, components, triangles, bytes, seconds per stage;
//! - renders: ground-truth views (reachable pixels green within 5 %, yellow off, red holes), keyframes beside the
//!   collider shaded in their camera, a top view with the path, falls and blocks, and the splats over the collider when
//!   a PLY is given.
//!
//! `--reference NAME=FILE.glb` scores an existing GLB in the same convention (the first experiment's, say) against the
//! first arm's region.

use anyhow::Context;
use orbiters_reconstruct::{
    camera::Camera,
    mesh::{self, walk, Mesh, Roi, Settings},
    ply, prepare,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct View {
    depth: String,
    camera_to_world: [f32; 12],
    projection: [f32; 4],
    width: u32,
    height: u32,
}

fn write_png(path: &Path, w: u32, h: u32, rgb: &[u8]) -> anyhow::Result<()> {
    let mut e = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path)?), w, h);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header()?.write_image_data(rgb)?;
    Ok(())
}

/// A shaded colour for a surface normal: walkable faces green-grey, walls orange-grey, lit from above-left.
fn shade(n: [f32; 3]) -> [f32; 3] {
    let light = (0.35 * n[0] + 0.8 * n[1].abs() + 0.45 * n[2]).abs() * 0.6 + 0.4;
    let base = if n[1].abs() >= 45f32.to_radians().cos() { [0.45, 0.85, 0.5] } else { [0.95, 0.65, 0.35] };
    base.map(|c| c * light)
}

#[derive(Default)]
struct Pool {
    errors: Vec<f32>,
    valid: usize,
    within: usize,
}

impl Pool {
    fn add(&mut self, truth: f32, d: f32) {
        self.valid += 1;
        if d.is_finite() {
            let e = (d - truth).abs() / truth;
            if e <= 0.05 {
                self.within += 1;
            }
            self.errors.push(e);
        }
    }
    fn json(&mut self) -> serde_json::Value {
        self.errors.sort_unstable_by(f32::total_cmp);
        serde_json::json!({
            "pixels": self.valid,
            "coverage": self.errors.len() as f64 / self.valid.max(1) as f64,
            "within5": self.within as f64 / self.valid.max(1) as f64,
            "median_relative": self.errors.get(self.errors.len() / 2),
            "p90_relative": self.errors.get(self.errors.len() * 9 / 10),
        })
    }
}

struct Inputs<'a> {
    session: &'a Path,
    out: &'a Path,
    gt: Option<&'a Path>,
    views: &'a [View],
    data: &'a orbiters_reconstruct::dataset::Dataset,
    splats: Option<&'a orbiters_reconstruct::gaussians::Gaussians>,
}

/// Scores `m` (as Unity imports it) and writes its renders; returns the arm's JSON.
fn evaluate(i: &Inputs, name: &str, m: &Mesh, roi: &Roi, volume: Option<&mesh::tsdf::Volume>, feet: &[[f32; 3]], eye_height: f32) -> anyhow::Result<serde_json::Value> {
    let (mut all, mut reach, mut seen) = (Pool::default(), Pool::default(), Pool::default());
    // Reachable pixels by the true surface's height over the lowest feet, and by its horizontal distance to the path.
    let mut by_height: [Pool; 3] = Default::default();
    let mut by_distance: [Pool; 3] = Default::default();
    let floor = feet.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
    let near_path = PathDistance::new(feet);
    let mut per_view = Vec::new();
    for (k, v) in i.views.iter().enumerate() {
        let c = Camera::from_strip(&v.camera_to_world, &v.projection, v.width, v.height, 1.);
        let rendered = m.depth(&c);
        let bytes = std::fs::read(i.gt.unwrap().join(&v.depth))?;
        let truth: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
        anyhow::ensure!(truth.len() == rendered.len(), "truth dimensions");
        let mut view_reach = Pool::default();
        let mut rgb = vec![0u8; truth.len() * 3];
        for (p, (&t, &d)) in truth.iter().zip(&rendered).enumerate() {
            if !(t > 0. && t.is_finite()) {
                rgb[p * 3..p * 3 + 3].copy_from_slice(&[40, 40, 70]);
                continue;
            }
            all.add(t, d);
            let world = c.unproject((p as u32 % c.width) as f32, (p as u32 / c.width) as f32, t);
            let inside = roi.contains(world);
            let colour = if inside {
                reach.add(t, d);
                view_reach.add(t, d);
                let up = world[1] - floor;
                by_height[if up < 0.15 { 0 } else if up < 2.0 { 1 } else { 2 }].add(t, d);
                let far = near_path.distance(world);
                by_distance[if far < 10.0 { 0 } else if far < 25.0 { 1 } else { 2 }].add(t, d);
                if volume.is_some_and(|v| v.observed(world)) {
                    seen.add(t, d);
                }
                if !d.is_finite() {
                    [230, 40, 40]
                } else if (d - t).abs() / t <= 0.05 {
                    [60, 200, 90]
                } else {
                    [230, 200, 40]
                }
            } else {
                let g = (255. * (1. - t / 80.).clamp(0., 1.) * 0.5) as u8;
                [g, g, g]
            };
            rgb[p * 3..p * 3 + 3].copy_from_slice(&colour);
        }
        per_view.push(view_reach.json());
        if k % 35 == 0 {
            write_png(&i.out.join(format!("{name}-gt-{k}.png")), c.width, c.height, &rgb)?;
        }
    }
    // Keyframes beside the collider shaded in their camera, and blended over them.
    for (k, v) in i.data.views.iter().step_by((i.data.views.len() / 3).max(1)).take(3).enumerate() {
        let c = v.camera;
        let (depth, normals) = m.render(&c);
        let (w, h) = (c.width as usize, c.height as usize);
        let mut rgb = vec![0u8; w * h * 9];
        for y in 0..h {
            for x in 0..w {
                let px = v.image[y * w + x];
                let photo = [px & 255, (px >> 8) & 255, (px >> 16) & 255].map(|c| c as f32 / 255.);
                let at = y * w + x;
                let mesh = if depth[at].is_finite() { Some(shade(normals[at])) } else { None };
                let panels = [photo, mesh.unwrap_or([0.4, 0.0, 0.4]), mesh.map_or(photo, |m| std::array::from_fn(|i| 0.45 * photo[i] + 0.55 * m[i]))];
                for (panel, colour) in panels.iter().enumerate() {
                    let o = (y * w * 3 + panel * w + x) * 3;
                    for ch in 0..3 {
                        rgb[o + ch] = (colour[ch].clamp(0., 1.) * 255.) as u8;
                    }
                }
            }
        }
        write_png(&i.out.join(format!("{name}-keyframe-{k}.png")), c.width * 3, c.height, &rgb)?;
        if k == 0 {
            if let Some(g) = i.splats {
                // Splat centres (sky excluded) over the shaded collider: alignment in the capture's own camera.
                let mut over = vec![0u8; w * h * 3];
                for at in 0..w * h {
                    let s = if depth[at].is_finite() { shade(normals[at]).map(|c| c * 0.6) } else { [0.15, 0.0, 0.15] };
                    for ch in 0..3 {
                        over[at * 3 + ch] = (s[ch] * 255.) as u8;
                    }
                }
                let mut near = vec![f32::INFINITY; w * h];
                for (j, &p) in g.means.iter().enumerate() {
                    if g.sky.get(j).copied().unwrap_or(false) {
                        continue;
                    }
                    let Some((x, y, z)) = c.project(p) else { continue };
                    let (x, y) = (x.round() as i64, y.round() as i64);
                    if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                        continue;
                    }
                    let at = y as usize * w + x as usize;
                    if z < near[at] && (!depth[at].is_finite() || z < depth[at] + 0.1) {
                        near[at] = z;
                        let col = g.colors[j].map(|v| (v.clamp(0., 1.) * 255.) as u8);
                        over[at * 3..at * 3 + 3].copy_from_slice(&col);
                    }
                }
                write_png(&i.out.join(format!("{name}-splats-over-collider.png")), c.width, c.height, &over)?;
            }
        }
    }
    let keyframe_cameras: Vec<_> = i.data.views.iter().map(|v| v.camera.position()).collect();
    let (supported, same_floor) = m.floor_support(&keyframe_cameras);
    let capsule = walk::Capsule::for_eye_height(eye_height);
    let walked = walk::walk(m, feet, &capsule);
    top_view(&i.out.join(format!("{name}-top.png")), m, feet, &walked)?;
    let audit = m.audit();
    let bounds_min: [f32; 3] = std::array::from_fn(|k| m.positions.iter().map(|p| p[k]).fold(f32::INFINITY, f32::min));
    let bounds_max: [f32; 3] = std::array::from_fn(|k| m.positions.iter().map(|p| p[k]).fold(f32::NEG_INFINITY, f32::max));
    let _ = i.session;
    Ok(serde_json::json!({
        "name": name,
        "all_pixels": all.json(),
        "reachable_pixels": reach.json(),
        "observed_reachable_pixels": seen.json(),
        "reachable_by_height": {"floor_below_0.15m": by_height[0].json(), "body_0.15_to_2m": by_height[1].json(), "above_2m": by_height[2].json()},
        "reachable_by_path_distance": {"under_10m": by_distance[0].json(), "10_to_25m": by_distance[1].json(), "over_25m": by_distance[2].json()},
        "keyframe_floor_support": supported as f64 / keyframe_cameras.len().max(1) as f64,
        "keyframe_single_floor_support": same_floor as f64 / keyframe_cameras.len().max(1) as f64,
        "walk": walked,
        "capsule": capsule,
        "audit": audit,
        "triangles": m.triangles.len(),
        "vertices": m.positions.len(),
        "bounds_min": bounds_min,
        "bounds_max": bounds_max,
        "per_view_reachable": per_view,
    }))
}

/// Horizontal distance to the walked path, on a 0.5 m grid.
struct PathDistance {
    x0: f32,
    z0: f32,
    w: usize,
    h: usize,
    d: Vec<f32>,
}

impl PathDistance {
    fn new(feet: &[[f32; 3]]) -> Self {
        let mut pts: Vec<[f32; 3]> = Vec::new();
        for p in feet {
            if pts.last().is_none_or(|l| ((p[0] - l[0]).powi(2) + (p[2] - l[2]).powi(2)).sqrt() >= 0.25) {
                pts.push(*p);
            }
        }
        let lo = |i: usize| pts.iter().map(|p| p[i]).fold(f32::INFINITY, f32::min) - 60.0;
        let hi = |i: usize| pts.iter().map(|p| p[i]).fold(f32::NEG_INFINITY, f32::max) + 60.0;
        let (x0, z0) = (lo(0), lo(2));
        let (w, h) = (((hi(0) - x0) / 0.5) as usize + 1, ((hi(2) - z0) / 0.5) as usize + 1);
        let d = (0..w * h)
            .map(|i| {
                let (x, z) = (x0 + (i % w) as f32 * 0.5 + 0.25, z0 + (i / w) as f32 * 0.5 + 0.25);
                pts.iter().map(|p| (p[0] - x).powi(2) + (p[2] - z).powi(2)).fold(f32::INFINITY, f32::min).sqrt()
            })
            .collect();
        Self { x0, z0, w, h, d }
    }
    fn distance(&self, p: [f32; 3]) -> f32 {
        let (x, z) = (((p[0] - self.x0) / 0.5).floor(), ((p[2] - self.z0) / 0.5).floor());
        if x < 0.0 || z < 0.0 || x as usize >= self.w || z as usize >= self.h {
            return f32::INFINITY;
        }
        self.d[z as usize * self.w + x as usize]
    }
}

/// A top view: height-shaded collider (walls darker), the path blue, falls red, blocks orange.
fn top_view(path: &Path, m: &Mesh, feet: &[[f32; 3]], walked: &walk::Report) -> anyhow::Result<()> {
    if m.positions.is_empty() {
        return Ok(());
    }
    let lo: [f32; 3] = std::array::from_fn(|k| m.positions.iter().map(|p| p[k]).fold(f32::INFINITY, f32::min));
    let hi: [f32; 3] = std::array::from_fn(|k| m.positions.iter().map(|p| p[k]).fold(f32::NEG_INFINITY, f32::max));
    let scale = (1600.0 / (hi[0] - lo[0]).max(hi[2] - lo[2]).max(1.0)).min(40.0);
    let (w, h) = (((hi[0] - lo[0]) * scale) as usize + 1, ((hi[2] - lo[2]) * scale) as usize + 1);
    let mut top = vec![f32::NEG_INFINITY; w * h];
    let mut rgb = vec![25u8; w * h * 3];
    // Image x = world x mirrored (a map seen from above in Unity, +Z up the image).
    let px = |p: [f32; 3]| (((hi[0] - p[0]) * scale), ((hi[2] - p[2]) * scale));
    for t in &m.triangles {
        let p = t.map(|v| m.positions[v as usize]);
        let n = mesh::normal(p);
        let s = p.map(px);
        let (x0, x1) = (s.iter().map(|q| q.0).fold(f32::INFINITY, f32::min).floor().max(0.) as usize, (s.iter().map(|q| q.0).fold(f32::NEG_INFINITY, f32::max).ceil() as usize).min(w - 1));
        let (y0, y1) = (s.iter().map(|q| q.1).fold(f32::INFINITY, f32::min).floor().max(0.) as usize, (s.iter().map(|q| q.1).fold(f32::NEG_INFINITY, f32::max).ceil() as usize).min(h - 1));
        let area = (s[1].0 - s[0].0) * (s[2].1 - s[0].1) - (s[2].0 - s[0].0) * (s[1].1 - s[0].1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let a = ((s[1].0 - fx) * (s[2].1 - fy) - (s[2].0 - fx) * (s[1].1 - fy)) / area;
                let b = ((s[2].0 - fx) * (s[0].1 - fy) - (s[0].0 - fx) * (s[2].1 - fy)) / area;
                let c = 1. - a - b;
                let inside = area.abs() > 1e-9 && a >= 0. && b >= 0. && c >= 0.;
                // Walls (nearly vertical in the top view) get at least their outline.
                let near = area.abs() <= 1e-9 || !inside;
                if near && !(n[1].abs() < 0.7 && seg_dist(s, fx, fy) < 0.8) {
                    continue;
                }
                let y_here = if inside { a * p[0][1] + b * p[1][1] + c * p[2][1] } else { p.iter().map(|q| q[1]).fold(f32::NEG_INFINITY, f32::max) };
                let at = y * w + x;
                if y_here > top[at] {
                    top[at] = y_here;
                    let v = ((y_here - lo[1]) / (hi[1] - lo[1]).max(0.1)).clamp(0., 1.);
                    let col = if n[1].abs() >= 0.7 { [0.3 + 0.6 * v, 0.35 + 0.6 * v, 0.3 + 0.5 * v] } else { [0.55, 0.3, 0.15] };
                    rgb[at * 3..at * 3 + 3].copy_from_slice(&col.map(|c| (c * 255.) as u8));
                }
            }
        }
    }
    let mut dot = |p: [f32; 3], col: [u8; 3], r: i64| {
        let (x, y) = px(p);
        for dy in -r..=r {
            for dx in -r..=r {
                let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h {
                    let at = yy as usize * w + xx as usize;
                    rgb[at * 3..at * 3 + 3].copy_from_slice(&col);
                }
            }
        }
    };
    for p in feet {
        dot(*p, [60, 120, 255], 1);
    }
    for p in &walked.fall_at {
        dot(*p, [255, 30, 30], 5);
    }
    for p in &walked.block_at {
        dot(*p, [255, 150, 0], 5);
    }
    write_png(path, w as u32, h as u32, &rgb)
}

fn seg_dist(s: [(f32, f32); 3], x: f32, y: f32) -> f32 {
    (0..3)
        .map(|k| {
            let (a, b) = (s[k], s[(k + 1) % 3]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let l = dx * dx + dy * dy;
            let t = if l > 0. { (((x - a.0) * dx + (y - a.1) * dy) / l).clamp(0., 1.) } else { 0. };
            ((a.0 + t * dx - x).powi(2) + (a.1 + t * dy - y).powi(2)).sqrt()
        })
        .fold(f32::INFINITY, f32::min)
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(a.len() >= 2, "mesh_bench <session> <out> [--gt DIR] [--splats PLY] [--voxel 0.04,0.05] [--radius M] [--no-fill] [--quest] [--label NAME] [--reference NAME=GLB]");
    let session = Path::new(&a[0]);
    let out = Path::new(&a[1]);
    std::fs::create_dir_all(out)?;
    let (mut gt, mut splats, mut voxels, mut label, mut references) = (None, None, vec![0.04f32], String::from("v2"), Vec::new());
    let mut base = Settings::default();
    let mut k = 2;
    while k < a.len() {
        let value = || a.get(k + 1).cloned().context("missing value");
        match a[k].as_str() {
            "--gt" => gt = Some(PathBuf::from(value()?)),
            "--splats" => splats = Some(PathBuf::from(value()?)),
            "--voxel" => voxels = value()?.split(',').map(|v| v.parse()).collect::<Result<_, _>>()?,
            "--radius" => base.radius = value()?.parse()?,
            "--error" => base.error = value()?.parse()?,
            "--max-depth" => base.max_depth = value()?.parse()?,
            "--quant-trunc" => base.quant_trunc = value()?.parse()?,
            "--label" => label = value()?,
            "--reference" => references.push(value()?),
            "--no-fill" => {
                base.fill = false;
                k -= 1
            }
            "--quest" => {
                base.budget = mesh::QUEST_BUDGET;
                k -= 1
            }
            other => anyhow::bail!("unknown option {other}"),
        }
        k += 2;
    }
    let load = std::time::Instant::now();
    let p = prepare::prepare(session, &prepare::Options { width: 640, aberration: false, ..Default::default() }, |_| {})?;
    let loading = load.elapsed().as_secs_f64();
    for s in p.describe() {
        eprintln!("{s}");
    }
    let views: Vec<View> = match &gt {
        Some(g) => std::fs::read_to_string(g.join("views.jsonl"))?.lines().map(serde_json::from_str).collect::<Result<_, _>>()?,
        None => vec![],
    };
    let g = splats.as_ref().map(|s| ply::read(s)).transpose()?;
    let inputs = Inputs { session, out, gt: gt.as_deref(), views: &views, data: &p.data, splats: g.as_ref() };
    let mut first: Option<(Roi, Vec<[f32; 3]>, f32)> = None;
    let mut summary = Vec::new();
    for &voxel in &voxels {
        let s = Settings { voxel, ..base.clone() };
        let name = format!("{label}-{:.0}cm", voxel * 100.);
        let built = mesh::build(session, &p.data, &s)?;
        let file = out.join(format!("{name}.glb"));
        let write = std::time::Instant::now();
        built.mesh.write(&file)?;
        let write_seconds = write.elapsed().as_secs_f64();
        let imported = Mesh::read_glb(&file, true)?;
        anyhow::ensure!(imported == built.mesh, "the GLB does not import back to the captured world exactly");
        let mut result = evaluate(&inputs, &name, &imported, &built.roi, Some(&built.volume), &built.path.feet, built.path.eye_height)?;
        result["build"] = built.stats.clone();
        result["loading_seconds"] = loading.into();
        result["write_seconds"] = write_seconds.into();
        result["bytes"] = std::fs::metadata(&file)?.len().into();
        result["volume_bytes"] = built.volume.bytes().into();
        result["settings"] = format!("{s:?}").into();
        std::fs::write(out.join(format!("{name}.json")), serde_json::to_vec_pretty(&result)?)?;
        let line = serde_json::json!({
            "name": name, "seconds": built.stats["seconds"], "triangles": result["triangles"], "bytes": result["bytes"],
            "reachable": result["reachable_pixels"], "all": result["all_pixels"], "observed": result["observed_reachable_pixels"],
            "walk_supported": result["walk"]["supported"], "walk_blocked": result["walk"]["blocked"], "falls": result["walk"]["falls"], "blocks": result["walk"]["blocks"],
            "single_floor": result["keyframe_single_floor_support"], "audit": result["audit"],
        });
        println!("{line}");
        summary.push(line);
        if first.is_none() {
            first = Some((built.roi.clone(), built.path.feet.clone(), built.path.eye_height));
        }
    }
    if let Some((roi, feet, eye_height)) = &first {
        for r in &references {
            let (name, file) = r.split_once('=').context("--reference NAME=FILE.glb")?;
            let m = Mesh::read_glb(Path::new(file), true)?;
            let mut result = evaluate(&inputs, name, &m, roi, None, feet, *eye_height)?;
            result["bytes"] = std::fs::metadata(file)?.len().into();
            std::fs::write(out.join(format!("{name}.json")), serde_json::to_vec_pretty(&result)?)?;
            let line = serde_json::json!({"name": name, "triangles": result["triangles"], "reachable": result["reachable_pixels"], "all": result["all_pixels"], "walk_supported": result["walk"]["supported"], "walk_blocked": result["walk"]["blocked"], "falls": result["walk"]["falls"], "blocks": result["walk"]["blocks"], "single_floor": result["keyframe_single_floor_support"], "audit": result["audit"]});
            println!("{line}");
            summary.push(line);
        }
    }
    std::fs::write(out.join("summary.json"), serde_json::to_vec_pretty(&summary)?)?;
    Ok(())
}
