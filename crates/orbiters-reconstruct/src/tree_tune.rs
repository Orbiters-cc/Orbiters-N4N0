//! VRChat's files of a reconstruction (OrbitersContracts Docs/splat-tree.md): its splat tree for PC, a smaller one for
//! Quest and the top of that one, which a world shows while the full file downloads. A tree keeps the splats that add
//! the most to the recorded cameras' images ([`Rank::Contribution`]); its leaves are then re-fitted to the full model's
//! renders from those cameras as the file's bytes hold them (colours within 0–1: a trained model holds about half its
//! splats' colours outside, which the colour bytes clip; [`ExportOptions::decoded`]); and each depth of merged boxes is
//! tuned on the GPU to look like the leaves it stands in for, at the resolution where that depth is drawn. Leaves and
//! merged depths are as the file stores them (crate::tree_file: positions in steps of each box's bounds, half-float
//! scales, the rotation's smallest three components, colour bytes): the target is what a world draws up close, and a
//! depth keeps its tuning only when what the file will hold of it scores better. Everything renders from the session's
//! recorded cameras, never its keyframes, so a session whose keyframes are gone exports all the same. Measured on the
//! lab's five sessions: the SOTA lab's reports/export-leaves.md (2026-11-02).

use crate::camera::Camera;
use crate::gaussians::Gaussians;
use crate::gpu::{Gpu, LearningRates, Target};
use crate::timing::PrepStage;
use crate::tree::{self, Tree};
use orbiters_core::contracts::frame_record::record_flags;
use std::path::Path;
use std::sync::Arc;

/// Splats in the PC file at most (41.9 MB of 20-byte records; version 1's 32 bytes made it 67.1): with Quest's, the
/// coarse one and a sky map, an upload stays under the Cloudflare limit of 100 MB in front of Orbiters (contracts'
/// Docs/splat-tree.md). Smaller records leave the cap as it was: the world's sort and blending cost the same a splat.
pub const PC_SPLATS: usize = 1 << 21;
/// In the Quest file at most (10.5 MB): what a Quest decodes and sorts smoothly.
pub const QUEST_SPLATS: usize = 1 << 19;
/// In the coarse file at most (about 1 MB).
pub const COARSE_SPLATS: usize = 49_152;
/// Training steps per depth of merged boxes.
const STEPS: u32 = 400;
/// Cameras a depth trains from at most (spread over the session).
const CAMERAS: usize = 24;
/// The widest camera a depth trains from.
const MAX_WIDTH: u32 = 960;
/// A merged box was tuned where its error looks about this many pixels wide ([`ExportOptions::before`]).
const DRAWN_AT_PIXELS: f32 = 1.5;
/// Where the PC and Quest trees' merged depths are tuned: their error about this many pixels wide in a recorded camera
/// (about 600 pixels per radian at 960 wide), near where the world draws them (2–4.5 px at 1000 px/rad on PC, 15–33 on
/// Quest). Against 1.5 px, Quest's cut at auto's start (τ = 33) gained 0.15 dB on exo and 0.46 on plaza.
const DRAWN_AT: [f32; 2] = [3.0, 20.0];
/// Re-fit steps a tree's leaves take ([`ExportOptions::refit_steps`]): 2000 gained plaza's Quest tree 0.05–0.07 dB over
/// 1000 for about twice the re-fit's time.
const REFIT_STEPS: u32 = 1000;
/// Recorded cameras the re-fit renders from at most ([`ExportOptions::cameras`]): 48 left a long capture's leaves fitted
/// to those 48 and worse between them (silent-dawn, 419 keyframes: PC leaves +0.12 dB on held-out views, +0.53 from 256).
const REFIT_CAMERAS: usize = 256;

pub struct VrchatFiles {
    pub pc: Vec<u8>,
    pub quest: Vec<u8>,
    pub coarse: Vec<u8>,
    /// Splats in each.
    pub splats: [usize; 3],
}

/// The session's recorded cameras (one a keyframe), at most [`MAX_WIDTH`] wide.
pub fn session_cameras(session: &Path) -> anyhow::Result<Vec<Camera>> {
    session_cameras_without(session, &[])
}

/// The same without the keyframes `skip` (the lab: a run's held-out keyframes stay unseen by the export too).
pub fn session_cameras_without(session: &Path, skip: &[i32]) -> anyhow::Result<Vec<Camera>> {
    let records = orbiters_core::bundle::read_records(session)?;
    Ok(records
        .iter()
        .filter(|r| r.has(record_flags::KEYFRAME) && r.has(record_flags::STRIP_OK) && r.keyframe_index >= 0 && r.width > 0 && r.height > 0)
        .filter(|r| !skip.contains(&r.keyframe_index))
        .map(|r| Camera::from_strip(&r.camera_to_world, &r.projection, r.width, r.height, (MAX_WIDTH as f32 / r.width as f32).min(1.0)))
        .filter(|c| c.fx.is_finite() && c.fx > 0.0)
        .collect())
}

/// How a tree picks the splats it keeps when a reconstruction has more than fit (lab switch `--rank`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rank {
    /// Opacity times area (crate::tree::importances): big opaque splats first, whether or not a view sees them (the
    /// ranking before 2026-11-02). It filled plaza's Quest tree with its sky shell: Quest leaves 23.1 dB on held-out views
    /// against 34.3 for the PC tree's.
    Area,
    /// What each adds to the recorded cameras' images: the sum of its blending weights α·T over their pixels
    /// (crate::gpu::Gpu::contribution, LightGaussian's global significance), rendered from the full model, so a session
    /// whose keyframes are gone ranks all the same. Quest leaves on held-out views: plaza 30.1 dB (+7.1), home +0.39,
    /// exo +0.18.
    #[default]
    Contribution,
    /// The PLY's seen statistics alone (no GPU): the largest weight times the keyframes that saw it times its footprint
    /// in pixels at the finest rate it was seen at. On exo as good as contribution (Quest leaves 23.63 against 23.62 dB):
    /// what a CPU-only export could use.
    Seen,
}

