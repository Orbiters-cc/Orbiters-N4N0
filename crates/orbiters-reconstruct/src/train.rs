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
    /// Keyframes held out instead of every `hold_out_every`-th (lab experiments on subsets).
    pub held_out: Option<Vec<usize>>,
    /// Depth steps render only the sample grid: a camera whose pixel centres are the samples' rays.
    pub depth_grid: bool,
    /// Weight λ of D-SSIM in the colour loss (0: L1 only).
    pub ssim: f32,
    /// Robust colour loss threshold (mean absolute error of coherent blobs left out; 0: off), from this fraction of
    /// the steps on.
    pub robust: f32,
    pub robust_from: f32,
    /// Relocation stops after this fraction of the steps (the rest only refines).
    pub relocate_until: f32,
    /// Held-out PSNR during training every this many steps (0: only at the end).
    pub evaluate_every: u32,
    /// Coarse-to-fine colour steps (DashGaussian-like): quarter resolution until this fraction of the steps, half
    /// resolution until `res2_until`, then full; the position learning rate holds until full resolution. 0: off.
    pub res4_until: f32,
    pub res2_until: f32,
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
            held_out: None,
            depth_grid: false,
            ssim: 0.0,
            robust: 0.0,
            robust_from: 0.25,
            relocate_until: 0.8,
            evaluate_every: 1000,
            res4_until: 0.0,
            res2_until: 0.0,
        }
    }
}

impl Settings {
    /// Sets one knob by name (lab experiments).
    pub fn set(&mut self, key: &str, value: &str) -> anyhow::Result<()> {
        let f = || value.parse::<f32>().map_err(|e| anyhow::anyhow!("{key}={value}: {e}"));
        let u = || value.parse::<u32>().map_err(|e| anyhow::anyhow!("{key}={value}: {e}"));
        match key {
            "depth_every" => self.depth_every = u()?,
            "depth_weight" => self.depth_weight = f()?,
            "relocate_every" => self.relocate_every = u()?,
            "opacity_reg" => self.opacity_reg = f()?,
            "scale_reg" => self.scale_reg = f()?,
            "max_radius" => self.max_radius = f()?,
            "hold_out_every" => self.hold_out_every = u()? as usize,
            "depth_grid" => self.depth_grid = f()? > 0.0,
            "ssim" => self.ssim = f()?,
            "robust" => self.robust = f()?,
            "robust_from" => self.robust_from = f()?,
            "relocate_until" => self.relocate_until = f()?,
            "evaluate_every" => self.evaluate_every = u()?,
            "res4_until" => self.res4_until = f()?,
            "res2_until" => self.res2_until = f()?,
            _ => anyhow::bail!("unknown setting {key}"),
        }
        Ok(())
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
    /// Of `seconds`: relocations (download, CPU work, upload) and held-out evaluations during training.
    pub relocation_seconds: f32,
    pub evaluation_seconds: f32,
}

/// The cameras' mean position.
fn camera_centre(data: &Dataset) -> [f32; 3] {
    let n = data.views.len().max(1) as f32;
    data.views.iter().map(|v| v.camera.position()).fold([0.0f32; 3], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n, a[2] + p[2] / n])
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
    let held_out: Vec<usize> = match &settings.held_out {
        Some(h) => h.clone(),
        None if settings.hold_out_every > 1 => (0..data.views.len()).filter(|i| i % settings.hold_out_every == settings.hold_out_every / 2).collect(),
        None => Vec::new(),
    };
    let train_views: Vec<usize> = (0..data.views.len()).filter(|i| !held_out.contains(i)).collect();
    let depth_views: Vec<usize> = (0..data.depth_views.len()).filter(|&i| data.depth_views[i].valid() > 0).collect();
    let samples: Vec<Vec<[f32; 4]>> = data
        .depth_views
        .iter()
        .map(|d| d.pixels.iter().zip(&d.depth).filter(|(_, z)| **z > 0.0).map(|(p, z)| [p[0], p[1], *z, 0.0]).collect())
        .collect();
    // Coarse depth cameras: pixel (gx, gy) is sample (gx, gy) of the grid, footprint kept at training resolution.
    let grids: Vec<Option<(Camera, Vec<[f32; 4]>, [f32; 2])>> = data.depth_views.iter().map(|d| if settings.depth_grid { depth_grid(d) } else { None }).collect();
    let scene_pixels = data.mask.iter().filter(|m| **m > 0.5).count() as f32;
    let extent = extent(data);
    let mut rng = fastrand::Rng::with_seed(settings.seed);

