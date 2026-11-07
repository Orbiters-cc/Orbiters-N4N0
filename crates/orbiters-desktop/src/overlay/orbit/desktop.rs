//! The panels in desktop mode (`orbiters-desktop --orbit-desktop --service <port> <token>`): a process of its own, started by the service while VRChat
//! runs without SteamVR. Each panel is a borderless topmost layered window over VRChat's, that never takes the focus. Where a picture's alpha is 0 clicks pass
//! to the game, so only the panels are in its way. All of it goes as soon as VRChat is not in front.
//!
//! What was measured on the real game, in a window at ten sizes (2026-11-06):
//! - **A click on a no-activate window over a Quick Menu button stays there**: the button does not act and VRChat keeps the foreground. VRChat's hover does
//!   follow the pointer under the panel, as it reads the pointer's place directly, but a hover does nothing.
//! - **With the Quick Menu open the mouse is free** (the pointer is hidden by VRChat, which draws its own: over a panel, whose window sets its own, Windows' shows).
//! - **With the wheel open the mouse is VRChat's**: the pointer is clipped to one pixel and the mouse's movement chooses the wheel's slice. Nothing can
//!   hover the panels with it. So **holding Left Alt points at them**: a low-level mouse hook, only while Alt is held with the wheel open, keeps the
//!   mouse's movement and buttons from the game and moves a pointer of ours ([`pointer`]). Let go, the mouse is VRChat's again as it was.
//! - **Tab frees nothing**: it is VRChat's own.

use super::detect::{self, Part, Seen, MenuKey, Watcher};
use super::zones::{self, Around, Placed, Zone};
use super::{Panel, Panels};
use crate::backend::Ctl;
use crate::ipc::{Client, Endpoint, Role};
use crate::state::{OrbitStatus, Snapshot};
use anyhow::Context as _;
use egui::{pos2, vec2, Pos2, Vec2};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::{w, Interface};
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{COLORREF, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
    DIB_RGB_COLORS, HBITMAP, HDC,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE, VK_LBUTTON, VK_LMENU};
use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, GetClassNameW, GetClientRect, GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindow,
    LoadCursorW, PeekMessageW, RegisterClassW, SetWindowPos, ShowWindow, TranslateMessage, UpdateLayeredWindow, HWND_TOPMOST, IDC_ARROW, MA_NOACTIVATE, MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_NOOWNERZORDER,
    SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

/// A turn of the loop with nothing showing: Esc and R are looked at, with the game in front, and nothing else happens.
const REST: Duration = Duration::from_millis(33);
/// With something showing: the pointer follows at about 120 a second, each panel at most once a frame.
const TURN: Duration = Duration::from_millis(8);
/// VRChat's main window's class.
const UNITY_CLASS: &str = "UnityWndClass";


