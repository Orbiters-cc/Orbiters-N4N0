//! Training in replay mode: Adam on the keyframes' colour ((1 − λ)·L1 + λ·D-SSIM over the scene pixels) and the
//! exact depth samples (relative L1), with MCMC-style relocation (Kheradmand et al. 2024): every so often, Gaussians
//! that faded out move onto live ones picked by opacity, the copies sharing the original's opacity and footprint, while
//! the set grows to its budget. Depth steps render only the depth samples' rays; colour steps train on every other
//! pixel each way for the first half of the run (and can run coarse to fine). Every `hold_out_every`-th keyframe is never
//! trained on and scores the result.

use crate::aberration::Undo;
use crate::camera::Camera;
use crate::dataset::{channel, Dataset, DepthView};
use crate::gaussians::{logit, sigmoid, Gaussians};
#[cfg(test)]
use crate::gaussians::{rotation_matrix, PACKED};
use crate::gpu::{Gpu, LearningRates, Relocation, Target};
use anyhow::ensure;
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
    /// The regularisers pull on every Gaussian every step, seen in it or not, as 3DGS-MCMC's do (off: only on the
    /// Gaussians each step shows, so a Gaussian's regularisation follows how often it is seen, and one no view shows
    /// is never pulled toward DEAD). Off until measured: on, a seed no step has shown yet gets only the regulariser's
    /// gradient, which Adam turns into full opacity-rate steps, so on a long walk most seeds would fade before a view
    /// shows them; `opacity_reg` needs retuning with it.
    pub regularise_unseen: bool,
    /// A relocated or faded Gaussian counts Adam's bias correction from its restart (about one learning rate a step
    /// while it settles) instead of from the run's first step, as 3DGS-MCMC does (3 to 6 times it). Off until
    /// measured: on, training moves away from the reference and from the runs the defaults were tuned on.
    pub adam_restart: bool,
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
    /// Colour steps before this fraction of the run train on every `stride`-th pixel each way, from a phase drawn each
    /// step: a camera whose pixel centres are those pixels' rays, the target and mask sampled there, and the 2D filter
    /// scaled to keep the training resolution's footprint, so each step trains the full-resolution model on an unbiased
    /// subset of its pixels (D-SSIM on the sub-grid). Half the run at stride 2 trained VRChat Home 16 % faster (70.8 →
    /// 59.8 s), Exoplanet Journey 13 %, the synthetic plaza 9 % and Canine 5 %, at equal held-out PSNR on each (three to
    /// five interleaved runs; the plaza's off-path scores equal too); with 1.2 times the steps, in the same time as
    /// none, Home and Exoplanet scored 0.3 dB higher. Unlike coarse to fine (box-filtered keyframes and the filter
    /// unscaled: −1.2 dB on Home), nothing here learns a blurred model. 0 or a stride of 1: every pixel.
    pub stride_until: f32,
    pub stride: u32,
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
    /// Covers the sky shell where no view looked (crate::sky) once training ends. On the capture simulator's off-path
    /// views (Bench Plaza: other heights, sideways, looking up) it gained 8 dB looking up and 1.3–1.8 dB over all
    /// views of three captures, for about a thousand Gaussians. Backing every cell the shell does not cover whole, from
    /// behind it, rather than only the empty ones, took the dark streaks off the edge of what was seen and scored 0.1 dB
    /// higher looking up and 0.15–0.18 dB on held-out keyframes on the same trained sets; completing it half way through
    /// training instead made no difference beyond the runs' noise (21.30 against 21.20 dB off-path, three and five runs).
    pub sky_fill: bool,
    /// Mip-Splatting's 3D filter's scale (crate::cpu::FILTER_3D; 0: none): no Gaussian is finer than a fifth of a
    /// pixel² at the keyframe that sees it most closely. Recomputed every 100 steps and after each relocation round,
    /// and fused into the result (crate::gaussians::Gaussians::fuse_filter). Off: on the off-path benchmark (plaza walk,
    /// three runs each) it cost 0.25–0.36 dB at the views' scale and below, and gained nothing at a 4× zoom-in, where
    /// the compensated 2D filter alone (crate::cpu::FILTER_2D) scored 5 dB above 3DGS's dilation.
    pub filter_3d: f32,
    /// Leaves out what the exact depth says is not there once training ends: floaters in front of measured surfaces and
    /// islands no depth saw that do not join the ground under the walked path (crate::cleanup).
    pub cleanup: bool,
    /// The sky as a map this many texels across behind the splats (crate::sky::SkyMap), seeded and trained beside
    /// them; the shell's discs stay only where the parallax test cannot confirm sky (crate::sky::give_way). 0, or a
    /// capture none of whose pixels saw the sky: the shell of discs and its completion ([`Settings::sky_fill`]).
    pub sky_map: u32,
    /// Of the colour steps, the largest share that trains on the capture rig's faces when the session has any
    /// (crate::dataset::Faces): views around the camera's path the keyframes never had, at a lower resolution and with
    /// no depth. A session with fewer faces than keyframes (a recording's rig moments, rather than a 3D picture's whole
    /// flight) gives them their share of the views instead ([`face_share`]). Those steps leave the sky map as it is: what
    /// only a face sees has no depth to tell the sky from a wall.
    pub face_share: f32,
    /// Depth seeded too few Gaussians, and seeds along the keyframes' rays made up for it (crate::init::add_from_rays;
    /// the worker sets it): relocation then grows the set while it is under a quarter of the budget, even while many
    /// Gaussians are dead. Otherwise the set grows only while few are, and a set that starts at a thousand never does.
    pub starved: bool,
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
            regularise_unseen: false,
            adam_restart: false,
            max_radius: 0.1,
            background: [0.0; 3],
            seed: 1,
            ssim: 0.2,
            face_share: 0.5,
            robust: 0.0,
            robust_from: 0.25,
            quarter_until: 0.0,
            half_until: 0.0,
            stride_until: 0.5,
            stride: 2,
            relocate_until: 0.5,
            fade_unseen_every: 1000,
            unseen_weight: 0.02,
            sky_fill: true,
            filter_3d: 0.0,
            cleanup: false,
            sky_map: 2048,
            starved: false,
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
    /// Mean PSNR over up to 16 held-out keyframes, when measured at this report.
    pub psnr: Option<f32>,
    pub seconds: f32,
}

