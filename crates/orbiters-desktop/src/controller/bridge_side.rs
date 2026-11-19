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

/// The processes running now: id, name (lower case), start (Unix seconds; 0 when unknown). One list of the system's, as Task
/// Manager reads it: no process is opened for it, VRChat's least of all (BRIDGE-3; sysinfo opened every process with
/// memory-read rights, a few times a second).
fn processes() -> Vec<(u32, String, u64)> {
    // FILETIME (100 ns since 1601) to Unix seconds.
    const UNIX_FROM_1601_S: u64 = 11_644_473_600;
    orbiters_booster::programs::scan()
        .into_iter()
        .map(|p| (p.pid, p.name.to_lowercase(), (p.created / 10_000_000).saturating_sub(UNIX_FROM_1601_S)))
        .collect()
}

fn is_vrchat(name: &str) -> bool {
    matches!(name, "vrchat.exe" | "vrchat")
}

fn is_launcher(name: &str) -> bool {
    matches!(name, "start_protected_game.exe" | "launch.exe")
}

/// VRChat process `pid` as the system's list has it now: running since when, gone, or unknown (the list could not be read:
/// that proves no exit).
pub(super) fn vrchat_identity(pid: u32) -> crate::bridge::game::Identity {
    use crate::bridge::game::Identity;
    let all = processes();
    if all.is_empty() {
        return Identity::Unknown;
    }
    match all.into_iter().find(|(p, name, _)| *p == pid && is_vrchat(name)) {
        Some((_, _, start)) => Identity::Running(start),
        None => Identity::Gone,
    }
}

/// The VRChats running now, by process id.
pub(super) fn vrchat_pids() -> Vec<u32> {
    processes().into_iter().filter(|(_, name, _)| is_vrchat(name)).map(|(pid, _, _)| pid).collect()
}

/// The VRChats running now with when each started (Unix seconds; 0 when Windows would not say).
fn vrchat_starts() -> Vec<(u32, u64)> {
    processes().into_iter().filter(|(_, name, _)| is_vrchat(name)).map(|(pid, _, start)| (pid, start)).collect()
}

fn launcher_runs() -> bool {
    processes().iter().any(|(_, name, _)| is_launcher(name))
}

/// A service's state from a short-lived `sc.exe queryex`, which is given 5 s and then ended: a hung query would hold a handle
/// of the service, and a service marked for deletion is deleted only once every handle of it is closed.
fn sc_state(service: &str) -> String {
    let mut command = std::process::Command::new("sc.exe");
    command.args(["queryex", service]).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let Ok(mut child) = command.spawn() else { return "UNKNOWN".into() };
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return "UNKNOWN".into();
            }
        }
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        use std::io::Read;
        let _ = stdout.read_to_string(&mut out);
    }
    sessions::service_state(&out)
}

/// Easy Anti-Cheat's launcher logs (`%APPDATA%\EasyAntiCheat\<product>\<deployment>\anticheatlauncher.log`), rewritten at
/// each launch.
fn launcher_logs() -> Vec<PathBuf> {
    let Some(root) = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("EasyAntiCheat")) else { return Vec::new() };
    let dirs = |p: &Path| std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect::<Vec<_>>();
    dirs(&root).iter().flat_map(|product| dirs(product)).map(|d| d.join("anticheatlauncher.log")).filter(|p| p.is_file()).collect()
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

    /// The bridge asks for a profile's game only while no session of it lives: a game still here on these ports is a start
    /// that failed or was left (2026-11-15 07:13: its OSC listener kept 9011 until the app restarted). It goes, its port freed.
    fn drop_stale(&self, osc: (u16, u16)) {
        let stale: Vec<String> = self.games.lock().unwrap().iter().filter(|(_, g)| g.osc == osc).map(|(id, _)| id.clone()).collect();
        for id in stale {
            self.forget(&id);
        }
    }

    fn sides(&self) -> Option<Arc<Sides>> {
        self.ctl.upgrade().map(|c| c.sides.clone())
    }
}

impl Opener for RealOpener {
    fn side(&self, session: &str, profile: u32, osc: (u16, u16)) -> Result<Arc<dyn Game>, Failure> {
        self.drop_stale(osc);
        let ctl = self.ctl.upgrade().ok_or_else(|| Failure::new("app_closing", "Orbiters - N4N0 is closing.", "Stop here."))?; // i18n: not shown
        let game = SideGame::new(Arc::downgrade(&ctl), self.exe.clone(), session, profile, osc, ctl.sides.clone())?;
        self.games.lock().unwrap().insert(session.to_string(), game.clone());
        Ok(game)
    }

