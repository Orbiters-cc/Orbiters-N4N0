//! The bridge's side sessions for real (bridge/sessions.rs): a VRChat in a profile of its own, beside the person's, which
//! the app does not follow. [`SideGame`] knows its VRChat by itself: the process that appeared after its launch (launches
//! are one at a time), the log whose arguments carry its own `--osc` ports, its own OSC ports (input sent to `in`, what
//! VRChat says heard on `out`), its own OSCQuery service (from that log), its window. What is the app's (VRChat's API
//! with the account signed in in Settings, the capture for a 3D picture, the reconstructions) it borrows from the
//! controller: a picture lends the app to this VRChat for the recording ([`Controller::lend_vrchat`]).

use super::bridge_game::{api_failure, RealGame};
use super::{Controller, Shared};
use crate::bridge::game::{CaptureSeen, Failure, Find, Game, GameWindow, LaunchPlan, Opener, OscArg, PictureFiles, ReconstructionSeen, Seen, Shot, WorldPage};
use crate::bridge::input::Input;
use crate::bridge::sessions::{self, Eac, SideMark, Sides};
use crate::bridge::win;
use orbiters_core::osc::{Latest, OscValue};
use orbiters_core::settings::{Photo3dLength, PictureMode};
use orbiters_core::vrchat_log::LogWatcher;
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};

/// A side session's log text read at most per look.
const LOG_CHUNK: u64 = 4 << 20;

/// The VRChats running now, by process id (names only, as the app's watcher reads them).
fn vrchat_pids() -> Vec<u32> {
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, sysinfo::ProcessRefreshKind::nothing());
    system
        .processes()
        .values()
        .filter(|p| matches!(p.name().to_string_lossy().to_lowercase().as_str(), "vrchat.exe" | "vrchat"))
        .map(|p| p.pid().as_u32())
        .collect()
}

fn launcher_runs() -> bool {
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, sysinfo::ProcessRefreshKind::nothing());
    system.processes().values().any(|p| matches!(p.name().to_string_lossy().to_lowercase().as_str(), "start_protected_game.exe" | "launch.exe"))
}

