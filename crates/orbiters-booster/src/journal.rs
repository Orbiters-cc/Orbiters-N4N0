//! The record that puts everything back. Every change the booster makes is written here, flushed to disk, **before** it
//! happens, so that whatever ends the process, between any two of these steps, what comes next finds out what was
//! changed and what it was before. This is the feature the other tools get wrong, so it is written to be tested by
//! killing a process at every step (see the tests at the end of this file).
//!
//! Restoring compares the value now with what the booster wrote:
//!
//! | The value now | Means | Done |
//! |---|---|---|
//! | What was written | Still the booster's | The original comes back, or the key goes if it was absent |
//! | The original | Already back, or never applied | The entry goes |
//! | Anything else | The person or another app changed it | Left alone, the entry goes, the person is told once |
//! | Out of reach (SteamVR closed) | | The entry stays, for later |
//! | Gone (the process ended, or the PID is another process now) | | The entry goes |
//!
//! Two processes write the journal, the overlay (SteamVR's settings) and the service (programs' priorities), so a named
//! mutex guards each read, change and write. A process killed holding it leaves it abandoned, which Windows hands to the
//! next one: nothing has to wait for a crashed owner.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The format's version. A newer one than this is not guessed at: nothing is changed or restored by an older app reading
/// it, and the newer app that wrote it runs recovery itself.
pub const VERSION: u32 = 1;
/// How far two floats may differ and still be the same value: SteamVR's settings keep floats, and a value read back may
/// differ in its last digits.
pub const FLOAT_SAME: f32 = 0.001;

/// A value a change wrote or found, with "there was none" as a value of its own: a key that was absent must come back
/// absent, and 0 or false would be another setting.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    Absent,
    Bool(bool),
    Float(f32),
    Int(i64),
}

impl Value {
    /// The same value, a float within [`FLOAT_SAME`]; a number written as a whole one in a file is still that number.
    pub fn same(&self, other: &Value) -> bool {
        match (*self, *other) {
            (Value::Absent, Value::Absent) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => (a - b).abs() <= FLOAT_SAME,
            (Value::Float(a), Value::Int(b)) | (Value::Int(b), Value::Float(a)) => (a - b as f32).abs() <= FLOAT_SAME,
            _ => false,
        }
    }
}

/// What a change was made to.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// A setting of SteamVR's.
    SteamVr { section: String, key: String },
    /// A program's priority class. The PID alone could be another program by now, so its creation time goes with it.
    Process { pid: u32, created: u64, name: String },
}

/// How far a change got. Only ever read to tell a person: the table at the top of the file does not need it, as the value
/// itself says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Intended,
    Applied,
}

/// One change in force, or about to be.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub target: Target,
    pub original: Value,
    pub written: Value,
    pub stage: Stage,
    pub at: String,
    pub app_version: String,
    /// If that other change turns out changed by hand, this one stays too. A sharpness made by hand needs the manual
    /// override it stands on: restoring that would throw away what the person just chose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leave_with: Option<Target>,
}

#[derive(Debug, Serialize, Deserialize)]
struct File {
    version: u32,
    entries: Vec<Entry>,
}

/// The journal could not be read, its copy neither. Nothing is guessed: the booster changes nothing and says so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unreadable;

/// What reading a change's target found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Read {
    Value(Value),
    /// It cannot be reached now (SteamVR is closed): the entry waits.
    Unreachable,
    /// It is no more (the process ended, or its PID is another process's): the entry goes.
    Gone,
}

/// Reads and writes what the journal's changes are made to: SteamVR's live settings, its file while it is closed, or a
/// process. Tests make their own.
pub trait Access {
    fn read(&mut self, target: &Target) -> Read;
    /// Sets the value (`Absent` takes the key away); whether that was done.
    fn write(&mut self, target: &Target, value: &Value) -> bool;
    /// Whether the target has no value of its own now. SteamVR answers a key that is not set with its default, so what is
    /// read does not say.
    fn absent_now(&mut self, target: &Target) -> bool {
        matches!(self.read(target), Read::Value(Value::Absent))
    }
    /// What to record as a new change's original. SteamVR's live reads do not tell a key not set from a default, so its file
    /// is what says absent there.
    fn original(&mut self, target: &Target) -> Read {
        self.read(target)
    }
}

/// What makes recovery run after a power loss: `RunOnce` in real use. The journal's owner asks for it while there are
/// entries, and takes it away when there are none.
pub trait Armed {
    fn arm(&self) -> bool;
    fn disarm(&self);
}

/// Nothing arms anything.
pub struct NotArmed;

impl Armed for NotArmed {
    fn arm(&self) -> bool {
        true
    }
    fn disarm(&self) {}
}

/// Between which steps a change or a restoration is. Real use ignores these, and the tests stop the process at each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The temporary copy is on disk; the journal itself is the old one.
    SavedTemp,
    /// Written `intended` and flushed; nothing applied.
    Intended,
    /// The change is in force; it was not read back.
    Applied,
    /// It was read back; the journal still says `intended`.
    ReadBack,
    /// The journal says `applied`.
    Marked,
    /// The original is back; the entry is still in the journal.
    Restored,
}

pub type Probe<'a> = &'a mut dyn FnMut(Step);

/// A change to make.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub target: Target,
    pub value: Value,
    pub leave_with: Option<Target>,
}

/// What came of a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    Done,
    /// It already had that value, with no entry of ours: nothing to do and nothing to undo.
    AlreadySo,
    /// It could not be read or reached, or the write did not take: undone, and no entry stays.
    Refused,
    /// An owned value is no longer what the booster wrote: the person's. The caller lets go of it.
    ChangedSince,
}

