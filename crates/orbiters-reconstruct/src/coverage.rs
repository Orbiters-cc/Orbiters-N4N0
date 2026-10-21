//! Lab: which keyframes to keep when fewer are wanted. Each keyframe sees a set of surface cells — 5 cm voxels of the
//! exact depth points of its nearest depth frame, each with the octant of the viewing direction — counted only where
//! the keyframe resolves them (a sample footprint of at most two voxels). Greedy set cover keeps, one at a time, the
//! keyframe that adds the most cells nobody kept sees yet. A recorder could use the same count live to decide when a
//! frame is worth keeping.

use crate::dataset::Dataset;
use std::collections::HashSet;

const VOXEL: f32 = 0.05;

/// The cells keyframe `v` sees (voxel coordinates and a viewing-direction octant).
fn cells(data: &Dataset, v: usize) -> HashSet<(i32, i32, i32, u8)> {
    let view = &data.views[v];
    let mut out = HashSet::new();
    let Some(d) = data.depth_views.iter().min_by(|a, b| (a.game_time - view.game_time).abs().total_cmp(&(b.game_time - view.game_time).abs())) else { return out };
    let eye = view.camera.position();
    for (i, &z) in d.depth.iter().enumerate() {
        if z <= 0.0 || d.is_dynamic(i) {
            continue;
        }
        // Resolved: the sample's footprint is at most two voxels.
        if z * d.spacing / d.camera.fx > 2.0 * VOXEL {
            continue;
        }
        let p = d.camera.unproject(d.pixels[i][0], d.pixels[i][1], z);
        let to_eye = [eye[0] - p[0], eye[1] - p[1], eye[2] - p[2]];
        let octant = u8::from(to_eye[0] > 0.0) | (u8::from(to_eye[1] > 0.0) << 1) | (u8::from(to_eye[2] > 0.0) << 2);
        out.insert(((p[0] / VOXEL).floor() as i32, (p[1] / VOXEL).floor() as i32, (p[2] / VOXEL).floor() as i32, octant));
    }
    out
}

/// `k` of the `candidates` (keyframe indices), most new coverage first; returned in keyframe order.
pub fn select(data: &Dataset, candidates: &[usize], k: usize) -> Vec<usize> {
    use rayon::prelude::*;
    let sets: Vec<HashSet<(i32, i32, i32, u8)>> = candidates.par_iter().map(|&v| cells(data, v)).collect();
    let mut seen: HashSet<(i32, i32, i32, u8)> = HashSet::new();
    let mut taken = vec![false; candidates.len()];
    let mut picked = Vec::with_capacity(k);
    for _ in 0..k.min(candidates.len()) {
        let best = (0..candidates.len()).filter(|&c| !taken[c]).max_by_key(|&c| sets[c].iter().filter(|cell| !seen.contains(*cell)).count());
        let Some(c) = best else { break };
        taken[c] = true;
        seen.extend(sets[c].iter().copied());
        picked.push(candidates[c]);
    }
    picked.sort_unstable();
    picked
}
