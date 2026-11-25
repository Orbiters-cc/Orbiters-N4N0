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
use orbiters_core::vrchat_log::{Lines, LogEvent, LogWatcher};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// VRChat makes its log within a minute of starting (measured 2 s: process 12:25:13, log 12:25:15). In nanoseconds, as the
/// system keeps both times: a log of the same second may be from before the process, and then is not its own.
const AFTER_START: u64 = 60_000_000_000;
const SECOND: u64 = 1_000_000_000;
/// A log made up to this long before its process started is shown as its own where it has none from after it (a clock set back, a
/// coarse date), and proves nothing. In whole seconds.
const BEFORE_START_S: u64 = 5;
/// Read from a log per look at most.
const CHUNK: u64 = 4 << 20;
/// How many looks a log is read whole in at most.
const WHOLE_LOOKS: usize = 64;

/// What a VRChat's log said of it so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LogFacts {
    pub args: Option<Vec<String>>,
    /// Who it says is signed in, as far as it was read.
    pub account: orbiters_core::vrchat_log::Account,
    pub world: Option<String>,
    pub query: Option<u16>,
    pub osc_in: Option<u16>,
    pub vr_disabled: bool,
    /// It could not be read at the last look: read anew at the next.
    pub unread: bool,
    /// More of it is there than a look reads, or it is in the middle of a line too long to keep: what it says last is not known
    /// yet.
    pub behind: bool,
}

/// A log as it is followed here: what it said, and how far it was read.
#[derive(Default)]
struct Followed {
    facts: LogFacts,
    lines: Lines,
}

impl Followed {
    /// Reads on, [`CHUNK`] at most. Whether more of it is there.
    fn read_on(&mut self, log: &Path) -> bool {
        // Cut shorter, or not to be read: what it said before is not what it says now.
        let Ok((file, len)) = std::fs::File::open(log).and_then(|f| f.metadata().map(|m| m.len()).map(|len| (f, len))) else { return self.unreadable() };
        if std::mem::take(&mut self.facts.unread) || len < self.lines.position() {
            *self = Self::default();
        }
        let mut text = String::new();
        let read = self.lines.read(&file, len, CHUNK, |line| {
            text.push_str(&String::from_utf8_lossy(line));
            text.push('\n');
        });
        self.facts.read(&text);
        let Ok(to_end) = read else { return self.unreadable() };
        self.facts.behind = !to_end || self.lines.skipping();
        !to_end
    }

    fn unreadable(&mut self) -> bool {
        self.facts.account.sure = false;
        self.facts.unread = true;
        false
    }
}

