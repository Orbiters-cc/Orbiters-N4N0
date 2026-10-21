//! Lab: depth for surfaces where the depth frames lost it. A world whose post-processing breaks the depth symbols
//! towards the frame's edges (crate::depthfix marks those samples unknown) still shows the same floor there. Per depth
//! frame: the dominant plane of its known samples (RANSAC), extended to the unknown samples whose rays meet it in front
//! of the camera, on rows where the frame measured that plane, and whose colour in the keyframe nearest in time matches
//! the plane's measured part. A candidate is kept only when other depth frames confirm it: at least one measured a
//! surface at that point, and none saw through it (crate::transients' verdict; a frame that measured nothing there,
//! beyond a platform's edge, sees through). Filled samples become ordinary depth: seeds, depth targets, free-space
//! evidence.

use crate::dataset::{sample_rgb, Dataset};
use crate::transients::{grid, verdict, Params};
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct FillParams {
    pub iterations: usize,
    /// A sample lies on the plane within max(`tolerance_abs`, `tolerance_rel` × its depth) metres.
    pub tolerance_abs: f32,
    pub tolerance_rel: f32,
    /// The plane must hold this many known samples, and this share of them.
    pub min_inliers: usize,
    pub min_share: f32,
    /// Grid rows away from a measured row of the plane that may be filled.
    pub row_reach: usize,
    pub min_depth: f32,
    pub max_depth: f32,
    /// Mean absolute difference (sRGB, 0..1) from the median colour of the plane's measured samples.
    pub color_tolerance: f32,
    /// Check candidates against every n-th depth frame.
    pub check_stride: usize,
}

