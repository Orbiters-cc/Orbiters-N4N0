//! The 3D view drawn on the processor's integrated GPU while the window draws on the graphics card
//! ([`orbiters_core::settings::ViewGpu`]): a thread of its own renders the splats on a device there, reads each frame
//! back and hands it over, and the window uploads the newest to a texture on its own device and shows it. Only those
//! pictures reach the graphics card (1.6 MB a frame while the camera moves, at most [`super::splats`]'s moving pixels).
//! The window never waits for the integrated GPU: a view asked for while a frame is drawn replaces the one asked
//! before, and the window shows the last frame until the next is there.

use super::splats::{Blit, BACKGROUND, REDRAW_CHECKS};
use eframe::egui_wgpu::{self, wgpu};
use orbiters_reconstruct::camera::Camera;
use orbiters_reconstruct::gpu::{Confidence, DepthOfField, FarField, Gpu, Target};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;

/// Frames drawn on the integrated GPU since the app started (the GPU bench counts them).
pub static DRAWN: AtomicU64 = AtomicU64::new(0);

/// A view of the splats: the camera, how poorly seen and far splats show, the depth of field, the auto focus's pixel
/// and the version of the marks it draws ([`Worker::set_marks`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Request {
    pub camera: Camera,
    pub confidence: Confidence,
    pub far: FarField,
    pub dof: DepthOfField,
    pub probe: Option<[f32; 2]>,
    pub marks: u64,
}

/// A drawn frame: RGBA8, encoded sRGB, rows packed.
struct Frame {
    size: (u32, u32),
    pixels: Vec<u8>,
}

#[derive(Default)]
struct State {
    asked: Option<Request>,
    frame: Option<Frame>,
    error: Option<String>,
    /// The auto focus's distance under its probe, as last read back.
    focus: Option<f32>,
    /// Marks the next frame draws with (the selection's, `splats/select.rs`), until the thread takes them.
    marks: Option<Arc<Vec<u8>>>,
    quit: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

/// The texture on the window's device that shows the last frame.
struct Shown {
    size: (u32, u32),
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    /// Frames shown in it.
    frames: u64,
}

/// The texture showing the last frame, for what draws over it (Edit in 3D's layers, `splats/edit3d.rs`).
#[derive(Clone)]
pub struct Latest(Arc<Mutex<Option<Shown>>>);

impl Latest {
    /// The texture, its size and how many frames it has shown.
    pub fn get(&self) -> Option<(wgpu::TextureView, (u32, u32), u64)> {
        let s = self.0.lock().ok()?;
        s.as_ref().map(|s| (s.texture.create_view(&Default::default()), s.size, s.frames))
    }
}

pub struct Worker {
    shared: Arc<Shared>,
    shown: Arc<Mutex<Option<Shown>>>,
    last: Option<Request>,
    /// The version of the marks handed over last (those `gpu` was started with: `marks`).
    marks: u64,
}

impl Worker {
    /// Starts drawing with `gpu`, a renderer on the integrated GPU's `device` and `queue` with the splats uploaded and
    /// marks of version `marks` set; `ctx` repaints for each frame.
    pub fn start(ctx: &egui::Context, gpu: Gpu, device: wgpu::Device, queue: wgpu::Queue, marks: u64) -> Self {
        let shared = Arc::new(Shared::default());
        let (s, ctx) = (shared.clone(), ctx.clone());
        std::thread::Builder::new().name("splats on the integrated GPU".into()).spawn(move || run(&s, &ctx, gpu, &device, &queue)).expect("thread"); // i18n: not shown
        Self { shared, shown: Arc::default(), last: None, marks }
    }

    /// Asks for this view, unless it is the one asked last.
    pub fn ask(&mut self, request: Request) {
        if self.last != Some(request) {
            self.last = Some(request);
            self.shared.state.lock().unwrap().asked = Some(request);
            self.shared.wake.notify_one();
        }
    }

    /// Draws the splats with `marks` (one byte each: orbiters_reconstruct::gpu::Gpu::set_marks) from the next frame on;
    /// the request naming their `version` draws again.
    pub fn set_marks(&mut self, version: u64, marks: Arc<Vec<u8>>) {
        if self.marks != version {
            self.marks = version;
            self.shared.state.lock().unwrap().marks = Some(marks);
        }
    }

    /// The version of the marks last handed over.
    pub fn marks(&self) -> u64 {
        self.marks
    }

    /// Why the last view could not be drawn.
    pub fn error(&self) -> Option<String> {
        self.shared.state.lock().unwrap().error.clone()
    }

