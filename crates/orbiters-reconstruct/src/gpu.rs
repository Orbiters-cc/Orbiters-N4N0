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
    backward: wgpu::ComputePipeline,
    loss_color: wgpu::ComputePipeline,
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
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::PRIMARY, ..wgpu::InstanceDescriptor::new_without_display_handle() });
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
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("orbiters-reconstruct"),
            required_features: wgpu::Features::SUBGROUP,
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
        let common = include_str!("shaders/common.wgsl");
        let module = |name: &str, src: &str, with_common: bool| {
            let code = if with_common { format!("{common}\n{src}") } else { src.to_string() };
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(name), source: wgpu::ShaderSource::Wgsl(code.into()) })
        };
        let pipe = |m: &wgpu::ShaderModule, entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: m,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let project = module("project", include_str!("shaders/project.wgsl"), true);
        let scan = module("scan", include_str!("shaders/scan.wgsl"), false);
        let duplicate = module("duplicate", include_str!("shaders/duplicate.wgsl"), true);
        let sort = module("sort", include_str!("shaders/sort.wgsl"), false);
        let ranges = module("ranges", include_str!("shaders/ranges.wgsl"), true);
        let raster = module("raster", include_str!("shaders/raster.wgsl"), true);
        let loss = module("loss", include_str!("shaders/loss.wgsl"), true);
        let depth_loss = module("depth_loss", include_str!("shaders/depth_loss.wgsl"), true);
        let project_backward = module("project_backward", include_str!("shaders/project_backward.wgsl"), true);
        let adam = module("adam", include_str!("shaders/adam.wgsl"), false);
        let k = Kernels {
            project: pipe(&project, "main"),
            scan_blocks: pipe(&scan, "scan_blocks"),
            add_sums: pipe(&scan, "add_sums"),
            total: pipe(&scan, "total"),
            duplicate: pipe(&duplicate, "main"),
            sort_count: pipe(&sort, "count"),
            sort_scatter: pipe(&sort, "scatter"),
            ranges: pipe(&ranges, "main"),
            forward: pipe(&raster, "forward"),
            backward: pipe(&raster, "backward"),
            loss_color: pipe(&loss, "color"),
            depth_loss: pipe(&depth_loss, "main"),
            project_backward: pipe(&project_backward, "main"),
            adam: pipe(&adam, "main"),
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
        let total_out = storage(&device, "total", 16);
        Ok(Self {
            adapter: format!("{} ({:?})", info.name, info.backend),
            device,
            queue,
            k,
            splats: None,
            image: None,
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
            errors,
        })
    }

    fn bind(&self, pipeline: &wgpu::ComputePipeline, entries: &[(u32, &wgpu::Buffer)]) -> wgpu::BindGroup {
        let layout = pipeline.get_bind_group_layout(0);
        let entries: Vec<wgpu::BindGroupEntry> = entries.iter().map(|(b, buf)| wgpu::BindGroupEntry { binding: *b, resource: buf.as_entire_binding() }).collect();
        self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &entries })
    }

    fn uniform<T: Pod>(&self, v: &T) -> wgpu::Buffer {
        self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::bytes_of(v), usage: wgpu::BufferUsages::UNIFORM })
    }

    fn dispatch(&self, enc: &mut wgpu::CommandEncoder, pipeline: &wgpu::ComputePipeline, bind: &wgpu::BindGroup, (x, y): (u32, u32)) {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
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
        if self.splats.as_ref().is_none_or(|s| s.cap < n) {
            let cap = (n + n / 2).max(1024);
            let d = &self.device;
            let f = |count: usize| (count * 4) as u64;
            self.splats = Some(SplatBuffers {
                cap,
                params: storage(d, "params", f(cap * PACKED)),
                grads: storage(d, "grads", f(cap * PACKED)),
                m1: storage(d, "moment1", f(cap * PACKED)),
                m2: storage(d, "moment2", f(cap * PACKED)),
                proj: storage(d, "projected", f(cap * 16)),
                counts: storage(d, "tile counts", f(cap)),
                offsets: storage(d, "tile offsets", f(cap)),
                grad2d: storage(d, "grad2d", f(cap * GRAD2D)),
                stats: storage(d, "stats", f(cap * 4)),
            });
        }
        let s = self.splats.as_ref().expect("allocated");
        self.queue.write_buffer(&s.params, 0, bytemuck::cast_slice(&g.pack()));
        let zeros = vec![0f32; n * PACKED];
        let (m1, m2) = moments.unwrap_or((&zeros, &zeros));
        self.queue.write_buffer(&s.m1, 0, bytemuck::cast_slice(m1));
        self.queue.write_buffer(&s.m2, 0, bytemuck::cast_slice(m2));
        self.queue.write_buffer(&s.stats, 0, bytemuck::cast_slice(&vec![0f32; n * 4]));
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
        let d = &self.device;
        let px = (w * h) as u64;
        let tiles = (w.div_ceil(cpu::TILE) * h.div_ceil(cpu::TILE)) as u64;
        self.image = Some(ImageBuffers {
            width: w,
            height: h,
            out_color: storage(d, "out colour", px * 16),
            out_state: storage(d, "out state", px * 8),
            d_pixel: storage(d, "d pixel", px * 16),
            target: storage(d, "target", px * 4),
            mask: storage(d, "mask", px * 4),
            ranges: storage(d, "tile ranges", tiles * 8),
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
            reg: if train { [self.opacity_reg, self.scale_reg, 0.0, 0.0] } else { [0.0; 4] },
        }
    }

    /// Renders (and, with a target, takes one optimisation step with `lr`). Returns the number of tile
    /// intersections.
    pub fn step(&mut self, cam: &Camera, background: [f32; 3], target: Target<'_>, lr: Option<LearningRates>) -> anyhow::Result<u32> {
        let n = self.count;
        if n == 0 {
            return Ok(0);
        }
        self.ensure_image(cam.width, cam.height);
        let loss = match &target {
            Target::Color { scene_pixels, weight, .. } => [*weight, 0.0, 0.0, *scene_pixels],
            Target::Depth { samples, weight } => [0.0, *weight, 0.0, samples.iter().filter(|s| s[2] > 0.0).count() as f32],
            Target::None => [0.0; 4],
        };
        let train = !matches!(target, Target::None);
        // Pass 1: project and count tile intersections.
        let frame = self.frame_for(cam, background, 0, loss, train);
        self.queue.write_buffer(&self.frame, 0, bytemuck::bytes_of(&frame));
        let (params, proj, counts, offsets) = {
            let s = self.splats.as_ref().context("no Gaussians")?;
            (s.params.clone(), s.proj.clone(), s.counts.clone(), s.offsets.clone())
        };
        let mut enc = self.device.create_command_encoder(&Default::default());
        let bind = self.bind(&self.k.project, &[(0, &self.frame), (1, &params), (2, &proj), (3, &counts)]);
        self.dispatch(&mut enc, &self.k.project, &bind, groups(n));
        self.scan(&mut enc, &counts, &offsets, n, 0);
        let info = self.uniform(&[n as u32, 0, 0, 0]);
        let total_out = self.total_out.clone();
        let bind = self.bind(&self.k.total, &[(0, &info), (1, &counts), (2, &offsets), (3, &total_out)]);
        self.dispatch(&mut enc, &self.k.total, &bind, (1, 1));
        enc.copy_buffer_to_buffer(&total_out, 0, &self.total_staging, 0, 16);
        self.queue.submit([enc.finish()]);
        self.total_staging.map_async(wgpu::MapMode::Read, .., |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let total = {
            let view = self.total_staging.get_mapped_range(..).expect("mapped");
            bytemuck::cast_slice::<u8, u32>(&view)[0]
        };
        self.total_staging.unmap();

        // Pass 2: sort, rasterise, and with a target: loss, backward, Adam.
        self.ensure_sort(total as usize);
        if let Some(e) = self.errors.lock().unwrap().take() {
            anyhow::bail!("GPU error: {e}");
        }
        // Each sort copy (sized half again over the entries) must fit one storage binding.
        let max_entries = (self.device.limits().max_storage_buffer_binding_size / 4 * 2 / 3).min(u32::MAX as u64) as u32;
        anyhow::ensure!(total <= max_entries, "{total} tile entries in one view: more than the GPU can sort ({max_entries})");
        let frame = self.frame_for(cam, background, total, loss, train);
        self.queue.write_buffer(&self.frame, 0, bytemuck::bytes_of(&frame));
        let (keys, values, hist, hist_offsets) = {
            let s = self.sort.as_ref().expect("sort");
            (s.keys.clone(), s.values.clone(), s.hist.clone(), s.hist_offsets.clone())
        };
        let (out_color, out_state, d_pixel, target_buf, mask, ranges) = {
            let i = self.image.as_ref().expect("image");
            (i.out_color.clone(), i.out_state.clone(), i.d_pixel.clone(), i.target.clone(), i.mask.clone(), i.ranges.clone())
        };
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.clear_buffer(&ranges, 0, None);
        if total > 0 {
            let bind = self.bind(&self.k.duplicate, &[(0, &self.frame), (1, &proj), (2, &offsets), (3, &keys[0]), (4, &values[0])]);
            self.dispatch(&mut enc, &self.k.duplicate, &bind, groups(n));
            let blocks = (total as usize).div_ceil(256);
            for pass in 0..8u32 {
                let (a, b) = ((pass % 2) as usize, ((pass + 1) % 2) as usize);
                let info = self.uniform(&SortInfo { n: total, blocks: blocks as u32, shift: pass * 4, pad: 0 });
                let bind = self.bind(&self.k.sort_count, &[(0, &info), (1, &keys[a]), (3, &hist)]);
                self.dispatch(&mut enc, &self.k.sort_count, &bind, groups_of(blocks));
                self.scan(&mut enc, &hist, &hist_offsets, blocks * 16, 0);
                let bind = self.bind(&self.k.sort_scatter, &[(0, &info), (1, &keys[a]), (2, &values[a]), (3, &hist_offsets), (4, &keys[b]), (5, &values[b])]);
                self.dispatch(&mut enc, &self.k.sort_scatter, &bind, groups_of(blocks));
            }
            let bind = self.bind(&self.k.ranges, &[(0, &self.frame), (1, &keys[0]), (2, &ranges)]);
            self.dispatch(&mut enc, &self.k.ranges, &bind, groups(total as usize));
        }
        let tiles = (cam.width.div_ceil(cpu::TILE), cam.height.div_ceil(cpu::TILE));
        let bind = self.bind(&self.k.forward, &[(0, &self.frame), (1, &proj), (2, &values[0]), (3, &ranges), (4, &out_color), (5, &out_state)]);
        self.dispatch(&mut enc, &self.k.forward, &bind, tiles);
        match target {
            Target::Color { rgba, .. } => {
                self.queue.write_buffer(&target_buf, 0, bytemuck::cast_slice(rgba));
                let bind = self.bind(&self.k.loss_color, &[(0, &self.frame), (1, &out_color), (2, &target_buf), (3, &mask), (4, &d_pixel)]);
                self.dispatch(&mut enc, &self.k.loss_color, &bind, groups((cam.width * cam.height) as usize));
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
                let bind = self.bind(&self.k.depth_loss, &[(0, &self.frame), (1, &out_color), (2, &buf), (3, &d_pixel)]);
                self.dispatch(&mut enc, &self.k.depth_loss, &bind, groups(padded.len()));
            }
            Target::None => {}
        }
        if train {
            self.backward_into(&mut enc, &proj, &values[0], &ranges, &out_state, &d_pixel, tiles);
            if let Some(lr) = lr {
                self.adam(&mut enc, lr);
            }
        }
        self.queue.submit([enc.finish()]);
        Ok(total)
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
        let bind = self.bind(&self.k.backward, &[(0, &self.frame), (1, proj), (2, values), (3, ranges), (5, out_state), (6, d_pixel), (7, &s.grad2d)]);
        self.dispatch(enc, &self.k.backward, &bind, tiles);
        let bind = self.bind(&self.k.project_backward, &[(0, &self.frame), (1, &s.params), (2, proj), (3, &s.grad2d), (4, &s.grads), (5, &s.stats)]);
        self.dispatch(enc, &self.k.project_backward, &bind, groups(self.count));
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
        let bind = self.bind(&self.k.adam, &[(0, &info), (1, &s.params), (2, &s.grads), (3, &s.m1), (4, &s.m2)]);
        self.dispatch(enc, &self.k.adam, &bind, groups(n));
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

    /// The last rendered image: (r, g, b, depth) per pixel.
    pub fn read_image(&self) -> Vec<[f32; 4]> {
        let i = self.image.as_ref().expect("image");
        let px = (i.width * i.height) as usize;
        self.read::<f32>(&i.out_color, px * 4).as_chunks::<4>().0.to_vec()
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
