//! Seeding: every exact depth sample becomes a flat disc on the surface it hit (the plan's spawning rule): normal
//! from the neighbouring samples, size from the sample's footprint, colour from the keyframes nearest in time that
//! see it. Samples closer than a disc to one already kept are merged away. Samples that hit nothing (the sky) become
//! discs facing the camera on a far shell.

use crate::camera::{dot, Camera};
use crate::dataset::{sample_rgb, Dataset};
use crate::gaussians::{logit, quat_from_z, Gaussians};
use std::collections::HashMap;

/// Distance of the sky shell, well inside the far plane of the sort keys.
const SKY: f32 = 400.0;

/// Seeds at most `budget` Gaussians (a random subset when there are more).
pub fn from_depth(data: &Dataset, budget: usize) -> Gaussians {
    let mut g = Gaussians::default();
    let mut seen: HashMap<(i32, i32, i32, i32), ()> = HashMap::new();
    let opacity = logit(0.6);
    for dv in &data.depth_views {
        let (gw, gh) = (dv.grid_width as i32, dv.grid_height as i32);
        let world = |gx: i32, gy: i32| -> Option<[f32; 3]> {
            if gx < 0 || gy < 0 || gx >= gw || gy >= gh {
                return None;
            }
            let i = (gy * gw + gx) as usize;
            let z = dv.depth[i];
            (z > 0.0).then(|| dv.camera.unproject(dv.pixels[i][0], dv.pixels[i][1], z))
        };
        let cam_pos = dv.camera.position();
        for gy in 0..gh {
            for gx in 0..gw {
                let i = (gy * gw + gx) as usize;
                let (u, v) = (dv.pixels[i][0], dv.pixels[i][1]);
                let z = dv.depth[i];
                let sky = z <= 0.0;
                if sky && !in_scene(data, u, v) {
                    continue;
                }
                let p = if sky { add(cam_pos, scale(normalize(sub(dv.camera.unproject(u, v, 1.0), cam_pos)), SKY)) } else { dv.camera.unproject(u, v, z) };
                let footprint = if sky { SKY } else { z } * dv.spacing / dv.camera.fx;
                // Merge: one disc per cell of about its own size (levels in powers of two, so sizes stay comparable).
                let level = footprint.max(1e-3).log2().round() as i32;
                let cell = 2f32.powi(level);
                let key = ((p[0] / cell).floor() as i32, (p[1] / cell).floor() as i32, (p[2] / cell).floor() as i32, level);
                if seen.insert(key, ()).is_some() {
                    continue;
                }
                let view = normalize(sub(cam_pos, p));
                let normal = if sky { None } else { surface_normal(&world, gx, gy, p, z) }.map(|n| if dot(n, view) < 0.0 { neg(n) } else { n }).unwrap_or(view);
                let color = color_at(data, p, dv.game_time).unwrap_or([0.5, 0.5, 0.5]);
                let s = (footprint * 0.75).max(1e-3).ln();
                g.push(p, [s, s, (footprint * 0.1).max(1e-4).ln()], quat_from_z(normal), opacity, color);
            }
        }
    }
    if g.len() > budget {
        let mut rng = fastrand::Rng::with_seed(7);
        let keep_p = budget as f64 / g.len() as f64;
        let keep: Vec<bool> = (0..g.len()).map(|_| rng.f64() < keep_p).collect();
        g.retain(&keep);
    }
    g
}

/// Whether a training pixel shows the world (not the strip or the skeleton band).
fn in_scene(data: &Dataset, u: f32, v: f32) -> bool {
    let (x, y) = (u.round(), v.round());
    x >= 0.0 && y >= 0.0 && (x as u32) < data.width && (y as u32) < data.height && data.mask[(y as u32 * data.width + x as u32) as usize] > 0.5
}

/// Normal from the neighbouring samples, unless they sit across a depth edge.
fn surface_normal(world: &impl Fn(i32, i32) -> Option<[f32; 3]>, gx: i32, gy: i32, p: [f32; 3], z: f32) -> Option<[f32; 3]> {
    let near = |q: [f32; 3]| (dot(sub(q, p), sub(q, p))).sqrt() < 0.15 * z;
    let pick = |a: Option<[f32; 3]>, b: Option<[f32; 3]>| match (a.filter(|q| near(*q)), b.filter(|q| near(*q))) {
        (Some(a), Some(b)) => Some(sub(a, b)),
        (Some(a), None) => Some(sub(a, p)),
        (None, Some(b)) => Some(sub(p, b)),
        (None, None) => None,
    };
    let du = pick(world(gx + 1, gy), world(gx - 1, gy))?;
    let dv = pick(world(gx, gy + 1), world(gx, gy - 1))?;
    let n = cross(du, dv);
    let len = dot(n, n).sqrt();
    (len > 1e-12).then(|| n.map(|c| c / len))
}

/// Colour of a world point in the keyframes nearest in time to `time` that see it.
fn color_at(data: &Dataset, p: [f32; 3], time: f32) -> Option<[f32; 3]> {
    let mut order: Vec<usize> = (0..data.views.len()).collect();
    order.sort_by(|&a, &b| (data.views[a].game_time - time).abs().total_cmp(&(data.views[b].game_time - time).abs()));
    for &i in order.iter().take(4) {
        let v = &data.views[i];
        let cam: &Camera = &v.camera;
        if let Some((u, vv, _)) = cam.project(p) {
            let (x, y) = (u.round() as i64, vv.round() as i64);
            if x >= 0 && y >= 0 && (x as u32) < data.width && (y as u32) < data.height && data.mask[(y as u32 * data.width + x as u32) as usize] > 0.5 {
                if let Some(c) = sample_rgb(&v.image, data.width, data.height, u, vv) {
                    return Some(c);
                }
            }
        }
    }
    None
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    a.map(|c| c * s)
}

fn neg(a: [f32; 3]) -> [f32; 3] {
    [-a[0], -a[1], -a[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt().max(1e-12);
    a.map(|c| c / l)
}
