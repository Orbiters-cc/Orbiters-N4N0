//! World feedback in the tray service (contracts `Docs/world-feedback.md`): the N4N0 world's `[N4N0-Feedback]` lines
//! read from VRChat's log (the one the app follows, by a follower of its own read to its end before each answer), the local address the world
//! loads to know Orbiters - N4N0 runs (`http://127.0.0.1:22541/n4n0/feedback/v1`), the member's likes, labels and
//! comments sent to Orbiters under the signed-in account through a queue kept on disk (retried with backoff), and the
//! worlds N4N0's menus give feedback on (the shown capture's world, the one the player is in, previous instances'),
//! handed to N4N0 ([`orbiters_n4n0::N4n0::set_feedback`]) whenever what they show changes.

use super::rest::Part;
use super::Shared;
use orbiters_contracts::world_feedback::{self as contract, Action as Line, Refusal};
use orbiters_core::account::Api;
use super::vrchats::TargetLog;
use orbiters_core::vrchat_log::LogEvent;
use orbiters_i18n::{mark, tr, tr_latin};
use orbiters_core::world_feedback::{client_id, Action, Client, Failure, Known, Queue, Seen, Set, Stamp};
use orbiters_n4n0_core::feedback::{CommentRow, FeedbackAsk, FeedbackView, LabelCount, Subject, Why, WorldFeedback, SUBJECTS_MAX};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How often the world's lines are looked for while VRChat's log is followed.
const LOG_EVERY: Duration = Duration::from_millis(250);
/// A world shown somewhere is read again from Orbiters after this long, for as long as it is wanted.
const READ_AGAIN: Duration = Duration::from_secs(30);
const WANTED_FOR: Duration = Duration::from_secs(300);
/// Worlds a menu asked about lately (the overlay's 3D worlds panel shows any capture's world) whose feedback the menus
/// get besides their subjects' (for [`WANTED_FOR`]).
const ASKED_MAX: usize = 6;
/// Previous instances are read again from their file after this long.
const INSTANCES_EVERY: Duration = Duration::from_secs(10);
/// What the answer gives the member's own comments as their author (the world says "You").
const ME: &str = mark("You");

fn now_ms() -> u64 {
    orbiters_n4n0::unix_ms()
}

/// The world's session as its lines tell it, the subject it selected, and the last action refused.
#[derive(Default)]
struct WorldSession {
    session: Option<String>,
    seen: u64,
    selected: Option<(String, String)>,
    error: Option<(u64, String, String)>,
}

pub(super) struct WorldFeedbackRuntime {
    log: TargetLog,
    /// The world the log last joined (lowercase), while in it.
    joined: Option<String>,
    world: WorldSession,
    known: HashMap<String, Known>,
    /// Worlds shown somewhere (the menus, the N4N0 world), since when.
    wanted: HashMap<String, Instant>,
    /// Names the menus and the world gave worlds.
    names: HashMap<String, String>,
    queue: Queue,
    queue_path: PathBuf,
    /// The last error of a menu's action, by world.
    menu_errors: HashMap<String, String>,
    /// Worlds a menu asked about (read, liked, labelled, commented), newest first, since when.
    asked: Vec<(String, Instant)>,
    instances: (Vec<orbiters_n4n0_core::menu::InstanceRow>, Option<Instant>),
    /// Why the local address cannot be served (another program has the port).
    pub(super) port_note: Option<String>,
    sent: Option<FeedbackView>,
    /// Bumped when the environment changes: an answer to a call made before is the other server's.
    scope: u64,
    /// The server and account of the moment: what is queued is for them, and only what is for them is sent.
    stamp: Stamp,
    /// Accounts whose credential Orbiters refused, and until when nothing of theirs is sent.
    parked: Vec<(Stamp, u64)>,
}

#[cfg(test)]
impl Default for WorldFeedbackRuntime {
    fn default() -> Self {
        Self::new(TargetLog::new(None, Default::default()))
    }
}

impl WorldFeedbackRuntime {
    pub(super) fn new(log: TargetLog) -> Self {
        let queue_path = if cfg!(test) { std::env::temp_dir().join(format!("orbiters-feedback-test-{}.json", std::process::id())) } else { Queue::file() };
        Self {
            log,
            joined: None,
            world: WorldSession::default(),
            known: HashMap::new(),
            wanted: HashMap::new(),
            names: HashMap::new(),
            queue: Queue::load(&queue_path, now_ms()),
            queue_path,
            menu_errors: HashMap::new(),
            asked: Vec::new(),
            instances: (Vec::new(), None),
            port_note: None,
            sent: None,
            scope: 0,
            stamp: Stamp::default(),
            parked: Vec::new(),
        }
    }
}

/// What waits for an account whose credential Orbiters refused, before it is tried again.
const SIGNED_OUT_PAUSE: Duration = Duration::from_secs(600);

/// Whether the log's world is one whose lines count (the N4N0 world, or any in the Development environment).
fn accepted(world: Option<&str>, development: bool) -> bool {
    world.is_some_and(|w| development || w.eq_ignore_ascii_case(contract::N4N0_WORLD))
}

impl WorldFeedbackRuntime {
    /// Reads VRChat's log to its end and acts on the world's lines. `signed_in`: an account can send what they ask.
    fn follow(&mut self, development: bool, signed_in: bool) -> bool {
        let followed = self.log.state.log_file.clone();
        let events = self.log.poll();
        let live = self.log.live();
        let mut changed = false;
        if self.log.state.log_file != followed {
            // Another VRChat's, or another run of it: what the last one's world said is not this one's.
            self.joined = None;
            self.world = WorldSession::default();
            changed = true;
        }
        for e in events {
            match e {
                LogEvent::Joining { world_id, .. } => {
                    let world = world_id.to_ascii_lowercase();
                    if self.joined.as_deref() != Some(world.as_str()) {
                        self.world = WorldSession::default();
                        changed = true;
                    }
                    self.joined = Some(world);
                }
                LogEvent::LeftRoom => {
                    self.joined = None;
                    self.world.selected = None;
                    changed = true;
                }
                LogEvent::WorldLine { message, at } if accepted(self.joined.as_deref(), development) => {
                    // A line read back from before following began, long ago, is not acted on (a like never goes twice).
                    let stale = !live && at.is_some_and(|at| (now_ms() / 1000).saturating_sub(at) > contract::STALE_SECONDS);
                    if let Some(line) = contract::parse(&message) {
                        changed |= self.line(line, stale, signed_in);
                    }
                }
                _ => {}
            }
        }
        changed
    }

    /// One of the world's lines.
    fn line(&mut self, line: contract::Line, stale: bool, signed_in: bool) -> bool {
        if self.world.session.as_deref() != Some(line.session.as_str()) {
            // A new session (the world loaded again), or one under way when Orbiters - N4N0 started.
            self.world = WorldSession { session: Some(line.session.clone()), ..Default::default() };
        } else if line.seq <= self.world.seen {
            return false;
        }
        self.world.seen = line.seq;
        let seq = line.seq;
        match line.action {
            Line::Hello => {}
            Line::Select { world, name, .. } => {
                let world = world.to_ascii_lowercase();
                if !name.is_empty() {
                    self.names.insert(world.clone(), name.clone());
                }
                self.wanted.insert(world.clone(), Instant::now());
                self.world.selected = Some((world, name));
            }
            _ if stale => {}
            _ if !signed_in => self.world.error = Some((seq, Refusal::SignIn.code().into(), Refusal::SignIn.text().into())),
            action => match world_action(action, &line.session) {
                Ok((world, action)) => {
                    self.world.error = None;
                    self.push(&world, action);
                }
                Err(refusal) => self.world.error = Some((seq, refusal.code().into(), refusal.text().into())),
            },
        }
        true
    }

