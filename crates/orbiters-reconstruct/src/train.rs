//! Training in replay mode: Adam on the keyframes' colour ((1 − λ)·L1 + λ·D-SSIM over the scene pixels) and the
//! exact depth samples (relative L1), with MCMC-style relocation (Kheradmand et al. 2024): every so often, Gaussians
//! that faded out move onto live ones picked by opacity, the copies sharing the original's opacity and footprint, while
//! the set grows to its budget, and while it binds the nearly transparent Gaussians off the measured surface take MCMC's
//! SGLD noise (lab switches place births by error, split them or add births at depth). Depth steps render only the depth samples' rays; colour steps train on every other
//! pixel each way for the first half of the run (and can run coarse to fine). Every `hold_out_every`-th keyframe is never
//! trained on and scores the result.

use crate::aberration::Undo;
use crate::camera::Camera;
use crate::checkpoint::{Interrupted, Lab, Pause, StepState, TrainedState};
use crate::dataset::{channel, Dataset, DepthView};
use crate::gaussians::{logit, sigmoid, Gaussians, PACKED as PACKED_FLOATS};
#[cfg(test)]
use crate::gaussians::{rotation_matrix, PACKED};
use crate::gpu::{Gpu, LearningRates, Relocation, Target};
use anyhow::ensure;
use orbiters_i18n::tr;
use std::sync::Arc;
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
    /// Every colour stays within 0–1 at every step (crate::gpu::Gpu::clamp_colors), what the splat files' colour bytes
    /// hold (the web `.splat`, VRChat's trees): a model trained so is what its files show. Off until measured (lab:
    /// the trees' colour bytes clip what about 45 % of a trained model's splats hold, crate::tree_tune).
    pub clamp_colors: bool,
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
    /// Fading stops for the rest of the run once a round finds more Gaussians unseen than the round before, by over a
    /// twentieth of the set, while the set did not grow between them (`false`: it never stops). Unseen counts that hold
    /// or fall (VRChat Home: 356k, 357k, 348k, 335k, 320k of 834k) are redundant cover relocation reuses, and while the
    /// set grows, so do its unseen copies (a 3D picture: 157k of 680k, then 278k of 868k, 320k of 1.0M); counts that
    /// keep rising in a set of one size (Silent Dawn, a 78 s recording: 244k, 368k, 483k, … 680k of 888k) are
    /// relocation's own copies faded again round after round, and the set never grows: on that recording fading to the
    /// end cost 1.0 dB on held-out keyframes (the best 90 % of their pixels) and half the result's splats; stopping kept
    /// the other captures' scores.
    pub fade_until_rising: bool,
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
    /// Of the colour steps, the share that trains on the detail views when the session has any
    /// (crate::dataset::detail_views: a 3D picture's close-ups, VRChat's camera zoomed in on the picture's framing where
    /// it stood, before it flew); the other colour steps pick among the other keyframes and the rig's faces as before,
    /// and the run takes as many more steps as keep their count ([`detail_iterations`]). Measured on the 3D pictures
    /// Enzo took with friends on 2026-11-03, the close-ups emulated (native-resolution crops of the start frame, the
    /// session trained at half resolution so that they carry twice its pixels a degree), scored on the people in the
    /// start view at full resolution (three repeats, which agree within 0.2 dB): the 8 s picture 22.3 → 36.2 dB (SSIM
    /// 0.87 → 0.99), the 4 s 22.1 → 27.7 dB (0.87 → 0.96; as plain keyframes, without a share, 26.7; at 40 %, 28.6 but
    /// 1.2 dB less on the best 90 % of held-out flight views). Held-out flight views: the 8 s picture 28.4 → 28.0 dB
    /// (about one standard deviation of its runs); the 4 s 25.3 → 23.3, its views of the people at other moments no
    /// longer matching the picture's moment (the static parts score as before). On the static plaza walk: the start view +4.2 dB, the
    /// held-out view 0.4 m on +0.8, the other held-out views −0.4. Training takes a third longer (the 8 s picture
    /// 24.6 → 33.1 s at half resolution). 0: the detail views train as any keyframe (never held out all the same).
    pub detail_share: f32,
    /// Depth seeded too few Gaussians, and seeds along the keyframes' rays made up for it (crate::init::add_from_rays;
    /// the worker sets it): relocation then grows the set while it is under a quarter of the budget, even while many
    /// Gaussians are dead. Otherwise the set grows only while few are, and a set that starts at a thousand never does.
    pub starved: bool,
    /// Which live Gaussians relocation copies ([`Sources`]): where the moved and new Gaussians are born. Opacity, as
    /// 3DGS-MCMC samples them: no score did better at the 1M budget (three interleaved runs, without the SGLD noise;
    /// reports/births.md of the lab). By rank alone, faint sources churn (see `sources_by_opacity`); by rank times opacity, Exoplanet Journey and
    /// the plaza scored within their noise and VRChat Home 0.2–0.3 dB higher with 10–19 % more splats and up to 18 %
    /// more time. At 300k, gradient ranks times opacity gained 0.16 dB on Exoplanet (with a fifth fewer splats) and
    /// 0.12 dB on the plaza, lost 0.16 dB on Home, and added nothing to the SGLD noise (`noise`).
    pub sources: Sources,
    /// A score-ranked source's chance is also its opacity, as 3DGS-MCMC's is: ranks alone copy faint Gaussians, whose
    /// copies split an opacity they cannot spare (eq. 9) and die by the next round (Exoplanet Journey: 150k of 566k
    /// dead every round instead of 30k, the set never grew and 303k splats were left instead of 646k).
    pub sources_by_opacity: bool,
    /// How a source and its copy share the source's footprint ([`Split`]). The footprint: ImprovedGS's long-axis split
    /// cost 0.3 dB on Exoplanet Journey and 0.28 dB off the plaza's path (one run each), on measured depth only it was
    /// within the runs' noise on all four captures.
    pub split: Split,
    /// 3DGS-MCMC's SGLD noise on the means while relocation runs (shaders/project_backward_adam.wgsl `sgld`): its λ,
    /// gsplat's `noise_lr` [`NOISE`], times the position learning rate; 0: none. The term σ(100·((1 − o) − 0.995))
    /// keeps it to nearly transparent Gaussians (half at opacity 0.005, a hundredth at 0.05): the faint ones relocation
    /// would otherwise find where they last fitted a transient or one view's error, still blending a little into the
    /// views that do not show it. On by default, spared on measured depth ([`Settings::noise_gate`]) and only while
    /// the budget binds ([`Settings::noise_dead_max`]): against no noise, three interleaved runs each at the 1M budget
    /// (lab reports/births.md), held-out PSNR +0.90 dB on VRChat Home (27.10 against 26.20), +0.41 dB on Exoplanet
    /// Journey (24.22 against 23.81), +0.06 dB on the synthetic plaza and +0.13 dB off its path (21.99 against 21.86),
    /// Silent Dawn unchanged (26.53 against 26.55, two runs); the best 90 % of pixels within the runs' noise
    /// (+0.24, +0.12, +0.04, −0.11); 6–20 % fewer splats and 2–17 % less time on all four. Gated by depth alone it
    /// gained as much or more on the short captures (Exoplanet +0.62, Home +0.64, the plaza +0.01 and +0.14 off the
    /// path; at a 300k budget +0.28, +0.52 and +0.45) but lost 0.78 dB on Silent Dawn (0.91 on the best 90 %). The
    /// confidence-aware densification paper (arXiv 2610.13541) found the noise worse than none for its own births.
    pub noise: f32,
    /// The noise spares the Gaussians anchored on the measured surface (within [`ANCHORED`] of it where depth steps
    /// see them since the last relocation round): a seed on exact depth has nothing to explore. Ungated, VRChat Home's
    /// best 90 % of pixels lost 0.25 dB (43.22 against 43.47, three runs, about 2 standard deviations) for 0.23 dB more
    /// on the whole image.
    pub noise_gate: bool,
    /// Render the training steps with the forward whose loop is nested (shaders/raster_forward_nested.wgsl,
    /// crate::gpu::Gpu::nested_forward): the same entries, order and outputs bit for bit, the loop structure of the
    /// gpu-stacks experiment's CUDA C++ port, 7–10 % faster than the flattened forward on dumped steps. Default since
    /// 2026-11-03: whole training runs on the RTX 4080 (Vulkan), the same binary with it off and on, 3 pairs a session,
    /// took 3.0 % less time on plaza, 4.4 % on exo, 2.2 % on Home and 2.8 % on Silent Dawn (one pair), 8 of 9 pairs on the
    /// large sessions faster and none slower, quality within noise (the SOTA lab's reports/gpu-stacks.md). Changes
    /// nothing on D3D12 (the flattened forward there: FXC compiles the nested one slowly). `--flat-forward` turns it off.
    pub nested_forward: bool,
    /// The noise moves only the Gaussians the step's view shows (Brush weights its noise by visibility): dense Adam
    /// steps every Gaussian every step, and on a long walk a faint Gaussian no view of the moment constrains wanders
    /// for thousands of steps. Off: on Silent Dawn, without the budget rule, it lost 0.44 dB held-out and 0.60 dB on
    /// the best 90 % (two runs) instead of 0.52 and 0.72; the budget rule is what fixes Silent Dawn.
    pub noise_visible: bool,
    /// The noise runs only in rounds whose relocation found at most this share of the set dead (faded, broken or huge;
    /// 0: every round): while the budget binds. [`NOISE_DEAD_MAX`], a quarter: Silent Dawn's control keeps about 30 %
    /// of its 2.6M dead round after round once fading stops (relocation's own copies dying again: more budget than
    /// the capture uses), and there the noise rescued faint Gaussians that should die, its set grew to the whole budget
    /// at the end of relocation and quality fell; with the rule the noise ran there only for the first thousand steps
    /// and four single late rounds. Exoplanet passes it from step 1600 on, VRChat Home and the plaza a few rounds after
    /// each fade (about 40 % of Home's rounds), and on Home it scored 0.26 dB higher than the noise in every round; on
    /// the 3D picture `needle` (313 views, two runs) +0.35 dB held-out and on the best 90 %. But the rule protects
    /// Silent Dawn through its dead share, not because the noise only hurts a budget it does not fill: at a 1M budget,
    /// which Silent Dawn fills (2–4 % dead), the noise lost 0.65 dB there and with the rule 0.71 (two runs each; the
    /// best 90 % 0.87 and 0.85). Whatever makes that world lose (faint content the noise scatters, as fog would be, is
    /// the next thing to look at) is not measured yet: a capture like it that fills its budget would lose the same.
    pub noise_dead_max: f32,
    /// Births at measured depth (crate::births, ConeGS without its proxy): this share of each relocation round's moves
    /// become new Gaussians at the depth samples of a few depth frames where the render errs most against the
    /// keyframes, a pixel's size across and the keyframes' colour, instead of copies (0: none). Off: a fifth of the
    /// moves cost 0.36 dB on Exoplanet Journey and 0.58 dB on the plaza's best 90 % at 1M (one run each) and 0.12 dB on
    /// Exoplanet at 300k (three runs), but gained 0.87 dB held-out, 0.67 dB on the best 90 % and 0.22 dB off the path
    /// on the synthetic plaza at 300k (three runs), and 0.68 dB on Home's best 90 % at 300k (−0.11 on the whole
    /// image), where ConeGS gains most: worth a switch for small budgets.
    pub depth_births: f32,
    /// Faces seen in the keyframes (crate::portraits).
    pub portraits: Vec<crate::portraits::Portrait>,
    /// The steps on tiles over them, on top of the others and all in the run's second half (crate::portraits); 0: none,
    /// and the detector does not run.
    pub portrait_steps: u32,
    /// How quickly a later view of a face counts for less among them, seconds (crate::portraits::Picker); 0: all alike.
    pub portrait_moment: f32,
    /// What the wearer filled, when they left this session (crate::wearer): what lies in it when training ends goes, and the
    /// floor they stood on is patched.
    pub wearer: Option<std::sync::Arc<crate::wearer::Volume>>,
}

