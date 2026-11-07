//! A fake VRChat, to run the whole bridge without the real game and without touching the desktop of the person who plays on
//! this computer: a small window of its own (real: found, captured and closed through the real calls, off-screen and never
//! activated), a stand-in OSC listener (real UDP), and a log writer in VRChat's format (read back by the real
//! `LogWatcher`). What it cannot do for real is send keyboard and mouse input: it records them. Whether the fake window "has
//! the foreground" is a switch for the same reason, off by default, when the real check on the real window says no.
//!
//! `orbiters-desktop --bridge-lab` runs it with the real MCP listener and the real keeper, behind a stand-in for Orbiters'
//! `features/mine`, so that any MCP client can drive it: `scripts/bridge-lab.mjs` does, with the official SDK.

use super::game::{CaptureSeen, Failure, Find, Game, GameWindow, LaunchPlan, OscArg, PictureFiles, ReconstructionSeen, Seen, Shot, WorldCard};
use super::input::{Input, MouseButton};
use orbiters_core::settings::{Photo3dLength, PictureMode};
use orbiters_core::vrchat_log::LogWatcher;
use std::collections::HashMap;
use std::io::Write;
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The fake game's account.
pub const LAB_USER: &str = "usr_11111111-2222-4333-8444-555555555555";
pub const LAB_WORLD_A: &str = "wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b";
pub const LAB_WORLD_B: &str = "wrld_7281543e-6888-4676-896a-fdd090552fc4";
/// Where the fake menu's camera icon is, in window pixels.
pub const CAMERA_ICON: (u32, u32) = (150, 150);
/// An avatar this lab's account may not wear.
pub const AVATAR_NOT_ALLOWED: &str = "avtr_00000000-0000-4000-8000-000000000000";

/// What the fake game does when asked, for tests of its failures.
#[derive(Clone, Debug)]
pub struct Behaviour {
    /// How long a world takes to load.
    pub load: Duration,
    /// The game ends up in this instance instead of the one asked for (a public one, say).
    pub land_in: Option<String>,
    /// The game never reaches its world.
    pub never_arrives: bool,
    /// `launch` fails.
    pub launch_fails: bool,
    /// The worn avatar has the capture kit.
    pub kit: bool,
    /// VRChat's tree says so; off, it only shows in what was recorded.
    pub tree_says_kit: bool,
    /// How long after the world's load the player exists: VRChat says it is joining long before.
    pub player_after: Duration,
    /// How long a recording takes.
    pub record: Duration,
    /// How long a reconstruction takes.
    pub reconstruct: Duration,
    /// It ends in this failure.
    pub reconstruct_fails: Option<String>,
    /// Asking Windows for the foreground works. Off, the real check on the fake window says no, as it is never activated.
    pub focus_works: bool,
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            load: Duration::from_millis(200),
            land_in: None,
            never_arrives: false,
            launch_fails: false,
            kit: true,
            tree_says_kit: true,
            player_after: Duration::ZERO,
            record: Duration::from_millis(300),
            reconstruct: Duration::from_millis(200),
            reconstruct_fails: None,
            focus_works: false,
        }
    }
}

#[derive(Default)]
struct State {
    behaviour: Behaviour,
    window: Option<LabWindow>,
    pid: Option<u32>,
    joining: Option<(Instant, String, String)>,
    /// A VRChat somebody else started: no window of ours.
    foreign: bool,
    foreground: bool,
    /// When the player exists in the world it is loading.
    player_from: Option<Instant>,
    /// Where the pointer is.
    pointer: Option<(u32, u32)>,
    injected: Vec<Input>,
    worn: Option<String>,
    released: Vec<Input>,
    menu_open: bool,
    camera_open: bool,
    pose_known: bool,
    capture_state: String,
    session: Option<PathBuf>,
    recording_until: Option<Instant>,
    pictures: HashMap<PathBuf, (u64, u64)>,
    queue: Vec<PathBuf>,
    reconstructing: Option<(PathBuf, Instant)>,
    done: Vec<PathBuf>,
    hold: bool,
    instances: Vec<String>,
    launches: Vec<LaunchPlan>,
    log_serial: u32,
    closes: u32,
}

/// One OSC message the stand-in received.
#[derive(Clone, Debug, PartialEq)]
pub struct Heard {
    pub address: String,
    pub value: f32,
}

