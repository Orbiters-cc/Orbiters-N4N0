//! Several VRChats at once: a **session** is one VRChat the bridge started, bound to one of VRChat's profiles
//! (`--profile=N`, each with its own signed-in account), driven by one agent at a time.
//!
//! - **`main`** is the session of before: tools without `session` act on it, as they always did. It is the VRChat the app
//!   itself follows (VRChat's default OSC ports, the log, the capture), and only exists while the person runs no VRChat.
//! - **Side sessions** (`open_session`) run beside anything else, the person's own VRChat (desktop or VR) included: each
//!   gets OSC ports of its own (`--osc=<in>:127.0.0.1:<out>`), its own log, window and input. The app never follows them:
//!   its process and log watchers skip them, its OSCQuery discovery takes only the VRChat on port 9000, and their
//!   datagrams to its OSC port (they find its service over mDNS like any VRChat) are dropped by their source's process.
//!   A 3D picture in a side session borrows the app's capture for its recording ("lent"): one picture at a time on this
//!   computer, and never while the person's own VRChat runs (the app is theirs then).
//!
//! Measured on this computer on 2026-11-15 (done6-bridge.md): `launch.exe` with `--profile` starts a VRChat of its own every
//! time (1.3 to 3.2 s to the process), never handing the link to a running one; all VRChats share one Easy Anti-Cheat service
//! process and its driver; closing any of them while others run leaves both running and an immediate launch works; after the
//! last one goes they unload in about 2 s (16 to 18 s after a VRChat that never signed in). Only the first VRChat gets
//! VRChat's link pipe (`VRCNP: Failed to create server: All pipe instances are busy` in the others).

use orbiters_core::locks::*;
use super::game::Failure;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The session of before, which tools without `session` act on.
pub const MAIN: &str = "main";
/// A session no call came for in this long may be taken over by another agent (`open_session` with `take_over`).
pub const LEASE_STALE: Duration = Duration::from_secs(10 * 60);
/// Side sessions' OSC ports: profile N gets `9000 + 10 N` in and the next port out, away from VRChat's own 9000/9001 and
/// from what OSC apps usually take (9002 to 9005).
pub const PORT_BASE: u16 = 9000;
/// VRChat sessions the bridge runs at most by default (main and side ones together), Settings' `vrchat_bridge_sessions`
/// changing it. Measured here: a VRChat at its sign-in page holds 1.9 GB of memory and with one in a world the 16 GB card
/// had 3 GB in use; a world usually takes 2 to 6 GB of graphics memory per VRChat (done6-bridge.md).
pub const DEFAULT_LIMIT: usize = 2;
/// The most Settings may allow.
pub const MOST: usize = 4;
/// Memory a new VRChat needs free, at least.
pub const MEMORY_NEEDED: u64 = 4 << 30;

/// A side session's OSC ports, from its profile.
pub fn ports_of(profile: u32) -> Option<(u16, u16)> {
    if profile == 0 {
        return None;
    }
    let into = PORT_BASE.checked_add(u16::try_from(profile).ok()?.checked_mul(10)?)?;
    Some((into, into.checked_add(1)?))
}

/// A session id an agent gets: the profile and an unguessable part, so that only the agent that opened it, or one it gave
/// the id, drives it.
pub fn new_id(profile: u32) -> String {
    // UUID v4s come from the system's random source.
    format!("p{profile}-{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
}

/// Who drives a session.
#[derive(Clone, Debug)]
pub struct Lease {
    /// What the agent calls itself (MCP's clientInfo), for list_sessions.
    pub owner: String,
    pub last_call: Instant,
}

impl Lease {
    pub fn stale(&self, now: Instant) -> bool {
        now.duration_since(self.last_call) > LEASE_STALE
    }
}

/// What the app needs to know about a side session (it reads these, never the bridge's sessions themselves).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideMark {
    pub session: String,
    pub profile: u32,
    pub osc_in: u16,
    pub osc_out: u16,
    pub pid: Option<u32>,
    pub log: Option<PathBuf>,
    /// Its VRChat's OSCQuery service (`Advertising Service VRChat-Client-… of type OSCQuery on <port>`).
    pub query: Option<SocketAddr>,
}

/// Who drives a VRChat running on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Driver {
    /// The person's own: started by them (Steam, VRChat's launcher, a link), never with the bridge's marks.
    Person,
    /// The bridge's main session (profile 0, VRChat's default ports): only while the person runs no VRChat of their own.
    Main,
    /// A side session of the bridge's, by its id.
    Session(String),
    /// Started by the bridge for a side session (its `--profile` of 1 or more and that profile's `--osc`, marks only the
    /// bridge passes) but held by no session now: an earlier run of the app's, or one whose agent left it running. Never
    /// the person's (2026-11-15 13:48: such a VRChat of the world tour's, PID 8244, was counted as the person's).
    Bridge,
}

