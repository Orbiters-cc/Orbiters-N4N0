//! World feedback (contracts `Docs/world-feedback.md`): the Orbiters API for a world's likes, labels and comments
//! (`/maps/worlds/:worldId/feedback`, `/maps/worlds/:worldId/comments`, `/maps/comments/:id`), and the member's actions
//! waiting to be sent: kept on disk while Orbiters cannot be reached, a world's likes and labels coalesced into the
//! latest state, comments by their own id so a retry never posts twice. What the member sees of a world is the server's
//! answer with these applied ([`Known::seen`]).

use crate::account::Api;
use orbiters_contracts::world_feedback::{self as contract, Refusal};
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A world's likes and labels go this long after the member's last change of them (the next change waits along).
pub const COALESCE: Duration = Duration::from_secs(2);
/// Waits between tries: 2, 4, 8 … seconds, at most a minute.
pub const RETRY_MAX: Duration = Duration::from_secs(60);
/// Actions not sent after this long are dropped.
pub const DROP_AFTER: Duration = Duration::from_secs(24 * 3600);

/// A world's feedback as Orbiters answers it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerView {
    pub world_id: String,
    pub likes: u32,
    #[serde(default)]
    pub labels: Vec<LabelCount>,
    #[serde(default)]
    pub justifications: Vec<Justification>,
    #[serde(default)]
    pub comments: Comments,
    #[serde(default)]
    pub mine: Option<Mine>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelCount {
    pub id: String,
    pub count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Justification {
    pub id: u64,
    pub label: String,
    #[serde(default)]
    pub by: Option<String>,
    pub text: String,
    #[serde(default)]
    pub at: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comments {
    pub count: u32,
    #[serde(default)]
    pub items: Vec<Comment>,
    #[serde(default)]
    pub next: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: u64,
    #[serde(default)]
    pub by: Option<String>,
    #[serde(default)]
    pub by_id: Option<u64>,
    pub text: String,
    /// RFC 3339.
    #[serde(default)]
    pub at: Option<String>,
    /// Hidden by moderators (shown to its author and to moderators only).
    #[serde(default)]
    pub hidden: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mine {
    pub liked: bool,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub justification: Option<String>,
    #[serde(default)]
    pub comment_ids: Vec<u64>,
}

/// Why a call did not go through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Orbiters refused it: the contract's code (when it gave one) and its sentence. Sending it again changes nothing.
    Refused { code: Option<Refusal>, message: String },
    /// The credential no longer works.
    SignedOut,
    /// Not now (unreachable, busy, too many at once): try again later.
    Later(String),
}

impl Failure {
    pub fn message(&self) -> String {
        match self {
            Failure::Refused { message, .. } => message.clone(),
            Failure::SignedOut => Refusal::SignIn.text().into(),
            Failure::Later(m) => m.clone(),
        }
    }
}

/// A world id as it may go into the API's path (VRChat's `wrld_` and a UUID; capitals read as VRChat's lowercase).
fn world_path(world: &str) -> Result<String, Failure> {
    let world = world.to_ascii_lowercase();
    if crate::vrchat_log::is_world_id(&world) {
        Ok(world)
    } else {
        Err(Failure::Refused { code: None, message: tr!("Not a VRChat world: {world}", world = format!("{world:?}")) })
    }
}

fn failure(status: u16, body: &str) -> Failure {
    let json: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let message = json.get("error").and_then(|e| e.as_str()).unwrap_or("").to_string();
    let code = json.get("code").and_then(|c| c.as_str()).and_then(Refusal::of_code);
    match status {
        401 => Failure::SignedOut,
        429 => Failure::Later(if message.is_empty() { Refusal::RateLimited.text().into() } else { message }),
        400..=499 => Failure::Refused { code, message: if message.is_empty() { tr!("Orbiters refused it ({status})", status = status) } else { message } },
        _ => Failure::Later(if message.is_empty() { tr!("Orbiters could not answer ({status})", status = status) } else { message }),
    }
}

/// Calls the feedback API with the member's credential (none: reading only).
pub struct Client<'a> {
    pub api: &'a Api,
    pub token: Option<&'a str>,
}

impl Client<'_> {
    fn call(&self, method: &str, path: &str, body: Option<serde_json::Value>) -> Result<(u16, String), Failure> {
        let url = self.api.url(path);
        let agent = self.api.agent();
        let auth = self.token.map(|t| format!("Bearer {t}")); // i18n: not shown (an HTTP header)
        let result = match (method, body) {
            ("GET", _) => {
                let mut r = agent.get(&url);
                if let Some(a) = &auth {
                    r = r.header("Authorization", a);
                }
                r.call()
            }
            ("DELETE", _) => {
                let mut r = agent.delete(&url);
                if let Some(a) = &auth {
                    r = r.header("Authorization", a);
                }
                r.call()
            }
            (method, body) => {
                let mut r = if method == "PUT" { agent.put(&url) } else { agent.post(&url) };
                if let Some(a) = &auth {
                    r = r.header("Authorization", a);
                }
                r.send_json(body.unwrap_or(serde_json::json!({})))
            }
        };
        let response = result.map_err(|e| Failure::Later(tr!("Orbiters could not be reached: {error}", error = e)))?;
        let status = response.status().as_u16();
        let text = response.into_body().with_config().limit(4 * 1024 * 1024).read_to_string().unwrap_or_default();
        if (200..300).contains(&status) {
            Ok((status, text))
        } else {
            Err(failure(status, &text))
        }
    }

    fn json<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, Failure> {
        serde_json::from_str(text).map_err(|e| Failure::Later(tr!("unexpected answer from Orbiters: {error}", error = e)))
    }

    /// A world's feedback (the member's own too when signed in).
    pub fn read(&self, world: &str) -> Result<ServerView, Failure> {
        let world = world_path(world)?;
        let (_, text) = self.call("GET", &format!("maps/worlds/{world}/feedback"), None)?;
        Self::json(&text)
    }

    /// Sets the member's like and labels as `set` says: the world's feedback as Orbiters answers it, `None` when it was
    /// done but its answer does not read (a newer shape: read the world again rather than set it again).
    pub fn set(&self, world: &str, set: &Set) -> Result<Option<ServerView>, Failure> {
        let world = world_path(world)?;
        let (_, text) = self.call("PUT", &format!("maps/worlds/{world}/feedback"), Some(set.body()))?;
        Ok(Self::json(&text).ok())
    }

    /// Writes a comment (`client_id`: its own id, so a retry answers the one made); `None` when it was written but its
    /// answer does not read.
    pub fn comment(&self, world: &str, text: &str, client_id: &str) -> Result<Option<Comment>, Failure> {
        let world = world_path(world)?;
        let body = serde_json::json!({ "text": text, "clientId": client_id });
        let (_, answer) = self.call("POST", &format!("maps/worlds/{world}/comments"), Some(body))?;
        #[derive(Deserialize)]
        struct Made {
            comment: Comment,
        }
        Ok(Self::json::<Made>(&answer).ok().map(|m| m.comment))
    }

    pub fn uncomment(&self, comment: u64) -> Result<(), Failure> {
        match self.call("DELETE", &format!("maps/comments/{comment}"), None) {
            // Gone already: what was asked.
            Err(Failure::Refused { code: Some(Refusal::NotFound), .. }) => Ok(()),
            other => other.map(|_| ()),
        }
    }

    pub fn report(&self, comment: u64, reason: Option<&str>) -> Result<(), Failure> {
        let body = serde_json::json!({ "reason": reason.unwrap_or("") });
        self.call("POST", &format!("maps/comments/{comment}/report"), Some(body)).map(|_| ())
    }
}

