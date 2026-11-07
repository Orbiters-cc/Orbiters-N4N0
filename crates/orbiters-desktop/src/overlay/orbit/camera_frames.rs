//! What one eye sees, read for VRChat's camera panel ([`super::camera_track`]), on a thread of its own and only while its camera is open.
//!
//! The compositor's picture of the eye comes first: what it covers is known from SteamVR, so nothing has to be guessed to place the tiles. Only around the last place while it is seen, ten
//! times a second; all of it four times a second while it is not. If it cannot be read, or `camera_use_window` asks for it, VRChat's own window instead, by Windows Graphics Capture, which
//! `Tune` says the share of. Dropped, it costs nothing, and it waits for its thread, which uses the SteamVR client. The picture goes no further than the reading of it.

use super::camera_track::{self, Found};
use orbiters_i18n::mark;
use orbiters_openvr::compositor::{Eye, MirrorSource};
use orbiters_openvr::mirror::EyeReader;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Which picture a look read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Eye,
    Window,
}

/// A look's result: what was found, on a picture of `size`, of what, and when it was read.
#[derive(Clone, Copy, Debug)]
pub struct Seen {
    pub found: Found,
    pub size: [f32; 2],
    pub source: Source,
    pub at: Instant,
}

/// What it reads from, and how.
#[derive(Clone, Copy, Debug)]
pub struct From {
    pub mirror: Option<MirrorSource>,
    pub adapter: Option<u32>,
    /// Also keeps a frame a second in the logs' folder, for measuring what a headset really shows.
    pub keep_frames: bool,
}

/// How the reading goes.
#[derive(Clone, Copy, Debug, Default)]
struct Health {
    /// Opening the window's capture fails.
    cannot_open: bool,
    /// Which picture is being read: VRChat's window has its own troubles.
    source: Option<Source>,
    /// When a frame came, and when the camera's panel was last on one.
    framed: Option<Instant>,
    found: Option<Instant>,
    /// Looks, and how many found it.
    looks: u32,
    finds: u32,
}

/// How long something has to go on before it is said: a panel out of sight for a moment says nothing.
const QUIET_FOR: Duration = Duration::from_secs(5);

fn trouble(health: &Health, began: Instant, now: Instant) -> Option<&'static str> {
    let long = |since: Option<Instant>| now.duration_since(since.unwrap_or(began)) >= QUIET_FOR;
    if health.cannot_open {
        return Some(mark("Orbiters cannot read VRChat's window, so it cannot find the camera there."));
    }
    if long(health.framed) {
        return Some(mark("VRChat's window shows no picture, so the camera cannot be found on it. It must not be minimized."));
    }
    if long(health.found) {
        return Some(mark("VRChat's camera is open, but its panel is not in view now. The tiles come when it is."));
    }
    None
}

/// Reads until dropped.
pub struct CameraWatch {
    health: Arc<Mutex<Health>>,
    began: Instant,
    seen: Arc<Mutex<Option<Seen>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CameraWatch {
    pub fn start(from: From) -> Self {
        let (seen, stop) = (Arc::new(Mutex::new(None)), Arc::new(AtomicBool::new(false)));
        let health = Arc::new(Mutex::new(Health::default()));
        let (s, st, h) = (seen.clone(), stop.clone(), health.clone());
        let thread = std::thread::Builder::new().name("camera panel".into()).spawn(move || run(&s, &st, &h, from)).ok(); // i18n: not shown (a thread's name)
        Self { health, began: Instant::now(), seen, stop, thread }
    }

    /// Why no tile can show, as far as reading goes: for Settings, which translates it.
    pub fn problem(&self) -> Option<&'static str> {
        let health = *self.health.lock().unwrap_or_else(|e| e.into_inner());
        trouble(&health, self.began, Instant::now())
    }

    /// The last look's result, once.
    pub fn take(&self) -> Option<Seen> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

impl Drop for CameraWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let health = *self.health.lock().unwrap_or_else(|e| e.into_inner());
        note(&format!("{:?}: {} looks, {} found the panel, over {} s", health.source, health.looks, health.finds, self.began.elapsed().as_secs())); // i18n: a log's line
    }
}

/// A line in `camera-tiles.log`, which is how a run in a headset can be read afterwards.
fn note(line: &str) {
    let dir = orbiters_core::paths::logs_dir();
    if std::fs::create_dir_all(&dir).is_ok() {
        if let Ok(mut file) = std::fs::OpenOptions::new().append(true).create(true).open(dir.join("camera-tiles.log")) {
            use std::io::Write;
            let _ = writeln!(file, "{} {line}", orbiters_core::now_utc_iso());
        }
    }
}