fn sc_state(service: &str) -> String {
    let mut command = std::process::Command::new("sc.exe");
    command.args(["queryex", service]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    match command.output() {
        Ok(out) => sessions::service_state(&String::from_utf8_lossy(&out.stdout)),
        Err(_) => "UNKNOWN".into(),
    }
}

/// What the bridge asks of the computer for its sessions.
pub(crate) struct RealOpener {
    ctl: Weak<Controller>,
    exe: PathBuf,
    games: Mutex<HashMap<String, Arc<SideGame>>>,
}

impl RealOpener {
    pub(crate) fn new(ctl: Weak<Controller>, exe: PathBuf) -> Self {
        Self { ctl, exe, games: Mutex::default() }
    }

    fn sides(&self) -> Option<Arc<Sides>> {
        self.ctl.upgrade().map(|c| c.sides.clone())
    }
}

impl Opener for RealOpener {
    fn side(&self, session: &str, profile: u32, osc: (u16, u16)) -> Result<Arc<dyn Game>, Failure> {
        let ctl = self.ctl.upgrade().ok_or_else(|| Failure::new("app_closing", "Orbiters - N4N0 is closing.", "Stop here."))?; // i18n: not shown
        let game = SideGame::new(Arc::downgrade(&ctl), self.exe.clone(), session, profile, osc, ctl.sides.clone())?;
        self.games.lock().unwrap().insert(session.to_string(), game.clone());
        Ok(game)
    }

    fn forget(&self, session: &str) {
        if let Some(game) = self.games.lock().unwrap().remove(session) {
            game.stop.store(true, Ordering::Relaxed);
            if game.lent_to_me() {
                game.unlend();
            }
        }
        if let Some(sides) = self.sides() {
            sides.remove(session);
        }
    }

    fn rename(&self, from: &str, to: &str) {
        let game = self.games.lock().unwrap().remove(from);
        if let Some(game) = game {
            let lent = game.lent_to_me();
            *game.session.lock().unwrap() = to.to_string();
            if let Some(sides) = self.sides() {
                let mark = sides.marks().into_iter().find(|m| m.session == from);
                sides.remove(from);
                if let Some(mark) = mark {
                    sides.put(SideMark { session: to.to_string(), ..mark });
                }
                if lent {
                    sides.lend(Some(to));
                }
            }
            self.games.lock().unwrap().insert(to.to_string(), game);
        }
    }

    fn vrchat_pids(&self) -> Vec<u32> {
        vrchat_pids()
    }

    fn others_profiles(&self) -> Vec<u32> {
        let Some(dir) = orbiters_core::paths::vrchat_log_dir() else { return Vec::new() };
        let sides = self.sides();
        let marks = sides.as_ref().map(|s| s.marks()).unwrap_or_default();
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        // Logs written to in the last three minutes: the VRChats running (VRChat writes several lines a minute).
        entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("output_log_"))
            .filter(|e| e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|d| d < Duration::from_secs(180))))
            .filter(|e| !marks.iter().any(|m| m.log.as_deref() == Some(e.path().as_path())))
            .filter_map(|e| sessions::complete_args(&sessions::read_head(&e.path(), 64 * 1024)))
            .filter(|args| !marks.iter().any(|m| args.contains(&sessions::osc_arg(m.osc_in, m.osc_out))))
            .map(|args| args.iter().find_map(|a| a.strip_prefix("--profile=").and_then(|p| p.parse().ok())).unwrap_or(0))
            .collect()
    }

    fn eac(&self) -> Eac {
        Eac {
            service: sc_state("EasyAntiCheat_EOS"),
            driver: sc_state("EasyAntiCheat_EOSSys"),
            vrchat_runs: !vrchat_pids().is_empty(),
            launcher_runs: launcher_runs(),
        }
    }

    fn memory_free(&self) -> Option<u64> {
        let mut system = sysinfo::System::new();
        system.refresh_memory();
        Some(system.available_memory())
    }

    fn limit(&self) -> usize {
        let set = self.ctl.upgrade().map_or(0, |c| c.with_settings(|s| s.experimental.vrchat_bridge_sessions));
        if set == 0 {
            sessions::DEFAULT_LIMIT
        } else {
            usize::from(set).clamp(1, sessions::MOST)
        }
    }

    fn foreground_pid(&self) -> Option<u32> {
        win::foreground_pid()
    }

    fn set_main(&self, pid: Option<u32>) {
        if let Some(sides) = self.sides() {
            sides.set_main(pid);
        }
    }
}

/// What a side session knows of its VRChat.
#[derive(Default)]
struct SideState {
    /// When it was last launched (the instant, and the time for the logs' dates), and the VRChats running before.
    launched: Option<(Instant, SystemTime)>,
    before: Vec<u32>,
    pid: Option<u32>,
    log: Option<PathBuf>,
    watcher: Option<LogWatcher>,
    /// How far its log was read for the account and its ports.
    read_to: u64,
    account: Option<(String, String)>,
    needs_sign_in: bool,
    query: Option<SocketAddr>,
    desktop: Option<bool>,
    looked: Option<Instant>,
    /// The app was lent to it for a picture, since.
    lent_at: Option<Instant>,
    /// Its last picture's session.
    last_picture: Option<PathBuf>,
}

pub(crate) struct SideGame {
    inner: RealGame,
    session: Mutex<String>,
    profile: u32,
    osc: (u16, u16),
    sides: Arc<Sides>,
    sender: UdpSocket,
    /// What its VRChat sent to its out port.
    values: Arc<Mutex<Latest>>,
    stop: Arc<AtomicBool>,
    state: Mutex<SideState>,
}

