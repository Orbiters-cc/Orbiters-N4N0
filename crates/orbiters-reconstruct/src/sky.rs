//! Lab: a sky cubemap behind the splats (gpu.rs `set_sky`, shaders/sky_lookup.wgsl) instead of a shell of discs
//! 400 m out, which shows ~24 px of parallax over a 10 m walk and never densifies. Six faces of N×N RGB texels, looked
//! up by world direction; seeded in closed form from the keyframe pixels whose rays hit nothing in the depth frame
//! nearest in time (exact under exact rotations), then trained with the splats.

use crate::dataset::{sample_rgb, Dataset};
use crate::transients::{grid, known};

/// Cube face (0 +x, 1 −x, 2 +y, 3 −y, 4 +z, 5 −z) and face coordinates in −1..1 of a direction: the mapping of
/// sky_lookup.wgsl's `sky_taps`.
pub fn face_uv(d: [f32; 3]) -> (usize, f32, f32) {
    let a = d.map(f32::abs);
    if a[0] >= a[1] && a[0] >= a[2] {
        if d[0] > 0.0 {
            (0, -d[2] / a[0], -d[1] / a[0])
        } else {
            (1, d[2] / a[0], -d[1] / a[0])
        }
    } else if a[1] >= a[2] {
        if d[1] > 0.0 {
            (2, d[0] / a[1], d[2] / a[1])
        } else {
            (3, d[0] / a[1], -d[2] / a[1])
        }
    } else if d[2] > 0.0 {
        (4, d[0] / a[2], -d[1] / a[2])
    } else {
        (5, -d[0] / a[2], -d[1] / a[2])
    }
}

/// The texel nearest to a direction.
pub fn texel(d: [f32; 3], n: usize) -> usize {
    let (face, u, v) = face_uv(d);
    let at = |c: f32| (((c + 1.0) * 0.5 * n as f32 - 0.5).round().max(0.0) as usize).min(n - 1);
    face * n * n + at(v) * n + at(u)
}

/// Texels (rgb, 0) averaged from every second keyframe pixel that sees the sky; texels no keyframe saw get the mean.
pub fn seed(data: &Dataset, n: usize) -> Vec<[f32; 4]> {
    let mut sum = vec![[0f64; 4]; 6 * n * n];
    let (w, h) = (data.width, data.height);
    for v in &data.views {
        let Some(d) = data.depth_views.iter().min_by(|a, b| (a.game_time - v.game_time).abs().total_cmp(&(b.game_time - v.game_time).abs())) else { continue };
        let Some(g) = grid(d) else { continue };
        let origin = v.camera.position();
        for y in (0..h).step_by(2) {
            for x in (0..w).step_by(2) {
                if data.mask[(y * w + x) as usize] < 0.5 {
                    continue;
                }
                // Far along the ray, the depth frame (a few centimetres away) sees the same direction.
                let far = v.camera.unproject(x as f32, y as f32, 1e4);
                let Some((u, vv, _)) = d.camera.project(far) else { continue };
                let (fx, fy) = ((u - g.x0) / g.sx, (vv - g.y0) / g.sy);
                if !(fx >= 0.0 && fy >= 0.0) || fx > (g.gw - 1) as f32 || fy > (g.gh - 1) as f32 {
                    continue;
                }
                let (i0, j0) = ((fx as usize).min(g.gw - 2), (fy as usize).min(g.gh - 2));
                let sky = [(0, 0), (1, 0), (0, 1), (1, 1)].iter().all(|&(di, dj)| {
                    let k = (j0 + dj) * g.gw + i0 + di;
                    known(d, k, &data.mask, w, h) && d.depth[k] <= 0.0
                });
                if !sky {
                    continue;
                }
                let Some(c) = sample_rgb(&v.image, w, h, x as f32, y as f32) else { continue };
                let dir = [far[0] - origin[0], far[1] - origin[1], far[2] - origin[2]];
                let t = &mut sum[texel(dir, n)];
                for k in 0..3 {
                    t[k] += c[k] as f64;
                }
                t[3] += 1.0;
            }
        }
    }
    let seen: Vec<&[f64; 4]> = sum.iter().filter(|t| t[3] > 0.0).collect();
    let mean = if seen.is_empty() { [0.0; 3] } else { [0, 1, 2].map(|k| seen.iter().map(|t| t[k] / t[3]).sum::<f64>() / seen.len() as f64) };
    sum.iter().map(|t| if t[3] > 0.0 { [(t[0] / t[3]) as f32, (t[1] / t[3]) as f32, (t[2] / t[3]) as f32, 0.0] } else { [mean[0] as f32, mean[1] as f32, mean[2] as f32, 0.0] }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_direction_lands_inside_its_face() {
        let mut rng = fastrand::Rng::with_seed(5);
        for _ in 0..10_000 {
            let d = [rng.f32() * 2.0 - 1.0, rng.f32() * 2.0 - 1.0, rng.f32() * 2.0 - 1.0];
            let (face, u, v) = face_uv(d);
            assert!(face < 6 && (-1.0..=1.0).contains(&u) && (-1.0..=1.0).contains(&v), "{d:?} → {face} {u} {v}");
        }
        // The six axes land in the middle of six different faces.
        let axes = [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0]];
        let faces: Vec<usize> = axes.iter().map(|a| face_uv(*a).0).collect();
        assert_eq!(faces, vec![0, 1, 2, 3, 4, 5]);
    }
}