impl Rank {
    pub fn parse(name: &str) -> anyhow::Result<Self> {
        Ok(match name {
            "area" => Self::Area,
            "contribution" => Self::Contribution,
            "seen" => Self::Seen,
            _ => anyhow::bail!("a rank is area, contribution or seen, not {name}"),
        })
    }
}

/// How [`vrchat_files_with`] makes the trees (lab switches of `orbiters-reconstruct --vrchat` and examples/treeloss.rs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExportOptions {
    pub rank: Rank,
    /// Steps re-fitting a tree's kept leaves to the full model's renders from the recorded cameras (distillation: no
    /// keyframe needed), when the tree keeps fewer splats than the model has (or, `decoded`, always); 0: the leaves stay
    /// as trained. One camera in eight checks the re-fit, which is kept only if it gains there too.
    pub refit_steps: u32,
    /// Cameras the re-fit renders from at most, spread over the session (one in eight checks it instead).
    pub cameras: usize,
    /// Cameras the contribution adds up over at most, spread over the session.
    pub rank_cameras: usize,
    /// Decode-aware: the leaves are re-fitted (with `refit_steps`) and the merged depths tuned as the file rounds them,
    /// every colour within 0–1 (the colour bytes), the merged depths against the leaves as rounded. The colour bytes'
    /// clip cost the full model 0.17–0.82 dB on held-out views (exo; home 0.51, silent-dawn 0.82), and the PC tree, which
    /// keeps every splat on every lab session, lost exactly that.
    pub decoded: bool,
    /// The most leaves the budget holds (a few builds), rather than 10 % fewer until the tree fits.
    pub tight: bool,
    /// Training steps per depth of merged boxes.
    pub tune_steps: u32,
    /// The PC and Quest trees' merged depths are tuned where their error looks this many pixels wide.
    pub drawn_at: [f32; 2],
    /// The re-fit's cameras are this wide at most: whole multiples of the recorded cameras (themselves at most 960 wide).
    pub refit_width: u32,
    /// A PC tree that keeps every splat re-fits only their colours and opacities (decode-aware: what the colour bytes
    /// clip), not where they are or their shape. A full re-fit moved plaza's sharp splats to fit the cameras it trained
    /// from and lost 0.4 dB from the others (so it was refused); appearance alone gained 2.7 dB there (+0.20 held out).
    pub pc_appearance: bool,
}

impl ExportOptions {
    /// The export as it was before contribution ranking and the re-fit (lab controls: `--before`).
    pub fn before() -> Self {
        Self {
            rank: Rank::Area,
            refit_steps: 0,
            cameras: 48,
            rank_cameras: 256,
            decoded: false,
            tight: false,
            tune_steps: STEPS,
            drawn_at: [DRAWN_AT_PIXELS; 2],
            refit_width: MAX_WIDTH,
            pc_appearance: false,
        }
    }
}

impl Default for ExportOptions {
    /// What uploads make: contribution ranking, the decode-aware re-fit (the PC tree's of appearance only) and merged
    /// depths tuned where they are drawn (the SOTA lab's reports/export-leaves.md).
    fn default() -> Self {
        Self {
            rank: Rank::Contribution,
            refit_steps: REFIT_STEPS,
            cameras: REFIT_CAMERAS,
            rank_cameras: 256,
            decoded: true,
            tight: false,
            tune_steps: STEPS,
            drawn_at: DRAWN_AT,
            refit_width: MAX_WIDTH,
            pc_appearance: true,
        }
    }
}

/// Each splat's score for [`Rank`] (higher is kept first). Contribution needs the GPU; without one it falls back to area.
pub fn rank(g: &Gaussians, cameras: &[Camera], gpu: Option<&mut Gpu>, rank: Rank) -> anyhow::Result<Vec<f32>> {
    match (rank, gpu) {
        (Rank::Contribution, Some(gpu)) if !cameras.is_empty() => {
            gpu.upload(g);
            for c in cameras {
                gpu.set_mask(c.width, c.height, &vec![1.0; (c.width * c.height) as usize])?;
            }
            let sums = gpu.contribution(cameras, [0.0; 3])?;
            // Splats no camera sees keep their area order, behind every seen one.
            let area = tree::importances(g);
            let top = area.iter().copied().fold(0.0f32, f32::max).max(1e-30);
            Ok(sums.iter().zip(&area).map(|(s, a)| if *s > 0.0 { *s + 1.0 } else { a / top }).collect())
        }
        (Rank::Seen, _) => {
            let area = tree::importances(g);
            Ok((0..g.len())
                .map(|i| {
                    let s = g.seen(i);
                    if s.weight < 0.0 {
                        return area[i];
                    }
                    let sigma2 = (2.0 * g.log_scales[i].iter().copied().fold(f32::NEG_INFINITY, f32::max)).exp();
                    s.weight * s.views.max(1) as f32 * (sigma2 * s.rate * s.rate).max(1.0)
                })
                .collect())
        }
        _ => Ok(tree::importances(g)),
    }
}

