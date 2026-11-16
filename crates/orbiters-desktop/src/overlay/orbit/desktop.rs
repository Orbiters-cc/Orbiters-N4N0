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
//! - **VRChat's wheel does nothing with the mouse's scroll wheel** (2026-11-08: six notches either way, its lit slice stayed): so with no key held the scroll
//!   wheel lights the ring's cells one by one and a click opens the one lit ([`scroll`]). The hook that sees it lets VRChat have every movement and click
//!   but those: its click is ours only after a scroll, and a movement of the mouse gives it back.

use super::detect::{self, Part, Seen, MenuKey, Watcher};
use super::zones::{self, Around, Placed, Zone};
use super::{Panel, Panels};
use orbiters_i18n::mark;
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
    crate::usage::forward(backend.clone());
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
    let mut scroller = scroll::Scroller::default();
    let mut said: Option<OrbitStatus> = None;
    let mut last_turn = Instant::now();
    let mut last_look = None::<Instant>;
    let mut camera = detect::CameraWatch::default();
    let mut last_camera_look = None::<Instant>;
    let mut was_menu = false;
    // When VRChat's camera panel was last seen, as said to the service (it is part of what proves a live camera before a recording), at most twice a second.
    let mut panel_seen_ns: Option<u64> = None;
    let mut frames_seen = FrameRate::default();
    let clicks = Clicks::start();
    let mut corner = Corner::default();
    let mut noted = MismatchNoted::default();
    loop {
        let turn = Instant::now();
        let dt = turn.duration_since(last_turn).as_secs_f32();
        last_turn = turn;
        pump();
        let snap = backend.view();
        if client.should_close() || !super::wanted_on_desktop(&snap) {
            return Ok(());
        }
        let mismatch_on = crate::mismatch::switches(&snap.settings).buttons;
        crate::mismatch::set_on(mismatch_on);
        // The corner's clicks: its own, kept apart from the panels'.
        let corner_clicks: Vec<bool> = MOUSE.with(|m| {
            let mut m = m.borrow_mut();
            let (mine, rest): (Vec<_>, Vec<_>) = m.drain(..).partition(|(_, h)| corner.owns(HWND(*h as *mut _)));
            *m = rest;
            mine.into_iter().map(|(down, _)| down).collect()
        });
        let found = game.find(&snap);
        // Exclusive full screen shows no other window: say so, and do nothing.
        let exclusive = found.is_some_and(|_| full_screen_exclusive());
        let status = OrbitStatus { note: exclusive.then(|| mark("VRChat is in exclusive full screen, which no other window can be above. Use a window or borderless full screen.").to_string()), steamvr_overrides: None, camera_note: None, camera_provisional: false, camera_panel_seen_ns: panel_seen_ns, camera_scale_measured: false };
        if said.as_ref() != Some(&status) {
            backend.orbit_status(status.clone());
            said = Some(status);
        }
        // Only with VRChat itself in front, or a panel's window: anything else in front and it all goes at once.
        let in_front = found.filter(|h| !exclusive && foreground_is(*h, &windows));
        crate::mismatch::note_change("window: VRChat in front, its size", &mut noted.front, in_front.map(|w| { let s = client_size(w); (s.x as i32, s.y as i32) })); // i18n: not shown (a menu mismatch report's line)
        let Some(window) = in_front else {
            corner.show(false, pos2(0.0, 0.0), vec2(0.0, 0.0), turn);
            // The parts of menus that closed are kept for a while; with the game behind another window nothing else comes by to let them go.
            panels.let_go(turn);
            pill.show(None, pos2(0.0, 0.0), vec2(0.0, 0.0));
            camera.reset();
            panel_seen_ns = None;
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
        // VRChat's camera panel: for the person's own VRChat only, never one an agent drives through the bridge (its sessions' VRChats, the app's when the bridge
        // started it, one lent to the app for a picture): those are the agent's, and it clicks on that panel.
        let camera_wanted = camera_tools_here(&snap);
        if !camera_wanted {
            camera.reset();
        }
        crate::mismatch::note_change("input: Left Alt held", &mut noted.alt, alt_held()); // i18n: not shown (a menu mismatch report's line)
        crate::mismatch::note_change("vrchat: camera open (OSC)", &mut noted.camera, snap.camera_open_said); // i18n: not shown (a menu mismatch report's line)
        if let Some(key) = keys.pressed() {
            crate::mismatch::note("input: key", || format!("{key:?}")); // i18n: not shown (a menu mismatch report's line)
            watcher.pressed(key, turn);
            if key == MenuKey::Esc {
                camera.esc(turn);
            }
        }
        let camera_frames = camera_wanted && camera.wants_frames(snap.camera_open_said, turn);
        if watcher.wants_frames() || camera_frames {
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
                // New frames, at the pace of the looks: the menus' and the camera's each at its own.
                let menus = watcher.wants_frames() && last_look.is_none_or(|t: Instant| turn.duration_since(t) >= watcher.look_every(turn));
                let panel = camera_frames && last_camera_look.is_none_or(|t: Instant| turn.duration_since(t) >= detect::CAMERA_LOOK_EVERY);
                if menus || panel {
                    let client_size = client_size(window);
                    if let Some(looked) = c.look(window, client_size, menus, panel) {
                        frames_seen.count(turn);
                        if let Some((found, coming)) = looked.menu {
                            crate::mismatch::note_change("look: VRChat's window shows (menu, the wheel's rim coming)", &mut noted.look, (found, coming)); // i18n: not shown (a menu mismatch report's line)
                            last_look = Some(turn);
                            watcher.frame_coming(found, coming, turn);
                        }
                        if let Some(found) = looked.camera {
                            last_camera_look = Some(turn);
                            camera.frame(found, turn);
                        }
                    }
                }
            }
        }
        let seen = watcher.tick(turn);
        if noted.seen != Some(seen) {
            crate::mismatch::note("decide: seen", || format!("{:?} -> {seen:?} ({})", noted.seen, watcher.why())); // i18n: not shown (a menu mismatch report's line)
            noted.seen = Some(seen);
        }
        let menu = match seen {
            Seen::Open(Around::Wheel) if snap.settings.orbit.around_wheel() => Some(Around::Wheel),
            Seen::Open(Around::QuickMenu) if snap.settings.orbit.around_quick_menu() => Some(Around::QuickMenu),
            _ => None,
        };
        // A menu of VRChat's that went may have opened its camera (with OSC off, nothing else says so).
        let menu_up = matches!(seen, Seen::Open(_));
        if was_menu && !menu_up {
            camera.menu_went(turn);
        }
        was_menu = menu_up;
        let panel = if camera_wanted { camera.tick(snap.camera_open_said, turn) } else { None };
        panel_seen_ns = match panel {
            Some(_) if panel_seen_ns.is_none_or(|at| orbiters_core::clock::now_ns().saturating_sub(at) >= 500_000_000) => Some(orbiters_core::clock::now_ns()),
            Some(_) => panel_seen_ns,
            None => None,
        };
        if !watcher.wants_frames() && !(camera_wanted && camera.wants_frames(snap.camera_open_said, turn)) && capture.is_some() {
            capture = None;
        }
        // VRChat's menus first: they close its camera on the desktop.
        let open = menu.or(panel.filter(|_| !menu_up).map(|_| Around::Camera));
        crate::mismatch::note_change("decide: ours follow", &mut noted.open, open); // i18n: not shown (a menu mismatch report's line)
        let size = client_size(window);
        let origin = client_origin(window);
        // The menu mismatch corner: shown, its clicks and keys, a report for a press. Never anything more.
        corner.show(mismatch_on, origin, size, turn);
        let mut asked = corner.keyed().filter(|_| mismatch_on);
        for down in corner_clicks {
            if down && asked.is_none() {
                asked = corner.clicked(cursor_at());
            }
        }
        if let Some(kind) = asked.filter(|_| mismatch_on) {
            corner.pressed(kind, turn);
            corner.show(true, origin, size, turn);
            clicks.play();
            let state = vec![
                ("seen".to_owned(), format!("{seen:?}")),
                ("watcher".to_owned(), watcher.why()),
                ("ours follow".to_owned(), format!("{open:?}")), // i18n: not shown (a menu mismatch report's line)
                ("ours".to_owned(), panels.doing()),
                ("VRChat's camera panel seen".to_owned(), format!("{:?}", panel.is_some())), // i18n: not shown (a menu mismatch report's line)
                ("window client size".to_owned(), format!("{} x {}", size.x, size.y)), // i18n: not shown (a menu mismatch report's line)
                ("Left Alt held".to_owned(), alt_held().to_string()), // i18n: not shown (a menu mismatch report's line)
            ];
            let picture = if crate::mismatch::switches(&snap.settings).picture { screen_picture(origin, size) } else { None };
            crate::mismatch::press(kind, crate::mismatch::Context { surface: "desktop".into(), state, ..Default::default() }, &snap.settings, snap.vrchat.world.log_file.clone(), picture);
        }
        let places = match (open, panel) {
            (Some(Around::Camera), Some(panel)) => zones::camera_on_screen(panel, size),
            (Some(menu), _) => zones::layout_as(menu, size, snap.settings.orbit.layout == orbiters_core::settings::OrbitLayout::Strip),
            (None, _) => Vec::new(),
        };
        panels.takeover_place = zones::place(Zone::Takeover, size);
        panels.back_place = zones::place(Zone::SheetBack, size);
        panels.hint_place = zones::place(Zone::SheetHint, size);
        panels.set_open(open, places, &snap);
        panels.follow_note(&snap);
        if panels.is_empty() {
            pointer.stop();
            windows.hide_all();
            pill.show(panels.away_words(&snap), origin, size);
            std::thread::sleep(if watcher.wants_frames() || camera_frames { TURN } else { REST });
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
        // With no key held, the scroll wheel picks a cell (VRChat's wheel has no use for it), and the next click opens it.
        scroller.follow(SCROLL_TO_PICK && open == Some(Around::Wheel) && !pointer.active() && !panels.sheet_open());
        let scrolled = scroller.lit();
        // Over VRChat's wheel while Alt points at ours, or while a 3D picture waits. VRChat lights the slice its own pointer last reached, and nothing from here can
        // unlight it (the movement is held from it, not undone): two things looked chosen (round 14's judges). While Alt is held its clicks are ours, so its wheel is
        // shown stepped back, still in view (round 13's judges: an opaque disc hid it all); while a 3D picture waits, an opaque disc says what waits.
        let _ = scrolled;
        panels.set_pointing((pointer.active() || panels.picture_waiting()) && !panels.sheet_open(), !snap.settings.reduce_motion);
        panels.set_key_caps(alt_held());
        let at = pointer.at().unwrap_or(pos2(cursor.0 as f32, cursor.1 as f32));
        let on = match scrolled {
            Some(cell) => panels.cell_point(cell),
            None => (open != Some(Around::Wheel) || pointer.at().is_some()).then(|| hit(&panels, at - origin.to_vec2())).flatten(),
        };
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
        if let Some(cell) = scroller.take_click() {
            if panels.press_cell(cell, &snap).clicked {
                clicks.play();
            }
        }
        // A button let go off a panel never reaches its window: what is held ends with it.
        if !pointer.active() && !button_down() && panels.all().any(|p| p.pressed.is_some()) {
            panels.release();
        }
        // What came into view lies over everything of ours, the pointer included: it goes back on top.
        if windows.show(&mut panels, &snap, origin, open) {
            pointer.raise();
            corner.raise();
        }
        let wait = TURN.saturating_sub(turn.elapsed());
        std::thread::sleep(wait);
    }
}

