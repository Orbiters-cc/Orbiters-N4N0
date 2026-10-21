//! Training in replay mode: Adam on the keyframes' colour ((1 − λ)·L1 + λ·D-SSIM over the scene pixels) and the
//! exact depth samples (relative L1), with MCMC-style relocation (Kheradmand et al. 2024): every so often, Gaussians
//! that faded out move onto live ones picked by opacity, the copies sharing the original's opacity and footprint, while
//! the set grows to its budget. Depth steps render only the depth samples' rays; colour steps can run coarse to fine. Every `hold_out_every`-th keyframe is never trained on and scores the result.

use crate::camera::Camera;
use crate::dataset::{channel, Dataset, DepthView};
use crate::gaussians::{logit, sigmoid, Gaussians};
#[cfg(test)]
use crate::gaussians::{rotation_matrix, PACKED};
use crate::gpu::{Gpu, LearningRates, Relocation, Target};
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
    /// Weight λ of D-SSIM in the colour loss (0: L1 only).
    pub ssim: f32,
    /// Robust colour loss: from `robust_from` of the steps on, pixels inside blobs that err by more than this (mean
    /// absolute error, 0–1) leave each colour step's loss: other players, animated posters, particles. 0: off.
    pub robust: f32,
    pub robust_from: f32,
    /// Coarse to fine (after DashGaussian): colour steps at quarter resolution until this fraction of the steps, at
    /// half resolution until `half_until`, then at full resolution; the position learning rate holds until then.
    /// `half_until` 0 (the default): always full resolution. 0.15 / 0.4 trained 15–20 % faster at no cost on the clean
    /// Canine capture, but cost about 1.2 dB on the best 90 % of VRChat Home's pixels in two repeats.
    pub quarter_until: f32,
    pub half_until: f32,
    /// Relocation stops after this fraction of the steps; the rest only refines. Half instead of 0.8 gained 0.7–1.0 dB
    /// on the Canine orbit (two seeds) and was neutral on VRChat Home: while rounds continue, the moved copies never
    /// settle.
    pub relocate_until: f32,
    /// Every this many steps from step 1000 while relocation runs (0: never), the Gaussians whose blending weight
    /// reaches `unseen_weight` in no scene pixel of any training view are faded, so the next relocations reuse them
    /// where the model is seen. A third of the budget sat where nothing sees it; reusing it gained 0.2–0.9 dB on four
    /// captures (VRChat Home +0.9 dB over two seeds), and with it half the budget matched the full budget without it on
    /// VRChat Home and Palm Point.
    pub fade_unseen_every: u32,
    pub unseen_weight: f32,
    /// Covers the sky shell where no view looked (crate::sky) once training ends.
    pub sky_fill: bool,
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
            ssim: 0.2,
            robust: 0.0,
            robust_from: 0.25,
            quarter_until: 0.0,
            half_until: 0.0,
            relocate_until: 0.5,
            fade_unseen_every: 1000,
            unseen_weight: 0.02,
            sky_fill: false,
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
    /// Tile entries of a recent step (rendering cost).
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

/// 100 steps per keyframe: VRChat Home trains as well at 75 to 150, and with relocation until half the run the Canine
/// orbit scores higher at 100 than it did at 150.
pub fn default_iterations(data: &Dataset) -> u32 {
    (data.views.len() as u32 * 100).clamp(3_000, 30_000)
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
        sum += gpu.score(&data.views[i].camera, background, &data.views[i].image)?;
    }
    Ok(Some(sum / views.len() as f32))
}

