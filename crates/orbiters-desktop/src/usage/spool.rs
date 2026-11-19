//! The service's side of the usage statistics: each link (window, overlay, desktop panels) is a session, started at its first step
//! and ended when it goes; its steps wait on disk (`usage/waiting.jsonl` in the local data folder, at most [`MAX_KEPT`]) until
//! Orbiters takes them at `POST /n4n0/metrics`, a batch every minute. A server that cannot be reached keeps them waiting; one that
//! refuses them (an older server without the route) has them dropped, without a word: nothing here is ever shown to the person.
//!
//! The install is a random id kept beside them, forgotten with them when the statistics are turned off. No account, no token is sent.

use super::{clean, now_ms, Kind, Step};
use crate::ipc::Role;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

/// Steps kept waiting at most (about 2 MB): the oldest go first.
pub const MAX_KEPT: usize = 10_000;
/// Steps per request.
pub const BATCH: usize = 500;
/// Performance windows kept waiting at most (a day at one a minute, about 1.5 MB): the oldest go first.
pub const MAX_PERF_KEPT: usize = 1440;
/// Performance windows per request.
pub const PERF_BATCH: usize = 120;
const WAITING: &str = "waiting.jsonl";
const PERF_WAITING: &str = "perf-waiting.jsonl";
const INSTALL: &str = "install-id";

/// One step as Orbiters receives it (backend `routes/n4n0Metrics.js`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub session: String,
    /// `app` (the window), `vr` (the SteamVR overlay), `vrchat-desktop` (the panels beside VRChat in desktop mode).
    pub surface: String,
    pub build: String,
    pub os: String,
    pub lang: String,
    pub at: u64,
    /// `start`, `enter`, `press`, `end`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<String>,
    /// Where the session comes from, for the admin to leave out the team's own testing: a debug build, a run the VR sandbox started,
    /// the VRChat bridge (an AI agent) driving at that moment. Absent when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub debug: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sandbox: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub agent: bool,
}

pub fn surface(role: Role) -> &'static str {
    match role {
        Role::Window => "app",
        Role::Overlay => "vr",
        Role::Orbit => "vrchat-desktop",
    }
}

struct Session {
    id: String,
    surface: &'static str,
    started: bool,
    last: Option<String>,
}

#[derive(Default)]
struct Inner {
    dir: Option<PathBuf>,
    install: Option<String>,
    sessions: HashMap<u64, Session>,
    waiting: VecDeque<Event>,
    loaded: bool,
    /// The performance windows waiting, and the one being summed.
    perf: VecDeque<super::perf::Window>,
    windows: super::perf::Windows,
    /// The VRChat bridge drives now; the team account's code (`perf::team_code`).
    agent: bool,
    team: Option<String>,
}

/// What a request did with a batch.
#[derive(Debug, PartialEq, Eq)]
pub enum Sent {
    Nothing,
    Taken(usize),
    /// Refused for good (4xx): dropped.
    Dropped(usize),
    /// Not reached, or busy: kept for later.
    Kept,
}

#[derive(Default)]
pub struct Spool(Mutex<Inner>);

