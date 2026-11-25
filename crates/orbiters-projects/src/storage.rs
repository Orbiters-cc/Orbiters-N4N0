//! Project-specific Unity disk failures and recovery of regenerable Library data.
//!
//! Three things live here, kept apart on purpose:
//!
//! - [`inspect`] and [`Health::notice`]: what the panel says about a project's disk, read every few seconds.
//! - [`preflight`]: what Open in Unity does by itself, which may only ever be instant. Enzo clicked Open on a 40 GB
//!   project with 7.2 GB free and watched "Opening Unity…" spin: Open had started walking and copying the whole Library
//!   to a USB disk, byte-for-byte verified, a matter of hours, with no progress and no way to stop it.
//! - [`repair`]: the long move, only when asked for: progress, Cancel, the original intact until the copy is verified.
//!
//! [`remove_orphans`] takes away what a repair could not: closing the app in the middle of a copy kills the thread and
//! leaves a cache folder no Library points at.
use orbiters_i18n::tr;
use anyhow::{bail, Context};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const IMPORT_RESERVE: u64 = 8 * 1024 * 1024 * 1024;

/// The largest Library Open in Unity moves by itself on a low disk (see [`preflight`]): 32 MiB in at most
/// [`INSTANT_ENTRIES`] files and folders. A Library that ever finished an import holds gigabytes in tens of thousands
/// of files, so this only catches one Unity barely started, where the move costs nothing and helps the first import.
/// The numbers bound the worst case on the slowest pair of disks here, a busy hard disk to a USB disk at some 30 MB/s
/// and a few milliseconds per file: about two seconds, short enough to pass unnoticed under "Opening Unity…". The walk
/// that decides stops at the limit, so deciding costs no more than that on a Library of any size.
pub const INSTANT_BYTES: u64 = 32 * 1024 * 1024;
pub const INSTANT_ENTRIES: usize = 256;

/// Names the cache folder instead of the fixed disks' (`Orbiters/UnityCache`), for screenshot runs and experiments on a
/// stand-in project: with it set, repairs copy only there and orphan removal looks only there.
pub const CACHE_DIR_VAR: &str = "ORBITERS_UNITY_CACHE_DIR";

const BACKUP_PREFIX: &str = "Library.orbiters-relocation-"; // i18n: not shown (a folder name)
/// The file a repair puts in its cache before the cache can become Library: such a cache is never removed on its
/// record's word (verifier, point 3).
const SWITCHED: &str = ".orbiters-switched"; // i18n: not shown (a file name)
/// The file a repair puts in its cache when it makes it, and removes when the cache may become Library: the only proof
/// a sweep accepts that a cache was never Library (verifier round 2: no marker means keep, always).
const COPYING: &str = ".orbiters-copying"; // i18n: not shown (a file name)

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Health {
    pub free: Option<u64>,
    pub disk_error: bool,
    pub open: bool,
    /// The disk `free` was read on ("H:\"): the one the Library really is on, through a redirect.
    pub disk: Option<PathBuf>,
}

/// What the panel has to say about a project's disk. Running low and Unity having actually failed are different things
/// to be told: the first does not stop anything, and the project opens as usual.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Notice {
    /// Under [`IMPORT_RESERVE`] free; Unity has not complained.
    RunningLow,
    /// Under [`IMPORT_RESERVE`] free, and Unity's log for this project says it ran out of room.
    OutOfSpace,
    /// Unity's log for this project says it ran out of room, but the disk has room now (or could not be read): there is
    /// nothing to move, opening the project again lets it finish.
    FailedEarlier,
}

impl Health {
    pub fn needs_repair(&self) -> bool {
        self.free.is_some_and(|n| n < IMPORT_RESERVE)
    }
    pub fn notice(&self) -> Option<Notice> {
        match (self.needs_repair(), self.disk_error) {
            (true, true) => Some(Notice::OutOfSpace),
            (true, false) => Some(Notice::RunningLow),
            (false, true) => Some(Notice::FailedEarlier),
            (false, false) => None,
        }
    }
}

pub fn matches_disk_error(log: &str, project: &Path) -> bool {
    // The whole text made comparable once, and the project's folder once (PROJECTS-24 c): it was two strings a line and a third for the
    // folder, some 20 000 lines of a 1 MiB log every 3 s. Lower case is a character's own here (a line's end is not a letter).
    let folder = format!("{}/", project.to_string_lossy().replace('\\', "/").to_lowercase());
    let log = log.replace('\\', "/").to_lowercase();
    log.lines().any(|line| (line.contains("enospc") || line.contains("no space left on device")) && line.contains(&folder))
}

/// Whether Unity's log (its last 1 MiB) says `project`'s disk was full. The answer is kept while the log keeps its length and time
/// (PROJECTS-24 c): a project's panel asks every 3 s, and the log changes only while Unity writes to it.
fn disk_error_in_log(log: Option<PathBuf>, project: &Path) -> bool {
    type Answer = (PathBuf, u64, std::time::SystemTime, PathBuf, bool);
    static LAST: Mutex<Option<Answer>> = Mutex::new(None);
    let Some(mut file) = log.as_ref().and_then(|p| std::fs::File::open(p).ok()) else { return false };
    let log = log.unwrap_or_default();
    let meta = file.metadata().ok();
    let len = meta.as_ref().map_or(0, |m| m.len());
    let modified = meta.and_then(|m| m.modified().ok());
    if let (Some(modified), Some((l, n, m, p, answer))) = (modified, &*LAST.lock().unwrap_or_else(|e| e.into_inner())) {
        if *l == log && *n == len && *m == modified && p == project {
            return *answer;
        }
    }
    let mut text = Vec::new();
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(1024 * 1024)));
    let _ = file.take(1024 * 1024).read_to_end(&mut text);
    let answer = matches_disk_error(&String::from_utf8_lossy(&text), project);
    if let Some(modified) = modified {
        *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some((log, len, modified, project.to_path_buf(), answer));
    }
    answer
}

/// The disk a path really is on, through links: "H:\".
#[cfg(not(unix))]
fn disk_of(path: &Path) -> Option<PathBuf> {
    let real = path.canonicalize().ok()?;
    let text = real.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    let root = Path::new(text).components().next()?;
    Some(PathBuf::from(format!("{}\\", root.as_os_str().to_string_lossy())))
}

/// The disk a path really is on, through links: `/` for the startup disk, `/Volumes/<name>` for another.
#[cfg(target_os = "macos")]
fn disk_of(path: &Path) -> Option<PathBuf> {
    crate::mac::volume_of(path).map(|v| v.mount)
}

/// Other systems of this kind list no disks to move a Library to ([`fixed_disks`](crate::discovery::fixed_disks)).
#[cfg(all(unix, not(target_os = "macos")))]
fn disk_of(path: &Path) -> Option<PathBuf> {
    path.canonicalize().ok().map(|_| PathBuf::from("/"))
}

