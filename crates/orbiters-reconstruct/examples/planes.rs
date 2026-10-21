//! Lab: the dominant planes of a session's exact depth. Fits planes to all depth frames' world points by RANSAC (3 cm
//! inliers), largest first, and prints each plane's normal, its height below the camera path when horizontal, its
//! share of the depth points, and how many keyframe pixels without depth look at it (rays that meet the plane inside
//! its footprint before any measured surface) — the pixels a plane prior could give depth to.
//!
//! ```text
//! cargo run --release --example planes -- <session> [planes]
//! ```

use anyhow::Context as _;
use orbiters_reconstruct::dataset;
use std::path::PathBuf;

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let session = PathBuf::from(it.next().context("usage: planes <session> [planes]")?);
    let count: usize = it.next().map(|s| s.parse()).transpose()?.unwrap_or(4);
    let data = dataset::load(&session, 1920, |_| {})?;
    let mut pts: Vec<V3> = Vec::new();
    for d in &data.depth_views {
        for (i, &z) in d.depth.iter().enumerate().step_by(3) {
            if z > 0.0 && z < 80.0 {
                let p = d.camera.unproject(d.pixels[i][0], d.pixels[i][1], z);
                pts.push([p[0] as f64, p[1] as f64, p[2] as f64]);
            }
        }
    }
    let cams: Vec<V3> = data.views.iter().map(|v| v.camera.position().map(f64::from)).collect();
    let cam_y = cams.iter().map(|c| c[1]).sum::<f64>() / cams.len().max(1) as f64;
    println!("{} depth points (every third sample), camera mean height {cam_y:.2} m", pts.len());
    let mut rng = fastrand::Rng::with_seed(9);
    let mut left: Vec<V3> = pts.clone();
    let total = pts.len() as f64;
    for k in 0..count {
        if left.len() < 1000 {
            break;
        }
        let mut best = (0usize, [0.0; 3], 0.0);
        for _ in 0..3000 {
            let (a, b, c) = (left[rng.usize(..left.len())], left[rng.usize(..left.len())], left[rng.usize(..left.len())]);
            let n = cross(sub(b, a), sub(c, a));
            let len = dot(n, n).sqrt();
            if len < 1e-9 {
                continue;
            }
            let n = n.map(|x| x / len);
            let off = dot(n, a);
            let inl = left.iter().step_by(7).filter(|p| (dot(n, **p) - off).abs() < 0.03).count();
            if inl > best.0 {
                best = (inl, n, off);
            }
        }
        let (_, mut n, mut off) = best;
        if n[1] < 0.0 {
            n = n.map(|x| -x);
            off = -off;
        }
        let (inside, rest): (Vec<V3>, Vec<V3>) = left.iter().partition(|p| (dot(n, **p) - off).abs() < 0.03);
        let tilt = n[1].clamp(-1.0, 1.0).acos().to_degrees();
        // Footprint: the inliers' bounding box in the plane's own axes.
        let ax = if n[1].abs() < 0.9 { cross(n, [0.0, 1.0, 0.0]) } else { cross(n, [1.0, 0.0, 0.0]) };
        let ax = ax.map(|x| x / dot(ax, ax).sqrt());
        let ay = cross(n, ax);
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for p in &inside {
            let (u, v) = (dot(*p, ax), dot(*p, ay));
            lo = [lo[0].min(u), lo[1].min(v)];
            hi = [hi[0].max(u), hi[1].max(v)];
        }
        // Keyframe pixels without depth (nearest depth frame) whose rays meet the plane inside its footprint.
        let (mut covered, mut no_depth) = (0usize, 0usize);
        for v in data.views.iter().step_by(4) {
            let Some(d) = data.depth_views.iter().min_by(|a, b| (a.game_time - v.game_time).abs().total_cmp(&(b.game_time - v.game_time).abs())) else { continue };
            let o = v.camera.position().map(f64::from);
            for (i, &z) in d.depth.iter().enumerate().step_by(5) {
                if z > 0.0 {
                    continue;
                }
                let far = d.camera.unproject(d.pixels[i][0], d.pixels[i][1], 100.0).map(f64::from);
                let dir = sub(far, o);
                let denom = dot(n, dir);
                if denom.abs() < 1e-9 {
                    continue;
                }
                no_depth += 1;
                let t = (off - dot(n, o)) / denom;
                if t <= 0.0 {
                    continue;
                }
                let hit = [o[0] + t * dir[0], o[1] + t * dir[1], o[2] + t * dir[2]];
                let (u, w) = (dot(hit, ax), dot(hit, ay));
                if u >= lo[0] && u <= hi[0] && w >= lo[1] && w <= hi[1] {
                    covered += 1;
                }
            }
        }
        println!(
            "plane {k}: normal ({:.2}, {:.2}, {:.2}), {tilt:.1}° from up, {} of the points ({:.1} %){}; footprint {:.1} × {:.1} m; {:.1} % of sampled no-depth rays meet it inside that footprint",
            n[0],
            n[1],
            n[2],
            inside.len(),
            100.0 * inside.len() as f64 / total,
            if tilt < 10.0 { format!(", {:.2} m below the mean camera", cam_y - off / n[1]) } else { String::new() },
            hi[0] - lo[0],
            hi[1] - lo[1],
            100.0 * covered as f64 / no_depth.max(1) as f64
        );
        left = rest;
    }
    Ok(())
}