/// The largest tree of the splats of `g` that `score` ranks highest within `max` splats, and those leaves, from
/// `start` leaves down: 10 % fewer a try, or (`tight`) as many fewer as the last try was over, and up again while it
/// fits.
fn fitting(g: &Gaussians, max: usize, score: &[f32], start: usize, tight: bool) -> (Vec<usize>, Tree) {
    let available = tree::eligible(g);
    let mut leaves = start.min(available);
    let mut best: Option<(Vec<usize>, Tree)> = None;
    for _ in 0..if tight { 6 } else { 64 } {
        let keep = tree::leaves_by(g, leaves, score);
        let t = tree::build(g, &keep);
        let (n, len) = (keep.len(), t.len());
        let fits = len <= max;
        if fits && best.as_ref().is_none_or(|(k, _)| k.len() < n) {
            best = Some((keep, t));
        }
        if !tight {
            if fits || leaves < 1000 {
                break;
            }
            leaves = leaves * 9 / 10;
            continue;
        }
        // The merged boxes are about a third of the leaves: scale by what the budget leaves, just under it.
        let next = (((n as f64 * max as f64 / len as f64) * 0.999) as usize).min(available);
        if (fits && (n == available || next <= n + n / 1000)) || next < 1000 {
            break;
        }
        leaves = if fits { next } else { next.min(n - 1) };
    }
    match best {
        Some(best) => best,
        None if tight => fitting(g, max, score, leaves * 9 / 10, false),
        None => {
            let keep = tree::leaves_by(g, leaves, score);
            let t = tree::build(g, &keep);
            (keep, t)
        }
    }
}

/// The splats `keep` of `g`, as a set of their own.
fn subset(g: &Gaussians, keep: &[usize]) -> Gaussians {
    let mut out = Gaussians::default();
    for &i in keep {
        out.push(g.means[i], g.log_scales[i], g.rotations[i], g.opacity_logits[i], g.colors[i]);
    }
    out
}

/// `g` as the file rounds it but for positions (crate::tree_file::round_attributes: half-float log scales, the
/// rotation's smallest three components, colour and opacity bytes), for the lab's scoring of a model as a file holds it
/// (examples/treeloss.rs).
pub fn quantize(g: &mut Gaussians) {
    crate::tree_file::round_attributes(g);
}

/// The full model's renders from `cameras` (black behind, as the leaves train): the re-fit's targets.
fn full_renders(g: &Gaussians, cameras: &[Camera], gpu: &mut Gpu) -> anyhow::Result<Vec<Arc<[u32]>>> {
    gpu.upload(g);
    cameras.iter().map(|c| Ok(packed(&crate::train::render(gpu, c, [0.0; 3])?).into())).collect()
}

/// The learning rates of tuning and re-fitting: training's, the means' a tenth of where training starts them.
fn learning_rates(cameras: &[Camera]) -> LearningRates {
    let centre = cameras.iter().fold([0.0f32; 3], |a, c| [0, 1, 2].map(|k| a[k] + c.position()[k] / cameras.len() as f32));
    let extent = cameras.iter().map(|c| (0..3).map(|k| (c.position()[k] - centre[k]).powi(2)).sum::<f32>().sqrt()).fold(1.0f32, f32::max);
    LearningRates { means: 1.6e-5 * extent, log_scales: 0.005, rotations: 0.001, opacity: 0.025, colors: 0.0025 }
}

/// Recorded cameras and the full model's renders from them (crate::tree_tune::full_renders).
type Views = [(Camera, Arc<[u32]>)];

/// Re-fits a tree's leaves (the splats `keep` of `g`) to the full model's renders from the cameras of `train`, then
/// builds the tree again from them within `max` (the re-fitted set's lowest-ranked splats left out if it no longer
/// fits). With `in_gamut`, every colour stays within 0–1 from the start (what the file's colour bytes hold: the leaves are
/// scored as the file would show them, and the other parameters make up for the colours a trained model holds outside).
/// None when the re-fit did not bring the leaves closer to the full model from the cameras of `check` (recorded cameras
/// it did not train on: on the plaza walk, 48 cameras made the leaves closer from those 48 and farther from the others,
/// −0.44 dB on held-out views): the tree stays as it was. Both sides are scored as the file stores them
/// (crate::tree_file::round_attributes: colour and opacity bytes, half-float log scales, the rotation's smallest three;
/// positions move only within their box's 16-bit steps): the leaves as trained against the leaves re-fitted.
#[allow(clippy::too_many_arguments)]
fn refit(
    g: &Gaussians,
    keep: &[usize],
    score: &[f32],
    max: usize,
    train: &Views,
    check: &Views,
    gpu: &mut Gpu,
    steps: u32,
    in_gamut: bool,
    appearance_only: bool,
    platform: &str,
) -> anyhow::Result<Option<Tree>> {
    let cameras: Vec<Camera> = train.iter().map(|v| v.0).collect();
    let cameras = &cameras[..];
    let stored = |leaves: &Gaussians| {
        let mut out = leaves.clone();
        crate::tree_file::round_attributes(&mut out);
        out
    };
    let mut leaves = subset(g, keep);
    // What the file would hold without the re-fit: the leaves as trained, as it stores them.
    let unfitted = stored(&leaves);
    if in_gamut {
        for c in &mut leaves.colors {
            *c = c.map(|v| v.clamp(0.0, 1.0));
        }
    }
    let base = learning_rates(cameras);
    // Appearance only: the splats stay where they are, as they are shaped.
    let base = if appearance_only { LearningRates { means: 0.0, log_scales: 0.0, rotations: 0.0, ..base } } else { base };
    for (c, _) in train.iter().chain(check) {
        gpu.set_mask(c.width, c.height, &vec![1.0; (c.width * c.height) as usize])?;
    }
    let mean_score = |gpu: &mut Gpu, views: &Views| -> anyhow::Result<f32> {
        Ok(views.iter().map(|(c, t)| gpu.score(c, [0.0; 3], t)).collect::<anyhow::Result<Vec<f32>>>()?.iter().sum::<f32>() / views.len() as f32)
    };
    let check = if check.is_empty() { train } else { check };
    gpu.upload(&unfitted);
    let (before, before_train) = (mean_score(gpu, check)?, mean_score(gpu, train)?);
    drop(unfitted);
    gpu.upload(&leaves);
    gpu.step = 0;
    let mut rng = fastrand::Rng::with_seed(2);
    gpu.clamp_colors = in_gamut;
    for s in 0..steps {
        let j = rng.usize(..cameras.len());
        let c = &cameras[j];
        // The means' rate falls tenfold over the re-fit, as training's does towards its end.
        let lr = LearningRates { means: base.means * 0.1f32.powf(s as f32 / steps as f32), ..base };
        let target = Target::Color { rgba: &train[j].1, scene_pixels: (c.width * c.height) as f32, weight: 1.0, ssim: 0.2, robust: 0.0 };
        if let Err(e) = gpu.step(c, [0.0; 3], target, Some(lr)) {
            gpu.clamp_colors = false;
            return Err(e);
        }
    }
    gpu.clamp_colors = false;
    let after_float = mean_score(gpu, check)?;
    let trained = gpu.download_params()?;
    for i in 0..leaves.len() {
        if trained.is_finite(i) {
            leaves.means[i] = trained.means[i];
            leaves.log_scales[i] = trained.log_scales[i];
            leaves.rotations[i] = trained.rotations[i];
            leaves.opacity_logits[i] = trained.opacity_logits[i];
            leaves.colors[i] = trained.colors[i];
        }
    }
    drop(trained);
    gpu.upload(&stored(&leaves));
    let (after, after_train) = (mean_score(gpu, check)?, mean_score(gpu, train)?);
    eprintln!(
        "{platform} leaves re-fit: {} of {} splats, {steps} steps from {} cameras, PSNR against the full model as the file stores them {before_train:.3} → {after_train:.3} dB there, {before:.3} → {after:.3} dB from {} others ({after_float:.3} before the file's rounding)",
        keep.len(),
        g.len(),
        train.len(),
        check.len()
    );
    if after <= before {
        return Ok(None);
    }
    // The re-fitted set fitted the budget before; moved, it may need a few splats fewer.
    let sub: Vec<f32> = keep.iter().map(|&i| score[i]).collect();
    let (_, tree) = fitting(&leaves, max, &sub, leaves.len(), true);
    Ok(Some(tree))
}