/// What came of restoring.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Restored {
    /// Came back, or was back already.
    pub back: Vec<Target>,
    /// Changed by hand or by another app while the booster was on, left as they are. The person is told once.
    pub kept: Vec<Target>,
    /// Out of reach or refused, still in the journal.
    pub waiting: Vec<Target>,
    /// Their processes ended.
    pub gone: Vec<Target>,
}

/// The journal on disk.
pub struct Store {
    path: PathBuf,
    armed: Box<dyn Armed + Send + Sync>,
    /// For tests: stopping the process at a step of saving too.
    #[doc(hidden)]
    pub probe_save: Option<fn(Step)>,
}

impl Store {
    pub fn new(dir: &Path, armed: Box<dyn Armed + Send + Sync>) -> Self {
        Self { path: dir.join("journal.json"), armed, probe_save: None }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> Result<Lock, Unreadable> {
        Lock::take(&self.path).ok_or(Unreadable)
    }

    /// The entries now.
    pub fn entries(&self) -> Result<Vec<Entry>, Unreadable> {
        let _lock = self.lock()?;
        self.load()
    }

    /// The file, or its previous copy if that is malformed. No journal at all is an empty one.
    fn load(&self) -> Result<Vec<Entry>, Unreadable> {
        enum Found {
            Entries(Vec<Entry>),
            Missing,
            Malformed,
            /// Written by a newer app: not to be read, and its older copy is older than it, so not that either.
            Newer,
        }
        let read = |path: &Path| -> Found {
            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Found::Missing,
                Err(_) => return Found::Malformed,
            };
            if let Ok(version) = serde_json::from_str::<serde_json::Value>(&text).map(|v| v["version"].as_u64().unwrap_or(0)) {
                if version > VERSION as u64 {
                    return Found::Newer;
                }
            }
            match serde_json::from_str::<File>(&text) {
                Ok(file) => Found::Entries(file.entries),
                Err(_) => Found::Malformed,
            }
        };
        match read(&self.path) {
            Found::Entries(entries) => Ok(entries),
            Found::Newer => Err(Unreadable),
            // The file itself is gone or malformed: the copy is at worst old, and the table above is safe on old entries.
            first => match read(&backup_of(&self.path)) {
                Found::Entries(entries) => Ok(entries),
                Found::Missing if matches!(first, Found::Missing) => Ok(Vec::new()),
                _ => Err(Unreadable),
            },
        }
    }

    fn save(&self, entries: &[Entry]) -> std::io::Result<()> {
        let file = File { version: VERSION, entries: entries.to_vec() };
        let bytes = serde_json::to_vec_pretty(&file).map_err(std::io::Error::other)?;
        replace_durably(&self.path, &bytes, true, self.probe_save)
    }

    /// Asked for while entries remain (`RunOnce` is used up when Windows runs it) and taken away when none do.
    fn follow_arming(&self, entries: &[Entry]) {
        if entries.is_empty() {
            self.armed.disarm();
        } else {
            let _ = self.armed.arm();
        }
    }

    /// Makes changes as one group, each by the journal's sequence: read the value now (the first original stays when one is
    /// already owned), write `intended` and flush, arm recovery, apply, read back (different: undo and say so), mark
    /// `applied`. A group saves the journal twice, not once for each of a browser's fifty processes.
    pub fn apply(&self, access: &mut dyn Access, changes: &[Change], probe: Probe) -> Result<Vec<Applied>, Unreadable> {
        let _lock = self.lock()?;
        let mut entries = self.load()?;
        let mut results = vec![Applied::Refused; changes.len()];
        let mut todo = Vec::new();
        // What an entry that already existed was before this call: put back as it was if this step does not happen.
        let mut before: Vec<Option<(Value, Stage)>> = vec![None; changes.len()];
        let now = crate::now_text();
        for (i, change) in changes.iter().enumerate() {
            let current = match access.read(&change.target) {
                Read::Value(value) => value,
                Read::Unreachable | Read::Gone => continue,
            };
            match entries.iter_mut().find(|e| e.target == change.target) {
                Some(entry) => {
                    // Back to its original after it was applied, or anything else: the person's, not to be changed again.
                    let put_back = entry.stage == Stage::Applied && current.same(&entry.original) && !current.same(&entry.written);
                    if put_back || (!current.same(&entry.written) && !current.same(&entry.original)) {
                        results[i] = Applied::ChangedSince;
                        continue;
                    }
                    if current.same(&change.value) {
                        results[i] = Applied::Done;
                        continue;
                    }
                    // Applying twice never overwrites the first original.
                    before[i] = Some((entry.written, entry.stage));
                    entry.written = change.value;
                    entry.stage = Stage::Intended;
                }
                None => {
                    if current.same(&change.value) {
                        results[i] = Applied::AlreadySo;
                        continue;
                    }
                    let original = match access.original(&change.target) {
                        Read::Value(value) => value,
                        Read::Unreachable | Read::Gone => continue,
                    };
                    entries.push(Entry {
                        target: change.target.clone(),
                        original,
                        written: change.value,
                        stage: Stage::Intended,
                        at: now.clone(),
                        app_version: env!("CARGO_PKG_VERSION").to_string(),
                        leave_with: change.leave_with.clone(),
                    });
                }
            }
            todo.push(i);
        }
        if todo.is_empty() {
            return Ok(results);
        }
        self.save(&entries).map_err(|_| Unreadable)?;
        probe(Step::Intended);
        if !self.armed.arm() {
            // Nothing is applied that could not come back after a power loss. Only what this call added goes: an entry
            // that existed is still owed, and goes back to what it said.
            for &i in &todo {
                let Some(at) = entries.iter().position(|e| e.target == changes[i].target) else { continue };
                match before[i] {
                    Some((written, stage)) => (entries[at].written, entries[at].stage) = (written, stage),
                    None => {
                        entries.remove(at);
                    }
                }
            }
            let _ = self.save(&entries);
            self.follow_arming(&entries);
            return Ok(results);
        }
        for &i in &todo {
            let change = &changes[i];
            let wrote = access.write(&change.target, &change.value);
            probe(Step::Applied);
            let took = wrote && matches!(access.read(&change.target), Read::Value(v) if v.same(&change.value));
            let Some(at) = entries.iter().position(|e| e.target == change.target) else { continue };
            if took {
                probe(Step::ReadBack);
                entries[at].stage = Stage::Applied;
                results[i] = Applied::Done;
                continue;
            }
            // Undone: to what an existing entry said, or to the original. The entry goes only once that is read back as so:
            // a write that half took and a read that failed (SteamVR dying mid-switch) leave it owed.
            let back_to = before[i].map_or(entries[at].original, |(written, _)| written);
            // Whether the undo's own write took or not, what counts is what is there now.
            let _ = access.write(&change.target, &back_to);
            let undone = match access.read(&change.target) {
                Read::Value(now) => now.same(&back_to) || (back_to == Value::Absent && access.absent_now(&change.target)),
                Read::Gone => true,
                Read::Unreachable => false,
            };
            match (undone, before[i]) {
                (true, None) => {
                    entries.remove(at);
                }
                (true, Some((written, stage))) => (entries[at].written, entries[at].stage) = (written, stage),
                // What is there now is not known: owed as the change, which the restore table settles either way.
                (false, _) => {}
            }
        }
        let saved = self.save(&entries);
        self.follow_arming(&entries);
        probe(Step::Marked);
        saved.map_err(|_| Unreadable)?;
        Ok(results)
    }

