//! The walked path and the region a person can reach from it.
//!
//! The path: the avatar's hips from the capture kit's skeleton frames when the session has them (the person, wherever
//! the camera was), their feet [`crate::ground::HIPS_PER_EYE_HEIGHT`] eye heights below; otherwise every recorded
//! camera pose in time order, taken as the eyes, the feet one eye height below (VRChat's `EyeHeightAsMeters` when the
//! session recorded it, else [`crate::ground::EYE_HEIGHT`]).
//!
//! The region: within `radius` metres horizontally of a point of the path, from `below` metres under the lowest feet to
//! `above` metres over the highest. On a 1 m grid of columns: the plaza's 40 m path and 40 m radius make about
//! 7 000 columns, a negligible test per sample.

use crate::{camera::Camera, dataset::Dataset, ground};
use orbiters_contracts::{frame_record::record_flags, performance};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct WalkPath {
    /// Where the walker's feet were, in time order (Unity world, metres).
    pub feet: Vec<[f32; 3]>,
    pub eye_height: f32,
    /// "hips", "camera" or "keyframes".
    pub source: &'static str,
}

/// The walked path of a session (see the module's comment).
pub fn walked(session: &Path, data: &Dataset) -> WalkPath {
    let eye_height = ground::eye_height(session, None).unwrap_or(ground::EYE_HEIGHT);
    if let Some(frames) = std::fs::read(session.join(performance::SKELETON_FILE)).ok().and_then(|b| performance::skeleton_frames::read(&b).ok()) {
        let mut frames: Vec<_> = frames.into_iter().filter(|f| f.hips.iter().all(|v| v.is_finite())).collect();
        frames.sort_by_key(|f| f.time_ns);
        if frames.len() >= 2 {
            let below = ground::HIPS_PER_EYE_HEIGHT * eye_height;
            return WalkPath { feet: frames.iter().map(|f| [f.hips[0], f.hips[1] - below, f.hips[2]]).collect(), eye_height, source: "hips" };
        }
    }
    let mut records: Vec<_> = orbiters_core::bundle::read_records(session)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.flags & record_flags::STRIP_OK != 0 && r.width > 0 && r.height > 0 && r.camera_to_world.iter().all(|v| v.is_finite()))
        .collect();
    records.sort_by_key(|r| r.capture_time_ns);
    let mut eyes: Vec<[f32; 3]> = records.iter().map(|r| Camera::from_strip(&r.camera_to_world, &r.projection, r.width, r.height, 1.0).position()).collect();
    let mut source = "camera";
    if eyes.is_empty() {
        let mut views: Vec<_> = data.views.iter().collect();
        views.sort_by(|a, b| a.game_time.total_cmp(&b.game_time));
        eyes = views.iter().map(|v| v.camera.position()).collect();
        source = "keyframes";
    }
    WalkPath { feet: eyes.iter().map(|e| [e[0], e[1] - eye_height, e[2]]).collect(), eye_height, source }
}

impl WalkPath {
    /// The path resampled every `spacing` metres horizontally: the first point, then points interpolated along it each
    /// `spacing` from the last kept, so a sparsely recorded walk leaves no gaps. A jump of more than [`TELEPORT`] between
    /// two recorded points (a portal, a respawn) is not walked: nothing is interpolated across it.
    pub fn resampled(&self, spacing: f32) -> Vec<[f32; 3]> {
        let mut out: Vec<[f32; 3]> = Vec::new();
        let Some(&first) = self.feet.first() else { return out };
        out.push(first);
        let mut since = 0.0f32;
        for w in self.feet.windows(2) {
            let d = ((w[1][0] - w[0][0]).powi(2) + (w[1][2] - w[0][2]).powi(2)).sqrt();
            if d > TELEPORT {
                out.push(w[1]);
                since = 0.0;
                continue;
            }
            if d <= 0.0 {
                continue;
            }
            let mut along = spacing - since;
            while along <= d {
                let t = along / d;
                out.push(std::array::from_fn(|i| w[0][i] + t * (w[1][i] - w[0][i])));
                along += spacing;
            }
            since = d - (along - spacing);
        }
        out
    }
}

