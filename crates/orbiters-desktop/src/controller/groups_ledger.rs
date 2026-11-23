//! The record of every group write, owned by the service (verify2 B-A, J7, J9): one small file per operation, on the disk
//! BEFORE anything is sent, and after. It says that a write was prepared, went out, and how it ended, and it outlives the
//! window, the page, the account's sign-in and the program. The window only shows it.
//!
//! An entry holds ids, hashes, times and a state. No text, no name, no picture of anyone: what a check compares is a hash
//! of what was asked and of what was there. The only id of another person is the member a role was given to or taken
//! from, which is what a check must read again.
//!
//! - `Prepared` → `Dispatched` → `Done` | `Refused` | `Unknown`. A program that ends between `Dispatched` and the answer
//!   leaves `Dispatched` on the disk; the next one reads it as `Unknown`, never as "not sent".
//! - Nothing here ends an `Unknown` by itself: no timeout, no count, no age. A read that identifies the very object does,
//!   or the person saying what they saw (kept as what the person said, `Said`).
//! - An entry that cannot be written refuses the write it is for.
use super::vrchat_data::Scope;
use orbiters_core::groups::{ArtSlot, FileTag, PostField, RoleField};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

/// Ended entries kept (for "already done" and the record), at most this many and this long. Those not ended are kept
/// whatever their age and number.
const ENDED_MAX: usize = 256;
const ENDED_FOR_MS: i64 = 30 * 24 * 3600 * 1000;
/// A write that ended done this long ago or less is said before the same is sent again.
const RECENT_MS: i64 = 24 * 3600 * 1000;
/// A read made sooner than this after a send proves nothing about its absence: VRChat may still be at it (the request's
/// own time limit is 30 s).
pub const SETTLES_AFTER_MS: i64 = 60 * 1000;

/// Who and where, for good: the environment, the Orbiters account (hashed) and the VRChat account. Not the sign-in's
/// revision, which changes while the account does not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    pub environment: String,
    pub orbiters: String,
    pub user: String,
}

impl Key {
    pub fn of(scope: &Scope) -> Option<Self> {
        let (development, member, user) = scope.lasting()?;
        Some(Self {
            environment: if development { "development" } else { "production" }.into(), // i18n: not shown (a record's words)
            orbiters: orbiters_core::groups::digest(&["orbiters", member.unwrap_or_default()]),
            user: user.into(),
        })
    }
    /// A folder's name for this account's own files.
    pub fn folder(&self) -> String {
        orbiters_core::groups::digest(&[&self.environment, &self.orbiters, &self.user])[..32].to_string()
    }
}

/// Every write there is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    AssignRole,
    UnassignRole,
    CreateRole,
    EditRole,
    DeleteRole,
    CreatePost,
    EditPost,
    DeletePost,
    ApplyBanner,
    ApplyIcon,
    UploadGallery,
    UploadIcon,
}

impl Kind {
    pub const ALL: [Kind; 12] = [
        Kind::AssignRole,
        Kind::UnassignRole,
        Kind::CreateRole,
        Kind::EditRole,
        Kind::DeleteRole,
        Kind::CreatePost,
        Kind::EditPost,
        Kind::DeletePost,
        Kind::ApplyBanner,
        Kind::ApplyIcon,
        Kind::UploadGallery,
        Kind::UploadIcon,
    ];
    /// What it makes has no id until VRChat answers: a check can only offer candidates, or settle on an exact match.
    pub fn makes(self) -> bool {
        matches!(self, Kind::CreateRole | Kind::CreatePost | Kind::UploadGallery | Kind::UploadIcon)
    }
    pub fn slot(self) -> Option<ArtSlot> {
        match self {
            Kind::ApplyBanner => Some(ArtSlot::Banner),
            Kind::ApplyIcon => Some(ArtSlot::Icon),
            _ => None,
        }
    }
    pub fn tag(self) -> Option<FileTag> {
        match self {
            Kind::UploadGallery => Some(FileTag::Gallery),
            Kind::UploadIcon => Some(FileTag::Icon),
            _ => None,
        }
    }
}

/// The ids a write is about.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// The file a slot held, or none: a value of its own, as "none there" and "not known" are not the same on the disk either.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Before {
    pub file: Option<String>,
}

