//! Renders egui frames without a window: screenshots for visual review and tests now, the VR overlay's texture
//! later. Uses the same egui-wgpu renderer and glow pipeline as the desktop window.

use anyhow::Context as _;
use egui::{ColorImage, RawInput};
use egui_wgpu::wgpu;

pub struct Offscreen {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
    format: wgpu::TextureFormat,
}

impl Offscreen {
    pub fn new() -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            flags: wgpu::InstanceFlags::default(),
            backend_options: wgpu::BackendOptions::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            display: None,
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .context("no GPU adapter")?;
        // Small allocator blocks and a small D3D12 view heap: the defaults reserve hundreds of MB up front.
        let limits = wgpu::Limits { max_texture_dimension_2d: 8192, max_non_sampler_bindings: 16_384, ..wgpu::Limits::default() };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("orbiters offscreen"),
            required_limits: limits,
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..Default::default()
        }))
        .context("no GPU device")?;
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut renderer = egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions { msaa_samples: 1, dithering: true, ..Default::default() });
        crate::glow::install(&device, format, 1, &mut renderer.callback_resources);
        Ok(Self { device, queue, renderer, format })
    }

    /// Runs one frame of `ui` and applies its texture changes; when `capture`, also paints it and returns the
    /// pixels (sRGB, opaque).
    pub fn frame(&mut self, ctx: &egui::Context, input: RawInput, ui: impl FnMut(&mut egui::Ui), capture: bool) -> anyhow::Result<Option<ColorImage>> {
        let screen = input.screen_rect.unwrap_or(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 820.0)));
        let mut full = ctx.run_ui(input, ui);
        for (id, deltas) in full.textures_delta.set.drain() {
            for delta in deltas {
                self.renderer.update_texture(&self.device, &self.queue, id, &delta);
            }
        }
        let image = if capture { Some(self.paint(ctx, &full, screen)) } else { None };
        for id in full.textures_delta.free.drain() {
            self.renderer.free_texture(&id);
        }
        image.transpose()
    }

    fn paint(&mut self, ctx: &egui::Context, full: &egui::FullOutput, screen: egui::Rect) -> anyhow::Result<ColorImage> {
        let ppp = full.pixels_per_point;
        let jobs = ctx.tessellate(full.shapes.clone(), ppp);
        let (w, h) = ((screen.width() * ppp).round() as u32, (screen.height() * ppp).round() as u32);
        let desc = egui_wgpu::ScreenDescriptor { size_in_pixels: [w, h], pixels_per_point: ppp };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("orbiters offscreen target"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("orbiters offscreen") });
        let mut commands = self.renderer.update_buffers(&self.device, &self.queue, &mut encoder, &jobs, &desc);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("orbiters offscreen"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            self.renderer.render(&mut pass.forget_lifetime(), &jobs, &desc);
        }
        let row = (w * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("orbiters offscreen readback"),
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        commands.push(encoder.finish());
        self.queue.submit(commands);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).context("GPU readback")?;
        let mapped = slice.get_mapped_range().context("map readback")?;
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            let start = y * row as usize;
            rgba.extend_from_slice(&mapped[start..start + w as usize * 4]);
        }
        drop(mapped);
        buffer.unmap();
        Ok(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba))
    }
}

/// Writes an image as PNG.
pub fn save_png(path: &std::path::Path, image: &ColorImage) -> anyhow::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, image.width() as u32, image.height() as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header()?;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    writer.write_image_data(&bytes)?;
    Ok(())
}
