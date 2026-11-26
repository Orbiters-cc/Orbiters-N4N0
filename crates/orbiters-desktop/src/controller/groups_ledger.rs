//! The record of every group write, owned by the service (verify2 B-A, J7, J9): one small file per operation, on the disk
//! BEFORE anything is sent, and after. It says that a write was prepared, went out, and how it ended, and it outlives the
//! window, the page, the account's sign-in and the program. The window only shows it.
//!
//! An entry holds ids, hashes, times and a state. No text, no name, no picture of anyone: what a check compares is a hash
//! of what was asked. The only id of another person is the member a role was given to or taken from, which is what a check
//! must read again; the membership is a hash, the objects there before are hashes.
//!
//! - `Prepared` → `Dispatched` → `Done` | `Refused` | `Unknown`, by VRChat's own answer to the write and nothing else. A
//!   program that ends between `Dispatched` and the answer leaves `Dispatched` on the disk; the next one reads it as
//!   `Unknown`, never as "not sent".
//! - A check reports what VRChat shows NOW, never what happened (verify3 Blocker 2): an `Unknown` stays one. It is closed
//!   when VRChat shows what was asked (whoever did it), or by the person: again, as a new operation; or dropped. Nothing
//!   else closes it: no timeout, no count, no age.
//! - Every dispatch of every window goes through [`Ledger::dispatch`]: the same write to the same place (whatever its
//!   spaces, Unicode form or order) while one may have gone is refused, and one sealed before another like it went is
//!   refused too: its person was told nothing of it (verify3 Blocker 3).
//! - One owner at a time: the folder's lock is held for as long as this lives. Another program on it sends nothing
//!   (verify3 Major 2). An entry that cannot be written refuses the write it is for; a record that cannot be read refuses
//!   the writes it may be of, until the person looked and goes on (verify3, deviation 7), for their own account (verify4 F5).
use orbiters_core::locks::*;
use super::vrchat_data::Scope;
use orbiters_core::groups::{ArtSlot, Beside, FileTag, Part, RoleField, Said};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// The form of an entry's file. A newer one is not read, not rewritten, not guessed at.
pub const FORMAT: u32 = 3;
/// Entries that ended kept (for "already done" and the record), and those the person closed (for the warning that the same
/// went before), at most this many of each and this long. Those open are kept whatever their age and number.
const ENDED_MAX: usize = 256;
const ENDED_FOR_MS: i64 = 30 * 24 * 3600 * 1000;
/// A write that is done since this long ago or less is said before the same is sent again.
const RECENT_MS: i64 = 24 * 3600 * 1000;
/// A read made sooner than this after a send says nothing by its absence: VRChat may still be at it (the request's own
/// time limit is 30 s).
pub const SETTLES_AFTER_MS: i64 = 60 * 1000;

/// Who and where, for good: the environment, the Orbiters account (hashed; by its id where it is known,
/// `Controller::groups_key`) and the VRChat account. Not the sign-in's revision, which changes while the account does not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    pub environment: String,
    pub orbiters: String,
    pub user: String,
}

impl Key {
    /// By the Orbiters member the scope was verified for (a name can be changed, or used again); while they have no id of
    /// their own, by their name.
    pub fn of(scope: &Scope) -> Option<Self> {
        let (development, member, user) = scope.lasting()?;
        Some(Self {
            environment: if development { "development" } else { "production" }.into(), // i18n: not shown (a record's words)
            orbiters: scope.orbiters_member().map_or_else(|| orbiters_core::groups::digest(&["orbiters", member.unwrap_or_default()]), str::to_string),
            user: user.into(),
        })
    }
    /// The same, only of a member known for certain: what is the person's own on this computer's disk is kept by it
    /// (verify4 F8).
    pub fn certain(scope: &Scope) -> Option<Self> {
        scope.orbiters_member().and_then(|_| Self::of(scope))
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
    DeletePost,
    ApplyBanner,
    ApplyIcon,
    UploadGallery,
    UploadIcon,
}

impl Kind {
    pub const ALL: [Kind; 11] = [
        Kind::AssignRole,
        Kind::UnassignRole,
        Kind::CreateRole,
        Kind::EditRole,
        Kind::DeleteRole,
        Kind::CreatePost,
        Kind::DeletePost,
        Kind::ApplyBanner,
        Kind::ApplyIcon,
        Kind::UploadGallery,
        Kind::UploadIcon,
    ];
    /// What it makes has no id until VRChat answers: a check can only offer what is there, for the person.
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
    /// The very membership, as a hash: leaving and joining again is another one (verify3 Blocker 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub membership: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// What a check compares with, as hashes and ids.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// A new role's, post's or picture's whole content, hashed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// The parts a role's change is of, and what they were asked to be, hashed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub role_fields: Vec<RoleField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wanted: Option<String>,
    /// The ids there were just before (posts, roles, files), hashed: what is made is none of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Vec<String>>,
    /// An upload's bytes: MD5 and size as VRChat's file API says them, and the studio's art they were made from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<u64>,
    /// The new post being written it came from, and which revision of it (the notification included): only that one is
    /// cleared when it is done (verify3 Major 6, verify4 F6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// `before` is all there was: every entry gave its id, and the list was read to its end (verify4 F2).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub whole: bool,
    /// What the group or the role was before, part by part, hashed: what else moved is seen by it (verify4 F7).
    #[serde(default, skip_serializing_if = "Said::is_empty")]
    pub was: Said,
}

/// An id as kept among those there before: enough to know it again, not to say it.
pub fn known_id(id: &str) -> String {
    orbiters_core::groups::digest(&["before", id])[..24].to_string() // i18n: not shown (a record's words)
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

/// What ended an entry, or why it was refused: a code, for the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum By {
    /// VRChat's own answer to the write: done and matching what was asked, or its "no".
    Answer,
    /// Refused before anything went out (the rules, the account, a change meanwhile, a wait VRChat asked for…).
    BeforeSending,
    /// Cancelled, replaced, or left unsent when the program ended.
    NeverSent,
}

/// What a look at VRChat said, the last one: now, not then.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// VRChat shows what was asked for, whoever did it.
    Shows,
    /// VRChat does not show it: it may never have applied, or was changed since.
    ShowsNot,
    CannotTell,
}

