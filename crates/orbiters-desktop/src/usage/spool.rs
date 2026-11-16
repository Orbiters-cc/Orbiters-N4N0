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
const WAITING: &str = "waiting.jsonl";
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
        };
        keep(&mut inner, vec![end]);
    }

    /// Turned off: everything waiting is forgotten, the install's id with it; sessions start again if it is turned on.
    pub fn forget(&self) {
        let mut inner = self.lock();
        inner.waiting.clear();
        inner.install = None;
        for session in inner.sessions.values_mut() {
            session.started = false;
            session.id = uuid::Uuid::new_v4().simple().to_string();
        }
        if let Some(dir) = &inner.dir {
            let _ = std::fs::remove_file(dir.join(WAITING));
            let _ = std::fs::remove_file(dir.join(INSTALL));
        }
    }

    pub fn waiting(&self) -> Vec<Event> {
        self.lock().waiting.iter().cloned().collect()
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

    /// Sends the oldest batch to `api`: the request runs without the lock, so steps keep arriving meanwhile.
    pub fn send(&self, api: &orbiters_core::account::Api) -> Sent {
        let batch: Vec<Event> = self.lock().waiting.iter().take(BATCH).cloned().collect();
        if batch.is_empty() {
            return Sent::Nothing;
        }
        let body = serde_json::json!({ "install": self.install(), "events": batch });
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
                eprintln!("usage statistics: Orbiters not reached ({e}), {} kept", batch.len());
                return Sent::Kept;
            }
        };
        let done = match status {
            200..=299 => Sent::Taken(batch.len()),
            408 | 429 => Sent::Kept,
            400..=499 => Sent::Dropped(batch.len()),
            _ => Sent::Kept,
        };
        if let Sent::Taken(n) | Sent::Dropped(n) = done {
            let mut inner = self.lock();
            // Only those sent: the first `n`, unless a Forget came meanwhile.
            let first = inner.waiting.iter().take(n).cloned().collect::<Vec<_>>();
            if first == batch {
                inner.waiting.drain(..n);
                rewrite(&inner);
            }
        }
        if let Sent::Dropped(n) = done {
            eprintln!("usage statistics: Orbiters refused {n} ({status}), dropped");
        }
        done
    }

    /// Every minute while the app runs, the waiting steps go to Orbiters while `enabled` says so (the first a little after the start).
    pub fn run(self: std::sync::Arc<Self>, enabled: impl Fn() -> bool + Send + 'static) {
        let _ = std::thread::Builder::new().name("usage-upload".into()).spawn(move || {
            std::thread::sleep(Duration::from_secs(20));
            loop {
                if enabled() {
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
    let Some(dir) = &inner.dir else { return };
    let Ok(text) = std::fs::read_to_string(dir.join(WAITING)) else { return };
    let mut read: VecDeque<Event> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    while read.len() > MAX_KEPT {
        read.pop_front();
    }
    read.extend(inner.waiting.drain(..));
    inner.waiting = read;
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