pub fn inspect(project: &Path) -> Health {
    let library = project.join("Library");
    let location = if library.is_dir() { &library } else { project };
    Health {
        free: crate::size::free_space(location),
        disk_error: disk_error_in_log(crate::editor_log(), project),
        open: crate::unity_open(project),
        disk: disk_of(location),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Where a cache goes.
// ---------------------------------------------------------------------------------------------------------------------

/// The cache folder of one disk: the local application-data folder's on the system disk, `Orbiters/UnityCache` at the
/// top of any other.
fn cache_root_of(disk: &Path) -> PathBuf {
    let local = dirs::data_local_dir();
    local.filter(|p| p.starts_with(disk)).unwrap_or_else(|| disk.to_path_buf()).join("Orbiters/UnityCache")
}

/// Every cache folder a repair may have used.
fn cache_roots() -> Vec<PathBuf> {
    if let Some(dir) = std::env::var_os(CACHE_DIR_VAR).filter(|d| !d.is_empty()) {
        return vec![PathBuf::from(dir)];
    }
    #[cfg(target_os = "macos")]
    return disks_used();
    #[cfg(not(target_os = "macos"))]
    crate::discovery::fixed_disks().iter().map(|d| cache_root_of(d)).collect()
}

/// The list of cache folders repairs copied to, on the Mac. macOS asks the person's permission before an app looks at an
/// external disk, so that cannot happen each time the window opens on the chance that a repair once used it.
#[cfg(target_os = "macos")]
fn used_file() -> PathBuf {
    crate::data_dir().join("unity-cache-folders.json")
}

/// The cache folders that may hold something: this disk's and the ones repairs copied to.
#[cfg(target_os = "macos")]
fn disks_used() -> Vec<PathBuf> {
    let listed: Vec<PathBuf> = std::fs::read(used_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let mut roots = vec![cache_root_of(Path::new("/"))];
    for root in listed {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

/// Keeps `root` in that list, before anything is copied there. A stand-in folder ([`CACHE_DIR_VAR`]) is not listed: it is
/// the only one looked at for as long as it is set.
#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
fn remember_cache_root(root: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    if std::env::var_os(CACHE_DIR_VAR).is_none_or(|d| d.is_empty()) && !disks_used().iter().any(|r| r == root) {
        let mut roots = disks_used();
        roots.push(root.to_path_buf());
        crate::write_json(&used_file(), &roots)?;
    }
    Ok(())
}

/// Cache folders to copy into, best first: the system disk's while it has room (it is the fastest here), then the disk
/// with the most room. Never the Library's own disk: moving within a disk frees nothing.
fn destinations(library: &Path) -> Vec<(PathBuf, u64)> {
    if let Some(dir) = std::env::var_os(CACHE_DIR_VAR).filter(|d| !d.is_empty()) {
        let dir = PathBuf::from(dir);
        let free = crate::size::free_space(&dir).unwrap_or(0);
        return vec![(dir, free)];
    }
    // With no Library yet, the disk it would be made on.
    let own = disk_of(library).or_else(|| library.parent().and_then(disk_of)).map(|d| crate::discovery::key(&d));
    let local = dirs::data_local_dir();
    let mut disks: Vec<(PathBuf, u64)> = crate::discovery::fixed_disks()
        .into_iter()
        .filter(|d| own.as_deref() != Some(crate::discovery::key(d).as_str()))
        .map(|d| {
            let free = crate::size::free_space(&d).unwrap_or(0);
            (d, free)
        })
        .collect();
    // A second APFS volume shares all its room with the first when they are of one container: nothing would be freed.
    #[cfg(target_os = "macos")]
    {
        let own = crate::mac::volume_of(library).or_else(|| library.parent().and_then(crate::mac::volume_of));
        disks.retain(|(d, _)| !own.as_ref().is_some_and(|own| crate::mac::volume_of(d).is_some_and(|v| crate::mac::share_room(own, &v))));
    }
    disks.sort_by_key(|(d, free)| (!local.as_ref().is_some_and(|p| p.starts_with(d)), std::cmp::Reverse(*free)));
    disks.into_iter().map(|(d, free)| (cache_root_of(&d), free)).collect()
}

/// The room a Library of `bytes` takes on its new disk, with the room Unity needs to go on importing there.
pub fn room_needed(bytes: u64) -> u64 {
    bytes.saturating_add(IMPORT_RESERVE)
}

/// The first destination with room for `needed`.
fn with_room(destinations: &[(PathBuf, u64)], needed: u64) -> Option<(PathBuf, u64)> {
    destinations.iter().find(|(_, free)| *free >= needed).cloned()
}

// ---------------------------------------------------------------------------------------------------------------------
// What Open in Unity does by itself.
// ---------------------------------------------------------------------------------------------------------------------

/// What Open in Unity does about a low disk, decided without touching anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Preflight {
    /// Opens as it is, the panel's warning staying: the disk has room, or a move would not be instant.
    Leave,
    /// No Library yet, or a tiny one (these files): moving it costs nothing and gives the first import a disk with room.
    Move(Vec<(PathBuf, u64)>),
}

/// Whether a Library of this size is one to move while opening: the whole of it within [`INSTANT_BYTES`] and
/// [`INSTANT_ENTRIES`], no link in it. Stops walking at the limits, so a 40 GB Library is answered in a moment.
fn tiny_library(library: &Path) -> Option<Vec<(PathBuf, u64)>> {
    let mut files = Vec::new();
    let mut bytes = 0u64;
    for (seen, entry) in walkdir::WalkDir::new(library).follow_links(false).into_iter().enumerate() {
        if seen > INSTANT_ENTRIES {
            return None;
        }
        let entry = entry.ok()?;
        let meta = entry.metadata().ok()?;
        if crate::discovery::is_link(&meta) {
            return None;
        }
        if entry.file_type().is_file() {
            bytes = bytes.saturating_add(meta.len());
            if bytes > INSTANT_BYTES {
                return None;
            }
            files.push((entry.path().to_path_buf(), meta.len()));
        }
    }
    Some(files)
}

/// See [`Preflight`].
pub fn preflight_decision(project: &Path, health: &Health) -> Preflight {
    if !health.needs_repair() || health.open {
        return Preflight::Leave;
    }
    let library = project.join("Library");
    match std::fs::symlink_metadata(&library) {
        // Already redirected: its disk is the full one, and only the person can free it.
        Ok(meta) if crate::discovery::is_link(&meta) => Preflight::Leave,
        Ok(_) => tiny_library(&library).map_or(Preflight::Leave, Preflight::Move),
        Err(_) => Preflight::Move(Vec::new()),
    }
}

/// What opening does about the project's disk: never a long operation (see [`INSTANT_BYTES`]). `Some` when it moved a
/// Library that barely existed. A problem doing so is returned for the caller to note, and must not stop Unity opening:
/// the panel's warning says what to do about the disk.
pub fn preflight(project: &Path) -> anyhow::Result<Option<PathBuf>> {
    let health = Health {
        free: crate::size::free_space(&{
            let library = project.join("Library");
            if library.is_dir() {
                library
            } else {
                project.to_path_buf()
            }
        }),
        open: crate::unity_open(project),
        ..Health::default()
    };
    preflight_with(project, &health, |library, needed| {
        let root = with_room(&destinations(library), needed).map(|(root, _)| root).with_context(|| tr("No local disk has room for this project's Library."))?;
        remember_cache_root(&root)?;
        Ok(root)
    })
}

/// [`preflight`] with its health and destination given. `pick`: the cache folder for a Library of `needed` bytes.
#[cfg_attr(not(any(windows, unix)), allow(unused_variables))]
fn preflight_with(project: &Path, health: &Health, pick: impl FnOnce(&Path, u64) -> anyhow::Result<PathBuf>) -> anyhow::Result<Option<PathBuf>> {
    let Preflight::Move(files) = preflight_decision(project, health) else { return Ok(None) };
    #[cfg(not(any(windows, unix)))]
    return Ok(None);
    #[cfg(any(windows, unix))]
    {
        drop(files);
        let project = project.canonicalize()?;
        let _lock = project_lock(&project)?;
        let library = project.join("Library");
        // Decided again under the lock and from the canonical folder: relocate takes paths inside that Library.
        let Preflight::Move(files) = preflight_decision(&project, health) else { return Ok(None) };
        let bytes: u64 = files.iter().map(|(_, n)| *n).sum();
        let root = pick(&library, room_needed(bytes))?;
        let target = root.join(uuid::Uuid::new_v4().simple().to_string());
        let progress = SharedProgress::default();
        let cancel = AtomicBool::new(false);
        let work = Work { progress: &progress, cancel: &cancel };
        match relocate(&project, &target, &files, &work, |target, library| redirect(target, library))? {
            RepairEnd::Moved(_) => Ok(Some(target)),
            RepairEnd::Cancelled { .. } => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The repair the person asks for.
// ---------------------------------------------------------------------------------------------------------------------

/// Where a repair is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub enum Phase {
    #[default]
    Starting,
    /// Taking away what an interrupted repair left, so that it does not count against the disks.
    Leftovers,
    /// Walking the Library.
    Measuring,
    /// Copying and checking every file. Cancelling is still free.
    Copying,
    /// Pointing Library at the copy: a moment, and past the point of cancelling.
    Switching,
    /// The move is done; the old Library is going.
    RemovingOld,
    /// Cancelled or failed; the partial copy is going. The original was never touched.
    RemovingPartial,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Progress {
    pub phase: Phase,
    /// The Library's size, growing while [`Phase::Measuring`].
    pub bytes_total: u64,
    pub files_total: u64,
    /// Written to the copy.
    pub bytes_copied: u64,
    /// Read back and found identical to the original.
    pub bytes_checked: u64,
    /// Copied and checked.
    pub files_done: u64,
    /// Files deleted, in the phases that delete.
    pub files_removed: u64,
    /// How many there are to delete.
    pub files_to_remove: u64,
    /// The file being copied, inside Library.
    pub file: String,
    /// The folder the copy goes to.
    pub destination: Option<PathBuf>,
}

impl Progress {
    /// How far the phase got, when that is known. Writing and reading back take about as long as each other, so a
    /// file's bytes count once for each and the bar does not stall on a large file's check.
    pub fn fraction(&self) -> Option<f32> {
        match self.phase {
            Phase::Copying => {
                let total = self.bytes_total.max(1) as f64 * 2.0;
                Some(((self.bytes_copied + self.bytes_checked) as f64 / total).clamp(0.0, 1.0) as f32)
            }
            Phase::RemovingOld | Phase::RemovingPartial if self.files_to_remove > 0 => {
                Some((self.files_removed as f64 / self.files_to_remove as f64).clamp(0.0, 1.0) as f32)
            }
            _ => None,
        }
    }
    /// Whether Cancel still does something: not once Library was switched, nor while cancelling.
    pub fn cancellable(&self) -> bool {
        matches!(self.phase, Phase::Starting | Phase::Leftovers | Phase::Measuring | Phase::Copying)
    }
}

pub type SharedProgress = Arc<Mutex<Progress>>;

fn update(progress: &SharedProgress, f: impl FnOnce(&mut Progress)) {
    f(&mut progress.lock().unwrap_or_else(|e| e.into_inner()));
}

/// What stops a repair from being offered, or where it would go.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Verdict {
    /// Can run: copies into this folder, on a disk with this much room.
    Ready { cache_root: PathBuf, free: u64 },
    /// Library is already a redirect, to here: its disk is the full one, and only freeing space there helps.
    Redirected { to: Option<PathBuf> },
    /// Library holds a link, so copying it would not preserve it.
    HasLink { at: PathBuf },
    /// No other local disk has room for the Library and Unity's imports.
    NoRoom { needed: u64, best: Option<(PathBuf, u64)> },
    /// Where there is nothing to redirect a folder with.
    Unsupported,
}

/// What a repair would do, told before it is asked for.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Plan {
    pub bytes: u64,
    pub files: u64,
    pub verdict: Verdict,
}

fn is_cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Relaxed)
}

/// Walks Library: its files and their lengths, or the link it holds (`Ok(Err)`). `None` when cancelled.
fn walk_library(library: &Path, progress: &SharedProgress, cancel: &AtomicBool) -> anyhow::Result<Option<Result<Vec<(PathBuf, u64)>, PathBuf>>> {
    let mut files = Vec::new();
    let (mut bytes, mut count) = (0u64, 0u64);
    if !library.is_dir() {
        return Ok(Some(Ok(files)));
    }
    for (i, entry) in walkdir::WalkDir::new(library).follow_links(false).into_iter().enumerate() {
        if is_cancelled(cancel) {
            return Ok(None);
        }
        let entry = entry.with_context(|| tr!("Could not read {path}", path = library.display()))?;
        let meta = entry.metadata().with_context(|| tr!("Could not read {path}", path = entry.path().display()))?;
        if crate::discovery::is_link(&meta) {
            return Ok(Some(Err(entry.path().to_path_buf())));
        }
        if entry.file_type().is_file() {
            bytes = bytes.saturating_add(meta.len());
            count += 1;
            files.push((entry.path().to_path_buf(), meta.len()));
        }
        if i % 256 == 0 {
            update(progress, |p| {
                p.bytes_total = bytes;
                p.files_total = count;
            });
        }
    }
    update(progress, |p| {
        p.bytes_total = bytes;
        p.files_total = count;
    });
    Ok(Some(Ok(files)))
}

fn link_target(link: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Ok(target) = junction::get_target(link) {
            return Some(target);
        }
    }
    std::fs::read_link(link).ok()
}

/// What a repair of `project` would do: measures its Library, on a worker, with `progress` counting as it goes. Nothing
/// is changed. `None` when cancelled.
pub fn plan(project: &Path, progress: &SharedProgress, cancel: &AtomicBool) -> anyhow::Result<Option<Plan>> {
    update(progress, |p| *p = Progress { phase: Phase::Measuring, ..Progress::default() });
    let library = project.join("Library");
    if cfg!(not(any(windows, unix))) {
        return Ok(Some(Plan { bytes: 0, files: 0, verdict: Verdict::Unsupported }));
    }
    if std::fs::symlink_metadata(&library).is_ok_and(|m| crate::discovery::is_link(&m)) {
        return Ok(Some(Plan { bytes: 0, files: 0, verdict: Verdict::Redirected { to: link_target(&library) } }));
    }
    let Some(walked) = walk_library(&library, progress, cancel)? else { return Ok(None) };
    let files = match walked {
        Ok(files) => files,
        Err(at) => return Ok(Some(Plan { bytes: 0, files: 0, verdict: Verdict::HasLink { at } })),
    };
    let bytes = files.iter().map(|(_, n)| *n).sum();
    let found = destinations(&library);
    Ok(Some(Plan { bytes, files: files.len() as u64, verdict: verdict_for(bytes, &found) }))
}

fn verdict_for(bytes: u64, destinations: &[(PathBuf, u64)]) -> Verdict {
    let needed = room_needed(bytes);
    match with_room(destinations, needed) {
        Some((cache_root, free)) => Verdict::Ready { cache_root, free },
        None => Verdict::NoRoom { needed, best: destinations.iter().max_by_key(|(_, free)| *free).cloned() },
    }
}

/// How a repair ended, short of an error.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum RepairEnd {
    Moved(Moved),
    /// Cancelled: Library is where it was. `left`: the partial copy, when it could not be deleted (it goes with the next
    /// [`remove_orphans`]).
    Cancelled { left: Option<PathBuf> },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Moved {
    pub target: PathBuf,
    pub bytes: u64,
    pub files: u64,
    /// The old Library, when it could not be deleted after the switch (the move itself succeeded; it goes with the next
    /// [`remove_orphans`]).
    pub old_left: Option<PathBuf>,
}

/// Moves `project`'s Library to a disk with room, showing how far it got in `progress`; run it on a worker. `cancel` stops
/// it at any time before the switch: Library stays as it was and the partial copy is deleted. A failure does the same
/// and says so. Unity must have quit: this holds the project's lock, so Unity cannot open it meanwhile (and is told so).
pub fn repair(project: &Path, progress: &SharedProgress, cancel: &AtomicBool) -> anyhow::Result<RepairEnd> {
    {
        use crate::unity6::authority::{allow, Action};
        allow(&crate::data_dir(), project, Action::Storage).map_err(|why| anyhow::anyhow!("{why}"))?;
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (project, progress, cancel);
        bail!("{}", tr("Not enough space for Unity imports. Free at least 8 GB on the project's disk."));
    }
    #[cfg(any(windows, unix))]
    {
        update(progress, |p| *p = Progress { phase: Phase::Leftovers, ..Progress::default() });
        // Whatever an interrupted repair left may be what stands between this one and the room it needs.
        let leftovers = remove_orphans(cancel);
        for (path, why) in &leftovers.failed {
            eprintln!("could not remove {}: {why}", path.display());
        }
        repair_with(project, progress, cancel, |library, needed| {
            let found = destinations(library);
            let root = with_room(&found, needed).map(|(root, _)| root).with_context(|| match found.iter().max_by_key(|(_, free)| *free) {
                Some((root, free)) => tr!(
                    "No local disk has room: the Library and Unity's imports need {needed} bytes; the most any other disk has is {free} bytes ({disk}).",
                    needed = needed,
                    free = free,
                    disk = root.display()
                ),
                None => tr("No other local disk was found to move the Library to.").to_string(),
            })?;
            remember_cache_root(&root)?;
            Ok(root)
        })
    }
}

/// [`repair`] with its destination given. `pick`: the cache folder for a Library needing `needed` bytes.
#[cfg(any(windows, unix))]
fn repair_with(project: &Path, progress: &SharedProgress, cancel: &AtomicBool, pick: impl FnOnce(&Path, u64) -> anyhow::Result<PathBuf>) -> anyhow::Result<RepairEnd> {
    anyhow::ensure!(project.is_dir(), "{}", tr!("The project's folder is missing: {path}", path = project.display()));
    anyhow::ensure!(!crate::unity_open(project), "{}", tr("Unity has this project open. Quit its window, then choose Repair storage."));
    let project = project.canonicalize()?;
    let _lock = project_lock(&project)?;
    let library = project.join("Library");
    if std::fs::symlink_metadata(&library).is_ok_and(|m| crate::discovery::is_link(&m)) {
        bail!("{}", tr("The project's Library is already redirected and its disk is full. Free space on that disk before retrying."));
    }
    update(progress, |p| p.phase = Phase::Measuring);
    let Some(walked) = walk_library(&library, progress, cancel)? else { return Ok(RepairEnd::Cancelled { left: None }) };
    let files = walked.map_err(|at| anyhow::anyhow!("{}", tr!("Library contains a linked folder ({path}); automatic relocation stopped without changing it.", path = at.display())))?;
    let bytes: u64 = files.iter().map(|(_, n)| *n).sum();
    // Checked again now, not only when the panel said what it would do: the disks may have filled since.
    let root = pick(&library, room_needed(bytes))?;
    let target = root.join(uuid::Uuid::new_v4().simple().to_string());
    relocate(&project, &target, &files, &Work { progress, cancel }, |target, library| redirect(target, library))
}

/// Makes `link` lead to the folder `target`: an NTFS junction.
#[cfg(windows)]
fn redirect(target: impl AsRef<Path>, link: impl AsRef<Path>) -> std::io::Result<()> {
    crate::disk::link(target, link)
}

#[cfg(windows)]
fn is_redirect(link: impl AsRef<Path>) -> std::io::Result<bool> {
    junction::exists(link)
}

#[cfg(windows)]
fn remove_redirect(link: impl AsRef<Path>) -> std::io::Result<()> {
    crate::disk::unlink(link)
}

/// Makes `link` lead to the folder `target`: a symbolic link, naming the whole path so that it holds wherever it is read
/// from (a project moved by hand takes it along, as on Windows).
#[cfg(unix)]
fn redirect(target: impl AsRef<Path>, link: impl AsRef<Path>) -> std::io::Result<()> {
    crate::disk::link(std::path::absolute(target)?, link)
}

#[cfg(unix)]
fn is_redirect(link: impl AsRef<Path>) -> std::io::Result<bool> {
    Ok(std::fs::symlink_metadata(link)?.file_type().is_symlink())
}

#[cfg(unix)]
fn remove_redirect(link: impl AsRef<Path>) -> std::io::Result<()> {
    crate::disk::remove_file(link)
}

/// What a move reports to and is stopped by.
#[cfg(any(windows, unix))]
struct Work<'a> {
    progress: &'a SharedProgress,
    cancel: &'a AtomicBool,
}

#[cfg(windows)]
fn project_lock(project: &Path) -> anyhow::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    crate::disk::create_dir_all(project.join("Temp"))?;
    crate::disk::open(std::fs::OpenOptions::new().write(true).create(true).truncate(false).share_mode(0), project.join("Temp/UnityLockfile"))
        .with_context(|| tr("Close this project's Unity window before repairing storage"))
}

/// The project's lock here is a lock on the same file, which also stops Unity if it takes one (`flock` and POSIX locks
/// meet on the Mac). An editor that has the project open without one is looked for by its command line too.
#[cfg(unix)]
fn project_lock(project: &Path) -> anyhow::Result<std::fs::File> {
    // What the person is told when something else holds the lock; asked for where it is told, so it follows the language of that moment.
    fn held() -> &'static str {
        tr("Close this project's Unity window before repairing storage")
    }
    crate::disk::create_dir_all(project.join("Temp"))?;
    let file = crate::disk::open(std::fs::OpenOptions::new().write(true).create(true).truncate(false), project.join("Temp/UnityLockfile")).with_context(held)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => bail!("{}", held()),
        // A file system that has no locks (exFAT): nobody can hold one there, Unity included.
        Err(std::fs::TryLockError::Error(e)) if e.kind() == std::io::ErrorKind::Unsupported || e.raw_os_error() == Some(libc::ENOTSUP) => {}
        Err(std::fs::TryLockError::Error(e)) => return Err(e).with_context(held),
    }
    anyhow::ensure!(crate::editor::of(&crate::editor::running(), project, false).is_none(), "{}", held());
    Ok(file)
}

/// The bytes compared at a time when a copy is read back, and what a read past the file cache wants its memory and its
/// lengths to be multiples of: the sector of the disk, 4096 bytes or a divisor of it on every disk Windows formats.
const CHECK_CHUNK: usize = 1024 * 1024;
const CHECK_ALIGN: usize = 4096;

/// The two buffers of [`copy_and_check`]: the second has room to start on a [`CHECK_ALIGN`] boundary.
#[cfg(any(windows, unix))]
fn check_buffers() -> [Vec<u8>; 2] {
    [vec![0u8; CHECK_CHUNK], vec![0u8; CHECK_CHUNK + CHECK_ALIGN]]
}