/// How far apart looks are, while it is seen and while it is not.
const LOOK_EVERY: Duration = Duration::from_millis(100);
const LOOK_WHOLE_EVERY: Duration = Duration::from_millis(250);
/// How many errors in a row before the eye is given up, until the camera opens again.
const EYE_TRIES: u32 = 8;

/// One look on a picture read: `area` is where it is in the whole.
fn looked(pixels: &[u8], width: usize, height: usize, stride: usize, offset: [f32; 2], whole: [usize; 2], last: Option<Found>, seen: &Mutex<Option<Seen>>, health: &Mutex<Health>, source: Source, at: Instant) -> Option<Found> {
    let in_part = pixels.len() < whole[0] * whole[1] * 4 && stride == width * 4 && width < whole[0];
    let found = match last {
        // Around the last place in what was read, unless what was read is that already.
        Some(last) if !in_part => camera_track::find_in(pixels, width, height, stride, camera_track::around(&last, width, height), false).or_else(|| camera_track::find(pixels, width, height, stride)),
        _ => camera_track::find_in(pixels, width, height, stride, [0, 0, width, height], width <= 1400),
    };
    let found = found.map(|f| f.shifted(offset));
    let mut h = health.lock().unwrap_or_else(|e| e.into_inner());
    h.framed = Some(at);
    h.looks = h.looks.saturating_add(1);
    if let Some(found) = found {
        h.found = Some(at);
        h.finds = h.finds.saturating_add(1);
        drop(h);
        *seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(Seen { found, size: [whole[0] as f32, whole[1] as f32], source, at });
    }
    found
}

/// A frame a second kept for measuring, if asked: only what is in `pixels`, and only on this computer.
fn keep(pixels: &[u8], width: usize, height: usize, stride: usize, source: Source, kept: &mut Option<Instant>) {
    if kept.is_some_and(|at| at.elapsed() < Duration::from_secs(1)) {
        return;
    }
    *kept = Some(Instant::now());
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        if let Some(row) = pixels.get(y * stride..y * stride + width * 4) {
            rgba.extend_from_slice(row);
        }
    }
    if source == Source::Window {
        rgba.chunks_exact_mut(4).for_each(|p| p.swap(0, 2));
    }
    rgba.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    let dir = orbiters_core::paths::logs_dir().join("camera-frames");
    let _ = std::fs::create_dir_all(&dir);
    if let Some(image) = image::RgbaImage::from_raw(width as u32, height as u32, rgba) {
        let _ = image.save(dir.join(format!("{source:?}-{}.png", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis()))));
    }
}

fn run(seen: &Mutex<Option<Seen>>, stop: &AtomicBool, health: &Mutex<Health>, from: From) {
    let mut kept = None;
    if let Some(mirror) = from.mirror {
        match EyeReader::new(from.adapter) {
            Ok(reader) => {
                health.lock().unwrap_or_else(|e| e.into_inner()).source = Some(Source::Eye);
                let (mut errors, mut last, mut said) = (0, None, false);
                while !stop.load(Ordering::Relaxed) && errors < EYE_TRIES {
                    let began = Instant::now();
                    // Around the last place while it is seen.
                    let area = last.and_then(|(f, whole): (Found, [u32; 2])| {
                        let a = camera_track::around(&f, whole[0] as usize, whole[1] as usize);
                        (a[2] > 0 && a[3] > 0).then_some([a[0] as u32, a[1] as u32, a[2] as u32, a[3] as u32])
                    });
                    match reader.read_part(mirror, Eye::Left, area) {
                        Ok(picture) => {
                            errors = 0;
                            let whole = picture.whole;
                            if !std::mem::replace(&mut said, true) {
                                note(&format!("reading the left eye, {} x {}", whole[0], whole[1])); // i18n: a log's line
                            }
                            if from.keep_frames && area.is_none() {
                                keep(&picture.rgba, picture.width as usize, picture.height as usize, picture.width as usize * 4, Source::Eye, &mut kept);
                            }
                            let offset = [picture.from[0] as f32, picture.from[1] as f32];
                            let found = looked(&picture.rgba, picture.width as usize, picture.height as usize, picture.width as usize * 4, offset, [whole[0] as usize, whole[1] as usize], None, seen, health, Source::Eye, began);
                            last = found.map(|f| (f, whole));
                        }
                        Err(e) => {
                            errors += 1;
                            last = None;
                            if errors == EYE_TRIES {
                                note(&format!("the eye cannot be read: {e}")); // i18n: a log's line
                            }
                        }
                    }
                    let every = if last.is_some() { LOOK_EVERY } else { LOOK_WHOLE_EVERY };
                    std::thread::sleep(every.saturating_sub(began.elapsed()));
                }
            }
            Err(e) => note(&format!("no reader for the eye: {e}")), // i18n: a log's line
        }
    }
    if !stop.load(Ordering::Relaxed) {
        run_on_window(seen, stop, health, from.keep_frames, &mut kept);
    }
}

