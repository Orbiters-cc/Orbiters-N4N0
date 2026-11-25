//! What was last read from VRChat, kept on this computer (REQUEST28: "when I open Orbiters N4N0, I would like the list of
//! friend to have the exact same state it had last time I closed"): one store for every page, so that each shows its last
//! state at once and changes in place when VRChat answers.
//!
//! - **One folder an account**, `<cache>/vrchat/<digest>/`, the digest of the Orbiters member (itself a digest of the
//!   environment and the member's id) and the VRChat account's id ([`Account`]). Nothing is looked up any other way:
//!   another member, environment or VRChat account has another folder, which is never opened for this one. The folder is
//!   there for the store only while its `account.json` names that very account.
//! - **`state.sqlite3`**: one record a thing (a friend, a group's page, a world), with when it was read and what it was
//!   read by. Only what changed is written, in one transaction ([`Store::write`]). The tray service alone writes it; the
//!   window reads it ([`Store::read`]).
//! - **`pictures/`**: the pictures ([`Pictures`]).
//! - A store this version cannot read is made anew; one of a newer version is left alone and not used. Neither is an error.
//! - Signing out of VRChat removes the account's folder ([`forget`]), its pictures with it; one not used for
//!   [`UNUSED_DAYS`] goes at a start ([`sweep`]). It holds what the friends list shows (names, ids, statuses, places, the
//!   account's notes), the groups' pages and pictures: no cookie, no password, no token.
//!
//! Nothing here talks to the network.

use crate::launcher::{Friend, Game, LauncherState, Source, VerifiedFor};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// The records' form. Another number than this one is not read; a larger one is not touched either.
const VERSION: i64 = 1;
/// An account's folder not used for this long goes.
pub const UNUSED_DAYS: u64 = 90;
/// A world's name and picture not used for this long go, and how many are kept.
const WORLD_DAYS: i64 = 60;
const WORLDS_MAX: i64 = 600;
const FILE: &str = "state.sqlite3";
const MARK: &str = "account.json";
/// The start of a folder's name while it is being removed.
const GONE: &str = "gone-";
/// The folders a sign-out could not remove yet, a name a line.
const LEFT: &str = "to-remove.txt";

/// The kinds of record. Groups' pages keep theirs by the group's id.
pub mod kind {
    /// The friends list without the friends (`list`), and each friend by their id.
    pub const LIST: &str = "list";
    pub const FRIEND: &str = "friend";
    /// A world's name and picture, an inventory item's picture.
    pub const WORLD: &str = "world";
    pub const DECORATION: &str = "decoration";
    /// The groups the account runs (`list`), a group's page, its first members, its first posts, its events.
    pub const GROUPS: &str = "groups";
    pub const GROUP: &str = "group";
    pub const MEMBERS: &str = "members";
    pub const POSTS: &str = "posts";
    pub const EVENTS: &str = "events";
    /// What Orbiters says of a group's events.
    pub const TOLD: &str = "events-on-orbiters";
    /// When the list was last live.
    pub const SEEN: &str = "seen";
}
/// The id of a record there is one of.
pub const ONLY: &str = "list";

/// An Orbiters member's VRChat account: whose a folder is.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Account {
    /// The digest of the environment and the member's id (the service's `Owner`).
    member: String,
    /// `usr_…` (or an old account's plain id).
    user: String,
}

impl Account {
    pub fn of(member: &str, user: &str) -> Option<Self> {
        let hex = member.len() == 32 && member.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        let id = !user.is_empty() && user.len() <= 64 && user.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'));
        (hex && id).then(|| Self { member: member.into(), user: user.into() })
    }
    pub fn verified(v: &VerifiedFor) -> Option<Self> {
        Self::of(&v.member, v.user.as_deref()?)
    }
    pub fn user(&self) -> &str {
        &self.user
    }
    fn folder(&self) -> String {
        let whose = format!("vrchat store\n{}\n{}", self.member, self.user); // i18n: not shown (a folder's name)
        Sha256::digest(whose.as_bytes()).iter().take(16).map(|b| format!("{b:02x}")).collect()
    }
    pub fn dir(&self, root: &Path) -> PathBuf {
        root.join(self.folder())
    }
}

/// Where the accounts' folders are.
pub fn root() -> PathBuf {
    crate::paths::cache_dir().join("vrchat")
}

/// Whether the account has a folder here: it is named in it.
pub fn there(root: &Path, account: &Account) -> bool {
    marked(&account.dir(root)).as_ref() == Some(account)
}

fn marked(dir: &Path) -> Option<Account> {
    serde_json::from_slice(&std::fs::read(dir.join(MARK)).ok()?).ok()
}

/// The account's folder made (the service, once VRChat said who is signed in), or marked used.
pub fn begin(root: &Path, account: &Account) -> bool {
    let dir = account.dir(root);
    let mark = dir.join(MARK);
    if there(root, account) {
        if let Ok(file) = std::fs::File::options().write(true).open(&mark) {
            let _ = file.set_modified(SystemTime::now());
        }
        return true;
    }
    let Ok(bytes) = serde_json::to_vec(account) else { return false };
    std::fs::create_dir_all(&dir).is_ok() && crate::write_atomic(&mark, &bytes).is_ok()
}