pub fn train(data: &Dataset, gpu: &mut Gpu, init: Gaussians, settings: &Settings, mut report: impl FnMut(&Progress)) -> anyhow::Result<Outcome> {
    let start = Instant::now();
    let iterations = if settings.iterations == 0 { default_iterations(data) } else { settings.iterations };
    let held_out: Vec<usize> = if settings.hold_out_every > 1 { (0..data.views.len()).filter(|i| i % settings.hold_out_every == settings.hold_out_every / 2).collect() } else { Vec::new() };
    let train_views: Vec<usize> = (0..data.views.len()).filter(|i| !held_out.contains(i)).collect();
    let depth_views: Vec<usize> = (0..data.depth_views.len()).filter(|&i| data.depth_views[i].valid() > 0).collect();
    let depth_targets: Vec<DepthTarget> = data.depth_views.iter().map(depth_target).collect();
    let scene_pixels = data.mask.iter().filter(|m| **m > 0.5).count() as f32;
    let extent = extent(data);
    let mut rng = fastrand::Rng::with_seed(settings.seed);

    // Feed the GPU even while another program keeps every core busy (crate::priority).
    let _feeding = crate::priority::Feeding::start();
    gpu.upload(&init);
    let coarse = if settings.half_until > 0.0 { levels(data) } else { Vec::new() };
    for l in &coarse {
        gpu.set_mask(l.width, l.height, &l.mask);
    }
    gpu.set_mask(data.width, data.height, &data.mask);
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
    for it in 1..=iterations {
        let progress = (it - 1) as f32 / iterations as f32;
        // Coarse to fine: this step's level (0 full, 1 half, 2 quarter resolution); positions decay from full on.
        let level = if coarse.is_empty() || progress >= settings.half_until {
            0
        } else if progress < settings.quarter_until {
            2
        } else {
            1
        };
        let decay = if coarse.is_empty() { progress } else { ((progress - settings.half_until) / (1.0 - settings.half_until)).max(0.0) };
        let lr = LearningRates {
            means: 1.6e-4 * extent * (0.01f32).powf(decay),
            log_scales: 0.005,
            rotations: 0.001,
            opacity: 0.05,
            colors: 0.0025,
        };
        let depth_step = !depth_views.is_empty() && settings.depth_weight > 0.0 && it % settings.depth_every.max(1) == 0;
        let intersections = if depth_step {
            let t = &depth_targets[depth_views[rng.usize(..depth_views.len())]];
            let full = gpu.dilation;
            gpu.dilation = t.dilation;
            let stepped = gpu.step(&t.camera, settings.background, Target::Depth { samples: &t.samples, weight: settings.depth_weight }, Some(lr));
            gpu.dilation = full;
            stepped?
        } else {
            let v = train_views[rng.usize(..train_views.len())];
            let (camera, rgba, pixels) = match level {
                0 => (&data.views[v].camera, &data.views[v].image, scene_pixels),
                l => (&coarse[l - 1].cameras[v], &coarse[l - 1].images[v], coarse[l - 1].scene_pixels),
            };
            let robust = if progress >= settings.robust_from { settings.robust } else { 0.0 };
            gpu.step(camera, settings.background, Target::Color { rgba, scene_pixels: pixels, weight: 1.0, ssim: settings.ssim, robust }, Some(lr))?
        };
        if settings.fade_unseen_every > 0 && it % settings.fade_unseen_every == 0 && it >= 1000 && it + settings.relocate_every <= relocate_until {
            let _cpu = crate::priority::CpuWork::start();
            let cams: Vec<Camera> = train_views.iter().map(|&i| data.views[i].camera).collect();
            let seen = gpu.visibility(&cams, settings.background)?;
            let faded = logit(1e-4).to_bits();
            let adjust: Vec<[u32; 4]> = seen.iter().enumerate().filter(|(_, w)| **w < settings.unseen_weight).map(|(i, _)| [i as u32, faded, 0f32.to_bits(), 0]).collect();
            eprintln!("it {it}: fading {} of {} Gaussians no training pixel sees", adjust.len(), seen.len());
            gpu.apply_relocation(&Relocation { adjust, copies: Vec::new(), len: seen.len() })?;
        }
        if it % 50 == 0 || it == iterations {
            let psnr = if it % 1000 == 0 || it == iterations {
                let _cpu = crate::priority::CpuWork::start();
                evaluate(gpu, data, &held_out, settings.background)?
            } else {
                None
            };
            report(&Progress { iteration: it, iterations, gaussians: gpu.count, intersections, psnr, seconds: start.elapsed().as_secs_f32() });
        }
        if it % settings.relocate_every == 0 && it >= 500 && it <= relocate_until {
            // The device classifies, the CPU samples the plan, the device moves the copies: a few MB cross the bus
            // instead of the whole set and its Adam moments both ways.
            let _cpu = crate::priority::CpuWork::start();
            let weights = gpu.relocation_weights(max_radius, centre, far_field)?;
            let n = weights.len();
            if it % 500 == 0 {
                let dead = weights.iter().filter(|w| **w == -1.0).count();
                eprintln!("it {it}: {n} Gaussians, {dead} dead or huge, {intersections} entries");
            }
            let plan = plan_relocation(&weights, (n + n / 20).min(settings.budget), &mut rng);
            gpu.apply_relocation(&plan)?;
            regularise(gpu, plan.len);
        }
    }
    let _cpu = crate::priority::CpuWork::start();
    let (mut g, _, _, _) = gpu.download();
    // Faded Gaussians that relocation never reused (it moves at most a twentieth per round, and stops at
    // `relocate_until`) show nowhere: the result leaves them out.
    let live: Vec<bool> = g.opacity_logits.iter().map(|&l| sigmoid(l) >= DEAD).collect();
    g.retain(&live);
    if settings.sky_fill {
        let added = crate::sky::complete(&mut g, centre, far_field, crate::init::SKY);
        eprintln!("sky: {added} Gaussians where no view saw the sky");
    }
    if gpu.overflows > 0 {
        eprintln!("{} steps had more tile entries than the sort expected (dropped for that step)", gpu.overflows);
    }
    let psnr_held_out = evaluate(gpu, data, &held_out, settings.background)?;
    let sample: Vec<usize> = train_views.iter().copied().step_by((train_views.len() / 8).max(1)).collect();
    let psnr_train = evaluate(gpu, data, &sample, settings.background)?.unwrap_or(0.0);
    Ok(Outcome { gaussians: g, iterations, held_out, psnr_held_out, psnr_train, seconds: start.elapsed().as_secs_f32() })
}