/// How a copy just written is read back, and made to last: the file system's own ways ([`OnDisk`]), or a test's, which
/// refuses a read or fails a flush where no disk at hand ever would.
///
/// Two separate things are asked of a moved Library before its original goes, and neither stands for the other:
///
/// 1. **That the copy holds the original's bytes** ([`same_bytes`]). The copy is read back and compared. Read back
///    through the system's file cache, as it used to be, it was compared with the very memory it had been written from
///    a moment earlier, whatever became of the write. So it is read past the cache where that can be done
///    ([`Written::past_cache`]): the file system then writes out what it still holds of the file and hands back what
///    the disk answers. Elsewhere it is written out first, which reports a write that failed, and read through the
///    cache ([`Written::flushed`]). Either way the comparison proves what the file system and the disk answer now. It
///    does not prove that the bytes are on the platter or in the flash: a disk answers from its own write cache.
/// 2. **That the disk's cache is emptied** ([`Written::persist`]). Once every file has passed, and before Library is
///    switched and the original removed, the last file written is flushed all the way. What that establishes: the
///    disk empties its write cache, which holds every file's bytes, each having been written out to be compared.
///    What it does not establish by any documented rule: that the earlier files' own entries (their names, sizes,
///    where their bytes lie) are on the disk too. On NTFS and ReFS that rests on the file system's journal; on a disk
///    without one every file is flushed all the way instead ([`flushes_every_file`]). What nothing here can reach:
///    a disk that reports a flush it did not do, or what a network share's server does with its cache.
#[cfg(any(windows, unix))]
trait Written {
    /// The copy, opened to be read past the system's file cache; `None` where that cannot be done.
    fn past_cache(&self, path: &Path) -> Option<Box<dyn Read>>;
    /// The copy, opened once the system has written it out (which reports a write that failed), to be read through
    /// the cache.
    fn flushed(&self, path: &Path) -> std::io::Result<Box<dyn Read>>;
    /// Flushes the copy at `path` all the way: the file system's log and the disk's own write cache.
    fn persist(&self, path: &Path) -> std::io::Result<()>;
    /// Whether every file is flushed all the way once it has passed, or the move's last file alone.
    fn persists_every_file(&self) -> bool;
}

/// The file system's own ways, on the disk a Library is moved to.
#[cfg(any(windows, unix))]
struct OnDisk {
    /// Every file flushed all the way, or the move's last file alone: by the destination's file system
    /// ([`flushes_every_file`]).
    every_file: bool,
}

#[cfg(any(windows, unix))]
impl OnDisk {
    /// For a move to the folder `destination`, there yet or not. Off Windows each copy is written out before it is
    /// read back, and the last one flushed all the way ([`written_out`]): as it was.
    fn to(destination: &Path) -> Self {
        Self { every_file: cfg!(windows) && flushes_every_file(file_system_of(destination).as_deref()) }
    }
}

#[cfg(any(windows, unix))]
impl Written for OnDisk {
    /// On Windows, `FILE_FLAG_NO_BUFFERING`. A disk or a driver that refuses such an open has none.
    fn past_cache(&self, path: &Path) -> Option<Box<dyn Read>> {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows::Win32::Storage::FileSystem::{FILE_FLAG_NO_BUFFERING, FILE_FLAG_SEQUENTIAL_SCAN};
            let file = std::fs::OpenOptions::new().read(true).custom_flags(FILE_FLAG_NO_BUFFERING.0 | FILE_FLAG_SEQUENTIAL_SCAN.0).open(path).ok()?;
            Some(Box::new(file))
        }
        #[cfg(not(windows))]
        {
            let _ = path;
            None
        }
    }

    fn flushed(&self, path: &Path) -> std::io::Result<Box<dyn Read>> {
        // Write access only to have it written out: a read-only handle cannot on Windows.
        written_out(&crate::disk::open(std::fs::OpenOptions::new().write(true), path)?)?;
        Ok(Box::new(std::fs::File::open(path)?))
    }

    /// `FlushFileBuffers` on Windows, `F_FULLFSYNC` on a Mac, `fsync` elsewhere: what `sync_all` is on each.
    fn persist(&self, path: &Path) -> std::io::Result<()> {
        crate::disk::open(std::fs::OpenOptions::new().write(true), path)?.sync_all()
    }

    fn persists_every_file(&self) -> bool {
        self.every_file
    }
}

/// The file systems whose journal a flush of one file commits for the files before it.
const JOURNALED: [&str; 2] = ["NTFS", "ReFS"];

/// Whether a Library moved to a disk of this file system (as Windows names it; `None` when it could not be asked) has
/// every file flushed all the way before its original goes, or its last file alone.
///
/// | The destination's file system | What is flushed all the way |
/// |---|---|
/// | NTFS, ReFS | the last file, once a move |
/// | exFAT, FAT32, FAT, any other name, and a disk that cannot be asked | every file |
///
/// Windows documents `FlushFileBuffers` for the file it is given: its bytes and its entry, and the disk's cache with
/// them. A flush of the last file therefore empties the disk's cache of every file's bytes, but that it also puts the
/// earlier files' entries on the disk rests on how NTFS works, not on a documented guarantee: NTFS writes its journal
/// in order, so committing it for one file commits what was logged before (ReFS is trusted the same way). exFAT and
/// FAT have no journal: there an earlier file's entry can still be in the system's memory when the original Library
/// goes, and a power cut at that moment can leave files of the copy missing or short. A Library is often moved to an
/// external drive, and those are often exFAT.
///
/// What a flush a file costs was measured on NTFS (2026-11-10, a release build, the SSD, two runs each, the last file
/// against every file): 1 GiB of 4 MiB files 4.4 and 4.2 s against 6.2 and 6.0 s; 256 MiB of 16 KiB files (16,384 of
/// them) 174 and 157 s against 198 and 231 s. About 3 ms a file, 30 % on the small files a Library is made of: five
/// more minutes on a hundred thousand, on a move people watch. So it is paid only where the journal is not there to
/// do it. Not timed on an exFAT disk: none was at hand (`a_measured_flush_of_every_file_or_the_last` takes a folder).
fn flushes_every_file(file_system: Option<&str>) -> bool {
    !file_system.is_some_and(|name| JOURNALED.iter().any(|journaled| name.eq_ignore_ascii_case(journaled)))
}

/// The file system of the disk the folder `path` is on, or will be (asked of the nearest folder that is there), as
/// Windows names it: `NTFS`, `ReFS`, `exFAT`, `FAT32`. `None` when it cannot be asked, and off Windows.
#[cfg(any(windows, unix))]
fn file_system_of(path: &Path) -> Option<String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{GetVolumeInformationW, GetVolumePathNameW};
        let there = path.ancestors().find(|p| p.exists())?;
        let wide: Vec<u16> = there.as_os_str().encode_wide().chain(Some(0)).collect();
        // The disk's root (or the folder a disk is mounted at): never longer than the path it is the start of.
        let mut root = vec![0u16; wide.len().max(260) + 2];
        // SAFETY: a null-terminated path, and a buffer whose length is given with it.
        unsafe { GetVolumePathNameW(PCWSTR(wide.as_ptr()), &mut root) }.ok()?;
        let mut name = [0u16; 64];
        // SAFETY: the null-terminated root just written, and a buffer whose length is given with it.
        unsafe { GetVolumeInformationW(PCWSTR(root.as_ptr()), None, None, None, None, Some(&mut name[..])) }.ok()?;
        let length = name.iter().position(|unit| *unit == 0).unwrap_or(name.len());
        Some(String::from_utf16_lossy(&name[..length])).filter(|name| !name.is_empty())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

/// Has the system write a file out to its disk, and says so when that failed. Off Windows this is `fsync` itself and
/// not `sync_all`, which on a Mac also has the disk empty its own cache (`F_FULLFSYNC`): milliseconds, for every file
/// of a Library. That part is asked once a move ([`Written::persist`]).
#[cfg(any(windows, unix))]
fn written_out(file: &std::fs::File) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: `fsync` on a descriptor this function's caller holds open.
        if unsafe { libc::fsync(file.as_raw_fd()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        file.sync_all()
    }
}

/// What a read past the cache fails with when its memory or its length does not fit the disk's sectors: Windows'
/// ERROR_INVALID_PARAMETER, `EINVAL` elsewhere.
const REFUSED_SHAPE: i32 = if cfg!(windows) { 87 } else { 22 };

/// Whether a read past the cache was refused for its shape: the disk's sectors are larger than [`CHECK_ALIGN`], so
/// neither the memory's start nor the lengths fit them.
#[cfg(any(windows, unix))]
fn refused_for_its_shape(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(REFUSED_SHAPE)
}

/// Copies one file and reads it back from the disk ([`Written`]). `false` when cancelled, partway through it if it is a
/// large one. The copy keeps the original's modification time and read-only flag, as Library's other readers (and
/// `std::fs::copy`) expect. `persist`: the file is flushed all the way once it has passed ([`Written::persist`]), while
/// it can still be opened to be written to.
#[cfg(any(windows, unix))]
fn copy_and_check(disk: &dyn Written, from: &Path, to: &Path, buffers: &mut [Vec<u8>; 2], work: &Work, persist: bool) -> anyhow::Result<bool> {
    let [left, right] = buffers;
    let mut source = std::fs::File::open(from)?;
    let meta = source.metadata()?;
    let mut copy = crate::disk::create_file(to)?;
    loop {
        if is_cancelled(work.cancel) {
            return Ok(false);
        }
        let n = source.read(left)?;
        if n == 0 {
            break;
        }
        copy.write_all(&left[..n])?;
        update(work.progress, |p| p.bytes_copied += n as u64);
    }
    if let Ok(modified) = meta.modified() {
        let _ = copy.set_modified(modified);
    }
    drop(copy);
    drop(source);
    let same = same_bytes(disk, from, to, left, right, |n| {
        update(work.progress, |p| p.bytes_checked += n as u64);
        !is_cancelled(work.cancel)
    })?;
    match same {
        None => return Ok(false),
        Some(true) => {}
        Some(false) => bail!("{}", tr!("A copied file differs from its original: {path}", path = from.display())),
    }
    if persist {
        disk.persist(to)?;
    }
    // Here a new file is not executable either, and Library holds programs (Burst's compiler, in a package).
    if cfg!(unix) || meta.permissions().readonly() {
        let _ = crate::disk::set_permissions(to, meta.permissions());
    }
    Ok(true)
}

/// Deletes the partial copy of a move that did not happen. The path when it could not be: its `.json` stays beside it, so
/// [`remove_orphans`] finds it.
#[cfg(any(windows, unix))]
fn discard_copy(target: &Path, work: &Work) -> Option<PathBuf> {
    let done = work.progress.lock().unwrap_or_else(|e| e.into_inner()).files_done + 1;
    update(work.progress, |p| {
        p.phase = Phase::RemovingPartial;
        p.files_removed = 0;
        p.files_to_remove = done;
    });
    let removed = if target.exists() {
        remove_tree(target, &mut || update(work.progress, |p| p.files_removed += 1))
    } else {
        Ok(())
    };
    match removed {
        Ok(()) => {
            let _ = crate::disk::remove_file(target.with_extension("json"));
            None
        }
        Err(e) => {
            eprintln!("could not remove the partial copy {}: {e}", target.display());
            Some(target.to_path_buf())
        }
    }
}

/// What a failure's message says about the partial copy.
#[cfg(any(windows, unix))]
fn partial_note(left: &Option<PathBuf>) -> String {
    match left {
        None => tr("The partial copy was deleted.").to_string(),
        Some(p) => tr!("The partial copy at {path} could not be deleted; Orbiters tries again the next time its window opens.", path = p.display()),
    }
}

/// Copies `files` (inside the project's Library) to `target`, checks them, then switches Library to a junction there.
/// Until the switch nothing of the project changes. Any failure or Cancel before it deletes the copy; a failed switch
/// restores Library first.
#[cfg(any(windows, unix))]
fn relocate(project: &Path, target: &Path, files: &[(PathBuf, u64)], work: &Work, link: impl FnOnce(&Path, &Path) -> std::io::Result<()>) -> anyhow::Result<RepairEnd> {
    relocate_on(&OnDisk::to(target), project, target, files, work, link)
}