    /// Queues an action (kept on disk at once).
    fn push(&mut self, world: &str, action: Action) {
        if let Action::Comment { client_id, .. } = &action {
            if self.queue.for_stamp(&self.stamp).items.iter().any(|p| matches!(&p.action, Action::Comment { client_id: c, .. } if c == client_id)) {
                return;
            }
        }
        self.queue.push_to(world, action, now_ms(), self.stamp.clone());
        self.wanted.insert(world.to_string(), Instant::now());
        self.save();
    }

    fn save(&self) {
        if let Err(e) = self.queue.save(&self.queue_path) {
            eprintln!("world feedback: the queue could not be kept: {e}");
        }
    }

    /// What the member sees: Orbiters' answer with what waits FOR THIS server and account (another account's waiting
    /// comment on this computer is never shown as this one's).
    fn seen(&self, world: &str) -> Seen {
        self.known.get(world).cloned().unwrap_or_default().seen(world, &self.queue.for_stamp(&self.stamp), tr_latin(ME))
    }

    /// The server and account of the moment: another than before (an environment switch, another account) and what
    /// was read and every answer still coming are the other's ([`Self::environment_changed`]).
    fn set_stamp(&mut self, stamp: Stamp) {
        if stamp != self.stamp {
            self.environment_changed();
            self.stamp = stamp;
        }
    }

    /// The local address's answer (contracts `Docs/world-feedback.md`, The probe and the state).
    fn answer(&self, signed_in: bool) -> serde_json::Value {
        let subject = self.world.selected.as_ref().map(|(world, name)| {
            let seen = self.seen(world);
            serde_json::json!({
                "world": world,
                "name": name,
                "loading": seen.loading,
                "likes": seen.likes,
                "liked": seen.liked,
                "labels": seen.labels.iter().map(|(id, count, mine)| serde_json::json!({ "id": id, "count": count, "mine": mine })).collect::<Vec<_>>(),
                "justification": seen.justification,
                "comments": {
                    "count": seen.comment_count,
                    "items": seen.comments.iter().take(contract::COMMENTS_SHOWN).map(|(id, by, text, at, mine)| serde_json::json!({
                        "id": id, "by": by, "text": text, "at": at, "mine": mine,
                    })).collect::<Vec<_>>(),
                },
                "pending": seen.pending,
            })
        });
        serde_json::json!({
            "n4n0": contract::ANSWER_VERSION,
            "app": format!("Orbiters - N4N0 {}", orbiters_core::app_version()),
            "signedIn": signed_in,
            "session": self.world.session,
            "seen": self.world.seen,
            "subject": subject,
            "error": self.world.error.as_ref().map(|(seq, code, message)| serde_json::json!({ "seq": seq, "code": code, "message": message })),
        })
    }

    /// The worlds N4N0's menus give feedback on, and what they show of each.
    fn view(&mut self, here: Option<(String, String)>, signed_in: bool) -> FeedbackView {
        if self.instances.1.is_none_or(|at| at.elapsed() > INSTANCES_EVERY) {
            self.instances = (orbiters_n4n0_core::invites::read_rows(), Some(Instant::now()));
        }
        let mut subjects: Vec<Subject> = Vec::new();
        let add = |subjects: &mut Vec<Subject>, world: &str, name: &str, why: Why| {
            let world = world.to_ascii_lowercase();
            if subjects.len() < SUBJECTS_MAX && contract::is_world_id(&world) && !subjects.iter().any(|s| s.world == world) {
                subjects.push(Subject { world, name: name.to_string(), why });
            }
        };
        if let Some((world, name)) = &self.world.selected {
            add(&mut subjects, world, name, Why::Capture);
        }
        if let Some((world, name)) = &here {
            add(&mut subjects, world, name, Why::Here);
        }
        for row in &self.instances.0 {
            let world = row.location.split(':').next().unwrap_or_default();
            add(&mut subjects, world, &row.world, Why::Instance { at: row.at });
        }
        // The subjects', then those a menu asked about lately that are not among them.
        self.asked.retain(|(_, at)| at.elapsed() < WANTED_FOR);
        let mut shown: Vec<&str> = subjects.iter().map(|s| s.world.as_str()).collect();
        for (world, _) in &self.asked {
            if !shown.contains(&world.as_str()) {
                shown.push(world);
            }
        }
        let worlds = shown.into_iter().map(|w| self.feedback_of(w)).collect();
        FeedbackView { signed_in, subjects, worlds }
    }

    /// What the menus show of a world's feedback.
    fn feedback_of(&self, world: &str) -> WorldFeedback {
        let seen = self.seen(world);
        WorldFeedback {
            world: world.to_string(),
            loading: seen.loading,
            likes: seen.likes,
            liked: seen.liked,
            labels: seen.labels.iter().map(|(id, count, mine)| LabelCount { id: id.clone(), count: *count, mine: *mine }).collect(),
            justification: seen.justification.clone(),
            comments: seen
                .comments
                .iter()
                .map(|(id, by, text, at, mine)| CommentRow {
                    id: *id,
                    by: by.clone(),
                    text: text.clone(),
                    at: at.as_deref().map_or(now_ms() / 1000, rfc3339_seconds),
                    mine: *mine,
                })
                .collect(),
            comment_count: seen.comment_count,
            pending: seen.pending,
            error: self.menu_errors.get(world).cloned(),
        }
    }

    /// A menu asked about `world` (lowercase): its feedback goes to the menus for a while, whatever their subjects.
    fn asked(&mut self, world: &str) {
        if !contract::is_world_id(world) {
            return;
        }
        self.asked.retain(|(w, _)| w != world);
        self.asked.insert(0, (world.to_string(), Instant::now()));
        self.asked.truncate(ASKED_MAX);
    }

    /// The next thing to do with Orbiters: an action due, else a wanted world not read lately.
    fn next_call(&mut self) -> Option<Call> {
        let now = now_ms();
        self.wanted.retain(|_, at| at.elapsed() < WANTED_FOR);
        // What waits for another server or account is never sent here; it leaves after a day.
        let before = self.queue.items.len();
        self.queue.expire(now);
        if self.queue.items.len() != before {
            self.save();
        }
        // A refused credential's account sends nothing until its pause ends, actions added meanwhile included.
        let parked = self.parked.iter().any(|(stamp, until)| stamp.sends_as(&self.stamp) && now < *until);
        if let Some(i) = self.queue.due_for(now, &self.stamp).filter(|_| !parked) {
            let p = &self.queue.items[i];
            return Some(Call::Send { world: p.world.clone(), action: p.action.clone(), op: p.op, to: self.stamp.clone() });
        }
        let stale = |k: Option<&Known>| k.is_none_or(|k| now.saturating_sub(k.read_at) > READ_AGAIN.as_millis() as u64);
        let world = self.wanted.keys().find(|w| stale(self.known.get(*w))).cloned()?;
        // Not asked again before this answer comes.
        self.known.entry(world.clone()).or_default().read_at = now;
        Some(Call::Read { world })
    }

