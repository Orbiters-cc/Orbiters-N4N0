//! A fake VRChat, to run the whole bridge without the real game and without touching the desktop of the person who plays on
//! this computer: a small window of its own (real: found, captured and closed through the real calls, off-screen and never
//! activated), a stand-in OSC listener (real UDP), and a log writer in VRChat's format (read back by the real
//! `LogWatcher`). What it cannot do for real is send keyboard and mouse input: it records them. Whether the fake window "has
//! the foreground" is a switch for the same reason, off by default, when the real check on the real window says no.
//!
//! `orbiters-desktop --bridge-lab` runs it with the real MCP listener and the real keeper, behind a stand-in for Orbiters'
//! `features/mine`, so that any MCP client can drive it: `scripts/bridge-lab.mjs` does, with the official SDK.

use super::game::{CaptureSeen, DiskSeen, EnvironmentSeen, Failure, Feedback, FeedbackAccount, Publish, PublishSeen, Find, Game, GameWindow, LaunchPlan, OscArg, PictureFiles, ReconstructionSeen, Seen, Shot, WorldCard, WorldPage};
use orbiters_n4n0_core::feedback::{FeedbackAsk, LabelCount, WorldFeedback};
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
    /// How long asking the window to close takes (a busy computer), before it counts as asked.
    pub close_takes: Duration,
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
    /// The profile has no account signed in: the game waits at its sign-in page.
    pub needs_sign_in: bool,
    /// The app is lent to a side session's picture (the main session's game).
    pub lent_away: bool,
    /// The account the game itself signed in with (a side session's profile), as its log says.
    pub account: Option<(String, String)>,
    /// A link handed to the running game shows the world's page, which waits for a click on Join (VRChat, 2026-11-13);
    /// off, it travels at once.
    pub link_opens_page: bool,
    /// VRChat's link pipe is another VRChat's: `send_link` is refused.
    pub link_refused: bool,
    /// `launch` starts a launcher that never brings a VRChat up (Easy Anti-Cheat's error window).
    pub launch_hangs: bool,
    /// VRChat comes up this long after `launch` (Easy Anti-Cheat loading first: 2026-11-15 12:25, its "301" line came before
    /// VRChat.exe).
    pub comes_up_after: Option<Duration>,
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            load: Duration::from_millis(200),
            land_in: None,
            close_takes: Duration::ZERO,
            never_arrives: false,
            launch_fails: false,
            kit: true,
            tree_says_kit: true,
            player_after: Duration::ZERO,
            record: Duration::from_millis(300),
            reconstruct: Duration::from_millis(200),
            reconstruct_fails: None,
            focus_works: false,
            needs_sign_in: false,
            lent_away: false,
            account: None,
            link_opens_page: true,
            link_refused: false,
            launch_hangs: false,
            comes_up_after: None,
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
    /// The running VRChat's command line, as its log lists it.
    args: Option<Vec<String>>,
    /// The person's own VRChat, started beside the lab's (the app follows it then).
    beside: Option<u32>,
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
    /// Where its camera was put (`set_camera_pose`), else where it opened.
    camera_pose: Option<[f32; 6]>,
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
    resizes: u32,
    /// The window's real client size at each click (from Windows, not from what the bridge passed).
    clicked_at_size: Vec<(u32, u32)>,
    /// Links handed to the running game.
    links: Vec<LaunchPlan>,
    /// The world's page a link opened, waiting for Join: world and instance.
    world_page: Option<(String, String)>,
    /// A launch whose VRChat comes up later: when, and its plan.
    coming: Option<(Instant, LaunchPlan)>,
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
        // Started by the person: their own arguments, no launch link of the bridge's.
        state.args = instance.map(|_| vec!["VRChat.exe".to_string(), "--no-vr".to_string()]);
        drop(state);
        if let Some(instance) = instance {
            self.new_log();
            self.write_join(LAB_WORLD_A, instance, "Someone's World");
        }
    }

    /// The person starts their own VRChat beside the lab's: the app follows theirs from then on.
    #[cfg(test)]
    pub fn person_starts_beside(&self) {
        self.state.lock().unwrap().beside = Some(4343);
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

    /// The VR sandbox's camera route reached this VRChat (vr.rs): its camera opens, and says where it is.
    pub fn vr_camera(&self) {
        let mut state = self.state.lock().unwrap();
        if state.pid.is_some() && !state.menu_open {
            state.camera_open = true;
            state.pose_known = true;
        }
    }

    /// The VR laser's trigger: on a world's page (a link sent to the running game), its Join.
    pub fn vr_click(&self) {
        let mut state = self.state.lock().unwrap();
        if let Some((world, instance)) = state.world_page.take() {
            let load = state.behaviour.load;
            state.joining = Some((Instant::now() + load, world, instance));
            state.player_from = Some(Instant::now() + load + state.behaviour.player_after);
        }
    }

    pub fn launches(&self) -> Vec<LaunchPlan> {
        self.state.lock().unwrap().launches.clone()
    }

    /// The links handed to the running game.
    #[cfg(test)]
    pub fn links(&self) -> Vec<LaunchPlan> {
        self.state.lock().unwrap().links.clone()
    }

    /// The world's page a link opened is up, waiting for its Join button.
    #[cfg(test)]
    pub fn world_page_up(&self) -> bool {
        self.state.lock().unwrap().world_page.is_some()
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
    pub fn resizes(&self) -> u32 {
        self.state.lock().unwrap().resizes
    }

    /// The window's client size, from Windows, at each left click.
    #[cfg(test)]
    pub fn clicked_at_size(&self) -> Vec<(u32, u32)> {
        self.state.lock().unwrap().clicked_at_size.clone()
    }

    /// VRChat goes somewhere else by itself (a portal, a home): its log says it joined there.
    #[cfg(test)]
    pub fn moves_to(&self, world: &str, instance: &str) {
        self.write_join(world, instance, if world == LAB_WORLD_A { "Lab World A" } else { "Lab World B" }); // i18n: not shown
    }

    /// Someone resizes the window, as the owner did to profile 1's on 2026-11-15 (the real call, on the fake window).
    #[cfg(test)]
    pub fn resize_by_hand(&self, width: u32, height: u32) {
        let pid = self.state.lock().unwrap().pid;
        if let Some(window) = pid.and_then(|pid| self.window(pid)) {
            super::win::resize_client(window.handle, width, height);
        }
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

    /// The VRChat of a launch comes up: its window, its process, its new log.
    fn come_up(&self, plan: &LaunchPlan) -> Result<(), Failure> {
        let window = LabWindow::open(&self.title).ok_or_else(|| Failure::new("start_failed", "The fake window did not open.", "This is the lab."))?; // i18n: not shown
        let mut state = self.state.lock().unwrap();
        state.window = Some(window);
        state.pid = Some(std::process::id());
        state.args = Some(plan.args());
        state.player_from = Some(Instant::now() + state.behaviour.load + state.behaviour.player_after);
        state.joining = (!state.behaviour.never_arrives).then(|| (Instant::now() + state.behaviour.load, plan.world_id.clone(), plan.instance.clone()));
        drop(state);
        self.new_log();
        // Like VRChat, its log names the start's own arguments first.
        self.write_line("Launching with args"); // i18n: not shown
        Ok(())
    }

    /// Lets time pass: what was due happens.
    fn advance(&self) {
        let due = {
            let mut state = self.state.lock().unwrap();
            if state.coming.as_ref().is_some_and(|(at, _)| Instant::now() >= *at) {
                state.coming.take()
            } else {
                None
            }
        };
        if let Some((_, plan)) = due {
            let _ = self.come_up(&plan);
        }
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

impl LabGame {
    /// The lab's folder: its log, and its pictures' sessions.
    pub fn dir(&self) -> &Path {
        &self.dir
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
            pid: state.beside.or(state.pid),
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
            session_account: state.behaviour.account.clone().filter(|_| running),
            needs_sign_in: running && state.behaviour.needs_sign_in,
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

    fn find_worlds(&self, find: &Find, page: usize) -> Result<WorldPage, Failure> {
        let card = |id: &str, name: &str| WorldCard { id: id.to_string(), name: name.to_string(), author: None, occupants: None, last_visit: None };
        Ok(match (find, page) {
            (Find::Search(query), _) if !query.to_lowercase().contains("lab") => WorldPage::default(),
            // VRChat's Spotlight rows, a world a page, two pages; every list as deep as VRChat's for popular.
            (Find::SpotlightPc | Find::SpotlightCrossPlatform, 0) => WorldPage { worlds: vec![card(LAB_WORLD_A, "Lab World A")], more: true }, // i18n: not shown
            (Find::SpotlightPc | Find::SpotlightCrossPlatform, 1) => WorldPage { worlds: vec![card(LAB_WORLD_B, "Lab World B")], more: false }, // i18n: not shown
            (Find::Popular, p) if p > 0 => WorldPage { worlds: vec![card(LAB_WORLD_B, "Lab World B")], more: true }, // i18n: not shown
            (_, 0) => WorldPage { worlds: vec![card(LAB_WORLD_A, "Lab World A"), card(LAB_WORLD_B, "Lab World B")], more: *find == Find::Popular }, // i18n: not shown
            _ => WorldPage::default(),
        })
    }

    fn launch(&self, plan: &LaunchPlan) -> Result<(), Failure> {
        let mut state = self.state.lock().unwrap();
        if state.behaviour.launch_fails {
            return Err(Failure::new("start_failed", "VRChat did not start: the lab says no.", "Ask the person to look at the computer.")); // i18n: not shown (an AI client reads it in the tool's answer; the lab is a fake VRChat for tests)
        }
        state.launches.push(plan.clone());
        if state.behaviour.launch_hangs {
            return Ok(());
        }
        if let Some(after) = state.behaviour.comes_up_after {
            state.coming = Some((Instant::now() + after, plan.clone()));
            return Ok(());
        }
        drop(state);
        self.come_up(plan)
    }

    fn adopt(&self, _since: Option<std::time::SystemTime>) -> Option<u32> {
        // The lab's game is one VRChat: a side session's own while it runs from a start with a profile and OSC ports.
        let state = self.state.lock().unwrap();
        let plan = state.launches.last()?;
        (plan.profile.is_some_and(|p| p != 0) && plan.osc.is_some()).then_some(state.pid).flatten()
    }

    fn send_link(&self, plan: &LaunchPlan) -> Result<(), Failure> {
        let mut state = self.state.lock().unwrap();
        if state.pid.is_none() || state.behaviour.link_refused {
            return Err(Failure::new("link_pipe_elsewhere", "VRChat's link pipe is another VRChat's.", "It is started in the instance instead.")); // i18n: not shown (the lab)
        }
        state.links.push(plan.clone());
        if state.behaviour.link_opens_page {
            state.world_page = Some((plan.world_id.clone(), plan.instance.clone()));
        } else {
            let load = state.behaviour.load;
            state.joining = Some((Instant::now() + load, plan.world_id.clone(), plan.instance.clone()));
            state.player_from = Some(Instant::now() + load + state.behaviour.player_after);
        }
        Ok(())
    }

    fn runs(&self, pid: u32) -> Option<bool> {
        let state = self.state.lock().unwrap();
        Some(state.pid == Some(pid) || state.beside == Some(pid))
    }

    fn launch_args(&self) -> Option<Vec<String>> {
        let state = self.state.lock().unwrap();
        state.args.clone().filter(|_| state.pid.is_some())
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

    /// The real call on the fake window, which is off every screen and never activated.
    fn resize(&self, window: &GameWindow, width: u32, height: u32) {
        self.state.lock().unwrap().resizes += 1;
        super::win::resize_client(window.handle, width, height);
    }

    fn close(&self, window: &GameWindow) -> Result<(), Failure> {
        let takes = self.state.lock().unwrap().behaviour.close_takes;
        std::thread::sleep(takes);
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

    fn give_back(&self) {
        self.state.lock().unwrap().behaviour.lent_away = false;
    }

    fn lent_away(&self) -> bool {
        self.state.lock().unwrap().behaviour.lent_away
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
                    let size = super::win::window_of(std::process::id(), Some(&self.title)).map(|w| (w.width, w.height));
                    state.clicked_at_size.extend(size);
                    let near = state.pointer.is_some_and(|(x, y): (u32, u32)| x.abs_diff(CAMERA_ICON.0) < 40 && y.abs_diff(CAMERA_ICON.1) < 40);
                    if state.menu_open && near {
                        state.camera_open = true;
                    }
                    // The world's page a link opened: its Join button.
                    if let Some((world, instance)) = state.world_page.take() {
                        let load = state.behaviour.load;
                        state.joining = Some((Instant::now() + load, world, instance));
                        state.player_from = Some(Instant::now() + load + state.behaviour.player_after);
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
        // A folder as a recording leaves one: its keyframes and the rest (the tour's tools measure and free it).
        let _ = std::fs::create_dir_all(session.join("keyframes"));
        let _ = std::fs::write(session.join("keyframes").join("0.qoi"), vec![0u8; 300_000]);
        let _ = std::fs::write(session.join("manifest.json"), b"{}");
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

    fn favorite_groups(&self) -> Result<Vec<(String, String)>, Failure> {
        Ok(vec![("worlds1".into(), "Lab playlist".into())]) // i18n: not shown
    }

    fn camera_pose(&self) -> Option<[f32; 6]> {
        let state = self.state.lock().unwrap();
        state.pose_known.then(|| state.camera_pose.unwrap_or([1.0, 1.5, 2.0, 0.0, 90.0, 0.0]))
    }

    fn set_camera_pose(&self, pose: [f32; 6]) -> Result<(), Failure> {
        self.state.lock().unwrap().camera_pose = Some(pose);
        Ok(())
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
        let mut menus = window::MENU_OPEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if open {
            menus.insert(window.handle);
        } else {
            menus.remove(&window.handle);
        }
        drop(menus);
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
        // A window thread always answers, at once on an idle machine; one running the whole test suite took more than the five
        // seconds this used to allow ("The fake window did not open.").
        ready_rx.recv_timeout(Duration::from_secs(60)).ok().flatten()?;
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

    /// The lab windows whose menu is open, by handle: each test has a window of its own, and they run side by side (one
    /// flag for all of them painted one test's menu into another's screenshots).
    pub(super) static MENU_OPEN: std::sync::Mutex<std::collections::BTreeSet<isize>> = std::sync::Mutex::new(std::collections::BTreeSet::new());

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
                    if MENU_OPEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains(&(window.0 as isize)) {
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
                    // A handle is used again by a later window, which starts with its menu closed.
                    MENU_OPEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&(window.0 as isize));
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
    bridge.set_opener(LabOpener::new(&dir.join("sessions")));
    bridge.set_feedback(LabFeedback::signed_in());
    bridge.set_publish(LabPublish::new(&dir));
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

/// The lab's world feedback: Orbiters as far as the label tools care, answering at once (or never, or refusing), and every
/// ask recorded.
#[derive(Default)]
pub struct LabFeedback {
    pub state: Mutex<LabFeedbackState>,
}

#[derive(Default)]
pub struct LabFeedbackState {
    pub account: Option<FeedbackAccount>,
    /// Orbiters cannot be reached: asks stay pending.
    pub offline: bool,
    /// Orbiters refuses every ask with this sentence.
    pub refuses: Option<String>,
    pub asks: Vec<FeedbackAsk>,
    /// Worlds asked about with show.
    pub shown: Vec<String>,
    /// By world: label id, how many gave it, whether the account did.
    pub labels: HashMap<String, Vec<(String, u32, bool)>>,
    pub justification: HashMap<String, String>,
    pending: HashMap<String, u32>,
    errors: HashMap<String, String>,
}

impl LabFeedback {
    /// Signed in to the development server as `Lab member`.
    pub fn signed_in() -> Arc<Self> {
        let lab = Self::default();
        lab.state.lock().unwrap().account = Some(FeedbackAccount {
            server: "http://127.0.0.1:4100/".into(),
            development: true,
            name: Some("Lab member".into()), // i18n: not shown
            id: Some("7".into()),
        });
        Arc::new(lab)
    }
}

impl Feedback for LabFeedback {
    fn account(&self) -> Option<FeedbackAccount> {
        self.state.lock().unwrap().account.clone()
    }

    fn show(&self, world: &str) {
        self.state.lock().unwrap().shown.push(world.to_string());
    }

    fn feedback(&self, world: &str) -> WorldFeedback {
        let st = self.state.lock().unwrap();
        WorldFeedback {
            world: world.to_string(),
            loading: st.offline && !st.labels.contains_key(world),
            labels: st.labels.get(world).map(|l| l.iter().map(|(id, count, mine)| LabelCount { id: id.clone(), count: *count, mine: *mine }).collect()).unwrap_or_default(),
            justification: st.justification.get(world).cloned(),
            pending: st.pending.get(world).copied().unwrap_or(0),
            error: st.errors.get(world).cloned(),
            ..Default::default()
        }
    }

    fn ask(&self, ask: FeedbackAsk) {
        let mut st = self.state.lock().unwrap();
        st.asks.push(ask.clone());
        let FeedbackAsk::Label { world, label, on, text } = ask else { return };
        st.errors.remove(&world);
        if let Some(refusal) = st.refuses.clone() {
            st.errors.insert(world, refusal);
            return;
        }
        if st.offline {
            *st.pending.entry(world).or_default() += 1;
            return;
        }
        if let Some(text) = text {
            st.justification.insert(world.clone(), text);
        }
        let labels = st.labels.entry(world).or_default();
        match labels.iter_mut().find(|l| l.0 == label) {
            Some(l) if l.2 != on => {
                l.1 = if on { l.1 + 1 } else { l.1.saturating_sub(1) };
                l.2 = on;
            }
            Some(_) => {}
            None if on => labels.push((label, 1, true)),
            None => {}
        }
    }
}

/// The lab's Orbiters for the tour's tools: an environment switch, publishing that answers after a moment (or fails), and
/// pictures' files deleted for real in the lab's own folder.
pub struct LabPublish {
    pub dir: PathBuf,
    pub state: Arc<Mutex<LabPublishState>>,
}

pub struct LabPublishState {
    pub development: bool,
    /// Accounts saved for production and development.
    pub signed_in: (bool, bool),
    pub labels_pending: u32,
    /// Every publish asked: folder, public.
    pub asked: Vec<(PathBuf, bool)>,
    pub published: HashMap<PathBuf, (bool, PublishSeen)>,
    /// Publishing fails with this sentence.
    pub fails: Option<String>,
    pub switches: Vec<bool>,
    pub free_gb: f64,
    /// Orbiters asks to wait this many seconds (429): what is asked is queued, nothing done until [`LabPublish::let_through`].
    pub rate_limited: Option<u64>,
}

impl LabPublish {
    /// In development, signed in to both environments.
    pub fn new(dir: &Path) -> Arc<Self> {
        Arc::new(Self {
            dir: dir.to_path_buf(),
            state: Arc::new(Mutex::new(LabPublishState {
                development: true,
                signed_in: (true, true),
                labels_pending: 0,
                asked: Vec::new(),
                published: HashMap::new(),
                fails: None,
                switches: Vec::new(),
                free_gb: 20.0,
                rate_limited: None,
            })),
        })
    }

    /// Orbiters' wait is over: what was queued is published (as the real queue does by itself).
    #[cfg(test)]
    pub fn let_through(&self) {
        let mut st = self.state.lock().unwrap();
        st.rate_limited = None;
        let website = if st.development { "https://dev.orbiters.cc/" } else { "https://orbiters.cc/" };
        let queued: Vec<PathBuf> = st.published.iter().filter(|(_, (_, s))| matches!(s, PublishSeen::RateLimited { .. })).map(|(p, _)| p.clone()).collect();
        for (n, path) in queued.into_iter().enumerate() {
            let public = st.asked.iter().rev().find(|(p, _)| *p == path).is_some_and(|(_, public)| *public);
            let development = st.development;
            let id = 1000 + n as u64;
            st.published.insert(path, (development, PublishSeen::Done { id, public, link: format!("{website}gallery"), world_id: LAB_WORLD_A.into() }));
        }
    }
}

impl Publish for LabPublish {
    fn environment(&self) -> EnvironmentSeen {
        let st = self.state.lock().unwrap();
        let (server, website) = if st.development { ("http://127.0.0.1:4100/", "https://dev.orbiters.cc/") } else { ("https://api.orbiters.cc/", "https://orbiters.cc/") };
        let here = if st.development { st.signed_in.1 } else { st.signed_in.0 };
        let other = if st.development { st.signed_in.0 } else { st.signed_in.1 };
        EnvironmentSeen {
            development: st.development,
            server: server.into(),
            website: website.into(),
            account: here.then(|| (Some("Lab member".to_string()), Some(if st.development { "7" } else { "70" }.to_string()))), // i18n: not shown
            other_signed_in: other,
            labels_pending: st.labels_pending,
        }
    }

    fn set_environment(&self, development: bool) -> Result<(), Failure> {
        let mut st = self.state.lock().unwrap();
        st.development = development;
        st.switches.push(development);
        Ok(())
    }

    fn sessions_dir(&self) -> PathBuf {
        self.dir.clone()
    }

    fn publish(&self, session: &Path, world_id: &str, _world_name: &str, public: bool) -> Result<(), Failure> {
        let (development, fails) = {
            let mut st = self.state.lock().unwrap();
            st.asked.push((session.to_path_buf(), public));
            let development = st.development;
            if let Some(wait) = st.rate_limited {
                let ahead = st.published.values().filter(|(_, s)| matches!(s, PublishSeen::RateLimited { .. })).count();
                st.published.insert(session.to_path_buf(), (development, PublishSeen::RateLimited { retry_after_s: wait, ahead }));
                return Ok(());
            }
            st.published.insert(session.to_path_buf(), (development, PublishSeen::Running { stage: "sending".into(), sent: 0, total: 10 }));
            (development, st.fails.clone())
        };
        let (state, session, world) = (self.state.clone(), session.to_path_buf(), world_id.to_string());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let mut st = state.lock().unwrap();
            let id = st.asked.len() as u64;
            let website = if development { "https://dev.orbiters.cc/" } else { "https://orbiters.cc/" };
            let seen = match fails {
                Some(message) => PublishSeen::Failed { message, explicit: false },
                None => PublishSeen::Done {
                    id,
                    public,
                    link: if public { format!("{website}maps/{world}/captures/{id}") } else { format!("{website}gallery") },
                    world_id: world,
                },
            };
            st.published.insert(session, (development, seen));
        });
        Ok(())
    }

    fn published(&self, session: &Path) -> PublishSeen {
        let st = self.state.lock().unwrap();
        match st.published.get(session) {
            Some((development, seen)) if *development == st.development => seen.clone(),
            _ => PublishSeen::None,
        }
    }

    fn delete(&self, session: &Path, everything: bool) -> Result<u64, Failure> {
        let before = orbiters_core::bundle::folder_size(session);
        let gone = if everything { std::fs::remove_dir_all(session) } else { std::fs::remove_dir_all(session.join("keyframes")) };
        gone.map_err(|e| Failure::new("not_freed", e.to_string(), "This is the lab."))?; // i18n: not shown
        Ok(before - orbiters_core::bundle::folder_size(session))
    }

    fn size(&self, session: &Path) -> u64 {
        orbiters_core::bundle::folder_size(session)
    }

    fn disks(&self) -> Vec<DiskSeen> {
        vec![DiskSeen { drive: "L:".into(), free_gb: self.state.lock().unwrap().free_gb, holds: vec!["pictures", "app"] }]
    }
}

/// The lab's side sessions: each a [`LabGame`] of its own (its window, log, OSC port), and a computer whose Easy Anti-Cheat,
/// limits, memory and other VRChats a test sets.
pub struct LabOpener {
    root: PathBuf,
    pub games: Mutex<Vec<(String, u32, Arc<LabGame>)>>,
    pub eac: Mutex<super::sessions::Eac>,
    /// Profiles VRChats of the person's run with.
    pub others: Mutex<Vec<u32>>,
    /// VRChats of the person's, by process id.
    pub persons: Mutex<Vec<u32>>,
    pub limit: Mutex<usize>,
    pub memory: Mutex<Option<u64>>,
    pub foreground: Mutex<Option<u32>>,
    /// Profiles whose account is not signed in.
    pub signed_out: Mutex<Vec<u32>>,
    pub forgotten: Mutex<Vec<String>>,
    /// What Easy Anti-Cheat's launcher says of a launch that ended.
    pub launcher_said: Mutex<Option<(i64, String)>>,
    /// The OSC ports whose start's launcher was ended.
    pub launchers_ended: Mutex<Vec<(u16, u16)>>,
    /// New sessions' launches never bring a VRChat up.
    pub launches_hang: Mutex<bool>,
    /// New sessions' VRChats come up this long after their launch.
    pub launches_come_up_after: Mutex<Option<Duration>>,
    /// New sessions' VRChats show the world's page for a link (as VRChat does) instead of travelling at once.
    pub links_open_page: Mutex<bool>,
    /// The VR sandbox, when this lab has one: `None` none set up; `Some(None)` free; `Some(Some(why))` the person's VR in use.
    pub vr: Mutex<Option<Option<String>>>,
    /// The rigs handed out, by session.
    pub rigs: Mutex<Vec<Arc<LabRig>>>,
}

impl LabOpener {
    pub fn new(root: &Path) -> Arc<Self> {
        Arc::new(Self {
            root: root.to_path_buf(),
            games: Mutex::default(),
            eac: Mutex::new(super::sessions::Eac { service: "ABSENT".into(), driver: "ABSENT".into(), vrchat_runs: false, launcher_runs: false, delete_pending: false }),
            others: Mutex::default(),
            persons: Mutex::default(),
            limit: Mutex::new(super::sessions::DEFAULT_LIMIT),
            memory: Mutex::new(Some(16 << 30)),
            foreground: Mutex::new(None),
            signed_out: Mutex::default(),
            forgotten: Mutex::default(),
            launcher_said: Mutex::default(),
            launchers_ended: Mutex::default(),
            launches_hang: Mutex::default(),
            launches_come_up_after: Mutex::default(),
            links_open_page: Mutex::default(),
            vr: Mutex::default(),
            rigs: Mutex::default(),
        })
    }

    /// The game of the session of `profile`.
    pub fn game_of(&self, profile: u32) -> Option<Arc<LabGame>> {
        self.games.lock().unwrap().iter().rev().find(|g| g.1 == profile).map(|g| g.2.clone())
    }
}

impl super::game::Opener for LabOpener {
    fn side(&self, session: &str, profile: u32, _osc: (u16, u16)) -> Result<Arc<dyn Game>, Failure> {
        // A VRChat of this profile whose session was forgotten (a start answered failed) still runs: it is the same game, as
        // the real VRChat is the same process.
        {
            let forgotten = self.forgotten.lock().unwrap().clone();
            let mut games = self.games.lock().unwrap();
            if let Some(left) = games.iter_mut().find(|g| g.1 == profile && forgotten.contains(&g.0) && g.2.state.lock().unwrap().pid.is_some()) {
                left.0 = session.to_string();
                return Ok(left.2.clone());
            }
        }
        let game = LabGame::with_osc_also(&self.root.join(session), None, None);
        let signed_out = self.signed_out.lock().unwrap().contains(&profile);
        let (hang, page) = (*self.launches_hang.lock().unwrap(), *self.links_open_page.lock().unwrap());
        let after = *self.launches_come_up_after.lock().unwrap();
        game.set_behaviour(|b| {
            b.launch_hangs = hang;
            b.comes_up_after = after;
            b.link_opens_page = page;
            b.needs_sign_in = signed_out;
            b.account = (!signed_out).then(|| (format!("usr_{profile:08}-0000-4000-8000-000000000000"), format!("Lab Profile {profile}"))); // i18n: not shown
        });
        self.games.lock().unwrap().push((session.to_string(), profile, game.clone()));
        if let Some(rig) = self.rigs.lock().unwrap().iter().find(|r| r.session == session) {
            *rig.game.lock().unwrap() = Some(game.clone());
        }
        Ok(game)
    }

    fn forget(&self, session: &str) {
        self.forgotten.lock().unwrap().push(session.to_string());
    }

    fn rename(&self, from: &str, to: &str) {
        for game in self.games.lock().unwrap().iter_mut().filter(|g| g.0 == from) {
            game.0 = to.to_string();
        }
    }

    fn vrchat_pids(&self) -> Vec<u32> {
        let mut pids: Vec<u32> = self.games.lock().unwrap().iter().filter_map(|g| g.2.state.lock().unwrap().pid).collect();
        pids.extend(self.persons.lock().unwrap().iter().copied());
        pids
    }

    fn others_profiles(&self) -> Vec<u32> {
        // The person's, and a forgotten session's VRChat still running: no session's mark any more.
        let forgotten = self.forgotten.lock().unwrap().clone();
        let mut profiles = self.others.lock().unwrap().clone();
        let games = self.games.lock().unwrap().clone();
        for game in games.iter().filter(|g| forgotten.contains(&g.0)) {
            // Time passes for it too: a VRChat of a start answered failed comes up late.
            game.2.advance();
        }
        profiles.extend(games.iter().filter(|g| forgotten.contains(&g.0) && g.2.state.lock().unwrap().pid.is_some()).map(|g| g.1));
        profiles
    }

    fn eac(&self) -> super::sessions::Eac {
        self.eac.lock().unwrap().clone()
    }

    fn memory_free(&self) -> Option<u64> {
        *self.memory.lock().unwrap()
    }

    fn limit(&self) -> usize {
        *self.limit.lock().unwrap()
    }

    fn foreground_pid(&self) -> Option<u32> {
        *self.foreground.lock().unwrap()
    }

    fn launcher_outcome(&self, _since: std::time::SystemTime) -> Option<(i64, String)> {
        self.launcher_said.lock().unwrap().clone()
    }

    fn end_launcher(&self, osc: (u16, u16)) -> usize {
        self.launchers_ended.lock().unwrap().push(osc);
        1
    }

    fn vr_rig(&self, session: &str) -> Option<Arc<dyn super::vr::VrRig>> {
        let busy = self.vr.lock().unwrap().clone()?;
        let rig = Arc::new(LabRig { session: session.to_string(), busy, game: Mutex::default(), calls: Mutex::default() });
        self.rigs.lock().unwrap().push(rig.clone());
        Some(rig)
    }
}

/// The lab's VR sandbox: records what the bridge asked of it and plays it on the session's lab VRChat.
pub struct LabRig {
    pub session: String,
    busy: Option<String>,
    game: Mutex<Option<Arc<LabGame>>>,
    /// `take`, `keep`, `camera`, `point x y w h click`, `eye`, `release`, in order (`keep` once per call at most).
    pub calls: Mutex<Vec<String>>,
}

impl LabRig {
    fn note(&self, call: String) {
        let mut calls = self.calls.lock().unwrap();
        if call != "keep" || calls.last().map(String::as_str) != Some("keep") {
            calls.push(call);
        }
    }
}

impl super::vr::VrRig for LabRig {
    fn take(&self) -> Result<(), Failure> {
        self.note("take".into());
        match &self.busy {
            Some(why) => Err(Failure::new("vr_busy", format!("No VR session now: {why}"), "Use a desktop session.")), // i18n: not shown (the lab)
            None => Ok(()),
        }
    }

    fn keep(&self) {
        self.note("keep".into());
    }

    fn camera(&self) -> Result<(), Failure> {
        self.note("camera".into());
        if let Some(game) = self.game.lock().unwrap().as_ref() {
            game.vr_camera();
        }
        Ok(())
    }

    fn point(&self, pixel: (f64, f64), size: (u32, u32), click: bool) -> Result<(), Failure> {
        self.note(format!("point {} {} {} {} {click}", pixel.0, pixel.1, size.0, size.1)); // i18n: not shown (the lab's record of calls)
        if click {
            if let Some(game) = self.game.lock().unwrap().as_ref() {
                game.vr_click();
            }
        }
        Ok(())
    }

    fn eye(&self) -> Result<Vec<u8>, Failure> {
        self.note("eye".into());
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(1620, 1800, image::Rgba([90, 120, 160, 255])))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| Failure::new("vr_sandbox", e.to_string(), "The lab."))?; // i18n: not shown
        Ok(png)
    }

    fn release(&self) {
        self.note("release".into());
    }
}
