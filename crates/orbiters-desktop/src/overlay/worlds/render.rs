//! The cube's pictures drawn on the GPU: the capture's splats through the splat renderer of the 3D view
//! (`orbiters_reconstruct::gpu::Gpu`, with the viewer's guards), one off-axis camera per face and eye
//! ([`super::stereo::Cube::camera`]), each picture copied into its half of the face's side-by-side texture.

use super::catalog::Capture;
use super::stereo::{Cube, Placement, PlayCamera, V3};
use anyhow::{anyhow, Context as _};
use orbiters_i18n::tr;
use eframe::wgpu;
use orbiters_reconstruct::gpu::{block_on, Gpu, Target};
use std::time::Instant;

/// What shows where no splat covers and the capture has no sky map: its horizon's colour, else this.
const BACKGROUND: [f32; 3] = [0.02, 0.02, 0.025];

/// A wgpu device on the GPU with DXGI adapter LUID `luid` (the headset's), DX12 on Windows; any high-performance GPU
/// without one (the offline review).
pub fn device(luid: Option<u64>) -> anyhow::Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    let backends = if cfg!(windows) { wgpu::Backends::DX12 } else { wgpu::Backends::VULKAN };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends, ..wgpu::InstanceDescriptor::new_without_display_handle() });
    let adapters = block_on(instance.enumerate_adapters(backends));
    let adapter = match luid {
        Some(luid) => adapters.into_iter().find(|a| adapter_luid(a) == Some(luid)).with_context(|| tr("the headset's GPU is not among wgpu's adapters"))?,
        None => {
            block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() }))
                .map_err(|e| anyhow!("{}", tr!("no GPU adapter: {error}", error = e)))?
        }
    };
    let desc = wgpu::DeviceDescriptor {
        label: Some("3d-worlds"),
        required_limits: adapter.limits(),
        memory_hints: wgpu::MemoryHints::MemoryUsage,
        ..Default::default()
    };
    let (device, queue) = block_on(adapter.request_device(&desc)).map_err(|e| anyhow!("{}", tr!("GPU device: {error}", error = e)))?;
    Ok((adapter, device, queue))
}

/// The DXGI adapter LUID of a wgpu adapter (DX12 only), as one number (high part above the low one).
pub fn adapter_luid(adapter: &wgpu::Adapter) -> Option<u64> {
    #[cfg(windows)]
    {
        // SAFETY: a read-only query of the adapter's DXGI object.
        unsafe {
            let hal = adapter.as_hal::<wgpu::hal::api::Dx12>()?;
            let desc = hal.as_raw().GetDesc1().ok()?;
            Some(((desc.AdapterLuid.HighPart as u32 as u64) << 32) | desc.AdapterLuid.LowPart as u64)
        }
    }
    #[cfg(not(windows))]
    {
        let _ = adapter;
        None
    }
}

/// Draws a capture's faces into textures.
pub struct FaceRenderer {
    /// The middle and reach of what is drawn, where marks leave the rest out.
    clipped: Option<(V3, f32)>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    gpu: Gpu,
    /// One eye's picture of a face, before it is copied into its half.
    scratch: Option<(u32, wgpu::Texture, wgpu::TextureView)>,
    /// Each face's latest pictures (both eyes): what new pictures are drawn into, then copied to SteamVR's texture.
    masters: Vec<wgpu::Texture>,
    /// Each eye's wide view, and the pass that resamples it into the faces.
    wide: [Option<(wgpu::Texture, wgpu::TextureView)>; 2],
    resample: Resample,
    background: [f32; 3],
    /// What the splats leave shows what is behind the picture: a capture standing small in the room.
    transparent: bool,
    total: usize,
    overflows_seen: u32,
}

/// The compute pass that writes a wide view's pixels into a face's texels it covers (a homography per face and eye),
/// leaving the others as they were.
struct Resample {
    pipeline: wgpu::ComputePipeline,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
}