pub fn run(args: &[String]) -> anyhow::Result<()> {
    let endpoint = Endpoint::from_args(args).context("--orbit-desktop needs --service <port> <token>")?;
    // SAFETY: at the start of the process, before any window.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let client = Client::connect(&endpoint, Role::Orbit)?;
    let backend: Ctl = Arc::new(crate::backend::Remote(client.clone()));
    let mut game = Game::default();
    let mut windows = Windows::default();
    let mut panels = Panels::new(backend.clone(), 1.0, true);
    let mut watcher = Watcher::default();
    // Ready before any key: making it takes more than 100 ms, as long as VRChat takes to show a menu.
    let mut gpu = Gpu::new().map_err(|e| eprintln!("capture: {e}")).ok();
    let mut capture: Option<Capture> = None;
    let mut keys = Keys::default();
    let mut hints = Hints::default();
    let mut pill = Pill::default();
    let mut pointer = pointer::Pointer::default();
    let mut said: Option<OrbitStatus> = None;
    let mut last_turn = Instant::now();
    let mut last_look = None::<Instant>;
    let mut frames_seen = FrameRate::default();
    let clicks = Clicks::start();
    loop {
        let turn = Instant::now();
        let dt = turn.duration_since(last_turn).as_secs_f32();
        last_turn = turn;
        pump();
        let snap = backend.view();
        if client.should_close() || !super::wanted_on_desktop(&snap) {
            return Ok(());
        }
        let found = game.find(&snap);
        // Exclusive full screen shows no other window: say so, and do nothing.
        let exclusive = found.is_some_and(|_| full_screen_exclusive());
        let status = OrbitStatus { note: exclusive.then(|| "VRChat is in exclusive full screen, which no other window can be above. Use a window or borderless full screen.".to_string()), steamvr_overrides: None };
        if said.as_ref() != Some(&status) {
            backend.orbit_status(status.clone());
            said = Some(status);
        }
        // Only with VRChat itself in front, or a panel's window: anything else in front and it all goes at once.
        let in_front = found.filter(|h| !exclusive && foreground_is(*h, &windows));
        let Some(window) = in_front else {
            pill.show(None, pos2(0.0, 0.0), vec2(0.0, 0.0));
            if watcher.wants_frames() || !panels.is_empty() || capture.is_some() {
                watcher.reset();
                capture = None;
                pointer.stop();
                panels.set_open(None, Vec::new(), &snap);
                windows.hide_all();
            }
            keys.reset();
            std::thread::sleep(REST);
            continue;
        };
        if let Some(key) = keys.pressed() {
            watcher.pressed(key, turn);
        }
        if watcher.wants_frames() {
            if capture.is_none() {
                if gpu.is_none() {
                    gpu = Gpu::new().map_err(|e| eprintln!("capture: {e}")).ok();
                }
                capture = gpu.as_ref().and_then(|g| Capture::new(window, g).map_err(|e| eprintln!("capture: {e}")).ok());
                // A device lost (a driver's update, say) is made again next time.
                if capture.is_none() {
                    gpu = None;
                }
            }
            if let Some(c) = capture.as_mut() {
                // New frames, at the pace of the looks.
                if last_look.is_none_or(|t: Instant| turn.duration_since(t) >= watcher.look_every(turn)) {
                    last_look = Some(turn);
                    let client_size = client_size(window);
                    let found = c.look(window, client_size);
                    if let Some((found, coming)) = found {
                        frames_seen.count(turn);
                        watcher.frame_coming(found, coming, turn);
                    }
                }
            }
        }
        let seen = watcher.tick(turn);
        if !watcher.wants_frames() && capture.is_some() {
            capture = None;
        }
        let open = match seen {
            Seen::Open(Around::Wheel) if snap.settings.orbit.around_wheel() => Some(Around::Wheel),
            Seen::Open(Around::QuickMenu) if snap.settings.orbit.around_quick_menu() => Some(Around::QuickMenu),
            _ => None,
        };
        let size = client_size(window);
        let origin = client_origin(window);
        let places = open.map_or_else(Vec::new, |menu| zones::layout(menu, size));
        panels.takeover_place = zones::place(Zone::Takeover, size);
        panels.back_place = zones::place(Zone::SheetBack, size);
        panels.hint_place = zones::place(Zone::SheetHint, size);
        panels.set_open(open, places, &snap);
        panels.follow_note(&snap);
        if panels.is_empty() {
            pointer.stop();
            windows.hide_all();
            pill.show(panels.away_words(&snap), origin, size);
            std::thread::sleep(if watcher.wants_frames() { TURN } else { REST });
            continue;
        }
        pill.show(None, origin, size);
        panels.follow(&snap, backend.library_version());
        panels.menu_turn(&snap);
        panels.step_springs(dt);
        // The pointer: Windows' with the Quick Menu; ours while Alt is held with the wheel, which holds Windows'. A sheet opened from the wheel keeps it, with no key held.
        let cursor = cursor_at();
        let sheet_on_wheel = open == Some(Around::Wheel) && panels.sheet_open();
        pointer.follow(open == Some(Around::Wheel) && (alt_held() || sheet_on_wheel), origin, size, window, sheet_on_wheel);
        // Over VRChat's wheel too while a 3D picture waits: VRChat's lit slice has nothing to do with it, and two things would look chosen.
        panels.set_pointing((pointer.active() || panels.picture_waiting()) && !panels.sheet_open(), !snap.settings.reduce_motion);
        let at = pointer.at().unwrap_or(pos2(cursor.0 as f32, cursor.1 as f32));
        let on = (open != Some(Around::Wheel) || pointer.at().is_some()).then(|| hit(&panels, at - origin.to_vec2())).flatten();
        panels.hover(on);
        // With the wheel open, the mouse is VRChat's until Alt is held: what nobody can guess is said in the caption.
        hints.follow(open, pointer.active());
        panels.set_hint(hints.now(open, pointer.active()));
        // The press: what Windows gave our windows (pointer over them) and what the hook kept from the game.
        for (down, _) in MOUSE.with(|m| std::mem::take(&mut *m.borrow_mut())) {
            press(&mut panels, down, &snap, &mut windows, &clicks);
        }
        let wheel = WHEEL.swap(0, std::sync::atomic::Ordering::Relaxed);
        if wheel != 0 {
            panels.wheel(wheel as f32 / 120.0);
        }
        if let Some(cell) = keys.digit(open == Some(Around::Wheel) && !panels.sheet_open()) {
            if panels.press_cell(cell, &snap).clicked {
                clicks.play();
            }
        }
        for down in pointer.take_buttons() {
            press(&mut panels, down, &snap, &mut windows, &clicks);
        }
        // A button let go off a panel never reaches its window: what is held ends with it.
        if !pointer.active() && !button_down() && panels.all().any(|p| p.pressed.is_some()) {
            panels.release();
        }
        // What came into view lies over everything of ours, the pointer included: it goes back on top.
        if windows.show(&mut panels, &snap, origin) {
            pointer.raise();
        }
        let wait = TURN.saturating_sub(turn.elapsed());
        std::thread::sleep(wait);
    }
}

/// What the caption says with nothing pointed at, which someone who has seen it often enough no longer needs in full.
#[derive(Default)]
struct Hints {
    /// How often Left Alt was used on the ring, and how often the Quick Menu opened, since this started.
    pointed: u32,
    openings: u32,
    was_open: Option<Around>,
    was_pointing: bool,
}

impl Hints {
    /// How many times the full words show.
    const FULL: u32 = 3;

    fn follow(&mut self, open: Option<Around>, pointing: bool) {
        if open != self.was_open && open == Some(Around::QuickMenu) {
            self.openings += 1;
        }
        if pointing && !self.was_pointing {
            self.pointed += 1;
        }
        (self.was_open, self.was_pointing) = (open, pointing);
    }

    fn now(&self, open: Option<Around>, pointing: bool) -> Option<(&'static str, &'static str)> {
        match open {
            // Now the ring takes the mouse: it said so, it says so.
            Some(Around::Wheel) if pointing && self.pointed <= Self::FULL => Some(("Click a tool", "")),
            Some(Around::Wheel) if pointing => None,
            Some(Around::Wheel) if self.pointed < Self::FULL => Some(("Hold Left Alt, then click a tool", "Or press its number")),
            Some(Around::Wheel) => Some(("Left Alt to click, or a number", "")),
            // Says what this is, and that it is not VRChat's.
            Some(Around::QuickMenu) if self.openings <= Self::FULL => Some(("Orbiters' own menu", "Worlds in 3D, videos, instances and 3D pictures.")),
            _ => None,
        }
    }
}

