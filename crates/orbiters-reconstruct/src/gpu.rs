//! The splatting kernels on the GPU (wgpu: Direct3D 12 or Vulkan on Windows, Vulkan on Linux) and the buffers they
//! work on. One forward + backward + Adam step per call; the host reads back only the intersection count (to size
//! the sort) and, when asked, the rendered image.

use crate::camera::Camera;
use crate::cpu;
use crate::gaussians::{Gaussians, PACKED};
use anyhow::{anyhow, Context as _};
use bytemuck::{Pod, Zeroable};
use std::future::Future;
use wgpu::util::DeviceExt;

const GRAD2D: usize = 10;

/// Kernel variants (lab experiments; `ORBITERS_LAB` lists them comma-separated).
#[derive(Clone, Copy, Debug, Default)]
pub struct KernelOptions {
    /// Native f32 atomic adds for gradient sums (Vulkan VK_EXT_shader_atomic_float) instead of CAS loops.
    pub float_atomics: bool,
    /// The backward's first ten subgroup lanes add one gradient slot each instead of lane 0 adding all ten.
    pub lane_adds: bool,
    /// No per-step wait for the intersection count: the sort is sized from counts read back a step or two later,
    /// dispatched indirectly over the count written on the GPU.
    pub pipelined: bool,
    /// Gradient sums as 64-bit fixed point (2⁻⁴⁰) with native i64 atomic adds (Vulkan VK_KHR_shader_atomic_int64,
    /// D3D12 SM 6.6 with DXC): the portable alternative where f32 atomics are missing.
    pub int64_atomics: bool,
}

impl KernelOptions {
    pub fn from_env() -> Self {
        let v = std::env::var("ORBITERS_LAB").unwrap_or_default();
        let has = |k: &str| v.split(',').any(|x| x.trim() == k);
        Self { float_atomics: has("float_atomics"), lane_adds: has("lane_adds"), pipelined: has("pipelined"), int64_atomics: has("int64_atomics") }
    }
}
const FAR: f32 = 1000.0;

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
    Color { rgba: &'a [u32], scene_pixels: f32, weight: f32 },
    Depth { samples: &'a [[f32; 4]], weight: f32 },
    /// No loss (forward only).
    None,
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
    /// Only on a training device (the backward pass needs subgroup operations).
    training: Option<Training>,
}

struct Training {
    backward: wgpu::ComputePipeline,
    loss_color: wgpu::ComputePipeline,
    ssim_partials: wgpu::ComputePipeline,
    ssim_gradient: wgpu::ComputePipeline,
    robust_residual: wgpu::ComputePipeline,
    robust_inliers: wgpu::ComputePipeline,
    depth_loss: wgpu::ComputePipeline,
    project_backward: wgpu::ComputePipeline,
    adam: wgpu::ComputePipeline,
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
}

/// Buffers for one image size.
struct ImageBuffers {
    width: u32,
    height: u32,
    out_color: wgpu::Buffer,
    out_state: wgpu::Buffer,
    d_pixel: wgpu::Buffer,
    target: wgpu::Buffer,
    mask: wgpu::Buffer,
    ranges: wgpu::Buffer,
    /// D-SSIM partial derivatives, three vec4s a pixel (training only).
    partials: wgpu::Buffer,
    /// Robust loss: per-pixel error and the mask of pixels kept this step (training only).
    resid: wgpu::Buffer,
    eff_mask: wgpu::Buffer,
}

/// Sort entries (two copies: the radix passes ping-pong) and the per-block digit histograms.
struct SortBuffers {
    cap: usize,
    keys: [wgpu::Buffer; 2],
    values: [wgpu::Buffer; 2],
    hist: wgpu::Buffer,
    hist_offsets: wgpu::Buffer,
}

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter: String,
    k: Kernels,
    splats: Option<SplatBuffers>,
    image: Option<ImageBuffers>,
    /// Buffers of other image sizes used before (coarse depth or low-resolution colour steps).
    image_cache: Vec<ImageBuffers>,
    /// Screen-space dilation in pixels² along x and y (0.3: the reference's; coarse renders scale it).
    pub dilation: [f32; 2],
    /// Weight λ of D-SSIM in the colour loss ((1 − λ)·L1 + λ·(1 − SSIM)); 0: L1 only.
    pub ssim: f32,
    /// Robust colour loss: pixels in coherent blobs erring by more than this (mean absolute, 0–1) are left out of
    /// the step; 0: off.
    pub robust: f32,
    sort: Option<SortBuffers>,
    samples: Option<(usize, wgpu::Buffer)>,
    scan_levels: Vec<(usize, wgpu::Buffer, wgpu::Buffer)>,
    frame: wgpu::Buffer,
    total_staging: wgpu::Buffer,
    total_out: wgpu::Buffer,
    pub count: usize,
    /// Adam steps taken (bias correction).
    pub step: u32,
    /// Weights of the opacity and scale (per metre) regularisers per Gaussian in training steps.
    pub opacity_reg: f32,
    pub scale_reg: f32,
    /// The first GPU error since the last step (wgpu reports them out of band).
    errors: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// GPU timestamps per section of each step (lab: [`Gpu::new_profiled`]).
    pub profiler: Option<Profiler>,
    options: KernelOptions,
    /// Pipelined steps: readbacks of the intersection count in flight.
    ring: Vec<Readback>,
    ring_next: usize,
    /// The last intersection count read back, a slowly decaying maximum of them, and steps whose entries did not
    /// all fit the sort buffers (pipelined).
    pub last_total: Option<u32>,
    recent_total: u32,
    pub overflows: u32,
    /// Host time spent waiting for the intersection count (the per-step sync) and encoding.
    pub host: std::cell::RefCell<HostTimes>,
}

/// A readback of one step's intersection count (pipelined steps).
struct Readback {
    buffer: wgpu::Buffer,
    /// 0 idle, 1 in flight, 2 mapped, 3 failed.
    state: std::sync::Arc<std::sync::atomic::AtomicU32>,
    submission: Option<wgpu::SubmissionIndex>,
    cap: u32,
}

const RING: usize = 3;

/// Host-side time of the steps so far.
#[derive(Default, Debug, Clone)]
pub struct HostTimes {
    pub steps: u64,
    pub wait: std::time::Duration,
    pub encode: std::time::Duration,
}

const QUERIES: u32 = 2048;

