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

const BACKUP_PREFIX: &str = "Library.orbiters-relocation-";

#[derive(Clone, Debug, Default)]
pub struct Health {
    pub free: Option<u64>,
    pub disk_error: bool,
    pub open: bool,
    /// The disk `free` was read on ("H:\"): the one the Library really is on, through a redirect.
    pub disk: Option<PathBuf>,
}

/// What the panel has to say about a project's disk. Running low and Unity having actually failed are different things
/// to be told: the first does not stop anything, and the project opens as usual.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    let path = project.to_string_lossy().replace('\\', "/").to_lowercase();
    log.lines().any(|line| {
        let line = line.replace('\\', "/").to_lowercase();
        (line.contains("enospc") || line.contains("no space left on device")) && line.contains(&format!("{path}/"))
    })
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
    let log = crate::editor_log();
    let mut text = Vec::new();
    if let Some(mut file) = log.and_then(|p| std::fs::File::open(p).ok()) {
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        let _ = file.seek(SeekFrom::Start(len.saturating_sub(1024 * 1024)));
        let _ = file.take(1024 * 1024).read_to_end(&mut text);
    }
    Health {
        free: crate::size::free_space(location),
        disk_error: matches_disk_error(&String::from_utf8_lossy(&text), project),
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
        let root = with_room(&destinations(library), needed).map(|(root, _)| root).context("No local disk has room for this project's Library.")?;
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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

#[derive(Clone, Debug, Default)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
        let entry = entry.with_context(|| format!("Could not read {}", library.display()))?;
        let meta = entry.metadata().with_context(|| format!("Could not read {}", entry.path().display()))?;
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepairEnd {
    Moved(Moved),
    /// Cancelled: Library is where it was. `left`: the partial copy, when it could not be deleted (it goes with the next
    /// [`remove_orphans`]).
    Cancelled { left: Option<PathBuf> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (project, progress, cancel);
        bail!("Not enough space for Unity imports. Free at least 8 GB on the project's disk.");
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
                Some((root, free)) => format!("No local disk has room: the Library and Unity's imports need {needed} bytes; the most any other disk has is {free} bytes ({}).", root.display()),
                None => "No other local disk was found to move the Library to.".to_string(),
            })?;
            remember_cache_root(&root)?;
            Ok(root)
        })
    }
}

/// [`repair`] with its destination given. `pick`: the cache folder for a Library needing `needed` bytes.
#[cfg(any(windows, unix))]
fn repair_with(project: &Path, progress: &SharedProgress, cancel: &AtomicBool, pick: impl FnOnce(&Path, u64) -> anyhow::Result<PathBuf>) -> anyhow::Result<RepairEnd> {
    anyhow::ensure!(project.is_dir(), "The project's folder is missing: {}", project.display());
    anyhow::ensure!(!crate::unity_open(project), "Unity has this project open. Quit its window, then choose Repair storage.");
    let project = project.canonicalize()?;
    let _lock = project_lock(&project)?;
    let library = project.join("Library");
    if std::fs::symlink_metadata(&library).is_ok_and(|m| crate::discovery::is_link(&m)) {
        bail!("The project's Library is already redirected and its disk is full. Free space on that disk before retrying.");
    }
    update(progress, |p| p.phase = Phase::Measuring);
    let Some(walked) = walk_library(&library, progress, cancel)? else { return Ok(RepairEnd::Cancelled { left: None }) };
    let files = walked.map_err(|at| anyhow::anyhow!("Library contains a linked folder ({}); automatic relocation stopped without changing it.", at.display()))?;
    let bytes: u64 = files.iter().map(|(_, n)| *n).sum();
    // Checked again now, not only when the panel said what it would do: the disks may have filled since.
    let root = pick(&library, room_needed(bytes))?;
    let target = root.join(uuid::Uuid::new_v4().simple().to_string());
    relocate(&project, &target, &files, &Work { progress, cancel }, |target, library| redirect(target, library))
}

/// Makes `link` lead to the folder `target`: an NTFS junction.
#[cfg(windows)]
fn redirect(target: impl AsRef<Path>, link: impl AsRef<Path>) -> std::io::Result<()> {
    junction::create(target, link)
}