/// `--orbit-bench <seconds>`: counts VRChat's frames through a capture of its window that only counts (what the panels' cost is measured with: run the same in each
/// state and compare). Prints frames a second, one line per 5 s. A tool for the report, not used by anything.
pub fn bench(args: &[String]) -> anyhow::Result<()> {
    let seconds: f32 = args.first().and_then(|a| a.parse().ok()).unwrap_or(20.0);
    // SAFETY: at the start of the process.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    // SAFETY: a plain query by name.
    let game = unsafe { windows::Win32::UI::WindowsAndMessaging::FindWindowW(w!("UnityWndClass"), w!("VRChat")) }.context("no VRChat window")?;
    let capture = Capture::new(game, &Gpu::new().map_err(|e| anyhow::anyhow!(e))?).map_err(|e| anyhow::anyhow!(e))?;
    let (begin, mut mark, mut frames) = (Instant::now(), Instant::now(), 0u32);
    while begin.elapsed().as_secs_f32() < seconds {
        while let Ok(frame) = capture.pool.TryGetNextFrame() {
            drop(frame);
            frames += 1;
        }
        if mark.elapsed() >= Duration::from_secs(5) {
            println!("{:.1}", frames as f32 / mark.elapsed().as_secs_f32());
            (mark, frames) = (Instant::now(), 0);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn press(panels: &mut Panels, down: bool, snap: &Snapshot, windows: &mut Windows, clicks: &Clicks) {
    if down {
        let pressed = panels.press(snap);
        if pressed.clicked {
            clicks.play();
        }
        if pressed.hide {
            panels.set_open(panels.open_menu(), Vec::new(), snap);
            windows.hide_all();
        }
    } else {
        panels.release();
    }
}

/// Which panel the point (window pixels) is on: its index in [`Panels::all`] and where in it in its units. The takeover in front first.
fn hit(panels: &Panels, p: Pos2) -> Option<(usize, Pos2)> {
    let all: Vec<&Panel> = panels.all().collect();
    (0..all.len()).rev().find_map(|i| {
        let panel = all[i];
        (panel.shown && panel.placed.contains(p)).then(|| {
            let uv = (p - panel.placed.rect.min) / panel.placed.rect.size();
            (i, panel.to_logical([uv.x, uv.y]))
        })
    })
}

fn alt_held() -> bool {
    // SAFETY: a plain query.
    unsafe { GetAsyncKeyState(VK_LMENU.0 as i32) as u16 & 0x8000 != 0 }
}

fn button_down() -> bool {
    // SAFETY: a plain query.
    unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000 != 0 }
}

fn cursor_at() -> (i32, i32) {
    let mut p = POINT::default();
    // SAFETY: the output is valid.
    let _ = unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
}

fn full_screen_exclusive() -> bool {
    // SAFETY: a plain query.
    unsafe { SHQueryUserNotificationState() }.is_ok_and(|s| s == QUNS_RUNNING_D3D_FULL_SCREEN)
}

fn client_size(window: HWND) -> Vec2 {
    let mut r = RECT::default();
    // SAFETY: the output is valid; a window gone only fails.
    let _ = unsafe { GetClientRect(window, &mut r) };
    vec2((r.right - r.left) as f32, (r.bottom - r.top) as f32)
}

fn client_origin(window: HWND) -> Pos2 {
    let mut p = POINT::default();
    // SAFETY: the output is valid.
    let _ = unsafe { ClientToScreen(window, &mut p) };
    pos2(p.x as f32, p.y as f32)
}

fn foreground_is(game: HWND, windows: &Windows) -> bool {
    // SAFETY: a plain query.
    let front = unsafe { GetForegroundWindow() };
    front == game || windows.owns(front)
}

fn pump() {
    let mut msg = MSG::default();
    // SAFETY: this thread's own messages.
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// The app's click, computed on a thread at the start, so that the first press already has it.
struct Clicks {
    output: orbiters_sound::Output,
    click: Arc<std::sync::OnceLock<Arc<orbiters_sound::Sound>>>,
}

impl Clicks {
    fn start() -> Self {
        let click = Arc::new(std::sync::OnceLock::new());
        let slot = click.clone();
        let _ = std::thread::Builder::new().name("orbit-click".into()).spawn(move || {
            use orbiters_sound::{render, sounds, RATE};
            let _ = slot.set(Arc::new(render(&sounds::ROCKER, RATE)));
        });
        Self { output: orbiters_sound::Output::new(), click }
    }

    fn play(&self) {
        if let Some(s) = self.click.get() {
            self.output.play(s);
        }
    }
}

/// VRChat's main window, found by its process and class.
#[derive(Default)]
struct Game {
    pid: Option<u32>,
    window: Option<isize>,
    last: Option<Instant>,
}

impl Game {
    fn find(&mut self, snap: &Snapshot) -> Option<HWND> {
        if self.pid != snap.vrchat.pid {
            self.pid = snap.vrchat.pid;
            self.window = None;
        }
        let pid = self.pid?;
        if let Some(h) = self.window {
            let hwnd = HWND(h as *mut _);
            // SAFETY: plain queries; a handle gone only says no.
            if unsafe { IsWindow(Some(hwnd)).as_bool() && !IsIconic(hwnd).as_bool() } {
                return Some(hwnd);
            }
            self.window = None;
        }
        // Not asked more than once a second while it has none yet (the game starting).
        if self.last.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return None;
        }
        self.last = Some(Instant::now());
        struct Search {
            pid: u32,
            found: Option<isize>,
        }
        unsafe extern "system" fn each(hwnd: HWND, l: LPARAM) -> windows::core::BOOL {
            // SAFETY: `l` is the `Search` below, alive through EnumWindows.
            let search = unsafe { &mut *(l.0 as *mut Search) };
            let mut owner = 0;
            // SAFETY: plain queries on a window enumerated now.
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
            if owner == search.pid {
                let mut name = [0u16; 32];
                // SAFETY: the buffer is valid.
                let n = unsafe { GetClassNameW(hwnd, &mut name) } as usize;
                if String::from_utf16_lossy(&name[..n]) == UNITY_CLASS {
                    search.found = Some(hwnd.0 as isize);
                    return false.into();
                }
            }
            true.into()
        }
        let mut search = Search { pid, found: None };
        // SAFETY: the pointer stays valid through the call.
        let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut search as *mut Search as isize)) };
        self.window = search.found;
        self.window.map(|h| HWND(h as *mut _))
    }
}