/// One VRChat running on this computer, as the app knows it ([`Sides::set_roster`]).
#[derive(Clone, Debug, PartialEq)]
pub struct VrchatRun {
    pub pid: u32,
    /// When its process started (Unix nanoseconds; 0 when Windows would not say).
    pub started_ns: u64,
    /// VRChat's `--profile` (0 without one).
    pub profile: u32,
    /// Whether `profile` is known: its log said its arguments, or the bridge started it on that profile.
    pub profile_known: bool,
    pub log: Option<PathBuf>,
    /// Where it takes OSC input: its `--osc` in port, or VRChat's default.
    pub osc_in: u16,
    /// Its OSCQuery service (`Advertising Service VRChat-Client-… of type OSCQuery on <port>`).
    pub query: Option<SocketAddr>,
    /// The account signed in (id, display name).
    pub account: Option<(String, String)>,
    /// Whether that is who is signed in now, for certain (the last its log says, read to its end).
    pub account_sure: bool,
    /// Whether `log` is its own for certain: made at or after its start, and no other VRChat's.
    pub log_sure: bool,
    /// Its log says nobody is signed in now.
    pub signed_out: bool,
    /// Its command line names exactly one profile (or none: the default one).
    pub one_profile: bool,
    pub world: Option<String>,
    /// Desktop (`--no-vr`) or in VR; unknown until its log said.
    pub desktop: Option<bool>,
    /// Whether its camera is open, as its own OSCQuery tree says (a mark for the picker, never proof for a recording).
    pub camera: Option<bool>,
    pub driver: Driver,
}

/// VRChat's own OSC input port, which a VRChat started without `--osc` takes.
pub const VRCHAT_OSC_IN: u16 = 9000;

/// The VRChat the app follows by default among `runs`: the person's own (the one on VRChat's default ports first, then
/// the one started last), else the bridge's main session's. Never a side session's or one the bridge left running.
pub fn default_target(runs: &[VrchatRun]) -> Option<&VrchatRun> {
    runs.iter()
        .filter(|r| r.driver == Driver::Person)
        .min_by_key(|r| (r.osc_in != VRCHAT_OSC_IN, std::cmp::Reverse(r.started_ns), r.pid))
        .or_else(|| runs.iter().find(|r| r.driver == Driver::Main))
}

/// The side sessions, for the app's watchers and OSC: shared by the controller and the bridge. Also every VRChat running
/// (the roster) and the one the app follows: its capture target, which the person may choose.
#[derive(Default)]
pub struct Sides {
    inner: Mutex<SidesInner>,
}

#[derive(Default)]
struct SidesInner {
    marks: Vec<SideMark>,
    /// The session the app is lent to for a 3D picture.
    lent: Option<String>,
    /// A side VRChat is being started: a log created meanwhile, whose arguments are not written yet, may be its own.
    launching: Option<Instant>,
    /// The main session's VRChat (the bridge's own, which the app follows when the person runs none).
    main: Option<u32>,
    /// Logs read for their arguments (only once they are all written).
    heads: HashMap<PathBuf, Vec<String>>,
    /// Every VRChat running, once the watcher looked (`None` before: the rules of before apply).
    roster: Option<Vec<VrchatRun>>,
    /// The VRChat the person chose to take pictures in, while it runs.
    chosen: Option<u32>,
    /// What a VRChat the app did not follow said of its camera over OSC (open, when), by process.
    camera_heard: HashMap<u32, (bool, u64)>,
}

/// How long a launching side VRChat's new log may stay without its arguments before it counts as somebody else's.
const LAUNCH_GRACE: Duration = Duration::from_secs(20);

impl Sides {
    pub fn put(&self, mark: SideMark) {
        let mut inner = self.inner.locked();
        inner.marks.retain(|m| m.session != mark.session);
        inner.marks.push(mark);
    }

    pub fn remove(&self, session: &str) {
        let mut inner = self.inner.locked();
        inner.marks.retain(|m| m.session != session);
        if inner.lent.as_deref() == Some(session) {
            inner.lent = None;
        }
    }

    pub fn marks(&self) -> Vec<SideMark> {
        self.inner.locked().marks.clone()
    }

    pub fn launching(&self, on: bool) {
        self.inner.locked().launching = on.then(Instant::now);
    }

    pub fn set_main(&self, pid: Option<u32>) {
        self.inner.locked().main = pid;
    }

    pub fn main_pid(&self) -> Option<u32> {
        self.inner.locked().main
    }

    pub fn side_pids(&self) -> Vec<u32> {
        self.inner.locked().marks.iter().filter_map(|m| m.pid).collect()
    }

    /// Lends the app to `session` (`None`: gives it back). Refused when it is lent to another.
    pub fn lend(&self, session: Option<&str>) -> bool {
        let mut inner = self.inner.locked();
        match (session, inner.lent.as_deref()) {
            (Some(s), Some(l)) if s != l => false,
            _ => {
                inner.lent = session.map(str::to_string);
                true
            }
        }
    }

    pub fn lent(&self) -> Option<SideMark> {
        let inner = self.inner.locked();
        let lent = inner.lent.as_deref()?;
        inner.marks.iter().find(|m| m.session == lent).cloned()
    }

    /// The VRChat the app follows among `running`: the side session it is lent to, else its target in the roster, else
    /// (before the roster is known) one that is no side session's (the person's, or the main session's), else none.
    pub fn app_pid(&self, running: &[u32]) -> Option<u32> {
        let sides = self.side_pids();
        if let Some(pid) = self.lent().and_then(|m| m.pid).filter(|p| running.contains(p)) {
            return Some(pid);
        }
        if self.inner.locked().roster.is_some() {
            return self.target().map(|t| t.pid).filter(|p| running.contains(p));
        }
        running.iter().copied().find(|p| !sides.contains(p))
    }