pub struct LabGame {
    /// Called as input goes out, for what must be true by then.
    #[allow(clippy::type_complexity)]
    pub(crate) sending: Mutex<Option<Box<dyn Fn() + Send>>>,
    dir: PathBuf,
    state: Mutex<State>,
    watcher: Mutex<LogWatcher>,
    log: Mutex<Option<std::fs::File>>,
    osc_out: UdpSocket,
    osc_in: SocketAddr,
    heard: Arc<Mutex<Vec<(Heard, Instant)>>>,
    capture_in_child: Option<PathBuf>,
    /// A port outside this process that OSC goes to as well, and that the guard is told about: it hears what happens after
    /// this process is killed.
    osc_also: Option<SocketAddr>,
    /// The title of its window: its own, as tests run side by side in one process.
    title: String,
}

impl LabGame {
    /// A fake game writing in `dir`. `capture_in_child`: as the real game, a screenshot through `--bridge-shot` of this program.
    #[cfg(test)]
    pub fn new(dir: &Path, capture_in_child: Option<PathBuf>) -> Arc<Self> {
        Self::with_osc_also(dir, capture_in_child, None)
    }

    pub fn with_osc_also(dir: &Path, capture_in_child: Option<PathBuf>, osc_also: Option<SocketAddr>) -> Arc<Self> {
        let _ = std::fs::create_dir_all(dir);
        let listener = UdpSocket::bind(("127.0.0.1", 0)).expect("a port for OSC");
        let osc_in = listener.local_addr().expect("its address");
        let heard: Arc<Mutex<Vec<(Heard, Instant)>>> = Arc::default();
        let hearing = heard.clone();
        let _ = std::thread::Builder::new().name("lab-osc".into()).spawn(move || {
            let mut buffer = [0u8; 2048];
            while let Ok(n) = listener.recv(&mut buffer) {
                let Ok((_, rosc::OscPacket::Message(message))) = rosc::decoder::decode_udp(&buffer[..n]) else { continue };
                let value = match message.args.first() {
                    Some(rosc::OscType::Int(i)) => *i as f32,
                    Some(rosc::OscType::Float(f)) => *f,
                    _ => continue,
                };
                hearing.lock().unwrap().push((Heard { address: message.addr, value }, Instant::now()));
            }
        });
        Arc::new(Self {
            sending: Mutex::default(),
            dir: dir.to_path_buf(),
            state: Mutex::new(State { capture_state: "idle".into(), ..Default::default() }),
            watcher: Mutex::new(LogWatcher::new(Some(dir.to_path_buf()))),
            log: Mutex::new(None),
            osc_out: UdpSocket::bind(("127.0.0.1", 0)).expect("a port to send from"),
            osc_in,
            heard,
            capture_in_child,
            osc_also,
            title: format!("{} {}", LabWindow::TITLE, uuid::Uuid::new_v4().simple()),
        })
    }

    pub fn set_behaviour(&self, change: impl FnOnce(&mut Behaviour)) {
        change(&mut self.state.lock().unwrap().behaviour);
    }

    /// The switch for the foreground: the real desktop's is never taken from the person.
    pub fn set_foreground(&self, foreground: bool) {
        self.state.lock().unwrap().foreground = foreground;
    }

    /// A VRChat that is somebody else's: running, in a world, not started by the bridge.
    #[cfg(test)]
    pub fn set_foreign_game(&self, instance: Option<&str>) {
        let mut state = self.state.lock().unwrap();
        state.foreign = instance.is_some();
        state.pid = instance.map(|_| 4242);
        drop(state);
        if let Some(instance) = instance {
            self.new_log();
            self.write_join(LAB_WORLD_A, instance, "Someone's World");
        }
    }

    /// What the stand-in OSC listener heard, oldest first, with when.
    pub fn heard(&self) -> Vec<(Heard, Instant)> {
        self.heard.lock().unwrap().clone()
    }

    /// The last value `address` got.
    pub fn last_value(&self, address: &str) -> Option<f32> {
        self.heard().iter().rev().find(|(h, _)| h.address == address).map(|(h, _)| h.value)
    }

    pub fn injected(&self) -> Vec<Input> {
        self.state.lock().unwrap().injected.clone()
    }

    #[cfg(test)]
    pub fn released(&self) -> Vec<Input> {
        self.state.lock().unwrap().released.clone()
    }