/// Esc and R pressed since the last look, with the game in front. `GetAsyncKeyState` is an ordinary query: it hooks nothing. A key going down is seen by its high bit
/// from one look to the next, and a press shorter than a turn by its low bit (which any other program's asking also clears: hence both).
#[derive(Default)]
struct Keys {
    was: [bool; 2],
    digits: [bool; 9],
}

impl Keys {
    /// The key just pressed, Esc before R when both were.
    fn pressed(&mut self) -> Option<MenuKey> {
        let mut any = None;
        for (was, (code, key)) in self.was.iter_mut().zip([(VK_ESCAPE.0 as i32, MenuKey::Esc), (0x52, MenuKey::R)]) {
            // SAFETY: a plain query.
            let state = unsafe { GetAsyncKeyState(code) } as u16;
            let down = state & 0x8000 != 0;
            if (down && !*was) || state & 1 != 0 {
                any = any.or(Some(key));
            }
            *was = down;
        }
        any
    }

    /// A number from 1 to 9 just pressed, as an index, when `live`. A key still down from before it was live counts for nothing.
    fn digit(&mut self, live: bool) -> Option<usize> {
        if !live {
            self.digits = [true; 9];
            return None;
        }
        let mut pressed = None;
        for (i, was) in self.digits.iter_mut().enumerate() {
            // SAFETY: a plain query.
            let down = unsafe { GetAsyncKeyState(0x31 + i as i32) } as u16 & 0x8000 != 0;
            if down && !*was {
                pressed = pressed.or(Some(i));
            }
            *was = down;
        }
        pressed
    }

    /// A press while something else is in front is not for VRChat.
    fn reset(&mut self) {
        let _ = self.pressed();
    }
}

/// How often VRChat's frames arrive in the capture, written to the log every 10 s while it runs: what the panels' cost is measured against.
#[derive(Default)]
struct FrameRate {
    since: Option<Instant>,
    frames: u32,
}

impl FrameRate {
    fn count(&mut self, now: Instant) {
        self.frames += 1;
        let since = *self.since.get_or_insert(now);
        if now.duration_since(since) >= Duration::from_secs(10) {
            eprintln!("orbit: {} looks in {:.1} s", self.frames, now.duration_since(since).as_secs_f32());
            self.since = Some(now);
            self.frames = 0;
        }
    }
}

// --------------------------------------------------------------------------------------------- the game's frames

/// Windows Graphics Capture of VRChat's window, alive only while there is something to look for; dropped, it costs nothing. The frames stay on the GPU: only the
/// two small parts a look reads are copied out ([`detect::parts`]), about 7% of the window at 16:9, and only as often as looks happen.
struct Capture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    item: GraphicsCaptureItem,
    winrt: IDirect3DDevice,
    size: (i32, i32),
    staging: [Option<(ID3D11Texture2D, u32, u32)>; 2],
    buffers: [Vec<u8>; 2],
}

/// The Direct3D device captures use, kept for as long as the process lasts: it holds nothing and does nothing between captures.
#[derive(Clone)]
struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt: IDirect3DDevice,
}

impl Gpu {
    fn new() -> Result<Self, String> {
        let text = |what: &str, e: windows::core::Error| format!("{what}: {}", e.message());
        let (mut device, mut context): (Option<ID3D11Device>, Option<ID3D11DeviceContext>) = (None, None);
        // SAFETY: the outputs are valid.
        unsafe { D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context)) }
            .map_err(|e| text("Direct3D", e))?;
        let (device, context) = (device.ok_or("no Direct3D device")?, context.ok_or("no Direct3D context")?);
        let dxgi: IDXGIDevice = device.cast().map_err(|e| text("DXGI", e))?;
        // SAFETY: the DXGI device is alive for the call.
        let winrt: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }.and_then(|d| d.cast()).map_err(|e| text("device", e))?;
        Ok(Self { device, context, winrt })
    }
}