    gpu.upload(&init);
    let coarse = if settings.res2_until > 0.0 { levels(data) } else { Vec::new() };
    for l in &coarse {
        gpu.set_mask(l.width, l.height, &l.mask);
    }
    gpu.set_mask(data.width, data.height, &data.mask);
    gpu.ssim = settings.ssim;
    let regularise = |gpu: &mut Gpu, n: usize| {
        gpu.opacity_reg = settings.opacity_reg / n.max(1) as f32;
        gpu.scale_reg = settings.scale_reg / (n.max(1) as f32 * extent);
    };
    regularise(gpu, init.len());
    let max_radius = settings.max_radius * data.width as f32;
    // The sky shell (seeded crate::init::SKY out) never spawns copies: opaque and plentiful, it would take the budget.
    let centre = camera_centre(data);
    let far_field = crate::init::SKY * 0.5;
    gpu.step = 0;
    let relocate_until = (iterations as f32 * settings.relocate_until) as u32;
    let (mut relocation_seconds, mut evaluation_seconds) = (0.0f32, 0.0f32);
    for it in 1..=iterations {
        let progress = (it - 1) as f32 / iterations as f32;
        // Coarse-to-fine: the level of this step (0 full, 1 half, 2 quarter) and the position decay held until full.
        let level = if progress < settings.res4_until { 2 } else if progress < settings.res2_until { 1 } else { 0 };
        let decay = if coarse.is_empty() { progress } else { ((progress - settings.res2_until) / (1.0 - settings.res2_until)).max(0.0) };
        let lr = LearningRates {
            means: 1.6e-4 * extent * (0.01f32).powf(decay),
            log_scales: 0.005,
            rotations: 0.001,
            opacity: 0.05,
            colors: 0.0025,
        };
        gpu.robust = if progress >= settings.robust_from { settings.robust } else { 0.0 };
        let depth_step = !depth_views.is_empty() && settings.depth_weight > 0.0 && it % settings.depth_every.max(1) == 0;
        let intersections = if depth_step {
            let d = depth_views[rng.usize(..depth_views.len())];
            match &grids[d] {
                Some((cam, grid_samples, dilation)) => {
                    let full = gpu.dilation;
                    gpu.dilation = *dilation;
                    let r = gpu.step(cam, settings.background, Target::Depth { samples: grid_samples, weight: settings.depth_weight }, Some(lr));
                    gpu.dilation = full;
                    r?
                }
                None => gpu.step(&data.depth_views[d].camera, settings.background, Target::Depth { samples: &samples[d], weight: settings.depth_weight }, Some(lr))?,
            }
        } else {
            let v = train_views[rng.usize(..train_views.len())];
            if level > 0 && !coarse.is_empty() {
                let l = &coarse[level - 1];
                debug_assert_eq!(l.factor, 1 << level);
                gpu.step(&l.cameras[v], settings.background, Target::Color { rgba: &l.images[v], scene_pixels: l.scene_pixels, weight: 1.0 }, Some(lr))?
            } else {
                gpu.step(&data.views[v].camera, settings.background, Target::Color { rgba: &data.views[v].image, scene_pixels, weight: 1.0 }, Some(lr))?
            }
        };
        if it % settings.relocate_every == 0 && it >= 500 && it <= relocate_until {
            let relocating = Instant::now();
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
            let far: Vec<bool> = current.means.iter().map(|m| (0..3).map(|k| (m[k] - centre[k]).powi(2)).sum::<f32>() > far_field * far_field).collect();
            relocate(&mut current, &mut m1, &mut m2, &huge, &far, grow_to, &mut rng);
            gpu.upload_with_moments(&current, Some((&m1, &m2)));
            regularise(gpu, current.len());
            let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
            relocation_seconds += relocating.elapsed().as_secs_f32();
        }
        if it % 50 == 0 || it == iterations {
            let evaluating = Instant::now();
            let due = settings.evaluate_every > 0 && it % settings.evaluate_every == 0;
            let psnr = if due || it == iterations { evaluate(gpu, data, &held_out, settings.background)? } else { None };
            evaluation_seconds += evaluating.elapsed().as_secs_f32();
            report(&Progress { iteration: it, iterations, gaussians: gpu.count, intersections, psnr, seconds: start.elapsed().as_secs_f32() });
        }
    }
    let (g, _, _, _) = gpu.download();
    let psnr_held_out = evaluate(gpu, data, &held_out, settings.background)?;
    let sample: Vec<usize> = train_views.iter().copied().step_by((train_views.len() / 8).max(1)).collect();
    let psnr_train = evaluate(gpu, data, &sample, settings.background)?.unwrap_or(0.0);
    Ok(Outcome { gaussians: g, iterations, held_out, psnr_held_out, psnr_train, seconds: start.elapsed().as_secs_f32(), relocation_seconds, evaluation_seconds })
}