impl SideGame {
    fn new(ctl: Weak<Controller>, exe: PathBuf, session: &str, profile: u32, osc: (u16, u16), sides: Arc<Sides>) -> Result<Arc<Self>, Failure> {
        let listener = UdpSocket::bind(("127.0.0.1", osc.1)).map_err(|e| {
            Failure::new(
                "port_taken",
                format!("This profile's OSC port {} is taken on this computer ({e}): another program listens there.", osc.1), // i18n: not shown (an AI client reads it)
                "Use another profile, or tell the person which program holds the port.", // i18n: not shown (an AI client reads it)
            )
        })?;
        listener.set_read_timeout(Some(Duration::from_millis(250))).ok();
        let sender = UdpSocket::bind(("127.0.0.1", 0)).map_err(|e| Failure::new("osc_failed", format!("No OSC socket: {e}"), "Tell the person."))?; // i18n: not shown
        let values = Arc::new(Mutex::new(Latest::default()));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (values, stop) = (values.clone(), stop.clone());
            std::thread::Builder::new()
                .name("bridge-side-osc".into())
                .spawn(move || {
                    let mut buffer = vec![0u8; 65536];
                    while !stop.load(Ordering::Relaxed) {
                        let Ok((n, from)) = listener.recv_from(&mut buffer) else { continue };
                        // Only this computer's VRChat talks to a loopback port.
                        if !from.ip().is_loopback() {
                            continue;
                        }
                        if let Ok((_, packet)) = rosc::decoder::decode_udp(&buffer[..n]) {
                            let mut messages = Vec::new();
                            flatten(packet, &mut messages);
                            let at = orbiters_core::clock::now_ns();
                            let mut values = values.lock().unwrap();
                            for message in messages {
                                let args: Vec<OscValue> = message.args.iter().filter_map(OscValue::from_osc).collect();
                                values.record(message.addr, args, at);
                            }
                        }
                    }
                })
                .map_err(|e| Failure::new("osc_failed", format!("No OSC listener: {e}"), "Tell the person."))?; // i18n: not shown
        }
        sides.put(SideMark { session: session.to_string(), profile, osc_in: osc.0, osc_out: osc.1, pid: None, log: None, query: None });
        Ok(Arc::new(Self {
            inner: RealGame::new(ctl, exe),
            session: Mutex::new(session.to_string()),
            profile,
            osc,
            sides,
            sender,
            values,
            stop,
            state: Mutex::default(),
        }))
    }

    fn ctl(&self) -> Result<Shared, Failure> {
        self.inner.ctl()
    }

    fn id(&self) -> String {
        self.session.lock().unwrap().clone()
    }

    fn to_vrchat(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.osc.0))
    }

    fn lent_to_me(&self) -> bool {
        self.sides.lent().is_some_and(|m| m.session == self.id())
    }

    /// Follows its VRChat: the process, the log, who signed in, its OSCQuery service. At most a few times a second.
    fn look(&self) {
        let mut st = self.state.lock().unwrap();
        if st.looked.is_some_and(|t| t.elapsed() < Duration::from_millis(300)) {
            return;
        }
        st.looked = Some(Instant::now());
        let Some((launched, launched_at)) = st.launched else { return };
        let running = vrchat_pids();
        match st.pid {
            Some(pid) if !running.contains(&pid) => st.pid = None,
            Some(_) => {}
            None if launched.elapsed() < Duration::from_secs(90) => {
                // The one VRChat that appeared after this launch (launches are one at a time) and is nobody else's.
                let others: Vec<u32> = self.sides.side_pids();
                let new: Vec<u32> = running.iter().copied().filter(|p| !st.before.contains(p) && !others.contains(p)).collect();
                if let [pid] = new[..] {
                    st.pid = Some(pid);
                }
            }
            None => {}
        }
        if st.log.is_none() {
            if let Some(dir) = orbiters_core::paths::vrchat_log_dir() {
                st.log = own_log(&dir, launched_at, &sessions::osc_arg(self.osc.0, self.osc.1));
                if let Some(log) = &st.log {
                    st.watcher = Some(LogWatcher::of_file(log.clone()));
                }
            }
        }
        if let Some(watcher) = st.watcher.as_mut() {
            watcher.poll();
        }
        if let Some(log) = st.log.clone() {
            let text = read_from(&log, st.read_to, LOG_CHUNK);
            st.read_to += text.len() as u64;
            let asked = text.rfind("No cached credentials found").or_else(|| text.rfind("AuthSelectionPage OnPageShown"));
            let signed = text.rfind("post login checks for ");
            match (asked, signed) {
                (Some(a), s) if s.is_none_or(|s| s < a) => {
                    st.needs_sign_in = true;
                    st.account = None;
                }
                (_, Some(_)) => {
                    st.needs_sign_in = false;
                    st.account = sessions::signed_in(&text);
                }
                _ => {}
            }
            if st.query.is_none() {
                st.query = sessions::query_port(&text).map(|p| SocketAddr::from(([127, 0, 0, 1], p)));
            }
            if st.desktop.is_none() && (text.contains("Arg: --no-vr") || text.contains("VR Disabled")) {
                st.desktop = Some(true);
            }
        }
        if st.pid.is_some() && st.log.is_some() || launched.elapsed() > Duration::from_secs(20) {
            self.sides.launching(false);
        }
        self.sides.put(SideMark {
            session: self.id(),
            profile: self.profile,
            osc_in: self.osc.0,
            osc_out: self.osc.1,
            pid: st.pid,
            log: st.log.clone(),
            query: st.query,
        });
    }

    /// Gives the app back (after a picture, or when it failed).
    fn unlend(&self) {
        if let Ok(ctl) = self.ctl() {
            ctl.lend_vrchat(None, None);
        }
        self.state.lock().unwrap().lent_at = None;
    }

    fn values(&self) -> Latest {
        self.values.lock().unwrap().clone()
    }
}