/// GPU timestamps around every compute pass, summed per section (the label set with `Gpu::section`).
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
        let ticks: Vec<u64> = {
            let view = self.readback.get_mapped_range(..).expect("mapped");
            bytemuck::cast_slice::<u8, u64>(&view[..n as usize * 8]).to_vec()
        };
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

impl Gpu {
    pub fn new() -> anyhow::Result<Self> {
        Self::new_with(false)
    }

    /// A training device that also times every pass ([`Gpu::profiler`]).
    pub fn new_profiled() -> anyhow::Result<Self> {
        Self::new_with(true)
    }

    fn new_with(profile: bool) -> anyhow::Result<Self> {
        // Lab: WGPU_BACKEND / WGPU_DX12_COMPILER pick the backend and D3D compiler.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::PRIMARY, ..wgpu::InstanceDescriptor::new_without_display_handle() }.with_env());
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| anyhow!("no GPU adapter: {e}"))?;
        let info = adapter.get_info();
        if !adapter.features().contains(wgpu::Features::SUBGROUP) {
            anyhow::bail!("{} has no subgroup operations, which reconstruction needs.", info.name);
        }
        let limits = adapter.limits();
        let profile = profile && adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let mut options = KernelOptions::from_env();
        options.float_atomics &= adapter.features().contains(wgpu::Features::SHADER_FLOAT32_ATOMIC);
        let int64 = wgpu::Features::SHADER_INT64 | wgpu::Features::SHADER_INT64_ATOMIC_ALL_OPS;
        options.int64_atomics &= !options.float_atomics && adapter.features().contains(int64);
        let mut features = if profile { wgpu::Features::SUBGROUP | wgpu::Features::TIMESTAMP_QUERY } else { wgpu::Features::SUBGROUP };
        if options.float_atomics {
            features |= wgpu::Features::SHADER_FLOAT32_ATOMIC;
        }
        if options.int64_atomics {
            features |= int64;
        }
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("orbiters-reconstruct"),
            required_features: features,
            required_limits: limits,
            ..Default::default()
        }))
        .map_err(|e| anyhow!("GPU device: {e}"))?;
        let errors = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        {
            let errors = errors.clone();
            device.on_uncaptured_error(std::sync::Arc::new(move |e: wgpu::Error| {
                errors.lock().unwrap().get_or_insert_with(|| e.to_string());
            }));
        }
        let mut gpu = Self::build_with(device, queue, format!("{} ({:?}){}", info.name, info.backend, if options.float_atomics || options.lane_adds || options.int64_atomics || options.pipelined { format!(" {options:?}") } else { String::new() }), true, options);
        gpu.errors = errors;
        if profile {
            gpu.profiler = Some(Profiler::new(&gpu.device, &gpu.queue));
        }
        Ok(gpu)
    }

    /// A renderer on someone else's device (a viewer's window): projection, sort and the forward pass only, so the
    /// device needs no features. [`Gpu::step`] takes only [`Target::None`] there.
    pub fn renderer(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self::build(device, queue, String::new(), false)
    }

    fn build(device: wgpu::Device, queue: wgpu::Queue, adapter: String, train: bool) -> Self {
        Self::build_with(device, queue, adapter, train, KernelOptions::default())
    }

    fn build_with(device: wgpu::Device, queue: wgpu::Queue, adapter: String, train: bool, options_k: KernelOptions) -> Self {
        let common = include_str!("shaders/common.wgsl");
        let module = |name: &str, parts: &[&str]| {
            let code = parts.join("\n");
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(name), source: wgpu::ShaderSource::Wgsl(code.into()) })
        };
        // Every kernel writes its workgroup memory before reading it: without wgpu's zero-fill (one thread clearing
        // whole arrays), D3D's FXC compiles the raster kernel in moments instead of 40 s.
        let options = wgpu::PipelineCompilationOptions { zero_initialize_workgroup_memory: false, ..Default::default() };
        let pipe = |m: &wgpu::ShaderModule, entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: m,
                entry_point: Some(entry),
                compilation_options: options.clone(),
                cache: None,
            })
        };
        let raster = include_str!("shaders/raster.wgsl");
        let project = module("project", &[common, include_str!("shaders/project.wgsl")]);
        let scan = module("scan", &[include_str!("shaders/scan.wgsl")]);
        let duplicate = module("duplicate", &[common, include_str!("shaders/duplicate.wgsl")]);
        let sort = module("sort", &[include_str!("shaders/sort.wgsl")]);
        let ranges = module("ranges", &[common, include_str!("shaders/ranges.wgsl")]);
        let forward = module("raster forward", &[common, raster, include_str!("shaders/raster_forward.wgsl")]);
        let present = module("present", &[common, include_str!("shaders/present.wgsl")]);
        let training = train.then(|| {
            let mut backward_src = include_str!("shaders/raster_backward.wgsl").replace("\r\n", "\n");
            if options_k.lane_adds {
                let serial = "                if (lane == 0u) {\n                    let g = bitcast<u32>(bb.w) * GRAD2D;\n                    add(g, s0.x);\n                    add(g + 1u, s0.y);\n                    add(g + 2u, s0.z);\n                    add(g + 3u, s0.w);\n                    add(g + 4u, s1.x);\n                    add(g + 5u, s1.y);\n                    add(g + 6u, s2.x);\n                    add(g + 7u, s2.y);\n                    add(g + 8u, s2.z);\n                    add(g + 9u, s2.w);\n                }";
                let parallel = "                if (lane < 10u) {\n                    // Lane k adds slot k: ten independent atomics instead of a chain of ten.\n                    var v = s0[lane & 3u];\n                    if (lane >= 4u) {\n                        v = s1[lane & 1u];\n                    }\n                    if (lane >= 6u) {\n                        v = s2[(lane - 6u) & 3u];\n                    }\n                    add(bitcast<u32>(bb.w) * GRAD2D + lane, v);\n                }";
                assert!(backward_src.contains(serial), "backward kernel changed");
                backward_src = backward_src.replace(serial, parallel);
            }
            if options_k.float_atomics {
                let cas_decl = "@group(0) @binding(7) var<storage, read_write> grad2d: array<atomic<u32>>;";
                let cas_add = "fn add(i: u32, v: f32) {\n    if (v == 0.0) {\n        return;\n    }\n    var old = atomicLoad(&grad2d[i]);\n    loop {\n        let r = atomicCompareExchangeWeak(&grad2d[i], old, bitcast<u32>(bitcast<f32>(old) + v));\n        if (r.exchanged) {\n            break;\n        }\n        old = r.old_value;\n    }\n}";
                assert!(backward_src.contains(cas_decl) && backward_src.contains(cas_add), "backward kernel changed");
                backward_src = backward_src
                    .replace(cas_decl, "@group(0) @binding(7) var<storage, read_write> grad2d: array<atomic<f32>>;")
                    .replace(cas_add, "fn add(i: u32, v: f32) {\n    if (v != 0.0) {\n        atomicAdd(&grad2d[i], v);\n    }\n}");
            }
            if options_k.int64_atomics {
                let cas_decl = "@group(0) @binding(7) var<storage, read_write> grad2d: array<atomic<u32>>;";
                let cas_add = "fn add(i: u32, v: f32) {\n    if (v == 0.0) {\n        return;\n    }\n    var old = atomicLoad(&grad2d[i]);\n    loop {\n        let r = atomicCompareExchangeWeak(&grad2d[i], old, bitcast<u32>(bitcast<f32>(old) + v));\n        if (r.exchanged) {\n            break;\n        }\n        old = r.old_value;\n    }\n}";
                assert!(backward_src.contains(cas_decl) && backward_src.contains(cas_add), "backward kernel changed");
                backward_src = backward_src
                    .replace(cas_decl, "@group(0) @binding(7) var<storage, read_write> grad2d: array<atomic<i64>>;")
                    .replace(cas_add, "fn add(i: u32, v: f32) {\n    if (v != 0.0) {\n        atomicAdd(&grad2d[i], i64(round(v * 1099511627776.0)));\n    }\n}");
            }
            let backward = module("raster backward", &[common, raster, &backward_src]);
            let mut project_backward_src = include_str!("shaders/project_backward.wgsl").replace("\r\n", "\n");
            if options_k.int64_atomics {
                let decl = "@group(0) @binding(3) var<storage, read> grad2d: array<f32>;";
                assert!(project_backward_src.contains(decl), "projection backward changed");
                project_backward_src = project_backward_src.replace(decl, "@group(0) @binding(3) var<storage, read> grad2d: array<i64>;\n\nfn g2(i: u32) -> f32 {\n    return f32(grad2d[i]) * (1.0 / 1099511627776.0);\n}");
                for k in [" + 9u", " + 8u", " + 7u", " + 6u", " + 5u", " + 4u", " + 3u", " + 2u", " + 1u", ""] {
                    project_backward_src = project_backward_src.replace(&format!("grad2d[g{k}]"), &format!("g2(g{k})"));
                }
            }
            let loss = module("loss", &[common, include_str!("shaders/loss.wgsl")]);
            let ssim = module("ssim", &[common, include_str!("shaders/ssim.wgsl")]);
            let robust = module("robust", &[common, include_str!("shaders/robust.wgsl")]);
            let depth_loss = module("depth_loss", &[common, include_str!("shaders/depth_loss.wgsl")]);
            let project_backward = module("project_backward", &[common, &project_backward_src]);
            let adam = module("adam", &[include_str!("shaders/adam.wgsl")]);
            Training {
                backward: pipe(&backward, "backward"),
                loss_color: pipe(&loss, "color"),
                ssim_partials: pipe(&ssim, "ssim_partials"),
                ssim_gradient: pipe(&ssim, "ssim_gradient"),
                robust_residual: pipe(&robust, "residual"),
                robust_inliers: pipe(&robust, "inliers"),
                depth_loss: pipe(&depth_loss, "main"),
                project_backward: pipe(&project_backward, "main"),
                adam: pipe(&adam, "main"),
            }
        });
        let k = Kernels {
            project: pipe(&project, "main"),
            scan_blocks: pipe(&scan, "scan_blocks"),
            add_sums: pipe(&scan, "add_sums"),
            total: pipe(&scan, "total"),
            duplicate: pipe(&duplicate, "main"),
            sort_count: pipe(&sort, "count"),
            sort_scatter: pipe(&sort, "scatter"),
            ranges: pipe(&ranges, "main"),
            forward: pipe(&forward, "forward"),
            present: pipe(&present, "main"),
            training,
        };
        let frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame"),
            size: std::mem::size_of::<Frame>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
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
        let ring = (0..RING)
            .map(|_| Readback {
                buffer: device.create_buffer(&wgpu::BufferDescriptor { label: Some("total readback"), size: 16, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
                state: Default::default(),
                submission: None,
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
            dilation: [cpu::DILATION as f32; 2],
            ssim: 0.0,
            robust: 0.0,
            sort: None,
            samples: None,
            scan_levels: Vec::new(),
            frame,
            total_staging,
            total_out,
            count: 0,
            step: 0,
            opacity_reg: 0.0,
            scale_reg: 0.0,
            errors: Default::default(),
            profiler: None,
            host: Default::default(),
            options: options_k,
            ring,
            ring_next: 0,
            last_total: None,
            recent_total: 0,
            overflows: 0,
        }
    }

    fn training(&self) -> anyhow::Result<&Training> {
        self.k.training.as_ref().context("this GPU only renders")
    }

    fn bind(&self, pipeline: &wgpu::ComputePipeline, entries: &[(u32, &wgpu::Buffer)]) -> wgpu::BindGroup {
        let layout = pipeline.get_bind_group_layout(0);
        let entries: Vec<wgpu::BindGroupEntry> = entries.iter().map(|(b, buf)| wgpu::BindGroupEntry { binding: *b, resource: buf.as_entire_binding() }).collect();
        self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &entries })
    }

    fn uniform<T: Pod>(&self, v: &T) -> wgpu::Buffer {
        self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::bytes_of(v), usage: wgpu::BufferUsages::UNIFORM })
    }

    /// Names the passes dispatched from now on (profiling).
    fn section(&self, label: &'static str) {
        if let Some(p) = &self.profiler {
            p.section.set(label);
        }
    }

    fn dispatch(&self, enc: &mut wgpu::CommandEncoder, pipeline: &wgpu::ComputePipeline, bind: &wgpu::BindGroup, (x, y): (u32, u32)) {
        let writes = self.profiler.as_ref().and_then(|p| p.next().map(|i| (p, i)));
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: None,
            timestamp_writes: writes.map(|(p, i)| wgpu::ComputePassTimestampWrites { query_set: &p.set, beginning_of_pass_write_index: Some(i), end_of_pass_write_index: Some(i + 1) }),
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.dispatch_workgroups(x, y, 1);
    }

    /// Uploads Gaussians (Adam moments and statistics start at zero).
    pub fn upload(&mut self, g: &Gaussians) {
        self.upload_with_moments(g, None);
    }

    /// Uploads Gaussians with their Adam moments (packed like the parameters).
    pub fn upload_with_moments(&mut self, g: &Gaussians, moments: Option<(&[f32], &[f32])>) {
        let n = g.len();
        let train = self.k.training.is_some();
        if self.splats.as_ref().is_none_or(|s| s.cap < n) {
            // A renderer keeps no room to spare (a viewer loads once) and no training buffers.
            let cap = if train { (n + n / 2).max(1024) } else { n.max(1) };
            let d = &self.device;
            let f = |count: usize| (count * 4) as u64;
            let t = |count: usize| if train { f(count) } else { 0 };
            self.splats = Some(SplatBuffers {
                cap,
                params: storage(d, "params", f(cap * PACKED)),
                grads: storage(d, "grads", t(cap * PACKED)),
                m1: storage(d, "moment1", t(cap * PACKED)),
                m2: storage(d, "moment2", t(cap * PACKED)),
                proj: storage(d, "projected", f(cap * 16)),
                counts: storage(d, "tile counts", f(cap)),
                offsets: storage(d, "tile offsets", f(cap)),
                // Twice the floats: room for 64-bit fixed-point sums.
                grad2d: storage(d, "grad2d", t(cap * GRAD2D * 2)),
                stats: storage(d, "stats", t(cap * 4)),
            });
        }
        let s = self.splats.as_ref().expect("allocated");
        self.queue.write_buffer(&s.params, 0, bytemuck::cast_slice(&g.pack()));
        if train {
            let zeros = vec![0f32; n * PACKED];
            let (m1, m2) = moments.unwrap_or((&zeros, &zeros));
            self.queue.write_buffer(&s.m1, 0, bytemuck::cast_slice(m1));
            self.queue.write_buffer(&s.m2, 0, bytemuck::cast_slice(m2));
            self.queue.write_buffer(&s.stats, 0, bytemuck::cast_slice(&vec![0f32; n * 4]));
        }
        self.count = n;
    }

    fn read<T: Pod>(&self, buffer: &wgpu::Buffer, count: usize) -> Vec<T> {
        let bytes = (count * std::mem::size_of::<T>()) as u64;
        if bytes == 0 {
            return Vec::new();
        }
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: bytes.next_multiple_of(4),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(buffer, 0, &staging, 0, bytes.next_multiple_of(4));
        self.queue.submit([enc.finish()]);
        staging.map_async(wgpu::MapMode::Read, .., |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let view = staging.get_mapped_range(..).expect("mapped");
        let out = bytemuck::cast_slice::<u8, T>(&view[..bytes as usize]).to_vec();
        drop(view);
        staging.unmap();
        out
    }

    /// The Gaussians, their Adam moments and densification statistics (packed).
    pub fn download(&self) -> (Gaussians, Vec<f32>, Vec<f32>, Vec<[f32; 4]>) {
        let s = self.splats.as_ref().expect("uploaded");
        let n = self.count;
        let params: Vec<f32> = self.read(&s.params, n * PACKED);
        let m1: Vec<f32> = self.read(&s.m1, n * PACKED);
        let m2: Vec<f32> = self.read(&s.m2, n * PACKED);
        let stats: Vec<[f32; 4]> = self.read(&s.stats, n * 4).as_chunks::<4>().0.to_vec();
        (Gaussians::unpack(&params), m1, m2, stats)
    }

    /// The last step's parameter gradients (packed), for tests.
    pub fn read_grads(&self) -> Vec<f32> {
        let s = self.splats.as_ref().expect("uploaded");
        self.read(&s.grads, self.count * PACKED)
    }

    fn ensure_image(&mut self, w: u32, h: u32) {
        if self.image.as_ref().is_some_and(|i| i.width == w && i.height == h) {
            return;
        }
        if let Some(old) = self.image.take() {
            self.image_cache.push(old);
        }
        if let Some(k) = self.image_cache.iter().position(|i| i.width == w && i.height == h) {
            self.image = Some(self.image_cache.swap_remove(k));
            return;
        }
        let d = &self.device;
        let px = (w * h) as u64;
        let tiles = (w.div_ceil(cpu::TILE) * h.div_ceil(cpu::TILE)) as u64;
        let train = if self.k.training.is_some() { 1 } else { 0 };
        self.image = Some(ImageBuffers {
            width: w,
            height: h,
            out_color: storage(d, "out colour", px * 16),
            out_state: storage(d, "out state", px * 8),
            d_pixel: storage(d, "d pixel", train * px * 16),
            target: storage(d, "target", train * px * 4),
            mask: storage(d, "mask", train * px * 4),
            ranges: storage(d, "tile ranges", tiles * 8),
            partials: storage(d, "ssim partials", train * px * 48),
            resid: storage(d, "residual", train * px * 4),
            eff_mask: storage(d, "kept pixels", train * px * 4),
        });
    }

    /// The loss mask (1 scene, 0 strip and band) for images of this size.
    pub fn set_mask(&mut self, w: u32, h: u32, mask: &[f32]) {
        self.ensure_image(w, h);
        self.queue.write_buffer(&self.image.as_ref().expect("image").mask, 0, bytemuck::cast_slice(mask));
    }

    fn ensure_sort(&mut self, n: usize) {
        if self.sort.as_ref().is_some_and(|s| s.cap >= n) {
            return;
        }
        let cap = (n + n / 2).max(1 << 16);
        let blocks = cap.div_ceil(256);
        let d = &self.device;
        self.sort = Some(SortBuffers {
            cap,
            keys: [storage(d, "keys a", cap as u64 * 4), storage(d, "keys b", cap as u64 * 4)],
            values: [storage(d, "values a", cap as u64 * 4), storage(d, "values b", cap as u64 * 4)],
            hist: storage(d, "digit histograms", blocks as u64 * 16 * 4),
            hist_offsets: storage(d, "digit offsets", blocks as u64 * 16 * 4),
        });
    }

    /// Exclusive scan of `n` u32s from `src` into `dst` (levels of 512).
    fn scan(&mut self, enc: &mut wgpu::CommandEncoder, src: &wgpu::Buffer, dst: &wgpu::Buffer, n: usize, level: usize) {
        let blocks = n.div_ceil(512);
        if self.scan_levels.len() <= level || self.scan_levels[level].0 < blocks {
            let cap = blocks.max(64) * 2;
            let sums = storage(&self.device, "scan sums", cap as u64 * 4);
            let scanned = storage(&self.device, "scan sums scanned", cap as u64 * 4);
            if self.scan_levels.len() <= level {
                self.scan_levels.push((cap, sums, scanned));
            } else {
                self.scan_levels[level] = (cap, sums, scanned);
            }
        }
        let info = self.uniform(&[n as u32, 0, 0, 0]);
        let (sums, scanned) = (self.scan_levels[level].1.clone(), self.scan_levels[level].2.clone());
        let bind = self.bind(&self.k.scan_blocks, &[(0, &info), (1, src), (2, dst), (3, &sums)]);
        self.dispatch(enc, &self.k.scan_blocks, &bind, groups_of(blocks));
        if blocks > 1 {
            self.scan(enc, &sums, &scanned, blocks, level + 1);
            let bind = self.bind(&self.k.add_sums, &[(0, &info), (2, dst), (3, &scanned)]);
            self.dispatch(enc, &self.k.add_sums, &bind, groups(n));
        }
    }

    fn frame_for(&self, cam: &Camera, background: [f32; 3], intersections: u32, loss: [f32; 4], train: bool) -> Frame {
        let tiles_x = cam.width.div_ceil(cpu::TILE);
        let tiles_y = cam.height.div_ceil(cpu::TILE);
        let tiles = (tiles_x * tiles_y).max(2);
        let tile_bits = 32 - (tiles - 1).leading_zeros();
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
            counts: [self.count as u32, intersections, 32 - tile_bits, 0],
            loss,
            reg: if train { [self.opacity_reg, self.scale_reg, self.dilation[0], self.dilation[1]] } else { [0.0, 0.0, self.dilation[0], self.dilation[1]] },
        }
    }

    fn dispatch_indirect(&self, enc: &mut wgpu::CommandEncoder, pipeline: &wgpu::ComputePipeline, bind: &wgpu::BindGroup, args: &wgpu::Buffer, offset: u64) {
        let writes = self.profiler.as_ref().and_then(|p| p.next().map(|i| (p, i)));
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: None,
            timestamp_writes: writes.map(|(p, i)| wgpu::ComputePassTimestampWrites { query_set: &p.set, beginning_of_pass_write_index: Some(i), end_of_pass_write_index: Some(i + 1) }),
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.dispatch_workgroups_indirect(args, offset);
    }

    /// Takes the count of a mapped readback slot (pipelined steps).
    fn take_readback(&mut self, slot: usize) {
        let r = &self.ring[slot];
        match r.state.load(std::sync::atomic::Ordering::SeqCst) {
            2 => {
                let raw = {
                    let view = r.buffer.get_mapped_range(..).expect("mapped");
                    bytemuck::cast_slice::<u8, u32>(&view)[0]
                };
                r.buffer.unmap();
                if raw > r.cap {
                    self.overflows += 1;
                }
                self.last_total = Some(raw);
                self.recent_total = raw.max(self.recent_total - self.recent_total / 32);
            }
            3 => {}
            _ => return,
        }
        self.ring[slot].state.store(0, std::sync::atomic::Ordering::SeqCst);
    }

    /// Takes every readback that has arrived, without waiting.
    fn drain_readbacks(&mut self) {
        let _ = self.device.poll(wgpu::PollType::Poll);
        for slot in 0..RING {
            self.take_readback(slot);
        }
    }

    /// Renders (and, with a target, takes one optimisation step with `lr`). Returns the number of tile
    /// intersections (pipelined: the latest count read back, a step or two old).
    pub fn step(&mut self, cam: &Camera, background: [f32; 3], target: Target<'_>, lr: Option<LearningRates>) -> anyhow::Result<u32> {
        let n = self.count;
        if n == 0 {
            return Ok(0);
        }
        if !matches!(target, Target::None) {
            self.training()?;
        }
        if let Some(e) = self.errors.lock().unwrap().take() {
            anyhow::bail!("GPU error: {e}");
        }
        self.ensure_image(cam.width, cam.height);
        let loss = match &target {
            Target::Color { scene_pixels, weight, .. } => [*weight, self.robust, self.ssim, *scene_pixels],
            Target::Depth { samples, weight } => [0.0, *weight, 0.0, samples.iter().filter(|s| s[2] > 0.0).count() as f32],
            Target::None => [0.0; 4],
        };
        let train = !matches!(target, Target::None);
        let pipelined = self.options.pipelined && self.last_total.is_some();
        // Each sort copy (sized half again over the entries) must fit one storage binding.
        let max_entries = (self.device.limits().max_storage_buffer_binding_size / 4 * 2 / 3).min(u32::MAX as u64) as u32;
        let frame = self.frame_for(cam, background, 0, loss, train);
        self.queue.write_buffer(&self.frame, 0, bytemuck::bytes_of(&frame));
        let (params, proj, counts, offsets) = {
            let s = self.splats.as_ref().context("no Gaussians")?;
            (s.params.clone(), s.proj.clone(), s.counts.clone(), s.offsets.clone())
        };
        // Pipelined: the sort is sized from the counts read back so far; entries past it are dropped for this step,
        // counted as an overflow when its count arrives, and the buffers grow for the next steps.
        let cap = if pipelined {
            self.drain_readbacks();
            let est = self.recent_total as usize;
            self.ensure_sort((est + est / 4 + 4096).min(max_entries as usize));
            self.sort.as_ref().expect("sort").cap.min(max_entries as usize) as u32
        } else {
            0
        };
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.section("project");
        let bind = self.bind(&self.k.project, &[(0, &self.frame), (1, &params), (2, &proj), (3, &counts)]);
        self.dispatch(&mut enc, &self.k.project, &bind, groups(n));
        self.scan(&mut enc, &counts, &offsets, n, 0);
        let info = self.uniform(&[n as u32, cap, 0, 0]);
        let total_out = self.total_out.clone();
        let bind = self.bind(&self.k.total, &[(0, &info), (1, &counts), (2, &offsets), (3, &total_out)]);
        self.dispatch(&mut enc, &self.k.total, &bind, (1, 1));
        let encoding;
        // Pipelined: the sort's bound (the kernels read the true count); else the count, waited for.
        let total = if pipelined {
            encoding = std::time::Instant::now();
            cap
        } else {
            enc.copy_buffer_to_buffer(&total_out, 0, &self.total_staging, 0, 16);
            self.queue.submit([enc.finish()]);
            enc = self.device.create_command_encoder(&Default::default());
            self.total_staging.map_async(wgpu::MapMode::Read, .., |_| {});
            let waited = std::time::Instant::now();
            let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            self.host.borrow_mut().wait += waited.elapsed();
            encoding = std::time::Instant::now();
            let total = {
                let view = self.total_staging.get_mapped_range(..).expect("mapped");
                bytemuck::cast_slice::<u8, u32>(&view)[0]
            };
            self.total_staging.unmap();
            anyhow::ensure!(total <= max_entries, "{total} tile entries in one view: more than the GPU can sort ({max_entries})");
            self.ensure_sort(total as usize);
            self.last_total = Some(total);
            self.recent_total = total.max(self.recent_total - self.recent_total / 32);
            total
        };
        let (keys, values, hist, hist_offsets) = {
            let s = self.sort.as_ref().expect("sort");
            (s.keys.clone(), s.values.clone(), s.hist.clone(), s.hist_offsets.clone())
        };
        let (out_color, out_state, d_pixel, target_buf, mut mask, ranges, partials, resid, eff_mask) = {
            let i = self.image.as_ref().expect("image");
            (i.out_color.clone(), i.out_state.clone(), i.d_pixel.clone(), i.target.clone(), i.mask.clone(), i.ranges.clone(), i.partials.clone(), i.resid.clone(), i.eff_mask.clone())
        };
        enc.clear_buffer(&ranges, 0, None);
        if total > 0 {
            self.section("duplicate");
            let bind = self.bind(&self.k.duplicate, &[(0, &self.frame), (1, &proj), (2, &offsets), (3, &keys[0]), (4, &values[0])]);
            self.dispatch(&mut enc, &self.k.duplicate, &bind, groups(n));
            // Pipelined: the histograms' stride is the capacity's blocks; only the counted blocks scatter.
            let blocks = (total as usize).div_ceil(256);
            self.section("sort");
            for pass in 0..8u32 {
                let (a, b) = ((pass % 2) as usize, ((pass + 1) % 2) as usize);
                let info = self.uniform(&SortInfo { n: total, blocks: blocks as u32, shift: pass * 4, pad: 0 });
                let bind = self.bind(&self.k.sort_count, &[(0, &info), (1, &keys[a]), (3, &hist), (6, &total_out)]);
                self.dispatch(&mut enc, &self.k.sort_count, &bind, groups_of(blocks));
                self.scan(&mut enc, &hist, &hist_offsets, blocks * 16, 0);
                let bind = self.bind(&self.k.sort_scatter, &[(0, &info), (1, &keys[a]), (2, &values[a]), (3, &hist_offsets), (4, &keys[b]), (5, &values[b]), (6, &total_out)]);
                if pipelined {
                    self.dispatch_indirect(&mut enc, &self.k.sort_scatter, &bind, &total_out, 8);
                } else {
                    self.dispatch(&mut enc, &self.k.sort_scatter, &bind, groups_of(blocks));
                }
            }
            self.section("ranges");
            let bind = self.bind(&self.k.ranges, &[(0, &self.frame), (1, &keys[0]), (2, &ranges), (3, &total_out)]);
            if pipelined {
                self.dispatch_indirect(&mut enc, &self.k.ranges, &bind, &total_out, 8);
            } else {
                self.dispatch(&mut enc, &self.k.ranges, &bind, groups(total as usize));
            }
        }
        let tiles = (cam.width.div_ceil(cpu::TILE), cam.height.div_ceil(cpu::TILE));
        self.section("forward");
        let bind = self.bind(&self.k.forward, &[(0, &self.frame), (1, &proj), (2, &values[0]), (3, &ranges), (4, &out_color), (5, &out_state)]);
        self.dispatch(&mut enc, &self.k.forward, &bind, tiles);
        match target {
            Target::Color { rgba, .. } => {
                self.queue.write_buffer(&target_buf, 0, bytemuck::cast_slice(rgba));
                let t = self.training()?;
                self.section("loss");
                if self.robust > 0.0 {
                    let bind = self.bind(&t.robust_residual, &[(0, &self.frame), (1, &out_color), (2, &target_buf), (3, &mask), (4, &resid)]);
                    self.dispatch(&mut enc, &t.robust_residual, &bind, groups((cam.width * cam.height) as usize));
                    let bind = self.bind(&t.robust_inliers, &[(0, &self.frame), (3, &mask), (4, &resid), (5, &eff_mask)]);
                    self.dispatch(&mut enc, &t.robust_inliers, &bind, tiles);
                    mask = eff_mask.clone();
                }
                if self.ssim > 0.0 {
                    let bind = self.bind(&t.ssim_partials, &[(0, &self.frame), (1, &out_color), (2, &target_buf), (3, &mask), (4, &partials)]);
                    self.dispatch(&mut enc, &t.ssim_partials, &bind, tiles);
                    let bind = self.bind(&t.ssim_gradient, &[(0, &self.frame), (1, &out_color), (2, &target_buf), (3, &mask), (4, &partials), (5, &d_pixel)]);
                    self.dispatch(&mut enc, &t.ssim_gradient, &bind, tiles);
                } else {
                    let bind = self.bind(&t.loss_color, &[(0, &self.frame), (1, &out_color), (2, &target_buf), (3, &mask), (4, &d_pixel)]);
                    self.dispatch(&mut enc, &t.loss_color, &bind, groups((cam.width * cam.height) as usize));
                }
            }
            Target::Depth { samples, .. } => {
                if self.samples.as_ref().is_none_or(|(c, _)| *c < samples.len()) {
                    let cap = samples.len().next_power_of_two().max(1024);
                    self.samples = Some((cap, storage(&self.device, "depth samples", cap as u64 * 16)));
                }
                let buf = self.samples.as_ref().expect("samples").1.clone();
                let mut padded = samples.to_vec();
                padded.resize(self.samples.as_ref().expect("samples").0, [0.0; 4]);
                self.queue.write_buffer(&buf, 0, bytemuck::cast_slice(&padded));
                enc.clear_buffer(&d_pixel, 0, None);
                let t = self.training()?;
                self.section("depth_loss");
                let bind = self.bind(&t.depth_loss, &[(0, &self.frame), (1, &out_color), (2, &buf), (3, &d_pixel)]);
                self.dispatch(&mut enc, &t.depth_loss, &bind, groups(padded.len()));
            }
            Target::None => {}
        }
        if train {
            self.backward_into(&mut enc, &proj, &values[0], &ranges, &out_state, &d_pixel, tiles);
            if let Some(lr) = lr {
                self.adam(&mut enc, lr);
            }
        }
        {
            let mut host = self.host.borrow_mut();
            host.encode += encoding.elapsed();
            host.steps += 1;
        }
        if pipelined {
            // The count comes back through a ring of readbacks; a slot still in flight is waited for, which keeps
            // the host at most a few steps ahead.
            let slot = self.ring_next;
            self.ring_next = (slot + 1) % RING;
            if self.ring[slot].state.load(std::sync::atomic::Ordering::SeqCst) == 1 {
                let waited = std::time::Instant::now();
                let index = self.ring[slot].submission.clone();
                let _ = self.device.poll(wgpu::PollType::Wait { submission_index: index, timeout: None });
                self.host.borrow_mut().wait += waited.elapsed();
            }
            self.take_readback(slot);
            enc.copy_buffer_to_buffer(&total_out, 0, &self.ring[slot].buffer, 0, 16);
            let index = self.queue.submit([enc.finish()]);
            let r = &mut self.ring[slot];
            r.cap = cap;
            r.submission = Some(index);
            r.state.store(1, std::sync::atomic::Ordering::SeqCst);
            let state = r.state.clone();
            r.buffer.map_async(wgpu::MapMode::Read, .., move |res| state.store(if res.is_ok() { 2 } else { 3 }, std::sync::atomic::Ordering::SeqCst));
            let _ = self.device.poll(wgpu::PollType::Poll);
        } else {
            self.queue.submit([enc.finish()]);
        }
        if let Some(p) = &self.profiler {
            p.collect(&self.device, &self.queue);
        }
        Ok(if pipelined { self.last_total.unwrap_or(0) } else { total })
    }

    #[allow(clippy::too_many_arguments)]
    fn backward_into(
        &self,
        enc: &mut wgpu::CommandEncoder,
        proj: &wgpu::Buffer,
        values: &wgpu::Buffer,
        ranges: &wgpu::Buffer,
        out_state: &wgpu::Buffer,
        d_pixel: &wgpu::Buffer,
        tiles: (u32, u32),
    ) {
        let s = self.splats.as_ref().expect("splats");
        enc.clear_buffer(&s.grad2d, 0, None);
        let t = self.k.training.as_ref().expect("a training GPU");
        self.section("backward");
        let bind = self.bind(&t.backward, &[(0, &self.frame), (1, proj), (2, values), (3, ranges), (5, out_state), (6, d_pixel), (7, &s.grad2d)]);
        self.dispatch(enc, &t.backward, &bind, tiles);
        self.section("project_backward");
        let bind = self.bind(&t.project_backward, &[(0, &self.frame), (1, &s.params), (2, proj), (3, &s.grad2d), (4, &s.grads), (5, &s.stats)]);
        self.dispatch(enc, &t.project_backward, &bind, groups(self.count));
    }

    fn adam(&mut self, enc: &mut wgpu::CommandEncoder, lr: LearningRates) {
        self.step += 1;
        let mut rates = [0f32; 16];
        rates[0..3].fill(lr.means);
        rates[3..6].fill(lr.log_scales);
        rates[6..10].fill(lr.rotations);
        rates[10] = lr.opacity;
        rates[11..14].fill(lr.colors);
        let n = self.count * PACKED;
        let info = self.uniform(&AdamInfo { lr: rates, beta: [0.9, 0.999, 1e-15, 0.0], n: [n as u32, self.step, 0, 0] });
        let s = self.splats.as_ref().expect("splats");
        let t = self.k.training.as_ref().expect("a training GPU");
        self.section("adam");
        let bind = self.bind(&t.adam, &[(0, &info), (1, &s.params), (2, &s.grads), (3, &s.m1), (4, &s.m2)]);
        self.dispatch(enc, &t.adam, &bind, groups(n));
    }

    /// Backward from given pixel gradients (r, g, b, depth per pixel) after a forward [`Gpu::step`] with
    /// [`Target::None`] on the same camera; leaves the parameter gradients in place (see [`Gpu::read_grads`]).
    pub fn backward_from(&mut self, cam: &Camera, d_pixel: &[[f32; 4]]) {
        let i = self.image.as_ref().expect("image");
        let s = self.sort.as_ref().expect("sort");
        let splats = self.splats.as_ref().expect("splats");
        self.queue.write_buffer(&i.d_pixel, 0, bytemuck::cast_slice(d_pixel));
        let mut enc = self.device.create_command_encoder(&Default::default());
        let tiles = (cam.width.div_ceil(cpu::TILE), cam.height.div_ceil(cpu::TILE));
        self.backward_into(&mut enc, &splats.proj, &s.values[0], &i.ranges, &i.out_state, &i.d_pixel, tiles);
        self.queue.submit([enc.finish()]);
    }

    /// Writes the last rendered image (colour over the background) into `target`, an RGBA8 storage texture of the
    /// camera's size: the image a viewer shows.
    pub fn present(&self, target: &wgpu::TextureView) {
        let Some(i) = self.image.as_ref() else { return };
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
    }

    /// The last rendered image: (r, g, b, depth) per pixel.
    pub fn read_image(&self) -> Vec<[f32; 4]> {
        let i = self.image.as_ref().expect("image");
        let px = (i.width * i.height) as usize;
        self.read::<f32>(&i.out_color, px * 4).as_chunks::<4>().0.to_vec()
    }

    /// The pixels the last robust colour step kept (1) or left out (0).
    pub fn read_kept(&self) -> Vec<f32> {
        let i = self.image.as_ref().expect("image");
        self.read::<f32>(&i.eff_mask, (i.width * i.height) as usize)
    }

    /// The last step's loss gradient per pixel (r, g, b, depth), for tests.
    pub fn read_d_pixel(&self) -> Vec<[f32; 4]> {
        let i = self.image.as_ref().expect("image");
        let px = (i.width * i.height) as usize;
        self.read::<f32>(&i.d_pixel, px * 4).as_chunks::<4>().0.to_vec()
    }

    /// The last rendered final transmittance per pixel.
    pub fn read_transmittance(&self) -> Vec<f32> {
        let i = self.image.as_ref().expect("image");
        let px = (i.width * i.height) as usize;
        self.read::<u32>(&i.out_state, px * 2).as_chunks::<2>().0.iter().map(|c| f32::from_bits(c[0])).collect()
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

    #[test]
    fn forward_and_backward_match_the_cpu_reference() {
        let gpu = Gpu::new();
        let Ok(mut gpu) = gpu else {
            eprintln!("skipped: {}", gpu.err().expect("error"));
            return;
        };
        eprintln!("adapter {}", gpu.adapter);
        let bg = [0.1f32, 0.2, 0.3];
        for (count, w, h, seed) in [(7, 40, 30, 3), (3000, 150, 90, 5)] {
            let (g, cam) = scene(count, w, h, seed);
            gpu.upload(&g);
            let total = gpu.step(&cam, bg, Target::None, None).expect("step");
            let img = gpu.read_image();
            let tr = gpu.read_transmittance();
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
            eprintln!("{count} Gaussians {w}×{h}: {total} intersections, worst colour {:.2e}, depth {:.2e}, T {:.2e}", worst[0], worst[1], worst[2]);
            assert!(total > 0);
            assert!(worst[0] < 2e-3 && worst[1] < 2e-3 && worst[2] < 2e-3, "{worst:?}");

            let mut rng = fastrand::Rng::with_seed(seed + 100);
            let d_pixel: Vec<[f32; 4]> = (0..n).map(|_| [rng.f32() - 0.5, rng.f32() - 0.5, rng.f32() - 0.5, (rng.f32() - 0.5) * 0.2]).collect();
            gpu.backward_from(&cam, &d_pixel);
            let grads = gpu.read_grads();
            let dc: Vec<f64> = d_pixel.iter().flat_map(|p| [p[0] as f64, p[1] as f64, p[2] as f64]).collect();
            let dd: Vec<f64> = d_pixel.iter().map(|p| p[3] as f64).collect();
            let reference = cpu::backward(&g, &cam, bg.map(f64::from), cpu::Rules::GPU, &dc, &dd);
            // Compare each group against its largest magnitude (f32 atomics sum in any order).
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
                assert!(worst < 1e-2, "{name}: {worst}");
            }
        }
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
        let (g, cam) = scene(3000, 150, 90, 5);
        let bg = [0.1, 0.2, 0.3];
        trainer.upload(&g);
        viewer.upload(&g);
        trainer.step(&cam, bg, Target::None, None).expect("trainer");
        viewer.step(&cam, bg, Target::None, None).expect("viewer");
        assert!(viewer.step(&cam, bg, Target::Depth { samples: &[], weight: 1.0 }, None).is_err());
        let (a, b) = (trainer.read_image(), viewer.read_image());
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
        let bytes: Vec<u8> = viewer.read(&staging, (row * cam.height) as usize);
        let (x, y) = (70usize, 40usize);
        let px = &bytes[y * row as usize + x * 4..][..4];
        let want = b[y * cam.width as usize + x];
        for c in 0..3 {
            assert!((px[c] as f32 / 255.0 - want[c].clamp(0.0, 1.0)).abs() < 1.5 / 255.0, "{px:?} vs {want:?}");
        }
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
        let Ok(mut gpu) = Gpu::new() else { return };
        let (w, h) = (40u32, 30u32);
        let (g, cam) = scene(300, w, h, 11);
        let mut rng = fastrand::Rng::with_seed(12);
        let target: Vec<u32> = (0..w * h).map(|_| rng.u32(..) | 0xff00_0000).collect();
        // A masked band like the capture strip.
        let mask: Vec<f32> = (0..w * h).map(|p| if (5..9).contains(&(p / w)) && (8..30).contains(&(p % w)) { 0.0 } else { 1.0 }).collect();
        gpu.upload(&g);
        gpu.set_mask(w, h, &mask);
        let scene_pixels = mask.iter().filter(|m| **m > 0.5).count() as f32;
        let y: Vec<f64> = target.iter().flat_map(|t| (0..3).map(move |c| ((t >> (8 * c)) & 255) as f64 / 255.0)).collect();
        for lambda in [1.0f32, 0.2] {
            gpu.ssim = lambda;
            gpu.step(&cam, [0.1, 0.2, 0.3], Target::Color { rgba: &target, scene_pixels, weight: 1.0 }, None).expect("step");
            let img = gpu.read_image();
            let dp = gpu.read_d_pixel();
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
            eprintln!("λ {lambda}: worst {worst:.3e} of {scale:.3e}");
            assert!(worst < 2e-3 * scale.max(1e-6), "λ {lambda}: {worst} of {scale}");
        }
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
        let start = std::time::Instant::now();
        let total = gpu.step(&cam, [0.0; 3], Target::None, None).expect("step") as usize;
        let _ = gpu.read_transmittance();
        eprintln!("{total} intersections in {:?}", start.elapsed());
        assert!(total > 1_000_000, "{total}");
        let s = gpu.sort.as_ref().expect("sort");
        let keys: Vec<u32> = gpu.read(&s.keys[0], total);
        let values: Vec<u32> = gpu.read(&s.values[0], total);
        let proj: Vec<f32> = gpu.read(&gpu.splats.as_ref().expect("splats").proj, g.len() * 16);
        let tiles_x = cam.width.div_ceil(cpu::TILE);
        let bits = 32 - (tiles_x * cam.height.div_ceil(cpu::TILE) - 1).leading_zeros();
        let depth_bits = 32 - bits;
        let mut per_gaussian = vec![0u32; g.len()];
        for j in 0..total {
            assert!(j == 0 || keys[j - 1] <= keys[j], "unsorted at {j}");
            let i = values[j] as usize;
            let p = &proj[i * 16..i * 16 + 16];
            let tile = keys[j] >> depth_bits;
            let (tx, ty) = (tile % tiles_x, tile / tiles_x);
            assert!(tx >= p[11].to_bits() && tx < p[13].to_bits() && ty >= p[12].to_bits() && ty < p[14].to_bits(), "entry {j} outside its Gaussian's tiles");
            per_gaussian[i] += 1;
        }
        for (i, &c) in per_gaussian.iter().enumerate() {
            let p = &proj[i * 16..i * 16 + 16];
            let expected = if p[15] == 0.0 { 0 } else { (p[13].to_bits() - p[11].to_bits()) * (p[14].to_bits() - p[12].to_bits()) };
            assert_eq!(c, expected, "Gaussian {i}");
        }
    }
}
