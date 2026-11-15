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

/// The side sessions, for the app's watchers and OSC: shared by the controller and the bridge.
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
}

/// How long a launching side VRChat's new log may stay without its arguments before it counts as somebody else's.
const LAUNCH_GRACE: Duration = Duration::from_secs(20);

impl Sides {
    pub fn put(&self, mark: SideMark) {
        let mut inner = self.inner.lock().unwrap();
        inner.marks.retain(|m| m.session != mark.session);
        inner.marks.push(mark);
    }

    pub fn remove(&self, session: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.marks.retain(|m| m.session != session);
        if inner.lent.as_deref() == Some(session) {
            inner.lent = None;
        }
    }

    pub fn marks(&self) -> Vec<SideMark> {
        self.inner.lock().unwrap().marks.clone()
    }

    pub fn launching(&self, on: bool) {
        self.inner.lock().unwrap().launching = on.then(Instant::now);
    }

    pub fn set_main(&self, pid: Option<u32>) {
        self.inner.lock().unwrap().main = pid;
    }

    pub fn main_pid(&self) -> Option<u32> {
        self.inner.lock().unwrap().main
    }

    pub fn side_pids(&self) -> Vec<u32> {
        self.inner.lock().unwrap().marks.iter().filter_map(|m| m.pid).collect()
    }

    /// Lends the app to `session` (`None`: gives it back). Refused when it is lent to another.
    pub fn lend(&self, session: Option<&str>) -> bool {
        let mut inner = self.inner.lock().unwrap();
        match (session, inner.lent.as_deref()) {
            (Some(s), Some(l)) if s != l => false,
            _ => {
                inner.lent = session.map(str::to_string);
                true
            }
        }
    }

    pub fn lent(&self) -> Option<SideMark> {
        let inner = self.inner.lock().unwrap();
        let lent = inner.lent.as_deref()?;
        inner.marks.iter().find(|m| m.session == lent).cloned()
    }

    /// The VRChat the app follows among `running`: the side session it is lent to, else one that is no side session's
    /// (the person's, or the main session's), else none.
    pub fn app_pid(&self, running: &[u32]) -> Option<u32> {
        let sides = self.side_pids();
        if let Some(pid) = self.lent().and_then(|m| m.pid).filter(|p| running.contains(p)) {
            return Some(pid);
        }
        running.iter().copied().find(|p| !sides.contains(p))
    }

    /// Whether the app leaves this log alone: a side session's that the app is not lent to. A log is a side session's when
    /// the session knows it, or when its arguments name a side session's `--osc` ports; a log without its arguments yet,
    /// new while a side VRChat starts, waits too (a person's VRChat starting in those seconds is followed a moment later).
    pub fn skips_log(&self, path: &Path) -> bool {
        let mut inner = self.inner.lock().unwrap();
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
pub fn datagram_filter(sides: std::sync::Arc<Sides>) -> orbiters_core::osc::DatagramFilter {
    let owners: Mutex<(HashMap<u16, u32>, Option<Instant>)> = Mutex::new((HashMap::new(), None));
    std::sync::Arc::new(move |from: SocketAddr| {
        let side_pids = sides.side_pids();
        if side_pids.is_empty() {
            return true;
        }
        let owner = {
            let mut owners = owners.lock().unwrap();
            if !owners.0.contains_key(&from.port()) && owners.1.is_none_or(|t| t.elapsed() > Duration::from_secs(1)) {
                *owners = (super::win::udp_owners(), Some(Instant::now()));
            }
            owners.0.get(&from.port()).copied()
        };
        let lent = sides.lent().and_then(|m| m.pid);
        !owner.is_some_and(|pid| side_pids.contains(&pid) && Some(pid) != lent)
    })
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

/// Who a VRChat's log says is signed in: `Preforming post login checks for <name> (usr_…)` (sic, VRChat's spelling).
pub fn signed_in(text: &str) -> Option<(String, String)> {
    text.lines().rev().find_map(|l| {
        let rest = l.split_once("post login checks for ")?.1;
        let (name, id) = rest.rsplit_once(" (")?;
        let id = id.trim_end().strip_suffix(')')?;
        id.starts_with("usr_").then(|| (id.to_string(), name.to_string()))
    })
}

/// Whether a VRChat's log says it waits at its sign-in page: no saved account (`No cached credentials found`), or a saved
/// one refused, and nobody signed in since.
pub fn needs_sign_in(text: &str) -> bool {
    let asked = text.rfind("No cached credentials found").or_else(|| text.rfind("AuthSelectionPage OnPageShown"));
    let signed = text.rfind("post login checks for ");
    asked.is_some_and(|a| signed.is_none_or(|s| s < a))
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
}

impl Eac {
    /// Whether a launch is safe now. With a VRChat running, the service and driver serve it and a launch beside it works
    /// at once (measured 2026-11-15, three times). With none, they must be stopped or gone first: a launch while the
    /// driver of a VRChat that just quit is still running fails with "EAC unload timeout" (30005, 2026-11-13).
    pub fn may_launch(&self) -> bool {
        if self.launcher_runs {
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
        let out = "2026.11.15 01:00:32 Debug      -  [Authentication - X] No cached credentials found, transitioning to provider selection.\n\
                   2026.11.15 01:00:32 Warning    -  VP AuthSelectionPage OnPageShown()\n";
        assert!(needs_sign_in(out));
        assert_eq!(signed_in(out), None);
        let in_ = format!("{out}2026.11.15 01:09:12 Debug      -  [Authentication - X] Preforming post login checks for Orbiters (usr_dfc96b2c-1aec-4775-9557-eef94a7f02c3)\n");
        assert!(!needs_sign_in(&in_));
        assert_eq!(signed_in(&in_), Some(("usr_dfc96b2c-1aec-4775-9557-eef94a7f02c3".into(), "Orbiters".into())));
        let spaced = "x Preforming post login checks for blackorbit 552a (usr_f5ff7d6e-58a9-49e9-8c73-a142139d552a)";
        assert_eq!(signed_in(spaced).unwrap().1, "blackorbit 552a");
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
        let eac = |service: &str, driver: &str, vrchat_runs, launcher_runs| Eac { service: service.into(), driver: driver.into(), vrchat_runs, launcher_runs };
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
}