impl Spool {
    /// Kept in `dir` from now on (what waited there from an earlier run is read back).
    pub fn open(&self, dir: PathBuf) {
        let mut inner = self.lock();
        inner.dir = Some(dir);
        inner.loaded = false;
        load(&mut inner);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A link of `role` came: its session starts at its first step.
    pub fn linked(&self, link: u64, role: Role) {
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.lock().sessions.insert(link, Session { id, surface: surface(role), started: false, last: None });
    }

    /// What link `link` noted. Nothing is kept while the statistics are off.
    pub fn steps(&self, link: u64, steps: Vec<Step>, enabled: bool) {
        if !enabled || steps.is_empty() {
            return;
        }
        let mut inner = self.lock();
        load(&mut inner);
        let lang = orbiters_i18n::language().tag().to_string();
        let inner_agent = inner.agent;
        let Some(session) = inner.sessions.get_mut(&link) else { return };
        let mut events = Vec::with_capacity(steps.len() + 1);
        let event = |session: &Session, at: u64, kind: &str| Event {
            session: session.id.clone(),
            surface: session.surface.into(),
            build: env!("ORBITERS_VERSION").into(),
            os: std::env::consts::OS.into(),
            lang: lang.clone(),
            at,
            kind: kind.into(),
            screen: None,
            from: None,
            control: None,
            debug: super::perf::debug_build(),
            sandbox: super::perf::sandbox_run(),
            agent: inner_agent,
        };
        for step in steps {
            // Checked again here: a client is not trusted with what leaves the computer.
            let screen = clean(&step.screen).map(str::to_string);
            let Some(screen) = screen else { continue };
            if !session.started {
                session.started = true;
                events.push(Event { screen: Some(screen.clone()), ..event(session, step.at, "start") });
            }
            let mut e = Event { screen: Some(screen.clone()), ..event(session, step.at, match step.kind { Kind::Enter => "enter", Kind::Press => "press" }) };
            match step.kind {
                Kind::Enter => {
                    e.from = step.from.as_deref().and_then(clean).map(str::to_string);
                    session.last = Some(screen);
                }
                Kind::Press => {
                    let Some(control) = step.control.as_deref().and_then(clean) else { continue };
                    e.control = Some(control.to_string());
                }
            }
            events.push(e);
        }
        keep(&mut inner, events);
    }

    /// Link `link` went: its session ends where it was last.
    pub fn unlinked(&self, link: u64, enabled: bool) {
        let mut inner = self.lock();
        let Some(session) = inner.sessions.remove(&link) else { return };
        if !enabled || !session.started {
            return;
        }
        let end = Event {
            session: session.id,
            surface: session.surface.into(),
            build: env!("ORBITERS_VERSION").into(),
            os: std::env::consts::OS.into(),
            lang: orbiters_i18n::language().tag().into(),
            at: now_ms(),
            kind: "end".into(),
            screen: session.last,
            from: None,
            control: None,
            debug: super::perf::debug_build(),
            sandbox: super::perf::sandbox_run(),
            agent: inner.agent,
        };
        keep(&mut inner, vec![end]);
    }

    /// Turned off: everything waiting is forgotten, the install's id with it; sessions start again if it is turned on.
    pub fn forget(&self) {
        let mut inner = self.lock();
        inner.waiting.clear();
        inner.perf.clear();
        inner.windows = Default::default();
        inner.install = None;
        for session in inner.sessions.values_mut() {
            session.started = false;
            session.id = uuid::Uuid::new_v4().simple().to_string();
        }
        if let Some(dir) = &inner.dir {
            let _ = std::fs::remove_file(dir.join(WAITING));
            let _ = std::fs::remove_file(dir.join(PERF_WAITING));
            let _ = std::fs::remove_file(dir.join(INSTALL));
        }
    }

    pub fn waiting(&self) -> Vec<Event> {
        self.lock().waiting.iter().cloned().collect()
    }

    #[cfg(test)]
    pub fn perf_waiting(&self) -> Vec<super::perf::Window> {
        self.lock().perf.iter().cloned().collect()
    }

    /// What the usage statistics say N4N0 is on now: the sessions started and each surface's screen.
    pub fn context(&self) -> (Vec<String>, std::collections::BTreeMap<String, String>) {
        let inner = self.lock();
        let mut sessions: Vec<String> = inner.sessions.values().filter(|s| s.started).map(|s| s.id.clone()).collect();
        sessions.sort();
        let screens = inner.sessions.values().filter_map(|s| s.last.clone().map(|l| (s.surface.to_string(), l))).collect();
        (sessions, screens)
    }

    /// Where what is noted from now on comes from: the VRChat bridge driving, the team account's code.
    pub fn origin(&self, agent: bool, team: Option<String>) {
        let mut inner = self.lock();
        inner.agent = agent;
        inner.team = team;
    }

    /// A performance sample (`perf::sample`); a window it closes waits to be sent. Nothing is kept while the statistics are off.
    pub fn perf(&self, sample: super::perf::Sample, enabled: bool) {
        let mut inner = self.lock();
        if !enabled {
            inner.windows = Default::default();
            return;
        }
        load(&mut inner);
        let agent = inner.agent;
        if let Some(window) = inner.windows.push(sample, agent) {
            keep_perf(&mut inner, window);
        }
    }

    /// The install's random id, made the first time it is needed.
    fn install(&self) -> String {
        let mut inner = self.lock();
        if let Some(id) = &inner.install {
            return id.clone();
        }
        let read = inner.dir.as_ref().and_then(|d| std::fs::read_to_string(d.join(INSTALL)).ok()).map(|s| s.trim().to_string()).filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()));
        let id = read.unwrap_or_else(|| {
            let id = uuid::Uuid::new_v4().simple().to_string();
            if let Some(dir) = &inner.dir {
                let _ = std::fs::create_dir_all(dir);
                let _ = std::fs::write(dir.join(INSTALL), &id);
            }
            id
        });
        inner.install = Some(id.clone());
        id
    }

    /// The longest the first send waits for the account to be known ([`Spool::run`]).
    pub const ORIGIN_WAIT: Duration = Duration::from_secs(60);

    /// Sends the oldest batch to `api`: the request runs without the lock, so steps keep arriving meanwhile.
    pub fn send(&self, api: &orbiters_core::account::Api) -> Sent {
        let (batch, perf, team) = {
            let inner = self.lock();
            (inner.waiting.iter().take(BATCH).cloned().collect::<Vec<Event>>(), inner.perf.iter().take(PERF_BATCH).cloned().collect::<Vec<_>>(), inner.team.clone())
        };
        if batch.is_empty() && perf.is_empty() {
            return Sent::Nothing;
        }
        // `perf` and `team` are new (2026-11-16): an older server reads `install` and `events` and leaves the rest.
        let mut body = serde_json::json!({ "install": self.install(), "events": batch });
        if !perf.is_empty() {
            body["perf"] = serde_json::json!(perf);
        }
        if let Some(team) = team {
            body["team"] = serde_json::json!(team);
        }
        let answer = api
            .agent()
            .post(&api.url("n4n0/metrics"))
            .config()
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            .build()
            .send_json(&body);
        let status = match answer {
            Ok(response) => response.status().as_u16(),
            Err(e) => {
                eprintln!("usage statistics: Orbiters not reached ({e}), {} and {} performance windows kept", batch.len(), perf.len());
                return Sent::Kept;
            }
        };
        let done = match status {
            200..=299 => Sent::Taken(batch.len() + perf.len()),
            408 | 429 => Sent::Kept,
            400..=499 => Sent::Dropped(batch.len() + perf.len()),
            _ => Sent::Kept,
        };
        if let Sent::Taken(_) | Sent::Dropped(_) = done {
            let mut inner = self.lock();
            // Only those sent: the first ones, unless a Forget came meanwhile.
            let first = inner.waiting.iter().take(batch.len()).cloned().collect::<Vec<_>>();
            if first == batch {
                inner.waiting.drain(..batch.len());
                rewrite(&inner);
            }
            let first = inner.perf.iter().take(perf.len()).cloned().collect::<Vec<_>>();
            if first == perf {
                inner.perf.drain(..perf.len());
                rewrite_perf(&inner);
            }
        }
        if let Sent::Dropped(n) = done {
            eprintln!("usage statistics: Orbiters refused {n} ({status}), dropped");
        }
        done
    }

    /// Every minute while the app runs, the waiting steps go to Orbiters while `enabled` says so (the first a little after the start).
    /// `origin` says who is using the app now (an agent driving it, the team's code), `None` while the account is still being
    /// read: a send takes it just before it goes, and the first one waits for it (at most [`Spool::ORIGIN_WAIT`]), so that the start of
    /// a run is not counted as a tester's for want of an answer yet (PRIV-4).
    pub fn run(self: std::sync::Arc<Self>, enabled: impl Fn() -> bool + Send + 'static, origin: impl Fn() -> Option<(bool, Option<String>)> + Send + 'static) {
        let _ = std::thread::Builder::new().name("usage-upload".into()).spawn(move || {
            let started = std::time::Instant::now();
            std::thread::sleep(Duration::from_secs(20));
            while origin().is_none() && started.elapsed() < Self::ORIGIN_WAIT {
                std::thread::sleep(Duration::from_secs(5));
            }
            loop {
                if enabled() {
                    if let Some((agent, team)) = origin() {
                        self.origin(agent, team);
                    }
                    let api = api();
                    // A backlog goes a batch after another; a failure waits for the next minute.
                    while let Sent::Taken(_) | Sent::Dropped(_) = self.send(&api) {}
                } else {
                    self.forget();
                }
                std::thread::sleep(Duration::from_secs(60));
            }
        });
    }
}