/// The member's like and labels to set for a world (what is not named stays as it is).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Set {
    #[serde(default)]
    pub liked: Option<bool>,
    #[serde(default)]
    pub labels: BTreeMap<String, bool>,
    #[serde(default)]
    pub justification: Option<String>,
}

impl Set {
    fn body(&self) -> serde_json::Value {
        let mut body = serde_json::json!({ "labels": self.labels });
        if let Some(liked) = self.liked {
            body["liked"] = liked.into();
        }
        if let Some(text) = &self.justification {
            body["justification"] = text.clone().into();
        }
        body
    }

    /// `later` on top of this one: the latest value of each.
    pub fn merge(&mut self, later: &Set) {
        if later.liked.is_some() {
            self.liked = later.liked;
        }
        for (label, on) in &later.labels {
            self.labels.insert(label.clone(), *on);
        }
        if later.justification.is_some() {
            self.justification = later.justification.clone();
        }
    }
}

/// What waits to be sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    Set(Set),
    Comment { text: String, client_id: String },
    Uncomment { comment: u64 },
    Report { comment: u64, reason: Option<String> },
}

/// An action waiting, for a world, since `since` (the last change it holds), tried `tries` times, next at `due` (Unix milliseconds).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub world: String,
    pub action: Action,
    pub since: u64,
    pub due: u64,
    #[serde(default)]
    pub tries: u32,
    /// Where it goes: the server and the account it was made for. Comment ids and test comments mean something on one
    /// server only, and an action is the member's own: it is sent there and nowhere else. One without (written by an
    /// earlier version) is dropped at load.
    #[serde(default)]
    pub to: Option<Stamp>,
    /// This action's own number (a new one each time a waiting Set takes a later change): an answer is for the item it
    /// was sent as, never another made for another account in the same millisecond, nor a Set changed while on its way.
    #[serde(default)]
    pub op: u64,
}