/// Keyframes at a coarser resolution: images box-filtered by 2 or 4 from the training resolution, their cameras, and
/// the mask (a coarse pixel is scene only if all its pixels are).
struct Level {
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
        out.push(Level { width: w, height: h, mask: mask.clone(), scene_pixels, images: images.clone(), cameras });
    }
    out
}

/// What a depth step renders: a camera, its samples (pixel x, pixel y, eye depth, 0) and its dilation.
struct DepthTarget {
    camera: Camera,
    samples: Vec<[f32; 4]>,
    dilation: [f32; 2],
}

/// A depth frame's step on a camera whose pixel centres are its sample rays: one pixel a sample, the footprint of the
/// training resolution kept by scaling the dilation (the grid is regular up to a pixel's rounding). A frame without a
/// regular grid renders at the training resolution.
fn depth_target(d: &DepthView) -> DepthTarget {
    let dil = crate::cpu::DILATION as f32;
    let (gw, gh) = (d.grid_width as usize, d.grid_height as usize);
    if gw >= 2 && gh >= 2 {
        let (x0, y0) = (d.pixels[0][0], d.pixels[0][1]);
        let sx = (d.pixels[gw - 1][0] - x0) / (gw - 1) as f32;
        let sy = (d.pixels[(gh - 1) * gw][1] - y0) / (gh - 1) as f32;
        if sx > 0.0 && sy > 0.0 {
            let c = &d.camera;
            let camera = Camera { r: c.r, t: c.t, fx: c.fx / sx, fy: c.fy / sy, cx: (c.cx - x0) / sx, cy: (c.cy - y0) / sy, width: gw as u32, height: gh as u32 };
            let samples = (0..gw * gh).filter(|&i| d.depth[i] > 0.0 && !d.is_dynamic(i)).map(|i| [(i % gw) as f32, (i / gw) as f32, d.depth[i], 0.0]).collect();
            return DepthTarget { camera, samples, dilation: [dil / (sx * sx), dil / (sy * sy)] };
        }
    }
    let samples = d.pixels.iter().zip(&d.depth).enumerate().filter(|(i, (_, z))| **z > 0.0 && !d.is_dynamic(*i)).map(|(_, (p, z))| [p[0], p[1], *z, 0.0]).collect();
    DepthTarget { camera: d.camera, samples, dilation: [dil; 2] }
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

/// Each Gaussian's relocation weight, as relocate.wgsl's `classify` computes it on the device: −1 if dead (faded,
/// broken, or `huge` on screen), −2 − its opacity if alive in the `far` field, else its opacity.
#[cfg(test)]
pub(crate) fn weights(g: &Gaussians, huge: &[bool], far: &[bool]) -> Vec<f32> {
    (0..g.len())
        .map(|i| {
            let opacity = sigmoid(g.opacity_logits[i]);
            let broken = g.means[i].iter().chain(&g.log_scales[i]).chain(std::iter::once(&g.opacity_logits[i])).any(|c| !c.is_finite());
            if broken || opacity < DEAD || huge[i] {
                -1.0
            } else if far[i] {
                -2.0 - opacity
            } else {
                opacity
            }
        })
        .collect()
}

/// Decides a relocation round from the Gaussians' [`weights`]: dead ones and new ones, up to `grow_to`, become copies
/// of live ones sampled by opacity, never far ones (unless nothing else lives).
pub fn plan_relocation(weights: &[f32], grow_to: usize, rng: &mut fastrand::Rng) -> Relocation {
    let n = weights.len();
    let opacity = |i: usize| if weights[i] >= 0.0 { weights[i] } else { -2.0 - weights[i] };
    let (mut dead, mut alive): (Vec<usize>, Vec<usize>) = (0..n).partition(|&i| weights[i] == -1.0);
    if alive.iter().any(|&i| weights[i] >= 0.0) {
        alive.retain(|&i| weights[i] >= 0.0);
    }
    // At most a twentieth of the set moves at once, and the set grows only while it uses what it has: a scene that
    // needs fewer Gaussians than the budget would otherwise churn copies that fade again.
    let cap = (n / 20).max(1);
    let grow = if dead.len() <= cap { grow_to.saturating_sub(n) } else { 0 };
    if dead.len() > cap {
        rng.shuffle(&mut dead);
        dead.truncate(cap);
    }
    let mut plan = Relocation { adjust: Vec::new(), copies: Vec::new(), len: n };
    let moves = dead.len() + grow;
    if moves == 0 || alive.is_empty() {
        return plan;
    }
    let mut cdf = Vec::with_capacity(alive.len());
    let mut acc = 0.0f64;
    for &i in &alive {
        acc += opacity(i) as f64;
        cdf.push(acc);
    }
    let mut ratio = vec![1usize; n];
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
            let (new_o, factor) = relocation(opacity(j) as f64, ratio[j]);
            let new_logit = logit((new_o as f32).clamp(DEAD, 1.0 - 1e-4));
            plan.adjust.push([j as u32, new_logit.to_bits(), (factor as f32).max(1e-6).ln().to_bits(), 0]);
        }
    }
    let mut free = dead.into_iter();
    for &j in &picks {
        // Somewhere inside the source's footprint, so the copies don't share every gradient forever.
        let e = [normal(rng), normal(rng), normal(rng)];
        let target = free.next().unwrap_or_else(|| {
            plan.len += 1;
            plan.len - 1
        });
        plan.copies.push([target as u32, j as u32, e[0].to_bits(), e[1].to_bits(), e[2].to_bits(), 0, 0, 0]);
    }
    plan
}