/// The result of a run: its Gaussians, which are also what the device holds now (scored with [`score_held_out`]).
pub struct Outcome {
    pub gaussians: Gaussians,
    pub iterations: u32,
    pub held_out: Vec<usize>,
    pub psnr_train: f32,
    pub seconds: f32,
}

/// The keyframes a run holds out (never trained on; they score the result): every `every`-th, from the middle of the
/// first `every`.
pub fn held_out(views: usize, every: usize) -> Vec<usize> {
    if every > 1 {
        (0..views).filter(|i| i % every == every / 2).collect()
    } else {
        Vec::new()
    }
}

/// At most `k` of `items`, spread evenly over them.
fn evenly(items: &[usize], k: usize) -> Vec<usize> {
    if items.len() <= k {
        return items.to_vec();
    }
    (0..k).map(|j| items[j * items.len() / k]).collect()
}

/// The cameras' mean position.
pub fn camera_centre(data: &Dataset) -> [f32; 3] {
    let n = data.views.len().max(1) as f32;
    data.views.iter().map(|v| v.camera.position()).fold([0.0f32; 3], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n, a[2] + p[2] / n])
}

/// A typical scene size: the larger of the cameras' spread and the median sampled depth (sets the position step).
pub fn extent(data: &Dataset) -> f32 {
    let positions: Vec<[f32; 3]> = data.views.iter().map(|v| v.camera.position()).collect();
    let n = positions.len().max(1) as f32;
    let c = positions.iter().fold([0.0f32; 3], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n, a[2] + p[2] / n]);
    let spread = positions.iter().map(|p| ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt()).fold(0.0, f32::max) * 1.1;
    // The median of at most about a million trusted samples (all of a long session's would take hundreds of MB).
    let total: usize = data.depth_views.iter().map(|d| d.depth.len()).sum();
    let stride = (total / 1_000_000).max(1);
    let mut depths: Vec<f32> =
        data.depth_views.iter().flat_map(|d| (0..d.depth.len()).map(move |i| (d, i))).step_by(stride).filter(|(d, i)| d.depth[*i] > 0.0 && !d.is_unknown(*i)).map(|(d, i)| d.depth[i]).collect();
    let median = if depths.is_empty() {
        1.0
    } else {
        let k = depths.len() / 2;
        *depths.select_nth_unstable_by(k, f32::total_cmp).1
    };
    spread.max(median).max(0.5)
}

/// 100 steps per keyframe: VRChat Home trains as well at 75 to 150, and with relocation until half the run the Canine
/// orbit scores higher at 100 than it did at 150. The rig's faces add none: on the plaza walk with rig frames, the
/// keyframes' steps scored 26.8 dB off the path where 2.4 times as many (faces counted half a keyframe) scored 25.9.
pub fn default_iterations(data: &Dataset) -> u32 {
    (data.views.len() as u32 * 100).clamp(3_000, 30_000)
}

/// The share of colour steps on the rig's faces: their share of all the views, at most `most`
/// ([`Settings::face_share`]): a 3D picture's faces outnumber its keyframes and get `most`; a recording's few rig
/// moments get as many steps per face as a keyframe gets.
pub fn face_share(faces: usize, keyframes: usize, most: f32) -> f32 {
    if faces == 0 {
        return 0.0;
    }
    (faces as f32 / (faces + keyframes) as f32).min(most)
}

/// PSNR over the scene pixels (rendered colour clamped to 0..1); NaN without any.
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
    if count == 0.0 {
        return f32::NAN;
    }
    let mse = (se / count).max(1e-10);
    (10.0 * (1.0 / mse).log10()) as f32
}