    fn forget(&self, session: &str) {
        let game = self.games.lock().unwrap().remove(session);
        if let Some(game) = game {
            game.stop.store(true, Ordering::Relaxed);
            if game.lent_to_me() {
                game.unlend();
            }
            // Its OSC port is free once this returns (the listener looks at `stop` every 250 ms).
            if let Some(listener) = game.listener.lock().unwrap().take() {
                let _ = listener.join();
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
            // A VRChat that quit dates its log as it ends: the session's own last one, closed a moment ago, is nobody running
            // (2026-11-15 07:13: open_session answered profile_in_use for it).
            .filter(|e| !sessions::log_says_quit(&sessions::read_tail(&e.path(), 4096)))
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
            delete_pending: win::service_delete_pending("EasyAntiCheat_EOSSys"),
        }
    }

    fn launcher_outcome(&self, since: SystemTime) -> Option<(i64, String)> {
        launcher_logs()
            .into_iter()
            .filter(|p| std::fs::metadata(p).and_then(|m| m.modified()).is_ok_and(|t| t + LOG_CREATED_SLACK >= since))
            .find_map(|p| sessions::launcher_outcome(&sessions::read_tail(&p, 16 * 1024)))
    }

    fn end_launcher(&self, osc: (u16, u16)) -> usize {
        let arg = sessions::osc_arg(osc.0, osc.1);
        // Only VRChat's launchers are opened to read their command lines (never VRChat itself, nor any other process).
        let launchers: Vec<sysinfo::Pid> = processes().into_iter().filter(|(_, name, _)| is_launcher(name)).map(|(pid, _, _)| sysinfo::Pid::from_u32(pid)).collect();
        if launchers.is_empty() {
            return 0;
        }
        let mut system = sysinfo::System::new();
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&launchers),
            true,
            sysinfo::ProcessRefreshKind::nothing().with_cmd(sysinfo::UpdateKind::Always),
        );
        // Only a launcher of this session's own start: its command line carries this session's `--osc` (2026-11-15 07:13: the
        // hung start_protected_game.exe's did).
        system
            .processes()
            .values()
            .filter(|p| is_launcher(&p.name().to_string_lossy().to_lowercase()))
            .filter(|p| p.cmd().iter().any(|a| a.to_string_lossy().contains(&arg)))
            .filter(|p| p.kill())
            .count()
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

    fn vr_rig(&self, session: &str) -> Option<Arc<dyn crate::bridge::vr::VrRig>> {
        let set = self.ctl.upgrade()?.with_settings(|s| s.experimental.vrchat_bridge_vr_sandbox.clone());
        let root = crate::bridge::vr::sandbox_root(&set)?;
        Some(Arc::new(crate::bridge::vr::SandboxRig::new(root, session)))
    }

    fn set_main(&self, pid: Option<u32>) {
        if let Some(sides) = self.sides() {
            sides.set_main(pid);
        }
    }

    fn vrchats(&self) -> serde_json::Value {
        let Some(ctl) = self.ctl.upgrade() else { return serde_json::Value::Array(Vec::new()) };
        vrchats_json(&ctl.view().vrchat.runs)
    }

    fn left_by_bridge(&self) -> Vec<u32> {
        self.sides().map(|s| s.roster().into_iter().filter(|r| r.driver == sessions::Driver::Bridge).map(|r| r.pid).collect()).unwrap_or_default()
    }
}