/// [`relocate`], reading the copy back and flushing it through `disk`: each file compared with its original, and the
/// last one flushed all the way before the switch ([`Written`]).
#[cfg(any(windows, unix))]
fn relocate_on(
    disk: &dyn Written,
    project: &Path,
    target: &Path,
    files: &[(PathBuf, u64)],
    work: &Work,
    link: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> anyhow::Result<RepairEnd> {
    let library = project.join("Library");
    let bytes: u64 = files.iter().map(|(_, n)| *n).sum();
    anyhow::ensure!(!target.exists(), "{}", tr("Cache destination already exists"));
    // The record first: a kill at any moment after this leaves a cache that says which project it was for, and the exact
    // name the old Library will have while it is moved aside: the only folder a later sweep may put back or remove
    // (PROJECTS-33: a folder merely named like one is the person's).
    let backup = project.join(format!("{BACKUP_PREFIX}{}", uuid::Uuid::new_v4().simple()));
    // Its phase too (PROJECTS-21): a cache whose record says it was still being copied was never Library, and a sweep may
    // remove it without asking which projects the gallery knows.
    let record = |phase: &str, backup_id: Option<crate::owned::FileId>| serde_json::json!({"project":project,"library":target,"backup":backup,"phase":phase,"backup_id":backup_id});
    crate::write_json(&target.with_extension("json"), &record("copying", None))?;
    crate::disk::create_dir_all(target.parent().with_context(|| tr("A file without a folder"))?)?;
    crate::disk::create_dir(target)?;
    crate::disk::open(std::fs::File::options().write(true).create_new(true), target.join(COPYING))?;
    update(work.progress, |p| {
        p.phase = Phase::Copying;
        p.bytes_total = bytes;
        p.files_total = files.len() as u64;
        p.bytes_copied = 0;
        p.bytes_checked = 0;
        p.files_done = 0;
        p.destination = Some(target.to_path_buf());
    });
    let mut buffers = check_buffers();
    let mut copy_all = || -> anyhow::Result<bool> {
        for (i, (file, _)) in files.iter().enumerate() {
            let relative = file.strip_prefix(&library)?;
            // A Library copied back from an earlier move holds its cache's markers: they are not taken along.
            if relative == Path::new(SWITCHED) || relative == Path::new(COPYING) {
                continue;
            }
            let to = target.join(relative);
            crate::disk::create_dir_all(to.parent().with_context(|| tr("A file without a folder"))?)?;
            update(work.progress, |p| p.file = relative.display().to_string());
            // Flushed all the way once it has passed, before the switch and before the original goes: every file, or the
            // last one ([`Written::persists_every_file`]).
            let persist = disk.persists_every_file() || i + 1 == files.len();
            if !copy_and_check(disk, file, &to, &mut buffers, work, persist).with_context(|| tr!("Could not copy {path}", path = relative.display()))? {
                return Ok(false);
            }
            update(work.progress, |p| p.files_done += 1);
        }
        Ok(true)
    };
    match copy_all() {
        Ok(true) => {}
        Ok(false) => return Ok(RepairEnd::Cancelled { left: discard_copy(target, work) }),
        Err(error) => {
            let left = discard_copy(target, work);
            return Err(error).with_context(|| tr!("Library was not moved and stays where it is. {note}", note = partial_note(&left)));
        }
    }
    // The last moment Cancel can still leave everything as it was.
    if is_cancelled(work.cancel) {
        return Ok(RepairEnd::Cancelled { left: discard_copy(target, work) });
    }
    update(work.progress, |p| p.phase = Phase::Switching);
    // From here the cache may become Library: no sweep removes it on the gallery's word any more. Two proofs first: a
    // file inside the cache itself (a record restored from an older copy cannot take it away), and the identity of the
    // Library about to be set aside (a folder put later at the backup's name is not it).
    let had_library = library.is_dir();
    let switching = (|| -> anyhow::Result<()> {
        crate::disk::open(std::fs::File::options().write(true).create_new(true), target.join(SWITCHED))?;
        crate::disk::remove_file(target.join(COPYING))?;
        let backup_id = if had_library { Some(crate::owned::id_of(&library)?) } else { None };
        crate::write_json(&target.with_extension("json"), &record("switching", backup_id))
    })();
    if let Err(error) = switching {
        let left = discard_copy(target, work);
        return Err(error).with_context(|| tr!("Library was not moved and stays where it is. {note}", note = partial_note(&left)));
    }
    if had_library {
        if let Err(error) = crate::disk::rename(&library, &backup) {
            let left = discard_copy(target, work);
            return Err(error).with_context(|| tr!("Something is using this project's Library, so it was not moved and stays where it is. {note}", note = partial_note(&left)));
        }
    }
    if let Err(error) = link(target, &library) {
        // A failed reparse-point creation can leave an empty directory behind.
        if is_redirect(&library).unwrap_or(false) {
            remove_redirect(&library)?;
        }
        if library.exists() {
            crate::disk::remove_dir(&library).with_context(|| tr("Could not remove an empty failed Library redirect"))?;
        }
        if had_library {
            // The copy stays when Library cannot come back: it is then the only complete one under that name.
            crate::disk::rename(&backup, &library).with_context(|| {
                tr!(
                    "Could not restore Library after a failed redirect. It is at {backup}, and a checked copy is at {copy}.",
                    backup = backup.display(),
                    copy = target.display()
                )
            })?;
        }
        let left = discard_copy(target, work);
        return Err(error).with_context(|| tr!("Could not redirect Unity's cache, so Library stays where it is. {note}", note = partial_note(&left)));
    }
    let mut old_left = None;
    if had_library {
        update(work.progress, |p| {
            p.phase = Phase::RemovingOld;
            p.files_removed = 0;
            p.files_to_remove = files.len() as u64;
        });
        // The backup is exactly the Library just renamed above, with links rejected before copying.
        if let Err(e) = remove_tree(&backup, &mut || update(work.progress, |p| p.files_removed += 1)) {
            eprintln!("Cache moved to {}, but old cache cleanup failed at {}: {e}", target.display(), backup.display());
            old_left = Some(backup);
        }
    }
    Ok(RepairEnd::Moved(Moved { target: target.to_path_buf(), bytes, files: files.len() as u64, old_left }))
}

/// Whether the file just written at `b` holds the bytes of `a`, as the disk answers them ([`Written`]); `on_chunk`
/// hears the bytes compared and answers whether to go on (`None` when it said to stop). `left` holds at least
/// [`CHECK_CHUNK`] bytes and `right` [`CHECK_ALIGN`] more ([`check_buffers`]).
#[cfg(any(windows, unix))]
fn same_bytes(disk: &dyn Written, a: &Path, b: &Path, left: &mut [u8], right: &mut [u8], mut on_chunk: impl FnMut(usize) -> bool) -> std::io::Result<Option<bool>> {
    if std::fs::metadata(a)?.len() != std::fs::metadata(b)?.len() {
        return Ok(Some(false));
    }
    let mut a = std::fs::File::open(a)?;
    let written = b;
    let (mut b, mut from_disk) = match disk.past_cache(written) {
        Some(b) => (b, true),
        None => (disk.flushed(written)?, false),
    };
    let left = &mut left[..CHECK_CHUNK];
    // Past the cache a read takes memory that starts on a sector's boundary and a whole number of sectors.
    let start = right.as_ptr().align_offset(CHECK_ALIGN);
    let right = &mut right[start..start + CHECK_CHUNK];
    let mut compared = false;
    loop {
        // Whole chunks of both, so that only the last of each is short: the one that meets the file's end.
        let n = read_up_to(&mut a, left, false)?;
        let m = match read_up_to(&mut *b, right, from_disk) {
            // A disk of larger sectors refuses the first read past the cache: written out and through the cache instead,
            // from the file's start. A refusal later on is a failure like any other: part of the file is compared already.
            Err(e) if from_disk && !compared && refused_for_its_shape(&e) => {
                b = disk.flushed(written)?;
                from_disk = false;
                read_up_to(&mut *b, right, false)?
            }
            m => m?,
        };
        compared = true;
        if n != m || left[..n] != right[..n] {
            return Ok(Some(false));
        }
        if n > 0 && !on_chunk(n) {
            return Ok(None);
        }
        if n < CHECK_CHUNK {
            return Ok(Some(true));
        }
    }
}

/// Fills `buffer` from `file`, short only at the file's end. With `sectors` (a file read past the cache), a read that
/// ends inside a sector met the file's end, and nothing more is asked: what is left of the buffer is no whole number
/// of sectors, which such a read refuses.
#[cfg(any(windows, unix))]
fn read_up_to(file: &mut dyn Read, buffer: &mut [u8], sectors: bool) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
        if sectors && filled % CHECK_ALIGN != 0 {
            break;
        }
    }
    Ok(filled)
}

/// Deletes a folder this module made (never following a link out of it), telling `on_file` of each file, going on past a
/// file that cannot be deleted so that as much as possible is freed. The first error, if any.
#[cfg_attr(not(any(windows, unix)), allow(dead_code))]
fn remove_tree(root: &Path, on_file: &mut dyn FnMut()) -> std::io::Result<()> {
    fn remove_link(path: &Path) -> std::io::Result<()> {
        crate::disk::remove_dir(path).or_else(|_| crate::disk::remove_file(path))
    }
    /// What stops a removal here is not the file's own read-only mode but its folder's: packages unpack read-only.
    #[cfg(unix)]
    fn open_parent(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        if let Some(parent) = path.parent() {
            if let Ok(meta) = std::fs::symlink_metadata(parent) {
                let mut open = meta.permissions();
                open.set_mode(open.mode() | 0o700);
                let _ = crate::disk::set_permissions(parent, open);
            }
        }
    }
    fn remove_file(path: &Path, meta: Option<std::fs::Metadata>) -> std::io::Result<()> {
        match crate::disk::remove_file(path) {
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                // A read-only file (the copy keeps the flag).
                if let Some(meta) = meta {
                    let mut writable = meta.permissions();
                    #[allow(clippy::permissions_set_readonly_false)]
                    writable.set_readonly(false);
                    let _ = crate::disk::set_permissions(path, writable);
                }
                #[cfg(unix)]
                open_parent(path);
                crate::disk::remove_file(path)
            }
            other => other,
        }
    }
    fn remove_folder(path: &Path) -> std::io::Result<()> {
        match crate::disk::remove_dir(path) {
            #[cfg(unix)]
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                open_parent(path);
                crate::disk::remove_dir(path)
            }
            other => other,
        }
    }
    if crate::discovery::is_link(&std::fs::symlink_metadata(root)?) {
        return remove_link(root);
    }
    let mut first = None;
    for entry in walkdir::WalkDir::new(root).follow_links(false).contents_first(true) {
        let done = match entry {
            Err(e) => Err(std::io::Error::other(e)),
            Ok(e) => {
                let meta = e.metadata().ok();
                if meta.as_ref().is_some_and(crate::discovery::is_link) {
                    remove_link(e.path())
                } else if e.file_type().is_dir() {
                    remove_folder(e.path())
                } else {
                    let done = remove_file(e.path(), meta);
                    if done.is_ok() {
                        on_file();
                    }
                    done
                }
            }
        };
        if let Err(e) = done {
            first.get_or_insert(e);
        }
    }
    first.map_or(Ok(()), Err)
}

// ---------------------------------------------------------------------------------------------------------------------
// What an interrupted repair left.
// ---------------------------------------------------------------------------------------------------------------------

/// Why a cache stayed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kept {
    /// A project's Library points at it.
    InUse,
    /// The project it names is on a disk that is not here: its Library cannot be checked.
    DiskMissing,
    /// The disk is here, the project's folder is not. It may have been moved, junction and all, so its Library cannot
    /// be checked either.
    ProjectMissing,
    /// Its project is in use (Unity, or a repair running now): looked at again next time.
    Busy,
    /// Without a readable record of the project it was for, or a Library link that cannot be read.
    Unknown,
}

/// What [`remove_orphans`] found.
#[derive(Debug, Default)]
pub struct Swept {
    pub removed: Vec<PathBuf>,
    pub kept: Vec<(PathBuf, Kept)>,
    /// Projects whose Library a kill had left renamed, put back.
    pub restored: Vec<PathBuf>,
    /// Old Libraries left beside a finished move, deleted.
    pub old_removed: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
}

/// Only one sweep at a time: a repair's and the one when the window opens would otherwise fight over a project's lock.
static SWEEP: Mutex<()> = Mutex::new(());

/// Does this Library point at `cache`? `None` when that cannot be told.
fn points_at(library: &Path, cache: &Path) -> Option<bool> {
    use crate::discovery::key;
    let meta = match std::fs::symlink_metadata(library) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(false),
        Err(_) => return None,
    };
    if !crate::discovery::is_link(&meta) {
        return Some(false);
    }
    let target = link_target(library);
    if target.as_deref().is_some_and(|t| key(t) == key(cache)) {
        return Some(true);
    }
    // Through another link, or the same folder under another name.
    let real = library.canonicalize();
    if let (Ok(a), Ok(b)) = (&real, cache.canonicalize()) {
        if key(a) == key(&b) {
            return Some(true);
        }
    }
    (target.is_some() || real.is_ok()).then_some(false)
}

/// Whether the disk a path names is connected. Only a path under `/Volumes` names a disk that can be away.
#[cfg(unix)]
fn disk_present(path: &Path) -> bool {
    use std::path::Component::{Normal, RootDir};
    let mut parts = path.components();
    match (parts.next(), parts.next(), parts.next()) {
        (Some(RootDir), Some(Normal(volumes)), Some(Normal(disk))) if volumes == "Volumes" => Path::new("/Volumes").join(disk).exists(), // i18n: not shown (a folder name; an error text, English until errors are decided; a macOS path part)
        _ => true,
    }
}

/// Whether the disk a path names is connected.
#[cfg(not(unix))]
fn disk_present(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    Path::new(text).components().next().is_some_and(|root| Path::new(&format!("{}\\", root.as_os_str().to_string_lossy())).exists())
}

/// A cache's folder name: what a repair names them (a UUID, 32 hex digits). Nothing else in the folder is looked at.
fn is_cache_id(name: &str) -> bool {
    name.len() == 32 && name.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The folders beside `project`'s Library named like an old Library a move leaves: proof of nothing, a person can name a
/// folder so.
#[cfg(test)]
fn named_like_backups(project: &Path) -> Vec<PathBuf> {
    let Ok(dir) = std::fs::read_dir(project) else { return vec![] };
    dir.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(BACKUP_PREFIX) && e.path().is_dir()).map(|e| e.path()).collect()
}

/// The old Library the move that made a cache set aside, when it is there: the folder its record names, directly in
/// `project`, named as a move names it, and a real folder (not a link). A record without one (an older version's) proves
/// nothing.
fn recorded_backup(record: Option<&serde_json::Value>, project: &Path) -> Option<PathBuf> {
    use crate::discovery::key;
    let backup = PathBuf::from(record?["backup"].as_str()?);
    let name = backup.file_name()?.to_string_lossy().into_owned();
    let id = name.strip_prefix(BACKUP_PREFIX)?;
    let beside = backup.parent().is_some_and(|parent| key(parent) == key(project));
    let real = std::fs::symlink_metadata(&backup).is_ok_and(|m| {
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            m.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse = false;
        m.is_dir() && !m.file_type().is_symlink() && !reparse
    });
    // The very folder the repair set aside, by identity: a folder restored or put at that name later is not it.
    let same = serde_json::from_value::<crate::owned::FileId>(record?["backup_id"].clone()).is_ok_and(|was| crate::owned::is_same(&backup, was));
    (is_cache_id(id) && beside && real && same).then_some(backup)
}

/// What to do with one cache.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Judgement {
    Keep(Kept),
    /// No Library points at it. `restore`: the project has no Library, a kill having left it under this name.
    Remove { project: PathBuf, restore: Option<PathBuf> },
    /// In use, with old Libraries left beside the project's (the switch was made, so the copy was complete).
    KeepAndClean { project: PathBuf, old: Vec<PathBuf> },
}

/// Decides about the cache `<root>/<id>` from its record, the Library of the project it names and of every project
/// `known` (a moved project's Library still points at it, whatever the record says). Reads only.
fn judge(cache: &Path, known: &[PathBuf]) -> Judgement {
    use Judgement::*;
    let record: Option<serde_json::Value> = std::fs::read(cache.with_extension("json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let Some(project) = record.as_ref().and_then(|r| r["project"].as_str()).map(PathBuf::from) else { return Keep(Kept::Unknown) };
    if !disk_present(&project) {
        return Keep(Kept::DiskMissing);
    }
    if !project.is_dir() || !crate::discovery::is_project(&project) {
        return Keep(Kept::ProjectMissing);
    }
    // The record must be this cache's own (a sidecar copied or restored beside another cache is not).
    if record.as_ref().and_then(|r| r["library"].as_str()).is_none_or(|l| crate::discovery::key(Path::new(l)) != crate::discovery::key(cache)) {
        return Keep(Kept::Unknown);
    }
    let own = project.join("Library");
    match points_at(&own, cache) {
        None => return Keep(Kept::Unknown),
        Some(true) => {
            let old: Vec<PathBuf> = recorded_backup(record.as_ref(), &project).into_iter().collect();
            return if old.is_empty() { Keep(Kept::InUse) } else { KeepAndClean { project, old } };
        }
        Some(false) => {}
    }
    // Any project known to point at it keeps it, whatever its record says.
    for other in known {
        match points_at(&other.join("Library"), cache) {
            Some(false) => {}
            Some(true) => return Keep(Kept::InUse),
            None => return Keep(Kept::Unknown),
        }
    }
    // Removal needs the record's proof, never a missing reference alone (a moved project may be nowhere in the list):
    // still copying and never made ready to switch, it was never Library; an interrupted switch at this very project
    // (its Library missing, the very folder set aside still there) is undone. Anything else, an unknown phase or an
    // older version's record without one, stays.
    // Positive proof only: the cache's own "still copying" file there and no switch file; anything that cannot be read
    // keeps the cache.
    let copying = std::fs::symlink_metadata(cache.join(COPYING)).is_ok_and(|m| m.is_file());
    let unswitched = std::fs::symlink_metadata(cache.join(SWITCHED)).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
    match record.as_ref().and_then(|r| r["phase"].as_str()) {
        Some("copying") if copying && unswitched => Remove { project, restore: None },
        Some("switching") => {
            let own_missing = std::fs::symlink_metadata(&own).is_err();
            match recorded_backup(record.as_ref(), &project) {
                Some(old) if own_missing => Remove { project, restore: Some(old) },
                _ => Keep(Kept::Unknown),
            }
        }
        _ => Keep(Kept::Unknown),
    }
}

/// Every cache and record in `roots`, by path of the cache.
fn caches_in(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut caches = Vec::new();
    for root in roots {
        let Ok(dir) = std::fs::read_dir(root) else { continue };
        let mut here: Vec<PathBuf> = dir
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let id = name.strip_suffix(".json").unwrap_or(&name);
                is_cache_id(id).then(|| root.join(id))
            })
            .collect();
        here.sort();
        here.dedup();
        caches.extend(here);
    }
    caches
}