/// The service's spool, for the performance self-check's thread (`controller::perf`): set once the service runs.
static CURRENT: std::sync::OnceLock<std::sync::Arc<Spool>> = std::sync::OnceLock::new();

pub fn set_current(spool: std::sync::Arc<Spool>) {
    let _ = CURRENT.set(spool);
}

pub fn current() -> Option<&'static std::sync::Arc<Spool>> {
    CURRENT.get()
}

/// Where the steps wait: `usage` in the local data folder. A debug build started by the VR sandbox keeps them in the sandbox's folder, as its
/// logs (`orbiters_core::paths::logs_dir`): a test build never writes the person's own.
pub fn folder() -> PathBuf {
    if cfg!(debug_assertions) {
        if let Some(dir) = std::env::var_os("ORBITERS_SANDBOX_LOGS").filter(|d| !d.is_empty()) {
            return PathBuf::from(dir).join("usage");
        }
    }
    orbiters_core::paths::local_data_root().join("usage")
}

/// The Orbiters the steps go to: the app's server; a debug build may be pointed at a server under test (`ORBITERS_USAGE_API`).
fn api() -> orbiters_core::account::Api {
    if cfg!(debug_assertions) {
        if let Some(base) = std::env::var("ORBITERS_USAGE_API").ok().filter(|b| !b.is_empty()) {
            return orbiters_core::account::Api::at(&base);
        }
    }
    orbiters_core::account::Api::new(orbiters_core::account::Server::current())
}

