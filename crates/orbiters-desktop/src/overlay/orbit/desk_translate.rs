//! Translation on the desktop (REQUEST15 item 2; the orbit menu's Translation, `Settings::translate`): the text VRChat's window shows, read a few times a
//! second, and its translation laid over it on the screen, as VR's ([`crate::overlay::translate`]) lays it in the room. The same reader, translators, cache and
//! painting; what differs is where the picture comes from and where a translation goes:
//!
//! - the picture is VRChat's own window through Windows Graphics Capture: only its pixels, never a window over it (ours included), so nothing of ours is read;
//! - there is one picture, not two eyes: [`orbiters_translate::core::Core`] is given the same picture as both eyes, so every line lies at the fallback distance
//!   on its ray and its quad, seen through the same view, falls back on the line's own pixels ([`screen_rect`]);
//! - a translation is a window of ours ([`super::desktop::PanelWindow`], letting every click through) over the line, right where it is in VRChat's window.
//!
//! On a screen the view moves with the mouse and the keys, not with a head that is tracked: what was read goes stale the moment it turns. So the reading thread
//! looks at a few small patches of the window ten times a second ([`Signature`]); when they change the translations go at once, and come back from a new look as
//! soon as the view is still. While one of VRChat's menus or ours is up they go too, and reading waits (VR does the same), as the menus cover the view.

