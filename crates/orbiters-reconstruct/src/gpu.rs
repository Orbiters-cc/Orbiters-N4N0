//! The splatting kernels on the GPU (wgpu: Direct3D 12 or Vulkan on Windows, Vulkan on Linux, Metal on a Mac) and the
//! buffers they work on. One forward + backward + Adam step per call. Training steps are pipelined: the host never
//! waits for a step, the sort is sized from intersection counts read back a few steps later and dispatched over the
//! count the GPU wrote itself. Images and parameters are read back only when asked. A step's small parameter blocks live in buffers it
//! rewrites and its bind groups are kept while their buffers live, so a step creates neither.
//!
//! GPU errors come back as errors: every readback and step checks for one (a training device keeps the first, so a
//! run stops at it), and a renderer on someone else's device catches its own in error scopes instead of reaching that
//! device's handler. On Metal this covers what wgpu validates only: wgpu-hal 30 does not read a command buffer's status,
//! so one the system stopped goes unheard. A single dispatch of 26 s ran to its end on an M3 (tests/bench.rs,
//! `how_long_a_dispatch_may_run`), far past the longest pass here, and [`Gpu::free_memory`] keeps runs within Metal's
//! recommended working set.

use crate::camera::Camera;
use crate::cpu;
pub use crate::dof::DepthOfField;
use crate::gaussians::{Gaussians, Seen, PACKED, SEEN_IN};
use anyhow::{anyhow, bail, Context as _};
use orbiters_i18n::tr;
use bytemuck::{Pod, Zeroable};
use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use wgpu::util::DeviceExt;

const GRAD2D: usize = 10;
/// [`Gpu::birth_stats`]: colour steps record each Gaussian's positional gradient and blended colour error, depth steps
/// its distance from the measured surface (shaders/param_grads.wgsl).
pub const BIRTH_STATS_COLOR: u32 = 1;
pub const BIRTH_STATS_DEPTH: u32 = 2;
const FAR: f32 = 1000.0;
/// Readbacks of intersection counts in flight: the host runs at most this many steps ahead of the GPU.
const RING: usize = 3;
/// Image sizes whose buffers a training device keeps besides the current one (full, half and quarter resolution,
/// the depth-sample grid).
const KEPT_SIZES: usize = 4;

/// How the backward pass adds up each Gaussian's gradients over the pixels of a tile. WGSL has no float atomics, so
/// the portable way is a compare-and-swap loop on the float's bits; native atomic adds are several times faster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GradientSums {
    /// Native f32 atomic adds (Vulkan with VK_EXT_shader_atomic_float, Metal).
    Float,
    /// 64-bit fixed point (units of 2⁻⁴⁰) with native i64 atomic adds (Vulkan with VK_KHR_shader_atomic_int64,
    /// D3D12 with Shader Model 6.6 through DXC). The sums do not depend on the order of the adds.
    FixedPoint,
    /// f32 adds as compare-and-swap loops (any device with subgroups).
    CompareAndSwap,
}

impl GradientSums {
    pub const ALL: [Self; 3] = [Self::Float, Self::FixedPoint, Self::CompareAndSwap];

    /// The fastest one `features` allow.
    pub fn best(features: wgpu::Features) -> Self {
        Self::ALL.into_iter().find(|s| features.contains(s.features())).unwrap_or(Self::CompareAndSwap)
    }

    /// Device features it needs.
    pub fn features(self) -> wgpu::Features {
        match self {
            Self::Float => wgpu::Features::SHADER_FLOAT32_ATOMIC,
            Self::FixedPoint => wgpu::Features::SHADER_INT64 | wgpu::Features::SHADER_INT64_ATOMIC_ALL_OPS,
            Self::CompareAndSwap => wgpu::Features::empty(),
        }
    }

    /// Bytes of one gradient slot.
    fn slot_bytes(self) -> u64 {
        if self == Self::FixedPoint {
            8
        } else {
            4
        }
    }

    /// WGSL of the backward's gradient buffer and `add`, and of the projection backward's `grad`.
    fn sources(self) -> (&'static str, &'static str) {
        match self {
            Self::Float => (include_str!("shaders/sums_float.wgsl"), include_str!("shaders/sums_read_float.wgsl")),
            Self::FixedPoint => (include_str!("shaders/sums_fixed.wgsl"), include_str!("shaders/sums_read_fixed.wgsl")),
            Self::CompareAndSwap => (include_str!("shaders/sums_cas.wgsl"), include_str!("shaders/sums_read_float.wgsl")),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Float => "f32 atomics",
            Self::FixedPoint => "64-bit fixed-point atomics",
            Self::CompareAndSwap => "compare-and-swap sums",
        }
    }
}

/// Choices for a training device; [`Gpu::new`] takes the defaults.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeviceOptions {
    /// Gradient sums to use (`None`: the fastest the adapter has). One the adapter lacks is an error.
    pub sums: Option<GradientSums>,
    /// Time every pass with GPU timestamps ([`Gpu::profiler`]), when the adapter can.
    pub profile: bool,
    /// The integrated GPU (by its device type: Windows' power preference may hand back the graphics card) instead of
    /// the fastest one.
    pub low_power: bool,
    /// Vulkan: the device's queue at LOW global priority (`VK_KHR_global_priority`), meant for work that should give way
    /// to interactive applications: training while VRChat runs. Where the device cannot, it trains at normal priority.
    pub low_priority: bool,
    /// Tests: the backward adds its sums as a subgroup of this many lanes would, and never with the 32-lane transposed
    /// sums (0: as the device runs). Lets a 32-lane GPU check the paths 4-, 8- and 16-lane devices take.
    pub sum_lanes: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default, Debug)]
struct Frame {
    r0: [f32; 4],
    r1: [f32; 4],
    r2: [f32; 4],
    focal: [f32; 4],
    limits: [f32; 4],
    background: [f32; 4],
    size: [u32; 4],
    counts: [u32; 4],
    loss: [f32; 4],
    reg: [f32; 4],
    confidence: [f32; 4],
    far: [f32; 4],
    horizon: [f32; 4],
    dof: [f32; 4],
    probe: [f32; 4],
}

/// How a viewer shows the splats its keyframes saw weakly, rarely, from one side only or from much farther than the eye
/// is now (common.wgsl `trust`, from crate::gaussians::Seen): faded out, blurred or turned to the haze of the horizon
/// ([`FarField::horizon`]), by
/// `amount` from 0 (as trained) to 1. Only with [`Gpu::guards`]; the sky shell never changes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Confidence {
    pub amount: f32,
    pub mode: ConfidenceMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConfidenceMode {
    #[default]
    Fade,
    Blur,
    Haze,
}

