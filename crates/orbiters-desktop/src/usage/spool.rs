//! The service's side of the usage statistics: each link (window, overlay, desktop panels) is a session, started at its first step
//! and ended when it goes; its steps wait on disk (`usage/waiting.jsonl` in the local data folder, at most [`MAX_KEPT`]) until
//! Orbiters takes them at `POST /n4n0/metrics`, a batch every minute. A server that cannot be reached keeps them waiting; one that
//! refuses them (an older server without the route) has them dropped, without a word: nothing here is ever shown to the person.
//!
//! The install is a random id kept beside them, forgotten with them when the statistics are turned off.
//!
//! **Who** (2026-11-19, Enzo: "identify the usages by user"): each step and performance window is stamped when it is counted with a
//! [`Stamp`]: whether the notice saying "linked to your Orbiters account" had been shown (`settings.usage.told_account`) and, if so,
//! the account signed in then. A batch holds only items of one stamp; one stamped with the account signed in now goes with that
//! account's own Orbiters credential (`Authorization: Bearer`, as every authenticated call), so the server learns the account from
//! its session, never from a field the app could make up. Nothing counted before that notice is ever sent with a credential, and it
//! goes under the install id of before ([`INSTALL`]); what is counted after it goes under a new one ([`INSTALL_TOLD`]), and each
//! session ends and starts again when the stamp changes: neither an install nor a session ties what was counted under the old
//! notice ("never your account") to an account. The stamp itself stays on this computer.

use orbiters_core::locks::*;
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
/// The install's id for what was counted before the notice said it is linked to the account; deleted once none of it waits.
const INSTALL: &str = "install-id";
/// The install's id for what was counted since (a new random one: the server cannot join the two).
const INSTALL_TOLD: &str = "install-id-2";

/// Under which notice, and for whom, a step or window was counted (see the module's doc). Kept on disk with what waits, never sent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    /// Counted after the notice that says it is linked to the account was shown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub told: bool,
    /// The Orbiters account signed in then (its id), only when `told`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// The server (its API address) that account is on: account ids are per server, so the same id on the other
    /// environment is another person. A stamp without it (counted before 2026-11-19 night4) is never sent with a credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
}

impl Stamp {
    pub fn is_none(&self) -> bool {
        *self == Self::default()
    }

    /// What is counted now: `told` from the settings, `account` the account signed in and the server it is on (ignored unless told).
    pub fn now(told: bool, account: Option<(String, String)>) -> Self {
        let (account, server) = account.filter(|_| told).map_or((None, None), |(a, s)| (Some(a), Some(s)));
        Self { told, account, server }
    }

    /// Whether a batch stamped so goes with `signed`'s credential to `base`: the very account it was counted for, on the very
    /// server, and only there.
    pub fn sends_as(&self, signed: &Signed, base: &str) -> bool {
        self.account.as_deref() == Some(signed.account.as_str()) && self.server.as_deref() == Some(signed.base.as_str()) && signed.base == base
    }
}

/// The signed-in account a batch stamped with it goes with: its id (as in [`Stamp::account`]), the API address its credential is for,
/// and the credential. Held in memory only for the request.
#[derive(Clone)]
pub struct Signed {
    pub account: String,
    pub base: String,
    pub token: String,
}