    pub fn launches(&self) -> Vec<LaunchPlan> {
        self.state.lock().unwrap().launches.clone()
    }

    #[cfg(test)]
    pub fn instances(&self) -> Vec<String> {
        self.state.lock().unwrap().instances.clone()
    }

    #[cfg(test)]
    pub fn closes(&self) -> u32 {
        self.state.lock().unwrap().closes
    }

    #[cfg(test)]
    pub fn reconstructions_held(&self) -> bool {
        self.state.lock().unwrap().hold
    }

    /// Starts a new log file, as each start of VRChat does.
    fn new_log(&self) {
        let mut state = self.state.lock().unwrap();
        state.log_serial += 1;
        let name = format!("output_log_2026-11-05_12-{:02}-{:02}.txt", state.log_serial / 60, state.log_serial % 60);
        drop(state);
        *self.log.lock().unwrap() = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join(name)).ok();
    }

    /// A line in VRChat's format.
    fn write_line(&self, message: &str) {
        if let Some(file) = self.log.lock().unwrap().as_mut() {
            let _ = writeln!(file, "2026.11.05 12:00:00 Debug      -  {message}");
            let _ = file.flush();
        }
    }

    fn write_join(&self, world: &str, instance: &str, name: &str) {
        self.write_line(&format!("[Behaviour] Joining {world}:{instance}"));
        self.write_line(&format!("[Behaviour] Joining or Creating Room: {name}"));
    }

    /// Lets time pass: what was due happens.
    fn advance(&self) {
        let now = Instant::now();
        let mut state = self.state.lock().unwrap();
        // The window closed: the game is gone.
        if state.window.as_ref().is_some_and(|w| w.closed.load(Ordering::Relaxed)) {
            state.window = None;
            state.pid = None;
            state.joining = None;
            state.menu_open = false;
            state.camera_open = false;
            state.pose_known = false;
            state.foreground = false;
            drop(state);
            self.write_line("[Behaviour] OnLeftRoom");
            state = self.state.lock().unwrap();
        }
        if let Some((at, world, instance)) = state.joining.clone() {
            if now >= at {
                state.joining = None;
                let instance = state.behaviour.land_in.clone().unwrap_or(instance);
                drop(state);
                self.write_join(&world, &instance, if world == LAB_WORLD_A { "Lab World A" } else { "Lab World B" }); // i18n: not shown
                state = self.state.lock().unwrap();
            }
        }
        if state.recording_until.is_some_and(|t| now >= t) {
            state.recording_until = None;
            state.capture_state = "idle".into();
            let kit = state.behaviour.kit;
            if let Some(session) = state.session.clone() {
                state.pictures.insert(session.clone(), (240, if kit { 40 } else { 0 }));
                if kit {
                    state.queue.push(session);
                }
            }
        }
        if state.reconstructing.as_ref().is_some_and(|(_, until)| now >= *until) {
            if let Some((session, _)) = state.reconstructing.take() {
                state.done.push(session);
            }
        }
        // Reconstructions wait for VRChat, as the app has them.
        if state.reconstructing.is_none() && !state.hold && state.pid.is_none() && !state.queue.is_empty() {
            let session = state.queue.remove(0);
            let reconstruct = state.behaviour.reconstruct;
            state.reconstructing = Some((session, now + reconstruct));
        }
    }
}

impl Game for LabGame {
    fn seen(&self) -> Seen {
        self.advance();
        let mut watcher = self.watcher.lock().unwrap();
        watcher.poll();
        let world = watcher.state.clone();
        drop(watcher);
        let state = self.state.lock().unwrap();
        let running = state.pid.is_some();
        let walking = ["/input/MoveForward", "/input/MoveBackward", "/input/MoveLeft", "/input/MoveRight"].iter().any(|a| self.last_value(a) == Some(1.0));
        Seen {
            running,
            pid: state.pid,
            log_found: true,
            world_id: world.world_id.clone().filter(|_| running),
            world_name: world.world_name.clone().filter(|_| running),
            instance: world.instance.clone().filter(|_| running),
            in_world: world.in_world && running,
            local_player: (running && state.player_from.is_none_or(|from| Instant::now() >= from)).then(|| "Lab Agent".to_string()), // i18n: not shown
            avatar_name: running.then(|| state.worn.as_ref().map_or_else(|| "Lab Avatar".to_string(), |id| format!("Lab Avatar {id}"))), // i18n: not shown
            others: 0,
            vrchat_user: Some((LAB_USER.to_string(), "Lab Agent".to_string())), // i18n: not shown
            desktop_mode: running.then_some(true),
            osc_found: running,
            camera_pose_known: state.pose_known,
            speed: running.then_some(if walking { 1.5 } else { 0.0 }),
            capture: CaptureSeen { state: state.capture_state.clone(), session: state.session.clone(), frames: 0, keyframes: 0, no_strip: None, error: None },
            reconstruction: state.reconstructing.as_ref().map(|(session, _)| ReconstructionSeen {
                session: session.clone(),
                stage: "training".into(),
                progress: 0.5,
            }),
            queue: state.queue.clone(),
            queue_waits_for: "once VRChat closes or the headset has been off a minute".into(), // i18n: not shown
            steamvr: false,
        }
    }