#[cfg(windows)]
fn is_redirect(link: impl AsRef<Path>) -> std::io::Result<bool> {
    junction::exists(link)
}

#[cfg(windows)]
fn remove_redirect(link: impl AsRef<Path>) -> std::io::Result<()> {
    junction::delete(link)
}

/// Makes `link` lead to the folder `target`: a symbolic link, naming the whole path so that it holds wherever it is read
/// from (a project moved by hand takes it along, as on Windows).
#[cfg(unix)]
fn redirect(target: impl AsRef<Path>, link: impl AsRef<Path>) -> std::io::Result<()> {
    std::os::unix::fs::symlink(std::path::absolute(target)?, link)
}

#[cfg(unix)]
fn is_redirect(link: impl AsRef<Path>) -> std::io::Result<bool> {
    Ok(std::fs::symlink_metadata(link)?.file_type().is_symlink())
}

#[cfg(unix)]
fn remove_redirect(link: impl AsRef<Path>) -> std::io::Result<()> {
    std::fs::remove_file(link)
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
    std::fs::create_dir_all(project.join("Temp"))?;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(project.join("Temp/UnityLockfile"))
        .context("Close this project's Unity window before repairing storage")
}

/// The project's lock here is a lock on the same file, which also stops Unity if it takes one (`flock` and POSIX locks
/// meet on the Mac). An editor that has the project open without one is looked for by its command line too.
#[cfg(unix)]
fn project_lock(project: &Path) -> anyhow::Result<std::fs::File> {
    const HELD: &str = "Close this project's Unity window before repairing storage";
    std::fs::create_dir_all(project.join("Temp"))?;
    let file = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(project.join("Temp/UnityLockfile")).context(HELD)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => bail!(HELD),
        // A file system that has no locks (exFAT): nobody can hold one there, Unity included.
        Err(std::fs::TryLockError::Error(e)) if e.kind() == std::io::ErrorKind::Unsupported || e.raw_os_error() == Some(libc::ENOTSUP) => {}
        Err(std::fs::TryLockError::Error(e)) => return Err(e).context(HELD),
    }
    anyhow::ensure!(crate::editor::of(&crate::editor::running(), project, false).is_none(), HELD);
    Ok(file)
}

/// Copies one file and reads it back. `false` when cancelled, partway through it if it is a large one. The copy keeps
/// the original's modification time and read-only flag, as Library's other readers (and `std::fs::copy`) expect.
#[cfg(any(windows, unix))]
fn copy_and_check(from: &Path, to: &Path, buffers: &mut [Vec<u8>; 2], work: &Work) -> anyhow::Result<bool> {
    let [left, right] = buffers;
    let mut source = std::fs::File::open(from)?;
    let meta = source.metadata()?;
    let mut copy = std::fs::File::create(to)?;
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
    copy.flush()?;
    if let Ok(modified) = meta.modified() {
        let _ = copy.set_modified(modified);
    }
    drop(copy);
    drop(source);
    let same = same_bytes(from, to, left, right, |n| {
        update(work.progress, |p| p.bytes_checked += n as u64);
        !is_cancelled(work.cancel)
    })?;
    match same {
        None => return Ok(false),
        Some(true) => {}
        Some(false) => bail!("A copied file differs from its original: {}", from.display()),
    }
    // Here a new file is not executable either, and Library holds programs (Burst's compiler, in a package).
    if cfg!(unix) || meta.permissions().readonly() {
        let _ = std::fs::set_permissions(to, meta.permissions());
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
            let _ = std::fs::remove_file(target.with_extension("json"));
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
        None => "The partial copy was deleted.".to_string(),
        Some(p) => format!("The partial copy at {} could not be deleted; Orbiters tries again the next time its window opens.", p.display()),
    }
}