/// Every folder of the VRChat account `user`, whichever Orbiters member's (`None`: of every account): its data and its
/// pictures. Whether it is all gone. What could not be removed is written down first and removed at a next start ([`sweep`]),
/// and is nobody's as soon as its mark is gone.
pub fn forget(root: &Path, user: Option<&str>) -> bool {
    let list = root.join(LEFT);
    let left: Vec<String> = std::fs::read_to_string(&list).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default();
    let of: Vec<PathBuf> = folders(root).into_iter().filter(|d| marked(d).is_some_and(|a| user.is_none_or(|u| u == a.user))).collect();
    if of.is_empty() {
        return true;
    }
    let names: Vec<String> = of.iter().filter_map(|d| Some(d.file_name()?.to_str()?.to_string())).chain(left.iter().cloned()).collect();
    let _ = crate::write_atomic(&list, names.join("
").as_bytes());
    let mut gone = true;
    for dir in &of {
        gone &= remove(root, dir);
    }
    if gone && left.is_empty() {
        let _ = std::fs::remove_file(&list);
    }
    gone
}

fn folders(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root).map(|e| e.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default()
}

/// Its mark first: whoever was about to write a picture in it finds no folder of theirs. Then out of its place at once (a
/// file of it being read refuses that for a moment), then its files.
fn remove(root: &Path, dir: &Path) -> bool {
    let unmarked = || std::fs::remove_file(dir.join(MARK)).is_ok() || !dir.join(MARK).exists();
    for again in 0..10 {
        if again > 0 {
            std::thread::sleep(Duration::from_millis(40));
        }
        if !unmarked() {
            continue;
        }
        let out = root.join(format!("{GONE}{}", uuid::Uuid::new_v4()));
        let at = if std::fs::rename(dir, &out).is_ok() { out } else { dir.to_path_buf() };
        if std::fs::remove_dir_all(&at).is_ok() || !at.exists() {
            return true;
        }
        if at != dir {
            // Out of its place: the rest goes at a next start.
            return false;
        }
    }
    false
}

/// At a start: what a removal left, folders that are nobody's, and those not used for [`UNUSED_DAYS`].
pub fn sweep(root: &Path) {
    let list = root.join(LEFT);
    if let Ok(text) = std::fs::read_to_string(&list) {
        // (A plain name of this folder alone.)
        if text.lines().filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_hexdigit())).fold(true, |gone, n| gone & (!root.join(n).exists() || remove(root, &root.join(n)))) {
            let _ = std::fs::remove_file(&list);
        }
    }
    for dir in folders(root) {
        let unused = || {
            let used = std::fs::metadata(dir.join(MARK)).and_then(|m| m.modified()).ok()?;
            SystemTime::now().duration_since(used).ok()
        };
        let left = dir.file_name().is_some_and(|n| n.to_string_lossy().starts_with(GONE));
        let nobodys = marked(&dir).is_none_or(|a| a.dir(root) != dir);
        if left || nobodys || unused().is_some_and(|age| age > Duration::from_secs(UNUSED_DAYS * 86_400)) {
            remove(root, &dir);
        }
    }
}

/// How much the accounts' folders take.
pub fn size(root: &Path) -> u64 {
    fn of(dir: &Path) -> u64 {
        std::fs::read_dir(dir).map(|e| e.flatten().map(|e| match e.metadata() { Ok(m) if m.is_dir() => of(&e.path()), Ok(m) => m.len(), Err(_) => 0 }).sum()).unwrap_or(0)
    }
    of(root)
}

/// A record: when it was read (Unix seconds), and its value.
#[derive(Clone, Debug, PartialEq)]
pub struct Record<T> {
    pub at: i64,
    pub value: T,
}