impl LogFacts {
    /// Takes in a new part of the log (whole lines).
    pub(crate) fn read(&mut self, text: &str) {
        if self.args.is_none() {
            self.args = sessions::complete_args(text);
        }
        self.account.read(text);
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

/// Whether a log made at `created` can be that of a VRChat started at `start` (Unix nanoseconds): made at its start or after it,
/// within the minute. One made before it is another's, however little.
pub(super) fn made_by(start: u64, created: u64) -> bool {
    start > 0 && created >= start && created <= start + AFTER_START
}

/// Whether a log made at `created` may be shown as that of a VRChat started at `start`: a moment before it too.
fn shown_by(start: u64, created: u64) -> bool {
    let (start_s, created_s) = (start / SECOND, created / SECOND);
    start > 0 && created_s + BEFORE_START_S >= start_s && created_s <= start_s + AFTER_START / SECOND
}

/// When the file was made (Unix nanoseconds), as precisely as the system says.
pub(super) fn made_at(meta: &std::fs::Metadata) -> Option<u64> {
    u64::try_from(meta.created().ok()?.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_nanos()).ok()
}

/// Which log is which VRChat's: a process keeps the log it was matched with while both last (and it is that process's: a number
/// used again has none of the last one's); a new one takes the log made in the minute after it started (the closest), never one
/// from before it. A process whose start Windows would not say (0) takes the one log left when it is the one process left.
pub(crate) fn match_logs(procs: &[(u32, u64)], logs: &[(PathBuf, u64)], prior: &HashMap<u32, PathBuf>) -> HashMap<u32, PathBuf> {
    let kept = |pid: &u32, log: &PathBuf| procs.iter().any(|(p, start)| p == pid && logs.iter().any(|(l, created)| l == log && (*start == 0 || created >= start)));
    let mut matched: HashMap<u32, PathBuf> = prior.iter().filter(|(pid, log)| kept(pid, log)).map(|(p, l)| (*p, l.clone())).collect();
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
            .filter(|(_, created)| made_by(*start, *created))
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

/// Which log each VRChat is followed and shown by: its own ([`match_logs`]); where it has none, the closest one left from a moment
/// before it started ([`shown_by`]), kept until one comes that no other VRChat here could have made.
pub(crate) fn follow_logs(procs: &[(u32, u64)], logs: &[(PathBuf, u64)], prior: &HashMap<u32, PathBuf>) -> HashMap<u32, PathBuf> {
    let others_too = |pid: u32, created: u64| procs.iter().any(|(other, start)| *other != pid && made_by(*start, created));
    let kept = |pid: &u32, log: &PathBuf| {
        let Some((_, start)) = procs.iter().find(|(p, _)| p == pid) else { return false };
        logs.iter().any(|(l, created)| l == log && shown_by(*start, *created) && !made_by(*start, *created))
            && !logs.iter().any(|(_, created)| made_by(*start, *created) && !others_too(*pid, *created))
    };
    let before: HashMap<u32, PathBuf> = prior.iter().filter(|(pid, log)| kept(pid, log)).map(|(pid, log)| (*pid, log.clone())).collect();
    let procs_left: Vec<(u32, u64)> = procs.iter().filter(|(pid, _)| !before.contains_key(pid)).copied().collect();
    let logs_left: Vec<(PathBuf, u64)> = logs.iter().filter(|(log, _)| !before.values().any(|l| l == log)).cloned().collect();
    let mut matched = match_logs(&procs_left, &logs_left, prior);
    matched.extend(before);
    let mut order: Vec<(u32, u64)> = procs_left.into_iter().filter(|(pid, _)| !matched.contains_key(pid)).collect();
    order.sort_by_key(|(pid, start)| (*start, *pid));
    for (pid, start) in order {
        let best = logs
            .iter()
            .filter(|(log, created)| shown_by(start, *created) && !matched.values().any(|l| l == log))
            .min_by_key(|(_, created)| created.abs_diff(start))
            .map(|(log, _)| log.clone());
        matched.extend(best.map(|log| (pid, log)));
    }
    matched
}

/// A VRChat that runs, or ran beside one that does (Unix nanoseconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Life {
    pid: u32,
    /// 0 when the system would not say.
    start: u64,
    /// First seen, and when it was seen to be gone.
    since: u64,
    until: Option<u64>,
}

impl Life {
    fn is(&self, pid: u32, start: u64) -> bool {
        // A start the system says at one look and not at another is of the same process while it was not seen to be gone.
        self.pid == pid && (self.start == start || (self.until.is_none() && (start == 0 || (self.start == 0 && start <= self.since))))
    }

    /// Whether it could have made a log of `created`.
    fn made(&self, created: u64) -> bool {
        made_by(self.start, created) && self.until.is_none_or(|until| created <= until)
    }

    /// Whether some log could be either's.
    fn beside(&self, other: &Life) -> bool {
        let from = self.start.max(other.start);
        let to = |life: &Life| life.until.unwrap_or(u64::MAX).min(if life.start == 0 { u64::MAX } else { life.start.saturating_add(AFTER_START) });
        from <= to(self).min(to(other))
    }
}

/// The VRChats seen on this computer, those that ended while another that could have made their log runs on among them: a log that
/// was in doubt between two of them is in doubt when one is gone. One for every look at them here ([`Roster::seeing`]).
#[derive(Default)]
pub struct Seen(Mutex<Vec<Life>>);

impl Seen {
    /// Takes in the VRChats that run at `now`; the ones that run, and the ended ones that count.
    fn look(&self, procs: &[(u32, u64)], now: u64) -> Vec<Life> {
        let mut lives = self.0.lock().unwrap();
        let mut new: Vec<(u32, u64)> = procs.to_vec();
        for life in lives.iter_mut() {
            match new.iter().position(|(pid, start)| life.is(*pid, *start)) {
                Some(i) => {
                    let (_, start) = new.remove(i);
                    life.start = life.start.max(start);
                    life.until = None;
                }
                None => {
                    life.until.get_or_insert(now);
                }
            }
        }
        lives.extend(new.into_iter().map(|(pid, start)| Life { pid, start, since: now, until: None }));
        // (A look that found none may be one that failed: nothing is forgotten by it.)
        if !procs.is_empty() {
            let running: Vec<Life> = lives.iter().filter(|life| life.until.is_none()).copied().collect();
            lives.retain(|life| life.until.is_none() || running.iter().any(|one| one.beside(life)));
        }
        lives.clone()
    }
}

/// The logs that are a VRChat's own for certain (by its process and start): made at or after its start, and no other VRChat here,
/// running or ended, or log could be taken for the other half of the pair. What a log says of whose a VRChat is counts for these
/// alone.
fn proven_logs(lives: &[Life], logs: &[(PathBuf, u64)]) -> HashMap<(u32, u64), PathBuf> {
    // Pair by pair: whoever has one candidate left takes it from the others'. A VRChat whose start is not known could be any
    // log's.
    if lives.iter().any(|life| life.start == 0) {
        return HashMap::new();
    }
    let mut left: Vec<(&Life, Vec<&PathBuf>)> = lives.iter().map(|life| (life, logs.iter().filter(|(_, created)| life.made(*created)).map(|(log, _)| log).collect())).collect();
    let mut found = HashMap::new();
    while let Some(i) = left.iter().position(|(_, candidates)| candidates.len() == 1 && left.iter().filter(|(_, other)| *other == *candidates).count() == 1) {
        let (life, candidates) = left.remove(i);
        for (_, others) in &mut left {
            others.retain(|log| *log != candidates[0]);
        }
        found.insert((life.pid, life.start), candidates[0].clone());
    }
    found
}

/// The VRChats of `procs` (none other was there) whose matched log is their own for certain ([`proven_logs`]).
#[cfg(test)]
pub(crate) fn certain(procs: &[(u32, u64)], logs: &[(PathBuf, u64)], matched: &HashMap<u32, PathBuf>) -> Vec<u32> {
    let lives: Vec<Life> = procs.iter().map(|(pid, start)| Life { pid: *pid, start: *start, since: 0, until: None }).collect();
    proven_logs(&lives, logs).into_iter().filter(|((pid, _), log)| matched.get(pid) == Some(log)).map(|((pid, _), _)| pid).collect()
}

/// One VRChat from what its log said (the driver the bridge's sessions know is put in by [`sessions::Sides::set_roster`]).
pub(crate) fn run_of(pid: u32, started_ns: u64, log: Option<&Path>, facts: Option<&LogFacts>) -> VrchatRun {
    let empty = LogFacts::default();
    let facts = facts.unwrap_or(&empty);
    let (profile, osc, bridge) = facts.marks();
    VrchatRun {
        pid,
        started_ns,
        profile,
        profile_known: facts.args.is_some(),
        log: log.map(Path::to_path_buf),
        osc_in: osc.map(|o| o.0).or(facts.osc_in).unwrap_or(sessions::VRCHAT_OSC_IN),
        query: facts.query.map(|p| SocketAddr::from(([127, 0, 0, 1], p))),
        account: facts.account.who.clone(),
        account_sure: facts.account.sure && !facts.behind,
        log_sure: false,
        signed_out: facts.account.signed_out,
        one_profile: facts.args.as_deref().is_some_and(|args| orbiters_core::vrchat_launch::profile_told(args) == Some(profile)),
        world: facts.world.clone(),
        desktop: facts.desktop(),
        camera: None,
        driver: if bridge { Driver::Bridge } else { Driver::Person },
    }
}

/// The roster as the watcher keeps it between looks.
#[derive(Default)]
pub(crate) struct Roster {
    logs: HashMap<PathBuf, Followed>,
    matched: HashMap<u32, PathBuf>,
    /// The VRChats (process, start) whose log was once found to be theirs for certain: a later one changes nothing of that.
    certain: Vec<(u32, u64)>,
    /// What every look here saw run.
    seen: Arc<Seen>,
}

impl Roster {
    /// One that shares what `seen` saw (the looks at the VRChats here are several: the watcher's, and one right before a private
    /// place is handed over).
    pub(crate) fn seeing(seen: Arc<Seen>) -> Self {
        Roster { seen, ..Roster::default() }
    }

    pub(crate) fn seen(&self) -> Arc<Seen> {
        self.seen.clone()
    }

    /// [`Self::scan`] until every log is read up to its end, or it is too much to read now.
    pub(crate) fn scan_whole(&mut self, procs: &[(u32, u64)], dir: Option<&Path>) -> Vec<VrchatRun> {
        self.look(procs, dir);
        for _ in 0..WHOLE_LOOKS {
            if !self.read_on() {
                break;
            }
        }
        self.runs(procs, |_| None)
    }

    /// Every VRChat in `procs` (process, start), with what its log in `dir` said. `camera` asks a VRChat's OSCQuery tree
    /// whether its camera is open (only asked with two VRChats or more: the picker's mark).
    pub(crate) fn scan(&mut self, procs: &[(u32, u64)], dir: Option<&Path>, camera: impl Fn(SocketAddr) -> Option<bool>) -> Vec<VrchatRun> {
        self.look(procs, dir);
        self.read_on();
        self.runs(procs, camera)
    }

    /// Reads every log followed on, a look's worth of each. Whether more of one is there.
    fn read_on(&mut self) -> bool {
        self.logs.iter_mut().fold(false, |more, (log, followed)| followed.read_on(log) | more)
    }

    /// Matches the VRChats in `procs` with the logs in `dir`.
    fn look(&mut self, procs: &[(u32, u64)], dir: Option<&Path>) {
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        let lives = self.seen.look(procs, now);
        if procs.is_empty() {
            self.matched.clear();
            self.logs.clear();
            self.certain.clear();
            return;
        }
        let earliest = procs.iter().map(|(_, s)| *s).filter(|s| *s > 0).min().unwrap_or(0);
        let earliest_seen = lives.iter().map(|life| life.start).filter(|s| *s > 0).min().unwrap_or(earliest);
        let all: Vec<(PathBuf, u64)> = dir
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
                let created = made_at(&meta)?;
                let fresh = meta.modified().ok()?.elapsed().is_ok_and(|d| d < Duration::from_secs(180));
                (created / SECOND + BEFORE_START_S >= earliest_seen / SECOND || (earliest == 0 && fresh)).then(|| (e.path(), created))
            })
            .collect();
        let logs: Vec<(PathBuf, u64)> = all.iter().filter(|(_, created)| created / SECOND + BEFORE_START_S >= earliest / SECOND).cloned().collect();
        let before = std::mem::take(&mut self.matched);
        self.matched = follow_logs(procs, &logs, &before);
        // Proven to be another one's than the closest: that one. What was certain stays.
        let proven = proven_logs(&lives, &all);
        for one in procs.iter().filter(|one| !self.certain.contains(one)) {
            let Some(log) = proven.get(one) else { continue };
            if self.matched.get(&one.0) == Some(log) || procs.iter().any(|other| self.certain.contains(other) && self.matched.get(&other.0) == Some(log)) {
                continue;
            }
            self.matched.retain(|_, other| other != log);
            self.matched.insert(one.0, log.clone());
        }
        self.certain.retain(|(pid, start)| procs.contains(&(*pid, *start)) && self.matched.contains_key(pid) && self.matched.get(pid) == before.get(pid));
        for one in procs {
            if !self.certain.contains(one) && proven.get(one).is_some_and(|log| self.matched.get(&one.0) == Some(log)) {
                self.certain.push(*one);
            }
        }
        let Roster { logs, matched, .. } = self;
        logs.retain(|log, _| matched.values().any(|used| used == log));
        for log in matched.values() {
            logs.entry(log.clone()).or_default();
        }
    }