/// Copies `files` (inside the project's Library) to `target`, checks them, then switches Library to a junction there.
/// Until the switch nothing of the project changes. Any failure or Cancel before it deletes the copy; a failed switch
/// restores Library first.
#[cfg(any(windows, unix))]
fn relocate(project: &Path, target: &Path, files: &[(PathBuf, u64)], work: &Work, link: impl FnOnce(&Path, &Path) -> std::io::Result<()>) -> anyhow::Result<RepairEnd> {
    let library = project.join("Library");
    let bytes: u64 = files.iter().map(|(_, n)| *n).sum();
    anyhow::ensure!(!target.exists(), "Cache destination already exists");
    // The record first: a kill at any moment after this leaves a cache that says which project it was for.
    crate::write_json(&target.with_extension("json"), &serde_json::json!({"project":project,"library":target}))?;
    std::fs::create_dir_all(target)?;
    update(work.progress, |p| {
        p.phase = Phase::Copying;
        p.bytes_total = bytes;
        p.files_total = files.len() as u64;
        p.bytes_copied = 0;
        p.bytes_checked = 0;
        p.files_done = 0;
        p.destination = Some(target.to_path_buf());
    });
    let mut buffers = [vec![0u8; 1024 * 1024], vec![0u8; 1024 * 1024]];
    let mut copy_all = || -> anyhow::Result<bool> {
        for (file, _) in files {
            let relative = file.strip_prefix(&library)?;
            let to = target.join(relative);
            std::fs::create_dir_all(to.parent().context("A file without a folder")?)?;
            update(work.progress, |p| p.file = relative.display().to_string());
            if !copy_and_check(file, &to, &mut buffers, work).with_context(|| format!("Could not copy {}", relative.display()))? {
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
            return Err(error).with_context(|| format!("Library was not moved and stays where it is. {}", partial_note(&left)));
        }
    }
    // The last moment Cancel can still leave everything as it was.
    if is_cancelled(work.cancel) {
        return Ok(RepairEnd::Cancelled { left: discard_copy(target, work) });
    }
    update(work.progress, |p| p.phase = Phase::Switching);
    let backup = project.join(format!("{BACKUP_PREFIX}{}", uuid::Uuid::new_v4().simple()));
    let had_library = library.is_dir();
    if had_library {
        if let Err(error) = std::fs::rename(&library, &backup) {
            let left = discard_copy(target, work);
            return Err(error).with_context(|| format!("Something is using this project's Library, so it was not moved and stays where it is. {}", partial_note(&left)));
        }
    }
    if let Err(error) = link(target, &library) {
        // A failed reparse-point creation can leave an empty directory behind.
        if is_redirect(&library).unwrap_or(false) {
            remove_redirect(&library)?;
        }
        if library.exists() {
            std::fs::remove_dir(&library).context("Could not remove an empty failed Library redirect")?;
        }
        if had_library {
            // The copy stays when Library cannot come back: it is then the only complete one under that name.
            std::fs::rename(&backup, &library).with_context(|| format!("Could not restore Library after a failed redirect. It is at {}, and a checked copy is at {}.", backup.display(), target.display()))?;
        }
        let left = discard_copy(target, work);
        return Err(error).with_context(|| format!("Could not redirect Unity's cache, so Library stays where it is. {}", partial_note(&left)));
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

/// Whether two files hold the same bytes; `on_chunk` hears the bytes compared and answers whether to go on (`None` when
/// it said to stop).
#[cfg_attr(not(any(windows, unix)), allow(dead_code))]
fn same_bytes(a: &Path, b: &Path, left: &mut [u8], right: &mut [u8], mut on_chunk: impl FnMut(usize) -> bool) -> std::io::Result<Option<bool>> {
    if std::fs::metadata(a)?.len() != std::fs::metadata(b)?.len() {
        return Ok(Some(false));
    }
    let mut a = std::fs::File::open(a)?;
    let mut b = std::fs::File::open(b)?;
    loop {
        let n = a.read(left)?;
        if n == 0 {
            return Ok(Some(true));
        }
        b.read_exact(&mut right[..n])?;
        if left[..n] != right[..n] {
            return Ok(Some(false));
        }
        if !on_chunk(n) {
            return Ok(None);
        }
    }
}

/// Deletes a folder this module made (never following a link out of it), telling `on_file` of each file, going on past a
/// file that cannot be deleted so that as much as possible is freed. The first error, if any.
#[cfg_attr(not(any(windows, unix)), allow(dead_code))]
fn remove_tree(root: &Path, on_file: &mut dyn FnMut()) -> std::io::Result<()> {
    fn remove_link(path: &Path) -> std::io::Result<()> {
        std::fs::remove_dir(path).or_else(|_| std::fs::remove_file(path))
    }
    /// What stops a removal here is not the file's own read-only mode but its folder's: packages unpack read-only.
    #[cfg(unix)]
    fn open_parent(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        if let Some(parent) = path.parent() {
            if let Ok(meta) = std::fs::symlink_metadata(parent) {
                let mut open = meta.permissions();
                open.set_mode(open.mode() | 0o700);
                let _ = std::fs::set_permissions(parent, open);
            }
        }
    }
    fn remove_file(path: &Path, meta: Option<std::fs::Metadata>) -> std::io::Result<()> {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                // A read-only file (the copy keeps the flag).
                if let Some(meta) = meta {
                    let mut writable = meta.permissions();
                    #[allow(clippy::permissions_set_readonly_false)]
                    writable.set_readonly(false);
                    let _ = std::fs::set_permissions(path, writable);
                }
                #[cfg(unix)]
                open_parent(path);
                std::fs::remove_file(path)
            }
            other => other,
        }
    }
    fn remove_folder(path: &Path) -> std::io::Result<()> {
        match std::fs::remove_dir(path) {
            #[cfg(unix)]
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                open_parent(path);
                std::fs::remove_dir(path)
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
        (Some(RootDir), Some(Normal(volumes)), Some(Normal(disk))) if volumes == "Volumes" => Path::new("/Volumes").join(disk).exists(),
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

/// The old Libraries a move left beside its project's Library.
fn backups(project: &Path) -> Vec<PathBuf> {
    let Ok(dir) = std::fs::read_dir(project) else { return vec![] };
    dir.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(BACKUP_PREFIX) && e.path().is_dir()).map(|e| e.path()).collect()
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
    let own = project.join("Library");
    match points_at(&own, cache) {
        None => return Keep(Kept::Unknown),
        Some(true) => {
            let old = backups(&project);
            return if old.is_empty() { Keep(Kept::InUse) } else { KeepAndClean { project, old } };
        }
        Some(false) => {}
    }
    for other in known {
        match points_at(&other.join("Library"), cache) {
            Some(false) => {}
            Some(true) => return Keep(Kept::InUse),
            None => return Keep(Kept::Unknown),
        }
    }
    // A Library renamed away, with the junction not yet made: it goes back. More than one such folder cannot be told apart.
    let restore = if std::fs::symlink_metadata(&own).is_err() {
        let mut old = backups(&project);
        match old.len() {
            0 => None,
            1 => old.pop(),
            _ => return Keep(Kept::Unknown),
        }
    } else {
        None
    };
    Remove { project, restore }
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
                let still = points_at(&project.join("Library"), &cache) == Some(true);
                drop(lock);
                if still {
                    for path in old {
                        match remove_tree(&path, &mut || {}) {
                            Ok(()) => swept.old_removed.push(path),
                            Err(e) => swept.failed.push((path, e.to_string())),
                        }
                    }
                }
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
                    if let Err(e) = std::fs::rename(&old, project.join("Library")) {
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
                        let _ = std::fs::remove_file(cache.with_extension("json"));
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
            assert_eq!(std::fs::read_dir(&library).unwrap().count() > 0, files > 0);
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
        assert_eq!(backups(&project), Vec::<PathBuf>::new());
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
        assert_eq!(backups(&project), Vec::<PathBuf>::new());
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
        assert_eq!(backups(&project), Vec::<PathBuf>::new());
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
        assert_eq!(backups(&project), Vec::<PathBuf>::new());
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
        crate::write_json(&cache.with_extension("json"), &serde_json::json!({"project": project, "library": cache})).unwrap();
        cache
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
        let old = project.join(format!("{BACKUP_PREFIX}abc"));
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
        let old = project.join(format!("{BACKUP_PREFIX}abc"));
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
    #[cfg(any(windows, unix))]
    #[test]
    #[ignore]
    fn a_measured_repair() {
        let mb: usize = std::env::var("ORBITERS_REPAIR_MB").ok().and_then(|s| s.parse().ok()).unwrap_or(1024);
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let project = project(temp.path(), "project", mb / 4, 4 * 1024 * 1024);
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