/// Applies a relocation plan on the CPU (what relocate.wgsl does on the device): sources first, then copies of the
/// adjusted sources. Adam moments of every Gaussian touched restart at zero.
#[cfg(test)]
pub(crate) fn apply_relocation(g: &mut Gaussians, m1: &mut Vec<f32>, m2: &mut Vec<f32>, plan: &Relocation) {
    let restart = |m: &mut Vec<f32>, i: usize| {
        if m.len() < (i + 1) * PACKED {
            m.resize((i + 1) * PACKED, 0.0);
        }
        m[i * PACKED..(i + 1) * PACKED].fill(0.0);
    };
    for a in &plan.adjust {
        let j = a[0] as usize;
        g.opacity_logits[j] = f32::from_bits(a[1]);
        for s in &mut g.log_scales[j] {
            *s += f32::from_bits(a[2]);
        }
        restart(m1, j);
        restart(m2, j);
    }
    for c in &plan.copies {
        let (t, j) = (c[0] as usize, c[1] as usize);
        let r = rotation_matrix(g.rotations[j]);
        let s = g.log_scales[j].map(f32::exp);
        let e = [f32::from_bits(c[2]) * s[0], f32::from_bits(c[3]) * s[1], f32::from_bits(c[4]) * s[2]];
        let mut mean = g.means[j];
        for (row, m) in r.iter().zip(mean.iter_mut()) {
            *m += row[0] * e[0] + row[1] * e[1] + row[2] * e[2];
        }
        let (log_scale, rotation, opacity_logit, color) = (g.log_scales[j], g.rotations[j], g.opacity_logits[j], g.colors[j]);
        if t < g.len() {
            g.means[t] = mean;
            g.log_scales[t] = log_scale;
            g.rotations[t] = rotation;
            g.opacity_logits[t] = opacity_logit;
            g.colors[t] = color;
        } else {
            g.push(mean, log_scale, rotation, opacity_logit, color);
        }
        restart(m1, t);
        restart(m2, t);
    }
}