/// Renders the Gaussians on the GPU from a camera: (r, g, b, depth) per pixel.
pub fn render(gpu: &mut Gpu, cam: &Camera, background: [f32; 3]) -> anyhow::Result<Vec<[f32; 4]>> {
    gpu.step(cam, background, Target::None, None)?;
    gpu.read_image()
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

/// Renders each of `views` once from what the device holds, hands each render to `each` (previews), and returns their
/// mean PSNR over the scene pixels. With the world's chromatic aberration undone in the keyframes (`undo`), a render
/// passes through that aberration and is scored against the keyframe as captured (read again from the session), as
/// the lab scored it: a plain render against the realigned keyframe reads about 0.1 dB high, against the captured one
/// low. The device renders eight views, the loading threads score and hand them on together, then the next eight (the
/// same scores, summed in view order).
pub fn score_held_out(
    gpu: &mut Gpu,
    data: &Dataset,
    undo: Option<&Undo>,
    views: &[usize],
    background: [f32; 3],
    each: impl Fn(usize, &[[f32; 4]]) -> anyhow::Result<()> + Sync,
) -> anyhow::Result<Option<f32>> {
    use rayon::prelude::*;
    let mut scores = Vec::with_capacity(views.len());
    for chunk in views.chunks(8) {
        let renders: Vec<(usize, Vec<[f32; 4]>)> = chunk.iter().map(|&i| Ok((i, render(gpu, &data.views[i].camera, background)?))).collect::<anyhow::Result<_>>()?;
        let chunk_scores: Vec<anyhow::Result<f32>> = crate::priority::on_loading_threads(|| {
            renders
                .par_iter()
                .map(|(i, img)| {
                    let captured = undo.and_then(|u| match crate::dataset::keyframe_image(&data.dir, data.views[*i].keyframe, &data.geometry) {
                        Ok(captured) => Some((u, captured)),
                        Err(e) => {
                            eprintln!("keyframe {} scored as loaded: {e:#}", data.views[*i].keyframe);
                            None
                        }
                    });
                    let psnr = match captured {
                        Some((u, captured)) => {
                            let seen = crate::aberration::aberrate(img, data.width as usize, data.height as usize, u.intensity, u.factor);
                            psnr(&seen, &captured, &u.mask)
                        }
                        None => psnr(img, &data.views[*i].image, &data.mask),
                    };
                    ensure!(psnr.is_finite(), "keyframe {} has no scene pixel to score", data.views[*i].keyframe);
                    each(*i, img)?;
                    Ok(psnr)
                })
                .collect()
        });
        for s in chunk_scores {
            scores.push(s?);
        }
    }
    Ok((!views.is_empty()).then(|| scores.iter().sum::<f32>() / views.len() as f32))
}

/// Whether step `it` fades what no training pixel sees: every `fade_unseen_every` steps from step 1000, while a
/// relocation round is still to come to reuse the slots.
fn fade_due(it: u32, s: &Settings, relocate_until: u32) -> bool {
    s.fade_unseen_every > 0 && it.is_multiple_of(s.fade_unseen_every) && it >= 1000 && it + s.relocate_every <= relocate_until
}

/// The Gaussians a result keeps: opaque enough to show (faded ones that relocation never reused show nowhere: it
/// moves at most a twentieth a round, and stops at `relocate_until`), and made of numbers.
pub fn live(g: &Gaussians) -> Vec<bool> {
    (0..g.len()).map(|i| g.is_finite(i) && sigmoid(g.opacity_logits[i]) >= DEAD).collect()
}

pub fn train(data: &Dataset, gpu: &mut Gpu, init: Gaussians, settings: &Settings, mut report: impl FnMut(&Progress)) -> anyhow::Result<Outcome> {
    let start = Instant::now();
    let iterations = if settings.iterations == 0 { default_iterations(data) } else { settings.iterations };
    let held_out = held_out(data.views.len(), settings.hold_out_every);
    let train_views: Vec<usize> = (0..data.views.len()).filter(|i| held_out.binary_search(i).is_err()).collect();
    ensure!(!train_views.is_empty(), "every keyframe is held out: nothing to train on");
    ensure!(!init.is_empty(), "no Gaussians to start from");
    // A session's frames whose every pixel is strip or band (or a mask never set) would train on nothing and score
    // nothing.
    let scene_pixels = data.mask.iter().filter(|m| **m > 0.5).count() as f32;
    ensure!(scene_pixels > 0.0, "the capture strip and the skeleton band cover all of these {}×{} keyframes: nothing to train on", data.width, data.height);
    let depth_views: Vec<usize> = (0..data.depth_views.len()).filter(|&i| data.depth_views[i].valid() > 0).collect();
    // The rig's faces, when they have scene pixels and a size of their own (their mask is kept per size).
    let faces = data.faces.as_ref().filter(|f| !f.views.is_empty() && (f.width, f.height) != (data.width, data.height));
    let face_pixels = faces.map_or(0.0, |f| f.mask.iter().filter(|m| **m > 0.5).count() as f32);
    let faces = faces.filter(|_| face_pixels > 0.0);
    let face_cams: Vec<Camera> = faces.map_or_else(Vec::new, |f| f.views.iter().map(|v| v.camera).collect());
    let face_steps = face_share(face_cams.len(), data.views.len(), settings.face_share);
    let extent = extent(data);
    let mut rng = fastrand::Rng::with_seed(settings.seed);
    // Mid-run scores on at most 16 held-out keyframes: enough to follow, cheap at any session length.
    let probe = evenly(&held_out, 16);

    // Feed the GPU even while another program keeps every core busy (crate::priority).
    let feeding = crate::priority::Feeding::start();
    let dev = gpu.device.clone();
    let setup_start = Instant::now();
    // The set grows to the budget: buffers of that size from the start never grow (and copy) on the way.
    gpu.reserve(settings.budget.max(init.len()));
    gpu.upload(&init);
    // The 3D filter follows the keyframes trained on, at their full resolution (the finest sampling).
    let filter_cams: Vec<Camera> = train_views.iter().map(|&i| data.views[i].camera).chain(face_cams.iter().copied()).collect();
    gpu.update_filter_3d(&filter_cams, settings.filter_3d)?;
    let coarse = if settings.half_until > 0.0 { levels(data) } else { Vec::new() };
    for l in &coarse {
        gpu.set_mask(l.width, l.height, &l.mask)?;
    }
    gpu.set_mask(data.width, data.height, &data.mask)?;
    if let Some(f) = faces {
        gpu.set_mask(f.width, f.height, &f.mask)?;
    }
    gpu.regularise_unseen = settings.regularise_unseen;
    gpu.adam_restart = settings.adam_restart;
    // Each phase of the colour steps' sub-grid (Settings::stride): its size, mask and scene pixels.
    let stride = settings.stride.max(1);
    let phases: Vec<(u32, u32, Vec<f32>, f32)> = if settings.stride_until > 0.0 && stride > 1 {
        (0..stride * stride)
            .map(|ph| {
                let (ox, oy) = (ph % stride, ph / stride);
                let (w, h) = ((data.width - ox).div_ceil(stride), (data.height - oy).div_ceil(stride));
                let mask: Vec<f32> = (0..h).flat_map(|j| (0..w).map(move |i| data.mask[((stride * j + oy) * data.width + stride * i + ox) as usize])).collect();
                let scene = mask.iter().filter(|m| **m > 0.5).count() as f32;
                (w, h, mask, scene)
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut sub_grid: Vec<u32> = Vec::new();
    let mut mask_phase = None;
    // The keyframes on the device when its memory has room for them beside the set (with a GB to spare): steps then
    // upload no pixels.
    const LEFT: u64 = 1 << 30;
    let keyframe_bytes: u64 = data.views.iter().map(|v| v.image.len() as u64 * 4).sum();
    let resident = gpu.free_memory().is_some_and(|free| free >= gpu.training_memory(settings.budget) + keyframe_bytes + LEFT);
    if resident {
        let images: Vec<&[u32]> = data.views.iter().map(|v| v.image.as_slice()).collect();
        gpu.set_keyframes(&images);
    }
    let regularise = |gpu: &mut Gpu, n: usize| {
        gpu.opacity_reg = settings.opacity_reg / n.max(1) as f32;
        gpu.scale_reg = settings.scale_reg / (n.max(1) as f32 * extent);
    };
    regularise(gpu, init.len());
    gpu.step = 0;
    crate::timing::add("4a train setup", setup_start.elapsed());
    let relocate_until = (iterations as f32 * settings.relocate_until) as u32;
    // Relocation rounds off the steps' path: a round's weights come back while training goes on, a helper thread plans
    // it (its own random stream), and the plan lands a few steps later (VRChat Home: −4 %, quality equal): the plan took
    // 20–30 ms of the CPU a round, with the GPU waiting.
    let mut pending_weights: Option<(u32, crate::gpu::PendingWeights)> = None;
    let mut pending_plan: Option<std::thread::JoinHandle<Relocation>> = None;
    for it in 1..=iterations {
        feeding.follow();
        if it % 500 == 1 {
            // While VRChat runs on this GPU, SteamVR's frame timing paces the steps (crate::governor).
            let beside = crate::priority::vrchat() && !gpu.integrated;
            if beside && gpu.governor.is_none() {
                gpu.governor = crate::governor::Governor::connect();
                if gpu.governor.is_some() {
                    eprintln!("it {it}: training between VRChat's frames (SteamVR's frame timing)");
                }
            } else if !beside && gpu.governor.is_some() {
                gpu.governor = None;
                eprintln!("it {it}: VRChat closed: training at full speed");
            }
        }
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
            // Built for the step (all of a long session's targets at once would take GBs).
            let t = depth_target(&data.depth_views[depth_views[rng.usize(..depth_views.len())]]);
            let full = gpu.filter_2d;
            gpu.filter_2d = t.filter_2d;
            let stepped = gpu.step(&t.camera, settings.background, Target::Depth { samples: &t.samples, weight: settings.depth_weight }, Some(lr));
            gpu.filter_2d = full;
            stepped?
        } else if let Some(f) = faces.filter(|_| rng.f32() < face_steps) {
            // A face, at its own resolution, the sky map left as it is.
            let v = &f.views[rng.usize(..f.views.len())];
            let robust = if progress >= settings.robust_from { settings.robust } else { 0.0 };
            let sky_lr = std::mem::replace(&mut gpu.sky_lr, 0.0);
            let stepped = gpu.step(&v.camera, settings.background, Target::Color { rgba: &v.image, scene_pixels: face_pixels, weight: 1.0, ssim: settings.ssim, robust }, Some(lr));
            gpu.sky_lr = sky_lr;
            stepped?
        } else {
            let v = train_views[rng.usize(..train_views.len())];
            let (camera, rgba, pixels) = match level {
                0 => (&data.views[v].camera, &data.views[v].image, scene_pixels),
                l => (&coarse[l - 1].cameras[v], &coarse[l - 1].images[v], coarse[l - 1].scene_pixels),
            };
            let robust = if progress >= settings.robust_from { settings.robust } else { 0.0 };
            if level == 0 && progress < settings.stride_until && !phases.is_empty() {
                let ph = rng.u32(..stride * stride);
                let (ox, oy) = (ph % stride, ph / stride);
                let (w, h, mask, scene) = &phases[ph as usize];
                if mask_phase != Some(ph) {
                    gpu.set_mask(*w, *h, mask)?;
                    mask_phase = Some(ph);
                }
                let c = &data.views[v].camera;
                let f = stride as f32;
                let cam = Camera { r: c.r, t: c.t, fx: c.fx / f, fy: c.fy / f, cx: (c.cx - ox as f32) / f, cy: (c.cy - oy as f32) / f, width: *w, height: *h };
                let img = &data.views[v].image;
                sub_grid.clear();
                for j in 0..*h {
                    let row = ((stride * j + oy) * data.width) as usize;
                    sub_grid.extend((0..*w).map(|i| img[row + (stride * i + ox) as usize]));
                }
                let full = gpu.filter_2d;
                gpu.filter_2d = [full[0] / (f * f), full[1] / (f * f)];
                let stepped = gpu.step(&cam, settings.background, Target::Color { rgba: &sub_grid, scene_pixels: *scene, weight: 1.0, ssim: settings.ssim, robust }, Some(lr));
                gpu.filter_2d = full;
                stepped?
            } else {
                if resident && level == 0 {
                    gpu.resident_target = Some(v);
                }
                gpu.step(camera, settings.background, Target::Color { rgba, scene_pixels: pixels, weight: 1.0, ssim: settings.ssim, robust }, Some(lr))?
            }
        };
        if fade_due(it, settings, relocate_until) {
            let _sync = crate::timing::on().then(|| dev.poll(wgpu::PollType::wait_indefinitely()));
            let fade_start = Instant::now();
            let _cpu = crate::priority::CpuWork::start();
            // The faces' views count: what only they see is not unseen.
            let cams: Vec<Camera> = train_views.iter().map(|&i| data.views[i].camera).chain(face_cams.iter().copied()).collect();
            let seen = gpu.visibility(&cams, settings.background, settings.unseen_weight)?;
            if seen.iter().all(|w| *w <= 0.0) {
                // Nothing seen at all is a broken round (a mask of zeros, a lost readback), not a model nothing sees.
                eprintln!("it {it}: no training pixel sees any Gaussian; nothing faded");
            } else {
                let faded = logit(1e-4).to_bits();
                let adjust: Vec<[u32; 4]> = seen.iter().enumerate().filter(|(_, w)| **w < settings.unseen_weight).map(|(i, _)| [i as u32, faded, 0f32.to_bits(), 0]).collect();
                eprintln!("it {it}: fading {} of {} Gaussians no training pixel sees", adjust.len(), seen.len());
                gpu.apply_relocation(&Relocation { adjust, copies: Vec::new(), len: seen.len() })?;
            }
            let _sync = crate::timing::on().then(|| dev.poll(wgpu::PollType::wait_indefinitely()));
            crate::timing::add("4b fade", fade_start.elapsed());
        }
        if it % 50 == 0 || it == iterations {
            // The last step's score is the result's, measured once it is final (crate::train::score_held_out).
            let psnr = if it % 1000 == 0 && it != iterations {
                let _cpu = crate::priority::CpuWork::start();
                crate::timing::section("4c probe scores", Some(&dev), || evaluate(gpu, data, &probe, settings.background))?
            } else {
                None
            };
            report(&Progress { iteration: it, iterations, gaussians: gpu.count, intersections, psnr, seconds: start.elapsed().as_secs_f32() });
        }
        {
            // A plan that is ready lands now; weights that arrived go to a planner.
            if pending_plan.as_ref().is_some_and(|h| h.is_finished()) {
                let plan = pending_plan.take().expect("checked").join().map_err(|_| anyhow::anyhow!("the relocation planner panicked"))?;
                crate::timing::section("4d3 relocation apply", None, || gpu.apply_relocation(&plan))?;
                regularise(gpu, plan.len);
                gpu.update_filter_3d(&filter_cams, settings.filter_3d)?;
            }
            if pending_plan.is_none() {
                if let Some((round, w)) = pending_weights.take() {
                    match w.take()? {
                        Some(weights) => {
                            let n = weights.len();
                            if round % 500 == 0 {
                                let dead = weights.iter().filter(|w| **w == -1.0).count();
                                eprintln!("it {round}: {n} Gaussians, {dead} dead or huge, {intersections} entries");
                            }
                            let (grow_to, starved) = ((n + n / 20).min(settings.budget), settings.starved && n < settings.budget / 4);
                            let mut plan_rng = fastrand::Rng::with_seed(settings.seed ^ (round as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
                            pending_plan = Some(std::thread::spawn(move || plan_relocation(&weights, grow_to, starved, &mut plan_rng)));
                        }
                        None => pending_weights = Some((round, w)),
                    }
                }
            }
        }
        if it % settings.relocate_every == 0 && it >= 500 && it <= relocate_until {
            // The last round's plan must land before the next round reads the set (rare: rounds are 100 steps apart).
            if let Some(h) = pending_plan.take() {
                let plan = h.join().map_err(|_| anyhow::anyhow!("the relocation planner panicked"))?;
                gpu.apply_relocation(&plan)?;
                regularise(gpu, plan.len);
            }
            if pending_weights.is_none() {
                pending_weights = Some((it, gpu.request_relocation_weights(settings.max_radius)?));
            }
        } else if it % 100 == 0 && it + 100 <= iterations {
            // As Mip-Splatting does: every 100 steps but the last 100's.
            crate::timing::section("4e 3D filter", Some(&dev), || gpu.update_filter_3d(&filter_cams, settings.filter_3d))?;
        }
    }
    drop(feeding);
    // A round still on its way (none should be: rounds end at `relocate_until`) moves nothing now.
    if let Some(h) = pending_plan.take() {
        let _ = h.join();
    }
    drop(pending_weights);
    let _sync = crate::timing::on().then(|| dev.poll(wgpu::PollType::wait_indefinitely()));
    let end_start = Instant::now();
    if let Some(g) = &gpu.governor {
        eprintln!("governor: {:.1} s waiting for VRChat's frames, {} back-offs", g.waited.as_secs_f32(), g.backoffs);
    }
    // The result keeps the filter in its scales and opacities: the PLY renders what training rendered.
    let mut g = gpu.download_params()?;
    g.fuse_filter();
    g.retain(&live(&g));
    ensure!(!g.is_empty(), "training left no Gaussian that shows anywhere");
    if settings.sky_fill {
        // The sky shell's far field, as the measurements took it: beyond half the shell's distance from the cameras.
        let added = crate::sky::complete(&mut g, camera_centre(data), crate::init::SKY * 0.5, crate::init::SKY);
        eprintln!("sky: {added} Gaussians where no view saw the sky");
    }
    // From here on the device holds the result: what the PLY keeps is what scores.
    gpu.upload(&g);
    // And what the keyframes saw of each, which the PLY keeps for viewers (crate::gaussians::Seen).
    g.seen = gpu.seen(&filter_cams, settings.background)?;
    let beyond = crate::cleanup::beyond(&g, &data.depth_views);
    for (s, b) in g.seen.iter_mut().zip(beyond) {
        s.beyond = b;
    }
    if settings.cleanup {
        let _cpu = crate::priority::CpuWork::start();
        let focal = filter_cams[0].fx.max(filter_cams[0].fy);
        let (keep, cleaned) = crate::cleanup::clean(&g, &data.depth_views, &filter_cams, focal);
        eprintln!("cleanup: {} floaters and {} islands left out of {}", cleaned.floaters, cleaned.islands, g.len());
        if cleaned.floaters + cleaned.islands > 0 {
            g.retain(&keep);
            gpu.upload(&g);
        }
    }
    if gpu.overflows > 0 {
        eprintln!("{} steps had more tile entries than the sort expected (dropped for that step)", gpu.overflows);
    }
    let sample: Vec<usize> = train_views.iter().copied().step_by((train_views.len() / 8).max(1)).collect();
    let psnr_train = evaluate(gpu, data, &sample, settings.background)?.unwrap_or(0.0);
    crate::timing::add("4f train end (download, seen, cleanup, train score)", end_start.elapsed());
    Ok(Outcome { gaussians: g, iterations, held_out, psnr_train, seconds: start.elapsed().as_secs_f32() })
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

/// The camera of an image box-filtered by `factor` (pixel centres at integers on both): [`Camera::scaled`].
pub fn scaled(c: &Camera, factor: u32) -> Camera {
    c.scaled(factor)
}

fn levels(data: &Dataset) -> Vec<Level> {
    let mut out: Vec<Level> = Vec::new();
    for factor in [2u32, 4] {
        // Each level halves the one before it.
        let (images, mask, w, h) = match out.last() {
            None => (data.views.iter().map(|v| half_image(&v.image, data.width, data.height)).collect::<Vec<_>>(), half_mask(&data.mask, data.width, data.height), data.width / 2, data.height / 2),
            Some(l) => (l.images.iter().map(|img| half_image(img, l.width, l.height)).collect(), half_mask(&l.mask, l.width, l.height), l.width / 2, l.height / 2),
        };
        let cameras = data.views.iter().map(|v| v.camera.scaled(factor)).collect();
        let scene_pixels = mask.iter().filter(|m| **m > 0.5).count() as f32;
        out.push(Level { width: w, height: h, mask, scene_pixels, images, cameras });
    }
    out
}

/// What a depth step renders: a camera, its samples (pixel x, pixel y, eye depth, weight) and its 2D filter.
pub struct DepthTarget {
    pub camera: Camera,
    pub samples: Vec<[f32; 4]>,
    pub filter_2d: [f32; 2],
}

/// A depth frame's step on a camera whose pixel centres are its sample rays: one pixel a sample, the footprint of the
/// training resolution kept by scaling the 2D filter (the grid is regular up to a pixel's rounding). A frame without a
/// regular grid renders at the training resolution. Only trusted samples are targets.
pub fn depth_target(d: &DepthView) -> DepthTarget {
    let filter = crate::cpu::FILTER_2D as f32;
    let (gw, gh) = (d.grid.width as usize, d.grid.height as usize);
    let px = &d.grid.pixels;
    let n = d.depth.len().min(px.len());
    let trusted = |i: &usize| d.depth[*i] > 0.0 && !d.is_unknown(*i);
    // A sample of a larger tile towards the frame's edges stands for more of the frame: it weighs its tile's sample
    // spacing (1 in a grid tile). By the spacing, not the area, which put up to 64 times a grid sample's weight on one
    // pixel: on the off-path benchmark area weights gained 0.5 dB over equal ones, spacing weights 1.3 dB.
    let weight = |i: usize| d.grid.step(i) as f32;
    if gw >= 2 && gh >= 2 && n == gw * gh {
        let (x0, y0) = (px[0][0], px[0][1]);
        let sx = (px[gw - 1][0] - x0) / (gw - 1) as f32;
        let sy = (px[(gh - 1) * gw][1] - y0) / (gh - 1) as f32;
        if sx > 0.0 && sy > 0.0 {
            let c = &d.camera;
            let camera = Camera { r: c.r, t: c.t, fx: c.fx / sx, fy: c.fy / sy, cx: (c.cx - x0) / sx, cy: (c.cy - y0) / sy, width: gw as u32, height: gh as u32 };
            let samples = (0..n).filter(trusted).map(|i| [(i % gw) as f32, (i / gw) as f32, d.depth[i], weight(i)]).collect();
            return DepthTarget { camera, samples, filter_2d: [filter / (sx * sx), filter / (sy * sy)] };
        }
    }
    let samples = (0..n).filter(trusted).map(|i| [px[i][0], px[i][1], d.depth[i], 0.0]).collect();
    DepthTarget { camera: d.camera, samples, filter_2d: [filter; 2] }
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
/// broken, or `huge` on screen), −2 − its opacity if alive on the sky shell, else its opacity.
#[cfg(test)]
pub(crate) fn weights(g: &Gaussians, huge: &[bool]) -> Vec<f32> {
    (0..g.len())
        .map(|i| {
            let opacity = sigmoid(g.opacity_logits[i]);
            if !g.is_finite(i) || opacity < DEAD || huge[i] {
                -1.0
            } else if g.sky[i] {
                -2.0 - opacity
            } else {
                opacity
            }
        })
        .collect()
}

/// Decides a relocation round from the Gaussians' [`weights`]: dead ones and new ones, up to `grow_to`, become copies
/// of live ones sampled by opacity, never sky ones (unless nothing else lives). A weight that is no number counts as
/// dead; weights that sum to nothing (a readback of zeros) move nothing. `starved` ([`Settings::starved`]): the set
/// grows whatever number are dead.
pub fn plan_relocation(weights: &[f32], grow_to: usize, starved: bool, rng: &mut fastrand::Rng) -> Relocation {
    let n = weights.len();
    let opacity = |i: usize| if weights[i] >= 0.0 { weights[i] } else { -2.0 - weights[i] };
    let (mut dead, mut alive): (Vec<usize>, Vec<usize>) = (0..n).partition(|&i| weights[i] == -1.0 || !weights[i].is_finite());
    if alive.iter().any(|&i| weights[i] >= 0.0) {
        alive.retain(|&i| weights[i] >= 0.0);
    }
    // At most a twentieth of the set moves at once, and the set grows only while it uses what it has: a scene that
    // needs fewer Gaussians than the budget would otherwise churn copies that fade again.
    let cap = (n / 20).max(1);
    let grow = if dead.len() <= cap || starved { grow_to.saturating_sub(n) } else { 0 };
    if dead.len() > cap {
        rng.shuffle(&mut dead);
        dead.truncate(cap);
    }
    let mut plan = Relocation { adjust: Vec::new(), copies: Vec::new(), len: n };
    // No more moves than the live ones can take (MAX_RATIO − 1 copies each): after a mass fade a few sources remain.
    let moves = (dead.len() + grow).min(alive.len() * (MAX_RATIO - 1));
    if moves == 0 {
        return plan;
    }
    let mut pool = alive.clone();
    let mut cdf = Vec::with_capacity(pool.len());
    let build = |pool: &[usize], cdf: &mut Vec<f64>| {
        cdf.clear();
        let mut acc = 0.0f64;
        for &i in pool {
            acc += opacity(i).max(0.0) as f64;
            cdf.push(acc);
        }
        acc
    };
    let mut acc = build(&pool, &mut cdf);
    if acc.is_nan() || acc <= 0.0 {
        return plan;
    }
    let mut ratio = vec![1usize; n];
    let mut picks = Vec::with_capacity(moves);
    let mut misses = 0;
    while picks.len() < moves {
        let r = rng.f64() * acc;
        let j = pool[cdf.partition_point(|&c| c < r).min(pool.len() - 1)];
        if ratio[j] >= MAX_RATIO {
            // Most of the weight sits on sources that took all they can: draw from the others from now on.
            misses += 1;
            if misses >= 16 {
                pool.retain(|&i| ratio[i] < MAX_RATIO);
                acc = build(&pool, &mut cdf);
                misses = 0;
                if pool.is_empty() || acc.is_nan() || acc <= 0.0 {
                    break;
                }
            }
            continue;
        }
        misses = 0;
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
        let (log_scale, rotation, opacity_logit, color, sky) = (g.log_scales[j], g.rotations[j], g.opacity_logits[j], g.colors[j], g.sky[j]);
        if t < g.len() {
            g.means[t] = mean;
            g.log_scales[t] = log_scale;
            g.rotations[t] = rotation;
            g.opacity_logits[t] = opacity_logit;
            g.colors[t] = color;
            g.sky[t] = sky;
        } else {
            g.push(mean, log_scale, rotation, opacity_logit, color);
            *g.sky.last_mut().expect("pushed") = sky;
        }
        restart(m1, t);
        restart(m2, t);
    }
}

/// Moves dead Gaussians (faded, broken, or `huge` on screen) and new ones, up to `grow_to`, onto live ones sampled
/// by opacity, never sky ones (unless nothing else lives), on the CPU (the tests' reference). Returns the number of
/// copies.
#[cfg(test)]
fn relocate(g: &mut Gaussians, m1: &mut Vec<f32>, m2: &mut Vec<f32>, huge: &[bool], grow_to: usize, rng: &mut fastrand::Rng) -> usize {
    let plan = plan_relocation(&weights(g, huge), grow_to, false, rng);
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
    fn faces_get_their_share_of_the_steps_up_to_half() {
        assert_eq!(face_share(0, 100, 0.5), 0.0);
        // A 3D picture: more faces than keyframes, half the steps.
        assert_eq!(face_share(450, 150, 0.5), 0.5);
        // A recording's rig moments: as many steps per face as per keyframe.
        assert!((face_share(300, 1200, 0.5) - 0.2).abs() < 1e-6);
    }

    #[test]
    fn the_depth_grid_camera_sees_every_sample_on_its_own_pixel() {
        use crate::camera::Geometry;
        use crate::dataset::DepthGrid;
        use orbiters_contracts::depth::DepthLayout;
        let (w, h) = (1920u32, 1080u32);
        let layout = DepthLayout::new(w, h).expect("layout");
        let pose = [0.96, 0.0, 0.28, 0.5, 0.0, 1.0, 0.0, 1.6, -0.28, 0.0, 0.96, -2.0];
        let camera = Camera::from_strip(&pose, &[1.2, 1.2 * w as f32 / h as f32, 0.0, 0.0], w, h, 1.0);
        let grid = std::sync::Arc::new(DepthGrid::new(&layout, &Geometry::new(w, h, 1)));
        let (gw, gh) = (grid.width, grid.height);
        let pixels = grid.pixels.clone();
        let depth: Vec<f32> = (0..pixels.len()).map(|i| 1.0 + (i % 7) as f32).collect();
        let mut view = DepthView { camera, grid, depth: depth.clone(), game_time: 0.0, unknown: Default::default(), decoder_unknown: 0, zero_may_be_unread: false };
        let t = depth_target(&view);
        assert_eq!((t.camera.width, t.camera.height), (gw, gh));
        assert_eq!(t.samples.len(), pixels.len());
        // Unknown samples (and samples that hit nothing) are no targets.
        view.unknown.insert(5);
        view.depth[9] = 0.0;
        let fewer = depth_target(&view);
        assert_eq!(fewer.samples.len(), pixels.len() - 2);
        assert!(fewer.samples.iter().all(|s| (s[1] as u32 * gw + s[0] as u32) as usize != 5));
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
        let moved = relocate(&mut g, &mut m1, &mut m2, &[false; 100], 104, &mut fastrand::Rng::with_seed(1));
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
        let moved = relocate(&mut g, &mut m1, &mut m2, &[false; 100], 200, &mut fastrand::Rng::with_seed(2));
        assert_eq!(moved, 5);
        assert_eq!(g.len(), 100);
        assert_eq!(g.opacity_logits.iter().filter(|&&l| sigmoid(l) < DEAD).count(), 25);
    }

    #[test]
    fn the_sky_shell_never_spawns_copies() {
        // Three faded, forty opaque sky ones at x = 400, the rest near x = 0..2.
        let mut g = Gaussians::default();
        for i in 0..100 {
            let x = if (3..43).contains(&i) { 400.0 } else { (i % 3) as f32 };
            g.push([x, 0.0, 0.0], [-4.0; 3], [1.0, 0.0, 0.0, 0.0], if i < 3 { logit(0.001) } else { logit(0.9) }, [0.5; 3]);
            g.sky[i] = (3..43).contains(&i);
        }
        let (mut m1, mut m2) = (vec![0.0; 100 * PACKED], vec![0.0; 100 * PACKED]);
        relocate(&mut g, &mut m1, &mut m2, &[false; 100], 104, &mut fastrand::Rng::with_seed(4));
        assert_eq!(g.len(), 104);
        assert_eq!(g.means.iter().filter(|m| m[0] > 100.0).count(), 40);
        assert_eq!(g.sky.iter().filter(|s| **s).count(), 40);
        // Far geometry that is not sky (a walk wider than the sky shell's 200 m) does spawn copies.
        let mut far = Gaussians::default();
        for i in 0..100 {
            far.push([if i < 50 { -300.0 } else { 300.0 }, 0.0, 0.0], [-4.0; 3], [1.0, 0.0, 0.0, 0.0], if i < 3 { logit(0.001) } else { logit(0.9) }, [0.5; 3]);
        }
        assert_eq!(relocate(&mut far, &mut vec![0.0; 100 * PACKED], &mut vec![0.0; 100 * PACKED], &[false; 100], 104, &mut fastrand::Rng::with_seed(4)), 7);
    }

    /// A starved set (seeded from rays, crate::init::add_from_rays) grows though many of it are dead; another does not.
    #[test]
    fn a_starved_set_grows_whatever_number_are_dead() {
        let mut rng = fastrand::Rng::with_seed(5);
        // 1,000 Gaussians, 400 dead.
        let w: Vec<f32> = (0..1000).map(|i| if i % 5 < 2 { -1.0 } else { 0.5 }).collect();
        let fed = plan_relocation(&w, 1050, false, &mut rng);
        assert_eq!(fed.len, 1000, "a set that does not use what it has does not grow");
        let starved = plan_relocation(&w, 1050, true, &mut rng);
        assert_eq!(starved.len, 1050);
        assert!(starved.copies.len() >= 50 + 50, "a twentieth of the dead moved, and fifty new: {}", starved.copies.len());
    }

    /// Weights that all read zero (a failed readback), sources that cannot take all the moves (a mass fade), weights
    /// that are no number: every plan ends.
    #[test]
    fn relocation_plans_end_whatever_the_weights() {
        let mut rng = fastrand::Rng::with_seed(8);
        let plan = plan_relocation(&[0.0; 1000], 1050, false, &mut rng);
        assert!(plan.copies.is_empty() && plan.adjust.is_empty() && plan.len == 1000);
        // Nine live sources and 9,991 faded ones: a twentieth (500) is more than nine can take (450).
        let mut w = vec![-1.0f32; 10_000];
        for i in 0..9 {
            w[i * 1000] = 0.5;
        }
        let plan = plan_relocation(&w, 10_500, false, &mut rng);
        assert_eq!(plan.copies.len(), 9 * (MAX_RATIO - 1));
        assert_eq!(plan.adjust.len(), 9);
        // One source holding nearly all the weight takes all it can (50 copies); the others take the other 50 moves.
        let mut w = vec![0.001f32; 200];
        w[0] = 1000.0;
        w.extend(vec![-1.0; 1800]);
        let plan = plan_relocation(&w, 2000, false, &mut rng);
        assert_eq!(plan.copies.len(), 100);
        assert_eq!(plan.copies.iter().filter(|c| c[1] == 0).count(), MAX_RATIO - 1);
        // NaN and ∞ count as dead (at least one slot moves a round); the copy comes from the one live source.
        let w = [f32::NAN, 0.5, -1.0, f32::INFINITY];
        let plan = plan_relocation(&w, 4, false, &mut rng);
        assert_eq!(plan.copies.len(), 1);
        assert!(plan.copies.iter().all(|c| c[1] == 1 && c[0] != 1));
    }

    /// Fades run every thousand steps from step 1000, while a relocation round is left to reuse what they free.
    #[test]
    fn the_fade_schedule() {
        let s = Settings::default();
        let until = (9000.0 * s.relocate_until) as u32;
        let due: Vec<u32> = (1..=9000).filter(|&it| fade_due(it, &s, until)).collect();
        assert_eq!(due, vec![1000, 2000, 3000, 4000]);
        assert!((1..=9000).all(|it| !fade_due(it, &Settings { fade_unseen_every: 0, ..s.clone() }, until)));
    }

    /// The result leaves out what shows nowhere: faded Gaussians and ones whose parameters are no numbers.
    #[test]
    fn the_result_keeps_only_what_shows() {
        let mut g = Gaussians::default();
        for o in [0.5, 0.004, 0.006, 0.9] {
            g.push([0.0; 3], [-2.0; 3], [1.0, 0.0, 0.0, 0.0], logit(o), [0.5; 3]);
        }
        g.rotations[3][2] = f32::NAN;
        assert_eq!(live(&g), vec![true, false, true, false]);
    }

    #[test]
    fn held_out_keyframes_and_mid_run_probes() {
        assert_eq!(held_out(20, 8), vec![4, 12]);
        assert!(held_out(20, 1).is_empty() && held_out(20, 0).is_empty());
        let many: Vec<usize> = (0..200).collect();
        let probe = evenly(&many, 16);
        assert_eq!(probe.len(), 16);
        assert!(probe.windows(2).all(|p| p[1] - p[0] >= 12));
        assert_eq!(evenly(&many[..5], 16), vec![0, 1, 2, 3, 4]);
    }
}