/// The VRChats on this computer as the Capture page shows them, for an agent (`list_sessions`, `get_state`).
pub(crate) fn vrchats_json(runs: &[crate::state::VrchatRunShown]) -> serde_json::Value {
    use crate::state::VrchatDriverShown;
    serde_json::Value::Array(
        runs.iter()
            .map(|r| {
                serde_json::json!({
                    "profile": r.profile,
                    "account": r.account,
                    "world_name": r.world,
                    "mode": r.desktop.map(|d| if d { "desktop" } else { "vr" }),
                    "camera_open": r.camera,
                    "driven_by": match &r.driver {
                        VrchatDriverShown::Person => "the person".to_string(),
                        VrchatDriverShown::Agent { session } => format!("session {session}"),
                        VrchatDriverShown::Left => "nobody: the bridge started it for a session that is gone".to_string(),
                    },
                    "pictures_taken_here": r.target,
                    "chosen_by_person": r.chosen,
                })
            })
            .collect(),
    )
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
    /// The app's picture started in it, since (the give-back counts from here: "once the picture is saved").
    lent_at: Option<Instant>,
    /// The app was lent to it, since (the two-minute rule counts from here).
    lent_since: Option<Instant>,
    /// Its last picture's session.
    last_picture: Option<PathBuf>,
    /// The logs of its earlier VRChats: never this start's.
    followed: Vec<PathBuf>,
    /// When `pid`'s process started: its identity with the pid, checked at every look.
    started: Option<u64>,
    /// Its VRChat could not be told apart from others: said to the agent and the person, never closed.
    unresolved: Option<String>,
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
    /// The thread listening on its out port: joined when the session goes, so the port is free again.
    listener: Mutex<Option<std::thread::JoinHandle<()>>>,
    state: Mutex<SideState>,
    /// Held by `take_picture` from the lend until its picture started (or failed): the bridge's 100 ms `tick` does not
    /// judge a lend whose picture is still starting, nor give it back meanwhile (CTL2-5).
    lending: Mutex<()>,
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
        let listener = {
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
                .map_err(|e| Failure::new("osc_failed", format!("No OSC listener: {e}"), "Tell the person."))? // i18n: not shown
        };
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
            listener: Mutex::new(Some(listener)),
            state: Mutex::default(),
            lending: Mutex::new(()),
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
        if st.log.is_none() {
            if let Some(dir) = orbiters_core::paths::vrchat_log_dir() {
                st.log = own_log(&dir, launched_at, self.profile, self.osc, &st.followed);
                if let Some(log) = &st.log {
                    st.watcher = Some(LogWatcher::of_file(log.clone()));
                }
            }
        }
        match st.pid {
            Some(pid) => {
                use crate::bridge::game::Identity;
                // The same process as the one paired with its log: its id and its start. Gone, or another process now: its
                // log is never paired again (with whatever VRChat survives it).
                let same = match vrchat_identity(pid) {
                    Identity::Running(start) => st.started.is_none_or(|s| s == start),
                    Identity::Unknown => true,
                    Identity::Gone => false,
                };
                if !same {
                    if let Some(log) = st.log.take() {
                        st.followed.push(log);
                    }
                    st.watcher = None;
                    st.read_to = 0;
                    st.pid = None;
                    st.started = None;
                }
            }
            None if launched.elapsed() < Duration::from_secs(90) => {
                // Never a process by its id alone (a VRChat the person starts meanwhile is new too, BRIDGE-1): only once this
                // session's own log is there (its command line has this profile and these OSC ports, which only the bridge
                // passes), the one VRChat that started in the minute before that log, after the launch, nobody else's, and
                // only when no other VRChat's log was made then (one started at the same moment would make it a guess).
                let created = st.log.as_ref().and_then(|l| std::fs::metadata(l).ok()).and_then(|m| m.created().or_else(|_| m.modified()).ok());
                if let Some(created) = created {
                    let since = launched_at.duration_since(SystemTime::UNIX_EPOCH).ok().map(|d| d.as_secs());
                    let created = created.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                    let near = orbiters_core::paths::vrchat_log_dir().map_or(usize::MAX, |dir| logs_made_near(&dir, created));
                    if let Some((pid, start)) = self.process_of_log(created, since, &st.before, near) {
                        st.pid = Some(pid);
                        st.started = Some(start);
                        st.unresolved = None;
                    }
                }
            }
            None => {
                if st.unresolved.is_none() {
                    st.unresolved = Some(format!(
                        "This session's VRChat (profile {}) could not be told apart from other VRChats on this computer: the bridge leaves them all alone. If it started, it may still be running on that profile's account until someone closes it.",
                        self.profile
                    )); // i18n: not shown (an AI client reads it; the bridge's log)
                }
            }
        }
        if let Some(watcher) = st.watcher.as_mut() {
            watcher.poll();
        }
        if let Some(log) = st.log.clone() {
            // Whole lines, counted in the file's bytes (controller/vrchats.rs, CTL2-6).
            let (text, consumed) = super::vrchats::read_log_from(&log, st.read_to, LOG_CHUNK);
            st.read_to += consumed;
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

    /// The process of this session's log made at `created_s` (Unix seconds): the one VRChat that no other session, not the
    /// main one and none running before the launch (`before`) has, started in the minute before its log was made (12:25:
    /// process 12:25:13, log 12:25:15) and not before the launch (`since_s`). Two such, or none: nothing is taken.
    fn process_of_log(&self, created_s: u64, since_s: Option<u64>, before: &[u32], logs_near: usize) -> Option<(u32, u64)> {
        let me = self.id();
        let others: Vec<u32> = self.sides.marks().into_iter().filter(|m| m.session != me).filter_map(|m| m.pid).collect();
        let now_s = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        pair(created_s, since_s, now_s, before, &others, self.sides.main_pid(), &vrchat_starts(), logs_near)
    }

    /// Gives the app back (after a picture, or when it failed).
    fn unlend(&self) {
        if let Ok(ctl) = self.ctl() {
            ctl.lend_vrchat(None, None);
        }
        let mut st = self.state.lock().unwrap();
        st.lent_at = None;
        st.lent_since = None;
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

/// How much earlier than the launch a log may say it was created (clock steps between the call and the file system).
const LOG_CREATED_SLACK: Duration = Duration::from_millis(250);

/// This session's log: the newest **created** since its launch whose arguments name its own `--osc`, and none it followed
/// before. Not "written to lately": the VRChat this session closed a moment ago, in the same profile with the same `--osc`,
/// last wrote its log as it quit (Windows dates a log when its handle closes, at the process's end), and the first look after
/// the launch comes before the new VRChat has made its own (2 s at least). On the first world tour (2026-11-15, 04:45 to 05:16)
/// that look took the old log every time the old VRChat had quit less than about 7 s before: the session followed the
/// previous world for the whole start (no arrival, its dead OSCQuery port so no camera and no kit) until `start_timeout`.
/// Whether VRChat `pid`, started at `start` (Unix seconds), may be the process of a session's log made at `created_s`: no other
/// session's and not the main one's, started in the minute before the log (5 s after at most), not before the launch.
fn fits_log(pid: u32, start: u64, created_s: u64, since_s: Option<u64>, others: &[u32], main: Option<u32>) -> bool {
    !others.contains(&pid) && main != Some(pid) && start > 0 && start <= created_s + 5 && start + 60 >= created_s && since_s.is_none_or(|s| start + 1 >= s)
}

/// How long a log must have been there before its process is chosen: a VRChat started at the same moment writes its own log
/// within seconds.
const LOG_SETTLED_S: u64 = 10;

/// The process of a session's log made at `created_s`, from the VRChats running (`starts`: pid, start): the one fitting it
/// ([`fits_log`]) and not running before the launch, once the log has been there [`LOG_SETTLED_S`] and when it is the only
/// VRChat log made then (`logs_near`). Anything less certain: none (the start stays unresolved, said, never closed).
#[allow(clippy::too_many_arguments)]
fn pair(created_s: u64, since_s: Option<u64>, now_s: u64, before: &[u32], others: &[u32], main: Option<u32>, starts: &[(u32, u64)], logs_near: usize) -> Option<(u32, u64)> {
    if now_s < created_s + LOG_SETTLED_S || logs_near != 1 {
        return None;
    }
    let candidates: Vec<(u32, u64)> = starts.iter().copied().filter(|(pid, start)| !before.contains(pid) && fits_log(*pid, *start, created_s, since_s, others, main)).collect();
    let [one] = candidates[..] else { return None };
    Some(one)
}

/// How many VRChat logs in `dir` were made in the minute before `created_s` or the 5 s after (the window [`fits_log`] uses).
fn logs_made_near(dir: &Path, created_s: u64) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("output_log_"))
                .filter_map(|e| e.metadata().ok()?.created().ok()?.duration_since(SystemTime::UNIX_EPOCH).ok().map(|d| d.as_secs()))
                .filter(|t| *t + 60 >= created_s && *t <= created_s + 5)
                .count()
        })
        .unwrap_or(usize::MAX)
}

