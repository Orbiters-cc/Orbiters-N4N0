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
    // The capture and its looks, off this thread: nothing here waits for the GPU.
    let looker = Looker::start();
    let mut camera_log = CameraLog::default();
    // Asked once a second at most: it is the shell's.
    let mut exclusive_said = (false, None::<Instant>);
    let mut keys = Keys::default();
    let mut hints = Hints::default();
    let mut pill = Pill::default();
    let mut pointer = pointer::Pointer::default();
    let mut scroller = scroll::Scroller::default();
    let mut said: Option<OrbitStatus> = None;
    let mut last_turn = Instant::now();
    let mut camera = detect::CameraWatch::default();
    let mut was_menu = false;
    // When VRChat's camera panel was last seen, as said to the service (it is part of what proves a live camera before a recording), at most twice a second.
    let mut panel_seen_ns: Option<u64> = None;
    let mut frames_seen = FrameRate::default();
    let clicks = Clicks::start();
    let mut corner = Corner::default();
    let mut noted = MismatchNoted::default();
    // The performance self-check (`perf`): this loop's cost, to the service every ten seconds.
    let mut meter = crate::perf::TurnMeter::new("orbit-desktop", Instant::now());
    // Translation on the desktop (REQUEST15 item 2): its reading thread while it is on, and its translations over VRChat's window.
    let mut translations = super::desk_translate::Host::default();
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
        if found.is_some() && exclusive_said.1.is_none_or(|t: Instant| t.elapsed() >= Duration::from_secs(1)) {
            let now_exclusive = full_screen_exclusive();
            if now_exclusive != exclusive_said.0 {
                super::super::vr_log::line(&format!("desk: exclusive full screen {now_exclusive}")); // i18n: not shown (a log's line)
            }
            exclusive_said = (now_exclusive, Some(Instant::now()));
        }
        // The shell says someone is in exclusive full screen, for the whole system: VRChat only when it is the one in front (a game on another screen is not).
        let exclusive = exclusive_said.0 && found.is_some_and(front_is);
        let status = OrbitStatus { note: exclusive.then(|| mark("VRChat is in exclusive full screen, which no other window can be above. Use a window or borderless full screen.").to_string()), steamvr_overrides: None, camera_note: None, camera_provisional: false, camera_panel_seen_ns: panel_seen_ns, camera_scale_measured: false };
        if said.as_ref() != Some(&status) {
            backend.orbit_status(status.clone());
            said = Some(status);
        }
        // As long as VRChat's window is on the screen, whoever has the keyboard (REQUEST15 item 1: they all went the moment another program had it). With
        // another program in front, ours lie right above VRChat's window, so whatever covers it covers them; its keys are not read.
        let presence = found.map_or(Presence::of(false, false, exclusive, false), |h| Presence::of(true, on_screen(h), exclusive, front_is(h)));
        let shown_on = found.filter(|_| presence.show);
        let keyboard = presence.keyboard;
        crate::mismatch::note_change("window: VRChat on screen, its size, its keyboard", &mut noted.front, shown_on.map(|w| { let s = client_size(w); (s.x as i32, s.y as i32, keyboard) })); // i18n: not shown (a menu mismatch report's line)
        if let Some(game) = shown_on {
            // Laid again, in order, when the layer changes or another window came between VRChat's and ours.
            let layer = (!keyboard).then_some(game);
            let changed = stack::set(layer);
            if changed || layer.is_some_and(|g| !stack::holds(g, &shown_windows(&windows, &pointer, &corner, &pill, &translations))) {
                // The translations lowest: they lie on VRChat's text, under anything of ours.
                translations.raise();
                windows.raise_all();
                pill.raise();
                pointer.raise();
                corner.raise();
            }
        }
        // Over a menu of VRChat's or of ours nothing is read and no translation shows (the menus cover the view): as last turn found them.
        let covered = was_menu || !panels.is_empty();
        translations.update(&snap, &backend, shown_on.map(|w| super::desk_translate::OnScreen { window: w, origin: client_origin(w), client: client_size(w), pid: snap.vrchat.pid.unwrap_or(0) }), covered);
        let Some(window) = shown_on else {
            corner.show(false, pos2(0.0, 0.0), vec2(0.0, 0.0), turn);
            // The parts of menus that closed are kept for a while; with the game behind another window nothing else comes by to let them go.
            panels.let_go(turn);
            pill.show(None, pos2(0.0, 0.0), vec2(0.0, 0.0));
            camera.reset();
            panel_seen_ns = None;
            looker.ask(None);
            let _ = looker.answers();
            camera_log.turn(turn.elapsed(), false, None, camera.take_went(), snap.camera_open_said, vec2(0.0, 0.0), false);
            if watcher.wants_frames() || !panels.is_empty() {
                watcher.reset();
                pointer.stop();
                panels.set_open(None, Vec::new(), &snap);
                windows.hide_all();
            }
            keys.reset();
            beat(&mut meter, &backend, turn);
            std::thread::sleep(REST);
            continue;
        };
        // The pointer over ours: VRChat's own shape in the accent chosen (REQUEST15 item 5), at the size of VRChat's screen's cursors.
        // SAFETY: a plain query; a window gone answers 0, read as 96.
        cursor::follow(super::panels::look::accent(), unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(window) });
        // VRChat's camera panel: for the person's own VRChat only, never one an agent drives through the bridge (its sessions' VRChats, the app's when the bridge
        // started it, one lent to the app for a picture): those are the agent's, and it clicks on that panel.
        let camera_wanted = camera_tools_here(&snap);
        if !camera_wanted {
            camera.reset();
        }
        crate::mismatch::note_change("input: Left Alt held", &mut noted.alt, alt_held()); // i18n: not shown (a menu mismatch report's line)
        crate::mismatch::note_change("vrchat: camera open (OSC)", &mut noted.camera, snap.camera_open_said); // i18n: not shown (a menu mismatch report's line)
        // VRChat's window of another size: its camera panel's place goes with it now, not two looks later.
        camera.follow_size(client_size(window));
        // Another program's keyboard: what is typed there is never a key of ours (R typed in a chat is not VRChat's R).
        if let Some(key) = if keyboard { keys.pressed() } else { keys.reset(); None } {
            crate::mismatch::note("input: key", || format!("{key:?}")); // i18n: not shown (a menu mismatch report's line)
            watcher.pressed(key, turn);
            if key == MenuKey::Esc {
                camera.esc(turn);
            }
        }
        let camera_frames = camera_wanted && camera.wants_frames(snap.camera_open_said, turn);
        // What the looks found since the last turn; then what they look for now, at the pace of each.
        for answer in looker.answers() {
            frames_seen.count(answer.at);
            camera_log.look(&answer);
            if let Some((found, coming)) = answer.menu {
                crate::mismatch::note_change("look: VRChat's window shows (menu, the wheel's rim coming)", &mut noted.look, (found, coming)); // i18n: not shown (a menu mismatch report's line)
                watcher.frame_coming(found, coming, answer.at);
            }
            if let Some(look) = answer.camera.filter(|_| camera_wanted) {
                camera.look(look, answer.at);
            }
        }
        let asked = (watcher.wants_frames() || camera_frames).then(|| Ask {
            window: window.0 as isize,
            pid: snap.vrchat.pid.unwrap_or(0),
            client: client_size(window),
            menus: watcher.wants_frames(),
            menu_every: watcher.look_every(turn),
            camera: camera_frames,
            shown: camera.shown(),
            open: snap.camera_open_said == Some(true),
        });
        looker.ask(asked);
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
        // VRChat's menus first: they close its camera on the desktop.
        let open = menu.or(panel.filter(|_| !menu_up).map(|_| Around::Camera));
        crate::mismatch::note_change("decide: ours follow", &mut noted.open, open); // i18n: not shown (a menu mismatch report's line)
        let size = client_size(window);
        let origin = client_origin(window);
        // The menu mismatch corner: shown, its clicks and keys, a report for a press. Never anything more.
        corner.show(mismatch_on, origin, size, turn);
        let mut asked = corner.keyed().filter(|_| mismatch_on && keyboard);
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
            // The screen's pixels over VRChat's window: only when nothing of another program lies on it (the corner can be pressed with another one in front),
            // else no picture at all (verify-r15 B6).
            let picture = if crate::mismatch::switches(&snap.settings).picture && !stack::covered(window, egui::Rect::from_min_size(origin, size)) { screen_picture(origin, size) } else { None };
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
            beat(&mut meter, &backend, turn);
            camera_log.turn(turn.elapsed(), false, panel, camera.take_went(), snap.camera_open_said, size, camera_frames);
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
        // Only while VRChat has the keyboard: the hook keeps the mouse from every program, and Alt held in another one is that one's.
        pointer.follow(keyboard && open == Some(Around::Wheel) && (alt_held() || sheet_on_wheel), origin, size, window, sheet_on_wheel);
        // With no key held, the scroll wheel picks a cell (VRChat's wheel has no use for it), and the next click opens it.
        scroller.follow(SCROLL_TO_PICK && keyboard && open == Some(Around::Wheel) && !pointer.active() && !panels.sheet_open());
        let scrolled = scroller.lit();
        // Over VRChat's wheel while Alt points at ours, or while a 3D picture waits. VRChat lights the slice its own pointer last reached, and nothing from here can
        // unlight it (the movement is held from it, not undone): two things looked chosen (round 14's judges). While Alt is held its clicks are ours, so its wheel is
        // shown stepped back, still in view (round 13's judges: an opaque disc hid it all); while a 3D picture waits, an opaque disc says what waits.
        let _ = scrolled;
        panels.set_pointing((pointer.active() || panels.picture_waiting()) && !panels.sheet_open(), !snap.settings.reduce_motion);
        panels.set_key_caps(keyboard && alt_held());
        let at = pointer.at().unwrap_or(pos2(cursor.0 as f32, cursor.1 as f32));
        let on = match scrolled {
            Some(cell) => panels.cell_point(cell),
            None => (open != Some(Around::Wheel) || pointer.at().is_some()).then(|| hit(&panels, at - origin.to_vec2())).flatten(),
        };
        // With another program in front, a part of ours it covers is not pointed at through it: only where Windows' pointer is over ours or VRChat's own window.
        let on = on.filter(|_| keyboard || pointer.at().is_some() || seen_at(cursor, window));
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
        if let Some(cell) = keys.digit(keyboard && open == Some(Around::Wheel) && !panels.sheet_open()) {
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
        let painted = windows.painted;
        if windows.show(&mut panels, &snap, origin, open) {
            pointer.raise();
            corner.raise();
        }
        beat(&mut meter, &backend, turn);
        camera_log.turn(turn.elapsed(), windows.painted != painted, panel, camera.take_went(), snap.camera_open_said, size, camera_frames);
        let wait = TURN.saturating_sub(turn.elapsed());
        std::thread::sleep(wait);
    }
}

