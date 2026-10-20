//! Training in replay mode: Adam on the keyframes' colour (masked L1) and the exact depth samples (relative L1),
//! with MCMC-style relocation (Kheradmand et al. 2024): every so often, Gaussians that faded out move onto live ones
//! picked by opacity, the copies sharing the original's opacity and footprint, while the set grows to its budget.
//! Every `hold_out_every`-th keyframe is never trained on and scores the result.

use crate::camera::Camera;
use crate::dataset::{channel, Dataset};
use crate::gaussians::{logit, rotation_matrix, sigmoid, Gaussians, PACKED};
use crate::gpu::{Gpu, LearningRates, Target};
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct Settings {
    /// Optimisation steps; 0 picks one from the number of keyframes.
    pub iterations: u32,
    /// Most Gaussians kept.
    pub budget: usize,
    /// One step in this many trains on a depth frame instead of a keyframe.
    pub depth_every: u32,
    /// Every this-many-th keyframe is held out (0: none).
    pub hold_out_every: usize,
    pub relocate_every: u32,
    pub depth_weight: f32,
    pub opacity_reg: f32,
    /// Per scene extent: the scale penalty is in the scene's units.
    pub scale_reg: f32,
    /// Gaussians whose screen radius ever exceeds this fraction of the image's width are relocated.
    pub max_radius: f32,
    pub background: [f32; 3],
    pub seed: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            iterations: 0,
            budget: 1_000_000,
            depth_every: 4,
            hold_out_every: 8,
            relocate_every: 100,
            depth_weight: 0.3,
            opacity_reg: 0.01,
            scale_reg: 0.01,
            max_radius: 0.1,
            background: [0.0; 3],
            seed: 1,
        }
    }
}

/// Opacity under which a Gaussian counts as dead and gets relocated.
const DEAD: f32 = 0.005;
/// Most copies of one Gaussian per relocation (the binomial sums lose precision past this).
const MAX_RATIO: usize = 51;

#[derive(Clone, Debug)]
pub struct Progress {
    pub iteration: u32,
    pub iterations: u32,
    pub gaussians: usize,
    /// Tile entries of the last step (rendering cost).
    pub intersections: u32,
    /// Mean PSNR over the held-out keyframes, when measured at this report.
    pub psnr: Option<f32>,
    pub seconds: f32,
}

pub struct Outcome {
    pub gaussians: Gaussians,
    pub iterations: u32,
    pub held_out: Vec<usize>,
    pub psnr_held_out: Option<f32>,
    pub psnr_train: f32,
    pub seconds: f32,
}

/// A typical scene size: the larger of the cameras' spread and the median sampled depth (sets the position step).
pub fn extent(data: &Dataset) -> f32 {
    let positions: Vec<[f32; 3]> = data.views.iter().map(|v| v.camera.position()).collect();
    let n = positions.len().max(1) as f32;
    let c = positions.iter().fold([0.0f32; 3], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n, a[2] + p[2] / n]);
    let spread = positions.iter().map(|p| ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt()).fold(0.0, f32::max) * 1.1;
    let mut depths: Vec<f32> = data.depth_views.iter().flat_map(|d| d.depth.iter().copied().filter(|z| *z > 0.0)).collect();
    let median = if depths.is_empty() {
        1.0
    } else {
        let k = depths.len() / 2;
        *depths.select_nth_unstable_by(k, f32::total_cmp).1
    };
    spread.max(median).max(0.5)
}

pub fn default_iterations(data: &Dataset) -> u32 {
    (data.views.len() as u32 * 150).clamp(3_000, 30_000)
}

/// PSNR over the scene pixels (rendered colour clamped to 0..1).
pub fn psnr(render: &[[f32; 4]], truth: &[u32], mask: &[f32]) -> f32 {
    let (mut se, mut count) = (0.0f64, 0.0f64);
    for (k, px) in render.iter().enumerate() {
        if mask[k] < 0.5 {
            continue;
        }
        for (ch, v) in px[..3].iter().enumerate() {
            let d = v.clamp(0.0, 1.0) as f64 - channel(truth[k], ch) as f64;
            se += d * d;
        }
        count += 3.0;
    }
    let mse = (se / count.max(1.0)).max(1e-10);
    (10.0 * (1.0 / mse).log10()) as f32
}