/// How a viewer shows what lies beyond the reach of the depth frames (crate::gaussians::Seen::beyond): as trained,
/// fogged towards `horizon` (encoded sRGB; crate::sky::horizon) as `1 − e^(−beyond / distance)`, or cropped, fading out
/// over `distance` metres. Only with [`Gpu::guards`]; the sky shell never changes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FarField {
    pub mode: FarMode,
    pub distance: f32,
    pub horizon: [f32; 3],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FarMode {
    #[default]
    Shown,
    Fog,
    Crop,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SortInfo {
    n: u32,
    blocks: u32,
    shift: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct AdamInfo {
    lr: [f32; 16],
    beta: [f32; 4],
    n: [u32; 4],
}

/// Learning rates per parameter group.
#[derive(Clone, Copy, Debug)]
pub struct LearningRates {
    pub means: f32,
    pub log_scales: f32,
    pub rotations: f32,
    pub opacity: f32,
    pub colors: f32,
}

/// What one training step looks at: a keyframe (colour loss) or a depth frame (depth loss).
pub enum Target<'a> {
    /// Colour loss (1 − ssim)·L1 + ssim·(1 − SSIM) over the scene pixels. With `robust` > 0, pixels inside blobs that
    /// err by more than it (mean absolute error, 0–1) leave this step's loss: what no static scene explains.
    Color { rgba: &'a [u32], scene_pixels: f32, weight: f32, ssim: f32, robust: f32 },
    Depth { samples: &'a [[f32; 4]], weight: f32 },
    /// No loss (forward only).
    None,
}

/// The learning rate of each of a Gaussian's 16 parameter slots (padding slots 0: Adam leaves them).
fn rates_of(lr: LearningRates) -> [f32; 16] {
    let mut rates = [0f32; 16];
    rates[0..3].fill(lr.means);
    rates[3..6].fill(lr.log_scales);
    rates[6..10].fill(lr.rotations);
    rates[10] = lr.opacity;
    rates[11..14].fill(lr.colors);
    rates
}

pub fn block_on<F: Future>(f: F) -> F::Output {
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    let mut f = std::pin::pin!(f);
    loop {
        if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
}

struct Kernels {
    project: wgpu::ComputePipeline,
    scan_blocks: wgpu::ComputePipeline,
    add_sums: wgpu::ComputePipeline,
    total: wgpu::ComputePipeline,
    duplicate: wgpu::ComputePipeline,
    sort_count: wgpu::ComputePipeline,
    sort_scatter: wgpu::ComputePipeline,
    ranges: wgpu::ComputePipeline,
    forward: wgpu::ComputePipeline,
    present: wgpu::ComputePipeline,
    present_alpha: wgpu::ComputePipeline,
    /// The sky map in place of the constant background (crate::sky::SkyMap).
    sky_composite: wgpu::ComputePipeline,
    /// Only on a training device (the backward pass needs subgroup operations).
    training: Option<Training>,
}

/// A training device's kernels. Its steps take their own paths: large Gaussians split off the projection and the tile
/// entries (shaders/project_split.wgsl, duplicate_split.wgsl), the sort's scatter by subgroup ballots
/// (sort_subgroups.wgsl), the backward with two pixels a thread on 16×16 tiles (raster_backward_blocks.wgsl), depth
/// steps on 8×8 tiles, and the projection backward fused with Adam (project_backward_adam.wgsl).
struct Training {
    sums: GradientSums,
    /// One pixel a thread (the reference; [`Gpu::backward_from`] on 16×16 tiles too) and two on 16×16 tiles.
    backward: wgpu::ComputePipeline,
    backward_blocks: wgpu::ComputePipeline,
    loss_color: wgpu::ComputePipeline,
    ssim_partials: wgpu::ComputePipeline,
    ssim_gradient: wgpu::ComputePipeline,
    robust_residual: wgpu::ComputePipeline,
    robust_inliers: wgpu::ComputePipeline,
    depth_loss: wgpu::ComputePipeline,
    /// The parameter gradients alone (tests: [`Gpu::read_grads`]), and with the Adam step (training).
    project_backward: wgpu::ComputePipeline,
    project_backward_adam: wgpu::ComputePipeline,
    relocate_classify: wgpu::ComputePipeline,
    relocate_adjust: wgpu::ComputePipeline,
    relocate_copy: wgpu::ComputePipeline,
    relocate_spawn: wgpu::ComputePipeline,
    squared_error: wgpu::ComputePipeline,
    /// The largest weights (the PLY's seen statistics), and whether they reach a threshold (fading).
    visibility: wgpu::ComputePipeline,
    visibility_over: wgpu::ComputePipeline,
    /// The summed weights of a view ([`Gpu::contribution`]), and their fold into the run's.
    contribution: wgpu::ComputePipeline,
    contribution_fold: wgpu::ComputePipeline,
    filter_3d: wgpu::ComputePipeline,
    seen: wgpu::ComputePipeline,
    sky_gradient: wgpu::ComputePipeline,
    sky_adam: wgpu::ComputePipeline,
    sort_scatter: wgpu::ComputePipeline,
    project: wgpu::ComputePipeline,
    project_big: wgpu::ComputePipeline,
    duplicate: wgpu::ComputePipeline,
    duplicate_big: wgpu::ComputePipeline,
    /// Depth steps on 8×8 tiles.
    project8: wgpu::ComputePipeline,
    project_big8: wgpu::ComputePipeline,
    duplicate8: wgpu::ComputePipeline,
    duplicate_big8: wgpu::ComputePipeline,
    forward8: wgpu::ComputePipeline,
    backward8: wgpu::ComputePipeline,
    /// The forward with its loop nested (shaders/raster_forward_nested.wgsl), on 16×16 and 8×8 tiles ([`Gpu::nested_forward`]).
    forward_nested: wgpu::ComputePipeline,
    forward_nested8: wgpu::ComputePipeline,
}

/// A relocation round decided on the CPU (crate::train::plan_relocation) for [`Gpu::apply_relocation`]: sources' new
/// opacity logits and log-scale deltas (`[index, logit bits, delta bits, 0]`), copies (`[target, source, three standard
/// normal offsets as bits, mode, 0, 0]`, mode 1 a long-axis split; targets are dead slots, then the slots past the end),
/// new Gaussians written whole (`[target, mean, log scales, rotation, opacity logit, colour, 0]` as bits: births at
/// measured depth, crate::births) and the new set's size.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Relocation {
    pub adjust: Vec<[u32; 4]>,
    pub copies: Vec<[u32; 8]>,
    pub spawns: Vec<[u32; 16]>,
    pub len: usize,
}

/// Buffers that grow with the number of Gaussians.
struct SplatBuffers {
    cap: usize,
    params: wgpu::Buffer,
    grads: wgpu::Buffer,
    m1: wgpu::Buffer,
    m2: wgpu::Buffer,
    proj: wgpu::Buffer,
    counts: wgpu::Buffer,
    offsets: wgpu::Buffer,
    grad2d: wgpu::Buffer,
    stats: wgpu::Buffer,
    /// Each Gaussian's 3D filter variance ([`Gaussians::filter_3d`]).
    filter: wgpu::Buffer,
    /// What the keyframes saw of each ([`Gaussians::seen`], eight floats), for a viewer's [`Confidence`] and
    /// [`FarField`].
    seen: wgpu::Buffer,
    /// A viewer's mark of each ([`Gpu::set_marks`]), a byte each, four to a word.
    marks: wgpu::Buffer,
}

/// Buffers for one image size (its loss mask lives apart, in [`Gpu::masks`], so dropping these never loses it).
struct ImageBuffers {
    width: u32,
    height: u32,
    out_color: wgpu::Buffer,
    out_state: wgpu::Buffer,
    d_pixel: wgpu::Buffer,
    target: wgpu::Buffer,
    ranges: wgpu::Buffer,
    /// D-SSIM partial derivatives, three vec4s a pixel (training only).
    partials: wgpu::Buffer,
    /// Robust loss: each pixel's error and the mask of pixels kept this step (training only).
    resid: wgpu::Buffer,
    kept: wgpu::Buffer,
}

/// Sort entries (two copies: the radix passes ping-pong) and the per-block digit histograms.
struct SortBuffers {
    cap: usize,
    keys: [wgpu::Buffer; 2],
    values: [wgpu::Buffer; 2],
    hist: wgpu::Buffer,
    hist_offsets: wgpu::Buffer,
}

/// The sky map behind the splats (crate::sky::SkyMap): `width` × `width / 2` texels of four floats (rgb, unused), and
/// on a training device what trains them.
struct SkyBuffers {
    width: u32,
    texels: wgpu::Buffer,
    learning: Option<SkyLearning>,
}

/// The sky map's gradient sums (four slots a texel, as the backward pass sums) and Adam moments (four floats a texel).
struct SkyLearning {
    grads: wgpu::Buffer,
    m1: wgpu::Buffer,
    m2: wgpu::Buffer,
}

/// The sky map's Adam step (shaders/sky_adam.wgsl): learning rate, β1, β2, ε; texels, step.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SkyAdamInfo {
    rate: [f32; 4],
    n: [u32; 4],
}

/// A readback of the auto focus's depth histogram ([`Gpu::probe`]): 0 idle, 1 in flight, 2 mapped, 3 failed; which
/// probe it holds.
struct Probe {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU32>,
    seq: u64,
}

/// A readback of one pipelined step's intersection count.
struct Readback {
    buffer: wgpu::Buffer,
    /// 0 idle, 1 in flight, 2 mapped, 3 failed.
    state: Arc<AtomicU32>,
    submission: Option<wgpu::SubmissionIndex>,
    /// The step's image size and sort capacity.
    size: (u32, u32),
    cap: u32,
}

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter: String,
    k: Kernels,
    splats: Option<SplatBuffers>,
    image: Option<ImageBuffers>,
    /// Buffers of other image sizes used lately (training devices only).
    image_cache: Vec<ImageBuffers>,
    sort: Option<SortBuffers>,
    samples: Option<(usize, wgpu::Buffer)>,
    scan_levels: Vec<(usize, wgpu::Buffer, wgpu::Buffer)>,
    frame: wgpu::Buffer,
    total_staging: wgpu::Buffer,
    /// Written by the `total` kernel: [count, count clamped to the sort, indirect workgroups x, y, z].
    total_out: wgpu::Buffer,
    ring: Vec<Readback>,
    ring_next: usize,
    /// Per image size, a slowly decaying maximum of the intersection counts read back (sizes the sort).
    estimates: Vec<((u32, u32), u32)>,
    pub count: usize,
    /// Adam steps taken (bias correction).
    pub step: u32,
    /// Weights of the opacity and scale (per metre) regularisers per Gaussian in training steps.
    pub opacity_reg: f32,
    pub scale_reg: f32,
    /// Whether the regularisers pull on the Gaussians a step does not show too (crate::train::Settings).
    pub regularise_unseen: bool,
    /// Whether relocated Gaussians count Adam's bias correction from their restart (crate::train::Settings).
    pub adam_restart: bool,
    /// Birth statistics training steps record for relocation (shaders/common.wgsl stats_mode; crate::train::Sources):
    /// [`BIRTH_STATS_COLOR`] on colour steps, [`BIRTH_STATS_DEPTH`] on depth steps.
    pub birth_stats: u32,
    /// 3DGS-MCMC's SGLD noise on the means (shaders/project_backward_adam.wgsl `sgld`), times the position learning
    /// rate (0: none; crate::train::Settings::noise), and whether Gaussians anchored on measured depth are spared it.
    pub noise: f32,
    pub noise_gate: bool,
    /// And whether only the Gaussians a step shows take it (crate::train::Settings::noise_visible).
    pub noise_visible: bool,
    /// A viewer's guards (project.wgsl; [`Gpu::renderer`] sets them): splats fade in over the first 20 cm from the eye,
    /// those holding the eye are left out, none reaches further than 1024 px on screen, and they sort by distance from
    /// the eye. Training never uses them: what they hide there would never be corrected.
    pub guards: bool,
    /// With the guards: how the splats the keyframes saw poorly show, and the far field.
    pub confidence: Confidence,
    pub far: FarField,
    /// With the guards: the depth of field (crate::dof), and the pixel whose depth the auto focus asks for
    /// ([`Gpu::probed`]).
    pub dof: DepthOfField,
    pub probe: Option<[f32; 2]>,
    probes: Vec<Probe>,
    probe_bins: wgpu::Buffer,
    /// Probes recorded, and the newest of them read back.
    probe_seq: u64,
    probe_taken: u64,
    /// The sky map ([`Gpu::set_sky`]), its Adam learning rate, and what the kernels bind without one.
    sky: Option<SkyBuffers>,
    pub sky_lr: f32,
    no_sky: wgpu::Buffer,
    /// The 2D filter's variance in pixels² along x and y: cpu::FILTER_2D at the training resolution; a render on a
    /// coarser grid of the same rays scales it to keep the footprint.
    pub filter_2d: [f32; 2],
    /// Pipelined steps whose tile entries did not all fit the sort (they were dropped for that step).
    pub overflows: u32,
    /// A viewer's frame left entries out (it did not wait for its count): draw again ([`Gpu::needs_redraw`]).
    redraw: bool,
    /// GPU timestamps per section of each step ([`DeviceOptions::profile`]).
    pub profiler: Option<Profiler>,
    /// Training beside VRChat in VR (crate::governor): each part of a step (projection and sort; the forward pass and
    /// its loss; the backward pass and Adam) goes to the GPU alone once VRChat's frames leave its time idle, and is
    /// waited for. `paced` holds each part's GPU time as last measured.
    pub governor: Option<crate::governor::Governor>,
    paced: [std::time::Duration; 3],
    /// The adapter of a training device (for [`Gpu::free_memory`]), and whether it is an integrated GPU (VRChat's frames
    /// never wait for it).
    wgpu_adapter: Option<wgpu::Adapter>,
    pub integrated: bool,
    /// The first GPU error a training device reported out of band (or its loss): every later step and readback fails
    /// with it.
    errors: Arc<std::sync::Mutex<Option<String>>>,
    /// Visibility rounds: whether a weight reaches this (fading; 0: the largest weight), and whether they may go without
    /// waiting for each view's count ([`Gpu::visibility`]).
    visibility_over: f32,
    visibility_pipelined: bool,
    /// Visibility rounds sum the weights instead ([`Gpu::contribution`]).
    visibility_sum: bool,
    /// Steps keep every colour within 0–1, what a splat file's colour bytes hold (crate::tree_tune's decode-aware
    /// re-fit and tuning).
    pub clamp_colors: bool,
    /// The keyframes on the device ([`Gpu::set_keyframes`]), and the one the next colour step trains on instead of
    /// uploading its pixels.
    keyframes: Vec<wgpu::Buffer>,
    pub resident_target: Option<usize>,
    /// The list of large Gaussians a training step's projection splits off (shaders/project_split.wgsl).
    big: Option<wgpu::Buffer>,
    /// Whether the step being recorded is on 8×8 tiles (depth steps); tests' switches for every step on 8×8 tiles, and
    /// for the one-pixel-a-thread backward on 16×16 tiles (the reference the two-pixel one is checked against).
    tile8_step: bool,
    pub(crate) tile8_all: bool,
    /// Lab (crate::train::Settings::nested_forward): a training device's steps render with the forward whose loop is
    /// nested (shaders/raster_forward_nested.wgsl) instead of `forward`: the same entries, order and outputs.
    pub nested_forward: bool,
    pub(crate) reference_backward: bool,
    /// Loss masks per image size ([`Gpu::set_mask`]).
    masks: Vec<((u32, u32), wgpu::Buffer)>,
    /// Gaussians the splat buffers make room for at least ([`Gpu::reserve`]).
    reserved: usize,
    /// The steps' small parameter blocks, one buffer per use, rewritten each step.
    uniforms: RefCell<HashMap<(&'static str, usize), wgpu::Buffer>>,
    /// The steps' bind groups by pipeline and buffers, kept until one of those buffers is replaced.
    binds: RefCell<HashMap<BindKey, wgpu::BindGroup>>,
    /// Asked to pause, the long rounds of renders outside the steps stop between two views, failing with
    /// crate::checkpoint::Interrupted (crate::train::train_from starts them again).
    pub pause: Option<Arc<crate::checkpoint::Pause>>,
}

/// A bind group's pipeline and its (binding, buffer) entries.
type BindKey = (wgpu::ComputePipeline, Vec<(u32, wgpu::Buffer)>);

/// Relocation weights on their way back from the device ([`Gpu::request_relocation_weights`]).
pub struct PendingWeights {
    staging: wgpu::Buffer,
    state: Arc<AtomicU32>,
    n: usize,
}

impl PendingWeights {
    /// The weights, once they have arrived (a step's poll moves the mapping on); `None` while in flight.
    pub fn take(&self) -> anyhow::Result<Option<Vec<f32>>> {
        match self.state.load(Ordering::SeqCst) {
            0 => Ok(None),
            1 => {
                let view = self.staging.get_mapped_range(..).map_err(|e| anyhow!("{}", tr!("reading back from the GPU: {error}", error = format!("{e:?}"))))?;
                let out = bytemuck::cast_slice::<u8, f32>(&view[..self.n * 4]).to_vec();
                drop(view);
                self.staging.unmap();
                Ok(Some(out))
            }
            _ => anyhow::bail!("{}", tr("the GPU did not hand the relocation weights back (was the device lost?)")),
        }
    }
}

/// One colour step's rasteriser buffers ([`Gpu::lab_raster_dump`]), as the kernels see them.
pub struct RasterDump {
    /// The frame uniform (shaders/common.wgsl `Frame`), as 32-bit words.
    pub frame: Vec<u32>,
    /// Projected Gaussians, 16 floats each (project.wgsl).
    pub proj: Vec<f32>,
    /// Sorted entries' Gaussian indices, and each tile's [first, end) of them.
    pub values: Vec<u32>,
    pub ranges: Vec<u32>,
    /// The forward's output (before the sky composite) and per-pixel state (T bits, one past the last entry).
    pub out_color: Vec<[f32; 4]>,
    pub out_state: Vec<[u32; 2]>,
    /// The loss's pixel gradients, and the backward's 2D gradient sums (10 floats a Gaussian).
    pub d_pixel: Vec<[f32; 4]>,
    pub grad2d: Vec<f32>,
    /// The sky map's texels (rgba) when there is one.
    pub sky: Vec<[f32; 4]>,
    pub count: usize,
    pub tiles: (u32, u32),
}

/// A downloaded set: the Gaussians, their Adam moments (packed like the parameters) and statistics (param_grads.wgsl).
pub type Download = (Gaussians, Vec<f32>, Vec<f32>, Vec<[f32; 8]>);

const QUERIES: u32 = 2048;

/// GPU timestamps around every compute pass, summed per section of the step.
pub struct Profiler {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    period_ns: f64,
    used: std::cell::Cell<u32>,
    labels: std::cell::RefCell<Vec<&'static str>>,
    section: std::cell::Cell<&'static str>,
    /// Milliseconds and passes per section.
    pub totals: std::cell::RefCell<std::collections::BTreeMap<&'static str, (f64, u64)>>,
    /// GPU milliseconds from the first pass's start to the last pass's end, summed over profiled steps, and steps.
    pub span: std::cell::Cell<(f64, u64)>,
}

impl Profiler {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let set = device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("timestamps"), ty: wgpu::QueryType::Timestamp, count: QUERIES });
        let bytes = QUERIES as u64 * 8;
        let resolve = device.create_buffer(&wgpu::BufferDescriptor { label: Some("timestamps resolve"), size: bytes, usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        let readback = device.create_buffer(&wgpu::BufferDescriptor { label: Some("timestamps readback"), size: bytes, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Self {
            set,
            resolve,
            readback,
            period_ns: queue.get_timestamp_period() as f64,
            used: std::cell::Cell::new(0),
            labels: Default::default(),
            section: std::cell::Cell::new("other"),
            totals: Default::default(),
            span: std::cell::Cell::new((0.0, 0)),
        }
    }

    fn next(&self) -> Option<u32> {
        let i = self.used.get();
        if i + 2 > QUERIES {
            return None;
        }
        self.used.set(i + 2);
        self.labels.borrow_mut().push(self.section.get());
        Some(i)
    }

    /// Reads this step's timestamps (waits for the step).
    fn collect(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let n = self.used.get();
        if n == 0 {
            return;
        }
        let mut enc = device.create_command_encoder(&Default::default());
        enc.resolve_query_set(&self.set, 0..n, &self.resolve, 0);
        enc.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, n as u64 * 8);
        queue.submit([enc.finish()]);
        self.readback.map_async(wgpu::MapMode::Read, .., |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let Ok(view) = self.readback.get_mapped_range(..) else {
            // The timings did not come back (a lost device): this step's are dropped.
            self.labels.borrow_mut().clear();
            self.used.set(0);
            return;
        };
        let ticks: Vec<u64> = bytemuck::cast_slice::<u8, u64>(&view[..n as usize * 8]).to_vec();
        drop(view);
        self.readback.unmap();
        let labels = std::mem::take(&mut *self.labels.borrow_mut());
        let mut totals = self.totals.borrow_mut();
        let (mut first, mut last) = (u64::MAX, 0u64);
        for (k, label) in labels.iter().enumerate() {
            let (a, b) = (ticks[2 * k], ticks[2 * k + 1]);
            first = first.min(a);
            last = last.max(b);
            let e = totals.entry(label).or_insert((0.0, 0));
            e.0 += b.saturating_sub(a) as f64 * self.period_ns / 1e6;
            e.1 += 1;
        }
        let (s, c) = self.span.get();
        self.span.set((s + last.saturating_sub(first) as f64 * self.period_ns / 1e6, c + 1));
        self.used.set(0);
    }

    pub fn reset(&self) {
        self.totals.borrow_mut().clear();
        self.span.set((0.0, 0));
    }
}

fn storage(device: &wgpu::Device, label: &str, bytes: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.max(16).next_multiple_of(16),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

/// Workgroups for `n` invocations of 256-wide workgroups, spread over two dimensions past 65535.
fn groups(n: usize) -> (u32, u32) {
    let g = n.div_ceil(256).max(1);
    if g <= 65_535 {
        (g as u32, 1)
    } else {
        (65_535, g.div_ceil(65_535) as u32)
    }
}

fn groups_of(blocks: usize) -> (u32, u32) {
    let g = blocks.max(1);
    if g <= 65_535 {
        (g as u32, 1)
    } else {
        (65_535, g.div_ceil(65_535) as u32)
    }
}

/// Bits of a sort key below the tile id for an image of `width × height`: room for one more id than there are tiles
/// (an all-ones key, a sentinel, names no tile), and at most 24, which f32 holds exactly: with 25 or more, the
/// farthest depth rounds up to 2^bits and spills into the tile id (duplicate.wgsl clamps it as well).
pub fn depth_bits(width: u32, height: u32) -> u32 {
    depth_bits_for(width, height, cpu::TILE)
}

/// [`depth_bits`] for tiles of `tile` pixels a side.
pub fn depth_bits_for(width: u32, height: u32, tile: u32) -> u32 {
    let tiles = (width.div_ceil(tile) * height.div_ceil(tile)).max(2);
    // The tile id takes the bits of `tiles`' length, the depth the rest.
    tiles.leading_zeros().min(24)
}

/// The depth part of a sort key, as duplicate.wgsl computes it: log depth between the near and far planes in
/// `bits` bits, never more than fits.
pub fn depth_key(depth: f32, bits: u32) -> u32 {
    let (near, far) = (cpu::NEAR as f32, FAR);
    let t = ((depth / near).log2() / (far / near).log2()).clamp(0.0, 1.0);
    let max = (1u32 << bits) - 1;
    ((t * max as f32) as u32).min(max)
}

/// A Vulkan device on `adapter` whose queue asks for LOW global priority (`VK_KHR_global_priority`, or the older
/// `VK_EXT_global_priority`).
///
/// # Safety
/// `desc` must be what the adapter supports (as for [`wgpu::Adapter::request_device`]).
#[cfg(target_vendor = "apple")]
unsafe fn low_priority_device(_: &wgpu::Adapter, _: &wgpu::DeviceDescriptor<'_>) -> anyhow::Result<(wgpu::Device, wgpu::Queue)> {
    anyhow::bail!("{}", tr("Metal has no queue priorities"))
}

#[cfg(not(target_vendor = "apple"))]
unsafe fn low_priority_device(adapter: &wgpu::Adapter, desc: &wgpu::DeviceDescriptor<'_>) -> anyhow::Result<(wgpu::Device, wgpu::Queue)> {
    use ash::vk;
    let open = {
        // SAFETY: the hal adapter is used here only, while `adapter` lives.
        let hal = unsafe { adapter.as_hal::<wgpu::hal::api::Vulkan>() }.with_context(|| tr("not a Vulkan adapter"))?;
        // SAFETY: a read-only query of the physical device.
        let extensions = unsafe { hal.shared_instance().raw_instance().enumerate_device_extension_properties(hal.raw_physical_device()) }?;
        let has = |name: &std::ffi::CStr| extensions.iter().any(|e| e.extension_name_as_c_str() == Ok(name));
        let extension = if has(ash::khr::global_priority::NAME) {
            ash::khr::global_priority::NAME
        } else if has(ash::ext::global_priority::NAME) {
            ash::ext::global_priority::NAME
        } else {
            anyhow::bail!("{}", tr("the GPU's Vulkan driver has no queue priorities"));
        };
        let mut priority = vk::DeviceQueueGlobalPriorityCreateInfoKHR::default().global_priority(vk::QueueGlobalPriorityKHR::LOW);
        let priority = &mut priority;
        let callback: Box<wgpu::hal::vulkan::CreateDeviceCallback<'_>> = Box::new(move |args| {
            args.extensions.push(extension);
            if let Some(queue) = args.queue_create_infos.first_mut() {
                *queue = queue.push_next(priority);
            }
        });
        // SAFETY: the callback adds a supported extension and a priority every driver accepts at LOW.
        unsafe { hal.open_with_callback(desc.required_features, &desc.required_limits, &desc.memory_hints, Some(callback)) }.map_err(|e| anyhow!("{e}"))?
    };
    // SAFETY: `open` came from this adapter, with the descriptor's features.
    unsafe { adapter.create_device_from_hal(open, desc) }.map_err(|e| anyhow!("{e}"))
}

/// Where a module's WGSL comes from, and the entries of it a device builds (crate::gpu::Gpu::build).
struct KernelSource {
    name: &'static str,
    code: String,
    entries: &'static [&'static str],
}

/// Every kernel module: the renderer's, and with `sums` a training device's (its backward built for `sum_lanes`,
/// see [`DeviceOptions::sum_lanes`]).
fn kernel_sources(sums: Option<GradientSums>, sum_lanes: u32) -> Vec<KernelSource> {
    let common = include_str!("shaders/common.wgsl");
    let raster = include_str!("shaders/raster.wgsl");
    // Depth steps' kernels on 8×8 tiles (shaders/common.wgsl's TILE).
    let common8 = common.replace("const TILE: u32 = 16u;", "const TILE: u32 = 8u;");
    let source = |name: &'static str, parts: &[&str], entries: &'static [&'static str]| KernelSource { name, code: parts.join("\n"), entries };
    let mut out = vec![
        source("project", &[common, include_str!("shaders/project.wgsl")], &["main"][..]),
        source("scan", &[include_str!("shaders/scan.wgsl")], &["scan_blocks", "add_sums", "total"]),
        source("duplicate", &[common, include_str!("shaders/duplicate.wgsl")], &["main"]),
        source("sort", &[include_str!("shaders/sort.wgsl")], &["count", "scatter"]),
        source("ranges", &[common, include_str!("shaders/ranges.wgsl")], &["main"]),
        source("raster forward", &[common, raster, include_str!("shaders/raster_forward.wgsl")], &["forward"]),
        source("present", &[common, include_str!("shaders/present.wgsl")], &["main"]),
        source("present alpha", &[common, include_str!("shaders/present_alpha.wgsl")], &["main"]),
        source("sky composite", &[common, include_str!("shaders/sky_lookup.wgsl"), include_str!("shaders/sky_composite.wgsl")], &["composite"]),
    ];
    if let Some(sums) = sums {
        let (add, read) = sums.sources();
        // The 32-lane transposed sums, and how many lanes add the ten sums otherwise (0: the subgroup's).
        let lanes = format!("const SUM10: bool = {};\nconst ADD_LANES: u32 = {sum_lanes}u;\n", sum_lanes == 0);
        let sky_lookup = include_str!("shaders/sky_lookup.wgsl");
        let backward = include_str!("shaders/raster_backward.wgsl");
        out.extend([
            source("raster backward", &[common, raster, add, lanes.as_str(), sky_lookup, backward, include_str!("shaders/raster_backward_blocks.wgsl")], &["backward", "backward_blocks"]),
            source("sky gradient", &[common, add, sky_lookup, include_str!("shaders/sky_gradient.wgsl")], &["gradient"]),
            source("sky adam", &[read, "const GRAD2D: u32 = 4u;\n", include_str!("shaders/sky_adam.wgsl")], &["adam"]),
            source("loss", &[common, include_str!("shaders/loss.wgsl")], &["color", "squared_error"]),
            source("ssim", &[common, include_str!("shaders/ssim.wgsl")], &["ssim_partials", "ssim_gradient"]),
            source("robust", &[common, include_str!("shaders/robust.wgsl")], &["residual", "inliers"]),
            source("depth_loss", &[common, include_str!("shaders/depth_loss.wgsl")], &["main"]),
            source("project_backward", &[common, read, include_str!("shaders/param_grads.wgsl"), include_str!("shaders/project_backward.wgsl")], &["main"]),
            source("project_backward adam", &[common, read, include_str!("shaders/param_grads.wgsl"), include_str!("shaders/project_backward_adam.wgsl")], &["main"]),
            source("relocate", &[include_str!("shaders/relocate.wgsl")], &["classify", "adjust", "copy", "spawn"]),
            source("raster visibility", &[common, raster, include_str!("shaders/raster_visibility.wgsl")], &["visibility", "visibility_over"]),
            source("raster contribution", &[common, raster, include_str!("shaders/raster_contribution.wgsl")], &["contribution"]),
            source("contribution fold", &[include_str!("shaders/contribution_fold.wgsl")], &["main"]),
            source("filter3d", &[include_str!("shaders/filter3d.wgsl")], &["main"]),
            source("seen", &[include_str!("shaders/seen.wgsl")], &["main"]),
            source("sort subgroups", &[include_str!("shaders/sort.wgsl"), include_str!("shaders/sort_subgroups.wgsl")], &["scatter_subgroups"]),
            source("project split", &[common, include_str!("shaders/project_split.wgsl")], &["project_split", "project_big"]),
            source("duplicate split", &[common, include_str!("shaders/duplicate_split.wgsl")], &["duplicate_split", "duplicate_big"]),
            source("project split 8", &[common8.as_str(), include_str!("shaders/project_split.wgsl")], &["project_split", "project_big"]),
            source("duplicate split 8", &[common8.as_str(), include_str!("shaders/duplicate_split.wgsl")], &["duplicate_split", "duplicate_big"]),
            source("raster forward 8", &[common8.as_str(), raster, include_str!("shaders/raster_forward.wgsl")], &["forward"]),
            source("raster backward 8", &[common8.as_str(), raster, add, lanes.as_str(), sky_lookup, backward], &["backward"]),
            source("raster forward nested", &[common, raster, include_str!("shaders/raster_forward_nested.wgsl")], &["forward_nested"]),
            source("raster forward nested 8", &[common8.as_str(), raster, include_str!("shaders/raster_forward_nested.wgsl")], &["forward_nested"]),
        ]);
    }
    out
}

/// On Direct3D 12 the nested forward's modules (shaders/raster_forward_nested.wgsl) hold the flattened forward under
/// its entry name: wgpu compiles HLSL there with FXC, which took 40 s over a nested raster loop (raster_forward.wgsl),
/// and a training device builds every module when it starts. [`Gpu::nested_forward`] then changes nothing on D3D12.
fn flattened_on_dx12(k: KernelSource) -> KernelSource {
    if !k.name.starts_with("raster forward nested") {
        return k;
    }
    let flat = include_str!("shaders/raster_forward.wgsl").replace("fn forward(", "fn forward_nested(");
    KernelSource { code: k.code.replace(include_str!("shaders/raster_forward_nested.wgsl"), &flat), ..k }
}

impl Gpu {
    pub fn new() -> anyhow::Result<Self> {
        Self::with_options(DeviceOptions::default())
    }

    pub fn with_options(options: DeviceOptions) -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::PRIMARY, ..wgpu::InstanceDescriptor::new_without_display_handle() });
        let adapter = if options.low_power {
            block_on(instance.enumerate_adapters(wgpu::Backends::PRIMARY))
                .into_iter()
                .find(|a| a.get_info().device_type == wgpu::DeviceType::IntegratedGpu)
                .with_context(|| tr("this computer has no integrated GPU (or it is turned off in the BIOS): train on the graphics card, or once VRChat closes"))?
        } else {
            block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() }))
                .map_err(|e| anyhow!("{}", tr!("no GPU adapter: {error}", error = e)))?
        };
        let info = adapter.get_info();
        let available = adapter.features();
        if !available.contains(wgpu::Features::SUBGROUP) {
            anyhow::bail!("{}", tr!("{name} has no subgroup operations, which reconstruction needs.", name = info.name));
        }
        let sums = match options.sums {
            Some(s) if !available.contains(s.features()) => anyhow::bail!("{}", tr!("{name} cannot do {feature}", name = info.name, feature = s.name())),
            Some(s) => s,
            None => GradientSums::best(available),
        };
        let profile = options.profile && available.contains(wgpu::Features::TIMESTAMP_QUERY);
        let mut features = wgpu::Features::SUBGROUP | sums.features();
        if profile {
            features |= wgpu::Features::TIMESTAMP_QUERY;
        }
        let desc = wgpu::DeviceDescriptor { label: Some("orbiters-reconstruct"), required_features: features, required_limits: adapter.limits(), ..Default::default() };
        // Metal has no queue priorities and no game shares a Mac's GPU, so there is nothing to say about it.
        let low = if options.low_priority && !cfg!(target_vendor = "apple") {
            // SAFETY: the callback only adds a supported extension and a queue priority (low_priority_device).
            match unsafe { low_priority_device(&adapter, &desc) } {
                Ok(d) => Some(d),
                Err(e) => {
                    eprintln!("training at normal GPU priority: {e:#}");
                    None
                }
            }
        } else {
            None
        };
        let low_priority = low.is_some();
        let (device, queue) = match low {
            Some(d) => d,
            None => block_on(adapter.request_device(&desc)).map_err(|e| anyhow!("{}", tr!("GPU device: {error}", error = e)))?,
        };
        let errors = Arc::new(std::sync::Mutex::new(None::<String>));
        {
            let errors = errors.clone();
            device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
                errors.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(|| e.to_string());
            }));
        }
        {
            // A driver reset or a GPU timeout while VRChat shares the GPU: the run ends with it.
            let errors = errors.clone();
            device.set_device_lost_callback(move |reason, message| {
                if reason != wgpu::DeviceLostReason::Destroyed {
                    errors.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(|| format!("the GPU device was lost ({message})"));
                }
            });
        }
        let name = format!("{} ({:?}, {}{})", info.name, info.backend, sums.name(), if low_priority { ", low priority" } else { "" });
        let mut gpu = Self::build(device, queue, name, Some(sums), options.sum_lanes, info.backend == wgpu::Backend::Dx12);
        gpu.errors = errors;
        gpu.integrated = info.device_type == wgpu::DeviceType::IntegratedGpu;
        gpu.wgpu_adapter = Some(adapter);
        if profile {
            gpu.profiler = Some(Profiler::new(&gpu.device, &gpu.queue));
        }
        Ok(gpu)
    }

    /// A renderer on someone else's device (a viewer's window): projection, sort and the forward pass only, so the
    /// device needs no features. [`Gpu::step`] takes only [`Target::None`] there.
    pub fn renderer(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let mut gpu = Self::build(device, queue, String::new(), None, 0, false);
        gpu.guards = true;
        gpu
    }

    /// Memory this device can still use, bytes, on a training device: what Metal recommends it keeps to, less what it
    /// has allocated (this process's, Metal does not tell another's), and never more than the system has left, since
    /// the GPU and every program share the same RAM. Metal's own figure does not follow how full the RAM is: 12.1 GB on
    /// a 16 GB M3, whatever the others use.
    #[cfg(target_vendor = "apple")]
    pub fn free_memory(&self) -> Option<u64> {
        use objc2_metal::MTLDevice as _;
        self.wgpu_adapter.as_ref()?;
        // SAFETY: read-only queries of the device.
        let left = unsafe {
            let hal = self.device.as_hal::<wgpu::hal::api::Metal>()?;
            let raw = hal.raw_device();
            raw.recommendedMaxWorkingSetSize().saturating_sub(raw.currentAllocatedSize() as u64)
        };
        Some(crate::apple::available_memory().map_or(left, |system| system.min(left)))
    }

    /// Graphics memory this device's adapter has free now, bytes (`VK_EXT_memory_budget`: the device-local heaps'
    /// budget less their usage, which counts every process's): None on other APIs or without the extension.
    #[cfg(not(target_vendor = "apple"))]
    pub fn free_memory(&self) -> Option<u64> {
        let adapter = self.wgpu_adapter.as_ref()?;
        // SAFETY: read-only queries of the adapter's physical device.
        unsafe {
            let hal = adapter.as_hal::<wgpu::hal::api::Vulkan>()?;
            let instance = hal.shared_instance().raw_instance();
            let physical = hal.raw_physical_device();
            let extensions = instance.enumerate_device_extension_properties(physical).ok()?;
            if !extensions.iter().any(|e| e.extension_name_as_c_str() == Ok(ash::ext::memory_budget::NAME)) {
                return None;
            }
            let mut budget = ash::vk::PhysicalDeviceMemoryBudgetPropertiesEXT::default();
            let heaps = {
                let mut properties = ash::vk::PhysicalDeviceMemoryProperties2::default().push_next(&mut budget);
                instance.get_physical_device_memory_properties2(physical, &mut properties);
                properties.memory_properties
            };
            heaps.memory_heaps_as_slice().iter().enumerate().filter(|(_, h)| h.flags.contains(ash::vk::MemoryHeapFlags::DEVICE_LOCAL)).map(|(i, _)| budget.heap_budget[i].saturating_sub(budget.heap_usage[i])).max()
        }
    }

    /// Graphics memory a training run of `budget` Gaussians takes about, bytes: its per-Gaussian buffers (parameters,
    /// gradients, Adam moments, projections, sums, statistics) with half again for sort entries, and 256 MB of images
    /// and sort histograms.
    pub fn training_memory(&self, budget: usize) -> u64 {
        let sums = self.k.training.as_ref().map_or(4, |t| t.sums.slot_bytes()) as usize;
        let per = 4 * PACKED * 4 + 16 * 4 + 8 + GRAD2D * sums + 16 + 4 + 32;
        (budget * per * 3 / 2) as u64 + (256 << 20)
    }

    /// The gradient sums of a training device.
    pub fn sums(&self) -> Option<GradientSums> {
        self.k.training.as_ref().map(|t| t.sums)
    }

    /// Whether a viewer's last frames should be drawn again: steps after the first at an image size do not wait for
    /// their tile count while the sort has half again the room the last count needed, and a frame whose count turned
    /// out larger left entries out (the next step waits and draws it whole). Asks the device without waiting.
    pub fn needs_redraw(&mut self) -> bool {
        let _ = self.device.poll(wgpu::PollType::Poll);
        for slot in 0..RING {
            self.take_readback(slot);
        }
        self.redraw
    }

    fn build(device: wgpu::Device, queue: wgpu::Queue, adapter: String, sums: Option<GradientSums>, sum_lanes: u32, dx12: bool) -> Self {
        // Every kernel writes its workgroup memory before reading it: without wgpu's zero-fill (one thread clearing
        // whole arrays), D3D's FXC compiles the raster kernel in moments instead of 40 s.
        let options = wgpu::PipelineCompilationOptions { zero_initialize_workgroup_memory: false, ..Default::default() };
        let mut pipes: HashMap<(&'static str, &'static str), wgpu::ComputePipeline> = HashMap::new();
        let compile = |k: KernelSource| -> Vec<((&'static str, &'static str), wgpu::ComputePipeline)> {
            // Without wgpu's injected bounds, loop and division checks the passes run 5–25 % faster (backward, sort and
            // duplicate least, projection most). Every kernel bounds its own indices (the duplicate pass caps its
            // writes at the keys' length), and every loop ends.
            let desc = wgpu::ShaderModuleDescriptor { label: Some(k.name), source: wgpu::ShaderSource::Wgsl(k.code.into()) };
            // SAFETY: as above, no kernel reads or writes outside its buffers or loops forever.
            let module = unsafe { device.create_shader_module_trusted(desc, wgpu::ShaderRuntimeChecks::unchecked()) };
            k.entries
                .iter()
                .map(|&entry| {
                    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                        label: Some(entry),
                        layout: None,
                        module: &module,
                        entry_point: Some(entry),
                        compilation_options: options.clone(),
                        cache: None,
                    });
                    ((k.name, entry), pipeline)
                })
                .collect()
        };
        // The modules compile on every core (the driver turns each pipeline into machine code on its own): a training
        // device's 39 pipelines took 0.8 s on one.
        {
            use rayon::prelude::*;
            let built: Vec<_> = kernel_sources(sums, sum_lanes).into_iter().map(|k| if dx12 { flattened_on_dx12(k) } else { k }).collect::<Vec<_>>().into_par_iter().map(compile).collect();
            pipes.extend(built.into_iter().flatten());
        }
        let mut pipe = |module: &'static str, entry: &'static str| pipes.remove(&(module, entry)).expect("a kernel of kernel_sources");
        let training = sums.map(|sums| Training {
            sums,
            backward: pipe("raster backward", "backward"),
            backward_blocks: pipe("raster backward", "backward_blocks"),
            loss_color: pipe("loss", "color"),
            ssim_partials: pipe("ssim", "ssim_partials"),
            ssim_gradient: pipe("ssim", "ssim_gradient"),
            robust_residual: pipe("robust", "residual"),
            robust_inliers: pipe("robust", "inliers"),
            depth_loss: pipe("depth_loss", "main"),
            project_backward: pipe("project_backward", "main"),
            project_backward_adam: pipe("project_backward adam", "main"),
            relocate_classify: pipe("relocate", "classify"),
            relocate_adjust: pipe("relocate", "adjust"),
            relocate_copy: pipe("relocate", "copy"),
            relocate_spawn: pipe("relocate", "spawn"),
            squared_error: pipe("loss", "squared_error"),
            visibility: pipe("raster visibility", "visibility"),
            visibility_over: pipe("raster visibility", "visibility_over"),
            contribution: pipe("raster contribution", "contribution"),
            contribution_fold: pipe("contribution fold", "main"),
            filter_3d: pipe("filter3d", "main"),
            seen: pipe("seen", "main"),
            sky_gradient: pipe("sky gradient", "gradient"),
            sky_adam: pipe("sky adam", "adam"),
            sort_scatter: pipe("sort subgroups", "scatter_subgroups"),
            project: pipe("project split", "project_split"),
            project_big: pipe("project split", "project_big"),
            duplicate: pipe("duplicate split", "duplicate_split"),
            duplicate_big: pipe("duplicate split", "duplicate_big"),
            project8: pipe("project split 8", "project_split"),
            project_big8: pipe("project split 8", "project_big"),
            duplicate8: pipe("duplicate split 8", "duplicate_split"),
            duplicate_big8: pipe("duplicate split 8", "duplicate_big"),
            forward8: pipe("raster forward 8", "forward"),
            backward8: pipe("raster backward 8", "backward"),
            forward_nested: pipe("raster forward nested", "forward_nested"),
            forward_nested8: pipe("raster forward nested 8", "forward_nested"),
        });
        let k = Kernels {
            project: pipe("project", "main"),
            scan_blocks: pipe("scan", "scan_blocks"),
            add_sums: pipe("scan", "add_sums"),
            total: pipe("scan", "total"),
            duplicate: pipe("duplicate", "main"),
            sort_count: pipe("sort", "count"),
            sort_scatter: pipe("sort", "scatter"),
            ranges: pipe("ranges", "main"),
            forward: pipe("raster forward", "forward"),
            present: pipe("present", "main"),
            present_alpha: pipe("present alpha", "main"),
            sky_composite: pipe("sky composite", "composite"),
            training,
        };
        let no_sky = storage(&device, "no sky", 16);
        let frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame"),
            size: std::mem::size_of::<Frame>() as u64,
            // COPY_SRC: Gpu::lab_raster_dump reads it back.
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let total_staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("total staging"),
            size: 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let total_out = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("total"),
            size: 32,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::INDIRECT,
            mapped_at_creation: false,
        });
        let probe_bytes = (crate::dof::BINS * 4) as u64;
        let probe_bins = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("focus probe"),
            size: probe_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let probes = (0..2)
            .map(|_| Probe {
                buffer: device.create_buffer(&wgpu::BufferDescriptor { label: Some("focus probe readback"), size: probe_bytes, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
                state: Default::default(),
                seq: 0,
            })
            .collect();
        let ring = (0..RING)
            .map(|_| Readback {
                buffer: device.create_buffer(&wgpu::BufferDescriptor { label: Some("total readback"), size: 16, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
                state: Default::default(),
                submission: None,
                size: (0, 0),
                cap: 0,
            })
            .collect();
        Self {
            adapter,
            device,
            queue,
            k,
            splats: None,
            image: None,
            image_cache: Vec::new(),
            sort: None,
            samples: None,
            scan_levels: Vec::new(),
            frame,
            total_staging,
            total_out,
            ring,
            ring_next: 0,
            estimates: Vec::new(),
            count: 0,
            step: 0,
            opacity_reg: 0.0,
            scale_reg: 0.0,
            regularise_unseen: false,
            adam_restart: false,
            birth_stats: 0,
            noise: 0.0,
            noise_gate: false,
            noise_visible: false,
            guards: false,
            confidence: Confidence::default(),
            far: FarField::default(),
            dof: DepthOfField::default(),
            probe: None,
            probes,
            probe_bins,
            probe_seq: 0,
            probe_taken: 0,
            sky: None,
            sky_lr: 0.01,
            no_sky,
            filter_2d: [cpu::FILTER_2D as f32; 2],
            overflows: 0,
            redraw: false,
            profiler: None,
            governor: None,
            paced: [std::time::Duration::from_millis(2); 3],
            wgpu_adapter: None,
            integrated: false,
            errors: Default::default(),
            visibility_over: 0.0,
            visibility_pipelined: false,
            visibility_sum: false,
            clamp_colors: false,
            keyframes: Vec::new(),
            resident_target: None,
            big: None,
            tile8_step: false,
            tile8_all: false,
            nested_forward: false,
            reference_backward: false,
            masks: Vec::new(),
            reserved: 0,
            uniforms: Default::default(),
            binds: Default::default(),
            pause: None,
        }
    }

    fn training(&self) -> anyhow::Result<&Training> {
        self.k.training.as_ref().with_context(|| tr("this GPU only renders"))
    }

    /// The GPU error a training device reported, if any (it stays: nothing after it can be trusted).
    fn check(&self) -> anyhow::Result<()> {
        match self.errors.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            Some(e) => bail!("{}", tr!("GPU error: {error}", error = e)),
            None => Ok(()),
        }
    }

    /// Runs `f` with the device's out-of-memory and validation errors caught in error scopes and returned, instead of
    /// reaching the device's handler (on a viewer's device, wgpu's default one panics).
    fn caught<T>(&mut self, f: impl FnOnce(&mut Self) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let memory = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let invalid = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let out = f(self);
        // Popped in reverse order, as scopes must be.
        let invalid = block_on(invalid.pop());
        let memory = block_on(memory.pop());
        match invalid.or(memory) {
            Some(e) => Err(anyhow!("{}", tr!("GPU error: {error}", error = e))),
            None => out,
        }
    }

    fn bind(&self, pipeline: &wgpu::ComputePipeline, entries: &[(u32, &wgpu::Buffer)]) -> wgpu::BindGroup {
        let layout = pipeline.get_bind_group_layout(0);
        let entries: Vec<wgpu::BindGroupEntry> = entries.iter().map(|(b, buf)| wgpu::BindGroupEntry { binding: *b, resource: buf.as_entire_binding() }).collect();
        self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &entries })
    }

    /// [`Gpu::bind`] kept for the next steps (only for buffers that live across steps).
    fn bind_cached(&self, pipeline: &wgpu::ComputePipeline, entries: &[(u32, &wgpu::Buffer)]) -> wgpu::BindGroup {
        let key = (pipeline.clone(), entries.iter().map(|(b, buf)| (*b, (*buf).clone())).collect::<Vec<_>>());
        if let Some(b) = self.binds.borrow().get(&key) {
            return b.clone();
        }
        let b = self.bind(pipeline, entries);
        self.binds.borrow_mut().insert(key, b.clone());
        b
    }

    /// Drops the kept bind groups (and with them the buffers they hold): some buffer was just replaced.
    fn forget_binds(&self) {
        self.binds.borrow_mut().clear();
    }

    fn uniform<T: Pod>(&self, v: &T) -> wgpu::Buffer {
        self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::bytes_of(v), usage: wgpu::BufferUsages::UNIFORM })
    }

    /// A step's parameter block in the buffer kept for `key` (one per use within a submission: a buffer written twice
    /// before a submit holds only the second value).
    fn uniform_at<T: Pod>(&self, key: (&'static str, usize), v: &T) -> wgpu::Buffer {
        let bytes = bytemuck::bytes_of(v);
        let size = (bytes.len() as u64).next_multiple_of(16);
        let kept = self.uniforms.borrow().get(&key).filter(|b| b.size() == size).cloned();
        let buffer = match kept {
            Some(b) => b,
            None => {
                let b = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some(key.0), size, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
                if self.uniforms.borrow_mut().insert(key, b.clone()).is_some() {
                    self.forget_binds();
                }
                b
            }
        };
        self.queue.write_buffer(&buffer, 0, bytes);
        buffer
    }

    /// Names the passes dispatched from now on (profiling).
    fn section(&self, label: &'static str) {
        if let Some(p) = &self.profiler {
            p.section.set(label);
        }
    }

    fn pass<'e>(&self, enc: &'e mut wgpu::CommandEncoder) -> wgpu::ComputePass<'e> {
        let writes = self.profiler.as_ref().and_then(|p| p.next().map(|i| (p, i)));
        enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: None,
            timestamp_writes: writes.map(|(p, i)| wgpu::ComputePassTimestampWrites { query_set: &p.set, beginning_of_pass_write_index: Some(i), end_of_pass_write_index: Some(i + 1) }),
        })
    }

    fn dispatch(&self, enc: &mut wgpu::CommandEncoder, pipeline: &wgpu::ComputePipeline, bind: &wgpu::BindGroup, (x, y): (u32, u32)) {
        let mut pass = self.pass(enc);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.dispatch_workgroups(x, y, 1);
    }

    /// A dispatch over the workgroups the `total` kernel wrote (256-wide, over the clamped count).
    fn dispatch_counted(&self, enc: &mut wgpu::CommandEncoder, pipeline: &wgpu::ComputePipeline, bind: &wgpu::BindGroup) {
        let mut pass = self.pass(enc);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.dispatch_workgroups_indirect(&self.total_out, 8);
    }

    /// Whether a pause was asked for, as the failure the rounds of renders stop with.
    pub fn stop_if_paused(&self) -> anyhow::Result<()> {
        if self.pause.as_ref().is_some_and(|p| p.asked()) {
            return Err(crate::checkpoint::Interrupted.into());
        }
        Ok(())
    }

    /// Everything the steps carry from one to the next on the device, once all of them have run: the parameters as they
    /// are packed there, each Gaussian's own restart slot too (crate::gaussians::RESTART_SLOT, which an upload zeroes),
    /// the Adam moments and the statistics since the last relocation round. The gradient sums are not part of it: the
    /// backward pass clears them with each step.
    pub fn training_state(&self) -> anyhow::Result<crate::checkpoint::TrainingBuffers> {
        let s = self.splats.as_ref().with_context(|| tr("nothing uploaded"))?;
        let n = self.count;
        Ok(crate::checkpoint::TrainingBuffers {
            params: self.read(&s.params, n * PACKED)?,
            m1: self.read(&s.m1, n * PACKED)?,
            m2: self.read(&s.m2, n * PACKED)?,
            stats: self.read(&s.stats, n * 8)?,
        })
    }

    /// Puts back what [`Gpu::training_state`] read: the next step goes on as if the device had never gone.
    pub fn restore_training(&mut self, state: &crate::checkpoint::TrainingBuffers) {
        self.wait_readbacks();
        self.estimates.clear();
        if let Err(e) = self.caught(|gpu| {
            let n = state.params.len() / PACKED;
            if gpu.splats.as_ref().is_none_or(|s| s.cap < n) {
                gpu.splats = Some(gpu.splat_buffers(n));
                gpu.forget_binds();
            }
            let s = gpu.splats.as_ref().expect("allocated");
            gpu.queue.write_buffer(&s.params, 0, bytemuck::cast_slice(&state.params));
            gpu.queue.write_buffer(&s.m1, 0, bytemuck::cast_slice(&state.m1));
            gpu.queue.write_buffer(&s.m2, 0, bytemuck::cast_slice(&state.m2));
            gpu.queue.write_buffer(&s.stats, 0, bytemuck::cast_slice(&state.stats));
            let unknown = Seen::UNKNOWN;
            let seen = vec![[unknown.weight, unknown.views as f32, unknown.sectors as f32, unknown.rate, unknown.beyond, 0.0, 0.0, 0.0]; n];
            gpu.queue.write_buffer(&s.seen, 0, bytemuck::cast_slice(&seen));
            gpu.queue.write_buffer(&s.marks, 0, &vec![0u8; n.div_ceil(4) * 4]);
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            enc.clear_buffer(&s.grad2d, 0, None);
            enc.clear_buffer(&s.filter, 0, None);
            gpu.queue.submit([enc.finish()]);
            gpu.count = n;
            Ok(())
        }) {
            self.errors.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(|| format!("{e:#}"));
        }
    }

    /// The sky map's texels and Adam moments, or `None` without a map trained on this device.
    pub fn sky_learning(&self) -> anyhow::Result<Option<crate::checkpoint::SkyLearningState>> {
        let Some((s, l)) = self.sky.as_ref().and_then(|s| Some((s, s.learning.as_ref()?))) else { return Ok(None) };
        let n = s.width as usize * (s.width as usize / 2);
        Ok(Some(crate::checkpoint::SkyLearningState { width: s.width, texels: self.read(&s.texels, n)?, m1: self.read(&l.m1, n)?, m2: self.read(&l.m2, n)? }))
    }

    /// Puts back what [`Gpu::sky_learning`] read.
    pub fn restore_sky_learning(&mut self, state: Option<&crate::checkpoint::SkyLearningState>) -> anyhow::Result<()> {
        let Some(state) = state else { return Ok(()) };
        let rgb: Vec<[f32; 3]> = state.texels.iter().map(|t| [t[0], t[1], t[2]]).collect();
        self.set_sky(Some((state.width, &rgb)))?;
        let s = self.sky.as_ref().expect("just set");
        let l = s.learning.as_ref().with_context(|| tr("a sky map's learning on a device that does not train"))?;
        self.queue.write_buffer(&s.texels, 0, bytemuck::cast_slice(&state.texels));
        self.queue.write_buffer(&l.m1, 0, bytemuck::cast_slice(&state.m1));
        self.queue.write_buffer(&l.m2, 0, bytemuck::cast_slice(&state.m2));
        Ok(())
    }

    /// Uploads a new set of Gaussians (Adam moments and statistics start at zero).
    pub fn upload(&mut self, g: &Gaussians) {
        // Counts of the previous set say nothing about this one.
        self.wait_readbacks();
        self.estimates.clear();
        self.upload_with_moments(g, None);
    }

    /// A viewer's marks of the Gaussians uploaded, one byte each in their order (crate::edit::Edit::view_marks): 0 as
    /// trained, 1 selected (tinted, project.wgsl's SELECTED), 2 or more left out (hidden or deleted). An upload clears
    /// them. A quarter of the bytes of one float per Gaussian: 0.75 MB for three million, written again whole with
    /// each change of a selection (a brush stroke's), which costs a frame nothing.
    pub fn set_marks(&mut self, marks: &[u8]) {
        let Some(s) = self.splats.as_ref() else { return };
        let n = self.count.min(marks.len());
        let mut words = vec![0u8; self.count.div_ceil(4) * 4];
        words[..n].copy_from_slice(&marks[..n]);
        self.queue.write_buffer(&s.marks, 0, &words);
    }

    /// Uploads Gaussians with their Adam moments (packed like the parameters): the same set after relocation. A set
    /// the device cannot hold fails the next step (and readback) instead of panicking here.
    pub fn upload_with_moments(&mut self, g: &Gaussians, moments: Option<(&[f32], &[f32])>) {
        let training = self.k.training.is_some();
        if !training {
            // A renderer's only errors are its own uploads': this one starts over.
            *self.errors.lock().unwrap_or_else(|p| p.into_inner()) = None;
        }
        if let Err(e) = self.caught(|gpu| {
            gpu.upload_inner(g, moments);
            Ok(())
        }) {
            self.errors.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(|| format!("{e:#}"));
        }
    }

    fn upload_inner(&mut self, g: &Gaussians, moments: Option<(&[f32], &[f32])>) {
        let n = g.len();
        let training = self.k.training.is_some();
        let fresh = self.splats.as_ref().is_none_or(|s| s.cap < n);
        if fresh {
            self.splats = Some(self.splat_buffers(n));
            self.forget_binds();
        }
        let s = self.splats.as_ref().expect("allocated");
        self.queue.write_buffer(&s.params, 0, bytemuck::cast_slice(&g.pack()));
        let filter: Vec<f32> = (0..n).map(|i| g.filter(i)).collect();
        self.queue.write_buffer(&s.filter, 0, bytemuck::cast_slice(&filter));
        let seen: Vec<[f32; 8]> = (0..n).map(|i| g.seen(i)).map(|s| [s.weight, s.views as f32, s.sectors as f32, s.rate, s.beyond, 0.0, 0.0, 0.0]).collect();
        self.queue.write_buffer(&s.seen, 0, bytemuck::cast_slice(&seen));
        // A new set: nothing marked.
        self.queue.write_buffer(&s.marks, 0, &vec![0u8; n.div_ceil(4) * 4]);
        if training {
            let mut enc = self.device.create_command_encoder(&Default::default());
            match moments {
                Some((m1, m2)) => {
                    self.queue.write_buffer(&s.m1, 0, bytemuck::cast_slice(m1));
                    self.queue.write_buffer(&s.m2, 0, bytemuck::cast_slice(m2));
                }
                None => {
                    enc.clear_buffer(&s.m1, 0, None);
                    enc.clear_buffer(&s.m2, 0, None);
                }
            }
            enc.clear_buffer(&s.stats, 0, None);
            if !fresh {
                // The backward's sums start at zero (project_backward clears what it reads after each step).
                enc.clear_buffer(&s.grad2d, 0, None);
            }
            self.queue.submit([enc.finish()]);
        }
        self.count = n;
    }

    fn read<T: Pod>(&self, buffer: &wgpu::Buffer, count: usize) -> anyhow::Result<Vec<T>> {
        let bytes = (count * std::mem::size_of::<T>()) as u64;
        if bytes == 0 {
            return Ok(Vec::new());
        }
        self.check()?;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: bytes.next_multiple_of(4),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(buffer, 0, &staging, 0, bytes.next_multiple_of(4));
        self.queue.submit([enc.finish()]);
        // 0 waiting, 1 mapped, 2 failed.
        let state = Arc::new(AtomicU32::new(0));
        {
            let state = state.clone();
            staging.map_async(wgpu::MapMode::Read, .., move |r| state.store(if r.is_ok() { 1 } else { 2 }, Ordering::SeqCst));
        }
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow!("{}", tr!("waiting for the GPU: {error}", error = e)))?;
        // An error since (out of memory, a lost device) means what came back is not the result.
        self.check()?;
        anyhow::ensure!(state.load(Ordering::SeqCst) == 1, "{}", tr("the GPU did not hand its results back (was the device lost?)"));
        let view = staging.get_mapped_range(..).map_err(|e| anyhow!("{}", tr!("reading back from the GPU: {error}", error = format!("{e:?}"))))?;
        let out = bytemuck::cast_slice::<u8, T>(&view[..bytes as usize]).to_vec();
        drop(view);
        staging.unmap();
        Ok(out)
    }

    /// The Gaussians, their Adam moments and densification statistics (packed).
    pub fn download(&self) -> anyhow::Result<Download> {
        let s = self.splats.as_ref().with_context(|| tr("nothing uploaded"))?;
        let n = self.count;
        let m1: Vec<f32> = self.read(&s.m1, n * PACKED)?;
        let m2: Vec<f32> = self.read(&s.m2, n * PACKED)?;
        let stats: Vec<[f32; 8]> = self.read::<f32>(&s.stats, n * 8)?.as_chunks::<8>().0.to_vec();
        Ok((self.download_params()?, m1, m2, stats))
    }

    /// The Gaussians alone, with their 3D filters (a finished run's result: its Adam moments and statistics stay on the
    /// device).
    pub fn download_params(&self) -> anyhow::Result<Gaussians> {
        let s = self.splats.as_ref().with_context(|| tr("nothing uploaded"))?;
        let mut g = Gaussians::unpack(&self.read::<f32>(&s.params, self.count * PACKED)?);
        g.filter_3d = self.read(&s.filter, self.count)?;
        Ok(g)
    }

    /// Sets every Gaussian's 3D filter (cpu::filter_3d) from the cameras that train it: `scale` / ν̂², with ν̂ the
    /// highest focal / depth among those that see its centre; 0 for one none sees, and for every one with `scale` 0.
    pub fn update_filter_3d(&mut self, cams: &[Camera], scale: f32) -> anyhow::Result<()> {
        self.check()?;
        let t = self.training()?;
        let s = self.splats.as_ref().with_context(|| tr("no Gaussians"))?;
        let n = self.count;
        let mut enc = self.device.create_command_encoder(&Default::default());
        if scale <= 0.0 || cams.is_empty() {
            enc.clear_buffer(&s.filter, 0, None);
        } else if n > 0 {
            let views: Vec<[f32; 20]> = cams
                .iter()
                .map(|c| {
                    let mut v = [0f32; 20];
                    for r in 0..3 {
                        v[r * 4..r * 4 + 3].copy_from_slice(&c.r[r]);
                        v[r * 4 + 3] = c.t[r];
                    }
                    v[12..16].copy_from_slice(&[c.fx, c.fy, c.cx, c.cy]);
                    v[16..18].copy_from_slice(&[c.width as f32, c.height as f32]);
                    v
                })
                .collect();
            let views = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("filter cameras"), contents: bytemuck::cast_slice(&views), usage: wgpu::BufferUsages::STORAGE });
            let info = self.uniform(&[n as u32, cams.len() as u32, scale.to_bits(), (cpu::NEAR as f32).to_bits()]);
            let bind = self.bind(&t.filter_3d, &[(0, &info), (1, &s.params), (2, &views), (3, &s.filter)]);
            self.section("filter_3d");
            self.dispatch(&mut enc, &t.filter_3d, &bind, groups(n));
        }
        self.queue.submit([enc.finish()]);
        Ok(())
    }

    /// Makes the splat buffers of the next uploads hold at least `n` Gaussians: a run that knows its budget never
    /// grows them (growing copies the set, and holds two for a moment).
    pub fn reserve(&mut self, n: usize) {
        self.reserved = n;
    }

    /// Buffers for at least `n` Gaussians (or the room [`Gpu::reserve`] asked for), else half again. A renderer keeps
    /// no room to spare (a viewer loads once) and no training buffers.
    fn splat_buffers(&self, n: usize) -> SplatBuffers {
        let slot_bytes = self.k.training.as_ref().map(|t| t.sums.slot_bytes());
        let cap = match slot_bytes {
            Some(_) if self.reserved >= n => self.reserved.max(1024),
            Some(_) => (n + n / 2).max(1024),
            None => n.max(1),
        };
        let d = &self.device;
        let f = |count: usize| (count * 4) as u64;
        let t = |count: usize| if slot_bytes.is_some() { f(count) } else { 0 };
        SplatBuffers {
            cap,
            params: storage(d, "params", f(cap * PACKED)),
            grads: storage(d, "grads", t(cap * PACKED)),
            m1: storage(d, "moment1", t(cap * PACKED)),
            m2: storage(d, "moment2", t(cap * PACKED)),
            proj: storage(d, "projected", f(cap * 16)),
            counts: storage(d, "tile counts", f(cap)),
            offsets: storage(d, "tile offsets", f(cap)),
            grad2d: storage(d, "grad2d", slot_bytes.unwrap_or(0) * (cap * GRAD2D) as u64),
            stats: storage(d, "stats", t(cap * 8)),
            filter: storage(d, "3D filter", f(cap)),
            seen: storage(d, "seen", f(cap * 8)),
            marks: storage(d, "marks", f(cap.div_ceil(4))),
        }
    }

    /// Relocation on the device, first half (crate::train::plan_relocation decides from what it reads back): per
    /// Gaussian −1 if dead (faded, broken, or wider on screen than `max_radius` of its image's width since the last
    /// round), −2 − its opacity if alive on the sky shell, else its opacity.
    pub fn relocation_weights(&self, max_radius: f32) -> anyhow::Result<Vec<f32>> {
        let t = self.training()?;
        let s = self.splats.as_ref().with_context(|| tr("no Gaussians"))?;
        let n = self.count;
        let weights = storage(&self.device, "relocation weights", (n * 4) as u64);
        let info = self.uniform(&[n as u32, 0, max_radius.to_bits(), 0]);
        let mut enc = self.device.create_command_encoder(&Default::default());
        let bind = self.bind(&t.relocate_classify, &[(0, &info), (1, &s.params), (2, &s.stats), (3, &weights)]);
        self.dispatch(&mut enc, &t.relocate_classify, &bind, groups(n));
        self.queue.submit([enc.finish()]);
        self.read(&weights, n)
    }

    /// [`Gpu::relocation_weights`] without waiting: the classification is queued behind the steps, and its result comes
    /// back through [`PendingWeights::take`] once the GPU gets there (training plans the round meanwhile, on another
    /// thread, from weights a few steps old). With `scores`, the birth scores follow the weights (relocate.wgsl
    /// `classify`: four floats a Gaussian in all).
    pub fn request_relocation_weights(&self, max_radius: f32, scores: bool) -> anyhow::Result<PendingWeights> {
        let t = self.training()?;
        let s = self.splats.as_ref().with_context(|| tr("no Gaussians"))?;
        let n = self.count;
        let floats = if scores { 4 * n } else { n };
        let weights = storage(&self.device, "relocation weights", (floats * 4) as u64);
        let info = self.uniform(&[n as u32, u32::from(scores), max_radius.to_bits(), 0]);
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("relocation weights readback"), size: (floats * 4).max(4) as u64, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let mut enc = self.device.create_command_encoder(&Default::default());
        let bind = self.bind(&t.relocate_classify, &[(0, &info), (1, &s.params), (2, &s.stats), (3, &weights)]);
        self.dispatch(&mut enc, &t.relocate_classify, &bind, groups(n));
        enc.copy_buffer_to_buffer(&weights, 0, &staging, 0, (floats * 4) as u64);
        self.queue.submit([enc.finish()]);
        let state = Arc::new(AtomicU32::new(0));
        {
            let state = state.clone();
            staging.map_async(wgpu::MapMode::Read, .., move |r| state.store(if r.is_ok() { 1 } else { 2 }, Ordering::SeqCst));
        }
        Ok(PendingWeights { staging, state, n: floats })
    }

    /// Relocation on the device, second half: applies the plan (sources first, then their copies, whose Adam moments
    /// restart at zero like the sources', counting their bias correction from this step with `adam_restart`, else from
    /// the run's first), clears the screen-size statistics and grows the set to `plan.len`.
    pub fn apply_relocation(&mut self, plan: &Relocation) -> anyhow::Result<()> {
        self.check()?;
        let n = self.count;
        if self.splats.as_ref().is_some_and(|s| s.cap < plan.len) {
            // Grow, keeping the parameters and moments of the current set.
            let grown = self.splat_buffers(plan.len);
            let old = self.splats.as_ref().expect("uploaded");
            let mut enc = self.device.create_command_encoder(&Default::default());
            let bytes = (n * PACKED * 4) as u64;
            for (from, to) in [(&old.params, &grown.params), (&old.m1, &grown.m1), (&old.m2, &grown.m2)] {
                enc.copy_buffer_to_buffer(from, 0, to, 0, bytes);
            }
            enc.copy_buffer_to_buffer(&old.filter, 0, &grown.filter, 0, (n * 4) as u64);
            self.queue.submit([enc.finish()]);
            self.splats = Some(grown);
            self.forget_binds();
        }
        let t = self.training()?;
        let s = self.splats.as_ref().with_context(|| tr("no Gaussians"))?;
        let mut enc = self.device.create_command_encoder(&Default::default());
        let passes: [(&wgpu::ComputePipeline, &[u8], usize); 3] = [
            (&t.relocate_adjust, bytemuck::cast_slice(&plan.adjust), plan.adjust.len()),
            (&t.relocate_copy, bytemuck::cast_slice(&plan.copies), plan.copies.len()),
            (&t.relocate_spawn, bytemuck::cast_slice(&plan.spawns), plan.spawns.len()),
        ];
        for (pipeline, contents, count) in passes {
            if count == 0 {
                continue;
            }
            let entries = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("relocation plan"), contents, usage: wgpu::BufferUsages::STORAGE });
            // The step a relocated Gaussian's bias correction counts from (adam.wgsl); 0 is the run's first.
            let restart = if self.adam_restart { self.step as f32 } else { 0.0 };
            let info = self.uniform(&[n as u32, count as u32, 0, restart.to_bits()]);
            let bind = self.bind(pipeline, &[(0, &info), (1, &s.params), (4, &entries), (5, &s.m1), (6, &s.m2)]);
            self.dispatch(&mut enc, pipeline, &bind, groups(count));
        }
        enc.clear_buffer(&s.stats, 0, None);
        self.queue.submit([enc.finish()]);
        self.count = plan.len;
        Ok(())
    }

    /// The last step's parameter gradients (packed), for tests.
    pub fn read_grads(&self) -> anyhow::Result<Vec<f32>> {
        let s = self.splats.as_ref().with_context(|| tr("nothing uploaded"))?;
        self.read(&s.grads, self.count * PACKED)
    }

    fn ensure_image(&mut self, w: u32, h: u32) {
        if self.image.as_ref().is_some_and(|i| i.width == w && i.height == h) {
            return;
        }
        let train = self.k.training.is_some();
        if let Some(old) = self.image.take() {
            // A viewer's sizes follow its window: it keeps none.
            if train {
                self.image_cache.push(old);
                if self.image_cache.len() > KEPT_SIZES {
                    self.image_cache.remove(0);
                    self.forget_binds();
                }
            } else {
                self.forget_binds();
            }
        }
        if let Some(k) = self.image_cache.iter().position(|i| i.width == w && i.height == h) {
            self.image = Some(self.image_cache.remove(k));
            return;
        }
        let d = &self.device;
        let px = (w * h) as u64;
        // Room for 8×8 tiles: depth steps render their sample grid on them.
        let tiles = (w.div_ceil(8) * h.div_ceil(8)) as u64;
        let t = if train { px } else { 0 };
        self.image = Some(ImageBuffers {
            width: w,
            height: h,
            out_color: storage(d, "out colour", px * 16),
            out_state: storage(d, "out state", px * 8),
            d_pixel: storage(d, "d pixel", t * 16),
            target: storage(d, "target", t * 4),
            ranges: storage(d, "tile ranges", tiles * 8),
            partials: storage(d, "ssim partials", t * 48),
            resid: storage(d, "residual", t * 4),
            kept: storage(d, "kept pixels", t * 4),
        });
    }

    /// The loss mask (1 scene, 0 strip and band) for images of this size: colour steps, scores and visibility rounds
    /// at that size need one.
    pub fn set_mask(&mut self, w: u32, h: u32, mask: &[f32]) -> anyhow::Result<()> {
        anyhow::ensure!(mask.len() == (w * h) as usize, "{}", tr!("a mask of {pixels} pixels for {width}×{height} images", pixels = mask.len(), width = w, height = h));
        let buffer = match self.masks.iter().find(|(s, _)| *s == (w, h)) {
            Some((_, b)) => b.clone(),
            None => {
                let b = storage(&self.device, "mask", (w * h) as u64 * 4);
                self.masks.push(((w, h), b.clone()));
                b
            }
        };
        self.queue.write_buffer(&buffer, 0, bytemuck::cast_slice(mask));
        Ok(())
    }

    /// The loss mask set for images of this size.
    fn mask_for(&self, w: u32, h: u32) -> anyhow::Result<wgpu::Buffer> {
        self.masks.iter().find(|(s, _)| *s == (w, h)).map(|(_, b)| b.clone()).with_context(|| tr!("no loss mask was set for {width}×{height} images", width = w, height = h))
    }

    fn ensure_sort(&mut self, n: usize) {
        if self.sort.as_ref().is_some_and(|s| s.cap >= n) {
            return;
        }
        let cap = (n + n / 2).max(1 << 16);
        // Histograms for 4-bit digits over blocks of 256, or 8-bit digits over blocks of 1024 (sort8.wgsl).
        let blocks = cap.div_ceil(256).max(cap.div_ceil(1024) * 16);
        let d = &self.device;
        self.sort = Some(SortBuffers {
            cap,
            keys: [storage(d, "keys a", cap as u64 * 4), storage(d, "keys b", cap as u64 * 4)],
            values: [storage(d, "values a", cap as u64 * 4), storage(d, "values b", cap as u64 * 4)],
            hist: storage(d, "digit histograms", blocks as u64 * 16 * 4),
            hist_offsets: storage(d, "digit offsets", blocks as u64 * 16 * 4),
        });
        self.forget_binds();
    }

    /// Exclusive scan of `n` u32s from `src` into `dst` (levels of 512); `what` names the scan's parameter buffers.
    fn scan(&mut self, enc: &mut wgpu::CommandEncoder, src: &wgpu::Buffer, dst: &wgpu::Buffer, n: usize, level: usize, what: &'static str) {
        let blocks = n.div_ceil(512);
        if self.scan_levels.len() <= level || self.scan_levels[level].0 < blocks {
            let cap = blocks.max(64) * 2;
            let sums = storage(&self.device, "scan sums", cap as u64 * 4);
            let scanned = storage(&self.device, "scan sums scanned", cap as u64 * 4);
            if self.scan_levels.len() <= level {
                self.scan_levels.push((cap, sums, scanned));
            } else {
                self.scan_levels[level] = (cap, sums, scanned);
                self.forget_binds();
            }
        }
        let info = self.uniform_at((what, level), &[n as u32, 0, 0, 0]);
        let (sums, scanned) = (self.scan_levels[level].1.clone(), self.scan_levels[level].2.clone());
        let bind = self.bind_cached(&self.k.scan_blocks, &[(0, &info), (1, src), (2, dst), (3, &sums)]);
        self.dispatch(enc, &self.k.scan_blocks, &bind, groups_of(blocks));
        if blocks > 1 {
            self.scan(enc, &sums, &scanned, blocks, level + 1, what);
            let bind = self.bind_cached(&self.k.add_sums, &[(0, &info), (2, dst), (3, &scanned)]);
            self.dispatch(enc, &self.k.add_sums, &bind, groups(n));
        }
    }

    /// The frame's uniform for a step from `cam` on tiles of `tile` pixels a side.
    fn frame_for(&self, cam: &Camera, background: [f32; 3], loss: [f32; 4], train: bool, tile: u32) -> Frame {
        let tiles_x = cam.width.div_ceil(tile);
        let tiles_y = cam.height.div_ceil(tile);
        let (opacity_reg, scale_reg) = if train { (self.opacity_reg, self.scale_reg) } else { (0.0, 0.0) };
        Frame {
            r0: [cam.r[0][0], cam.r[0][1], cam.r[0][2], cam.t[0]],
            r1: [cam.r[1][0], cam.r[1][1], cam.r[1][2], cam.t[1]],
            r2: [cam.r[2][0], cam.r[2][1], cam.r[2][2], cam.t[2]],
            focal: [cam.fx, cam.fy, cam.cx, cam.cy],
            limits: [
                (cpu::FRUSTUM_CLAMP * (0.5 * cam.width as f64 / cam.fx as f64)) as f32,
                (cpu::FRUSTUM_CLAMP * (0.5 * cam.height as f64 / cam.fy as f64)) as f32,
                cpu::NEAR as f32,
                FAR,
            ],
            background: [background[0], background[1], background[2], 0.0],
            size: [cam.width, cam.height, tiles_x, tiles_y],
            counts: [self.count as u32, u32::from(self.guards && !train), depth_bits_for(cam.width, cam.height, tile), u32::from(train && self.regularise_unseen)],
            loss,
            reg: [opacity_reg, scale_reg, self.filter_2d[0], self.filter_2d[1]],
            confidence: [
                self.confidence.amount.clamp(0.0, 1.0),
                match self.confidence.mode {
                    ConfidenceMode::Fade => 0.0,
                    ConfidenceMode::Blur => 1.0,
                    ConfidenceMode::Haze => 2.0,
                },
                0.0,
                0.0,
            ],
            far: [
                match self.far.mode {
                    FarMode::Shown => 0.0,
                    FarMode::Fog => 1.0,
                    FarMode::Crop => 2.0,
                },
                self.far.distance,
                if train { 0.0 } else { self.visibility_over },
                0.0,
            ],
            horizon: [self.far.horizon[0], self.far.horizon[1], self.far.horizon[2], self.sky.as_ref().map_or(0.0, |s| s.width as f32)],
            // The focus goes along for the focus plane's tint even with no aperture.
            dof: if train || self.dof.focus <= 0.0 {
                [0.0; 4]
            } else {
                [if self.dof.on() { self.dof.aperture } else { 0.0 }, self.dof.focus, self.dof.near.clamp(0.0, 1.0), self.dof.far.clamp(0.0, 1.0)]
            },
            probe: [
                self.probe.map_or(0.0, |p| p[0]),
                self.probe.map_or(0.0, |p| p[1]),
                if self.probe.is_some() && !train { 1.0 } else { 0.0 },
                if self.dof.show_plane && self.dof.focus > 0.0 && !train { 1.0 } else { 0.0 },
            ],
        }
    }

    /// The focus distance (metres along the view) under the [`Gpu::probe`] pixel of the newest frame whose probe came
    /// back since the last call (crate::dof::focus_of), without waiting: `None` while none has.
    pub fn probed(&mut self) -> Option<f32> {
        let _ = self.device.poll(wgpu::PollType::Poll);
        let mut newest: Option<(u64, f32)> = None;
        for p in &mut self.probes {
            match p.state.load(Ordering::SeqCst) {
                2 => {
                    let bins = p.buffer.get_mapped_range(..).ok().map(|view| bytemuck::cast_slice::<u8, u32>(&view).to_vec());
                    p.buffer.unmap();
                    p.state.store(0, Ordering::SeqCst);
                    if let Some(bins) = bins {
                        if p.seq > self.probe_taken && newest.is_none_or(|(s, _)| p.seq > s) {
                            newest = Some((p.seq, crate::dof::focus_of(&bins)));
                        }
                    }
                }
                3 => p.state.store(0, Ordering::SeqCst),
                _ => {}
            }
        }
        let (seq, focus) = newest?;
        self.probe_taken = seq;
        Some(focus)
    }

    /// Whether a probe is on its way back ([`Gpu::probed`] has it after a poll or two).
    pub fn probing(&self) -> bool {
        self.probes.iter().any(|p| p.state.load(Ordering::SeqCst) != 0)
    }

    fn estimate(&self, size: (u32, u32)) -> Option<u32> {
        self.estimates.iter().find(|(s, _)| *s == size).map(|(_, e)| *e)
    }

    /// Folds an intersection count of an image size into its estimate (a maximum that decays by a 32nd a count).
    fn note_total(&mut self, size: (u32, u32), total: u32) {
        match self.estimates.iter_mut().find(|(s, _)| *s == size) {
            Some((_, e)) => *e = total.max(*e - *e / 32),
            None => self.estimates.push((size, total)),
        }
    }

    /// Takes the count of a readback slot that has arrived (frees the slot either way once it is no longer in flight).
    fn take_readback(&mut self, slot: usize) {
        let state = self.ring[slot].state.load(Ordering::SeqCst);
        let r = &self.ring[slot];
        let total = if state == 2 { r.buffer.get_mapped_range(..).ok().map(|view| bytemuck::cast_slice::<u8, u32>(&view)[0]) } else { None };
        if state == 2 {
            r.buffer.unmap();
        }
        if state >= 2 {
            match total {
                Some(total) => {
                    let (size, cap) = (r.size, r.cap);
                    if total > cap {
                        self.overflows += 1;
                        if self.k.training.is_none() {
                            // A viewer's frame missed entries: the next waits for its count, and it should be drawn again.
                            self.redraw = true;
                        }
                    }
                    self.note_total(size, total);
                }
                // A count that never came back: the device failed, and the next step says so.
                None => {
                    self.errors.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(|| "a step's tile count did not come back from the GPU".into());
                }
            }
            self.ring[slot].state.store(0, Ordering::SeqCst);
        }
    }

    /// Waits for a slot's readback if it is in flight, then takes it.
    fn wait_readback(&mut self, slot: usize) {
        if self.ring[slot].state.load(Ordering::SeqCst) == 1 {
            let index = self.ring[slot].submission.clone();
            let start = std::time::Instant::now();
            let _ = self.device.poll(wgpu::PollType::Wait { submission_index: index, timeout: None });
            crate::timing::add("8 steps: waiting for the GPU", start.elapsed());
        }
        self.take_readback(slot);
    }

    fn wait_readbacks(&mut self) {
        for slot in 0..RING {
            self.wait_readback(slot);
        }
    }

    /// Renders (and, with a target, takes one optimisation step with `lr`). Returns the number of tile intersections:
    /// on a training device the latest count read back for this image size (a few steps old, as steps do not wait).
    pub fn step(&mut self, cam: &Camera, background: [f32; 3], target: Target<'_>, lr: Option<LearningRates>) -> anyhow::Result<u32> {
        let start = std::time::Instant::now();
        let out = self.caught(|gpu| gpu.run(cam, background, target, lr, None));
        if lr.is_some() {
            crate::timing::add("8 steps: inside Gpu::step", start.elapsed());
        }
        out
    }

    /// What [`Gpu::step`] does, or with `seen` a visibility round (raster_visibility.wgsl) in place of the forward pass,
    /// sorted over its exact count: a round that dropped entries would leave their Gaussians unseen, and fade them.
    fn run(&mut self, cam: &Camera, background: [f32; 3], target: Target<'_>, lr: Option<LearningRates>, seen: Option<&wgpu::Buffer>) -> anyhow::Result<u32> {
        let n = self.count;
        if n == 0 {
            return Ok(0);
        }
        if !matches!(target, Target::None) || seen.is_some() {
            self.training()?;
        }
        self.check()?;
        anyhow::ensure!(cam.width > 0 && cam.height > 0, "{}", tr!("a view of {width}×{height} pixels", width = cam.width, height = cam.height));
        // A colour target is a keyframe of the camera's size, and like a visibility round it needs the loss mask of
        // that size (a size whose mask was never set would train on nothing).
        let mask = match &target {
            Target::Color { rgba, .. } => {
                anyhow::ensure!(rgba.len() == (cam.width * cam.height) as usize, "{}", tr!("a keyframe of {pixels} pixels for a {width}×{height} camera", pixels = rgba.len(), width = cam.width, height = cam.height));
                Some(self.mask_for(cam.width, cam.height)?)
            }
            _ if seen.is_some() => Some(self.mask_for(cam.width, cam.height)?),
            _ => None,
        };
        self.ensure_image(cam.width, cam.height);
        let size = (cam.width, cam.height);
        let (loss, ssim, robust) = match &target {
            // loss.y holds the robust threshold for colour steps, loss.z the D-SSIM weight.
            Target::Color { scene_pixels, weight, ssim, robust, .. } => ([*weight, *robust, *ssim, *scene_pixels], *ssim, *robust),
            // Each sample by its weight (at least 1: crate::train::depth_target), over the view's total.
            Target::Depth { samples, weight } => ([0.0, *weight, 0.0, samples.iter().filter(|s| s[2] > 0.0).map(|s| s[3].max(1.0)).sum::<f32>()], 0.0, 0.0),
            Target::None => ([0.0; 4], 0.0, 0.0),
        };
        let train = !matches!(target, Target::None);
        // Each sort copy (sized half again over the entries) must fit one storage binding.
        let max_entries = (self.device.limits().max_storage_buffer_binding_size / 4 * 2 / 3).min(u32::MAX as u64) as u32;
        // A training device renders depth steps' sample grids on 8×8 tiles (shaders/common.wgsl's TILE): the grid's
        // 66 tiles of 16×16 left the GPU half idle and every pixel thousands of entries to walk.
        let tile8 = self.k.training.is_some() && (matches!(target, Target::Depth { .. }) || self.tile8_all);
        let tile = if tile8 { 8 } else { cpu::TILE };
        self.tile8_step = tile8;
        let mut frame = self.frame_for(cam, background, loss, train, tile);
        // The birth statistics this step records (shaders/common.wgsl stats_mode), training steps only.
        frame.confidence[2] = match &target {
            Target::Color { .. } if self.birth_stats & BIRTH_STATS_COLOR != 0 => 1.0,
            Target::Depth { .. } if self.birth_stats & BIRTH_STATS_DEPTH != 0 => 2.0,
            _ => 0.0,
        };
        self.queue.write_buffer(&self.frame, 0, bytemuck::bytes_of(&frame));
        let (params, proj, counts, offsets, filter, seen_buf, marks) = {
            let s = self.splats.as_ref().with_context(|| tr("no Gaussians"))?;
            (s.params.clone(), s.proj.clone(), s.counts.clone(), s.offsets.clone(), s.filter.clone(), s.seen.clone(), s.marks.clone())
        };
        // A training device sizes the sort from the counts read back for this image size, without waiting: entries
        // past the end are dropped for this step and counted as an overflow when its count arrives, and the buffers
        // grow for the next steps. A viewer goes without waiting only while its sort holds half again the count it last
        // saw at this size (its camera moves a little a frame; waiting, it sizes the sort to twice the count); a frame
        // that overflowed anyway makes the next one wait, and [`Gpu::needs_redraw`] says to draw it. The first step at
        // a size waits, and so do visibility rounds unless [`Gpu::visibility`] lets them go (it checks for dropped
        // entries after the round, and waits the round again if any were).
        let training = self.k.training.is_some();
        let _ = self.device.poll(wgpu::PollType::Poll);
        for slot in 0..RING {
            self.take_readback(slot);
        }
        let roomy = |gpu: &Self, est: u32| !gpu.redraw && gpu.sort.as_ref().is_some_and(|s| s.cap as u64 >= est as u64 + est as u64 / 2);
        let pipelined = match (seen, self.estimate(size)) {
            (None, Some(est)) if training || roomy(self, est) => Some(est),
            (Some(_), Some(est)) if training && self.visibility_pipelined => Some(est),
            _ => None,
        };
        let cap = match pipelined {
            Some(est) if training => {
                let est = est as usize;
                self.ensure_sort((est + est / 4 + 4096).min(max_entries as usize));
                self.sort.as_ref().expect("sort").cap.min(max_entries as usize) as u32
            }
            Some(_) => self.sort.as_ref().expect("sort").cap.min(max_entries as usize) as u32,
            None => 0,
        };
        self.pace_wait(0);
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.section("project");
        // A training device's own projection has no viewer's guards, confidence or far field: with them (a viewer on
        // a training device) it renders as viewers do.
        let split = !self.guards && self.k.training.is_some();
        match self.k.training.as_ref().filter(|_| split) {
            Some(t) => {
                // Large Gaussians' tiles are counted by whole workgroups (shaders/project_split.wgsl).
                let (split, big_pass) = if tile8 { (t.project8.clone(), t.project_big8.clone()) } else { (t.project.clone(), t.project_big.clone()) };
                if self.big.is_none() {
                    self.big = Some(storage(&self.device, "large Gaussians", 65536 * 4));
                }
                let big = self.big.clone().expect("made above");
                enc.clear_buffer(&big, 0, Some(4));
                let bind = self.bind_cached(&split, &[(0, &self.frame), (1, &params), (2, &proj), (3, &counts), (4, &filter), (6, &big)]);
                self.dispatch(&mut enc, &split, &bind, groups(n));
                let bind = self.bind_cached(&big_pass, &[(2, &proj), (3, &counts), (6, &big)]);
                self.dispatch(&mut enc, &big_pass, &bind, (512, 1));
            }
            None => {
                if frame.probe[2] > 0.5 {
                    enc.clear_buffer(&self.probe_bins, 0, None);
                }
                let bins = self.probe_bins.clone();
                let bind = self.bind_cached(&self.k.project, &[(0, &self.frame), (1, &params), (2, &proj), (3, &counts), (4, &filter), (5, &seen_buf), (6, &bins), (7, &marks)]);
                self.dispatch(&mut enc, &self.k.project, &bind, groups(n));
            }
        }
        self.scan(&mut enc, &counts, &offsets, n, 0, "scan counts");
        let info = self.uniform_at(("total", 0), &[n as u32, cap, 0, 0]);
        let total_out = self.total_out.clone();
        let bind = self.bind_cached(&self.k.total, &[(0, &info), (1, &counts), (2, &offsets), (3, &total_out)]);
        self.dispatch(&mut enc, &self.k.total, &bind, (1, 1));
        // Pipelined: the sort's capacity (the kernels read the true, clamped count); else the count, waited for.
        let total = if pipelined.is_some() {
            cap
        } else {
            enc.copy_buffer_to_buffer(&total_out, 0, &self.total_staging, 0, 16);
            self.queue.submit([enc.finish()]);
            enc = self.device.create_command_encoder(&Default::default());
            let state = Arc::new(AtomicU32::new(0));
            {
                let state = state.clone();
                self.total_staging.map_async(wgpu::MapMode::Read, .., move |r| state.store(if r.is_ok() { 1 } else { 2 }, Ordering::SeqCst));
            }
            self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow!("{}", tr!("waiting for the GPU: {error}", error = e)))?;
            let total = match self.total_staging.get_mapped_range(..) {
                Ok(view) if state.load(Ordering::SeqCst) == 1 => bytemuck::cast_slice::<u8, u32>(&view)[0],
                _ => bail!("{}", tr("the GPU did not hand back a view's tile count (was the device lost?)")),
            };
            self.total_staging.unmap();
            anyhow::ensure!(total <= max_entries, "{}", tr!("{total} tile entries in one view: more than the GPU can sort ({max_entries})", total = total, max_entries = max_entries));
            self.note_total(size, total);
            // A viewer's next frames go without waiting while the sort has half again the estimate's room (`roomy`):
            // it gets that room now, or every frame would wait.
            let room = if training {
                total as usize
            } else {
                let est = self.estimate(size).unwrap_or(total) as usize;
                (est + est / 2).min(max_entries as usize)
            };
            self.ensure_sort(room);
            if seen.is_none() {
                self.redraw = false;
            }
            total
        };
        let (keys, values, hist, hist_offsets) = {
            let s = self.sort.as_ref().expect("sort");
            (s.keys.clone(), s.values.clone(), s.hist.clone(), s.hist_offsets.clone())
        };
        let (out_color, out_state, d_pixel, target_buf, ranges, partials, resid, kept) = {
            let i = self.image.as_ref().expect("image");
            (i.out_color.clone(), i.out_state.clone(), i.d_pixel.clone(), i.target.clone(), i.ranges.clone(), i.partials.clone(), i.resid.clone(), i.kept.clone())
        };
        enc.clear_buffer(&ranges, 0, None);
        if total > 0 {
            self.section("duplicate");
            match (self.k.training.as_ref().filter(|_| split), self.big.clone()) {
                (Some(t), Some(big)) => {
                    let (split, big_pass) = if tile8 { (t.duplicate8.clone(), t.duplicate_big8.clone()) } else { (t.duplicate.clone(), t.duplicate_big.clone()) };
                    let bind = self.bind_cached(&split, &[(0, &self.frame), (1, &proj), (2, &offsets), (3, &keys[0]), (4, &values[0]), (5, &counts)]);
                    self.dispatch(&mut enc, &split, &bind, groups(n));
                    let bind = self.bind_cached(&big_pass, &[(0, &self.frame), (1, &proj), (2, &offsets), (3, &keys[0]), (4, &values[0]), (5, &counts), (6, &big)]);
                    self.dispatch(&mut enc, &big_pass, &bind, (512, 1));
                }
                _ => {
                    let bind = self.bind_cached(&self.k.duplicate, &[(0, &self.frame), (1, &proj), (2, &offsets), (3, &keys[0]), (4, &values[0]), (5, &counts)]);
                    self.dispatch(&mut enc, &self.k.duplicate, &bind, groups(n));
                }
            }
            // Pipelined, the histograms' stride is the capacity's blocks and only the counted blocks scatter.
            let blocks = (total as usize).div_ceil(256);
            self.section("sort");
            for pass in 0..8u32 {
                let (a, b) = ((pass % 2) as usize, ((pass + 1) % 2) as usize);
                let info = self.uniform_at(("sort", pass as usize), &SortInfo { n: total, blocks: blocks as u32, shift: pass * 4, pad: 0 });
                let bind = self.bind_cached(&self.k.sort_count, &[(0, &info), (1, &keys[a]), (3, &hist), (6, &total_out)]);
                self.dispatch(&mut enc, &self.k.sort_count, &bind, groups_of(blocks));
                self.scan(&mut enc, &hist, &hist_offsets, blocks * 16, 0, "scan digits");
                // A training device ranks by subgroup ballots (shaders/sort_subgroups.wgsl): 0.42 ms against 1.11.
                let scatter = self.k.training.as_ref().map_or(&self.k.sort_scatter, |t| &t.sort_scatter);
                let bind = self.bind_cached(scatter, &[(0, &info), (1, &keys[a]), (2, &values[a]), (3, &hist_offsets), (4, &keys[b]), (5, &values[b]), (6, &total_out)]);
                if pipelined.is_some() {
                    self.dispatch_counted(&mut enc, scatter, &bind);
                } else {
                    self.dispatch(&mut enc, scatter, &bind, groups_of(blocks));
                }
            }
            self.section("ranges");
            let bind = self.bind_cached(&self.k.ranges, &[(0, &self.frame), (1, &keys[0]), (2, &ranges), (3, &total_out)]);
            if pipelined.is_some() {
                self.dispatch_counted(&mut enc, &self.k.ranges, &bind);
            } else {
                self.dispatch(&mut enc, &self.k.ranges, &bind, groups(total as usize));
            }
        }
        let tiles = (cam.width.div_ceil(tile), cam.height.div_ceil(tile));
        // The loss passes work on 16×16 blocks of pixels whatever the raster's tiles.
        let loss_tiles = (cam.width.div_ceil(cpu::TILE), cam.height.div_ceil(cpu::TILE));
        enc = self.pace(enc, 0)?;
        if let Some(seen) = seen {
            // A visibility round walks the entries as the forward pass would, recording what each Gaussian shows.
            let t = self.training()?;
            let mask = mask.as_ref().expect("checked above");
            if self.visibility_sum {
                let bind = self.bind(&t.contribution, &[(0, &self.frame), (1, &proj), (2, &values[0]), (3, &ranges), (8, mask), (9, seen)]);
                self.dispatch(&mut enc, &t.contribution, &bind, tiles);
            } else if self.visibility_over > 0.0 {
                let bind = self.bind(&t.visibility_over, &[(0, &self.frame), (1, &proj), (2, &values[0]), (3, &ranges), (8, mask), (10, seen)]);
                self.dispatch(&mut enc, &t.visibility_over, &bind, tiles);
            } else {
                let bind = self.bind(&t.visibility, &[(0, &self.frame), (1, &proj), (2, &values[0]), (3, &ranges), (8, mask), (9, seen)]);
                self.dispatch(&mut enc, &t.visibility, &bind, tiles);
            }
            // Governed, the round went to the GPU already (and this encoder is empty).
            let enc = self.pace(enc, 1)?;
            self.queue.submit([enc.finish()]);
            return Ok(total);
        }
        self.section("forward");
        let forward = match self.k.training.as_ref() {
            Some(t) if tile8 && self.nested_forward => &t.forward_nested8,
            Some(t) if tile8 => &t.forward8,
            Some(t) if self.nested_forward => &t.forward_nested,
            _ => &self.k.forward,
        };
        let bind = self.bind_cached(forward, &[(0, &self.frame), (1, &proj), (2, &values[0]), (3, &ranges), (4, &out_color), (5, &out_state)]);
        self.dispatch(&mut enc, forward, &bind, tiles);
        if let Some(texels) = self.sky.as_ref().map(|s| s.texels.clone()) {
            self.section("sky composite");
            let bind = self.bind_cached(&self.k.sky_composite, &[(0, &self.frame), (4, &out_color), (5, &out_state), (8, &texels)]);
            self.dispatch(&mut enc, &self.k.sky_composite, &bind, groups((cam.width * cam.height) as usize));
        }
        let colour_step = matches!(target, Target::Color { .. });
        match target {
            Target::Color { rgba, .. } => {
                let mask = mask.expect("checked above");
                let target_buf = match self.resident_target.take().and_then(|k| self.keyframes.get(k)) {
                    Some(b) if b.size() == (rgba.len() * 4) as u64 => b.clone(),
                    _ => {
                        self.queue.write_buffer(&target_buf, 0, bytemuck::cast_slice(rgba));
                        target_buf
                    }
                };
                let t = self.training()?;
                self.section("loss");
                // The pixels this step trains on: the scene mask, less the robust loss's outlier blobs.
                let mut used = mask.clone();
                if robust > 0.0 {
                    let bind = self.bind_cached(&t.robust_residual, &[(0, &self.frame), (1, &out_color), (2, &target_buf), (3, &mask), (4, &resid)]);
                    self.dispatch(&mut enc, &t.robust_residual, &bind, groups((cam.width * cam.height) as usize));
                    let bind = self.bind_cached(&t.robust_inliers, &[(0, &self.frame), (3, &mask), (4, &resid), (5, &kept)]);
                    self.dispatch(&mut enc, &t.robust_inliers, &bind, loss_tiles);
                    used = kept.clone();
                }
                if ssim > 0.0 {
                    let bind = self.bind_cached(&t.ssim_partials, &[(0, &self.frame), (1, &out_color), (2, &target_buf), (3, &used), (4, &partials)]);
                    self.dispatch(&mut enc, &t.ssim_partials, &bind, loss_tiles);
                    let bind = self.bind_cached(&t.ssim_gradient, &[(0, &self.frame), (1, &out_color), (2, &target_buf), (3, &used), (4, &partials), (5, &d_pixel)]);
                    self.dispatch(&mut enc, &t.ssim_gradient, &bind, loss_tiles);
                } else {
                    let bind = self.bind_cached(&t.loss_color, &[(0, &self.frame), (1, &out_color), (2, &target_buf), (3, &used), (4, &d_pixel)]);
                    self.dispatch(&mut enc, &t.loss_color, &bind, groups((cam.width * cam.height) as usize));
                }
            }
            Target::Depth { samples, .. } => {
                if self.samples.as_ref().is_none_or(|(c, _)| *c < samples.len()) {
                    let cap = samples.len().next_power_of_two().max(1024);
                    self.samples = Some((cap, storage(&self.device, "depth samples", cap as u64 * 16)));
                    self.forget_binds();
                }
                let (room, buf) = self.samples.clone().expect("samples");
                // The samples, then zeros ("no sample") to the end: the kernel runs over the whole buffer. The queue's
                // write lands before the encoder's clear runs.
                if !samples.is_empty() {
                    self.queue.write_buffer(&buf, 0, bytemuck::cast_slice(samples));
                }
                if samples.len() < room {
                    enc.clear_buffer(&buf, (samples.len() * 16) as u64, None);
                }
                enc.clear_buffer(&d_pixel, 0, None);
                let t = self.training()?;
                self.section("depth_loss");
                let bind = self.bind_cached(&t.depth_loss, &[(0, &self.frame), (1, &out_color), (2, &buf), (3, &d_pixel)]);
                self.dispatch(&mut enc, &t.depth_loss, &bind, groups(room));
            }
            Target::None => {}
        }
        if train {
            enc = self.pace(enc, 1)?;
            self.backward_into(&mut enc, &proj, &values[0], &ranges, &out_state, &d_pixel, tiles, lr);
            if lr.is_some() && colour_step {
                self.sky_step(&mut enc, &out_state, &d_pixel, (cam.width * cam.height) as usize);
            }
        }
        // The auto focus's histogram, to a free readback (none free: this frame's is not read).
        let probe_slot = if frame.probe[2] > 0.5 && !split { self.probes.iter().position(|p| p.state.load(Ordering::SeqCst) == 0) } else { None };
        if let Some(i) = probe_slot {
            enc.copy_buffer_to_buffer(&self.probe_bins, 0, &self.probes[i].buffer, 0, (crate::dof::BINS * 4) as u64);
        }
        let probe_mapped = |gpu: &mut Self| {
            if let Some(i) = probe_slot {
                gpu.probe_seq += 1;
                let p = &mut gpu.probes[i];
                p.seq = gpu.probe_seq;
                p.state.store(1, Ordering::SeqCst);
                let state = p.state.clone();
                p.buffer.map_async(wgpu::MapMode::Read, .., move |res| state.store(if res.is_ok() { 2 } else { 3 }, Ordering::SeqCst));
            }
        };
        if pipelined.is_some() {
            // The count comes back through the ring; a slot still in flight is waited for first.
            let slot = self.ring_next;
            self.ring_next = (slot + 1) % RING;
            self.wait_readback(slot);
            enc.copy_buffer_to_buffer(&total_out, 0, &self.ring[slot].buffer, 0, 16);
            let index = self.queue.submit([enc.finish()]);
            let r = &mut self.ring[slot];
            r.size = size;
            r.cap = cap;
            r.submission = Some(index);
            r.state.store(1, Ordering::SeqCst);
            let state = r.state.clone();
            r.buffer.map_async(wgpu::MapMode::Read, .., move |res| state.store(if res.is_ok() { 2 } else { 3 }, Ordering::SeqCst));
            probe_mapped(self);
        } else {
            self.queue.submit([enc.finish()]);
            probe_mapped(self);
        }
        self.paced_part(2)?;
        if let Some(p) = &self.profiler {
            p.collect(&self.device, &self.queue);
        }
        Ok(if pipelined.is_some() { self.estimate(size).unwrap_or(0) } else { total })
    }

    /// Follows VRChat on this graphics card (as crate::priority::govern last saw it, so only in a process that governs
    /// itself): while it runs, SteamVR's frame timing paces the steps (crate::governor); once it closes, nothing does.
    /// Some(true) when the governor just joined, Some(false) when it just left.
    pub fn follow_vrchat(&mut self) -> Option<bool> {
        let beside = crate::priority::vrchat() && !self.integrated;
        if beside && self.governor.is_none() {
            self.governor = crate::governor::Governor::connect();
            self.governor.is_some().then_some(true)
        } else if !beside && self.governor.is_some() {
            self.governor = None;
            Some(false)
        } else {
            None
        }
    }

    /// While governed: waits until VRChat's frames leave part `part` of a step its GPU time idle (a governor that saw
    /// SteamVR quit is dropped).
    fn pace_wait(&mut self, part: usize) {
        if let Some(g) = self.governor.as_mut() {
            if !g.wait(self.paced[part]) {
                self.governor = None;
            }
        }
    }

    /// While governed: hands part `part` (what `enc` holds) to the GPU, times it to its end, and waits for the next part's
    /// turn; the encoder to go on with. Ungoverned, `enc` itself.
    fn pace(&mut self, enc: wgpu::CommandEncoder, part: usize) -> anyhow::Result<wgpu::CommandEncoder> {
        if self.governor.is_none() {
            return Ok(enc);
        }
        self.queue.submit([enc.finish()]);
        self.paced_part(part)?;
        self.pace_wait(part + 1);
        Ok(self.device.create_command_encoder(&Default::default()))
    }

    /// While governed: waits for the GPU to finish what was submitted, part `part` of a step, and keeps its time.
    fn paced_part(&mut self, part: usize) -> anyhow::Result<()> {
        if self.governor.is_none() {
            return Ok(());
        }
        let start = std::time::Instant::now();
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow!("{}", tr!("waiting for the GPU: {error}", error = e)))?;
        // A moving average: one slow part (VRChat's work in between) moves it a quarter.
        let took = start.elapsed();
        self.paced[part] = (self.paced[part] * 3 + took) / 4;
        Ok(())
    }

    /// The backward pass: the rasteriser's (two pixels a thread on 16×16 tiles, one on 8×8: depth steps), then the
    /// projection backward, with this step's Adam step (`lr`) in the same pass, or writing the parameter gradients alone
    /// (tests: [`Gpu::read_grads`]).
    #[allow(clippy::too_many_arguments)]
    fn backward_into(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        proj: &wgpu::Buffer,
        values: &wgpu::Buffer,
        ranges: &wgpu::Buffer,
        out_state: &wgpu::Buffer,
        d_pixel: &wgpu::Buffer,
        tiles: (u32, u32),
        lr: Option<LearningRates>,
    ) {
        if lr.is_some() {
            self.step += 1;
        }
        // Gates (project_backward_adam.wgsl): bit 0 the noise spares the anchored, bit 1 only what this step shows, bit 2
        // colours stay within 0–1.
        let (noise, gate) = (self.noise, u32::from(self.noise_gate) | u32::from(self.noise_visible) << 1 | u32::from(self.clamp_colors) << 2);
        let info = lr.map(|lr| self.uniform_at(("adam", 0), &AdamInfo { lr: rates_of(lr), beta: [0.9, 0.999, 1e-15, noise * lr.means], n: [(self.count * PACKED) as u32, self.step, gate, 0x5eed] }));
        let s = self.splats.as_ref().expect("splats");
        // No clear of grad2d: it is zero here, as the projection backward zeroes the sums it reads (new buffers start
        // zeroed).
        let t = self.k.training.as_ref().expect("a training GPU");
        self.section("backward");
        let sky = self.sky.as_ref().map_or(&self.no_sky, |s| &s.texels);
        // The one-pixel kernel on 16×16 tiles only as tests' reference.
        let backward = match (self.tile8_step, self.reference_backward) {
            (true, _) => &t.backward8,
            (false, true) => &t.backward,
            (false, false) => &t.backward_blocks,
        };
        let bind = self.bind_cached(backward, &[(0, &self.frame), (1, proj), (2, values), (3, ranges), (5, out_state), (6, d_pixel), (7, &s.grad2d), (8, sky)]);
        self.dispatch(enc, backward, &bind, tiles);
        match info {
            Some(info) => {
                self.section("project_backward + adam");
                let bind = self.bind_cached(&t.project_backward_adam, &[(0, &self.frame), (1, &s.params), (2, proj), (3, &s.grad2d), (4, &s.filter), (5, &s.stats), (6, &s.m1), (7, &s.m2), (8, &info)]);
                self.dispatch(enc, &t.project_backward_adam, &bind, groups(self.count));
            }
            None => {
                self.section("project_backward");
                let bind = self.bind_cached(&t.project_backward, &[(0, &self.frame), (1, &s.params), (2, proj), (3, &s.grad2d), (4, &s.grads), (5, &s.stats), (6, &s.filter)]);
                self.dispatch(enc, &t.project_backward, &bind, groups(self.count));
            }
        }
    }

    /// Uploads every keyframe once (packed RGBA, as the colour steps take them) for steps to name in
    /// [`Gpu::resident_target`] instead of uploading their pixels: 8.3 MB a step through `write_buffer`, about a
    /// millisecond of the CPU's time a step, which bounds small scenes' steps.
    pub fn set_keyframes(&mut self, images: &[&[u32]]) {
        use rayon::prelude::*;
        // The copies into the upload buffers on every core.
        let device = &self.device;
        self.keyframes = images
            .par_iter()
            .map(|img| device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("keyframe"), contents: bytemuck::cast_slice(img), usage: wgpu::BufferUsages::STORAGE }))
            .collect();
    }

    /// The sky map's gradient from this colour step's pixel gradients, then its Adam step (at the splats' step) on the
    /// texels the step looked up (shaders/sky_adam.wgsl): Adam over all two million texels and a clear of their sums
    /// cost 5–10 % of a training run.
    fn sky_step(&mut self, enc: &mut wgpu::CommandEncoder, out_state: &wgpu::Buffer, d_pixel: &wgpu::Buffer, pixels: usize) {
        let Some((width, texels, l)) = self.sky.as_ref().and_then(|s| Some((s.width, s.texels.clone(), s.learning.as_ref()?))) else {
            return;
        };
        let (grads, m1, m2) = (l.grads.clone(), l.m1.clone(), l.m2.clone());
        let count = width as usize * (width as usize / 2);
        let info = self.uniform_at(("sky adam", 0), &SkyAdamInfo { rate: [self.sky_lr, 0.9, 0.999, 1e-15], n: [count as u32, self.step, 0, 0] });
        let t = self.k.training.as_ref().expect("a training GPU");
        self.section("sky gradient");
        let bind = self.bind_cached(&t.sky_gradient, &[(0, &self.frame), (5, out_state), (6, d_pixel), (7, &grads)]);
        self.dispatch(enc, &t.sky_gradient, &bind, groups(pixels));
        self.section("sky adam");
        let bind = self.bind_cached(&t.sky_adam, &[(0, &info), (1, &texels), (2, &m1), (3, &grads), (4, &m2)]);
        self.dispatch(enc, &t.sky_adam, &bind, groups(count));
    }

    /// A sky map of `width` × `width / 2` texels (rgb, crate::sky::SkyMap's layout) behind the splats from now on;
    /// `None` takes it away. A training device trains it with the colour steps.
    pub fn set_sky(&mut self, sky: Option<(u32, &[[f32; 3]])>) -> anyhow::Result<()> {
        let Some((width, rgb)) = sky else {
            self.sky = None;
            self.forget_binds();
            return Ok(());
        };
        anyhow::ensure!(width >= 4 && width % 4 == 0 && rgb.len() == (width as usize) * (width as usize / 2), "{}", tr!("a sky map of {texels} texels for {width} across", texels = rgb.len(), width = width));
        let texels: Vec<[f32; 4]> = rgb.iter().map(|c| [c[0], c[1], c[2], 0.0]).collect();
        let bytes = (texels.len() * 16) as u64;
        let t = storage(&self.device, "sky texels", bytes);
        self.queue.write_buffer(&t, 0, bytemuck::cast_slice(&texels));
        let learning = self.k.training.as_ref().map(|k| SkyLearning {
            grads: storage(&self.device, "sky grads", texels.len() as u64 * 4 * k.sums.slot_bytes()),
            m1: storage(&self.device, "sky m1", bytes),
            m2: storage(&self.device, "sky m2", bytes),
        });
        self.sky = Some(SkyBuffers { width, texels: t, learning });
        self.forget_binds();
        Ok(())
    }

    /// Which texels of the sky map training moved (their Adam moments are no longer zero), or `None` without a map
    /// trained on this device.
    pub fn sky_trained(&self) -> anyhow::Result<Option<Vec<bool>>> {
        let Some((s, l)) = self.sky.as_ref().and_then(|s| Some((s, s.learning.as_ref()?))) else { return Ok(None) };
        let n = s.width as usize * (s.width as usize / 2);
        let m2: Vec<[f32; 4]> = self.read(&l.m2, n)?;
        Ok(Some(m2.iter().map(|m| m[0] != 0.0 || m[1] != 0.0 || m[2] != 0.0).collect()))
    }

    /// The sky map as it is now (rgb per texel), or `None` without one.
    pub fn sky(&self) -> anyhow::Result<Option<(u32, Vec<[f32; 3]>)>> {
        let Some(s) = self.sky.as_ref() else { return Ok(None) };
        let n = s.width as usize * (s.width as usize / 2);
        let texels: Vec<[f32; 4]> = self.read(&s.texels, n)?;
        Ok(Some((s.width, texels.iter().map(|t| [t[0], t[1], t[2]]).collect())))
    }

    /// Backward from given pixel gradients (r, g, b, depth per pixel) after a forward [`Gpu::step`] with
    /// [`Target::None`] on the same camera; leaves the parameter gradients in place (see [`Gpu::read_grads`]).
    pub fn backward_from(&mut self, cam: &Camera, d_pixel: &[[f32; 4]]) {
        let i = self.image.as_ref().expect("image");
        let s = self.sort.as_ref().expect("sort");
        let splats = self.splats.as_ref().expect("splats");
        self.queue.write_buffer(&i.d_pixel, 0, bytemuck::cast_slice(d_pixel));
        let mut enc = self.device.create_command_encoder(&Default::default());
        // On the tiles the last render used.
        let tile = if self.tile8_step { 8 } else { cpu::TILE };
        let tiles = (cam.width.div_ceil(tile), cam.height.div_ceil(tile));
        let (proj, values, ranges, out_state, d_pixel) = (splats.proj.clone(), s.values[0].clone(), i.ranges.clone(), i.out_state.clone(), i.d_pixel.clone());
        self.backward_into(&mut enc, &proj, &values, &ranges, &out_state, &d_pixel, tiles, None);
        self.queue.submit([enc.finish()]);
    }

    /// Lab (`examples/rasterdump.rs`, the gpu-stacks experiment): one colour step's rasteriser inputs and outputs, read
    /// back so ports of the forward and backward kernels to other GPU languages (Triton, CUDA C++) can be checked and
    /// timed on real data. Runs the step without an Adam step, then the forward alone again (its output before the sky
    /// composite) and the rasteriser's backward alone into zeroed sums (the projection backward would read and zero
    /// them). Float sums on 16×16 tiles only (the training colour steps' path on Vulkan with f32 atomics).
    pub fn lab_raster_dump(&mut self, cam: &Camera, target: Target<'_>) -> anyhow::Result<RasterDump> {
        anyhow::ensure!(matches!(target, Target::Color { .. }), "a colour step"); // i18n: not shown (the lab raster dump of examples/rasterdump.rs: no window starts it)
        self.step(cam, [0.0; 3], target, None)?;
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        anyhow::ensure!(!self.tile8_step, "16×16 tiles"); // i18n: not shown (the lab raster dump of examples/rasterdump.rs: no window starts it)
        let t = self.k.training.as_ref().context("a training device")?; // i18n: not shown (the lab raster dump of examples/rasterdump.rs: no window starts it)
        anyhow::ensure!(matches!(t.sums, GradientSums::Float), "float gradient sums (this device has {:?})", t.sums); // i18n: not shown (the lab raster dump of examples/rasterdump.rs: no window starts it)
        let backward = if self.reference_backward { t.backward.clone() } else { t.backward_blocks.clone() };
        let i = self.image.as_ref().context("image")?;
        let s = self.sort.as_ref().context("sort")?;
        let sp = self.splats.as_ref().context("splats")?;
        let (out_color, out_state, d_pixel, ranges) = (i.out_color.clone(), i.out_state.clone(), i.d_pixel.clone(), i.ranges.clone());
        let (proj, values, grad2d) = (sp.proj.clone(), s.values[0].clone(), sp.grad2d.clone());
        let tiles = (cam.width.div_ceil(cpu::TILE), cam.height.div_ceil(cpu::TILE));
        let ntiles = (tiles.0 * tiles.1) as usize;
        let px = (cam.width * cam.height) as usize;
        let ranges_v: Vec<u32> = self.read(&ranges, 2 * ntiles)?;
        let entries = ranges_v.chunks(2).map(|r| r[1]).max().unwrap_or(0) as usize;
        let sky = self.sky.as_ref().map(|s| s.texels.clone());
        let mut enc = self.device.create_command_encoder(&Default::default());
        let forward = if self.nested_forward { t.forward_nested.clone() } else { self.k.forward.clone() };
        let bind = self.bind_cached(&forward, &[(0, &self.frame), (1, &proj), (2, &values), (3, &ranges), (4, &out_color), (5, &out_state)]);
        self.dispatch(&mut enc, &forward, &bind, tiles);
        enc.clear_buffer(&grad2d, 0, None);
        let sky_buf = sky.clone().unwrap_or_else(|| self.no_sky.clone());
        let bind = self.bind_cached(&backward, &[(0, &self.frame), (1, &proj), (2, &values), (3, &ranges), (5, &out_state), (6, &d_pixel), (7, &grad2d), (8, &sky_buf)]);
        self.dispatch(&mut enc, &backward, &bind, tiles);
        self.queue.submit([enc.finish()]);
        let sky_texels = match (&sky, self.sky.as_ref()) {
            (Some(b), Some(m)) => self.read(b, m.width as usize * (m.width as usize / 2))?,
            _ => Vec::new(),
        };
        Ok(RasterDump {
            frame: self.read(&self.frame, std::mem::size_of::<Frame>() / 4)?,
            proj: self.read(&proj, self.count * 16)?,
            values: self.read(&values, entries)?,
            ranges: ranges_v,
            out_color: self.read(&out_color, px)?,
            out_state: self.read(&out_state, px)?,
            d_pixel: self.read(&d_pixel, px)?,
            grad2d: self.read(&grad2d, self.count * GRAD2D)?,
            sky: sky_texels,
            count: self.count,
            tiles,
        })
    }

    /// Lab (`examples/rasterdump.rs --time N`): after [`Gpu::lab_raster_dump`], the forward and the rasteriser's backward
    /// of that step again `reps` times, each in its own submission, timed with GPU timestamps (needs
    /// [`DeviceOptions::profile`]): (forward ms, backward ms) per repeat, the backward into sums cleared before it.
    pub fn lab_time_raster(&mut self, cam: &Camera, reps: usize) -> anyhow::Result<Vec<(f64, f64)>> {
        let t = self.k.training.as_ref().context("a training device")?; // i18n: not shown (the lab raster dump of examples/rasterdump.rs: no window starts it)
        let backward = if self.reference_backward { t.backward.clone() } else { t.backward_blocks.clone() };
        let i = self.image.as_ref().context("image")?;
        let s = self.sort.as_ref().context("sort")?;
        let sp = self.splats.as_ref().context("splats")?;
        let (out_color, out_state, d_pixel, ranges) = (i.out_color.clone(), i.out_state.clone(), i.d_pixel.clone(), i.ranges.clone());
        let (proj, values, grad2d) = (sp.proj.clone(), s.values[0].clone(), sp.grad2d.clone());
        let tiles = (cam.width.div_ceil(cpu::TILE), cam.height.div_ceil(cpu::TILE));
        let sky_buf = self.sky.as_ref().map_or_else(|| self.no_sky.clone(), |s| s.texels.clone());
        let forward = if self.nested_forward { t.forward_nested.clone() } else { self.k.forward.clone() };
        let fbind = self.bind_cached(&forward, &[(0, &self.frame), (1, &proj), (2, &values), (3, &ranges), (4, &out_color), (5, &out_state)]);
        let bbind = self.bind_cached(&backward, &[(0, &self.frame), (1, &proj), (2, &values), (3, &ranges), (5, &out_state), (6, &d_pixel), (7, &grad2d), (8, &sky_buf)]);
        anyhow::ensure!(self.profiler.is_some(), "a profiling device (DeviceOptions::profile)");
        let mut out = Vec::with_capacity(reps);
        for _ in 0..reps {
            self.profiler.as_ref().expect("profiler").reset();
            let mut enc = self.device.create_command_encoder(&Default::default());
            self.section("forward");
            self.dispatch(&mut enc, &forward, &fbind, tiles);
            enc.clear_buffer(&grad2d, 0, None);
            self.section("backward");
            self.dispatch(&mut enc, &backward, &bbind, tiles);
            self.queue.submit([enc.finish()]);
            let p = self.profiler.as_ref().expect("profiler");
            p.collect(&self.device, &self.queue);
            let totals = p.totals.borrow();
            out.push((totals.get("forward").map_or(0.0, |v| v.0), totals.get("backward").map_or(0.0, |v| v.0)));
        }
        Ok(out)
    }

    /// Writes the last rendered image (colour over the background) into `target`, an RGBA8 storage texture of the
    /// camera's size: the image a viewer shows. An error (a texture of another size or format) is caught and logged,
    /// not handed to the device's handler.
    pub fn present(&self, target: &wgpu::TextureView) {
        let Some(i) = self.image.as_ref() else { return };
        let invalid = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = self.k.present.get_bind_group_layout(0);
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("present"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.frame.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: i.out_color.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(target) },
            ],
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.dispatch(&mut enc, &self.k.present, &bind, (i.width.div_ceil(16), i.height.div_ceil(16)));
        self.queue.submit([enc.finish()]);
        if let Some(e) = block_on(invalid.pop()) {
            eprintln!("splat view: {e}");
        }
    }

    /// [`Gpu::present`] with what the splats left of the background as transparency, in premultiplied colour: for a
    /// render over black with no sky map.
    pub fn present_alpha(&self, target: &wgpu::TextureView) {
        let Some(i) = self.image.as_ref() else { return };
        let invalid = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = self.k.present_alpha.get_bind_group_layout(0);
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("present alpha"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.frame.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: i.out_color.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(target) },
                wgpu::BindGroupEntry { binding: 3, resource: i.out_state.as_entire_binding() },
            ],
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.dispatch(&mut enc, &self.k.present_alpha, &bind, (i.width.div_ceil(16), i.height.div_ceil(16)));
        self.queue.submit([enc.finish()]);
        if let Some(e) = block_on(invalid.pop()) {
            eprintln!("splat view: {e}");
        }
    }

    /// The last rendered image: (r, g, b, depth) per pixel.
    pub fn read_image(&self) -> anyhow::Result<Vec<[f32; 4]>> {
        let i = self.image.as_ref().with_context(|| tr("nothing rendered"))?;
        let px = (i.width * i.height) as usize;
        Ok(self.read::<f32>(&i.out_color, px * 4)?.as_chunks::<4>().0.to_vec())
    }

    /// The largest blending weight each Gaussian reaches in a scene pixel (the mask set for the image size) of any of
    /// these views: what training ever sees of it (raster_visibility.wgsl).
    /// What the training keyframes `cams` see of each Gaussian ([`Seen`]): its largest blending weight in their scene
    /// pixels, and the keyframes where it reaches [`SEEN_IN`], the compass sectors they looked from and their finest
    /// sampling rate. One visibility round a keyframe.
    pub fn seen(&mut self, cams: &[Camera], background: [f32; 3]) -> anyhow::Result<Vec<Seen>> {
        let n = self.count;
        let view = storage(&self.device, "seen in a view", (n * 4) as u64);
        let out = storage(&self.device, "seen", (n * 16) as u64);
        for cam in cams {
            self.stop_if_paused()?;
            self.caught(|gpu| gpu.run(cam, background, Target::None, None, Some(&view)))?;
            let t = self.training()?;
            let s = self.splats.as_ref().with_context(|| tr("no Gaussians"))?;
            let eye = cam.position();
            let info = self.uniform(&[
                n as u32,
                SEEN_IN.to_bits(),
                cam.fx.max(cam.fy).to_bits(),
                0,
                eye[0].to_bits(),
                eye[1].to_bits(),
                eye[2].to_bits(),
                0,
                cam.r[2][0].to_bits(),
                cam.r[2][1].to_bits(),
                cam.r[2][2].to_bits(),
                cam.t[2].to_bits(),
            ]);
            let bind = self.bind(&t.seen, &[(0, &info), (1, &s.params), (2, &view), (3, &out)]);
            let mut enc = self.device.create_command_encoder(&Default::default());
            self.dispatch(&mut enc, &t.seen, &bind, groups(n));
            self.queue.submit([enc.finish()]);
        }
        let raw: Vec<[u32; 4]> = self.read::<u32>(&out, n * 4)?.as_chunks::<4>().0.to_vec();
        Ok(raw.into_iter().map(|[w, views, sectors, rate]| Seen { weight: f32::from_bits(w), views, sectors: sectors as u16, rate: f32::from_bits(rate), beyond: 0.0 }).collect())
    }

    /// Each Gaussian's contribution to the scene pixels (the mask set for each image size) of `cams`: the sum of the
    /// blending weights α·T it reaches over them (LightGaussian's global significance; raster_contribution.wgsl), in
    /// pixels. Each view waits for its count, so no entry is dropped.
    pub fn contribution(&mut self, cams: &[Camera], background: [f32; 3]) -> anyhow::Result<Vec<f32>> {
        let n = self.count;
        if n == 0 {
            return Ok(Vec::new());
        }
        let view = storage(&self.device, "contribution in a view", (n * 4) as u64);
        let total = storage(&self.device, "contribution", (n * 4) as u64);
        self.visibility_sum = true;
        let overflows = self.overflows;
        let round = cams.iter().try_for_each(|cam| -> anyhow::Result<()> {
            self.caught(|gpu| gpu.run(cam, background, Target::None, None, Some(&view)))?;
            let t = self.training()?;
            let info = self.uniform(&[n as u32, 0, 0, 0]);
            let bind = self.bind(&t.contribution_fold, &[(0, &info), (1, &view), (2, &total)]);
            let mut enc = self.device.create_command_encoder(&Default::default());
            self.dispatch(&mut enc, &t.contribution_fold, &bind, groups(n));
            self.queue.submit([enc.finish()]);
            Ok(())
        });
        self.visibility_sum = false;
        round?;
        if self.overflows != overflows {
            eprintln!("a contribution round dropped tile entries");
        }
        self.read::<f32>(&total, n)
    }

    /// Each Gaussian's largest blending weight in a scene pixel of any of `cams` (`at_least` 0), or with `at_least` a
    /// weight of at least that where one reaches it and 0 elsewhere (fading: threshold stores instead of an atomic
    /// maximum at every blend). The views go without waiting for their counts where an estimate sizes their sort (waiting
    /// cost fading's 720 views on VRChat Home about a second); a round that dropped entries, which would leave their
    /// Gaussians unseen and fade them, is done again, waiting.
    pub fn visibility(&mut self, cams: &[Camera], background: [f32; 3], at_least: f32) -> anyhow::Result<Vec<f32>> {
        let n = self.count;
        self.visibility_over = at_least;
        let mut out = None;
        for pipelined in [true, false] {
            let seen = storage(&self.device, "seen", (n * 4) as u64);
            self.visibility_pipelined = pipelined;
            let overflows = self.overflows;
            let round = cams.iter().try_for_each(|cam| {
                self.stop_if_paused()?;
                self.caught(|gpu| gpu.run(cam, background, Target::None, None, Some(&seen)).map(|_| ()))
            });
            self.wait_readbacks();
            if let Err(e) = round {
                out = Some(Err(e));
                break;
            }
            if self.overflows == overflows {
                out = Some(self.read::<u32>(&seen, n).map(|bits| bits.into_iter().map(f32::from_bits).collect()));
                break;
            }
            eprintln!("a visibility round dropped tile entries: done again, waiting for each view's count");
        }
        self.visibility_pipelined = false;
        self.visibility_over = 0.0;
        out.unwrap_or_else(|| Err(anyhow!("{}", tr("a visibility round dropped tile entries even when waiting"))))
    }

    /// PSNR of a render from `cam` against a keyframe over the scene pixels (the mask set for its size), computed on
    /// the device: only one partial sum per 256 pixels crosses the bus.
    pub fn score(&mut self, cam: &Camera, background: [f32; 3], rgba: &[u32]) -> anyhow::Result<f32> {
        self.training()?;
        anyhow::ensure!(rgba.len() == (cam.width * cam.height) as usize, "{}", tr!("a keyframe of {pixels} pixels for a {width}×{height} camera", pixels = rgba.len(), width = cam.width, height = cam.height));
        let mask = self.mask_for(cam.width, cam.height)?;
        self.step(cam, background, Target::None, None)?;
        let t = self.training()?;
        let i = self.image.as_ref().with_context(|| tr("nothing rendered"))?;
        self.queue.write_buffer(&i.target, 0, bytemuck::cast_slice(rgba));
        let (gx, gy) = groups((cam.width * cam.height) as usize);
        let mut enc = self.device.create_command_encoder(&Default::default());
        let bind = self.bind_cached(&t.squared_error, &[(0, &self.frame), (1, &i.out_color), (2, &i.target), (3, &mask), (4, &i.d_pixel)]);
        self.dispatch(&mut enc, &t.squared_error, &bind, (gx, gy));
        self.queue.submit([enc.finish()]);
        let partial: Vec<f32> = self.read(&i.d_pixel, (gx * gy) as usize * 4)?;
        let (se, count) = partial.as_chunks::<4>().0.iter().fold((0f64, 0f64), |(s, c), p| (s + p[0] as f64, c + p[1] as f64));
        // No scene pixel is no score (not a perfect one).
        anyhow::ensure!(count > 0.0, "{}", tr!("no scene pixel to score in a {width}×{height} view", width = cam.width, height = cam.height));
        let mse = (se / count).max(1e-10);
        Ok((10.0 * (1.0 / mse).log10()) as f32)
    }

    /// The pixels the last robust colour step kept (1) or left out (0).
    pub fn read_kept(&self) -> anyhow::Result<Vec<f32>> {
        let i = self.image.as_ref().with_context(|| tr("nothing rendered"))?;
        self.read::<f32>(&i.kept, (i.width * i.height) as usize)
    }

    /// The last step's loss gradient per pixel (r, g, b, depth), for tests.
    pub fn read_d_pixel(&self) -> anyhow::Result<Vec<[f32; 4]>> {
        let i = self.image.as_ref().with_context(|| tr("nothing rendered"))?;
        let px = (i.width * i.height) as usize;
        Ok(self.read::<f32>(&i.d_pixel, px * 4)?.as_chunks::<4>().0.to_vec())
    }

    /// The last rendered final transmittance per pixel.
    pub fn read_transmittance(&self) -> anyhow::Result<Vec<f32>> {
        let i = self.image.as_ref().with_context(|| tr("nothing rendered"))?;
        let px = (i.width * i.height) as usize;
        Ok(self.read::<u32>(&i.out_state, px * 2)?.as_chunks::<2>().0.iter().map(|c| f32::from_bits(c[0])).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Many random Gaussians in front of a camera whose image is not a whole number of tiles.
    fn scene(count: usize, width: u32, height: u32, seed: u64) -> (Gaussians, Camera) {
        let pose = [0.96, 0.0, 0.28, 0.0, 0.0, 1.0, 0.0, 0.0, -0.28, 0.0, 0.96, 0.0];
        let cam = Camera::from_strip(&pose, &[1.2, 1.2 * width as f32 / height as f32, 0.0, 0.0], width, height, 1.0);
        let mut g = Gaussians::default();
        let mut rng = fastrand::Rng::with_seed(seed);
        for _ in 0..count {
            let z = 1.0 + rng.f32() * 5.0;
            let p = cam.unproject(-10.0 + rng.f32() * (width as f32 + 20.0), -10.0 + rng.f32() * (height as f32 + 20.0), z);
            g.push(
                p,
                [-3.4 + rng.f32() * 1.2, -3.4 + rng.f32() * 1.2, -4.0 + rng.f32() * 1.5],
                [0.5 + rng.f32(), rng.f32() - 0.5, rng.f32() - 0.5, rng.f32() - 0.5],
                -1.5 + rng.f32() * 3.0,
                [rng.f32(), rng.f32(), rng.f32()],
            );
        }
        (g, cam)
    }

    /// A training device for every kind of gradient sums this adapter can do (none without a suitable GPU).
    fn devices() -> Vec<Gpu> {
        devices_with(0)
    }

    /// [`devices`] whose backward adds its sums as a subgroup of `lanes` lanes would ([`DeviceOptions::sum_lanes`]).
    fn devices_with(lanes: u32) -> Vec<Gpu> {
        GradientSums::ALL
            .into_iter()
            .filter_map(|sums| match Gpu::with_options(DeviceOptions { sums: Some(sums), sum_lanes: lanes, ..Default::default() }) {
                Ok(gpu) => Some(gpu),
                Err(e) => {
                    eprintln!("skipped {sums:?}: {e:#}");
                    None
                }
            })
            .collect()
    }

    /// On D3D12 the nested forward's modules are the flattened forward under the nested entry name, and still validate.
    #[test]
    fn d3d12_builds_the_flattened_forward_under_the_nested_name() {
        let nested: Vec<_> = kernel_sources(Some(GradientSums::Float), 0).into_iter().filter(|k| k.name.starts_with("raster forward nested")).collect();
        assert_eq!(nested.len(), 2);
        for k in nested {
            assert!(k.code.contains("for (var b0"), "{}: the nested loop off D3D12", k.name);
            let f = flattened_on_dx12(k);
            assert!(f.code.contains("fn forward_nested(") && !f.code.contains("for (var b0") && f.code.contains("let slot = (e - first)"), "{}", f.name);
            let module = naga::front::wgsl::parse_str(&f.code).unwrap_or_else(|e| panic!("{}: {}", f.name, e.emit_to_string(&f.code)));
            naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).unwrap_or_else(|e| panic!("{}: {e:?}", f.name));
        }
    }

    /// Every kernel module of every kind of device parses and validates, and translates for Vulkan (SPIR-V) and, but for
    /// the f32-atomic sums no D3D12 device offers, for Direct3D (HLSL, shader model 6.6). On the CPU: no GPU needed.
    #[test]
    fn every_kernel_validates_and_translates() {
        let mut sources = kernel_sources(None, 0);
        for sums in GradientSums::ALL {
            for lanes in [0, 4, 8] {
                sources.extend(kernel_sources(Some(sums), lanes).into_iter().map(|k| (sums, k)).filter(|(_, k)| lanes == 0 || k.name == "raster backward").map(|(sums, k)| KernelSource { name: k.name, code: format!("// {sums:?}\n{}", k.code), entries: k.entries }));
            }
        }
        for k in &sources {
            let module = naga::front::wgsl::parse_str(&k.code).unwrap_or_else(|e| panic!("{}: {}", k.name, e.emit_to_string(&k.code)));
            let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).unwrap_or_else(|e| panic!("{}: {e:?}", k.name));
            for entry in k.entries {
                assert!(module.entry_points.iter().any(|e| e.name == *entry), "{} has no entry {entry}", k.name);
            }
            // Subgroup operations need SPIR-V 1.3.
            let spirv = naga::back::spv::write_vec(&module, &info, &naga::back::spv::Options { lang_version: (1, 3), ..Default::default() }, None);
            assert!(spirv.is_ok(), "{}: SPIR-V {:?}", k.name, spirv.err());
            if !k.code.starts_with("// Float") {
                for entry in k.entries {
                    let mut out = String::new();
                    let options = naga::back::hlsl::Options { shader_model: naga::back::hlsl::ShaderModel::V6_6, ..Default::default() };
                    let pipeline = naga::back::hlsl::PipelineOptions { entry_point: Some((naga::ShaderStage::Compute, (*entry).into())) };
                    let written = naga::back::hlsl::Writer::new(&mut out, &options, &pipeline).write(&module, &info, None);
                    assert!(written.is_ok(), "{}::{entry}: HLSL {:?}", k.name, written.err());
                }
            }
        }
    }

    /// Every kernel a Mac builds translates to Metal's language (the Mac itself compiles them in every test here that
    /// makes a device; this says so on any computer): no FixedPoint, Metal has no 64-bit atomic adds. MSL 3.0: f32
    /// atomics need it.
    #[test]
    fn every_kernel_translates_for_metal() {
        let mut sources: Vec<(String, KernelSource)> = kernel_sources(None, 0).into_iter().map(|k| ("viewer".to_string(), k)).collect();
        for sums in [GradientSums::Float, GradientSums::CompareAndSwap] {
            sources.extend(kernel_sources(Some(sums), 0).into_iter().map(|k| (format!("{sums:?}"), k)));
        }
        for (kind, k) in &sources {
            let module = naga::front::wgsl::parse_str(&k.code).unwrap_or_else(|e| panic!("{}: {}", k.name, e.emit_to_string(&k.code)));
            let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).unwrap_or_else(|e| panic!("{}: {e:?}", k.name));
            for entry in k.entries {
                let options = naga::back::msl::Options { lang_version: (3, 0), fake_missing_bindings: true, ..Default::default() };
                let pipeline = naga::back::msl::PipelineOptions { entry_point: Some((naga::ShaderStage::Compute, (*entry).into())), ..Default::default() };
                let written = naga::back::msl::write_string(&module, &info, &options, &pipeline);
                assert!(written.is_ok(), "{kind} {}::{entry}: MSL {:?}", k.name, written.err());
            }
        }
    }

    /// Sort keys: never more depth bits than f32 holds exactly, and a splat beyond the far plane stays in its tile.
    #[test]
    fn depth_keys_never_spill_into_the_tile_id() {
        // The 1080p depth-sample grid (240 × 110: 105 tiles) left 25 depth bits.
        for (w, h) in [(240u32, 110u32), (40, 30), (16, 16), (1920, 1080), (3840, 2160)] {
            let bits = depth_bits(w, h);
            let tiles = w.div_ceil(cpu::TILE) * h.div_ceil(cpu::TILE);
            assert!(bits <= 24 && (tiles - 1) < (1u32 << (32 - bits)) - 1, "{w}×{h}: {bits} bits");
            for z in [0.01f32, cpu::NEAR as f32, 1.0, 999.0, FAR, 5000.0, f32::INFINITY] {
                assert!(depth_key(z, bits) < (1 << bits), "{w}×{h}, z {z}");
            }
            // Order holds up to the far plane.
            assert!(depth_key(1.0, bits) < depth_key(2.0, bits) && depth_key(999.0, bits) <= depth_key(FAR, bits));
        }
    }

    #[test]
    fn forward_and_backward_match_the_cpu_reference() {
        forward_and_backward_match(devices());
        let reference: Vec<Gpu> = devices()
            .into_iter()
            .map(|mut g| {
                g.reference_backward = true;
                g
            })
            .collect();
        forward_and_backward_match(reference);
    }

    /// A training step's fused projection backward and Adam step moves every Gaussian as the projection backward's
    /// gradients and a CPU Adam step do (those it shows and, with no gradient, those it does not).
    #[test]
    fn fused_adam_steps_as_a_cpu_adam_step() {
        let Some(mut gpu) = devices().into_iter().next() else { return };
        let (g, cam) = scene(3000, 160, 90, 61);
        let mut zoomed = cam;
        zoomed.fx *= 2.5;
        zoomed.fy *= 2.5;
        let mut rng = fastrand::Rng::with_seed(62);
        let target: Vec<u32> = (0..cam.width * cam.height).map(|_| rng.u32(..) | 0xff00_0000).collect();
        gpu.set_mask(cam.width, cam.height, &vec![1.0; (cam.width * cam.height) as usize]).expect("mask");
        let lr = LearningRates { means: 1e-3, log_scales: 0.005, rotations: 0.001, opacity: 0.05, colors: 0.0025 };
        let px = (cam.width * cam.height) as f32;
        // Moments from a first step on the whole view, then a step on a zoomed view that leaves some Gaussians unshown.
        gpu.upload(&g);
        gpu.step(&cam, [0.0; 3], Target::Color { rgba: &target, scene_pixels: px, weight: 1.0, ssim: 0.2, robust: 0.0 }, Some(lr)).expect("step");
        let (before, m1, m2, _) = gpu.download().expect("download");
        gpu.step(&zoomed, [0.0; 3], Target::Color { rgba: &target, scene_pixels: px, weight: 1.0, ssim: 0.2, robust: 0.0 }, None).expect("gradients");
        let grads = gpu.read_grads().expect("gradients");
        gpu.upload_with_moments(&before, Some((&m1, &m2)));
        gpu.step = 1;
        gpu.step(&zoomed, [0.0; 3], Target::Color { rgba: &target, scene_pixels: px, weight: 1.0, ssim: 0.2, robust: 0.0 }, Some(lr)).expect("step");
        let (after, _, _, _) = gpu.download().expect("download");
        let rates = rates_of(lr);
        let (b1, b2, eps, t) = (0.9f64, 0.999f64, 1e-15f64, 2.0f64);
        let (p0, p1) = (before.pack(), after.pack());
        let (mut compared, mut off, mut unshown_moved) = (0usize, 0usize, 0usize);
        for k in 0..p0.len() {
            let slot = k % PACKED;
            if rates[slot] == 0.0 {
                continue;
            }
            let (g, m, v) = (grads[k] as f64, m1[k] as f64, m2[k] as f64);
            let m = b1 * m + (1.0 - b1) * g;
            let v = b2 * v + (1.0 - b2) * g * g;
            let expected = p0[k] as f64 - rates[slot] as f64 * (m / (1.0 - b1.powf(t))) / ((v / (1.0 - b2.powf(t))).sqrt() + eps);
            let moved = (p0[k] as f64 - expected).abs();
            if g == 0.0 && moved > 0.0 {
                unshown_moved += 1;
            }
            // Float atomics sum each gradient in any order: compare where the step moved the parameter.
            if moved > 1e-6 {
                compared += 1;
                if (p1[k] as f64 - expected).abs() > 0.01 * moved + 1e-7 {
                    off += 1;
                }
            }
        }
        eprintln!("fused Adam against a CPU Adam step: {off} of {compared} moved parameters off by over 1 %; {unshown_moved} moved by momentum alone");
        assert!(unshown_moved > 100, "the zoomed view showed every Gaussian");
        assert!(compared > 10_000 && off * 1000 < compared, "{off} of {compared}");
    }

    /// A paused run's state comes back whole on another device: parameters, each Gaussian's restart slot, moments,
    /// statistics, and the sky map's texels and moments; and the steps go on from it.
    #[test]
    fn a_devices_training_state_comes_back_on_another_device() {
        let (w, h) = (64, 48);
        let (g, cam) = scene(400, w, h, 7);
        let target = vec![0xff40_8040u32; (w * h) as usize];
        let lr = LearningRates { means: 1e-3, log_scales: 0.005, rotations: 0.001, opacity: 0.05, colors: 0.0025 };
        let color = || Target::Color { rgba: &target, scene_pixels: (w * h) as f32, weight: 1.0, ssim: 0.2, robust: 0.0 };
        let Some(mut a) = devices().into_iter().next() else { return };
        a.upload(&g);
        a.set_mask(w, h, &vec![1.0; (w * h) as usize]).unwrap();
        a.set_sky(Some((8, &vec![[0.25, 0.5, 0.75]; 32]))).unwrap();
        for _ in 0..20 {
            a.step(&cam, [0.0; 3], color(), Some(lr)).unwrap();
        }
        let state = a.training_state().unwrap();
        let sky = a.sky_learning().unwrap().expect("a sky map");
        let step = a.step;
        assert!(state.m2.iter().any(|v| *v != 0.0), "the steps left moments");
        drop(a);
        let Some(mut b) = devices().into_iter().next() else { return };
        b.restore_training(&state);
        b.restore_sky_learning(Some(&sky)).unwrap();
        b.step = step;
        b.set_mask(w, h, &vec![1.0; (w * h) as usize]).unwrap();
        let again = b.training_state().unwrap();
        assert_eq!((&again.params, &again.m1, &again.m2, &again.stats), (&state.params, &state.m1, &state.m2, &state.stats));
        let sky_again = b.sky_learning().unwrap().expect("a sky map");
        assert_eq!((sky_again.width, &sky_again.m1, &sky_again.m2), (sky.width, &sky.m1, &sky.m2));
        let before = b.score(&cam, [0.0; 3], &target).unwrap();
        for _ in 0..20 {
            b.step(&cam, [0.0; 3], color(), Some(lr)).unwrap();
        }
        assert!(b.score(&cam, [0.0; 3], &target).unwrap() > before, "and it goes on learning");
    }

    #[test]
    fn a_low_priority_device_trains_alike() {
        let Ok(gpu) = Gpu::with_options(DeviceOptions { low_priority: true, ..Default::default() }) else { return };
        eprintln!("adapter {}; free {:?} MB", gpu.adapter, gpu.free_memory().map(|b| b >> 20));
        if let Some(free) = gpu.free_memory() {
            assert!(free > 64 << 20 && free < 1 << 40, "{free}");
        }
        forward_and_backward_match(vec![gpu]);
    }

    /// The backward's generic sums as 4-, 8- and 16-lane subgroups add them (lanes adding slot k, k + lanes, …): on an
    /// 8-lane device slots 8 and 9 (blue and depth) were never added.
    #[test]
    fn narrow_subgroup_sums_match_the_cpu_reference() {
        for lanes in [4, 8, 16] {
            forward_and_backward_match(devices_with(lanes).into_iter().filter(|g| g.sums() != Some(GradientSums::CompareAndSwap)).collect());
        }
    }

    fn forward_and_backward_match(devices: Vec<Gpu>) {
        for mut gpu in devices {
            eprintln!("adapter {}", gpu.adapter);
            let bg = [0.1f32, 0.2, 0.3];
            for (count, w, h, seed, filtered) in [(7, 40, 30, 3, false), (3000, 150, 90, 5, false), (7, 40, 30, 3, true), (3000, 150, 90, 5, true)] {
                let (mut g, cam) = scene(count, w, h, seed);
                if filtered {
                    // 3D filters from none to about a typical Gaussian's width.
                    let mut rng = fastrand::Rng::with_seed(seed + 7);
                    for f in &mut g.filter_3d {
                        *f = (rng.f32() * 0.05).powi(2);
                    }
                }
                gpu.upload(&g);
                let total = gpu.step(&cam, bg, Target::None, None).expect("step");
                let img = gpu.read_image().expect("image");
                let tr = gpu.read_transmittance().expect("transmittance");
                let cpu = cpu::render(&g, &cam, bg.map(f64::from), cpu::Rules::GPU);
                let n = (w * h) as usize;
                let mut worst = [0f64; 3];
                for k in 0..n {
                    for (ch, v) in img[k][..3].iter().enumerate() {
                        worst[0] = worst[0].max((*v as f64 - cpu.color[k * 3 + ch]).abs());
                    }
                    worst[1] = worst[1].max((img[k][3] as f64 - cpu.depth[k]).abs() / cpu.depth[k].max(1.0));
                    worst[2] = worst[2].max((tr[k] as f64 - cpu.transmittance[k]).abs());
                }
                eprintln!("{count} Gaussians {w}×{h}{}: {total} intersections, worst colour {:.2e}, depth {:.2e}, T {:.2e}", if filtered { " filtered" } else { "" }, worst[0], worst[1], worst[2]);
                assert!(total > 0);
                assert!(worst[0] < 2e-3 && worst[1] < 2e-3 && worst[2] < 2e-3, "{worst:?}");

                let mut rng = fastrand::Rng::with_seed(seed + 100);
                let d_pixel: Vec<[f32; 4]> = (0..n).map(|_| [rng.f32() - 0.5, rng.f32() - 0.5, rng.f32() - 0.5, (rng.f32() - 0.5) * 0.2]).collect();
                gpu.backward_from(&cam, &d_pixel);
                let grads = gpu.read_grads().expect("gradients");
                let dc: Vec<f64> = d_pixel.iter().flat_map(|p| [p[0] as f64, p[1] as f64, p[2] as f64]).collect();
                let dd: Vec<f64> = d_pixel.iter().map(|p| p[3] as f64).collect();
                let reference = cpu::backward(&g, &cam, bg.map(f64::from), cpu::Rules::GPU, &dc, &dd);
                // Compare each group against its largest magnitude (float atomics sum in any order).
                let groups: [(&str, Vec<(usize, f64)>); 5] = [
                    ("means", (0..g.len()).flat_map(|i| (0..3).map(move |k| (i, k))).map(|(i, k)| (i * PACKED + k, reference.means[i][k])).collect()),
                    ("log_scales", (0..g.len()).flat_map(|i| (0..3).map(move |k| (i, k))).map(|(i, k)| (i * PACKED + 3 + k, reference.log_scales[i][k])).collect()),
                    ("rotations", (0..g.len()).flat_map(|i| (0..4).map(move |k| (i, k))).map(|(i, k)| (i * PACKED + 6 + k, reference.rotations[i][k])).collect()),
                    ("opacity", (0..g.len()).map(|i| (i * PACKED + 10, reference.opacity_logits[i])).collect()),
                    ("colors", (0..g.len()).flat_map(|i| (0..3).map(move |k| (i, k))).map(|(i, k)| (i * PACKED + 11 + k, reference.colors[i][k])).collect()),
                ];
                for (name, values) in groups {
                    let scale = values.iter().map(|(_, r)| r.abs()).fold(0.0, f64::max).max(1e-9);
                    let worst = values.iter().map(|&(slot, r)| (grads[slot] as f64 - r).abs()).fold(0.0, f64::max) / scale;
                    eprintln!("  {name}: worst {worst:.2e} of {scale:.3e}");
                    assert!(worst < 1e-2, "{:?} {name}: {worst}", gpu.sums());
                }
            }
        }
    }

    /// The 3D filter kernel sets what cpu::filter_3d computes, keeps it through download, and clears it at scale 0.
    #[test]
    fn the_3d_filter_matches_the_cpu() {
        let Some(mut gpu) = devices().into_iter().next() else { return };
        let (mut g, cam) = scene(5000, 160, 90, 31);
        // Off to the side, where no camera looks.
        for k in 0..50 {
            g.push(cam.unproject(800.0 + k as f32, 45.0, 3.0), [-3.0; 3], [1.0, 0.0, 0.0, 0.0], 0.0, [0.5; 3]);
        }
        // Three more cameras: one nearer, one beside, one turned away.
        let near = Camera { t: [cam.t[0], cam.t[1], cam.t[2] - 1.5], ..cam };
        let beside = Camera { t: [cam.t[0] + 2.0, cam.t[1], cam.t[2]], ..cam };
        let away = Camera { r: [[-cam.r[0][0], -cam.r[0][1], -cam.r[0][2]], cam.r[1], [-cam.r[2][0], -cam.r[2][1], -cam.r[2][2]]], ..cam };
        let cams = [cam, near, beside, away];
        gpu.upload(&g);
        gpu.update_filter_3d(&cams, 0.2).expect("filter");
        let got = gpu.download_params().expect("download").filter_3d;
        let want = cpu::filter_3d(&g, &cams, 0.2);
        // A centre within a rounding of the 15 % margin may land on either side of it.
        let off = got.iter().zip(&want).filter(|(a, b)| (**a as f64 - **b).abs() > 1e-4 * b.abs().max(1e-12)).count();
        eprintln!("{off} of {} filters differ; {} unseen", g.len(), want.iter().filter(|f| **f == 0.0).count());
        assert!(off <= 2, "{off}");
        assert!(want.contains(&0.0) && want.iter().any(|f| *f > 0.0));
        // The nearer camera samples most finely what it sees: some filters come from it.
        let alone = cpu::filter_3d(&g, &[cam], 0.2);
        assert!(want.iter().zip(&alone).any(|(a, b)| *a > 0.0 && a < b));
        gpu.update_filter_3d(&cams, 0.0).expect("no filter");
        assert!(gpu.download_params().expect("download").filter_3d.iter().all(|f| *f == 0.0));
    }

    #[test]
    fn a_renderer_on_a_plain_device_draws_what_training_renders() {
        let Ok(mut trainer) = Gpu::new() else { return };
        // WGPU_BACKEND picks the viewer's backend (dx12 like the window on Windows).
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
        let Ok(adapter) = block_on(instance.request_adapter(&Default::default())) else { return };
        eprintln!("viewer adapter: {:?}", adapter.get_info().backend);
        let (device, queue) = block_on(adapter.request_device(&Default::default())).expect("device");
        let mut viewer = Gpu::renderer(device.clone(), queue);
        // The guards change what a viewer shows near the eye and the order of overlaps (a_viewers_guards).
        viewer.guards = false;
        let (g, cam) = scene(3000, 150, 90, 5);
        let bg = [0.1, 0.2, 0.3];
        trainer.upload(&g);
        viewer.upload(&g);
        trainer.step(&cam, bg, Target::None, None).expect("trainer");
        viewer.step(&cam, bg, Target::None, None).expect("viewer");
        assert!(viewer.step(&cam, bg, Target::Depth { samples: &[], weight: 1.0 }, None).is_err());
        let (a, b) = (trainer.read_image().expect("image"), viewer.read_image().expect("image"));
        let worst = a.iter().zip(&b).map(|(p, q)| (0..3).map(|c| (p[c] - q[c]).abs()).fold(0f32, f32::max)).fold(0f32, f32::max);
        assert!(worst < 1e-4, "{worst}");
        // Presented into an RGBA8 storage texture.
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: cam.width, height: cam.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        viewer.present(&texture.create_view(&Default::default()));
        let row = (cam.width * 4).next_multiple_of(256);
        let staging = storage(&device, "present readback", (row * cam.height) as u64);
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &staging, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None } },
            wgpu::Extent3d { width: cam.width, height: cam.height, depth_or_array_layers: 1 },
        );
        viewer.queue.submit([enc.finish()]);
        let bytes: Vec<u8> = viewer.read(&staging, (row * cam.height) as usize).expect("present readback");
        let (x, y) = (70usize, 40usize);
        let px = &bytes[y * row as usize + x * 4..][..4];
        let want = b[y * cam.width as usize + x];
        for c in 0..3 {
            assert!((px[c] as f32 / 255.0 - want[c].clamp(0.0, 1.0)).abs() < 1.5 / 255.0, "{px:?} vs {want:?}");
        }
    }

    /// What keyframes see of each Gaussian: the largest weight, the keyframes where it shows, the sectors they looked from
    /// and their finest sampling rate; nothing of one no keyframe shows.
    #[test]
    fn seen_counts_keyframes_sectors_and_the_finest_rate() {
        let Some(mut gpu) = devices().into_iter().next() else { return };
        let (w, h) = (64u32, 48u32);
        // Two splats at the origin's side of +Z: one in the open, one far off to the side.
        let mut g = Gaussians::default();
        let opaque = crate::gaussians::logit(0.8);
        g.push([0.0, 0.0, 0.0], [(0.05f32).ln(); 3], [1.0, 0.0, 0.0, 0.0], opaque, [0.5; 3]);
        g.push([50.0, 0.0, 0.0], [(0.05f32).ln(); 3], [1.0, 0.0, 0.0, 0.0], opaque, [0.5; 3]);
        // Looking at the origin from 2 m towards -Z (sector 8), from 3 m towards +X (sector 4), and again from -Z at 4 m.
        let look = |eye: [f32; 3]| Camera::look(eye, [-eye[0], -eye[1], -eye[2]], [0.0, 1.0, 0.0], 1.0, w, h);
        let cams = [look([0.0, 0.0, -2.0]), look([3.0, 0.0, 0.0]), look([0.0, 0.0, -4.0])];
        gpu.upload(&g);
        gpu.set_mask(w, h, &vec![1.0; (w * h) as usize]).expect("mask");
        let seen = gpu.seen(&cams, [0.0; 3]).expect("seen");
        let focal = cams[0].fx.max(cams[0].fy);
        assert!(seen[0].weight > 0.5 && seen[0].weight <= 0.8, "{:?}", seen[0]);
        assert_eq!((seen[0].views, seen[0].sectors), (3, 1 << 8 | 1 << 4), "{:?}", seen[0]);
        assert!((seen[0].rate - focal / 2.0).abs() < 1e-3 * focal, "{:?}", seen[0]);
        assert_eq!(seen[1], Seen { weight: 0.0, views: 0, sectors: 0, rate: 0.0, beyond: 0.0 });
    }

    /// A viewer's confidence control fades, blurs or hazes a splat by how little the keyframes saw it, never one they saw
    /// well, one of the sky shell or one whose record is unknown.
    #[test]
    fn a_viewer_shows_poorly_seen_splats_as_asked() {
        let Ok(mut gpu) = Gpu::new() else { return };
        gpu.guards = true;
        let cam = Camera::look([0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 1.0, 64, 48);
        let focal = cam.fx.max(cam.fy);
        // Seen well, from where the eye looks now (sector 8: from the splat towards -Z) among others.
        let well = Seen { weight: 0.9, views: 12, sectors: 0b1_1111_0000, rate: focal / 3.0, beyond: 0.0 };
        let poorly = Seen { weight: 0.0005, views: 1, sectors: 1, rate: focal / 3.0, beyond: 0.0 };
        let mut draw = |seen: Seen, sky: bool, confidence: Confidence| {
            let mut g = Gaussians::default();
            g.push([0.0, 0.0, 3.0], [(0.1f32).ln(); 3], [1.0, 0.0, 0.0, 0.0], crate::gaussians::logit(0.7), [1.0, 0.2, 0.2]);
            g.seen[0] = seen;
            g.sky[0] = sky;
            gpu.confidence = confidence;
            gpu.far.horizon = [0.0, 0.0, 1.0];
            gpu.upload(&g);
            crate::train::render(&mut gpu, &cam, [0.0, 0.0, 1.0]).expect("render")
        };
        let off = Confidence::default();
        let fade = Confidence { amount: 1.0, mode: ConfidenceMode::Fade };
        let centre = |img: &[[f32; 4]]| img[24 * 64 + 32];
        let plain = centre(&draw(poorly, false, off));
        // Seen poorly: faded out (only the background left), turned to the horizon's colour, blurred (fainter at the
        // centre, wider: brighter at the edge).
        assert!(centre(&draw(poorly, false, fade))[0] < 0.05 * plain[0], "{plain:?}");
        let hazed = centre(&draw(poorly, false, Confidence { amount: 1.0, mode: ConfidenceMode::Haze }));
        assert!(hazed[0] < 0.1 && hazed[2] > 0.9, "{hazed:?}");
        let blurred = draw(poorly, false, Confidence { amount: 1.0, mode: ConfidenceMode::Blur });
        let sharp = draw(poorly, false, off);
        assert!(centre(&blurred)[0] < centre(&sharp)[0] && blurred[24 * 64 + 26][0] > sharp[24 * 64 + 26][0] + 0.01);
        // Half the amount, about half the fade (the poorly seen splat's trust is about 0).
        let half = centre(&draw(poorly, false, Confidence { amount: 0.5, mode: ConfidenceMode::Fade }))[0];
        assert!(half > 0.4 * plain[0] && half < 0.8 * plain[0], "{half} of {}", plain[0]);
        // Seen well, on the sky shell, or unknown: as trained.
        for (seen, sky) in [(well, false), (poorly, true), (Seen::UNKNOWN, false)] {
            let shown = centre(&draw(seen, sky, fade));
            assert!((shown[0] - plain[0]).abs() < 1e-4, "{seen:?} {sky}: {shown:?} {plain:?}");
        }
        // Up close (ten times finer than the nearest keyframe saw it): even a well seen splat fades.
        let close = Seen { rate: focal / 30.0, ..well };
        assert!(centre(&draw(close, false, fade))[0] < 0.05 * plain[0]);
        // Seen well, but only from the other side (sectors 0 and 1): fades by the three sectors and more between.
        let behind = centre(&draw(Seen { sectors: 0b11, ..well }, false, fade))[0];
        assert!(behind > 0.25 * plain[0] && behind < 0.6 * plain[0], "{behind} of {}", plain[0]);
    }

    /// A viewer's marks (crate::edit): a selected splat is tinted towards the selection's yellow, a hidden or deleted one
    /// is not drawn (the one behind shows through it), and a new upload starts with nothing marked. On the window's own
    /// renderer too ([`Gpu::renderer`]), which binds the marks as the training device's viewer does.
    #[test]
    fn a_viewer_tints_what_is_selected_and_leaves_out_what_is_hidden() {
        let Ok(gpu) = Gpu::new() else { return };
        let mut viewer = Gpu::renderer(gpu.device.clone(), gpu.queue.clone());
        let cam = Camera::look([0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 1.0, 64, 48);
        let mut g = Gaussians::default();
        // Red in front, blue behind it.
        g.push([0.0, 0.0, 2.0], [(0.2f32).ln(); 3], [1.0, 0.0, 0.0, 0.0], crate::gaussians::logit(0.95), [1.0, 0.0, 0.0]);
        g.push([0.0, 0.0, 4.0], [(0.5f32).ln(); 3], [1.0, 0.0, 0.0, 0.0], crate::gaussians::logit(0.95), [0.0, 0.0, 1.0]);
        viewer.upload(&g);
        let centre = |gpu: &mut Gpu| crate::train::render(gpu, &cam, [0.0; 3]).expect("render")[24 * 64 + 32];
        let plain = centre(&mut viewer);
        assert!(plain[0] > 0.8 && plain[2] < 0.1, "red in front: {plain:?}");
        viewer.set_marks(&[1, 0]);
        let selected = centre(&mut viewer);
        assert!(selected[1] > 0.3 && selected[0] > 0.8 && selected[2] < 0.2, "tinted yellow: {selected:?}");
        for left_out in [2u8, 3] {
            viewer.set_marks(&[left_out, 0]);
            let through = centre(&mut viewer);
            assert!(through[0] < 0.05 && through[2] > 0.8, "the blue one through the {left_out}: {through:?}");
        }
        viewer.set_marks(&[2, 2]);
        assert!(centre(&mut viewer).iter().take(3).all(|c| *c < 0.01), "everything left out: the background");
        // Marks for fewer splats than uploaded mark only those; a new upload clears them.
        viewer.set_marks(&[2]);
        assert!(centre(&mut viewer)[2] > 0.8);
        viewer.upload(&g);
        assert!((centre(&mut viewer)[0] - plain[0]).abs() < 1e-4);
    }

    /// A viewer's far field fogs a splat beyond the depth frames' reach towards the horizon's colour, or crops it,
    /// by how far beyond it lies; one within the reach, or on the sky shell, stays as trained.
    #[test]
    fn a_viewer_fogs_or_crops_the_far_field() {
        let Ok(mut gpu) = Gpu::new() else { return };
        gpu.guards = true;
        let cam = Camera::look([0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 1.0, 64, 48);
        let mut draw = |beyond: f32, sky: bool, far: FarField| {
            let mut g = Gaussians::default();
            g.push([0.0, 0.0, 3.0], [(0.1f32).ln(); 3], [1.0, 0.0, 0.0, 0.0], crate::gaussians::logit(0.9), [1.0, 0.0, 0.0]);
            g.seen[0] = Seen { weight: 0.9, views: 12, sectors: 0b1111, rate: cam.fx / 3.0, beyond };
            g.sky[0] = sky;
            gpu.far = far;
            gpu.upload(&g);
            crate::train::render(&mut gpu, &cam, [0.0; 3]).expect("render")[24 * 64 + 32]
        };
        let horizon = [0.2, 0.4, 0.8];
        let fog = FarField { mode: FarMode::Fog, distance: 10.0, horizon };
        let crop = FarField { mode: FarMode::Crop, distance: 10.0, horizon };
        let plain = draw(20.0, false, FarField::default());
        // 20 m beyond at 10 m: 1 − e^−2 of the way to the horizon's colour.
        let fogged = draw(20.0, false, fog);
        let t = 1.0 - (-2.0f32).exp();
        assert!((fogged[2] / plain[0] - t * 0.8).abs() < 0.03 && (fogged[0] / plain[0] - (1.0 - t + t * 0.2)).abs() < 0.03, "{fogged:?} {plain:?}");
        // Cropped past the distance; half way, partly.
        assert!(draw(20.0, false, crop)[0] < 1e-3);
        let half = draw(5.0, false, crop)[0];
        assert!(half > 0.2 * plain[0] && half < 0.8 * plain[0], "{half}");
        // Within the reach, or the sky: as trained.
        for (beyond, sky) in [(0.0, false), (20.0, true)] {
            assert!((draw(beyond, sky, fog)[0] - plain[0]).abs() < 1e-4);
        }
    }

    /// A viewer's guards: a splat near the eye fades, one holding the eye is left out, one longer than the view is
    /// shrunk to it, and overlaps follow the distance from the eye rather than the depth along the view.
    #[test]
    fn a_viewers_guards() {
        let Ok(mut gpu) = Gpu::new() else { return };
        let cam = Camera::look([0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 1.0, 64, 48);
        let splat = |g: &mut Gaussians, at: [f32; 3], scale: [f32; 3], opacity: f32, color: [f32; 3]| g.push(at, scale.map(f32::ln), [1.0, 0.0, 0.0, 0.0], crate::gaussians::logit(opacity), color);
        let mut draw = |g: &Gaussians, guards: bool| {
            gpu.guards = guards;
            gpu.upload(g);
            crate::train::render(&mut gpu, &cam, [0.0; 3]).expect("render")
        };
        let red = |img: &[[f32; 4]]| img.iter().map(|p| p[0] as f64).sum::<f64>();
        // 10 cm from the eye: smoothstep(5 cm, 20 cm) leaves about a quarter of its weight.
        let mut near = Gaussians::default();
        splat(&mut near, [0.0, 0.0, 0.1], [0.01; 3], 0.5, [1.0, 0.0, 0.0]);
        let ratio = red(&draw(&near, true)) / red(&draw(&near, false));
        assert!(ratio > 0.2 && ratio < 0.3, "{ratio}");
        // Half a metre wide around a point 30 cm ahead: the eye is inside.
        let mut around = Gaussians::default();
        splat(&mut around, [0.0, 0.0, 0.3], [0.5; 3], 0.5, [1.0, 0.0, 0.0]);
        assert!(red(&draw(&around, false)) > 100.0);
        assert_eq!(red(&draw(&around, true)), 0.0);
        // 5 m long, 4 m ahead: σ 55 px across a 48-px view, shrunk to reach no further than 48 px.
        let mut long = Gaussians::default();
        splat(&mut long, [0.0, 0.0, 4.0], [5.0, 0.05, 0.05], 0.5, [1.0, 0.0, 0.0]);
        let edge = |img: &[[f32; 4]]| img[24 * 64][0];
        let (open, guarded) = (edge(&draw(&long, false)), edge(&draw(&long, true)));
        assert!(open > 0.15 && guarded < 0.3 * open, "{open} {guarded}");
        // Red straight ahead 2 m away, green off to the side nearer along the view (1.9 m) but farther from the eye
        // (2.25 m): where they overlap, depth order puts green in front, distance order red.
        let mut pair = Gaussians::default();
        splat(&mut pair, [0.0, 0.0, 2.0], [0.5; 3], 0.9, [1.0, 0.0, 0.0]);
        splat(&mut pair, [1.2, 0.0, 1.9], [0.5; 3], 0.9, [0.0, 1.0, 0.0]);
        let at = |img: &[[f32; 4]]| img[24 * 64 + 46];
        let (by_depth, by_distance) = (at(&draw(&pair, false)), at(&draw(&pair, true)));
        assert!(by_depth[1] > by_depth[0] && by_distance[0] > by_distance[1], "{by_depth:?} {by_distance:?}");
    }

    /// A viewer's depth of field: the splat at the focus as it was, one away from it wider and fainter with its light
    /// kept, nothing changed without an aperture; the focus plane tinted while asked.
    #[test]
    fn a_viewer_blurs_splats_by_their_depth() {
        let Ok(mut gpu) = Gpu::new() else { return };
        gpu.guards = true;
        let (w, h) = (160u32, 120u32);
        let cam = Camera::look([0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 0.9, w, h);
        let mut g = Gaussians::default();
        let opaque = crate::gaussians::logit(0.95);
        // Red 2 m ahead on the left, green 12 m ahead on the right: about the same size on screen.
        g.push([-0.35, 0.0, 2.0], [(0.03f32).ln(); 3], [1.0, 0.0, 0.0, 0.0], opaque, [1.0, 0.0, 0.0]);
        g.push([2.1, 0.0, 12.0], [(0.18f32).ln(); 3], [1.0, 0.0, 0.0, 0.0], opaque, [0.0, 1.0, 0.0]);
        gpu.upload(&g);
        let px = |p: [f32; 3]| cam.project(p).map(|(x, y, _)| (y.round() as u32 * w + x.round() as u32) as usize).expect("ahead");
        let (red_at, green_at) = (px([-0.35, 0.0, 2.0]), px([2.1, 0.0, 12.0]));
        let mut draw = |dof: DepthOfField| {
            gpu.dof = dof;
            crate::train::render(&mut gpu, &cam, [0.0; 3]).expect("render")
        };
        let sum = |img: &[[f32; 4]], c: usize| img.iter().map(|p| p[c] as f64).sum::<f64>();
        let sharp = draw(DepthOfField::default());
        let lens = |focus: f32| DepthOfField { aperture: crate::dof::APERTURE_MAX, focus, near: 1.0, far: 1.0, show_plane: false };
        // No aperture: the same picture.
        assert_eq!(draw(DepthOfField { aperture: 0.0, ..lens(2.0) }), sharp);
        // Focused on red: red as it was, green spread out (its peak lower, its light about the same).
        let near = draw(lens(2.0));
        assert!((near[red_at][0] - sharp[red_at][0]).abs() < 1e-3, "{:?} {:?}", near[red_at], sharp[red_at]);
        assert!(near[green_at][1] < 0.6 * sharp[green_at][1], "{:?} {:?}", near[green_at], sharp[green_at]);
        let (lit_sharp, lit_near) = (sharp.iter().filter(|p| p[1] > 0.02).count(), near.iter().filter(|p| p[1] > 0.02).count());
        assert!(2 * lit_near > 3 * lit_sharp, "{lit_near} {lit_sharp}");
        let kept = sum(&near, 1) / sum(&sharp, 1);
        assert!(kept > 0.8 && kept < 1.2, "{kept}");
        // Focused on green: the other way round.
        let far = draw(lens(12.0));
        assert!((far[green_at][1] - sharp[green_at][1]).abs() < 1e-3);
        assert!(far[red_at][0] < 0.6 * sharp[red_at][0]);
        // Only the background blurred: red in front of a far focus stays sharp.
        let only_far = draw(DepthOfField { near: 0.0, ..lens(12.0) });
        assert!((only_far[red_at][0] - sharp[red_at][0]).abs() < 1e-3);
        // The focus plane, tinted while it is set.
        let plane = draw(DepthOfField { show_plane: true, ..lens(2.0) });
        assert!(plane[red_at][1] > 0.3 && plane[red_at][0] < near[red_at][0], "{:?}", plane[red_at]);
        // A frame without the depth of field after one with it: as before.
        assert_eq!(draw(DepthOfField::default()), sharp);
    }

    /// The auto focus's probe: the depth where what is under the pixel hides half of what is behind it, the faint veil
    /// in front of it passed, far away where nothing hides half; not changed by the blur it sets.
    #[test]
    fn the_focus_probe_finds_the_dominant_surface_under_a_pixel() {
        let Ok(mut gpu) = Gpu::new() else { return };
        gpu.guards = true;
        let (w, h) = (96u32, 72u32);
        let cam = Camera::look([0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 0.9, w, h);
        let mut g = Gaussians::default();
        // A wall of opaque splats 6 m ahead over the left half, a faint veil 1.5 m ahead over the middle, a small far
        // splat at 40 m on the right.
        for y in -12..=12 {
            for x in -24..=0 {
                g.push([x as f32 * 0.2, y as f32 * 0.2, 6.0], [(0.12f32).ln(), (0.12f32).ln(), (0.02f32).ln()], [1.0, 0.0, 0.0, 0.0], crate::gaussians::logit(0.9), [0.6; 3]);
            }
        }
        g.push([0.0, 0.0, 1.5], [0.3, 0.3, 0.01].map(f32::ln), [1.0, 0.0, 0.0, 0.0], crate::gaussians::logit(0.15), [0.9; 3]);
        g.push([8.0, 0.0, 40.0], [1.0, 1.0, 0.1].map(f32::ln), [1.0, 0.0, 0.0, 0.0], crate::gaussians::logit(0.95), [0.2, 0.3, 0.9]);
        gpu.upload(&g);
        let probe = |gpu: &mut Gpu, at: [f32; 3], dof: DepthOfField| {
            let (x, y, _) = cam.project(at).expect("ahead");
            gpu.probe = Some([x, y]);
            gpu.dof = dof;
            gpu.step(&cam, [0.0; 3], Target::None, None).expect("render");
            gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
            let f = gpu.probed().expect("a probe came back");
            assert!(gpu.probed().is_none(), "each probe is taken once");
            f
        };
        let step = (crate::dof::FARTHEST.ln() - crate::dof::NEAREST.ln()) / crate::dof::BINS as f32;
        let near = |f: f32, z: f32| (f.ln() - z.ln()).abs() <= 1.5 * step;
        // Through the veil at the wall (and in front of it, where there is only the wall).
        let f = probe(&mut gpu, [-0.05, 0.0, 6.0], DepthOfField::default());
        assert!(near(f, 6.0), "{f}");
        let f = probe(&mut gpu, [-2.0, 1.0, 6.0], DepthOfField::default());
        assert!(near(f, 6.0), "{f}");
        // The far splat, and nothing (the background): far away.
        let f = probe(&mut gpu, [8.0, 0.0, 40.0], DepthOfField::default());
        assert!(near(f, 40.0), "{f}");
        assert_eq!(probe(&mut gpu, [3.0, -2.5, 10.0], DepthOfField::default()), crate::dof::FAR_FOCUS);
        // The blur does not move it: a strong depth of field focused elsewhere still finds the wall.
        let blurred = DepthOfField { aperture: crate::dof::APERTURE_MAX, focus: 40.0, near: 1.0, far: 1.0, show_plane: false };
        let f = probe(&mut gpu, [-0.05, 0.0, 6.0], blurred);
        assert!(near(f, 6.0), "{f}");
        gpu.probe = None;
    }

    /// The depth of field's cost on a real capture: `ORBITERS_DOF_PLY=<splats.ply> cargo test -p orbiters-reconstruct
    /// --lib gpu::tests::depth_of_field_frame_time -- --ignored --nocapture`. Frames of 1600×900 from the splats' median
    /// looking four ways, each waited for, without and with a depth of field and with the auto focus's probe.
    #[test]
    #[ignore]
    fn depth_of_field_frame_time() {
        let Ok(path) = std::env::var("ORBITERS_DOF_PLY") else { return };
        let g = crate::ply::read(std::path::Path::new(&path)).expect("ply");
        let mut gpu = Gpu::new().expect("gpu");
        gpu.guards = true;
        gpu.upload(&g);
        let median = |k: usize| {
            let mut v: Vec<f32> = (0..g.len()).map(|i| g.means[i][k]).collect();
            v.sort_by(f32::total_cmp);
            v[v.len() / 2]
        };
        let eye = [median(0), median(1), median(2)];
        println!("{} splats, eye {eye:?}", g.len());
        let modes = [
            ("off", DepthOfField::default(), None),
            ("aperture 40 %", DepthOfField { aperture: 0.4 * crate::dof::APERTURE_MAX, focus: 3.0, near: 1.0, far: 1.0, show_plane: false }, None),
            ("aperture 100 %", DepthOfField { aperture: crate::dof::APERTURE_MAX, focus: 3.0, near: 1.0, far: 1.0, show_plane: false }, None),
            ("aperture 100 %, near focus 0.8 m", DepthOfField { aperture: crate::dof::APERTURE_MAX, focus: 0.8, near: 1.0, far: 1.0, show_plane: false }, None),
            ("aperture 40 % + probe", DepthOfField { aperture: 0.4 * crate::dof::APERTURE_MAX, focus: 3.0, near: 1.0, far: 1.0, show_plane: false }, Some([800.0, 450.0])),
            // Again, after the first round warmed the GPU up.
            ("off", DepthOfField::default(), None),
        ];
        for (name, dof, probe) in modes {
            gpu.dof = dof;
            gpu.probe = probe;
            let mut total = std::time::Duration::ZERO;
            let mut entries = 0u64;
            for k in 0..4 {
                let a = k as f32 * std::f32::consts::FRAC_PI_2;
                let cam = Camera::look(eye, [a.sin(), 0.0, a.cos()], [0.0, 1.0, 0.0], 1.0, 1600, 900);
                for i in 0..23 {
                    let start = std::time::Instant::now();
                    let n = gpu.step(&cam, [0.0; 3], Target::None, None).expect("step");
                    gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
                    if probe.is_some() {
                        let _ = gpu.probed();
                    }
                    if i >= 3 {
                        total += start.elapsed();
                        entries += n as u64;
                    }
                }
            }
            println!("{name}: {:.2} ms a frame, {} tile entries", total.as_secs_f64() * 1000.0 / 80.0, entries / 80);
        }
    }

    #[test]
    fn pipelined_steps_render_what_waiting_steps_render_at_every_size() {
        let Ok(mut gpu) = Gpu::new() else { return };
        let (g, full) = scene(20_000, 640, 360, 21);
        let half = crate::train::scaled(&full, 2);
        gpu.upload(&g);
        let bg = [0.0; 3];
        // The first step at a size waits for its count; later ones at that size do not.
        let mut images = Vec::new();
        for cam in [&full, &half, &half, &full, &half, &full] {
            gpu.step(cam, bg, Target::None, None).expect("step");
            images.push(gpu.read_image().expect("image"));
        }
        assert_eq!(images[1], images[2], "half resolution, pipelined");
        assert_eq!(images[0], images[3], "full resolution after a size change, pipelined");
        assert_eq!(images[1], images[4]);
        assert_eq!(images[0], images[5]);
        gpu.wait_readbacks();
        assert_eq!(gpu.overflows, 0);
    }

    /// f64 reference of the colour loss (1 − λ)·mean|x − y| + λ·(1 − mean SSIM) over scene pixels and channels.
    fn ssim_loss(x: &[f64], y: &[f64], mask: &[f32], w: usize, h: usize, lambda: f64) -> f64 {
        let g: Vec<f64> = (0..11).map(|i| (-(((i as f64) - 5.0).powi(2)) / 4.5).exp()).collect();
        let gs: f64 = g.iter().sum();
        let g: Vec<f64> = g.iter().map(|v| v / gs).collect();
        let n = mask.iter().filter(|m| **m > 0.5).count() as f64 * 3.0;
        let at = |img: &[f64], px: i64, py: i64, ch: usize| -> f64 {
            if px < 0 || py < 0 || px >= w as i64 || py >= h as i64 || mask[py as usize * w + px as usize] < 0.5 {
                0.0
            } else {
                img[(py as usize * w + px as usize) * 3 + ch]
            }
        };
        let (mut l1, mut s) = (0.0, 0.0);
        for py in 0..h {
            for px in 0..w {
                if mask[py * w + px] < 0.5 {
                    continue;
                }
                for ch in 0..3 {
                    l1 += (x[(py * w + px) * 3 + ch] - y[(py * w + px) * 3 + ch]).abs();
                    let (mut mx, mut my, mut xx, mut yy, mut xy) = (0.0, 0.0, 0.0, 0.0, 0.0);
                    for dy in 0..11 {
                        for dx in 0..11 {
                            let wgt = g[dy] * g[dx];
                            let (qx, qy) = (px as i64 + dx as i64 - 5, py as i64 + dy as i64 - 5);
                            let (a, b) = (at(x, qx, qy, ch), at(y, qx, qy, ch));
                            mx += wgt * a;
                            my += wgt * b;
                            xx += wgt * a * a;
                            yy += wgt * b * b;
                            xy += wgt * a * b;
                        }
                    }
                    let (c1, c2) = (0.0001, 0.0009);
                    s += (2.0 * mx * my + c1) * (2.0 * (xy - mx * my) + c2) / ((mx * mx + my * my + c1) * ((xx - mx * mx) + (yy - my * my) + c2));
                }
            }
        }
        (1.0 - lambda) * l1 / n + lambda * (1.0 - s / n)
    }

    #[test]
    fn ssim_gradient_matches_finite_differences() {
        for mut gpu in devices() {
            let (w, h) = (40u32, 30u32);
            let (g, cam) = scene(300, w, h, 11);
            let mut rng = fastrand::Rng::with_seed(12);
            let target: Vec<u32> = (0..w * h).map(|_| rng.u32(..) | 0xff00_0000).collect();
            // A masked band like the capture strip.
            let mask: Vec<f32> = (0..w * h).map(|p| if (5..9).contains(&(p / w)) && (8..30).contains(&(p % w)) { 0.0 } else { 1.0 }).collect();
            gpu.upload(&g);
            gpu.set_mask(w, h, &mask).expect("mask");
            let scene_pixels = mask.iter().filter(|m| **m > 0.5).count() as f32;
            let y: Vec<f64> = target.iter().flat_map(|t| (0..3).map(move |c| ((t >> (8 * c)) & 255) as f64 / 255.0)).collect();
            for lambda in [1.0f32, 0.2] {
                gpu.step(&cam, [0.1, 0.2, 0.3], Target::Color { rgba: &target, scene_pixels, weight: 1.0, ssim: lambda, robust: 0.0 }, None).expect("step");
                let img = gpu.read_image().expect("image");
                let dp = gpu.read_d_pixel().expect("gradient");
                let x: Vec<f64> = img.iter().flat_map(|p| (0..3).map(move |c| p[c] as f64)).collect();
                let mut worst = 0f64;
                let mut scale = 0f64;
                for k in 0..40 {
                    let p = (k * 397 + 13) % (w * h) as usize;
                    let ch = k % 3;
                    if mask[p] < 0.5 || (x[p * 3 + ch] - y[p * 3 + ch]).abs() < 1e-3 {
                        continue;
                    }
                    let e = 1e-4;
                    let mut xp = x.clone();
                    xp[p * 3 + ch] += e;
                    let mut xm = x.clone();
                    xm[p * 3 + ch] -= e;
                    let fd = (ssim_loss(&xp, &y, &mask, w as usize, h as usize, lambda as f64) - ssim_loss(&xm, &y, &mask, w as usize, h as usize, lambda as f64)) / (2.0 * e);
                    worst = worst.max((dp[p][ch] as f64 - fd).abs());
                    scale = scale.max(fd.abs());
                }
                eprintln!("{:?} λ {lambda}: worst {worst:.3e} of {scale:.3e}", gpu.sums());
                assert!(worst < 2e-3 * scale.max(1e-6), "λ {lambda}: {worst} of {scale}");
            }
        }
    }

    /// A training device (the scatter by subgroup ballots, large Gaussians split off the projection and the tile entries)
    /// sorts the same entries as a viewer (neither), bit for bit.
    #[test]
    fn training_and_viewers_sort_alike() {
        let Ok(mut gpu) = Gpu::new() else { return };
        let mut viewer = Gpu::renderer(gpu.device.clone(), gpu.queue.clone());
        // Without its guards (they sort by distance from the eye, not depth along the view).
        viewer.guards = false;
        let (mut g, cam) = scene(400_000, 1280, 720, 9);
        // And some that fill much of the frame (large tile boxes: the training projection's list).
        let mut rng = fastrand::Rng::with_seed(10);
        for _ in 0..50 {
            let p = cam.unproject(rng.f32() * 1280.0, rng.f32() * 720.0, 1.0 + rng.f32() * 2.0);
            g.push(p, [-0.8 + rng.f32(), -0.8 + rng.f32(), -3.0], [1.0, rng.f32() - 0.5, rng.f32() - 0.5, 0.0], 1.0, [0.5; 3]);
        }
        let mut sorted = Vec::new();
        for device in [&mut gpu, &mut viewer] {
            device.upload(&g);
            device.step(&cam, [0.0; 3], Target::None, None).expect("step");
            let total = device.read::<u32>(&device.total_out, 2).expect("total")[1] as usize;
            let s = device.sort.as_ref().expect("sort");
            let keys: Vec<u32> = device.read(&s.keys[0], total).expect("keys");
            let values: Vec<u32> = device.read(&s.values[0], total).expect("values");
            sorted.push((keys, values));
        }
        assert!(sorted[0].0.len() > 1_000_000);
        assert!(sorted[0] == sorted[1], "training and viewers sorted differently");
    }

    #[test]
    fn large_scenes_sort_every_tile_entry() {
        let gpu = Gpu::new();
        let Ok(mut gpu) = gpu else {
            eprintln!("skipped: {}", gpu.err().expect("error"));
            return;
        };
        let (g, cam) = scene(400_000, 1280, 720, 9);
        gpu.upload(&g);
        // Once waiting for the count, once pipelined (sorted over the count the GPU wrote).
        for pass in 0..2 {
            let start = std::time::Instant::now();
            gpu.step(&cam, [0.0; 3], Target::None, None).expect("step");
            let total = gpu.read::<u32>(&gpu.total_out, 2).expect("total")[1] as usize;
            eprintln!("pass {pass}: {total} intersections in {:?}", start.elapsed());
            assert!(total > 1_000_000, "{total}");
            let s = gpu.sort.as_ref().expect("sort");
            let keys: Vec<u32> = gpu.read(&s.keys[0], total).expect("keys");
            let values: Vec<u32> = gpu.read(&s.values[0], total).expect("values");
            let proj: Vec<f32> = gpu.read(&gpu.splats.as_ref().expect("splats").proj, g.len() * 16).expect("projected");
            let tiles_x = cam.width.div_ceil(cpu::TILE);
            let depth_bits = depth_bits(cam.width, cam.height);
            let counted: Vec<u32> = gpu.read(&gpu.splats.as_ref().expect("splats").counts, g.len()).expect("counts");
            let mut per_gaussian = vec![0u32; g.len()];
            for j in 0..total {
                assert!(j == 0 || keys[j - 1] <= keys[j], "unsorted at {j}");
                assert_ne!(keys[j], u32::MAX, "sentinel at {j}: counting and emitting disagree");
                let i = values[j] as usize;
                let p = &proj[i * 16..i * 16 + 16];
                let tile = keys[j] >> depth_bits;
                let (tx, ty) = (tile % tiles_x, tile / tiles_x);
                assert!(tx >= p[11].to_bits() && tx < p[13].to_bits() && ty >= p[12].to_bits() && ty < p[14].to_bits(), "entry {j} outside its Gaussian's tiles");
                per_gaussian[i] += 1;
            }
            let (mut exact, mut boxes) = (0u64, 0u64);
            for (i, &c) in per_gaussian.iter().enumerate() {
                let p = &proj[i * 16..i * 16 + 16];
                let area = if p[15] == 0.0 { 0 } else { (p[13].to_bits() - p[11].to_bits()) * (p[14].to_bits() - p[12].to_bits()) };
                assert_eq!(c, counted[i], "Gaussian {i}");
                assert!(c <= area, "Gaussian {i}: {c} entries in a box of {area} tiles");
                exact += c as u64;
                boxes += area as u64;
            }
            eprintln!("  {exact} entries, {boxes} in the tile boxes");
        }
        gpu.wait_readbacks();
        assert_eq!(gpu.overflows, 0);
    }

    /// Birth statistics (shaders/common.wgsl stats_mode) ride in loss slots a step does not use: a colour step's and a
    /// depth step's gradients are those of a step without them, and what they add up to is what the render says: over
    /// every Gaussian, the colour error each blended sums to Σ error × (1 − T) over the scene pixels, and the weight at
    /// depth samples to Σ (1 − T) over the samples' pixels.
    #[test]
    fn birth_statistics_leave_every_gradient_as_it_was_and_add_up() {
        let Some(mut gpu) = devices().into_iter().next() else { return };
        let (mut g, cam) = scene(3000, 96, 64, 71);
        // A third of the set on one plane at depth 3, whose samples measure it.
        for i in (0..g.len()).step_by(3) {
            g.means[i] = cam.unproject((i % 96) as f32, ((i / 96) % 64) as f32, 3.0);
        }
        let mut rng = fastrand::Rng::with_seed(72);
        let target: Vec<u32> = (0..cam.width * cam.height).map(|_| rng.u32(..) | 0xff00_0000).collect();
        let mask: Vec<f32> = (0..cam.width * cam.height).map(|p| if p % 7 == 0 { 0.0 } else { 1.0 }).collect();
        gpu.set_mask(cam.width, cam.height, &mask).expect("mask");
        let px = mask.iter().sum::<f32>();
        let samples: Vec<[f32; 4]> = (0..400).map(|k| [(k % 40) as f32 * 2.0 + 5.0, (k / 40) as f32 * 5.0 + 3.0, 3.0, 1.0]).collect();
        for ssim in [0.0, 0.2] {
            for depth in [false, true] {
                let mut grads = Vec::new();
                for stats in [0, BIRTH_STATS_COLOR | BIRTH_STATS_DEPTH] {
                    gpu.birth_stats = stats;
                    gpu.upload(&g);
                    let t = if depth { Target::Depth { samples: &samples, weight: 0.3 } } else { Target::Color { rgba: &target, scene_pixels: px, weight: 1.0, ssim, robust: 0.0 } };
                    gpu.step(&cam, [0.0; 3], t, None).expect("step");
                    grads.push(gpu.read_grads().expect("gradients"));
                }
                let worst = grads[0].iter().zip(&grads[1]).map(|(a, b)| (a - b).abs() / a.abs().max(1e-6)).fold(0.0f32, f32::max);
                assert!(worst < 1e-3, "depth {depth} (D-SSIM {ssim}): gradients off by {worst} with birth statistics");
                // What the render says: its colour and each pixel's transmittance after the last splat.
                let image = gpu.read_image().expect("image");
                let left = gpu.read_transmittance().expect("transmittance");
                let (_, _, _, stats) = gpu.download().expect("download");
                if !depth {
                    let expected: f64 = (0..image.len())
                        .filter(|&p| mask[p] > 0.5)
                        .map(|p| (0..3).map(|c| (image[p][c] as f64 - crate::dataset::channel(target[p], c) as f64).abs()).sum::<f64>() / 3.0 * (1.0 - left[p] as f64))
                        .sum();
                    let got: f64 = stats.iter().map(|s| s[3] as f64).sum();
                    assert!((got - expected).abs() < 2e-3 * expected, "colour error blended: {got} on the device, {expected} from the render");
                    assert!(stats.iter().filter(|s| s[1] == 1.0).count() > 1000 && stats.iter().all(|s| s[1] <= 1.0 && s[0] >= 0.0));
                    assert!(stats.iter().any(|s| s[0] > 0.0));
                } else {
                    let pixels: std::collections::BTreeSet<usize> = samples.iter().map(|s| (s[1] as u32 * cam.width + s[0] as u32) as usize).collect();
                    let expected: f64 = pixels.iter().map(|&p| 1.0 - left[p] as f64).sum();
                    let got: f64 = stats.iter().map(|s| s[5] as f64).sum();
                    assert!((got - expected).abs() < 2e-3 * expected, "weight at depth samples: {got} on the device, {expected} from the render");
                    // The plane's Gaussians sit on the measured depth; most others do not.
                    let on: Vec<f32> = (0..g.len()).step_by(3).filter(|&i| stats[i][5] > 0.01).map(|i| stats[i][4] / stats[i][5]).collect();
                    assert!(on.len() > 20 && on.iter().all(|r| *r < 1e-3), "{} on the plane, worst {:?}", on.len(), on.iter().cloned().fold(0.0f32, f32::max));
                    let off = (0..g.len()).filter(|i| i % 3 != 0 && stats[*i][5] > 0.01).filter(|&i| stats[i][4] / stats[i][5] > 0.05).count();
                    assert!(off > 20, "{off} off the plane");
                }
            }
        }
        gpu.birth_stats = 0;
    }

    /// The long-axis split and births at measured depth on the device: what the CPU reference makes of the same plan.
    #[test]
    fn long_axis_splits_and_births_on_the_device_match_the_cpu() {
        let Some(mut gpu) = devices().into_iter().next() else { return };
        let (mut g, _) = scene(3000, 64, 48, 13);
        for i in (0..g.len()).step_by(25) {
            g.opacity_logits[i] = crate::gaussians::logit(0.001);
        }
        let n = g.len();
        let ones = vec![1.0f32; n * PACKED];
        gpu.upload_with_moments(&g, Some((&ones, &ones)));
        let weights = gpu.relocation_weights(1e9).expect("weights");
        let spawns: Vec<[f32; 14]> = (0..200).map(|k| [k as f32 * 0.01, 1.0, 2.0, -4.0, -4.5, -6.0, 0.9, 0.1, -0.3, 0.2, -2.2, 0.1, 0.5, 0.9]).collect();
        let births = crate::train::Births { split: crate::train::Split::LongAxis, spawns: &spawns, spawn_share: 0.25, ..Default::default() };
        let plan = crate::train::plan_relocation_by(&weights, n + n / 20, false, &births, &mut fastrand::Rng::with_seed(14));
        let split = plan.copies.iter().filter(|c| c[5] == 1).count();
        assert!(split > 50 && !plan.spawns.is_empty(), "{split} split, {} born", plan.spawns.len());
        let mut expected = g.clone();
        let (mut m1, mut m2) = (ones.clone(), ones.clone());
        crate::train::apply_relocation(&mut expected, &mut m1, &mut m2, &plan);
        gpu.apply_relocation(&plan).expect("apply");
        let (got, got_m1, got_m2, _) = gpu.download().expect("download");
        assert_eq!(got.len(), expected.len());
        let (a, b) = (got.pack(), expected.pack());
        for (k, (x, y)) in a.iter().zip(&b).enumerate() {
            let same = x == y || (x - y).abs() <= 1e-5 * y.abs().max(1.0);
            assert!(same, "Gaussian {} float {}: {x} on the device, {y} on the CPU", k / PACKED, k % PACKED);
        }
        assert_eq!(got_m1, m1);
        assert_eq!(got_m2, m2);
    }

    /// SGLD noise (3DGS-MCMC) moves the nearly transparent Gaussians' means by about Σ·η times its scale, and the
    /// opaque ones' not at all.
    #[test]
    fn sgld_noise_moves_only_the_nearly_transparent() {
        let Some(mut gpu) = devices().into_iter().next() else { return };
        let (mut g, cam) = scene(2000, 96, 64, 81);
        for i in 0..g.len() {
            g.opacity_logits[i] = crate::gaussians::logit(if i % 2 == 0 { 0.002 } else { 0.6 });
            g.log_scales[i] = [-3.0; 3];
        }
        let target: Vec<u32> = vec![0xff40_8020; (cam.width * cam.height) as usize];
        gpu.set_mask(cam.width, cam.height, &vec![1.0; (cam.width * cam.height) as usize]).expect("mask");
        // Positions only (the other rates 0), so with the noise off the step is the same both times up to the order of
        // the gradient sums.
        let lr = LearningRates { means: 1e-4, log_scales: 0.0, rotations: 0.0, opacity: 0.0, colors: 0.0 };
        let px = (cam.width * cam.height) as f32;
        let mut after = Vec::new();
        for noise in [0.0, 5e5] {
            gpu.noise = noise;
            gpu.upload(&g);
            gpu.step = 0;
            gpu.step(&cam, [0.0; 3], Target::Color { rgba: &target, scene_pixels: px, weight: 1.0, ssim: 0.0, robust: 0.0 }, Some(lr)).expect("step");
            after.push(gpu.download_params().expect("download"));
        }
        gpu.noise = 0.0;
        let moved = |i: usize| (0..3).map(|k| (after[1].means[i][k] - after[0].means[i][k]).powi(2)).sum::<f32>().sqrt();
        // σ² = e^−6 ≈ 2.5e-3 m², the gate at opacity 0.002 is σ(0.3) ≈ 0.57 and the scale 1e-4 × 5e5 = 50: about
        // 0.07 m a standard normal, 0.11 m for the length of three.
        let transparent: Vec<f32> = (0..g.len()).step_by(2).map(moved).collect();
        let mean = transparent.iter().sum::<f32>() / transparent.len() as f32;
        assert!(mean > 0.07 && mean < 0.16, "the transparent ones moved {mean} m on average");
        let opaque = (1..g.len()).step_by(2).map(moved).fold(0.0f32, f32::max);
        assert!(opaque < 1e-5, "an opaque one moved {opaque} m");
        // With `noise_visible`, transparent Gaussians the step does not show (behind the camera) stay where they are.
        let mut behind = g.clone();
        let back = [-cam.r[2][0], -cam.r[2][1], -cam.r[2][2]];
        for i in (0..behind.len()).step_by(4) {
            let p = behind.means[i];
            behind.means[i] = [p[0] + 12.0 * back[0], p[1] + 12.0 * back[1], p[2] + 12.0 * back[2]];
        }
        let mut runs = Vec::new();
        for visible in [false, true] {
            gpu.noise = 5e5;
            gpu.noise_visible = visible;
            gpu.upload(&behind);
            gpu.step = 0;
            gpu.step(&cam, [0.0; 3], Target::Color { rgba: &target, scene_pixels: px, weight: 1.0, ssim: 0.0, robust: 0.0 }, Some(lr)).expect("step");
            runs.push(gpu.download_params().expect("download"));
        }
        gpu.noise = 0.0;
        gpu.noise_visible = false;
        let off = |r: &Gaussians, i: usize| (0..3).map(|k| (r.means[i][k] - behind.means[i][k]).powi(2)).sum::<f32>().sqrt();
        let unseen: Vec<usize> = (0..behind.len()).step_by(4).collect();
        assert!(unseen.iter().map(|&i| off(&runs[0], i)).sum::<f32>() / unseen.len() as f32 > 0.05, "all Gaussians take the noise by default");
        assert!(unseen.iter().all(|&i| off(&runs[1], i) == 0.0), "an unseen Gaussian moved with noise_visible");
    }

    #[test]
    fn relocation_on_the_device_matches_the_cpu() {
        let Some(mut gpu) = devices().into_iter().next() else { return };
        let (mut g, _) = scene(3000, 64, 48, 11);
        // Faded ones, broken ones (a mean, a colour) and opaque sky ones; growth to twice the set outgrows the buffers.
        for i in (0..g.len()).step_by(40) {
            g.opacity_logits[i] = crate::gaussians::logit(0.001);
        }
        g.means[7][0] = f32::NAN;
        g.colors[11][1] = f32::INFINITY;
        for i in (3..g.len()).step_by(13) {
            g.means[i] = [500.0, 1.0, 2.0];
            g.sky[i] = true;
        }
        let n = g.len();
        let ones = vec![1.0f32; n * PACKED];
        gpu.upload_with_moments(&g, Some((&ones, &ones)));
        let weights = gpu.relocation_weights(1e9).expect("weights");
        let expected_weights = crate::train::weights(&g, &vec![false; n]);
        assert!(expected_weights[7] == -1.0 && expected_weights[11] == -1.0);
        for (i, (a, b)) in weights.iter().zip(&expected_weights).enumerate() {
            assert!((a - b).abs() < 1e-5, "Gaussian {i}: weight {a} on the device, {b} on the CPU");
        }
        let plan = crate::train::plan_relocation(&weights, 2 * n, false, &mut fastrand::Rng::with_seed(3));
        assert!(!plan.adjust.is_empty() && plan.len == 2 * n, "{} adjusted, {} Gaussians", plan.adjust.len(), plan.len);
        let mut expected = g.clone();
        let (mut m1, mut m2) = (ones.clone(), ones.clone());
        crate::train::apply_relocation(&mut expected, &mut m1, &mut m2, &plan);
        gpu.apply_relocation(&plan).expect("apply");
        let (got, got_m1, got_m2, stats) = gpu.download().expect("download");
        assert_eq!(got.len(), expected.len());
        let (a, b) = (got.pack(), expected.pack());
        for (k, (x, y)) in a.iter().zip(&b).enumerate() {
            let same = x == y || (x.is_nan() && y.is_nan()) || (x - y).abs() <= 1e-5 * y.abs().max(1.0);
            assert!(same, "Gaussian {} float {}: {x} on the device, {y} on the CPU", k / PACKED, k % PACKED);
        }
        assert_eq!(got_m1, m1);
        assert_eq!(got_m2, m2);
        assert!(stats.iter().all(|s| *s == [0.0; 8]));
    }

    #[test]
    fn device_scores_match_the_cpu_psnr() {
        let Some(mut gpu) = devices().into_iter().next() else { return };
        let (w, h) = (150u32, 90u32);
        let (g, cam) = scene(3000, w, h, 21);
        gpu.upload(&g);
        let mut rng = fastrand::Rng::with_seed(22);
        let target: Vec<u32> = (0..w * h).map(|_| rng.u32(..) | 0xff00_0000).collect();
        // Part of the frame is not scene (a strip).
        let mask: Vec<f32> = (0..w * h).map(|p| if p / w < 10 { 0.0 } else { 1.0 }).collect();
        gpu.set_mask(w, h, &mask).expect("mask");
        let bg = [0.1, 0.2, 0.3];
        let img = crate::train::render(&mut gpu, &cam, bg).expect("render");
        let cpu = crate::train::psnr(&img, &target, &mask);
        let device = gpu.score(&cam, bg, &target).expect("score");
        assert!((cpu - device).abs() < 1e-3, "PSNR {cpu} on the CPU, {device} on the device");
        // No scene pixel is no score, not 100 dB; a size without a mask is an error, not an empty loss.
        gpu.set_mask(w, h, &vec![0.0; (w * h) as usize]).expect("mask");
        assert!(gpu.score(&cam, bg, &target).is_err());
        let small = cam.scaled(2);
        assert!(gpu.step(&small, bg, Target::Color { rgba: &target[..(small.width * small.height) as usize], scene_pixels: 1.0, weight: 1.0, ssim: 0.0, robust: 0.0 }, None).is_err());
        assert!(gpu.step(&cam, bg, Target::Color { rgba: &target[..10], scene_pixels: 1.0, weight: 1.0, ssim: 0.0, robust: 0.0 }, None).is_err());
    }

    /// On a small image (whose keys had 25 depth bits), a splat beyond the far plane stays behind a near one, in odd
    /// tiles too (where its key used to turn into its own tile's nearest entry).
    #[test]
    fn a_splat_beyond_the_far_plane_draws_behind() {
        let Some(mut gpu) = devices().into_iter().next() else { return };
        let (w, h) = (240u32, 110u32);
        let cam = Camera::look([0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 1.0, w, h);
        let opaque = crate::gaussians::logit(0.99);
        let mut g = Gaussians::default();
        // A red disc 2 km away filling the view, and a green one 3 m ahead.
        g.push(cam.unproject(120.0, 55.0, 2000.0), [6.5, 6.5, 0.0], [1.0, 0.0, 0.0, 0.0], opaque, [1.0, 0.0, 0.0]);
        g.push(cam.unproject(120.0, 55.0, 3.0), [0.0, 0.0, -3.0], [1.0, 0.0, 0.0, 0.0], opaque, [0.0, 1.0, 0.0]);
        gpu.upload(&g);
        gpu.step(&cam, [0.0; 3], Target::None, None).expect("step");
        let img = gpu.read_image().expect("image");
        // Pixel (136, 56) lies in tile 53 (odd).
        for (x, y) in [(120u32, 55u32), (136, 56)] {
            let p = img[(y * w + x) as usize];
            assert!(p[1] > 0.8 && p[0] < 0.2, "({x}, {y}): {p:?}");
        }
    }

    /// The sky map shows where the splats leave the view open, and colour steps teach it the colour seen there (each
    /// pixel's gradient goes to the texels it looked up), leaving the colour behind the splats alone: with every kind
    /// of gradient sums.
    #[test]
    fn the_sky_map_shows_behind_the_splats_and_learns() {
        for mut gpu in devices() {
            let sums = gpu.sums();
            let (w, h) = (64u32, 48u32);
            let cam = Camera::look([0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 1.0, w, h);
            let mut g = Gaussians::default();
            // An opaque green disc in the middle; around it, the sky.
            g.push(cam.unproject(32.0, 24.0, 3.0), [-1.3, -1.3, -4.0], [1.0, 0.0, 0.0, 0.0], crate::gaussians::logit(0.99), [0.0, 1.0, 0.0]);
            gpu.upload(&g);
            gpu.set_mask(w, h, &vec![1.0; (w * h) as usize]).expect("mask");
            let width = 32u32;
            gpu.set_sky(Some((width, &vec![[0.2, 0.3, 0.9]; (width * width / 2) as usize]))).expect("sky");
            gpu.step(&cam, [0.0; 3], Target::None, None).expect("step");
            let img = gpu.read_image().expect("image");
            assert!((img[0][2] - 0.9).abs() < 1e-3 && (img[0][0] - 0.2).abs() < 1e-3, "{sums:?}: the sky behind an open pixel: {:?}", img[0]);
            assert!(img[(24 * w + 32) as usize][1] > 0.9, "{sums:?}: the disc in front: {:?}", img[(24 * w + 32) as usize]);
            // An orange sky to learn; the disc green as it is (colour steps with the splats' learning rates at 0).
            let orange = 0xff00_80ffu32;
            let target: Vec<u32> = img.iter().map(|p| if p[1] > 0.9 { 0xff00_ff00 } else { orange }).collect();
            let still = LearningRates { means: 0.0, log_scales: 0.0, rotations: 0.0, opacity: 0.0, colors: 0.0 };
            gpu.sky_lr = 0.02;
            for _ in 0..200 {
                gpu.step(&cam, [0.0; 3], Target::Color { rgba: &target, scene_pixels: (w * h) as f32, weight: 1.0, ssim: 0.0, robust: 0.0 }, Some(still)).expect("step");
            }
            gpu.step(&cam, [0.0; 3], Target::None, None).expect("step");
            let img = gpu.read_image().expect("image");
            let corner = img[0];
            assert!((corner[0] - 1.0).abs() < 0.05 && (corner[1] - 0.5).abs() < 0.05 && corner[2] < 0.05, "{sums:?}: learnt sky: {corner:?}");
            assert!(img[(24 * w + 32) as usize][1] > 0.9, "{sums:?}: the disc stays green");
            let (_, texels) = gpu.sky().expect("read").expect("a map");
            // Texels no pixel looked up (behind the camera) never move.
            let behind = crate::sky::texel([0.0, 0.0, -1.0], width as usize);
            assert_eq!(texels[behind], [0.2, 0.3, 0.9], "{sums:?}");
            let trained = gpu.sky_trained().expect("read").expect("a trained map");
            assert!(!trained[behind] && trained[crate::sky::texel([0.0, 0.3, 1.0], width as usize)], "{sums:?}: which texels moved");
        }
    }

    /// A viewer draws the sky map behind the splats as training does, and keeps nothing that trains it.
    #[test]
    fn a_renderer_draws_the_sky_map_as_training_does() {
        let Ok(mut trainer) = Gpu::new() else { return };
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
        let Ok(adapter) = block_on(instance.request_adapter(&Default::default())) else { return };
        let (device, queue) = block_on(adapter.request_device(&Default::default())).expect("device");
        let mut viewer = Gpu::renderer(device, queue);
        viewer.guards = false;
        let (g, cam) = scene(200, 150, 90, 5);
        let width = 64u32;
        let sky: Vec<[f32; 3]> = (0..width * width / 2).map(|t| [(t % width) as f32 / width as f32, (t / width) as f32 / (width / 2) as f32, 0.6]).collect();
        viewer.upload(&g);
        viewer.step(&cam, [0.0; 3], Target::None, None).expect("viewer");
        let plain = viewer.read_image().expect("image");
        trainer.upload(&g);
        for gpu in [&mut trainer, &mut viewer] {
            gpu.set_sky(Some((width, &sky))).expect("sky");
            gpu.step(&cam, [0.0; 3], Target::None, None).expect("step");
        }
        assert!(viewer.sky.as_ref().is_some_and(|s| s.learning.is_none()), "a viewer keeps no gradients or moments");
        let (a, b) = (trainer.read_image().expect("image"), viewer.read_image().expect("image"));
        let worst = a.iter().zip(&b).map(|(p, q)| (0..3).map(|c| (p[c] - q[c]).abs()).fold(0f32, f32::max)).fold(0f32, f32::max);
        assert!(worst < 1e-4, "{worst}");
        assert!(plain.iter().zip(&b).any(|(p, q)| (q[2] - p[2]) > 0.3), "the sky shows through the gaps");
    }

    #[test]
    fn eight_pixel_tiles_render_the_same_image_and_gradients() {
        let Some(mut gpu) = devices().into_iter().next() else { return };
        let (g, cam) = scene(20_000, 168, 95, 51);
        gpu.upload(&g);
        let mut renders = Vec::new();
        for tile8 in [false, true] {
            gpu.tile8_all = tile8;
            // Twice: a size's pipelined sort is sized from the counts seen at that size, which 8×8 tiles raise.
            for _ in 0..2 {
                gpu.step(&cam, [0.1, 0.2, 0.3], Target::None, None).expect("step");
            }
            gpu.wait_readbacks();
            let before = gpu.overflows;
            gpu.step(&cam, [0.1, 0.2, 0.3], Target::None, None).expect("step");
            renders.push(gpu.read_image().expect("image"));
            gpu.wait_readbacks();
            assert_eq!(gpu.overflows, before, "the compared render dropped entries");
        }
        assert!(renders[0].iter().any(|p| p[3] > 0.0));
        assert!(renders[0] == renders[1], "8×8 tiles rendered differently");
        // And the backward on them against the CPU reference.
        gpu.tile8_all = true;
        forward_and_backward_match(vec![gpu]);
    }

    /// The nested forward (shaders/raster_forward_nested.wgsl) walks the same entries in the same order with the same
    /// arithmetic as `forward`: the same image bit for bit, on 16×16 and 8×8 tiles, with tiles of several batches (20k
    /// splats on a small image) and partial tiles; and the training path through it matches the CPU reference.
    #[test]
    fn nested_forward_renders_the_same_image_bit_for_bit() {
        let Some(mut gpu) = devices().into_iter().next() else { return };
        let (g, cam) = scene(20_000, 168, 95, 53);
        gpu.upload(&g);
        for tile8 in [false, true] {
            gpu.tile8_all = tile8;
            let mut renders = Vec::new();
            for nested in [false, true] {
                gpu.nested_forward = nested;
                for _ in 0..2 {
                    gpu.step(&cam, [0.1, 0.2, 0.3], Target::None, None).expect("step");
                }
                gpu.wait_readbacks();
                let before = gpu.overflows;
                gpu.step(&cam, [0.1, 0.2, 0.3], Target::None, None).expect("step");
                renders.push(gpu.read_image().expect("image"));
                gpu.wait_readbacks();
                assert_eq!(gpu.overflows, before, "the compared render dropped entries");
            }
            assert!(renders[0].iter().any(|p| p[3] > 0.0));
            let differ = renders[0].iter().zip(&renders[1]).filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.to_bits() != y.to_bits())).count();
            assert_eq!(differ, 0, "{differ} pixels differ (8×8 tiles: {tile8})");
        }
        gpu.tile8_all = false;
        forward_and_backward_match(vec![gpu]);
    }

    #[test]
    fn threshold_visibility_fades_what_the_largest_weight_fades() {
        let Some(mut gpu) = devices().into_iter().next() else { return };
        let (g, cam) = scene(20_000, 320, 180, 43);
        gpu.upload(&g);
        let mask: Vec<f32> = (0..cam.width * cam.height).map(|p| if p / cam.width < 20 { 0.0 } else { 1.0 }).collect();
        gpu.set_mask(cam.width, cam.height, &mask).expect("mask");
        let mut zoomed = cam;
        zoomed.fx *= 2.0;
        zoomed.fy *= 2.0;
        let cams = [cam, zoomed];
        let largest = gpu.visibility(&cams, [0.0; 3], 0.0).expect("visibility");
        let over = gpu.visibility(&cams, [0.0; 3], 0.02).expect("visibility");
        let faded = largest.iter().filter(|w| **w < 0.02).count();
        assert!(faded > 100 && faded < g.len() - 100, "{faded} of {} faded", g.len());
        for (i, (a, b)) in largest.iter().zip(&over).enumerate() {
            assert_eq!(*a < 0.02, *b < 0.02, "Gaussian {i}: largest weight {a}, threshold store {b}");
        }
    }

    #[test]
    fn visibility_sees_only_what_scene_pixels_show() {
        let Some(mut gpu) = devices().into_iter().next() else { return };
        let (w, h) = (64u32, 48u32);
        let (mut g, cam) = scene(1, w, h, 41);
        g.means.clear();
        g.log_scales.clear();
        g.rotations.clear();
        g.opacity_logits.clear();
        g.colors.clear();
        g.sky.clear();
        g.filter_3d.clear();
        g.seen.clear();
        // In front, in the open: seen. Behind it, fully hidden: unseen. Off screen: unseen. Under the masked rows: unseen.
        let ahead = |x: f32, y: f32, z: f32| cam.unproject(x, y, z);
        let opaque = crate::gaussians::logit(0.99);
        g.push(ahead(32.0, 30.0, 3.0), [-1.0; 3], [1.0, 0.0, 0.0, 0.0], opaque, [0.5; 3]);
        g.push(ahead(32.0, 30.0, 6.0), [-3.0; 3], [1.0, 0.0, 0.0, 0.0], opaque, [0.5; 3]);
        g.push(ahead(500.0, 30.0, 3.0), [-3.0; 3], [1.0, 0.0, 0.0, 0.0], opaque, [0.5; 3]);
        g.push(ahead(32.0, 3.0, 3.0), [-4.0; 3], [1.0, 0.0, 0.0, 0.0], opaque, [0.5; 3]);
        gpu.upload(&g);
        let mask: Vec<f32> = (0..w * h).map(|p| if p / w < 8 { 0.0 } else { 1.0 }).collect();
        gpu.set_mask(w, h, &mask).expect("mask");
        let seen = gpu.visibility(&[cam], [0.0; 3], 0.0).expect("visibility");
        assert!(seen[0] > 0.9, "front Gaussian: {}", seen[0]);
        // Behind an α = 0.99 splat: at most 1 % of the light reaches it.
        assert!(seen[1] < 0.011, "hidden Gaussian: {}", seen[1]);
        assert_eq!(seen[2], 0.0, "off-screen Gaussian");
        assert!(seen[3] < 1e-3, "Gaussian under the masked rows: {}", seen[3]);
    }

    /// The contributions of all Gaussians add up to what they cover of the scene pixels: with white splats over black,
    /// each pixel's colour is the sum of the blending weights α·T of its splats. Two views count twice; masked rows not.
    #[test]
    fn contributions_add_up_to_the_covered_scene_pixels() {
        let Some(mut gpu) = devices().into_iter().next() else { return };
        let (mut g, cam) = scene(20_000, 320, 180, 47);
        for c in &mut g.colors {
            *c = [1.0; 3];
        }
        gpu.upload(&g);
        let mask: Vec<f32> = (0..cam.width * cam.height).map(|p| if p / cam.width < 20 { 0.0 } else { 1.0 }).collect();
        gpu.set_mask(cam.width, cam.height, &mask).expect("mask");
        let covered: f64 = crate::train::render(&mut gpu, &cam, [0.0; 3]).expect("render").iter().zip(&mask).map(|(p, m)| (p[0] * m) as f64).sum();
        let sums = gpu.contribution(&[cam, cam], [0.0; 3]).expect("contribution");
        let total: f64 = sums.iter().map(|s| *s as f64).sum();
        assert!((total - 2.0 * covered).abs() < 2e-3 * covered, "contributions {total} for {covered} pixels covered twice");
        // The visibility round's largest weight is never more than the sum.
        let largest = gpu.visibility(&[cam], [0.0; 3], 0.0).expect("visibility");
        assert!(largest.iter().zip(&sums).all(|(l, s)| *l <= *s / 2.0 + 1e-3), "a largest weight above its sum");
    }
}