    fn camera_open(&self) -> Option<bool> {
        Some(self.state.lock().unwrap().camera_open)
    }

    fn create_private_instance(&self, world: &str, region: &str) -> Result<String, Failure> {
        let mut state = self.state.lock().unwrap();
        let instance = format!("{}~private({LAB_USER})~region({region})~nonce(lab)", 10_000 + state.instances.len());
        state.instances.push(format!("{world}:{instance}"));
        Ok(instance)
    }

    fn find_worlds(&self, find: &Find, _page: usize) -> Result<Vec<WorldCard>, Failure> {
        let card = |id: &str, name: &str| WorldCard { id: id.to_string(), name: name.to_string(), author: None, occupants: None, last_visit: None };
        Ok(match find {
            Find::Search(query) if !query.to_lowercase().contains("lab") => Vec::new(),
            _ => vec![card(LAB_WORLD_A, "Lab World A"), card(LAB_WORLD_B, "Lab World B")], // i18n: not shown
        })
    }

    fn launch(&self, plan: &LaunchPlan) -> Result<(), Failure> {
        let mut state = self.state.lock().unwrap();
        if state.behaviour.launch_fails {
            return Err(Failure::new("start_failed", "VRChat did not start: the lab says no.", "Ask the person to look at the computer.")); // i18n: not shown (an AI client reads it in the tool's answer; the lab is a fake VRChat for tests)
        }
        state.launches.push(plan.clone());
        let window = LabWindow::open(&self.title).ok_or_else(|| Failure::new("start_failed", "The fake window did not open.", "This is the lab."))?; // i18n: not shown
        state.window = Some(window);
        state.pid = Some(std::process::id());
        state.player_from = Some(Instant::now() + state.behaviour.load + state.behaviour.player_after);
        state.joining = (!state.behaviour.never_arrives).then(|| (Instant::now() + state.behaviour.load, plan.world_id.clone(), plan.instance.clone()));
        drop(state);
        self.new_log();
        // Like VRChat, its log names the start's own arguments first.
        self.write_line("Launching with args"); // i18n: not shown
        Ok(())
    }

    fn window(&self, pid: u32) -> Option<GameWindow> {
        if self.state.lock().unwrap().foreign {
            return None;
        }
        super::win::window_of(pid, Some(&self.title))
    }

    fn is_foreground(&self, window: &GameWindow) -> bool {
        self.state.lock().unwrap().foreground || super::win::is_foreground(window.handle)
    }

    fn bring_forward(&self, _window: &GameWindow) {
        // Never on the person's desktop: a stand-in for Windows saying yes, when the test says it does.
        let mut state = self.state.lock().unwrap();
        state.foreground = state.behaviour.focus_works;
    }

    fn close(&self, window: &GameWindow) -> Result<(), Failure> {
        self.state.lock().unwrap().closes += 1;
        super::win::close_window(window.handle)
    }

    fn capture(&self, window: &GameWindow, max_width: u32) -> Result<Shot, Failure> {
        match &self.capture_in_child {
            Some(exe) => super::capture::shot_in_child(exe, window.handle, max_width),
            None => super::capture::shot(window.handle, max_width).map_err(|e| Failure::new("screenshot_failed", e, "This is the lab.")), // i18n: not shown
        }
    }