    /// The auto focus's distance under the probe of the last frame that had one.
    pub fn focus(&self) -> Option<f32> {
        self.shared.state.lock().unwrap().focus
    }

    /// The paint callback that shows the last frame.
    pub fn callback(&self) -> impl egui_wgpu::CallbackTrait {
        FrameCallback { shared: self.shared.clone(), shown: self.shown.clone() }
    }

    /// Where the callback shows the last frame.
    pub fn shown(&self) -> Latest {
        Latest(self.shown.clone())
    }
}

impl Drop for Worker {
    /// The thread ends after the frame it may be drawing (not waited for: the window carries on).
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().quit = true;
        self.shared.wake.notify_one();
    }
}

/// The worker's thread: draws what is asked, then for a few frames asks the GPU whether a frame left splats out (its
/// tile count comes back later, [`Gpu::needs_redraw`]) and draws it again if so.
fn run(shared: &Shared, ctx: &egui::Context, mut gpu: Gpu, device: &wgpu::Device, queue: &wgpu::Queue) {
    let mut readback: Option<Readback> = None;
    let mut last: Option<Request> = None;
    let mut checks = 0u8;
    loop {
        let (asked, marks) = {
            let mut s = shared.state.lock().unwrap();
            let asked = loop {
                if s.quit {
                    return;
                }
                if let Some(r) = s.asked.take() {
                    break Some(r);
                }
                if checks == 0 {
                    s = shared.wake.wait(s).unwrap();
                    continue;
                }
                s = shared.wake.wait_timeout(s, Duration::from_millis(16)).unwrap().0;
                if s.asked.is_none() && !s.quit {
                    break None;
                }
            };
            (asked, s.marks.take())
        };
        if let Some(m) = marks {
            gpu.set_marks(&m);
        }
        let request = match (asked, last) {
            (Some(r), _) => r,
            (None, Some(r)) => {
                checks -= 1;
                if !gpu.needs_redraw() {
                    continue;
                }
                r
            }
            (None, None) => {
                checks = 0;
                continue;
            }
        };
        gpu.confidence = request.confidence;
        gpu.far = request.far;
        gpu.dof = request.dof;
        gpu.probe = request.probe;
        let (w, h) = (request.camera.width, request.camera.height);
        let drawn = gpu.step(&request.camera, BACKGROUND, Target::None, None).and_then(|_| {
            let r = match readback.take() {
                Some(r) if r.size == (w, h) => r,
                _ => Readback::new(device, w, h),
            };
            gpu.present(&r.view);
            let frame = r.read(device, queue);
            readback = Some(r);
            frame
        });
        // Its probe came back with the frame (the read waited for the GPU).
        let focus = if request.probe.is_some() { gpu.probed() } else { None };
        {
            let mut s = shared.state.lock().unwrap();
            if focus.is_some() {
                s.focus = focus;
            }
            match drawn {
                Ok(frame) => {
                    DRAWN.fetch_add(1, Ordering::Relaxed);
                    s.frame = Some(frame);
                    s.error = None;
                    checks = REDRAW_CHECKS;
                }
                Err(e) => s.error = Some(format!("{e:#}")),
            }
        }
        last = Some(request);
        ctx.request_repaint();
    }
}

/// The texture the renderer presents into on the integrated GPU, and the buffer it is copied to for reading.
struct Readback {
    size: (u32, u32),
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    buffer: wgpu::Buffer,
    /// Bytes a row in the buffer (copies align rows to 256 bytes).
    row: u32,
}

impl Readback {
    fn new(device: &wgpu::Device, w: u32, h: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("splats on the integrated GPU"), // i18n: not shown
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("splats read back"), // i18n: not shown
            size: u64::from(row) * u64::from(h),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { size: (w, h), texture, view, buffer, row }
    }

    /// The presented frame, once the GPU has drawn it.
    fn read(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> anyhow::Result<Frame> {
        let (w, h) = self.size;
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.row), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        let slice = self.buffer.slice(..);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow::anyhow!("waiting for the integrated GPU: {e}"))?;
        rx.recv()?.map_err(|e| anyhow::anyhow!("reading the frame back: {e}"))?;
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        {
            let data = slice.get_mapped_range().map_err(|e| anyhow::anyhow!("reading the frame back: {e:?}"))?;
            for row in data.chunks_exact(self.row as usize) {
                pixels.extend_from_slice(&row[..(w * 4) as usize]);
            }
        }
        self.buffer.unmap();
        Ok(Frame { size: (w, h), pixels })
    }
}

struct FrameCallback {
    shared: Arc<Shared>,
    shown: Arc<Mutex<Option<Shown>>>,
}