/// Keyframes at a coarser resolution: images box-filtered by `factor` from the training resolution, their cameras, and
/// the mask (a coarse pixel is scene only if all its pixels are).
struct Level {
    factor: u32,
    width: u32,
    height: u32,
    mask: Vec<f32>,
    scene_pixels: f32,
    images: Vec<Vec<u32>>,
    cameras: Vec<Camera>,
}

fn half_image(image: &[u32], w: u32, h: u32) -> Vec<u32> {
    let (hw, hh) = (w / 2, h / 2);
    let mut out = vec![0u32; (hw * hh) as usize];
    for y in 0..hh {
        for x in 0..hw {
            let at = |dx: u32, dy: u32| image[((2 * y + dy) * w + 2 * x + dx) as usize];
            let px = [at(0, 0), at(1, 0), at(0, 1), at(1, 1)];
            let mut v = 255u32 << 24;
            for c in 0..3 {
                let sum: u32 = px.iter().map(|p| (p >> (8 * c)) & 255).sum();
                v |= ((sum + 2) / 4) << (8 * c);
            }
            out[(y * hw + x) as usize] = v;
        }
    }
    out
}

fn half_mask(mask: &[f32], w: u32, h: u32) -> Vec<f32> {
    let (hw, hh) = (w / 2, h / 2);
    let mut out = vec![0f32; (hw * hh) as usize];
    for y in 0..hh {
        for x in 0..hw {
            let at = |dx: u32, dy: u32| mask[((2 * y + dy) * w + 2 * x + dx) as usize];
            out[(y * hw + x) as usize] = at(0, 0).min(at(1, 0)).min(at(0, 1)).min(at(1, 1));
        }
    }
    out
}

/// The camera of an image box-filtered by `factor` (pixel centres at integers on both).
pub fn scaled(c: &Camera, factor: u32) -> Camera {
    let f = factor as f32;
    Camera { r: c.r, t: c.t, fx: c.fx / f, fy: c.fy / f, cx: (c.cx + 0.5) / f - 0.5, cy: (c.cy + 0.5) / f - 0.5, width: c.width / factor, height: c.height / factor }
}

fn levels(data: &Dataset) -> Vec<Level> {
    let mut out: Vec<Level> = Vec::new();
    let (mut w, mut h) = (data.width, data.height);
    let mut images: Vec<Vec<u32>> = data.views.iter().map(|v| v.image.clone()).collect();
    let mut mask = data.mask.clone();
    for factor in [2u32, 4] {
        images = images.iter().map(|img| half_image(img, w, h)).collect();
        mask = half_mask(&mask, w, h);
        w /= 2;
        h /= 2;
        let cameras = data.views.iter().map(|v| scaled(&v.camera, factor)).collect();
        let scene_pixels = mask.iter().filter(|m| **m > 0.5).count() as f32;
        out.push(Level { factor, width: w, height: h, mask: mask.clone(), scene_pixels, images: images.clone(), cameras });
    }
    out
}

