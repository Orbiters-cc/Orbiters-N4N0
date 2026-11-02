//! Births at measured depth (crate::train::Settings::depth_births): ConeGS (Baranowski et al., 3DV 2026, arXiv
//! 2511.06810) places new Gaussians on the rays of the pixels a render gets most wrong, at the depth an iNGP proxy
//! gives them, a pixel's cone across. N4N0 measures that depth: each relocation round renders a few depth frames on
//! their sample grids (one pixel a sample ray, as depth steps train), compares each trusted sample's rendered colour
//! with what the training keyframes nearest in time saw at its point, and draws new Gaussians at the samples by that
//! error without replacement: a disc on the surface the neighbouring samples give, `SIZE` keyframe pixels across, of
//! the keyframes' colour.

use crate::dataset::Dataset;
use crate::gaussians::{logit, quat_from_z};
use crate::gpu::Gpu;

/// A new Gaussian's footprint along the surface, in training keyframe pixels at its depth (ConeGS sizes its births
/// to the pixel cone: λ_scale 2 over a radius, one pixel across).
const SIZE: f32 = 1.0;
/// And its opacity (ConeGS and 3DGS's random seeds: 0.1).
const OPACITY: f32 = 0.1;
/// Depth frames rendered a round.
const FRAMES: usize = 8;

/// What births need from a session: the depth frames with trusted samples on a regular grid, and the training
/// keyframes nearest each in time (held-out ones never colour anything).
pub struct Spawner {
    frames: Vec<usize>,
    nearest: Vec<Vec<usize>>,
    focal: f32,
}

impl Spawner {
    pub fn new(data: &Dataset, held_out: &[usize]) -> Self {
        let mut times: Vec<(f32, usize)> = (0..data.views.len()).filter(|i| !held_out.contains(i)).map(|i| (data.views[i].game_time, i)).collect();
        times.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let frames: Vec<usize> = (0..data.depth_views.len())
            .filter(|&i| {
                let d = &data.depth_views[i];
                d.valid() > 0 && d.grid.width >= 2 && d.grid.height >= 2 && d.depth.len() == (d.grid.width * d.grid.height) as usize
            })
            .collect();
        let nearest = data.depth_views.iter().map(|d| crate::init::nearest_in_time(&times, d.game_time, 4)).collect();
        let focal = data.views.first().map_or(1.0, |v| v.camera.fx.max(v.camera.fy));
        Self { frames, nearest, focal }
    }

    /// Up to `count` new Gaussians (mean, log scales, rotation, opacity logit, colour: crate::gpu::Relocation's spawn
    /// layout) where the device's render of [`FRAMES`] random depth frames errs most.
    pub fn draw(&self, gpu: &mut Gpu, data: &Dataset, count: usize, background: [f32; 3], rng: &mut fastrand::Rng) -> anyhow::Result<Vec<[f32; 14]>> {
        if self.frames.is_empty() || count == 0 {
            return Ok(Vec::new());
        }
        // (key, frame, sample) of every candidate: Efraimidis–Spirakis keys ln(u) / error, the largest `count` drawn.
        let mut candidates: Vec<(f32, usize, usize)> = Vec::new();
        for _ in 0..FRAMES.min(self.frames.len()) {
            let f = self.frames[rng.usize(..self.frames.len())];
            let dv = &data.depth_views[f];
            let t = crate::train::depth_target(dv);
            if (t.camera.width, t.camera.height) != (dv.grid.width, dv.grid.height) {
                continue;
            }
            let full = gpu.filter_2d;
            gpu.filter_2d = t.filter_2d;
            let image = crate::train::render(gpu, &t.camera, background);
            gpu.filter_2d = full;
            let image = image?;
            for s in &t.samples {
                let i = (s[1] as u32 * dv.grid.width + s[0] as u32) as usize;
                let [u, v] = dv.grid.pixels[i];
                let p = dv.camera.unproject(u, v, s[2]);
                let Some(seen) = crate::init::color_at(data, p, &self.nearest[f]) else { continue };
                let r = image[i];
                let error = (0..3).map(|c| (r[c].clamp(0.0, 1.0) - seen[c]).abs()).sum::<f32>() / 3.0;
                if error > 1e-4 {
                    candidates.push((rng.f32().max(1e-12).ln() / error, f, i));
                }
            }
        }
        if candidates.len() > count {
            candidates.select_nth_unstable_by(count - 1, |a, b| b.0.total_cmp(&a.0));
            candidates.truncate(count);
        }
        let opacity = logit(OPACITY);
        Ok(candidates
            .into_iter()
            .filter_map(|(_, f, i)| {
                let dv = &data.depth_views[f];
                let [u, v] = dv.grid.pixels[i];
                let z = dv.depth[i];
                let p = dv.camera.unproject(u, v, z);
                let color = crate::init::color_at(data, p, &self.nearest[f])?;
                let normal = crate::init::facing_normal(dv, i, p, false);
                let s = (SIZE * z / self.focal).max(1e-5).ln();
                let q = quat_from_z(normal);
                Some([p[0], p[1], p[2], s, s, s - 10f32.ln(), q[0], q[1], q[2], q[3], opacity, color[0], color[1], color[2]])
            })
            .collect())
    }
}