    fn runs(&self, procs: &[(u32, u64)], camera: impl Fn(SocketAddr) -> Option<bool>) -> Vec<VrchatRun> {
        let mut runs: Vec<VrchatRun> = procs
            .iter()
            .map(|(pid, start)| {
                let log = self.matched.get(pid);
                let mut run = run_of(*pid, *start, log.map(PathBuf::as_path), log.and_then(|l| self.logs.get(l)).map(|l| &l.facts));
                run.log_sure = self.certain.contains(&(*pid, *start));
                run
            })
            .collect();
        if runs.len() > 1 {
            for run in &mut runs {
                run.camera = run.query.and_then(&camera);
            }
        }
        runs.sort_by_key(|r| (r.started_ns, r.pid));
        runs
    }
}

/// VRChat's log as the app follows it: its target's ([`sessions::Sides::target`]), never a side session's or another VRChat's.
/// Whatever the app reads of the world it is in, it reads through one of these.
pub(crate) struct TargetLog {
    log: LogWatcher,
    dir: Option<PathBuf>,
    sides: Arc<sessions::Sides>,
    target: Option<(u32, Option<PathBuf>)>,
}

impl TargetLog {
    /// Of the logs in `dir` (`None`: VRChat's own folder).
    pub(crate) fn new(dir: Option<PathBuf>, sides: Arc<sessions::Sides>) -> Self {
        Self { log: Self::watcher(&dir, &sides), dir, sides, target: None }
    }

