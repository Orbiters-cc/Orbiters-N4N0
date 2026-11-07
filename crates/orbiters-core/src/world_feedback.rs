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
        let (_, text) = self.call("GET", &format!("maps/worlds/{world}/feedback"), None)?;
        Self::json(&text)
    }

    /// Sets the member's like and labels as `set` says.
    pub fn set(&self, world: &str, set: &Set) -> Result<ServerView, Failure> {
        let (_, text) = self.call("PUT", &format!("maps/worlds/{world}/feedback"), Some(set.body()))?;
        Self::json(&text)
    }

    /// Writes a comment (`client_id`: its own id, so a retry answers the one made).
    pub fn comment(&self, world: &str, text: &str, client_id: &str) -> Result<Comment, Failure> {
        let body = serde_json::json!({ "text": text, "clientId": client_id });
        let (_, answer) = self.call("POST", &format!("maps/worlds/{world}/comments"), Some(body))?;
        #[derive(Deserialize)]
        struct Made {
            comment: Comment,
        }
        Ok(Self::json::<Made>(&answer)?.comment)
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

/// An action waiting, for a world, since `since`, tried `tries` times, next at `due` (Unix milliseconds).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub world: String,
    pub action: Action,
    pub since: u64,
    pub due: u64,
    #[serde(default)]
    pub tries: u32,
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
    pub fn load(path: &Path, now_ms: u64) -> Self {
        let mut queue: Queue = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        queue.items.retain(|p| now_ms.saturating_sub(p.since) < DROP_AFTER.as_millis() as u64);
        queue
    }

    /// Writes it whole (a new file moved over the old one).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self).unwrap_or_default())?;
        std::fs::rename(&tmp, path)
    }

    /// Adds an action: a world's likes and labels go into the one waiting for it (sent [`COALESCE`] after this
    /// change); a comment deleted before it went is never sent.
    pub fn push(&mut self, world: &str, action: Action, now_ms: u64) {
        let due = now_ms + COALESCE.as_millis() as u64;
        match action {
            Action::Set(set) => {
                if let Some(p) = self.items.iter_mut().find(|p| p.world == world && matches!(p.action, Action::Set(_))) {
                    if let Action::Set(waiting) = &mut p.action {
                        waiting.merge(&set);
                    }
                    p.due = p.due.max(due);
                    return;
                }
                self.items.push(Pending { world: world.into(), action: Action::Set(set), since: now_ms, due, tries: 0 });
            }
            other => self.items.push(Pending { world: world.into(), action: other, since: now_ms, due: now_ms, tries: 0 }),
        }
    }

    /// The first action due at `now_ms`, if any (its index).
    pub fn due(&self, now_ms: u64) -> Option<usize> {
        self.items.iter().position(|p| p.due <= now_ms)
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

    pub fn of(&self, world: &str) -> impl Iterator<Item = &Pending> + '_ {
        let world = world.to_string();
        self.items.iter().filter(move |p| p.world == world)
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
    /// The server's view with `queue`'s actions for `world` applied.
    pub fn seen(&self, world: &str, queue: &Queue, me: &str) -> Seen {
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
        for p in queue.of(world) {
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
        q.push("w", set(Some(true), &[]), 5);
        q.save(&path).unwrap();
        assert_eq!(Queue::load(&path, 10).items.len(), 1);
        assert!(Queue::load(&path, DROP_AFTER.as_millis() as u64 + 10).items.is_empty(), "a day old: dropped");
        let _ = std::fs::remove_dir_all(dir);
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
        let seen = known.seen("w", &q, "You");
        assert_eq!((seen.likes, seen.liked, seen.pending), (5, true, 3));
        assert_eq!(seen.labels, vec![("meetup".to_string(), 3, false), ("mirror".to_string(), 2, false), ("pens".to_string(), 1, true)]);
        assert_eq!(seen.comment_count, 2);
        assert_eq!(seen.comments.iter().map(|c| (c.0, c.2.as_str(), c.4)).collect::<Vec<_>>(), [(0, "new", true), (8, "theirs", false)]);
        assert!(Known::default().seen("w", &Queue::default(), "You").loading);
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
        let view = member.set(&world, &set).unwrap();
        assert_eq!(view.likes, 1);
        assert_eq!(view.mine.as_ref().map(|m| m.liked), Some(true));
        assert_eq!(view.labels.len(), 2);
        let refused = member.set(
            &world,
            &Set { labels: [("ai-world".to_string(), true)].into_iter().collect(), justification: Some("too short".into()), ..Default::default() },
        );
        assert!(matches!(refused, Err(Failure::Refused { code: Some(Refusal::JustificationTooShort), .. })), "{refused:?}");
        let id = client_id();
        let first = member.comment(&world, "Great place for meetups!", &id).unwrap();
        let again = member.comment(&world, "Great place for meetups!", &id);
        // A retry inside the server's 20 s window is held back (429, tried later) or answers the same comment.
        match again {
            Ok(c) => assert_eq!(c.id, first.id),
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