#[cfg(not(windows))]
fn run_on_window(_: &Mutex<Option<Seen>>, _: &AtomicBool, _: &Mutex<Health>, _: bool, _: &mut Option<Instant>) {}

#[cfg(windows)]
fn run_on_window(seen: &Mutex<Option<Seen>>, stop: &AtomicBool, health: &Mutex<Health>, keep_frames: bool, kept: &mut Option<Instant>) {
    let told = |what: &dyn Fn(&mut Health)| what(&mut health.lock().unwrap_or_else(|e| e.into_inner()));
    told(&|h| h.source = Some(Source::Window));
    let mut said = false;
    while !stop.load(Ordering::Relaxed) {
        // VRChat may not be there, or its window may go: looked for again.
        match imp::Frames::open() {
            Ok(mut frames) => {
                said = false;
                told(&|h| h.cannot_open = false);
                let mut last = None;
                while !stop.load(Ordering::Relaxed) && frames.alive() {
                    let began = Instant::now();
                    if let Some((bgra, _, _, stride, left, top, client)) = frames.next() {
                        // Only what is the game's: the frame's title bar and borders are not.
                        let start = top as usize * stride + left as usize * 4;
                        if let Some(game) = bgra.get(start..) {
                            if keep_frames {
                                keep(game, client[0] as usize, client[1] as usize, stride, Source::Window, kept);
                            }
                            last = looked(game, client[0] as usize, client[1] as usize, stride, [0.0, 0.0], [client[0] as usize, client[1] as usize], last, seen, health, Source::Window, began);
                        }
                    }
                    std::thread::sleep(LOOK_EVERY.saturating_sub(began.elapsed()));
                }
            }
            Err(e) => {
                if !std::mem::replace(&mut said, true) {
                    note(&format!("VRChat's window cannot be read: {e}")); // i18n: a log's line
                }
                told(&|h| h.cannot_open = true);
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Settings says why, and a panel out of sight for a moment says nothing.
    #[test]
    fn what_keeps_the_tiles_away_is_said() {
        let began = Instant::now();
        let at = |s: u64| began + Duration::from_secs(s);
        let mut health = Health::default();
        assert_eq!(trouble(&health, began, at(1)), None, "it has only just begun");
        assert!(trouble(&health, began, at(6)).is_some_and(|said| said.contains("minimized")), "no frame at all");
        health.framed = Some(at(6));
        assert!(trouble(&health, began, at(7)).is_some_and(|said| said.contains("not in view")));
        health.found = Some(at(7));
        assert_eq!(trouble(&health, began, at(8)), None, "seen");
        health.cannot_open = true;
        assert!(trouble(&health, began, at(8)).is_some_and(|said| said.contains("cannot read")));
    }
}

#[cfg(windows)]
mod imp {
    use windows::core::{w, Interface};
    use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
    use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Win32::Foundation::{HMODULE, HWND, POINT, RECT};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetClientRect, GetWindowRect, IsWindow};

    pub struct Frames {
        window: HWND,
        device: ID3D11Device,
        context: ID3D11DeviceContext,
        winrt: IDirect3DDevice,
        pool: Direct3D11CaptureFramePool,
        _session: GraphicsCaptureSession,
        _item: GraphicsCaptureItem,
        size: (i32, i32),
        staging: Option<(ID3D11Texture2D, u32, u32)>,
        pixels: Vec<u8>,
    }

    impl Frames {
        pub fn open() -> Result<Self, String> {
            let text = |what: &str, e: windows::core::Error| format!("{what}: {}", e.message());
            // SAFETY: a plain query by name.
            let window = unsafe { FindWindowW(w!("UnityWndClass"), w!("VRChat")) }.map_err(|e| text("no VRChat window", e))?; // i18n: not shown (only printed to the log, with `camera panel:`)
            let (mut device, mut context): (Option<ID3D11Device>, Option<ID3D11DeviceContext>) = (None, None);
            // SAFETY: the outputs are valid.
            unsafe { D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context)) }.map_err(|e| text("Direct3D", e))?;
            let (device, context) = (device.ok_or("no Direct3D device")?, context.ok_or("no Direct3D context")?); // i18n: not shown (only printed to the log, with `camera panel:`)
            let dxgi: IDXGIDevice = device.cast().map_err(|e| text("DXGI", e))?;
            // SAFETY: the DXGI device is alive for the call.
            let winrt: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }.and_then(|d| d.cast()).map_err(|e| text("device", e))?;
            let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(|e| text("capture item", e))?; // i18n: not shown (only printed to the log, with `camera panel:`)
            // SAFETY: a window handle; one that is gone only fails.
            let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(window) }.map_err(|e| text("capture item", e))?; // i18n: not shown (only printed to the log, with `camera panel:`)
            let size = item.Size().map_err(|e| text("size", e))?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size).map_err(|e| text("frames", e))?;
            let session = pool.CreateCaptureSession(&item).map_err(|e| text("session", e))?;
            let _ = session.SetIsCursorCaptureEnabled(false);
            let _ = session.SetIsBorderRequired(false);
            session.StartCapture().map_err(|e| text("start", e))?;
            Ok(Self { window, device, context, winrt, pool, _session: session, _item: item, size: (size.Width, size.Height), staging: None, pixels: Vec::new() })
        }

        pub fn alive(&self) -> bool {
            // SAFETY: a plain query.
            unsafe { IsWindow(Some(self.window)) }.as_bool()
        }

        /// The newest frame's pixels: them, its size, their stride, where the game's area starts in them, and how large that is.
        pub fn next(&mut self) -> Option<(&[u8], u32, u32, usize, u32, u32, [u32; 2])> {
            let mut newest = None;
            while let Ok(frame) = self.pool.TryGetNextFrame() {
                newest = Some(frame);
            }
            let frame = newest?;
            let content = frame.ContentSize().ok()?;
            if (content.Width, content.Height) != self.size {
                self.size = (content.Width, content.Height);
                let _ = self.pool.Recreate(&self.winrt, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, content);
                return None;
            }
            let access: IDirect3DDxgiInterfaceAccess = frame.Surface().ok()?.cast().ok()?;
            // SAFETY: the surface is a Direct3D 11 texture, as the pool was made for.
            let texture: ID3D11Texture2D = unsafe { access.GetInterface() }.ok()?;
            let (width, height) = (content.Width as u32, content.Height as u32);
            let staging = self.staging_for(width, height)?;
            // SAFETY: both textures are this device's and of one format; the box is the frame's; it is unmapped before anything else happens to it.
            let stride = unsafe {
                let region = D3D11_BOX { left: 0, top: 0, front: 0, right: width, bottom: height, back: 1 };
                self.context.CopySubresourceRegion(&staging, 0, 0, 0, 0, &texture, 0, Some(&region));
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                self.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).ok()?;
                let pitch = mapped.RowPitch;
                self.pixels.clear();
                self.pixels.extend_from_slice(std::slice::from_raw_parts(mapped.pData as *const u8, (pitch * height) as usize));
                self.context.Unmap(&staging, 0);
                pitch as usize
            };
            let (mut outer, mut client, mut corner) = (RECT::default(), RECT::default(), POINT::default());
            // SAFETY: plain queries with valid outputs.
            unsafe {
                GetWindowRect(self.window, &mut outer).ok()?;
                GetClientRect(self.window, &mut client).ok()?;
                let _ = ClientToScreen(self.window, &mut corner);
            }
            let (left, top) = ((corner.x - outer.left).max(0) as u32, (corner.y - outer.top).max(0) as u32);
            let area = [(client.right - client.left).max(0) as u32, (client.bottom - client.top).max(0) as u32];
            let area = [area[0].min(width.saturating_sub(left)), area[1].min(height.saturating_sub(top))];
            Some((&self.pixels, width, height, stride, left, top, area))
        }

        fn staging_for(&mut self, width: u32, height: u32) -> Option<ID3D11Texture2D> {
            if let Some((t, w, h)) = &self.staging {
                if *w == width && *h == height {
                    return Some(t.clone());
                }
            }
            let description = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut staging: Option<ID3D11Texture2D> = None;
            // SAFETY: the description and the output are valid.
            unsafe { self.device.CreateTexture2D(&description, None, Some(&mut staging)) }.ok()?;
            let staging = staging?;
            self.staging = Some((staging.clone(), width, height));
            Some(staging)
        }
    }
}