/// Which live Gaussians a relocation round copies onto the dead and new slots (crate::train::plan_relocation), from
/// the statistics the colour steps since the last round recorded (shaders/param_grads.wgsl). Rank-weighted like the
/// confidence-aware densification of Hofer, Kerbl, Steinberger et al. (arXiv 2610.13541): a source's chance is its
/// rank among the live ones (0 to 1), so no outlier takes every copy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sources {
    /// By opacity, as 3DGS-MCMC samples them.
    #[default]
    Opacity,
    /// By the mean norm of the screen-space positional gradient over the colour steps that showed it (3DGS's
    /// densification statistic).
    Gradient,
    /// By the mean colour error it blended (Σ error × α·T over a step's scene pixels).
    Error,
    /// Both: rank(gradient) × (0.2 + rank(error)), the paper's gradient × low-confidence product with blended error
    /// standing for its learned confidence.
    Confidence,
}

/// How a relocation copy is placed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Split {
    /// Anywhere inside the source's footprint (a standard normal offset in its frame), both shrunk by MCMC's eq. 9.
    #[default]
    Footprint,
    /// A source with one copy splits in two along its longest axis (ImprovedGS, Deng et al. arXiv 2508.12313): the
    /// halves at ±0.45 of that axis's scale, it shrunk to 0.55 and the others to √(1 − 0.45²). A disc on a surface
    /// splits within its plane. Sources with more copies place them as [`Split::Footprint`] does.
    LongAxis,
    /// [`Split::LongAxis`] only for sources anchored on the measured surface ([`ANCHORED`]), where depth confirms the
    /// surface the halves land on; the others as [`Split::Footprint`].
    LongAxisOnDepth,
}

/// The detail views' share of the colour steps ([`Settings::detail_share`]).
pub const DETAIL_SHARE: f32 = 0.25;

/// SGLD noise's λ for `--noise` ([`Settings::noise`]): gsplat's `noise_lr` for 3DGS-MCMC, measured here.
pub const NOISE: f32 = 5e5;

/// The share of the set a relocation round may find dead for the noise to run ([`Settings::noise_dead_max`]).
pub const NOISE_DEAD_MAX: f32 = 0.25;

/// A Gaussian is anchored on the measured surface when, α·T-weighted over the depth samples it blends into, its centre
/// lies within this share of the measured depth (shaders/project_backward_adam.wgsl's ANCHORED).
pub const ANCHORED: f32 = 0.02;

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
            clamp_colors: false,
            adam_restart: false,
            max_radius: 0.1,
            background: [0.0; 3],
            seed: 1,
            ssim: 0.2,
            face_share: 0.5,
            detail_share: DETAIL_SHARE,
            robust: 0.0,
            robust_from: 0.25,
            quarter_until: 0.0,
            half_until: 0.0,
            stride_until: 0.5,
            stride: 2,
            relocate_until: 0.5,
            fade_unseen_every: 1000,
            unseen_weight: 0.02,
            fade_until_rising: true,
            sky_fill: true,
            filter_3d: 0.0,
            cleanup: false,
            sky_map: 2048,
            starved: false,
            sources: Sources::Opacity,
            sources_by_opacity: false,
            split: Split::Footprint,
            noise: NOISE,
            noise_gate: true,
            nested_forward: true,
            noise_visible: false,
            noise_dead_max: NOISE_DEAD_MAX,
            depth_births: 0.0,
            portraits: Vec::new(),
            portrait_steps: crate::portraits::STEPS,
            portrait_moment: crate::portraits::MOMENT,
            wearer: None,
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
    /// Gaussians that left with the wearer's volume, and floor patched where they stood (crate::wearer).
    pub wearer_left: usize,
    pub floor_patched: usize,
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

/// The keyframes a run on `data` holds out: every `every`-th of those that are not detail views
/// (crate::dataset::detail_views), which a run always trains on: they are the only views of a 3D picture's subject at
/// their resolution and moment, and held out they would leave it as mushy as before. A session without detail views
/// holds out what [`held_out`] says. A view with no scene pixel at all has nothing to score: a camera that flew through a wearer
/// who leaves the picture has all of its pixels out, and held out it ended the whole reconstruction ("no scene pixel to score",
/// the second real 3D picture of 2026-11-06).
pub fn held_out_views(data: &Dataset, every: usize) -> Vec<usize> {
    let detail = data.detail_views();
    let scored = |i: usize| crate::wearer::scene(&data.mask, &data.views[i].image).iter().any(|m| *m > 0.0);
    let others: Vec<usize> = (0..data.views.len()).filter(|i| detail.binary_search(i).is_err() && scored(*i)).collect();
    held_out(others.len(), every).into_iter().map(|j| others[j]).collect()
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

/// The most Gaussians a session trains with, from its training views (keyframes and rig faces): a short capture keeps
/// the million it always had, and longer ones get more, up to [`MOST_GAUSSIANS`]. Measured on held-out keyframes (the
/// best 90 % of their pixels; the desktop repository's docs/capture-detail-2026-10-29.md): a 419-view recording gained
/// 0.67 dB at two million and 1.02 dB at three, a 313-view 3D picture 0.61 dB at two and nothing more at three,
/// VRChat Home's 137 views nothing (−0.1 to −0.2 dB, within its noise).
pub fn budget_for(views: usize) -> usize {
    (views * GAUSSIANS_PER_VIEW).clamp(FEWEST_GAUSSIANS, MOST_GAUSSIANS)
}

/// The masks colour steps name on the device beside each image size's own (crate::gpu::Gpu::set_step_mask): the one of
/// a tile over a face, then one for each phase of the sub-grid.
const TILE_MASK: u32 = 0;
const PHASE_MASKS: u32 = 1;

pub const GAUSSIANS_PER_VIEW: usize = 7_000;
pub const FEWEST_GAUSSIANS: usize = 1_000_000;
pub const MOST_GAUSSIANS: usize = 3_000_000;

/// 100 steps per keyframe: VRChat Home trains as well at 75 to 150, and with relocation until half the run the Canine
/// orbit scores higher at 100 than it did at 150. The rig's faces add none: on the plaza walk with rig frames, the
/// keyframes' steps scored 26.8 dB off the path where 2.4 times as many (faces counted half a keyframe) scored 25.9.
pub fn default_iterations(data: &Dataset) -> u32 {
    (data.views.len() as u32 * 100).clamp(3_000, 30_000)
}

/// The steps of a session with detail views (crate::dataset::detail_views) and `views` others: as many as
/// [`default_iterations`] gives the others, divided by what the detail views' `share` of the colour steps leaves them,
/// so every other view and face gets as many steps as without the detail views, and the detail views' steps come on
/// top. Taken from the others instead (the share without more steps), the 3D picture of 2026-11-03 lost 2.5 dB on the
/// best 90 % of a held-out flight view's pixels at a quarter, and more at half.
pub fn detail_iterations(views: usize, share: f32) -> u32 {
    let base = (views as u32 * 100).clamp(3_000, 30_000) as f32;
    (base / (1.0 - share.clamp(0.0, 0.9))).round() as u32
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
        // Over the scene pixels the wearer does not show in (crate::gpu::Target::Color).
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
        gpu.stop_if_paused()?;
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
                            psnr(&seen, &captured, &crate::wearer::scene(&u.mask, &data.views[*i].image))
                        }
                        None => psnr(img, &data.views[*i].image, &crate::wearer::scene(&data.mask, &data.views[*i].image)),
                    };
                    ensure!(psnr.is_finite(), "{}", tr!("keyframe {index} has no scene pixel to score", index = data.views[*i].keyframe));
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

