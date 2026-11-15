//! Every VRChat running on this computer (the roster), and the one the app follows: its **capture target**.
//!
//! Several VRChats run at once when the VRChat bridge drives sessions of its own beside the person's (bridge/sessions.rs),
//! or when a session's VRChat is left running. 2026-11-15 13:48: the world tour's VRChat (profile 1, PID 8244, left by an
//! earlier run of the app and held by no session after the update) ran beside Enzo's own; every watcher of the app took
//! "a VRChat that is no session's" for his, and its datagrams reached the app unfiltered. Here each VRChat is known by its
//! own evidence: its process, its own log (created with the process), that log's command line (`--profile`, `--osc`, which
//! only the bridge passes to a side session), who signed in, the world, desktop or VR, its OSCQuery service and its camera.
//! The app's process watcher, log watcher, OSC (input port, tree, the datagrams it takes) and capture then follow one of
//! them: the person's own by default, or the one they choose on the Capture page ([`super::Controller::choose_vrchat`]).

use crate::bridge::sessions::{self, Driver, VrchatRun};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A log made up to this long before its process started is not its own (clock steps between them).
const BEFORE_START_S: u64 = 5;
/// VRChat makes its log within a minute of starting (measured 2 s: process 12:25:13, log 12:25:15).
const AFTER_START_S: u64 = 60;
/// Read from a log per look at most.
const CHUNK: u64 = 4 << 20;

/// What a VRChat's log said of it so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LogFacts {
    pub args: Option<Vec<String>>,
    pub read_to: u64,
    pub account: Option<(String, String)>,
    pub world: Option<String>,
    pub query: Option<u16>,
    pub osc_in: Option<u16>,
    pub vr_disabled: bool,
}

impl LogFacts {
    /// Takes in a new part of the log (whole lines).
    pub(crate) fn read(&mut self, text: &str) {
        if self.args.is_none() {
            self.args = sessions::complete_args(text);
        }
        if let Some(account) = sessions::signed_in(text) {
            self.account = Some(account);
        }
        if let Some(world) = text.lines().rev().find_map(|l| l.split_once("[Behaviour] Entering Room: ").map(|(_, w)| w.trim().to_string())) {
            self.world = Some(world);
        }
        if self.query.is_none() {
            self.query = sessions::query_port(text);
        }
        if self.osc_in.is_none() {
            self.osc_in = text.lines().find_map(|l| l.split_once(" of type OSC on ").and_then(|(_, p)| p.trim().parse().ok()));
        }
        self.vr_disabled |= text.contains("VR Disabled");
    }

    /// Its profile, its `--osc` ports, and whether the bridge started it for a side session.
    pub(crate) fn marks(&self) -> (u32, Option<(u16, u16)>, bool) {
        let args = self.args.as_deref().unwrap_or_default();
        let profile = args.iter().find_map(|a| a.strip_prefix("--profile=").and_then(|p| p.parse().ok())).unwrap_or(0);
        let osc = args.iter().find_map(|a| {
            let rest = a.strip_prefix("--osc=")?;
            let mut parts = rest.split(':');
            let into = parts.next()?.parse().ok()?;
            let out = parts.nth(1)?.parse().ok()?;
            Some((into, out))
        });
        let bridge = osc.is_some_and(|o| sessions::started_for(args, profile, o));
        (profile, osc, bridge)
    }

    pub(crate) fn desktop(&self) -> Option<bool> {
        let args = self.args.as_deref()?;
        Some(args.iter().any(|a| a == "--no-vr") || self.vr_disabled)
    }
}