/// Renders the Gaussians on the GPU from a camera: (r, g, b, depth) per pixel.
pub fn render(gpu: &mut Gpu, cam: &Camera, background: [f32; 3]) -> anyhow::Result<Vec<[f32; 4]>> {
    gpu.step(cam, background, Target::None, None)?;
    Ok(gpu.read_image())
}

pub fn evaluate(gpu: &mut Gpu, data: &Dataset, views: &[usize], background: [f32; 3]) -> anyhow::Result<Option<f32>> {
    if views.is_empty() {
        return Ok(None);
    }
    let mut sum = 0.0;
    for &i in views {
        let img = render(gpu, &data.views[i].camera, background)?;
        sum += psnr(&img, &data.views[i].image, &data.mask);
    }
    Ok(Some(sum / views.len() as f32))
}

pub fn train(data: &Dataset, gpu: &mut Gpu, init: Gaussians, settings: &Settings, mut report: impl FnMut(&Progress)) -> anyhow::Result<Outcome> {
    let start = Instant::now();
    let iterations = if settings.iterations == 0 { default_iterations(data) } else { settings.iterations };
    let held_out: Vec<usize> = if settings.hold_out_every > 1 { (0..data.views.len()).filter(|i| i % settings.hold_out_every == settings.hold_out_every / 2).collect() } else { Vec::new() };
    let train_views: Vec<usize> = (0..data.views.len()).filter(|i| !held_out.contains(i)).collect();
    let depth_views: Vec<usize> = (0..data.depth_views.len()).filter(|&i| data.depth_views[i].valid() > 0).collect();
    let samples: Vec<Vec<[f32; 4]>> = data
        .depth_views
        .iter()
        .map(|d| d.pixels.iter().zip(&d.depth).filter(|(_, z)| **z > 0.0).map(|(p, z)| [p[0], p[1], *z, 0.0]).collect())
        .collect();
    let scene_pixels = data.mask.iter().filter(|m| **m > 0.5).count() as f32;
    let extent = extent(data);
    let mut rng = fastrand::Rng::with_seed(settings.seed);

    gpu.upload(&init);
    gpu.set_mask(data.width, data.height, &data.mask);
    let regularise = |gpu: &mut Gpu, n: usize| {
        gpu.opacity_reg = settings.opacity_reg / n.max(1) as f32;
        gpu.scale_reg = settings.scale_reg / (n.max(1) as f32 * extent);
    };
    regularise(gpu, init.len());
    let max_radius = settings.max_radius * data.width as f32;
    gpu.step = 0;
    let relocate_until = iterations * 8 / 10;
    for it in 1..=iterations {
        let progress = (it - 1) as f32 / iterations as f32;
        let lr = LearningRates {
            means: 1.6e-4 * extent * (0.01f32).powf(progress),
            log_scales: 0.005,
            rotations: 0.001,
            opacity: 0.05,
            colors: 0.0025,
        };
        let depth_step = !depth_views.is_empty() && settings.depth_weight > 0.0 && it % settings.depth_every.max(1) == 0;
        let intersections = if depth_step {
            let d = depth_views[rng.usize(..depth_views.len())];
            gpu.step(&data.depth_views[d].camera, settings.background, Target::Depth { samples: &samples[d], weight: settings.depth_weight }, Some(lr))?
        } else {
            let v = train_views[rng.usize(..train_views.len())];
            gpu.step(&data.views[v].camera, settings.background, Target::Color { rgba: &data.views[v].image, scene_pixels, weight: 1.0 }, Some(lr))?
        };
        if it % settings.relocate_every == 0 && it >= 500 && it <= relocate_until {
            let (mut current, mut m1, mut m2, stats) = gpu.download();
            let grow_to = (current.len() + current.len() / 20).min(settings.budget);
            if it % 500 == 0 {
                let mut big: Vec<f32> = current.log_scales.iter().map(|s| s[0].max(s[1]).max(s[2]).exp()).collect();
                big.sort_by(f32::total_cmp);
                let dead = current.opacity_logits.iter().filter(|&&l| sigmoid(l) < DEAD).count();
                let huge = stats.iter().filter(|s| s[2] > max_radius).count();
                let q = |f: f32| big[((big.len() - 1) as f32 * f) as usize];
                eprintln!("it {it}: {} Gaussians, {dead} dead, {huge} huge, largest axis p50 {:.4} p90 {:.4} p99 {:.4} max {:.2}, {intersections} entries", current.len(), q(0.5), q(0.9), q(0.99), q(1.0));
            }
            let huge: Vec<bool> = stats.iter().map(|s| s[2] > max_radius).collect();
            relocate(&mut current, &mut m1, &mut m2, &huge, grow_to, &mut rng);
            gpu.upload_with_moments(&current, Some((&m1, &m2)));
            regularise(gpu, current.len());
        }
        if it % 50 == 0 || it == iterations {
            let psnr = if it % 1000 == 0 || it == iterations { evaluate(gpu, data, &held_out, settings.background)? } else { None };
            report(&Progress { iteration: it, iterations, gaussians: gpu.count, intersections, psnr, seconds: start.elapsed().as_secs_f32() });
        }
    }
    let (g, _, _, _) = gpu.download();
    let psnr_held_out = evaluate(gpu, data, &held_out, settings.background)?;
    let sample: Vec<usize> = train_views.iter().copied().step_by((train_views.len() / 8).max(1)).collect();
    let psnr_train = evaluate(gpu, data, &sample, settings.background)?.unwrap_or(0.0);
    Ok(Outcome { gaussians: g, iterations, held_out, psnr_held_out, psnr_train, seconds: start.elapsed().as_secs_f32() })
}