/// What a run starts from: the seeds, or where a paused run stood (crate::checkpoint).
pub enum Start {
    Seeds(Gaussians),
    /// In the middle of its steps: everything the steps carry from one to the next.
    Steps(Box<StepState>),
    /// Its steps done: only what follows them is left ([`finish`]).
    Trained(Box<TrainedState>),
}

/// How a run ended.
pub enum Ran {
    Finished(Outcome),
    /// Asked to pause (crate::checkpoint::Pause): what its next start takes. The device can go now: a run paused holds
    /// nothing on it that this does not hold.
    Paused(Start),
}

/// What asks a run to pause, and the lab's asks at chosen points of it (crate::checkpoint::Lab).
#[derive(Clone, Copy, Default)]
pub struct Control<'a> {
    pub pause: Option<&'a Arc<Pause>>,
    pub lab: Option<&'a Lab>,
}

impl Control<'_> {
    /// Whether the run should pause before step `done + 1`, having started this time after step `from`.
    fn pause_before(&self, done: u32, from: u32, iterations: u32) -> bool {
        self.pause.is_some_and(|p| p.asked()) || self.lab.is_some_and(|l| l.pause_at_step(done, from, iterations))
    }
}

/// [`train_from`], from seeds and to its end.
pub fn train(data: &Dataset, gpu: &mut Gpu, init: Gaussians, settings: &Settings, report: impl FnMut(&Progress)) -> anyhow::Result<Outcome> {
    match train_from(data, gpu, Start::Seeds(init), settings, Control::default(), report)? {
        Ran::Finished(outcome) => Ok(outcome),
        Ran::Paused(_) => anyhow::bail!("{}", tr("a run nothing can pause paused")),
    }
}