/// What a write changes.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// `source`: the request it was read by.
    Put { kind: &'static str, id: String, at: i64, source: &'static str, data: String },
    Drop { kind: &'static str, id: String },
    /// Every record of this kind whose id is not one of these.
    Only { kind: &'static str, ids: Vec<String> },
}

impl Change {
    pub fn put<T: Serialize>(kind: &'static str, id: &str, at: i64, source: &'static str, value: &T) -> Option<Self> {
        Some(Change::Put { kind, id: id.into(), at, source, data: serde_json::to_string(value).ok()? })
    }
}

/// An account's records.
pub struct Store {
    db: Connection,
    account: Account,
}

impl Store {
    /// For writing: made when there is none, made anew when it cannot be read; none when it is of a newer version or the
    /// account has no folder ([`begin`]).
    pub fn open(root: &Path, account: &Account) -> Option<Self> {
        if !there(root, account) {
            return None;
        }
        let file = account.dir(root).join(FILE);
        match Self::prepared(&file, true) {
            Ok(Some(db)) => return Some(Self { db, account: account.clone() }),
            Ok(None) => return None,
            Err(()) => {}
        }
        for end in ["", "-wal", "-shm", "-journal"] {
            let mut name = file.clone().into_os_string();
            name.push(end);
            let _ = std::fs::remove_file(name);
        }
        Self::prepared(&file, true).ok().flatten().map(|db| Self { db, account: account.clone() })
    }

    /// For reading only, as another process has it; none when there is none, or it cannot be read.
    pub fn read(root: &Path, account: &Account) -> Option<Self> {
        let file = account.dir(root).join(FILE);
        if !there(root, account) || !file.is_file() {
            return None;
        }
        Self::prepared(&file, false).ok().flatten().map(|db| Self { db, account: account.clone() })
    }

    /// `Ok(None)`: a newer version's (or, for reading, an unfinished one). `Err`: not a store that can be read.
    fn prepared(file: &Path, write: bool) -> Result<Option<Connection>, ()> {
        let open = |e: rusqlite::Error| {
            if cfg!(debug_assertions) {
                eprintln!("the kept VRChat state could not be opened: {e}"); // i18n: not shown (a developer's log line)
            }
        };
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX | if write { OpenFlags::SQLITE_OPEN_CREATE } else { OpenFlags::empty() };
        let db = Connection::open_with_flags(file, flags).map_err(open)?;
        let _ = db.busy_timeout(Duration::from_millis(if write { 3000 } else { 60 }));
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(open)?; // i18n: not shown (SQL)
        if version > VERSION {
            return Ok(None);
        }
        if write {
            // Kept state, not the person's work: no flush of the disk at each commit.
            db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA secure_delete=ON;").map_err(open)?; // i18n: not shown (SQL)
            if version < VERSION {
                db.execute_batch(&format!(
                    "BEGIN IMMEDIATE; DROP TABLE IF EXISTS records; \
                     CREATE TABLE records (kind TEXT NOT NULL, id TEXT NOT NULL, at INTEGER NOT NULL, source TEXT NOT NULL, data TEXT NOT NULL, PRIMARY KEY (kind, id)) WITHOUT ROWID; \
                     PRAGMA user_version={VERSION}; COMMIT;" // i18n: not shown (SQL)
                ))
                .map_err(open)?;
            }
        } else if version != VERSION {
            return Ok(None);
        }
        // A file damaged past its first page says so here.
        db.query_row("SELECT count(*) FROM records", [], |r| r.get::<_, i64>(0)).map_err(open)?; // i18n: not shown (SQL)
        Ok(Some(db))
    }

    pub fn account(&self) -> &Account {
        &self.account
    }

    /// One record, when it is there and readable.
    pub fn get<T: DeserializeOwned>(&self, kind: &str, id: &str) -> Option<Record<T>> {
        let (at, data): (i64, String) = self
            .db
            .query_row("SELECT at, data FROM records WHERE kind = ?1 AND id = ?2", params![kind, id], |r| Ok((r.get(0)?, r.get(1)?))) // i18n: not shown (SQL)
            .optional()
            .ok()??;
        Some(Record { at, value: serde_json::from_str(&data).ok()? })
    }

    /// Every record of a kind that can be read, by id.
    pub fn all<T: DeserializeOwned>(&self, kind: &str) -> Vec<(String, Record<T>)> {
        let Ok(mut q) = self.db.prepare_cached("SELECT id, at, data FROM records WHERE kind = ?1 ORDER BY id") else { return Vec::new() }; // i18n: not shown (SQL)
        let Ok(rows) = q.query_map(params![kind], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))) else { return Vec::new() };
        rows.flatten().filter_map(|(id, at, data)| Some((id, Record { at, value: serde_json::from_str(&data).ok()? }))).collect()
    }

    /// All of `changes` or none. Whether they are written.
    pub fn write(&mut self, changes: &[Change]) -> bool {
        if changes.is_empty() {
            return true;
        }
        let write = |db: &mut Connection| -> rusqlite::Result<()> {
            let t = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            for c in changes {
                match c {
                    Change::Put { kind, id, at, source, data } => {
                        t.prepare_cached("INSERT OR REPLACE INTO records (kind, id, at, source, data) VALUES (?1, ?2, ?3, ?4, ?5)")?.execute(params![kind, id, at, source, data])?; // i18n: not shown (SQL)
                    }
                    Change::Drop { kind, id } => {
                        t.prepare_cached("DELETE FROM records WHERE kind = ?1 AND id = ?2")?.execute(params![kind, id])?; // i18n: not shown (SQL)
                    }
                    Change::Only { kind, ids } => {
                        let keep: HashSet<&str> = ids.iter().map(String::as_str).collect();
                        let there: Vec<String> = {
                            let mut q = t.prepare_cached("SELECT id FROM records WHERE kind = ?1")?; // i18n: not shown (SQL)
                            let rows = q.query_map(params![kind], |r| r.get(0))?;
                            rows.collect::<rusqlite::Result<_>>()?
                        };
                        for id in there.iter().filter(|id| !keep.contains(id.as_str())) {
                            t.prepare_cached("DELETE FROM records WHERE kind = ?1 AND id = ?2")?.execute(params![kind, id])?; // i18n: not shown (SQL)
                        }
                    }
                }
            }
            t.commit()
        };
        write(&mut self.db).is_ok()
    }

    /// What grew old: worlds not used for long, or too many of them. The write-ahead log goes into the file.
    pub fn tidy(&mut self, now: i64) {
        let _ = self.db.execute("DELETE FROM records WHERE kind = ?1 AND at < ?2", params![kind::WORLD, now - WORLD_DAYS * 86_400]); // i18n: not shown (SQL)
        let _ = self.db.execute("DELETE FROM records WHERE kind = ?1 AND id NOT IN (SELECT id FROM records WHERE kind = ?1 ORDER BY at DESC LIMIT ?2)", params![kind::WORLD, WORLDS_MAX]); // i18n: not shown (SQL)
        let _ = self.db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())); // i18n: not shown (SQL)
    }

    /// The friends list as it was last on show, when it can be read: marked [`LauncherState::kept`] with when it was last live.
    pub fn launcher(&self) -> Option<LauncherState> {
        let head = self.get::<LauncherState>(kind::LIST, ONLY)?;
        let mut state = head.value;
        if !matches!(&state.source, Source::SignedIn { user_id, .. } if *user_id == self.account.user) {
            return None;
        }
        state.friends = self.all::<Friend>(kind::FRIEND).into_iter().map(|(_, r)| r.value).collect();
        // A friend who could not be read leaves the list whole.
        let there: HashSet<&str> = state.friends.iter().map(|f| f.id.as_str()).collect();
        for g in &mut state.groups {
            g.members.retain(|id| there.contains(id.as_str()));
        }
        state.kept = Some(self.get::<i64>(kind::SEEN, ONLY).map_or(head.at, |r| r.value.max(head.at)));
        Some(state)
    }

    /// The names and pictures of worlds kept, with when each was read.
    pub fn worlds(&self) -> HashMap<String, Record<World>> {
        self.all::<World>(kind::WORLD).into_iter().collect()
    }

    /// The inventory items' pictures kept.
    pub fn decorations(&self) -> HashMap<String, String> {
        self.all::<String>(kind::DECORATION).into_iter().map(|(id, r)| (id, r.value)).collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct World {
    pub name: String,
    pub picture: String,
}

/// The list without its friends and without what is not the account's or is of the moment: what this computer says (the
/// game, its profiles, the engine), the notice, an open profile, what is being saved.
pub fn head(list: &LauncherState) -> LauncherState {
    LauncherState {
        source: list.source.clone(),
        possible: list.possible,
        me: list.me.clone(),
        groups: list.groups.clone(),
        invites: list.invites.clone(),
        boops: list.boops.clone(),
        boop_emojis: list.boop_emojis.clone(),
        // Not `loaded`: it shows, is read again, and nothing is saved over the account's from it.
        mine: list.mine.clone().map(|m| crate::launcher::MyProfile { loaded: false, listing: None, list_failed: None, saving: None, next_change_at: None, answered: 0, ..m }).filter(|m| *m != Default::default()),
        game: Game::default(),
        ..LauncherState::default()
    }
}

/// What changed of the list since `before` (as it was last written), to write.
pub fn launcher_changes(before: Option<&LauncherState>, now: &LauncherState, at: i64) -> Vec<Change> {
    let mut out = Vec::new();
    let list = head(now);
    if before.is_none_or(|b| head(b) != list) {
        out.extend(Change::put(kind::LIST, ONLY, at, "friends list", &list)); // i18n: not shown (a record's words)
    }
    let had: HashMap<&str, &Friend> = before.map(|b| b.friends.iter().map(|f| (f.id.as_str(), f)).collect()).unwrap_or_default();
    for f in now.friends.iter().filter(|f| had.get(f.id.as_str()).is_none_or(|b| *b != *f)) {
        out.extend(Change::put(kind::FRIEND, &f.id, at, "friends list", f)); // i18n: not shown (a record's words)
    }
    let there: HashSet<&str> = now.friends.iter().map(|f| f.id.as_str()).collect();
    if before.is_none() {
        out.push(Change::Only { kind: kind::FRIEND, ids: there.iter().map(|id| id.to_string()).collect() });
    } else {
        out.extend(had.keys().filter(|id| !there.contains(**id)).map(|id| Change::Drop { kind: kind::FRIEND, id: id.to_string() }));
    }
    out
}

/// A size a picture is drawn at: its small copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Size {
    /// A face, an icon: 128 pixels at most on its longer side.
    Face,
    /// A banner, a cover: 640.
    Wide,
}