impl Capture {
    fn new(window: HWND, gpu: &Gpu) -> Result<Self, String> {
        let text = |what: &str, e: windows::core::Error| format!("{what}: {}", e.message());
        let Gpu { device, context, winrt } = gpu.clone();
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(|e| text("capture item", e))?;
        // SAFETY: a window handle; asking for one that is gone only fails.
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(window) }.map_err(|e| text("capture item", e))?;
        let size = item.Size().map_err(|e| text("size", e))?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size).map_err(|e| text("frame pool", e))?;
        let session = pool.CreateCaptureSession(&item).map_err(|e| text("session", e))?;
        // No pointer in the picture, and Windows' yellow border off: this looks at the game, nobody is shown being recorded.
        let _ = session.SetIsCursorCaptureEnabled(false);
        let _ = session.SetIsBorderRequired(false);
        session.StartCapture().map_err(|e| text("start", e))?;
        Ok(Self { device, context, pool, session, item, winrt, size: (size.Width, size.Height), staging: [None, None], buffers: [Vec::new(), Vec::new()] })
    }

    /// The newest frame's look: `None` when no new frame came. The window's title bar and borders are in the frame, its client area counted from its own corner.
    fn look(&mut self, window: HWND, client: Vec2) -> Option<(Option<Around>, bool)> {
        let mut newest = None;
        while let Ok(frame) = self.pool.TryGetNextFrame() {
            newest = Some(frame);
        }
        let frame = newest?;
        let content = frame.ContentSize().ok()?;
        if (content.Width, content.Height) != self.size {
            // The window changed size: frames of the new one from now on.
            self.size = (content.Width, content.Height);
            let _ = self.pool.Recreate(&self.winrt, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, content);
            return None;
        }
        let surface = frame.Surface().ok()?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast().ok()?;
        // SAFETY: the surface is a Direct3D 11 texture, as the pool was made for.
        let texture: ID3D11Texture2D = unsafe { access.GetInterface() }.ok()?;
        let (dx, dy) = client_in_frame(window);
        let parts = detect::parts(client);
        let mut read: [Option<(u32, u32, u32, u32, u32)>; 2] = [None, None];
        for (i, part) in parts.iter().enumerate() {
            let (x0, y0) = (part.min.x.max(0.0) as u32, part.min.y.max(0.0) as u32);
            let (width, height) = (part.width().ceil() as u32, part.height().ceil() as u32);
            if width == 0 || height == 0 {
                return None;
            }
            let staging = self.staging_for(i, width, height)?;
            let region = D3D11_BOX { left: x0 + dx, top: y0 + dy, front: 0, right: x0 + dx + width, bottom: y0 + dy + height, back: 1 };
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            // SAFETY: both textures are this device's and of one format; the box is inside the frame (the parts are inside the client area), and the mapping is read
            // within its pitch and height, and unmapped before anything else happens to it.
            unsafe {
                self.context.CopySubresourceRegion(&staging, 0, 0, 0, 0, &texture, 0, Some(&region));
                self.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).ok()?;
                let pitch = mapped.RowPitch;
                let bytes = std::slice::from_raw_parts(mapped.pData as *const u8, (pitch * height) as usize);
                self.buffers[i].clear();
                self.buffers[i].extend_from_slice(bytes);
                self.context.Unmap(&staging, 0);
                read[i] = Some((x0, y0, width, height, pitch));
            }
        }
        let part = |i: usize| -> Option<Part<'_>> {
            let (x0, y0, width, height, stride) = read[i]?;
            Some(Part { x0, y0, width, height, stride, bgra: &self.buffers[i] })
        };
        Some((detect::look(&part(0)?, &part(1)?, client), detect::wheel_coming(&part(0)?, client)))
    }

    fn staging_for(&mut self, i: usize, width: u32, height: u32) -> Option<ID3D11Texture2D> {
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
        unsafe { self.device.CreateTexture2D(&description, None, Some(&mut staging)) }.ok()?;
        let staging = staging?;
        self.staging[i] = Some((staging.clone(), width, height));
        Some(staging)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
        let _ = &self.item;
    }
}

/// Where the client area's corner is in the frames of Windows Graphics Capture (the window's visual bounds).
fn client_in_frame(window: HWND) -> (u32, u32) {
    let (mut corner, mut bounds) = (POINT::default(), RECT::default());
    // SAFETY: the outputs and their sizes are passed; a window gone only fails.
    let known = unsafe {
        ClientToScreen(window, &mut corner).as_bool() && DwmGetWindowAttribute(window, DWMWA_EXTENDED_FRAME_BOUNDS, &mut bounds as *mut RECT as *mut _, std::mem::size_of::<RECT>() as u32).is_ok()
    };
    if known {
        ((corner.x - bounds.left).max(0) as u32, (corner.y - bounds.top).max(0) as u32)
    } else {
        (0, 0)
    }
}

// --------------------------------------------------------------------------------------------- the panels' windows

thread_local! {
    /// What the panels' windows heard: a button going down or up, and which window.
    static MOUSE: RefCell<Vec<(bool, isize)>> = const { RefCell::new(Vec::new()) };
}

/// The panels' window procedure: takes the mouse's button, and never the focus. That it answers at all is what keeps the click from the game.
/// The mouse wheel's movement since last read, in Windows' units (120 a notch): from our windows' messages, or the pointer's hook.
static WHEEL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        windows::Win32::UI::WindowsAndMessaging::WM_MOUSEWHEEL => {
            WHEEL.fetch_add(((wparam.0 >> 16) & 0xffff) as u16 as i16 as i32, std::sync::atomic::Ordering::Relaxed);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            MOUSE.with(|m| m.borrow_mut().push((true, hwnd.0 as isize)));
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            MOUSE.with(|m| m.borrow_mut().push((false, hwnd.0 as isize)));
            LRESULT(0)
        }
        // SAFETY: what the system passed on.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn class() -> windows::core::PCWSTR {
    let name = w!("OrbitersOrbitPanel");
    // SAFETY: registered once per process; a second time only fails.
    unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        let class = WNDCLASSW { lpfnWndProc: Some(procedure), hInstance: module.into(), lpszClassName: name, hCursor: cursor, ..Default::default() };
        RegisterClassW(&class);
    }
    name
}

/// A window of a panel: premultiplied pixels with per-pixel alpha, moved and faded without a new picture. `clicks` false lets them through (our pointer).
struct PanelWindow {
    hwnd: HWND,
    dc: HDC,
    bitmap: HBITMAP,
    bits: *mut u8,
    size: SIZE,
    shown: bool,
    at: POINT,
    alpha: u8,
    /// New pixels are in its bitmap, not yet on the screen: `UpdateLayeredWindow` is what shows them, moved or not.
    stale: bool,
}

