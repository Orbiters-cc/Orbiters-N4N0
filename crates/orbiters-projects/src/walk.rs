//! Walking folders for Unity projects (PRJ-10). A folder is listed, and that is all that is asked of it: what its children are comes
//! with its listing, and its version file is looked at only where the listing has `ProjectSettings`. Several folders at once on a disk
//! without seeks, one on a hard disk, every disk beside the others. What the folders held is kept ([`Index`]) with when each was last
//! changed, on the file systems that keep that (NTFS and ReFS, a Mac's): the next walk lists only those that changed since. (A folder's
//! time is asked of the folder: the one its parent's listing gives is not kept up to date on NTFS.)
use crate::discovery::{asks_permission, is_link, is_project, skip_dir, unfinished_copy};
use orbiters_i18n::tr;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

/// A folder that changed less than this before it was looked at may change again within what its file system tells apart: listed next time.
const SETTLED: Duration = Duration::from_secs(3);
/// A disk that answers nothing for this long is given up.
const STALLED: Duration = Duration::from_secs(60);

const HAS_SETTINGS: u8 = 1;
const HAS_MARKER: u8 = 2;

/// What the folders of the last walk held: for each, when it was last changed and the folders in it a walk goes to.
#[derive(Default)]
pub(crate) struct Index {
    /// As it is in its file: [`Found::record`] after [`Found::record`].
    bytes: Vec<u8>,
    /// Where each folder's is in it, by its path's hash.
    at: HashMap<u64, usize>,
}

const MAGIC: &[u8; 8] = b"OrbFold1";

impl Index {
    pub(crate) fn load(file: &Path) -> Index {
        std::fs::read(file).ok().and_then(Index::of).unwrap_or_default()
    }

    fn of(bytes: Vec<u8>) -> Option<Index> {
        let mut at = HashMap::new();
        let mut i = bytes.starts_with(MAGIC).then_some(MAGIC.len())?;
        while i < bytes.len() {
            let start = i;
            let header = bytes.get(i..i + 19)?;
            let hash = u64::from_le_bytes(header[..8].try_into().ok()?);
            let names = u16::from_le_bytes(header[17..19].try_into().ok()?);
            i += 19;
            for _ in 0..names {
                i += 2 + usize::from(u16::from_le_bytes(bytes.get(i..i + 2)?.try_into().ok()?));
            }
            if i > bytes.len() {
                return None;
            }
            at.insert(hash, start);
        }
        Some(Index { bytes, at })
    }

    /// What the folder `hash` held when it was last changed at `stamp`.
    fn get(&self, hash: u64, stamp: u64) -> Option<(u8, Vec<&str>)> {
        let i = *self.at.get(&hash)?;
        let field = |a: usize, b: usize| &self.bytes[i + a..i + b];
        if stamp == 0 || u64::from_le_bytes(field(8, 16).try_into().ok()?) != stamp {
            return None;
        }
        let (flags, names) = (self.bytes[i + 16], u16::from_le_bytes(field(17, 19).try_into().ok()?));
        let mut j = i + 19;
        let mut list = Vec::with_capacity(names.into());
        for _ in 0..names {
            let n = usize::from(u16::from_le_bytes(self.bytes[j..j + 2].try_into().ok()?));
            list.push(std::str::from_utf8(&self.bytes[j + 2..j + 2 + n]).ok()?);
            j += 2 + n;
        }
        Some((flags, list))
    }

    /// Writes what a walk found; `cut`: the walk did not go everywhere, what it did not see stays as it was.
    pub(crate) fn save(&self, file: &Path, found: &[Vec<u8>], cut: bool) -> std::io::Result<()> {
        use std::io::Write;
        let tmp = file.with_extension(format!("{}.tmp", std::process::id()));
        let written = (|| {
            let mut out = std::io::BufWriter::new(crate::disk::create_file(&tmp)?);
            out.write_all(MAGIC)?;
            for part in found {
                out.write_all(part)?;
            }
            if cut {
                let seen: HashSet<u64> = found.iter().filter_map(|part| Index::of([MAGIC.as_slice(), part].concat())).flat_map(|i| i.at.into_keys()).collect();
                let mut kept: Vec<(u64, usize)> = self.at.iter().map(|(h, i)| (*h, *i)).filter(|(h, _)| !seen.contains(h)).collect();
                kept.sort_by_key(|(_, i)| *i);
                for (_, i) in kept {
                    out.write_all(&self.bytes[i..self.end(i)])?;
                }
            }
            out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
            crate::disk::rename(&tmp, file)
        })();
        if written.is_err() {
            let _ = crate::disk::remove_file(&tmp);
        }
        written
    }