impl egui_wgpu::CallbackTrait for FrameCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(blit) = Blit::of(device, resources) else { return Vec::new() };
        let Some(frame) = self.shared.state.lock().unwrap().frame.take() else { return Vec::new() };
        let mut shown = self.shown.lock().unwrap();
        let (w, h) = frame.size;
        if shown.as_ref().is_none_or(|s| s.size != frame.size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("splats from the integrated GPU"), // i18n: not shown
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let bind_group = blit.bind_group(device, &texture.create_view(&Default::default()));
            *shown = Some(Shown { size: frame.size, texture, bind_group, frames: 0 });
        }
        let s = shown.as_mut().expect("made above");
        s.frames += 1;
        queue.write_texture(
            s.texture.as_image_copy(),
            &frame.pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        let (Some(blit), Ok(shown)) = (resources.get::<Blit>(), self.shown.lock()) else { return };
        let Some(s) = shown.as_ref() else { return };
        blit.draw(pass, &s.bind_group);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_reconstruct::gaussians::{Gaussians, Seen};
    use orbiters_reconstruct::gpu::block_on;

    /// A wall of coloured splats two metres ahead.
    fn wall() -> Gaussians {
        let n = 400;
        Gaussians {
            means: (0..n).map(|i| [(i % 20) as f32 * 0.1 - 1.0, (i / 20) as f32 * 0.1 - 1.0, 2.0]).collect(),
            log_scales: vec![[-2.5; 3]; n],
            rotations: vec![[1.0, 0.0, 0.0, 0.0]; n],
            opacity_logits: vec![2.0; n],
            colors: (0..n).map(|i| [(i % 20) as f32 / 20.0, (i / 20) as f32 / 20.0, 0.5]).collect(),
            sky: vec![false; n],
            filter_3d: vec![0.0; n],
            seen: vec![Seen::UNKNOWN; n],
        }
    }

    #[test]
    fn the_integrated_gpu_hands_over_what_the_window_would_have_drawn() {
        let Ok((device, queue)) = crate::gpu::integrated_device() else { return };
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
        let Ok(adapter) = block_on(instance.request_adapter(&Default::default())) else { return };
        let (wd, wq) = block_on(adapter.request_device(&Default::default())).expect("device");
        let g = wall();
        let camera = Camera::look([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 1.0, 150, 90);
        let request = Request { camera, confidence: Confidence::default(), far: FarField::default(), dof: DepthOfField::default(), probe: None, marks: 0 };

        // On the window's device, as the paint callback draws.
        let mut window = Gpu::renderer(wd.clone(), wq.clone());
        window.upload(&g);
        let r = Readback::new(&wd, 150, 90);
        let draw = |window: &mut Gpu| {
            window.step(&camera, BACKGROUND, Target::None, None).expect("window");
            window.present(&r.view);
            r.read(&wd, &wq).expect("window frame")
        };
        let want = draw(&mut window);

        let mut gpu = Gpu::renderer(device.clone(), queue.clone());
        gpu.upload(&g);
        let mut worker = Worker::start(&egui::Context::default(), gpu, device, queue, 0);
        let next = |worker: &mut Worker| {
            let start = std::time::Instant::now();
            loop {
                if let Some(f) = worker.shared.state.lock().unwrap().frame.take() {
                    break f;
                }
                assert!(worker.error().is_none(), "{:?}", worker.error());
                assert!(start.elapsed().as_secs() < 30, "no frame");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        worker.ask(request);
        let frame = next(&mut worker);
        assert_eq!(frame.size, (150, 90));
        assert_eq!(frame.pixels.len(), want.pixels.len());
        assert!(want.pixels.chunks(4).any(|p| p[0] > 40), "the wall shows");
        let worst = |a: &Frame, b: &Frame| a.pixels.iter().zip(&b.pixels).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        assert!(worst(&frame, &want) <= 2, "{}", worst(&frame, &want));
        // The selection's marks reach the integrated GPU's thread: the left half of the wall hidden, its middle
        // selected, drawn there as the window draws them.
        let marks: Vec<u8> = (0..g.len()).map(|i| if i % 20 < 10 { 2 } else { u8::from(i % 20 == 12) }).collect();
        window.set_marks(&marks);
        let want_marked = draw(&mut window);
        assert!(worst(&want_marked, &want) > 40, "the marks change the picture");
        worker.set_marks(1, Arc::new(marks));
        worker.ask(Request { marks: 1, ..request });
        let marked = next(&mut worker);
        assert!(worst(&marked, &want_marked) <= 2, "{}", worst(&marked, &want_marked));
    }
}