    /// Restores the entries `which` picks, by the table at the top of this file.
    pub fn restore(&self, access: &mut dyn Access, which: &dyn Fn(&Entry) -> bool, probe: Probe) -> Result<Restored, Unreadable> {
        let _lock = self.lock()?;
        let entries = self.load()?;
        if entries.is_empty() {
            return Ok(Restored::default());
        }
        let mut out = Restored::default();
        let entries_before = entries.clone();
        let verdicts = verdicts(&entries, access, which);
        let mut left = Vec::with_capacity(entries.len());
        for (entry, verdict) in entries.into_iter().zip(verdicts) {
            match verdict {
                Verdict::NotPicked => left.push(entry),
                Verdict::Ours => {
                    let done = access.write(&entry.target, &entry.original) && is_back(&entry, access);
                    probe(Step::Restored);
                    if done {
                        out.back.push(entry.target);
                    } else {
                        // Tried again next time, and in the journal until it works.
                        out.waiting.push(entry.target.clone());
                        left.push(entry);
                    }
                }
                Verdict::Back => out.back.push(entry.target),
                Verdict::Changed => out.kept.push(entry.target),
                Verdict::Follows => {}
                Verdict::Unreachable => {
                    out.waiting.push(entry.target.clone());
                    left.push(entry);
                }
                Verdict::Gone => out.gone.push(entry.target),
            }
        }
        // Nothing settled: nothing written. A value that can never be restored must not rewrite the journal at each try.
        if left == entries_before {
            return Ok(out);
        }
        let saved = self.save(&left);
        self.follow_arming(&left);
        saved.map_err(|_| Unreadable)?;
        Ok(out)
    }

    /// Lets go of values no longer what the booster wrote, without restoring the rest: what checks the owned values while
    /// the booster is on. Those it let go of the person changed; they keep them, and it stops testing them.
    pub fn release_changed(&self, access: &mut dyn Access, which: &dyn Fn(&Entry) -> bool) -> Result<Vec<Target>, Unreadable> {
        let _lock = self.lock()?;
        let entries = self.load()?;
        let verdicts = verdicts(&entries, access, which);
        let mut kept = Vec::new();
        let mut left = Vec::with_capacity(entries.len());
        let mut changed = false;
        for (entry, verdict) in entries.into_iter().zip(verdicts) {
            match verdict {
                // Back at its original after the booster's change was in force: someone put it back. That is a choice too,
                // theirs, and told as one, or the booster would only make it again.
                Verdict::Changed | Verdict::Back if verdict == Verdict::Changed || entry.stage == Stage::Applied => {
                    kept.push(entry.target);
                    changed = true;
                }
                Verdict::Follows | Verdict::Back | Verdict::Gone => changed = true,
                _ => left.push(entry),
            }
        }
        if changed {
            let saved = self.save(&left);
            self.follow_arming(&left);
            saved.map_err(|_| Unreadable)?;
        }
        Ok(kept)
    }

    /// Whether anything is owed. A journal that cannot be read counts as owing: nobody can say it does not.
    pub fn has_entries(&self) -> bool {
        !matches!(self.entries(), Ok(entries) if entries.is_empty())
    }

    /// Arms recovery again if entries remain, as `RunOnce` is used up by running: what the restorer ends with.
    pub fn rearm(&self) -> Result<usize, Unreadable> {
        let entries = self.entries()?;
        self.follow_arming(&entries);
        Ok(entries.len())
    }