/// What a check compares with, as hashes and ids.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// A new role's, post's or picture's whole content, hashed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// The parts an edit changes, what they were asked to be, and what the whole object was before it, hashed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub post_fields: Vec<PostField>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub role_fields: Vec<RoleField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wanted: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was: Option<String>,
    /// The ids there were just before (posts, roles, files): what is made is none of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Vec<String>>,
    /// What was in the slot just before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<Before>,
    /// An upload's bytes: MD5 and size as VRChat's file API says them, and the studio's art they were made from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Prepared,
    Dispatched,
    Done,
    Refused,
    Unknown,
}

/// How an ended entry was settled, or why it was refused: a code, for the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum By {
    /// VRChat's own answer to the write, matching what was asked.
    Answer,
    /// A later read that identified the very object.
    Read,
    /// Refused before anything went out (the rules, the account, a change meanwhile, a wait VRChat asked for…).
    BeforeSending,
    /// Cancelled, replaced, or left unsent when the program ended.
    NeverSent,
    /// The person said it was done.
    Person,
}

/// What the person said of a write nothing could settle. The entry stays what it is, with what they said.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Said {
    Done,
    NotDone,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub op: String,
    pub key: Key,
    pub group: String,
    pub kind: Kind,
    pub target: Target,
    /// The exact payload (method, path, body or bytes), hashed.
    pub payload: String,
    pub evidence: Evidence,
    pub state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<By>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<Said>,
    /// What it made, once known (a post, a role, a file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made: Option<String>,
    /// A check ran and could not settle it: the person may say what they saw.
    #[serde(default)]
    pub checked: bool,
    /// Milliseconds since 1970.
    pub prepared_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatched_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
}

impl Entry {
    /// It may have been done, and nothing has said.
    pub fn open(&self) -> bool {
        matches!(self.state, State::Dispatched | State::Unknown) && self.said.is_none()
    }
    fn same_place(&self, other: &Entry) -> bool {
        self.key == other.key && self.group == other.group && self.kind == other.kind && self.target == other.target
    }
    /// The same thing sent to the same place.
    pub fn identical(&self, other: &Entry) -> bool {
        self.same_place(other) && self.payload == other.payload
    }
    /// Something else sent to the same object (another role change of the same person, another edit of the same post, its
    /// deletion, another picture for the same slot): one may overtake the other.
    fn same_object(&self, other: &Entry) -> bool {
        if self.key != other.key || self.group != other.group {
            return false;
        }
        let makes = |e: &Entry| e.kind.makes();
        match (makes(self), makes(other)) {
            (true, true) => self.kind == other.kind,
            (false, false) if self.kind.slot().is_some() || other.kind.slot().is_some() => self.kind == other.kind,
            (false, false) => {
                let id = |e: &Entry| (e.target.user.clone(), if e.target.user.is_some() { None } else { e.target.role.clone().or(e.target.post.clone()) });
                id(self) == id(other)
            }
            _ => false,
        }
    }
}

/// What another entry says of a write about to go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Warning {
    /// Another change of the same object may or may not have been done.
    AnotherMayHaveGone,
    /// The same was done less than a day ago.
    AlreadyDone,
    /// The same went out before; the person said it was not done.
    SaidNotDone,
    /// A record of an earlier write could not be read.
    RecordUnreadable,
}

/// The entry could not be written: nothing is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotKept;

/// Why a dispatch may not begin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    NotKept,
    /// The same may already have been sent.
    MaySent,
    /// Not a prepared entry (already sent once, cancelled, gone).
    NotPrepared,
}

#[derive(Default)]
struct Kept {
    entries: Vec<Entry>,
    loaded: bool,
    unreadable: bool,
    /// Operations a thread of this program is sending now.
    active: HashSet<String>,
}

pub struct Ledger {
    dir: PathBuf,
    kept: Mutex<Kept>,
    /// Moves the clock (tests).
    #[cfg(test)]
    pub(crate) ahead_ms: std::sync::atomic::AtomicI64,
    /// Every write of an entry fails (tests: a full or refused disk).
    #[cfg(test)]
    pub(crate) full: std::sync::atomic::AtomicBool,
}