/// A turn of the loop ended (its wait not counted): the self-check notes it, and sends its ten seconds when they are up.
fn beat(meter: &mut crate::perf::TurnMeter, backend: &Ctl, turn: Instant) {
    meter.turn(turn.elapsed());
    if let Some(turns) = meter.take(Instant::now()) {
        backend.perf_turns(turns);
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

/// Whether the screen's point `at` shows ours or VRChat's window, nothing that covers them: Windows' own hit testing.
fn seen_at(at: (i32, i32), game: HWND) -> bool {
    // SAFETY: a plain query.
    let under = unsafe { windows::Win32::UI::WindowsAndMessaging::WindowFromPoint(POINT { x: at.0, y: at.1 }) };
    under == game || stack::ours(under)
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

/// Whether the keyboard is VRChat's: its window in front, or one of ours (which never take the focus, but a click could leave one there).
fn front_is(game: HWND) -> bool {
    // SAFETY: a plain query.
    let front = unsafe { GetForegroundWindow() };
    front == game || stack::ours(front)
}

/// Every window of ours that shows: what must lie right above VRChat's while another program has the keyboard.
fn shown_windows(windows: &Windows, pointer: &pointer::Pointer, corner: &Corner, pill: &Pill, translations: &super::desk_translate::Host) -> Vec<HWND> {
    let mut all: Vec<HWND> = windows.list.values().filter(|w| w.shown).map(|w| w.hwnd).chain(translations.windows().map(|w| w.hwnd)).collect();
    all.extend([pointer.window(), corner.window.as_ref(), pill.window.as_ref()].into_iter().flatten().filter(|w| w.shown).map(|w| w.hwnd));
    all
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
            // SAFETY: plain queries; a handle gone only says no. Still VRChat's: Windows may give a closed window's handle to another (verify-r15).
            if unsafe { IsWindow(Some(hwnd)).as_bool() && !IsIconic(hwnd).as_bool() } && is_vrchat_window(hwnd, pid) {
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

/// Whether `hwnd` is VRChat's main window: of process `pid`, of Unity's window class. Asked again of a handle kept, and before a capture of it starts: a
/// window that closed may have its handle given to another program's (verify-r15).
pub(super) fn is_vrchat_window(hwnd: HWND, pid: u32) -> bool {
    if hwnd.is_invalid() || pid == 0 {
        return false;
    }
    let mut owner = 0u32;
    let mut name = [0u16; 32];
    // SAFETY: plain queries about a window; the buffer outlives the call; a window gone answers 0.
    let n = unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }
        GetWindowThreadProcessId(hwnd, Some(&mut owner));
        GetClassNameW(hwnd, &mut name)
    };
    owner == pid && String::from_utf16_lossy(&name[..n.max(0) as usize]) == UNITY_CLASS
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
    camera: Option<detect::CameraLook>,
}

/// The Direct3D device captures use, kept for as long as the process lasts: it holds nothing and does nothing between captures.
#[derive(Clone)]
pub(super) struct Gpu {
    pub(super) device: ID3D11Device,
    pub(super) context: ID3D11DeviceContext,
    pub(super) winrt: IDirect3DDevice,
}

impl Gpu {
    pub(super) fn new() -> Result<Self, String> {
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

/// The box (left, top, right, bottom) of a part at `[x0, y0, width, height]` of the client area, which lies at `offset` in the frame,
/// when the whole of it is inside a frame of `frame` pixels; `None` otherwise (and on an overflow).
pub(super) fn source_box([x0, y0, width, height]: [u32; 4], offset: (u32, u32), frame: (u32, u32)) -> Option<[u32; 4]> {
    let left = x0.checked_add(offset.0)?;
    let top = y0.checked_add(offset.1)?;
    let right = left.checked_add(width)?;
    let bottom = top.checked_add(height)?;
    (width > 0 && height > 0 && right <= frame.0 && bottom <= frame.1).then_some([left, top, right, bottom])
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
    /// `shown`: where the camera's panel shows now, and `open`: VRChat says its camera is open ([`detect::camera_look`]).
    fn look(&mut self, window: HWND, client: Vec2, menus: bool, camera: bool, shown: Option<egui::Rect>, open: bool) -> Option<Looked> {
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
        // What the copies may read: the frame's texture, and of it what this frame holds (REL-3).
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: a plain read of the texture's description into a local.
        unsafe { texture.GetDesc(&mut desc) };
        let frame_size = (desc.Width.min(content.Width.max(0) as u32), desc.Height.min(content.Height.max(0) as u32));
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
            // The client's size was measured a moment ago, the frame may still be of the window's older, smaller size: a box
            // outside the frame is undefined in Direct3D 11 (an access violation in d3d11.dll was seen, REL-3). Then no look
            // this frame; the next frame is of the new size.
            let [left, top, right, bottom] = source_box([x0, y0, width, height], (dx, dy), frame_size)?;
            let staging = self.staging_for(i, width, height)?;
            let region = D3D11_BOX { left, top, front: 0, right, bottom, back: 1 };
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            // SAFETY: both textures are this device's and of one format; the box is inside the frame (`source_box`), and the mapping is read
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
            Some(detect::camera_look(&lines?, client, shown, open))
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

// --------------------------------------------------------------------------------------------- the looks, on a thread of their own

/// What the loop asks of the looks now: which window, which of VRChat's things to look for, and at what pace.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ask {
    window: isize,
    /// VRChat's process: the window is asked again to be its own before its capture starts.
    pid: u32,
    client: Vec2,
    menus: bool,
    menu_every: Duration,
    camera: bool,
    /// Where the camera's panel shows, and whether VRChat says its camera is open.
    shown: Option<egui::Rect>,
    open: bool,
}

/// What a look found, and when its frame was read.
struct Answer {
    at: Instant,
    menu: Option<(Option<Around>, bool)>,
    camera: Option<detect::CameraLook>,
    /// How long the look took, reading the frame's parts back from the GPU included.
    took: Duration,
}

/// Windows Graphics Capture and the looks at its frames, on a thread of their own (L-deskcam-glitch, 2026-11-15). They ran in the turn that also moves, paints and
/// presses the panels: each look copies its parts out of the GPU and waits for that copy (`Map` right after `CopySubresourceRegion` makes the CPU wait for the GPU to
/// reach the copy, behind whatever VRChat has queued: in full screen at 2560 x 1440 the GPU is busiest), once for each part, and with VRChat's camera open there are
/// nine parts every 30 ms. Every such wait was a turn in which the column neither moved, nor answered the mouse, nor painted: the column ran at the pace of the
/// GPU's queue, not of the screen. Now the loop only says what it wants ([`Looker::ask`]) and takes what was found ([`Looker::answers`]); it never waits for the GPU.
struct Looker {
    asked: Arc<std::sync::Mutex<Option<Ask>>>,
    answers: std::sync::mpsc::Receiver<Answer>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Looker {
    fn start() -> Self {
        let asked: Arc<std::sync::Mutex<Option<Ask>>> = Arc::default();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, answers) = std::sync::mpsc::channel();
        let (wanted, stopping) = (asked.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("orbit-looks".into())
            .spawn(move || {
                // SAFETY: this thread's own WinRT apartment, for the capture made on it.
                unsafe {
                    let _ = RoInitialize(RO_INIT_MULTITHREADED);
                }
                let mut gpu: Option<Gpu> = None;
                let mut capture: Option<(isize, Capture)> = None;
                let (mut last_menu, mut last_camera) = (None::<Instant>, None::<Instant>);
                while !stopping.load(std::sync::atomic::Ordering::Relaxed) {
                    let Some(ask) = *wanted.lock().unwrap() else {
                        // Nothing to look for: no capture at all.
                        capture = None;
                        std::thread::sleep(Duration::from_millis(15));
                        continue;
                    };
                    if capture.as_ref().is_none_or(|(w, _)| *w != ask.window) {
                        // The old window's capture ends before the new one starts.
                        drop(capture.take());
                        if gpu.is_none() {
                            gpu = Gpu::new().map_err(|e| eprintln!("capture: {e}")).ok();
                        }
                        let window = HWND(ask.window as *mut _);
                        capture = gpu.as_ref().filter(|_| is_vrchat_window(window, ask.pid)).and_then(|g| Capture::new(window, g).map_err(|e| eprintln!("capture: {e}")).ok()).map(|c| (ask.window, c));
                        // A device lost (a driver's update, say) is made again next time.
                        if capture.is_none() {
                            gpu = None;
                            std::thread::sleep(Duration::from_millis(100));
                            continue;
                        }
                    }
                    let now = Instant::now();
                    let menus = ask.menus && last_menu.is_none_or(|t| now.duration_since(t) >= ask.menu_every);
                    let camera = ask.camera && last_camera.is_none_or(|t| now.duration_since(t) >= detect::CAMERA_LOOK_EVERY);
                    if !menus && !camera {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    let Some((window, c)) = capture.as_mut() else { continue };
                    let began = Instant::now();
                    let looked = c.look(HWND(*window as *mut _), ask.client, menus, camera, ask.shown, ask.open);
                    crate::perf::elsewhere(crate::perf::Part::Capture, began.elapsed());
                    let Some(looked) = looked else {
                        // No new frame yet.
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    };
                    if menus {
                        last_menu = Some(now);
                    }
                    if camera {
                        last_camera = Some(now);
                    }
                    if tx.send(Answer { at: now, menu: looked.menu, camera: looked.camera, took: now.elapsed() }).is_err() {
                        break;
                    }
                }
            })
            .ok();
        Self { asked, answers, stop, thread }
    }

    /// What to look for from now on; `None`: nothing, and the capture goes.
    fn ask(&self, ask: Option<Ask>) {
        let mut asked = self.asked.lock().unwrap();
        if *asked != ask {
            *asked = ask;
        }
    }

    /// What was found since the last call, in order.
    fn answers(&self) -> Vec<Answer> {
        self.answers.try_iter().collect()
    }
}

impl Drop for Looker {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the column and the looks did, said in `vr-test.log` every 10 s while VRChat's camera is looked for, and each time the column comes or goes, with why: what
/// L-deskcam-glitch could not be told from (2026-11-15: the desktop's process said nothing anywhere).
#[derive(Default)]
struct CameraLog {
    since: Option<Instant>,
    looks: u32,
    look_ms: f32,
    slowest_look_ms: f32,
    there: u32,
    gone: u32,
    unclear: u32,
    turns: u32,
    slowest_turn_ms: f32,
    paints: u32,
    was: Option<egui::Rect>,
}

impl CameraLog {
    fn look(&mut self, answer: &Answer) {
        if let Some(look) = answer.camera {
            let ms = answer.took.as_secs_f32() * 1000.0;
            self.looks += 1;
            self.look_ms += ms;
            self.slowest_look_ms = self.slowest_look_ms.max(ms);
            match look.at_shown {
                detect::Rim::There => self.there += 1,
                detect::Rim::Gone => self.gone += 1,
                detect::Rim::Unclear => self.unclear += 1,
            }
        }
    }

    /// A turn of the loop, how long it took and whether it painted a part; `shown` where the column stands, `went` why it went, `said` VRChat's word over OSC.
    #[allow(clippy::too_many_arguments)]
    fn turn(&mut self, took: Duration, painted: bool, shown: Option<egui::Rect>, went: Option<detect::CameraGone>, said: Option<bool>, client: Vec2, looking: bool) {
        if shown != self.was {
            match (self.was, shown) {
                (None, Some(at)) => super::super::vr_log::line(&format!("desk camera: column shown at {at:?} in {}x{} (osc {said:?})", client.x, client.y)), // i18n: not shown (a log's line)
                (Some(_), None) => super::super::vr_log::line(&format!("desk camera: column gone ({went:?}, osc {said:?}) in {}x{}", client.x, client.y)), // i18n: not shown (a log's line)
                (Some(_), Some(at)) => super::super::vr_log::line(&format!("desk camera: column moved to {at:?} in {}x{}", client.x, client.y)), // i18n: not shown (a log's line)
                (None, None) => {}
            }
            self.was = shown;
        }
        if !looking && shown.is_none() {
            self.since = None;
            return;
        }
        let ms = took.as_secs_f32() * 1000.0;
        self.turns += 1;
        self.slowest_turn_ms = self.slowest_turn_ms.max(ms);
        self.paints += u32::from(painted);
        let since = *self.since.get_or_insert_with(Instant::now);
        if since.elapsed() >= Duration::from_secs(10) {
            let secs = since.elapsed().as_secs_f32();
            super::super::vr_log::line(&format!(
                "desk camera: {:.0} looks/s, {:.1} ms each (slowest {:.1}); rim there {} gone {} unclear {}; {:.0} turns/s, slowest {:.1} ms, {} with a paint; shown {}", // i18n: not shown (a log's line)
                self.looks as f32 / secs,
                self.look_ms / self.looks.max(1) as f32,
                self.slowest_look_ms,
                self.there,
                self.gone,
                self.unclear,
                self.turns as f32 / secs,
                self.slowest_turn_ms,
                self.paints,
                shown.is_some()
            ));
            *self = Self { was: self.was, since: Some(Instant::now()), ..Self::default() };
        }
    }
}

/// Where the client area's corner is in the frames of Windows Graphics Capture (the window's visual bounds).
pub(super) fn client_in_frame(window: HWND) -> (u32, u32) {
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

/// What this thread's panel windows heard since the last call: a button going down or up, and which window
/// ([`PanelWindow::id`]).
pub(crate) fn take_presses() -> Vec<(bool, isize)> {
    MOUSE.with(|m| std::mem::take(&mut *m.borrow_mut()))
}

/// The panels' window procedure: takes the mouse's button, and never the focus. That it answers at all is what keeps the click from the game.
/// The mouse wheel's movement since last read, in Windows' units (120 a notch): from our windows' messages, or the pointer's hook.
static WHEEL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

unsafe extern "system" fn procedure(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // Over ours, the pointer VRChat shows on the desktop, in the accent chosen (REQUEST15 item 5); Windows' arrow until it is made.
        windows::Win32::UI::WindowsAndMessaging::WM_SETCURSOR if (lparam.0 & 0xffff) as u32 == windows::Win32::UI::WindowsAndMessaging::HTCLIENT && cursor::current().is_some() => {
            // SAFETY: a cursor made by `cursor::follow`, never destroyed while the process lives.
            unsafe { windows::Win32::UI::WindowsAndMessaging::SetCursor(cursor::current()) };
            LRESULT(1)
        }
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
/// (Also the window process's card "Orbiters - N4N0 will minimize to save memory", `crate::window_rest::card`.)
pub(crate) struct PanelWindow {
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
    /// What tells its presses from other windows' ([`take_presses`]).
    pub(crate) fn id(&self) -> isize {
        self.hwnd.0 as isize
    }

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

    pub(crate) fn new(width: u32, height: u32, clicks: bool) -> anyhow::Result<Self> {
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
    pub(crate) fn picture(&mut self, rgba: &[u8], changed: [u32; 4]) {
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

    /// Puts the window where it goes, at this alpha, and shows it: above everything while VRChat has the keyboard (VRChat's a window among others, and what it has
    /// it may come above again), right above VRChat's window while another program has it ([`stack`]).
    pub(crate) fn place(&mut self, at: POINT, alpha: u8) {
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
        let origin = POINT { x: 0, y: 0 };
        // SAFETY: live handles; the structures outlive the call.
        crate::perf::uploaded(u64::from(self.size.cx.max(0) as u32) * u64::from(self.size.cy.max(0) as u32));
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
                stack::lay(self.hwnd);
            }
        }
        self.shown = true;
        self.at = at;
        self.alpha = alpha;
        self.stale = false;
    }

    /// [`PanelWindow::place`] only when something changed: new pixels, not shown, moved or another alpha. A translation at rest is not given to Windows
    /// again every turn (verify-r15). Says whether it was.
    pub(crate) fn place_if_changed(&mut self, at: POINT, alpha: u8) -> bool {
        if self.stale || !self.shown || self.at != at || self.alpha != alpha {
            self.place(at, alpha);
            return true;
        }
        false
    }

    /// Its picture is on the screen: Windows took the last one and the window is visible (`crate::window_rest::card`).
    pub(crate) fn presented(&self) -> bool {
        // SAFETY: a plain query on a live window.
        self.shown && !self.stale && unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(self.hwnd) }.as_bool()
    }

    /// Above every other window of ours, in the layer this thread's windows lie in now ([`stack`]).
    pub(crate) fn raise(&self) {
        stack::lay(self.hwnd);
    }

    /// Its size in pixels.
    pub(crate) fn size(&self) -> [u32; 2] {
        [self.size.cx.max(0) as u32, self.size.cy.max(0) as u32]
    }

    pub(crate) fn hide(&mut self) {
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
    /// Pictures put in windows so far: what the log counts.
    painted: u64,
    /// The zones showing, in the order they lie, bottom first.
    order: Vec<Zone>,
}

impl Windows {
    fn hide_all(&mut self) {
        for w in self.list.values_mut() {
            w.hide();
        }
    }

    /// Every one showing laid again in the order they lie, bottom first: where ours lie changed ([`stack`]).
    fn raise_all(&self) {
        for zone in &self.order {
            if let Some(w) = self.list.get(zone).filter(|w| w.shown) {
                w.raise();
            }
        }
    }

    /// New pictures, then every panel where its spring has it, taking the mouse or not as [`catches_mouse`] says for the menu `open`. Says whether one came into view,
    /// which then lies over all others.
    fn show(&mut self, panels: &mut Panels, snap: &Snapshot, origin: Pos2, open: Option<Around>) -> bool {
        let list = &mut self.list;
        let painted = &mut self.painted;
        panels.paint(snap, |panel, changed| {
            *painted += 1;
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
        self.order = showing.iter().map(|(z, _, _)| *z).collect();
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

// --------------------------------------------------------------------------------------------- the pointer over ours

/// The pointer over ours on the desktop (REQUEST15 item 5): VRChat draws its own pointer on the desktop, a small slanted arrowhead with a notch at its back, and
/// hides Windows'; over a window of ours Windows' white arrow showed instead, from another program. This is that shape (drawn here, not VRChat's picture), of
/// the same size, in the accent the person chose, with a thin dark edge so that it reads on anything.
pub(crate) mod cursor {
    use super::*;
    use egui::Color32;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::atomic::AtomicU64;
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyCursor, HCURSOR, ICONINFO, SM_CXCURSOR};

    /// The cursor made for the accent and size now, and that accent and size (0xRRGGBB << 16 | side; all ones: none yet).
    static NOW: AtomicIsize = AtomicIsize::new(0);
    static KEY: AtomicU64 = AtomicU64::new(u64::MAX);

    /// Windows' cursor size, pixels, at `dpi` (96: 100 %).
    pub(crate) fn side_for(dpi: u32) -> u32 {
        // SAFETY: a plain query.
        let side = unsafe { windows::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXCURSOR, dpi.max(96)) };
        (side.max(0) as u32).clamp(32, 256)
    }

    /// The arrow in a box of 24 units: its tip at the top left (the hotspot), its two wings, the notch between them at its back. Two triangles, as it is not
    /// convex.
    pub(crate) const TIP: Pos2 = pos2(2.0, 2.0);
    const WING_RIGHT: Pos2 = pos2(19.5, 9.0);
    const NOTCH: Pos2 = pos2(10.6, 10.6);
    const WING_DOWN: Pos2 = pos2(9.0, 19.5);

    /// The arrow in a square of `px` pixels at `scale` (1.0: the 24 units' box), RGBA premultiplied.
    pub(crate) fn picture(px: u32, scale: f32, colour: Color32) -> Vec<u8> {
        let mut canvas = crate::overlay::Canvas::new(px, px);
        canvas.paint(0.0, |p| {
            let at = |q: Pos2| pos2(q.x * scale, q.y * scale);
            let outline = vec![at(TIP), at(WING_RIGHT), at(NOTCH), at(WING_DOWN)];
            // The dark edge first, half of it outside the arrow, then the arrow over it.
            p.add(egui::Shape::closed_line(outline, egui::Stroke::new(2.4 * scale, Color32::from_black_alpha(200))));
            for tri in [[TIP, WING_RIGHT, NOTCH], [TIP, NOTCH, WING_DOWN]] {
                p.add(egui::Shape::convex_polygon(tri.iter().map(|q| at(*q)).collect(), colour, egui::Stroke::NONE));
            }
        });
        canvas.pixels().to_vec()
    }

    /// The cursor for the accent now, when one was made.
    pub(crate) fn current() -> Option<HCURSOR> {
        let h = NOW.load(Ordering::Relaxed);
        (h != 0).then_some(HCURSOR(h as *mut _))
    }

    /// A turn: a cursor of the accent `colour` at the size Windows gives cursors at `dpi` (VRChat's screen's) once, and again only when either changes. The one
    /// replaced is destroyed (verify-r15: each change leaked one); a pointer over ours shows the new one first, so the old one is never destroyed while shown.
    pub(crate) fn follow(colour: Color32, dpi: u32) {
        let side = side_for(dpi);
        let key = (u64::from(u32::from_be_bytes([0, colour.r(), colour.g(), colour.b()])) << 16) | u64::from(side);
        if KEY.load(Ordering::Relaxed) == key {
            return;
        }
        KEY.store(key, Ordering::Relaxed);
        let Some(made) = make(side, colour) else { return };
        let old = NOW.swap(made.0 as isize, Ordering::Relaxed);
        if old != 0 {
            let mut p = POINT::default();
            // SAFETY: plain calls; `old` was made by `make` and is this process's alone; it is no longer handed out (NOW holds the new one).
            unsafe {
                let _ = GetCursorPos(&mut p);
                if stack::ours(windows::Win32::UI::WindowsAndMessaging::WindowFromPoint(p)) {
                    windows::Win32::UI::WindowsAndMessaging::SetCursor(Some(made));
                }
                let _ = DestroyCursor(HCURSOR(old as *mut _));
            }
        }
    }

    /// A cursor of `side` pixels: the arrow about as large as VRChat's on a screen (21 pixels from its tip at Windows' usual 32), its hotspot on the tip.
    pub(crate) fn make(side: u32, colour: Color32) -> Option<HCURSOR> {
        let scale = side as f32 / 28.0;
        let premultiplied = picture(side, scale, colour);
        // A cursor's colours are straight, not premultiplied, in BGRA rows from the top.
        let mut bgra = Vec::with_capacity(premultiplied.len());
        for p in premultiplied.chunks_exact(4) {
            let a = p[3];
            let un = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
            bgra.extend_from_slice(&[un(p[2]), un(p[1]), un(p[0]), a]);
        }
        // SAFETY: plain GDI calls; the bitmaps made here are deleted before returning (the cursor keeps its own copies).
        unsafe {
            let header = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: side as i32, biHeight: -(side as i32), biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() };
            let info = BITMAPINFO { bmiHeader: header, ..Default::default() };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let colour_bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());
            let mask_bits = vec![0u8; (side as usize).div_ceil(16) * 2 * side as usize];
            let mask = windows::Win32::Graphics::Gdi::CreateBitmap(side as i32, side as i32, 1, 1, Some(mask_bits.as_ptr() as *const _));
            let hot = (TIP.x * scale).round() as u32;
            let icon = ICONINFO { fIcon: false.into(), xHotspot: hot, yHotspot: hot, hbmMask: mask, hbmColor: colour_bitmap };
            let made = CreateIconIndirect(&icon).ok();
            let _ = DeleteObject(colour_bitmap.into());
            let _ = DeleteObject(mask.into());
            made.map(|i| HCURSOR(i.0))
        }
    }
}

// --------------------------------------------------------------------------------------------- where ours lie among other programs' windows

/// Where the windows of ours lie among the other programs' (REQUEST15 item 1). While VRChat has the keyboard they are topmost, as they always were (VRChat brought
/// in front again never lands on them). While another program has it, VRChat's window is still on the screen beside or under it, and ours stay with it: no longer
/// topmost, right above VRChat's window, so that whatever covers VRChat covers them too, as if they were part of it. They never take the focus either way.
pub(crate) mod stack {
    use super::*;
    use std::cell::Cell;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindow, GetWindowLongPtrW, IsWindowVisible, GWL_EXSTYLE, GW_HWNDPREV, HWND_NOTOPMOST, HWND_TOP, SWP_NOMOVE};

    thread_local! {
        /// The window ours lie right above, or 0: topmost. Each thread's windows by their own (a test's windows are its thread's).
        static ABOVE: Cell<isize> = const { Cell::new(0) };
    }

    /// How ours lie from now on: right above `game`, or topmost with `None`. Says whether that changed (then every window of ours showing is laid again, in order).
    pub(crate) fn set(game: Option<HWND>) -> bool {
        let new = game.map_or(0, |h| h.0 as isize);
        ABOVE.with(|a| a.replace(new)) != new
    }

    /// Where they lie now: right above that window, or topmost.
    pub(crate) fn above() -> Option<HWND> {
        let a = ABOVE.with(Cell::get);
        (a != 0).then_some(HWND(a as *mut _))
    }

    fn flags() -> windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS {
        SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOOWNERZORDER
    }

    pub(super) fn topmost(hwnd: HWND) -> bool {
        // SAFETY: a plain query; a window gone answers 0.
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        style & WS_EX_TOPMOST.0 as isize != 0
    }

    /// A window of ours: one of this process's panel windows (the class every one of them has). Any other window of this process (a test's stand-in) is not.
    pub(crate) fn ours(hwnd: HWND) -> bool {
        if hwnd.is_invalid() {
            return false;
        }
        let mut pid = 0u32;
        let mut name = [0u16; 32];
        // SAFETY: plain queries about a window; the buffer outlives the call.
        let n = unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            GetClassNameW(hwnd, &mut name)
        };
        pid == std::process::id() && String::from_utf16_lossy(&name[..n.max(0) as usize]) == "OrbitersOrbitPanel"
    }

    /// The first window above `game` in Windows' order that shows and is not ours: what lies over VRChat's window, which ours go right under. `None`: nothing but ours.
    pub(crate) fn over(game: HWND) -> Option<HWND> {
        // SAFETY: plain queries walking Windows' order; a window gone ends the walk.
        let mut at = unsafe { GetWindow(game, GW_HWNDPREV) }.ok();
        // Bounded: Windows' order can change under the walk.
        for _ in 0..4096 {
            let h = at.filter(|h| !h.is_invalid())?;
            // SAFETY: a plain query.
            if unsafe { IsWindowVisible(h) }.as_bool() && !ours(h) {
                return Some(h);
            }
            // SAFETY: as above.
            at = unsafe { GetWindow(h, GW_HWNDPREV) }.ok();
        }
        None
    }

    /// Puts `hwnd` on top of ours, in the layer they lie in now: topmost; or right above VRChat's window, under whatever lies over it.
    pub(crate) fn lay(hwnd: HWND) {
        let flags = flags();
        // SAFETY: plain calls on live windows; a window gone only fails.
        unsafe {
            match above() {
                // VRChat itself topmost (verify-r15): ours in its band, right under what covers it there, else on top of that band.
                Some(game) if topmost(game) => {
                    let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, flags);
                    if let Some(cover) = over(game) {
                        let _ = SetWindowPos(hwnd, Some(cover), 0, 0, 0, 0, flags);
                    }
                }
                Some(game) => match over(game) {
                    // Right under what covers VRChat (a topmost window placed after an ordinary one is no longer topmost: Windows' own rule).
                    Some(cover) if !topmost(cover) => {
                        let _ = SetWindowPos(hwnd, Some(cover), 0, 0, 0, 0, flags);
                    }
                    // Nothing ordinary over VRChat: the top of the ordinary windows, which is right above it. Under every topmost one (the taskbar, a pinned player).
                    _ => {
                        if topmost(hwnd) {
                            let _ = SetWindowPos(hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, flags);
                        }
                        let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, flags);
                    }
                },
                None => {
                    let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, flags);
                }
            }
        }
    }

    /// Whether a window of another program lies on any part of `client` (screen pixels) of `game`: one that shows, above it in Windows' order, not ours. A
    /// picture of the screen there would hold that program's pixels.
    pub(crate) fn covered(game: HWND, client: egui::Rect) -> bool {
        use windows::Win32::Graphics::Dwm::DWMWA_CLOAKED;
        // SAFETY: plain queries walking Windows' order.
        let mut at = unsafe { GetWindow(game, GW_HWNDPREV) }.ok();
        for _ in 0..4096 {
            let Some(h) = at.filter(|h| !h.is_invalid()) else { return false };
            let mut cloaked = 0u32;
            let mut r = RECT::default();
            // SAFETY: plain queries; the outputs and their sizes are passed.
            let (visible, known) = unsafe {
                let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, std::mem::size_of::<u32>() as u32);
                (IsWindowVisible(h).as_bool() && !IsIconic(h).as_bool(), DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut _, std::mem::size_of::<RECT>() as u32).is_ok() || windows::Win32::UI::WindowsAndMessaging::GetWindowRect(h, &mut r).is_ok())
            };
            if visible && cloaked == 0 && known && !ours(h) {
                let its = egui::Rect::from_min_max(pos2(r.left as f32, r.top as f32), pos2(r.right as f32, r.bottom as f32));
                if its.intersects(client) && its.intersect(client).area() > 0.0 {
                    return true;
                }
            }
            // SAFETY: as above.
            at = unsafe { GetWindow(h, GW_HWNDPREV) }.ok();
        }
        true
    }

    /// Whether every window of `shown` lies right above `game`, under anything else that shows: what ours keep while another program has the keyboard. Another
    /// window put between them (one shown later, not brought in front) is what breaks it.
    pub(crate) fn holds(game: HWND, shown: &[HWND]) -> bool {
        if shown.is_empty() {
            return true;
        }
        let mut seen = 0;
        // SAFETY: plain queries walking Windows' order.
        let mut at = unsafe { GetWindow(game, GW_HWNDPREV) }.ok();
        for _ in 0..4096 {
            let Some(h) = at.filter(|h| !h.is_invalid()) else { break };
            if shown.contains(&h) {
                seen += 1;
                if seen == shown.len() {
                    return true;
                }
            // SAFETY: a plain query.
            } else if unsafe { IsWindowVisible(h) }.as_bool() && !ours(h) {
                return false;
            }
            // SAFETY: as above.
            at = unsafe { GetWindow(h, GW_HWNDPREV) }.ok();
        }
        seen == shown.len()
    }
}

/// Whether VRChat's window `game` shows on the screen: visible, not minimised (found windows never are, [`Game::find`]) and not cloaked (on another virtual
/// desktop, or being put away). Whatever has the keyboard.
fn on_screen(game: HWND) -> bool {
    use windows::Win32::Graphics::Dwm::DWMWA_CLOAKED;
    let mut cloaked = 0u32;
    // SAFETY: plain queries; the output and its size are passed.
    unsafe {
        let visible = windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(game).as_bool() && !IsIconic(game).as_bool();
        let _ = DwmGetWindowAttribute(game, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, std::mem::size_of::<u32>() as u32);
        visible && cloaked == 0
    }
}

/// What the loop decides from VRChat's window and the keyboard's owner, each turn (REQUEST15 item 1): whether ours show at all, and whether VRChat has the keyboard.
/// Before, ours went the moment another program had the keyboard (`in_front` = VRChat in front): now they stay as long as VRChat's window is on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Presence {
    /// Ours show: VRChat's window is there, not minimised, not hidden, not in exclusive full screen.
    pub(crate) show: bool,
    /// The keyboard is VRChat's (or a window of ours has it): the keys of ours (Esc and R for its menus, 1 to 6 on the ring, Left Alt, Alt+F7 to F9) are read, the
    /// pointer's hook may take the mouse, and ours are topmost. Otherwise none of that: what is typed is for the program in front.
    pub(crate) keyboard: bool,
}

impl Presence {
    pub(crate) fn of(found: bool, on_screen: bool, exclusive: bool, front_is_game_or_ours: bool) -> Self {
        let show = found && on_screen && !exclusive;
        Self { show, keyboard: show && front_is_game_or_ours }
    }
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
    pub(crate) const SIZE: Vec2 = vec2(320.0, 3.0 * Self::ROW + 2.0 * Self::GAP);
    const ROW: f32 = 30.0;
    const GAP: f32 = 5.0;
    pub(super) const MARGIN: f32 = 14.0;

    /// Where it lies in a window of `client` pixels (window pixels), and its zoom: in the bottom left corner, lifted over any part of ours that any of the
    /// menus' layouts puts there (the strip under the Quick Menu in a squarish window), so that it stays in one place for a window's size and covers none of
    /// them. Only a page of ours, open, may lie under it, and it stays above that.
    pub(crate) fn rect(client: Vec2) -> (egui::Rect, f32) {
        // By the window's height, and by its width in a squarish window, where the wheel's second ring and caption leave a narrower column.
        let zoom = (client.y / 720.0).min(client.x / 1050.0).clamp(0.75, 2.0);
        let size = Self::SIZE * zoom;
        let first = egui::Rect::from_min_size(pos2(Self::MARGIN * zoom, client.y - Self::MARGIN * zoom - size.y), size);
        let parts: Vec<egui::Rect> = [zones::layout_as(Around::QuickMenu, client, true), zones::layout(Around::QuickMenu, client), zones::layout(Around::Wheel, client)]
            .concat()
            .into_iter()
            .chain(zones::place(Zone::SheetHint, client))
            .map(|p| p.rect)
            .collect();
        // The lowest place in the corner's column that lies on none of them, no higher than 30 % of the window (VRChat's hints are in its top
        // corners); with no room, at the bottom, above ours.
        let free = |r: &egui::Rect| !parts.iter().any(|p| p.intersects(r.expand(4.0 * zoom)));
        let step = 4.0 * zoom;
        let r = (0..)
            .map(|i| first.translate(vec2(0.0, -(i as f32) * step)))
            .take_while(|r| r.top() >= client.y * 0.3)
            .find(free)
            .unwrap_or(first);
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
        let room = caps.left() - dot.x - 20.0;
        let size = crate::mismatch::fitting(p, words, &[13.0, 12.5, 12.0, 11.5, 11.0], room);
        orbiters_ui::widgets::text_truncated(p, pos2(dot.x + 13.0, r.center().y), egui::Align2::LEFT_CENTER, &Txt::new(words, size).weight(weight::SEMIBOLD).color(colour), room + 2.0);
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
    front: Option<Option<(i32, i32, bool)>>,
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
    fn raise(&self) {
        if let Some(w) = self.window.as_ref().filter(|w| w.shown) {
            w.raise();
        }
    }

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
    /// The left button's edges as the hook saw them, in order (true: down), at most [`EDGES_AT_MOST`] waiting. A button already
    /// down when the hook started is the game's: its up goes on to the game, not to the panels (verify-vr: two counters could
    /// not keep the order of an up and a down between two reads, and the game's up was swallowed).
    #[derive(Debug, Default)]
    pub(super) struct Edges {
        queue: std::collections::VecDeque<bool>,
        /// The game holds the button (it was down when the hook started).
        game_holds: bool,
    }

    pub(super) const EDGES_AT_MOST: usize = 64;

    impl Edges {
        pub(super) fn started(game_holds: bool) -> Self {
            Self { queue: Default::default(), game_holds }
        }

        /// An edge seen by the hook: whether it is swallowed (the panels') or goes on (the game's).
        pub(super) fn edge(&mut self, down: bool) -> bool {
            if !down && std::mem::take(&mut self.game_holds) {
                return false;
            }
            if self.queue.len() >= EDGES_AT_MOST {
                // Full (the panels stopped reading): the button goes on to the game rather than vanish (verify2).
                return false;
            }
            self.queue.push_back(down);
            true
        }

        /// The edges since the last read, in order.
        pub(super) fn take(&mut self) -> Vec<bool> {
            self.queue.drain(..).collect()
        }
    }

    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE,
        WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_MOUSEWHEEL,
    };

    /// The mouse's distance since the hook was set; buttons pressed and let go since last read.
    static DX: AtomicI64 = AtomicI64::new(0);
    static DY: AtomicI64 = AtomicI64::new(0);
    static REST_X: AtomicI32 = AtomicI32::new(0);
    static REST_Y: AtomicI32 = AtomicI32::new(0);
    static EDGES: std::sync::Mutex<Option<Edges>> = std::sync::Mutex::new(None);
    static SET: AtomicBool = AtomicBool::new(false);
    /// VRChat's window while the hook is set: the hook keeps nothing from anyone once it is not in front (verify-r15).
    static GAME: AtomicI64 = AtomicI64::new(0);

    /// Whether the hook may keep the mouse now: VRChat (or a window of ours) has the keyboard.
    pub(super) fn takes_now(game: isize) -> bool {
        // SAFETY: a plain query.
        let front = unsafe { GetForegroundWindow() };
        game != 0 && (front.0 as isize == game || stack::ours(front))
    }

    unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 && SET.load(Ordering::Relaxed) && takes_now(GAME.load(Ordering::Relaxed) as isize) {
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
                WM_LBUTTONDOWN | WM_LBUTTONUP => {
                    let down = message == WM_LBUTTONDOWN;
                    let swallowed = EDGES.lock().map(|mut e| e.get_or_insert_with(Edges::default).edge(down)).unwrap_or(false);
                    if swallowed {
                        return LRESULT(1);
                    }
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
    /// The pointer's picture, in pixels at 100 %.
    const PX: u32 = 24;

    #[derive(Default)]
    pub struct Pointer {
        /// The hook's thread, while Alt is held.
        thread: Option<(u32, std::thread::JoinHandle<()>)>,
        /// Where it is in screen pixels.
        at: Option<Pos2>,
        taken: (i64, i64),
        window: Option<PanelWindow>,
        /// The accent and size its picture was made with: made again when either changes (verify-r15: it kept the first accent).
        look: (u32, u32),
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
                GAME.store(game.0 as isize as i64, Ordering::Relaxed);
                self.begin();
            }
            let (dx, dy) = (DX.load(Ordering::Relaxed), DY.load(Ordering::Relaxed));
            let moved = vec2((dx - self.taken.0) as f32, (dy - self.taken.1) as f32);
            self.taken = (dx, dy);
            let window = egui::Rect::from_min_size(origin, client);
            let at = (self.at.unwrap_or(origin) + moved).clamp(window.min, window.max);
            self.at = Some(at);
            let accent = super::super::panels::look::accent();
            // SAFETY: a plain query; a window gone answers 0.
            let px = (PX * unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(game) }.max(96) / 96).clamp(PX, PX * 4);
            let look = (u32::from_be_bytes([0, accent.r(), accent.g(), accent.b()]), px);
            if self.look != look {
                self.look = look;
                self.window = None;
            }
            if self.window.is_none() {
                self.window = PanelWindow::new(px, px, false).map_err(|e| eprintln!("pointer window: {e}")).ok().map(|mut w| {
                    let picture = super::cursor::picture(px, px as f32 / PX as f32, accent);
                    w.picture(&picture, [0, 0, px, px]);
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
            let (x, y) = cursor_at();
            REST_X.store(x, Ordering::Relaxed);
            REST_Y.store(y, Ordering::Relaxed);
            let (tx, rx) = std::sync::mpsc::channel();
            // Given up on (it did not start within a second): a hook that comes late never takes the mouse (verify2).
            let abandoned = std::sync::Arc::new(AtomicBool::new(false));
            let late = abandoned.clone();
            let spawned = std::thread::Builder::new().name("orbit-mouse-hook".into()).spawn(move || {
                // SAFETY: a low-level hook of this thread, unset below; its messages are pumped right here, as such a hook needs.
                unsafe {
                    let module = GetModuleHandleW(None).unwrap_or_default();
                    let Ok(set) = SetWindowsHookExW(WH_MOUSE_LL, Some(hook), Some(module.into()), 0) else {
                        let _ = tx.send(0);
                        return;
                    };
                    // The left button as it is now that the hook is in (no event reaches it before this thread pumps): held, it is
                    // the game's until it is let go (verify2: read before the hook, a press in between was swallowed).
                    let held = windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x01) as u16 & 0x8000 != 0;
                    if let Ok(mut edges) = EDGES.lock() {
                        *edges = Some(Edges::started(held));
                    }
                    if late.load(Ordering::Acquire) || tx.send(windows::Win32::System::Threading::GetCurrentThreadId()).is_err() {
                        let _ = UnhookWindowsHookEx(set);
                        return;
                    }
                    SET.store(true, Ordering::Relaxed);
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
                    SET.store(false, Ordering::Relaxed);
                    let _ = UnhookWindowsHookEx(set);
                }
            });
            let answer = rx.recv_timeout(Duration::from_secs(1));
            if answer.is_err() {
                abandoned.store(true, Ordering::Release);
            }
            drop(rx);
            if let (Ok(handle), Ok(id)) = (spawned, answer) {
                if id != 0 {
                    self.thread = Some((id, handle));
                }
            }
        }

        pub(super) fn window(&self) -> Option<&PanelWindow> {
            self.window.as_ref()
        }

        /// Back above every window of ours, as what it points at must never cover it.
        pub fn raise(&self) {
            if let Some(w) = self.window.as_ref().filter(|w| w.shown) {
                w.raise();
            }
        }

        /// Alt let go, the wheel gone, anything: the mouse is VRChat's again at once.
        pub fn stop(&mut self) {
            // First, as the scroll's `stop` does: the hook passes everything on from now, whatever its thread does.
            SET.store(false, Ordering::Relaxed);
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
            EDGES.lock().ok().and_then(|mut e| e.as_mut().map(Edges::take)).unwrap_or_default()
        }
    }

    impl Drop for Pointer {
        fn drop(&mut self) {
            self.stop();
        }
    }


}

#[cfg(test)]
mod tests {
    /// OVL2-6 and verify-vr: the edges come in the order they happened; a button held by the game as the hook started has its up
    /// passed on to the game, and a press after it is the panels'.
    #[test]
    fn button_edges_keep_their_order_and_the_game_keeps_its_up() {
        use super::pointer::{Edges, EDGES_AT_MOST};
        let mut edges = Edges::started(true);
        assert!(!edges.edge(false), "the game's up goes to the game");
        assert!(edges.edge(true), "a new press is the panels'");
        assert_eq!(edges.take(), [true], "a down while the finger is down, not a click");
        assert!(edges.edge(false));
        assert_eq!(edges.take(), [false]);
        let mut edges = Edges::started(false);
        assert!(edges.edge(false), "an up with no down is a plain up, and ends");
        for down in [true, false, true] {
            edges.edge(down);
        }
        assert_eq!(edges.take(), [false, true, false, true]);
        assert!(edges.take().is_empty());
        for _ in 0..EDGES_AT_MOST {
            assert!(edges.edge(true));
        }
        assert!(!edges.edge(false), "full: a release goes on to the game rather than vanish");
        assert_eq!(edges.take().len(), EDGES_AT_MOST, "bounded");
    }

    /// REL-3: a part's copy box is used only when it lies wholly inside the frame (a frame of the window's older size read with the
    /// new client's parts was a box outside the texture).
    #[test]
    fn a_copy_box_outside_the_frame_is_never_used() {
        use super::source_box;
        let frame = (1936, 1119);
        assert_eq!(source_box([10, 20, 100, 50], (8, 31), frame), Some([18, 51, 118, 101]));
        assert_eq!(source_box([1828, 0, 100, 50], (8, 31), frame), Some([1836, 31, 1936, 81]), "exactly fitting");
        assert_eq!(source_box([1829, 0, 100, 50], (8, 31), frame), None, "beyond the right edge");
        assert_eq!(source_box([0, 1039, 100, 50], (8, 31), frame), None, "beyond the bottom");
        assert_eq!(source_box([0, 0, 0, 50], (8, 31), frame), None, "nothing to copy");
        assert_eq!(source_box([u32::MAX - 4, 0, 10, 10], (8, 31), (u32::MAX, u32::MAX)), None, "an overflow");
        // The old window's frame, the new window's part: what happened while VRChat's window grew.
        assert_eq!(source_box([1200, 600, 300, 200], (8, 31), (1296, 759)), None);
    }

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
                use windows::Win32::UI::WindowsAndMessaging::{MINMAXINFO, WM_GETMINMAXINFO};
                // Any size, as a game's window in full screen on a screen larger than this computer's: Windows holds a window to its screens' size otherwise.
                if message == WM_GETMINMAXINFO {
                    // SAFETY: the system passes a MINMAXINFO with this message.
                    unsafe { (*(lparam.0 as *mut MINMAXINFO)).ptMaxTrackSize = POINT { x: 16384, y: 16384 } };
                    return LRESULT(0);
                }
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

    /// Whose a window is, for a failure's message: its process and its class.
    fn whose(hwnd: HWND) -> String {
        let mut pid = 0u32;
        let mut class = [0u16; 128];
        // SAFETY: plain questions about a window; the buffer outlives the call.
        let (len, _) = unsafe { (GetClassNameW(hwnd, &mut class), GetWindowThreadProcessId(hwnd, Some(&mut pid))) };
        let class = String::from_utf16_lossy(&class[..len.max(0) as usize]);
        format!("{hwnd:?} of process {pid} (this one is {}), class {class:?}", std::process::id())
    }

    /// The name of the mutex that keeps these tests' real windows one test at a time across processes, and the file that says who holds it.
    const WINDOWS_MUTEX: windows::core::PCWSTR = w!("Local\\OrbitersDesktopTests.RealWindows");
    const WINDOWS_HOLDER: &str = "orbiters-desktop-tests-real-windows.holder";

    /// These tests' real windows lie at one place off every screen ([`off_screen`]), the same in every process, and Windows' hit testing answers for the whole desktop:
    /// two test processes at once (two streams' runs, a test binary copied out and run beside the queue) lay their windows over each other's, and one finds a window
    /// "neither ours nor the stand-in" (BUILD-8). One such test at a time in the process ([`super::super::flows::ONE_AT_A_TIME`]), then one across processes, always in
    /// that order, held from the first window to the last one's end; let go in the other order on drop, panics included.
    struct WindowsAlone {
        _across: AcrossProcesses,
        _in_process: std::sync::MutexGuard<'static, ()>,
    }

    fn windows_alone() -> WindowsAlone {
        let in_process = super::super::flows::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let across = across_processes(Duration::from_secs(600)).unwrap_or_else(|e| panic!("{e}"));
        WindowsAlone { _across: across, _in_process: in_process }
    }

    /// The real windows' place held across processes: the named mutex, owned by the thread that took it.
    struct AcrossProcesses(windows::Win32::Foundation::HANDLE);

    /// Takes the real windows' place across processes, waiting at most `wait` for another process to let go; the error says who holds it.
    fn across_processes(wait: Duration) -> Result<AcrossProcesses, String> {
        use windows::Win32::Foundation::{CloseHandle, WAIT_ABANDONED, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
        let holder = std::env::temp_dir().join(WINDOWS_HOLDER);
        // SAFETY: a named mutex this process opens and closes (in `Drop` or below).
        let mutex = unsafe { CreateMutexW(None, false, WINDOWS_MUTEX) }.map_err(|e| format!("the real windows' mutex: {e}"))?;
        // SAFETY: a live handle.
        let got = unsafe { WaitForSingleObject(mutex, wait.as_millis().min(u32::MAX as u128 - 1) as u32) };
        if got != WAIT_OBJECT_0 && got != WAIT_ABANDONED {
            // SAFETY: opened above, not owned.
            let _ = unsafe { CloseHandle(mutex) };
            let who = std::fs::read_to_string(&holder).unwrap_or_else(|_| "nobody said".into());
            return Err(format!("another test process kept the real windows' place for more than {wait:?} ({got:?}); its holder: {who}"));
        }
        if got == WAIT_ABANDONED {
            eprintln!("the real windows' mutex was left by a process that ended without letting go; taken over"); // i18n: not shown (a test's note)
        }
        let me = format!("process {} test {:?}", std::process::id(), std::thread::current().name().unwrap_or("?"));
        let _ = std::fs::write(&holder, me);
        Ok(AcrossProcesses(mutex))
    }

    impl Drop for AcrossProcesses {
        fn drop(&mut self) {
            use windows::Win32::Foundation::CloseHandle;
            use windows::Win32::System::Threading::ReleaseMutex;
            // SAFETY: owned by this thread since `across_processes` (a test holds it on its own thread), released and closed once.
            unsafe {
                let _ = ReleaseMutex(self.0);
                let _ = CloseHandle(self.0);
            }
        }
    }

    /// Another process holding the real windows' place keeps a test here waiting, and lets it in once it goes; a test here waits for nothing when nobody holds it.
    #[test]
    fn the_real_windows_are_one_test_process_s_at_a_time() {
        use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject, INFINITE};
        // This process's real-window tests wait outside: none of them holds the place across processes while this one plays the other process.
        let _alone = super::super::flows::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        drop(across_processes(Duration::from_secs(600)).expect("nobody else holds it"));
        // Another process, as far as the mutex knows: another thread (a mutex is its owning thread's), holding it until told.
        let (held, hold) = (std::sync::mpsc::channel(), std::sync::mpsc::channel::<()>());
        let other = std::thread::spawn(move || {
            // SAFETY: as in `across_processes`: opened, owned, released and closed on this thread.
            unsafe {
                let mutex = CreateMutexW(None, false, WINDOWS_MUTEX).expect("the mutex");
                assert_eq!(WaitForSingleObject(mutex, INFINITE), WAIT_OBJECT_0);
                let _ = std::fs::write(std::env::temp_dir().join(WINDOWS_HOLDER), "the other test thread");
                held.0.send(()).unwrap();
                let _ = hold.1.recv();
                let _ = ReleaseMutex(mutex);
                let _ = CloseHandle(mutex);
            }
        });
        held.1.recv().unwrap();
        let refused = across_processes(Duration::from_millis(200)).err().expect("kept out while another holds it");
        assert!(refused.contains("the other test thread"), "{refused}");
        hold.0.send(()).unwrap();
        let _in = across_processes(Duration::from_secs(600)).expect("let in once the other goes");
        other.join().unwrap();
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
                (p, Some(zone.unwrap_or_else(|| panic!("at {p:?} the mouse goes to a window that is neither ours nor the stand-in: {}", whose(got)))))
            })
            .collect()
    }

    /// [`grid`], with the points that go to `besides` (the mismatch corner) left out.
    fn grid_besides(windows: &Windows, stand_in: &StandIn, origin: Pos2, client: Vec2, besides: Option<HWND>) -> Vec<(Pos2, Option<Zone>)> {
        let mut points: Vec<Pos2> = (0..(client.y as i32 / 6)).flat_map(|y| (0..(client.x as i32 / 6)).map(move |x| pos2(x as f32 * 6.0 + 3.0, y as f32 * 6.0 + 3.0))).collect();
        points.push(pos2((client.x / 2.0).floor(), (client.y / 2.0).floor()));
        points
            .into_iter()
            .filter_map(|p| {
                let got = under(POINT { x: origin.x as i32 + p.x as i32, y: origin.y as i32 + p.y as i32 });
                if Some(got) == besides {
                    return None;
                }
                if got == stand_in.0 {
                    return Some((p, None));
                }
                let zone = windows.list.iter().find(|(_, w)| w.hwnd == got).map(|(z, _)| *z);
                Some((p, Some(zone.unwrap_or_else(|| panic!("at {p:?} the mouse goes to a window that is neither ours nor the stand-in: {}", whose(got))))))
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
        let _alone = windows_alone();
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
        let _alone = windows_alone();
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
        let _alone = windows_alone();
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
                    assert!(windows.list.values().any(|w| w.hwnd == got), "{open:?} {client:?}: {p:?} goes to a window neither ours nor VRChat's: {}", whose(got)); // i18n: not shown (a menu mismatch report's line)
                    assert!(open != Around::Wheel, "{open:?} {client:?}: with the wheel open {p:?} goes to a part of ours"); // i18n: not shown (a menu mismatch report's line)
                    on_ours += 1;
                }
            }
            eprintln!("mismatch corner, {open:?} {client:?}: {on_corner} points on its rows, {on_ours} on our parts"); // i18n: not shown (a test's measurement)
            assert!(on_corner > 0);
            if open == Around::Wheel {
                assert_eq!(under(POINT { x: origin.x as i32 + centre.x as i32, y: origin.y as i32 + centre.y as i32 }), stand_in.0, "{open:?} {client:?}: the centre, where VRChat holds the pointer with its wheel open, is VRChat's");
            } // i18n: not shown (a menu mismatch report's line)
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
            let column = egui::Rect::from_min_max(pos2(0.0, client.y * 0.3), pos2(rect.right() + 8.0, client.y));
            let blocking: Vec<(Zone, egui::Rect)> = [zones::layout_as(Around::QuickMenu, client, true), zones::layout(Around::QuickMenu, client), zones::layout(Around::Wheel, client)]
                .concat()
                .into_iter()
                .filter(|p| p.rect.intersects(column))
                .map(|p| (p.zone, p.rect))
                .collect();
            eprintln!("corner {client:?}: {rect:?}; in its column: {blocking:?}"); // i18n: not shown (a test's measurement)
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
        let _alone = windows_alone();
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

    /// L-deskcam-glitch, the column's lag: at rest beside VRChat's camera panel at Enzo's screen size (2560 x 1440), a turn of the loop paints nothing and moves no
    /// window, and costs little; when a recording starts, only what changed is painted again (Record and the words). Measured times are printed for the report.
    #[test]
    fn the_camera_column_at_rest_costs_a_turn_nothing_and_a_change_only_its_parts() {
        let _alone = windows_alone();
        for client in [vec2(1280.0, 720.0), vec2(2560.0, 1440.0)] {
            let panel = egui::Rect::from_center_size(pos2(client.x / 2.0, client.y / 2.0), vec2(616.0, 350.0) * client.y / 720.0);
            let mut snap = crate::screenshot::idle_snapshot(&orbiters_core::settings::Settings::default(), &[]);
            snap.steamvr.running = false;
            snap.camera_open_said = Some(true);
            snap.worn_kit = Some(true);
            let places = zones::camera_on_screen(panel, client);
            let (mut panels, mut windows, _stand_in, origin) = put_up_as(Around::Camera, client, false, places.clone(), snap.clone());
            let turn = |panels: &mut Panels, windows: &mut Windows, snap: &Snapshot| {
                let began = Instant::now();
                pump();
                panels.set_open(Some(Around::Camera), places.clone(), snap);
                panels.follow(snap, 0);
                panels.menu_turn(snap);
                panels.step_springs(0.008);
                let moved: Vec<POINT> = windows.list.values().map(|w| w.at).collect();
                windows.show(panels, snap, origin, Some(Around::Camera));
                let still = windows.list.values().map(|w| w.at).collect::<Vec<_>>() == moved;
                (began.elapsed(), still)
            };
            for _ in 0..20 {
                turn(&mut panels, &mut windows, &snap);
            }
            let painted = windows.painted;
            let mut at_rest = Vec::new();
            for _ in 0..200 {
                let (took, still) = turn(&mut panels, &mut windows, &snap);
                assert!(still, "{client:?}: a window moved at rest");
                at_rest.push(took.as_secs_f64() * 1000.0);
            }
            assert_eq!(windows.painted, painted, "{client:?}: a part was painted again with nothing changed");
            // A recording starts: Record becomes its clock and Stop, and the words under the tiles say so.
            let mut counting = snap.clone();
            counting.capture.state = crate::state::CaptureState::Recording;
            let (took, _) = turn(&mut panels, &mut windows, &counting);
            let repainted = windows.painted - painted;
            at_rest.sort_by(f64::total_cmp);
            let (median, worst) = (at_rest[at_rest.len() / 2], at_rest[at_rest.len() - 1]);
            eprintln!("column turn {client:?}: at rest median {median:.3} ms, slowest {worst:.3} ms; recording-starts turn {:.3} ms, {repainted} parts painted", took.as_secs_f64() * 1000.0); // i18n: not shown (a test's measurement)
            assert!((1..=3).contains(&repainted), "{client:?}: a recording starting painted {repainted} parts");
            assert!(median < 4.0, "{client:?}: a turn at rest takes {median} ms");
        }
    }

    /// The loop never waits for the capture: asking for looks and taking their answers return at once, while the looks' thread is busy failing to capture a window
    /// that cannot be captured (and so sleeps between tries).
    #[test]
    fn asking_for_looks_never_waits_for_the_capture() {
        let looker = Looker::start();
        let ask = Ask { window: 0x7fff_fff0, pid: 0, client: vec2(2560.0, 1440.0), menus: true, menu_every: Duration::ZERO, camera: true, shown: None, open: true };
        let mut slowest = Duration::ZERO;
        let began = Instant::now();
        while began.elapsed() < Duration::from_millis(400) {
            let t = Instant::now();
            looker.ask(Some(ask));
            let answers = looker.answers();
            slowest = slowest.max(t.elapsed());
            assert!(answers.is_empty(), "nothing can be looked at in no window");
            std::thread::sleep(Duration::from_millis(8));
        }
        looker.ask(None);
        eprintln!("asking for looks: slowest {:.3} ms", slowest.as_secs_f64() * 1000.0); // i18n: not shown (a test's measurement)
        assert!(slowest < Duration::from_millis(5), "{slowest:?}");
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

    // --------------------------------------------------------------------------------------------- L-fullscreen-resize

    use super::super::{Action, Pressed, Sheet};

    /// VRChat's camera panel's rim in a window of `client`: centred, scaling with the window (measured at 960 x 540, 1280 x 720 and 1920 x 1080, `done7-deskcam.md`).
    fn camera_rim(client: Vec2) -> egui::Rect {
        egui::Rect::from_center_size(pos2(client.x / 2.0, client.y / 2.0), vec2(616.0, 350.0) * client.y / 720.0)
    }

    /// What is up over the stand-in: VRChat's menu, and what of ours shows with it.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Up {
        /// The Quick Menu with its side tiles alone.
        Tiles,
        /// The Quick Menu with this page open beside its tiles.
        Page(Sheet),
        /// The 3D worlds' page, its list scrolled down: Enzo's picture (`logs7/fullscreen-resize.png`).
        WorldsList,
        /// The 3D worlds' page on a world chosen, with its small 3D view.
        WorldChosen,
        /// VRChat's wheel with our ring.
        Wheel,
        /// A page opened from the wheel.
        WheelPage,
        /// VRChat's camera with our column at its left.
        Camera,
    }

    impl Up {
        fn name(self) -> String {
            match self {
                Up::Page(sheet) => format!("page-{sheet:?}").to_lowercase(),
                other => format!("{other:?}").to_lowercase(),
            }
        }
    }

    /// What a part had that a change of the window's size must leave as it was.
    #[derive(Debug, PartialEq)]
    struct Held {
        sheet: Option<Sheet>,
        title: String,
        scroll: f32,
        world: Option<(crate::overlay::worlds::panel::Page, String, bool)>,
        list: Option<f32>,
    }

    /// The desktop's parts as [`run`] turns them, over a stand-in for VRChat's window that is moved and resized for real through Windows: each turn reads its client
    /// area's size and place back as `run` does ([`client_size`], [`client_origin`]) and lays everything out from them.
    struct Rig {
        panels: Panels,
        windows: Windows,
        /// The menu mismatch corner, on as a tester has it: it follows the window as every part does.
        corner: Corner,
        stand_in: StandIn,
        snap: Snapshot,
        open: Around,
        up: Up,
    }

    impl Rig {
        fn new(up: Up, client: Vec2) -> Self {
            let mut snap = crate::screenshot::idle_snapshot(&orbiters_core::settings::Settings::default(), &[]);
            let open = match up {
                Up::Wheel | Up::WheelPage => Around::Wheel,
                Up::Camera => Around::Camera,
                _ => Around::QuickMenu,
            };
            if up == Up::Camera {
                snap.steamvr.running = false;
                snap.camera_open_said = Some(true);
            }
            let stand_in = StandIn::new(off_screen(client), client);
            let mut rig = Self { panels: Panels::new(Arc::new(crate::screenshot::Still), 1.0, true), windows: Windows::default(), corner: Corner::default(), stand_in, snap, open, up };
            rig.settle("the menu's parts");
            let snap = rig.snap.clone();
            match up {
                Up::Page(sheet) => {
                    if sheet == Sheet::Menu {
                        rig.panels.stage.host.menu.page = orbiters_n4n0_core::menu::Page::States;
                    }
                    rig.panels.open_sheet(sheet, "", &snap);
                }
                Up::WheelPage => rig.panels.open_sheet(Sheet::All, "", &snap),
                Up::WorldsList | Up::WorldChosen => {
                    rig.panels.act(Action::Worlds, &snap, &mut Pressed::default());
                    assert_eq!(rig.panels.facts.sheet, Some(Sheet::Worlds), "the 3D worlds' page opened");
                    let listing = (1..=40)
                        .map(|n| format!(r#"{{"slot":{n},"key":{n},"id":{n},"worldId":"wrld_00000000-0000-0000-0000-{n:012}","name":"World {n}","author":"Maker {n}","uploader":"a","view":{{"position":[0,1,0],"target":[0,1,1]}}}}"#))
                        .collect::<Vec<_>>()
                        .join(",");
                    let entries = crate::overlay::worlds::catalog::parse(&format!(r#"{{"previews":[{listing}]}}"#)).expect("a listing");
                    // The page's own read of the listing first: what it brings would replace this one, and take the list back to its top, at any moment.
                    let began = Instant::now();
                    while rig.panels.stage.worlds.as_ref().is_some_and(|w| w.listing_read_for_test()) {
                        assert!(began.elapsed() < Duration::from_secs(30), "the listing's read never ended");
                        rig.turn();
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    let worlds = rig.panels.stage.worlds.as_mut().expect("the worlds' page");
                    worlds.listing_for_test(entries.clone());
                    rig.settle("its list");
                    if up == Up::WorldsList {
                        // The list knows how far it goes once it has drawn the 40 worlds: until then a scroll is held at its top.
                        let began = Instant::now();
                        while rig.panels.stage.worlds.as_ref().is_some_and(|w| w.list_scroll_for_test() < 260.0) {
                            assert!(began.elapsed() < Duration::from_secs(20), "the list never scrolled");
                            if let Some(worlds) = rig.panels.stage.worlds.as_mut() {
                                worlds.scroll_by(260.0 - worlds.list_scroll_for_test());
                            }
                            rig.turn();
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    } else {
                        rig.panels.stage.worlds.as_mut().expect("the worlds' page").pick_for_test(entries[6].clone());
                    }
                }
                _ => {}
            }
            rig.settle("everything at rest");
            if up == Up::WorldsList {
                assert!(rig.held().list.is_some_and(|s| s > 100.0), "the list is scrolled down: {:?}", rig.held());
                assert!(!rig.panels.stage.worlds.as_ref().is_some_and(|w| w.listing_read_for_test()), "no read of the listing under way");
            }
            rig
        }

        fn client(&self) -> Vec2 {
            client_size(self.stand_in.0)
        }

        fn origin(&self) -> Pos2 {
            client_origin(self.stand_in.0)
        }

        /// Where the parts go in a window of `client`, as `run` lays them.
        fn places(&self, client: Vec2) -> Vec<Placed> {
            match self.open {
                Around::Camera => zones::camera_on_screen(camera_rim(client), client),
                menu => zones::layout_as(menu, client, self.snap.settings.orbit.layout == orbiters_core::settings::OrbitLayout::Strip),
            }
        }

        /// One turn of [`run`]'s loop, from the window as it is now.
        fn turn(&mut self) {
            pump();
            let (client, origin) = (self.client(), self.origin());
            let places = self.places(client);
            self.panels.takeover_place = zones::place(Zone::Takeover, client);
            self.panels.back_place = zones::place(Zone::SheetBack, client);
            self.panels.hint_place = zones::place(Zone::SheetHint, client);
            self.panels.set_open(Some(self.open), places, &self.snap);
            self.panels.follow(&self.snap, 0);
            self.panels.menu_turn(&self.snap);
            self.panels.step_springs(0.05);
            // As `run`: the corner each turn at the window's size, back on top when a part came into view.
            self.corner.show(true, origin, client, Instant::now());
            if self.windows.show(&mut self.panels, &self.snap, origin, Some(self.open)) {
                self.corner.raise();
            }
        }

        fn at_rest(&self) -> bool {
            self.panels.all().filter(|p| p.shown).all(|p| p.painted && p.appear.settled()) && (!self.panels.sheet_open() || self.panels.takeover.as_ref().is_some_and(|p| p.painted))
        }

        fn settle(&mut self, what: &str) {
            let began = Instant::now();
            loop {
                self.turn();
                if self.at_rest() {
                    break;
                }
                assert!(began.elapsed() < Duration::from_secs(20), "{:?} {what}: it never came", self.up);
                std::thread::sleep(Duration::from_millis(2));
            }
            // A few more, for what loads on a first picture.
            for _ in 0..3 {
                self.turn();
            }
        }

        /// VRChat's window goes to `at` with a client area of `client`, through Windows, as full screen, maximising, a drag or a change of screen does.
        fn change(&mut self, at: POINT, client: Vec2) {
            // SAFETY: the stand-in's own window.
            unsafe {
                SetWindowPos(self.stand_in.0, None, at.x, at.y, client.x as i32, client.y as i32, SWP_NOACTIVATE | windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER)
                    .expect("the stand-in moves");
            }
            pump();
            assert_eq!(self.client(), client, "Windows gave the stand-in another size");
        }

        fn held(&self) -> Held {
            let worlds = self.panels.stage.worlds.as_ref().filter(|_| self.panels.facts.sheet == Some(Sheet::Worlds));
            Held {
                sheet: self.panels.facts.sheet,
                title: self.panels.facts.sheet_title.clone(),
                scroll: self.panels.facts.gallery_scroll,
                world: worlds.and_then(|w| w.shown_world_for_test()),
                list: worlds.map(|w| w.list_scroll_for_test()),
            }
        }

        /// What is wrong with what shows now: a part not where the window's size puts it, a window of ours not where its part is or of another size, a window up with no
        /// part, a part up with no window.
        fn wrong(&self, when: &str) -> Vec<String> {
            let (client, origin) = (self.client(), self.origin());
            let mut wants: HashMap<Zone, Placed> = self.places(client).into_iter().map(|p| (p.zone, p)).collect();
            for zone in [Zone::Takeover, Zone::SheetBack, Zone::SheetHint] {
                if let Some(place) = zones::place(zone, client) {
                    wants.insert(zone, place);
                }
            }
            let mut wrong = Vec::new();
            for panel in self.panels.all() {
                if let Some(want) = wants.get(&panel.zone) {
                    if panel.placed.rect != want.rect {
                        wrong.push(format!("{when}: {:?} lies at {:?}, the window puts it at {:?}", panel.zone, panel.placed.rect, want.rect));
                    }
                }
                if !(panel.shown && panel.painted) {
                    continue;
                }
                match self.windows.list.get(&panel.zone).filter(|w| w.shown) {
                    Some(window) => {
                        let (_, at, _) = placed_at(panel, origin);
                        if window.at != at || window.size.cx != panel.px[0] as i32 || window.size.cy != panel.px[1] as i32 {
                            wrong.push(format!("{when}: the window of {:?} is at {:?} {:?}, its part at {at:?} {:?}", panel.zone, window.at, window.size, panel.px));
                        }
                    }
                    None => wrong.push(format!("{when}: {:?} shows with no window up", panel.zone)),
                }
            }
            for (zone, window) in &self.windows.list {
                if window.shown && !self.panels.all().any(|p| p.zone == *zone && p.shown && p.painted) {
                    wrong.push(format!("{when}: the window of {zone:?} shows with no part of it up"));
                }
            }
            // The mismatch corner: where this size puts it, at its size.
            let (rect, _) = Corner::rect(client);
            let at = POINT { x: (origin.x + rect.min.x) as i32, y: (origin.y + rect.min.y) as i32 };
            let size = [rect.width().ceil() as i32, rect.height().ceil() as i32];
            match self.corner.window.as_ref().filter(|w| w.shown) {
                Some(w) if w.at == at && [w.size.cx, w.size.cy] == size => {}
                Some(w) => wrong.push(format!("{when}: the mismatch corner is at {:?} {:?}, this window puts it at {at:?} {size:?}", w.at, w.size)),
                None => wrong.push(format!("{when}: the mismatch corner is not up")),
            }
            wrong
        }

        /// What is wrong with the mouse now (the grid test of `done6-deskwheel.md`, at the window's new size): a point that goes to a window of ours where nothing of ours
        /// is pressed, a part whose middle Windows gives to another window than the one of the part the pointer is found on there; with the wheel, any point of ours.
        fn mouse_wrong(&self, when: &str) -> Vec<String> {
            let (client, origin) = (self.client(), self.origin());
            let mut wrong = Vec::new();
            // The corner's own rows take the mouse: each row's middle is the corner's, and the corner's points are left out of the grid of our parts.
            let corner = self.corner.window.as_ref().map(|w| w.hwnd);
            let (rect, zoom) = Corner::rect(client);
            for (i, kind) in crate::mismatch::Kind::ALL.into_iter().enumerate() {
                let middle = rect.min + vec2(Corner::SIZE.x / 2.0, i as f32 * (Corner::ROW + Corner::GAP) + Corner::ROW / 2.0) * zoom;
                if Some(under(POINT { x: origin.x as i32 + middle.x as i32, y: origin.y as i32 + middle.y as i32 })) != corner {
                    wrong.push(format!("{when}: the middle of the corner's {kind:?} row is not the corner's"));
                }
            }
            let seen = grid_besides(&self.windows, &self.stand_in, origin, client, corner);
            for (p, zone) in &seen {
                if zone.is_some() && (self.open == Around::Wheel || hit(&self.panels, *p).is_none()) {
                    wrong.push(format!("{when}: {p:?} goes to {zone:?}, where nothing of ours is pressed"));
                    break;
                }
            }
            if self.open != Around::Wheel {
                for panel in self.panels.all().filter(|p| p.shown && p.painted && !p.zone.passes_clicks()) {
                    let middle = panel.placed.rect.center();
                    let got = under(POINT { x: origin.x as i32 + middle.x as i32, y: origin.y as i32 + middle.y as i32 });
                    let found = hit(&self.panels, middle).map(|(i, _)| self.panels.all().nth(i).expect("a part").zone);
                    let window = found.and_then(|z| self.windows.list.get(&z)).map(|w| w.hwnd);
                    if window != Some(got) {
                        // What tells a window lying under another from a window with nothing painted there.
                        let ours: Vec<HWND> = self.windows.list.values().map(|w| w.hwnd).collect();
                        let order: Vec<Zone> = from_the_top(&ours).iter().filter_map(|h| self.windows.list.iter().find(|(_, w)| w.hwnd == *h).map(|(z, _)| *z)).collect();
                        let facts = found.and_then(|z| self.windows.list.get(&z)).map(|w| {
                            let (x, y) = (origin.x as i32 + middle.x as i32 - w.at.x, origin.y as i32 + middle.y as i32 - w.at.y);
                            let inside = x >= 0 && y >= 0 && x < w.size.cx && y < w.size.cy;
                            // SAFETY: the DIB section holds the window's pixels while it lives; read inside them only.
                            let alpha = inside.then(|| unsafe { *w.bits.add(((y * w.size.cx + x) * 4 + 3) as usize) });
                            format!("its window shown {} at {:?} size {:?} alpha {}, its pixel there alpha {alpha:?}", w.shown, w.at, w.size, w.alpha)
                        });
                        wrong.push(format!(
                            "{when}: the middle of {:?} is found on {found:?}, and Windows gives it to {} ({}; ours from the top: {order:?})",
                            panel.zone,
                            if got == self.stand_in.0 { "VRChat".to_string() } else { self.windows.list.iter().find(|(_, w)| w.hwnd == got).map_or_else(|| whose(got), |(z, _)| format!("{z:?}")) },
                            facts.unwrap_or_default()
                        ));
                    }
                }
            }
            wrong
        }

        /// What shows over the window now, as Windows composes it (each window of ours at its place, its pixels with its alpha, in the order they lie) over a stand-in of
        /// the game and of VRChat's menu, saved as `<name>.png` in `ORBITERS_RESIZE_SHOTS` when that is set.
        fn picture(&self, name: &str) {
            let Some(dir) = std::env::var_os("ORBITERS_RESIZE_SHOTS") else { return };
            let (client, origin) = (self.client(), self.origin());
            let (w, h) = (client.x as i32, client.y as i32);
            let mut image = image::RgbImage::from_fn(w as u32, h as u32, |_, y| {
                let t = y as f32 / h as f32;
                image::Rgb([(14.0 + 10.0 * t) as u8, (20.0 + 18.0 * t) as u8, (34.0 + 30.0 * t) as u8])
            });
            let mut fill = |r: egui::Rect, c: [u8; 3], round: bool| {
                for y in r.top().max(0.0) as i32..(r.bottom() as i32).min(h) {
                    for x in r.left().max(0.0) as i32..(r.right() as i32).min(w) {
                        let d = (pos2(x as f32 + 0.5, y as f32 + 0.5) - r.center()) / (r.size() / 2.0);
                        if !round || d.length() <= 1.0 {
                            image.put_pixel(x as u32, y as u32, image::Rgb(c));
                        }
                    }
                }
            };
            let (center, unit) = zones::frame(self.open, client);
            match self.open {
                Around::Wheel => {
                    fill(egui::Rect::from_center_size(center, vec2(200.0, 200.0) * unit), [7, 55, 58], true);
                    fill(egui::Rect::from_center_size(center, vec2(96.0, 96.0) * unit), [22, 34, 43], true);
                }
                Around::QuickMenu => {
                    let r = |x0: f32, y0: f32, x1: f32, y1: f32| egui::Rect::from_min_max(center + vec2(x0, y0) * unit, center + vec2(x1, y1) * unit);
                    fill(r(-351.0, -150.0, 351.0, 186.0), [25, 34, 42], false);
                    fill(r(-188.0, -204.0, 188.0, 204.0), [22, 34, 43], false);
                }
                Around::Camera => {
                    let rim = camera_rim(client);
                    fill(rim, [40, 44, 52], false);
                    fill(rim.shrink(8.0 * client.y / 720.0), [120, 150, 170], false);
                }
            }
            let ours: Vec<HWND> = self.windows.list.values().map(|w| w.hwnd).collect();
            for hwnd in from_the_top(&ours).into_iter().rev() {
                let Some(window) = self.windows.list.values().find(|w| w.hwnd == hwnd) else { continue };
                let (ww, wh) = (window.size.cx, window.size.cy);
                // SAFETY: the DIB section holds ww x wh 32-bit pixels while the window lives.
                let bits = unsafe { std::slice::from_raw_parts(window.bits, (ww * wh * 4) as usize) };
                let (ox, oy) = (window.at.x - origin.x as i32, window.at.y - origin.y as i32);
                let k = window.alpha as f32 / 255.0;
                for y in 0..wh {
                    for x in 0..ww {
                        let (dx, dy) = (ox + x, oy + y);
                        if dx < 0 || dy < 0 || dx >= w || dy >= h {
                            continue;
                        }
                        let i = ((y * ww + x) * 4) as usize;
                        let a = bits[i + 3] as f32 / 255.0 * k;
                        if a <= 0.0 {
                            continue;
                        }
                        let d = image.get_pixel_mut(dx as u32, dy as u32);
                        for (c, s) in [(0, bits[i + 2]), (1, bits[i + 1]), (2, bits[i])] {
                            d.0[c] = (s as f32 * k + d.0[c] as f32 * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
                        }
                    }
                }
            }
            let path = std::path::PathBuf::from(dir).join(format!("{name}.png"));
            image.save(&path).expect("the picture is saved");
            eprintln!("picture: {}", path.display()); // i18n: not shown (a test's output)
        }
    }

    /// The changes of VRChat's window a person makes, from a 1280 x 720 window: where its client area goes and its size. Full screen (Alt+Enter, F11 and VRChat's own
    /// setting all make the same change: the client area becomes the screen's) and back, maximised and restored, dragged to other sizes and shapes, onto a larger screen
    /// with another scale (this process is per-monitor DPI aware, so a new scale is a new size in pixels), the screen's resolution changed, moved without a new size.
    fn changes() -> Vec<(&'static str, Vec2, POINT)> {
        // Off every screen, with room for the largest.
        let base = off_screen(vec2(2560.0, 1440.0));
        let at = |dx: i32, dy: i32| POINT { x: base.x + dx, y: base.y + dy };
        vec![
            ("full-screen", vec2(1920.0, 1080.0), at(0, 0)),
            ("windowed-again", vec2(1280.0, 720.0), at(300, 160)),
            ("maximised", vec2(1920.0, 1017.0), at(0, 23)),
            ("restored", vec2(1280.0, 720.0), at(300, 160)),
            ("dragged-wider", vec2(1366.0, 768.0), at(300, 160)),
            ("dragged-taller", vec2(1100.0, 800.0), at(300, 160)),
            ("other-screen", vec2(2560.0, 1440.0), at(0, 0)),
            ("resolution", vec2(1600.0, 900.0), at(0, 0)),
            ("moved", vec2(1600.0, 900.0), at(120, 40)),
            ("windowed-last", vec2(1280.0, 720.0), at(300, 160)),
        ]
    }

    /// Runs `up` through `changes`, from a 1280 x 720 window: after each change, from the very next turn, every part of ours is where the new window puts it (never at
    /// the old size or place, nor a mix of both), and at rest the mouse goes where they are drawn and what they had is still there. Says all that went wrong.
    fn follow_through(up: Up, changes: &[(&'static str, Vec2, POINT)], pictures: &[Vec2]) -> Vec<String> {
        let start = vec2(1280.0, 720.0);
        let mut rig = Rig::new(up, start);
        let name = up.name();
        let mut wrong = rig.wrong(&format!("{name} windowed"));
        let held = rig.held();
        rig.picture(&format!("{name}-1280x720-windowed"));
        for (what, client, at) in changes {
            rig.change(*at, *client);
            rig.turn();
            let size = format!("{}x{}", client.x, client.y);
            let shot = pictures.contains(client);
            if shot {
                rig.picture(&format!("{name}-{size}-{what}-first-turn"));
            }
            wrong.extend(rig.wrong(&format!("{name} {what} {size}, the first turn")));
            rig.settle(what);
            if shot {
                rig.picture(&format!("{name}-{size}-{what}"));
            }
            let when = format!("{name} {what} {size}, at rest");
            wrong.extend(rig.wrong(&when));
            wrong.extend(rig.mouse_wrong(&when));
            if rig.panels.sheet_open() || matches!(up, Up::Page(_) | Up::WheelPage | Up::WorldsList | Up::WorldChosen) {
                assert!(rig.panels.takeover.as_ref().is_some_and(|p| p.painted && p.shown), "{when}: the page is still up");
            }
            let now = rig.held();
            if now != held {
                wrong.push(format!("{when}: it had {held:?}, it has {now:?}"));
            }
        }
        wrong
    }

    /// Enzo, 2026-11-15 (`logs7/fullscreen-resize.png`): the 3D worlds' page open beside VRChat's Quick Menu in a window, and VRChat put in full screen. The page's head
    /// and its frame, and the side tiles, came to the full screen's size and place; the page itself (its search, chips and cards) stayed at the window's old size and
    /// place, in a corner, over an empty body. Every part must be where the full screen puts it from the next turn on, the list where it was scrolled to, and back.
    #[test]
    fn the_worlds_page_follows_vrchat_going_full_screen() {
        let _alone = windows_alone();
        let all = changes();
        let his: Vec<_> = all.iter().filter(|(what, _, _)| matches!(*what, "full-screen" | "windowed-again" | "other-screen")).cloned().collect();
        let wrong = follow_through(Up::WorldsList, &his, &[vec2(1280.0, 720.0), vec2(1920.0, 1080.0), vec2(2560.0, 1440.0)]);
        assert!(wrong.is_empty(), "{} things wrong:\n{}", wrong.len(), wrong.join("\n"));
    }

    /// Every kind of change of VRChat's window, with every kind of part of ours up: the side tiles alone, a page of each kind (the 3D worlds' list and a world chosen
    /// with its small 3D view among them), the wheel's ring, a page from the wheel, the camera's column.
    #[test]
    fn every_part_follows_every_change_of_vrchat_s_window() {
        let _alone = windows_alone();
        let ups = [
            Up::Tiles,
            Up::WorldsList,
            Up::WorldChosen,
            Up::Page(Sheet::All),
            Up::Page(Sheet::Pictures),
            Up::Page(Sheet::Instances),
            Up::Page(Sheet::Menu),
            Up::Page(Sheet::Talk),
            Up::Page(Sheet::Settings),
            Up::Wheel,
            Up::WheelPage,
            Up::Camera,
        ];
        let pictures = [vec2(1280.0, 720.0), vec2(1920.0, 1080.0), vec2(2560.0, 1440.0)];
        let mut wrong = Vec::new();
        for up in ups {
            let found = follow_through(up, &changes(), &pictures);
            eprintln!("{up:?}: {} things wrong", found.len()); // i18n: not shown (a test's measurement)
            wrong.extend(found);
        }
        assert!(wrong.is_empty(), "{} things wrong:\n{}", wrong.len(), wrong.join("\n"));
    }

    /// REQUEST15 item 1: ours stay while VRChat's window is on the screen, whoever has the keyboard; they go when it is minimised, hidden, closed or in exclusive
    /// full screen; the keys of ours are read only while VRChat has the keyboard. Before, `in_front` hid everything the moment another program was in front.
    #[test]
    fn ours_stay_while_vrchat_shows_whoever_has_the_keyboard() {
        let other_program_in_front = Presence::of(true, true, false, false);
        assert_eq!(other_program_in_front, Presence { show: true, keyboard: false }, "shown, but nothing typed elsewhere is ours");
        assert_eq!(Presence::of(true, true, false, true), Presence { show: true, keyboard: true }, "VRChat in front: as ever");
        assert_eq!(Presence::of(true, false, false, true), Presence { show: false, keyboard: false }, "minimised or hidden: gone");
        assert_eq!(Presence::of(false, false, false, false), Presence { show: false, keyboard: false }, "closed: gone");
        assert_eq!(Presence::of(true, true, true, true), Presence { show: false, keyboard: false }, "exclusive full screen: gone");
    }

    /// The window seen as on the screen: shown yes; minimised (Windows keeps it, iconic) or hidden no.
    #[test]
    fn a_minimised_or_hidden_vrchat_window_is_not_on_the_screen() {
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWMINNOACTIVE;
        let _alone = windows_alone();
        let client = vec2(320.0, 180.0);
        let game = StandIn::new(off_screen(client), client);
        assert!(on_screen(game.0), "shown");
        // SAFETY: plain calls on the test's own window.
        let _ = unsafe { ShowWindow(game.0, SW_SHOWMINNOACTIVE) };
        assert!(!on_screen(game.0), "minimised");
        // SAFETY: as above.
        let _ = unsafe { ShowWindow(game.0, SW_SHOWNOACTIVATE) };
        assert!(on_screen(game.0), "back");
        // SAFETY: as above.
        let _ = unsafe { ShowWindow(game.0, SW_HIDE) };
        assert!(!on_screen(game.0), "hidden");
    }

    /// REQUEST15 item 5: the pointer over ours is VRChat's desktop arrow (its tip the hotspot, a notch at its back) in the accent chosen, a cursor of Windows'
    /// at its usual sizes, made again only when the accent changes.
    #[test]
    fn the_pointer_over_ours_is_vrchat_s_arrow_in_the_accent() {
        let accent = egui::Color32::from_rgb(220, 60, 140);
        let px = cursor::picture(24, 1.0, accent);
        let at = |x: usize, y: usize| -> [u8; 4] { let i = (y * 24 + x) * 4; [px[i], px[i + 1], px[i + 2], px[i + 3]] };
        let near = |a: [u8; 4], b: [u8; 4]| a.iter().zip(b).all(|(x, y)| (*x as i32 - y as i32).abs() <= 3);
        assert!(near(at(9, 6), [220, 60, 140, 255]), "the accent inside it: {:?}", at(9, 6));
        assert!(near(at(8, 12), [220, 60, 140, 255]), "both wings: {:?}", at(8, 12));
        for (x, y) in [(17, 17), (23, 0), (0, 23), (14, 14)] {
            assert_eq!(at(x, y)[3], 0, "clear behind its notch and round it at {x},{y}");
        }
        for side in [32, 48, 64] {
            assert!(cursor::make(side, accent).is_some(), "a cursor of {side} pixels");
        }
        cursor::follow(accent, 96);
        let first = cursor::current();
        assert!(first.is_some());
        cursor::follow(accent, 96);
        assert_eq!(cursor::current(), first, "the same accent: the same cursor");
        cursor::follow(egui::Color32::from_rgb(10, 200, 120), 96);
        assert_ne!(cursor::current(), first, "another accent: another cursor");
        // verify-r15: the one replaced is destroyed, not leaked; a larger screen scale makes a larger one.
        let mut info = windows::Win32::UI::WindowsAndMessaging::ICONINFO::default();
        // SAFETY: a plain query about a cursor handle; one destroyed only fails.
        assert!(unsafe { windows::Win32::UI::WindowsAndMessaging::GetIconInfo(windows::Win32::UI::WindowsAndMessaging::HICON(first.unwrap().0), &mut info) }.is_err(), "the replaced cursor is gone");
        let second = cursor::current();
        cursor::follow(egui::Color32::from_rgb(10, 200, 120), 192);
        assert_ne!(cursor::current(), second, "another scale: another cursor");
        assert!(cursor::side_for(192) > cursor::side_for(96), "larger at 200 %");
    }

    /// verify-r15: a kept window handle is VRChat's only while it is of VRChat's process and Unity's class: another window, or one that closed, is not.
    #[test]
    fn a_window_is_vrchat_s_only_while_its_process_and_class_say_so() {
        let _alone = windows_alone();
        unsafe extern "system" fn plain(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            // SAFETY: what the system passed on.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        // SAFETY: plain Win32 calls on this test's own hidden window, destroyed below.
        let unity = unsafe {
            let module = GetModuleHandleW(None).unwrap_or_default();
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(plain), hInstance: module.into(), lpszClassName: w!("UnityWndClass"), ..Default::default() });
            CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, w!("UnityWndClass"), w!(""), WS_POPUP, 0, 0, 8, 8, None, None, None, None).expect("a window of Unity's class")
        };
        let me = std::process::id();
        assert!(is_vrchat_window(unity, me), "its process, its class");
        assert!(!is_vrchat_window(unity, me + 1), "another process's VRChat");
        let other = StandIn::new(off_screen(vec2(8.0, 8.0)), vec2(8.0, 8.0));
        assert!(!is_vrchat_window(other.0, me), "another class");
        // SAFETY: made above.
        let _ = unsafe { DestroyWindow(unity) };
        assert!(!is_vrchat_window(unity, me), "closed");
    }

    /// verify-r15: VRChat's window topmost (its own setting, a tool's): with another program in front ours stay right above it, under a topmost window that
    /// covers it, not over that one. And the Alt pointer's hook keeps nothing once VRChat is not in front.
    #[test]
    fn ours_stay_under_a_topmost_cover_of_a_topmost_vrchat() {
        let _alone = windows_alone();
        let client = vec2(320.0, 180.0);
        let at = off_screen(client);
        // SAFETY: a plain query.
        let front = unsafe { GetForegroundWindow() };
        let game = StandIn::new(at, client);
        let cover = StandIn::new(at, client);
        let flags = SWP_NOSIZE | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE | SWP_NOACTIVATE;
        // SAFETY: the test's own windows.
        unsafe {
            let _ = SetWindowPos(game.0, Some(HWND_TOPMOST), 0, 0, 0, 0, flags);
            let _ = SetWindowPos(cover.0, Some(HWND_TOPMOST), 0, 0, 0, 0, flags);
        }
        stack::set(Some(game.0));
        let mut ours = clear_window();
        ours.place(at, 255);
        assert_eq!(order(&[cover.0, ours.hwnd, game.0]), [cover.0, ours.hwnd, game.0], "under the cover, right above VRChat");
        assert!(stack::holds(game.0, &[ours.hwnd]));
        stack::set(None);
        // SAFETY: a plain query.
        assert_eq!(unsafe { GetForegroundWindow() }, front, "nothing took the keyboard");
        assert!(!pointer::takes_now(game.0 .0 as isize), "VRChat not in front: the hook keeps nothing");
        assert!(!pointer::takes_now(0));
    }

    /// verify-r15: a window at rest is not given to Windows again; moved, faded or repainted it is.
    #[test]
    fn a_window_at_rest_is_not_placed_again() {
        let _alone = windows_alone();
        let at = off_screen(vec2(8.0, 8.0));
        let mut w = clear_window();
        assert!(w.place_if_changed(at, 255), "first shown");
        assert!(!w.place_if_changed(at, 255), "at rest");
        assert!(w.place_if_changed(POINT { x: at.x + 1, y: at.y }, 255), "moved");
        assert!(w.place_if_changed(POINT { x: at.x + 1, y: at.y }, 200), "faded");
        w.picture(&[0u8; 8 * 8 * 4], [0, 0, 8, 8]);
        assert!(w.place_if_changed(POINT { x: at.x + 1, y: at.y }, 200), "repainted");
    }

    /// Where real windows lie, by Windows' own order (top first), among `among`.
    fn order(among: &[HWND]) -> Vec<HWND> {
        from_the_top(among)
    }

    /// REQUEST15 item 1: while another program has the keyboard, ours lie right above VRChat's window and under the window that covers it (they do not float over
    /// other programs), are not topmost, and take no focus; with VRChat's keyboard again they are topmost, in the order they had. A window that comes between
    /// VRChat's and ours is seen ([`stack::holds`]) and ours are laid again.
    #[test]
    fn ours_lie_right_above_vrchat_and_under_what_covers_it() {
        use windows::Win32::UI::WindowsAndMessaging::{HWND_TOP, SWP_NOMOVE};
        let _alone = windows_alone();
        let client = vec2(320.0, 180.0);
        let at = off_screen(client);
        // SAFETY: a plain query. Before any window is made: none of this test's may take the keyboard.
        let front = unsafe { GetForegroundWindow() };
        let game = StandIn::new(at, client);
        // Another program's window over VRChat's (made after it, it lies above).
        let cover = StandIn::new(at, client);
        assert!(stack::set(Some(game.0)), "the layer changes");
        let (mut low, mut high) = (clear_window(), clear_window());
        low.place(at, 255);
        high.place(at, 255);
        let all = [cover.0, high.hwnd, low.hwnd, game.0];
        assert_eq!(order(&all), all, "cover, then ours in their order, then VRChat");
        assert!(!stack::topmost(low.hwnd) && !stack::topmost(high.hwnd), "not topmost while another program has the keyboard");
        assert!(stack::holds(game.0, &[low.hwnd, high.hwnd]));
        assert_eq!(stack::over(game.0), Some(cover.0), "what covers VRChat, ours skipped");
        // VRChat brought above ours (clicked, then another program in front again before our next turn): seen, and laid again.
        // SAFETY: the test's own window.
        let _ = unsafe { SetWindowPos(game.0, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE) };
        assert!(!stack::holds(game.0, &[low.hwnd, high.hwnd]), "VRChat over ours is seen");
        low.raise();
        high.raise();
        assert!(stack::holds(game.0, &[low.hwnd, high.hwnd]));
        // VRChat now lies over the cover: ours right above it, in their order.
        assert_eq!(order(&all), [high.hwnd, low.hwnd, game.0, cover.0]);
        // VRChat's keyboard again: topmost, over the cover, same order.
        assert!(stack::set(None));
        low.raise();
        high.raise();
        assert_eq!(order(&all)[..2], [high.hwnd, low.hwnd], "topmost, above everything");
        assert!(stack::topmost(low.hwnd) && stack::topmost(high.hwnd));
        assert!(!stack::holds(game.0, &[low.hwnd, high.hwnd]), "the cover lies between: not right above VRChat");
        // And back, the other program brought in front again: a topmost window laid after an ordinary one is ordinary again.
        // SAFETY: the test's own window.
        let _ = unsafe { SetWindowPos(cover.0, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE) };
        assert!(stack::set(Some(game.0)));
        low.raise();
        high.raise();
        assert!(!stack::topmost(low.hwnd) && stack::holds(game.0, &[low.hwnd, high.hwnd]));
        assert_eq!(order(&all), all, "the cover covers ours again");
        // Nothing of all that took the keyboard.
        // SAFETY: a plain query.
        assert_eq!(unsafe { GetForegroundWindow() }, front, "the focus stayed where it was");
        assert!(!stack::set(Some(game.0)), "no change: nothing laid again");
        stack::set(None);
    }

    /// verify-r15 B6: the menu mismatch report's picture of the screen is taken only when no window of another program lies on VRChat's window; ours may.
    #[test]
    fn a_report_s_picture_is_never_taken_through_another_program() {
        let _alone = windows_alone();
        let client = vec2(320.0, 180.0);
        let at = off_screen(client);
        let game = StandIn::new(at, client);
        let rect = egui::Rect::from_min_size(pos2(at.x as f32, at.y as f32), client);
        assert!(!stack::covered(game.0, rect), "nothing on it");
        let mut ours = clear_window();
        ours.place(at, 255);
        assert!(!stack::covered(game.0, rect), "ours on it: the report shows them");
        // Another program's window on a part of it.
        let cover = StandIn::new(POINT { x: at.x + 200, y: at.y + 100 }, vec2(300.0, 200.0));
        assert!(stack::covered(game.0, rect), "a browser over part of VRChat");
        // Beside it, touching nothing.
        // SAFETY: the test's own window.
        let _ = unsafe { SetWindowPos(cover.0, None, at.x + 400, at.y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER) };
        assert!(!stack::covered(game.0, rect), "beside it: nothing of it in the picture");
    }

    /// With nothing ordinary over VRChat's window (the person clicked the desktop's background, or a window on another screen that lies lower), ours lie on top
    /// of the ordinary windows, which is right above VRChat, and still under every topmost window.
    #[test]
    fn with_nothing_over_vrchat_ours_lie_right_above_it() {
        let _alone = windows_alone();
        let client = vec2(320.0, 180.0);
        let at = off_screen(client);
        let game = StandIn::new(at, client);
        stack::set(Some(game.0));
        let mut ours = clear_window();
        ours.place(at, 255);
        assert!(!stack::topmost(ours.hwnd));
        assert!(stack::holds(game.0, &[ours.hwnd]), "right above VRChat");
        stack::set(None);
    }
}