    fn osc(&self, address: &str, value: OscArg) -> Result<(), Failure> {
        let args = vec![match value {
            OscArg::Int(i) => rosc::OscType::Int(i),
            OscArg::Float(f) => rosc::OscType::Float(f),
        }];
        let bytes = rosc::encoder::encode(&rosc::OscPacket::Message(rosc::OscMessage { addr: address.to_string(), args })).expect("an OSC message encodes");
        self.osc_out.send_to(&bytes, self.osc_in).map(drop).map_err(|e| Failure::new("osc_failed", e.to_string(), "This is the lab."))?; // i18n: not shown
        if let Some(also) = self.osc_also {
            let _ = self.osc_out.send_to(&bytes, also);
        }
        // VRChat says where its camera is when it moves.
        if address.starts_with("/input/Look") || address.starts_with("/input/Move") {
            let mut state = self.state.lock().unwrap();
            state.pose_known = state.pose_known || state.camera_open;
        }
        Ok(())
    }

    fn wear(&self, avatar_id: &str) -> Result<(), Failure> {
        let mut state = self.state.lock().unwrap();
        // VRChat only changes to an avatar this account may wear, and says nothing when it may not.
        if avatar_id != AVATAR_NOT_ALLOWED {
            state.worn = Some(avatar_id.to_string());
            state.behaviour.kit = true;
        }
        Ok(())
    }

    fn osc_target(&self) -> Option<SocketAddr> {
        Some(self.osc_also.unwrap_or(self.osc_in))
    }

    fn capture_kit(&self) -> Option<bool> {
        let state = self.state.lock().unwrap();
        state.behaviour.tree_says_kit.then_some(state.behaviour.kit)
    }