/// Whether the camera's column is for this VRChat: the setting is on, and it is the person's own, not one an agent drives through the bridge.
fn camera_tools_here(snap: &Snapshot) -> bool {
    let driven = snap.vrchat.pid.is_some_and(|pid| snap.bridge.driven.contains(&pid));
    snap.settings.orbit.camera_tiles && !driven
}

/// Whether the scroll wheel picks a cell of the ring, with no key held ([`scroll`]). Off since dev.25: in dev.24 a scroll's click that opened a sheet froze the
/// panels once on the real game, and the cause is not proven yet. Left Alt and the number keys work as ever.
const SCROLL_TO_PICK: bool = false;

/// What the caption says with nothing pointed at, which someone who has seen it often enough no longer needs in full.
#[derive(Default)]
pub(super) struct Hints {
    /// How often Left Alt was used on the ring, and how often the Quick Menu opened, since this started.
    pointed: u32,
    openings: u32,
    was_open: Option<Around>,
    was_pointing: bool,
}

impl Hints {
    /// How many times the full words show.
    const FULL: u32 = 3;

    pub(super) fn follow(&mut self, open: Option<Around>, pointing: bool) {
        if open != self.was_open && open == Some(Around::QuickMenu) {
            self.openings += 1;
        }
        if pointing && !self.was_pointing {
            self.pointed += 1;
        }
        (self.was_open, self.was_pointing) = (open, pointing);
    }