/// A new [`Pending::op`].
fn next_op() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let clock = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    (clock ^ ((std::process::id() as u64) << 40)).wrapping_add(N.fetch_add(1, Ordering::Relaxed)).max(1)
}

/// The server (its API address) and the Orbiters account an action was made for: its id, and a digest of its credential
/// (an account file may name no id until the account check answers). Neither known (nobody signed in, or the account of
/// the environment just chosen not read yet): it is never sent, by anyone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub server: String,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

impl Stamp {
    /// Whether an action made for `self` may go out as `now`: the same server, and the same account by its id or by its
    /// credential; an action made for nobody goes as nobody.
    pub fn sends_as(&self, now: &Stamp) -> bool {
        self.server == now.server
            && ((self.account.is_some() && self.account == now.account) || (self.credential.is_some() && self.credential == now.credential))
    }

    /// Whether an action made for `self` shows as waiting to `now`: one that may go as `now`, or one made for exactly
    /// `now` (a press while nobody is signed in shows as waiting there, and is still never sent as anybody).
    pub fn shows_as(&self, now: &Stamp) -> bool {
        self == now || self.sends_as(now)
    }
}

/// The actions waiting, oldest first, kept in a file of their own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queue {
    pub items: Vec<Pending>,
}

impl Queue {
    /// The queue's file in the app's folder.
    pub fn file() -> PathBuf {
        crate::paths::config_dir().join("world-feedback-queue.json")
    }

    /// The queue kept in `path` (none when there is no readable one), its actions older than [`DROP_AFTER`] left out.
    /// Each action is read on its own: one that does not read (a file cut short, a newer version's) or names no world
    /// is left out and the others kept, the file first kept beside as `world-feedback-queue.json.bad-<time>`. A file
    /// that cannot be read at all is not saved over unseen ([`crate::write_atomic`]).
    pub fn load(path: &Path, now_ms: u64) -> Self {
        let text = match crate::read_text(path) {
            Ok(Some(text)) => text,
            Ok(None) => return Self::default(),
            Err(e) => {
                eprintln!("world feedback queue: {}: {e}", path.display());
                return Self::default();
            }
        };
        let value: Option<serde_json::Value> = serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text)).ok();
        let items = value.as_ref().and_then(|v| v.get("items")).and_then(serde_json::Value::as_array);
        let mut queue = Queue::default();
        let mut lost = items.is_none();
    let mut unstamped = Vec::new();
        for item in items.into_iter().flatten() {
            match serde_json::from_value::<Pending>(item.clone()) {
                Ok(mut p) => {
                    // The contract takes a world id in capitals too; the queue keeps VRChat's own lowercase.
                    p.world = p.world.to_ascii_lowercase();
                    if p.op == 0 {
                        p.op = next_op();
                    }
                    if p.to.is_none() {
                        // From before actions said where they go (nothing in the file tells which server or account):
                        // never sent where it may not be for, and never deleted: kept beside, unsent (below).
                        unstamped.push(p);
                    } else if crate::vrchat_log::is_world_id(&p.world) {
                        queue.items.push(p);
                    } else {
                        lost = true;
                    }
                }
                Err(_) => lost = true,
            }
        }
        if lost || !unstamped.is_empty() {
            let aside = crate::keep_aside(path);
            eprintln!(
                "world feedback queue: {} partly unreadable or from before actions said where they go ({} such), the file kept as {aside:?}",
                path.display(),
                unstamped.len()
            );
            // Not kept beside: they stay in the queue, unsent and kept (never handed out, never aged out).
            if aside.is_none() {
                queue.items.extend(unstamped);
            }
        }
        queue.items.retain(|p| p.to.is_none() || now_ms.saturating_sub(p.since) < DROP_AFTER.as_millis() as u64);
        queue
    }

    /// Writes it whole ([`crate::write_atomic`]: a crash or a power cut leaves the old queue or the new one).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::write_atomic(path, &serde_json::to_vec(self)?)
    }

    /// Adds an action: a world's likes and labels go into the one waiting for it (sent [`COALESCE`] after this
    /// change); a comment deleted before it went is never sent.
    pub fn push(&mut self, world: &str, action: Action, now_ms: u64) {
        self.push_to(world, action, now_ms, Stamp::default());
    }

    /// [`Queue::push`] for the server and account `to`: a world's likes and labels go together only with those made
    /// for the same.
    pub fn push_to(&mut self, world: &str, action: Action, now_ms: u64, to: Stamp) {
        let due = now_ms + COALESCE.as_millis() as u64;
        match action {
            Action::Set(set) => {
                if let Some(p) = self.items.iter_mut().find(|p| p.world == world && matches!(p.action, Action::Set(_)) && p.to.as_ref() == Some(&to)) {
                    if let Action::Set(waiting) = &mut p.action {
                        waiting.merge(&set);
                    }
                    p.due = p.due.max(due);
                    // It waits since this change: an old one it goes with (offline a day) does not take it along.
                    p.since = now_ms;
                    // Another number: an answer to the values sent before does not take these later ones away.
                    p.op = next_op();
                    return;
                }
                self.items.push(Pending { world: world.into(), action: Action::Set(set), since: now_ms, due, tries: 0, to: Some(to), op: next_op() });
            }
            other => self.items.push(Pending { world: world.into(), action: other, since: now_ms, due: now_ms, tries: 0, to: Some(to), op: next_op() }),
        }
    }

    /// The first action due at `now_ms`, if any (its index).
    pub fn due(&self, now_ms: u64) -> Option<usize> {
        self.items.iter().position(|p| p.due <= now_ms)
    }

    /// The first action due at `now_ms` that may go out as `now` ([`Stamp::sends_as`]); the others wait for their
    /// server and account (and leave after [`DROP_AFTER`]).
    pub fn due_for(&self, now_ms: u64, now: &Stamp) -> Option<usize> {
        self.items.iter().position(|p| p.due <= now_ms && p.to.as_ref().is_some_and(|to| to.sends_as(now)))
    }

    /// Drops the actions older than [`DROP_AFTER`], wherever they were for.
    pub fn expire(&mut self, now_ms: u64) {
        self.items.retain(|p| p.to.is_none() || now_ms.saturating_sub(p.since) < DROP_AFTER.as_millis() as u64);
    }

    /// The actions that may go out as `now` ([`Stamp::sends_as`]): what this server and account show as waiting.
    pub fn for_stamp<'a>(&'a self, now: &'a Stamp) -> impl Iterator<Item = &'a Pending> {
        self.items.iter().filter(|p| p.to.as_ref().is_some_and(|to| to.shows_as(now)))
    }

    /// The action at `i` failed for now: tried again after 2, 4, 8 … seconds (at most a minute), dropped after a day.
    pub fn retry(&mut self, i: usize, now_ms: u64) {
        let Some(p) = self.items.get_mut(i) else { return };
        p.tries += 1;
        let wait = (2_000u64 << p.tries.saturating_sub(1).min(5)).min(RETRY_MAX.as_millis() as u64);
        p.due = now_ms + wait;
        if now_ms.saturating_sub(p.since) > DROP_AFTER.as_millis() as u64 {
            self.items.remove(i);
        }
    }
}