/// Farther than this between two recorded points (metres) is a teleport, not a walk.
pub const TELEPORT: f32 = 2.0;

/// The reachable region (see the module's comment).
#[derive(Clone, Debug)]
pub struct Roi {
    pub cell: f32,
    pub x0: i32,
    pub z0: i32,
    pub w: usize,
    pub h: usize,
    pub mask: Vec<bool>,
    pub y_min: f32,
    pub y_max: f32,
}

impl Roi {
    pub fn new(path: &WalkPath, radius: f32, below: f32, above: f32) -> Roi {
        let cell = 1.0f32;
        let points = path.resampled(0.5);
        let lo = |i: usize| points.iter().map(|p| p[i]).fold(f32::INFINITY, f32::min);
        let hi = |i: usize| points.iter().map(|p| p[i]).fold(f32::NEG_INFINITY, f32::max);
        let x0 = ((lo(0) - radius) / cell).floor() as i32 - 1;
        let z0 = ((lo(2) - radius) / cell).floor() as i32 - 1;
        let w = (((hi(0) + radius) / cell).ceil() as i32 + 2 - x0).max(1) as usize;
        let h = (((hi(2) + radius) / cell).ceil() as i32 + 2 - z0).max(1) as usize;
        let mut mask = vec![false; w * h];
        let r = (radius / cell).ceil() as i32 + 1;
        for p in &points {
            let cx = (p[0] / cell).floor() as i32 - x0;
            let cz = (p[2] / cell).floor() as i32 - z0;
            for dz in -r..=r {
                for dx in -r..=r {
                    let (x, z) = (cx + dx, cz + dz);
                    if x < 0 || z < 0 || x >= w as i32 || z >= h as i32 {
                        continue;
                    }
                    let centre = [(x + x0) as f32 * cell + 0.5 * cell, (z + z0) as f32 * cell + 0.5 * cell];
                    if (centre[0] - p[0]).powi(2) + (centre[1] - p[2]).powi(2) <= radius * radius {
                        mask[z as usize * w + x as usize] = true;
                    }
                }
            }
        }
        Roi { cell, x0, z0, w, h, mask, y_min: lo(1) - below, y_max: hi(1) + above }
    }

    pub fn contains(&self, p: [f32; 3]) -> bool {
        if !(p[1] >= self.y_min && p[1] <= self.y_max) {
            return false;
        }
        let x = (p[0] / self.cell).floor() as i64 - self.x0 as i64;
        let z = (p[2] / self.cell).floor() as i64 - self.z0 as i64;
        x >= 0 && z >= 0 && (x as usize) < self.w && (z as usize) < self.h && self.mask[z as usize * self.w + x as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_follows_the_path() {
        // Walked from x = 0 to 10 in 1.5 m steps, then teleported to x = 40.
        let mut feet: Vec<[f32; 3]> = (0..=6).map(|k| [k as f32 * 1.5 + 1.0, 0., 0.]).collect();
        feet.insert(0, [0., 0., 0.]);
        feet.push([40., 0., 0.]);
        let path = WalkPath { feet, eye_height: 1.6, source: "test" };
        let roi = Roi::new(&path, 5.0, 1.0, 3.0);
        assert!(roi.contains([0., 0., 0.]) && roi.contains([10., 2.9, 4.]) && roi.contains([5., -0.9, 4.]));
        assert!(!roi.contains([5., 0., 6.]), "beyond the radius");
        assert!(!roi.contains([5., 3.5, 0.]) && !roi.contains([5., -1.5, 0.]), "outside the height band");
        assert!(!roi.contains([-7., 0., 0.]));
        assert!(roi.contains([40., 0., 4.]) && !roi.contains([25., 0., 0.]), "a teleport is not walked");
        let r = path.resampled(0.5);
        assert!(r.windows(2).all(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][2] - w[0][2]).powi(2)).sqrt() <= 0.5 + 1e-4 || w[1][0] == 40.0));
    }
}