    fn end(&self, i: usize) -> usize {
        let names = u16::from_le_bytes([self.bytes[i + 17], self.bytes[i + 18]]);
        (0..names).fold(i + 19, |j, _| j + 2 + usize::from(u16::from_le_bytes([self.bytes[j], self.bytes[j + 1]])))
    }
}

/// The path of a folder, told from every other (FNV-1a: the same on every run).
fn hash(path: &Path) -> u64 {
    path.as_os_str().as_encoded_bytes().iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

/// What is in a folder.
struct Held {
    flags: u8,
    /// The folders a walk goes to.
    folders: Vec<std::ffi::OsString>,
}

impl Held {
    fn list(path: &Path) -> Option<Held> {
        let mut held = Held { flags: 0, folders: vec![] };
        for e in std::fs::read_dir(path).ok()?.flatten() {
            let name = e.file_name();
            if name == crate::templates::INCOMPLETE {
                held.flags |= HAS_MARKER;
            }
            // (A link is no folder here, and this is of the listing: nothing more is asked of the disk.)
            if !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            if name == "ProjectSettings" { // i18n: not shown (a folder name)
                held.flags |= HAS_SETTINGS;
            }
            if !skip_dir(&name.to_string_lossy()) {
                held.folders.push(name);
            }
        }
        Some(held)
    }

    /// Adds it to what a walk found: `hash`, `stamp`, flags, the count of folders, each's length and name. Not when a name is not text.
    fn record(&self, hash: u64, stamp: u64, out: &mut Vec<u8>) {
        let names: Option<Vec<&str>> = self.folders.iter().map(|n| n.to_str().filter(|n| n.len() <= usize::from(u16::MAX))).collect();
        let Some((names, count)) = names.and_then(|n| Some((u16::try_from(n.len()).ok().map(|c| (n, c)))?)) else { return };
        if stamp == 0 {
            return;
        }
        out.extend(hash.to_le_bytes());
        out.extend(stamp.to_le_bytes());
        out.push(self.flags);
        out.extend(count.to_le_bytes());
        for n in names {
            out.extend((n.len() as u16).to_le_bytes());
            out.extend(n.as_bytes());
        }
    }
}

/// When a folder was last changed, as its file system keeps it; 0 when it cannot be told from a change a moment later.
fn stamp(path: &Path) -> Option<u64> {
    let m = std::fs::symlink_metadata(path).ok()?;
    if !m.is_dir() || is_link(&m) {
        return None;
    }
    let modified = m.modified().ok()?;
    let settled = SystemTime::now().duration_since(modified).is_ok_and(|age| age > SETTLED);
    let nanos = modified.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_nanos();
    Some(if settled { (nanos as u64).max(1) } else { 0 })
}

/// How a walk goes.
#[derive(Clone)]
struct How {
    /// How many folders deep, from each root.
    depth: Option<u32>,
    /// Leaving out the folders reading which makes the system ask the person something.
    quiet: bool,
    /// The folders' times tell whether they changed.
    times: bool,
    /// What the last walk found (none: everything is listed).
    index: Option<Arc<Index>>,
}

/// The folders still to look at, shared by the threads of one pool. Each has its own besides, and gives half of it when another waits.
struct Pool {
    how: How,
    shared: Mutex<(Vec<(PathBuf, u32)>, usize)>,
    wake: Condvar,
    waiting: AtomicUsize,
    stop: AtomicBool,
    looked: AtomicU64,
}

enum Event {
    Found(PathBuf),
    /// A thread ended, with what it recorded of the folders.
    Done(usize, Vec<u8>),
}

impl Pool {
    fn new(roots: impl Iterator<Item = PathBuf>, how: How) -> Pool {
        Pool {
            how,
            shared: Mutex::new((roots.map(|r| (r, 0)).collect(), 0)),
            wake: Condvar::new(),
            waiting: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            looked: AtomicU64::new(0),
        }
    }

    /// A folder to look at; `None` when none is left and no thread is looking at one.
    fn next(&self, own: &mut Vec<(PathBuf, u32)>) -> Option<(PathBuf, u32)> {
        if let Some(item) = own.pop() {
            return Some(item);
        }
        let mut shared = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        shared.1 -= 1;
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return None;
            }
            if let Some(item) = shared.0.pop() {
                shared.1 += 1;
                return Some(item);
            }
            if shared.1 == 0 {
                self.wake.notify_all();
                return None;
            }
            self.waiting.fetch_add(1, Ordering::Relaxed);
            shared = self.wake.wait_timeout(shared, Duration::from_millis(100)).unwrap_or_else(PoisonError::into_inner).0;
            self.waiting.fetch_sub(1, Ordering::Relaxed);
        }
    }

    fn share(&self, own: &mut Vec<(PathBuf, u32)>) {
        if own.len() > 1 && self.waiting.load(Ordering::Relaxed) > 0 {
            let half = own.len() / 2;
            self.shared.lock().unwrap_or_else(PoisonError::into_inner).0.extend(own.drain(..half));
            self.wake.notify_all();
        }
    }

    /// One thread's part, until no folder is left.
    fn run(&self, mut found: impl FnMut(PathBuf)) -> Vec<u8> {
        let (mut own, mut out) = (vec![], vec![]);
        while let Some((path, depth)) = self.next(&mut own) {
            if self.stop.load(Ordering::Relaxed) {
                break;
            }
            self.look(path, depth, &mut own, &mut out, &mut found);
            self.looked.fetch_add(1, Ordering::Relaxed);
            self.share(&mut own);
        }
        out
    }

    fn look(&self, path: PathBuf, depth: u32, own: &mut Vec<(PathBuf, u32)>, out: &mut Vec<u8>, found: &mut impl FnMut(PathBuf)) {
        let key = hash(&path);
        let stamp = if self.how.times {
            let Some(stamp) = stamp(&path) else { return };
            stamp
        } else {
            0
        };
        let known = self.how.index.as_ref().and_then(|i| i.get(key, stamp)).map(|(flags, names)| Held { flags, folders: names.into_iter().map(Into::into).collect() });
        let Some(held) = known.or_else(|| Held::list(&path)) else { return };
        held.record(key, stamp, out);
        if (held.flags & HAS_SETTINGS != 0 && is_project(&path)) || (held.flags & HAS_MARKER != 0 && unfinished_copy(&path)) {
            found(path);
            return;
        }
        if self.how.depth.is_none_or(|max| depth < max) {
            let there = held.folders.into_iter().map(|name| path.join(name)).filter(|p| !(self.how.quiet && asks_permission(p)));
            own.extend(there.map(|p| (p, depth + 1)));
        }
    }
}