    /// Every VRChat running, as the watcher last saw them, with each one's driver brought up to date with the sessions.
    pub fn set_roster(&self, mut runs: Vec<VrchatRun>) {
        let mut inner = self.inner.locked();
        for run in &mut runs {
            if let Some(mark) = inner.marks.iter().find(|m| m.pid == Some(run.pid)) {
                run.driver = Driver::Session(mark.session.clone());
            } else if inner.main == Some(run.pid) {
                run.driver = Driver::Main;
            }
        }
        if inner.chosen.is_some_and(|c| !runs.iter().any(|r| r.pid == c)) {
            inner.chosen = None;
        }
        inner.camera_heard.retain(|pid, _| runs.iter().any(|r| r.pid == *pid));
        inner.roster = Some(runs);
    }

    /// Whether the watcher looked at the VRChats running yet.
    pub fn roster_known(&self) -> bool {
        self.inner.locked().roster.is_some()
    }

    pub fn roster(&self) -> Vec<VrchatRun> {
        self.inner.locked().roster.clone().unwrap_or_default()
    }

    /// The person chooses which VRChat their pictures are taken in (`None`: the default, their own).
    pub fn choose(&self, pid: Option<u32>) {
        let mut inner = self.inner.locked();
        inner.chosen = pid.filter(|p| inner.roster.as_ref().is_none_or(|r| r.iter().any(|x| x.pid == *p)));
    }

    pub fn chosen(&self) -> Option<u32> {
        self.inner.locked().chosen
    }

    /// The VRChat the person's capture follows: the one they chose, else [`default_target`]. A bridge picture's lend does not
    /// change it.
    pub fn person_target(&self) -> Option<VrchatRun> {
        let inner = self.inner.locked();
        let runs = inner.roster.as_ref()?;
        inner.chosen.and_then(|c| runs.iter().find(|r| r.pid == c)).or_else(|| default_target(runs)).cloned()
    }

    /// The VRChat the app follows now: the side session it is lent to for a picture, else [`Sides::person_target`].
    pub fn target(&self) -> Option<VrchatRun> {
        let lent = self.lent();
        let inner = self.inner.locked();
        let runs = inner.roster.as_ref()?;
        if let Some(mark) = lent {
            let run = mark.pid.and_then(|p| runs.iter().find(|r| r.pid == p)).cloned();
            return run.or_else(|| {
                mark.pid.map(|pid| VrchatRun {
                    pid,
                    started_ns: 0,
                    profile: mark.profile,
                    profile_known: true,
                    log: mark.log.clone(),
                    osc_in: mark.osc_in,
                    query: mark.query,
                    account: None,
                    account_sure: false,
                    log_sure: false,
                    signed_out: false,
                    one_profile: true,
                    world: None,
                    desktop: Some(true),
                    camera: None,
                    driver: Driver::Session(mark.session.clone()),
                })
            });
        }
        inner.chosen.and_then(|c| runs.iter().find(|r| r.pid == c)).or_else(|| default_target(runs)).cloned()
    }

    /// What a VRChat the app does not follow said of its camera over OSC, while it was not followed.
    pub fn camera_heard(&self, pid: u32) -> Option<(bool, u64)> {
        self.inner.locked().camera_heard.get(&pid).copied()
    }

    pub fn hear_camera(&self, pid: u32, open: bool, at_ns: u64) {
        self.inner.locked().camera_heard.insert(pid, (open, at_ns));
    }

    /// The processes of every VRChat known to run (the roster's and the sessions').
    pub fn known_pids(&self) -> Vec<u32> {
        let inner = self.inner.locked();
        let mut pids: Vec<u32> = inner.roster.iter().flatten().map(|r| r.pid).collect();
        pids.extend(inner.marks.iter().filter_map(|m| m.pid));
        pids.extend(inner.main);
        pids.sort_unstable();
        pids.dedup();
        pids
    }

    /// Whether the app leaves this log alone: a side session's that the app is not lent to. A log is a side session's when
    /// the session knows it, or when its arguments name a side session's `--osc` ports; a log without its arguments yet,
    /// new while a side VRChat starts, waits too (a person's VRChat starting in those seconds is followed a moment later).
    pub fn skips_log(&self, path: &Path) -> bool {
        // The roster names the target's log: that one, and no other (the VRChat followed and its log are one).
        if let Some(target) = self.target() {
            if let Some(log) = &target.log {
                return log != path;
            }
        }
        let mut inner = self.inner.locked();
        if inner.roster.iter().flatten().any(|r| r.log.as_deref() == Some(path)) {
            // A log of a VRChat the app does not follow.
            return true;
        }
        let lent_log = inner.lent.as_ref().and_then(|l| inner.marks.iter().find(|m| &m.session == l)).and_then(|m| m.log.clone());
        if lent_log.as_deref() == Some(path) {
            return false;
        }
        if inner.marks.iter().any(|m| m.log.as_deref() == Some(path)) {
            return true;
        }
        if inner.marks.is_empty() && inner.launching.is_none() {
            return false;
        }
        let args = match inner.heads.get(path) {
            Some(args) => args.clone(),
            None => {
                let head = read_head(path, 64 * 1024);
                match complete_args(&head) {
                    Some(args) => {
                        if inner.heads.len() > 64 {
                            inner.heads.clear();
                        }
                        inner.heads.insert(path.to_path_buf(), args.clone());
                        args
                    }
                    // Not written yet: a side VRChat's, while one starts.
                    None => return inner.launching.is_some_and(|at| at.elapsed() < LAUNCH_GRACE),
                }
            }
        };
        let lent = inner.lent.clone();
        inner.marks.iter().any(|m| Some(&m.session) != lent.as_ref() && args.iter().any(|a| *a == osc_arg(m.osc_in, m.osc_out)))
    }
}