/// New opacity and scale factor of a Gaussian of opacity `o` split into `n` copies, so their sum renders like the
/// original (MCMC paper, eq. 9).
fn relocation(o: f64, n: usize) -> (f64, f64) {
    let new_o = 1.0 - (1.0 - o).powf(1.0 / n as f64);
    let mut denom = 0.0;
    for i in 1..=n {
        let mut binom = 1.0f64;
        for k in 0..i {
            if k > 0 {
                binom = binom * (i - k) as f64 / k as f64;
            }
            let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
            denom += binom * sign / ((k + 1) as f64).sqrt() * new_o.powi(k as i32 + 1);
        }
    }
    (new_o, o / denom)
}

/// Moves dead Gaussians (faded, broken, or `huge` on screen) and new ones, up to `grow_to`, onto live ones sampled
/// by opacity. Adam moments of every Gaussian touched restart at zero.
fn relocate(g: &mut Gaussians, m1: &mut Vec<f32>, m2: &mut Vec<f32>, huge: &[bool], grow_to: usize, rng: &mut fastrand::Rng) -> usize {
    let opacity: Vec<f32> = g.opacity_logits.iter().map(|&l| sigmoid(l)).collect();
    let is_dead = |i: usize| opacity[i] < DEAD || !opacity[i].is_finite() || huge[i] || g.means[i].iter().chain(&g.log_scales[i]).any(|c| !c.is_finite());
    let (dead, alive): (Vec<usize>, Vec<usize>) = (0..g.len()).partition(|&i| is_dead(i));
    let moves = dead.len() + grow_to.saturating_sub(g.len());
    if moves == 0 || alive.is_empty() {
        return 0;
    }
    let mut cdf = Vec::with_capacity(alive.len());
    let mut acc = 0.0f64;
    for &i in &alive {
        acc += opacity[i] as f64;
        cdf.push(acc);
    }
    let mut ratio = vec![1usize; g.len()];
    let mut picks = Vec::with_capacity(moves);
    while picks.len() < moves {
        let r = rng.f64() * acc;
        let j = alive[cdf.partition_point(|&c| c < r).min(alive.len() - 1)];
        if ratio[j] >= MAX_RATIO {
            continue;
        }
        ratio[j] += 1;
        picks.push(j);
    }
    for &j in &alive {
        if ratio[j] > 1 {
            let (new_o, factor) = relocation(opacity[j] as f64, ratio[j]);
            g.opacity_logits[j] = logit((new_o as f32).clamp(DEAD, 1.0 - 1e-4));
            let f = (factor as f32).max(1e-6).ln();
            for s in &mut g.log_scales[j] {
                *s += f;
            }
            m1[j * PACKED..(j + 1) * PACKED].fill(0.0);
            m2[j * PACKED..(j + 1) * PACKED].fill(0.0);
        }
    }
    let mut free = dead.into_iter();
    for &j in &picks {
        // Somewhere inside the source's footprint, so the copies don't share every gradient forever.
        let r = rotation_matrix(g.rotations[j]);
        let s = g.log_scales[j].map(f32::exp);
        let e = [normal(rng) * s[0], normal(rng) * s[1], normal(rng) * s[2]];
        let mut mean = g.means[j];
        for (row, m) in r.iter().zip(mean.iter_mut()) {
            *m += row[0] * e[0] + row[1] * e[1] + row[2] * e[2];
        }
        let (log_scale, rotation, opacity_logit, color) = (g.log_scales[j], g.rotations[j], g.opacity_logits[j], g.colors[j]);
        match free.next() {
            Some(i) => {
                g.means[i] = mean;
                g.log_scales[i] = log_scale;
                g.rotations[i] = rotation;
                g.opacity_logits[i] = opacity_logit;
                g.colors[i] = color;
                m1[i * PACKED..(i + 1) * PACKED].fill(0.0);
                m2[i * PACKED..(i + 1) * PACKED].fill(0.0);
            }
            None => {
                g.push(mean, log_scale, rotation, opacity_logit, color);
                m1.extend_from_slice(&[0.0; PACKED]);
                m2.extend_from_slice(&[0.0; PACKED]);
            }
        }
    }
    picks.len()
}