/// `n` threads of `pool`, each telling what it found.
fn start(pool: &Arc<Pool>, n: usize, number: usize, tell: &mpsc::Sender<Event>) -> Vec<std::thread::JoinHandle<()>> {
    pool.shared.lock().unwrap_or_else(PoisonError::into_inner).1 = n;
    (0..n)
        .map(|_| {
            let (pool, tell) = (pool.clone(), tell.clone());
            std::thread::spawn(move || {
                background();
                let out = pool.run(|p| drop(tell.send(Event::Found(p))));
                let _ = tell.send(Event::Done(number, out));
            })
        })
        .collect()
}

/// The folders under `roots`, `depth` deep: what a look at the places projects are known to be finds. No index, no thread.
pub(crate) fn near(roots: impl Iterator<Item = PathBuf>, depth: Option<u32>, quiet: bool, cancel: &AtomicBool, mut found: impl FnMut(PathBuf)) {
    // A root may be a link, which a walk does not follow.
    let roots = roots.filter(|r| std::fs::symlink_metadata(r).is_ok_and(|m| m.is_dir() && !is_link(&m)));
    let pool = Pool::new(roots, How { depth, quiet, times: false, index: None });
    pool.shared.lock().unwrap_or_else(PoisonError::into_inner).1 = 1;
    let mut own = vec![];
    let mut out = vec![];
    while let Some((path, depth)) = pool.next(&mut own) {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        pool.look(path, depth, &mut own, &mut out, &mut found);
    }
}