use super::desktop::{client_in_frame, Gpu, PanelWindow};
use crate::backend::Ctl;
use crate::overlay::translate::{self as vr_translate, LanFound, Nobody};
use crate::state::{Snapshot, TranslateState, TranslateStatus};
use egui::{pos2, Pos2, Rect, Vec2};
use orbiters_core::settings::{TranslateProvider, TranslateSettings};
use orbiters_translate::core::{Activity, Core, Quad, Reader, MOST_SHOWN};
use orbiters_translate::geometry::{EyeView, Mirror, IDENTITY};
use orbiters_translate::paint::Painted;
use orbiters_translate::place::Placed;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::Interface;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Direct3D11::{ID3D11Texture2D, D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

/// How long a translation takes to appear, and to go.
const FADE: Duration = Duration::from_millis(160);
/// How often the patches are looked at for a view that moved.
const SIGNATURE_EVERY: Duration = Duration::from_millis(100);
/// VRChat's field of view on a screen (its default, degrees from top to bottom): what the view a picture is read through has. Every line lies at the
/// fallback distance whatever it is (one picture tells no distance), so it only sets how large letters are in degrees, which the reader's rules measure.
const VERTICAL_FOV_DEG: f32 = 60.0;
/// The part of the window read (left, top, width, height, percent): VRChat's own figures along its top edge (frames a second, its hints) left out.
const AREA_PCT: [u32; 4] = [2, 7, 96, 89];
/// A part this large or larger is read again about its middle at its own scale, every other look, for small text (as VR does about where the head looks).
const ZOOM_FROM: u32 = 1500;
const ZOOM_SIDE: u32 = 960;

/// Whether VRChat's window is read at all: translation on, and turned on on the desktop itself (`window_on`), whoever translates: nothing is read or sent
/// before (verify2-r15 S3).
pub(crate) fn reads_window(settings: &TranslateSettings) -> bool {
    settings.enabled && settings.window_on && orbiters_core::here::HERE.window_text
}

/// The view a picture of `size` pixels is read through: an eye at the origin looking along -Z, VRChat's field of view, square pixels.
pub(crate) fn view(size: [u32; 2]) -> EyeView {
    let ty = (VERTICAL_FOV_DEG.to_radians() / 2.0).tan();
    let tx = ty * size[0] as f32 / size[1].max(1) as f32;
    EyeView { eye: IDENTITY, mirror: Mirror { from: [-tx, -ty], spans: [2.0 * tx, 2.0 * ty] }, size: [size[0] as f32, size[1] as f32] }
}

/// Where a translation's quad lies in the picture it was read in, pixels of that picture: the box round its corners seen through [`view`]. `None` when it is
/// not in front.
pub(crate) fn screen_rect(quad: &Quad, size: [u32; 2]) -> Option<Rect> {
    let v = view(size);
    let corners = Placed { pose: quad.pose, width: quad.width, height: quad.height, measured: false, distance: 0.0 }.corners();
    let points: Option<Vec<[f32; 2]>> = corners.iter().map(|c| v.project(*c)).collect();
    let points = points?;
    let (mut min, mut max) = (pos2(f32::MAX, f32::MAX), pos2(f32::MIN, f32::MIN));
    for p in points {
        min = min.min(pos2(p[0], p[1]));
        max = max.max(pos2(p[0], p[1]));
    }
    let r = Rect::from_min_max(min, max);
    (r.is_finite() && r.width() >= 2.0 && r.height() >= 2.0).then_some(r)
}

/// `p` (whole picture pixels, x, y, width, height) as percents of `size`.
fn pct(size: [u32; 2], p: [u32; 4]) -> [u32; 4] {
    [size[0] * p[0] / 100, size[1] * p[1] / 100, size[0] * p[2] / 100, size[1] * p[3] / 100]
}

/// Straight-alpha RGBA of `w` x `h` scaled to `to` with bilinear samples, then premultiplied: what a window of ours shows.
pub(crate) fn scaled_premultiplied(rgba: &[u8], w: u32, h: u32, to: [u32; 2]) -> Vec<u8> {
    let (tw, th) = (to[0].max(1), to[1].max(1));
    let mut out = vec![0u8; (tw * th * 4) as usize];
    if w == 0 || h == 0 || rgba.len() < (w * h * 4) as usize {
        return out;
    }
    let (sx, sy) = (w as f32 / tw as f32, h as f32 / th as f32);
    let at = |x: u32, y: u32| -> [f32; 4] {
        let i = ((y.min(h - 1) * w + x.min(w - 1)) * 4) as usize;
        // Premultiplied before mixing: a clear neighbour's colour does not bleed into the letters' edges.
        let a = rgba[i + 3] as f32 / 255.0;
        [rgba[i] as f32 * a, rgba[i + 1] as f32 * a, rgba[i + 2] as f32 * a, rgba[i + 3] as f32]
    };
    for y in 0..th {
        let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
        let (y0, ty) = (fy.floor() as u32, fy.fract());
        for x in 0..tw {
            let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
            let (x0, tx) = (fx.floor() as u32, fx.fract());
            let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
            let o = ((y * tw + x) * 4) as usize;
            for k in 0..4 {
                let top = a[k] + (b[k] - a[k]) * tx;
                let bottom = c[k] + (d[k] - c[k]) * tx;
                out[o + k] = (top + (bottom - top) * ty).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

// --------------------------------------------------------------------------------------------- whether the view moved

/// Small patches of VRChat's window, as grey means of their cells: what tells a view that turned or walked from one that stays (a sign's text does not move
/// unless the view does). Seven patches over the window, 6 x 6 cells of 8 pixels each.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Signature(Vec<f32>);

/// Where the patches are, in hundredths of the client area (their middles).
const PATCHES: [[u32; 2]; 7] = [[50, 50], [25, 30], [75, 30], [25, 72], [75, 72], [50, 22], [50, 82]];
const PATCH: u32 = 48;
const CELL: u32 = 8;

/// The patches in a client area of `client` pixels (x, y, width, height), in [`PATCHES`] order.
pub(crate) fn patch_rects(client: [u32; 2]) -> Vec<[u32; 4]> {
    PATCHES
        .iter()
        .map(|[px, py]| {
            let x = (client[0] * px / 100).saturating_sub(PATCH / 2).min(client[0].saturating_sub(PATCH));
            let y = (client[1] * py / 100).saturating_sub(PATCH / 2).min(client[1].saturating_sub(PATCH));
            [x, y, PATCH, PATCH]
        })
        .collect()
}

impl Signature {
    /// From each patch's BGRA (rows of `stride` bytes), in [`PATCHES`] order.
    pub(crate) fn of(patches: &[(&[u8], u32)]) -> Self {
        let mut out = Vec::with_capacity(patches.len() * 36);
        for (bgra, stride) in patches {
            for cy in 0..PATCH / CELL {
                for cx in 0..PATCH / CELL {
                    let mut sum = 0.0;
                    for y in cy * CELL..(cy + 1) * CELL {
                        for x in cx * CELL..(cx + 1) * CELL {
                            let i = (y * stride + x * 4) as usize;
                            let Some(p) = bgra.get(i..i + 3) else { continue };
                            sum += 0.114 * p[0] as f32 + 0.587 * p[1] as f32 + 0.299 * p[2] as f32;
                        }
                    }
                    out.push(sum / (CELL * CELL) as f32);
                }
            }
        }
        Self(out)
    }

    /// Whether the view moved between `self` and `now`: most patches changed (a turn or a step changes all of them; a video, a door, a person walking past
    /// changes one or two).
    pub(crate) fn moved(&self, now: &Signature) -> bool {
        if self.0.len() != now.0.len() {
            return true;
        }
        if self.0.is_empty() {
            return false;
        }
        let per = self.0.len() / PATCHES.len();
        let changed = self.0.chunks(per).zip(now.0.chunks(per)).filter(|(a, b)| a.iter().zip(b.iter()).map(|(x, y)| (x - y).abs()).sum::<f32>() / per as f32 > 6.0).count();
        changed * 2 > PATCHES.len()
    }
}

/// Whether the view moved since the last look, from the patches sampled between looks. Reset as a whole (a menu came and went, another window, another size):
/// a reset with "moving" left set kept every look from happening, so the translations stayed hidden for good (verify-r15).
#[derive(Clone, Debug, Default)]
pub(crate) struct Motion {
    /// The patches as the last look saw them; `None`: a look is due at once.
    looked: Option<Signature>,
    /// The patches last sampled.
    previous: Option<Signature>,
    moving: bool,
}

impl Motion {
    /// A sample of the patches between looks. Moved from what the last look saw: moving. Moving, and the same twice in a row: still again, and a look is due.
    pub(crate) fn sample(&mut self, now: Option<Signature>) {
        if let (Some(then), Some(n)) = (&self.looked, &now) {
            if then.moved(n) {
                self.moving = true;
            }
        }
        if self.moving && self.previous.as_ref().zip(now.as_ref()).is_some_and(|(p, n)| !p.moved(n)) {
            self.moving = false;
            self.looked = None;
        }
        self.previous = now;
    }

    /// A look was read: what it saw is what moves are measured from.
    pub(crate) fn looked(&mut self, seen: Option<Signature>) {
        self.looked = seen;
    }

    /// Everything known of the view forgotten: not moving, and a look due at once.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn moving(&self) -> bool {
        self.moving
    }

    /// A look is due at once (nothing read since the view last changed).
    pub(crate) fn needs_look(&self) -> bool {
        self.looked.is_none()
    }

    pub(crate) fn last_sample(&self) -> Option<Signature> {
        self.previous.clone()
    }
}

// --------------------------------------------------------------------------------------------- VRChat's window, captured

/// Windows Graphics Capture of VRChat's window, for the reading thread: a whole look, or its patches. Frames stay on the graphics card; only what is read comes
/// back.
struct Grab {
    gpu: Gpu,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    _item: GraphicsCaptureItem,
    size: (i32, i32),
    window: isize,
    /// The newest frame, kept until a newer one comes: patches and looks read the same frame when nothing moved.
    frame: Option<ID3D11Texture2D>,
    staging: Vec<Option<(ID3D11Texture2D, u32, u32)>>,
}

impl Grab {
    fn new(window: HWND, gpu: &Gpu) -> Result<Self, String> {
        let text = |what: &str, e: windows::core::Error| format!("{what}: {}", e.message());
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(|e| text("capture item", e))?; // i18n: not shown (logged, never drawn)
        // SAFETY: a window handle; asking for one that is gone only fails.
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(window) }.map_err(|e| text("capture item", e))?; // i18n: not shown (logged, never drawn)
        let size = item.Size().map_err(|e| text("size", e))?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&gpu.winrt, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size).map_err(|e| text("frame pool", e))?; // i18n: not shown (logged, never drawn)
        let session = pool.CreateCaptureSession(&item).map_err(|e| text("session", e))?;
        // No pointer in the picture, and no yellow border: nobody is being recorded, the game's text is read.
        let _ = session.SetIsCursorCaptureEnabled(false);
        let _ = session.SetIsBorderRequired(false);
        session.StartCapture().map_err(|e| text("start", e))?;
        Ok(Self { gpu: gpu.clone(), pool, session, _item: item, size: (size.Width, size.Height), window: window.0 as isize, frame: None, staging: Vec::new() })
    }

    /// The newest frame since the last call, kept; `false` when the window changed size (frames of its new size come next).
    fn newest(&mut self) -> bool {
        let mut newest = None;
        while let Ok(frame) = self.pool.TryGetNextFrame() {
            newest = Some(frame);
        }
        let Some(frame) = newest else { return true };
        let Ok(content) = frame.ContentSize() else { return true };
        if (content.Width, content.Height) != self.size {
            self.size = (content.Width, content.Height);
            let _ = self.pool.Recreate(&self.gpu.winrt, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, content);
            self.frame = None;
            return false;
        }
        let texture = frame.Surface().ok().and_then(|s| s.cast::<IDirect3DDxgiInterfaceAccess>().ok()).and_then(|a| {
            // SAFETY: the surface is a Direct3D 11 texture, as the pool was made for.
            unsafe { a.GetInterface::<ID3D11Texture2D>() }.ok()
        });
        if texture.is_some() {
            self.frame = texture;
        }
        true
    }

    fn staging_for(&mut self, i: usize, width: u32, height: u32) -> Option<ID3D11Texture2D> {
        if self.staging.len() <= i {
            self.staging.resize_with(i + 1, || None);
        }
        if let Some((t, w, h)) = &self.staging[i] {
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
        unsafe { self.gpu.device.CreateTexture2D(&description, None, Some(&mut staging)) }.ok()?;
        let staging = staging?;
        self.staging[i] = Some((staging.clone(), width, height));
        Some(staging)
    }

    /// The part `[x, y, w, h]` of the client area of the kept frame, into `out` as BGRA rows of the returned stride. `None` when there is no frame, or the part
    /// is not wholly inside it (a frame of the window's older size).
    fn read(&mut self, slot: usize, part: [u32; 4], out: &mut Vec<u8>) -> Option<u32> {
        let texture = self.frame.clone()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: a plain read of the texture's description into a local.
        unsafe { texture.GetDesc(&mut desc) };
        let frame = (desc.Width.min(self.size.0.max(0) as u32), desc.Height.min(self.size.1.max(0) as u32));
        let (dx, dy) = client_in_frame(HWND(self.window as *mut _));
        let [left, top, right, bottom] = super::desktop::source_box(part, (dx, dy), frame)?;
        let staging = self.staging_for(slot, part[2], part[3])?;
        let region = D3D11_BOX { left, top, front: 0, right, bottom, back: 1 };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: both textures are this device's, of one format; the box lies inside the frame (`source_box`); the mapping is read within its pitch and height
        // and unmapped at once.
        unsafe {
            self.gpu.context.CopySubresourceRegion(&staging, 0, 0, 0, 0, &texture, 0, Some(&region));
            self.gpu.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).ok()?;
            let pitch = mapped.RowPitch;
            let bytes = std::slice::from_raw_parts(mapped.pData as *const u8, (pitch * part[3]) as usize);
            out.clear();
            out.extend_from_slice(bytes);
            self.gpu.context.Unmap(&staging, 0);
            Some(pitch)
        }
    }

    /// The patches' signature of the kept frame in a client area of `client` pixels.
    fn signature(&mut self, client: [u32; 2]) -> Option<Signature> {
        if client[0] < PATCH * 3 || client[1] < PATCH * 3 {
            return None;
        }
        let mut buffers: Vec<(Vec<u8>, u32)> = Vec::with_capacity(PATCHES.len());
        for (i, part) in patch_rects(client).into_iter().enumerate() {
            let mut out = Vec::new();
            let stride = self.read(1 + i, part, &mut out)?;
            buffers.push((out, stride));
        }
        let parts: Vec<(&[u8], u32)> = buffers.iter().map(|(b, s)| (b.as_slice(), *s)).collect();
        Some(Signature::of(&parts))
    }

    /// The whole client area of the kept frame, RGBA.
    fn picture(&mut self, client: [u32; 2]) -> Option<Vec<u8>> {
        let mut bgra = Vec::new();
        let stride = self.read(0, [0, 0, client[0], client[1]], &mut bgra)? as usize;
        let mut rgba = Vec::with_capacity((client[0] * client[1] * 4) as usize);
        for row in bgra.chunks(stride).take(client[1] as usize) {
            for px in row[..(client[0] as usize * 4).min(row.len())].chunks_exact(4) {
                rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
        }
        (rgba.len() == (client[0] * client[1] * 4) as usize).then_some(rgba)
    }
}

/// VRChat's window's client area as its own frame shows it (Windows Graphics Capture: its pixels only, whatever lies over it), RGBA, once a frame comes
/// within `wait`; `None` otherwise. For a report's picture (verify2-r15 blocker 2).
pub(super) fn frame_of(window: HWND, client: [u32; 2], wait: Duration) -> Option<Vec<u8>> {
    if client[0] == 0 || client[1] == 0 {
        return None;
    }
    let gpu = Gpu::new().ok()?;
    let mut grab = Grab::new(window, &gpu).ok()?;
    let until = Instant::now() + wait;
    while Instant::now() < until {
        if grab.newest() && grab.frame.is_some() {
            if let Some(rgba) = grab.picture(client) {
                return Some(rgba);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

impl Drop for Grab {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

// --------------------------------------------------------------------------------------------- the reading thread and the host

/// What the loop and the reading thread share.
#[derive(Default)]
struct Shared {
    /// VRChat's window and its client area's size, while it is on the screen; `None` otherwise. With VRChat's process, asked again before a capture starts.
    window: Option<(isize, [u32; 2])>,
    pid: u32,
    /// A menu (VRChat's or ours) covers the view: no reading, nothing shown.
    paused: bool,
    settings: Option<(TranslateSettings, String)>,
    /// The latest scene, with the size of the picture it was read in.
    scene: Option<(orbiters_translate::core::Scene, [u32; 2])>,
    /// The view moved since the last look: what was read is stale.
    moving: bool,
    status: TranslateStatus,
}

/// A translation on the screen: its window, the line it shows, where.
struct Shown {
    window: PanelWindow,
    track: u64,
    picture: Arc<Painted>,
    /// Client pixels of the picture it was read in.
    rect: Rect,
    alpha: f32,
    to: f32,
}

/// The desktop's translations: the reading thread while Translation is on, and a window for each translation shown.
#[derive(Default)]
pub struct Host {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Threads turned off that may still be in a look: joined once they end, never waited for.
    retired: Vec<std::thread::JoinHandle<()>>,
    shown: Vec<Shown>,
    last_turn: Option<Instant>,
    status_sent: Option<(Instant, TranslateStatus)>,
    remote_set: bool,
    /// VRChat's window and its size the translations shown were placed in.
    placed_in: Option<(isize, [u32; 2])>,
}

/// Where VRChat's window is this turn, for [`Host::update`].
#[derive(Clone, Copy, Debug)]
pub struct OnScreen {
    pub window: HWND,
    /// Its client area's corner on the screen, and its size.
    pub origin: Pos2,
    pub client: Vec2,
    /// VRChat's process.
    pub pid: u32,
}

impl Host {
    /// A turn of the desktop's loop: on or off as the setting says, VRChat's window where it is (`None`: not on the screen), `covered` while a menu of VRChat's or
    /// ours is up. Never waits.
    pub fn update(&mut self, snap: &Snapshot, backend: &Ctl, on: Option<OnScreen>, covered: bool) {
        self.retired.retain(|t| !t.is_finished());
        let wanted = reads_window(&snap.settings.translate);
        if !wanted {
            if self.thread.is_some() {
                self.stop_thread();
            }
            self.clear();
            self.report(backend, TranslateStatus::default());
            return;
        }
        if !self.remote_set {
            // Orbiters' translation service through the desktop service, which holds the account's credential (as the VR overlay does).
            let backend = backend.clone();
            *vr_translate::REMOTE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(move |batch: &orbiters_translate::provider::Batch| backend.translate(batch.clone()).map_err(Into::into)));
            self.remote_set = true;
        }
        if self.thread.is_none() {
            self.start();
        }
        let now = Instant::now();
        let dt = self.last_turn.map_or(0.0, |t| (now - t).as_secs_f32());
        self.last_turn = Some(now);
        let (scene, moving, status) = {
            let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            s.window = on.map(|o| (o.window.0 as isize, [o.client.x.max(0.0) as u32, o.client.y.max(0.0) as u32]));
            s.pid = on.map_or(0, |o| o.pid);
            s.paused = covered;
            s.settings = Some((snap.settings.translate.clone(), vr_translate::target(&snap.settings)));
            (s.scene.take(), s.moving, s.status.clone())
        };
        // A menu over the view, another window or another size: what showed was for a view that may not be there when it comes back (verify-r15).
        let now_in = on.map(|o| (o.window.0 as isize, [o.client.x.max(0.0) as u32, o.client.y.max(0.0) as u32]));
        if covered || now_in != self.placed_in {
            self.clear();
            self.placed_in = now_in;
        }
        if let Some((scene, size)) = scene.filter(|_| !covered) {
            self.place(&scene.quads, size, on);
        }
        // Nothing over a menu, nothing stale over a view that moved, nothing where VRChat's window is not.
        let hide = covered || moving || on.is_none();
        let shown_now = self.animate(dt, hide, on);
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).status.shown = shown_now;
        self.report(backend, TranslateStatus { shown: shown_now, ..status });
    }

    /// Each translation showing laid again, in order: where ours lie changed ([`super::desktop::stack`]).
    pub fn raise(&self) {
        for w in self.windows() {
            w.raise();
        }
    }

    /// The windows of the translations showing: they lie with ours ([`super::desktop::stack`]).
    pub fn windows(&self) -> impl Iterator<Item = &PanelWindow> {
        self.shown.iter().map(|s| &s.window).filter(|w| w.presented())
    }

    fn start(&mut self) {
        self.stop = Arc::new(AtomicBool::new(false));
        let status = TranslateStatus { state: TranslateState::Starting, ..Default::default() };
        self.shared = Arc::new(Mutex::new(Shared { status, ..Default::default() }));
        let (shared, stop) = (self.shared.clone(), self.stop.clone());
        self.thread = std::thread::Builder::new().name("desk translate".into()).spawn(move || run(&shared, &stop)).ok(); // i18n: not shown (a thread's name)
    }

    fn stop_thread(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        vr_translate::stop_downloads();
        if let Some(t) = self.thread.take() {
            self.retired.push(t);
        }
    }

    /// The scene's quads onto the windows: a line keeps its window while it shows; lines no longer wanted fade out.
    fn place(&mut self, quads: &[Quad], size: [u32; 2], on: Option<OnScreen>) {
        let wanted: Vec<(&Quad, Rect)> = quads.iter().take(MOST_SHOWN).filter_map(|q| screen_rect(q, size).map(|r| (q, r))).collect();
        for s in &mut self.shown {
            if !wanted.iter().any(|(q, _)| q.track == s.track) {
                s.to = 0.0;
            }
        }
        // The picture read may be of another size than the window now (it is being resized): what it says is placed in proportion.
        let scale = on.map_or(egui::vec2(1.0, 1.0), |o| egui::vec2(o.client.x / size[0].max(1) as f32, o.client.y / size[1].max(1) as f32));
        for (q, r) in wanted {
            let rect = Rect::from_min_max(pos2(r.min.x * scale.x, r.min.y * scale.y), pos2(r.max.x * scale.x, r.max.y * scale.y));
            let rect = Rect::from_min_max(rect.min.round(), rect.max.round());
            let px = [rect.width().max(1.0) as u32, rect.height().max(1.0) as u32];
            let at = self.shown.iter().position(|s| s.track == q.track);
            let fresh = |q: &Quad| -> Option<Shown> {
                let mut window = PanelWindow::new(px[0], px[1], false).map_err(|e| eprintln!("translation window: {e}")).ok()?; // i18n: not shown (a log's line)
                window.picture(&scaled_premultiplied(&q.picture.rgba, q.picture.width, q.picture.height, px), [0, 0, px[0], px[1]]);
                Some(Shown { window, track: q.track, picture: q.picture.clone(), rect, alpha: 0.0, to: 1.0 })
            };
            match at {
                Some(i) if self.shown[i].window.size() == px => {
                    let s = &mut self.shown[i];
                    if !Arc::ptr_eq(&s.picture, &q.picture) {
                        s.window.picture(&scaled_premultiplied(&q.picture.rgba, q.picture.width, q.picture.height, px), [0, 0, px[0], px[1]]);
                        s.picture = q.picture.clone();
                    }
                    s.rect = rect;
                    s.to = 1.0;
                }
                Some(i) => {
                    // Another size: a window of its size, shown at once (it was showing).
                    if let Some(mut made) = fresh(q) {
                        made.alpha = self.shown[i].alpha;
                        self.shown[i] = made;
                    }
                }
                None => {
                    if let Some(made) = fresh(q) {
                        self.shown.push(made);
                    }
                }
            }
        }
    }

    /// Fades each toward where it goes, puts it on the screen over VRChat's window, and lets go of those gone. Says how many show.
    fn animate(&mut self, dt: f32, hide: bool, on: Option<OnScreen>) -> u32 {
        let step = if hide { 1.0 } else { dt / FADE.as_secs_f32() };
        for s in &mut self.shown {
            let to = if hide { 0.0 } else { s.to };
            s.alpha = if to > s.alpha { (s.alpha + step).min(to) } else { (s.alpha - step).max(to) };
            match on {
                Some(o) if s.alpha > 0.0 => {
                    let at = POINT { x: (o.origin.x + s.rect.min.x) as i32, y: (o.origin.y + s.rect.min.y) as i32 };
                    s.window.place_if_changed(at, (s.alpha * 255.0).round() as u8);
                }
                _ => s.window.hide(),
            }
        }
        self.shown.retain(|s| s.to > 0.0 || s.alpha > 0.0);
        self.shown.iter().filter(|s| s.alpha > 0.5).count() as u32
    }

    fn clear(&mut self) {
        for s in &mut self.shown {
            s.window.hide();
        }
        self.shown.clear();
    }

    fn report(&mut self, backend: &Ctl, status: TranslateStatus) {
        let due = self.status_sent.as_ref().is_none_or(|(at, last)| *last != status && at.elapsed() >= Duration::from_millis(500));
        if due {
            backend.translate_status(status.clone());
            self.status_sent = Some((Instant::now(), status));
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        vr_translate::stop_downloads();
    }
}

/// What a look read, followed (and so asked of the translator) only while translation is still wanted: turned off during the read, nothing is. `None` then.
#[allow(clippy::too_many_arguments)]
fn follow_unless_stopped(
    core: &mut Core,
    stop: &AtomicBool,
    now: f64,
    look: &orbiters_translate::pipeline::Look,
    lines: Vec<orbiters_translate::ocr::Line>,
    timings: orbiters_translate::pipeline::Timings,
    gaze: ([f32; 3], [f32; 3]),
) -> Option<orbiters_translate::core::Scene> {
    if stop.load(Ordering::Relaxed) {
        return None;
    }
    Some(core.follow(now, look, lines, timings, gaze))
}

/// The view forgotten ([`Motion::reset`]), and the host told nothing moves (it shows nothing stale meanwhile: it cleared on what caused this).
fn reset_motion(motion: &mut Motion, shared: &Mutex<Shared>) {
    motion.reset();
    let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
    s.moving = false;
    s.scene = None;
}

fn set_state(shared: &Mutex<Shared>, state: TranslateState) {
    shared.lock().unwrap_or_else(|e| e.into_inner()).status.state = state;
}

/// The reading thread: the text reader (downloaded the first time), the translator the settings ask for, then looks at VRChat's window as long as it is on.
fn run(shared: &Mutex<Shared>, stop: &AtomicBool) {
    vr_translate::lower_priority();
    if !orbiters_translate::models::missing().is_empty() {
        set_state(shared, TranslateState::Downloading { done: 0, total: orbiters_translate::models::missing_bytes() });
        let result = orbiters_translate::models::install(stop, &mut |done, total| set_state(shared, TranslateState::Downloading { done, total }));
        if let Err(e) = result {
            if !stop.load(Ordering::Relaxed) {
                set_state(shared, TranslateState::Problem { key: orbiters_i18n::mark("The text reader could not be downloaded.").into(), detail: Some(e.to_string()) });
            }
            return;
        }
    }
    set_state(shared, TranslateState::Starting);
    let engine = match orbiters_translate::pipeline::Engine::load(2) {
        Ok(e) => e,
        Err(e) => {
            set_state(shared, TranslateState::Problem { key: orbiters_i18n::mark("The text reader could not start.").into(), detail: Some(e.to_string()) });
            return;
        }
    };
    let (mut settings, mut target) = loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if let Some(s) = shared.lock().unwrap_or_else(|e| e.into_inner()).settings.clone() {
            break s;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let reader_of = |s: &TranslateSettings, target: &str| Reader { target: target.to_string(), reads: if s.keep_english { vec!["en".to_string()] } else { vec![] } };
    let mut lan = LanFound::default();
    let (mut core, mut translator_state) = match vr_translate::translator(&settings, &mut lan, true) {
        Ok(t) => (Core::new(Some(engine), t, reader_of(&settings, &target)), None),
        Err(state) => (Core::new(Some(engine), Box::new(Nobody), reader_of(&settings, &target)), Some(state)),
    };
    let started = Instant::now();
    core.clock = started;
    core.keep_cache(vr_translate::kept_translations());
    let mut translator_checked = Instant::now();
    // The built-in translator's download, while reading goes on: bytes so far, in all, and how it ended.
    let fetching: Arc<Mutex<Option<(u64, u64, Option<Result<(), String>>)>>> = Arc::default();
    let gpu = Gpu::new().map_err(|e| eprintln!("desk translate: {e}")).ok(); // i18n: not shown (a log's line)
    let mut grab: Option<Grab> = None;
    let mut looks = 0u32;
    let mut cost = Duration::from_millis(300);
    let mut last_look: Option<Instant> = None;
    let mut last_signature: Option<Instant> = None;
    // Whether the view moved since the last look ([`Motion`]).
    let mut motion = Motion::default();
    // Nothing new last time: a slower pace until something changes.
    let mut quiet = false;
    vr_translate::note("desktop reading started"); // i18n: not shown (a log's line)
    while !stop.load(Ordering::Relaxed) {
        let (window, paused, now_settings, pid) = {
            let s = shared.lock().unwrap_or_else(|e| e.into_inner());
            (s.window, s.paused, s.settings.clone(), s.pid)
        };
        core.save_cache(false);
        // Settings changed: another translator, another language.
        if let Some((s, t)) = now_settings {
            let translator_changed = s.provider != settings.provider || s.server != settings.server || s.model != settings.model || s.orbiters_agreed != settings.orbiters_agreed || s.orbiters_agreed_window != settings.orbiters_agreed_window;
            if translator_state == Some(TranslateState::NoLocalTranslator) && s.provider == TranslateProvider::ThisComputer {
                let mut f = fetching.lock().unwrap_or_else(|e| e.into_inner());
                if f.is_none() {
                    *f = Some((0, 0, None));
                    let (fetching, stop) = (fetching.clone(), vr_translate::stop_flag(stop));
                    let _ = std::thread::Builder::new().name("translate download".into()).spawn(move || { // i18n: not shown (a thread's name)
                        vr_translate::lower_priority();
                        let result = orbiters_translate::models::install_files(&orbiters_translate::builtin::FILES, &stop, &mut |done, total| {
                            if let Some(f) = fetching.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                                (f.0, f.1) = (done, total);
                            }
                        });
                        if let Some(f) = fetching.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                            f.2 = Some(result.map_err(|e| e.to_string()));
                        }
                    });
                }
            }
            let downloaded = matches!(*fetching.lock().unwrap_or_else(|e| e.into_inner()), Some((_, _, Some(Ok(())))));
            let retry = translator_state.is_some() && (translator_checked.elapsed() > Duration::from_secs(15) || (downloaded && translator_state == Some(TranslateState::NoLocalTranslator)));
            if translator_changed || retry {
                translator_checked = Instant::now();
                match vr_translate::translator(&s, &mut lan, true) {
                    Ok(tr) => {
                        core.set_translator(tr);
                        translator_state = None;
                    }
                    Err(state) => {
                        core.set_translator(Box::new(Nobody));
                        translator_state = Some(state);
                    }
                }
            }
            if s != settings || t != target {
                core.set_reader(reader_of(&s, &t));
            }
            (settings, target) = (s, t);
        }
        // Translations that came meanwhile show at once; none from before a look is due again (they would lie where the view no longer has them).
        if let Some(scene) = core.poll(started.elapsed().as_secs_f64(), ([0.0; 3], [0.0, 0.0, -1.0])).filter(|_| !motion.needs_look()) {
            let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
            let size = s.window.map(|(_, c)| c).unwrap_or([1, 1]);
            s.scene = Some((scene, size));
        }
        vr_translate::take_logs(&mut core);
        let Some((hwnd, client)) = window.filter(|_| gpu.is_some()) else {
            if gpu.is_none() {
                set_state(shared, TranslateState::Problem { key: orbiters_i18n::mark("VRChat's window cannot be read here.").into(), detail: None });
            }
            grab = None;
            std::thread::sleep(Duration::from_millis(60));
            continue;
        };
        if paused {
            let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
            if !matches!(s.status.state, TranslateState::Problem { .. } | TranslateState::NeedsAgreement | TranslateState::NeedsAddress | TranslateState::NoLocalTranslator) {
                s.status.state = TranslateState::Paused;
            }
            drop(s);
            // Back from a menu, the view may have changed: all of it forgotten, a new look at once.
            reset_motion(&mut motion, shared);
            std::thread::sleep(Duration::from_millis(60));
            continue;
        }
        if grab.as_ref().is_none_or(|g| g.window != hwnd) {
            // Still VRChat's: a closed window's handle may be another program's now (verify-r15).
            grab = gpu.as_ref().filter(|_| super::desktop::is_vrchat_window(HWND(hwnd as *mut _), pid)).and_then(|g| Grab::new(HWND(hwnd as *mut _), g).map_err(|e| eprintln!("desk translate: {e}")).ok()); // i18n: not shown (a log's line)
            reset_motion(&mut motion, shared);
            if grab.is_none() {
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
        }
        let Some(g) = grab.as_mut() else { continue };
        if !g.newest() {
            // Its size changed: what was read no longer lies where it was.
            reset_motion(&mut motion, shared);
        }
        // Did the view move? Ten times a second, a few small patches.
        if last_signature.is_none_or(|t| t.elapsed() >= SIGNATURE_EVERY) {
            last_signature = Some(Instant::now());
            motion.sample(g.signature(client));
            shared.lock().unwrap_or_else(|e| e.into_inner()).moving = motion.moving();
        }
        let pace = vr_translate::pace_for(cost, false, quiet);
        let due = motion.needs_look() || last_look.is_none_or(|t| t.elapsed() >= pace);
        if motion.moving() || !due {
            std::thread::sleep(Duration::from_millis(30));
            continue;
        }
        let began = Instant::now();
        let Some(rgba) = g.picture(client) else {
            std::thread::sleep(Duration::from_millis(30));
            continue;
        };
        motion.looked(motion.last_sample().or_else(|| g.signature(client)));
        last_look = Some(began);
        looks += 1;
        let view = view(client);
        let picture = orbiters_translate::ocr::Rgba { width: client[0], height: client[1], pixels: &rgba };
        let pair = orbiters_translate::place::Pair { pictures: [picture, picture], from: [[0, 0]; 2], views: [view, view] };
        let area = pct(client, AREA_PCT);
        let zoom = (area[2] > ZOOM_FROM && looks % 2 == 0).then(|| {
            let side = ZOOM_SIDE.min(area[2]).min(area[3]);
            [client[0] / 2 - side / 2, client[1] / 2 - side / 2, side, side]
        });
        let look = orbiters_translate::pipeline::Look { pair, area, shown: &[], zoom };
        let read = match core.engine.as_mut() {
            Some(engine) => engine.read(&look),
            None => Err(anyhow::anyhow!("the text reader is not loaded")), // i18n: not shown (log detail)
        };
        let gaze = ([0.0; 3], [0.0, 0.0, -1.0]);
        let now = started.elapsed().as_secs_f64() - began.elapsed().as_secs_f64();
        cost = began.elapsed();
        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
        match read {
            Ok((lines, timings)) => {
                let before = core.tracker.tracks.iter().map(|t| t.id).max().unwrap_or(0);
                // Turned off during the read: what it found is never followed, nor asked for (verify-r15 B7).
                let Some(scene) = follow_unless_stopped(&mut core, stop, now, &look, lines, timings, gaze) else { break };
                quiet = !core.tracker.tracks.iter().any(|t| t.id > before);
                let fetch = fetching.lock().unwrap_or_else(|e| e.into_inner()).clone();
                s.status.state = match (&translator_state, &scene.activity) {
                    (Some(TranslateState::NoLocalTranslator), _) if matches!(fetch, Some((_, _, None))) => {
                        let (done, total, _) = fetch.unwrap_or_default();
                        TranslateState::DownloadingTranslator { done, total }
                    }
                    (Some(TranslateState::NoLocalTranslator), _) if matches!(fetch, Some((_, _, Some(Err(_))))) => TranslateState::Problem {
                        key: orbiters_i18n::mark("The built-in translator could not be downloaded.").into(),
                        detail: fetch.and_then(|f| f.2).and_then(|r| r.err()),
                    },
                    (Some(state), _) => state.clone(),
                    (None, Activity::Failed(f)) => vr_translate::failure_state(f),
                    (None, Activity::Translating(n)) => TranslateState::Translating(*n as u32),
                    (None, Activity::Waking) => TranslateState::Waking,
                    (None, Activity::Looking) => TranslateState::Looking,
                };
                s.status.lines = scene.lines as u32;
                s.status.look_ms = Some(cost.as_millis() as u32);
                s.status.server = lan.found.as_ref().map(|f| f.0.clone());
                s.status.models = lan.found.as_ref().map(|f| f.1.clone()).unwrap_or_default();
                s.status.waited_s = scene.waiting_s.map(|w| w as u32);
                s.status.small_model = lan.model.clone().filter(|m| orbiters_translate::provider::too_small(m));
                s.scene = Some((scene, client));
            }
            Err(e) => s.status.state = TranslateState::Problem { key: orbiters_i18n::mark("The text reader could not start.").into(), detail: Some(e.to_string()) },
        }
    }
    core.save_cache(true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_translate::ocr::{Line, TextBox};
    use orbiters_translate::picture::{draw_text, Text};
    use orbiters_translate::pipeline::{Look, Timings};
    use orbiters_translate::provider::{Batch, Failure, Translator};

    struct Echo;
    impl Translator for Echo {
        fn translate(&mut self, batch: &Batch) -> Result<Vec<String>, Failure> {
            Ok(batch.lines.iter().map(|l| format!("T {}", l.text)).collect())
        }
    }

    /// The box round the dark pixels of an RGBA picture: where a line drawn on a light ground is.
    fn ink(rgba: &[u8], w: u32) -> Rect {
        let (mut min, mut max) = (pos2(f32::MAX, f32::MAX), pos2(f32::MIN, f32::MIN));
        for (i, p) in rgba.chunks_exact(4).enumerate() {
            if (p[0] as u32 + p[1] as u32 + p[2] as u32) < 300 {
                let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
                min = min.min(pos2(x, y));
                max = max.max(pos2(x + 1.0, y + 1.0));
            }
        }
        Rect::from_min_max(min, max)
    }

    /// REQUEST15 item 2: a line read in VRChat's window, given to the same core as VR's with that one picture as both eyes, comes back as a translation whose
    /// window lies right over the line, at its height: the one picture tells no distance, so the line lies on its own ray and falls back onto its own pixels.
    #[test]
    fn a_line_read_in_vrchat_s_window_gets_its_translation_right_over_it() {
        let size = [1280u32, 720];
        for (text, at) in [("出口はこちらです", [300.0, 200.0]), ("Добро пожаловать", [700.0, 520.0])] {
            let picture = draw_text(size[0], size[1], [235, 228, 210], &[Text { text: text.into(), at, size: 44.0, colour: [20, 20, 20] }]);
            let b = ink(&picture.rgba, size[0]);
            let boxed = TextBox { corners: [[b.min.x, b.min.y], [b.max.x, b.min.y], [b.max.x, b.max.y], [b.min.x, b.max.y]], score: 0.9, vertical: false };
            let line = Line { boxed, text: text.into(), confidence: 0.95, reader: orbiters_translate::ocr::Reader::Main, glyphs: Vec::new() };
            let v = view(size);
            let rgba = orbiters_translate::ocr::Rgba { width: size[0], height: size[1], pixels: &picture.rgba };
            let pair = orbiters_translate::place::Pair { pictures: [rgba, rgba], from: [[0, 0]; 2], views: [v, v] };
            let look = Look { pair, area: [0, 0, size[0], size[1]], shown: &[], zoom: None };
            let mut core = Core::new(None, Box::new(Echo), Reader { target: "en".into(), reads: vec![] });
            let gaze = ([0.0; 3], [0.0, 0.0, -1.0]);
            let mut quads = Vec::new();
            for k in 0..400 {
                let scene = core.follow(k as f64 * 0.05, &look, vec![line.clone()], Timings::default(), gaze);
                if !scene.quads.is_empty() {
                    quads = scene.quads;
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(quads.len(), 1, "{text}: translated and shown");
            let r = screen_rect(&quads[0], size).expect("in front");
            assert!(r.contains(b.center()), "{text}: the translation {r:?} lies over the line {b:?}");
            assert!((r.center().y - b.center().y).abs() < b.height() * 0.5, "{text}: at its height ({r:?} for {b:?})");
            assert!(r.height() > b.height() * 0.6 && r.height() < b.height() * 2.5, "{text}: about as tall as its letters ({r:?} for {b:?})");
        }
    }

    /// verify-r15 B7: translation turned off while a look was being read: what it read is not followed, and no line of it reaches the translator.
    #[test]
    fn a_look_read_after_off_asks_for_nothing() {
        struct Counting(Arc<Mutex<usize>>);
        impl Translator for Counting {
            fn translate(&mut self, batch: &Batch) -> Result<Vec<String>, Failure> {
                *self.0.lock().unwrap() += batch.lines.len();
                Ok(batch.lines.iter().map(|l| l.text.clone()).collect())
            }
        }
        let size = [640u32, 360];
        let picture = draw_text(size[0], size[1], [235, 228, 210], &[Text { text: "出口はこちらです".into(), at: [100.0, 100.0], size: 40.0, colour: [20, 20, 20] }]);
        let b = ink(&picture.rgba, size[0]);
        let boxed = TextBox { corners: [[b.min.x, b.min.y], [b.max.x, b.min.y], [b.max.x, b.max.y], [b.min.x, b.max.y]], score: 0.9, vertical: false };
        let line = Line { boxed, text: "出口はこちらです".into(), confidence: 0.95, reader: orbiters_translate::ocr::Reader::Main, glyphs: Vec::new() };
        let v = view(size);
        let rgba = orbiters_translate::ocr::Rgba { width: size[0], height: size[1], pixels: &picture.rgba };
        let look = Look { pair: orbiters_translate::place::Pair { pictures: [rgba, rgba], from: [[0, 0]; 2], views: [v, v] }, area: [0, 0, size[0], size[1]], shown: &[], zoom: None };
        let asked = Arc::new(Mutex::new(0));
        let mut core = Core::new(None, Box::new(Counting(asked.clone())), Reader { target: "en".into(), reads: vec![] });
        let stop = AtomicBool::new(true);
        for k in 0..20 {
            assert!(follow_unless_stopped(&mut core, &stop, k as f64 * 0.05, &look, vec![line.clone()], Timings::default(), ([0.0; 3], [0.0, 0.0, -1.0])).is_none());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(*asked.lock().unwrap(), 0, "nothing asked after Off");
        assert!(core.tracker.tracks.is_empty(), "nothing followed");
        // Still on, the same look is followed and asked for: the test sees what it says it does.
        stop.store(false, Ordering::Relaxed);
        for k in 0..200 {
            let _ = follow_unless_stopped(&mut core, &stop, 2.0 + k as f64 * 0.05, &look, vec![line.clone()], Timings::default(), ([0.0; 3], [0.0, 0.0, -1.0]));
            if *asked.lock().unwrap() > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(*asked.lock().unwrap() > 0);
    }

    /// verify-r15: the view moving, then a menu up and gone (or another window, another size): everything known of the view is forgotten together, so the next
    /// look happens and the translations come back; moving left set with nothing to compare to kept them hidden for good.
    #[test]
    fn a_view_that_moved_then_a_menu_still_gets_its_look() {
        let client = [1280u32, 720];
        let scene = |dx: i32| move |x: u32, y: u32| -> u8 { let x = x as i32 + dx; (((x / 37 + y as i32 / 23) % 5) * 50 + (x % 7) * 3) as u8 };
        let mut m = Motion::default();
        assert!(m.needs_look());
        m.looked(Some(signature(client, &scene(0))));
        m.sample(Some(signature(client, &scene(0))));
        assert!(!m.moving() && !m.needs_look(), "still");
        m.sample(Some(signature(client, &scene(40))));
        assert!(m.moving(), "turned");
        // A menu opens while it turns: forgotten, and a look due when it goes.
        m.reset();
        assert!(!m.moving() && m.needs_look(), "never stuck moving");
        m.looked(Some(signature(client, &scene(40))));
        m.sample(Some(signature(client, &scene(80))));
        assert!(m.moving());
        m.sample(Some(signature(client, &scene(80))));
        assert!(!m.moving() && m.needs_look(), "still again: a look at once");
    }

    /// The patches of an in-memory BGRA picture, as [`Grab::signature`] reads them from the capture.
    fn signature(client: [u32; 2], pixel: &dyn Fn(u32, u32) -> u8) -> Signature {
        let parts: Vec<Vec<u8>> = patch_rects(client)
            .iter()
            .map(|[x0, y0, w, h]| (0..*h).flat_map(|y| (0..*w).flat_map(move |x| { let g = pixel(x0 + x, y0 + y); [g, g, g, 255] })).collect())
            .collect();
        let with_stride: Vec<(&[u8], u32)> = parts.iter().map(|p| (p.as_slice(), PATCH * 4)).collect();
        Signature::of(&with_stride)
    }

    /// A view that turns changes most patches: what was read goes; a video playing in one corner, or nothing at all, is a still view.
    #[test]
    fn a_turned_view_is_told_from_a_still_one() {
        let client = [1280u32, 720];
        let scene = |dx: i32| move |x: u32, y: u32| -> u8 { let x = x as i32 + dx; (((x / 37 + y as i32 / 23) % 5) * 50 + (x % 7) * 3) as u8 };
        let still = signature(client, &scene(0));
        assert!(!still.moved(&signature(client, &scene(0))), "nothing changed");
        assert!(still.moved(&signature(client, &scene(40))), "turned: everything shifted");
        // One patch's place changes (a video, a door): the view did not move.
        let r = patch_rects(client)[1];
        let video = move |x: u32, y: u32| -> u8 { if x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3] { 255 - scene(0)(x, y) } else { scene(0)(x, y) } };
        assert!(!still.moved(&signature(client, &video)), "one part changed: still");
    }

    /// A translation's picture, made as large as the line on the screen: clear stays clear, letters keep their colour, edges are premultiplied.
    #[test]
    fn a_translation_is_scaled_and_premultiplied_for_its_window() {
        // Straight alpha: an opaque red half and a clear half.
        let rgba: Vec<u8> = (0..4 * 2).flat_map(|i| if i % 4 < 2 { [200, 10, 10, 255] } else { [90, 90, 90, 0] }).collect();
        let out = scaled_premultiplied(&rgba, 4, 2, [8, 4]);
        assert_eq!(&out[0..4], &[200, 10, 10, 255], "the letters' colour");
        let last = out.len() - 4;
        assert_eq!(&out[last..], &[0, 0, 0, 0], "clear is clear, its colour gone");
        assert!(out.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]), "premultiplied");
    }

    /// verify2-r15 S3: translation turned on by VR or an older version reads nothing of VRChat's window, whoever translates (an AI server on the network
    /// included), until it is turned on on the desktop.
    #[test]
    fn nothing_is_read_before_it_is_turned_on_on_the_desktop() {
        use orbiters_core::settings::TranslateProvider;
        for provider in [TranslateProvider::ThisComputer, TranslateProvider::Address, TranslateProvider::Orbiters] {
            let upgraded = TranslateSettings { enabled: true, provider, server: "http://192.168.1.20:1234".into(), orbiters_agreed: true, ..Default::default() };
            assert!(!reads_window(&upgraded), "{provider:?}: on for VR only");
            assert_eq!(reads_window(&TranslateSettings { window_on: true, ..upgraded.clone() }), orbiters_core::here::HERE.window_text, "{provider:?}: on on the desktop");
            assert!(!reads_window(&TranslateSettings { enabled: false, window_on: true, ..upgraded }), "{provider:?}: off");
        }
    }

    /// REQUEST15 item 2: the view a picture is read through puts a pixel's ray back on that pixel.
    #[test]
    fn the_view_puts_each_pixel_back_where_it_was() {
        let v = view([1920, 1080]);
        for px in [[0.5, 0.5], [960.0, 540.0], [1800.0, 100.0], [200.0, 1000.0]] {
            let (o, d) = v.ray(px);
            let back = v.project(orbiters_translate::geometry::add(o, orbiters_translate::geometry::scale(d, 2.0))).unwrap();
            assert!((back[0] - px[0]).abs() < 0.01 && (back[1] - px[1]).abs() < 0.01, "{px:?} -> {back:?}");
        }
    }
}