/// The app's OSC filter: a datagram whose source port belongs to a side session's VRChat is dropped (unless the app is
/// lent to it). Measured 2026-11-15: a side VRChat finds the app's OSC service over mDNS like any VRChat and sends its
/// avatar's values there too, from a UDP socket of its own process (profile 1 sent ~330 a second from port 57303, which
/// Windows' UDP table lists under its process id).
///
/// With several VRChats (the roster), only the one the app follows is heard: a datagram of any other VRChat is dropped,
/// and what it says of its camera kept for it ([`Sides::camera_heard`]), so that choosing it later knows whether its camera
/// is open without waiting for it to say so again.
pub fn datagram_filter(sides: std::sync::Arc<Sides>) -> orbiters_core::osc::DatagramFilter {
    let owners: Mutex<(HashMap<u16, u32>, Option<Instant>)> = Mutex::new((HashMap::new(), None));
    std::sync::Arc::new(move |from: SocketAddr, datagram: &[u8]| {
        let known = sides.known_pids();
        if known.len() < 2 && sides.side_pids().is_empty() {
            return true;
        }
        let owner = {
            let mut owners = owners.locked();
            if !owners.0.contains_key(&from.port()) && owners.1.is_none_or(|t| t.elapsed() > Duration::from_secs(1)) {
                *owners = (super::win::udp_owners(), Some(Instant::now()));
            }
            owners.0.get(&from.port()).copied()
        };
        let followed = sides.target().map(|t| t.pid).or_else(|| sides.lent().and_then(|m| m.pid));
        takes_datagram(&sides, owner, followed, datagram)
    })
}

/// [`datagram_filter`]'s rule, given the datagram's owning process and the VRChat followed.
pub fn takes_datagram(sides: &Sides, owner: Option<u32>, followed: Option<u32>, datagram: &[u8]) -> bool {
    let Some(pid) = owner else { return true };
    if Some(pid) == followed {
        return true;
    }
    let side = sides.side_pids().contains(&pid);
    if !side && !sides.known_pids().contains(&pid) {
        // Not a VRChat (another OSC program), or one not seen yet: as before.
        return true;
    }
    if let Some(open) = camera_mode_in(datagram) {
        sides.hear_camera(pid, open, orbiters_core::clock::now_ns());
    }
    false
}

/// Whether a datagram says VRChat's camera is open (`/usercamera/Mode` not 0), when it says.
pub fn camera_mode_in(datagram: &[u8]) -> Option<bool> {
    const MODE: &[u8] = b"/usercamera/Mode";
    if !datagram.windows(MODE.len()).any(|w| w == MODE) {
        return None;
    }
    fn find(packet: rosc::OscPacket) -> Option<bool> {
        match packet {
            rosc::OscPacket::Message(m) if m.addr == "/usercamera/Mode" => match m.args.first() {
                Some(rosc::OscType::Int(mode)) => Some(*mode != 0),
                _ => None,
            },
            rosc::OscPacket::Message(_) => None,
            rosc::OscPacket::Bundle(b) => b.content.into_iter().filter_map(find).next_back(),
        }
    }
    rosc::decoder::decode_udp(datagram).ok().and_then(|(_, p)| find(p))
}

/// A new strict private instance of `account`, as VRChat's own client names one: what a side session joins.
pub fn own_private_instance(account: &str, region: &str) -> String {
    let id = uuid::Uuid::new_v4();
    let number = u32::from_le_bytes(id.as_bytes()[..4].try_into().unwrap_or_default()) % 90_000 + 10_000;
    format!("{number}~private({account})~region({region})~nonce({id})")
}

/// The `--osc` argument of a side session.
pub fn osc_arg(osc_in: u16, osc_out: u16) -> String {
    format!("--osc={osc_in}:127.0.0.1:{osc_out}")
}

pub fn read_head(path: &Path, most: usize) -> String {
    use std::io::Read;
    let mut bytes = Vec::new();
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.take(most as u64).read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A log's command line (`Launching with args: N` then N lines of `Arg: …`), once all of it is written.
pub fn complete_args(head: &str) -> Option<Vec<String>> {
    let mut lines = head.lines();
    let count: usize = lines.by_ref().find_map(|l| l.split_once("Launching with args: ").map(|(_, n)| n.trim().parse().ok()))??;
    let args: Vec<String> = lines.filter_map(|l| l.split_once(" Arg: ").map(|(_, a)| a.to_string())).take(count).collect();
    (args.len() == count).then_some(args)
}

/// The OSCQuery port a VRChat's log says it advertises.
pub fn query_port(text: &str) -> Option<u16> {
    text.lines().find_map(|l| l.split_once("of type OSCQuery on ").and_then(|(_, p)| p.trim().parse().ok()))
}

/// Easy Anti-Cheat's state, as `sc queryex` says it (tools/vr-sandbox/vrchat_life.py's rule, measured 2026-11-13 and 15).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Eac {
    /// `EasyAntiCheat_EOS`'s state (`RUNNING`, `STOPPED`, `ABSENT`…).
    pub service: String,
    /// `EasyAntiCheat_EOSSys`'s, the driver.
    pub driver: String,
    /// A VRChat runs (any, the person's included).
    pub vrchat_runs: bool,
    /// The launcher or the bootstrapper of a launch still runs.
    pub launcher_runs: bool,
    /// The driver service's registry key has `DeleteFlag = 1`: Windows marked it for deletion and something still holds it.
    /// Easy Anti-Cheat's launcher then fails with "CreateService failed with 1072" (measured 2026-11-15 07:17, and the same
    /// state on 2026-11-14 22:13): no VRChat starts on this computer until Windows restarts.
    pub delete_pending: bool,
}