fn load(inner: &mut Inner) {
    if inner.loaded {
        return;
    }
    inner.loaded = true;
    let Some(dir) = inner.dir.clone() else { return };
    let text = std::fs::read_to_string(dir.join(WAITING)).unwrap_or_default();
    let mut read: VecDeque<Event> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    while read.len() > MAX_KEPT {
        read.pop_front();
    }
    read.extend(inner.waiting.drain(..));
    inner.waiting = read;
    let mut perf: VecDeque<super::perf::Window> = std::fs::read_to_string(dir.join(PERF_WAITING)).map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()).unwrap_or_default();
    while perf.len() > MAX_PERF_KEPT {
        perf.pop_front();
    }
    perf.extend(inner.perf.drain(..));
    inner.perf = perf;
}

fn keep_perf(inner: &mut Inner, window: super::perf::Window) {
    inner.perf.push_back(window.clone());
    if inner.perf.len() > MAX_PERF_KEPT {
        inner.perf.pop_front();
        rewrite_perf(inner);
        return;
    }
    let Some(dir) = &inner.dir else { return };
    let _ = std::fs::create_dir_all(dir);
    if let (Ok(mut file), Ok(mut line)) = (std::fs::OpenOptions::new().create(true).append(true).open(dir.join(PERF_WAITING)), serde_json::to_string(&window)) {
        line.push('\n');
        let _ = file.write_all(line.as_bytes());
    }
}

fn rewrite_perf(inner: &Inner) {
    let Some(dir) = &inner.dir else { return };
    let _ = std::fs::create_dir_all(dir);
    let mut lines = String::new();
    for w in &inner.perf {
        if let Ok(line) = serde_json::to_string(w) {
            lines.push_str(&line);
            lines.push('\n');
        }
    }
    let tmp = dir.join("perf-waiting.jsonl.tmp");
    if std::fs::write(&tmp, lines).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join(PERF_WAITING));
    }
}

fn keep(inner: &mut Inner, events: Vec<Event>) {
    if events.is_empty() {
        return;
    }
    let over = (inner.waiting.len() + events.len()).saturating_sub(MAX_KEPT);
    inner.waiting.extend(events.iter().cloned());
    if over > 0 {
        inner.waiting.drain(..over.min(inner.waiting.len()));
        rewrite(inner);
        return;
    }
    let Some(dir) = &inner.dir else { return };
    let _ = std::fs::create_dir_all(dir);
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(WAITING)) {
        let mut lines = String::new();
        for e in &events {
            if let Ok(line) = serde_json::to_string(e) {
                lines.push_str(&line);
                lines.push('\n');
            }
        }
        let _ = file.write_all(lines.as_bytes());
    }
}

fn rewrite(inner: &Inner) {
    let Some(dir) = &inner.dir else { return };
    let _ = std::fs::create_dir_all(dir);
    let mut lines = String::new();
    for e in &inner.waiting {
        if let Ok(line) = serde_json::to_string(e) {
            lines.push_str(&line);
            lines.push('\n');
        }
    }
    let tmp = dir.join("waiting.jsonl.tmp");
    if std::fs::write(&tmp, lines).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join(WAITING));
    }
}