fn normal(rng: &mut fastrand::Rng) -> f32 {
    // Box–Muller.
    let u = rng.f32().max(1e-7);
    let v = rng.f32();
    (-2.0 * u.ln()).sqrt() * (std::f32::consts::TAU * v).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_keep_the_opacity_of_the_original_where_they_overlap() {
        for o in [0.1f64, 0.5, 0.9] {
            for n in [2usize, 5, 20] {
                let (new_o, factor) = relocation(o, n);
                assert!((1.0 - (1.0 - new_o).powi(n as i32) - o).abs() < 1e-9);
                assert!(factor > 0.0 && factor.is_finite(), "{o} {n}: {factor}");
            }
        }
        // One copy is the original.
        let (o1, f1) = relocation(0.7, 1);
        assert!((o1 - 0.7).abs() < 1e-12 && (f1 - 1.0).abs() < 1e-12);
    }

    #[test]
    fn relocation_fills_dead_slots_then_grows() {
        let mut g = Gaussians::default();
        for i in 0..10 {
            g.push([i as f32, 0.0, 0.0], [-2.0; 3], [1.0, 0.0, 0.0, 0.0], if i < 3 { logit(0.001) } else { logit(0.8) }, [0.5; 3]);
        }
        let mut m1 = vec![1.0; 10 * PACKED];
        let mut m2 = vec![1.0; 10 * PACKED];
        let moved = relocate(&mut g, &mut m1, &mut m2, &[false; 10], 12, &mut fastrand::Rng::with_seed(1));
        assert_eq!(moved, 5);
        assert_eq!(g.len(), 12);
        assert_eq!(m1.len(), 12 * PACKED);
        assert!(g.opacity_logits.iter().all(|&l| sigmoid(l) >= DEAD));
        assert!(m1[..PACKED].iter().all(|&m| m == 0.0));
    }
}