/// A world's feedback as known here: Orbiters' last answer, and when it came (Unix milliseconds).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Known {
    pub view: Option<ServerView>,
    pub read_at: u64,
}

/// What the member sees of a world: the server's view with the actions waiting applied.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    pub loading: bool,
    pub likes: u32,
    pub liked: bool,
    /// Every label counted or given by the member, highest count first, then the list's order: (id, count, mine).
    pub labels: Vec<(String, u32, bool)>,
    pub justification: Option<String>,
    /// Newest first: (id, by, text, RFC 3339 time, mine); id 0 for a comment on its way.
    pub comments: Vec<(u64, String, String, Option<String>, bool)>,
    pub comment_count: u32,
    pub pending: u32,
}

impl Known {
    /// The server's view with the actions `waiting` for `world` applied (those of other worlds are passed over).
    pub fn seen<'a>(&self, world: &str, waiting: impl Iterator<Item = &'a Pending>, me: &str) -> Seen {
        let view = self.view.clone().unwrap_or_default();
        let mine = view.mine.clone().unwrap_or_default();
        let mut seen = Seen {
            loading: self.view.is_none(),
            likes: view.likes,
            liked: mine.liked,
            justification: mine.justification.clone(),
            comment_count: view.comments.count,
            ..Default::default()
        };
        let mut labels: Vec<(String, u32, bool)> = view.labels.iter().map(|l| (l.id.clone(), l.count, mine.labels.contains(&l.id))).collect();
        for id in &mine.labels {
            if !labels.iter().any(|l| &l.0 == id) {
                labels.push((id.clone(), 1, true));
            }
        }
        seen.comments = view
            .comments
            .items
            .iter()
            .filter(|c| c.hidden.is_none())
            .map(|c| (c.id, c.by.clone().unwrap_or_default(), c.text.clone(), c.at.clone(), mine.comment_ids.contains(&c.id)))
            .collect();
        let mut newer = Vec::new();
        for p in waiting.filter(|p| p.world == world) {
            seen.pending += 1;
            match &p.action {
                Action::Set(set) => {
                    if let Some(on) = set.liked {
                        if on != seen.liked {
                            seen.likes = if on { seen.likes + 1 } else { seen.likes.saturating_sub(1) };
                            seen.liked = on;
                        }
                    }
                    for (id, on) in &set.labels {
                        match labels.iter_mut().find(|l| &l.0 == id) {
                            Some(l) if l.2 != *on => {
                                l.2 = *on;
                                l.1 = if *on { l.1 + 1 } else { l.1.saturating_sub(1) };
                            }
                            Some(_) => {}
                            None if *on => labels.push((id.clone(), 1, true)),
                            None => {}
                        }
                    }
                    if set.justification.is_some() {
                        seen.justification = set.justification.clone();
                    }
                }
                Action::Comment { text, .. } => {
                    newer.push((0, me.to_string(), text.clone(), None, true));
                    seen.comment_count += 1;
                }
                Action::Uncomment { comment } => {
                    if let Some(at) = seen.comments.iter().position(|c| c.0 == *comment) {
                        seen.comments.remove(at);
                        seen.comment_count = seen.comment_count.saturating_sub(1);
                    }
                }
                Action::Report { .. } => seen.pending -= 1,
            }
        }
        newer.reverse();
        newer.extend(seen.comments);
        seen.comments = newer;
        let order = |id: &str| contract::labels().iter().position(|l| l.id == id).unwrap_or(usize::MAX);
        labels.retain(|l| l.1 > 0 || l.2);
        labels.sort_by(|a, b| b.1.cmp(&a.1).then(order(&a.0).cmp(&order(&b.0))));
        seen.labels = labels;
        seen
    }
}