impl Default for Ledger {
    fn default() -> Self {
        let root = orbiters_core::paths::local_data_root().join("group-ledger");
        // Each test's controller keeps its own.
        #[cfg(test)]
        let root = root.join(uuid::Uuid::new_v4().to_string());
        Self::at(root)
    }
}

impl Ledger {
    pub fn at(dir: PathBuf) -> Self {
        Self {
            dir,
            kept: Mutex::default(),
            #[cfg(test)]
            ahead_ms: Default::default(),
            #[cfg(test)]
            full: Default::default(),
        }
    }
    #[cfg(test)]
    pub(crate) fn dir(&self) -> PathBuf {
        self.dir.clone()
    }
    pub fn now(&self) -> i64 {
        let now = (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
        #[cfg(test)]
        let now = now + self.ahead_ms.load(std::sync::atomic::Ordering::Relaxed);
        now
    }
    fn file(&self, op: &str) -> PathBuf {
        self.dir.join(format!("{op}.json"))
    }
    fn write(&self, e: &Entry) -> Result<(), NotKept> {
        #[cfg(test)]
        if self.full.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(NotKept);
        }
        let bytes = serde_json::to_vec(e).map_err(|_| NotKept)?;
        std::fs::create_dir_all(&self.dir).map_err(|_| NotKept)?;
        orbiters_core::write_atomic_private(&self.file(&e.op), &bytes).map_err(|_| NotKept)
    }
    /// What an earlier program left: prepared and never sent, or sent and never answered (which is unknown, not "not
    /// sent"). A file that cannot be read is left where it is, and said.
    fn load(&self, k: &mut Kept) {
        if std::mem::replace(&mut k.loaded, true) {
            return;
        }
        let files = match std::fs::read_dir(&self.dir) {
            Ok(files) => files,
            Err(e) => {
                k.unreadable = e.kind() != std::io::ErrorKind::NotFound;
                return;
            }
        };
        for f in files {
            let Ok(f) = f else {
                k.unreadable = true;
                continue;
            };
            let path = f.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            match std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Entry>(&b).ok()) {
                Some(e) if path.file_stem().and_then(|s| s.to_str()) == Some(e.op.as_str()) => k.entries.push(e),
                _ => k.unreadable = true,
            }
        }
        let now = self.now();
        for e in k.entries.iter_mut() {
            match e.state {
                State::Prepared => (e.state, e.by, e.ended_at) = (State::Refused, Some(By::NeverSent), Some(now)),
                State::Dispatched => e.state = State::Unknown,
                _ => continue,
            }
            let _ = self.write(e);
        }
        k.entries.sort_by_key(|e| e.prepared_at);
        self.trim(k);
    }
    fn trim(&self, k: &mut Kept) {
        let now = self.now();
        let mut ended: Vec<(String, i64)> = k.entries.iter().filter(|e| matches!(e.state, State::Done | State::Refused)).map(|e| (e.op.clone(), e.ended_at.unwrap_or(e.prepared_at))).collect();
        ended.sort_by_key(|(_, at)| -at);
        let gone: HashSet<String> = ended.into_iter().enumerate().filter(|(i, (_, at))| *i >= ENDED_MAX || now - at > ENDED_FOR_MS).map(|(_, (op, _))| op).collect();
        for op in &gone {
            let _ = std::fs::remove_file(self.file(op));
        }
        k.entries.retain(|e| !gone.contains(&e.op));
    }
    fn with<T>(&self, f: impl FnOnce(&mut Kept) -> T) -> T {
        let mut k = self.kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.load(&mut k);
        f(&mut k)
    }

    /// Written as prepared, or the write is not offered.
    pub fn prepare(&self, e: Entry) -> Result<(), NotKept> {
        self.with(|k| {
            self.write(&e)?;
            k.entries.push(e);
            self.trim(k);
            Ok(())
        })
    }
    /// An open entry of exactly this: it may already have been sent.
    pub fn may_have_gone(&self, e: &Entry) -> Option<String> {
        self.with(|k| k.entries.iter().find(|x| x.op != e.op && x.open() && x.identical(e)).map(|x| x.op.clone()))
    }
    pub fn warning(&self, e: &Entry) -> Option<Warning> {
        let now = self.now();
        self.with(|k| {
            let others = || k.entries.iter().filter(|x| x.op != e.op);
            if others().any(|x| x.open() && x.same_object(e)) {
                return Some(Warning::AnotherMayHaveGone);
            }
            if others().any(|x| x.state == State::Unknown && x.said == Some(Said::NotDone) && x.identical(e)) {
                return Some(Warning::SaidNotDone);
            }
            if e.kind.makes() && others().any(|x| x.state == State::Done && x.identical(e) && now - x.ended_at.unwrap_or(0) < RECENT_MS) {
                return Some(Warning::AlreadyDone);
            }
            k.unreadable.then_some(Warning::RecordUnreadable)
        })
    }
    /// The point of no return: on the disk as sent, or not sent. The last write to the disk before the request. Another
    /// change of the same object that may have gone since it was sealed stops it unless the person was told (`warned`).
    pub fn dispatch(&self, op: &str, warned: bool) -> Result<(), Stop> {
        let now = self.now();
        self.with(|k| {
            let at = k.entries.iter().position(|e| e.op == op && e.state == State::Prepared).ok_or(Stop::NotPrepared)?;
            let mut e = k.entries[at].clone();
            if k.entries.iter().any(|x| x.op != e.op && x.open() && (x.identical(&e) || !warned && x.same_object(&e))) {
                return Err(Stop::MaySent);
            }
            (e.state, e.dispatched_at) = (State::Dispatched, Some(now));
            self.write(&e).map_err(|_| Stop::NotKept)?;
            k.entries[at] = e;
            k.active.insert(op.to_string());
            Ok(())
        })
    }
    /// How it ended. Kept in memory even when the disk refuses it: the disk then still says `Dispatched`, which the next
    /// program reads as unknown.
    pub fn end(&self, op: &str, state: State, by: Option<By>, made: Option<String>) {
        let now = self.now();
        self.with(|k| {
            k.active.remove(op);
            if let Some(e) = k.entries.iter_mut().find(|e| e.op == op) {
                (e.state, e.by) = (state, by);
                e.made = made.or(e.made.take());
                e.ended_at = matches!(state, State::Done | State::Refused).then_some(now);
                let e = e.clone();
                let _ = self.write(&e);
            }
            self.trim(k);
        })
    }
    pub fn checked(&self, op: &str) {
        self.with(|k| {
            if let Some(e) = k.entries.iter_mut().find(|e| e.op == op) {
                e.checked = true;
                let e = e.clone();
                let _ = self.write(&e);
            }
        })
    }
    /// What the person said. It stays `Unknown` when they said it was not done: a record, not a result.
    pub fn said(&self, op: &str, said: Said, made: Option<String>) -> bool {
        let now = self.now();
        self.with(|k| {
            let active = k.active.contains(op);
            let Some(at) = k.entries.iter().position(|e| e.op == op && e.state == State::Unknown && e.checked && !active) else { return false };
            let mut e = k.entries[at].clone();
            e.said = Some(said);
            if said == Said::Done {
                (e.state, e.by, e.ended_at) = (State::Done, Some(By::Person), Some(now));
                e.made = made.or(e.made.take());
            }
            // Kept, or not said at all: a hold is never lifted by words the next program would not find.
            if self.write(&e).is_err() {
                return false;
            }
            k.entries[at] = e;
            true
        })
    }
    pub fn get(&self, key: &Key, op: &str) -> Option<Entry> {
        self.with(|k| k.entries.iter().find(|e| e.op == op && e.key == *key).cloned())
    }
    pub fn active(&self, op: &str) -> bool {
        self.with(|k| k.active.contains(op))
    }
    /// The account's entries, the newest first.
    pub fn of(&self, key: &Key) -> Vec<Entry> {
        self.with(|k| k.entries.iter().rev().filter(|e| e.key == *key).cloned().collect())
    }
    /// Prepared and not sent: gone, as never sent.
    pub fn never_sent(&self, op: &str) {
        let now = self.now();
        // One step: what a commit dispatches meanwhile is not called unsent.
        self.with(|k| {
            if let Some(e) = k.entries.iter_mut().find(|e| e.op == op && e.state == State::Prepared) {
                (e.state, e.by, e.ended_at) = (State::Refused, Some(By::NeverSent), Some(now));
                let e = e.clone();
                let _ = self.write(&e);
            }
            self.trim(k);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Key {
        Key { environment: "production".into(), orbiters: "o".into(), user: "usr_a".into() }
    }
    fn entry(l: &Ledger, op: &str, kind: Kind, post: Option<&str>, payload: &str) -> Entry {
        Entry {
            op: op.into(),
            key: key(),
            group: "grp_1".into(),
            kind,
            target: Target { post: post.map(str::to_string), ..Default::default() },
            payload: payload.into(),
            evidence: Evidence::default(),
            state: State::Prepared,
            by: None,
            said: None,
            made: None,
            checked: false,
            prepared_at: l.now(),
            dispatched_at: None,
            ended_at: None,
        }
    }
    fn fresh() -> Ledger {
        Ledger::default()
    }
    /// Another program over the same folder.
    fn again(l: &Ledger) -> Ledger {
        Ledger::at(l.dir())
    }

    /// Nothing a check compares with is another thing once it has been on the disk: "none there" above all, which is not
    /// "not known".
    #[test]
    fn the_disk_keeps_every_part() {
        let l = fresh();
        for (i, previous) in [None, Some(Before { file: None }), Some(Before { file: Some("file_a".into()) })].into_iter().enumerate() {
            let mut e = entry(&l, &format!("op-{i}"), Kind::ApplyIcon, None, "p");
            e.target = Target { user: Some("usr_b".into()), role: Some("grol_a".into()), post: Some("gpos_a".into()), file: Some("file_b".into()) };
            e.evidence = Evidence {
                content: Some("c".into()),
                post_fields: PostField::ALL.to_vec(),
                role_fields: RoleField::ALL.to_vec(),
                wanted: Some("w".into()),
                was: Some("x".into()),
                before: Some(vec![]),
                previous,
                md5: Some("m".into()),
                size: Some(u64::MAX),
                art: Some(u64::MAX),
            };
            (e.state, e.by, e.made, e.checked, e.dispatched_at) = (State::Unknown, Some(By::Read), Some("gpos_b".into()), true, Some(5));
            l.prepare(e.clone()).unwrap();
            assert_eq!(again(&l).get(&key(), &e.op), Some(e));
        }
    }

    /// B-A: whatever ends the program, what was sent is still there, as unknown; what was only prepared was never sent.
    #[test]
    fn an_entry_outlives_the_program() {
        let l = fresh();
        for op in ["prepared", "sent", "done"] {
            l.prepare(entry(&l, op, Kind::CreatePost, None, op)).unwrap();
        }
        l.dispatch("sent", true).unwrap();
        l.dispatch("done", true).unwrap();
        l.end("done", State::Done, Some(By::Answer), Some("gpos_1".into()));
        let l2 = again(&l);
        let state = |op: &str| l2.get(&key(), op).map(|e| (e.state, e.by)).unwrap();
        assert_eq!(state("prepared"), (State::Refused, Some(By::NeverSent)));
        assert_eq!(state("sent"), (State::Unknown, None));
        assert_eq!(state("done"), (State::Done, Some(By::Answer)));
        assert_eq!(again(&l2).get(&key(), "sent").unwrap().state, State::Unknown, "and after yet another");
    }

    /// The same thing never goes twice while the first may have gone; another to the same object warns; nothing but the
    /// person's word, or a result, ends that.
    #[test]
    fn the_same_is_never_sent_twice_unseen() {
        let l = fresh();
        l.prepare(entry(&l, "a", Kind::EditPost, Some("gpos_1"), "p")).unwrap();
        l.prepare(entry(&l, "b", Kind::EditPost, Some("gpos_1"), "p")).unwrap();
        l.prepare(entry(&l, "c", Kind::EditPost, Some("gpos_1"), "other")).unwrap();
        l.prepare(entry(&l, "d", Kind::EditPost, Some("gpos_2"), "p")).unwrap();
        l.prepare(entry(&l, "e", Kind::DeletePost, Some("gpos_1"), "del")).unwrap();
        l.dispatch("a", false).unwrap();
        assert_eq!(l.dispatch("a", false), Err(Stop::NotPrepared), "one operation goes once");
        assert_eq!(l.dispatch("b", false), Err(Stop::MaySent));
        assert_eq!(l.dispatch("b", true), Err(Stop::MaySent), "the identical one is refused whatever was shown");
        assert_eq!(l.dispatch("c", false), Err(Stop::MaySent), "sealed before the other went: its person was told nothing");
        let get = |op: &str| l.get(&key(), op).unwrap();
        assert_eq!(l.may_have_gone(&get("b")), Some("a".into()));
        assert_eq!(l.warning(&get("c")), Some(Warning::AnotherMayHaveGone));
        assert_eq!(l.warning(&get("e")), Some(Warning::AnotherMayHaveGone), "its deletion is the same object");
        assert_eq!(l.warning(&get("d")), None);
        l.end("a", State::Unknown, None, None);
        assert!(!l.said("a", Said::NotDone, None), "not before a check");
        l.ahead_ms.store(400 * 24 * 3600 * 1000, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(l.dispatch("b", false), Err(Stop::MaySent), "no age ends an unknown");
        l.checked("a");
        assert!(l.said("a", Said::NotDone, None));
        assert_eq!((get("a").state, get("a").said), (State::Unknown, Some(Said::NotDone)), "kept as what it is, with what was said");
        assert_eq!(l.warning(&get("b")), Some(Warning::SaidNotDone));
        l.dispatch("b", false).unwrap();
    }

    /// Nothing is sent that is not written down first; a failed last write leaves "unknown" for the next program.
    #[test]
    fn a_disk_that_refuses_stops_the_write() {
        let l = fresh();
        l.full.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(l.prepare(entry(&l, "a", Kind::CreatePost, None, "p")), Err(NotKept));
        l.full.store(false, std::sync::atomic::Ordering::Relaxed);
        l.prepare(entry(&l, "a", Kind::CreatePost, None, "p")).unwrap();
        l.full.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(l.dispatch("a", false), Err(Stop::NotKept));
        assert_eq!(l.get(&key(), "a").unwrap().state, State::Prepared, "not sent");
        l.full.store(false, std::sync::atomic::Ordering::Relaxed);
        l.dispatch("a", false).unwrap();
        l.full.store(true, std::sync::atomic::Ordering::Relaxed);
        l.end("a", State::Done, Some(By::Answer), None);
        assert_eq!(again(&l).get(&key(), "a").unwrap().state, State::Unknown);
    }

    /// J7: open entries are never evicted by number or age; ended ones are, oldest first.
    #[test]
    fn open_entries_are_never_evicted() {
        let l = fresh();
        for i in 0..ENDED_MAX + 80 {
            let op = format!("op-{i}");
            l.prepare(entry(&l, &op, Kind::CreatePost, None, &op)).unwrap();
            l.dispatch(&op, true).unwrap();
            if i % 2 == 0 {
                l.end(&op, State::Done, Some(By::Answer), None);
            } else {
                l.end(&op, State::Unknown, None, None);
            }
        }
        l.ahead_ms.store(ENDED_FOR_MS * 2, std::sync::atomic::Ordering::Relaxed);
        l.end("op-1", State::Unknown, None, None);
        let left = again(&l).of(&key());
        assert_eq!(left.iter().filter(|e| e.state == State::Unknown).count(), (ENDED_MAX + 80) / 2);
        assert_eq!(left.iter().filter(|e| e.state == State::Done).count(), 0);
    }

    /// J9: one account's record, under another environment, Orbiters account or VRChat account, is not the other's; a
    /// file that cannot be read is said and left alone.
    #[test]
    fn entries_are_their_accounts_own() {
        let l = fresh();
        l.prepare(entry(&l, "a", Kind::CreatePost, None, "p")).unwrap();
        for k in [Key { environment: "development".into(), ..key() }, Key { orbiters: "x".into(), ..key() }, Key { user: "usr_b".into(), ..key() }] {
            assert!(l.get(&k, "a").is_none() && l.of(&k).is_empty());
        }
        std::fs::write(l.dir().join("broken.json"), b"{").unwrap();
        let l2 = again(&l);
        let e = entry(&l2, "b", Kind::CreatePost, None, "q");
        assert_eq!(l2.warning(&e), Some(Warning::RecordUnreadable));
        assert!(l.dir().join("broken.json").exists());
    }
}