    /// How long until [`Self::next_call`] may have one by the clock alone: never with nothing queued and no world shown (what is
    /// queued and shown meanwhile wakes the sender).
    fn next_due(&self, now: u64) -> Option<Duration> {
        let parked = self.parked.iter().filter(|(stamp, until)| stamp.sends_as(&self.stamp) && now < *until).map(|(_, until)| *until).max();
        let queued = self.queue.items.iter().filter(|p| p.to.as_ref().is_some_and(|to| to.sends_as(&self.stamp))).map(|p| p.due.max(parked.unwrap_or(0)));
        let read = self.wanted.keys().map(|w| self.known.get(w).map_or(now, |k| k.read_at + READ_AGAIN.as_millis() as u64 + 1));
        queued.chain(read).min().map(|at| Duration::from_millis(at.saturating_sub(now).max(300)))
    }

    /// The other server's answers are forgotten, and those still coming are not taken.
    fn environment_changed(&mut self) {
        self.known.clear();
        self.menu_errors.clear();
        self.sent = None;
        self.scope = self.scope.wrapping_add(1);
    }

    /// What Orbiters answered to `call`.
    fn answered(&mut self, call: &Call, result: Result<Option<orbiters_core::world_feedback::ServerView>, Failure>) {
        self.answered_in(call, result, self.scope);
    }

    /// [`Self::answered`] for a call made in `scope`: when the environment changed since, the answer is the other
    /// server's, and only what it did to the queue counts (a sent action is not sent again); what it says of the world
    /// is not shown.
    fn answered_in(&mut self, call: &Call, result: Result<Option<orbiters_core::world_feedback::ServerView>, Failure>, scope: u64) {
        let now = now_ms();
        let index = |q: &Queue| match call {
            Call::Send { op, .. } => q.items.iter().position(|p| p.op == *op),
            Call::Read { .. } => None,
        };
        if scope != self.scope {
            if let Some(i) = index(&self.queue) {
                match result {
                    Ok(_) | Err(Failure::Refused { .. }) => drop(self.queue.items.remove(i)),
                    Err(_) => self.queue.retry(i, now),
                }
                self.save();
            }
            return;
        }
        match (call, result) {
            (Call::Read { world }, Ok(Some(view))) | (Call::Send { world, .. }, Ok(Some(view))) => {
                self.known.insert(world.clone(), Known { view: Some(view), read_at: now });
                if let Some(i) = index(&self.queue) {
                    self.queue.items.remove(i);
                    self.save();
                }
                self.menu_errors.remove(world);
            }
            (Call::Send { world, .. }, Ok(None)) => {
                if let Some(i) = index(&self.queue) {
                    self.queue.items.remove(i);
                    self.save();
                }
                // Read again at once: the comment shows with its id.
                if let Some(k) = self.known.get_mut(world) {
                    k.read_at = 0;
                }
                self.wanted.insert(world.clone(), Instant::now());
                self.menu_errors.remove(world);
            }
            (Call::Read { world }, Ok(None)) => {
                self.known.entry(world.clone()).or_default().read_at = now;
            }
            (Call::Send { world, .. }, Err(Failure::Refused { code, message })) => {
                if let Some(i) = index(&self.queue) {
                    self.queue.items.remove(i);
                    self.save();
                }
                let code = code.map_or("refused", |c| c.code()).to_string();
                self.world.error = Some((self.world.seen, code, message.clone()));
                self.menu_errors.insert(world.clone(), message);
            }
            (Call::Send { to, .. }, Err(Failure::SignedOut)) => {
                // The credential is refused: nothing of the account that action was sent as goes for a while (the account
                // is asked about again meanwhile), its actions added meanwhile included, rather than each action again
                // every minute for a day.
                let until = now + SIGNED_OUT_PAUSE.as_millis() as u64;
                self.parked.retain(|(_, u)| *u > now);
                self.parked.push((to.clone(), until));
            }
            (Call::Send { .. }, Err(_)) => {
                if let Some(i) = index(&self.queue) {
                    self.queue.retry(i, now);
                    self.save();
                }
            }
            (Call::Read { world }, Err(_)) => {
                // Tried again after READ_AGAIN. Meanwhile the menus say Orbiters did not answer, rather than reading for ever (VR feedback F6: the development
                // server answered 500 to every read, and Write a comment waited on it without a word).
                self.known.entry(world.clone()).or_default().read_at = now;
                if self.known.get(world).is_none_or(|k| k.view.is_none()) {
                    self.menu_errors.insert(world.clone(), tr("Orbiters did not answer for this world's feedback. It tries again by itself.").to_string());
                }
            }
        }
    }
}

/// A world's line as an action to send, or why it is refused (the contract's rules, as Orbiters checks them too).
fn world_action(action: Line, session: &str) -> Result<(String, Action), Refusal> {
    Ok(match action {
        Line::Like { world, on } => (world.to_ascii_lowercase(), Action::Set(Set { liked: Some(on), ..Default::default() })),
        Line::Label { world, label, on, text } => {
            let known = contract::label(&label).ok_or(Refusal::UnknownLabel)?;
            let justification = match (on, known.justify) {
                (true, Some(_)) => Some(contract::justification(known, text.as_deref().unwrap_or(""))?),
                _ => None,
            };
            (world.to_ascii_lowercase(), Action::Set(Set { labels: [(label, on)].into_iter().collect(), justification, ..Default::default() }))
        }
        Line::Comment { world, text, id } => {
            (world.to_ascii_lowercase(), Action::Comment { text: contract::comment(&text)?, client_id: format!("n4n0w-{session}-{id}") })
        }
        Line::Uncomment { world, comment } => (world.to_ascii_lowercase(), Action::Uncomment { comment }),
        Line::Report { world, comment, reason } => (world.to_ascii_lowercase(), Action::Report { comment, reason }),
        Line::Hello | Line::Select { .. } => return Err(Refusal::NotFound),
    })
}

/// A menu's ask as an action to send (`Show` reads a world).
fn menu_action(ask: &FeedbackAsk) -> Option<(String, Action)> {
    Some(match ask.clone() {
        FeedbackAsk::Show { .. } => return None,
        FeedbackAsk::Like { world, on } => (world, Action::Set(Set { liked: Some(on), ..Default::default() })),
        FeedbackAsk::Label { world, label, on, text } => {
            (world, Action::Set(Set { labels: [(label, on)].into_iter().collect(), justification: text, ..Default::default() }))
        }
        FeedbackAsk::Comment { world, text } => (world, Action::Comment { text, client_id: client_id() }),
        FeedbackAsk::Uncomment { world, comment } => (world, Action::Uncomment { comment }),
    })
}

/// A call to Orbiters.
#[derive(Clone, Debug)]
enum Call {
    Read { world: String },
    /// An action waiting: its own number, and the server and account it was made for and goes as.
    Send { world: String, action: Action, op: u64, to: Stamp },
}

fn perform(call: &Call, api: Option<&(Api, String)>) -> Result<Option<orbiters_core::world_feedback::ServerView>, Failure> {
    let read_api;
    let client = match api {
        Some((api, token)) => Client { api, token: Some(token.as_str()) },
        None => {
            read_api = Api::new(orbiters_core::account::Server::current());
            Client { api: &read_api, token: None }
        }
    };
    match call {
        Call::Read { world } => client.read(world).map(Some),
        Call::Send { .. } if api.is_none() => Err(Failure::SignedOut),
        Call::Send { world, action, .. } => match action {
            Action::Set(set) => client.set(world, set),
            Action::Comment { text, client_id } => client.comment(world, text, client_id).map(|_| None),
            Action::Uncomment { comment } => client.uncomment(*comment).map(|_| None),
            Action::Report { comment, reason } => client.report(*comment, reason.as_deref()).map(|_| None),
        },
    }
}