/// A new id for a comment (8 lowercase hex characters from the clock and the process, enough for one member).
pub fn client_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let mixed = nanos ^ ((std::process::id() as u64) << 32) ^ (N.fetch_add(1, Ordering::Relaxed) as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    format!("n4n0-{:016x}", mixed.wrapping_mul(0xBF58_476D_1CE4_E5B9))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// COR-5: a change made just before an old one it goes with would be dropped is sent all the same, its own day ahead of it;
    /// the server's wait it was under stays.
    #[test]
    fn a_fresh_change_is_not_dropped_with_an_old_one() {
        let day = DROP_AFTER.as_millis() as u64;
        let mut queue = Queue::default();
        queue.push(WORLD, Action::Set(Set { liked: Some(true), ..Default::default() }), 0);
        queue.items[0].due = day + 30_000;
        queue.push(WORLD, Action::Set(Set { liked: Some(false), ..Default::default() }), day - 500);
        queue.expire(day + 2_000);
        assert_eq!(queue.items.len(), 1, "the new change is there");
        assert_eq!(queue.items[0].due, day + 30_000);
        queue.retry(0, day + 40_000);
        assert_eq!(queue.items.len(), 1);
        queue.expire(2 * day - 500);
        assert!(queue.items.is_empty(), "a day after it");
    }

    /// VR1115 labels, a real check against the development server on this computer, never production: it takes a label
    /// from the Orbiters account saved for it (read only), shows it as the member's own, and takes it back; an id outside the
    /// list is refused. `cargo test -p orbiters-core --lib -- --ignored live_dev_server --nocapture`.
    #[test]
    #[ignore = "talks to the development server on 127.0.0.1:4100 with the account saved for it"]
    fn live_dev_server_takes_a_label_and_gives_it_back() {
        let server = crate::account::Server::Development;
        assert_eq!(server.api(), "http://127.0.0.1:4100/", "the development server only");
        let account = crate::account::read(server).expect("an Orbiters account saved for the development server");
        let api = crate::account::Api::new(server);
        let client = Client { api: &api, token: Some(&account.token) };
        // Silent Beach, one of VRChat's popular worlds (find_worlds popular, page 20, 2026-11-15).
        let world = "wrld_99a5eb21-1d58-4f13-b66a-73d327c42705";
        let mine = |view: &ServerView| view.mine.as_ref().is_some_and(|m| m.labels.iter().any(|l| l == "chill"));
        let count = |view: &ServerView| view.labels.iter().find(|l| l.id == "chill").map_or(0, |l| l.count);
        let before = client.read(world).expect("read");
        let had = mine(&before);
        println!("account {:?} (id {:?}) on {}: chill {} before, count {}", account.username, account.user, server.api(), had, count(&before));
        let set = |on: bool| Set { labels: [("chill".to_string(), on)].into_iter().collect(), ..Default::default() };
        let changed = client.set(world, &set(!had)).expect("set").expect("its answer reads");
        println!("after: mine {}, count {}", mine(&changed), count(&changed));
        assert_eq!(mine(&changed), !had);
        let back = client.set(world, &set(had)).expect("set back").expect("its answer reads");
        println!("back: mine {}, count {}", mine(&back), count(&back));
        assert_eq!((mine(&back), count(&back)), (had, count(&before)), "as it was");
        let unknown = Set { labels: [("beautiful".to_string(), true)].into_iter().collect(), ..Default::default() };
        let refused = client.set(world, &unknown);
        println!("unknown label: {refused:?}");
        assert!(matches!(refused, Err(Failure::Refused { code: Some(Refusal::UnknownLabel), .. })), "{refused:?}");
    }

    fn set(liked: Option<bool>, labels: &[(&str, bool)]) -> Action {
        Action::Set(Set { liked, labels: labels.iter().map(|(l, on)| (l.to_string(), *on)).collect(), justification: None })
    }

    #[test]
    fn a_worlds_changes_go_together_with_their_latest_values() {
        let mut q = Queue::default();
        q.push("w", set(Some(true), &[("meetup", true)]), 1000);
        q.push("w", set(Some(false), &[("mirror", true), ("meetup", false)]), 1500);
        q.push("x", set(Some(true), &[]), 1500);
        assert_eq!(q.items.len(), 2);
        let Action::Set(s) = &q.items[0].action else { panic!() };
        assert_eq!(s.liked, Some(false));
        assert_eq!(s.labels.get("meetup"), Some(&false));
        assert_eq!(s.labels.get("mirror"), Some(&true));
        assert_eq!(q.items[0].due, 3500, "2 s after the last change");
        assert_eq!(q.due(3000), None);
        assert_eq!(q.due(3600), Some(0));
        q.push("w", Action::Comment { text: "hi".into(), client_id: "n4n0-1".into() }, 4000);
        assert_eq!(q.due(4000), Some(0));
    }

    #[test]
    fn retries_wait_longer_each_time_and_end_after_a_day() {
        let mut q = Queue::default();
        q.push("w", Action::Uncomment { comment: 3 }, 0);
        let waits: Vec<u64> = (0..7)
            .map(|_| {
                q.retry(0, 10_000);
                q.items[0].due - 10_000
            })
            .collect();
        assert_eq!(waits, [2000, 4000, 8000, 16000, 32000, 60000, 60000]);
        q.retry(0, DROP_AFTER.as_millis() as u64 + 1);
        assert!(q.items.is_empty());
        let dir = std::env::temp_dir().join(format!("orbiters-feedback-queue-{}", std::process::id()));
        let path = dir.join("q.json");
        let mut q = Queue::default();
        q.push(WORLD, set(Some(true), &[]), 5);
        q.save(&path).unwrap();
        assert_eq!(Queue::load(&path, 10).items.len(), 1);
        assert!(Queue::load(&path, DROP_AFTER.as_millis() as u64 + 10).items.is_empty(), "a day old: dropped");
        let _ = std::fs::remove_dir_all(dir);
    }

    const WORLD: &str = "wrld_99a5eb21-1d58-4f13-b66a-73d327c42705";

    /// One action that does not read, or that names no world, costs only itself: the others are kept as they were, and
    /// the file is kept beside before the next save replaces it.
    #[test]
    fn a_damaged_queue_keeps_what_reads_and_the_file() {
        let dir = std::env::temp_dir().join(format!("orbiters-feedback-damaged-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("q.json");
        let mut q = Queue::default();
        q.push(WORLD, set(Some(true), &[("mirror", true)]), 5);
        q.push(&WORLD.to_uppercase().replace("WRLD", "wrld"), Action::Comment { text: "hi".into(), client_id: "n4n0-1".into() }, 6);
        q.push("wrld_../../maps/comments/1", Action::Uncomment { comment: 1 }, 7);
        let mut value = serde_json::to_value(&q).unwrap();
        value["items"].as_array_mut().unwrap().push(serde_json::json!({"world": WORLD, "action": {"kind": "from_a_newer_version"}, "since": 8, "due": 8}));
        std::fs::write(&path, value.to_string()).unwrap();
        let loaded = Queue::load(&path, 10);
        assert_eq!(loaded.items.len(), 2, "{:?}", loaded.items);
        assert_eq!((loaded.items[0].clone(), loaded.items[1].world.as_str(), loaded.items[1].due), (q.items[0].clone(), WORLD, 6));
        let kept = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("q.json.bad-")).count();
        assert_eq!(kept, 1, "the file kept beside");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// CORE2-1: an action goes to the server and account it was made for, never to another; one from before actions
    /// said where they go is dropped, not sent.
    #[test]
    fn an_action_goes_only_where_it_was_made() {
        let dev = Stamp { server: "http://127.0.0.1:4100/".into(), account: Some("7".into()), credential: None };
        let prod = Stamp { server: "https://api.orbiters.cc/".into(), account: Some("7".into()), credential: None };
        let other = Stamp { account: Some("8".into()), ..prod.clone() };
        let mut q = Queue::default();
        q.push_to(WORLD, Action::Uncomment { comment: 5 }, 0, dev.clone());
        q.push_to(WORLD, set(Some(true), &[]), 0, dev.clone());
        q.push_to(WORLD, set(Some(false), &[]), 0, prod.clone());
        assert_eq!(q.items.len(), 3, "a like of each server waits apart");
        let due = |q: &Queue, s: &Stamp| (0..q.items.len()).filter(|&i| q.items[i].to.as_ref().unwrap().sends_as(s)).collect::<Vec<_>>();
        assert_eq!(due(&q, &prod), [2]);
        assert_eq!(q.due_for(10_000, &prod), Some(2));
        assert_eq!(q.due_for(10_000, &other), None, "nor another account's");
        q.items.remove(2);
        assert_eq!(q.due_for(10_000, &prod), None, "Development's delete never goes to production");
        assert_eq!(q.due_for(10_000, &dev), Some(0), "it goes back home");
        // Made while nobody was known (signed out, or the account of a just-chosen environment not read yet): it goes as
        // nobody, never as the first account to come (night4, verify2).
        let nobody = Stamp { account: None, ..prod.clone() };
        q.push_to(WORLD, Action::Report { comment: 3, reason: None }, 0, nobody.clone());
        assert!(q.due_for(10_000, &other).is_none() && q.due_for(10_000, &prod).is_none() && q.due_for(10_000, &nobody).is_none());
        // An account file without an id: known by its credential, still the same account once the check gives the id.
        let by_credential = Stamp { account: None, credential: Some("c1".into()), ..prod.clone() };
        q.push_to(WORLD, Action::Report { comment: 4, reason: None }, 0, by_credential);
        let checked = Stamp { account: Some("9".into()), credential: Some("c1".into()), ..prod.clone() };
        assert_eq!(q.due_for(10_000, &checked).map(|i| &q.items[i].action), Some(&Action::Report { comment: 4, reason: None }));
        let elsewhere = Stamp { account: Some("10".into()), credential: Some("c2".into()), ..prod.clone() };
        assert!(q.due_for(10_000, &elsewhere).is_none(), "another account, another credential: nothing");
        q.expire(DROP_AFTER.as_millis() as u64 + 1);
        assert!(q.items.is_empty(), "a day old: gone wherever it was for");
        // An earlier version's file: its actions say nowhere, and go nowhere, but are never deleted: the file is kept
        // beside as it was (verifier, CORE2-1).
        let dir = std::env::temp_dir().join(format!("orbiters-feedback-unstamped-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("q.json");
        let old = serde_json::json!({"items": [{"world": WORLD, "action": {"kind": "comment", "text": "hi", "client_id": "n4n0-1"}, "since": 1, "due": 1}]}).to_string();
        std::fs::write(&path, &old).unwrap();
        let loaded = Queue::load(&path, 2);
        assert!(loaded.due_for(10, &prod).is_none() && loaded.due_for(10, &dev).is_none(), "sent nowhere");
        let kept: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("q.json.bad-")).collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read_to_string(kept[0].path()).unwrap(), old, "kept beside, unsent");
        let _ = std::fs::remove_dir_all(&dir);
        // What waits for another account is not shown as this one's.
        let mut q = Queue::default();
        q.push_to(WORLD, Action::Comment { text: "A's draft".into(), client_id: "n4n0-a".into() }, 0, prod.clone());
        assert_eq!(q.for_stamp(&prod).count(), 1);
        assert!(q.for_stamp(&other).next().is_none(), "B never sees A's waiting comment");
    }

    #[test]
    fn only_world_ids_go_into_the_apis_paths() {
        assert_eq!(world_path(&WORLD.to_uppercase().replace("WRLD", "wrld")).unwrap(), WORLD);
        for bad in ["", "w", "wrld_../../maps/comments/1", "wrld_99a5eb21-1d58-4f13-b66a-73d327c42705/x"] {
            assert!(matches!(world_path(bad), Err(Failure::Refused { code: None, .. })), "{bad}");
        }
    }

    #[test]
    fn what_the_member_sees_has_what_is_waiting() {
        let view = ServerView {
            world_id: "w".into(),
            likes: 4,
            labels: vec![LabelCount { id: "mirror".into(), count: 3 }, LabelCount { id: "meetup".into(), count: 3 }],
            comments: Comments {
                count: 2,
                items: vec![
                    Comment { id: 9, by: Some("Me".into()), text: "mine".into(), ..Default::default() },
                    Comment { id: 8, by: Some("Kana".into()), text: "theirs".into(), ..Default::default() },
                ],
                next: None,
            },
            mine: Some(Mine { liked: false, labels: vec!["mirror".into()], justification: None, comment_ids: vec![9] }),
            ..Default::default()
        };
        let known = Known { view: Some(view), read_at: 1 };
        let mut q = Queue::default();
        q.push("w", set(Some(true), &[("mirror", false), ("pens", true)]), 0);
        q.push("w", Action::Comment { text: "new".into(), client_id: "c".into() }, 0);
        q.push("w", Action::Uncomment { comment: 9 }, 0);
        q.push("other", set(Some(true), &[]), 0);
        let seen = known.seen("w", q.items.iter(), "You");
        assert_eq!((seen.likes, seen.liked, seen.pending), (5, true, 3));
        assert_eq!(seen.labels, vec![("meetup".to_string(), 3, false), ("mirror".to_string(), 2, false), ("pens".to_string(), 1, true)]);
        assert_eq!(seen.comment_count, 2);
        assert_eq!(seen.comments.iter().map(|c| (c.0, c.2.as_str(), c.4)).collect::<Vec<_>>(), [(0, "new", true), (8, "theirs", false)]);
        assert!(Known::default().seen("w", std::iter::empty(), "You").loading);
    }

    #[test]
    fn refusals_carry_the_contracts_code() {
        assert_eq!(
            failure(400, r#"{"error":"Say why in at least 10 words.","code":"justification-too-short"}"#),
            Failure::Refused { code: Some(Refusal::JustificationTooShort), message: "Say why in at least 10 words.".into() }
        );
        assert_eq!(failure(401, "{}"), Failure::SignedOut);
        assert!(matches!(failure(429, r#"{"error":"Too many attempts."}"#), Failure::Later(_)));
        assert!(matches!(failure(503, ""), Failure::Later(_)));
        assert!(client_id().len() >= 8 && client_id() != client_id());
    }

    /// Against a disposable server: `ORBITERS_TEST_API=http://127.0.0.1:4230 ORBITERS_TEST_TOKEN=<a member's token>
    /// cargo test -p orbiters-core world_feedback::tests::against_a_server -- --ignored`.
    #[test]
    #[ignore]
    fn against_a_server() {
        let api = Api::at(&std::env::var("ORBITERS_TEST_API").expect("ORBITERS_TEST_API"));
        let token = std::env::var("ORBITERS_TEST_TOKEN").expect("ORBITERS_TEST_TOKEN");
        let world = format!(
            "wrld_{:08x}-0000-4000-8000-{:012x}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
        );
        let anyone = Client { api: &api, token: None };
        let member = Client { api: &api, token: Some(&token) };
        let empty = anyone.read(&world).unwrap();
        assert_eq!((empty.likes, empty.mine.clone()), (0, None));
        let set = Set { liked: Some(true), labels: [("meetup".to_string(), true), ("mirror".to_string(), true)].into_iter().collect(), justification: None };
        let view = member.set(&world, &set).unwrap().expect("its answer reads");
        assert_eq!(view.likes, 1);
        assert_eq!(view.mine.as_ref().map(|m| m.liked), Some(true));
        assert_eq!(view.labels.len(), 2);
        let refused = member.set(
            &world,
            &Set { labels: [("ai-world".to_string(), true)].into_iter().collect(), justification: Some("too short".into()), ..Default::default() },
        );
        assert!(matches!(refused, Err(Failure::Refused { code: Some(Refusal::JustificationTooShort), .. })), "{refused:?}");
        let id = client_id();
        let first = member.comment(&world, "Great place for meetups!", &id).unwrap().expect("its answer reads");
        let again = member.comment(&world, "Great place for meetups!", &id);
        // A retry inside the server's 20 s window is held back (429, tried later) or answers the same comment.
        match again {
            Ok(c) => assert_eq!(c.map(|c| c.id), Some(first.id)),
            Err(e) => assert!(matches!(e, Failure::Later(_)), "{e:?}"),
        }
        let read = member.read(&world).unwrap();
        assert_eq!(read.comments.count, 1);
        assert_eq!(read.mine.as_ref().map(|m| m.comment_ids.clone()), Some(vec![first.id]));
        member.uncomment(first.id).unwrap();
        member.uncomment(first.id).unwrap();
        assert_eq!(anyone.read(&world).unwrap().comments.count, 0);
        assert_eq!(Client { api: &api, token: Some("orbit-not-a-token") }.set(&world, &set), Err(Failure::SignedOut));
    }
}