/// Every message of a packet, bundles opened.
fn flatten(packet: rosc::OscPacket, out: &mut Vec<rosc::OscMessage>) {
    match packet {
        rosc::OscPacket::Message(m) => out.push(m),
        rosc::OscPacket::Bundle(b) => b.content.into_iter().for_each(|p| flatten(p, out)),
    }
}

/// This session's log: the newest written since its launch whose arguments name its own `--osc`.
fn own_log(dir: &Path, since: SystemTime, osc: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("output_log_"))
        .filter(|e| e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t + Duration::from_secs(5) >= since))
        .filter(|e| sessions::complete_args(&sessions::read_head(&e.path(), 64 * 1024)).is_some_and(|args| args.iter().any(|a| a == osc)))
        .max_by_key(|e| e.file_name())
        .map(|e| e.path())
}

fn read_from(path: &Path, from: u64, most: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let mut bytes = Vec::new();
    if let Ok(mut file) = std::fs::File::open(path) {
        if file.seek(SeekFrom::Start(from)).is_ok() {
            let _ = file.take(most).read_to_end(&mut bytes);
        }
    }
    // Up to the last whole line: the rest is read next time.
    let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

impl Game for SideGame {
    fn seen(&self) -> Seen {
        self.look();
        let Ok(ctl) = self.ctl() else { return Seen::default() };
        let snap = ctl.view();
        let lent = self.lent_to_me();
        let st = self.state.lock().unwrap();
        let world = st.watcher.as_ref().map(|w| w.state.clone()).unwrap_or_default();
        let running = st.pid.is_some();
        let values = self.values.lock().unwrap();
        let pose_known = values.get(super::burst::POSE).is_some_and(|(v, _)| {
            let floats = super::burst::floats(v);
            floats.len() == 6 && floats.iter().any(|f| *f != 0.0)
        });
        let speed = match values.first("/avatar/parameters/VelocityMagnitude") {
            Some(OscValue::Float(v)) if v.is_finite() => Some(v),
            _ => None,
        };
        let stats = snap.capture.stats.as_ref();
        let capture = if lent {
            CaptureSeen {
                state: serde_json::to_value(snap.capture.state).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default(),
                session: snap.capture.session_dir.clone(),
                frames: stats.map_or(0, |s| s.frames),
                keyframes: stats.map_or(0, |s| s.keyframes),
                no_strip: snap.capture.no_strip.map(|n| n.message_english().to_string()),
                error: snap.capture.error.clone(),
            }
        } else {
            CaptureSeen { state: "idle".into(), session: st.last_picture.clone(), ..Default::default() }
        };
        Seen {
            running,
            pid: st.pid,
            log_found: st.log.is_some(),
            world_id: world.world_id.clone(),
            world_name: world.world_name.clone(),
            instance: world.instance.clone(),
            in_world: running && world.in_world,
            local_player: world.local_player.clone(),
            avatar_name: world.avatar_name.clone().or_else(|| world.local_player.as_ref().and_then(|me| world.avatars.get(me).cloned())),
            others: world.others(),
            vrchat_user: snap.vrchat_login.user.as_ref().map(|u| (u.id.clone(), u.display_name.clone())),
            desktop_mode: if running { st.desktop } else { None },
            osc_found: st.query.is_some(),
            camera_pose_known: pose_known,
            speed,
            capture,
            reconstruction: snap.reconstruction.as_ref().map(|job| ReconstructionSeen {
                session: job.session_dir.clone(),
                stage: format!("{:?}", job.stage).to_lowercase(),
                progress: job.progress,
            }),
            queue: (*snap.reconstruct_queue).clone(),
            queue_waits_for: ctl.reconstruction_waits_for().to_string(),
            steamvr: snap.steamvr.running,
            session_account: st.account.clone(),
            needs_sign_in: st.needs_sign_in,
        }
    }

    fn camera_open(&self) -> Option<bool> {
        let query = self.state.lock().unwrap().query?;
        orbiters_core::osc::vrchat_camera_values(query, Duration::from_millis(500)).into_iter().find_map(|(p, v)| match v {
            OscValue::Int(mode) if p == "/usercamera/Mode" => Some(mode != 0),
            _ => None,
        })
    }

    fn capture_kit(&self) -> Option<bool> {
        let query = self.state.lock().unwrap().query?;
        let values = orbiters_core::osc::vrchat_values(query, Duration::from_secs(1)).ok()?;
        super::hide_self::kit_in(&values)
    }

    fn create_private_instance(&self, world: &str, region: &str) -> Result<String, Failure> {
        self.ctl()?.vrchat_source().create_private_instance(world, region).map_err(|e| api_failure(e, "making the instance")) // i18n: not shown
    }

    fn find_worlds(&self, find: &Find, page: usize) -> Result<WorldPage, Failure> {
        self.inner.find_worlds(find, page)
    }

    fn launch(&self, plan: &LaunchPlan) -> Result<(), Failure> {
        {
            let mut st = self.state.lock().unwrap();
            let last_picture = st.last_picture.take();
            *st = SideState { launched: Some((Instant::now(), SystemTime::now())), before: vrchat_pids(), last_picture, ..Default::default() };
        }
        self.values.lock().unwrap().clone_from(&Latest::default());
        self.sides.launching(true);
        let started = win::start_vrchat_beside(&plan.args());
        if started.is_err() {
            self.sides.launching(false);
        }
        started
    }

    fn window(&self, pid: u32) -> Option<GameWindow> {
        win::window_of(pid, None)
    }

    fn is_foreground(&self, window: &GameWindow) -> bool {
        win::is_foreground(window.handle)
    }

    fn bring_forward(&self, window: &GameWindow) {
        win::bring_forward(window.handle);
    }

    fn close(&self, window: &GameWindow) -> Result<(), Failure> {
        win::close_window(window.handle)
    }

    fn capture(&self, window: &GameWindow, max_width: u32) -> Result<Shot, Failure> {
        self.inner.capture(window, max_width)
    }

    fn osc(&self, address: &str, value: OscArg) -> Result<(), Failure> {
        let arg = match value {
            OscArg::Int(i) => rosc::OscType::Int(i),
            OscArg::Float(f) => rosc::OscType::Float(f),
        };
        let packet = rosc::OscPacket::Message(rosc::OscMessage { addr: address.to_string(), args: vec![arg] });
        let bytes = rosc::encoder::encode(&packet).map_err(|e| Failure::new("osc_failed", format!("OSC: {e}"), "Tell the person."))?; // i18n: not shown
        self.sender.send_to(&bytes, self.to_vrchat()).map(drop).map_err(|e| Failure::new("osc_failed", format!("OSC to VRChat failed: {e}"), "Call get_state.")) // i18n: not shown
    }

    fn wear(&self, avatar_id: &str) -> Result<(), Failure> {
        let packet = rosc::OscPacket::Message(rosc::OscMessage {
            addr: orbiters_core::osc::AVATAR_CHANGE.to_string(),
            args: vec![rosc::OscType::String(avatar_id.to_string())],
        });
        let bytes = rosc::encoder::encode(&packet).map_err(|e| Failure::new("osc_failed", format!("OSC: {e}"), "Tell the person."))?; // i18n: not shown
        self.sender.send_to(&bytes, self.to_vrchat()).map(drop).map_err(|e| Failure::new("osc_failed", format!("OSC to VRChat failed: {e}"), "Call get_state.")) // i18n: not shown
    }

    fn osc_target(&self) -> Option<SocketAddr> {
        Some(self.to_vrchat())
    }

    fn inject(&self, window: &GameWindow, input: &[Input]) -> Result<(), Failure> {
        win::inject(window, input)
    }

    fn release(&self, input: &[Input]) {
        win::release(input);
    }

    /// Lends the app to this VRChat for the recording: refused while another VRChat is the app's (the person's, or the
    /// main session's) and while another session's picture runs.
    fn take_picture(&self, length: Photo3dLength, mode: PictureMode) -> Result<(), Failure> {
        let ctl = self.ctl()?;
        let (pid, query) = {
            let st = self.state.lock().unwrap();
            (st.pid, st.query)
        };
        let (Some(pid), Some(query)) = (pid, query) else {
            return Err(Failure::new("osc_off", "This session's VRChat has not said where its OSC is yet.", "Wait a moment and call get_state.")); // i18n: not shown
        };
        // The main session's VRChat is the bridge's own: borrowed from it while it records nothing.
        let (sides, main) = (self.sides.side_pids(), self.sides.main_pid());
        if vrchat_pids().iter().any(|p| !sides.contains(p) && Some(*p) != main) {
            return Err(Failure::new(
                "app_in_use",
                "The person's own VRChat runs, which Orbiters - N4N0 follows: its capture is theirs, so no 3D picture can be taken in a side session meanwhile.", // i18n: not shown
                "Walk, look and screenshot meanwhile, and try again once that VRChat is closed; tell the person if they want pictures now.", // i18n: not shown
            )
            .retry_after(300));
        }
        if !matches!(ctl.view().capture.state, crate::state::CaptureState::Idle) {
            return Err(Failure::new("picture_busy", "Orbiters - N4N0 is recording already (another session's picture).", "Try again in 30 s.").retry_after(30)); // i18n: not shown
        }
        if !self.sides.lend(Some(&self.id())) {
            return Err(Failure::new("picture_busy", "Another session is taking a 3D picture: one at a time on this computer.", "Try again in 30 s.").retry_after(30)); // i18n: not shown
        }
        let mark = SideMark { pid: Some(pid), query: Some(query), ..self.sides.marks().into_iter().find(|m| m.session == self.id()).unwrap_or(SideMark {
            session: self.id(),
            profile: self.profile,
            osc_in: self.osc.0,
            osc_out: self.osc.1,
            pid: None,
            log: None,
            query: None,
        }) };
        ctl.lend_vrchat(Some(mark), Some(self.values()));
        self.state.lock().unwrap().lent_at = Some(Instant::now());
        // The app follows this VRChat once its watcher has looked (every 3 s) and read its log to the end.
        let world = self.state.lock().unwrap().watcher.as_ref().and_then(|w| w.state.world_id.clone());
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(10) {
            let view = ctl.view();
            if view.vrchat.pid == Some(pid) && view.vrchat.running && view.vrchat.world.world_id == world {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if ctl.view().vrchat.pid != Some(pid) {
            self.unlend();
            return Err(Failure::new("picture_did_not_start", "Orbiters - N4N0 did not follow this session's VRChat in time.", "Try again; if it stays, tell the person.")); // i18n: not shown
        }
        let taken = self.inner.take_picture(length, mode);
        if taken.is_err() {
            self.unlend();
        }
        taken
    }

    fn stop_picture(&self) {
        if self.lent_to_me() {
            self.inner.stop_picture();
        }
    }

    fn picture_files(&self, session: &Path) -> PictureFiles {
        self.inner.picture_files(session)
    }

    fn hold_reconstructions(&self, hold: bool) {
        self.inner.hold_reconstructions(hold);
    }

    /// Gives the app back once the picture is saved (or after two minutes whatever happens).
    fn tick(&self) {
        self.look();
        let Some(since) = self.state.lock().unwrap().lent_at else { return };
        let Ok(ctl) = self.ctl() else { return };
        let view = ctl.view();
        let idle = matches!(view.capture.state, crate::state::CaptureState::Idle);
        if idle && since.elapsed() > Duration::from_secs(3) || since.elapsed() > Duration::from_secs(120) {
            if let Some(session) = view.capture.session_dir.clone() {
                self.state.lock().unwrap().last_picture = Some(session);
            }
            self.unlend();
        }
    }
}

impl Drop for SideGame {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Controller {
    /// Lends the app to a side session's VRChat for a 3D picture (`None`: gives it back to the person's VRChat): the
    /// process and log watchers follow it, OSC goes to its input port and its tree, its datagrams are taken, and what it
    /// said of its camera and avatar is what the app knows now.
    pub(crate) fn lend_vrchat(&self, to: Option<SideMark>, values: Option<Latest>) {
        match &to {
            Some(mark) => {
                self.sides.lend(Some(&mark.session));
            }
            None => {
                self.sides.lend(None);
            }
        }
        let route = to.as_ref().and_then(|m| m.query.map(|q| (SocketAddr::from(([127, 0, 0, 1], m.osc_in)), q)));
        let _ = self.with_osc(|o| o.lend(route));
        let mut osc = self.osc_values.lock().unwrap();
        // The camera's opening was heard, on the session's own out port, by the session: the proof the app asks of a live
        // camera before it records (`require_live_camera`) is that VRChat's, not a value the app wrote.
        let camera_open = values.as_ref().and_then(|v| v.first(super::CAMERA_MODE)).map(|m| matches!(m, OscValue::Int(mode) if mode != 0));
        osc.camera_received = camera_open;
        osc.camera_command = None;
        osc.camera_commanded.clear();
        match values {
            Some(values) => {
                for (address, (args, at)) in values.iter() {
                    osc.latest.record(address.clone(), args.clone(), *at);
                }
            }
            // The person's VRChat says its own camera again when it moves: the lent one's pose is not theirs.
            None => {
                let mut kept = Latest::default();
                for (address, (args, at)) in osc.latest.iter().filter(|(a, _)| a.as_str() != super::burst::POSE) {
                    kept.record(address.clone(), args.clone(), *at);
                }
                osc.latest = kept;
            }
        }
        drop(osc);
        self.update(|s| s.camera_open_said = camera_open);
    }
}