/// The three files of `g` (a reconstruction), named `key` (the upload they belong to). Without a GPU, the merged
/// boxes stay as merged.
pub fn vrchat_files(g: &Gaussians, cameras: &[Camera], gpu: Option<&mut Gpu>, key: u32, progress: impl FnMut(f32)) -> anyhow::Result<VrchatFiles> {
    vrchat_files_with(g, cameras, gpu, key, &ExportOptions::default(), progress)
}

/// [`vrchat_files`] made as `options` say.
pub fn vrchat_files_with(
    g: &Gaussians,
    cameras: &[Camera],
    mut gpu: Option<&mut Gpu>,
    key: u32,
    options: &ExportOptions,
    mut progress: impl FnMut(f32),
) -> anyhow::Result<VrchatFiles> {
    let poses: Vec<([f32; 3], [f32; 3])> = cameras.iter().map(|c| (c.position(), c.forward())).collect();
    let spread: Vec<Camera> = crate::dataset::spread(&poses, options.cameras).into_iter().map(|i| cameras[i]).collect();
    // The re-fit's cameras at up to `refit_width` pixels wide (whole multiples of the recorded cameras' size).
    let refit_cameras: Vec<Camera> = spread.iter().map(|c| c.upscaled((options.refit_width / c.width.max(1)).max(1))).collect();
    let score = {
        let _time = PrepStage::new("tree rank");
        let ranking: Vec<Camera> = crate::dataset::spread(&poses, options.rank_cameras).into_iter().map(|i| cameras[i]).collect();
        rank(g, &ranking, gpu.as_deref_mut(), options.rank)?
    };
    let (pc_keep, mut pc) = {
        let _time = PrepStage::new("tree build pc");
        fitting(g, PC_SPLATS, &score, PC_SPLATS * 3 / 4, options.tight)
    };
    anyhow::ensure!(pc.leaf_splats().len() >= 1000, "This reconstruction has too few splats to show the world.");
    let same = pc.len() <= QUEST_SPLATS;
    let mut quest = if same {
        None
    } else {
        let _time = PrepStage::new("tree build quest");
        Some(fitting(g, QUEST_SPLATS, &score, QUEST_SPLATS * 3 / 4, options.tight))
    };
    eprintln!(
        "trees: PC {} leaves ({} splats), Quest {} leaves ({} splats), of {} splats ({} visible)",
        pc_keep.len(),
        pc.len(),
        quest.as_ref().map_or(pc_keep.len(), |q| q.0.len()),
        quest.as_ref().map_or(pc.len(), |q| q.1.len()),
        g.len(),
        tree::eligible(g)
    );
    progress(0.1);
    if let Some(gpu) = gpu {
        if options.refit_steps > 0 && !spread.is_empty() {
            let available = tree::eligible(g);
            // Decode-aware, every tree's leaves are re-fitted (the colour bytes clip what a full tree keeps too).
            let cut = |keep: &[usize]| keep.len() < available || options.decoded;
            let cut_pc = pc_keep.len() < available;
            if cut(&pc_keep) || quest.as_ref().is_some_and(|q| cut(&q.0)) {
                let _time = PrepStage::new("refit targets");
                let rendered = full_renders(g, &refit_cameras, gpu)?;
                // One camera in eight checks the re-fit instead of training it.
                let (mut train, mut check) = (Vec::new(), Vec::new());
                for (k, view) in refit_cameras.iter().copied().zip(rendered).enumerate() {
                    if refit_cameras.len() >= 16 && k % 8 == 4 {
                        check.push(view)
                    } else {
                        train.push(view)
                    }
                }
                drop(_time);
                if cut(&pc_keep) {
                    let _time = PrepStage::new("refit pc");
                    if let Some(t) = refit(
                        g,
                        &pc_keep,
                        &score,
                        PC_SPLATS,
                        &train,
                        &check,
                        gpu,
                        options.refit_steps,
                        options.decoded,
                        options.pc_appearance && !cut_pc,
                        "pc",
                    )? {
                        pc = t;
                    }
                }
                if let Some((keep, q)) = quest.as_mut().filter(|q| cut(&q.0)) {
                    let _time = PrepStage::new("refit quest");
                    if let Some(t) = refit(g, keep, &score, QUEST_SPLATS, &train, &check, gpu, options.refit_steps, options.decoded, false, "quest")? {
                        *q = t;
                    }
                }
            }
        }
        // Decode-aware tuning keeps the merged depths' colours within 0–1 too; what the file stores of leaves and merged
        // depths is what tune_for scores either way (crate::tree_file, Tree::select_stored).
        let share = if quest.is_none() { 0.85 } else { 0.55 };
        let _time = PrepStage::new("tune pc");
        gpu.clamp_colors = options.decoded;
        let tuned = tune_for(&mut pc, cameras, gpu, "pc", options.tune_steps, options.drawn_at[0], |p| progress(0.1 + share * p));
        gpu.clamp_colors = false;
        tuned?;
        drop(_time);
        if let Some((_, q)) = quest.as_mut() {
            let _time = PrepStage::new("tune quest");
            gpu.clamp_colors = options.decoded;
            let tuned = tune_for(q, cameras, gpu, "quest", options.tune_steps, options.drawn_at[1], |p| progress(0.65 + 0.3 * p));
            gpu.clamp_colors = false;
            tuned?;
        }
    }
    let _time = PrepStage::new("tree serialize");
    let quest = quest.map(|(_, q)| q);
    let q = quest.as_ref().unwrap_or(&pc);
    let coarse = q.top(COARSE_SPLATS);
    progress(1.0);
    let pc_bytes = pc.file(key, 0);
    let quest_bytes = match &quest {
        Some(q) => q.file(key, 0),
        None => pc_bytes.clone(),
    };
    Ok(VrchatFiles { splats: [pc.len(), q.len(), coarse.len()], pc: pc_bytes, quest: quest_bytes, coarse: coarse.file(key, tree::FLAG_COARSE) })
}