impl PanelWindow {
    fn new(width: u32, height: u32, clicks: bool) -> anyhow::Result<Self> {
        let extra = if clicks { Default::default() } else { WS_EX_TRANSPARENT };
        // SAFETY: plain Win32 calls; every handle made here is released on drop.
        unsafe {
            let hwnd = CreateWindowExW(WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | extra, class(), w!("Orbiters"), WS_POPUP, 0, 0, width as i32, height as i32, None, None, None, None)?;
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);
            let header = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            };
            let bmi = BITMAPINFO { bmiHeader: header, ..Default::default() };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) => b,
                Err(e) => {
                    let _ = DeleteDC(dc);
                    let _ = DestroyWindow(hwnd);
                    return Err(e.into());
                }
            };
            SelectObject(dc, bitmap.into());
            Ok(Self { hwnd, dc, bitmap, bits: bits.cast(), size: SIZE { cx: width as i32, cy: height as i32 }, shown: false, at: POINT::default(), alpha: 0, stale: true })
        }
    }

    /// New pixels in the part `changed` (x0, y0, x1, y1) of premultiplied RGBA of the window's size.
    fn picture(&mut self, rgba: &[u8], changed: [u32; 4]) {
        let (w, h) = (self.size.cx as usize, self.size.cy as usize);
        if rgba.len() < w * h * 4 {
            return;
        }
        self.stale = true;
        // SAFETY: the DIB section holds w x h 32-bit pixels for as long as the window lives.
        let out = unsafe { std::slice::from_raw_parts_mut(self.bits, w * h * 4) };
        for y in changed[1] as usize..(changed[3] as usize).min(h) {
            let (a, b) = ((y * w + changed[0] as usize) * 4, (y * w + (changed[2] as usize).min(w)) * 4);
            for (o, i) in out[a..b].chunks_exact_mut(4).zip(rgba[a..b].chunks_exact(4)) {
                o[0] = i[2];
                o[1] = i[1];
                o[2] = i[0];
                o[3] = i[3];
            }
        }
    }

    /// Puts the window where it goes, at this alpha, and shows it (above everything: VRChat's a window among others, and what it has it may come above again).
    fn place(&mut self, at: POINT, alpha: u8) {
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
        let origin = POINT { x: 0, y: 0 };
        // SAFETY: live handles; the structures outlive the call.
        unsafe {
            let screen = GetDC(None);
            let _ = UpdateLayeredWindow(self.hwnd, Some(screen), Some(&at), Some(&self.size), Some(self.dc), Some(&origin), COLORREF(0), Some(&blend), ULW_ALPHA);
            ReleaseDC(None, screen);
            if !self.shown {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOSIZE | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
            }
        }
        self.shown = true;
        self.at = at;
        self.alpha = alpha;
        self.stale = false;
    }

    /// Above every other window of ours.
    fn raise(&self) {
        // SAFETY: a live window.
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOSIZE | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
        }
    }

    fn hide(&mut self) {
        if self.shown {
            // SAFETY: a live window.
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.shown = false;
        }
    }
}