impl Default for FillParams {
    fn default() -> Self {
        Self { iterations: 300, tolerance_abs: 0.03, tolerance_rel: 0.01, min_inliers: 300, min_share: 0.15, row_reach: 2, min_depth: 0.3, max_depth: 60.0, color_tolerance: 0.08, check_stride: 2 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct FillStats {
    pub frames: usize,
    pub frames_with_plane: usize,
    pub unknown: usize,
    pub candidates: usize,
    /// Candidates no other frame could check (kept unknown).
    pub unconfirmed: usize,
    /// Candidates another frame saw through (kept unknown).
    pub contradicted: usize,
    pub filled: usize,
}

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot3(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// Candidates of one depth frame: (sample, eye depth, world point).
fn candidates(data: &Dataset, f: usize, p: &FillParams) -> (bool, Vec<(usize, f32, V3)>) {
    let d = &data.depth_views[f];
    let n = d.depth.len();
    if d.dynamic.len() != n || !d.dynamic.iter().any(|x| *x) {
        return (false, Vec::new());
    }
    let known: Vec<usize> = (0..n).filter(|&i| d.depth[i] > 0.0 && !d.dynamic[i]).collect();
    if known.len() < p.min_inliers {
        return (false, Vec::new());
    }
    let pts: Vec<V3> = known.iter().map(|&i| d.camera.unproject(d.pixels[i][0], d.pixels[i][1], d.depth[i])).collect();
    let tol = |k: usize| p.tolerance_abs.max(p.tolerance_rel * d.depth[known[k]]);
    let mut rng = fastrand::Rng::with_seed(f as u64 + 1);
    let mut best = (0usize, [0f32; 3], 0f32);
    for _ in 0..p.iterations {
        let (a, b, c) = (pts[rng.usize(..pts.len())], pts[rng.usize(..pts.len())], pts[rng.usize(..pts.len())]);
        let nrm = cross(sub(b, a), sub(c, a));
        let len = dot3(nrm, nrm).sqrt();
        if len < 1e-6 {
            continue;
        }
        let nrm = nrm.map(|x| x / len);
        let off = dot3(nrm, a);
        let inl = (0..pts.len()).step_by(4).filter(|&k| (dot3(nrm, pts[k]) - off).abs() < tol(k)).count() * 4;
        if inl > best.0 {
            best = (inl, nrm, off);
        }
    }
    let (_, nrm, off) = best;
    let inliers: Vec<usize> = (0..pts.len()).filter(|&k| (dot3(nrm, pts[k]) - off).abs() < tol(k)).collect();
    if inliers.len() < p.min_inliers || (inliers.len() as f32) < p.min_share * known.len() as f32 {
        return (false, Vec::new());
    }
    let (w, h) = (data.width, data.height);
    let Some(view) = data.views.iter().min_by(|a, b| (a.game_time - d.game_time).abs().total_cmp(&(b.game_time - d.game_time).abs())) else { return (true, Vec::new()) };
    let colour_of = |q: V3| -> Option<[f32; 3]> {
        let (u, v, _) = view.camera.project(q)?;
        sample_rgb(&view.image, w, h, u, v)
    };
    let (gw, gh) = (d.grid_width as usize, d.grid_height as usize);
    let mut rows = vec![false; gh];
    let mut colours: Vec<[f32; 3]> = Vec::new();
    for (j, &k) in inliers.iter().enumerate() {
        let gy = known[k] / gw;
        for r in gy.saturating_sub(p.row_reach)..(gy + p.row_reach + 1).min(gh) {
            rows[r] = true;
        }
        if j % (inliers.len() / 200).max(1) == 0 {
            if let Some(c) = colour_of(pts[k]) {
                colours.push(c);
            }
        }
    }
    if colours.len() < 20 {
        return (true, Vec::new());
    }
    let median = |ch: usize| {
        let mut v: Vec<f32> = colours.iter().map(|c| c[ch]).collect();
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    };
    let floor = [median(0), median(1), median(2)];
    let o = d.camera.position();
    let mut out = Vec::new();
    for i in 0..n {
        if !d.dynamic[i] || !rows[i / gw] {
            continue;
        }
        let q = d.camera.unproject(d.pixels[i][0], d.pixels[i][1], 1.0);
        let dir = sub(q, o);
        let denom = dot3(nrm, dir);
        if denom.abs() < 1e-6 {
            continue;
        }
        // `dir` reaches eye depth 1, so the hit's eye depth is t.
        let t = (off - dot3(nrm, o)) / denom;
        if !(p.min_depth..=p.max_depth).contains(&t) {
            continue;
        }
        let hit = [o[0] + t * dir[0], o[1] + t * dir[1], o[2] + t * dir[2]];
        let Some(c) = colour_of(hit) else { continue };
        if ((c[0] - floor[0]).abs() + (c[1] - floor[1]).abs() + (c[2] - floor[2]).abs()) / 3.0 > p.color_tolerance {
            continue;
        }
        out.push((i, t, hit));
    }
    (true, out)
}

/// Fills unknown samples of every depth frame from its dominant plane (see the module).
pub fn fill(data: &mut Dataset, p: &FillParams) -> FillStats {
    let mut st = FillStats { frames: data.depth_views.len(), ..Default::default() };
    st.unknown = data.depth_views.iter().map(|d| d.dynamic.iter().filter(|x| **x).count()).sum();
    let found: Vec<(bool, Vec<(usize, f32, V3)>)> = (0..data.depth_views.len()).into_par_iter().map(|f| candidates(data, f, p)).collect();
    st.frames_with_plane = found.iter().filter(|f| f.0).count();
    // Confirmation by the other frames.
    let grids: Vec<_> = data.depth_views.iter().map(grid).collect();
    let checkers: Vec<usize> = (0..data.depth_views.len()).step_by(p.check_stride.max(1)).collect();
    let margins = Params::default();
    let size = (data.width, data.height);
    let decided: Vec<Vec<(usize, f32, u8)>> = found
        .par_iter()
        .enumerate()
        .map(|(f, (_, cands))| {
            cands
                .iter()
                .map(|&(i, t, x)| {
                    let (mut seen, mut through) = (0u32, 0u32);
                    for &g in &checkers {
                        if g == f {
                            continue;
                        }
                        let (dg, Some(gg)) = (&data.depth_views[g], &grids[g]) else { continue };
                        let Some((u, v, z)) = dg.camera.project(x) else { continue };
                        match verdict(dg, gg, u, v, z, &margins, &data.mask, size) {
                            Some(true) => through += 1,
                            Some(false) => seen += 1,
                            None => {}
                        }
                    }
                    // 0 fill, 1 unconfirmed, 2 contradicted.
                    let outcome = if through > 0 { 2 } else if seen == 0 { 1 } else { 0 };
                    (i, t, outcome)
                })
                .collect()
        })
        .collect();
    for (f, list) in decided.into_iter().enumerate() {
        st.candidates += list.len();
        let d = &mut data.depth_views[f];
        for (i, t, outcome) in list {
            match outcome {
                0 => {
                    d.depth[i] = t;
                    d.dynamic[i] = false;
                    st.filled += 1;
                }
                1 => st.unconfirmed += 1,
                _ => st.contradicted += 1,
            }
        }
    }
    st
}