/// A render as a keyframe's packed pixels (RGBA8, red in the low byte).
fn packed(render: &[[f32; 4]]) -> Vec<u32> {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    render.iter().map(|p| byte(p[0]) | byte(p[1]) << 8 | byte(p[2]) << 16 | 255 << 24).collect()
}

/// A leaf render depends on the exact scaled camera, not the depth being tuned. Keep at most 64 MiB between
/// depths; current targets retain their Arc when an older cache entry is evicted. Never share between trees:
/// PC and Quest can have different leaves.
#[derive(Default)]
struct LeafTargets {
    entries: std::collections::VecDeque<(Camera, Arc<[u32]>)>,
    bytes: usize,
}

impl LeafTargets {
    const MAX_BYTES: usize = 64 << 20;

    fn get(&self, camera: &Camera) -> Option<Arc<[u32]>> {
        self.entries.iter().find(|(c, _)| c == camera).map(|(_, pixels)| pixels.clone())
    }

    fn insert(&mut self, camera: Camera, pixels: Arc<[u32]>) {
        let bytes = std::mem::size_of_val(pixels.as_ref());
        if bytes > Self::MAX_BYTES {
            return;
        }
        while self.bytes + bytes > Self::MAX_BYTES {
            let (_, old) = self.entries.pop_front().expect("cache has bytes");
            self.bytes -= std::mem::size_of_val(old.as_ref());
        }
        self.bytes += bytes;
        self.entries.push_back((camera, pixels));
    }
}

fn depth_views(tree: &Tree, cameras: &[Camera], tuned: &[usize], drawn_at: f32) -> Vec<Camera> {
    cameras
        .iter()
        .filter_map(|c| {
            let mut seen: Vec<f32> = tuned
                .iter()
                .filter_map(|&i| {
                    let n = &tree.nodes[i];
                    let p = c.to_camera(n.center);
                    (p[2] > 0.1).then(|| n.error * c.fx / p[2])
                })
                .collect();
            if seen.is_empty() {
                return None;
            }
            let k = seen.len() / 2;
            let median = *seen.select_nth_unstable_by(k, f32::total_cmp).1;
            let factor = ((median / drawn_at).ceil() as u32).clamp(1, (c.width / 48).max(1));
            Some(c.scaled(factor))
        })
        .collect()
}

/// Tunes every depth of `tree`'s merged boxes against its leaves.
pub fn tune(tree: &mut Tree, cameras: &[Camera], gpu: &mut Gpu, mut progress: impl FnMut(f32)) -> anyhow::Result<()> {
    tune_for(tree, cameras, gpu, "tree", STEPS, DRAWN_AT_PIXELS, &mut progress)
}