/// How an unknown one was closed. It stays unknown: no one says it was, or was not, done.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Closing {
    /// VRChat shows what was asked for: there is nothing more to do, and nothing is sent again.
    Shows,
    /// The person asks for it again, as a new operation.
    Again,
    /// The person leaves it.
    Dropped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub v: u32,
    pub op: String,
    pub key: Key,
    pub group: String,
    pub kind: Kind,
    pub target: Target,
    /// The exact payload (method, path, body or bytes), hashed: what is sealed is what goes.
    pub payload: String,
    /// The same payload whatever its spaces, Unicode form or order, hashed: what makes it the same again.
    pub same: String,
    pub evidence: Evidence,
    pub state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<By>,
    /// What it made, once known (a post, a role, a file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made: Option<String>,
    /// A check was asked for: the person may decide.
    #[serde(default)]
    pub checked: bool,
    /// The last look's answer, and whether the look was the person's own (on vrchat.com).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Answer>,
    #[serde(default)]
    pub looked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed: Option<Closing>,
    /// What VRChat shows otherwise than it was, beside what was asked (VRChat's, or someone else's meanwhile), until the
    /// person has seen to it (verify4, sparse PUTs).
    #[serde(default, skip_serializing_if = "nothing")]
    pub beside: Beside,
    /// Milliseconds since 1970.
    pub prepared_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatched_at: Option<i64>,
    /// When it ended or was closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
}

fn nothing(b: &Beside) -> bool {
    !b.any()
}

impl Entry {
    /// It may have been done, and it is not closed.
    pub fn open(&self) -> bool {
        matches!(self.state, State::Dispatched | State::Unknown) && self.closed.is_none()
    }
    /// VRChat answered that it is done, or shows what was asked for.
    pub fn stands(&self) -> bool {
        self.state == State::Done || self.closed == Some(Closing::Shows)
    }
    fn ended(&self) -> bool {
        matches!(self.state, State::Done | State::Refused) || self.closed.is_some()
    }
    /// What its state says it must hold is there, and nothing that contradicts it.
    fn sound(&self) -> bool {
        let sent = matches!(self.state, State::Dispatched | State::Done | State::Unknown);
        let times = self.dispatched_at.is_some() || !sent;
        let settled = match self.state {
            State::Done => self.by == Some(By::Answer),
            State::Refused => self.by.is_some(),
            _ => self.by.is_none(),
        };
        let about = match self.kind {
            Kind::AssignRole | Kind::UnassignRole => self.target.user.is_some() && self.target.role.is_some() && self.target.membership.is_some(),
            Kind::EditRole | Kind::DeleteRole => self.target.role.is_some(),
            Kind::DeletePost => self.target.post.is_some(),
            Kind::ApplyBanner | Kind::ApplyIcon => self.target.file.is_some(),
            _ => true,
        };
        let closing = self.closed.is_none() || self.state == State::Unknown;
        times && settled && about && closing && !self.same.is_empty() && !self.payload.is_empty() && !self.key.user.is_empty()
    }
    fn same_place(&self, other: &Entry) -> bool {
        // An upload goes to the account's own pictures, whichever group's page it was asked from.
        self.key == other.key && self.kind == other.kind && self.target == other.target && (self.kind.tag().is_some() || self.group == other.group)
    }
    /// The same thing sent to the same place.
    pub fn identical(&self, other: &Entry) -> bool {
        self.same_place(other) && self.same == other.same
    }
    /// Something else sent to the same object (another role change of the same person, another change of the same role,
    /// its deletion, another picture for the same slot): one may overtake the other.
    fn same_object(&self, other: &Entry) -> bool {
        if self.key != other.key || (self.kind.tag().is_none() && self.group != other.group) {
            return false;
        }
        match (self.kind.makes(), other.kind.makes()) {
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

/// What other entries say of a write about to go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Warning {
    /// Another change of the same object may or may not have been done.
    AnotherMayHaveGone,
    /// The same went out before and no one knows what became of it: the person asked for it again, or left it.
    SameBefore,
    /// The same was done less than a day ago.
    AlreadyDone,
    /// A record of an earlier write could not be read; the person looked and went on.
    RecordUnreadable,
}

/// The entry could not be written, or not now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Not {
    /// The disk refused it: nothing is sent.
    Kept,
    /// Another program holds the record.
    Owner,
    /// A record it may be of cannot be read.
    Read,
}

/// Why a dispatch may not begin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Not(Not),
    /// The same may already have been sent.
    MaySent,
    /// Another like it went out since it was sealed: the person was told nothing of it.
    Since,
    /// Not a prepared entry (already sent once, cancelled, gone).
    NotPrepared,
}