    pub(super) fn now(&self, open: Option<Around>, pointing: bool) -> Option<(&'static str, &'static str)> {
        match open {
            // Now the ring takes the mouse: it said so, it says so.
            Some(Around::Wheel) if pointing && self.pointed <= Self::FULL => Some((mark("Click a tool"), "")),
            Some(Around::Wheel) if pointing => None,
            // The scroll wheel first, where it is on: it needs no key (round 12's judges: Left Alt was the friction of the desktop's wheel).
            Some(Around::Wheel) if SCROLL_TO_PICK && self.pointed < Self::FULL => Some((mark("Scroll to pick a tool, then click"), mark("Or hold Left Alt to point; keys 1 to 6"))),
            Some(Around::Wheel) if SCROLL_TO_PICK => Some((mark("Scroll to pick, or hold Left Alt"), "")),
            Some(Around::Wheel) if self.pointed < Self::FULL => Some((mark("Hold Left Alt, then click a tool"), mark("Or press its number, 1 to 6, from the top clockwise"))),
            Some(Around::Wheel) => Some((mark("Left Alt to click, or a number"), "")),
            // Says what this is, and that it is not VRChat's.
            Some(Around::QuickMenu) if self.openings <= Self::FULL => Some((mark("Your tools"), mark("Worlds in 3D, videos, instances and 3D pictures."))),
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
    let game = unsafe { windows::Win32::UI::WindowsAndMessaging::FindWindowW(w!("UnityWndClass"), w!("VRChat")) }.context("no VRChat window")?; // i18n: not shown (a developer's benchmark command)
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
    staging: Vec<Option<(ID3D11Texture2D, u32, u32)>>,
    buffers: Vec<Vec<u8>>,
}

/// Where a part was read from and how: x0, y0, width, height, the bytes of a row.
type ReadPart = (u32, u32, u32, u32, u32);

/// What a look at a frame found: VRChat's menu and whether its wheel's rim is in place, when the menus were looked for; its camera panel's place, when that was.
struct Looked {
    menu: Option<(Option<Around>, bool)>,
    camera: Option<Option<egui::Rect>>,
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
        let (device, context) = (device.ok_or("no Direct3D device")?, context.ok_or("no Direct3D context")?); // i18n: not shown (logged by the caller, never drawn)
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
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(|e| text("capture item", e))?; // i18n: not shown (logged, never drawn)
        // SAFETY: a window handle; asking for one that is gone only fails.
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(window) }.map_err(|e| text("capture item", e))?; // i18n: not shown
        let size = item.Size().map_err(|e| text("size", e))?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size).map_err(|e| text("frame pool", e))?; // i18n: not shown
        let session = pool.CreateCaptureSession(&item).map_err(|e| text("session", e))?;
        // No pointer in the picture, and Windows' yellow border off: this looks at the game, nobody is shown being recorded.
        let _ = session.SetIsCursorCaptureEnabled(false);
        let _ = session.SetIsBorderRequired(false);
        session.StartCapture().map_err(|e| text("start", e))?;
        Ok(Self { device, context, pool, session, item, winrt, size: (size.Width, size.Height), staging: Vec::new(), buffers: Vec::new() })
    }

    /// The newest frame's look, for VRChat's `menus`, its `camera` panel or both: `None` when no new frame came. The window's title bar and borders are in the frame,
    /// its client area counted from its own corner.
    fn look(&mut self, window: HWND, client: Vec2, menus: bool, camera: bool) -> Option<Looked> {
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
        // Each part keeps its slot, and so its staging texture, whichever of them a look reads: the menus' two first, the camera's lines after them.
        let menu_parts = detect::parts(client);
        let camera_parts = detect::camera_parts(client);
        let parts: Vec<Option<egui::Rect>> =
            menu_parts.iter().map(|r| menus.then_some(*r)).chain(camera_parts.iter().map(|r| camera.then_some(*r))).collect();
        if self.buffers.len() < parts.len() {
            self.buffers.resize_with(parts.len(), Vec::new);
            self.staging.resize_with(parts.len(), || None);
        }
        let mut read: Vec<Option<ReadPart>> = vec![None; parts.len()];
        for (i, part) in parts.iter().enumerate() {
            let Some(part) = part else { continue };
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
        let menu = if menus { Some((detect::look(&part(0)?, &part(1)?, client), detect::wheel_coming(&part(0)?, client))) } else { None };
        let camera = if camera {
            let from = menu_parts.len();
            let lines: Option<Vec<Part<'_>>> = (from..parts.len()).map(part).collect();
            Some(detect::camera_panel(&lines?, client))
        } else {
            None
        };
        Some(Looked { menu, camera })
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
    /// The bitmap the device context held before ours, put back before ours is deleted.
    first_bitmap: windows::Win32::Graphics::Gdi::HGDIOBJ,
    /// Its painted pixels take the mouse from what is under them (no `WS_EX_TRANSPARENT`).
    catches: bool,
}

impl PanelWindow {
    /// Whether its painted pixels take the mouse from what is under them, from now on: with `false` every pixel of it lets the mouse through, as if it were not there.
    fn catch_mouse(&mut self, on: bool) {
        if self.catches == on {
            return;
        }
        use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE};
        // SAFETY: a live window of this thread; only its extended style's one bit changes, which Windows' hit testing reads as it goes.
        unsafe {
            let style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let transparent = WS_EX_TRANSPARENT.0 as isize;
            let _ = SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, if on { style & !transparent } else { style | transparent });
        }
        self.catches = on;
    }

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
            let first_bitmap = SelectObject(dc, bitmap.into());
            Ok(Self { hwnd, dc, bitmap, bits: bits.cast(), size: SIZE { cx: width as i32, cy: height as i32 }, shown: false, at: POINT::default(), alpha: 0, stale: true, first_bitmap, catches: clicks })
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
            let updated = UpdateLayeredWindow(self.hwnd, Some(screen), Some(&at), Some(&self.size), Some(self.dc), Some(&origin), COLORREF(0), Some(&blend), ULW_ALPHA);
            ReleaseDC(None, screen);
            // A layered window shown without its picture is drawn as an opaque black rectangle of its whole size: what round 17's judges saw round the wheel's
            // caption. It shows only with its picture on it, and is hidden, to try again on the next turn, when Windows refused the picture.
            if let Err(e) = updated {
                eprintln!("panel window: its picture was refused ({e}), it stays hidden"); // i18n: not shown (a log's line)
                if self.shown {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                    self.shown = false;
                }
                self.stale = true;
                return;
            }
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
            // Ours out of the device context first: a bitmap still selected is not deleted, and each window made anew (a panel of a new size) leaked one,
            // until Windows had none left to give and refused pictures.
            SelectObject(self.dc, self.first_bitmap);
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

    /// New pictures, then every panel where its spring has it, taking the mouse or not as [`catches_mouse`] says for the menu `open`. Says whether one came into view,
    /// which then lies over all others.
    fn show(&mut self, panels: &mut Panels, snap: &Snapshot, origin: Pos2, open: Option<Around>) -> bool {
        let list = &mut self.list;
        panels.paint(snap, |panel, changed| {
            let [w, h] = panel.px;
            let clicks = catches_mouse(panel.zone, open);
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
        for (zone, window) in &mut self.list {
            window.catch_mouse(catches_mouse(*zone, open));
        }
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

/// Whether the window of a part in `zone` takes the mouse from VRChat where it has pixels, with the menu `open` up; where it does not, the mouse reaches VRChat exactly
/// as if Orbiters were not running.
///
/// With VRChat's wheel open, none does. VRChat then holds Windows' pointer at its window's centre (its slice follows the mouse's movement and its click goes where the
/// pointer is held), and the ring's 3D pictures cell lies over that point at every window size (160 hundredths of the wheel's radius up and left of its centre): Windows
/// gave that cell every press, which it did nothing with, and VRChat's wheel never had a click (Enzo, 2026-11-15, dev.56; every build since the ring, 2026-11-06). The
/// ring takes nothing from Windows' pointer anyway: its cells are pressed through Left Alt's pointer, whose hook keeps the mouse from everything while it lasts, and
/// through the number keys; a page opened from the wheel has that hook too.
fn catches_mouse(zone: Zone, open: Option<Around>) -> bool {
    !zone.passes_clicks() && open != Some(Around::Wheel)
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

// --------------------------------------------------------------------------------------------- the menu mismatch corner

/// The beta's menu mismatch buttons on the desktop ([`crate::mismatch`]): three rows in the bottom left corner of VRChat's window (VRChat's own hints are in its top
/// corners), above every window of ours, each pressed with a plain click or with Left Alt and its function key, which also works while VRChat's wheel holds the
/// pointer at its window's centre. Only its rows take the mouse; a press says Noted on its row for a moment and reports, nothing else.
#[derive(Default)]
pub(crate) struct Corner {
    window: Option<PanelWindow>,
    canvas: Option<crate::overlay::Canvas>,
    px: [u32; 2],
    zoom: f32,
    /// What its picture shows: the row that says Noted, and the language.
    drawn: Option<(Option<crate::mismatch::Kind>, orbiters_i18n::Language)>,
    noted: Option<(crate::mismatch::Kind, Instant)>,
    keys: [bool; 3],
}

impl Corner {
    /// Its size in its units (pixels of a 720 high window), its margin from the window's edges.
    pub(crate) const SIZE: Vec2 = vec2(268.0, 3.0 * Self::ROW + 2.0 * Self::GAP);
    const ROW: f32 = 30.0;
    const GAP: f32 = 5.0;
    pub(super) const MARGIN: f32 = 14.0;

    /// Where it lies in a window of `client` pixels (window pixels), and its zoom: in the bottom left corner, lifted over any part of ours that any of the
    /// menus' layouts puts there (the strip under the Quick Menu in a squarish window), so that it stays in one place for a window's size and covers none of
    /// them. Only a page of ours, open, may lie under it, and it stays above that.
    pub(crate) fn rect(client: Vec2) -> (egui::Rect, f32) {
        let zoom = (client.y / 720.0).clamp(0.75, 2.0);
        let size = Self::SIZE * zoom;
        let first = egui::Rect::from_min_size(pos2(Self::MARGIN * zoom, client.y - Self::MARGIN * zoom - size.y), size);
        let parts: Vec<egui::Rect> = [zones::layout_as(Around::QuickMenu, client, true), zones::layout(Around::QuickMenu, client), zones::layout(Around::Wheel, client)]
            .concat()
            .into_iter()
            .chain(zones::place(Zone::SheetHint, client))
            .map(|p| p.rect)
            .collect();
        let mut r = first;
        for _ in 0..6 {
            let Some(top) = parts.iter().filter(|p| p.intersects(r.expand(4.0 * zoom))).map(|p| p.top()).reduce(f32::min) else { break };
            r = r.translate(vec2(0.0, top - 6.0 * zoom - r.bottom()));
        }
        // VRChat's hints are in the window's top corners: no higher than this; with no room, at the bottom, above ours.
        if r.top() < client.y * 0.3 {
            r = first;
        }
        (r, zoom)
    }

    /// The row at `p` in its units.
    pub(super) fn row_at(p: Pos2) -> Option<crate::mismatch::Kind> {
        if p.x < 0.0 || p.x > Self::SIZE.x {
            return None;
        }
        crate::mismatch::Kind::ALL.into_iter().enumerate().find_map(|(i, k)| {
            let top = i as f32 * (Self::ROW + Self::GAP);
            (p.y >= top && p.y < top + Self::ROW).then_some(k)
        })
    }

    pub(super) fn owns(&self, hwnd: HWND) -> bool {
        self.window.as_ref().is_some_and(|w| w.hwnd == hwnd)
    }

    /// Shown over VRChat's window at `origin` (screen pixels) of `client` pixels while `on`, else hidden.
    pub(super) fn show(&mut self, on: bool, origin: Pos2, client: Vec2, now: Instant) {
        if !on {
            if let Some(w) = self.window.as_mut() {
                w.hide();
            }
            return;
        }
        let (rect, zoom) = Self::rect(client);
        let px = [rect.width().ceil() as u32, rect.height().ceil() as u32];
        if self.px != px || self.window.is_none() {
            self.window = PanelWindow::new(px[0], px[1], true).map_err(|e| eprintln!("mismatch corner window: {e}")).ok(); // i18n: not shown (a menu mismatch report's line)
            self.canvas = Some(crate::overlay::Canvas::zoomed(px[0], px[1], zoom));
            self.px = px;
            self.zoom = zoom;
            self.drawn = None;
        }
        let (Some(window), Some(canvas)) = (self.window.as_mut(), self.canvas.as_mut()) else { return };
        let noted = self.noted.filter(|(_, at)| now.duration_since(*at) < Duration::from_millis(1600)).map(|(k, _)| k);
        let look = (noted, orbiters_i18n::language());
        if self.drawn != Some(look) {
            // The first paint loads the fonts.
            for _ in 0..if self.drawn.is_none() { 2 } else { 1 } {
                canvas.paint(0.0, |p| paint_corner(p, noted));
            }
            window.picture(canvas.pixels(), [0, 0, px[0], px[1]]);
            self.drawn = Some(look);
        }
        let at = POINT { x: (origin.x + rect.min.x) as i32, y: (origin.y + rect.min.y) as i32 };
        if window.stale || !window.shown || window.at.x != at.x || window.at.y != at.y {
            window.place(at, 255);
        }
    }

    /// Above every other window of ours.
    pub(super) fn raise(&self) {
        if let Some(w) = self.window.as_ref().filter(|w| w.shown) {
            w.raise();
        }
    }

    /// A click at `screen` (screen pixels) on it: the row there.
    pub(super) fn clicked(&self, screen: (i32, i32)) -> Option<crate::mismatch::Kind> {
        let w = self.window.as_ref().filter(|w| w.shown)?;
        Self::row_at(pos2((screen.0 - w.at.x) as f32 / self.zoom, (screen.1 - w.at.y) as f32 / self.zoom))
    }

    /// Left Alt and a row's function key just pressed (F7, F8, F9), with the game in front.
    pub(super) fn keyed(&mut self) -> Option<crate::mismatch::Kind> {
        let alt = alt_held();
        let mut pressed = None;
        for (i, kind) in crate::mismatch::Kind::ALL.into_iter().enumerate().take(self.keys.len()) {
            // SAFETY: a plain query. VK_F1 is 0x70.
            let down = unsafe { GetAsyncKeyState(0x70 + kind.function_key() as i32 - 1) } as u16 & 0x8000 != 0;
            if alt && down && !self.keys[i] {
                pressed = pressed.or(Some(kind));
            }
            self.keys[i] = down;
        }
        pressed
    }

    pub(super) fn pressed(&mut self, kind: crate::mismatch::Kind, now: Instant) {
        self.noted = Some((kind, now));
    }
}

/// The corner's picture in its units: a row a kind, its words and its keys, the one just pressed saying Noted.
pub(crate) fn paint_corner(p: &egui::Painter, noted: Option<crate::mismatch::Kind>) {
    use orbiters_ui::theme::{self, weight, Txt};
    for (i, kind) in crate::mismatch::Kind::ALL.into_iter().enumerate() {
        let r = egui::Rect::from_min_size(pos2(0.0, i as f32 * (Corner::ROW + Corner::GAP)), vec2(Corner::SIZE.x, Corner::ROW));
        let is_noted = noted == Some(kind);
        p.rect_filled(r, r.height() / 2.0, if is_noted { theme::mix(theme::INK_950, theme::ACCENT, 0.35) } else { theme::alpha(theme::INK_950, 0.9) });
        p.rect_stroke(r, r.height() / 2.0, egui::Stroke::new(1.0, if is_noted { theme::ACCENT } else { theme::white(0.16) }), egui::StrokeKind::Inside);
        let dot = pos2(r.left() + 15.0, r.center().y);
        if is_noted {
            p.line_segment([dot + vec2(-5.0, 0.0), dot + vec2(-1.5, 4.0)], egui::Stroke::new(2.2, theme::ACCENT));
            p.line_segment([dot + vec2(-1.5, 4.0), dot + vec2(6.0, -4.5)], egui::Stroke::new(2.2, theme::ACCENT));
        } else if kind == crate::mismatch::Kind::MenuShouldBeOff {
            p.circle_stroke(dot, 5.5, egui::Stroke::new(1.6, theme::FG_3));
            p.line_segment([dot + vec2(-4.0, 4.0), dot + vec2(4.0, -4.0)], egui::Stroke::new(1.6, theme::FG_3));
        } else {
            p.circle_filled(dot, 5.5, theme::ACCENT);
        }
        let keys = format!("Alt+F{}", kind.function_key());
        let caps = orbiters_ui::widgets::text(p, pos2(r.right() - 12.0, r.center().y), egui::Align2::RIGHT_CENTER, &Txt::new(&keys, 10.5).color(theme::FG_3));
        let (words, colour) = if is_noted { (orbiters_i18n::tr(crate::mismatch::NOTED_WORDS), theme::ACCENT) } else { (orbiters_i18n::tr(kind.words()), theme::FG) };
        orbiters_ui::widgets::text_truncated(p, pos2(dot.x + 13.0, r.center().y), egui::Align2::LEFT_CENTER, &Txt::new(words, 13.0).weight(weight::SEMIBOLD).color(colour), caps.left() - dot.x - 20.0);
    }
}

/// What VRChat's window shows now, as the screen has it (ours over it included), made small: the picture of a menu mismatch report.
fn screen_picture(origin: Pos2, client: Vec2) -> Option<crate::mismatch::Picture> {
    use windows::Win32::Graphics::Gdi::{BitBlt, CAPTUREBLT, SRCCOPY};
    let (w, h) = (client.x as i32, client.y as i32);
    if w <= 0 || h <= 0 {
        return None;
    }
    // SAFETY: plain GDI calls; every handle made here is released before returning.
    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let header = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() };
        let bmi = BITMAPINFO { bmiHeader: header, ..Default::default() };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok();
        let mut out = None;
        if let Some(bitmap) = bitmap {
            let old = SelectObject(dc, bitmap.into());
            if BitBlt(dc, 0, 0, w, h, Some(screen), origin.x as i32, origin.y as i32, SRCCOPY | CAPTUREBLT).is_ok() {
                let bgra = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
                let rgba: Vec<u8> = bgra.chunks_exact(4).flat_map(|c| [c[2], c[1], c[0], 255]).collect();
                out = crate::mismatch::Picture::small(w as u32, h as u32, &rgba);
            }
            SelectObject(dc, old);
            let _ = DeleteObject(bitmap.into());
        }
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        out
    }
}

/// What the desktop's loop last noted for the menu mismatch reports, to note only changes.
#[derive(Default)]
struct MismatchNoted {
    front: Option<Option<(i32, i32)>>,
    alt: Option<bool>,
    look: Option<(Option<Around>, bool)>,
    seen: Option<Seen>,
    open: Option<Option<Around>>,
    camera: Option<Option<bool>>,
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
/// The scroll wheel on the ring, with no key held: a hook that only keeps from the game what VRChat's wheel has no use for (the scroll wheel), and the next click
/// after it. Any movement of the mouse ends it, and the click is VRChat's again.
pub mod scroll {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT,
    };

    static SET: AtomicBool = AtomicBool::new(false);
    /// A scroll came, and no movement since: the next click is ours.
    static ENGAGED: AtomicBool = AtomicBool::new(false);
    static NOTCHES: AtomicI32 = AtomicI32::new(0);
    static MOVED: AtomicI32 = AtomicI32::new(0);
    static DOWNS: AtomicU32 = AtomicU32::new(0);
    static REST_X: AtomicI32 = AtomicI32::new(0);
    static REST_Y: AtomicI32 = AtomicI32::new(0);
    /// How far the mouse moves, in pixels from where VRChat holds it, before the ring gives the click back.
    const LEAVE: i32 = 24;

    unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 && SET.load(Ordering::Relaxed) {
            // SAFETY: for WH_MOUSE_LL, lparam is a live MSLLHOOKSTRUCT.
            let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            match wparam.0 as u32 {
                WM_MOUSEWHEEL => {
                    NOTCHES.fetch_add((info.mouseData >> 16) as u16 as i16 as i32, Ordering::Relaxed);
                    MOVED.store(0, Ordering::Relaxed);
                    ENGAGED.store(true, Ordering::Relaxed);
                    return LRESULT(1);
                }
                // Passed on always: VRChat's wheel follows it. Enough of it, and the click is VRChat's again.
                WM_MOUSEMOVE if ENGAGED.load(Ordering::Relaxed) => {
                    let off = (info.pt.x - REST_X.load(Ordering::Relaxed)).abs() + (info.pt.y - REST_Y.load(Ordering::Relaxed)).abs();
                    if MOVED.fetch_add(off, Ordering::Relaxed) + off > LEAVE {
                        ENGAGED.store(false, Ordering::Relaxed);
                    }
                }
                WM_LBUTTONDOWN if ENGAGED.load(Ordering::Relaxed) => {
                    DOWNS.fetch_add(1, Ordering::Relaxed);
                    return LRESULT(1);
                }
                WM_LBUTTONUP if ENGAGED.load(Ordering::Relaxed) => return LRESULT(1),
                _ => {}
            }
        }
        // SAFETY: passing on what the system passed.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    #[derive(Default)]
    pub struct Scroller {
        thread: Option<(u32, std::thread::JoinHandle<()>)>,
        /// The cell lit, clockwise from the top (0 Worlds in 3D), while engaged.
        lit: Option<usize>,
        taken_notches: i32,
        downs: u32,
    }

    impl Scroller {
        /// A turn: `wanted` says the wheel is open with no key held and no sheet.
        pub fn follow(&mut self, wanted: bool) {
            if !wanted {
                self.stop();
                return;
            }
            if self.thread.is_none() {
                self.begin();
            }
            if !ENGAGED.load(Ordering::Relaxed) {
                self.lit = None;
            }
            let notches = NOTCHES.load(Ordering::Relaxed);
            let steps = (notches - self.taken_notches) / 120;
            if steps != 0 {
                self.taken_notches += steps * 120;
                // Down goes clockwise, as a list goes down; the first notch lands on the first cell either way.
                let next = match self.lit {
                    None if steps < 0 => 0,
                    None => CELLS - 1,
                    Some(at) => (at as i32 - steps).rem_euclid(CELLS as i32) as usize,
                };
                self.lit = Some(next);
            }
        }

        pub fn lit(&self) -> Option<usize> {
            self.lit
        }

        /// The cell a click opened, once.
        pub fn take_click(&mut self) -> Option<usize> {
            let downs = DOWNS.load(Ordering::Relaxed);
            if downs == self.downs {
                return None;
            }
            self.downs = downs;
            eprintln!("orbit: a scroll's click on cell {:?}", self.lit); // i18n: not shown (a log's line)
            let cell = self.lit;
            // Done: the click is VRChat's again until the next scroll.
            ENGAGED.store(false, Ordering::Relaxed);
            self.lit = None;
            cell
        }

        fn begin(&mut self) {
            ENGAGED.store(false, Ordering::Relaxed);
            MOVED.store(0, Ordering::Relaxed);
            self.taken_notches = NOTCHES.load(Ordering::Relaxed);
            self.downs = DOWNS.load(Ordering::Relaxed);
            self.lit = None;
            let (x, y) = cursor_at();
            REST_X.store(x, Ordering::Relaxed);
            REST_Y.store(y, Ordering::Relaxed);
            let (tx, rx) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new().name("orbit-scroll-hook".into()).spawn(move || {
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
                eprintln!("orbit: the scroll wheel's hook ended"); // i18n: not shown (a log's line)
            });
            if let (Ok(handle), Ok(id)) = (spawned, rx.recv_timeout(Duration::from_secs(1))) {
                if id != 0 {
                    self.thread = Some((id, handle));
                }
            }
        }

        /// The hook lets everything through at once, and its thread ends on its own: nothing here waits for it (a sheet opened by a scroll's click once froze the
        /// panels on the turn that stopped it, 2026-11-08).
        pub fn stop(&mut self) {
            SET.store(false, Ordering::Relaxed);
            ENGAGED.store(false, Ordering::Relaxed);
            if let Some((id, handle)) = self.thread.take() {
                // SAFETY: a message to the hook's own thread.
                unsafe {
                    let _ = PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0));
                }
                drop(handle);
            }
            self.lit = None;
        }
    }

    impl Drop for Scroller {
        fn drop(&mut self) {
            self.stop();
        }
    }

    /// The ring's six cells.
    const CELLS: usize = 6;
}

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

    /// Round 17's judges saw an opaque black box round the wheel's caption: a layered window shown without its picture. A window made anew (a panel of another size)
    /// leaked its bitmap, still selected in its device context, until Windows refused pictures. Made and dropped many times, the process's GDI objects stay as they were.
    #[test]
    fn a_panel_window_made_again_and_again_leaks_nothing() {
        use windows::Win32::System::Threading::{GetCurrentProcess, GetGuiResources, GR_GDIOBJECTS};
        // SAFETY: a plain question about this process.
        let count = || unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) };
        drop(clear_window());
        let before = count();
        for _ in 0..200 {
            let mut window = clear_window();
            window.place(POINT { x: -20000, y: -20000 }, 0);
            window.hide();
        }
        let after = count();
        // Process-wide: tests on other threads make and free their own. A leak is one bitmap a window at least, 200 here.
        assert!(after < before + 60, "GDI objects {before} then {after}: a window leaks");
    }

    /// A stand-in for VRChat's window: a plain window that takes the mouse everywhere, under ours as VRChat's is.
    struct StandIn(HWND);

    impl StandIn {
        fn new(at: POINT, size: Vec2) -> Self {
            unsafe extern "system" fn plain(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
                // SAFETY: what the system passed on.
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            }
            let name = w!("OrbitersTestStandIn");
            // SAFETY: plain Win32 calls; the window is destroyed on drop.
            unsafe {
                let module = GetModuleHandleW(None).unwrap_or_default();
                let class = WNDCLASSW { lpfnWndProc: Some(plain), hInstance: module.into(), lpszClassName: name, ..Default::default() };
                RegisterClassW(&class);
                let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, name, w!(""), WS_POPUP, at.x, at.y, size.x as i32, size.y as i32, None, None, None, None)
                    .expect("a stand-in window");
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                Self(hwnd)
            }
        }
    }

    impl Drop for StandIn {
        fn drop(&mut self) {
            // SAFETY: made in `new`, destroyed once.
            let _ = unsafe { DestroyWindow(self.0) };
        }
    }

    /// Where the stand-in's client area goes: left of every screen, so that nobody sees these windows and no real window lies there.
    fn off_screen(client: Vec2) -> POINT {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN};
        // SAFETY: plain queries.
        let (left, top) = unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN)) };
        POINT { x: left - client.x as i32 - 3000, y: top }
    }

    /// The window that gets the mouse at `p` (screen pixels), by Windows' own hit testing: what a press there goes to.
    fn under(p: POINT) -> HWND {
        // SAFETY: a plain query.
        unsafe { windows::Win32::UI::WindowsAndMessaging::WindowFromPoint(p) }
    }

    /// The desktop's parts as [`run`] lays them, paints them and puts up their windows, over a stand-in for VRChat's window of `client` pixels with `open` up, turned
    /// until each part rests where it goes. `sheet` presses All tools on the wing first, and waits for its page.
    fn put_up(open: Around, client: Vec2, sheet: bool) -> (Panels, Windows, StandIn, Pos2) {
        let snap = crate::screenshot::idle_snapshot(&orbiters_core::settings::Settings::default(), &[]);
        let strip = snap.settings.orbit.layout == orbiters_core::settings::OrbitLayout::Strip;
        put_up_as(open, client, sheet, zones::layout_as(open, client, strip), snap)
    }

    /// The same with the parts in `places` and the service saying `snap`.
    fn put_up_as(open: Around, client: Vec2, sheet: bool, places: Vec<Placed>, snap: Snapshot) -> (Panels, Windows, StandIn, Pos2) {
        let mut panels = Panels::new(Arc::new(crate::screenshot::Still), 1.0, true);
        let mut windows = Windows::default();
        let corner = off_screen(client);
        let stand_in = StandIn::new(corner, client);
        let origin = pos2(corner.x as f32, corner.y as f32);
        panels.takeover_place = zones::place(Zone::Takeover, client);
        panels.back_place = zones::place(Zone::SheetBack, client);
        panels.hint_place = zones::place(Zone::SheetHint, client);
        let turn = |panels: &mut Panels, windows: &mut Windows| {
            pump();
            panels.set_open(Some(open), places.clone(), &snap);
            panels.follow(&snap, 0);
            panels.menu_turn(&snap);
            panels.step_springs(0.05);
            windows.show(panels, &snap, origin, Some(open));
        };
        let rests = |panels: &Panels| panels.all().filter(|p| p.shown).all(|p| p.painted && p.appear.settled()) && (!sheet || panels.takeover.as_ref().is_some_and(|p| p.painted));
        let until = |panels: &mut Panels, windows: &mut Windows, what: &str, done: &dyn Fn(&Panels) -> bool| {
            let began = Instant::now();
            loop {
                turn(panels, windows);
                if done(panels) {
                    break;
                }
                assert!(began.elapsed() < Duration::from_secs(20), "{what}: it never came");
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        until(&mut panels, &mut windows, "the menu's parts at rest", &|p: &Panels| p.all().filter(|p| p.shown).all(|p| p.painted && p.appear.settled()));
        if sheet {
            let (i, at) = panels
                .all()
                .enumerate()
                .find(|(_, p)| matches!(p.zone, Zone::WingAll | Zone::QmStrip))
                .map(|(i, p)| (i, p.to_logical([0.5, 0.5])))
                .expect("All tools beside the Quick Menu");
            panels.hover(Some((i, at)));
            turn(&mut panels, &mut windows);
            panels.press(&snap);
            turn(&mut panels, &mut windows);
            panels.release();
            panels.hover(None);
            until(&mut panels, &mut windows, "All tools' page", &rests);
            assert!(panels.sheet_open(), "All tools opened its page");
        }
        until(&mut panels, &mut windows, "everything at rest", &rests);
        (panels, windows, stand_in, origin)
    }

    /// What gets the mouse at each point of a grid over the game's window, every 6 pixels, and at its centre, where VRChat holds the pointer while its wheel is open:
    /// `None` VRChat (the stand-in), or the zone of ours. Any other window there fails.
    fn grid(windows: &Windows, stand_in: &StandIn, origin: Pos2, client: Vec2) -> Vec<(Pos2, Option<Zone>)> {
        let mut points: Vec<Pos2> = (0..(client.y as i32 / 6)).flat_map(|y| (0..(client.x as i32 / 6)).map(move |x| pos2(x as f32 * 6.0 + 3.0, y as f32 * 6.0 + 3.0))).collect();
        points.push(pos2((client.x / 2.0).floor(), (client.y / 2.0).floor()));
        points
            .into_iter()
            .map(|p| {
                let got = under(POINT { x: origin.x as i32 + p.x as i32, y: origin.y as i32 + p.y as i32 });
                if got == stand_in.0 {
                    return (p, None);
                }
                let zone = windows.list.iter().find(|(_, w)| w.hwnd == got).map(|(z, _)| *z);
                (p, Some(zone.unwrap_or_else(|| panic!("at {p:?} the mouse goes to a window that is neither ours nor the stand-in"))))
            })
            .collect()
    }

    /// Windows' hit testing as these tests ask it: a painted pixel of a window of ours that takes the mouse is ours, a clear one is the stand-in's.
    fn hit_testing_reads_the_pixels(origin: Pos2, stand_in: &StandIn) {
        let at = POINT { x: origin.x as i32 + 40, y: origin.y as i32 + 40 };
        let mut opaque = PanelWindow::new(8, 8, true).expect("a window");
        opaque.picture(&[255u8; 8 * 8 * 4], [0, 0, 8, 8]);
        opaque.place(at, 255);
        let mut clear = PanelWindow::new(8, 8, true).expect("a window");
        clear.picture(&[0u8; 8 * 8 * 4], [0, 0, 8, 8]);
        clear.place(POINT { x: at.x + 20, y: at.y }, 255);
        assert_eq!(under(POINT { x: at.x + 4, y: at.y + 4 }), opaque.hwnd, "a painted pixel of a window that takes the mouse takes it");
        assert_eq!(under(POINT { x: at.x + 24, y: at.y + 4 }), stand_in.0, "a clear pixel lets the mouse through");
        opaque.catch_mouse(false);
        assert_eq!(under(POINT { x: at.x + 4, y: at.y + 4 }), stand_in.0, "a painted pixel of a window that lets the mouse through lets it through");
        opaque.catch_mouse(true);
        assert_eq!(under(POINT { x: at.x + 4, y: at.y + 4 }), opaque.hwnd, "and takes it again");
    }

    /// Enzo, 2026-11-15, desktop mode: with VRChat's wheel open, a click on its buttons did nothing. VRChat holds Windows' pointer at its window's centre while the wheel
    /// is open (its slice follows the mouse's movement, the click goes where the pointer is held), and the ring's 3D pictures cell lies there at any window size: Windows
    /// gave that cell's window every press, and the cell, which only Left Alt's pointer presses, did nothing with it. With the wheel open no window of ours may take the
    /// mouse anywhere; the ring's cells are pressed through Left Alt's pointer and the number keys, as ever.
    #[test]
    fn with_vrchat_s_wheel_open_every_click_reaches_vrchat() {
        let _alone = super::super::flows::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        for client in [vec2(1280.0, 720.0), vec2(1920.0, 1080.0), vec2(1000.0, 1000.0)] {
            let (panels, windows, stand_in, origin) = put_up(Around::Wheel, client, false);
            hit_testing_reads_the_pixels(origin, &stand_in);
            // The ring is up, painted, in its windows: what this test is about is there to take the mouse.
            let cells: Vec<&Panel> = panels.all().filter(|p| p.shown && p.zone.ring().is_some() && p.zone.group().is_none()).collect();
            assert_eq!(cells.len(), 6, "{client:?}: the ring's six cells show");
            assert!(cells.iter().all(|p| p.pixels().chunks_exact(4).any(|px| px[3] > 0) && windows.list.get(&p.zone).is_some_and(|w| w.shown)), "{client:?}: each cell has its picture on screen");
            let seen = grid(&windows, &stand_in, origin, client);
            let ours: Vec<&(Pos2, Option<Zone>)> = seen.iter().filter(|(_, z)| z.is_some()).collect();
            let centre = seen.last().expect("the centre");
            eprintln!("wheel {client:?}: {} points, {} reach VRChat, {} ours (centre: {:?})", seen.len(), seen.len() - ours.len(), ours.len(), centre.1); // i18n: not shown (a test's measurement)
            assert_eq!(centre.1, None, "{client:?}: the centre of VRChat's window, where it holds the pointer with its wheel open, is VRChat's");
            assert!(ours.is_empty(), "{client:?}: with the wheel open {} points of the window go to ours, first {:?}", ours.len(), ours.first());
            // The ring's cells are still pressed where Left Alt's pointer is: the middle of each is on it.
            for cell in &cells {
                let ring = cell.zone.ring().expect("a cell");
                let middle = cell.placed.center + zones::polar((ring.inner + ring.outer) / 2.0, (ring.from + ring.to) / 2.0) * cell.placed.unit;
                let found = hit(&panels, middle).map(|(i, _)| panels.all().nth(i).expect("a part").zone);
                assert_eq!(found, Some(cell.zone), "{client:?}: Left Alt's pointer at the middle of {:?} is on it", cell.zone);
            }
        }
    }

    /// The same rule with VRChat's Quick Menu up and the mouse free, with the side tiles alone and with a page open beside them: a point goes to ours only on a part
    /// of ours that is pressed there, and every part's middle is ours (its buttons still work).
    #[test]
    fn with_the_quick_menu_open_only_our_parts_take_the_mouse() {
        let _alone = super::super::flows::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let client = vec2(1280.0, 720.0);
        for sheet in [false, true] {
            let (panels, windows, stand_in, origin) = put_up(Around::QuickMenu, client, sheet);
            hit_testing_reads_the_pixels(origin, &stand_in);
            let seen = grid(&windows, &stand_in, origin, client);
            let ours = seen.iter().filter(|(_, z)| z.is_some()).count();
            eprintln!("quick menu, page {sheet}: {} points, {} reach VRChat, {ours} ours", seen.len(), seen.len() - ours); // i18n: not shown (a test's measurement)
            for (p, zone) in &seen {
                if zone.is_some() {
                    assert!(hit(&panels, *p).is_some(), "page {sheet}: {p:?} goes to {zone:?}, where nothing of ours is pressed");
                }
            }
            assert!(ours > 0, "page {sheet}: our parts take the mouse");
            for panel in panels.all().filter(|p| p.shown && !p.zone.passes_clicks()) {
                let middle = panel.placed.rect.center();
                let got = under(POINT { x: origin.x as i32 + middle.x as i32, y: origin.y as i32 + middle.y as i32 });
                assert!(got != stand_in.0, "page {sheet}: the middle of {:?} is ours", panel.zone);
            }
        }
    }

    /// The menu mismatch corner (Settings › Beta feedback), as the real loop shows it over VRChat's window with the wheel open and with the Quick Menu and a page:
    /// only its rows take the mouse, it lies above every part of ours, the centre where VRChat holds the pointer with its wheel open stays VRChat's, and a click on
    /// each row's middle is that row.
    #[test]
    fn the_mismatch_corner_takes_only_its_rows_and_never_the_wheel_s_centre() {
        let _alone = super::super::flows::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        for (open, client) in [(Around::Wheel, vec2(1280.0, 720.0)), (Around::Wheel, vec2(1920.0, 1080.0)), (Around::QuickMenu, vec2(1280.0, 720.0)), (Around::QuickMenu, vec2(1000.0, 900.0))] {
            let (_panels, windows, stand_in, origin) = put_up(open, client, open == Around::QuickMenu);
            let mut corner = Corner::default();
            corner.show(true, origin, client, Instant::now());
            corner.raise();
            pump();
            let hwnd = corner.window.as_ref().expect("its window").hwnd; // i18n: not shown (a menu mismatch report's line)
            let (rect, zoom) = Corner::rect(client);
            let mut points: Vec<Pos2> = (0..(client.y as i32 / 6)).flat_map(|y| (0..(client.x as i32 / 6)).map(move |x| pos2(x as f32 * 6.0 + 3.0, y as f32 * 6.0 + 3.0))).collect();
            let centre = pos2((client.x / 2.0).floor(), (client.y / 2.0).floor());
            points.push(centre);
            let (mut on_corner, mut on_ours) = (0, 0);
            for p in points {
                let got = under(POINT { x: origin.x as i32 + p.x as i32, y: origin.y as i32 + p.y as i32 });
                if got == hwnd {
                    on_corner += 1;
                    // A row's edge is smoothed over a pixel or so.
                    let q = ((p - rect.min) / zoom).to_pos2();
                    let on_row = [0.0, -2.0, 2.0].iter().any(|dy| Corner::row_at(q + vec2(0.0, dy / zoom)).is_some() || Corner::row_at(q + vec2(dy / zoom, 0.0)).is_some());
                    assert!(rect.expand(2.0).contains(p) && on_row, "{open:?} {client:?}: {p:?} goes to the corner off its rows"); // i18n: not shown (a menu mismatch report's line)
                } else if got != stand_in.0 {
                    assert!(windows.list.values().any(|w| w.hwnd == got), "{open:?} {client:?}: {p:?} goes to a window neither ours nor VRChat's"); // i18n: not shown (a menu mismatch report's line)
                    assert!(open != Around::Wheel, "{open:?} {client:?}: with the wheel open {p:?} goes to a part of ours"); // i18n: not shown (a menu mismatch report's line)
                    on_ours += 1;
                }
            }
            eprintln!("mismatch corner, {open:?} {client:?}: {on_corner} points on its rows, {on_ours} on our parts"); // i18n: not shown (a test's measurement)
            assert!(on_corner > 0);
            assert_eq!(under(POINT { x: origin.x as i32 + centre.x as i32, y: origin.y as i32 + centre.y as i32 }), stand_in.0, "{open:?} {client:?}: the centre is VRChat's"); // i18n: not shown (a menu mismatch report's line)
            for kind in crate::mismatch::Kind::ALL {
                let i = crate::mismatch::Kind::ALL.iter().position(|k| *k == kind).expect("a kind") as f32;
                let middle = rect.min + vec2(Corner::SIZE.x / 2.0, i * (Corner::ROW + Corner::GAP) + Corner::ROW / 2.0) * zoom;
                let screen = (origin.x as i32 + middle.x as i32, origin.y as i32 + middle.y as i32);
                assert_eq!(under(POINT { x: screen.0, y: screen.1 }), hwnd, "{open:?} {client:?}: the middle of {kind:?} is the corner's, above everything of ours"); // i18n: not shown (a menu mismatch report's line)
                assert_eq!(corner.clicked(screen), Some(kind), "{open:?} {client:?}: a click there is {kind:?}"); // i18n: not shown (a menu mismatch report's line)
            }
        }
    }

    /// Where the corner lies: in the window, in its bottom part (VRChat's hints are in its top corners), on no part of any of our menus' layouts.
    #[test]
    fn the_mismatch_corner_lies_on_nothing_of_ours_nor_on_vrchat_s_hints() {
        for client in [vec2(1280.0, 720.0), vec2(1920.0, 1080.0), vec2(960.0, 540.0), vec2(1000.0, 900.0), vec2(1000.0, 1000.0), vec2(1280.0, 800.0), vec2(2560.0, 1080.0), vec2(3440.0, 1440.0), vec2(800.0, 600.0)] {
            let (rect, _) = Corner::rect(client);
            assert!(egui::Rect::from_min_size(pos2(0.0, 0.0), client).contains_rect(rect), "{client:?}: in the window"); // i18n: not shown (a menu mismatch report's line)
            assert!(rect.top() >= client.y * 0.3, "{client:?}: clear of VRChat's hints in the top corners"); // i18n: not shown (a menu mismatch report's line)
            let parts = [zones::layout_as(Around::QuickMenu, client, true), zones::layout(Around::QuickMenu, client), zones::layout(Around::Wheel, client)].concat();
            for part in parts.iter().chain(zones::place(Zone::SheetHint, client).iter()) {
                assert!(!part.rect.intersects(rect), "{client:?}: the corner {rect:?} lies on {:?} {:?}", part.zone, part.rect); // i18n: not shown (a menu mismatch report's line)
            }
        }
    }

    /// Enzo, 2026-11-15 (L-desk-camera): with VRChat's camera panel open on the desktop the mouse is free, as with the Quick Menu. N4N0's column at the panel's left
    /// takes a plain click on each of its tools; every other point of the window, the camera's picture and VRChat's own buttons under it above all, is VRChat's, also
    /// while the corners round the panel say a code is looked for (their window lets every click through).
    #[test]
    fn with_vrchat_s_camera_open_only_the_column_takes_the_mouse() {
        let _alone = super::super::flows::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        for (client, panel) in [
            (vec2(1280.0, 720.0), egui::Rect::from_min_max(pos2(332.0, 185.0), pos2(948.0, 535.0))),
            (vec2(1920.0, 1080.0), egui::Rect::from_min_max(pos2(498.0, 277.5), pos2(1422.0, 802.5))),
        ] {
            for looking in [false, true] {
                let mut snap = crate::screenshot::idle_snapshot(&orbiters_core::settings::Settings::default(), &[]);
                snap.steamvr.running = false;
                snap.camera_open_said = Some(true);
                if looking {
                    snap.camera_tools.qr = crate::controller::camera_tools::Qr::Looking;
                }
                let (panels, windows, stand_in, origin) = put_up_as(Around::Camera, client, false, zones::camera_on_screen(panel, client), snap);
                hit_testing_reads_the_pixels(origin, &stand_in);
                let column = [Zone::CameraPeople, Zone::CameraWorld, Zone::CameraRecord, Zone::CameraQr];
                for zone in column {
                    let part = panels.all().find(|p| p.zone == zone && p.shown).unwrap_or_else(|| panic!("{client:?}: no {zone:?}"));
                    let middle = part.placed.rect.center();
                    let got = under(POINT { x: origin.x as i32 + middle.x as i32, y: origin.y as i32 + middle.y as i32 });
                    assert_eq!(windows.list.get(&zone).map(|w| w.hwnd), Some(got), "{client:?}: a click on {zone:?} is not ours");
                    assert_eq!(hit(&panels, middle).map(|(i, _)| panels.all().nth(i).expect("a part").zone), Some(zone), "{client:?}: the pointer on {zone:?} is not on it");
                }
                assert_eq!(panels.all().any(|p| p.zone == Zone::CameraScan && p.shown), looking, "the corners show while a code is looked for, and only then");
                let seen = grid(&windows, &stand_in, origin, client);
                let ours = seen.iter().filter(|(_, z)| z.is_some()).count();
                eprintln!("camera {client:?}, looking {looking}: {} points, {} reach VRChat, {ours} ours", seen.len(), seen.len() - ours); // i18n: not shown (a test's measurement)
                let k = panel.height() / 350.0;
                let buttons = egui::Rect::from_min_max(pos2(panel.left() + 51.0 * k, panel.bottom()), pos2(panel.right() + 25.0 * k, panel.bottom() + 86.0 * k));
                for (p, zone) in &seen {
                    if zone.is_some() {
                        assert!(hit(&panels, *p).is_some(), "{client:?}: {p:?} goes to {zone:?}, where nothing of ours is pressed");
                        assert!(column.contains(&zone.unwrap()) || zone == &Some(Zone::CameraAct), "{client:?}: {p:?} goes to {zone:?}");
                    }
                    if panel.expand(2.0).contains(*p) || buttons.contains(*p) {
                        assert_eq!(*zone, None, "{client:?}: {p:?}, on VRChat's camera, goes to ours");
                    }
                }
                assert!(ours > 0);
            }
        }
    }

    /// The camera's column is the person's: a VRChat the bridge started (main's or a side session's, lent to the app or not) never gets it, as an agent clicks on
    /// that panel; and the setting turns it off.
    #[test]
    fn the_camera_column_is_only_for_the_person_s_own_vrchat() {
        let mut snap = crate::screenshot::idle_snapshot(&orbiters_core::settings::Settings::default(), &[]);
        snap.vrchat.pid = Some(4242);
        assert!(camera_tools_here(&snap));
        snap.bridge.driven = vec![17, 4242];
        assert!(!camera_tools_here(&snap), "a VRChat the bridge drives");
        snap.bridge.driven = vec![17];
        assert!(camera_tools_here(&snap), "the bridge drives another one beside it");
        snap.settings.orbit.camera_tiles = false;
        assert!(!camera_tools_here(&snap), "turned off in Settings");
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