impl Eac {
    /// Whether a launch is safe now. With a VRChat running, the service and driver serve it and a launch beside it works
    /// at once (measured 2026-11-15, three times). With none, they must be stopped or gone first: a launch while the
    /// driver of a VRChat that just quit is still running fails with "EAC unload timeout" (30005, 2026-11-13).
    pub fn may_launch(&self) -> bool {
        if self.launcher_runs || self.delete_pending {
            return false;
        }
        let stopped = |s: &str| matches!(s, "STOPPED" | "ABSENT");
        self.vrchat_runs && self.service == "RUNNING" && self.driver == "RUNNING" || stopped(&self.service) && stopped(&self.driver)
    }

    /// What a refused launch says.
    pub fn failure(&self) -> Failure {
        Failure::new(
            "eac_busy",
            format!(
                "Easy Anti-Cheat of a VRChat that just closed is still unloading (service {}, driver {}): a launch now would fail with \"EAC unload timeout\".",
                self.service, self.driver
            ),
            "Try again in 30 s. If this lasts minutes, stop and tell the person: only a restart of the computer clears a driver marked for deletion.",
        )
        .retry_after(30)
    }

    /// What a launch answers while the driver service is marked for deletion: nothing is tried, as no launch can work.
    pub fn stuck_failure() -> Failure {
        Failure::new(
            "eac_stuck",
            "Easy Anti-Cheat's driver service (EasyAntiCheat_EOSSys) is marked for deletion: VRChat cannot start on this computer until Windows restarts. Nothing was started.",
            "Stop starting VRChat and tell the person: restarting the computer clears it. A VRChat already running goes on working: keep using it.",
        )
    }
}

/// The last outcome Easy Anti-Cheat's launcher wrote in its log (`anticheatlauncher.log`: `Launcher finished with: 206,
/// 'CreateService failed with 1072.'.`): its code and its words. `None` while no launch has finished.
pub fn launcher_outcome(log: &str) -> Option<(i64, String)> {
    let line = log.lines().rev().find(|l| l.contains("Launcher finished with: "))?;
    let rest = line.split_once("Launcher finished with: ")?.1;
    let (code, words) = rest.split_once(',').unwrap_or((rest, ""));
    let words = words.trim().trim_end_matches('.').trim_matches('\'').trim_end_matches('.').to_string();
    Some((code.trim().parse().ok()?, words))
}

/// What an outcome of Easy Anti-Cheat's launcher means. Read from this computer's 171 launcher logs of 2026-11-09 to 15
/// (`anticheatlauncher.log` and its `anticheatlauncher-backup-*.log`): 154 said `301, 'Easy Anti-Cheat successfully loaded
/// in-game'`, each followed by "waiting for game window to become visible" and VRChat's window; 17 said `206` (`'CreateService
/// failed with 1072.'` 9 times, `'EAC unload timeout'` 7 times, once garbled words), each followed by "Terminating with: 206"
/// and no game. On 2026-11-15 12:25 the 301 line came 1.5 s after the launch, before VRChat.exe was seen: taking it for a
/// failure disowned the VRChat it had just started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LauncherSaid {
    /// 301: the anti-cheat is loaded and the game is starting.
    Loaded,
    /// 206: the launch failed, no game comes.
    Failed,
    /// A code never seen here: not a failure by itself. Whether VRChat of this start runs decides.
    Unknown,
}

pub fn launcher_said(code: i64) -> LauncherSaid {
    match code {
        301 => LauncherSaid::Loaded,
        206 => LauncherSaid::Failed,
        _ => LauncherSaid::Unknown,
    }
}

/// What a launch answers when no VRChat of it was found running, from the launcher log's outcome. `ended`: how many launcher
/// processes of this start the bridge ended; it looked for this start's VRChat after that and found none.
pub fn launch_failure(outcome: Option<&(i64, String)>, ended: usize) -> Failure {
    if outcome.is_some_and(|(code, words)| launcher_said(*code) == LauncherSaid::Failed && words.contains("1072")) {
        return Eac::stuck_failure();
    }
    let checked = format!(
        "The bridge ended {ended} launcher process(es) of this start, then looked again: no VRChat of this start runs. If one still comes up in this session's profile with its OSC ports, this session takes it as its own (get_state says so)."
    );
    let (said, next) = match outcome {
        Some((code, words)) if launcher_said(*code) == LauncherSaid::Failed => (
            format!("VRChat did not start: Easy Anti-Cheat's launcher said {code}: {words}."),
            "Wait 90 s (Easy Anti-Cheat unloads its driver about 77 s after a VRChat quits), then call go_to_world or open_session once more. If it fails again, stop starting VRChat and tell the person.",
        ),
        Some((code, words)) => (
            format!("Easy Anti-Cheat's launcher said {code} ({words}), but no VRChat of this start came up in time."),
            "Call get_state in a minute: a VRChat of this start that comes up late is this session's. If none has come, call go_to_world or open_session once more; if that fails too, stop starting VRChat and tell the person.",
        ),
        None => (
            "VRChat did not start: Easy Anti-Cheat's launcher wrote no outcome (it may have shown \"Launch Error (30005)\").".to_string(),
            "Wait 90 s (Easy Anti-Cheat unloads its driver about 77 s after a VRChat quits), then call go_to_world or open_session once more. If it fails again, stop starting VRChat and tell the person.",
        ),
    };
    Failure::new("start_failed", format!("{said} {checked}"), next).retry_after(90)
}