/// What [`remove_orphans`] would do on this computer, changing nothing.
pub fn orphans_preview() -> Vec<(PathBuf, String)> {
    let known: Vec<PathBuf> = crate::discovery::cached().projects.into_iter().map(|p| p.path).collect();
    caches_in(&cache_roots()).into_iter().map(|c| {
        let verdict = judge(&c, &known);
        (c, format!("{verdict:?}"))
    }).collect()
}

/// Removes `Orbiters/UnityCache/<id>` folders and their records on every fixed disk when no Library points at them: what a
/// repair leaves when the window is closed or the app killed mid-copy. Run on a worker, when the window opens and before a
/// repair.
///
/// A cache is kept whenever its being unused cannot be proved: the project named by its record has a Library link there
/// (or any project the gallery knows does: a project moved by hand takes its junction along), the project's disk is
/// missing, its folder is missing, the record cannot be read, its Library link cannot be read, or the project's lock is
/// held. A repair running now holds that lock from before its cache exists until after the switch, so it is never mistaken
/// for what an interrupted one left. A cache unused now can't be picked up again (each repair makes a new one), so the
/// lock is only held to decide, and Unity may open the project while a large orphan is being deleted.
pub fn remove_orphans(cancel: &AtomicBool) -> Swept {
    let known: Vec<PathBuf> = crate::discovery::cached().projects.into_iter().map(|p| p.path).collect();
    sweep(&cache_roots(), &known, cancel)
}