    /// Takes the journal's files away, and only with nothing left in them: an uninstall's data removal never deletes what
    /// is still owed.
    pub fn remove_if_empty(&self) -> bool {
        let Ok(entries) = self.entries() else { return false };
        if !entries.is_empty() {
            return false;
        }
        self.armed.disarm();
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(backup_of(&self.path));
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::remove_dir(dir);
        }
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Verdict {
    NotPicked,
    Ours,
    Back,
    Changed,
    /// It stays because the change it stands on was made by hand: only that one is told.
    Follows,
    Unreachable,
    Gone,
}

fn verdicts(entries: &[Entry], access: &mut dyn Access, which: &dyn Fn(&Entry) -> bool) -> Vec<Verdict> {
    let mut out: Vec<Verdict> = entries
        .iter()
        .map(|entry| {
            if !which(entry) {
                return Verdict::NotPicked;
            }
            match access.read(&entry.target) {
                Read::Unreachable => Verdict::Unreachable,
                Read::Gone => Verdict::Gone,
                Read::Value(now) if now.same(&entry.written) => Verdict::Ours,
                Read::Value(now) if now.same(&entry.original) => Verdict::Back,
                Read::Value(_) if entry.original == Value::Absent && access.absent_now(&entry.target) => Verdict::Back,
                Read::Value(_) => Verdict::Changed,
            }
        })
        .collect();
    for i in 0..entries.len() {
        let Some(other) = &entries[i].leave_with else { continue };
        let stands_on_a_hand_change = entries.iter().zip(&out).any(|(e, v)| e.target == *other && *v == Verdict::Changed);
        if stands_on_a_hand_change && !matches!(out[i], Verdict::NotPicked | Verdict::Unreachable) {
            out[i] = Verdict::Follows;
        }
    }
    out
}

fn is_back(entry: &Entry, access: &mut dyn Access) -> bool {
    match access.read(&entry.target) {
        Read::Value(now) => now.same(&entry.original) || (entry.original == Value::Absent && access.absent_now(&entry.target)),
        Read::Gone => true,
        Read::Unreachable => false,
    }
}

fn backup_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".bak");
    PathBuf::from(name)
}

/// Replaces a file so that a process killed or a computer stopped at any moment leaves the old one or the new one whole:
/// a temporary copy flushed to the disk, then moved over it through the disk's own write cache. With `backup` the previous
/// one stays beside it as `.bak`.
pub fn replace_durably(path: &Path, bytes: &[u8], backup: bool, probe: Option<fn(Step)>) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(".orbiters-tmp");
    let temp = PathBuf::from(temp);
    {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        // FlushFileBuffers on Windows.
        file.sync_all()?;
    }
    if let Some(probe) = probe {
        probe(Step::SavedTemp);
    }
    if backup && path.exists() {
        let copy = backup_of(path);
        std::fs::copy(path, &copy)?;
        // Flushing needs write access, on Windows.
        std::fs::OpenOptions::new().write(true).open(&copy)?.sync_all()?;
    }
    move_over(&temp, path)
}

#[cfg(windows)]
fn move_over(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};
    let wide = |p: &Path| p.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
    let (from, to) = (wide(from), wide(to));
    // SAFETY: both NUL-terminated and alive for the call.
    unsafe { MoveFileExW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) }
        .map_err(|e| std::io::Error::other(e.to_string()))
}

#[cfg(not(windows))]
fn move_over(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)?;
    if let Some(dir) = to.parent() {
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

/// Holds the named mutex guarding the journal at this path.
struct Lock {
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
    #[cfg(not(windows))]
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Lock {
    #[cfg(windows)]
    fn take(path: &Path) -> Option<Lock> {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
        // The name follows the path, so that tests' journals in their scratch folders never wait on the real one.
        let key = path.to_string_lossy().to_lowercase();
        let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
        let name: Vec<u16> = format!("Local\\OrbitersBoosterJournal-{hash:016x}").encode_utf16().chain([0]).collect();
        // SAFETY: a NUL-terminated name, and a handle closed when this is dropped.
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }.ok()?;
        // Abandoned: its owner was killed, and that is what this is written for; the journal itself is always whole.
        // SAFETY: a live handle.
        let waited = unsafe { WaitForSingleObject(handle, 10_000) };
        if waited == WAIT_OBJECT_0 || waited == WAIT_ABANDONED {
            Some(Lock { handle })
        } else {
            // SAFETY: opened above, not held.
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(handle);
            }
            None
        }
    }

    #[cfg(not(windows))]
    fn take(_path: &Path) -> Option<Lock> {
        // The booster is hidden where VRChat does not run; this is here for tests and for the crate to build.
        static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        Some(Lock { _guard: ONE.lock().unwrap_or_else(|e| e.into_inner()) })
    }
}