/// `2026-10-29T21:04:00Z` (or with milliseconds) in Unix seconds; now when it does not read.
fn rfc3339_seconds(text: &str) -> u64 {
    let b = text.as_bytes();
    let n = |from: usize, len: usize| text.get(from..from + len).and_then(|s| s.parse::<i64>().ok());
    let parsed = (|| {
        if b.len() < 19 || b[4] != b'-' || b[10] != b'T' {
            return None;
        }
        let (y, mo, d, h, mi, s) = (n(0, 4)?, n(5, 2)?, n(8, 2)?, n(11, 2)?, n(14, 2)?, n(17, 2)?);
        // Days from the civil date (Howard Hinnant's algorithm).
        let y = if mo <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        Some((days * 86_400 + h * 3600 + mi * 60 + s) as u64)
    })();
    parsed.unwrap_or_else(|| now_ms() / 1000)
}

impl super::Controller {
    fn feedback_api(&self) -> Option<(Api, String)> {
        self.account_api()
    }

    /// The world the player is in now (VRChat's log as the watchers read it), with its name.
    fn feedback_here(&self) -> Option<(String, String)> {
        let s = self.snapshot.lock().unwrap();
        let w = &s.vrchat.world;
        if !w.in_world {
            return None;
        }
        w.world_id.clone().map(|id| (id, w.world_name.clone().unwrap_or_default()))
    }

    /// Hands N4N0's menus what they show of world feedback, when it changed.
    fn feedback_publish(self: &Shared) {
        let here = self.feedback_here();
        let signed_in = self.feedback_api().is_some();
        let view = {
            let mut rt = self.world_feedback.lock().unwrap();
            let view = rt.view(here, signed_in);
            if rt.sent.as_ref() == Some(&view) {
                return;
            }
            rt.sent = Some(view.clone());
            view
        };
        self.n4n0.set_feedback(view);
    }

    /// What one of N4N0's menus asked (controller/n4n0.rs: `Host::world_feedback`).
    pub(super) fn world_feedback_ask(self: &Shared, ask: FeedbackAsk) {
        let (_, stamp) = self.account_api_for();
        {
            let mut rt = self.world_feedback.lock().unwrap();
            rt.set_stamp(stamp);
            match menu_action(&ask) {
                Some((world, action)) => {
                    let world = world.to_ascii_lowercase();
                    rt.menu_errors.remove(&world);
                    rt.asked(&world);
                    rt.push(&world, action);
                }
                None => {
                    if let FeedbackAsk::Show { world } = &ask {
                        let world = world.to_ascii_lowercase();
                        rt.asked(&world);
                        rt.wanted.insert(world.clone(), Instant::now());
                        // Read now rather than when it was last read.
                        if let Some(k) = rt.known.get_mut(&world) {
                            k.read_at = k.read_at.min(now_ms().saturating_sub(READ_AGAIN.as_millis() as u64 - 1000));
                        }
                    }
                }
            }
        }
        self.rests.wake("world-feedback-send");
        self.feedback_publish();
    }

    /// Why the N4N0 world cannot reach this app, when it cannot (for the window).
    pub fn world_feedback_note(&self) -> Option<String> {
        self.world_feedback.lock().unwrap().port_note.clone()
    }

    /// World label changes and comments waiting to be sent (to whichever environment is active when they go).
    pub(super) fn feedback_pending(&self) -> u32 {
        let rt = self.world_feedback.lock().unwrap();
        rt.queue.for_stamp(&rt.stamp).items.len() as u32
    }

    /// The Development environment was switched: what Orbiters answered for worlds is the other server's, so it is read again.
    pub(super) fn world_feedback_environment_changed(&self) {
        self.world_feedback.lock().unwrap().environment_changed();
    }

    fn feedback_follow(self: &Shared) -> bool {
        let (development, api, stamp) = self.feedback_context();
        let mut rt = self.world_feedback.lock().unwrap();
        rt.set_stamp(stamp);
        let changed = rt.follow(development, api.is_some());
        drop(rt);
        if changed {
            // A world to read, an action to send.
            self.rests.wake("world-feedback-send");
        }
        changed
    }

    /// One read of whom world feedback is for now, for EVERY reader of the log (the follower, the world's local address):
    /// the environment chosen now, its account (when read for it) and the policy that goes with that environment, never
    /// one from before a switch and one from after.
    fn feedback_context(&self) -> (bool, Option<(Api, String)>, Stamp) {
        let (api, stamp) = self.account_api_for();
        let development = stamp.server == Api::new(orbiters_core::account::Server::Development).url("");
        (development, api, stamp)
    }
}

/// World feedback for the VRChat bridge's label tools (bridge/tools.rs `world_labels`, `label_world`): what a press on the
/// worlds page asks goes through [`super::Controller::world_feedback_ask`] here too, so the queue, the account, Orbiters'
/// rules and its limits are the person's own.
pub(super) struct RealFeedback {
    pub(super) ctl: std::sync::Weak<super::Controller>,
}

impl crate::bridge::game::Feedback for RealFeedback {
    fn account(&self) -> Option<crate::bridge::game::FeedbackAccount> {
        let (server, name, id) = self.ctl.upgrade()?.feedback_account()?;
        Some(crate::bridge::game::FeedbackAccount {
            server: server.api().to_string(),
            development: server == orbiters_core::account::Server::Development,
            name,
            id,
        })
    }

    fn show(&self, world: &str) {
        if let Some(ctl) = self.ctl.upgrade() {
            ctl.world_feedback_ask(FeedbackAsk::Show { world: world.to_string() });
        }
    }

    fn feedback(&self, world: &str) -> WorldFeedback {
        self.ctl.upgrade().map(|ctl| ctl.world_feedback.lock().unwrap().feedback_of(&world.to_ascii_lowercase())).unwrap_or_default()
    }

    fn ask(&self, ask: FeedbackAsk) {
        if let Some(ctl) = self.ctl.upgrade() {
            ctl.world_feedback_ask(ask);
        }
    }
}