/// The uniforms' slots: a dispatch each in one batch (six faces, two eyes).
const RESAMPLE_SLOTS: u64 = 12;
const RESAMPLE_SLOT: u64 = 256;

const RESAMPLE_WGSL: &str = include_str!("resample.wgsl");

impl Resample {
    fn new(device: &wgpu::Device) -> Self {
        let module = device
            .create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("3d-worlds resample"), source: wgpu::ShaderSource::Wgsl(RESAMPLE_WGSL.into()) }); // i18n: not shown (a GPU debug label)
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("3d-worlds resample"), // i18n: not shown (a GPU debug label)
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("3d-worlds wide"), // i18n: not shown (a GPU debug label)
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("3d-worlds resample uniforms"), // i18n: not shown (a GPU debug label)
            size: RESAMPLE_SLOTS * RESAMPLE_SLOT,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, sampler, uniforms }
    }
}

/// From how far out of its reach a capture standing small fades to nothing at its edge.
const SMALL_FADES_FROM: f32 = 0.8;

impl FaceRenderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let gpu = Gpu::renderer(device.clone(), queue.clone());
        let resample = Resample::new(&device);
        Self { clipped: None, transparent: false, device, queue, gpu, scratch: None, masters: Vec::new(), wide: [None, None], resample, background: BACKGROUND, total: 0, overflows_seen: 0 }
    }

    /// Only what lies within `reach` of `middle` is drawn, with the middle moved away from where the splats are sorted from. `None`, all of them again. Asked with each batch, read
    /// again when it moved enough to see. On this thread, as a capture has millions: the marks themselves cost a frame nothing (`Gpu::set_marks`).
    pub fn clip(&mut self, clip: Option<&(V3, f32, std::sync::Arc<Capture>)>) {
        let Some((middle, reach, capture)) = clip else {
            if self.clipped.take().is_some() {
                self.gpu.count = self.total;
                self.gpu.set_marks(&[]);
            }
            return;
        };
        let near = |a: f32, b: f32| (a - b).abs() < *reach * 0.02;
        if self.clipped.is_some_and(|(c, r)| near(c[0], middle[0]) && near(c[2], middle[2]) && near(r, *reach)) {
            return;
        }
        self.clipped = Some((*middle, *reach));
        let within = reach * reach;
        // Its last part fades out (round 18's judges: a ragged cut through a shack and across the floor): whole to here, then thinner to nothing at its reach.
        let whole = (reach * SMALL_FADES_FROM) * (reach * SMALL_FADES_FROM);
        let marks: Vec<u8> = capture
            .gaussians
            .means
            .iter()
            .map(|m| {
                // Across the ground only: moved, it shows the same round as before, with nothing cut for its height.
                let (x, z) = (m[0] - middle[0], m[2] - middle[2]);
                let far = x * x + z * z;
                if far <= whole {
                    0
                } else if far <= within {
                    // 128 whole to 255 nothing (the shader's faded marks).
                    let t = (far.sqrt() - reach * SMALL_FADES_FROM) / (reach * (1.0 - SMALL_FADES_FROM));
                    128 + (t.clamp(0.0, 1.0) * 127.0) as u8
                } else {
                    2
                }
            })
            .collect();
        self.gpu.count = self.total;
        self.gpu.set_marks(&marks);
    }

    /// Uploads `capture`'s splats and sky map (the first ones drawn at most later: [`FaceRenderer::render`]'s count).
    pub fn load(&mut self, capture: &Capture) -> anyhow::Result<()> {
        self.gpu.upload(&capture.gaussians);
        self.clipped = None;
        self.gpu.set_sky(capture.sky.as_ref().map(|s| (s.width, s.rgb.as_slice())))?;
        self.background = capture.sky.as_ref().map(|s| s.horizon()).or_else(|| orbiters_reconstruct::sky::horizon(&capture.gaussians)).unwrap_or(BACKGROUND);
        self.total = capture.gaussians.len();
        self.transparent = false;
        Ok(())
    }

    /// What was just loaded shows with nothing around it, no sky and no background.
    pub fn make_transparent(&mut self) -> anyhow::Result<()> {
        self.gpu.set_sky(None)?;
        self.background = [0.0; 3];
        self.transparent = true;
        Ok(())
    }

    fn present_on(gpu: &orbiters_reconstruct::gpu::Gpu, transparent: bool, view: &wgpu::TextureView) {
        if transparent {
            gpu.present_alpha(view);
        } else {
            gpu.present(view);
        }
    }

    fn scratch(&mut self, width: u32) -> &wgpu::TextureView {
        if self.scratch.as_ref().is_none_or(|s| s.0 != width) {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("3d-worlds eye picture"), // i18n: not shown (a GPU debug label)
                size: wgpu::Extent3d { width, height: width, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.scratch = Some((width, texture, view));
        }
        &self.scratch.as_ref().expect("made above").2
    }

    /// Draws face `face` of `cube` for the eye at `eye` (`eye_index` 0 left, 1 right) into its half of `target` (a
    /// texture twice the cube's width across), the first `count` splats only.
    pub fn render(
        &mut self,
        cube: &Cube,
        placement: &Placement,
        eye: V3,
        eye_index: usize,
        face: usize,
        count: usize,
        target: &wgpu::Texture,
    ) -> anyhow::Result<()> {
        let width = cube.width();
        let camera = cube.camera(face, eye, placement);
        self.gpu.count = count.min(self.total);
        if self.gpu.count == 0 {
            // Nothing shows yet: the background alone, as the renderer would draw it with no splat.
            self.gpu.count = 1.min(self.total);
        }
        self.gpu.step(&camera, self.background, Target::None, None)?;
        self.scratch(width);
        let (_, texture, view) = self.scratch.as_ref().expect("made above");
        Self::present_on(&self.gpu, self.transparent, view);
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyTextureInfo {
                texture: target,
                mip_level: 0,
                origin: wgpu::Origin3d { x: eye_index as u32 * width, y: 0, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d { width, height: width, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        Ok(())
    }

    /// Face `face`'s latest pictures, a texture twice `width` across (made, empty, when the width changed).
    pub fn master(&mut self, face: usize, width: u32) -> &wgpu::Texture {
        if self.masters.first().is_none_or(|t| t.height() != width) {
            self.masters = (0..6)
                .map(|_| {
                    self.device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("3d-worlds face pictures"), // i18n: not shown (a GPU debug label)
                        size: wgpu::Extent3d { width: 2 * width, height: width, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
                        view_formats: &[],
                    })
                })
                .collect();
        }
        &self.masters[face]
    }

    /// Draws face `face` for the eye at `eye` into its pictures (see [`FaceRenderer::render`]).
    pub fn render_master(&mut self, cube: &Cube, placement: &Placement, eye: V3, eye_index: usize, face: usize, count: usize) -> anyhow::Result<()> {
        let target = self.master(face, cube.width()).clone();
        self.render(cube, placement, eye, eye_index, face, count, &target)
    }

    /// Draws eye `eye_index`'s wide view `camera` (play space) of the first `count` splats.
    pub fn render_wide(&mut self, eye_index: usize, camera: &PlayCamera, placement: &Placement, count: usize) -> anyhow::Result<()> {
        if self.wide[eye_index].as_ref().is_none_or(|(t, _)| t.width() != camera.width || t.height() != camera.height) {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("3d-worlds wide view"), // i18n: not shown (a GPU debug label)
                size: wgpu::Extent3d { width: camera.width, height: camera.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.wide[eye_index] = Some((texture, view));
        }
        self.gpu.count = count.min(self.total).max(1.min(self.total));
        self.gpu.step(&camera.camera(placement), self.background, Target::None, None)?;
        let (_, view) = self.wide[eye_index].as_ref().expect("made above");
        Self::present_on(&self.gpu, self.transparent, view);
        Ok(())
    }

    /// Resamples the eyes' wide views (drawn by [`FaceRenderer::render_wide`]) into the faces' pictures: `jobs` lists
    /// (face, eye, its wide camera); then each face listed in `publish` is copied to its target.
    pub fn resample(&mut self, cube: &Cube, jobs: &[(usize, usize, PlayCamera)], publish: &[(usize, &wgpu::Texture)]) {
        let width = cube.width();
        self.master(0, width);
        let mut enc = self.device.create_command_encoder(&Default::default());
        for (slot, &(face, eye, camera)) in jobs.iter().take(RESAMPLE_SLOTS as usize).enumerate() {
            let Some((_, wide_view)) = self.wide[eye].as_ref() else { continue };
            let h = cube.homography(face, &camera);
            let mut bytes = [0u8; 64];
            for (r, row) in h.iter().enumerate() {
                for (c, v) in row.iter().enumerate() {
                    bytes[r * 16 + c * 4..r * 16 + c * 4 + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
            for (i, v) in [width, camera.width, camera.height, eye as u32 * width].iter().enumerate() {
                bytes[48 + i * 4..52 + i * 4].copy_from_slice(&v.to_le_bytes());
            }
            let offset = slot as u64 * RESAMPLE_SLOT;
            self.queue.write_buffer(&self.resample.uniforms, offset, &bytes);
            let face_view = self.masters[face].create_view(&Default::default());
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("3d-worlds resample"), // i18n: not shown (a GPU debug label)
                layout: &self.resample.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.resample.uniforms,
                            offset,
                            size: wgpu::BufferSize::new(64),
                        }),
                    },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(wide_view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.resample.sampler) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&face_view) },
                ],
            });
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.resample.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(width.div_ceil(16), width.div_ceil(16), 1);
        }
        for &(face, target) in publish {
            enc.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo { texture: &self.masters[face], mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::TexelCopyTextureInfo { texture: target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::Extent3d { width: 2 * width, height: width, depth_or_array_layers: 1 },
            );
        }
        self.queue.submit([enc.finish()]);
    }

    /// Waits until the GPU finished what was asked. Returns whether a picture drawn since left splats out (the
    /// renderer's sort was short for it): those faces are drawn again.
    pub fn finish(&mut self) -> anyhow::Result<bool> {
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow!("{}", tr!("waiting for the GPU: {error}", error = e)))?;
        let redraw = self.gpu.needs_redraw();
        // A later eye can consume and clear the renderer's redraw flag for an earlier eye. The monotonic counter
        // still remembers that the stereo batch lost entries, so the complete batch is refreshed.
        let overflowed = self.gpu.overflows != self.overflows_seen;
        self.overflows_seen = self.gpu.overflows;
        Ok(redraw || overflowed)
    }

    /// Draws the eye's own view (as a headset would show it), into an RGBA8 picture: the reference the cube's
    /// pictures are compared with offline.
    pub fn render_view(&mut self, camera: &orbiters_reconstruct::camera::Camera, count: usize) -> anyhow::Result<Vec<u8>> {
        self.gpu.count = count.min(self.total).max(1.min(self.total));
        self.gpu.step(camera, self.background, Target::None, None)?;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("3d-worlds reference view"), // i18n: not shown (a GPU debug label)
            size: wgpu::Extent3d { width: camera.width, height: camera.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self::present_on(&self.gpu, self.transparent, &texture.create_view(&Default::default()));
        read_texture(&self.device, &self.queue, &texture)
    }
}