impl Size {
    pub fn pixels(self) -> u32 {
        match self {
            Size::Face => 128,
            Size::Wide => 640,
        }
    }
    fn end(self) -> &'static str {
        match self {
            Size::Face => "s",
            Size::Wide => "w",
        }
    }
}

/// A picture kept for a person: the one their address names, or the one before it.
#[derive(Clone, Debug, PartialEq)]
pub enum Kept<T> {
    Exact(T),
    /// The person's picture before (shown until the new one arrived).
    Older(T),
    Nothing,
}

const MAGIC: &[u8; 8] = b"ORBVPI1\n";
/// The folder is measured at most this often.
const MEASURE_EVERY: Duration = Duration::from_secs(20);

/// VRChat's pictures kept on this computer: a file a picture and size, ready to be shown (small, quick to decode), and the
/// bytes VRChat sent beside it. Named by the VRChat file and version ([`picture_key`]): a signed or another size's address
/// of the same picture is the same file, and never downloaded again (a version of a VRChat file never changes). Past the
/// size allowed, the files used longest ago go. Every read and write is of files alone, safe across processes: each is
/// written whole beside its place, then put there.
pub struct Pictures {
    dir: PathBuf,
    /// The account's mark: nothing is written once it is gone (signed out).
    mark: Option<(PathBuf, Account)>,
    cap: Mutex<u64>,
    touched: Mutex<HashSet<PathBuf>>,
    measured: Mutex<Option<Instant>>,
}