/// Starts world feedback: the log's follower, the calls to Orbiters and the local address.
pub(super) fn spawn(ctl: Shared) {
    let c = ctl.clone();
    let _ = std::thread::Builder::new().name("world-feedback-log".into()).spawn(move || {
        let mut beat = 0u32;
        let rest = c.rest("world-feedback-log", |s| (s.vrchat.running, s.steamvr.running, Part::of(&s.account), s.settings.development));
        while !c.quitting.load(std::sync::atomic::Ordering::Relaxed) {
            let changed = c.feedback_follow();
            // The menus follow the world, the queue and what Orbiters answers (at most every second when nothing moves).
            if changed || beat.is_multiple_of(4) {
                c.feedback_publish();
            }
            // Without VRChat no line of its log comes, and nothing moves but what publishes itself.
            let view = c.view();
            let here = view.vrchat.running || view.steamvr.running || c.shown();
            beat = if here { beat.wrapping_add(1) } else { 0 };
            rest.wait(here.then_some(LOG_EVERY));
        }
    });
    let c = ctl.clone();
    let _ = std::thread::Builder::new().name("world-feedback-send".into()).spawn(move || {
        let rest = c.rest("world-feedback-send", |s| (Part::of(&s.account), s.settings.development));
        while !c.quitting.load(std::sync::atomic::Ordering::Relaxed) {
            // The account and server read once: what is handed out is for them, and goes with them.
            let (api, stamp) = c.account_api_for();
            let (call, scope) = {
                let mut rt = c.world_feedback.lock().unwrap();
                // Switched between reading the account and here: read it again, never a call for the other server.
                if stamp.server != Api::new(orbiters_core::account::Server::current()).url("") {
                    drop(rt);
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                rt.set_stamp(stamp);
                (rt.next_call(), rt.scope)
            };
            let Some(call) = call else {
                let due = c.world_feedback.lock().unwrap().next_due(now_ms());
                rest.wait(due);
                continue;
            };
            let result = perform(&call, api.as_ref());
            if matches!(result, Err(Failure::SignedOut)) && api.is_none() {
                // Nothing to send with: kept until an account is signed in.
                if let Call::Send { op, .. } = &call {
                    let mut rt = c.world_feedback.lock().unwrap();
                    if let Some(i) = rt.queue.items.iter().position(|p| p.op == *op) {
                        rt.queue.items[i].due = now_ms() + 10_000;
                    }
                }
            } else {
                c.world_feedback.lock().unwrap().answered_in(&call, result, scope);
            }
            c.feedback_publish();
        }
    });
    if !cfg!(test) {
        let _ = std::thread::Builder::new().name("world-feedback-probe".into()).spawn(move || serve(ctl));
    }
}

/// The local address: `GET /n4n0/feedback/v1` on 127.0.0.1:22541 (contracts `Docs/world-feedback.md`).
fn serve(ctl: Shared) {
    let listener = loop {
        if ctl.quitting.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        match TcpListener::bind(("127.0.0.1", contract::PORT)) {
            Ok(l) => {
                ctl.world_feedback.lock().unwrap().port_note = None;
                break l;
            }
            Err(e) => {
                ctl.world_feedback.lock().unwrap().port_note =
                    Some(tr!("Port {port} is taken ({error}): the N4N0 world cannot reach Orbiters - N4N0 to give feedback", port = contract::PORT, error = e));
                std::thread::sleep(Duration::from_secs(30));
            }
        }
    };
    for stream in listener.incoming() {
        if ctl.quitting.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let Ok(stream) = stream else { continue };
        let c = ctl.clone();
        let _ = std::thread::Builder::new().name("world-feedback-answer".into()).spawn(move || {
            let _ = answer(&c, stream);
        });
    }
}

/// The request's method, path and Host, from at most 8 KB of it.
fn request(stream: &mut TcpStream) -> Option<(String, String, String)> {
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut data = Vec::new();
    let mut buf = [0u8; 2048];
    while !data.windows(4).any(|w| w == b"\r\n\r\n") && data.len() < 8192 {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
    }
    let text = String::from_utf8_lossy(&data);
    let mut lines = text.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, target) = (first.next()?.to_string(), first.next()?.to_string());
    let host =
        lines.find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("host")).map(|(_, v)| v.trim().to_string())).unwrap_or_default();
    Some((method, target, host))
}

/// The status and body for a request: only `GET` of the path, only to 127.0.0.1:22541 or localhost:22541, only while
/// the player is in a world whose lines count.
fn route(method: &str, target: &str, host: &str, in_world: bool) -> Result<(), (u16, &'static str)> {
    let path = target.split('?').next().unwrap_or_default();
    let hosts = [format!("127.0.0.1:{}", contract::PORT), format!("localhost:{}", contract::PORT)];
    if !hosts.iter().any(|h| h.eq_ignore_ascii_case(host)) {
        return Err((421, "Misdirected Request")); // i18n: not shown (the HTTP reason phrase of the status line a world's page reads)
    }
    if method != "GET" || path != contract::PATH || !in_world {
        return Err((404, "Not Found")); // i18n: not shown (the HTTP reason phrase of the status line a world's page reads)
    }
    Ok(())
}

fn answer(ctl: &Shared, stream: TcpStream) -> std::io::Result<()> {
    let (development, api, stamp) = ctl.feedback_context();
    respond(&ctl.world_feedback, stream, development, api.is_some(), Some(stamp))
}