fn sweep(roots: &[PathBuf], known: &[PathBuf], cancel: &AtomicBool) -> Swept {
    let mut swept = Swept::default();
    // Waiting for the other one (it may be deleting tens of gigabytes), but not past a Cancel.
    let _one = loop {
        match SWEEP.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::Poisoned(e)) => break e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) if is_cancelled(cancel) => return swept,
            Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    };
    #[cfg(any(windows, unix))]
    for cache in caches_in(roots) {
        if is_cancelled(cancel) {
            break;
        }
        let verdict = judge(&cache, known);
        match verdict {
            Judgement::Keep(why) => swept.kept.push((cache, why)),
            Judgement::KeepAndClean { project, old } => {
                // Under the lock, looked at again: a repair may be deleting these at this moment.
                let Ok(lock) = project_lock(&project) else {
                    swept.kept.push((cache, Kept::Busy));
                    continue;
                };
                // Decided again under the lock, and removed while it is held: the old Library must still be the very
                // folder the record names.
                let still = points_at(&project.join("Library"), &cache) == Some(true);
                if still {
                    let record: Option<serde_json::Value> = std::fs::read(cache.with_extension("json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
                    let proven = recorded_backup(record.as_ref(), &project);
                    for path in old.into_iter().filter(|p| proven.as_ref() == Some(p)) {
                        match remove_tree(&path, &mut || {}) {
                            Ok(()) => swept.old_removed.push(path),
                            Err(e) => swept.failed.push((path, e.to_string())),
                        }
                    }
                }
                drop(lock);
                swept.kept.push((cache, Kept::InUse));
            }
            Judgement::Remove { project, .. } => {
                let Ok(lock) = project_lock(&project) else {
                    swept.kept.push((cache, Kept::Busy));
                    continue;
                };
                // Decided again under the lock: nothing can change it now.
                let Judgement::Remove { restore, .. } = judge(&cache, known) else {
                    drop(lock);
                    swept.kept.push((cache, Kept::Busy));
                    continue;
                };
                if let Some(old) = restore {
                    if let Err(e) = crate::disk::rename(&old, project.join("Library")) {
                        drop(lock);
                        // The cache may be the only good copy until the old Library is back: it stays.
                        swept.failed.push((old, e.to_string()));
                        swept.kept.push((cache, Kept::Unknown));
                        continue;
                    }
                    swept.restored.push(project.clone());
                }
                drop(lock);
                let removed = if cache.exists() { remove_tree(&cache, &mut || {}) } else { Ok(()) };
                match removed {
                    Ok(()) => {
                        let _ = crate::disk::remove_file(cache.with_extension("json"));
                        swept.removed.push(cache);
                    }
                    Err(e) => swept.failed.push((cache, e.to_string())),
                }
            }
        }
    }
    #[cfg(not(any(windows, unix)))]
    let _ = (roots, known, cancel);
    swept
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in project: its version file, and a Library of `files` files of `size` bytes each.
    fn project(root: &Path, name: &str, files: usize, size: usize) -> PathBuf {
        let project = root.join(name);
        std::fs::create_dir_all(project.join("ProjectSettings")).unwrap();
        std::fs::write(project.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
        std::fs::create_dir_all(project.join("Assets")).unwrap();
        if files > 0 {
            std::fs::create_dir_all(project.join("Library/Artifacts/ab")).unwrap();
            for i in 0..files {
                // Different bytes per file, so that a mix-up would show.
                let bytes: Vec<u8> = (0..size).map(|b| (b.wrapping_mul(i + 7) >> 3) as u8).collect();
                std::fs::write(project.join(format!("Library/Artifacts/ab/{i}.bin")), bytes).unwrap();
            }
        }
        project
    }

    fn low() -> Health {
        Health { free: Some(IMPORT_RESERVE / 2), ..Health::default() }
    }

    fn folder_is_empty(path: &Path) -> bool {
        std::fs::read_dir(path).map_or(true, |mut d| d.next().is_none())
    }

    #[cfg(unix)]
    #[test]
    fn a_full_disk_in_unitys_log_on_the_mac_names_the_project_too() {
        let log = "Failed to write file: ENOSPC: no space left on device, open '/Users/enzo/Unity Projects/Avatar/Library/Artifacts/ab/file'";
        assert!(matches_disk_error(log, Path::new("/Users/enzo/Unity Projects/Avatar")));
        assert!(!matches_disk_error(log, Path::new("/Users/enzo/Unity Projects/Avatar copy")));
    }

    #[cfg(unix)]
    #[test]
    fn a_disk_is_away_only_if_it_is_named_under_volumes() {
        assert!(disk_present(Path::new("/Users/enzo/Avatar")));
        assert!(disk_present(Path::new("/Volumes")));
        assert!(!disk_present(Path::new("/Volumes/Not a disk connected here/Avatar")));
    }

    #[test]
    fn disk_errors_are_bound_to_the_exact_project() {
        let log = "com.unity.burst: ENOSPC: no space left on device, copyfile 'cache' -> 'H:\\Projects\\Avatar\\Library\\PackageCache\\.tmp\\file'";
        assert!(matches_disk_error(log, Path::new("H:/Projects/Avatar")));
        assert!(!matches_disk_error(log, Path::new("H:/Projects/Avatar copy")));
        assert!(!matches_disk_error("missing dependency", Path::new("H:/Projects/Avatar")));
    }

    /// PROJECTS-24 c: Unity's log is read again when it changes (its length or its time), not at each look while it stays the same.
    #[test]
    fn unitys_log_is_read_again_only_when_it_changed() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("Editor.log");
        let project = temp.path().join("Avatar");
        let full = format!("ENOSPC: no space left on device, open '{}/Library/a'\n", project.to_string_lossy());
        let quiet = "x".repeat(full.len());
        let long_ago = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        let set_time = || std::fs::File::options().write(true).open(&log).unwrap().set_modified(long_ago).unwrap();
        std::fs::write(&log, &quiet).unwrap();
        set_time();
        assert!(!disk_error_in_log(Some(log.clone()), &project));
        // The same length and time: the answer kept (the file is not read; here it says otherwise, which only a read would see).
        std::fs::write(&log, &full).unwrap();
        set_time();
        assert!(!disk_error_in_log(Some(log.clone()), &project), "unchanged by its length and time: not read again");
        // Unity writes on: read again.
        std::fs::write(&log, format!("{full}more\n")).unwrap();
        assert!(disk_error_in_log(Some(log.clone()), &project));
        assert!(!disk_error_in_log(Some(log.clone()), &temp.path().join("Avatar copy")), "another project is another answer");
        assert!(!disk_error_in_log(None, &project));
    }

    #[test]
    fn historical_error_does_not_relocate_a_disk_with_space() {
        assert!(!Health { free: Some(IMPORT_RESERVE), disk_error: true, ..Health::default() }.needs_repair());
        assert!(Health { free: Some(1024), ..Health::default() }.needs_repair());
        assert!(!Health { free: None, disk_error: true, ..Health::default() }.needs_repair());
    }

    #[test]
    fn the_panel_tells_running_low_from_unity_having_failed() {
        assert_eq!(Health { free: Some(IMPORT_RESERVE), ..Health::default() }.notice(), None);
        assert_eq!(Health { free: None, ..Health::default() }.notice(), None);
        assert_eq!(Health { free: Some(1024), ..Health::default() }.notice(), Some(Notice::RunningLow));
        assert_eq!(Health { free: Some(1024), disk_error: true, ..Health::default() }.notice(), Some(Notice::OutOfSpace));
        // The state the panel had a heading for and could never reach: Unity failed, and there is room now.
        assert_eq!(Health { free: Some(IMPORT_RESERVE), disk_error: true, ..Health::default() }.notice(), Some(Notice::FailedEarlier));
        assert_eq!(Health { free: None, disk_error: true, ..Health::default() }.notice(), Some(Notice::FailedEarlier));
    }

    #[test]
    fn a_low_disk_with_a_real_library_decides_not_to_copy() {
        let temp = tempfile::tempdir().unwrap();
        // Too many files, however small.
        let many = project(temp.path(), "many", INSTANT_ENTRIES + 1, 8);
        assert_eq!(preflight_decision(&many, &low()), Preflight::Leave);
        // Too many bytes, however few files (a sparse file: nothing is written).
        let large = project(temp.path(), "large", 1, 8);
        let file = std::fs::File::create(large.join("Library/big.bin")).unwrap();
        file.set_len(INSTANT_BYTES + 1).unwrap();
        drop(file);
        assert_eq!(preflight_decision(&large, &low()), Preflight::Leave);
        // Room on the disk, or Unity holding the project: left alone at any size.
        let tiny = project(temp.path(), "tiny", 2, 8);
        assert_eq!(preflight_decision(&tiny, &Health { free: Some(IMPORT_RESERVE), ..Health::default() }), Preflight::Leave);
        assert_eq!(preflight_decision(&tiny, &Health { open: true, ..low() }), Preflight::Leave);
        // No Library, or a tiny one: moved.
        assert!(matches!(preflight_decision(&tiny, &low()), Preflight::Move(f) if f.len() == 2));
        let none = project(temp.path(), "none", 0, 0);
        assert_eq!(preflight_decision(&none, &low()), Preflight::Move(vec![]));
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn opening_with_a_low_disk_does_not_copy_an_existing_library() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let project = project(temp.path(), "project", INSTANT_ENTRIES + 10, 64);
        let picked = std::cell::Cell::new(false);
        let moved = preflight_with(&project, &low(), |_, _| {
            picked.set(true);
            Ok(cache.clone())
        })
        .unwrap();
        assert_eq!(moved, None);
        assert!(!picked.get(), "a destination was looked for: that is the start of a copy");
        assert!(!cache.exists(), "nothing may be copied");
        let meta = std::fs::symlink_metadata(project.join("Library")).unwrap();
        assert!(!crate::discovery::is_link(&meta), "Library stays a real folder");
        assert!(project.join("Library/Artifacts/ab/0.bin").is_file());
    }

    /// Verifier round 2: a Library copied back from an earlier move still holds that cache's marker files; moving it
    /// again works, and the new cache carries its own markers only.
    #[cfg(any(windows, unix))]
    #[test]
    fn a_library_copied_back_with_old_markers_moves_again() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let project = project(temp.path(), "back", 2, 64);
        std::fs::write(project.join("Library").join(SWITCHED), b"").unwrap();
        std::fs::write(project.join("Library").join(COPYING), b"").unwrap();
        let moved = preflight_with(&project, &low(), |_, _| Ok(cache.clone())).unwrap().expect("moved");
        assert!(is_redirect(project.join("Library")).unwrap());
        assert!(moved.join(SWITCHED).is_file() && !moved.join(COPYING).exists());
        remove_redirect(project.join("Library")).unwrap();
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn opening_moves_a_library_that_barely_exists_and_none_at_all() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        for (name, files) in [("tiny", 3), ("empty", 0)] {
            let project = project(temp.path(), name, files, 256);
            let moved = preflight_with(&project, &low(), |_, needed| {
                assert!(needed >= IMPORT_RESERVE);
                Ok(cache.clone())
            })
            .unwrap()
            .expect("moved");
            let library = project.join("Library");
            assert!(is_redirect(&library).unwrap());
            assert!(moved.starts_with(&cache));
            // The cache's own switch marker aside (`SWITCHED`, inside it since verifier point 3).
            let entries = std::fs::read_dir(&library).unwrap().flatten().filter(|e| e.file_name() != SWITCHED).count();
            assert_eq!(entries > 0, files > 0);
            remove_redirect(&library).unwrap();
        }
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn a_failed_preflight_leaves_everything_as_it_was() {
        let temp = tempfile::tempdir().unwrap();
        let project = project(temp.path(), "project", 3, 256);
        let result = preflight_with(&project, &low(), |_, _| anyhow::bail!("no disk has room"));
        assert!(result.is_err());
        assert!(!crate::discovery::is_link(&std::fs::symlink_metadata(project.join("Library")).unwrap()));
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn another_editor_or_repair_cannot_acquire_the_project_lock() {
        let temp = tempfile::tempdir().unwrap();
        let first = project_lock(temp.path()).unwrap();
        assert!(project_lock(temp.path()).is_err());
        drop(first);
        assert!(project_lock(temp.path()).is_ok());
    }

    #[cfg(any(windows, unix))]
    fn files_of(project: &Path) -> Vec<(PathBuf, u64)> {
        let library = project.join("Library");
        match walk_library(&library, &SharedProgress::default(), &AtomicBool::new(false)).unwrap().unwrap() {
            Ok(files) => files,
            Err(at) => panic!("a link at {}", at.display()),
        }
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn relocation_preserves_cache_and_rolls_back_failed_redirect() {
        let temp = tempfile::tempdir().unwrap();
        let project = project(temp.path(), "project", 1, 4096);
        let library = project.join("Library");
        let file = library.join("Artifacts/ab/0.bin");
        let original = std::fs::read(&file).unwrap();
        let files = files_of(&project);
        let progress = SharedProgress::default();
        let cancel = AtomicBool::new(false);
        let work = Work { progress: &progress, cancel: &cancel };
        let failed = temp.path().join("failed");
        assert!(relocate(&project, &failed, &files, &work, |_, l| {
            std::fs::create_dir(l)?;
            Err(std::io::Error::other("injected failure"))
        })
        .is_err());
        assert_eq!(std::fs::read(&file).unwrap(), original);
        assert!(!failed.exists(), "a failed redirect leaves no copy behind");
        assert!(!failed.with_extension("json").exists());
        let target = temp.path().join("relocated");
        let RepairEnd::Moved(moved) = relocate(&project, &target, &files, &work, |t, l| redirect(t, l)).unwrap() else { panic!("not moved") };
        assert_eq!(moved.old_left, None);
        assert!(is_redirect(&library).unwrap());
        assert_eq!(std::fs::read(&file).unwrap(), original);
        assert_eq!(std::fs::read(target.join("Artifacts/ab/0.bin")).unwrap(), original);
        assert_eq!(named_like_backups(&project), Vec::<PathBuf>::new());
        remove_redirect(&library).unwrap();
    }

    /// The real disk, counting what is asked of it, and failing where a test says so: no disk at hand refuses a read
    /// past the cache or fails a flush, so the branches for one would never run.
    #[cfg(any(windows, unix))]
    #[derive(Default)]
    struct Probe {
        /// Opens past the cache, opens after a flush, and the files flushed all the way, in order.
        past_cache: std::cell::Cell<usize>,
        flushed: std::cell::Cell<usize>,
        persisted: std::cell::RefCell<Vec<PathBuf>>,
        /// The read past the cache (0 the first) that is refused for its shape, as a disk of larger sectors refuses it.
        refuse_read: Option<usize>,
        fail_flush: bool,
        fail_persist: bool,
        /// Every file flushed all the way, or the last alone.
        every_file: bool,
    }

    /// The real disk behind a [`Probe`], which has its own say on what is flushed.
    #[cfg(any(windows, unix))]
    const ON_DISK: OnDisk = OnDisk { every_file: false };

    /// A reader whose `refuse`-th read fails as a read past the cache does on a disk it does not fit.
    #[cfg(any(windows, unix))]
    struct Refusing {
        inner: Box<dyn Read>,
        reads: usize,
        refuse: usize,
    }

    #[cfg(any(windows, unix))]
    impl Read for Refusing {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.reads += 1;
            if self.reads - 1 == self.refuse {
                return Err(std::io::Error::from_raw_os_error(REFUSED_SHAPE));
            }
            self.inner.read(buffer)
        }
    }

    #[cfg(any(windows, unix))]
    impl Written for Probe {
        fn past_cache(&self, path: &Path) -> Option<Box<dyn Read>> {
            let inner = match (ON_DISK.past_cache(path), self.refuse_read) {
                (Some(inner), _) => inner,
                // Where nothing is read past the cache, a refusal is still injected, on a plain read.
                (None, Some(_)) => Box::new(std::fs::File::open(path).ok()?) as Box<dyn Read>,
                (None, None) => return None,
            };
            self.past_cache.set(self.past_cache.get() + 1);
            Some(match self.refuse_read {
                Some(refuse) => Box::new(Refusing { inner, reads: 0, refuse }),
                None => inner,
            })
        }

        fn flushed(&self, path: &Path) -> std::io::Result<Box<dyn Read>> {
            self.flushed.set(self.flushed.get() + 1);
            if self.fail_flush {
                return Err(std::io::Error::other("injected: the copy could not be written out"));
            }
            ON_DISK.flushed(path)
        }

        fn persist(&self, path: &Path) -> std::io::Result<()> {
            self.persisted.borrow_mut().push(path.to_path_buf());
            if self.fail_persist {
                return Err(std::io::Error::other("injected: the disk did not flush"));
            }
            ON_DISK.persist(path)
        }

        fn persists_every_file(&self) -> bool {
            self.every_file
        }
    }

    /// The bytes [`project`] writes into its `i`-th file.
    fn bytes_of(i: usize, size: usize) -> Vec<u8> {
        (0..size).map(|b| (b.wrapping_mul(i + 7) >> 3) as u8).collect()
    }

    /// Which of the two a move does is chosen by the file system of the disk it goes to: the last file alone where a
    /// journal commits the files before it (NTFS, ReFS), every file anywhere else: exFAT and FAT32, which external
    /// drives often are, a file system of any other name, and a disk that cannot be asked. Off Windows, as it was.
    #[cfg(any(windows, unix))]
    #[test]
    fn a_disk_without_a_journal_has_every_file_flushed() {
        for journaled in ["NTFS", "ntfs", "ReFS", "REFS"] {
            assert!(!flushes_every_file(Some(journaled)), "{journaled}: the last file alone");
        }
        for other in ["exFAT", "EXFAT", "FAT32", "FAT", "FAT12", "UDF", "CDFS", "Btrfs", "NTFS3", "Re", "something new", ""] {
            assert!(flushes_every_file(Some(other)), "{other:?}: every file");
        }
        assert!(flushes_every_file(None), "a disk that cannot be asked: every file");

        // The disk the tests run on, asked for real: a folder that is not there yet is asked through one that is.
        let temp = tempfile::tempdir().unwrap();
        let named = file_system_of(temp.path());
        assert_eq!(file_system_of(&temp.path().join("not").join("there yet")), named);
        if cfg!(windows) {
            let named = named.expect("Windows names the file system of the temporary folder's disk");
            assert_eq!(OnDisk::to(&temp.path().join("cache")).persists_every_file(), flushes_every_file(Some(&named)), "{named}");
        } else {
            assert_eq!(named, None);
            assert!(!OnDisk::to(temp.path()).persists_every_file());
        }
    }

    /// A copy is read back from the disk, not from the file cache it was just written through, at every size around a
    /// sector's and a chunk's: the same bytes pass, the whole of them counted for the progress; one byte changed in
    /// the last sector, or one byte more or less, does not. On Windows every one of these reads went past the cache and
    /// none fell back to a flush: the disk the tests run on is one that can.
    #[cfg(any(windows, unix))]
    #[test]
    fn a_copy_is_read_back_from_the_disk() {
        let temp = tempfile::tempdir().unwrap();
        let (a, b) = (temp.path().join("original.bin"), temp.path().join("copy.bin"));
        let [mut left, mut right] = check_buffers();
        let disk = Probe::default();
        let mut checks = 0;
        for size in [0, 1, 511, 512, 4095, 4096, 4097, CHECK_CHUNK - 1, CHECK_CHUNK, CHECK_CHUNK + 1, 2 * CHECK_CHUNK + 123] {
            let bytes: Vec<u8> = (0..size).map(|i| (i.wrapping_mul(31) >> 2) as u8).collect();
            std::fs::write(&a, &bytes).unwrap();
            std::fs::write(&b, &bytes).unwrap();
            let mut counted = 0;
            let same = same_bytes(&disk, &a, &b, &mut left, &mut right, |n| {
                counted += n;
                true
            });
            checks += 1;
            assert_eq!(same.unwrap(), Some(true), "{size} bytes");
            assert_eq!(counted, size, "bytes counted of {size}");
            if size > 0 {
                let mut changed = bytes.clone();
                changed[size - 1] ^= 0x40;
                std::fs::write(&b, &changed).unwrap();
                assert_eq!(same_bytes(&disk, &a, &b, &mut left, &mut right, |_| true).unwrap(), Some(false), "a changed last byte of {size}");
                checks += 1;
                // A byte short is seen in the lengths, before either is opened.
                std::fs::write(&b, &bytes[..size - 1]).unwrap();
                assert_eq!(same_bytes(&disk, &a, &b, &mut left, &mut right, |_| true).unwrap(), Some(false), "a byte short of {size}");
            }
            // Told to stop, it says so instead of answering.
            if size > CHECK_CHUNK {
                std::fs::write(&b, &bytes).unwrap();
                assert_eq!(same_bytes(&disk, &a, &b, &mut left, &mut right, |_| false).unwrap(), None);
                checks += 1;
            }
        }
        let (past_cache, flushed) = (disk.past_cache.get(), disk.flushed.get());
        if cfg!(windows) {
            assert_eq!((past_cache, flushed), (checks, 0), "every copy read past the cache, none through it");
        } else {
            assert_eq!((past_cache, flushed), (0, checks), "every copy written out, then read");
        }
        assert!(disk.persisted.borrow().is_empty(), "the comparison itself flushes nothing all the way");
    }

    /// A disk that refuses the first read past the cache (its sectors are larger than the check's memory fits) has
    /// each copy written out and read through the cache instead, from its start, and the Library moves whole. A
    /// refusal further into a file is a failure of the copy like any other: nothing is switched, Library stays.
    #[cfg(any(windows, unix))]
    #[test]
    fn a_disk_that_refuses_the_read_past_the_cache_is_read_through_it() {
        let temp = tempfile::tempdir().unwrap();
        let size = CHECK_CHUNK + 300_000;
        let project = project(temp.path(), "project", 4, size);
        let library = project.join("Library");
        let files = files_of(&project);
        let progress = SharedProgress::default();
        let cancel = AtomicBool::new(false);
        let work = Work { progress: &progress, cancel: &cancel };

        // The second read of a file refused: after its first megabyte was compared.
        let late = Probe { refuse_read: Some(1), ..Default::default() };
        let target = temp.path().join("late");
        let error = relocate_on(&late, &project, &target, &files, &work, |t, l| redirect(t, l)).unwrap_err();
        assert!(format!("{error:#}").contains("stays where it is"), "{error:#}");
        assert_eq!((late.flushed.get(), late.persisted.borrow().len()), (0, 0), "no going back to the file's start once part of it is compared");
        assert!(!target.exists() && !is_redirect(&library).unwrap_or(false));
        assert_eq!(std::fs::read(library.join("Artifacts/ab/2.bin")).unwrap(), bytes_of(2, size));

        // The first read refused, of every file: each is written out and read through the cache.
        let disk = Probe { refuse_read: Some(0), ..Default::default() };
        let target = temp.path().join("moved");
        let RepairEnd::Moved(moved) = relocate_on(&disk, &project, &target, &files, &work, |t, l| redirect(t, l)).unwrap() else { panic!("not moved") };
        assert_eq!((disk.past_cache.get(), disk.flushed.get()), (4, 4), "each file tried past the cache, then written out");
        assert_eq!(moved.files, 4);
        assert!(is_redirect(&library).unwrap());
        for i in 0..4 {
            assert!(std::fs::read(target.join(format!("Artifacts/ab/{i}.bin"))).unwrap() == bytes_of(i, size), "file {i} of the moved Library");
        }
        let end = progress.lock().unwrap().clone();
        assert_eq!((end.bytes_copied, end.bytes_checked), (4 * size as u64, 4 * size as u64), "the bytes read again from the start are counted once");
        remove_redirect(&library).unwrap();
    }

    /// Before Library is switched and its original removed, the last file written is flushed all the way, once a
    /// move: the file system's log and the disk's own write cache, which a comparison of bytes does not reach. A flush
    /// that fails, that one or the one a copy is written out with where it cannot be read past the cache, stops the
    /// move with Library where it was and no copy left.
    #[cfg(any(windows, unix))]
    #[test]
    fn the_copy_is_flushed_once_before_the_switch_and_a_failed_flush_keeps_the_library() {
        let temp = tempfile::tempdir().unwrap();
        let project = project(temp.path(), "project", 3, 5000);
        let library = project.join("Library");
        let files = files_of(&project);
        let progress = SharedProgress::default();
        let cancel = AtomicBool::new(false);
        let work = Work { progress: &progress, cancel: &cancel };
        let kept = |target: &Path, error: anyhow::Error| {
            let said = format!("{error:#}");
            assert!(said.contains("stays where it is") && said.contains("partial copy was deleted"), "{said}");
            assert!(!target.exists() && !target.with_extension("json").exists(), "a copy left behind");
            assert!(!is_redirect(&library).unwrap_or(false));
            for i in 0..3 {
                assert_eq!(std::fs::read(library.join(format!("Artifacts/ab/{i}.bin"))).unwrap(), bytes_of(i, 5000));
            }
            assert_eq!(named_like_backups(&project), Vec::<PathBuf>::new());
        };
        let never = |_: &Path, _: &Path| -> std::io::Result<()> { panic!("Library switched after a flush that failed") };

        // The last flush fails: every file had passed its comparison.
        let disk = Probe { fail_persist: true, ..Default::default() };
        let target = temp.path().join("unflushed");
        kept(&target, relocate_on(&disk, &project, &target, &files, &work, never).unwrap_err());
        assert_eq!(disk.persisted.borrow().len(), 1);

        // A copy cannot be written out, on a disk that refuses the read past the cache.
        let disk = Probe { refuse_read: Some(0), fail_flush: true, ..Default::default() };
        let target = temp.path().join("unwritten");
        kept(&target, relocate_on(&disk, &project, &target, &files, &work, never).unwrap_err());
        assert_eq!((disk.flushed.get(), disk.persisted.borrow().len()), (1, 0), "stopped at the first file");

        // And as it goes when nothing fails: one flush, of the last file copied, before the switch.
        let disk = Probe::default();
        let target = temp.path().join("moved");
        let last = target.join(files.last().unwrap().0.strip_prefix(&library).unwrap());
        let moved = relocate_on(&disk, &project, &target, &files, &work, |t, l| {
            assert_eq!(*disk.persisted.borrow(), [last.clone()], "flushed before Library is switched");
            redirect(t, l)
        });
        assert!(matches!(moved.unwrap(), RepairEnd::Moved(_)));
        assert_eq!(*disk.persisted.borrow(), [last], "once a move");
        remove_redirect(&library).unwrap();

        // The other way, a flush a file: every file, in the order copied, all before the switch.
        let project = self::project(temp.path(), "every file", 3, 5000);
        let library = project.join("Library");
        let files = files_of(&project);
        let disk = Probe { every_file: true, ..Default::default() };
        let target = temp.path().join("moved every file");
        let copies: Vec<PathBuf> = files.iter().map(|(file, _)| target.join(file.strip_prefix(&library).unwrap())).collect();
        let moved = relocate_on(&disk, &project, &target, &files, &work, |t, l| {
            assert_eq!(*disk.persisted.borrow(), copies, "every file flushed before Library is switched");
            redirect(t, l)
        });
        assert!(matches!(moved.unwrap(), RepairEnd::Moved(_)));
        assert_eq!(*disk.persisted.borrow(), copies);
        remove_redirect(&library).unwrap();
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn the_copy_keeps_times_and_the_read_only_flag_and_can_be_deleted_again() {
        let temp = tempfile::tempdir().unwrap();
        let project = project(temp.path(), "project", 2, 4096);
        let file = project.join("Library/Artifacts/ab/1.bin");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(90_000);
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(old).unwrap();
        let mut readonly = std::fs::metadata(&file).unwrap().permissions();
        readonly.set_readonly(true);
        std::fs::set_permissions(&file, readonly).unwrap();
        let files = files_of(&project);
        let progress = SharedProgress::default();
        let cancel = AtomicBool::new(false);
        let target = temp.path().join("target");
        // The old Library holds a read-only file too: its removal must go through.
        relocate(&project, &target, &files, &Work { progress: &progress, cancel: &cancel }, |t, l| redirect(t, l)).unwrap();
        let copied = std::fs::metadata(target.join("Artifacts/ab/1.bin")).unwrap();
        assert!(copied.permissions().readonly());
        let delta = copied.modified().unwrap().duration_since(old).unwrap_or_default();
        assert!(delta.as_secs() < 3, "kept its modification time");
        assert_eq!(named_like_backups(&project), Vec::<PathBuf>::new());
        remove_redirect(project.join("Library")).unwrap();
        remove_tree(&target, &mut || {}).unwrap();
        assert!(!target.exists());
    }

    /// Runs a repair on a thread, cancelling it once some of the bytes were copied.
    #[cfg(any(windows, unix))]
    fn repair_cancelled_midway(project: &Path, cache: &Path) -> (RepairEnd, Progress) {
        let progress = SharedProgress::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let run = {
            let (project, cache, progress, cancel) = (project.to_path_buf(), cache.to_path_buf(), progress.clone(), cancel.clone());
            std::thread::spawn(move || repair_with(&project, &progress, &cancel, |_, _| Ok(cache)))
        };
        let seen = loop {
            let now = progress.lock().unwrap().clone();
            if now.bytes_copied > 0 || run.is_finished() {
                cancel.store(true, Ordering::Relaxed);
                break now;
            }
            std::thread::yield_now();
        };
        (run.join().unwrap().unwrap(), seen)
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn cancel_leaves_the_library_intact_and_no_partial_copy() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let project = project(temp.path(), "project", 96, 1024 * 1024);
        let before: Vec<(PathBuf, Vec<u8>)> = files_of(&project).into_iter().map(|(p, _)| {
            let bytes = std::fs::read(&p).unwrap();
            (p, bytes)
        }).collect();
        let (end, seen) = repair_cancelled_midway(&project, &cache);
        assert_eq!(end, RepairEnd::Cancelled { left: None });
        assert!(seen.bytes_copied > 0 && seen.bytes_copied < seen.bytes_total, "cancelled in the middle: {seen:?}");
        assert!(!crate::discovery::is_link(&std::fs::symlink_metadata(project.join("Library")).unwrap()));
        for (path, bytes) in before {
            assert_eq!(std::fs::read(&path).unwrap(), bytes, "{}", path.display());
        }
        assert!(folder_is_empty(&cache), "no partial copy or record left");
        assert_eq!(named_like_backups(&project), Vec::<PathBuf>::new());
        // The lock went with it: Unity can open the project, a repair can run again.
        assert!(!crate::unity_open(&project));
        let RepairEnd::Moved(moved) = repair_with(&project, &SharedProgress::default(), &AtomicBool::new(false), |_, _| Ok(cache.clone())).unwrap() else { panic!("not moved") };
        assert_eq!(moved.files, 96);
        remove_redirect(project.join("Library")).unwrap();
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn a_failure_in_the_copy_leaves_the_library_and_says_so() {
        let temp = tempfile::tempdir().unwrap();
        let project = project(temp.path(), "project", 3, 1024);
        let mut files = files_of(&project);
        // A file gone by the time it is copied.
        files.push((project.join("Library/Artifacts/ab/gone.bin"), 10));
        let target = temp.path().join("cache/abc");
        let progress = SharedProgress::default();
        let cancel = AtomicBool::new(false);
        let error = relocate(&project, &target, &files, &Work { progress: &progress, cancel: &cancel }, |t, l| redirect(t, l)).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("stays where it is"), "{text}");
        assert!(text.contains("partial copy was deleted"), "{text}");
        assert!(!target.exists());
        assert!(!target.with_extension("json").exists());
        assert!(project.join("Library/Artifacts/ab/0.bin").is_file());
    }

    /// Files in use do not stop a rename here, as they do on Windows. A folder that cannot be renamed still can.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_library_that_cannot_be_renamed_stops_the_switch_and_keeps_it() {
        use std::os::unix::ffi::OsStrExt;
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let project = project(temp.path(), "project", 3, 1024);
        let library = std::ffi::CString::new(project.join("Library").as_os_str().as_bytes()).unwrap();
        let set = |flags: libc::c_uint| assert_eq!(unsafe { libc::chflags(library.as_ptr(), flags) }, 0);
        set(libc::UF_IMMUTABLE);
        let error = repair_with(&project, &SharedProgress::default(), &AtomicBool::new(false), |_, _| Ok(cache.clone())).unwrap_err();
        set(0);
        let text = format!("{error:#}");
        assert!(text.contains("stays where it is"), "{text}");
        assert!(project.join("Library/Artifacts/ab/0.bin").is_file());
        assert!(!crate::discovery::is_link(&std::fs::symlink_metadata(project.join("Library")).unwrap()));
        assert!(folder_is_empty(&cache), "the checked copy goes too");
    }

    #[cfg(windows)]
    #[test]
    fn something_using_the_library_stops_the_switch_and_keeps_it() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let project = project(temp.path(), "project", 3, 1024);
        // A file held open without sharing deletion: the folder cannot be renamed.
        use std::os::windows::fs::OpenOptionsExt;
        let held = std::fs::OpenOptions::new().read(true).share_mode(1).open(project.join("Library/Artifacts/ab/0.bin")).unwrap();
        let error = repair_with(&project, &SharedProgress::default(), &AtomicBool::new(false), |_, _| Ok(cache.clone())).unwrap_err();
        drop(held);
        let text = format!("{error:#}");
        assert!(text.contains("stays where it is"), "{text}");
        assert!(project.join("Library/Artifacts/ab/0.bin").is_file());
        assert!(folder_is_empty(&cache), "the checked copy goes too");
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn progress_counts_up_to_the_whole_library() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let project = project(temp.path(), "project", 24, 512 * 1024);
        let progress = SharedProgress::default();
        let RepairEnd::Moved(moved) = repair_with(&project, &progress, &AtomicBool::new(false), |_, _| Ok(cache.clone())).unwrap() else { panic!("not moved") };
        let end = progress.lock().unwrap().clone();
        assert_eq!(end.phase, Phase::RemovingOld);
        assert_eq!((end.files_done, end.files_total, end.files_removed), (24, 24, 24));
        assert_eq!((end.bytes_copied, end.bytes_checked, end.bytes_total), (moved.bytes, moved.bytes, 24 * 512 * 1024));
        assert_eq!(end.destination.as_deref(), Some(moved.target.as_path()));
        assert!(!end.cancellable());
        remove_redirect(project.join("Library")).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn programs_in_library_stay_programs_and_read_only_folders_are_deleted() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let project = project(temp.path(), "project", 2, 1024);
        // A package's compiler, in a folder unpacked read-only.
        let tools = project.join("Library/PackageCache/com.unity.burst/hostmac");
        std::fs::create_dir_all(&tools).unwrap();
        let tool = tools.join("bcl");
        std::fs::write(&tool, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&tools, std::fs::Permissions::from_mode(0o555)).unwrap();
        let RepairEnd::Moved(moved) = repair_with(&project, &SharedProgress::default(), &AtomicBool::new(false), |_, _| Ok(cache.clone())).unwrap() else { panic!("not moved") };
        assert_eq!(moved.old_left, None, "the read-only folder went with the rest");
        let copied = moved.target.join("Library/PackageCache/com.unity.burst/hostmac/bcl");
        let copied = if copied.exists() { copied } else { moved.target.join("PackageCache/com.unity.burst/hostmac/bcl") };
        assert_eq!(std::fs::metadata(&copied).unwrap().permissions().mode() & 0o777, 0o755);
        // Reached through the project, as Unity does.
        assert_eq!(std::fs::metadata(project.join("Library/PackageCache/com.unity.burst/hostmac/bcl")).unwrap().permissions().mode() & 0o111, 0o111);
        assert_eq!(named_like_backups(&project), Vec::<PathBuf>::new());
    }

    #[test]
    fn a_plan_says_what_stops_it() {
        let temp = tempfile::tempdir().unwrap();
        let roomy = [(temp.path().join("small"), 10), (temp.path().join("large"), room_needed(100))];
        assert_eq!(verdict_for(100, &roomy), Verdict::Ready { cache_root: temp.path().join("large"), free: room_needed(100) });
        assert_eq!(
            verdict_for(101, &roomy),
            Verdict::NoRoom { needed: room_needed(101), best: Some((temp.path().join("large"), room_needed(100))) }
        );
        assert_eq!(verdict_for(1, &[]), Verdict::NoRoom { needed: room_needed(1), best: None });
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn a_plan_measures_without_changing_anything_and_sees_a_redirect() {
        let temp = tempfile::tempdir().unwrap();
        let project = project(temp.path(), "project", 5, 1000);
        let progress = SharedProgress::default();
        let plan = plan(&project, &progress, &AtomicBool::new(false)).unwrap().unwrap();
        assert_eq!((plan.bytes, plan.files), (5000, 5));
        assert_eq!(progress.lock().unwrap().files_total, 5);
        let elsewhere = temp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let junctioned = temp.path().join("junctioned");
        std::fs::create_dir_all(&junctioned).unwrap();
        redirect(&elsewhere, junctioned.join("Library")).unwrap();
        let redirected = super::plan(&junctioned, &progress, &AtomicBool::new(false)).unwrap().unwrap();
        assert!(matches!(redirected.verdict, Verdict::Redirected { to: Some(_) }));
        // A cancelled measurement answers nothing.
        assert_eq!(super::plan(&project, &progress, &AtomicBool::new(true)).unwrap(), None);
        remove_redirect(junctioned.join("Library")).unwrap();
    }

    // The orphans. Every case below names real folders under a temporary directory: `sweep` is never given a real disk.

    /// A cache and its record, as a repair writes them.
    fn cache_for(root: &Path, id_digit: char, project: &Path) -> PathBuf {
        let cache = root.join(id_digit.to_string().repeat(32));
        std::fs::create_dir_all(cache.join("Artifacts")).unwrap();
        std::fs::write(cache.join("Artifacts/data.bin"), b"cache data").unwrap();
        std::fs::write(cache.join(COPYING), b"").unwrap();
        crate::write_json(&cache.with_extension("json"), &serde_json::json!({"project": project, "library": cache, "phase": "copying"})).unwrap();
        cache
    }

    /// The backup name a repair records for `cache` before it moves `project`'s Library aside.
    fn with_backup(cache: &Path, project: &Path) -> PathBuf {
        let old = project.join(format!("{BACKUP_PREFIX}{}", "b".repeat(32)));
        let id = crate::owned::id_of(&project.join("Library")).unwrap();
        crate::write_json(&cache.with_extension("json"), &serde_json::json!({"project": project, "library": cache, "backup": old, "phase": "switching", "backup_id": id}))
            .unwrap();
        old
    }

    /// Verifier, point 3: an older "copying" record restored over a cache that became a moved project's Library, an
    /// older version's record without a phase, and a sidecar beside another cache: all kept.
    #[cfg(any(windows, unix))]
    #[test]
    fn a_stale_or_foreign_record_never_removes_a_cache() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("UnityCache");
        let project = project(temp.path(), "project", 1, 64);
        // Became Library somewhere (the switch marker inside it), its record later replaced by an old "copying" one.
        let live = cache_for(&root, '5', &project);
        std::fs::write(live.join(SWITCHED), b"").unwrap();
        // An older version's record: no phase.
        let legacy = cache_for(&root, '4', &project);
        crate::write_json(&legacy.with_extension("json"), &serde_json::json!({"project": project, "library": legacy})).unwrap();
        // Verifier round 2: no "still copying" file in it (made by an older version, or Library of a moved project),
        // with a matching "copying" record and another valid project now at the record's place: kept.
        let markerless = cache_for(&root, '6', &project);
        std::fs::remove_file(markerless.join(COPYING)).unwrap();
        // A sidecar naming another cache.
        let foreign = cache_for(&root, '3', &project);
        crate::write_json(&foreign.with_extension("json"), &serde_json::json!({"project": project, "library": root.join("1".repeat(32)), "phase": "copying"})).unwrap();
        let swept = run_sweep(&root, &[]);
        assert!(swept.removed.is_empty(), "{swept:?}");
        for c in [&live, &legacy, &foreign, &markerless] {
            assert!(c.join("Artifacts/data.bin").is_file());
        }
    }

    /// A folder put at the recorded backup's name after the repair (a restored archive, another folder) is neither
    /// removed beside a live cache nor made the project's Library.
    #[cfg(any(windows, unix))]
    #[test]
    fn a_folder_put_at_the_backups_name_later_is_not_the_backup() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("UnityCache");
        let project = project(temp.path(), "project", 1, 64);
        let cache = cache_for(&root, '2', &project);
        let recorded = with_backup(&cache, &project);
        // The real old Library went; another folder now has its name.
        std::fs::remove_dir_all(project.join("Library")).unwrap();
        std::fs::create_dir_all(&recorded).unwrap();
        std::fs::write(recorded.join("restored.txt"), "someone's").unwrap();
        let swept = run_sweep(&root, &[]);
        assert!(swept.restored.is_empty() && swept.removed.is_empty() && swept.old_removed.is_empty(), "{swept:?}");
        assert!(recorded.join("restored.txt").is_file() && !project.join("Library").exists());
        // Beside a cache in use: not removed either.
        redirect(&cache, project.join("Library")).unwrap();
        let swept = run_sweep(&root, &[]);
        assert!(swept.old_removed.is_empty(), "{swept:?}");
        assert!(recorded.join("restored.txt").is_file());
        remove_redirect(project.join("Library")).unwrap();
    }

    /// PROJECTS-33: a folder only named like an old Library (the person's archive, or one an older version left) is never
    /// removed nor put back as Library; the cache it may belong to stays.
    #[cfg(any(windows, unix))]
    #[test]
    fn a_folder_only_named_like_an_old_library_is_left_alone() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("UnityCache");
        let project = project(temp.path(), "project", 2, 64);
        let mine = project.join(format!("{BACKUP_PREFIX}my archive"));
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::write(mine.join("keep.txt"), "mine").unwrap();
        // In use, with the person's folder beside it.
        let cache = cache_for(&root, '8', &project);
        let recorded = with_backup(&cache, &project);
        std::fs::rename(project.join("Library"), &recorded).unwrap();
        redirect(&cache, project.join("Library")).unwrap();
        let swept = run_sweep(&root, &[]);
        assert_eq!(swept.old_removed, vec![recorded.clone()], "only the recorded one");
        assert!(mine.join("keep.txt").is_file());
        remove_redirect(project.join("Library")).unwrap();
        let _ = std::fs::remove_dir(project.join("Library"));
        // Library missing, only the person's folder there: it is not made the Library, and the cache stays.
        let swept = run_sweep(&root, &[]);
        assert!(swept.restored.is_empty() && swept.removed.is_empty(), "{swept:?}");
        assert!(mine.join("keep.txt").is_file() && !project.join("Library").exists());
        assert!(cache.join("Artifacts/data.bin").is_file());
    }

    /// PROJECTS-21: a cache whose record says it became Library is kept even when no project the gallery knows points at
    /// it (the project was moved, junction and all, and its new place is not in the cached list yet); one whose record
    /// says it was still being copied goes, whatever the list.
    #[cfg(any(windows, unix))]
    #[test]
    fn the_records_phase_decides_without_the_gallerys_list() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("UnityCache");
        let project = project(temp.path(), "project", 1, 64);
        let switched = cache_for(&root, '7', &project);
        crate::write_json(&switched.with_extension("json"), &serde_json::json!({"project": project, "library": switched, "phase": "switching"})).unwrap();
        let copying = cache_for(&root, '6', &project);
        crate::write_json(&copying.with_extension("json"), &serde_json::json!({"project": project, "library": copying, "phase": "copying"})).unwrap();
        let swept = run_sweep(&root, &[]);
        assert_eq!(swept.removed, vec![copying.clone()]);
        assert!(switched.join("Artifacts/data.bin").is_file(), "the moved project's Library stays");
        assert!(project.join("Library").is_dir(), "the project's own Library is not touched");
    }

    #[cfg(any(windows, unix))]
    fn run_sweep(root: &Path, known: &[PathBuf]) -> Swept {
        sweep(&[root.to_path_buf()], known, &AtomicBool::new(false))
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn an_interrupted_copy_is_removed_and_its_project_left_alone() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("UnityCache");
        // The Avatorbit case: a real Library, a cache naming the project, no junction.
        let project = project(temp.path(), "project", 2, 64);
        let cache = cache_for(&root, 'a', &project);
        let swept = run_sweep(&root, &[]);
        assert_eq!(swept.removed, vec![cache.clone()]);
        assert!(!cache.exists());
        assert!(!cache.with_extension("json").exists());
        assert!(project.join("Library/Artifacts/ab/0.bin").is_file());
        assert!(!crate::discovery::is_link(&std::fs::symlink_metadata(project.join("Library")).unwrap()));
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn a_record_with_verbatim_mixed_separators_is_read_as_the_app_wrote_it() {
        // What the real ones look like: `\\?\H:\…` for the project, `I:\Orbiters/UnityCache\…` for the Library.
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("UnityCache");
        let project = project(temp.path(), "project", 0, 0);
        let canonical = project.canonicalize().unwrap();
        let cache = cache_for(&root, 'b', &canonical);
        redirect(&cache, canonical.join("Library")).unwrap();
        let swept = run_sweep(&root, &[]);
        assert_eq!(swept.kept, vec![(cache.clone(), Kept::InUse)]);
        assert!(cache.join("Artifacts/data.bin").is_file());
        remove_redirect(project.join("Library")).unwrap();
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn a_cache_in_use_is_kept_with_or_without_the_gallery_knowing_its_project() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("UnityCache");
        // Named by its record, not in any list: the N4N0 sample avatar's case.
        let named = project(temp.path(), "named", 0, 0);
        let cache = cache_for(&root, 'c', &named);
        redirect(&cache, named.join("Library")).unwrap();
        let swept = run_sweep(&root, &[]);
        assert_eq!(swept.kept, vec![(cache.clone(), Kept::InUse)]);
        assert!(swept.removed.is_empty());
        assert!(cache.join("Artifacts/data.bin").is_file());
        assert!(cache.with_extension("json").is_file());
        remove_redirect(named.join("Library")).unwrap();
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn a_project_moved_by_hand_keeps_its_cache_when_the_gallery_lists_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("UnityCache");
        // The record names the old place, which holds a real Library now; the moved copy has the junction.
        let old_place = project(temp.path(), "old-place", 1, 64);
        let moved = project(temp.path(), "moved", 0, 0);
        let cache = cache_for(&root, 'd', &old_place);
        redirect(&cache, moved.join("Library")).unwrap();
        let swept = run_sweep(&root, &[old_place.clone(), moved.clone()]);
        assert_eq!(swept.kept, vec![(cache.clone(), Kept::InUse)]);
        assert!(cache.join("Artifacts/data.bin").is_file());
        remove_redirect(moved.join("Library")).unwrap();
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn what_cannot_be_checked_is_kept() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("UnityCache");
        // A disk that is not here (no computer has every letter; pick one that is free).
        #[cfg(windows)]
        let away = {
            let free_letter = (b'D'..=b'Z').rev().map(|l| format!("{}:\\", l as char)).find(|d| !Path::new(d).exists()).expect("a free drive letter");
            PathBuf::from(format!(r"{free_letter}Projects\Avatorbit"))
        };
        #[cfg(unix)]
        let away = PathBuf::from("/Volumes/Not a disk connected here/Projects/Avatorbit");
        let gone_disk = cache_for(&root, '1', &away);
        // The disk is here, the folder is not (it may have been moved, junction and all).
        let gone_folder = cache_for(&root, '2', &temp.path().join("moved-away"));
        // No record at all.
        let no_record = root.join("3".repeat(32));
        std::fs::create_dir_all(&no_record).unwrap();
        // A record that cannot be read.
        let bad_record = root.join("4".repeat(32));
        std::fs::create_dir_all(&bad_record).unwrap();
        std::fs::write(bad_record.with_extension("json"), b"not json").unwrap();
        // Not one of ours: never looked at.
        let not_ours = root.join("not-a-cache");
        std::fs::create_dir_all(&not_ours).unwrap();
        let swept = run_sweep(&root, &[]);
        assert!(swept.removed.is_empty());
        assert_eq!(
            swept.kept,
            vec![(gone_disk.clone(), Kept::DiskMissing), (gone_folder.clone(), Kept::ProjectMissing), (no_record.clone(), Kept::Unknown), (bad_record.clone(), Kept::Unknown)]
        );
        for kept in [gone_disk, gone_folder, no_record, bad_record, not_ours] {
            assert!(kept.is_dir());
        }
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn a_repair_running_now_is_not_taken_for_an_interrupted_one() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("UnityCache");
        let project = project(temp.path(), "project", 1, 64);
        let cache = cache_for(&root, 'e', &project);
        // The repair (or Unity) holds the project's lock.
        let lock = project_lock(&project).unwrap();
        let swept = run_sweep(&root, &[]);
        assert_eq!(swept.kept, vec![(cache.clone(), Kept::Busy)]);
        assert!(cache.join("Artifacts/data.bin").is_file());
        drop(lock);
        assert_eq!(run_sweep(&root, &[]).removed, vec![cache]);
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn a_kill_between_the_two_renames_puts_the_library_back() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("UnityCache");
        let project = project(temp.path(), "project", 2, 64);
        let cache = cache_for(&root, 'f', &project);
        let old = with_backup(&cache, &project);
        std::fs::rename(project.join("Library"), &old).unwrap();
        let swept = run_sweep(&root, &[]);
        assert_eq!(swept.restored, vec![project.clone()]);
        assert_eq!(swept.removed, vec![cache.clone()]);
        assert!(project.join("Library/Artifacts/ab/1.bin").is_file());
        assert!(!old.exists());
        assert!(!cache.exists());
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn a_kill_after_the_switch_finishes_removing_the_old_library() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("UnityCache");
        let project = project(temp.path(), "project", 2, 64);
        let cache = cache_for(&root, '9', &project);
        let old = with_backup(&cache, &project);
        std::fs::rename(project.join("Library"), &old).unwrap();
        redirect(&cache, project.join("Library")).unwrap();
        let swept = run_sweep(&root, &[]);
        assert_eq!(swept.kept, vec![(cache.clone(), Kept::InUse)]);
        assert_eq!(swept.old_removed, vec![old.clone()]);
        assert!(!old.exists());
        assert!(cache.join("Artifacts/data.bin").is_file(), "the cache in use stays");
        remove_redirect(project.join("Library")).unwrap();
    }

    /// The repair the test below kills: run by that test, in a process of its own.
    #[cfg(any(windows, unix))]
    #[test]
    #[ignore]
    fn repair_to_be_killed() {
        let (Some(project), Some(cache)) = (std::env::var_os("ORBITERS_TEST_KILL_PROJECT"), std::env::var_os("ORBITERS_TEST_KILL_CACHE")) else { return };
        let cache = PathBuf::from(cache);
        repair_with(Path::new(&project), &SharedProgress::default(), &AtomicBool::new(false), |_, _| Ok(cache)).unwrap();
    }

    /// Enzo's case for real: the process killed in the middle of a copy, as closing the app does. Nothing runs to clean up.
    #[cfg(any(windows, unix))]
    #[test]
    fn a_repair_killed_midway_leaves_the_library_and_its_copy_goes_at_the_next_sweep() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("UnityCache");
        let project = project(temp.path(), "project", 0, 0);
        let artifacts = project.join("Library/Artifacts");
        std::fs::create_dir_all(&artifacts).unwrap();
        let chunk = vec![0x5au8; 1024 * 1024];
        for i in 0..256 {
            std::fs::write(artifacts.join(format!("{i}.bin")), &chunk).unwrap();
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "storage::tests::repair_to_be_killed", "--ignored"])
            .env("ORBITERS_TEST_KILL_PROJECT", &project)
            .env("ORBITERS_TEST_KILL_CACHE", &root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // Once it has started copying.
        let start = std::time::Instant::now();
        let copying = || walkdir::WalkDir::new(&root).into_iter().flatten().any(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "bin"));
        while !copying() {
            assert!(start.elapsed().as_secs() < 60, "the repair did not start");
            assert!(child.try_wait().unwrap().is_none(), "the repair ended before it could be killed");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        // What the kill left: the Library as it was, a copy no Library points at, its record.
        assert!(!crate::discovery::is_link(&std::fs::symlink_metadata(project.join("Library")).unwrap()));
        assert_eq!(std::fs::read_dir(&artifacts).unwrap().count(), 256);
        let left = caches_in(std::slice::from_ref(&root));
        assert_eq!(left.len(), 1);
        assert!(left[0].with_extension("json").is_file());
        let swept = run_sweep(&root, &[]);
        assert_eq!(swept.removed, left);
        assert!(folder_is_empty(&root));
        assert_eq!(std::fs::read(artifacts.join("255.bin")).unwrap(), chunk);
    }

    /// Prints what the sweep would do on this computer, changing nothing: `cargo test -p orbiters-projects
    /// orphans_on_this_computer -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn orphans_on_this_computer() {
        for (cache, verdict) in orphans_preview() {
            println!("{}: {verdict}", cache.display());
        }
    }

    /// A move at a size where progress can be read, on the temporary folder's disk:
    /// `ORBITERS_REPAIR_MB=1024 cargo test -p orbiters-projects a_measured_repair -- --ignored --nocapture`.
    /// Lab: what flushing every file of a moved Library all the way costs against flushing its last file alone, on
    /// the two shapes a Library has: large files and many small ones. Each move is of a Library made anew in the
    /// temporary folder, or in `ORBITERS_REPAIR_FOLDER` (a folder on the disk to time: an exFAT drive), the two ways in
    /// turn, `ORBITERS_REPAIR_RUNS` times (2). Says the disk's file system and which way the app takes there.
    /// `cargo test --release -p orbiters-projects --lib -- --ignored --nocapture a_measured_flush`.
    #[cfg(any(windows, unix))]
    #[test]
    #[ignore]
    fn a_measured_flush_of_every_file_or_the_last() {
        let runs: usize = std::env::var("ORBITERS_REPAIR_RUNS").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
        let folder = std::env::var_os("ORBITERS_REPAIR_FOLDER").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        println!(
            "{}: {:?}, where the app flushes {}",
            folder.display(),
            file_system_of(&folder),
            if OnDisk::to(&folder).persists_every_file() { "every file" } else { "the last file" }
        );
        for (megabytes, kilobytes) in [(1024usize, 4096usize), (256, 16)] {
            let (files, size) = (megabytes * 1024 / kilobytes, kilobytes * 1024);
            for run in 0..runs {
                for every_file in [false, true] {
                    let temp = tempfile::tempdir_in(&folder).unwrap();
                    let project = project(temp.path(), "project", files, size);
                    let listed = files_of(&project);
                    let progress = SharedProgress::default();
                    let cancel = AtomicBool::new(false);
                    let disk = Probe { every_file, ..Default::default() };
                    let start = std::time::Instant::now();
                    let end = relocate_on(&disk, &project, &temp.path().join("cache"), &listed, &Work { progress: &progress, cancel: &cancel }, |t, l| redirect(t, l)).unwrap();
                    let seconds = start.elapsed().as_secs_f64();
                    assert!(matches!(end, RepairEnd::Moved(_)));
                    println!(
                        "{megabytes} MiB of {kilobytes} KiB files ({files}), run {run}, {}: {seconds:.1} s, {} flushed all the way, {} read past the cache",
                        if every_file { "every file flushed" } else { "the last file flushed" },
                        disk.persisted.borrow().len(),
                        disk.past_cache.get()
                    );
                    remove_redirect(project.join("Library")).unwrap();
                }
            }
        }
    }

    #[cfg(any(windows, unix))]
    #[test]
    #[ignore]
    fn a_measured_repair() {
        let mb: usize = std::env::var("ORBITERS_REPAIR_MB").ok().and_then(|s| s.parse().ok()).unwrap_or(1024);
        // The size of each file, KiB (4 MiB; Library's own are mostly a few KiB).
        let kb: usize = std::env::var("ORBITERS_REPAIR_FILE_KB").ok().and_then(|s| s.parse().ok()).unwrap_or(4096);
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let project = project(temp.path(), "project", mb * 1024 / kb, kb * 1024);
        let progress = SharedProgress::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let start = std::time::Instant::now();
        let run = {
            let (project, cache, progress, cancel) = (project.clone(), cache.clone(), progress.clone(), cancel.clone());
            std::thread::spawn(move || repair_with(&project, &progress, &cancel, |_, _| Ok(cache)))
        };
        while !run.is_finished() {
            let p = progress.lock().unwrap().clone();
            println!(
                "{:6.1} s {:?} {:5.1}% copied {} MiB checked {} MiB of {} MiB, {} of {} files, removed {}",
                start.elapsed().as_secs_f32(),
                p.phase,
                p.fraction().unwrap_or(0.0) * 100.0,
                p.bytes_copied >> 20,
                p.bytes_checked >> 20,
                p.bytes_total >> 20,
                p.files_done,
                p.files_total,
                p.files_removed
            );
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        let end = run.join().unwrap().unwrap();
        println!("{:.1} s: {end:?}", start.elapsed().as_secs_f32());
        remove_redirect(project.join("Library")).unwrap();
    }
}