/// A side session's own log: made since its launch, whose command line is this session's start (its profile and OSC ports).
fn own_log(dir: &Path, since: SystemTime, profile: u32, osc: (u16, u16), followed: &[PathBuf]) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("output_log_"))
        .filter(|e| !followed.iter().any(|f| f == &e.path()))
        .filter(|e| e.metadata().and_then(|m| m.created().or_else(|_| m.modified())).is_ok_and(|t| t + LOG_CREATED_SLACK >= since))
        .filter(|e| sessions::complete_args(&sessions::read_head(&e.path(), 64 * 1024)).is_some_and(|args| sessions::started_for(&args, profile, osc)))
        .max_by_key(|e| e.file_name())
        .map(|e| e.path())
}


impl Game for SideGame {
    fn unresolved(&self) -> Option<String> {
        self.state.lock().unwrap().unresolved.clone()
    }

    fn give_back(&self) {
        if self.lent_to_me() {
            self.unlend();
        }
    }

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

    fn persons_capture_here(&self) -> bool {
        let pid = self.state.lock().unwrap().pid;
        pid.is_some() && self.sides.chosen() == pid && !self.lent_to_me() && self.ctl().is_ok_and(|c| c.is_capturing())
    }

    fn chosen_by_person(&self) -> bool {
        let pid = self.state.lock().unwrap().pid;
        pid.is_some() && self.sides.chosen() == pid
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
            // A profile's account does not change between its starts: kept, so that a start that failed is not taken for a
            // profile waiting at its sign-in page (2026-11-15 07:09: go_to_world answered a stale needs_sign_in). The new
            // VRChat's log says it again, or that it waits for a sign-in.
            let account = st.account.take();
            let mut followed = std::mem::take(&mut st.followed);
            followed.extend(st.log.take());
            // A handful is enough: older logs are minutes old, past any launch's moment.
            let drop_old = followed.len().saturating_sub(8);
            followed.drain(..drop_old);
            *st = SideState { launched: Some((Instant::now(), SystemTime::now())), before: vrchat_pids(), last_picture, followed, account, ..Default::default() };
        }
        self.values.lock().unwrap().clone_from(&Latest::default());
        self.sides.launching(true);
        let started = win::start_vrchat_beside(&plan.args());
        if started.is_err() {
            self.sides.launching(false);
        }
        started
    }

    fn adopt(&self, since: Option<SystemTime>) -> Option<u32> {
        let dir = orbiters_core::paths::vrchat_log_dir()?;
        let floor = since.and_then(|t| t.checked_sub(LOG_CREATED_SLACK));
        // Its log: created since the bridge's launch (when known), written to within three minutes, not of a VRChat that quit,
        // and whose command line is this session's start.
        let (log, created) = std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("output_log_"))
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                let created = meta.created().or_else(|_| meta.modified()).ok()?;
                meta.modified().ok()?.elapsed().is_ok_and(|d| d < Duration::from_secs(180)).then_some((e.path(), created))
            })
            .filter(|(_, created)| floor.is_none_or(|f| *created >= f))
            .filter(|(path, _)| !sessions::log_says_quit(&sessions::read_tail(path, 4096)))
            .filter(|(path, _)| sessions::complete_args(&sessions::read_head(path, 64 * 1024)).is_some_and(|args| sessions::started_for(&args, self.profile, self.osc)))
            .max_by_key(|(path, _)| path.file_name().map(|n| n.to_os_string()))?;
        let created_s = created.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
        let since_s = since.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_secs());
        // Never a log whose process went before (its survivor would be a guess), nor a process running before the launch.
        let (followed, before) = {
            let st = self.state.lock().unwrap();
            (st.followed.clone(), st.before.clone())
        };
        if followed.contains(&log) {
            return None;
        }
        let (pid, start) = self.process_of_log(created_s, since_s, &before, logs_made_near(&dir, created_s))?;
        {
            let mut st = self.state.lock().unwrap();
            st.started = Some(start);
            st.unresolved = None;
            if st.log.as_ref() != Some(&log) {
                st.watcher = Some(LogWatcher::of_file(log.clone()));
                st.log = Some(log.clone());
                st.read_to = 0;
            }
            st.followed.retain(|f| f != &log);
            st.pid = Some(pid);
            if st.launched.is_none() {
                st.launched = Some((Instant::now(), created));
            }
            st.looked = None;
        }
        self.look();
        Some(pid)
    }

    fn send_link(&self, plan: &LaunchPlan) -> Result<(), Failure> {
        use super::travel::Travel as _;
        let Some(pid) = self.state.lock().unwrap().pid else {
            return Err(Failure::new("link_unavailable", "This session's VRChat is not running.", "It is started in the instance instead.")); // i18n: not shown (an AI client reads it)
        };
        let (pipe, server) = super::travel::Real.connect().map_err(|e| Failure::new("link_unavailable", format!("VRChat's link pipe cannot be opened: {e}"), "It is started in the instance instead."))?; // i18n: not shown (an AI client reads it)
        // Only the first VRChat started serves the pipe: written to anyone else's, the link would move the person's VRChat.
        if server != Some(pid) {
            return Err(Failure::new(
                "link_pipe_elsewhere",
                format!("VRChat's link pipe is served by another VRChat (process {}), not this session's ({pid}).", server.map_or("unknown".to_string(), |p| p.to_string())), // i18n: not shown (an AI client reads it)
                "It is started in the instance instead.", // i18n: not shown (an AI client reads it)
            ));
        }
        pipe.send(&super::travel::link(&plan.world_id, &plan.instance)).map_err(|e| Failure::new("link_unavailable", format!("VRChat did not take the link: {e}"), "It is started in the instance instead.")) // i18n: not shown (an AI client reads it)
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

    fn resize(&self, window: &GameWindow, width: u32, height: u32) {
        win::resize_client(window.handle, width, height);
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

    fn favorite_groups(&self) -> Result<Vec<(String, String)>, Failure> {
        self.inner.favorite_groups()
    }

    fn camera_pose(&self) -> Option<[f32; 6]> {
        let values = self.values.lock().unwrap();
        values.get(super::burst::POSE).and_then(|(v, _)| <[f32; 6]>::try_from(super::burst::floats(v).as_slice()).ok()).filter(|p| p.iter().any(|f| *f != 0.0))
    }

    fn set_camera_pose(&self, pose: [f32; 6]) -> Result<(), Failure> {
        let packet = rosc::OscPacket::Message(rosc::OscMessage { addr: super::burst::POSE.to_string(), args: pose.iter().map(|v| rosc::OscType::Float(*v)).collect() });
        let bytes = rosc::encoder::encode(&packet).map_err(|e| Failure::new("osc_failed", format!("OSC: {e}"), "Tell the person."))?; // i18n: not shown
        self.sender.send_to(&bytes, self.to_vrchat()).map_err(|e| Failure::new("osc_failed", format!("OSC to VRChat failed: {e}"), "Call get_state."))?; // i18n: not shown
        // What a picture starts from, and what a lend carries to the app.
        let values = pose.iter().map(|v| OscValue::Float(*v)).collect();
        self.values.lock().unwrap().record(super::burst::POSE.to_string(), values, orbiters_core::clock::now_ns());
        Ok(())
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
        // The person chose this very VRChat for their own pictures: theirs while they keep it chosen.
        if self.sides.chosen() == Some(pid) {
            return Err(Failure::new(
                "app_in_use",
                "The person chose this session's VRChat for their own 3D pictures in Orbiters - N4N0 (its Capture page): the capture is theirs here until they choose another.", // i18n: not shown
                "Walk, look and screenshot meanwhile (your input waits while they record), and try again later; tell the person if you need pictures now.", // i18n: not shown
            )
            .retry_after(120));
        }
        // The main session's VRChat is the bridge's own: borrowed from it while it records nothing. With the roster known, a
        // VRChat the bridge left running (its marks) is never counted as the person's.
        let (sides, main) = (self.sides.side_pids(), self.sides.main_pid());
        let persons = match self.sides.person_target() {
            Some(t) => t.driver != sessions::Driver::Main,
            None => !self.sides.roster_known() && vrchat_pids().iter().any(|p| !sides.contains(p) && Some(*p) != main),
        };
        if persons {
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
        // From the lend to the picture's start: `tick` leaves this lend alone meanwhile.
        let _lending = self.lending.lock().unwrap_or_else(|e| e.into_inner());
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
        self.state.lock().unwrap().lent_since = Some(Instant::now());
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
        match taken {
            Ok(()) => self.state.lock().unwrap().lent_at = Some(Instant::now()),
            Err(_) => self.unlend(),
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
        // A picture still starting (the lend is being made, the watchers following): not judged now.
        let Ok(_lending) = self.lending.try_lock() else { return };
        let (started, since) = {
            let st = self.state.lock().unwrap();
            (st.lent_at, st.lent_since)
        };
        let Some(since) = since else { return };
        let Ok(ctl) = self.ctl() else { return };
        let view = ctl.view();
        let idle = matches!(view.capture.state, crate::state::CaptureState::Idle);
        if give_back_due(started.map(|t| t.elapsed()), since.elapsed(), idle) {
            if let Some(session) = view.capture.session_dir.clone() {
                self.state.lock().unwrap().last_picture = Some(session);
            }
            self.unlend();
        }
    }
}

/// Whether a lend is over: its picture started (`started` ago) and the capture is Idle again a while after, or two
/// minutes passed since the lend whatever happens. A lend whose picture has not started yet is not over by being Idle.
fn give_back_due(started: Option<Duration>, lent_for: Duration, idle: bool) -> bool {
    idle && started.is_some_and(|s| s > Duration::from_secs(3)) || lent_for > Duration::from_secs(120)
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
        // One lend or give-back at a time, whole: the bridge's tick (listener thread) and a picture (client thread) no
        // longer interleave their four steps (CTL2-5).
        let _one = self.lend_gate.lock().unwrap_or_else(|e| e.into_inner());
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
        // Another VRChat's camera from here: an answer asked before the loan (or the give-back) is not this one's.
        self.camera_route_changed();
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
        match to.and_then(|m| m.pid) {
            // What was just put in is this VRChat's: followed as it stands.
            Some(pid) => *self.followed.lock().unwrap() = Some(pid),
            // Back to the VRChat the person's capture follows (theirs, or the one they chose), with its own camera state.
            None => {
                *self.followed.lock().unwrap() = None;
                self.follow_target();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// CTL2-5: a lend whose picture has not started is not given back for being Idle (the watchers may take more than
    /// 3 s to follow); once its picture started, Idle 3 s later gives it back; two minutes give it back whatever.
    #[test]
    fn a_lend_is_given_back_only_after_its_picture_started_or_two_minutes() {
        let s = Duration::from_secs;
        assert!(!give_back_due(None, s(5), true), "still starting: the watchers have not followed yet");
        assert!(!give_back_due(Some(s(1)), s(6), true), "just started: the state may not say Recording yet");
        assert!(give_back_due(Some(s(4)), s(9), true), "started, then Idle: saved");
        assert!(!give_back_due(Some(s(30)), s(35), false), "still recording");
        assert!(give_back_due(None, s(121), false), "two minutes whatever happens");
    }

    /// Two free loopback ports.
    fn free_ports() -> (u16, u16) {
        let a = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let b = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        (a.local_addr().unwrap().port(), b.local_addr().unwrap().port())
    }

    /// 2026-11-15 07:13: a start that failed kept its session's OSC listener, and every open_session after it answered
    /// port_taken (the port held by the app itself) until the app restarted. A game left on those ports is recognised as the
    /// bridge's own and goes before a new one binds them; a forgotten one frees its port at once.
    #[test]
    fn a_failed_starts_osc_listener_is_its_own_and_is_freed_for_the_next() {
        let osc = free_ports();
        let opener = RealOpener::new(Weak::new(), PathBuf::new());
        let left = SideGame::new(Weak::new(), PathBuf::new(), "p1-000000000001", 1, osc, Arc::new(Sides::default())).unwrap();
        opener.games.lock().unwrap().insert("p1-000000000001".into(), left);
        let taken = SideGame::new(Weak::new(), PathBuf::new(), "p1-000000000002", 1, osc, Arc::new(Sides::default())).err().expect("held by the left one");
        assert_eq!(taken.code, "port_taken");
        // What open_session does next: the left game goes, then the port is free (the app here is closing, so no game).
        assert_eq!(opener.side("p1-000000000002", 1, osc).err().map(|f| f.code), Some("app_closing"));
        assert!(opener.games.lock().unwrap().is_empty());
        let next = SideGame::new(Weak::new(), PathBuf::new(), "p1-000000000003", 1, osc, Arc::new(Sides::default())).expect("the port is free again");
        opener.games.lock().unwrap().insert("p1-000000000003".into(), next);
        opener.forget("p1-000000000003");
        UdpSocket::bind(("127.0.0.1", osc.1)).expect("freed when forgotten");
    }

    /// A log as profile 1's VRChat writes it (its first lines as on 2026-11-15, 04:58:40).
    fn write_log(path: &Path, world: &str) {
        let mut f = std::fs::File::create(path).unwrap();
        writeln!(f, "2026.11.15 04:58:40 Debug      -  Launching with args: 3").unwrap();
        writeln!(f, "2026.11.15 04:58:40 Debug      -  Arg: vrchat://launch?ref=orbiters.cc&id={world}:84156~private(usr_dfc96b2c-1aec-4775-9557-eef94a7f02c3)~region(eu)").unwrap();
        writeln!(f, "2026.11.15 04:58:40 Debug      -  Arg: --profile=1").unwrap();
        writeln!(f, "2026.11.15 04:58:40 Debug      -  Arg: --osc=9010:127.0.0.1:9011").unwrap();
    }

    #[test]
    fn a_start_follows_its_own_new_log_never_the_one_its_last_vrchat_just_left() {
        let dir = std::env::temp_dir().join(format!("orbiters-own-log-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        // The session's last VRChat, in the last world: made a while ago, written to until it quit, a moment before the launch.
        let old = dir.join("output_log_2026-11-15_04-57-20.txt");
        write_log(&old, "wrld_41efe3b1-9931-40ab-a15d-6946d22481b5");
        std::thread::sleep(Duration::from_millis(600));
        std::fs::OpenOptions::new().append(true).open(&old).unwrap().write_all(b"2026.11.15 04:58:30 Debug      -  uSpeak: Stop Microphone\n").unwrap();
        let since = SystemTime::now();
        // The first look after the launch: the new VRChat has no log yet. The old one, written to 0 s ago, is not this start's.
        assert_eq!(own_log(&dir, since, 1, (9010, 9011), &[]), None, "the log its previous VRChat just left is not this start's");
        // Then the new VRChat makes its own: that one, at once.
        std::thread::sleep(Duration::from_millis(50));
        let new = dir.join("output_log_2026-11-15_04-58-38.txt");
        write_log(&new, "wrld_ae001ea3-ed05-42f0-adf2-3d47efd10a77");
        assert_eq!(own_log(&dir, since, 1, (9010, 9011), &[]), Some(new.clone()));
        // A log this session followed before is never taken again, whatever its dates.
        assert_eq!(own_log(&dir, since, 1, (9010, 9011), std::slice::from_ref(&new)), None);
        // Another session's log (other ports) is not this one's.
        assert_eq!(own_log(&dir, since, 1, (9020, 9021), &[]), None);
        // The person's own VRChat (no profile, no ports of a session) is never a session's log.
        let person = dir.join("output_log_2026-11-15_04-58-39.txt");
        std::fs::write(&person, "2026.11.15 04:58:40 Debug      -  Launching with args: 1\n2026.11.15 04:58:40 Debug      -  Arg: --no-vr\n").unwrap();
        assert_eq!(own_log(&dir, since, 1, (9010, 9011), std::slice::from_ref(&new)), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn processes_are_listed_without_opening_any() {
        // BRIDGE-3: the system's own list, with names and start times (this test's process among them).
        let me = processes().into_iter().find(|(pid, _, _)| *pid == std::process::id()).expect("this process is listed");
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
        assert!(me.1.ends_with(".exe") && me.2 > 0 && me.2 <= now && now - me.2 < 3600, "{me:?}");
    }

    #[test]
    fn a_log_is_paired_with_its_process_only_when_nothing_else_could_have_written_it() {
        // verify-n4n0 (2): the timing alone paired the session's log with a surviving VRChat of the person's.
        let (log, launch) = (1_000_000, 999_980);
        let starts = [(7, 999_990), (8, 999_000)];
        assert_eq!(pair(log, Some(launch), log + 12, &[], &[], None, &starts, 1), Some((7, 999_990)), "settled, alone: paired");
        assert_eq!(pair(log, Some(launch), log + 3, &[], &[], None, &starts, 1), None, "a log just made: another VRChat's may come");
        assert_eq!(pair(log, Some(launch), log + 12, &[], &[], None, &starts, 2), None, "another VRChat's log made then: a guess");
        let two = [(7, 999_990), (9, 999_995)];
        assert_eq!(pair(log, Some(launch), log + 12, &[], &[], None, &two, 1), None, "two processes fit: a guess");
        assert_eq!(pair(log, Some(launch), log + 12, &[7], &[], None, &starts, 1), None, "running before the launch");
    }

    #[test]
    fn a_new_process_is_a_session_s_only_by_its_own_log() {
        // BRIDGE-1: during a start, the person's VRChat is new too. A process is taken only when it started in the minute
        // before the session's own log was made, after the launch, and is nobody else's.
        let (log, launch) = (1_000_000, 999_980);
        assert!(fits_log(7, 999_990, log, Some(launch), &[], None), "started 10 s before its log, after the launch");
        assert!(!fits_log(7, 999_930, log, Some(launch), &[], None), "started before the launch: not this start's");
        assert!(!fits_log(7, 999_900, log, None, &[], None), "more than a minute before the log");
        assert!(!fits_log(7, 1_000_010, log, Some(launch), &[], None), "after the log was made");
        assert!(!fits_log(7, 999_990, log, Some(launch), &[7], None), "another session's");
        assert!(!fits_log(7, 999_990, log, Some(launch), &[], Some(7)), "the main session's");
        assert!(!fits_log(7, 0, log, None, &[], None), "start unknown");
    }
}