/// A camera whose pixel centres are a depth frame's sample rays (the grid is regular up to a pixel's rounding), the
/// samples at their grid pixels, and the dilation that keeps the training resolution's footprint.
fn depth_grid(d: &crate::dataset::DepthView) -> Option<(Camera, Vec<[f32; 4]>, [f32; 2])> {
    let (gw, gh) = (d.grid_width as usize, d.grid_height as usize);
    if gw < 2 || gh < 2 {
        return None;
    }
    let (x0, y0) = (d.pixels[0][0], d.pixels[0][1]);
    let sx = (d.pixels[gw - 1][0] - x0) / (gw - 1) as f32;
    let sy = (d.pixels[(gh - 1) * gw][1] - y0) / (gh - 1) as f32;
    if !(sx > 0.0 && sy > 0.0) {
        return None;
    }
    let c = &d.camera;
    let cam = Camera { r: c.r, t: c.t, fx: c.fx / sx, fy: c.fy / sy, cx: (c.cx - x0) / sx, cy: (c.cy - y0) / sy, width: gw as u32, height: gh as u32 };
    let samples = (0..gw * gh).filter(|&i| d.depth[i] > 0.0).map(|i| [(i % gw) as f32, (i / gw) as f32, d.depth[i], 0.0]).collect();
    let dil = crate::cpu::DILATION as f32;
    Some((cam, samples, [dil / (sx * sx), dil / (sy * sy)]))
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
/// by opacity, never `far` ones (unless nothing else lives). Adam moments of every Gaussian touched restart at zero.
fn relocate(g: &mut Gaussians, m1: &mut Vec<f32>, m2: &mut Vec<f32>, huge: &[bool], far: &[bool], grow_to: usize, rng: &mut fastrand::Rng) -> usize {
    let opacity: Vec<f32> = g.opacity_logits.iter().map(|&l| sigmoid(l)).collect();
    let is_dead = |i: usize| opacity[i] < DEAD || !opacity[i].is_finite() || huge[i] || g.means[i].iter().chain(&g.log_scales[i]).any(|c| !c.is_finite());
    let (mut dead, mut alive): (Vec<usize>, Vec<usize>) = (0..g.len()).partition(|&i| is_dead(i));
    if alive.iter().any(|&i| !far[i]) {
        alive.retain(|&i| !far[i]);
    }
    // At most a twentieth of the set moves at once, and the set grows only while it uses what it has: a scene that
    // needs fewer Gaussians than the budget would otherwise churn copies that fade again.
    let cap = (g.len() / 20).max(1);
    let grow = if dead.len() <= cap { grow_to.saturating_sub(g.len()) } else { 0 };
    if dead.len() > cap {
        rng.shuffle(&mut dead);
        dead.truncate(cap);
    }
    let moves = dead.len() + grow;
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
        for i in 0..100 {
            g.push([i as f32, 0.0, 0.0], [-2.0; 3], [1.0, 0.0, 0.0, 0.0], if i < 3 { logit(0.001) } else { logit(0.8) }, [0.5; 3]);
        }
        let mut m1 = vec![1.0; 100 * PACKED];
        let mut m2 = vec![1.0; 100 * PACKED];
        let moved = relocate(&mut g, &mut m1, &mut m2, &[false; 100], &[false; 100], 104, &mut fastrand::Rng::with_seed(1));
        assert_eq!(moved, 7);
        assert_eq!(g.len(), 104);
        assert_eq!(m1.len(), 104 * PACKED);
        assert!(g.opacity_logits.iter().all(|&l| sigmoid(l) >= DEAD));
        assert!(m1[..PACKED].iter().all(|&m| m == 0.0));
    }

    #[test]
    fn a_set_with_many_dead_moves_a_twentieth_and_does_not_grow() {
        let mut g = Gaussians::default();
        for i in 0..100 {
            g.push([i as f32, 0.0, 0.0], [-2.0; 3], [1.0, 0.0, 0.0, 0.0], if i < 30 { logit(0.001) } else { logit(0.8) }, [0.5; 3]);
        }
        let (mut m1, mut m2) = (vec![0.0; 100 * PACKED], vec![0.0; 100 * PACKED]);
        let moved = relocate(&mut g, &mut m1, &mut m2, &[false; 100], &[false; 100], 200, &mut fastrand::Rng::with_seed(2));
        assert_eq!(moved, 5);
        assert_eq!(g.len(), 100);
        assert_eq!(g.opacity_logits.iter().filter(|&&l| sigmoid(l) < DEAD).count(), 25);
    }

    #[test]
    fn the_far_field_never_spawns_copies() {
        // Three faded, forty opaque far ones at x = 400, the rest near x = 0..2.
        let mut g = Gaussians::default();
        for i in 0..100 {
            let x = if (3..43).contains(&i) { 400.0 } else { (i % 3) as f32 };
            g.push([x, 0.0, 0.0], [-4.0; 3], [1.0, 0.0, 0.0, 0.0], if i < 3 { logit(0.001) } else { logit(0.9) }, [0.5; 3]);
        }
        let far: Vec<bool> = g.means.iter().map(|m| m[0] > 100.0).collect();
        let (mut m1, mut m2) = (vec![0.0; 100 * PACKED], vec![0.0; 100 * PACKED]);
        relocate(&mut g, &mut m1, &mut m2, &[false; 100], &far, 104, &mut fastrand::Rng::with_seed(4));
        assert_eq!(g.len(), 104);
        assert_eq!(g.means.iter().filter(|m| m[0] > 100.0).count(), 40);
    }
}