impl std::fmt::Debug for Signed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signed").field("account", &self.account).field("base", &self.base).field("token", &"(hidden)").finish() // i18n: not shown
    }
}

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
    /// Who it was counted for: on disk only, taken off before it is sent.
    #[serde(default, skip_serializing_if = "Stamp::is_none")]
    pub who: Stamp,
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
    /// The install's two ids (before and since the notice of the account), read or made when first needed.
    install: Option<String>,
    install_told: Option<String>,
    /// Who what is counted now is for; `None` until told once (`Spool::collecting`).
    who: Option<Stamp>,
    sessions: HashMap<u64, Session>,
    waiting: VecDeque<Event>,
    loaded: bool,
    /// The performance windows waiting, and the one being summed.
    perf: VecDeque<super::perf::Window>,
    windows: super::perf::Windows,
    /// The VRChat bridge drives now.
    agent: bool,
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
        self.0.locked()
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
        let who = inner.who.clone().unwrap_or_default();
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
            who: who.clone(),
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
            who: inner.who.clone().unwrap_or_default(),
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
        inner.install_told = None;
        for session in inner.sessions.values_mut() {
            session.started = false;
            session.id = uuid::Uuid::new_v4().simple().to_string();
        }
        if let Some(dir) = &inner.dir {
            let _ = std::fs::remove_file(dir.join(WAITING));
            let _ = std::fs::remove_file(dir.join(PERF_WAITING));
            let _ = std::fs::remove_file(dir.join(INSTALL));
            let _ = std::fs::remove_file(dir.join(INSTALL_TOLD));
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

    /// Where what is noted from now on comes from: the VRChat bridge driving.
    pub fn origin(&self, agent: bool) {
        self.lock().agent = agent;
    }

    /// Who what is counted from now on is for. When it changes, each session that started ends where it was, under the stamp it had,
    /// and starts again under a new id at its next step: a session is never of two stamps (the server would join them by its id).
    pub fn collecting(&self, who: Stamp) {
        let mut inner = self.lock();
        let Some(before) = inner.who.replace(who.clone()) else { return };
        if before == who {
            return;
        }
        load(&mut inner);
        let mut ends = Vec::new();
        let lang = orbiters_i18n::language().tag().to_string();
        let agent = inner.agent;
        for session in inner.sessions.values_mut() {
            if session.started {
                ends.push(Event {
                    session: session.id.clone(),
                    surface: session.surface.into(),
                    build: env!("ORBITERS_VERSION").into(),
                    os: std::env::consts::OS.into(),
                    lang: lang.clone(),
                    at: now_ms(),
                    kind: "end".into(),
                    screen: session.last.clone(),
                    from: None,
                    control: None,
                    debug: super::perf::debug_build(),
                    sandbox: super::perf::sandbox_run(),
                    agent,
                    who: before.clone(),
                });
            }
            session.started = false;
            session.id = uuid::Uuid::new_v4().simple().to_string();
        }
        keep(&mut inner, ends);
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
        let mut sample = sample;
        sample.who = inner.who.clone().unwrap_or_default();
        if let Some(window) = inner.windows.push(sample, agent) {
            keep_perf(&mut inner, window);
        }
    }

    /// The install's random id for what was counted before (`told` false) or since the notice of the account, made the first time
    /// it is needed.
    fn install(&self, told: bool) -> String {
        let mut inner = self.lock();
        let inner = &mut *inner;
        let (slot, file) = if told { (&mut inner.install_told, INSTALL_TOLD) } else { (&mut inner.install, INSTALL) };
        if let Some(id) = slot {
            return id.clone();
        }
        let read = inner.dir.as_ref().and_then(|d| std::fs::read_to_string(d.join(file)).ok()).map(|s| s.trim().to_string()).filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()));
        let id = read.unwrap_or_else(|| {
            let id = uuid::Uuid::new_v4().simple().to_string();
            if let Some(dir) = &inner.dir {
                let _ = std::fs::create_dir_all(dir);
                let _ = std::fs::write(dir.join(file), &id);
            }
            id
        });
        *slot = Some(id.clone());
        id
    }

    /// The install's id of before, once nothing counted before the notice of the account waits and that notice was shown: forgotten,
    /// so that nothing on this computer still holds it.
    fn drop_old_install(inner: &mut Inner) {
        if !inner.who.as_ref().is_some_and(|w| w.told) || inner.waiting.iter().any(|e| !e.who.told) || inner.perf.iter().any(|w| !w.who.told) {
            return;
        }
        inner.install = None;
        if let Some(dir) = &inner.dir {
            let _ = std::fs::remove_file(dir.join(INSTALL));
        }
    }

    /// The longest the first send waits for the account to be known ([`Spool::run`]).
    pub const ORIGIN_WAIT: Duration = Duration::from_secs(60);

    /// Sends the oldest batch to `api`: the request runs without the lock, so steps keep arriving meanwhile. A batch holds the oldest
    /// items of one [`Stamp`]; it goes with `signed`'s credential only when it was stamped with that very account and the credential is
    /// for this server, otherwise with none (never with another account's, never for what was counted before the notice of the account).
    pub fn send(&self, api: &orbiters_core::account::Api, signed: Option<&Signed>) -> Sent {
        let (batch, perf, who) = {
            let inner = self.lock();
            let Some(who) = inner.waiting.front().map(|e| e.who.clone()).or_else(|| inner.perf.front().map(|w| w.who.clone())) else { return Sent::Nothing };
            (
                inner.waiting.iter().take_while(|e| e.who == who).take(BATCH).cloned().collect::<Vec<Event>>(),
                inner.perf.iter().take_while(|w| w.who == who).take(PERF_BATCH).cloned().collect::<Vec<_>>(),
                who,
            )
        };
        let token = signed.filter(|s| who.sends_as(s, &api.url(""))).map(|s| s.token.clone());
        // `perf` is new (2026-11-16): an older server reads `install` and `events` and leaves the rest. The stamps stay here.
        let events: Vec<Event> = batch.iter().cloned().map(|e| Event { who: Stamp::default(), ..e }).collect();
        let mut body = serde_json::json!({ "install": self.install(who.told), "events": events });
        if !perf.is_empty() {
            let windows: Vec<_> = perf.iter().cloned().map(|w| super::perf::Window { who: Stamp::default(), ..w }).collect();
            body["perf"] = serde_json::json!(windows);
        }
        let mut request = api.agent().post(&api.url("n4n0/metrics")).config().timeout_global(Some(Duration::from_secs(20))).http_status_as_error(false).build();
        if let Some(token) = &token {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
        let answer = request.send_json(&body);
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
            Self::drop_old_install(&mut inner);
        }
        if let Sent::Dropped(n) = done {
            eprintln!("usage statistics: Orbiters refused {n} ({status}), dropped");
        }
        done
    }

    /// Every minute while the app runs, the waiting steps go to Orbiters while `enabled` says so (the first a little after the start).
    /// `origin` says who is using the app now (an agent driving it; the account signed in, with its credential, for the batches
    /// stamped with it), `None` while the account is still being read: a send takes it just before it goes, and the first one waits
    /// for it (at most [`Spool::ORIGIN_WAIT`]), so that the start of a run is not counted as a tester's for want of an answer (PRIV-4).
    pub fn run(self: std::sync::Arc<Self>, enabled: impl Fn() -> bool + Send + 'static, origin: impl Fn() -> Option<(bool, Option<Signed>)> + Send + 'static) {
        let _ = std::thread::Builder::new().name("usage-upload".into()).spawn(move || {
            let started = std::time::Instant::now();
            std::thread::sleep(Duration::from_secs(20));
            while origin().is_none() && started.elapsed() < Self::ORIGIN_WAIT {
                std::thread::sleep(Duration::from_secs(5));
            }
            loop {
                if enabled() {
                    // A backlog goes a batch after another; a failure waits for the next minute. Before EACH batch the switch,
                    // the account signed in and the server are read again: one turned off, signed out or switched meanwhile
                    // is never the one a later batch goes as.
                    loop {
                        if !enabled() {
                            self.forget();
                            break;
                        }
                        let signed = origin().and_then(|(agent, signed)| {
                            self.origin(agent);
                            signed
                        });
                        let api = api();
                        if !matches!(self.send(&api, signed.as_ref()), Sent::Taken(_) | Sent::Dropped(_)) {
                            break;
                        }
                    }
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