/// The disks that are one device, walked by one pool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Device {
    pub roots: Vec<PathBuf>,
    pub threads: usize,
    /// Its file systems keep when a folder was last changed.
    pub times: bool,
}

/// `roots` by the device they are on, with how many threads suit it.
pub(crate) fn devices(roots: Vec<PathBuf>) -> Vec<Device> {
    let mut all: Vec<(Option<u32>, Device)> = vec![];
    for root in roots {
        let (number, threads) = device(&root);
        let times = keeps_times(&root);
        match all.iter_mut().find(|(n, d)| n.is_some() && *n == number && d.times == times) {
            Some((_, d)) => d.roots.push(root),
            None => all.push((number, Device { roots: vec![root], threads, times })),
        }
    }
    all.into_iter().map(|(_, d)| d).collect()
}

/// What a walk of the disks did.
#[derive(Default)]
pub(crate) struct Walked {
    pub folders: u64,
    pub warnings: Vec<String>,
    /// What was recorded of the folders ([`Index::save`]).
    pub recorded: Vec<Vec<u8>>,
    /// It did not go everywhere: it was stopped, or a disk did not answer.
    pub cut: bool,
}

/// Walks every device beside the others; `found` hears of each project. A disk that does not answer for a minute must not hold the
/// others: it is given up, what it had given stays, and it is named in the warnings.
pub(crate) fn disks(devices: Vec<Device>, quiet: bool, index: &Arc<Index>, cancel: &AtomicBool, mut found: impl FnMut(PathBuf)) -> Walked {
    struct Running {
        device: Device,
        pool: Arc<Pool>,
        threads: Vec<std::thread::JoinHandle<()>>,
        left: usize,
        last: (u64, Instant),
    }
    let (tx, rx) = mpsc::channel();
    let mut running: Vec<Running> = devices
        .into_iter()
        .enumerate()
        .map(|(number, device)| {
            let how = How { depth: None, quiet, times: device.times, index: Some(index.clone()) };
            let pool = Arc::new(Pool::new(device.roots.iter().cloned(), how));
            let threads = start(&pool, device.threads, number, &tx);
            Running { left: threads.len(), device, pool, threads, last: (0, Instant::now()) }
        })
        .collect();
    drop(tx);
    let mut out = Walked::default();
    while running.iter().any(|r| r.left > 0) {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(Event::Found(p)) => found(p),
            Ok(Event::Done(number, recorded)) => {
                running[number].left -= 1;
                out.recorded.push(recorded);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        for r in running.iter_mut().filter(|r| r.left > 0) {
            let looked = r.pool.looked.load(Ordering::Relaxed);
            if looked != r.last.0 {
                r.last = (looked, Instant::now());
            }
            let stalled = r.last.1.elapsed() > STALLED;
            if stalled {
                let roots = r.device.roots.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
                out.warnings.push(tr!("{folder} stopped responding for 60 seconds. Its scan was interrupted; Refresh retries it.", folder = roots));
            }
            if stalled || cancel.load(Ordering::Relaxed) {
                out.cut = true;
                r.left = 0;
                r.pool.stop.store(true, Ordering::Relaxed);
                r.pool.wake.notify_all();
                #[cfg(windows)]
                for thread in &r.threads {
                    use std::os::windows::io::AsRawHandle;
                    // SAFETY: a thread of this process, whose handle is held.
                    unsafe {
                        let _ = windows::Win32::System::IO::CancelSynchronousIo(windows::Win32::Foundation::HANDLE(thread.as_raw_handle()));
                    }
                }
            }
        }
    }
    out.folders = running.iter().map(|r| r.pool.looked.load(Ordering::Relaxed)).sum();
    for r in running.into_iter().filter(|r| !r.pool.stop.load(Ordering::Relaxed)) {
        for thread in r.threads {
            let _ = thread.join();
        }
    }
    // What the threads of a walk that was stopped had recorded by then.
    while let Ok(event) = rx.recv_timeout(Duration::from_millis(if out.cut { 200 } else { 0 })) {
        if let Event::Done(_, recorded) = event {
            out.recorded.push(recorded);
        }
    }
    out
}

/// This thread's work is nobody's wait: the processor, and the disk too, serve everything else first.
fn background() {
    crate::discovery::low_priority();
    #[cfg(windows)]
    // SAFETY: changes the calling thread alone, for as long as it lives.
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
}

/// How many threads look at a disk without seeks, at one that is not known, at a hard disk.
const THREADS: (usize, usize, usize) = (4, 2, 1);

/// The device `root` is on, and how many threads suit it.
fn device(root: &Path) -> (Option<u32>, usize) {
    if cfg!(windows) {
        let device = orbiters_core::disk::device(root);
        let threads = match device.seeks {
            Some(false) => THREADS.0,
            Some(true) => THREADS.2,
            None => THREADS.1,
        };
        return (device.number, threads);
    }
    // A Mac's own disk; another may be any.
    (None, if root.starts_with("/Users") { THREADS.0 } else { THREADS.1 })
}

/// Whether the file system at `root` changes a folder's time whenever what it holds changes: FAT and exFAT do not.
fn keeps_times(root: &Path) -> bool {
    #[cfg(windows)]
    {
        crate::storage::file_system_of(root).is_some_and(|name| ["NTFS", "ReFS"].iter().any(|n| name.eq_ignore_ascii_case(n)))
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(path) = std::ffi::CString::new(root.as_os_str().as_bytes()) else { return false };
        // SAFETY: a valid C string and a statfs to fill; all zeros is a valid value of one.
        let mut s: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(path.as_ptr(), &mut s) } != 0 {
            return false;
        }
        let name: Vec<u8> = s.f_fstypename.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
        matches!(name.as_slice(), b"apfs" | b"hfs")
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = root;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(at: &Path) {
        std::fs::create_dir_all(at.join("ProjectSettings")).unwrap();
        std::fs::create_dir_all(at.join("Assets/Deep/Project")).unwrap();
        std::fs::write(at.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
    }
    fn at(root: &Path, threads: usize) -> Vec<Device> {
        vec![Device { roots: vec![root.into()], threads, times: true }]
    }
    fn walk(root: &Path, threads: usize, index: &Arc<Index>) -> (Vec<PathBuf>, Walked) {
        let mut found = vec![];
        let walked = disks(at(root, threads), false, index, &AtomicBool::new(false), |p| found.push(p));
        found.sort();
        (found, walked)
    }
    /// The index a walk made, through its file.
    fn kept(walked: &Walked, file: &Path) -> Arc<Index> {
        Index::default().save(file, &walked.recorded, false).unwrap();
        Arc::new(Index::load(file))
    }
    /// Every folder as if nothing had changed for an hour.
    fn settle(root: &Path) {
        let then = SystemTime::now() - Duration::from_secs(3600);
        for e in walkdir::WalkDir::new(root).into_iter().flatten().filter(|e| e.file_type().is_dir()) {
            #[cfg(windows)]
            let folder = {
                use std::os::windows::fs::OpenOptionsExt;
                std::fs::OpenOptions::new().access_mode(0x100).share_mode(7).custom_flags(0x0200_0000).open(e.path()).unwrap()
            };
            #[cfg(not(windows))]
            let folder = std::fs::File::open(e.path()).unwrap();
            folder.set_modified(then).unwrap();
        }
    }

    #[test]
    fn a_walk_finds_every_project_however_many_threads_it_has() {
        let temp = tempfile::Builder::new().prefix("walk").tempdir().unwrap();
        let root = temp.path();
        let projects = ["A", "Deep/Er/B", "C", "Library/Not", "node_modules/Not", ".hidden/Not", "A/Assets/Not a project"];
        for p in projects {
            project(&root.join(p));
        }
        for i in 0..300 {
            std::fs::create_dir_all(root.join(format!("Plain/{}/{i}", i % 17))).unwrap();
        }
        // Not one: its version file is not there. One that is a copy cut short: shown.
        std::fs::create_dir_all(root.join("Half/ProjectSettings")).unwrap();
        project(&root.join("Cut"));
        std::fs::write(root.join("Cut").join(crate::templates::INCOMPLETE), "").unwrap();
        // A link is not gone through.
        #[cfg(windows)]
        junction::create(root.join("Deep"), root.join("Linked")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("Deep"), root.join("Linked")).unwrap();
        let wanted = ["A", "C", "Cut", "Deep/Er/B"].map(|p| p.split('/').fold(root.to_path_buf(), |at, part| at.join(part)));
        for threads in [1, 2, 8] {
            let (found, walked) = walk(root, threads, &Arc::default());
            assert_eq!(found, wanted, "{threads} threads");
            assert!(!walked.cut && walked.warnings.is_empty() && walked.folders > 320, "{}", walked.folders);
        }
        let mut close = vec![];
        near(std::iter::once(root.to_path_buf()), Some(1), false, &AtomicBool::new(false), |p| close.push(p));
        close.sort();
        assert_eq!(close, wanted[..3], "one folder deep");
        assert_eq!(crate::discovery::found_in(root), wanted);
    }

    /// PRJ-10: a folder that did not change is not listed again; whatever changes in one is found.
    #[test]
    fn a_second_walk_lists_only_what_changed() {
        let temp = tempfile::Builder::new().prefix("walk").tempdir().unwrap();
        let (root, file) = (temp.path().join("Root"), temp.path().join("folders.bin"));
        project(&root.join("A"));
        std::fs::create_dir_all(root.join("Work/Later/ProjectSettings")).unwrap();
        std::fs::create_dir_all(root.join("Work/Gone")).unwrap();
        project(&root.join("Work/Gone/Project"));
        settle(&root);
        let (found, walked) = walk(&root, 2, &Arc::default());
        assert_eq!(found.len(), 2);
        let index = kept(&walked, &file);
        assert!(index.at.len() >= 4 && index.at.len() as u64 == walked.folders, "{} of {}", index.at.len(), walked.folders);
        // What the index says is taken where nothing changed: one of another content shows it is.
        let mut told = Held { flags: HAS_SETTINGS, folders: vec![] };
        told.folders.push("Not there".into());
        let mut bytes = MAGIC.to_vec();
        told.record(hash(&root.join("Work")), stamp(&root.join("Work")).unwrap(), &mut bytes);
        let (found, _) = walk(&root, 2, &Arc::new(Index::of(bytes).unwrap()));
        assert_eq!(found, [root.join("A")], "not listed");
        // Nothing changed.
        let (found, again) = walk(&root, 2, &index);
        assert_eq!((found.len(), again.folders), (2, walked.folders));
        // A project made in a folder that was there, a version file put where its folder was, a project removed.
        project(&root.join("Work/New"));
        std::fs::write(root.join("Work/Later/ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
        std::fs::remove_dir_all(root.join("Work/Gone/Project")).unwrap();
        let (found, _) = walk(&root, 2, &index);
        assert_eq!(found, ["A", "Work/Later", "Work/New"].map(|p| p.split('/').fold(root.clone(), |at, part| at.join(part))));
    }

    /// A file that is cut, damaged or of another version is no index: everything is listed.
    #[test]
    fn a_damaged_index_is_none() {
        let temp = tempfile::Builder::new().prefix("walk").tempdir().unwrap();
        let (root, file) = (temp.path().join("Root"), temp.path().join("folders.bin"));
        project(&root.join("A"));
        settle(&root);
        let (_, walked) = walk(&root, 1, &Arc::default());
        kept(&walked, &file);
        let bytes = std::fs::read(&file).unwrap();
        for damaged in [vec![], bytes[..bytes.len() - 1].to_vec(), bytes[..20].to_vec(), b"OrbFold0".to_vec(), vec![0xff; 100]] {
            std::fs::write(&file, &damaged).unwrap();
            let index = Arc::new(Index::load(&file));
            assert!(index.at.is_empty());
            assert_eq!(walk(&root, 1, &index).0.len(), 1);
        }
        // A walk that was cut keeps what it did not see.
        let index = Index::of(bytes).unwrap();
        index.save(&file, &[], true).unwrap();
        assert_eq!(Index::load(&file).at.len(), index.at.len());
        index.save(&file, &walked.recorded, true).unwrap();
        assert_eq!(Index::load(&file).at.len(), index.at.len());
    }

    #[test]
    fn a_walk_that_is_stopped_stops() {
        let temp = tempfile::Builder::new().prefix("walk").tempdir().unwrap();
        project(&temp.path().join("A"));
        let walked = disks(at(temp.path(), 4), false, &Arc::default(), &AtomicBool::new(true), |_| {});
        assert!(walked.cut && walked.warnings.is_empty());
    }

    /// This computer's disks are told apart (read only).
    #[test]
    fn disks_are_grouped_by_their_device() {
        let roots = crate::discovery::fixed_disks();
        let devices = devices(roots.clone());
        let mut all: Vec<PathBuf> = devices.iter().flat_map(|d| d.roots.clone()).collect();
        all.sort();
        let mut roots = roots;
        roots.sort();
        assert_eq!(all, roots);
        assert!(devices.iter().all(|d| (1..=4).contains(&d.threads)), "{devices:?}");
        println!("{devices:?}");
    }

    /// The walk as it was: a look at each folder, at its version file, then its listing, on one thread.
    fn walk_before(root: &Path) -> (Vec<PathBuf>, u64) {
        let (mut stack, mut found, mut n) = (vec![root.to_path_buf()], vec![], 0);
        while let Some(p) = stack.pop() {
            let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
            if is_link(&m) || !m.is_dir() {
                continue;
            }
            n += 1;
            if is_project(&p) {
                found.push(p);
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&p) else { continue };
            for e in entries.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) && !skip_dir(&e.file_name().to_string_lossy()) && !asks_permission(&e.path()) {
                    stack.push(e.path());
                }
            }
        }
        found.sort();
        (found, n)
    }

    /// Before and after, on a tree of 50,000 folders with a file in each: `cargo test -p orbiters-projects --lib -- --ignored
    /// --nocapture a_measured_walk`.
    #[test]
    #[ignore = "a measurement: makes 50,000 folders"]
    fn a_measured_walk_of_fifty_thousand_folders() {
        let temp = tempfile::Builder::new().prefix("walk").tempdir().unwrap();
        let (root, file) = (temp.path().join("Root"), temp.path().join("folders.bin"));
        for a in 0..50 {
            for b in 0..40 {
                for c in 0..24 {
                    let at = root.join(format!("folder {a}/group {b}/item {c}"));
                    std::fs::create_dir_all(&at).unwrap();
                    std::fs::write(at.join("a file.txt"), "x").unwrap();
                }
            }
            if a % 5 == 0 {
                project(&root.join(format!("folder {a}/Project")));
            }
        }
        settle(&root);
        let timed = |what: &str, f: &mut dyn FnMut() -> (Vec<PathBuf>, u64)| {
            let mut best = f64::MAX;
            let mut got = (vec![], 0);
            for _ in 0..3 {
                let started = Instant::now();
                got = f();
                best = best.min(started.elapsed().as_secs_f64());
            }
            println!("{what}: {:.2} s, {} folders, {} projects", best, got.1, got.0.len());
            got.0
        };
        let before = timed("as it was, one thread", &mut || walk_before(&root));
        let mut last = Walked::default();
        for threads in [1, 2, 4] {
            let found = timed(&format!("everything listed, {threads} threads"), &mut || {
                let (found, walked) = walk(&root, threads, &Arc::default());
                last = walked;
                (found, last.folders)
            });
            assert_eq!(found, before);
        }
        let index = kept(&last, &file);
        println!("the index: {} bytes", std::fs::metadata(&file).unwrap().len());
        for threads in [1, 2, 4] {
            let found = timed(&format!("nothing changed, {threads} threads"), &mut || {
                let (found, walked) = walk(&root, threads, &index);
                (found, walked.folders)
            });
            assert_eq!(found, before);
        }
    }
}
