//! Lab: Gaussians behind every measured surface. A Gaussian whose centre lies behind the exact depth (beyond the sample
//! at its pixel, by a margin and its own size) in every depth frame that looks at it was never where a depth frame
//! could see it: under a floor or inside a wall. With colour fixed per Gaussian, that is where the optimiser puts the
//! virtual images that explain glossy reflections (they show as loose specks from other heights), and floaters behind
//! surfaces. A sample that hit nothing counts as infinitely far (the sky stays); unknown samples (crate::depthfix) say
//! nothing.

use crate::dataset::Dataset;
use crate::gaussians::Gaussians;
use crate::transients::grid;
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct CarveParams {
    /// Compare against every n-th depth frame.
    pub frame_stride: usize,
    pub margin_abs: f32,
    pub margin_rel: f32,
    /// The Gaussian's extent along the ray: this many times its largest scale.
    pub sigmas: f32,
    /// Fewest depth frames with a measured surface at its pixel before a verdict.
    pub min_votes: usize,
}

impl Default for CarveParams {
    fn default() -> Self {
        Self { frame_stride: 4, margin_abs: 0.05, margin_rel: 0.03, sigmas: 2.0, min_votes: 3 }
    }
}

/// Indices of the Gaussians behind every measured surface that looks at them.
pub fn behind_surfaces(g: &Gaussians, data: &Dataset, p: &CarveParams) -> Vec<u32> {
    let frames: Vec<usize> = (0..data.depth_views.len()).step_by(p.frame_stride.max(1)).collect();
    let grids: Vec<_> = frames.iter().map(|&f| grid(&data.depth_views[f])).collect();
    (0..g.len())
        .into_par_iter()
        .filter(|&i| {
            let m = g.means[i];
            let extent = p.sigmas * g.log_scales[i].iter().fold(f32::MIN, |a, s| a.max(*s)).exp();
            let (mut front, mut behind) = (0usize, 0usize);
            for (k, &f) in frames.iter().enumerate() {
                let (d, Some(gg)) = (&data.depth_views[f], &grids[k]) else { continue };
                let Some((u, v, z)) = d.camera.project(m) else { continue };
                let (fx, fy) = ((u - gg.x0) / gg.sx, (v - gg.y0) / gg.sy);
                if !(fx >= -0.5 && fy >= -0.5) || fx > gg.gw as f32 - 0.5 || fy > gg.gh as f32 - 0.5 {
                    continue;
                }
                let s = (fy.round() as usize).min(gg.gh - 1) * gg.gw + (fx.round() as usize).min(gg.gw - 1);
                if d.is_dynamic(s) {
                    continue;
                }
                let (px, py) = (d.pixels[s][0].round(), d.pixels[s][1].round());
                if px < 0.0 || py < 0.0 || px >= data.width as f32 || py >= data.height as f32 || data.mask[(py as u32 * data.width + px as u32) as usize] < 0.5 {
                    continue;
                }
                let depth = d.depth[s];
                if depth <= 0.0 || z <= depth + p.margin_abs.max(p.margin_rel * depth) + extent {
                    front += 1;
                } else {
                    behind += 1;
                }
                if front > 0 {
                    return false;
                }
            }
            front == 0 && behind >= p.min_votes
        })
        .map(|i| i as u32)
        .collect()
}