#[allow(clippy::too_many_arguments)]
fn tune_for(
    tree: &mut Tree,
    cameras: &[Camera],
    gpu: &mut Gpu,
    platform: &str,
    steps: u32,
    drawn_at: f32,
    mut progress: impl FnMut(f32),
) -> anyhow::Result<()> {
    if cameras.is_empty() {
        return Ok(());
    }
    let poses: Vec<([f32; 3], [f32; 3])> = cameras.iter().map(|c| (c.position(), c.forward())).collect();
    let cameras: Vec<Camera> = crate::dataset::spread(&poses, CAMERAS).into_iter().map(|i| cameras[i]).collect();
    let lr = learning_rates(&cameras);
    // The leaves as the file stores them: what a world draws where a merged depth stands in for them.
    let leaves = tree.select_stored(|n| n.leaf);
    let mut depths: Vec<u32> = tree.nodes.iter().filter(|n| !n.leaf).map(|n| n.depth).collect();
    depths.sort_unstable();
    depths.dedup();
    let mut rng = fastrand::Rng::with_seed(1);
    let mut leaf_targets = LeafTargets::default();
    for (done, &depth) in depths.iter().enumerate() {
        let _depth_time = PrepStage::new(format!("tune {platform} depth {depth}"));
        // Prepared in the background (a process that follows VRChat: crate::priority::govern), the steps wait for
        // VRChat's frames as training's do.
        match gpu.follow_vrchat() {
            Some(true) => eprintln!("tree depth {depth}: tuning between VRChat's frames (SteamVR's frame timing)"),
            Some(false) => eprintln!("tree depth {depth}: VRChat closed: tuning at full speed"),
            None => {}
        }
        let gpu_start = gpu.profiler.as_ref().map(|p| p.span.get().0);
        let tuned: Vec<usize> = (0..tree.nodes.len()).filter(|&i| !tree.nodes[i].leaf && tree.nodes[i].depth == depth).collect();
        // Each camera at the resolution where this depth's boxes are drawn: their median error about `drawn_at` pixels.
        let views = depth_views(tree, &cameras, &tuned, drawn_at);
        if views.is_empty() {
            continue;
        }
        let _target_time = PrepStage::new(format!("{platform} depth {depth} targets"));
        if views.iter().any(|v| leaf_targets.get(v).is_none()) {
            gpu.upload(&leaves);
        }
        let targets: Vec<Arc<[u32]>> = views
            .iter()
            .map(|v| {
                if let Some(pixels) = leaf_targets.get(v) {
                    return Ok(pixels);
                }
                let pixels: Arc<[u32]> = packed(&crate::train::render(gpu, v, [0.0; 3])?).into();
                leaf_targets.insert(*v, pixels.clone());
                Ok(pixels)
            })
            .collect::<anyhow::Result<_>>()?;
        drop(_target_time);
        // As the file stores it: the tuning starts from what a world would draw, and `before` scores that.
        let cut = tree.select_stored(|n| n.depth == depth || (n.leaf && n.depth < depth));
        // Where each tuned box's splats sit in the cut (it keeps the tree's box order).
        let mut at = Vec::with_capacity(tuned.len());
        let mut offset = 0usize;
        for n in tree.nodes.iter().filter(|n| n.depth == depth || (n.leaf && n.depth < depth)) {
            if !n.leaf && n.depth == depth {
                at.push((offset, n.start as usize, n.count as usize));
            }
            offset += n.count as usize;
        }
        gpu.upload(&cut);
        gpu.step = 0;
        for v in &views {
            gpu.set_mask(v.width, v.height, &vec![1.0; (v.width * v.height) as usize])?;
        }
        let mean_score = |gpu: &mut Gpu| -> anyhow::Result<f32> {
            Ok(views.iter().zip(&targets).map(|(v, t)| gpu.score(v, [0.0; 3], t)).collect::<anyhow::Result<Vec<f32>>>()?.iter().sum::<f32>()
                / views.len() as f32)
        };
        let before = mean_score(gpu)?;
        let _step_time = PrepStage::new(format!("{platform} depth {depth} steps"));
        for _ in 0..steps {
            let j = rng.usize(..views.len());
            let v = &views[j];
            let target = Target::Color { rgba: &targets[j], scene_pixels: (v.width * v.height) as f32, weight: 1.0, ssim: 0.2, robust: 0.0 };
            gpu.step(v, [0.0; 3], target, Some(lr))?;
        }
        // What the file will hold of the tuned boxes: their trained splats (a splat whose training went non-finite keeps
        // its own), rounded box by box as it stores them (the rest of the cut stays as it started).
        let trained = gpu.download_params()?;
        let mut tuned_cut = cut.clone();
        for &(from, to, count) in &at {
            for k in 0..count {
                let (source, i) = if trained.is_finite(from + k) { (&trained, from + k) } else { (&tree.splats, to + k) };
                tuned_cut.means[from + k] = source.means[i];
                tuned_cut.log_scales[from + k] = source.log_scales[i];
                tuned_cut.rotations[from + k] = source.rotations[i];
                tuned_cut.opacity_logits[from + k] = source.opacity_logits[i];
                tuned_cut.colors[from + k] = source.colors[i];
            }
            crate::tree_file::round_box(&mut tuned_cut, from..from + count);
        }
        gpu.upload(&tuned_cut);
        let after = mean_score(gpu)?;
        drop(_step_time);
        eprintln!("tree depth {depth}: {} boxes, PSNR against the leaves {before:.6} → {after:.6} dB", tuned.len());
        if std::env::var_os("ORBITERS_PREP_TIMING").is_some() {
            eprintln!(
                "upload_quality {}",
                serde_json::json!({"platform":platform,"depth":depth,"boxes":tuned.len(),"before_db":before,"after_db":after,"retained_db":before.max(after),"views":views.len(),"gpu_ms":gpu_start.zip(gpu.profiler.as_ref()).map(|(start,p)|p.span.get().0-start)})
            );
        }
        // Boxes that already looked like their leaves can only lose to the noise of a few hundred steps.
        if after <= before {
            progress((done + 1) as f32 / depths.len() as f32);
            continue;
        }
        for (from, to, count) in at {
            for k in 0..count {
                if trained.is_finite(from + k) {
                    tree.splats.means[to + k] = trained.means[from + k];
                    tree.splats.log_scales[to + k] = trained.log_scales[from + k];
                    tree.splats.rotations[to + k] = trained.rotations[from + k];
                    tree.splats.opacity_logits[to + k] = trained.opacity_logits[from + k];
                    tree.splats.colors[to + k] = trained.colors[from + k];
                }
            }
        }
        progress((done + 1) as f32 / depths.len() as f32);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cloud(n: usize) -> Gaussians {
        let mut g = Gaussians::default();
        let mut rng = fastrand::Rng::with_seed(3);
        for _ in 0..n {
            let q = [rng.f32() - 0.5, rng.f32() - 0.5, rng.f32() - 0.5, rng.f32() - 0.5];
            g.push(
                [rng.f32() * 30.0, rng.f32() * 3.0, rng.f32() * 30.0],
                [-4.0 + rng.f32() * 2.0; 3],
                q,
                rng.f32() * 6.0 - 3.0,
                [rng.f32() * 1.2 - 0.1, 0.5, rng.f32()],
            );
        }
        g
    }

    #[test]
    fn a_tight_fit_keeps_more_leaves_within_the_budget() {
        let g = cloud(60_000);
        let score = tree::importances(&g);
        for max in [9_000, 20_000] {
            let (loose_keep, loose) = fitting(&g, max, &score, max * 3 / 4, false);
            let (tight_keep, tight) = fitting(&g, max, &score, max * 3 / 4, true);
            assert!(loose.len() <= max && tight.len() <= max, "{} and {} splats for {max}", loose.len(), tight.len());
            assert!(tight_keep.len() >= loose_keep.len() && tight.len() >= max * 98 / 100, "{} leaves ({} splats) for {max}", tight_keep.len(), tight.len());
            assert_eq!(tight.leaf_splats().len(), tight_keep.len());
        }
        // Everything fits: everything is kept, tight or not.
        let (keep, _) = fitting(&g, 1 << 20, &score, 1 << 19, true);
        assert_eq!(keep.len(), tree::eligible(&g));
    }

    /// A wall of small splats in front of the cameras (about seven deep: a few thousandths of the light gets through), a
    /// large one hidden behind it, and a few cameras.
    fn hidden_scene() -> (Gaussians, Vec<Camera>) {
        let mut g = Gaussians::default();
        let mut rng = fastrand::Rng::with_seed(5);
        for _ in 0..8000 {
            let p = [rng.f32() * 4.0 - 2.0, rng.f32() * 3.0 - 1.5, 4.0 + rng.f32() * 0.05];
            let q = [1.0, rng.f32() * 0.2, rng.f32() * 0.2, rng.f32() * 0.2];
            g.push(p, [(0.06f32).ln(), (0.06f32).ln(), (0.005f32).ln()], q, 3.0, [rng.f32(), rng.f32(), rng.f32()]);
        }
        // Big and opaque, but behind the wall from every camera.
        g.push([0.0, 0.0, 6.0], [(0.4f32).ln(); 3], [1.0, 0.0, 0.0, 0.0], 4.0, [1.0, 0.0, 0.0]);
        let cameras = (0..6).map(|k| Camera::look([k as f32 * 0.2 - 0.5, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 0.9, 160, 120)).collect();
        (g, cameras)
    }

    #[test]
    fn contribution_keeps_what_the_cameras_see_over_what_is_big() {
        let Ok(mut gpu) = Gpu::new() else { return };
        let (g, cameras) = hidden_scene();
        let hidden = g.len() - 1;
        let area = rank(&g, &cameras, Some(&mut gpu), Rank::Area).unwrap();
        let seen = rank(&g, &cameras, Some(&mut gpu), Rank::Contribution).unwrap();
        // By area the hidden splat is the most important of all; by what the cameras see, it is not worth half the
        // budget (the wall's back layers, behind its front ones, are worth little too).
        assert!(area[..hidden].iter().all(|a| *a < area[hidden]));
        let below = seen[..hidden].iter().filter(|s| **s < seen[hidden]).count();
        assert!(below < hidden / 2, "{below} of {hidden} visible splats rank below the hidden one");
        assert!(tree::leaves_by(&g, hidden / 2, &area).contains(&hidden));
        assert!(!tree::leaves_by(&g, hidden / 2, &seen).contains(&hidden));
    }

    #[test]
    fn a_refit_brings_the_kept_leaves_closer_to_the_full_model() {
        let Ok(mut gpu) = Gpu::new() else { return };
        let (g, cameras) = hidden_scene();
        let score = tree::importances(&g);
        let keep = tree::leaves_by(&g, g.len() / 3, &score);
        let targets = full_renders(&g, &cameras, &mut gpu).unwrap();
        gpu.set_mask(160, 120, &[1.0; 160 * 120]).unwrap();
        let psnr = |t: &Gaussians, gpu: &mut Gpu| {
            gpu.upload(t);
            cameras.iter().zip(&targets).map(|(c, p)| gpu.score(c, [0.0; 3], p).unwrap()).sum::<f32>() / cameras.len() as f32
        };
        let before = psnr(&subset(&g, &keep), &mut gpu);
        // Trained from five cameras, checked from the sixth.
        let views: Vec<(Camera, Arc<[u32]>)> = cameras.iter().copied().zip(targets.iter().cloned()).collect();
        let tree = refit(&g, &keep, &score, usize::MAX, &views[..5], &views[5..], &mut gpu, 300, true, false, "test").unwrap().expect("a re-fit that gains");
        assert!(tree.splats.colors.iter().flatten().all(|c| (0.0..=1.0).contains(c)), "colours within the bytes' range");
        let after = psnr(&tree.leaf_splats(), &mut gpu);
        assert!(after > before + 1.0, "{before} → {after} dB");
        assert!(tree.leaf_splats().len() <= keep.len());
    }

    #[test]
    fn leaf_targets_only_reuse_the_exact_camera_and_resolution() {
        let camera = Camera { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3], fx: 2.0, fy: 2.0, cx: 1.0, cy: 1.0, width: 2, height: 2 };
        let pixels: Arc<[u32]> = vec![1, 2, 3, 4].into();
        let mut targets = LeafTargets::default();
        targets.insert(camera, pixels.clone());
        assert!(Arc::ptr_eq(&targets.get(&camera).unwrap(), &pixels));
        assert!(targets.get(&Camera { width: 1, ..camera }).is_none());
        assert!(targets.get(&Camera { t: [1.0, 0.0, 0.0], ..camera }).is_none());
    }

    /// Compare the actual serialized upload files, including colour/rotation quantization, on identical views.
    /// ORBITERS_TEST_SESSION=<semicolon-separated sessions>, ORBITERS_PREP_BASE/FINAL=<saved numbered files>.
    /// Optional ORBITERS_PREP_BASE_REPEAT averages an independently recorded baseline's scores, since float-atomic
    /// training varies between identical runs. The gate stays 0.05 dB; each baseline score is printed too.
    #[test]
    #[ignore]
    fn serialized_upload_quality() {
        fn read(path: &Path) -> Tree {
            Tree::parse(&std::fs::read(path).unwrap()).unwrap()
        }
        let base = std::path::PathBuf::from(std::env::var_os("ORBITERS_PREP_BASE").unwrap());
        let repeat = std::env::var_os("ORBITERS_PREP_BASE_REPEAT").map(std::path::PathBuf::from);
        let final_dir = std::path::PathBuf::from(std::env::var_os("ORBITERS_PREP_FINAL").unwrap());
        let mut gpu = Gpu::new().unwrap();
        let mut failed = Vec::new();
        for (index, session) in std::env::var("ORBITERS_TEST_SESSION").unwrap().split(';').enumerate() {
            let cameras = session_cameras(Path::new(session)).unwrap();
            let poses: Vec<_> = cameras.iter().map(|c| (c.position(), c.forward())).collect();
            let cameras: Vec<_> = crate::dataset::spread(&poses, CAMERAS).into_iter().map(|i| cameras[i]).collect();
            for platform in ["pc", "quest", "coarse"] {
                let relative = format!("{index}/{platform}.tree");
                let before = read(&base.join(&relative));
                let repeated = repeat.as_ref().map(|dir| read(&dir.join(&relative)));
                let after = read(&final_dir.join(&relative));
                assert_eq!(before.nodes, after.nodes);
                if let Some(repeated) = &repeated {
                    assert_eq!(before.nodes, repeated.nodes);
                    if platform != "coarse" {
                        assert!(before.leaf_splats() == repeated.leaf_splats(), "baseline leaf splats changed");
                    }
                }
                let reference = if platform == "coarse" { read(&base.join(format!("{index}/quest.tree"))) } else { before.clone() };
                let leaves = reference.leaf_splats();
                if platform != "coarse" {
                    assert!(leaves == after.leaf_splats(), "exported leaf splats changed");
                }
                let deepest = before.nodes.iter().map(|n| n.depth).max().unwrap();
                for depth in 0..=deepest {
                    let tuned: Vec<_> =
                        before.nodes.iter().enumerate().filter_map(|(i, n)| (n.depth == depth && (!n.leaf || platform == "coarse")).then_some(i)).collect();
                    if tuned.is_empty() {
                        continue;
                    }
                    let views = depth_views(&reference, &cameras, &tuned, DRAWN_AT_PIXELS);
                    if views.is_empty() {
                        continue;
                    }
                    let trees = std::iter::once(&before).chain(repeated.iter()).chain(std::iter::once(&after));
                    let cuts: Vec<_> = trees.map(|tree| tree.select(|n| n.depth == depth || (n.leaf && n.depth < depth))).collect();
                    let mut scores = vec![0.0f32; cuts.len()];
                    for v in &views {
                        gpu.upload(&leaves);
                        let target = packed(&crate::train::render(&mut gpu, v, [0.0; 3]).unwrap());
                        gpu.set_mask(v.width, v.height, &vec![1.0; (v.width * v.height) as usize]).unwrap();
                        for (i, cut) in cuts.iter().enumerate() {
                            // Reset estimates for each evaluated view, so none drops tile entries from an older camera.
                            gpu.upload(cut);
                            scores[i] += gpu.score(v, [0.0; 3], &target).unwrap() / views.len() as f32;
                        }
                    }
                    let final_score = *scores.last().unwrap();
                    let baselines = &scores[..scores.len() - 1];
                    let baseline = baselines.iter().sum::<f32>() / baselines.len() as f32;
                    let delta = final_score - baseline;
                    eprintln!(
                        "file_quality {}",
                        serde_json::json!({"session":index,"platform":platform,"depth":depth,"baseline_db":baseline,"baseline_runs_db":baselines,"final_db":final_score,"delta_db":delta})
                    );
                    if delta < -0.05 {
                        failed.push((index, platform, depth, delta));
                    }
                }
            }
        }
        assert!(failed.is_empty(), "serialized render quality regressions: {failed:?}");
    }
}