    fn watcher(dir: &Option<PathBuf>, sides: &Arc<sessions::Sides>) -> LogWatcher {
        let mut log = LogWatcher::new(dir.clone());
        let sides = sides.clone();
        log.set_skip(Arc::new(move |path: &Path| sides.skips_log(path)));
        log
    }

    /// [`LogWatcher::poll`] of the target's log.
    pub(crate) fn poll(&mut self) -> Vec<LogEvent> {
        let target = self.sides.target().map(|t| (t.pid, t.log));
        if target != self.target {
            // Another VRChat followed: its log from its last world, as at a start (never its whole past as news).
            let followed = &self.log.state.log_file;
            if target.as_ref().is_some_and(|(_, log)| log.is_some() && followed.is_some() && log != followed) {
                self.log = Self::watcher(&self.dir, &self.sides);
            }
            self.target = target;
        }
        self.log.poll()
    }
}

impl std::ops::Deref for TargetLog {
    type Target = LogWatcher;

    fn deref(&self) -> &LogWatcher {
        &self.log
    }
}

/// What the interface shows of a VRChat (state.rs), from the roster's.
pub(crate) fn shown(run: &VrchatRun, target: Option<u32>, chosen: Option<u32>) -> crate::state::VrchatRunShown {
    use crate::state::{VrchatDriverShown, VrchatRunShown};
    VrchatRunShown {
        pid: run.pid,
        profile: run.profile,
        profile_known: run.profile_known,
        account: run.account.as_ref().map(|(_, name)| name.clone()),
        account_id: run.account.as_ref().filter(|_| run.profile_known && run.one_profile && !run.signed_out && run.log_sure && run.account_sure).map(|(id, _)| id.clone()),
        logged: run.profile_known && run.log_sure,
        signed_out: run.signed_out && run.profile_known && run.one_profile,
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
        // After the endpoint changed, before what is known of the camera is reset: an answer asked of the last VRChat,
        // before or during the change, is not shown now (verify-controller, C-8).
        let _ = self.with_osc(|o| o.lend(route));
        self.camera_route_changed();
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

    /// C-6 / CTL2-6: the log is read on by the file's bytes (a byte that is not UTF-8 is one byte, not three of text), and a line
    /// longer than a look is passed over once it ended, the next one read. BRG-4: while it has not ended it is read once, proves
    /// nothing, and its tail is never read as a line of its own (COR-1).
    #[test]
    fn the_log_is_read_on_in_file_bytes_and_past_an_overlong_line() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("orbiters-roster-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("output_log_2026-11-15_13-38-44.txt");
        let world = |name: &[u8]| [&b"2026.11.15 13:40:00 Debug      -  [Behaviour] Entering Room: "[..], name].concat();
        std::fs::write(&path, [OWN_LOG.as_bytes(), &world(b"n\xE9t\xE9"), b"\n2026.11.15 13:40:01 Debug      -  partial"].concat()).unwrap();
        let born = made_at(&std::fs::metadata(&path).unwrap()).unwrap();
        let mut roster = Roster::default();
        let look = |roster: &mut Roster| {
            let run = shown(&roster.scan_whole(&[(7, born)], Some(&dir))[0], None, None);
            (run.world, run.account_id)
        };
        let me = || Some("usr_1339e525-d75d-4832-9104-98dd89287681".to_string());
        assert_eq!(look(&mut roster), (Some("n\u{fffd}t\u{fffd}".into()), me()));
        let mut log = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        // A line that does not end, longer than several looks, with what would be VRChat's record at each place a read could end.
        log.write_all(&vec![b'x'; (2 * CHUNK) as usize]).unwrap();
        assert_eq!(look(&mut roster), (Some("n\u{fffd}t\u{fffd}".into()), None), "in the middle of a line: nothing is proven");
        let position = roster.logs[&path].lines.position();
        assert_eq!(position, std::fs::metadata(&path).unwrap().len(), "read once, to its end");
        assert!(!roster.read_on() && roster.logs[&path].lines.position() == position);
        log.write_all(b"2026.11.15 13:41:00 Debug      -  [Authentication - x] Preforming post login checks for Other (usr_2)\n").unwrap();
        assert_eq!(look(&mut roster).1, me(), "its end is no line of VRChat's");
        log.write_all(&[&world(b"after")[..], b"\n"].concat()).unwrap();
        assert_eq!(look(&mut roster), (Some("after".into()), me()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// COR-1: a line that is no record of VRChat's (a further line of a script's message) names no account, whatever it says.
    #[test]
    fn a_line_that_is_no_record_names_no_account() {
        let dir = std::env::temp_dir().join(format!("orbiters-roster-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("output_log_2026-11-15_13-38-44.txt");
        for fake in ["[Authentication - x] Preforming post login checks for Other (usr_2)", "Preforming post login checks for Other (usr_2)", "x [Authentication - x] Preforming post login checks for Other (usr_2)"] {
            std::fs::write(&path, format!("{OWN_LOG}2026.11.15 13:50:00 Debug      -  [World] a message\n{fake}\n")).unwrap();
            let born = made_at(&std::fs::metadata(&path).unwrap()).unwrap();
            let run = shown(&Roster::default().scan_whole(&[(7, born)], Some(&dir))[0], None, None);
            assert_eq!((run.account_id, run.account.as_deref()), (None, Some("blackorbit")), "{fake}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// COR-1: the words of VRChat's sign-in in a record of a world's script, framed as VRChat's own: they confirm nobody, and a
    /// private place of the account they name is not handed to that game.
    #[test]
    fn a_world_s_record_hands_no_private_place_over() {
        use orbiters_core::vrchat_launch::{way, To, Way};
        let dir = std::env::temp_dir().join(format!("orbiters-roster-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("output_log_2026-11-15_13-38-44.txt");
        let to = |after: &str, owner: &str| {
            std::fs::write(&path, format!("{OWN_LOG}{after}")).unwrap();
            let born = made_at(&std::fs::metadata(&path).unwrap()).unwrap();
            let run = shown(&Roster::default().scan_whole(&[(7, born)], Some(&dir))[0], None, None);
            way(true, Some(owner), To::Running { several: true, proven: run.account_id.as_deref() })
        };
        let me = "usr_1339e525-d75d-4832-9104-98dd89287681";
        assert_eq!(to("", me), Way::Direct, "what VRChat said before its first world");
        assert_eq!(to("", "usr_2"), Way::OtherAccount);
        let forged = "2026.11.15 13:50:00 Debug      -  [Authentication - x] Preforming post login checks for Other (usr_2)
";
        assert_eq!(to(forged, "usr_2"), Way::UnknownAccount, "a script's word");
        assert_eq!(to(forged, me), Way::UnknownAccount);
        std::fs::remove_dir_all(&dir).unwrap();
    }

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
        assert!(vr.one_profile);
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

    /// Whose private places a VRChat may be sent to is told by its own log, by the account's id: nobody's while it waits at its
    /// sign-in page, nor where its command line names several profiles (which one it took is not known).
    #[test]
    fn an_account_is_proven_by_its_id_while_signed_in_on_one_profile() {
        let id = |f: &LogFacts| {
            let run = run_of(37076, 1_794_749_922, None, Some(f));
            assert_eq!(shown(&run, None, None).account_id, None, "a log that is not its own for certain says nothing of whose it is");
            let shown = shown(&VrchatRun { log_sure: true, ..run }, None, None);
            (shown.account_id, shown.signed_out)
        };
        let mine = Some("usr_1339e525-d75d-4832-9104-98dd89287681".to_string());
        let mut f = facts(OWN_LOG);
        assert_eq!(id(&f), (mine.clone(), false));
        f.read("2026.11.15 14:00:00 Debug      -  [Authentication - x] AuthSelectionPage OnPageShown\n");
        assert_eq!(id(&f), (None, true), "signed out in the game");
        f.read("2026.11.15 14:01:00 Debug      -  [Behaviour] Entering Room: FRONTLINE KIZUNA\n");
        assert_eq!(id(&f), (None, true));
        f.read("2026.11.15 14:02:00 Debug      -  [Authentication - x] Preforming post login checks for other (usr_2)\n");
        assert_eq!(id(&f), (Some("usr_2".into()), false));
        // R5: a later sign-in whose line names nobody, and a script's, take the account with them (its name stays a label).
        f.read("2026.11.15 14:03:00 Debug      -  [Authentication - x] Preforming post login checks for other\n");
        assert_eq!((id(&f), f.account.who.as_ref().map(|a| a.1.as_str())), ((None, false), Some("other")));
        f.read("2026.11.15 14:04:00 Debug      -  [Authentication - x] Preforming post login checks for other (usr_2)\n");
        assert_eq!(id(&f), (Some("usr_2".into()), false));
        f.read("2026.11.15 14:05:00 Debug      -  [World] post login checks for blackorbit (usr_1339e525-d75d-4832-9104-98dd89287681)\n");
        assert_eq!(id(&f), (None, false));
        let several = facts(&OWN_LOG.replace("--watch-worlds", "--profile=1"));
        assert_eq!(id(&several), (None, false));
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

    /// R5 (the verifier's synthetic check): a log made before a VRChat started is never its own, not even three seconds before
    /// and the only one there; and whose a VRChat is counts only from a log that can be no other's.
    #[test]
    fn a_log_from_before_a_vrchat_started_says_nothing_of_it() {
        let one = |created: u64| vec![(PathBuf::from("x"), created)];
        let none = HashMap::new();
        for created in [995, 997, 999] {
            assert!(match_logs(&[(7, 1000)], &one(created), &none).is_empty(), "{created}");
        }
        for created in [1000, 1002, 1060] {
            let m = match_logs(&[(7, 1000)], &one(created), &none);
            assert_eq!((m.len(), certain(&[(7, 1000)], &one(created), &m)), (1, vec![7]), "{created}");
        }
        // A number used again has none of the last process's log.
        assert!(match_logs(&[(7, 2000)], &one(1002), &HashMap::from([(7, PathBuf::from("x"))])).is_empty());
        // Two VRChats, each with its own; and the earlier one's log of two that came in its minute, by elimination.
        let logs = vec![(PathBuf::from("a"), 1002), (PathBuf::from("b"), 1032)];
        let procs = [(1, 1000), (2, 1030)];
        let m = match_logs(&procs, &logs, &none);
        let mut sure = certain(&procs, &logs, &m);
        sure.sort_unstable();
        assert_eq!(sure, [1, 2]);
        // One log two VRChats could have made, two logs one VRChat could have: followed as before, and proof of nothing.
        let m = match_logs(&[(1, 1000), (2, 1010)], &logs[..0], &none);
        assert!(m.is_empty());
        let shared = vec![(PathBuf::from("c"), 1012)];
        let procs = [(1, 1000), (2, 1010)];
        assert!(certain(&procs, &shared, &match_logs(&procs, &shared, &none)).is_empty());
        let two = vec![(PathBuf::from("c"), 1002), (PathBuf::from("d"), 1003)];
        assert!(certain(&[(1, 1000)], &two, &match_logs(&[(1, 1000)], &two, &none)).is_empty());
        // A start Windows would not say: the one log left is followed, and proves nothing of anyone.
        let m = match_logs(&[(99, 0)], &one(1002), &none);
        assert_eq!((m.len(), certain(&[(99, 0)], &one(1002), &m).len()), (1, 0));
    }

    /// The verifier's check: a log made 10.110 s into a minute is not that of a VRChat born at 10.880 s, the same second.
    #[test]
    fn a_log_of_the_same_second_from_before_a_vrchat_is_not_its_own() {
        const SECOND: u64 = 1_000_000_000;
        let (born, log) = (10 * SECOND + 880_000_000, |at: u64| vec![(PathBuf::from("x"), at)]);
        let none = HashMap::new();
        let before = log(10 * SECOND + 110_000_000);
        let matched = match_logs(&[(7, born)], &before, &none);
        assert!(matched.is_empty() && certain(&[(7, born)], &before, &matched).is_empty());
        let after = log(10 * SECOND + 880_000_100);
        let matched = match_logs(&[(7, born)], &after, &none);
        assert_eq!(certain(&[(7, born)], &after, &matched), [7]);
        // From the file itself: a process born a moment after it was made has none.
        let dir = std::env::temp_dir().join(format!("orbiters-roster-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("output_log_2026-11-15_13-38-44.txt");
        std::fs::write(&path, OWN_LOG).unwrap();
        let made = made_at(&std::fs::metadata(&path).unwrap()).unwrap();
        let proven = |born: u64| shown(&Roster::default().scan_whole(&[(7, born)], Some(&dir))[0], None, None).account_id.is_some();
        assert!(proven(made) && proven(made - 2 * SECOND) && !proven(made + 100));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The verifier's probe: two VRChats overlap and one log comes that either could have made; its writer ends. The log is the
    /// survivor's no more than before, whoever looks: the one that saw both, and one that starts looking only now.
    #[test]
    fn a_log_of_two_vrchats_is_no_proof_of_the_one_that_is_left() {
        const SECOND: u64 = 1_000_000_000;
        let dir = std::env::temp_dir().join(format!("orbiters-roster-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("output_log_2026-11-15_13-38-44.txt");
        std::fs::write(&path, OWN_LOG).unwrap();
        let made = made_at(&std::fs::metadata(&path).unwrap()).unwrap();
        let (first, second) = ((1, made - SECOND), (2, made - SECOND / 2));
        let mut roster = Roster::default();
        let proven = |roster: &mut Roster, procs: &[(u32, u64)]| roster.scan(procs, Some(&dir), |_| None).iter().filter_map(|run| shown(run, None, None).account_id.map(|_| run.pid)).collect::<Vec<_>>();
        assert!(proven(&mut roster, &[first, second]).is_empty(), "either's");
        assert!(proven(&mut roster, &[second]).is_empty(), "still either's");
        // (A look that found none, as one that failed would.)
        assert!(proven(&mut roster, &[]).is_empty());
        assert!(proven(&mut roster, &[second]).is_empty());
        // One that looks only now, sharing what was seen (the look right before a place is handed over), and at the same time.
        assert!(proven(&mut Roster::seeing(roster.seen()), &[second]).is_empty());
        // Nothing else was there since: a log of the survivor's own, which the other could not have made, says which is whose.
        let life = |(pid, start): (u32, u64), until: Option<u64>| Life { pid, start, since: start, until };
        let gone = life(first, Some(made + SECOND));
        let logs = vec![(path.clone(), made), (dir.join("own"), made + 2 * SECOND)];
        assert!(proven_logs(&[gone, life(second, None)], &logs[..1]).is_empty());
        assert_eq!(proven_logs(&[gone, life(second, None)], &logs), HashMap::from([(first, path.clone()), (second, dir.join("own"))]));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A VRChat that ended is in the way of no other's proof than one it could have made the log of: one started again after it
    /// (it goes as soon as it came, and another stays), one whose log is told apart by the ended one's own, still there.
    #[test]
    fn an_ended_vrchat_takes_no_later_ones_proof() {
        const SECOND: u64 = 1_000_000_000;
        // Started again: the ended one was seen to be gone before that one's log was made.
        let seen = Seen::default();
        seen.look(&[(1, 100 * SECOND)], 101 * SECOND);
        let lives = seen.look(&[(2, 104 * SECOND)], 105 * SECOND);
        let log = |name: &str, at: u64| (PathBuf::from(name), at * SECOND);
        assert_eq!(proven_logs(&lives, &[log("b", 106)]), HashMap::from([((2, 104 * SECOND), PathBuf::from("b"))]));
        // Its number used again, at once.
        let seen = Seen::default();
        seen.look(&[(1, 100 * SECOND)], 101 * SECOND);
        let lives = seen.look(&[(1, 104 * SECOND)], 105 * SECOND);
        assert_eq!(lives.iter().filter(|life| life.until.is_none()).map(|life| life.start).collect::<Vec<_>>(), [104 * SECOND]);
        assert_eq!(proven_logs(&lives, &[log("b", 106)]), HashMap::from([((1, 104 * SECOND), PathBuf::from("b"))]));
        // Two beside one another, each with its log: the one that ended keeps its own.
        let seen = Seen::default();
        seen.look(&[(1, 100 * SECOND), (2, 110 * SECOND)], 111 * SECOND);
        let lives = seen.look(&[(2, 110 * SECOND)], 120 * SECOND);
        assert_eq!(proven_logs(&lives, &[log("a", 102), log("b", 112)]).get(&(2, 110 * SECOND)), Some(&PathBuf::from("b")));
        // Long over, or a start the system said at one look and not at another: nothing is left of it, and it is one process.
        assert_eq!(seen.look(&[(3, 900 * SECOND)], 901 * SECOND).iter().map(|life| life.pid).collect::<Vec<_>>(), [3]);
        let seen = Seen::default();
        seen.look(&[(1, 0)], 101 * SECOND);
        for start in [100 * SECOND, 0, 100 * SECOND] {
            assert_eq!(seen.look(&[(1, start)], 102 * SECOND), [Life { pid: 1, start: 100 * SECOND, since: 101 * SECOND, until: None }]);
        }
    }

    /// What is shown of two games stays each one's: a log from a moment before its game is not given up for another game's, only
    /// for one that can be no other's.
    #[test]
    fn a_log_from_a_moment_before_stays_until_its_games_own_comes() {
        const SECOND: u64 = 1_000_000_000;
        let log = |name: &str, at: u64| (PathBuf::from(name), at * SECOND);
        let (one, two) = ((1, 100 * SECOND + 1), (2, 130 * SECOND));
        let early = log("early", 100);
        let first = follow_logs(&[one], std::slice::from_ref(&early), &HashMap::new());
        assert_eq!(first.get(&1), Some(&early.0));
        let logs = [early.clone(), log("second", 131)];
        let both = follow_logs(&[one, two], &logs, &first);
        assert_eq!((both.get(&1), both.get(&2)), (Some(&logs[0].0), Some(&logs[1].0)));
        let logs = [early, log("own", 102)];
        assert_eq!(follow_logs(&[one], &logs, &first).get(&1), Some(&logs[1].0));
    }

    /// 0.3.9-dev.42's five seconds, for what is shown alone: a game whose log was made a moment before it started (a clock set
    /// back, a coarse date) shows its account and its world, and is proof of nothing. Its own log, when one comes, takes over.
    #[test]
    fn a_log_from_a_moment_before_is_shown_and_proves_nothing() {
        const SECOND: u64 = 1_000_000_000;
        let dir = std::env::temp_dir().join(format!("orbiters-roster-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("output_log_2026-11-15_13-38-44.txt");
        std::fs::write(&path, OWN_LOG).unwrap();
        let made = made_at(&std::fs::metadata(&path).unwrap()).unwrap();
        let look = |born: u64| shown(&Roster::default().scan_whole(&[(7, born)], Some(&dir))[0], None, None);
        for after in [1, SECOND / 2, 3 * SECOND, 5 * SECOND - made % SECOND] {
            let run = look(made + after);
            assert_eq!((run.account.as_deref(), run.world.as_deref(), run.profile_known), (Some("blackorbit"), Some("FRONTLINE KIZUNA"), true), "{after}");
            assert_eq!((run.account_id, run.logged), (None, false), "{after}");
        }
        let run = look(made + 7 * SECOND);
        assert_eq!((run.account, run.profile_known), (None, false), "too long before");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A log longer than a look reads is read whole before whose it is counts: the sign-in at its end, not the one at its start.
    #[test]
    fn a_long_log_proves_nothing_before_its_end_is_read() {
        let dir = std::env::temp_dir().join(format!("orbiters-roster-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("output_log_2026-11-15_13-38-44.txt");
        let filler = format!("{}\n", "x".repeat(1023)).repeat((CHUNK / 1024) as usize);
        std::fs::write(&path, [OWN_LOG, &filler, "2026.11.15 13:39:00 Debug      -  [Authentication - x] Preforming post login checks for Sample (usr_00000000-0000-4000-8000-00000000000b)\n"].concat()).unwrap();
        let born = made_at(&std::fs::metadata(&path).unwrap()).unwrap();
        let mut roster = Roster::default();
        let mut look = |whole: bool| shown(&if whole { roster.scan_whole(&[(7, born)], Some(&dir)) } else { roster.scan(&[(7, born)], Some(&dir), |_| None) }[0], None, None).account_id;
        assert_eq!(look(false), None, "not its end yet");
        assert_eq!(look(true).as_deref(), Some("usr_00000000-0000-4000-8000-00000000000b"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// R5: what a log said counts while it can be read, whole: one cut shorter is read anew, one that cannot be read says nothing.
    #[test]
    fn a_log_that_cannot_be_read_any_more_proves_nothing() {
        let dir = std::env::temp_dir().join(format!("orbiters-roster-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("output_log_2026-11-15_13-38-44.txt");
        std::fs::write(&log, OWN_LOG).unwrap();
        let start = std::fs::metadata(&log).unwrap().created().unwrap().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos() as u64 - 1_000_000_000;
        let mut roster = Roster::default();
        let mut look = || {
            let run = roster.scan(&[(37076, start)], Some(&dir), |_| None).remove(0);
            let shown = shown(&run, None, None);
            (shown.account_id, shown.logged)
        };
        let mine = Some("usr_1339e525-d75d-4832-9104-98dd89287681".to_string());
        assert_eq!(look(), (mine.clone(), true));
        assert_eq!(look(), (mine.clone(), true));
        // Cut shorter: what it said is gone with it.
        std::fs::write(&log, OWN_LOG.split_inclusive('\n').take(9).collect::<String>()).unwrap();
        assert_eq!(look().0, None);
        std::fs::write(&log, OWN_LOG).unwrap();
        assert_eq!(look(), (mine.clone(), true));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o000)).unwrap();
            assert_eq!(look().0, None, "not to be read");
            std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(look().0, mine, "read again, from its start");
        }
        std::fs::remove_file(&log).unwrap();
        assert_eq!(look(), (None, false), "gone");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn people_see_a_short_session_never_its_key() {
        assert_eq!(short_session("p1-3f2a9c0d1e2f"), "p1-3f2a…");
        assert_eq!(short_session("main"), "main");
    }
}