    fn inject(&self, window: &GameWindow, input: &[Input]) -> Result<(), Failure> {
        if let Some(sending) = self.sending.lock().unwrap().as_ref() {
            sending();
        }
        let mut state = self.state.lock().unwrap();
        if !state.foreground && !super::win::is_foreground(window.handle) {
            return Err(Failure::new("not_foreground", "VRChat is not the foreground window, so no keyboard or mouse input was sent.", "Call focus_game.")); // i18n: not shown (an AI client reads it in the tool's answer; the lab is a fake VRChat for tests)
        }
        // Recorded, never sent: this computer's input belongs to the person at it.
        for event in input {
            state.injected.push(*event);
            match *event {
                Input::Key { scan: 0x01, down: true, .. } => {
                    state.menu_open = !state.menu_open;
                    LabWindow::show_menu(window, state.menu_open);
                }
                Input::MoveTo { x, y } => state.pointer = Some((x, y)),
                Input::Button { button: MouseButton::Left, down: true } => {
                    let near = state.pointer.is_some_and(|(x, y): (u32, u32)| x.abs_diff(CAMERA_ICON.0) < 40 && y.abs_diff(CAMERA_ICON.1) < 40);
                    if state.menu_open && near {
                        state.camera_open = true;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn release(&self, input: &[Input]) {
        self.state.lock().unwrap().released.extend_from_slice(input);
    }

    fn take_picture(&self, length: Photo3dLength, _mode: PictureMode) -> Result<(), Failure> {
        let mut state = self.state.lock().unwrap();
        if !state.camera_open {
            return Err(Failure::new("camera_closed", "Open the camera before", "This is the lab.")); // i18n: not shown (an AI client reads it in the tool's answer; the lab is a fake VRChat for tests)
        }
        let number = state.pictures.len() + state.session.is_some() as usize;
        let session = self.dir.join(format!("session-{number}-{length:?}"));
        state.session = Some(session);
        state.capture_state = "recording".into();
        let record = state.behaviour.record;
        state.recording_until = Some(Instant::now() + record);
        Ok(())
    }

    fn stop_picture(&self) {
        let mut state = self.state.lock().unwrap();
        state.recording_until = None;
        state.capture_state = "idle".into();
    }

    fn picture_files(&self, session: &Path) -> PictureFiles {
        let state = self.state.lock().unwrap();
        let (frames, keyframes) = state.pictures.get(session).copied().unwrap_or_default();
        let done = state.done.iter().any(|d| d == session);
        let failed = state.behaviour.reconstruct_fails.clone();
        PictureFiles { frames, keyframes, reconstructed: done && failed.is_none(), failed: failed.filter(|_| done) }
    }

    fn hold_reconstructions(&self, hold: bool) {
        self.state.lock().unwrap().hold = hold;
    }
}

/// The fake game's window, on a thread of its own with its own message loop. Never activated, and off every screen: the
/// person at this computer does not see or lose anything to it.
pub struct LabWindow {
    pub closed: Arc<AtomicBool>,
}

impl Default for LabWindow {
    fn default() -> Self {
        Self { closed: Arc::new(AtomicBool::new(true)) }
    }
}

impl LabWindow {
    pub const TITLE: &'static str = "VRChat lab"; // i18n: not shown

    /// The fake Quick Menu with its camera icon: what a model looking at the picture finds and clicks.
    #[cfg(windows)]
    fn show_menu(window: &GameWindow, open: bool) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Gdi::{InvalidateRect, UpdateWindow};
        window::MENU_OPEN.store(open, Ordering::Relaxed);
        // SAFETY: asks a window of this process to paint again; one gone meanwhile only makes this fail.
        unsafe {
            let handle = HWND(window.handle as *mut _);
            let _ = InvalidateRect(Some(handle), None, true);
            // Painted before the key's call answers: a screenshot right after it shows the menu.
            let _ = UpdateWindow(handle);
        }
    }

    #[cfg(not(windows))]
    fn show_menu(_window: &GameWindow, _open: bool) {}

    #[cfg(not(windows))]
    pub fn open(_title: &str) -> Option<Self> {
        None
    }

    #[cfg(windows)]
    pub fn open(title: &str) -> Option<Self> {
        let closed = Arc::new(AtomicBool::new(false));
        let ending = closed.clone();
        let title = title.to_string();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let faint = title.contains("faint");
        std::thread::Builder::new().name("lab-window".into()).spawn(move || window::run(ready_tx, ending, &title, faint)).ok()?;
        ready_rx.recv_timeout(Duration::from_secs(5)).ok().flatten()?;
        Some(Self { closed })
    }
}

#[cfg(windows)]
mod window {
    use super::*;
    use windows::core::w;
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, SetBkMode, SetTextColor, DT_CENTER, DT_SINGLELINE, DT_VCENTER, PAINTSTRUCT,
        TRANSPARENT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetMessageW, PostQuitMessage, RegisterClassW, SetLayeredWindowAttributes, ShowWindow,
        TranslateMessage, LWA_ALPHA, MSG, SW_SHOWNOACTIVATE, WM_DESTROY, WM_PAINT, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_EX_TRANSPARENT, WS_OVERLAPPEDWINDOW, WS_POPUP,
    };

    pub(super) static MENU_OPEN: AtomicBool = AtomicBool::new(false);

    /// Green above, magenta below: a screenshot can be checked for being this window, the right way up. With the menu
    /// open, a grey panel at the top left, and on it a camera button at [`CAMERA_ICON`].
    unsafe extern "system" fn procedure(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // SAFETY: Windows' own calls for a window of this thread, with what it passed.
        unsafe {
            match message {
                WM_PAINT => {
                    let mut paint = PAINTSTRUCT::default();
                    let dc = BeginPaint(window, &mut paint);
                    let mut client = RECT::default();
                    let _ = GetClientRect(window, &mut client);
                    let middle = (client.top + client.bottom) / 2;
                    for (rect, color) in [(RECT { bottom: middle, ..client }, 0x0000_C800u32), (RECT { top: middle, ..client }, 0x00C8_00C8u32)] {
                        let brush = CreateSolidBrush(COLORREF(color));
                        FillRect(dc, &rect, brush);
                        let _ = DeleteObject(brush.into());
                    }
                    if MENU_OPEN.load(Ordering::Relaxed) {
                        let (x, y) = (CAMERA_ICON.0 as i32, CAMERA_ICON.1 as i32);
                        let panel = RECT { left: 20, top: 20, right: 300, bottom: 240 };
                        let button = RECT { left: x - 40, top: y - 30, right: x + 40, bottom: y + 30 };
                        for (rect, color) in [(panel, 0x0030_3030u32), (button, 0x00E0_E0E0u32)] {
                            let brush = CreateSolidBrush(COLORREF(color));
                            FillRect(dc, &rect, brush);
                            let _ = DeleteObject(brush.into());
                        }
                        let _ = SetBkMode(dc, TRANSPARENT);
                        let mut text: Vec<u16> = "Camera".encode_utf16().collect(); // i18n: not shown
                        let mut area = button;
                        let _ = SetTextColor(dc, COLORREF(0x0000_0000));
                        DrawTextW(dc, &mut text, &mut area, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                        let mut title: Vec<u16> = "Quick Menu".encode_utf16().collect(); // i18n: not shown
                        let mut area = RECT { left: 20, top: 24, right: 300, bottom: 60 };
                        let _ = SetTextColor(dc, COLORREF(0x00FF_FFFF));
                        DrawTextW(dc, &mut title, &mut area, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                    }
                    let _ = EndPaint(window, &paint);
                    LRESULT(0)
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(window, message, wparam, lparam),
            }
        }
    }

    pub(super) fn run(ready: std::sync::mpsc::Sender<Option<()>>, closed: Arc<AtomicBool>, title: &str, faint: bool) {
        // SAFETY: a window of this thread, its class registered once per process (a second time only fails), its messages
        // pumped here until it goes.
        unsafe {
            let class = w!("OrbitersBridgeLab");
            let instance = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(procedure), hInstance: instance, lpszClassName: class, ..Default::default() });
            let title = windows::core::HSTRING::from(title);
            // Off every screen, as a tool window; or (`faint`) on the screen but as good as invisible, and clicks go through it:
            // Windows Graphics Capture takes neither tool windows nor windows off every screen.
            let (ex_style, style, x, y) = if faint {
                (WS_EX_LAYERED | WS_EX_TRANSPARENT, WS_OVERLAPPEDWINDOW, 0, 0)
            } else {
                (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW, WS_POPUP, -6000, -6000)
            };
            let Ok(window) = CreateWindowExW(ex_style, class, &title, style, x, y, 640, 360, None, None, Some(instance), None) else {
                let _ = ready.send(None);
                closed.store(true, Ordering::Relaxed);
                return;
            };
            if faint {
                let _ = SetLayeredWindowAttributes(window, COLORREF(0), 1, LWA_ALPHA);
            }
            let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
            let _ = ready.send(Some(()));
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        closed.store(true, Ordering::Relaxed);
    }
}

/// The stand-in for Orbiters: `GET /features/mine?names=…` with the lab's credential, saying what `owner` says. With
/// `reachable` off it closes connections without an answer.
pub struct StandIn {
    pub address: String,
    pub owner: Arc<AtomicBool>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub reachable: Arc<AtomicBool>,
}

impl StandIn {
    pub fn start() -> Self {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a port");
        let address = format!("http://{}", listener.local_addr().expect("its address"));
        let owner = Arc::new(AtomicBool::new(true));
        let reachable = Arc::new(AtomicBool::new(true));
        let (is_owner, is_reachable) = (owner.clone(), reachable.clone());
        let _ = std::thread::Builder::new().name("lab-orbiters".into()).spawn(move || {
            use std::io::Read;
            for stream in listener.incoming().flatten() {
                let (is_owner, is_reachable) = (is_owner.clone(), is_reachable.clone());
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 2048];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut buffer) {
                            Ok(n) if n > 0 => request.extend_from_slice(&buffer[..n]),
                            _ => return,
                        }
                    }
                    if !is_reachable.load(Ordering::Relaxed) {
                        return;
                    }
                    let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
                    let (status, body) = if !request.contains("authorization: bearer orbit-lab") {
                        ("401 Unauthorized", r#"{"message":"expired"}"#.to_string()) // i18n: not shown
                    } else if request.starts_with("get /features/mine?names=n4n0-vrchat-bridge ") {
                        ("200 OK", format!(r#"{{"features":{{"n4n0-vrchat-bridge":{}}}}}"#, is_owner.load(Ordering::Relaxed)))
                    } else {
                        ("404 Not Found", r#"{"message":"no"}"#.to_string()) // i18n: not shown
                    };
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        Self { address, owner, reachable }
    }
}

/// The keeper's view of the lab: signed in, on the stand-in server, with Settings' switch here.
pub struct LabFlag {
    pub api: orbiters_core::account::Api,
    pub signed_in: AtomicBool,
    pub enabled: AtomicBool,
}

impl super::keeper::Flag for LabFlag {
    fn identity(&self) -> Option<String> {
        self.signed_in.load(Ordering::Relaxed).then(|| self.api.url(""))
    }

    fn ask(&self) -> anyhow::Result<bool> {
        self.api.member_feature("orbit-lab", orbiters_core::features::VRCHAT_BRIDGE)
    }

    fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    fn turn_off(&self) {
        self.enabled.store(false, Ordering::Relaxed);
    }
}

/// `--bridge-lab [dir]`: the real listener and keeper around the fake game, with the bridge switched on and the stand-in
/// saying owner. Prints a line of JSON with its address and token, then takes lines on standard input and answers each with
/// a line of JSON: `owner on|off`, `signed-in on|off`, `foreground on|off`, `person`, `hotkey`, `stop`, `heard`, `injected`,
/// `launches`, `quit`.
pub fn run(args: &[String]) -> anyhow::Result<()> {
    use std::io::BufRead;
    let dir = args.first().map_or_else(|| std::env::temp_dir().join(format!("orbiters-bridge-lab-{}", std::process::id())), PathBuf::from);
    let exe = std::env::current_exe()?;
    let osc_also = args.get(1).and_then(|port| port.parse::<u16>().ok()).map(|port| SocketAddr::from(([127, 0, 0, 1], port)));
    let game = LabGame::with_osc_also(&dir, Some(exe.clone()), osc_also);
    let timing = super::Timing { instance_every: Duration::from_secs(1), start_limit: Duration::from_secs(20) };
    let bridge = super::Bridge::with_timing(game.clone(), Some(exe), timing);
    let stand_in = StandIn::start();
    let flag = Arc::new(LabFlag { api: orbiters_core::account::Api::at(&stand_in.address), signed_in: AtomicBool::new(true), enabled: AtomicBool::new(true) });
    let keeper = super::keeper::Keeper::new(
        bridge.clone(),
        flag.clone(),
        dir.join("keys.dat"),
        Duration::from_secs(1),
        // Long enough for a loaded machine: the real one is half an hour, and a server's no ends it at once all the same.
        Duration::from_secs(30),
        Box::new(|_| {}),
        Box::new(|_| Some(super::person::Listener::none())),
    );
    keeper.start();
    let begin = Instant::now();
    while keeper.port().is_none() && begin.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let keys = super::keeper::Keys::load_or_create(&dir.join("keys.dat"));
    println!(
        "{}",
        serde_json::json!({ "address": format!("http://127.0.0.1:{}/mcp", keeper.port().unwrap_or(0)), "token": keys.token, "dir": dir, "world_a": LAB_WORLD_A, "world_b": LAB_WORLD_B, "camera_icon": CAMERA_ICON, "pid": std::process::id() })
    );
    let _ = std::io::stdout().flush();
    let on = |word: Option<&str>| word == Some("on");
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let mut words = line.split(' ');
        let answer = match words.next().unwrap_or_default() {
            "owner" => {
                stand_in.owner.store(on(words.next()), Ordering::Relaxed);
                serde_json::json!({ "ok": true })
            }
            "signed-in" => {
                flag.signed_in.store(on(words.next()), Ordering::Relaxed);
                serde_json::json!({ "ok": true })
            }
            "foreground" => {
                let foreground = on(words.next());
                game.set_foreground(foreground);
                game.set_behaviour(|b| b.focus_works = foreground);
                serde_json::json!({ "ok": true })
            }
            "person" => {
                bridge.heard(super::person::Heard::PersonInput);
                serde_json::json!({ "ok": true })
            }
            "hotkey" => {
                bridge.heard(super::person::Heard::Hotkey);
                serde_json::json!({ "ok": true })
            }
            "resume" => {
                bridge.hold(false);
                serde_json::json!({ "ok": true })
            }
            "stop" => {
                keeper.command(super::Command::Stop);
                serde_json::json!({ "ok": true, "port": keeper.port() })
            }
            "port" => serde_json::json!({ "port": keeper.port() }),
            "heard" => serde_json::json!(game.heard().iter().map(|(h, _)| serde_json::json!([h.address, h.value])).collect::<Vec<_>>()),
            "injected" => serde_json::json!(game.injected().len()),
            "launches" => serde_json::json!(game.launches().iter().map(LaunchPlan::args).collect::<Vec<_>>()),
            "osc-target" => serde_json::json!(game.osc_target()),
            "quit" => break,
            other => serde_json::json!({ "error": format!("no such command: {other}") }),
        };
        println!("{answer}");
        let _ = std::io::stdout().flush();
    }
    keeper.shutdown();
    Ok(())
}