/// Which log is which VRChat's: a process keeps the log it was matched with while both last; a new one takes the log made
/// in the minute after it started (the closest), not another's. A process whose start Windows would not say (0) takes the
/// one log left when it is the one process left.
pub(crate) fn match_logs(procs: &[(u32, u64)], logs: &[(PathBuf, u64)], prior: &HashMap<u32, PathBuf>) -> HashMap<u32, PathBuf> {
    let mut matched: HashMap<u32, PathBuf> = prior.iter().filter(|(pid, log)| procs.iter().any(|(p, _)| p == *pid) && logs.iter().any(|(l, _)| l == *log)).map(|(p, l)| (*p, l.clone())).collect();
    let mut order: Vec<&(u32, u64)> = procs.iter().filter(|(p, _)| !matched.contains_key(p)).collect();
    order.sort_by_key(|(pid, start)| (*start, *pid));
    for (pid, start) in order {
        if *start == 0 {
            continue;
        }
        let taken: Vec<&PathBuf> = matched.values().collect();
        let best = logs
            .iter()
            .filter(|(l, _)| !taken.contains(&l))
            .filter(|(_, created)| *created + BEFORE_START_S >= *start && *created <= start + AFTER_START_S)
            .min_by_key(|(_, created)| created.abs_diff(*start))
            .map(|(l, _)| l.clone());
        if let Some(log) = best {
            matched.insert(*pid, log);
        }
    }
    let unmatched: Vec<u32> = procs.iter().map(|(p, _)| *p).filter(|p| !matched.contains_key(p)).collect();
    if let [pid] = unmatched[..] {
        if procs.iter().any(|(p, s)| *p == pid && *s == 0) {
            let taken: Vec<PathBuf> = matched.values().cloned().collect();
            let left: Vec<&PathBuf> = logs.iter().map(|(l, _)| l).filter(|l| !taken.contains(*l)).collect();
            if let [log] = left[..] {
                matched.insert(pid, log.clone());
            }
        }
    }
    matched
}

/// One VRChat from what its log said (the driver the bridge's sessions know is put in by [`sessions::Sides::set_roster`]).
pub(crate) fn run_of(pid: u32, started_s: u64, log: Option<&Path>, facts: Option<&LogFacts>) -> VrchatRun {
    let empty = LogFacts::default();
    let facts = facts.unwrap_or(&empty);
    let (profile, osc, bridge) = facts.marks();
    VrchatRun {
        pid,
        started_s,
        profile,
        log: log.map(Path::to_path_buf),
        osc_in: osc.map(|o| o.0).or(facts.osc_in).unwrap_or(sessions::VRCHAT_OSC_IN),
        query: facts.query.map(|p| SocketAddr::from(([127, 0, 0, 1], p))),
        account: facts.account.clone(),
        world: facts.world.clone(),
        desktop: facts.desktop(),
        camera: None,
        driver: if bridge { Driver::Bridge } else { Driver::Person },
    }
}

/// The roster as the watcher keeps it between looks.
#[derive(Default)]
pub(crate) struct Roster {
    facts: HashMap<PathBuf, LogFacts>,
    matched: HashMap<u32, PathBuf>,
}

impl Roster {
    /// Every VRChat in `procs` (process, start), with what its log in `dir` said. `camera` asks a VRChat's OSCQuery tree
    /// whether its camera is open (only asked with two VRChats or more: the picker's mark).
    pub(crate) fn scan(&mut self, procs: &[(u32, u64)], dir: Option<&Path>, camera: impl Fn(SocketAddr) -> Option<bool>) -> Vec<VrchatRun> {
        if procs.is_empty() {
            self.matched.clear();
            self.facts.clear();
            return Vec::new();
        }
        let earliest = procs.iter().map(|(_, s)| *s).filter(|s| *s > 0).min().unwrap_or(0);
        let logs: Vec<(PathBuf, u64)> = dir
            .and_then(|d| std::fs::read_dir(d).ok())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.starts_with("output_log_") && name.ends_with(".txt")
            })
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                let created = meta.created().or_else(|_| meta.modified()).ok()?.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs();
                let fresh = meta.modified().ok()?.elapsed().is_ok_and(|d| d < Duration::from_secs(180));
                (created + BEFORE_START_S >= earliest || (earliest == 0 && fresh)).then(|| (e.path(), created))
            })
            .collect();
        self.matched = match_logs(procs, &logs, &self.matched);
        let used: Vec<PathBuf> = self.matched.values().cloned().collect();
        self.facts.retain(|l, _| used.contains(l));
        for log in &used {
            let facts = self.facts.entry(log.clone()).or_default();
            let text = read_from(log, facts.read_to, CHUNK);
            facts.read_to += text.len() as u64;
            facts.read(&text);
        }
        let mut runs: Vec<VrchatRun> = procs
            .iter()
            .map(|(pid, start)| {
                let log = self.matched.get(pid);
                run_of(*pid, *start, log.map(PathBuf::as_path), log.and_then(|l| self.facts.get(l)))
            })
            .collect();
        if runs.len() > 1 {
            for run in &mut runs {
                run.camera = run.query.and_then(&camera);
            }
        }
        runs.sort_by_key(|r| (r.started_s, r.pid));
        runs
    }
}