/// What names a picture: its VRChat file and version (the same picture at any size), or a digest of another address.
pub fn picture_key(url: &str) -> String {
    match crate::picture_sources::vrchat_file(url) {
        Some((file, version)) => format!("{file}/{version}"),
        None => format!("url-{}", hex(url)),
    }
}

fn hex(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().take(12).map(|b| format!("{b:02x}")).collect()
}

impl Pictures {
    /// An account's.
    pub fn of(root: &Path, account: &Account, cap: u64) -> Self {
        Self { mark: Some((root.to_path_buf(), account.clone())), ..Self::at(account.dir(root).join("pictures"), cap) }
    }
    /// A folder that is no account's (this computer's own copy of VRCX's or VRCNext's pictures, tests).
    pub fn at(dir: PathBuf, cap: u64) -> Self {
        Self { dir, mark: None, cap: Mutex::new(cap), touched: Mutex::default(), measured: Mutex::default() }
    }
    pub fn set_cap(&self, cap: u64) {
        *self.cap.lock().unwrap_or_else(|e| e.into_inner()) = cap;
    }
    fn writable(&self) -> bool {
        self.mark.as_ref().is_none_or(|(root, account)| there(root, account))
    }
    fn base(&self, key: &str) -> PathBuf {
        let name = hex(key);
        self.dir.join(&name[..2]).join(name)
    }
    fn small(&self, key: &str, size: Size) -> PathBuf {
        self.base(key).with_extension(size.end())
    }
    fn original(&self, key: &str) -> PathBuf {
        self.base(key).with_extension("o")
    }
    /// The person's picture kept last: which it is.
    fn person(&self, user: &str) -> PathBuf {
        self.dir.join("people").join(hex(user))
    }
    fn read_file(path: &Path, key: &str) -> Option<Vec<u8>> {
        let bytes = std::fs::read(path).ok()?;
        let rest = bytes.strip_prefix(&MAGIC[..])?;
        let n = u16::from_le_bytes([*rest.first()?, *rest.get(1)?]) as usize;
        // Another picture under the same name (never, but for a file of another's making).
        (rest.get(2..2 + n)? == key.as_bytes()).then(|| rest[2 + n..].to_vec())
    }
    fn write_file(path: &Path, key: &str, body: &[u8]) -> bool {
        let mut bytes = Vec::with_capacity(body.len() + key.len() + 10);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(key.len() as u16).to_le_bytes());
        bytes.extend_from_slice(key.as_bytes());
        bytes.extend_from_slice(body);
        path.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && crate::write_atomic(path, &bytes).is_ok()
    }

    /// The small copy of the picture at `url` (for `user`, a person's id, theirs before it when it is not kept), decoded by
    /// `decode`; a file that does not decode is made again.
    pub fn small_copy<T>(&self, user: Option<&str>, url: &str, size: Size, decode: impl Fn(&[u8]) -> Option<T>) -> Kept<T> {
        let key = picture_key(url);
        let read = |key: &str| {
            let path = self.small(key, size);
            let body = Self::read_file(&path, key)?;
            match decode(&body) {
                Some(t) => {
                    self.touch(&path);
                    Some(t)
                }
                None => {
                    let _ = std::fs::remove_file(&path);
                    None
                }
            }
        };
        if let Some(t) = read(&key) {
            return Kept::Exact(t);
        }
        let before = user.and_then(|u| std::fs::read_to_string(self.person(u)).ok()).filter(|k| *k != key);
        before.and_then(|k| read(&k)).map_or(Kept::Nothing, Kept::Older)
    }

    /// Whether exactly that picture is kept at that size.
    pub fn has(&self, url: &str, size: Size) -> bool {
        self.small(&picture_key(url), size).is_file()
    }

    /// The bytes VRChat sent, when kept.
    pub fn original_copy(&self, url: &str) -> Option<Vec<u8>> {
        let key = picture_key(url);
        Self::read_file(&self.original(&key), &key)
    }

    /// Keeps a picture: `small` (as it is drawn at `size`) and what VRChat sent, when it is worth keeping; it is `user`'s
    /// picture from now on.
    pub fn put(&self, user: Option<&str>, url: &str, size: Size, small: &[u8], original: Option<&[u8]>) -> bool {
        if !self.writable() {
            return false;
        }
        let key = picture_key(url);
        let path = self.small(&key, size);
        if !Self::write_file(&path, &key, small) {
            return false;
        }
        if let Some(bytes) = original {
            Self::write_file(&self.original(&key), &key, bytes);
        }
        if let Some(user) = user {
            let of = self.person(user);
            // (The one before may be another person's too, a shared avatar's: it goes with what was used longest ago.)
            if std::fs::read_to_string(&of).ok().as_deref() != Some(key.as_str()) && of.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) {
                let _ = crate::write_atomic(&of, key.as_bytes());
            }
        }
        self.touched.lock().unwrap_or_else(|e| e.into_inner()).insert(path);
        let due = {
            let mut measured = self.measured.lock().unwrap_or_else(|e| e.into_inner());
            let due = measured.is_none_or(|at| at.elapsed() >= MEASURE_EVERY);
            if due {
                *measured = Some(Instant::now());
            }
            due
        };
        if due {
            self.evict();
        }
        true
    }

    /// One shown now is used: it goes last (once a run a file).
    fn touch(&self, path: &Path) {
        if self.touched.lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_path_buf()) {
            if let Ok(file) = std::fs::File::options().write(true).open(path) {
                let _ = file.set_modified(SystemTime::now());
            }
        }
    }

    /// Past the size allowed: what VRChat sent goes first (the small copies are what is shown), then the small copies, each
    /// the ones used longest ago, until a quarter is free; and what an interrupted write left over an hour ago.
    pub fn evict(&self) {
        let cap = *self.cap.lock().unwrap_or_else(|e| e.into_inner());
        let mut files: Vec<(bool, SystemTime, u64, PathBuf)> = Vec::new();
        for folder in folders(&self.dir) {
            let people = folder.file_name().is_some_and(|n| n == "people");
            for e in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
                let Ok(meta) = e.metadata() else { continue };
                let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                let path = e.path();
                match path.extension().and_then(|x| x.to_str()) {
                    Some("tmp") => {
                        if SystemTime::now().duration_since(modified).unwrap_or_default() > Duration::from_secs(3600) {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                    _ if people => {}
                    end => files.push((end != Some("o"), modified, meta.len(), path)),
                }
            }
        }
        let mut total: u64 = files.iter().map(|f| f.2).sum();
        if total <= cap {
            return;
        }
        files.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        for (_, _, size, path) in files {
            if total <= cap / 4 * 3 {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total -= size;
                self.touched.lock().unwrap_or_else(|e| e.into_inner()).remove(&path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::{FriendGroup, GroupKind, Presence, Whereabouts};

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let dir = crate::paths::cache_dir().join(format!("vrchat-store-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const MEMBER: &str = "0123456789abcdef0123456789abcdef";
    const OTHER: &str = "fedcba9876543210fedcba9876543210";
    fn usr(n: u32) -> String {
        format!("usr_00000000-0000-4000-8000-{n:012}")
    }
    fn account(member: &str, n: u32) -> Account {
        Account::of(member, &usr(n)).unwrap()
    }

    /// A made-up account's list of `n` friends.
    pub(super) fn list(n: u32) -> LauncherState {
        let mut s = crate::launcher::sample();
        let of = s.friends[0].clone();
        s.source = Source::SignedIn { user_id: usr(1), name: "Tester".into() };
        s.friends = (0..n).map(|i| Friend { id: usr(100 + i), name: format!("Friend {i}"), status_text: format!("status {i} — ここ"), note: format!("note {i}"), ..of.clone() }).collect();
        s.groups = vec![FriendGroup { id: "in-game".into(), name: String::new(), kind: GroupKind::InGame, members: s.friends.iter().map(|f| f.id.clone()).collect() }];
        s
    }

    #[test]
    fn the_list_comes_back_as_it_was_and_only_what_changed_is_written() {
        let t = Temp::new();
        let a = account(MEMBER, 1);
        assert!(Store::open(&t.0, &a).is_none(), "no folder before VRChat said who is signed in");
        assert!(begin(&t.0, &a));
        let mut s = list(5);
        let changes = launcher_changes(None, &s, 1000);
        assert_eq!(changes.len(), 1 + 5 + 1);
        let mut store = Store::open(&t.0, &a).unwrap();
        assert!(store.write(&changes) && launcher_changes(Some(&s), &s, 1001).is_empty(), "nothing changed: nothing to write");
        // While the service has it open, the window reads it.
        let read = Store::read(&t.0, &a).unwrap().launcher().unwrap();
        assert_eq!(read.friends, s.friends);
        assert_eq!((&read.groups, &read.invites, &read.boops, &read.me), (&s.groups, &s.invites, &s.boops, &s.me));
        assert_eq!((read.kept, read.notice, read.profile), (Some(1000), None, None));
        let before = s.clone();
        s.friends[2].presence = Presence::Offline;
        s.friends[2].whereabouts = Whereabouts::Offline;
        let gone = s.friends.remove(4).id;
        s.groups[0].members.retain(|m| *m != gone);
        let changes = launcher_changes(Some(&before), &s, 1100);
        assert_eq!(changes.len(), 3, "the head (its groups), the friend who changed, the one who left");
        assert!(store.write(&changes));
        drop(store);
        let read = Store::open(&t.0, &a).unwrap().launcher().unwrap();
        assert_eq!((read.friends, read.groups), (s.friends, s.groups));
    }

    #[test]
    fn an_account_is_never_anothers() {
        let t = Temp::new();
        let (a, member, user) = (account(MEMBER, 1), account(OTHER, 1), account(MEMBER, 2));
        begin(&t.0, &a);
        Store::open(&t.0, &a).unwrap().write(&launcher_changes(None, &list(2), 1));
        for other in [&member, &user] {
            assert!(!there(&t.0, other) && Store::read(&t.0, other).is_none() && Store::open(&t.0, other).is_none());
        }
        // Another's folder with this one's records in it (copied by hand) is not read as theirs.
        begin(&t.0, &user);
        std::fs::copy(a.dir(&t.0).join(FILE), user.dir(&t.0).join(FILE)).unwrap();
        assert!(Store::open(&t.0, &user).unwrap().launcher().is_none());
        // And a folder whose mark names another is nobody's.
        std::fs::copy(a.dir(&t.0).join(MARK), user.dir(&t.0).join(MARK)).unwrap();
        assert!(Store::read(&t.0, &user).is_none());
        assert!(Account::of("x", &usr(1)).is_none() && Account::of(MEMBER, "").is_none() && Account::of(MEMBER, "../x").is_none());
    }

    #[test]
    fn signing_out_removes_the_account_everywhere_and_nothing_is_written_after() {
        let t = Temp::new();
        let (a, dev, other) = (account(MEMBER, 1), account(OTHER, 1), account(MEMBER, 2));
        for x in [&a, &dev, &other] {
            begin(&t.0, x);
            Store::open(&t.0, x).unwrap().write(&launcher_changes(None, &list(2), 1));
        }
        let pictures = Pictures::of(&t.0, &a, 1 << 20);
        let url = "https://api.vrchat.cloud/api/1/file/file_00000000-0000-4000-8000-000000000001/1/file";
        assert!(pictures.put(Some(&usr(5)), url, Size::Face, b"small", Some(b"all of it")));
        assert!(forget(&t.0, Some(&usr(1))));
        assert!(!a.dir(&t.0).exists() && !dev.dir(&t.0).exists() && there(&t.0, &other));
        assert!(!pictures.put(None, url, Size::Face, b"small", None) && !a.dir(&t.0).exists(), "a picture that arrives late is not kept");
        assert!(forget(&t.0, Some(&usr(1))), "nothing left to forget");
        // An interrupted one is finished at the next start, and what could not say whose session it was forgets everyone's.
        std::fs::write(t.0.join(LEFT), other.folder()).unwrap();
        sweep(&t.0);
        assert!(!there(&t.0, &other) && !t.0.join(LEFT).exists());
        begin(&t.0, &other);
        assert!(forget(&t.0, None) && !there(&t.0, &other));
    }

    #[test]
    fn a_store_that_cannot_be_read_is_made_anew_and_a_newer_one_left_alone() {
        let t = Temp::new();
        let a = account(MEMBER, 1);
        begin(&t.0, &a);
        let file = a.dir(&t.0).join(FILE);
        for broken in [&b"not a database at all, and long enough to be taken for one by its length alone"[..], &[0u8; 0], &[0xFFu8; 8192]] {
            for end in ["-wal", "-shm"] {
                let _ = std::fs::remove_file(a.dir(&t.0).join(format!("{FILE}{end}")));
            }
            std::fs::write(&file, broken).unwrap();
            assert!(Store::read(&t.0, &a).is_none_or(|s| s.launcher().is_none()));
            let mut store = Store::open(&t.0, &a).expect("made anew");
            assert!(store.launcher().is_none() && store.write(&launcher_changes(None, &list(3), 5)));
            assert_eq!(store.launcher().unwrap().friends.len(), 3);
        }
        // Half of it gone (cut short): never an error, and it can be written again.
        {
            let mut store = Store::open(&t.0, &a).unwrap();
            store.write(&launcher_changes(None, &list(300), 5));
            store.tidy(10);
        }
        let whole = std::fs::read(&file).unwrap();
        std::fs::write(&file, &whole[..whole.len() / 2]).unwrap();
        let _ = Store::read(&t.0, &a).map(|s| s.launcher());
        let mut store = Store::open(&t.0, &a).expect("readable or made anew");
        let _ = store.launcher();
        // A record that does not read is left out; the rest shows.
        drop(store);
        let _ = std::fs::remove_file(&file);
        store = Store::open(&t.0, &a).unwrap();
        store.write(&launcher_changes(None, &list(3), 5));
        store.write(&[Change::Put { kind: kind::FRIEND, id: usr(101), at: 5, source: "", data: "{\"id\":".into() }]);
        let read = store.launcher().unwrap();
        assert_eq!((read.friends.len(), read.groups[0].members.len()), (2, 2));
        drop(store);
        // A newer version's is not read and not touched.
        Connection::open(&file).unwrap().execute_batch("PRAGMA user_version=99").unwrap();
        assert!(Store::open(&t.0, &a).is_none() && Store::read(&t.0, &a).is_none());
        assert_eq!(Connection::open(&file).unwrap().query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 99);
    }

    #[test]
    fn what_is_left_or_unused_goes_at_a_start() {
        let t = Temp::new();
        let (a, old) = (account(MEMBER, 1), account(MEMBER, 2));
        begin(&t.0, &a);
        begin(&t.0, &old);
        std::fs::File::options().write(true).open(old.dir(&t.0).join(MARK)).unwrap().set_modified(SystemTime::now() - Duration::from_secs((UNUSED_DAYS + 1) * 86_400)).unwrap();
        std::fs::create_dir_all(t.0.join(format!("{GONE}x/pictures"))).unwrap();
        std::fs::create_dir_all(t.0.join("nobodys")).unwrap();
        sweep(&t.0);
        assert_eq!(folders(&t.0), vec![a.dir(&t.0)]);
    }

    #[test]
    fn a_picture_is_its_file_and_version_and_a_persons_before_shows_meanwhile() {
        let t = Temp::new();
        let p = Pictures::at(t.0.clone(), 1 << 20);
        let url = |file: u32, v: u32, rest: &str| format!("https://api.vrchat.cloud/api/1/{rest}file_00000000-0000-4000-8000-{file:012}/{v}/file?sig={v}x");
        let text = |b: &[u8]| String::from_utf8(b.to_vec()).ok();
        let who = usr(7);
        assert_eq!(p.small_copy(Some(&who), &url(1, 1, "file/"), Size::Face, text), Kept::Nothing);
        assert!(p.put(Some(&who), &url(1, 1, "file/"), Size::Face, b"one", Some(b"one whole")));
        let signed = url(1, 1, "image/").replace("?sig=1x", "?another=signature");
        assert_eq!(p.small_copy(None, &signed, Size::Face, text), Kept::Exact("one".into()), "another address of the same picture");
        assert_eq!(p.original_copy(&signed).as_deref(), Some(&b"one whole"[..]));
        assert_eq!(p.small_copy(Some(&who), &url(1, 1, "file/"), Size::Wide, text), Kept::Nothing, "another size is another copy");
        assert_eq!(p.small_copy(Some(&who), &url(1, 2, "file/"), Size::Face, text), Kept::Older("one".into()));
        assert!(p.put(Some(&who), &url(1, 2, "file/"), Size::Face, b"two", None));
        assert_eq!(p.small_copy(Some(&who), &url(1, 2, "file/"), Size::Face, text), Kept::Exact("two".into()));
        assert!(p.has(&url(1, 1, "file/"), Size::Face), "the one before may be another person's too");
        // One that does not decode is made again.
        assert_eq!(p.small_copy(None, &url(1, 2, "file/"), Size::Face, |_| None::<()>), Kept::Nothing);
        assert!(!p.has(&url(1, 2, "file/"), Size::Face));
    }

    #[test]
    fn past_the_size_allowed_what_was_sent_goes_first_then_what_was_used_longest_ago() {
        let t = Temp::new();
        let p = Pictures::at(t.0.clone(), u64::MAX);
        let url = |n: u32| format!("https://api.vrchat.cloud/api/1/file/file_00000000-0000-4000-8000-{n:012}/1/file");
        for n in 0..10 {
            assert!(p.put(None, &url(n), Size::Face, &[0u8; 1000], Some(&[0u8; 4000])));
            let path = p.small(&picture_key(&url(n)), Size::Face);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(SystemTime::now() - Duration::from_secs(1000 - u64::from(n))).unwrap();
        }
        p.set_cap(20_000);
        p.evict();
        assert!((0..10).all(|n| p.has(&url(n), Size::Face)), "only what was sent went");
        assert!((0..10).filter(|n| p.original_copy(&url(*n)).is_some()).count() < 4);
        p.set_cap(6_000);
        p.evict();
        let kept: Vec<u32> = (0..10).filter(|n| p.has(&url(*n), Size::Face)).collect();
        assert!(!kept.is_empty() && kept.len() <= 4 && kept.iter().all(|n| *n >= 6), "{kept:?}");
    }

    #[test]
    fn four_hundred_friends_load_in_milliseconds() {
        let t = Temp::new();
        let a = account(MEMBER, 1);
        begin(&t.0, &a);
        let s = list(400);
        let wrote = Instant::now();
        {
            let mut store = Store::open(&t.0, &a).unwrap();
            assert!(store.write(&launcher_changes(None, &s, 1)));
            store.tidy(2);
        }
        let wrote = wrote.elapsed();
        let size = std::fs::metadata(a.dir(&t.0).join(FILE)).unwrap().len();
        let mut took: Vec<Duration> = (0..9)
            .map(|_| {
                let from = Instant::now();
                assert_eq!(Store::open(&t.0, &a).unwrap().launcher().unwrap().friends.len(), 400);
                from.elapsed()
            })
            .collect();
        took.sort();
        let mut one = s.clone();
        one.friends[7].status_text = "moved".into();
        let mut store = Store::open(&t.0, &a).unwrap();
        let from = Instant::now();
        assert!(store.write(&launcher_changes(Some(&s), &one, 3)));
        eprintln!("400 friends: {} KB, written whole in {wrote:?}, one change in {:?}, loaded in {:?} (median of 9; worst {:?})", size / 1024, from.elapsed(), took[4], took[8]);
        assert!(size < 2 << 20, "{size}");
        // A debug build on a busy computer: far above what it takes.
        assert!(took[4] < Duration::from_millis(500), "{took:?}");
    }
}