/// Answers one request on `stream` from what `runtime` knows, once it read the log to its end.
fn respond(runtime: &std::sync::Mutex<WorldFeedbackRuntime>, mut stream: TcpStream, development: bool, signed_in: bool, stamp: Option<Stamp>) -> std::io::Result<()> {
    let Some((method, target, host)) = request(&mut stream) else { return Ok(()) };
    let (status, body) = {
        let mut rt = runtime.lock().unwrap();
        // The lines this reads are queued for whom the follower would queue them now (verify2: never the scope of before).
        if let Some(stamp) = stamp {
            rt.set_stamp(stamp);
        }
        // What the world logged before asking counts in this answer.
        rt.follow(development, signed_in);
        match route(&method, &target, &host, accepted(rt.joined.as_deref(), development)) {
            Ok(()) => ((200, "OK"), rt.answer(signed_in).to_string()),
            Err(status) => (status, String::new()),
        }
    };
    let head = if body.is_empty() {
        format!("HTTP/1.1 {} {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", status.0, status.1) // i18n: not shown
    } else {
        format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/json; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            status.0,
            status.1,
            body.len()
        )
    };
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORLD: &str = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd";

    fn line(seq: u64, body: &str) -> contract::Line {
        contract::parse(&format!(r#"[N4N0-Feedback] {{"v":1,"session":"5f3a9c01","seq":{seq},{body}}}"#)).expect("a line")
    }

    #[test]
    fn the_worlds_lines_become_actions_once_each() {
        let mut rt =
            WorldFeedbackRuntime { queue: Queue::default(), queue_path: std::env::temp_dir().join("orbiters-feedback-unit.json"), ..Default::default() };
        assert!(rt.line(line(1, r#""t":"hello""#), false, true));
        rt.line(line(2, &format!(r#""t":"select","world":"{WORLD}","name":"Home","preview":1,"slot":2,"key":3"#)), false, true);
        assert_eq!(rt.world.selected, Some((WORLD.into(), "Home".into())));
        rt.line(line(3, &format!(r#""t":"like","world":"{WORLD}","on":true"#)), false, true);
        assert!(!rt.line(line(3, &format!(r#""t":"like","world":"{WORLD}","on":false"#)), false, true), "the same number again");
        rt.line(line(4, &format!(r#""t":"label","world":"{WORLD}","label":"ai-world","on":true,"text":"too short""#)), false, true);
        assert_eq!(rt.world.error.as_ref().map(|e| e.1.as_str()), Some("justification-too-short"));
        rt.line(line(5, &format!(r#""t":"label","world":"{WORLD}","label":"mirror","on":true"#)), false, true);
        assert_eq!(rt.world.error, None, "an accepted action clears the error");
        rt.line(line(6, &format!(r#""t":"comment","world":"{WORLD}","text":"Great!","id":"9c1e04ab""#)), false, true);
        rt.line(line(7, &format!(r#""t":"comment","world":"{WORLD}","text":"Great!","id":"9c1e04ab""#)), false, true);
        assert_eq!(rt.queue.items.len(), 2, "likes and labels together, one comment");
        let Action::Set(set) = &rt.queue.items[0].action else { panic!() };
        assert_eq!((set.liked, set.labels.get("mirror")), (Some(true), Some(&true)));
        assert!(matches!(&rt.queue.items[1].action, Action::Comment { client_id, .. } if client_id == "n4n0w-5f3a9c01-9c1e04ab"));
        rt.line(line(8, &format!(r#""t":"like","world":"{WORLD}","on":false"#)), true, true);
        let Action::Set(set) = &rt.queue.items[0].action else { panic!() };
        assert_eq!(set.liked, Some(true), "a stale line is not acted on");
        rt.line(line(9, &format!(r#""t":"like","world":"{WORLD}","on":false"#)), false, false);
        assert_eq!(rt.world.error.as_ref().map(|e| e.1.as_str()), Some("sign-in"));
        let answer = rt.answer(true);
        assert_eq!(answer["seen"], 9);
        assert_eq!(answer["subject"]["world"], WORLD);
        assert_eq!(answer["subject"]["liked"], true);
        assert_eq!(answer["subject"]["likes"], 1);
        assert_eq!(answer["subject"]["comments"]["items"][0]["text"], "Great!");
        assert_eq!(answer["subject"]["comments"]["items"][0]["by"], "You");
        assert_eq!(answer["error"]["code"], "sign-in");
        // A new session (the world loaded again) starts over.
        let again = contract::parse(r#"[N4N0-Feedback] {"v":1,"t":"hello","session":"00000001","seq":1}"#).unwrap();
        assert!(rt.line(again, false, true));
        assert_eq!((rt.world.seen, rt.world.selected.clone()), (1, None));
    }

    /// What the world logs is in the next answer of the local address, over a real socket and a log file in VRChat's
    /// shape (`Docs/world-feedback.md`): the probe while in the N4N0 world, the subject, the like, the next line read
    /// before answering, another host refused.
    #[test]
    fn the_world_reads_what_it_logged_through_the_local_address() {
        let dir = std::env::temp_dir().join(format!("orbiters-feedback-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("output_log_2026-10-29_21-00-00.txt");
        let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
        let stamp = format!("{:04}.{:02}.{:02} {:02}:{:02}:{:02}", now.year(), now.month() as u8, now.day(), now.hour(), now.minute(), now.second());
        let line = |message: &str| format!("{stamp} Debug      -  {message}\n");
        let lines = [
            line(&format!("[Behaviour] Joining {}:12345~region(eu)", contract::N4N0_WORLD)),
            line(r#"[N4N0-Feedback] {"v":1,"t":"hello","session":"5f3a9c01","seq":1}"#),
            line(&format!(
                r#"[N4N0-Feedback] {{"v":1,"t":"select","session":"5f3a9c01","seq":2,"world":"{WORLD}","name":"Home","preview":1,"slot":2,"key":3}}"#
            )),
            line(&format!(r#"[N4N0-Feedback] {{"v":1,"t":"like","session":"5f3a9c01","seq":3,"world":"{WORLD}","on":true}}"#)),
        ];
        std::fs::write(&log, lines.concat()).unwrap();
        let rt = std::sync::Mutex::new(WorldFeedbackRuntime {
            log: TargetLog::new(Some(dir.clone()), Default::default()),
            queue: Queue::default(),
            queue_path: dir.join("queue.json"),
            ..Default::default()
        });
        let ask = |rt: &std::sync::Mutex<WorldFeedbackRuntime>, host: &str| -> (String, serde_json::Value) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    let (stream, _) = listener.accept().unwrap();
                    respond(rt, stream, false, true, None).unwrap();
                });
                let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
                write!(client, "GET /n4n0/feedback/v1 HTTP/1.1\r\nHost: {host}\r\nUser-Agent: UnityPlayer\r\n\r\n").unwrap();
                let mut text = String::new();
                client.read_to_string(&mut text).unwrap();
                let (head, body) = text.split_once("\r\n\r\n").unwrap();
                (head.to_string(), serde_json::from_str(body).unwrap_or(serde_json::Value::Null))
            })
        };
        let (head, json) = ask(&rt, "127.0.0.1:22541");
        assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
        assert!(head.contains("Content-Type: application/json; charset=utf-8") && !head.contains("Access-Control"));
        assert_eq!((json["n4n0"].as_u64(), json["seen"].as_u64(), json["signedIn"].as_bool()), (Some(1), Some(3), Some(true)));
        assert_eq!(json["subject"]["world"], WORLD);
        assert_eq!(
            (json["subject"]["liked"].as_bool(), json["subject"]["likes"].as_u64(), json["subject"]["pending"].as_u64()),
            (Some(true), Some(1), Some(1))
        );
        // A line written just before asking is in the answer.
        let more = line(&format!(r#"[N4N0-Feedback] {{"v":1,"t":"label","session":"5f3a9c01","seq":4,"world":"{WORLD}","label":"pens","on":true}}"#));
        std::fs::OpenOptions::new().append(true).open(&log).unwrap().write_all(more.as_bytes()).unwrap();
        let (_, json) = ask(&rt, "localhost:22541");
        assert_eq!(json["seen"].as_u64(), Some(4));
        assert_eq!(json["subject"]["labels"][0]["id"], "pens");
        let (head, _) = ask(&rt, "attacker.example:22541");
        assert!(head.starts_with("HTTP/1.1 421"), "{head}");
        // Elsewhere, nothing.
        std::fs::OpenOptions::new().append(true).open(&log).unwrap().write_all(line(&format!("[Behaviour] Joining {WORLD}:1")).as_bytes()).unwrap();
        let (head, _) = ask(&rt, "127.0.0.1:22541");
        assert!(head.starts_with("HTTP/1.1 404"), "{head}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// DCT-3: the lines that count are the followed VRChat's alone, of two in worlds whose lines count; and when the other one is
    /// followed, its past is not news, and nothing of the first one's world stays.
    #[test]
    fn only_the_followed_vrchats_lines_count() {
        use super::super::vrchats::{run_of, LogFacts};
        use crate::bridge::sessions::{Driver, Sides};
        let dir = std::env::temp_dir().join(format!("orbiters-feedback-two-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let line = |message: &str| format!("2026.10.18 20:23:20 Debug      -  {message}\n");
        let said = |session: &str, seq: u64, body: &str| line(&format!(r#"[N4N0-Feedback] {{"v":1,"session":"{session}","seq":{seq},{body}}}"#));
        let joined = line(&format!("[Behaviour] Joining {}:12345~region(eu)", contract::N4N0_WORLD));
        let like = |session: &str, seq| said(session, seq, &format!(r#""t":"like","world":"{WORLD}","on":true"#));
        let (own, side) = (dir.join("output_log_2026-11-15_13-38-44.txt"), dir.join("output_log_2026-11-15_14-00-00.txt"));
        std::fs::write(&own, [joined.clone(), said("5f3a9c01", 1, &format!(r#""t":"select","world":"{WORLD}","name":"Home","preview":1,"slot":2,"key":3"#))].concat()).unwrap();
        std::fs::write(&side, &joined).unwrap();
        let run = |pid: u32, log: &PathBuf, driver: Driver| crate::bridge::sessions::VrchatRun { driver, ..run_of(pid, 1, Some(log), Some(&LogFacts::default())) };
        let sides = std::sync::Arc::new(Sides::default());
        sides.set_roster(vec![run(1, &own, Driver::Person), run(2, &side, Driver::Bridge)]);
        let mut rt = WorldFeedbackRuntime {
            log: TargetLog::new(Some(dir.clone()), sides.clone()),
            queue: Queue::default(),
            queue_path: dir.join("queue.json"),
            ..Default::default()
        };
        assert!(rt.follow(false, true));
        let write = |log: &PathBuf, text: String| std::fs::OpenOptions::new().append(true).open(log).unwrap().write_all(text.as_bytes()).unwrap();
        write(&side, [said("00000002", 1, r#""t":"hello""#), like("00000002", 2)].concat());
        rt.follow(false, true);
        assert_eq!(rt.queue.items.len(), 0, "the other VRChat's line is nobody's here");
        assert_eq!((rt.world.session.as_deref(), rt.world.selected.as_ref().map(|s| s.1.as_str())), (Some("5f3a9c01"), Some("Home")));
        write(&own, like("5f3a9c01", 2));
        rt.follow(false, true);
        assert_eq!(rt.queue.items.len(), 1, "the followed one's is the member's");
        // The other one is followed from now on.
        sides.choose(Some(2));
        assert!(rt.follow(false, true));
        assert_eq!((rt.queue.items.len(), rt.world.session.as_deref(), rt.world.selected.clone()), (1, Some("00000002"), None), "its past is not acted on");
        write(&side, said("00000002", 3, &format!(r#""t":"comment","world":"{WORLD}","text":"Great!","id":"9c1e04ab""#)));
        rt.follow(false, true);
        assert_eq!(rt.queue.items.len(), 2, "what it says now is");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The overlay's 3D worlds panel shows any capture's world: what it asks about comes back with the subjects'.
    #[test]
    fn a_world_a_menu_asked_about_comes_with_the_subjects() {
        let mut rt = WorldFeedbackRuntime {
            queue: Queue::default(),
            queue_path: std::env::temp_dir().join(format!("orbiters-feedback-asked-{}.json", std::process::id())),
            instances: (Vec::new(), Some(Instant::now())),
            ..Default::default()
        };
        rt.asked("not a world");
        assert!(rt.view(None, true).worlds.is_empty());
        rt.asked(WORLD);
        rt.push(WORLD, Action::Set(Set { liked: Some(true), ..Default::default() }));
        let view = rt.view(None, true);
        assert!(view.subjects.is_empty(), "asked, not a subject");
        assert_eq!(view.worlds.iter().map(|w| (w.world.as_str(), w.liked, w.likes, w.pending)).collect::<Vec<_>>(), [(WORLD, true, 1, 1)]);
        // The world the player is in and asked about: once.
        let view = rt.view(Some((WORLD.to_uppercase().replace("WRLD", "wrld"), "Home".into())), true);
        assert_eq!((view.subjects.len(), view.worlds.len()), (1, 1));
        // The newest few only.
        for i in 0..ASKED_MAX + 2 {
            rt.asked(&format!("wrld_4432ea9b-729c-46e3-8eaf-{i:012x}"));
        }
        let view = rt.view(None, true);
        assert_eq!(view.worlds.len(), ASKED_MAX);
        assert_eq!(view.worlds[0].world, format!("wrld_4432ea9b-729c-46e3-8eaf-{:012x}", ASKED_MAX + 1));
        let _ = std::fs::remove_file(&rt.queue_path);
    }

    /// Verify2 (night4): a refused credential parks its account, actions added after the refusal included; an answer is
    /// for its own action (its number), never one made for another account in the same millisecond; nobody known sends
    /// nothing.
    #[test]
    fn a_refusal_parks_its_account_and_answers_find_their_own_action() {
        let mut rt = WorldFeedbackRuntime {
            queue: Queue::default(),
            queue_path: std::env::temp_dir().join(format!("orbiters-feedback-parked-{}.json", uuid::Uuid::new_v4())),
            instances: (Vec::new(), Some(Instant::now())),
            ..Default::default()
        };
        let a = Stamp { server: "https://api.orbiters.cc/".into(), account: Some("1".into()), credential: None };
        rt.set_stamp(a.clone());
        rt.push(WORLD, Action::Uncomment { comment: 1 });
        let Some(call @ Call::Send { .. }) = rt.next_call() else { panic!("a send") };
        let scope = rt.scope;
        rt.answered_in(&call, Err(Failure::SignedOut), scope);
        rt.push(WORLD, Action::Uncomment { comment: 2 });
        assert!(rt.next_call().is_none_or(|c| !matches!(c, Call::Send { .. })), "parked, the later action too");
        // Two identical actions for two accounts: the answer to B's leaves A's.
        let mut q = Queue::default();
        let b = Stamp { account: Some("2".into()), ..a.clone() };
        let at = now_ms();
        q.push_to(WORLD, Action::Uncomment { comment: 5 }, at, a.clone());
        q.push_to(WORLD, Action::Uncomment { comment: 5 }, at, b.clone());
        rt.queue = q;
        rt.parked.clear();
        rt.set_stamp(b.clone());
        let Some(call @ Call::Send { .. }) = rt.next_call() else { panic!("B's send") };
        let scope = rt.scope;
        rt.answered_in(&call, Ok(None), scope);
        assert_eq!(rt.queue.items.iter().map(|p| p.to.clone().unwrap().account).collect::<Vec<_>>(), [Some("1".to_string())], "A's stays");
        // Nobody known: nothing goes.
        rt.set_stamp(Stamp { account: None, ..a.clone() });
        rt.push(WORLD, Action::Uncomment { comment: 9 });
        assert!(rt.next_call().is_none_or(|c| !matches!(c, Call::Send { .. })));
        let _ = std::fs::remove_file(&rt.queue_path);
    }

    /// Verifier (CORE2-1): another account on the same server (or another server) is another scope: A's late answer is
    /// not shown to B, A's waiting comment is not B's, and it is never sent as B.
    #[test]
    fn another_account_is_another_scope() {
        let mut rt = WorldFeedbackRuntime {
            queue: Queue::default(),
            queue_path: std::env::temp_dir().join(format!("orbiters-feedback-accounts-{}.json", uuid::Uuid::new_v4())),
            instances: (Vec::new(), Some(Instant::now())),
            ..Default::default()
        };
        let a = Stamp { server: "https://api.orbiters.cc/".into(), account: Some("1".into()), credential: None };
        let b = Stamp { account: Some("2".into()), ..a.clone() };
        rt.set_stamp(a);
        rt.asked(WORLD);
        rt.push(WORLD, Action::Comment { text: "A's draft".into(), client_id: "n4n0-a".into() });
        let read = Call::Read { world: WORLD.into() };
        let asked_as_a = rt.scope;
        rt.set_stamp(b);
        assert_ne!(rt.scope, asked_as_a, "a new account is a new scope");
        let answer = orbiters_core::world_feedback::ServerView { world_id: WORLD.into(), likes: 3, ..Default::default() };
        rt.answered_in(&read, Ok(Some(answer)), asked_as_a);
        assert!(rt.known.get(WORLD).is_none_or(|k| k.view.is_none()), "A's answer is not B's view");
        assert!(rt.seen(WORLD).comments.iter().all(|c| c.2 != "A's draft") && rt.seen(WORLD).pending == 0, "A's waiting comment is not shown to B");
        assert!(rt.next_call().is_none_or(|c| !matches!(c, Call::Send { .. })), "nor sent as B");
        let _ = std::fs::remove_file(&rt.queue_path);
    }

    /// CORE-C2: a read the server of before an environment switch answers late shows nothing of that server in the
    /// new one; an action it took is not sent again.
    #[test]
    fn an_answer_from_before_an_environment_switch_is_not_shown() {
        let mut rt = WorldFeedbackRuntime {
            queue: Queue::default(),
            queue_path: std::env::temp_dir().join(format!("orbiters-feedback-switch-{}.json", uuid::Uuid::new_v4())),
            instances: (Vec::new(), Some(Instant::now())),
            ..Default::default()
        };
        rt.asked(WORLD);
        rt.push(WORLD, Action::Set(Set { liked: Some(true), ..Default::default() }));
        let read = Call::Read { world: WORLD.into() };
        let p = rt.queue.items[0].clone();
        let send = Call::Send { world: p.world.clone(), action: p.action.clone(), op: p.op, to: Stamp::default() };
        let before = rt.scope;
        rt.environment_changed();
        let theirs = orbiters_core::world_feedback::ServerView { world_id: WORLD.into(), likes: 41, ..Default::default() };
        rt.answered_in(&read, Ok(Some(theirs.clone())), before);
        rt.answered_in(&send, Ok(Some(theirs.clone())), before);
        assert!(rt.known.get(WORLD).is_none_or(|k| k.view.is_none()), "the other server's view is not shown");
        assert!(rt.queue.items.is_empty(), "taken by the server it went to: not sent again");
        // Back to the first environment (A→B→A): an answer from before both switches is still not taken.
        rt.environment_changed();
        rt.answered_in(&read, Ok(Some(theirs.clone())), before);
        assert!(rt.known.get(WORLD).is_none_or(|k| k.view.is_none()));
        // An answer of this environment is.
        rt.answered_in(&read, Ok(Some(theirs)), rt.scope);
        assert_eq!(rt.known[WORLD].view.as_ref().map(|v| v.likes), Some(41));
        let _ = std::fs::remove_file(&rt.queue_path);
    }

    /// VR feedback F6: the development server answered 500 to every read of a world's feedback, and the menus showed it as read for ever, refusing Write a
    /// comment without a word. A read Orbiters did not answer is said; one that works takes it back.
    #[test]
    fn a_read_orbiters_did_not_answer_is_said_and_one_that_works_takes_it_back() {
        let mut rt = WorldFeedbackRuntime {
            queue: Queue::default(),
            queue_path: std::env::temp_dir().join(format!("orbiters-feedback-unread-{}.json", std::process::id())),
            instances: (Vec::new(), Some(Instant::now())),
            ..Default::default()
        };
        rt.asked(WORLD);
        let read = Call::Read { world: WORLD.into() };
        rt.answered(&read, Err(Failure::Later("500".into())));
        let unread = rt.view(None, true).worlds[0].clone();
        assert!(unread.loading, "nothing read");
        assert!(unread.error.is_some(), "and that is said");
        rt.answered(&read, Ok(Some(orbiters_core::world_feedback::ServerView::default())));
        let read_now = rt.view(None, true).worlds[0].clone();
        assert!(!read_now.loading && read_now.error.is_none(), "{read_now:?}");
        let _ = std::fs::remove_file(&rt.queue_path);
    }

    #[test]
    fn only_the_n4n0_world_and_the_development_environment_count() {
        assert!(accepted(Some(contract::N4N0_WORLD), false));
        assert!(!accepted(Some(WORLD), false));
        assert!(accepted(Some(WORLD), true));
        assert!(!accepted(None, true));
    }

    #[test]
    fn the_address_answers_its_path_to_its_own_host_only() {
        assert_eq!(route("GET", "/n4n0/feedback/v1", "127.0.0.1:22541", true), Ok(()));
        assert_eq!(route("GET", "/n4n0/feedback/v1?x=1", "localhost:22541", true), Ok(()));
        assert_eq!(route("GET", "/n4n0/feedback/v1", "evil.example:22541", true), Err((421, "Misdirected Request")));
        assert_eq!(route("POST", "/n4n0/feedback/v1", "127.0.0.1:22541", true), Err((404, "Not Found")));
        assert_eq!(route("GET", "/other", "127.0.0.1:22541", true), Err((404, "Not Found")));
        assert_eq!(route("GET", "/n4n0/feedback/v1", "127.0.0.1:22541", false), Err((404, "Not Found")), "another world learns nothing");
    }

    #[test]
    fn times_read() {
        assert_eq!(rfc3339_seconds("2026-10-29T21:04:00Z"), 1_793_307_840);
        assert_eq!(rfc3339_seconds("2026-10-29T21:04:00.000Z"), 1_793_307_840);
        assert_eq!(rfc3339_seconds("1970-01-01T00:00:00Z"), 0);
    }

    /// VR1115 labels: the bridge's label_world asks what presses of Label it on the worlds page ask, into the same queue
    /// (one coalesced set for the world, sent under the signed-in account), and reads back what the menus show.
    #[test]
    fn a_bridge_label_goes_the_way_of_a_press() {
        use crate::bridge::game::Feedback;
        let ctl = super::super::Controller::new(orbiters_core::settings::Settings::default());
        let queue_path = std::env::temp_dir().join(format!("orbiters-feedback-bridge-{}.json", std::process::id()));
        {
            let mut rt = ctl.world_feedback.lock().unwrap();
            rt.queue = Queue::default();
            rt.queue_path = queue_path.clone();
        }
        let feedback = RealFeedback { ctl: std::sync::Arc::downgrade(&ctl) };
        assert!(feedback.account().is_none(), "a test has no Orbiters account");
        feedback.ask(FeedbackAsk::Label { world: WORLD.to_uppercase().replace("WRLD", "wrld"), label: "mirror".into(), on: true, text: None });
        feedback.ask(FeedbackAsk::Label { world: WORLD.into(), label: "pens".into(), on: true, text: None });
        feedback.ask(FeedbackAsk::Label { world: WORLD.into(), label: "dark".into(), on: false, text: None });
        {
            let rt = ctl.world_feedback.lock().unwrap();
            assert_eq!(rt.queue.items.len(), 1, "one set for the world, as presses make: {:?}", rt.queue.items);
            assert_eq!(rt.queue.items[0].world, WORLD, "by its lowercase id");
            let Action::Set(set) = &rt.queue.items[0].action else { panic!() };
            assert_eq!(
                set.labels.iter().map(|(k, v)| (k.as_str(), *v)).collect::<Vec<_>>(),
                [("dark", false), ("mirror", true), ("pens", true)]
            );
            assert!(Queue::load(&queue_path, now_ms()).items.len() == 1, "kept on disk at once, as a press");
        }
        let known = feedback.feedback(WORLD);
        assert_eq!(known.pending, 1);
        assert!(known.labels.iter().any(|l| l.id == "mirror" && l.mine) && known.labels.iter().any(|l| l.id == "pens" && l.mine), "{known:?}");
        assert!(!known.labels.iter().any(|l| l.id == "dark" && l.mine));
        let _ = std::fs::remove_file(&queue_path);
    }

    #[test]
    fn menus_ask_the_same_actions() {
        let ask = FeedbackAsk::Label { world: WORLD.into(), label: "ai-world".into(), on: true, text: Some("x".into()) };
        let (world, Action::Set(set)) = menu_action(&ask).unwrap() else { panic!() };
        assert_eq!((world.as_str(), set.justification.as_deref()), (WORLD, Some("x")));
        assert!(menu_action(&FeedbackAsk::Show { world: WORLD.into() }).is_none());
        assert!(matches!(menu_action(&FeedbackAsk::Comment { world: WORLD.into(), text: "hi".into() }), Some((_, Action::Comment { .. }))));
    }
}