/// Moves dead Gaussians (faded, broken, or `huge` on screen) and new ones, up to `grow_to`, onto live ones sampled
/// by opacity, never `far` ones (unless nothing else lives), on the CPU (the tests' reference). Returns the number of
/// copies.
#[cfg(test)]
fn relocate(g: &mut Gaussians, m1: &mut Vec<f32>, m2: &mut Vec<f32>, huge: &[bool], far: &[bool], grow_to: usize, rng: &mut fastrand::Rng) -> usize {
    let plan = plan_relocation(&weights(g, huge, far), grow_to, rng);
    apply_relocation(g, m1, m2, &plan);
    plan.copies.len()
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
    fn the_depth_grid_camera_sees_every_sample_on_its_own_pixel() {
        use orbiters_contracts::depth::DepthLayout;
        let (w, h) = (1920u32, 1080u32);
        let layout = DepthLayout::new(w, h).expect("layout");
        let pose = [0.96, 0.0, 0.28, 0.5, 0.0, 1.0, 0.0, 1.6, -0.28, 0.0, 0.96, -2.0];
        let camera = Camera::from_strip(&pose, &[1.2, 1.2 * w as f32 / h as f32, 0.0, 0.0], w, h, 1.0);
        let (gw, gh) = (layout.tiles_x * orbiters_contracts::depth::SAMPLES_X, layout.tiles_y * orbiters_contracts::depth::SAMPLES_Y);
        let mut pixels = Vec::new();
        for gy in 0..gh {
            for gx in 0..gw {
                let (px, py) = layout.grid_pixel(gx, gy);
                pixels.push([px as f32, py as f32]);
            }
        }
        let depth: Vec<f32> = (0..pixels.len()).map(|i| 1.0 + (i % 7) as f32).collect();
        let view = DepthView { camera, grid_width: gw, grid_height: gh, pixels: pixels.clone(), depth: depth.clone(), game_time: 0.0, spacing: 8.0, dynamic: Vec::new() };
        let t = depth_target(&view);
        assert_eq!((t.camera.width, t.camera.height), (gw, gh));
        assert_eq!(t.samples.len(), pixels.len());
        // Each sample's world point lands on its grid pixel: exactly across, within the rows' rounding down.
        let mut worst = [0f32; 2];
        for s in &t.samples {
            let i = (s[1] as u32 * gw + s[0] as u32) as usize;
            let p = camera.unproject(pixels[i][0], pixels[i][1], depth[i]);
            let (u, v, z) = t.camera.project(p).expect("in front");
            assert!((z - depth[i]).abs() < 1e-3 * depth[i]);
            worst = [worst[0].max((u - s[0]).abs()), worst[1].max((v - s[1]).abs())];
        }
        assert!(worst[0] < 1e-3 && worst[1] < 0.1, "{worst:?}");
    }

    #[test]
    fn scaled_cameras_match_box_filtered_pixels() {
        let pose = [0.96, 0.0, 0.28, 0.5, 0.0, 1.0, 0.0, 1.6, -0.28, 0.0, 0.96, -2.0];
        let cam = Camera::from_strip(&pose, &[1.2, 2.1, 0.01, -0.02], 1920, 1080, 1.0);
        for factor in [2u32, 4] {
            let s = scaled(&cam, factor);
            assert_eq!((s.width, s.height), (1920 / factor, 1080 / factor));
            for (x, y) in [(0.0f32, 0.0f32), (957.3, 211.8), (1919.0, 1079.0)] {
                let p = cam.unproject(x, y, 3.5);
                let (u, v, _) = s.project(p).expect("in front");
                // Pixel x of the coarse image averages pixels f·x .. f·x + f − 1 of the full one.
                let f = factor as f32;
                assert!((u - ((x + 0.5) / f - 0.5)).abs() < 1e-3 && (v - ((y + 0.5) / f - 0.5)).abs() < 1e-3, "{factor}: {u} {v}");
            }
        }
    }

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