#[derive(Default)]
struct Kept {
    entries: Vec<Entry>,
    loaded: bool,
    /// Records that could not be read, left as they are.
    unreadable: Vec<PathBuf>,
    /// Those an account looked into and goes on past, by that account: its word, and no other's (verify4 F5).
    looked: HashMap<PathBuf, HashMap<String, SystemTime>>,
    /// Operations a thread of this program is sending now.
    active: HashSet<String>,
    /// The folder is this program's while this is held.
    lock: Option<std::fs::File>,
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
    /// What ends with the program: the lock on the folder, and what it had in memory.
    #[cfg(test)]
    pub(crate) fn let_go(&self) {
        *self.kept.lock().unwrap() = Kept::default();
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
    fn write(&self, e: &Entry) -> Result<(), Not> {
        #[cfg(test)]
        if self.full.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Not::Kept);
        }
        let bytes = serde_json::to_vec(e).map_err(|_| Not::Kept)?;
        orbiters_core::write_atomic_private(&self.file(&e.op), &bytes).map_err(|_| Not::Kept)
    }
    /// The folder, held by this program from its first use on (an operating system's lock: it ends with the program,
    /// however it ends).
    fn own(&self, k: &mut Kept) -> bool {
        if k.lock.is_some() {
            return true;
        }
        if std::fs::create_dir_all(&self.dir).is_err() {
            return false;
        }
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(self.dir.join("owner.lock"));
        let Some(file) = file.ok().filter(|f| f.try_lock().is_ok()) else { return false };
        k.lock = Some(file);
        true
    }
    fn entry_of(path: &Path) -> Option<Entry> {
        let e: Entry = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        (e.v == FORMAT && e.sound() && path.file_stem().and_then(|s| s.to_str()) == Some(e.op.as_str())).then_some(e)
    }
    /// What an earlier program left: prepared and never sent, or sent and never answered (which is unknown, not "not
    /// sent"). A file that cannot be read, or is of a newer form, is left as it is, and said.
    fn load(&self, k: &mut Kept) {
        if std::mem::replace(&mut k.loaded, true) {
            return;
        }
        let Ok(files) = std::fs::read_dir(&self.dir) else {
            k.unreadable.push(self.dir.clone());
            return;
        };
        for f in files {
            let Ok(path) = f.map(|f| f.path()) else {
                k.unreadable.push(self.dir.clone());
                continue;
            };
            match path.extension().and_then(|x| x.to_str()) {
                Some("json") => self.take(k, path),
                Some("looked") => self.take_look(k, &path),
                _ => {}
            }
        }
        k.entries.sort_by_key(|e| e.prepared_at);
        self.trim(k);
    }
    /// The mark an account's look at a record leaves: `<record>.<account's folder name>.looked`.
    fn mark(path: &Path, key: &Key) -> PathBuf {
        path.with_extension(format!("{}.looked", key.folder())) // i18n: not shown (a file's name)
    }
    fn take_look(&self, k: &mut Kept, path: &Path) {
        let Some((stem, folder)) = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.rsplit_once('.')) else { return };
        let at = std::fs::metadata(path).and_then(|m| m.modified()).unwrap_or_else(|_| SystemTime::now());
        k.looked.entry(self.dir.join(format!("{stem}.json"))).or_default().insert(folder.into(), at);
    }
    fn take(&self, k: &mut Kept, path: PathBuf) {
        let Some(mut e) = Self::entry_of(&path) else {
            k.unreadable.push(path);
            return;
        };
        match e.state {
            State::Prepared => (e.state, e.by, e.ended_at) = (State::Refused, Some(By::NeverSent), Some(self.now())),
            State::Dispatched => e.state = State::Unknown,
            _ => {
                k.entries.push(e);
                return;
            }
        }
        let _ = self.write(&e);
        k.entries.push(e);
    }
    /// Records that could not be read a moment ago (held by another program for a while) are read again.
    fn read_again(&self, k: &mut Kept) {
        for path in std::mem::take(&mut k.unreadable) {
            if path == self.dir {
                k.loaded = false;
            } else if path.exists() {
                self.take(k, path);
            }
        }
        if !k.loaded {
            k.entries.clear();
            self.load(k);
        }
        k.entries.sort_by_key(|e| e.prepared_at);
    }
    /// Whether a record that cannot be read may be `key`'s (of `group`, when there is one): what it says of itself where it
    /// can be read at all, else it may be of anything.
    fn may_be(path: &Path, key: &Key, group: Option<&str>) -> bool {
        let v: Option<serde_json::Value> = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let Some(v) = v else { return true };
        let theirs: Option<Key> = v.get("key").and_then(|k| serde_json::from_value(k.clone()).ok());
        let of = v.get("group").and_then(|g| g.as_str());
        let other_account = theirs.is_some_and(|t| t != *key);
        let other_group = of.is_some_and(|of| group.is_some_and(|g| g != of));
        !other_account && !other_group
    }
    /// The records that cannot be read, may be `key`'s, and that it has not looked into.
    fn hiding<'a>(&'a self, k: &'a Kept, key: &'a Key, group: Option<&'a str>) -> impl Iterator<Item = &'a PathBuf> {
        let folder = key.folder();
        k.unreadable.iter().filter(move |path| !k.looked.get(*path).is_some_and(|l| l.contains_key(&folder)) && Self::may_be(path, key, group))
    }
    fn hidden(&self, k: &Kept, key: &Key, group: Option<&str>) -> bool {
        self.hiding(k, key, group).next().is_some()
    }
    /// `key` looked into one lately and goes on: it is warned of.
    fn went_past(&self, k: &Kept, key: &Key) -> bool {
        let folder = key.folder();
        k.unreadable.iter().any(|path| k.looked.get(path).and_then(|l| l.get(&folder)).is_some_and(|at| at.elapsed().is_ok_and(|age| (age.as_millis() as i64) < ENDED_FOR_MS) || at.elapsed().is_err()))
    }
    fn trim(&self, k: &mut Kept) {
        let now = self.now();
        let mut gone: HashSet<String> = HashSet::new();
        for closed in [false, true] {
            let mut over: Vec<(String, i64)> = k.entries.iter().filter(|e| e.ended() && e.closed.is_some() == closed).map(|e| (e.op.clone(), e.ended_at.unwrap_or(e.prepared_at))).collect();
            over.sort_by_key(|(_, at)| -at);
            gone.extend(over.into_iter().enumerate().filter(|(i, (_, at))| *i >= ENDED_MAX || now - at > ENDED_FOR_MS).map(|(_, (op, _))| op));
        }
        for op in &gone {
            let _ = std::fs::remove_file(self.file(op));
        }
        k.entries.retain(|e| !gone.contains(&e.op));
    }
    /// Only for the owner of the folder: another program's record is not read, and nothing is written over it.
    fn with<T>(&self, f: impl FnOnce(&mut Kept) -> T) -> Result<T, Not> {
        let mut k = self.kept.locked();
        if !self.own(&mut k) {
            return Err(Not::Owner);
        }
        self.load(&mut k);
        Ok(f(&mut k))
    }
    /// An entry changed, on the disk first: what the disk refuses has not changed.
    fn change(&self, k: &mut Kept, at: usize, f: impl FnOnce(&mut Entry)) -> Result<(), Not> {
        let mut e = k.entries[at].clone();
        f(&mut e);
        if e.ended() {
            e.evidence.before = None;
        }
        self.write(&e)?;
        k.entries[at] = e;
        Ok(())
    }

    pub fn owned(&self) -> bool {
        self.with(|_| ()).is_ok()
    }
    /// How many records cannot be read that may be this account's, and that it has not looked into.
    pub fn unreadable(&self, key: &Key) -> usize {
        self.with(|k| {
            self.read_again(k);
            self.hiding(k, key, None).count()
        })
        .unwrap_or(0)
    }
    /// The person looked and goes on: for their account alone. The records stay as they are, and hold whoever else they may
    /// be of (verify4 F5).
    pub fn set_aside(&self, key: &Key) -> Result<(), Not> {
        self.with(|k| {
            self.read_again(k);
            let mine: Vec<PathBuf> = self.hiding(k, key, None).cloned().collect();
            for path in mine {
                if path == self.dir || orbiters_core::write_atomic_private(&Self::mark(&path, key), b"").is_err() {
                    return Err(Not::Read);
                }
                k.looked.entry(path).or_default().insert(key.folder(), SystemTime::now());
            }
            Ok(())
        })?
    }
    /// Written as prepared, or the write is not offered.
    pub fn prepare(&self, e: Entry) -> Result<(), Not> {
        self.with(|k| {
            self.read_again(k);
            if self.hidden(k, &e.key, Some(&e.group).filter(|_| e.kind.tag().is_none()).map(String::as_str)) {
                return Err(Not::Read);
            }
            self.write(&e)?;
            k.entries.push(e);
            self.trim(k);
            Ok(())
        })?
    }
    /// An open entry of exactly this: it may already have been sent.
    pub fn may_have_gone(&self, e: &Entry) -> Option<String> {
        self.with(|k| k.entries.iter().find(|x| x.op != e.op && x.open() && x.identical(e)).map(|x| x.op.clone())).ok().flatten()
    }
    fn conflicts_in(&self, k: &Kept, e: &Entry) -> (Option<Warning>, Vec<String>) {
        let now = self.now();
        let (mut worst, mut ops) = (self.went_past(k, &e.key).then_some((Warning::RecordUnreadable, 0)), Vec::new());
        for x in k.entries.iter().filter(|x| x.op != e.op) {
            let w = if x.open() && x.same_object(e) {
                (Warning::AnotherMayHaveGone, 3)
            } else if matches!(x.closed, Some(Closing::Again | Closing::Dropped)) && x.identical(e) {
                (Warning::SameBefore, 2)
            } else if e.kind.makes() && x.stands() && x.identical(e) && now - x.ended_at.unwrap_or(0) < RECENT_MS {
                (Warning::AlreadyDone, 1)
            } else {
                continue;
            };
            ops.push(x.op.clone());
            worst = worst.filter(|o| o.1 >= w.1).or(Some(w));
        }
        (worst.map(|w| w.0), ops)
    }
    /// What the person is told before it goes, and of which entries.
    pub fn conflicts(&self, e: &Entry) -> (Option<Warning>, Vec<String>) {
        self.with(|k| self.conflicts_in(k, e)).unwrap_or_default()
    }
    /// The point of no return, for every window and surface of the service: on the disk as sent, or not sent. The last write
    /// to the disk before the request. What the person was told (`told`, the entries they were warned of) is compared, in the
    /// same step, with what is so now: the same that may have gone is refused, and so is anything like it that went, done or
    /// not, since this was sealed.
    pub fn dispatch(&self, op: &str, told: &[String]) -> Result<(), Stop> {
        let now = self.now();
        self.with(|k| {
            let at = k.entries.iter().position(|e| e.op == op && e.state == State::Prepared).ok_or(Stop::NotPrepared)?;
            let e = k.entries[at].clone();
            let others = || k.entries.iter().filter(|x| x.op != e.op);
            if others().any(|x| x.open() && x.identical(&e)) {
                return Err(Stop::MaySent);
            }
            let (_, like) = self.conflicts_in(k, &e);
            let went_since = others().any(|x| x.identical(&e) && x.stands() && x.dispatched_at.is_some_and(|t| t >= e.prepared_at) && !told.contains(&x.op));
            if went_since || like.iter().any(|x| !told.contains(x)) {
                return Err(Stop::Since);
            }
            if self.hidden(k, &e.key, Some(&e.group).filter(|_| e.kind.tag().is_none()).map(String::as_str)) {
                return Err(Stop::Not(Not::Read));
            }
            self.change(k, at, |e| (e.state, e.dispatched_at) = (State::Dispatched, Some(now))).map_err(Stop::Not)?;
            k.active.insert(op.to_string());
            Ok(())
        })
        .map_err(Stop::Not)?
    }
    /// How it ended, by VRChat's answer or before it. Kept in memory even when the disk refuses it: the disk then still
    /// says `Dispatched`, which the next program reads as unknown.
    pub fn end(&self, op: &str, state: State, by: Option<By>, made: Option<String>, beside: Beside) {
        let now = self.now();
        let _ = self.with(|k| {
            k.active.remove(op);
            if let Some(e) = k.entries.iter_mut().find(|e| e.op == op) {
                (e.state, e.by) = (state, by);
                e.beside = beside;
                e.made = made.or(e.made.take());
                e.ended_at = matches!(state, State::Done | State::Refused).then_some(now);
                if e.ended() {
                    e.evidence.before = None;
                }
                let e = e.clone();
                let _ = self.write(&e);
            }
            self.trim(k);
        });
    }
    /// On the disk as sent, by a thread of this program that is at it, and not ended: what may go now, once.
    pub fn going(&self, op: &str) -> bool {
        self.with(|k| k.active.contains(op) && k.entries.iter().any(|e| e.op == op && e.state == State::Dispatched && e.closed.is_none())).unwrap_or(false)
    }
    /// What the group or the role was at the last read before the send, for a check of an answer lost.
    pub fn was(&self, op: &str, was: &Said) {
        let _ = self.with(|k| {
            if let Some(e) = k.entries.iter_mut().find(|e| e.op == op) {
                e.evidence.was = was.clone();
            }
        });
    }
    /// Parts that moved beside the write `op` are as they were again: nothing more is said of those.
    pub fn put_back(&self, key: &Key, op: &str, parts: &[Part]) {
        let _ = self.with(|k| {
            if let Some(at) = k.entries.iter().position(|e| e.op == op && e.key == *key) {
                let _ = self.change(k, at, |e| e.beside.moved.retain(|p| !parts.contains(p)));
            }
        });
    }
    /// What a check found moved beside what was asked.
    pub fn beside(&self, op: &str, beside: Beside) {
        let _ = self.with(|k| {
            if let Some(at) = k.entries.iter().position(|e| e.op == op) {
                let _ = self.change(k, at, |e| e.beside = beside);
            }
        });
    }
    /// The person saw to what moved beside it (put back, or left): nothing more is said of it.
    pub fn seen_to(&self, key: &Key, op: &str) -> bool {
        self.with(|k| match k.entries.iter().position(|e| e.op == op && e.key == *key) {
            Some(at) => self.change(k, at, |e| e.beside = Beside::default()).is_ok(),
            None => false,
        })
        .unwrap_or(false)
    }
    /// The entry of an unknown write that no thread is at.
    fn undecided(k: &Kept, op: &str) -> Option<usize> {
        k.entries.iter().position(|e| e.op == op && e.state == State::Unknown && e.closed.is_none()).filter(|_| !k.active.contains(op))
    }
    /// A look was asked for: whatever comes of it, the person may decide.
    pub fn looking(&self, op: &str) {
        let _ = self.with(|k| {
            if let Some(at) = Self::undecided(k, op) {
                let _ = self.change(k, at, |e| e.checked = true);
            }
        });
    }
    /// What a look said; `by_person`, on vrchat.com, told by the person after a check. It closes nothing.
    pub fn answer(&self, op: &str, answer: Answer, by_person: bool) -> bool {
        self.with(|k| {
            let Some(at) = Self::undecided(k, op).filter(|at| k.entries[*at].checked) else { return false };
            self.change(k, at, |e| (e.answer, e.looked) = (Some(answer), by_person)).is_ok()
        })
        .unwrap_or(false)
    }
    /// Closed, only once it was looked at, and "again" only when a look says VRChat does not show it. On the disk first: a
    /// hold is never lifted by words the next program would not find.
    pub fn close(&self, op: &str, closing: Closing, by_person: bool, made: Option<String>) -> bool {
        let now = self.now();
        self.with(|k| {
            let Some(at) = Self::undecided(k, op).filter(|at| k.entries[*at].checked) else { return false };
            if closing == Closing::Again && k.entries[at].answer != Some(Answer::ShowsNot) {
                return false;
            }
            let kept = self
                .change(k, at, |e| {
                    (e.closed, e.ended_at) = (Some(closing), Some(now));
                    if closing == Closing::Shows {
                        (e.answer, e.looked) = (Some(Answer::Shows), by_person);
                        e.made = made.or(e.made.take());
                    }
                })
                .is_ok();
            self.trim(k);
            kept
        })
        .unwrap_or(false)
    }
    pub fn get(&self, key: &Key, op: &str) -> Option<Entry> {
        self.with(|k| k.entries.iter().find(|e| e.op == op && e.key == *key).cloned()).ok().flatten()
    }
    pub fn active(&self, op: &str) -> bool {
        self.with(|k| k.active.contains(op)).unwrap_or(false)
    }
    /// The account's entries, the newest first.
    pub fn of(&self, key: &Key) -> Vec<Entry> {
        self.with(|k| k.entries.iter().rev().filter(|e| e.key == *key).cloned().collect()).unwrap_or_default()
    }
    /// Prepared and not sent: gone, as never sent.
    pub fn never_sent(&self, op: &str) {
        let now = self.now();
        // One step: what a commit dispatches meanwhile is not called unsent.
        let _ = self.with(|k| {
            if let Some(e) = k.entries.iter_mut().find(|e| e.op == op && e.state == State::Prepared) {
                (e.state, e.by, e.ended_at) = (State::Refused, Some(By::NeverSent), Some(now));
                e.evidence.before = None;
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
    use std::sync::atomic::Ordering;

    fn key() -> Key {
        Key { environment: "production".into(), orbiters: "o".into(), user: "usr_a".into() }
    }
    fn entry(l: &Ledger, op: &str, kind: Kind, post: Option<&str>, payload: &str) -> Entry {
        let mut target = Target { post: post.map(str::to_string), ..Default::default() };
        match kind {
            Kind::AssignRole | Kind::UnassignRole => (target.user, target.role, target.membership) = (Some("usr_b".into()), Some("grol_a".into()), Some("m".into())),
            Kind::EditRole | Kind::DeleteRole => target.role = Some("grol_a".into()),
            Kind::ApplyBanner | Kind::ApplyIcon => target.file = Some("file_a".into()),
            _ => {}
        }
        Entry {
            v: FORMAT,
            op: op.into(),
            key: key(),
            group: "grp_1".into(),
            kind,
            target,
            payload: payload.into(),
            same: payload.into(),
            evidence: Evidence::default(),
            state: State::Prepared,
            by: None,
            made: None,
            checked: false,
            answer: None,
            looked: false,
            closed: None,
            beside: Beside::default(),
            prepared_at: l.now(),
            dispatched_at: None,
            ended_at: None,
        }
    }
    fn fresh() -> Ledger {
        Ledger::default()
    }
    /// The next program over the same folder: this one is over.
    fn again(l: &Ledger) -> Ledger {
        l.let_go();
        Ledger::at(l.dir())
    }
    /// Sent, whatever was said before it.
    fn send(l: &Ledger, op: &str) {
        let e = l.get(&key(), op).unwrap();
        l.dispatch(op, &l.conflicts(&e).1).unwrap();
    }
    fn lost(l: &Ledger, op: &str) {
        send(l, op);
        l.end(op, State::Unknown, None, None, Beside::default());
    }

    /// Nothing a check compares with is another thing once it has been on the disk.
    #[test]
    fn the_disk_keeps_every_part() {
        let l = fresh();
        let mut e = entry(&l, "op-1", Kind::ApplyIcon, None, "p");
        e.target = Target { user: Some("usr_b".into()), membership: Some("m".into()), role: Some("grol_a".into()), post: Some("gpos_a".into()), file: Some("file_b".into()) };
        e.evidence = Evidence { content: Some("c".into()), role_fields: RoleField::ALL.to_vec(), wanted: Some("w".into()), before: Some(vec![]), md5: Some("m".into()), size: Some(u64::MAX), art: Some(u64::MAX), draft: Some("d".into()), revision: Some("r".into()), whole: true, was: [(orbiters_core::groups::Part::Rules, "x".to_string())].into() };
        e.beside = Beside { moved: vec![orbiters_core::groups::Part::Icon], unread: vec![orbiters_core::groups::Part::Links] };
        (e.state, e.made, e.checked, e.answer, e.looked, e.dispatched_at) = (State::Unknown, Some("gpos_b".into()), true, Some(Answer::ShowsNot), true, Some(5));
        l.prepare(e.clone()).unwrap();
        assert_eq!(again(&l).get(&key(), &e.op), Some(e));
    }

    /// B-A: whatever ends the program, what was sent is still there, as unknown; what was only prepared was never sent.
    #[test]
    fn an_entry_outlives_the_program() {
        let l = fresh();
        for op in ["prepared", "sent", "done"] {
            l.prepare(entry(&l, op, Kind::CreatePost, None, op)).unwrap();
        }
        send(&l, "sent");
        send(&l, "done");
        l.end("done", State::Done, Some(By::Answer), Some("gpos_1".into()), Beside::default());
        let l2 = again(&l);
        let state = |op: &str| l2.get(&key(), op).map(|e| (e.state, e.by)).unwrap();
        assert_eq!(state("prepared"), (State::Refused, Some(By::NeverSent)));
        assert_eq!(state("sent"), (State::Unknown, None));
        assert_eq!(state("done"), (State::Done, Some(By::Answer)));
        assert_eq!(again(&l2).get(&key(), "sent").unwrap().state, State::Unknown, "and after yet another");
    }

    /// The same thing never goes twice while the first may have gone; another to the same object warns; nothing closes it
    /// but a look and the person.
    #[test]
    fn the_same_is_never_sent_twice_unseen() {
        let l = fresh();
        l.prepare(entry(&l, "a", Kind::DeletePost, Some("gpos_1"), "p")).unwrap();
        l.prepare(entry(&l, "b", Kind::DeletePost, Some("gpos_1"), "p")).unwrap();
        l.prepare(entry(&l, "d", Kind::DeletePost, Some("gpos_2"), "p")).unwrap();
        l.prepare(entry(&l, "r1", Kind::EditRole, None, "name")).unwrap();
        l.prepare(entry(&l, "r2", Kind::EditRole, None, "other")).unwrap();
        l.prepare(entry(&l, "r3", Kind::DeleteRole, None, "gone")).unwrap();
        l.dispatch("a", &[]).unwrap();
        assert_eq!(l.dispatch("a", &[]), Err(Stop::NotPrepared), "one operation goes once");
        assert_eq!(l.dispatch("b", &[]), Err(Stop::MaySent));
        assert_eq!(l.dispatch("b", &["a".into()]), Err(Stop::MaySent), "whatever was shown");
        let get = |op: &str| l.get(&key(), op).unwrap();
        assert_eq!(l.may_have_gone(&get("b")), Some("a".into()));
        assert_eq!(l.conflicts(&get("d")).0, None, "another post's");
        l.dispatch("r1", &[]).unwrap();
        for op in ["r2", "r3"] {
            assert_eq!(l.dispatch(op, &[]), Err(Stop::Since), "{op}: sealed before the other went: its person was told nothing");
            assert_eq!(l.conflicts(&get(op)), (Some(Warning::AnotherMayHaveGone), vec!["r1".into()]), "{op}");
        }
        l.dispatch("r2", &["r1".into()]).unwrap();
        l.end("a", State::Unknown, None, None, Beside::default());
        for closing in [Closing::Shows, Closing::Again, Closing::Dropped] {
            assert!(!l.close("a", closing, true, None), "not before a look");
        }
        l.ahead_ms.store(400 * 24 * 3600 * 1000, Ordering::Relaxed);
        assert_eq!(l.dispatch("b", &[]), Err(Stop::MaySent), "no age closes it");
        l.looking("a");
        assert!(!l.close("a", Closing::Again, true, None), "not again unless a look says VRChat does not show it");
        assert!(l.answer("a", Answer::ShowsNot, true));
        assert_eq!(l.dispatch("b", &[]), Err(Stop::MaySent), "an answer closes nothing");
        assert!(l.close("a", Closing::Again, true, None));
        assert_eq!((get("a").state, get("a").closed), (State::Unknown, Some(Closing::Again)), "still unknown");
        assert_eq!(l.dispatch("b", &[]), Err(Stop::Since), "sealed before the person was told of it");
        l.prepare(entry(&l, "b2", Kind::DeletePost, Some("gpos_1"), "p")).unwrap();
        assert_eq!(l.conflicts(&get("b2")), (Some(Warning::SameBefore), vec!["a".into()]));
        l.dispatch("b2", &["a".into()]).unwrap();
    }

    /// verify3 Blocker 3: of several sealed alike before any went, one goes, whatever came of it; the others must be
    /// confirmed again, warned.
    #[test]
    fn of_several_sealed_alike_one_goes() {
        for (what, end) in [("done", Some(State::Done)), ("still being sent", None), ("unknown", Some(State::Unknown))] {
            for kind in Kind::ALL {
                let l = fresh();
                let post = (kind == Kind::DeletePost).then_some("gpos_1");
                for op in ["a", "b"] {
                    let e = entry(&l, op, kind, post, "p");
                    assert_eq!(l.conflicts(&e), (None, vec![]));
                    l.prepare(e).unwrap();
                }
                l.dispatch("a", &[]).unwrap();
                if let Some(state) = end {
                    l.end("a", state, (state == State::Done).then_some(By::Answer), None, Beside::default());
                }
                let stop = if end == Some(State::Done) { Stop::Since } else { Stop::MaySent };
                assert_eq!(l.dispatch("b", &[]), Err(stop), "{kind:?}, the first {what}");
            }
        }
        // Sealed again after it was done: warned when it makes something new, and then it may go.
        let l = fresh();
        l.prepare(entry(&l, "a", Kind::CreatePost, None, "p")).unwrap();
        send(&l, "a");
        l.end("a", State::Done, Some(By::Answer), None, Beside::default());
        let b = entry(&l, "b", Kind::CreatePost, None, "p");
        assert_eq!(l.conflicts(&b), (Some(Warning::AlreadyDone), vec!["a".into()]));
        l.prepare(b).unwrap();
        l.prepare(entry(&l, "c", Kind::CreatePost, None, "p")).unwrap();
        l.dispatch("b", &["a".into()]).unwrap();
        l.end("b", State::Done, Some(By::Answer), None, Beside::default());
        assert_eq!(l.dispatch("c", &["a".into()]), Err(Stop::Since), "warned of the first, not of the second");
        // One VRChat shows counts as done; one never sent as nothing.
        let l = fresh();
        l.prepare(entry(&l, "a", Kind::CreatePost, None, "p")).unwrap();
        lost(&l, "a");
        l.looking("a");
        assert!(l.close("a", Closing::Shows, true, Some("gpos_1".into())));
        assert_eq!(l.conflicts(&entry(&l, "b", Kind::CreatePost, None, "p")).0, Some(Warning::AlreadyDone));
        let l = fresh();
        l.prepare(entry(&l, "a", Kind::CreatePost, None, "p")).unwrap();
        l.never_sent("a");
        assert_eq!(l.conflicts(&entry(&l, "b", Kind::CreatePost, None, "p")).0, None);
    }

    /// An upload is the same from whichever group's page; a role for another membership of the same person is not the same
    /// write, and is warned of.
    #[test]
    fn what_the_same_is() {
        let l = fresh();
        l.prepare(entry(&l, "a", Kind::UploadGallery, None, "p")).unwrap();
        lost(&l, "a");
        let mut other = entry(&l, "b", Kind::UploadGallery, None, "p");
        other.group = "grp_2".into();
        assert_eq!(l.may_have_gone(&other), Some("a".into()));
        let mut post = entry(&l, "c", Kind::CreatePost, None, "p");
        l.prepare(post.clone()).unwrap();
        lost(&l, "c");
        post.group = "grp_2".into();
        assert_eq!(l.may_have_gone(&post), None, "another group's post");
        l.prepare(entry(&l, "r", Kind::AssignRole, None, "p")).unwrap();
        lost(&l, "r");
        let mut back = entry(&l, "r2", Kind::AssignRole, None, "p");
        back.target.membership = Some("another".into());
        assert_eq!((l.may_have_gone(&back), l.conflicts(&back).0), (None, Some(Warning::AnotherMayHaveGone)));
    }

    /// Nothing is sent that is not written down first; nothing is closed that could not be; a failed last write leaves
    /// "unknown" for the next program.
    #[test]
    fn a_disk_that_refuses_stops_the_write() {
        let l = fresh();
        assert!(l.owned());
        l.full.store(true, Ordering::Relaxed);
        assert_eq!(l.prepare(entry(&l, "a", Kind::DeletePost, Some("gpos_1"), "p")), Err(Not::Kept));
        l.full.store(false, Ordering::Relaxed);
        l.prepare(entry(&l, "a", Kind::DeletePost, Some("gpos_1"), "p")).unwrap();
        l.full.store(true, Ordering::Relaxed);
        assert_eq!(l.dispatch("a", &[]), Err(Stop::Not(Not::Kept)));
        assert_eq!(l.get(&key(), "a").unwrap().state, State::Prepared);
        l.full.store(false, Ordering::Relaxed);
        l.dispatch("a", &[]).unwrap();
        l.end("a", State::Unknown, None, None, Beside::default());
        l.looking("a");
        l.full.store(true, Ordering::Relaxed);
        assert!(!l.close("a", Closing::Dropped, true, None));
        assert!(l.get(&key(), "a").unwrap().open(), "still held");
        l.end("a", State::Done, Some(By::Answer), None, Beside::default());
        assert_eq!(again(&l).get(&key(), "a").unwrap().state, State::Unknown);
    }

    /// J7, verify3 Minor 1: open entries are never evicted by number or age; ended and closed ones are, oldest first, and
    /// what they no longer need goes when they end.
    #[test]
    fn open_entries_are_never_evicted() {
        let l = fresh();
        for i in 0..3 * (ENDED_MAX + 20) {
            let op = format!("op-{i}");
            let mut e = entry(&l, &op, Kind::CreatePost, None, &op);
            e.evidence.before = Some(vec![known_id("gpos_1")]);
            l.prepare(e).unwrap();
            send(&l, &op);
            l.end(&op, if i % 3 == 0 { State::Done } else { State::Unknown }, (i % 3 == 0).then_some(By::Answer), None, Beside::default());
            if i % 3 == 1 {
                l.looking(&op);
                assert!(l.close(&op, Closing::Dropped, true, None));
            }
        }
        let count = |l: &Ledger, f: &dyn Fn(&Entry) -> bool| l.of(&key()).iter().filter(|e| f(e)).count();
        assert_eq!((count(&l, &|e| e.open()), count(&l, &|e| e.state == State::Done), count(&l, &|e| e.closed.is_some())), (ENDED_MAX + 20, ENDED_MAX, ENDED_MAX));
        assert_eq!(count(&l, &|e| e.ended() && e.evidence.before.is_some()), 0, "nothing kept that no check needs");
        let l = again(&l);
        l.ahead_ms.store(ENDED_FOR_MS * 2, Ordering::Relaxed);
        assert_eq!(count(&l, &|e| e.open()), ENDED_MAX + 20);
        assert_eq!(count(&l, &|e| e.ended()), 0);
    }

    /// verify3 Major 2: one program at a time; the next takes over what the first left, once it is gone.
    #[test]
    fn one_program_owns_the_record() {
        let a = fresh();
        a.prepare(entry(&a, "a", Kind::CreatePost, None, "p")).unwrap();
        let b = Ledger::at(a.dir());
        assert!(a.owned() && !b.owned());
        assert_eq!(b.prepare(entry(&b, "b", Kind::CreatePost, None, "p")), Err(Not::Owner));
        assert_eq!(b.dispatch("a", &[]), Err(Stop::Not(Not::Owner)));
        assert!(b.of(&key()).is_empty());
        assert_eq!(a.get(&key(), "a").unwrap().state, State::Prepared, "not rewritten by the other");
        a.dispatch("a", &[]).unwrap();
        a.let_go();
        assert_eq!(b.get(&key(), "a").unwrap().state, State::Unknown);
        assert_eq!(b.may_have_gone(&entry(&b, "b", Kind::CreatePost, None, "p")), Some("a".into()));
    }

    /// J9: one account's record, under another environment, Orbiters account or VRChat account, is not the other's.
    #[test]
    fn entries_are_their_accounts_own() {
        let l = fresh();
        l.prepare(entry(&l, "a", Kind::CreatePost, None, "p")).unwrap();
        for k in [Key { environment: "development".into(), ..key() }, Key { orbiters: "x".into(), ..key() }, Key { user: "usr_b".into(), ..key() }] {
            assert!(l.get(&k, "a").is_none() && l.of(&k).is_empty());
            let mut e = entry(&l, "b", Kind::CreatePost, None, "p");
            e.key = k;
            assert_eq!((l.may_have_gone(&e), l.conflicts(&e).0), (None, None));
        }
    }

    /// verify3 Major 1, deviation 7: a record that cannot be read (cut short, of a newer form, holding what contradicts
    /// itself) is left as it is and stops what it may be of, until the person looked and goes on; it is warned of after that. One
    /// plainly of another account or group stops nothing here.
    #[test]
    fn a_record_that_cannot_be_read_stops_what_it_may_be_of() {
        let text = |change: &dyn Fn(&mut serde_json::Value)| {
            let l = fresh();
            let mut e = entry(&l, "old", Kind::CreatePost, None, "p");
            (e.state, e.by, e.dispatched_at) = (State::Done, Some(By::Answer), Some(5));
            let mut v = serde_json::to_value(&e).unwrap();
            change(&mut v);
            v.to_string()
        };
        assert!(serde_json::from_str::<Entry>(&text(&|_| {})).unwrap().sound());
        let cut = text(&|_| {})[..40].to_string();
        let rows: Vec<(&str, String, bool)> = vec![
            ("cut short", cut, true),
            ("empty", String::new(), true),
            ("a newer form", text(&|v| v["v"] = serde_json::json!(FORMAT + 1)), true),
            ("a state unknown here", text(&|v| v["state"] = serde_json::json!("queued")), true),
            ("a kind unknown here", text(&|v| v["kind"] = serde_json::json!("edit_post")), true),
            ("done and never sent", text(&|v| drop(v.as_object_mut().unwrap().remove("dispatched_at"))), true),
            ("another file's name", text(&|v| v["op"] = serde_json::json!("elsewhere")), true),
            ("a newer form of another account's", text(&|v| (v["v"], v["key"]["user"]) = (serde_json::json!(99), serde_json::json!("usr_z"))), false),
            ("a newer form of another group's", text(&|v| (v["v"], v["group"]) = (serde_json::json!(99), serde_json::json!("grp_9"))), false),
        ];
        for (what, bytes, stops) in rows {
            let l = fresh();
            l.prepare(entry(&l, "sealed", Kind::CreatePost, None, "q")).unwrap();
            l.let_go();
            let path = l.dir().join("old.json");
            std::fs::write(&path, &bytes).unwrap();
            let l = Ledger::at(l.dir());
            let kept = l.prepare(entry(&l, "b", Kind::CreatePost, None, "p"));
            assert_eq!(kept, if stops { Err(Not::Read) } else { Ok(()) }, "{what}");
            assert!(l.get(&key(), "old").is_none());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes, "{what}: left as it is");
            if !stops {
                continue;
            }
            assert_eq!(l.unreadable(&key()), 1, "{what}");
            l.set_aside(&key()).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes, "{what}: still as it is");
            let e = entry(&l, "c", Kind::CreatePost, None, "p");
            assert_eq!((l.unreadable(&key()), l.conflicts(&e).0), (0, Some(Warning::RecordUnreadable)), "{what}: warned of");
            l.prepare(e).unwrap();
            assert_eq!(again(&l).conflicts(&entry(&l, "d", Kind::CreatePost, None, "p")).0, Some(Warning::RecordUnreadable), "{what}: by the next program too");
        }
        // verify4 F5: an account's look is its own: another's records, and those no one can attribute, still hold it.
        let l = fresh();
        l.prepare(entry(&l, "sealed", Kind::CreatePost, None, "q")).unwrap();
        l.let_go();
        let other = Key { user: "usr_b".into(), ..key() };
        for (name, k) in [("mine", &key()), ("theirs", &other)] {
            std::fs::write(l.dir().join(format!("{name}.json")), serde_json::json!({"v": 99, "key": k, "group": "grp_1"}).to_string()).unwrap();
        }
        std::fs::write(l.dir().join("cut.json"), "{").unwrap();
        let l = Ledger::at(l.dir());
        assert_eq!((l.unreadable(&key()), l.unreadable(&other)), (2, 2), "each its own, and the one of anybody");
        l.set_aside(&key()).unwrap();
        let own = |l: &Ledger| {
            assert_eq!((l.unreadable(&key()), l.unreadable(&other)), (0, 2), "the other decided nothing");
            assert_eq!(l.prepare(Entry { key: other.clone(), ..entry(l, "theirs-now", Kind::CreatePost, None, "p") }), Err(Not::Read));
            let warned = l.conflicts(&Entry { key: other.clone(), ..entry(l, "x", Kind::CreatePost, None, "p") }).0;
            assert_eq!((warned, l.conflicts(&entry(l, "y", Kind::CreatePost, None, "p")).0), (None, Some(Warning::RecordUnreadable)));
        };
        own(&l);
        own(&again(&l));
        // Found between the seal and the send: nothing goes.
        let l = fresh();
        l.prepare(entry(&l, "a", Kind::CreatePost, None, "p")).unwrap();
        l.kept.lock().unwrap().unreadable.push(l.dir().join("later.json"));
        assert_eq!(l.dispatch("a", &[]), Err(Stop::Not(Not::Read)));
        // Readable again (it was held for a moment): taken in, and nothing is stopped.
        let l = fresh();
        l.prepare(entry(&l, "a", Kind::CreatePost, None, "p")).unwrap();
        lost(&l, "a");
        l.let_go();
        let whole = std::fs::read(l.dir().join("a.json")).unwrap();
        std::fs::write(l.dir().join("a.json"), b"{").unwrap();
        let l = Ledger::at(l.dir());
        assert_eq!(l.unreadable(&key()), 1);
        std::fs::write(l.dir().join("a.json"), whole).unwrap();
        assert_eq!((l.unreadable(&key()), l.get(&key(), "a").unwrap().state), (0, State::Unknown));
    }
}