/// The run from `from`. Asked to pause, it stops between two steps (any relocation round on its way lands first, as it
/// would have a few steps later: its plan is 20–30 ms of the CPU), reads back what the steps carry
/// (crate::gpu::Gpu::training_state) and returns it. Resumed, the steps go on with the same random stream, the same
/// learning-rate position and the same optimiser state, and end as an uninterrupted run does: no two runs of a session
/// are the same bit for bit (two without a pause or a relocation round differ too, the gradients' atomic sums taking
/// their floats in whatever order the GPU runs them), so what counts is the scores (the lab's reports/pause-resume.md).
pub fn train_from(data: &Dataset, gpu: &mut Gpu, from: Start, settings: &Settings, control: Control<'_>, mut report: impl FnMut(&Progress)) -> anyhow::Result<Ran> {
    let start = Instant::now();
    let held_out = held_out_views(data, settings.hold_out_every);
    // A 3D picture's zoomed-in views of its subject (never held out): their share of the colour steps, on top of the
    // steps the other views get without them (detail_iterations); the other colour steps pick among the others only.
    let detail = if settings.detail_share > 0.0 && settings.detail_share < 1.0 { data.detail_views() } else { Vec::new() };
    let train_views: Vec<usize> = (0..data.views.len()).filter(|i| held_out.binary_search(i).is_err()).collect();
    let others: Vec<usize> = train_views.iter().copied().filter(|i| detail.binary_search(i).is_err()).collect();
    ensure!(!others.is_empty(), "{}", tr("every keyframe is held out: nothing to train on"));
    let iterations = match settings.iterations {
        0 if detail.is_empty() => default_iterations(data),
        0 => detail_iterations(data.views.len() - detail.len(), settings.detail_share),
        n => n,
    };
    // A session's faces (crate::portraits): tile steps on top of the others, all in the second half. Without any the run
    // is the same step for step: no extra draw from the random stream, the same progress.
    let tile = crate::portraits::TILE;
    let fits = data.width >= tile && data.height >= tile;
    let portraits: Vec<crate::portraits::Portrait> = if settings.portrait_steps > 0 && fits { settings.portraits.iter().filter(|p| held_out.binary_search(&p.view).is_err()).copied().collect() } else { Vec::new() };
    let picker = (!portraits.is_empty()).then(|| crate::portraits::Picker::new(&portraits, settings.portrait_moment));
    let base_iterations = iterations;
    let extra = if picker.is_some() { crate::portraits::steps_for(portraits.len(), settings.portrait_steps) } else { 0 };
    let iterations = base_iterations + extra;
    let first_half = base_iterations / 2;
    let tile_chance = crate::portraits::tile_chance(base_iterations, extra);
    let tile_px = (tile * tile) as usize;
    let (mut tile_target, mut tile_mask): (Vec<u32>, Vec<f32>) = if picker.is_some() { (vec![0; tile_px], vec![0.0; tile_px]) } else { (Vec::new(), Vec::new()) };
    if !portraits.is_empty() {
        eprintln!("{} views of faces: {extra} steps on tiles over them, on top of {base_iterations}", portraits.len());
    }
    if !detail.is_empty() {
        eprintln!("{} detail views (keyframes {:?}): {:.0} % of the colour steps", detail.len(), detail.iter().map(|&i| data.views[i].keyframe).collect::<Vec<_>>(), settings.detail_share * 100.0);
    }
    let (seeds, steps, trained) = match from {
        Start::Seeds(g) => (Some(g), None, None),
        Start::Steps(s) => (None, Some(s), None),
        Start::Trained(t) => (None, None, Some(t)),
    };
    ensure!(seeds.as_ref().is_none_or(|g| !g.is_empty()), "{}", tr("no Gaussians to start from"));
    if let Some(s) = &steps {
        ensure!(s.iterations == iterations, "{}", tr!("a paused run of {paused} steps resumed as one of {resumed}", paused = s.iterations, resumed = iterations));
    }
    // The training time before this start (what the report and the time left go by).
    let earlier = steps.as_ref().map_or(0.0, |s| s.seconds) + trained.as_ref().map_or(0.0, |t| t.seconds);
    // A session's frames whose every pixel is strip or band (or a mask never set) would train on nothing and score
    // nothing.
    let scene_pixels = data.mask.iter().filter(|m| **m > 0.5).count() as f32;
    ensure!(scene_pixels > 0.0, "{}", tr!("the capture strip and the skeleton band cover all of these {width}×{height} keyframes: nothing to train on", width = data.width, height = data.height));
    let depth_views: Vec<usize> = (0..data.depth_views.len()).filter(|&i| data.depth_views[i].valid() > 0).collect();
    // The rig's faces, when they have scene pixels and sizes of their own (their mask is kept per size).
    let faces = data.faces.as_ref().filter(|f| !f.views.is_empty() && f.sizes.iter().all(|s| s.scene_pixels > 0.0 && (s.width, s.height) != (data.width, data.height)));
    let face_cams: Vec<Camera> = faces.map_or_else(Vec::new, |f| f.views.iter().map(|v| v.camera).collect());
    let face_steps = face_share(face_cams.len(), data.views.len(), settings.face_share);
    let extent = extent(data);
    eprintln!("extent: {extent:.2} m");
    let mut rng = fastrand::Rng::with_seed(steps.as_ref().map_or(settings.seed, |s| s.rng));
    // Mid-run scores on at most 16 held-out keyframes: enough to follow, cheap at any session length.
    let probe = evenly(&held_out, 16);

    // Feed the GPU even while another program keeps every core busy (crate::priority).
    let feeding = crate::priority::Feeding::start();
    let dev = gpu.device.clone();
    let setup_start = Instant::now();
    // The set grows to the budget: buffers of that size from the start never grow (and copy) on the way.
    let held = seeds.as_ref().map_or(0, Gaussians::len).max(steps.as_ref().map_or(0, |s| s.buffers.params.len() / PACKED_FLOATS));
    gpu.reserve(settings.budget.max(held));
    let first_set = match (&seeds, &steps, &trained) {
        (Some(g), ..) => {
            gpu.upload(g);
            g.len()
        }
        (_, Some(s), _) => {
            gpu.restore_training(&s.buffers);
            gpu.restore_sky_learning(s.sky.as_ref())?;
            s.buffers.params.len() / PACKED_FLOATS
        }
        (_, _, Some(t)) => {
            // What the device held when the run paused (the worker gave it back its sky map).
            gpu.upload(&t.gaussians);
            t.gaussians.len()
        }
        _ => unreachable!("a run starts from one of them"),
    };
    drop(seeds);
    // The 3D filter follows the keyframes trained on, at their full resolution (the finest sampling).
    let filter_cams: Vec<Camera> = train_views.iter().map(|&i| data.views[i].camera).chain(face_cams.iter().copied()).collect();
    gpu.update_filter_3d(&filter_cams, settings.filter_3d)?;
    let coarse = if settings.half_until > 0.0 { levels(data) } else { Vec::new() };
    for l in &coarse {
        gpu.set_mask(l.width, l.height, &l.mask)?;
    }
    gpu.set_mask(data.width, data.height, &data.mask)?;
    for s in faces.iter().flat_map(|f| &f.sizes) {
        gpu.set_mask(s.width, s.height, &s.mask)?;
    }
    gpu.regularise_unseen = settings.regularise_unseen;
    gpu.clamp_colors = settings.clamp_colors;
    gpu.adam_restart = settings.adam_restart;
    // The statistics relocation's sources and splits read, and the noise.
    let scored = settings.sources != Sources::Opacity || settings.split == Split::LongAxisOnDepth;
    gpu.birth_stats = if settings.sources != Sources::Opacity { crate::gpu::BIRTH_STATS_COLOR } else { 0 }
        | if settings.split == Split::LongAxisOnDepth || (settings.noise > 0.0 && settings.noise_gate) { crate::gpu::BIRTH_STATS_DEPTH } else { 0 };
    gpu.noise = settings.noise;
    gpu.noise_visible = settings.noise_visible;
    gpu.noise_gate = settings.noise_gate;
    gpu.nested_forward = settings.nested_forward;
    let spawner = (settings.depth_births > 0.0).then(|| crate::births::Spawner::new(data, &held_out));
    // Each phase of the colour steps' sub-grid (Settings::stride): its size and scene pixels, its mask on the device
    // under a slot of its own (the four of a stride of two are as large as one keyframe's: 8 MB at 1080p). The device
    // kept one mask a size, so each change of phase, three steps in four, uploaded its 2 MB again.
    let stride = settings.stride.max(1);
    let phases: Vec<(u32, u32, f32)> = if settings.stride_until > 0.0 && stride > 1 {
        (0..stride * stride)
            .map(|ph| {
                let (ox, oy) = (ph % stride, ph / stride);
                let (w, h) = ((data.width - ox).div_ceil(stride), (data.height - oy).div_ceil(stride));
                let mask: Vec<f32> = (0..h).flat_map(|j| (0..w).map(move |i| data.mask[((stride * j + oy) * data.width + stride * i + ox) as usize])).collect();
                let scene = mask.iter().filter(|m| **m > 0.5).count() as f32;
                gpu.set_step_mask(PHASE_MASKS + ph, w, h, &mask)?;
                Ok((w, h, scene))
            })
            .collect::<anyhow::Result<_>>()?
    } else {
        Vec::new()
    };
    let mut sub_grid: Vec<u32> = Vec::new();
    // The keyframes on the device when its memory has room for them beside the set (with a GB to spare): steps then
    // upload no pixels. Not on a Mac, whose GPU reads the RAM they are in already: the second copy saved nothing there
    // (500 steps at 1080p on an M3: 74.3 s with it, 74.0 and 75.9 s without) and took their whole size from everything else.
    const LEFT: u64 = 1 << 30;
    let keyframe_bytes: u64 = data.views.iter().map(|v| v.image.len() as u64 * 4).sum();
    let paused_already = control.pause.is_some_and(|p| p.asked());
    // A run resumed past its steps (Start::Trained) never takes a step: no keyframe is read from the device (C4).
    let room = |free: u64| free >= gpu.training_memory(settings.budget) + keyframe_bytes + LEFT;
    let resident = keyframes_resident(cfg!(target_vendor = "apple"), paused_already, trained.is_some(), || gpu.free_memory().is_some_and(room));
    if resident {
        let images: Vec<&[u32]> = data.views.iter().map(|v| v.image.as_slice()).collect();
        gpu.set_keyframes(&images);
    }
    regularise_for(gpu, settings, first_set, extent);
    gpu.step = 0;
    gpu.pause = control.pause.cloned();
    // A run resumed past its steps follows VRChat before what follows them (RECONSTRUCT-4): it never reaches the periodic
    // look below. A run resumed between steps does NOT (night2, the verifier): pacing changes when its steps run, and
    // which step a relocation plan lands at depends on that timing (Rounds::advance looks whether the planner finished),
    // so it would change the picture. It looks at the next multiple of 500 as before. Finishing takes no relocation
    // round (they are all joined before it), so pacing it changes only its time.
    if paced_before_loop(trained.is_some()) {
        if gpu.follow_vrchat() == Some(true) {
            eprintln!("resumed beside VRChat: training paced from the start");
        }
    }
    crate::timing::add("4a train setup", setup_start.elapsed());
    // Relocation ends where it would without the tile steps: they come after it (crate::portraits).
    let relocate_until = (base_iterations as f32 * settings.relocate_until) as u32;
    // Relocation rounds off the steps' path: a round's weights come back while training goes on, a helper thread plans
    // it (its own random stream), and the plan lands a few steps later (VRChat Home: −4 %, quality equal): the plan took
    // 20–30 ms of the CPU a round, with the GPU waiting.
    let mut rounds = Rounds::default();
    let mut last_psnr = steps.as_ref().and_then(|s| s.psnr).or(trained.as_ref().and_then(|t| t.psnr));
    // What the last fading round found (unseen, of how many), and whether rounds still fade (Settings::fade_until_rising).
    let mut last_round: Option<(usize, usize)> = None;
    let mut fading = true;
    // A fading round a pause cut short runs again right after the next step.
    let mut fade_again = false;
    let mut from_step = 0;
    if let Some(s) = &steps {
        from_step = s.iteration;
        gpu.step = s.gpu_step;
        gpu.noise = s.noise;
        gpu.opacity_reg = s.opacity_reg;
        gpu.scale_reg = s.scale_reg;
        last_round = s.last_round;
        fading = s.fading;
        fade_again = s.fade_again;
    }
    drop(steps);
    // A run resumed past its steps goes straight to what follows them.
    let first_step = if trained.is_some() { iterations + 1 } else { from_step + 1 };
    for it in first_step..=iterations {
        if control.pause_before(it - 1, from_step, iterations) {
            if let Some(p) = control.pause {
                p.ask();
            }
            // What is on its way lands first: nothing of a round is kept half done.
            rounds.settle(gpu, &dev, settings, it - 1, relocate_until, scored, extent, &filter_cams)?;
            eprintln!("it {}: pausing", it - 1);
            let state = StepState {
                iteration: it - 1,
                iterations,
                gaussians: gpu.count,
                gpu_step: gpu.step,
                rng: rng.get_seed(),
                noise: gpu.noise,
                opacity_reg: gpu.opacity_reg,
                scale_reg: gpu.scale_reg,
                last_round,
                fading,
                fade_again,
                seconds: earlier + start.elapsed().as_secs_f32(),
                psnr: last_psnr,
                buffers: gpu.training_state()?,
                sky: gpu.sky_learning()?,
            };
            return Ok(Ran::Paused(Start::Steps(Box::new(state))));
        }
        feeding.follow();
        if it % 500 == 1 {
            // While VRChat runs on this GPU, SteamVR's frame timing paces the steps (crate::governor).
            match gpu.follow_vrchat() {
                Some(true) if gpu.governor.as_ref().is_some_and(|g| g.on_desktop()) => eprintln!("it {it}: VRChat on the desktop: training on at most a third of the GPU"),
                Some(true) => eprintln!("it {it}: training between VRChat's frames (SteamVR's frame timing)"),
                Some(false) => eprintln!("it {it}: VRChat closed: training at full speed"),
                None => {}
            }
        }
        let progress = crate::portraits::progress(it, base_iterations, extra);
        if it > relocate_until {
            // Exploration only while relocation can still reuse what it moves (gsplat stops it with refinement too).
            gpu.noise = 0.0;
        }
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
        // Drawn only when there are detail views: a session without keeps its random stream, step for step.
        let detail_step = !depth_step && !detail.is_empty() && rng.f32() < settings.detail_share;
        let portrait_step = !depth_step && picker.is_some() && it > first_half && rng.f32() < tile_chance;
        let intersections = if let Some(p) = picker.as_ref().filter(|_| portrait_step) {
            let portrait = &portraits[p.pick(&mut rng)];
            let v = &data.views[portrait.view];
            let (ox, oy) = crate::portraits::tile_over(portrait, data.width, data.height, &mut rng);
            let t = tile as usize;
            for j in 0..t {
                let row = (oy as usize + j) * data.width as usize + ox as usize;
                tile_target[j * t..(j + 1) * t].copy_from_slice(&v.image[row..row + t]);
                tile_mask[j * t..(j + 1) * t].copy_from_slice(&data.mask[row..row + t]);
            }
            // The tile's own mask, under its own slot: no other mask of a tile's size is written over.
            gpu.set_step_mask(TILE_MASK, tile, tile, &tile_mask)?;
            gpu.step_mask = Some(TILE_MASK);
            let robust = if progress >= settings.robust_from { settings.robust } else { 0.0 };
            let sky_lr = std::mem::replace(&mut gpu.sky_lr, 0.0);
            // A tile's pixel counts as a whole frame's does: over its own few pixels the loss's gradient would be eight
            // times a frame's, and Adam would take the frames' steps for noise.
            let cam = crate::portraits::tile_camera(&v.camera, ox, oy);
            let stepped = gpu.step(&cam, settings.background, Target::Color { rgba: &tile_target, scene_pixels, weight: 1.0, ssim: settings.ssim, robust }, Some(lr));
            gpu.sky_lr = sky_lr;
            stepped?
        } else if depth_step {
            // Built for the step (all of a long session's targets at once would take GBs).
            let t = depth_target(&data.depth_views[depth_views[rng.usize(..depth_views.len())]]);
            let full = gpu.filter_2d;
            gpu.filter_2d = t.filter_2d;
            let stepped = gpu.step(&t.camera, settings.background, Target::Depth { samples: &t.samples, weight: settings.depth_weight }, Some(lr));
            gpu.filter_2d = full;
            stepped?
        } else if let Some(f) = faces.filter(|_| !detail_step && rng.f32() < face_steps) {
            // A face, at its own resolution, the sky map left as it is.
            let v = &f.views[rng.usize(..f.views.len())];
            let robust = if progress >= settings.robust_from { settings.robust } else { 0.0 };
            let sky_lr = std::mem::replace(&mut gpu.sky_lr, 0.0);
            let scene_pixels = f.size_of(&v.camera).map_or(0.0, |s| s.scene_pixels);
            let stepped = gpu.step(&v.camera, settings.background, Target::Color { rgba: &v.image, scene_pixels, weight: 1.0, ssim: settings.ssim, robust }, Some(lr));
            gpu.sky_lr = sky_lr;
            stepped?
        } else {
            let v = if detail_step { detail[rng.usize(..detail.len())] } else { others[rng.usize(..others.len())] };
            let (camera, rgba, pixels) = match level {
                0 => (&data.views[v].camera, &data.views[v].image, scene_pixels),
                l => (&coarse[l - 1].cameras[v], &coarse[l - 1].images[v], coarse[l - 1].scene_pixels),
            };
            let robust = if progress >= settings.robust_from { settings.robust } else { 0.0 };
            if level == 0 && progress < settings.stride_until && !phases.is_empty() {
                let ph = rng.u32(..stride * stride);
                let (ox, oy) = (ph % stride, ph / stride);
                let (w, h, scene) = &phases[ph as usize];
                let c = &data.views[v].camera;
                let f = stride as f32;
                let cam = Camera { r: c.r, t: c.t, fx: c.fx / f, fy: c.fy / f, cx: (c.cx - ox as f32) / f, cy: (c.cy - oy as f32) / f, width: *w, height: *h };
                let img = &data.views[v].image;
                sub_grid.clear();
                for j in 0..*h {
                    let row = ((stride * j + oy) * data.width) as usize;
                    sub_grid.extend((0..*w).map(|i| img[row + (stride * i + ox) as usize]));
                }
                gpu.step_mask = Some(PHASE_MASKS + ph);
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
        if fading && (std::mem::take(&mut fade_again) || fade_due(it, settings, relocate_until)) {
            let _sync = crate::timing::on().then(|| dev.poll(wgpu::PollType::wait_indefinitely()));
            let fade_start = Instant::now();
            let _cpu = crate::priority::CpuWork::start();
            // The faces' views count: what only they see is not unseen.
            let cams: Vec<Camera> = train_views.iter().map(|&i| data.views[i].camera).chain(face_cams.iter().copied()).collect();
            let seen = match gpu.visibility(&cams, settings.background, settings.unseen_weight) {
                Err(e) if e.is::<Interrupted>() => {
                    // The next start does this round again: nothing of it was applied.
                    fade_again = true;
                    continue;
                }
                seen => seen?,
            };
            if seen.iter().all(|w| *w <= 0.0) {
                // Nothing seen at all is a broken round (a mask of zeros, a lost readback), not a model nothing sees.
                eprintln!("it {it}: no training pixel sees any Gaussian; nothing faded");
            } else {
                let faded = logit(1e-4).to_bits();
                let adjust: Vec<[u32; 4]> = seen.iter().enumerate().filter(|(_, w)| **w < settings.unseen_weight).map(|(i, _)| [i as u32, faded, 0f32.to_bits(), 0]).collect();
                let rising = last_round.is_some_and(|(unseen, of)| adjust.len() > unseen + of / 20 && seen.len() <= of);
                if settings.fade_until_rising && rising {
                    // Relocation's copies, unseen again: fading them once more would only keep the set from growing.
                    eprintln!("it {it}: {} of {} Gaussians unseen, more than the last round in a set that did not grow: fading stops", adjust.len(), seen.len());
                    fading = false;
                } else {
                    eprintln!("it {it}: fading {} of {} Gaussians no training pixel sees", adjust.len(), seen.len());
                    last_round = Some((adjust.len(), seen.len()));
                    gpu.apply_relocation(&Relocation { adjust, len: seen.len(), ..Default::default() })?;
                }
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
            last_psnr = psnr.or(last_psnr);
            report(&Progress { iteration: it, iterations, gaussians: gpu.count, intersections, psnr, seconds: earlier + start.elapsed().as_secs_f32() });
        }
        rounds.advance(gpu, settings, it, relocate_until, scored, extent, &filter_cams)?;
        if it % settings.relocate_every == 0 && it >= 500 && it <= relocate_until {
            // The last round's plan must land before the next round reads the set (rare: rounds are 100 steps apart).
            if let Some(h) = rounds.plan.take() {
                let plan = h.join().map_err(|_| anyhow::anyhow!("{}", tr("the relocation planner panicked")))?;
                gpu.apply_relocation(&plan)?;
                regularise_for(gpu, settings, plan.len, extent);
            }
            if rounds.weights.is_none() {
                // Births at measured depth for this round, drawn now from what the device holds (crate::births).
                let spawns = match &spawner {
                    Some(sp) => {
                        let _cpu = crate::priority::CpuWork::start();
                        let count = ((settings.depth_births * (gpu.count / 10) as f32).ceil() as usize).max(1);
                        crate::timing::section("4d0 depth births", Some(&dev), || sp.draw(gpu, data, count, settings.background, &mut rng))?
                    }
                    None => Vec::new(),
                };
                rounds.weights = Some((it, gpu.request_relocation_weights(settings.max_radius, scored)?, spawns));
            }
        } else if it % 100 == 0 && it + 100 <= iterations {
            // As Mip-Splatting does: every 100 steps but the last 100's.
            crate::timing::section("4e 3D filter", Some(&dev), || gpu.update_filter_3d(&filter_cams, settings.filter_3d))?;
        }
    }
    drop(feeding);
    // A round still on its way (none should be: rounds end at `relocate_until`) moves nothing now.
    if let Some(h) = rounds.plan.take() {
        let _ = h.join();
    }
    drop(rounds);
    let _sync = crate::timing::on().then(|| dev.poll(wgpu::PollType::wait_indefinitely()));
    let end_start = Instant::now();
    let paced = gpu.pacing();
    if paced.vr || paced.desktop {
        eprintln!(
            "governor: {:.1} s waiting for VRChat's frames ({} looks without room, longest {:.1} s), {} back-offs, {} waits cut short by a pause",
            paced.waited.as_secs_f32(),
            paced.waits,
            paced.longest.as_secs_f32(),
            paced.backoffs,
            paced.cut_by_pause
        );
    }
    let (mut wearer_left, mut floor_patched) = trained.as_ref().map_or((0, 0), |t| (t.wearer_left, t.floor_patched));
    let mut g = match trained {
        // The wearer went before this was kept: never twice.
        Some(t) => t.gaussians,
        None => {
            // The result keeps the filter in its scales and opacities: the PLY renders what training rendered.
            let mut g = gpu.download_params()?;
            g.fuse_filter();
            g.retain(&live(&g));
            if let Some(volume) = &settings.wearer {
                wearer_left = volume.retain_outside(&mut g) + crate::wearer::leave_out_unseen(data, &held_out, volume, &mut g);
                floor_patched = crate::wearer::patch_floor(volume, &mut g);
                eprintln!("the wearer's volume: {wearer_left} Gaussians left; the floor they stood on: {floor_patched} patched");
            }
            ensure!(!g.is_empty(), "{}", tr("training left no Gaussian that shows anywhere"));
            if settings.sky_fill {
                // The sky shell's far field, as the measurements took it: beyond half the shell's distance from the cameras.
                let added = crate::sky::complete(&mut g, camera_centre(data), crate::init::SKY * 0.5, crate::init::SKY);
                eprintln!("sky: {added} Gaussians where no view saw the sky");
            }
            // From here on the device holds the result: what the PLY keeps is what scores.
            gpu.upload(&g);
            g
        }
    };
    if let Some(lab) = control.lab {
        lab.entering(crate::checkpoint::LabStage::Finish, control.pause);
    }
    let psnr_train = match finish(data, gpu, &mut g, settings, &train_views, &filter_cams) {
        Ok(psnr) => psnr,
        Err(e) if e.is::<Interrupted>() => {
            // Seconds of renders to do again, with what they read kept: the steps are done, the set is final.
            eprintln!("pausing after the last step");
            let seconds = earlier + start.elapsed().as_secs_f32();
            return Ok(Ran::Paused(Start::Trained(Box::new(TrainedState { gaussians: g, iterations, seconds, psnr: last_psnr, wearer_left, floor_patched }))));
        }
        Err(e) => return Err(e),
    };
    crate::timing::add("4f train end (download, seen, cleanup, train score)", end_start.elapsed());
    Ok(Ran::Finished(Outcome { gaussians: g, iterations, held_out, psnr_train, seconds: earlier + start.elapsed().as_secs_f32(), wearer_left, floor_patched }))
}

/// What follows the steps, on the device holding the result: what the keyframes saw of each Gaussian, the clean-up and the
/// score on the training views. Renders that take seconds, and a pause cuts them short ([`Interrupted`]) with nothing
/// lost but them: `g` stays as it was until they are done.
fn finish(data: &Dataset, gpu: &mut Gpu, g: &mut Gaussians, settings: &Settings, train_views: &[usize], filter_cams: &[Camera]) -> anyhow::Result<f32> {
    // What the keyframes saw of each, which the PLY keeps for viewers (crate::gaussians::Seen).
    let mut seen = gpu.seen(filter_cams, settings.background)?;
    let beyond = crate::cleanup::beyond(g, &data.depth_views);
    for (s, b) in seen.iter_mut().zip(beyond) {
        s.beyond = b;
    }
    g.seen = seen;
    if settings.cleanup {
        let _cpu = crate::priority::CpuWork::start();
        let focal = filter_cams[0].fx.max(filter_cams[0].fy);
        let (keep, cleaned) = crate::cleanup::clean(g, &data.depth_views, filter_cams, focal);
        eprintln!("cleanup: {} floaters and {} islands left out of {}", cleaned.floaters, cleaned.islands, g.len());
        if cleaned.floaters + cleaned.islands > 0 {
            g.retain(&keep);
            gpu.upload(g);
        }
    }
    if gpu.overflows > 0 {
        eprintln!("{} steps had more tile entries than the sort expected (dropped for that step)", gpu.overflows);
    }
    let sample: Vec<usize> = train_views.iter().copied().step_by((train_views.len() / 8).max(1)).collect();
    Ok(evaluate(gpu, data, &sample, settings.background)?.unwrap_or(0.0))
}

/// Whether the keyframes go on the device: never on a Mac (its GPU reads the RAM they are in), never when a pause is
/// already asked, never for a run resumed past its steps (no step reads them), else when `room` says they fit.
fn keyframes_resident(apple: bool, paused: bool, trained_only: bool, room: impl FnOnce() -> bool) -> bool {
    !apple && !paused && !trained_only && room()
}

/// Whether this start follows VRChat before its loop: only a run resumed past its steps, which takes no step and so no
/// relocation round whose landing step pacing could move. Fresh and between-steps starts keep their old schedule.
fn paced_before_loop(trained_only: bool) -> bool {
    trained_only
}

/// The regularisers' weights per Gaussian of a set of `n`.
fn regularise_for(gpu: &mut Gpu, settings: &Settings, n: usize, extent: f32) {
    gpu.opacity_reg = settings.opacity_reg / n.max(1) as f32;
    gpu.scale_reg = settings.scale_reg / (n.max(1) as f32 * extent);
}

/// The relocation rounds off the steps' path: a round's weights on their way back from the device, and a plan being made
/// from them on a helper thread.
#[derive(Default)]
struct Rounds {
    weights: Option<(u32, crate::gpu::PendingWeights, Vec<[f32; 14]>)>,
    plan: Option<std::thread::JoinHandle<Relocation>>,
}

impl Rounds {
    /// A plan that is ready lands now; weights that arrived go to a planner.
    #[allow(clippy::too_many_arguments)]
    fn advance(&mut self, gpu: &mut Gpu, settings: &Settings, it: u32, relocate_until: u32, scored: bool, extent: f32, filter_cams: &[Camera]) -> anyhow::Result<()> {
        if self.plan.as_ref().is_some_and(|h| h.is_finished()) {
            let plan = self.plan.take().expect("checked").join().map_err(|_| anyhow::anyhow!("{}", tr("the relocation planner panicked")))?;
            crate::timing::section("4d3 relocation apply", None, || gpu.apply_relocation(&plan))?;
            regularise_for(gpu, settings, plan.len, extent);
            gpu.update_filter_3d(filter_cams, settings.filter_3d)?;
        }
        if self.plan.is_none() {
            if let Some((round, w, spawns)) = self.weights.take() {
                match w.take()? {
                    Some(mut weights) => {
                        // With birth scores, they follow the weights (Gpu::request_relocation_weights).
                        let scores = if scored { weights.split_off(weights.len() / 4) } else { Vec::new() };
                        let n = weights.len();
                        if settings.noise > 0.0 && settings.noise_dead_max > 0.0 && it <= relocate_until {
                            // The noise runs while the budget binds: not in a round whose relocation found much of
                            // the set dead (Settings::noise_dead_max).
                            let share = weights.iter().filter(|w| **w == -1.0 || !w.is_finite()).count() as f32 / n.max(1) as f32;
                            let on = share <= settings.noise_dead_max;
                            if on != (gpu.noise > 0.0) {
                                eprintln!("it {round}: {:.0} % dead: noise {}", share * 100.0, if on { "on" } else { "off" });
                            }
                            gpu.noise = if on { settings.noise } else { 0.0 };
                        }
                        if round % 500 == 0 {
                            let dead = weights.iter().filter(|w| **w == -1.0).count();
                            eprintln!("it {round}: {n} Gaussians, {dead} dead or huge");
                        }
                        let (grow_to, starved) = ((n + n / 20).min(settings.budget), settings.starved && n < settings.budget / 4);
                        let mut plan_rng = fastrand::Rng::with_seed(settings.seed ^ (round as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
                        let (sources, by_opacity, split, share) = (settings.sources, settings.sources_by_opacity, settings.split, settings.depth_births);
                        self.plan = Some(std::thread::spawn(move || {
                            let births = Births { sources, by_opacity, split, scores: scored.then_some(scores.as_slice()), spawns: &spawns, spawn_share: share };
                            plan_relocation_by(&weights, grow_to, starved, &births, &mut plan_rng)
                        }));
                    }
                    None => self.weights = Some((round, w, spawns)),
                }
            }
        }
        Ok(())
    }

    /// Waits for what is on its way, and lands it: the GPU's queue, the weights, the planner.
    #[allow(clippy::too_many_arguments)]
    fn settle(&mut self, gpu: &mut Gpu, dev: &wgpu::Device, settings: &Settings, it: u32, relocate_until: u32, scored: bool, extent: f32, filter_cams: &[Camera]) -> anyhow::Result<()> {
        while self.weights.is_some() || self.plan.is_some() {
            dev.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow::anyhow!("{}", tr!("waiting for the GPU: {error}", error = e)))?;
            if self.plan.as_ref().is_some_and(|h| !h.is_finished()) {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            self.advance(gpu, settings, it, relocate_until, scored, extent, filter_cams)?;
        }
        Ok(())
    }
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

/// An image at half its size, each pixel the mean of the four under it. One of the four left out (alpha 0: it showed the
/// wearer, crate::wearer::is_out) leaves the pixel out, as the least of the four makes its mask ([`half_mask`]): the
/// loss reads who is left out from the target's alpha (crate::gpu::Target::Color), and a coarse level made opaque
/// whatever was under it trained the wearer back in, at half and at a quarter of the size.
fn half_image(image: &[u32], w: u32, h: u32) -> Vec<u32> {
    let (hw, hh) = (w / 2, h / 2);
    let mut out = vec![0u32; (hw * hh) as usize];
    for y in 0..hh {
        for x in 0..hw {
            let at = |dx: u32, dy: u32| image[((2 * y + dy) * w + 2 * x + dx) as usize];
            let px = [at(0, 0), at(1, 0), at(0, 1), at(1, 1)];
            let mut v = if px.iter().any(|p| crate::wearer::is_out(*p)) { 0 } else { 255u32 << 24 };
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

/// What a relocation round places where (crate::train::plan_relocation_by): which live Gaussians it copies and how,
/// from the birth scores the device read back after the weights (relocate.wgsl `classify`: positional gradient, colour
/// error and distance from the measured surface, `n` each), and births at measured depth (crate::births) taking
/// `spawn_share` of the moves, in the order they were drawn.
#[derive(Clone, Copy, Debug, Default)]
pub struct Births<'a> {
    pub sources: Sources,
    /// Ranked sources' chances times their opacity ([`Settings::sources_by_opacity`]).
    pub by_opacity: bool,
    pub split: Split,
    pub scores: Option<&'a [f32]>,
    pub spawns: &'a [[f32; 14]],
    pub spawn_share: f32,
}

/// Each of `pool`'s ranks by `score` among them, from 1/len (lowest) to 1 (highest), into `out` (indexed like the set);
/// a score that is not positive (a Gaussian no colour step showed) or no number ranks 0.
fn rank_into(pool: &[usize], score: impl Fn(usize) -> f32, out: &mut [f32]) {
    let mut order: Vec<(f32, usize)> = pool.iter().map(|&i| (score(i), i)).filter(|(v, _)| v.is_finite() && *v > 0.0).collect();
    order.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    for &i in pool {
        out[i] = 0.0;
    }
    let len = pool.len().max(1) as f32;
    let skipped = pool.len() - order.len();
    for (k, &(_, i)) in order.iter().enumerate() {
        out[i] = (skipped + k + 1) as f32 / len;
    }
}

/// Decides a relocation round from the Gaussians' [`weights`]: dead ones and new ones, up to `grow_to`, become copies
/// of live ones sampled by opacity, never sky ones (unless nothing else lives). A weight that is no number counts as
/// dead; weights that sum to nothing (a readback of zeros) move nothing. `starved` ([`Settings::starved`]): the set
/// grows whatever number are dead.
pub fn plan_relocation(weights: &[f32], grow_to: usize, starved: bool, rng: &mut fastrand::Rng) -> Relocation {
    plan_relocation_by(weights, grow_to, starved, &Births::default(), rng)
}

/// [`plan_relocation`] with sources, splits and births at measured depth as `births` says.
pub fn plan_relocation_by(weights: &[f32], grow_to: usize, starved: bool, births: &Births, rng: &mut fastrand::Rng) -> Relocation {
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
    let mut plan = Relocation { len: n, ..Default::default() };
    // Births at measured depth take their share of the moves (they need no source).
    let wanted = dead.len() + grow;
    let spawned = ((births.spawn_share.clamp(0.0, 1.0) * wanted as f32).round() as usize).min(births.spawns.len());
    // No more moves than the live ones can take (MAX_RATIO − 1 copies each): after a mass fade a few sources remain.
    let moves = (wanted - spawned).min(alive.len() * (MAX_RATIO - 1));
    let mut free = dead.into_iter();
    let mut target = |plan: &mut Relocation| {
        free.next().unwrap_or_else(|| {
            plan.len += 1;
            plan.len - 1
        })
    };
    for b in &births.spawns[..spawned] {
        let t = target(&mut plan);
        let mut entry = [0u32; 16];
        entry[0] = t as u32;
        for (e, v) in entry[1..15].iter_mut().zip(b) {
            *e = v.to_bits();
        }
        plan.spawns.push(entry);
    }
    if moves == 0 {
        return plan;
    }
    // The sampling weight of each live Gaussian (Sources).
    let scores = births.scores.filter(|s| s.len() >= 3 * n);
    let mut chance = vec![0f32; n];
    match (births.sources, scores) {
        (Sources::Opacity, _) | (_, None) => alive.iter().for_each(|&i| chance[i] = opacity(i)),
        (Sources::Gradient, Some(s)) => rank_into(&alive, |i| s[i], &mut chance),
        (Sources::Error, Some(s)) => rank_into(&alive, |i| s[n + i], &mut chance),
        (Sources::Confidence, Some(s)) => {
            let mut error = vec![0f32; n];
            rank_into(&alive, |i| s[i], &mut chance);
            rank_into(&alive, |i| s[n + i], &mut error);
            alive.iter().for_each(|&i| chance[i] *= 0.2 + error[i]);
        }
    }
    if births.by_opacity && births.sources != Sources::Opacity && scores.is_some() {
        alive.iter().for_each(|&i| chance[i] *= opacity(i));
    }
    let mut pool = alive.clone();
    let mut cdf = Vec::with_capacity(pool.len());
    let build = |pool: &[usize], cdf: &mut Vec<f64>| {
        cdf.clear();
        let mut acc = 0.0f64;
        for &i in pool {
            acc += chance[i].max(0.0) as f64;
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
    // Sources split along their longest axis (Split): one copy each, and anchored when the split asks for depth.
    let anchored = |j: usize| scores.is_some_and(|s| (0.0..ANCHORED).contains(&s[2 * n + j]));
    let halves = |j: usize| {
        ratio[j] == 2
            && match births.split {
                Split::Footprint => false,
                Split::LongAxis => true,
                Split::LongAxisOnDepth => anchored(j),
            }
    };
    for &j in &alive {
        if ratio[j] > 1 {
            let (new_o, factor) = relocation(opacity(j) as f64, ratio[j]);
            let new_logit = logit((new_o as f32).clamp(DEAD, 1.0 - 1e-4));
            // A split's halves get their footprints in the copy (relocate.wgsl `copy`, mode 1).
            let delta = if halves(j) { 0.0 } else { (factor as f32).max(1e-6).ln() };
            plan.adjust.push([j as u32, new_logit.to_bits(), delta.to_bits(), 0]);
        }
    }
    for &j in &picks {
        // Somewhere inside the source's footprint, so the copies don't share every gradient forever.
        let e = [normal(rng), normal(rng), normal(rng)];
        let t = target(&mut plan);
        plan.copies.push([t as u32, j as u32, e[0].to_bits(), e[1].to_bits(), e[2].to_bits(), u32::from(halves(j)), 0, 0]);
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
    let mut put = |g: &mut Gaussians, t: usize, mean: [f32; 3], log_scale: [f32; 3], rotation: [f32; 4], opacity_logit: f32, color: [f32; 3], sky: bool| {
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
    };
    for c in &plan.copies {
        let (t, j) = (c[0] as usize, c[1] as usize);
        let r = rotation_matrix(g.rotations[j]);
        let s = g.log_scales[j].map(f32::exp);
        let (log_scale, rotation, opacity_logit, color, sky) = (g.log_scales[j], g.rotations[j], g.opacity_logits[j], g.colors[j], g.sky[j]);
        if c[5] == 1 {
            // Split along the longest axis (Split::LongAxis).
            let axis = (0..3).fold(0, |a, k| if log_scale[k] > log_scale[a] { k } else { a });
            let off: [f32; 3] = std::array::from_fn(|row| r[row][axis] * 0.45 * s[axis]);
            let shrunk: [f32; 3] = std::array::from_fn(|k| log_scale[k] + if k == axis { 0.55f32.ln() } else { 0.8930f32.ln() });
            let mean = g.means[j];
            put(g, t, std::array::from_fn(|k| mean[k] - off[k]), shrunk, rotation, opacity_logit, color, sky);
            g.means[j] = std::array::from_fn(|k| mean[k] + off[k]);
            g.log_scales[j] = shrunk;
            continue;
        }
        let e = [f32::from_bits(c[2]) * s[0], f32::from_bits(c[3]) * s[1], f32::from_bits(c[4]) * s[2]];
        let mut mean = g.means[j];
        for (row, m) in r.iter().zip(mean.iter_mut()) {
            *m += row[0] * e[0] + row[1] * e[1] + row[2] * e[2];
        }
        put(g, t, mean, log_scale, rotation, opacity_logit, color, sky);
    }
    for b in &plan.spawns {
        let v: [f32; 14] = std::array::from_fn(|k| f32::from_bits(b[1 + k]));
        put(g, b[0] as usize, [v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8], v[9]], v[10], [v[11], v[12], v[13]], false);
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

    /// C4 and RECONSTRUCT-4: a run resumed past its steps puts no keyframe on the device and never asks for room; any
    /// resume follows VRChat before its first step, a fresh run as before (at its step 1).
    #[test]
    fn a_trained_resume_uploads_no_keyframes_and_resumes_paced() {
        let asked = std::cell::Cell::new(false);
        let room = || {
            asked.set(true);
            true
        };
        assert!(!keyframes_resident(false, false, true, room) && !asked.get());
        assert!(keyframes_resident(false, false, false, || true));
        assert!(!keyframes_resident(true, false, false, || true) && !keyframes_resident(false, true, false, || true));
        assert!(!keyframes_resident(false, false, false, || false));
        // night2: a run resumed between steps keeps its old schedule (pacing would move where relocation plans land).
        assert!(paced_before_loop(true));
        assert!(!paced_before_loop(false));
    }

    /// The coarse levels keep the wearer out. The loss reads who is left out from the target's alpha, and a level's
    /// pixel is left out when any pixel under it is, through both reductions (half, then a quarter); every other pixel
    /// stays an opaque mean of its four. A level made opaque whatever was under it trained the wearer back in.
    #[test]
    fn coarse_levels_keep_out_what_shows_the_wearer() {
        use crate::wearer::is_out;
        let pixels = [0x00ff_ffff, 0xff11_2233, 0xff11_2233, 0xff11_2233];
        assert!(is_out(half_image(&pixels, 2, 2)[0]));
        assert_eq!(half_image(&[0xff10_2030, 0xff30_4050, 0xff10_2030, 0xff30_4050], 2, 2), [0xff20_3040]);
        // A view of 8×4 with one pixel of the wearer, at (5, 2): out at (2, 1) of its half, at (1, 0) of its quarter.
        let (w, h) = (8u32, 4u32);
        let mut image = vec![0xff40_8020u32; (w * h) as usize];
        image[(2 * w + 5) as usize] &= 0x00ff_ffff;
        let whole = vec![0xff40_8020u32; (w * h) as usize];
        let view = |image: Vec<u32>| crate::dataset::View { camera: Camera::look([0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 1.0, w, h), image, game_time: 0.0, keyframe: 0 };
        let data = Dataset {
            dir: Default::default(),
            views: vec![view(image), view(whole)],
            depth_views: Vec::new(),
            faces: None,
            width: w,
            height: h,
            geometry: crate::camera::Geometry::new(w, h, 1),
            mask: vec![1.0; (w * h) as usize],
            loaded: Default::default(),
        };
        let levels = levels(&data);
        assert_eq!(levels.iter().map(|l| (l.width, l.height)).collect::<Vec<_>>(), [(4, 2), (2, 1)]);
        let out = |level: &Level, view: usize| level.images[view].iter().map(|p| is_out(*p)).collect::<Vec<_>>();
        assert_eq!(out(&levels[0], 0), [false, false, false, false, false, false, true, false]);
        assert_eq!(out(&levels[1], 0), [false, true]);
        // The colours are the means they were, and a view nobody is in stays whole at both levels.
        assert!(levels.iter().all(|l| l.images[0].iter().all(|p| p & 0x00ff_ffff == 0x0040_8020)));
        assert!(levels.iter().all(|l| l.images[1].iter().all(|p| *p == 0xff40_8020)));
        // The scene mask is the session's own, the same for every view: the wearer is in the targets alone.
        assert!(levels.iter().all(|l| l.mask.iter().all(|m| *m == 1.0) && l.scene_pixels == (l.width * l.height) as f32));
    }

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

    /// Sources drawn by a birth score never include a Gaussian no colour step showed (score 0), favour the high ranks,
    /// and draw by rank, not by value: one huge score takes no more than its rank's share.
    #[test]
    fn births_follow_the_scores_they_are_sampled_by() {
        let n = 20_000;
        // Every 20th dead; the others alive at one opacity, the first half never shown (score 0, the lowest ranks).
        let weights: Vec<f32> = (0..n).map(|i| if i % 20 == 0 { -1.0 } else { 0.5 }).collect();
        let (mut up, mut down) = (vec![0f32; 3 * n], vec![0f32; 3 * n]);
        for i in n / 2..n {
            up[i] = i as f32;
            up[n + i] = i as f32;
            down[n + i] = (n - i) as f32;
        }
        up[n - 1] = 1e30;
        // Rank-proportional over the upper half (ranks 0.5 to 1): the mean source at about 15,555 by gradient, 15,966
        // by gradient × (0.2 + error) both rising, 14,445 by a falling error (about 90 a standard error).
        for (sources, scores, mean_at) in [(Sources::Gradient, &up, 15_555.0), (Sources::Confidence, &up, 15_966.0), (Sources::Error, &down, 14_445.0)] {
            let births = Births { sources, scores: Some(scores), ..Default::default() };
            let plan = plan_relocation_by(&weights, n, false, &births, &mut fastrand::Rng::with_seed(9));
            assert_eq!(plan.copies.len(), n / 20, "{sources:?}");
            let picked: Vec<usize> = plan.copies.iter().map(|c| c[1] as usize).collect();
            assert!(picked.iter().all(|&j| j >= n / 2), "{sources:?}: a source no colour step showed");
            let mean = picked.iter().sum::<usize>() as f32 / picked.len() as f32;
            assert!((mean - mean_at).abs() < 300.0, "{sources:?}: mean source {mean}, expected about {mean_at}");
            assert!(picked.iter().filter(|&&j| j == n - 1).count() < 10, "{sources:?}: an outlier took the copies");
        }
        // By opacity (the default), sources ignore the scores.
        let plan = plan_relocation_by(&weights, n, false, &Births { scores: Some(&up), ..Default::default() }, &mut fastrand::Rng::with_seed(9));
        assert!(plan.copies.iter().any(|c| (c[1] as usize) < n / 2));
    }

    /// With `by_opacity`, ranked sources' chances are times their opacity: faint high-ranked Gaussians, whose copies
    /// would die, take few of the copies.
    #[test]
    fn ranked_sources_by_opacity_spare_the_faint() {
        let n = 10_000;
        // All alive, score rising with the index; every other one faint (opacity 0.01).
        let weights: Vec<f32> = (0..n).map(|i| if i % 20 == 0 { -1.0 } else if i % 2 == 1 { 0.01 } else { 0.8 }).collect();
        let mut scores = vec![0f32; 3 * n];
        for (i, s) in scores[..n].iter_mut().enumerate() {
            *s = i as f32 + 1.0;
        }
        let faint = |plan: &Relocation| plan.copies.iter().filter(|c| c[1] % 2 == 1).count() as f32 / plan.copies.len() as f32;
        let ranked = plan_relocation_by(&weights, n, false, &Births { sources: Sources::Gradient, scores: Some(&scores), ..Default::default() }, &mut fastrand::Rng::with_seed(15));
        let weighted = plan_relocation_by(&weights, n, false, &Births { sources: Sources::Gradient, by_opacity: true, scores: Some(&scores), ..Default::default() }, &mut fastrand::Rng::with_seed(15));
        assert!(faint(&ranked) > 0.4, "ranks alone: {} faint", faint(&ranked));
        assert!(faint(&weighted) < 0.05, "by opacity: {} faint", faint(&weighted));
        let mean = weighted.copies.iter().map(|c| c[1] as f32).sum::<f32>() / weighted.copies.len() as f32;
        assert!(mean > 0.6 * n as f32, "still by rank: mean source {mean}");
    }

    /// A long-axis split halves sources with one copy (mode 1, the footprint left to the copy kernel); on depth, only
    /// sources anchored on the measured surface.
    #[test]
    fn a_long_axis_split_halves_sources_with_one_copy() {
        let n = 4000;
        let weights: Vec<f32> = (0..n).map(|i| if i % 20 == 0 { -1.0 } else { 0.5 }).collect();
        let mut scores = vec![0f32; 3 * n];
        for i in 0..n {
            // Every other Gaussian anchored (1 % from the surface), the rest 10 % off or never seen by depth.
            scores[2 * n + i] = [0.01, 0.1, -1.0, 0.01][i % 4];
        }
        for split in [Split::LongAxis, Split::LongAxisOnDepth] {
            let births = Births { split, scores: Some(&scores), ..Default::default() };
            let plan = plan_relocation_by(&weights, n, false, &births, &mut fastrand::Rng::with_seed(10));
            let mut copies = vec![0usize; n];
            for c in &plan.copies {
                copies[c[1] as usize] += 1;
            }
            let mut halved = 0;
            for c in &plan.copies {
                let j = c[1] as usize;
                let expected = copies[j] == 1 && (split == Split::LongAxis || scores[2 * n + j] == 0.01);
                assert_eq!(c[5] == 1, expected, "{split:?}: source {j} with {} copies", copies[j]);
                halved += usize::from(c[5] == 1);
            }
            assert!(halved > plan.copies.len() / 3, "{split:?}: {halved} of {} halved", plan.copies.len());
            for a in &plan.adjust {
                let halves = plan.copies.iter().any(|c| c[1] == a[0] && c[5] == 1);
                assert_eq!(f32::from_bits(a[2]) == 0.0, halves, "a split's source keeps its footprint for the copy kernel");
            }
        }
    }

    /// Births at measured depth take their share of the moves, into the dead slots first, in the order drawn.
    #[test]
    fn births_at_depth_take_their_share_of_the_moves() {
        let n = 1000;
        let weights: Vec<f32> = (0..n).map(|i| if i % 20 == 0 { -1.0 } else { 0.5 }).collect();
        let spawns: Vec<[f32; 14]> = (0..30).map(|k| [k as f32, 0.0, 0.0, -3.0, -3.0, -5.0, 1.0, 0.0, 0.0, 0.0, -2.0, 0.1, 0.2, 0.3]).collect();
        let births = Births { spawns: &spawns, spawn_share: 0.2, ..Default::default() };
        let plan = plan_relocation_by(&weights, n, false, &births, &mut fastrand::Rng::with_seed(11));
        assert_eq!(plan.spawns.len(), 10);
        assert_eq!(plan.copies.len(), 40);
        assert!(plan.spawns.iter().all(|b| b[0] % 20 == 0 && (b[0] as usize) < n), "spawns into dead slots");
        assert!(plan.spawns.iter().enumerate().all(|(k, b)| f32::from_bits(b[1]) == k as f32 && f32::from_bits(b[11]) == -2.0));
        // Growth: past the end once the dead slots are taken; a short list gives what it has.
        let plan = plan_relocation_by(&weights, n + 50, false, &Births { spawns: &spawns, spawn_share: 1.0, ..Default::default() }, &mut fastrand::Rng::with_seed(12));
        assert_eq!((plan.spawns.len(), plan.copies.len(), plan.len), (30, 70, n + 50));
    }

    /// The CPU reference splits along the longest axis as the copy kernel does: two halves at ±0.45 of its scale.
    #[test]
    fn the_long_axis_split_keeps_the_halves_on_the_axis() {
        let mut g = Gaussians::default();
        g.push([1.0, 2.0, 3.0], [0.1f32.ln(), 0.4f32.ln(), 0.01f32.ln()], [1.0, 0.0, 0.0, 0.0], logit(0.8), [0.5; 3]);
        g.push([0.0; 3], [-3.0; 3], [1.0, 0.0, 0.0, 0.0], logit(0.001), [0.5; 3]);
        let (mut m1, mut m2) = (vec![0.0; 2 * PACKED], vec![0.0; 2 * PACKED]);
        let plan = Relocation { adjust: vec![[0, logit(0.55).to_bits(), 0f32.to_bits(), 0]], copies: vec![[1, 0, 0, 0, 0, 1, 0, 0]], spawns: Vec::new(), len: 2 };
        apply_relocation(&mut g, &mut m1, &mut m2, &plan);
        assert!((g.means[0][1] - (2.0 + 0.18)).abs() < 1e-6 && (g.means[1][1] - (2.0 - 0.18)).abs() < 1e-6, "{:?}", g.means);
        assert!((g.log_scales[1][1].exp() - 0.22).abs() < 1e-6 && (g.log_scales[1][0].exp() - 0.0893).abs() < 1e-6);
        assert_eq!(g.log_scales[0], g.log_scales[1]);
        assert!((sigmoid(g.opacity_logits[1]) - 0.55).abs() < 1e-6);
    }

    /// What the defaults train with (the lab's reports/births.md): copies by opacity inside their source's footprint,
    /// no births at depth, and MCMC's SGLD noise on the nearly transparent Gaussians off the measured surface while the
    /// budget binds (rounds that find at most a quarter of the set dead), on every Gaussian, seen or not.
    #[test]
    fn the_defaults_explore_while_the_budget_binds() {
        let s = Settings::default();
        assert_eq!((s.sources, s.split, s.depth_births, s.sources_by_opacity), (Sources::Opacity, Split::Footprint, 0.0, false));
        assert!(s.noise == NOISE && s.noise_gate && !s.noise_visible && s.noise_dead_max == NOISE_DEAD_MAX);
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
    fn detail_views_are_never_held_out_and_train_on_top_of_the_others() {
        let s = crate::dataset::fixture::session("detail-held-out", 160, 90, 20, &[]);
        let mut d = crate::dataset::load_with(&s.dir, 160, crate::dataset::Budget::UNLIMITED, |_| {}).unwrap();
        // Without detail views: every 8th from the 4th, as before.
        assert_eq!(held_out_views(&d, 8), held_out(20, 8));
        // Views 4 and 5 zoomed in: never held out; every 8th of the 18 others.
        for v in &mut d.views[4..6] {
            v.camera.fy *= 2.35;
        }
        assert_eq!(held_out_views(&d, 8), vec![6, 14]);
        assert!(held_out_views(&d, 0).is_empty());
        // Steps: the others' 100 each (3000 at least), and a quarter more on top for a quarter of the colour steps.
        assert_eq!(detail_iterations(37, 0.25), 4933);
        assert_eq!(detail_iterations(10, 0.0), 3000);
        assert_eq!(detail_iterations(62, 0.25), 8267);
        assert!(detail_iterations(10, 1.0) <= 30_000, "a share of all is capped");
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