impl Drop for PanelWindow {
    fn drop(&mut self) {
        // SAFETY: made in `new`, released once.
        unsafe {
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// The windows of the panels showing, by zone: each kept while hidden as long as its size is the panel's.
#[derive(Default)]
struct Windows {
    list: HashMap<Zone, PanelWindow>,
}

impl Windows {
    fn owns(&self, hwnd: HWND) -> bool {
        self.list.values().any(|w| w.hwnd == hwnd)
    }

    fn hide_all(&mut self) {
        for w in self.list.values_mut() {
            w.hide();
        }
    }

    /// New pictures, then every panel where its spring has it. Says whether one came into view, which then lies over all others.
    fn show(&mut self, panels: &mut Panels, snap: &Snapshot, origin: Pos2) -> bool {
        let list = &mut self.list;
        panels.paint(snap, |panel, changed| {
            let [w, h] = panel.px;
            let clicks = !panel.zone.passes_clicks();
            let window = list.entry(panel.zone).and_modify(|window| {
                if window.size.cx != w as i32 || window.size.cy != h as i32 {
                    *window = PanelWindow::new(w, h, clicks).expect("a window was made before");
                }
            });
            let window = match window {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => match PanelWindow::new(w, h, clicks) {
                    Ok(made) => e.insert(made),
                    Err(err) => {
                        eprintln!("panel window: {err}");
                        return;
                    }
                },
            };
            window.picture(panel.pixels(), changed);
        });
        let showing: Vec<(Zone, POINT, u8)> = panels.all().filter(|p| p.painted && p.shown).map(|p| placed_at(p, origin)).collect();
        let came_up = place_in_order(&mut self.list, &showing);
        for (zone, window) in &mut self.list {
            if !showing.iter().any(|(z, _, _)| z == zone) {
                window.hide();
            }
        }
        came_up
    }
}

/// Places the windows in the order they lie: what comes into view goes above what shows, so the sheet's page stays over its back. Says whether one came into view.
fn place_in_order(list: &mut HashMap<Zone, PanelWindow>, showing: &[(Zone, POINT, u8)]) -> bool {
    let mut raise = false;
    for (zone, at, alpha) in showing {
        if let Some(window) = list.get_mut(zone) {
            // One coming into view goes on top: those after it in order go on top of it again.
            let was_up = window.shown;
            if window.stale || !window.shown || window.at != *at || window.alpha != *alpha {
                window.place(*at, *alpha);
            }
            if !was_up {
                raise = true;
            } else if raise {
                window.raise();
            }
        }
    }
    raise
}

/// Where the panel's window goes and its alpha as its spring has them: it comes from the menu's side.
fn placed_at(panel: &Panel, origin: Pos2) -> (Zone, POINT, u8) {
    let Placed { rect, center, unit, .. } = panel.placed;
    let way = panel.zone.ring().map_or((rect.center() - center).normalized(), |ring| zones::polar(1.0, ring.middle()));
    let at = origin + rect.min.to_vec2() + way * panel.outward() * unit;
    (panel.zone, POINT { x: at.x.round() as i32, y: at.y.round() as i32 }, (panel.appear.alpha() * 255.0) as u8)
}

// --------------------------------------------------------------------------------------------- the pill with no menu up

/// Says what a 3D picture is doing while no menu is up: with VRChat's camera open on the desktop none can be, so nothing else can say it. It lets clicks through,
/// and is only there while it has something to say. The camera records its own stream, not this window.
#[derive(Default)]
struct Pill {
    window: Option<PanelWindow>,
    canvas: Option<crate::overlay::Canvas>,
    words: Option<(String, String)>,
    px: [u32; 2],
}

impl Pill {
    /// The caption's size, in its units, and how far under the window's top.
    const SIZE: Vec2 = vec2(560.0, 56.0);
    const TOP: f32 = 96.0;

    fn show(&mut self, words: Option<(String, String)>, origin: Pos2, client: Vec2) {
        let Some(said) = words else {
            if let Some(window) = self.window.as_mut() {
                window.hide();
            }
            self.words = None;
            return;
        };
        let zoom = (client.y / 720.0).max(0.5);
        let px = [(Self::SIZE.x * zoom).ceil() as u32, (Self::SIZE.y * zoom).ceil() as u32];
        if self.px != px || self.window.is_none() {
            self.window = PanelWindow::new(px[0], px[1], false).map_err(|e| eprintln!("pill window: {e}")).ok();
            self.canvas = Some(crate::overlay::Canvas::zoomed(px[0], px[1], zoom));
            self.px = px;
            self.words = None;
        }
        let (Some(window), Some(canvas)) = (self.window.as_mut(), self.canvas.as_mut()) else { return };
        if self.words.as_ref() != Some(&said) {
            let pixels = canvas.paint(0.0, |p| super::panels::draw_words(p, egui::Rect::from_min_size(pos2(0.0, 0.0), Self::SIZE), &said.0, &said.1));
            window.picture(pixels, [0, 0, px[0], px[1]]);
            self.words = Some(said);
        }
        let at = POINT { x: (origin.x + (client.x - px[0] as f32) / 2.0) as i32, y: (origin.y + Self::TOP * zoom) as i32 };
        if window.stale || !window.shown || window.at.x != at.x || window.at.y != at.y {
            window.place(at, 255);
        }
    }
}

// --------------------------------------------------------------------------------------------- the pointer while the wheel has the mouse

/// While the wheel is open VRChat holds the system's pointer (clipped to one pixel) and reads the mouse's movement for its slices. Holding Left Alt keeps the mouse
/// from the game for that time and moves a pointer of ours over the panels. A low-level hook is the one means to keep it from the game; it is set only
/// for as long as Alt is held with the wheel open, on a thread that does nothing else (a hook must answer at once), and anything that ends this ends the thread.
pub mod pointer {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU32, Ordering};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE,
        WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_MOUSEWHEEL,
    };

    /// The mouse's distance since the hook was set; buttons pressed and let go since last read.
    static DX: AtomicI64 = AtomicI64::new(0);
    static DY: AtomicI64 = AtomicI64::new(0);
    static REST_X: AtomicI32 = AtomicI32::new(0);
    static REST_Y: AtomicI32 = AtomicI32::new(0);
    static DOWNS: AtomicU32 = AtomicU32::new(0);
    static UPS: AtomicU32 = AtomicU32::new(0);
    static SET: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 && SET.load(Ordering::Relaxed) {
            let message = wparam.0 as u32;
            // SAFETY: for WH_MOUSE_LL, lparam is a live MSLLHOOKSTRUCT.
            let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            match message {
                // The system's pointer stays where it rests, so each movement comes as an offset from there.
                WM_MOUSEMOVE => {
                    DX.fetch_add((info.pt.x - REST_X.load(Ordering::Relaxed)) as i64, Ordering::Relaxed);
                    DY.fetch_add((info.pt.y - REST_Y.load(Ordering::Relaxed)) as i64, Ordering::Relaxed);
                    return LRESULT(1);
                }
                WM_LBUTTONDOWN => {
                    DOWNS.fetch_add(1, Ordering::Relaxed);
                    return LRESULT(1);
                }
                WM_LBUTTONUP => {
                    UPS.fetch_add(1, Ordering::Relaxed);
                    return LRESULT(1);
                }
                WM_MOUSEWHEEL => {
                    WHEEL.fetch_add((info.mouseData >> 16) as u16 as i16 as i32, Ordering::Relaxed);
                    return LRESULT(1);
                }
                WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN | WM_MBUTTONUP => return LRESULT(1),
                _ => {}
            }
        }
        // SAFETY: passing on what the system passed.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    /// Where it starts from the wheel's centre: on the first cell.
    const START: Vec2 = vec2(0.0, -140.0);
    /// The pointer's picture, in pixels.
    const PX: u32 = 24;

    #[derive(Default)]
    pub struct Pointer {
        /// The hook's thread, while Alt is held.
        thread: Option<(u32, std::thread::JoinHandle<()>)>,
        /// Where it is in screen pixels.
        at: Option<Pos2>,
        taken: (i64, i64),
        downs: u32,
        ups: u32,
        window: Option<PanelWindow>,
    }

    impl Pointer {
        pub fn active(&self) -> bool {
            self.thread.is_some()
        }

        pub fn at(&self) -> Option<Pos2> {
            self.at
        }

        /// A turn: `wanted` says Alt is held with the wheel open.
        pub fn follow(&mut self, wanted: bool, origin: Pos2, client: Vec2, game: HWND, keep: bool) {
            if !wanted {
                self.stop();
                return;
            }
            let _ = keep;
            if self.thread.is_none() {
                let (center, radius) = zones::wheel(client);
                let start = self.at.unwrap_or(origin + center.to_vec2() + START * radius / 100.0);
                self.at = Some(start);
                self.taken = (0, 0);
                let _ = game;
                self.begin();
            }
            let (dx, dy) = (DX.load(Ordering::Relaxed), DY.load(Ordering::Relaxed));
            let moved = vec2((dx - self.taken.0) as f32, (dy - self.taken.1) as f32);
            self.taken = (dx, dy);
            let window = egui::Rect::from_min_size(origin, client);
            let at = (self.at.unwrap_or(origin) + moved).clamp(window.min, window.max);
            self.at = Some(at);
            if self.window.is_none() {
                self.window = PanelWindow::new(PX, PX, false).map_err(|e| eprintln!("pointer window: {e}")).ok().map(|mut w| {
                    let picture = picture();
                    w.picture(&picture, [0, 0, PX, PX]);
                    w
                });
            }
            if let Some(w) = self.window.as_mut() {
                let spot = POINT { x: at.x as i32, y: at.y as i32 };
                if !w.shown || w.at != spot {
                    w.place(spot, 255);
                }
            }
        }

        fn begin(&mut self) {
            DX.store(0, Ordering::Relaxed);
            DY.store(0, Ordering::Relaxed);
            DOWNS.store(0, Ordering::Relaxed);
            UPS.store(0, Ordering::Relaxed);
            let (x, y) = cursor_at();
            REST_X.store(x, Ordering::Relaxed);
            REST_Y.store(y, Ordering::Relaxed);
            self.downs = 0;
            self.ups = 0;
            let (tx, rx) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new().name("orbit-mouse-hook".into()).spawn(move || {
                // SAFETY: a low-level hook of this thread, unset below; its messages are pumped right here, as such a hook needs.
                unsafe {
                    let module = GetModuleHandleW(None).unwrap_or_default();
                    let Ok(set) = SetWindowsHookExW(WH_MOUSE_LL, Some(hook), Some(module.into()), 0) else {
                        let _ = tx.send(0);
                        return;
                    };
                    SET.store(true, Ordering::Relaxed);
                    let _ = tx.send(windows::Win32::System::Threading::GetCurrentThreadId());
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
                    SET.store(false, Ordering::Relaxed);
                    let _ = UnhookWindowsHookEx(set);
                }
            });
            if let (Ok(handle), Ok(id)) = (spawned, rx.recv_timeout(Duration::from_secs(1))) {
                if id != 0 {
                    self.thread = Some((id, handle));
                }
            }
        }

        /// Back above every window of ours, as what it points at must never cover it.
        pub fn raise(&self) {
            if let Some(w) = self.window.as_ref().filter(|w| w.shown) {
                w.raise();
            }
        }

        /// Alt let go, the wheel gone, anything: the mouse is VRChat's again at once.
        pub fn stop(&mut self) {
            if let Some((id, handle)) = self.thread.take() {
                // SAFETY: a message to the hook's own thread.
                unsafe {
                    let _ = PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0));
                }
                let _ = handle.join();
            }
            if let Some(w) = self.window.as_mut() {
                w.hide();
            }
            self.at = None;
        }