/// Whether a VRChat's command line (its log's `Arg:` lines) is a start of the bridge's side session of `profile` on `osc`:
/// that profile and those OSC ports, which only the bridge passes. Never one of the default profile (the person's and the
/// main session's) or without these ports.
pub fn started_for(args: &[String], profile: u32, osc: (u16, u16)) -> bool {
    profile != 0 && ports_of(profile) == Some(osc) && args.iter().any(|a| *a == format!("--profile={profile}")) && args.iter().any(|a| *a == osc_arg(osc.0, osc.1))
}

/// Whether a VRChat log is of a VRChat that has quit: its last lines say so (`VRCApplication: HandleApplicationQuit`).
/// Windows dates a log at its process's end, so a log written to a minute ago can be of a VRChat that is gone.
pub fn log_says_quit(tail: &str) -> bool {
    tail.contains("HandleApplicationQuit")
}

/// The end of a file, at most `most` bytes.
pub fn read_tail(path: &Path, most: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else { return String::new() };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(most)));
    let mut bytes = Vec::new();
    let _ = file.take(most).read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `sc queryex`'s state line, `ABSENT` for a service that is not installed (1060).
pub fn service_state(sc_output: &str) -> String {
    if sc_output.contains("1060") && !sc_output.contains("STATE") {
        return "ABSENT".into();
    }
    sc_output
        .lines()
        .find_map(|l| {
            let rest = l.trim().strip_prefix("STATE")?;
            rest.split_whitespace().nth(2).map(str::to_string)
        })
        .unwrap_or_else(|| "UNKNOWN".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_side_sessions_instance_is_a_strict_private_one_of_its_own_account() {
        let account = "usr_dfc96b2c-1aec-4775-9557-eef94a7f02c3";
        let instance = own_private_instance(account, "eu");
        assert!(orbiters_core::vrchat_api::private_instance_of(&instance, account), "{instance}");
        assert!(!orbiters_core::vrchat_api::private_instance_of(&instance, "usr_1339e525-d75d-4832-9104-98dd89287681"));
        assert!(!instance.contains("canRequestInvite") && instance.contains("~nonce("));
        assert_ne!(instance, own_private_instance(account, "eu"));
    }

    #[test]
    fn side_ports_are_their_own_and_never_vrchats_defaults() {
        assert_eq!(ports_of(1), Some((9010, 9011)));
        assert_eq!(ports_of(2), Some((9020, 9021)));
        assert_eq!(ports_of(0), None, "profile 0 in a side session would take 9000: refused, it is the app's");
        assert_eq!(ports_of(7000), None);
        assert!(new_id(1).starts_with("p1-") && new_id(1).len() == 15 && new_id(1) != new_id(1));
    }

    #[test]
    fn what_a_vrchat_log_says_of_its_account_and_its_ports() {
        // As in profile 1's log of 2026-11-15, before and after Enzo signed in.
        let mut account = orbiters_core::vrchat_log::Account::default();
        account.read(
            "2026.11.15 01:00:32 Debug      -  [Authentication - X] No cached credentials found, transitioning to provider selection.\n\
             2026.11.15 01:00:32 Warning    -  VP AuthSelectionPage OnPageShown()\n",
        );
        assert!(account.signed_out && account.who.is_none());
        account.read("2026.11.15 01:09:12 Debug      -  [Authentication - X] Preforming post login checks for Sample 552a (usr_dfc96b2c-1aec-4775-9557-eef94a7f02c3)\n");
        assert!(!account.signed_out && account.sure);
        assert_eq!(account.who, Some(("usr_dfc96b2c-1aec-4775-9557-eef94a7f02c3".into(), "Sample 552a".into())));
        let head = "2026.11.15 01:00:29 Debug      -  Launching with args: 3\n\
                    2026.11.15 01:00:29 Debug      -  Arg: C:\\VRChat\\VRChat.exe\n\
                    2026.11.15 01:00:29 Debug      -  Arg: --profile=1\n\
                    2026.11.15 01:00:29 Debug      -  Arg: --osc=9010:127.0.0.1:9011\n\
                    2026.11.15 01:00:31 Debug      -  Advertising Service VRChat-Client-0D0E0D of type OSCQuery on 60102\n";
        assert_eq!(complete_args(head).unwrap()[2], osc_arg(9010, 9011));
        assert_eq!(complete_args(&head[..120]), None, "not all written yet");
        assert_eq!(query_port(head), Some(60102));
    }

    #[test]
    fn the_app_follows_the_persons_vrchat_and_skips_the_side_sessions() {
        let dir = std::env::temp_dir().join(format!("orbiters-sides-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let person = dir.join("output_log_2026-11-15_01-00-00.txt");
        let side = dir.join("output_log_2026-11-15_01-08-16.txt");
        let unknown_side = dir.join("output_log_2026-11-15_01-09-00.txt");
        std::fs::write(&person, "t Debug      -  Launching with args: 1\nt Debug      -  Arg: C:\\VRChat.exe\n").unwrap();
        std::fs::write(&side, "").unwrap();
        std::fs::write(&unknown_side, "t Debug      -  Launching with args: 2\nt Debug      -  Arg: x\nt Debug      -  Arg: --osc=9020:127.0.0.1:9021\n").unwrap();
        let sides = Sides::default();
        assert!(!sides.skips_log(&person) && !sides.skips_log(&side), "no side session: nothing is skipped");
        sides.put(SideMark { session: "p1-a".into(), profile: 1, osc_in: 9010, osc_out: 9011, pid: Some(20420), log: Some(side.clone()), query: None });
        sides.put(SideMark { session: "p2-b".into(), profile: 2, osc_in: 9020, osc_out: 9021, pid: Some(41760), log: None, query: None });
        assert!(sides.skips_log(&side), "known");
        assert!(sides.skips_log(&unknown_side), "its --osc names a side session");
        assert!(!sides.skips_log(&person));
        assert_eq!(sides.app_pid(&[20420, 41760, 5000]), Some(5000), "the person's");
        assert_eq!(sides.app_pid(&[20420, 41760]), None, "only side sessions run: the app follows none");
        assert!(sides.lend(Some("p1-a")));
        assert!(!sides.lend(Some("p2-b")), "one picture at a time");
        assert_eq!(sides.app_pid(&[20420, 41760, 5000]), Some(20420), "lent");
        assert!(!sides.skips_log(&side), "lent: followed");
        assert!(sides.lend(None));
        assert!(sides.skips_log(&side));
        sides.remove("p1-a");
        assert!(sides.skips_log(&unknown_side));
        sides.remove("p2-b");
        assert!(!sides.skips_log(&unknown_side), "gone");
        // A brand-new log without its arguments yet, while a side VRChat starts.
        let fresh = dir.join("output_log_2026-11-15_01-10-00.txt");
        std::fs::write(&fresh, "t Debug      -  Launching with").unwrap();
        sides.launching(true);
        assert!(sides.skips_log(&fresh));
        sides.launching(false);
        assert!(!sides.skips_log(&fresh));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_launch_waits_for_easy_anti_cheat() {
        let eac = |service: &str, driver: &str, vrchat_runs, launcher_runs| Eac { service: service.into(), driver: driver.into(), vrchat_runs, launcher_runs, delete_pending: false };
        assert!(eac("ABSENT", "ABSENT", false, false).may_launch(), "nothing loaded");
        assert!(eac("STOPPED", "STOPPED", false, false).may_launch(), "stopped but not deleted does not block (2026-11-13)");
        assert!(eac("RUNNING", "RUNNING", true, false).may_launch(), "beside a running VRChat (2026-11-15)");
        assert!(!eac("STOPPED", "RUNNING", false, false).may_launch(), "the driver of one that just quit: 30005");
        assert!(!eac("RUNNING", "RUNNING", false, false).may_launch(), "still unloading");
        assert!(!eac("RUNNING", "RUNNING", true, true).may_launch(), "another launch under way");
        assert!(!eac("STOP_PENDING", "STOP_PENDING", false, false).may_launch());
        assert_eq!(eac("STOPPED", "RUNNING", false, false).failure().code, "eac_busy");
        let sc = "SERVICE_NAME: EasyAntiCheat_EOS\n        TYPE               : 10  WIN32_OWN_PROCESS\n        STATE              : 4  RUNNING\n                                (STOPPABLE)\n        PID                : 27436\n";
        assert_eq!(service_state(sc), "RUNNING");
        assert_eq!(service_state("[SC] EnumQueryServicesStatus:OpenService FAILED 1060:\n\nThe specified service does not exist"), "ABSENT");
    }

    #[test]
    fn a_driver_marked_for_deletion_refuses_every_launch_with_its_reason() {
        // 2026-11-15 07:17: STOPPED, Start 4, DeleteFlag 1, and every launch, cold ones included, refused.
        let stuck = Eac { service: "STOPPED".into(), driver: "STOPPED".into(), vrchat_runs: false, launcher_runs: false, delete_pending: true };
        assert!(!stuck.may_launch());
        assert!(!Eac { vrchat_runs: true, service: "RUNNING".into(), driver: "RUNNING".into(), ..stuck.clone() }.may_launch());
        // The launcher's own log of that launch, as it was (times UTC).
        let log = "[06:17:32:094] [Windows] [EAC Bootstrapper] [Info] Connect result: No error (0) Response Code: 200 Destination IP: 52.222.201.96
                   [06:17:33:444] [Windows] [EAC Bootstrapper] [Info] Launcher finished with: 206, 'CreateService failed with 1072.'.
                   [06:17:33:444] [Windows] [EAC Bootstrapper] [Info] Unlocking the launch lock, allowing next game to launch to take place.
";
        let outcome = launcher_outcome(log).unwrap();
        assert_eq!(outcome, (206, "CreateService failed with 1072".to_string()));
        assert_eq!(launch_failure(Some(&outcome), 0).code, "eac_stuck");
        let none = launch_failure(None, 1);
        assert_eq!(none.code, "start_failed");
        assert_eq!(launcher_outcome("[x] Log file open"), None);
    }

    /// The launcher's outcomes, verbatim from this computer's logs (`anticheatlauncher-backup-2026.11.15-11.24.59.log`, the
    /// 12:25 `anticheatlauncher.log`, `-06.04.55`, `-2026.11.13-15.16.54`): 301 is the game starting, 206 a failure.
    #[test]
    fn easy_anti_cheats_outcomes_are_read_from_evidence_and_only_a_known_failure_fails() {
        let loaded = "[11:25:13:653] [Windows] [EAC Bootstrapper] [Info] Launcher finished with: 301, 'Easy Anti-Cheat successfully loaded in-game'.
[11:25:13:653] [Windows] [EAC Bootstrapper] [Info] Successfully initialized the Easy Anti-Cheat module, waiting for game window to become visible.
[11:25:13:653] [Windows] [EAC Bootstrapper] [Info] Unlocking the launch lock, allowing next game to launch to take place.
";
        let said = launcher_outcome(loaded).unwrap();
        assert_eq!(said, (301, "Easy Anti-Cheat successfully loaded in-game".to_string()));
        assert_eq!(launcher_said(said.0), LauncherSaid::Loaded, "12:25: VRChat.exe 8244 came up right after this line");
        let stuck = launcher_outcome("[06:04:55:187] [Windows] [EAC Bootstrapper] [Info] Launcher finished with: 206, 'CreateService failed with 1072.'.\n").unwrap();
        assert_eq!(launcher_said(stuck.0), LauncherSaid::Failed);
        let timeout = launcher_outcome(
            "[15:16:39:736] [Windows] [EAC Bootstrapper] [Info] Launcher finished with: 206, 'EAC unload timeout'.
[15:16:39:737] [Windows] [EAC Bootstrapper] [Info] Unlocking the launch lock, allowing next game to launch to take place.
[15:16:54:678] [Windows] [EAC Bootstrapper] [Info] Terminating with: 206.
",
        )
        .unwrap();
        assert_eq!(timeout, (206, "EAC unload timeout".to_string()));
        assert_eq!(launcher_said(timeout.0), LauncherSaid::Failed);
        assert_eq!(launcher_said(0), LauncherSaid::Unknown, "a code never seen is no failure by itself");
        assert_eq!(launcher_said(30005), LauncherSaid::Unknown, "30005 is the error window's number, never a launcher outcome here");
        // What a start that came to nothing says: what was checked, never "nothing is left running" unchecked.
        let failed = launch_failure(Some(&timeout), 1);
        assert_eq!((failed.code, failed.retry_after_s), ("start_failed", Some(90)));
        assert!(failed.message.contains("EAC unload timeout") && failed.message.contains("ended 1 launcher") && failed.message.contains("looked again"), "{}", failed.message);
        assert!(!failed.message.contains("nothing of this start is left running"));
        let late = launch_failure(Some(&said), 0);
        assert_eq!(late.code, "start_failed");
        assert!(late.message.contains("said 301") && late.message.contains("came up in time") && !late.message.contains("did not start"), "{}", late.message);
        assert!(late.next.contains("get_state"), "{}", late.next);
    }

    /// Only the bridge's own side-session start is ever taken as a session's: the 12:25 VRChat's command line, verbatim, and
    /// the same without the session's marks.
    #[test]
    fn a_vrchat_is_a_sessions_own_only_by_its_profile_and_its_osc_ports() {
        let log = "2026.11.15 12:25:17 Debug      -  Launching with args: 12
2026.11.15 12:25:17 Debug      -  Arg: C:\\Program Files (x86)\\Steam\\steamapps\\common\\VRChat\\VRChat.exe
2026.11.15 12:25:17 Debug      -  Arg: vrchat://launch?ref=orbiters.cc&id=wrld_19150cfd-170b-4ca7-a32e-6ba0ae080c09:14696~private(usr_dfc96b2c-1aec-4775-9557-eef94a7f02c3)~region(eu)~nonce(a805a8c8-7d15-4cad-a193-a992918219bf)
2026.11.15 12:25:17 Debug      -  Arg: --no-vr
2026.11.15 12:25:17 Debug      -  Arg: -screen-fullscreen
2026.11.15 12:25:17 Debug      -  Arg: 0
2026.11.15 12:25:17 Debug      -  Arg: -screen-width
2026.11.15 12:25:17 Debug      -  Arg: 1280
2026.11.15 12:25:17 Debug      -  Arg: -screen-height
2026.11.15 12:25:17 Debug      -  Arg: 720
2026.11.15 12:25:17 Debug      -  Arg: --profile=1
2026.11.15 12:25:17 Debug      -  VRCFlowCommandLine resolved --profile=1
2026.11.15 12:25:17 Debug      -  Arg: --osc=9010:127.0.0.1:9011
2026.11.15 12:25:17 Debug      -  Arg: --startup-begin-ts=5136942001
";
        let args = complete_args(log).unwrap();
        assert!(started_for(&args, 1, (9010, 9011)));
        assert!(!started_for(&args, 2, (9020, 9021)), "another profile's session");
        assert!(!started_for(&args, 1, (9020, 9021)), "ports not its profile's");
        let without = |drop: &str| args.iter().filter(|a| !a.starts_with(drop)).cloned().collect::<Vec<_>>();
        assert!(!started_for(&without("--osc"), 1, (9010, 9011)), "profile 1 without the session's ports: the person's");
        assert!(!started_for(&without("--profile"), 1, (9010, 9011)), "the default profile: never");
        let person = vec!["VRChat.exe".to_string(), "--profile=0".to_string(), osc_arg(9010, 9011)];
        assert!(!started_for(&person, 0, (9010, 9011)), "profile 0 is never a session's");
    }

    #[test]
    fn a_log_of_a_vrchat_that_quit_is_nobody_running() {
        // The tour's own last VRChat (07-03-20), quit at 07:04:51: still "written to lately", but gone.
        let tail = "2026.11.15 07:04:50 Debug      -  [VRCTrackingManager] [IK Debug Log] Set Graceful Quit: True
                    2026.11.15 07:04:51 Debug      -  VRCApplication: HandleApplicationQuit at 89.79087
";
        assert!(log_says_quit(tail));
        assert!(!log_says_quit("2026.11.15 07:04:44 Debug      -  [VRCTrackingSteam] FPS: 75.8"));
    }
}