#[cfg(windows)]
impl Drop for Lock {
    fn drop(&mut self) {
        // SAFETY: held by this thread since `take`, closed once.
        unsafe {
            let _ = windows::Win32::System::Threading::ReleaseMutex(self.handle);
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    /// A scratch folder of a test's own, gone when it ends.
    pub(crate) struct Scratch(pub PathBuf);

    impl Scratch {
        pub(crate) fn new(name: &str) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!("orbiters-booster-test-{}-{name}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Settings kept in a JSON file of the test's own, so that what was written outlives a killed process.
    pub(crate) struct FileAccess {
        pub path: PathBuf,
        pub fail_writes: bool,
        pub reachable: bool,
    }

    impl FileAccess {
        pub(crate) fn new(path: PathBuf) -> Self {
            Self { path, fail_writes: false, reachable: true }
        }

        pub(crate) fn values(&self) -> BTreeMap<String, Value> {
            std::fs::read(&self.path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
        }

        pub(crate) fn set(&self, key: &str, value: Value) {
            let mut values = self.values();
            if value == Value::Absent {
                values.remove(key);
            } else {
                values.insert(key.to_string(), value);
            }
            std::fs::write(&self.path, serde_json::to_vec(&values).unwrap()).unwrap();
        }
    }

    fn key_of(target: &Target) -> String {
        match target {
            Target::SteamVr { key, .. } => key.clone(),
            Target::Process { pid, .. } => pid.to_string(),
        }
    }

    impl Access for FileAccess {
        fn read(&mut self, target: &Target) -> Read {
            if !self.reachable {
                return Read::Unreachable;
            }
            Read::Value(self.values().get(&key_of(target)).copied().unwrap_or(Value::Absent))
        }
        fn write(&mut self, target: &Target, value: &Value) -> bool {
            if self.fail_writes || !self.reachable {
                return false;
            }
            self.set(&key_of(target), *value);
            true
        }
    }

    pub(crate) fn setting(key: &str) -> Target {
        Target::SteamVr { section: "steamvr".into(), key: key.into() }
    }

    fn change(key: &str, value: Value) -> Change {
        Change { target: setting(key), value, leave_with: None }
    }

    fn store(dir: &Path) -> Store {
        Store::new(dir, Box::new(NotArmed))
    }

    #[test]
    fn a_change_comes_back_exactly() {
        let dir = Scratch::new("exact");
        let store = store(&dir.0);
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        access.set("scale", Value::Float(1.4));
        assert_eq!(store.apply(&mut access, &[change("scale", Value::Float(1.19))], &mut |_| {}).unwrap(), vec![Applied::Done]);
        assert_eq!(access.values()["scale"], Value::Float(1.19));
        let got = store.restore(&mut access, &|_| true, &mut |_| {}).unwrap();
        assert_eq!(got.back, vec![setting("scale")]);
        assert_eq!(access.values()["scale"], Value::Float(1.4));
        assert!(store.entries().unwrap().is_empty());
    }

    /// An absent key comes back absent, not as the 0 or false an ordinary read would have made of it. A manual override
    /// that was not set is SteamVR's Auto: Auto comes back as Auto.
    #[test]
    fn an_absent_key_comes_back_absent() {
        let dir = Scratch::new("absent");
        let store = store(&dir.0);
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        store.apply(&mut access, &[change("supersampleManualOverride", Value::Bool(true))], &mut |_| {}).unwrap();
        assert_eq!(access.values()["supersampleManualOverride"], Value::Bool(true));
        store.restore(&mut access, &|_| true, &mut |_| {}).unwrap();
        assert!(!access.values().contains_key("supersampleManualOverride"));
    }

    #[test]
    fn a_change_by_hand_stays_and_is_told_once() {
        let dir = Scratch::new("hand");
        let store = store(&dir.0);
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        access.set("scale", Value::Float(1.4));
        store.apply(&mut access, &[change("scale", Value::Float(1.19))], &mut |_| {}).unwrap();
        access.set("scale", Value::Float(0.9));
        let got = store.restore(&mut access, &|_| true, &mut |_| {}).unwrap();
        assert_eq!(got.kept, vec![setting("scale")]);
        assert_eq!(access.values()["scale"], Value::Float(0.9), "the person's own value");
        assert!(store.entries().unwrap().is_empty());
        assert_eq!(store.restore(&mut access, &|_| true, &mut |_| {}).unwrap(), Restored::default(), "told once");
    }

    /// A sharpness set by hand stands on the manual override the booster turned on: both stay.
    #[test]
    fn what_a_hand_change_stands_on_stays_too() {
        let dir = Scratch::new("group");
        let store = store(&dir.0);
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        let group = [Change { target: setting("override"), value: Value::Bool(true), leave_with: Some(setting("scale")) }, change("scale", Value::Float(1.0))];
        assert_eq!(store.apply(&mut access, &group, &mut |_| {}).unwrap(), vec![Applied::Done; 2]);
        access.set("scale", Value::Float(0.8));
        let got = store.release_changed(&mut access, &|_| true).unwrap();
        assert_eq!(got, vec![setting("scale")], "told about the one the person changed");
        assert_eq!(access.values()["override"], Value::Bool(true));
        assert!(store.entries().unwrap().is_empty());
    }

    #[test]
    fn applying_twice_keeps_the_first_original() {
        let dir = Scratch::new("twice");
        let store = store(&dir.0);
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        access.set("scale", Value::Float(1.4));
        store.apply(&mut access, &[change("scale", Value::Float(1.19))], &mut |_| {}).unwrap();
        assert_eq!(store.apply(&mut access, &[change("scale", Value::Float(1.0))], &mut |_| {}).unwrap(), vec![Applied::Done]);
        let entries = store.entries().unwrap();
        assert_eq!((entries.len(), entries[0].original, entries[0].written), (1, Value::Float(1.4), Value::Float(1.0)));
        store.restore(&mut access, &|_| true, &mut |_| {}).unwrap();
        assert_eq!(access.values()["scale"], Value::Float(1.4));
    }

    /// A change the person made after the booster's, then the booster asked to change it again: not overwritten.
    #[test]
    fn a_second_change_refuses_a_value_changed_since() {
        let dir = Scratch::new("since");
        let store = store(&dir.0);
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        access.set("scale", Value::Float(1.4));
        store.apply(&mut access, &[change("scale", Value::Float(1.19))], &mut |_| {}).unwrap();
        access.set("scale", Value::Float(0.9));
        assert_eq!(store.apply(&mut access, &[change("scale", Value::Float(1.0))], &mut |_| {}).unwrap(), vec![Applied::ChangedSince]);
        assert_eq!(access.values()["scale"], Value::Float(0.9));
    }

    #[test]
    fn a_failed_restore_keeps_its_entry_for_the_next_try() {
        let dir = Scratch::new("retry");
        let store = store(&dir.0);
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        access.set("scale", Value::Float(1.4));
        store.apply(&mut access, &[change("scale", Value::Float(1.19))], &mut |_| {}).unwrap();
        access.fail_writes = true;
        assert_eq!(store.restore(&mut access, &|_| true, &mut |_| {}).unwrap().waiting, vec![setting("scale")]);
        assert_eq!(store.entries().unwrap().len(), 1, "the journal survives");
        access.fail_writes = false;
        assert_eq!(store.restore(&mut access, &|_| true, &mut |_| {}).unwrap().back, vec![setting("scale")]);
        assert_eq!(access.values()["scale"], Value::Float(1.4));
    }

    #[test]
    fn a_write_that_does_not_take_leaves_no_entry() {
        let dir = Scratch::new("refused");
        let store = store(&dir.0);
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        access.fail_writes = true;
        assert_eq!(store.apply(&mut access, &[change("scale", Value::Float(1.0))], &mut |_| {}).unwrap(), vec![Applied::Refused]);
        assert!(store.entries().unwrap().is_empty());
    }

    #[test]
    fn out_of_reach_waits_and_nothing_is_lost() {
        let dir = Scratch::new("reach");
        let store = store(&dir.0);
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        access.set("scale", Value::Float(1.4));
        store.apply(&mut access, &[change("scale", Value::Float(1.19))], &mut |_| {}).unwrap();
        access.reachable = false;
        assert_eq!(store.restore(&mut access, &|_| true, &mut |_| {}).unwrap().waiting, vec![setting("scale")]);
        assert_eq!(store.entries().unwrap().len(), 1);
    }

    /// A malformed journal falls back on the copy; with both malformed nothing is guessed, nothing is changed, and the
    /// file stays for someone to look at.
    #[test]
    fn a_malformed_journal_falls_back_and_then_refuses() {
        let dir = Scratch::new("malformed");
        let store = store(&dir.0);
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        access.set("scale", Value::Float(1.4));
        store.apply(&mut access, &[change("scale", Value::Float(1.19))], &mut |_| {}).unwrap();
        store.apply(&mut access, &[change("scale", Value::Float(1.0))], &mut |_| {}).unwrap();
        std::fs::write(store.path(), b"{ half a fi").unwrap();
        let entries = store.entries().unwrap();
        assert_eq!(entries[0].original, Value::Float(1.4), "from the copy");
        std::fs::write(backup_of(store.path()), b"not json").unwrap();
        assert_eq!(store.entries(), Err(Unreadable));
        assert_eq!(store.apply(&mut access, &[change("other", Value::Bool(true))], &mut |_| {}), Err(Unreadable));
        assert!(!access.values().contains_key("other"), "nothing changed");
        assert!(store.has_entries(), "an unreadable journal counts as owed");
        assert!(!store.remove_if_empty());
        assert_eq!(std::fs::read(store.path()).unwrap(), b"{ half a fi", "kept as it was");
    }

    /// F8: a journal from a newer app, beside an older copy: neither is read, and the older one is never saved over it.
    #[test]
    fn a_newer_journal_is_not_replaced_by_its_older_copy() {
        let dir = Scratch::new("newer-bak");
        let store = store(&dir.0);
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        store.apply(&mut access, &[change("scale", Value::Float(1.0))], &mut |_| {}).unwrap();
        store.apply(&mut access, &[change("scale", Value::Float(0.9))], &mut |_| {}).unwrap();
        let newer = format!("{{\"version\":{},\"entries\":[],\"more\":1}}", VERSION + 1);
        std::fs::write(store.path(), &newer).unwrap();
        assert_eq!(store.entries(), Err(Unreadable));
        assert_eq!(store.restore(&mut access, &|_| true, &mut |_| {}), Err(Unreadable));
        assert_eq!(store.apply(&mut access, &[change("other", Value::Bool(true))], &mut |_| {}), Err(Unreadable));
        assert_eq!(std::fs::read_to_string(store.path()).unwrap(), newer, "untouched");
    }

    /// F1: the second step of a change can't arm recovery. The first step is still in force, and its entry, with its
    /// original, stays.
    #[test]
    fn a_step_that_cannot_arm_keeps_the_step_before() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        #[derive(Clone)]
        struct Failing(Arc<AtomicBool>);
        impl Armed for Failing {
            fn arm(&self) -> bool {
                !self.0.load(Ordering::SeqCst)
            }
            fn disarm(&self) {}
        }
        let fail = Arc::new(AtomicBool::new(false));
        let dir = Scratch::new("arm-step");
        let store = Store::new(&dir.0, Box::new(Failing(fail.clone())));
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        access.set("scale", Value::Float(1.4));
        store.apply(&mut access, &[change("scale", Value::Float(1.19))], &mut |_| {}).unwrap();
        fail.store(true, Ordering::SeqCst);
        assert_eq!(store.apply(&mut access, &[change("scale", Value::Float(1.0))], &mut |_| {}).unwrap(), vec![Applied::Refused]);
        let entries = store.entries().unwrap();
        assert_eq!(entries.len(), 1, "step 1 is still owed");
        assert_eq!((entries[0].original, entries[0].written, entries[0].stage), (Value::Float(1.4), Value::Float(1.19), Stage::Applied));
        assert_eq!(access.values()["scale"], Value::Float(1.19));
        store.restore(&mut access, &|_| true, &mut |_| {}).unwrap();
        assert_eq!(access.values()["scale"], Value::Float(1.4));
    }

    /// F1: the write took, the read back and the undo both failed (SteamVR dying mid-switch). The value is still there, so
    /// the entry is too, and it comes back once the target answers.
    #[test]
    fn a_change_whose_undo_failed_stays_owed() {
        struct Dying {
            inner: FileAccess,
            writes: u32,
        }
        impl Access for Dying {
            fn read(&mut self, target: &Target) -> Read {
                if self.writes >= 1 {
                    return Read::Unreachable;
                }
                self.inner.read(target)
            }
            fn write(&mut self, target: &Target, value: &Value) -> bool {
                self.writes += 1;
                self.writes == 1 && self.inner.write(target, value)
            }
        }
        let dir = Scratch::new("dying");
        let store = store(&dir.0);
        let mut dying = Dying { inner: FileAccess::new(dir.0.join("settings.json")), writes: 0 };
        dying.inner.set("scale", Value::Float(1.4));
        assert_eq!(store.apply(&mut dying, &[change("scale", Value::Float(1.19))], &mut |_| {}).unwrap(), vec![Applied::Refused]);
        assert_eq!(store.entries().unwrap().len(), 1, "still owed");
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        assert_eq!(access.values()["scale"], Value::Float(1.19), "the value it left");
        store.restore(&mut access, &|_| true, &mut |_| {}).unwrap();
        assert_eq!(access.values()["scale"], Value::Float(1.4));
    }

    /// F2: a value that can't be restored now doesn't make every try rewrite the journal.
    #[test]
    fn a_restore_that_settles_nothing_writes_nothing() {
        static SAVES: AtomicU32 = AtomicU32::new(0);
        let dir = Scratch::new("no-rewrite");
        let mut store = store(&dir.0);
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        store.apply(&mut access, &[change("scale", Value::Float(1.0))], &mut |_| {}).unwrap();
        store.probe_save = Some(|_| {
            SAVES.fetch_add(1, Ordering::SeqCst);
        });
        access.reachable = false;
        for _ in 0..5 {
            assert_eq!(store.restore(&mut access, &|_| true, &mut |_| {}).unwrap().waiting.len(), 1);
        }
        assert_eq!(SAVES.load(Ordering::SeqCst), 0);
    }

    /// M6: put back to its original by someone after it was applied: the person's, not changed again.
    #[test]
    fn a_value_put_back_by_hand_is_not_changed_again() {
        let dir = Scratch::new("put-back-by-hand");
        let store = store(&dir.0);
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        access.set("scale", Value::Float(1.4));
        store.apply(&mut access, &[change("scale", Value::Float(1.19))], &mut |_| {}).unwrap();
        access.set("scale", Value::Float(1.4));
        assert_eq!(store.apply(&mut access, &[change("scale", Value::Float(1.19))], &mut |_| {}).unwrap(), vec![Applied::ChangedSince]);
        assert_eq!(access.values()["scale"], Value::Float(1.4));
    }

    #[test]
    fn a_newer_format_is_not_guessed_at() {
        let dir = Scratch::new("newer");
        let store = store(&dir.0);
        std::fs::write(store.path(), format!("{{\"version\":{},\"entries\":[]}}", VERSION + 1)).unwrap();
        assert_eq!(store.entries(), Err(Unreadable));
    }

    #[test]
    fn uninstall_removes_only_an_empty_journal() {
        let dir = Scratch::new("remove");
        let store = store(&dir.0);
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        store.apply(&mut access, &[change("scale", Value::Float(1.0))], &mut |_| {}).unwrap();
        assert!(!store.remove_if_empty(), "still owed");
        assert!(store.path().exists());
        store.restore(&mut access, &|_| true, &mut |_| {}).unwrap();
        assert!(store.remove_if_empty());
        assert!(!store.path().exists());
    }

    /// Recovery is asked for from the moment there is an entry until there is none, and a change that cannot arm it does
    /// not happen.
    #[test]
    fn recovery_is_armed_while_there_are_entries() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        #[derive(Clone, Default)]
        struct Shared {
            armed: Arc<AtomicBool>,
            fail: Arc<AtomicBool>,
        }
        impl Armed for Shared {
            fn arm(&self) -> bool {
                if self.fail.load(Ordering::SeqCst) {
                    return false;
                }
                self.armed.store(true, Ordering::SeqCst);
                true
            }
            fn disarm(&self) {
                self.armed.store(false, Ordering::SeqCst);
            }
        }
        let dir = Scratch::new("armed");
        let armed = Shared::default();
        let store = Store::new(&dir.0, Box::new(armed.clone()));
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        store.apply(&mut access, &[change("scale", Value::Float(1.0))], &mut |_| {}).unwrap();
        assert!(armed.armed.load(Ordering::SeqCst));
        // RunOnce is used up when Windows runs it: what restores at sign-in arms it again while something is still owed.
        armed.armed.store(false, Ordering::SeqCst);
        access.reachable = false;
        store.restore(&mut access, &|_| true, &mut |_| {}).unwrap();
        store.rearm().unwrap();
        assert!(armed.armed.load(Ordering::SeqCst), "armed again");
        access.reachable = true;
        store.restore(&mut access, &|_| true, &mut |_| {}).unwrap();
        assert!(!armed.armed.load(Ordering::SeqCst));
        armed.fail.store(true, Ordering::SeqCst);
        assert_eq!(store.apply(&mut access, &[change("scale", Value::Float(1.0))], &mut |_| {}).unwrap(), vec![Applied::Refused]);
        assert!(!access.values().contains_key("scale"), "not applied");
        assert!(store.entries().unwrap().is_empty());
    }

    // ------------------------------------------------------------------------- killing a process at every step

    const KILL_DIR: &str = "ORBITERS_BOOSTER_KILL_DIR";
    const KILL_AT: &str = "ORBITERS_BOOSTER_KILL_AT";
    const KILL_DOING: &str = "ORBITERS_BOOSTER_KILL_DOING";

    fn step_name(step: Step) -> &'static str {
        match step {
            Step::SavedTemp => "saved-temp",
            Step::Intended => "intended",
            Step::Applied => "applied",
            Step::ReadBack => "read-back",
            Step::Marked => "marked",
            Step::Restored => "restored",
        }
    }

    /// Says it got there, then waits to be killed from outside, as nothing should get to tidy up.
    fn stop_here_if_asked(step: Step) {
        if std::env::var(KILL_AT).as_deref() == Ok(step_name(step)) {
            let dir = PathBuf::from(std::env::var_os(KILL_DIR).unwrap());
            std::fs::write(dir.join("reached"), step_name(step)).unwrap();
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }

    /// Not a test: what [`killed_at`] runs in a process of its own, and kills.
    #[test]
    #[ignore = "run by the kill tests, in a process of its own"]
    fn killed_child() {
        let Some(dir) = std::env::var_os(KILL_DIR).map(PathBuf::from) else { return };
        let mut store = store(&dir);
        store.probe_save = Some(stop_here_if_asked);
        let mut access = FileAccess::new(dir.join("settings.json"));
        if std::env::var(KILL_DOING).as_deref() == Ok("apply") {
            let changes = [change("scale", Value::Float(1.19)), change("override", Value::Bool(true))];
            store.apply(&mut access, &changes, &mut stop_here_if_asked).unwrap();
        } else {
            store.restore(&mut access, &|_| true, &mut stop_here_if_asked).unwrap();
        }
        std::fs::write(dir.join("finished"), "").unwrap();
    }

    /// Runs [`killed_child`], kills the process once it has reached `step`, and leaves the folder as the kill left it.
    fn killed_at(dir: &Path, doing: &str, step: &str) {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "journal::tests::killed_child", "--ignored", "--nocapture"])
            .env(KILL_DIR, dir)
            .env(KILL_AT, step)
            .env(KILL_DOING, doing)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        while !dir.join("reached").exists() {
            if let Ok(Some(status)) = child.try_wait() {
                panic!("the child ended by itself before {step}: {status:?}");
            }
            assert!(Instant::now() < deadline, "never reached {step}");
            std::thread::sleep(Duration::from_millis(5));
        }
        child.kill().unwrap();
        let _ = child.wait();
        let _ = std::fs::remove_file(dir.join("reached"));
    }

    /// The process making a change is killed from outside at every step of it, and what the next one finds puts everything
    /// back: the scale as it was, and the override, absent before, absent again.
    #[test]
    fn killed_at_every_step_of_a_change_everything_comes_back() {
        for step in ["saved-temp", "intended", "applied", "read-back", "marked"] {
            let dir = Scratch::new("kill-apply");
            let mut access = FileAccess::new(dir.0.join("settings.json"));
            access.set("scale", Value::Float(1.4));
            killed_at(&dir.0, "apply", step);
            assert!(!dir.0.join("finished").exists());
            let store = store(&dir.0);
            let got = store.restore(&mut access, &|_| true, &mut |_| {}).unwrap();
            assert!(got.kept.is_empty() && got.waiting.is_empty(), "{step}: {got:?}");
            let values = access.values();
            assert_eq!(values.get("scale"), Some(&Value::Float(1.4)), "{step}");
            assert!(!values.contains_key("override"), "{step}");
            assert!(store.entries().unwrap().is_empty(), "{step}");
        }
    }

    /// The process restoring is killed too: the next one finishes, and nothing is restored twice into something wrong.
    #[test]
    fn killed_while_restoring_the_next_one_finishes() {
        for step in ["saved-temp", "restored"] {
            let dir = Scratch::new("kill-restore");
            let mut access = FileAccess::new(dir.0.join("settings.json"));
            access.set("scale", Value::Float(1.4));
            store(&dir.0).apply(&mut access, &[change("scale", Value::Float(1.19)), change("override", Value::Bool(true))], &mut |_| {}).unwrap();
            killed_at(&dir.0, "restore", step);
            let store = store(&dir.0);
            let got = store.restore(&mut access, &|_| true, &mut |_| {}).unwrap();
            assert!(got.kept.is_empty(), "{step}: {got:?}");
            let values = access.values();
            assert_eq!(values.get("scale"), Some(&Value::Float(1.4)), "{step}");
            assert!(!values.contains_key("override"), "{step}");
            assert!(store.entries().unwrap().is_empty(), "{step}");
        }
    }

    /// A process killed holding the journal's mutex must not stop the next one.
    #[test]
    fn the_lock_a_killed_process_held_is_free_after_it() {
        let dir = Scratch::new("kill-lock");
        killed_at(&dir.0, "apply", "intended");
        let started = Instant::now();
        store(&dir.0).entries().unwrap();
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }

    /// Several writers at once keep the journal whole, and lose nothing of each other's.
    #[test]
    fn writers_at_the_same_time_keep_the_journal_whole() {
        let dir = Scratch::new("writers");
        let keys: Vec<String> = (0..4).map(|i| format!("key{i}")).collect();
        std::thread::scope(|s| {
            for key in &keys {
                let dir = dir.0.clone();
                s.spawn(move || {
                    let store = store(&dir);
                    let mut access = FileAccess::new(dir.join(format!("{key}.json")));
                    for _ in 0..10 {
                        store.apply(&mut access, &[change(key, Value::Bool(true))], &mut |_| {}).unwrap();
                        store.restore(&mut access, &|e: &Entry| key_of(&e.target) == *key, &mut |_| {}).unwrap();
                    }
                });
            }
        });
        assert!(store(&dir.0).entries().unwrap().is_empty());
    }
}