fn read_from(path: &Path, from: u64, most: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let mut bytes = Vec::new();
    if let Ok(mut file) = std::fs::File::open(path) {
        if file.seek(SeekFrom::Start(from)).is_ok() {
            let _ = file.take(most).read_to_end(&mut bytes);
        }
    }
    let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// What the interface shows of a VRChat (state.rs), from the roster's.
pub(crate) fn shown(run: &VrchatRun, target: Option<u32>, chosen: Option<u32>) -> crate::state::VrchatRunShown {
    use crate::state::{VrchatDriverShown, VrchatRunShown};
    VrchatRunShown {
        pid: run.pid,
        profile: run.profile,
        account: run.account.as_ref().map(|(_, name)| name.clone()),
        world: run.world.clone(),
        desktop: run.desktop,
        camera: run.camera,
        driver: match &run.driver {
            Driver::Person => VrchatDriverShown::Person,
            Driver::Main => VrchatDriverShown::Agent { session: "main".into() }, // i18n: not shown (the session's name, as the bridge calls it)
            Driver::Session(id) => VrchatDriverShown::Agent { session: short_session(id) },
            Driver::Bridge => VrchatDriverShown::Left,
        },
        target: target == Some(run.pid),
        chosen: chosen == Some(run.pid),
    }
}

/// How people know a VRChat: its account's name, else its profile.
pub(crate) fn who(run: &crate::state::VrchatRunShown) -> String {
    run.account.clone().unwrap_or_else(|| orbiters_i18n::tr!("profile {number}", number = run.profile))
}

/// A session id as people see it: its profile and the first characters (`p1-3f2a…`), never the whole key that drives it.
pub(crate) fn short_session(id: &str) -> String {
    match id.split_once('-') {
        Some((profile, rest)) => format!("{profile}-{}…", rest.chars().take(4).collect::<String>()),
        None => id.to_string(),
    }
}

impl super::Controller {
    /// The person chooses which VRChat pictures are taken in (`None`: their own again). Refused during a recording: its
    /// frames, its OSC and its world are one VRChat's.
    pub fn choose_vrchat(&self, pid: Option<u32>) -> anyhow::Result<()> {
        anyhow::ensure!(!self.is_capturing(), "{}", orbiters_i18n::tr("Finish the current capture first."));
        if let Some(pid) = pid {
            anyhow::ensure!(self.sides.roster().iter().any(|r| r.pid == pid), "{}", orbiters_i18n::tr("That VRChat is not running any more."));
        }
        let default = sessions::default_target(&self.sides.roster()).map(|r| r.pid);
        // Choosing their own again is the default, which follows them when it restarts.
        self.sides.choose(pid.filter(|p| Some(*p) != default));
        self.follow_target();
        self.publish_runs();
        Ok(())
    }

    /// Points the app at the VRChat it follows now ([`sessions::Sides::target`]), when that changed: OSC goes to its input
    /// port and its tree, only its datagrams are taken, and what is known of its camera is its own (heard while it was not
    /// followed, or asked of its tree), never the last VRChat's. The process and log watchers follow it on their next look.
    pub(crate) fn follow_target(&self) {
        let target = self.sides.target();
        let pid = target.as_ref().map(|t| t.pid);
        let before = {
            let mut followed = self.followed.lock().unwrap();
            if *followed == pid {
                return;
            }
            std::mem::replace(&mut *followed, pid)
        };
        let now = orbiters_core::clock::now_ns();
        // What the last one said of its camera stays with it, for when it is followed again.
        if let Some(old) = before {
            let view = self.view();
            let received = self.osc_values.lock().unwrap().camera_received;
            match (view.camera_open_said, received) {
                (Some(false), _) => self.sides.hear_camera(old, false, now),
                (Some(true), Some(true)) => self.sides.hear_camera(old, true, now),
                _ => {}
            }
        }
        let default_port = self.with_settings(|s| s.osc.vrchat_port);
        let route = target
            .as_ref()
            .filter(|t| t.osc_in != default_port || self.sides.lent().is_some())
            .and_then(|t| t.query.map(|q| (SocketAddr::from(([127, 0, 0, 1], t.osc_in)), q)));
        let _ = self.with_osc(|o| o.lend(route));
        let heard = pid.and_then(|p| self.sides.camera_heard(p));
        {
            let mut osc = self.osc_values.lock().unwrap();
            osc.camera_command = None;
            osc.camera_commanded.clear();
            osc.camera_received = heard.map(|(open, _)| open);
            osc.camera_closed_at = heard.filter(|(open, _)| !open).map(|(_, at)| at);
            osc.latest = orbiters_core::osc::Latest::default();
        }
        self.update(|s| {
            s.camera_open_said = heard.map(|(open, _)| open);
            s.orbit.camera_panel_seen_ns = None;
            s.camera_tools.qr_generation += 1;
            s.camera_tools.qr = super::camera_tools::Qr::Off;
        });
        self.invalidate_kit(None);
        eprintln!("following VRChat {pid:?} (was {before:?}), its camera heard {heard:?}"); // i18n: not shown (diagnostics)
        if heard.is_none() && pid.is_some() {
            // Its tree says how its camera is, as at an OSC start: shown, never proof for a recording.
            let camera = self.camera_values().into_iter().find_map(|(p, v)| match v {
                orbiters_core::osc::OscValue::Int(mode) if p == super::CAMERA_MODE => Some(mode != 0),
                _ => None,
            });
            if let Some(open) = camera {
                self.update_if(|s| {
                    if s.camera_open_said.is_some() || *self.followed.lock().unwrap() != pid {
                        return false;
                    }
                    s.camera_open_said = Some(open);
                    true
                });
            }
        }
    }

    /// "Open VRChat's camera first", naming the VRChat when several run (the camera to open is the target's).
    pub(crate) fn camera_first(&self) -> String {
        match self.view().vrchat.several_target() {
            Some(target) => orbiters_i18n::tr!("Open the camera in {who}'s VRChat first", who = who(target)),
            None => orbiters_i18n::tr("Open VRChat's camera first").to_string(),
        }
    }

    /// The roster in the snapshot, for the interface.
    pub(crate) fn publish_runs(&self) {
        let runs = self.sides.roster();
        let target = self.sides.target().map(|t| t.pid);
        let chosen = self.sides.chosen();
        let shown: Vec<_> = runs.iter().map(|r| shown(r, target, chosen)).collect();
        self.update_if(|s| {
            if s.vrchat.runs == shown {
                return false;
            }
            s.vrchat.runs = shown;
            true
        });
    }

    /// VRChat names every Stream Camera's Spout sender `VRCSender1` (its KlakSpout plug-in, an older Spout SDK, writes one
    /// shared info block per name: the last VRChat to start streaming is the one a receiver reads). Before a recording, every
    /// other VRChat whose camera streams has its Spout Stream switched off, so that the frames are the target's; they are
    /// switched on again after it ([`Self::unquiet_other_streams`]). Whether it did any.
    pub(crate) fn quiet_other_streams(&self) -> bool {
        let target = self.sides.target().map(|t| t.pid);
        let others: Vec<VrchatRun> = self.sides.roster().into_iter().filter(|r| Some(r.pid) != target).collect();
        let mut quieted = Vec::new();
        for run in others {
            let Some(query) = run.query else { continue };
            let values = orbiters_core::osc::vrchat_camera_values(query, Duration::from_millis(400));
            let open = values.iter().any(|(p, v)| p == super::CAMERA_MODE && matches!(v, orbiters_core::osc::OscValue::Int(m) if *m != 0));
            let streaming = values.iter().any(|(p, v)| p == super::CAMERA_STREAMING && v.truthy());
            if open && streaming {
                let to = SocketAddr::from(([127, 0, 0, 1], run.osc_in));
                if send_streaming(to, false).is_ok() {
                    eprintln!("Spout: VRChat {} streams too: its Spout Stream is off while this recording runs", run.pid); // i18n: not shown (diagnostics)
                    quieted.push(to);
                }
            }
        }
        let any = !quieted.is_empty();
        self.quieted.lock().unwrap().extend(quieted);
        any
    }

    /// Switches the Spout Stream of the VRChats [`Self::quiet_other_streams`] switched off on again.
    pub(crate) fn unquiet_other_streams(&self) {
        for to in std::mem::take(&mut *self.quieted.lock().unwrap()) {
            let _ = send_streaming(to, true);
        }
    }
}

/// `/usercamera/Streaming` to the VRChat taking input at `to`.
fn send_streaming(to: SocketAddr, on: bool) -> std::io::Result<()> {
    let packet = rosc::OscPacket::Message(rosc::OscMessage { addr: "/usercamera/Streaming".into(), args: vec![rosc::OscType::Bool(on)] });
    let bytes = rosc::encoder::encode(&packet).map_err(|e| std::io::Error::other(e.to_string()))?;
    std::net::UdpSocket::bind(("127.0.0.1", 0))?.send_to(&bytes, to).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The heads of the two logs of 2026-11-15 13:48, as VRChat wrote them (abridged to what is read).
    const TOUR_LOG: &str = "2026.11.15 12:25:17 Debug      -  Launching with args: 4\n\
2026.11.15 12:25:17 Debug      -  Arg: C:\\Program Files (x86)\\Steam\\steamapps\\common\\VRChat\\VRChat.exe\n\
2026.11.15 12:25:17 Debug      -  Arg: --no-vr\n\
2026.11.15 12:25:17 Debug      -  Arg: --profile=1\n\
2026.11.15 12:25:17 Debug      -  Arg: --osc=9010:127.0.0.1:9011\n\
2026.11.15 12:25:19 Debug      -  Advertising Service VRChat-Client-222420 of type OSCQuery on 50361\n\
2026.11.15 12:25:19 Debug      -  Advertising Service VRChat-Client-222420 of type OSC on 9010\n\
2026.11.15 12:25:21 Debug      -  [Authentication - x] Preforming post login checks for Orbiters (usr_dfc96b2c-1aec-4775-9557-eef94a7f02c3)\n\
2026.11.15 13:25:59 Debug      -  [Behaviour] Entering Room: Audience Anarchy\n";
    const OWN_LOG: &str = "2026.11.15 13:38:46 Debug      -  Launching with args: 6\n\
2026.11.15 13:38:46 Debug      -  Arg: C:\\Program Files (x86)\\Steam\\steamapps\\common\\VRChat\\VRChat.exe\n\
2026.11.15 13:38:46 Debug      -  Arg: vrchat://launch?ref=VRCQuickLauncher&id=wrld_776a6a37-f307-43f4-aeef-8f151f38dba4:813\n\
2026.11.15 13:38:46 Debug      -  Arg: --watch-worlds\n\
2026.11.15 13:38:46 Debug      -  Arg: --profile=0\n\
2026.11.15 13:38:46 Debug      -  Arg: --no-vr\n\
2026.11.15 13:38:46 Debug      -  Arg: --startup-begin-ts=49235087616\n\
2026.11.15 13:38:49 Debug      -  Advertising Service VRChat-Client-8C7341 of type OSCQuery on 61246\n\
2026.11.15 13:38:49 Debug      -  Advertising Service VRChat-Client-8C7341 of type OSC on 9000\n\
2026.11.15 13:38:50 Debug      -  [Authentication - x] Preforming post login checks for blackorbit (usr_1339e525-d75d-4832-9104-98dd89287681)\n\
2026.11.15 13:39:42 Debug      -  [Behaviour] Entering Room: FRONTLINE KIZUNA\n";

    fn facts(text: &str) -> LogFacts {
        let mut f = LogFacts::default();
        f.read(text);
        f
    }

    #[test]
    fn each_vrchat_is_known_by_its_own_log() {
        let tour = run_of(8244, 1_794_745_513, None, Some(&facts(TOUR_LOG)));
        assert_eq!((tour.profile, tour.osc_in, tour.driver.clone()), (1, 9010, Driver::Bridge), "left by the bridge, never the person's");
        assert_eq!(tour.account.as_ref().map(|a| a.1.as_str()), Some("Orbiters"));
        assert_eq!(tour.world.as_deref(), Some("Audience Anarchy"));
        assert_eq!(tour.query, Some(SocketAddr::from(([127, 0, 0, 1], 50361))));
        assert_eq!(tour.desktop, Some(true));
        let own = run_of(37076, 1_794_749_922, None, Some(&facts(OWN_LOG)));
        assert_eq!((own.profile, own.osc_in, own.driver.clone()), (0, 9000, Driver::Person));
        assert_eq!(own.account.as_ref().map(|a| a.1.as_str()), Some("blackorbit"));
        assert_eq!(own.world.as_deref(), Some("FRONTLINE KIZUNA"));
        // His own is the target among the two, whichever started first.
        let runs = [tour.clone(), own.clone()];
        assert_eq!(sessions::default_target(&runs).map(|r| r.pid), Some(37076));
        let runs = [own, tour];
        assert_eq!(sessions::default_target(&runs).map(|r| r.pid), Some(37076));
    }

    /// A VR session of the bridge's (nofocus, on the VR sandbox): its VRChat starts without `--no-vr` but with its profile's
    /// `--profile` and `--osc`, as `LaunchPlan` writes them. It is an agent's, in VR, never the person's: held by a session or
    /// left by one, it is never the default target, even when it is the only VRChat in VR or the only one running.
    #[test]
    fn a_vr_sessions_vrchat_is_an_agents_in_vr_never_the_persons() {
        let plan = crate::bridge::game::LaunchPlan { world_id: String::new(), instance: String::new(), width: 1280, height: 720, profile: Some(2), osc: sessions::ports_of(2), vr: true };
        let mut log = format!("2026.11.15 15:40:00 Debug      -  Launching with args: {}\n", plan.args().len() + 1);
        log.push_str("2026.11.15 15:40:00 Debug      -  Arg: C:\\Program Files (x86)\\Steam\\steamapps\\common\\VRChat\\VRChat.exe\n");
        for arg in plan.args() {
            log.push_str(&format!("2026.11.15 15:40:00 Debug      -  Arg: {arg}\n"));
        }
        let vr = run_of(51000, 1_794_757_200, None, Some(&facts(&log)));
        assert_eq!((vr.profile, vr.osc_in, vr.desktop, vr.driver.clone()), (2, 9020, Some(false), Driver::Bridge));
        assert_eq!(sessions::default_target(std::slice::from_ref(&vr)).map(|r| r.pid), None, "alone, still not the person's");
        let own = run_of(37076, 1_794_749_922, None, Some(&facts(OWN_LOG)));
        let sides = sessions::Sides::default();
        sides.put(sessions::SideMark { session: "p2-0123456789ab".into(), profile: 2, osc_in: 9020, osc_out: 9021, pid: Some(51000), log: None, query: None });
        sides.set_roster(vec![vr, own]);
        let runs = sides.roster();
        assert_eq!(runs.iter().find(|r| r.pid == 51000).map(|r| r.driver.clone()), Some(Driver::Session("p2-0123456789ab".into())));
        assert_eq!(sides.target().map(|t| t.pid), Some(37076), "his own desktop VRChat is the target");
        let shown = shown(&runs[0], Some(37076), None);
        assert_eq!((shown.desktop, shown.driver), (Some(false), crate::state::VrchatDriverShown::Agent { session: "p2-0123…".into() }));
    }

    #[test]
    fn a_process_takes_the_log_made_as_it_started() {
        // 12:24:58 a start that failed; 12:25:13 the tour's VRChat, log 12:25:15; 13:38:42 his own, log 13:38:44.
        let logs = vec![(PathBuf::from("a-12-24-58"), 1000), (PathBuf::from("b-12-25-15"), 1017), (PathBuf::from("c-13-38-44"), 5446)];
        let procs = [(8244, 1015), (37076, 5444)];
        let m = match_logs(&procs, &logs, &HashMap::new());
        assert_eq!(m.get(&8244), Some(&PathBuf::from("b-12-25-15")));
        assert_eq!(m.get(&37076), Some(&PathBuf::from("c-13-38-44")));
        // Kept while both last; a process whose start is unknown takes the one log left only when it is the one left.
        let m2 = match_logs(&[(8244, 1015), (99, 0)], &logs, &m);
        assert_eq!(m2.get(&8244), Some(&PathBuf::from("b-12-25-15")));
        assert_eq!(m2.get(&99), None, "two logs left: none is taken");
        let m3 = match_logs(&[(99, 0)], &logs[1..2], &HashMap::new());
        assert_eq!(m3.get(&99), Some(&PathBuf::from("b-12-25-15")));
    }

    #[test]
    fn people_see_a_short_session_never_its_key() {
        assert_eq!(short_session("p1-3f2a9c0d1e2f"), "p1-3f2a…");
        assert_eq!(short_session("main"), "main");
    }
}