        /// The left button's changes since the last read (true: down), in order.
        pub fn take_buttons(&mut self) -> Vec<bool> {
            let (downs, ups) = (DOWNS.load(Ordering::Relaxed), UPS.load(Ordering::Relaxed));
            let mut out = Vec::new();
            while self.downs < downs || self.ups < ups {
                if self.downs < downs {
                    out.push(true);
                    self.downs += 1;
                }
                if self.ups < self.downs && self.ups < ups {
                    out.push(false);
                    self.ups += 1;
                }
            }
            out
        }
    }

    impl Drop for Pointer {
        fn drop(&mut self) {
            self.stop();
        }
    }

    /// The pointer's picture: the accent's arrow on a dark edge, so that it reads on anything. RGBA premultiplied.
    fn picture() -> Vec<u8> {
        let mut canvas = crate::overlay::Canvas::new(PX, PX);
        canvas.paint(0.0, |p| {
            let body = [pos2(2.0, 1.0), pos2(2.0, 18.0), pos2(7.0, 13.5), pos2(13.5, 13.5)];
            p.add(egui::Shape::convex_polygon(body.to_vec(), orbiters_ui::theme::ACCENT, egui::Stroke::new(1.4, orbiters_ui::theme::INK_950)));
        });
        canvas.pixels().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Those of ours that show, as they lie, top first.
    fn from_the_top(ours: &[HWND]) -> Vec<HWND> {
        unsafe extern "system" fn each(hwnd: HWND, list: LPARAM) -> windows::core::BOOL {
            // SAFETY: the Vec given to EnumWindows below, alive for the call.
            unsafe { (*(list.0 as *mut Vec<HWND>)).push(hwnd) };
            true.into()
        }
        let mut all: Vec<HWND> = Vec::new();
        // SAFETY: the callback only pushes to that Vec.
        let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut all as *mut Vec<HWND> as isize)) };
        // SAFETY: a plain question about a window.
        all.retain(|h| ours.contains(h) && unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(*h) }.as_bool());
        all
    }

    /// Fully clear pictures: these real windows are never seen, wherever the tests run.
    fn clear_window() -> PanelWindow {
        let mut window = PanelWindow::new(8, 8, false).expect("a window");
        window.picture(&[0u8; 8 * 8 * 4], [0, 0, 8, 8]);
        window
    }

    #[test]
    fn what_comes_into_view_says_so_and_the_pointer_goes_back_over_it() {
        let at = POINT { x: 0, y: 0 };
        let mut list = HashMap::new();
        list.insert(Zone::WheelWorlds, clear_window());
        list.insert(Zone::BloomPeople, clear_window());
        let mut pointer = clear_window();
        let (ring, bloom, arrow) = (list[&Zone::WheelWorlds].hwnd, list[&Zone::BloomPeople].hwnd, pointer.hwnd);

        // The ring, with the pointer put up over it.
        assert!(place_in_order(&mut list, &[(Zone::WheelWorlds, at, 255)]));
        pointer.place(at, 255);
        assert_eq!(from_the_top(&[ring, bloom, arrow]), [arrow, ring]);
        // Nothing new: nothing to raise, nothing moves.
        assert!(!place_in_order(&mut list, &[(Zone::WheelWorlds, at, 255)]));

        // The second ring blooms: it lands over the pointer, as Enzo saw, and is said to have.
        assert!(place_in_order(&mut list, &[(Zone::WheelWorlds, at, 255), (Zone::BloomPeople, at, 255)]));
        assert_eq!(from_the_top(&[bloom, arrow])[0], bloom, "what the raise below is for");
        pointer.raise();
        assert_eq!(from_the_top(&[ring, bloom, arrow]), [arrow, bloom, ring]);
    }
}