/// A face texture: twice `width` across (left eye, right eye), `width` high, RGBA8 holding encoded sRGB.
pub fn face_texture(device: &wgpu::Device, width: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("3d-worlds face"), // i18n: not shown (a GPU debug label)
        size: wgpu::Extent3d { width: 2 * width, height: width, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// A texture's pixels (RGBA8, rows packed).
pub fn read_texture(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> anyhow::Result<Vec<u8>> {
    let (w, h) = (texture.width(), texture.height());
    let row = (w * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("3d-worlds readback"), // i18n: not shown (a GPU debug label)
        size: (row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow!("{}", tr!("waiting for the GPU: {error}", error = e)))?;
    let data = slice.get_mapped_range().map_err(|e| anyhow!("{}", tr!("reading back: {error}", error = format!("{e:?}"))))?;
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize {
        out.extend_from_slice(&data[y * row as usize..y * row as usize + w as usize * 4]);
    }
    drop(data);
    buffer.unmap();
    Ok(out)
}

/// Times a closure, in milliseconds.
pub fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let out = f();
    (out, start.elapsed().as_secs_f64() * 1000.0)
}

#[cfg(test)]
mod small_tests {
    use super::super::stereo::{self, Small};
    use super::*;

    /// Draws a real session small: `ORBIT_SESSION=<its folder> ORBIT_OUT=<a .png>`. What surrounds it must be transparent,
    /// which only a headset could otherwise tell.
    #[test]
    #[ignore = "needs a GPU and a real session"]
    fn a_capture_small_stands_in_nothing() {
        let dir = std::path::PathBuf::from(std::env::var("ORBIT_SESSION").unwrap());
        let out = std::path::PathBuf::from(std::env::var("ORBIT_OUT").unwrap());
        let full = super::super::catalog::read_local(&dir, &Default::default()).unwrap();
        let capture = full.around_subject();
        // A head at the room's middle, looking down -Z.
        let head = orbiters_openvr::Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.6], [0.0, 0.0, 1.0, 0.0]] };
        let small = Small::new(&capture.view, &head, capture.small_reach());
        let placement = small.placement(capture.view.eye());
        println!("{} splats, showing {} within {} m at size {}", capture.gaussians.len(), capture.within(small.reach()), small.reach(), small.size());
        let (_adapter, device, queue) = super::device(None).unwrap();
        let mut renderer = FaceRenderer::new(device, queue);
        renderer.load(&capture).unwrap();
        renderer.make_transparent().unwrap();
        let (w, h) = (1024, 1024);
        let camera = stereo::eye_camera(&head, [-1.0, 1.0, -1.0, 1.0], w, h, &placement);
        let t = std::time::Instant::now();
        let pixels = renderer.render_view(&camera, capture.within(small.reach())).unwrap();
        println!("drawn in {:?}", t.elapsed());
        let alpha = |x: u32, y: u32| pixels[((y * w + x) * 4 + 3) as usize];
        for (x, y) in [(2, 2), (w - 3, 2), (2, h - 3), (w - 3, h - 3)] {
            assert_eq!(alpha(x, y), 0, "nothing in the corner {x},{y}");
        }
        let covered = pixels.chunks_exact(4).filter(|p| p[3] > 200).count();
        let clear = pixels.chunks_exact(4).filter(|p| p[3] == 0).count();
        println!("{:.1}% solid, {:.1}% clear", covered as f32 * 100.0 / (w * h) as f32, clear as f32 * 100.0 / (w * h) as f32);
        assert!(covered > 1000, "it shows");
        assert!(pixels.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]), "premultiplied");
        // Over a checkerboard, as the room behind it.
        let mut shown = pixels.clone();
        for (i, p) in shown.chunks_exact_mut(4).enumerate() {
            let (x, y) = (i as u32 % w, i as u32 / w);
            let back: u32 = if (x / 32 + y / 32) % 2 == 0 { 70 } else { 110 };
            for c in 0..3 {
                p[c] = (p[c] as u32 + back * (255 - p[3] as u32) / 255).min(255) as u8;
            }
            p[3] = 255;
        }
        super::super::shots::write_png(&out, w, h, &shown).unwrap();
    }

    /// A camera at `eye` looking at `at` (play space), up kept up.
    fn looking(eye: stereo::V3, at: stereo::V3) -> orbiters_openvr::Matrix34 {
        let back = stereo::sub(eye, at);
        let back = stereo::scale(back, 1.0 / stereo::length(back));
        let right = stereo::cross([0.0, 1.0, 0.0], back);
        let right = stereo::scale(right, 1.0 / stereo::length(right));
        let up = stereo::cross(back, right);
        orbiters_openvr::Matrix34 { m: [[right[0], up[0], back[0], eye[0]], [right[1], up[1], back[1], eye[1]], [right[2], up[2], back[2], eye[2]]] }
    }

    /// Floaters' evidence: `ORBIT_SESSION` drawn small before and after its floaters go, and only what went, from the same
    /// cameras, into `ORBIT_OUT_DIR` as `<ORBIT_NAME>-<camera>-{before,after,removed}.png`, with what it took and how much of the
    /// picture the removed splats made. `FLOATER_RULE` ("faint,large,round,blob,solid,around,empty") tries another rule, `ORBIT_MASK`
    /// (a byte a splat) another choice; `ORBIT_MASK_OUT` writes the rule's; `ORBIT_DUMP` writes the capture and its small view.
    #[test]
    #[ignore = "needs a GPU and a real session"]
    fn floaters_before_and_after() {
        use super::super::floaters;
        let dir = std::path::PathBuf::from(std::env::var("ORBIT_SESSION").unwrap());
        let out = std::path::PathBuf::from(std::env::var("ORBIT_OUT_DIR").unwrap());
        let name = std::env::var("ORBIT_NAME").unwrap_or_else(|_| "capture".into());
        std::fs::create_dir_all(&out).unwrap();
        let full = super::super::catalog::read_local(&dir, &Default::default()).unwrap();
        let rule = match std::env::var("FLOATER_RULE") {
            Ok(r) => {
                let v: Vec<f32> = r.split(',').map(|x| x.trim().parse().unwrap()).collect();
                floaters::Rule { faint: v[0], large: v[1], round: v[2], blob: v[3], solid: v[4], around: v[5], empty: v[6] }
            }
            Err(_) => floaters::Rule::default(),
        };
        // Timed as it runs when a capture opens, a few times.
        let mut floating = Vec::new();
        let mut times = Vec::new();
        for _ in 0..5 {
            let t = std::time::Instant::now();
            floating = floaters::floating_by(&full.gaussians, &rule);
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        if let Ok(out) = std::env::var("ORBIT_MASK_OUT") {
            std::fs::write(out, floating.iter().map(|f| *f as u8).collect::<Vec<u8>>()).unwrap();
        }
        if let Ok(mask) = std::env::var("ORBIT_MASK") {
            floating = std::fs::read(mask).unwrap().into_iter().map(|b| b != 0).collect();
        }
        times.sort_by(f64::total_cmp);
        let removed = floating.iter().filter(|f| **f).count();
        let n = full.gaussians.len();
        let g = &full.gaussians;
        let alpha = |i: usize| 1.0 / (1.0 + (-g.opacity_logits[i]).exp());
        let weight = |i: usize| alpha(i) * g.log_scales[i].iter().map(|s| s.exp()).product::<f32>().powf(2.0 / 3.0);
        let (mut all_w, mut gone_w) = (0.0f64, 0.0f64);
        for i in 0..n {
            all_w += weight(i) as f64;
            if floating[i] {
                gone_w += weight(i) as f64;
            }
        }
        println!(
            "{name}: {rule:?}: {n} splats in, {removed} removed ({:.2}%), {:.1} ms (median of 5, fastest {:.1}); removed share of opacity x area {:.2}%",
            removed as f64 * 100.0 / n as f64,
            times[2],
            times[0],
            gone_w * 100.0 / all_w
        );
        let before = full.small_keeping(&vec![true; n]);
        let after = full.small_keeping(&floating.iter().map(|f| !f).collect::<Vec<_>>());
        let only = full.small_keeping(&floating);
        let head = orbiters_openvr::Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.6], [0.0, 0.0, 1.0, 0.0]] };
        let small = Small::new(&before.view, &head, before.small_reach());
        let placement = small.placement(before.view.eye());
        let middle = small.anchor;
        if let Ok(dump) = std::env::var("ORBIT_DUMP") {
            use std::io::Write;
            let mut f = std::io::BufWriter::new(std::fs::File::create(&dump).unwrap());
            f.write_all(&(n as u32).to_le_bytes()).unwrap();
            for i in 0..n {
                for v in g.means[i].iter().chain(&g.log_scales[i]).chain(&g.rotations[i]).chain(std::iter::once(&g.opacity_logits[i])).chain(&g.colors[i]) {
                    f.write_all(&v.to_le_bytes()).unwrap();
                }
            }
            let frame = full.small_frame();
            let eye = placement.invert(head.m.map(|r| r[3]));
            let at = placement.invert(middle);
            std::fs::write(
                format!("{dump}.json"),
                format!(
                    "{{\"middle\":{:?},\"reach\":{},\"ground\":{},\"eye\":{:?},\"at\":{:?},\"rule\":{:?}}}",
                    frame.middle,
                    frame.reach,
                    frame.ground,
                    eye,
                    at,
                    format!("{rule:?}")
                ),
            )
            .unwrap();
        }
        let (_adapter, device, queue) = super::device(None).unwrap();
        let mut renderer = FaceRenderer::new(device, queue);
        let (w, h) = (1024u32, 1024u32);
        let tan = 0.62;
        // As held before the eyes, then from higher and from a side, closer.
        let cameras = [
            ("front", looking(head.m.map(|r| r[3]), middle)),
            ("above", looking(stereo::add(middle, [0.0, 0.42, 0.42]), middle)),
            ("side", looking(stereo::add(middle, [0.45, 0.25, 0.3]), middle)),
        ];
        // Each camera's picture before, to compare the others with.
        let mut first: Vec<Vec<u8>> = Vec::new();
        let shown_removed = only.within(small.reach());
        println!("{name}: within the small view {} splats, {} of them removed", before.within(small.reach()), shown_removed);
        for (which, capture) in [("before", &before), ("after", &after), ("removed", &only)] {
            renderer.load(capture).unwrap();
            renderer.make_transparent().unwrap();
            let count = capture.within(small.reach());
            for (c, (camera_name, view)) in cameras.iter().enumerate() {
                let camera = stereo::eye_camera(view, [-tan, tan, -tan, tan], w, h, &placement);
                let pixels = renderer.render_view(&camera, count).unwrap();
                if which == "before" {
                    first.push(pixels.clone());
                } else {
                    // Removed: how opaque what went is against what was there. After: how many pixels changed visibly.
                    let solid = |p: &[u8]| p.chunks_exact(4).map(|q| q[3] as f64).sum::<f64>();
                    let changed =
                        pixels.chunks_exact(4).zip(first[c].chunks_exact(4)).filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 24)).count();
                    let seen = first[c].chunks_exact(4).filter(|p| p[3] > 8).count().max(1);
                    match which {
                        "removed" => println!(
                            "{name} {camera_name}: what went is {:.1}% of the opacity drawn before",
                            solid(&pixels) * 100.0 / solid(&first[c]).max(1.0)
                        ),
                        _ => println!("{name} {camera_name}: {:.1}% of the capture's pixels changed by more than 24/255", changed as f64 * 100.0 / seen as f64),
                    }
                }
                let mut shown = pixels.clone();
                for (i, p) in shown.chunks_exact_mut(4).enumerate() {
                    let (x, y) = (i as u32 % w, i as u32 / w);
                    let back: u32 = if (x / 32 + y / 32) % 2 == 0 { 70 } else { 110 };
                    for c in 0..3 {
                        p[c] = (p[c] as u32 + back * (255 - p[3] as u32) / 255).min(255) as u8;
                    }
                    p[3] = 255;
                }
                super::super::shots::write_png(&out.join(format!("{name}-{camera_name}-{which}.png")), w, h, &shown).unwrap();
            }
        }
    }
}
