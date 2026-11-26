//! Installing, updating and uninstalling, without the window: the window ([`crate::ui`]) runs these on a thread and
//! shows their [`Progress`]; `--silent` runs them as they are.
//!
//! An install unpacks the payload into a folder of its own inside the install folder (`.setup-new`, checked file by
//! file), waits for nothing to run from the install folder any more, then moves each file in place, the old ones
//! aside (`.setup-old`; [`crate::swap`]). Moves within one folder cannot run out of space halfway, and if one fails
//! anyway (a file held by another program, a power cut), the ones made are moved back and the install is as it was. Only
//! the files of the payload and of the last install's record are touched: the folder may be one the person chose, with
//! their own files in it.

use crate::system;
use anyhow::Context as _;
use orbiters_i18n::tr;
use orbiters_update::{inside, payload, InstallRecord, PRODUCT, RECORD_FILE};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Where the payload is unpacked, inside the install folder.
pub(crate) const STAGING: &str = ".setup-new";
/// Where the files an install replaces wait until it has finished, inside the install folder.
pub(crate) const BACKUP: &str = ".setup-old";
/// How long the app gets to quit before the window offers to close it (it finishes saving a recording first).
const PATIENCE: Duration = Duration::from_secs(20);
/// Space kept free beyond the files themselves.
const MARGIN: u64 = 64 << 20;

/// What the person chose.
#[derive(Clone, Debug)]
pub struct Options {
    pub dir: PathBuf,
    pub desktop_shortcut: bool,
    pub start_with_windows: bool,
    pub start_with_steamvr: bool,
    /// The release channel the app follows.
    pub channel: String,
}

/// What a running task shows, and what the window can ask of it.
#[derive(Default)]
pub struct Progress {
    status: Mutex<Status>,
    /// The person chose to close what would not close.
    force: AtomicBool,
    /// The person gave up waiting for it.
    cancel: AtomicBool,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub fraction: f32,
    pub label: String,
    /// The app has not closed after [`PATIENCE`]: the window offers to close it.
    pub stuck: bool,
    /// What Windows would not take (a shortcut, the installed-apps entry): the install is done all the same.
    pub notes: Vec<String>,
    /// The outcome, once it ended.
    pub done: Option<Result<(), String>>,
}

impl Progress {
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    fn set(&self, fraction: f32, label: impl Into<String>) {
        let mut s = self.status.lock().unwrap();
        s.fraction = fraction.clamp(0.0, 1.0);
        s.label = label.into();
    }

    fn label(&self, label: impl Into<String>) {
        self.status.lock().unwrap().label = label.into();
    }

    fn stuck(&self, stuck: bool) {
        self.status.lock().unwrap().stuck = stuck;
    }

    fn note(&self, notes: Vec<String>) {
        self.status.lock().unwrap().notes = notes;
    }

    /// Shows a state as a running task would (the screenshots).
    pub fn show(&self, fraction: f32, label: &str, stuck: bool) {
        self.set(fraction, label);
        self.stuck(stuck);
    }

    pub fn finish(&self, result: anyhow::Result<()>) {
        self.status.lock().unwrap().done = Some(result.map_err(|e| format!("{e:#}")));
    }

    /// Closes what is still running from the install folder.
    pub fn force(&self) {
        self.force.store(true, Ordering::Relaxed);
    }

    /// Stops waiting (the task ends with an error; nothing was changed yet).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn forced(&self) -> bool {
        self.force.load(Ordering::Relaxed)
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// The programs of this install in `dir`: the `.exe` files the last install's record and the payload name, at their
/// exact places. Only these are waited for or closed (PROJECTS-15): the folder may hold other programs of the person's.
fn own_programs<'a>(dir: &Path, files: impl Iterator<Item = &'a str>) -> Vec<PathBuf> {
    let mut programs: Vec<PathBuf> = files.filter(|f| f.to_ascii_lowercase().ends_with(".exe")).filter_map(|f| inside(dir, f)).collect();
    programs.push(dir.join(orbiters_update::APP_EXE));
    programs.sort();
    programs.dedup();
    programs
}

/// Makes sure none of this install's `programs` runs any more. The app is asked to quit as its own Quit does, when it
/// runs from there (an instance from elsewhere, a development build, is left alone; its files are not these). Past
/// [`PATIENCE`] the window offers to close what is left; `silent` (an automatic update, nobody to ask) closes nothing:
/// the update waits for another time.
fn close_app(dir: &Path, programs: &[PathBuf], progress: &Progress, silent: bool) -> anyhow::Result<()> {
    if system::processes_of(programs).is_empty() {
        return Ok(());
    }
    progress.label(tr("Closing Orbiters - N4N0…"));
    if system::app_runs_from(dir) {
        system::ask_app_to_quit();
    }
    let stop = || progress.forced() || progress.cancelled();
    if !system::wait_for_programs(programs, PATIENCE, stop) {
        if silent {
            // Nobody to ask and nothing closed by force: the app may be saving a recording. It gets longer, then the
            // update waits for another time (`main.rs` schedules it at the next sign-in).
            system::wait_for_programs(programs, silent_patience(), || false);
        } else {
            progress.stuck(true);
            while !system::wait_for_programs(programs, Duration::from_secs(1), stop) && !stop() {}
            progress.stuck(false);
            anyhow::ensure!(!progress.cancelled(), "{}", tr("Cancelled: Orbiters - N4N0 was still running."));
            system::terminate_programs(programs);
        }
    }
    if !system::processes_of(programs).is_empty() {
        return Err(StillRunning(dir.to_path_buf()).into());
    }
    Ok(())
}

/// How much longer an automatic update waits for the app to quit by itself.
fn silent_patience() -> Duration {
    if cfg!(test) {
        Duration::from_secs(1)
    } else {
        Duration::from_secs(120)
    }
}

/// The install's programs did not quit: nothing was changed, the update can be tried again.
#[derive(Debug)]
pub struct StillRunning(pub PathBuf);
impl std::fmt::Display for StillRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&orbiters_i18n::tr!("Orbiters - N4N0 is still running from {folder}.", folder = self.0.display()))
    }
}
impl std::error::Error for StillRunning {}

/// Whether `path` is a real folder: there, and not a link or junction.
fn real_dir(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink() && m.file_attributes() & 0x400 == 0)
}

/// `relative` in `dir`, when nothing between them is a link or a junction (PROJECTS-42): a folder of the install
/// redirected elsewhere would make an update or uninstall move or remove files outside the install folder.
pub(crate) fn contained(dir: &Path, relative: &str) -> Option<PathBuf> {
    use std::os::windows::fs::MetadataExt;
    const REPARSE_POINT: u32 = 0x400;
    let path = inside(dir, relative)?;
    // The base itself too: a backup or staging folder that is a junction leads elsewhere. One not made yet is made by
    // setup, a real folder.
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        _ if real_dir(dir) => {}
        _ => return None,
    }
    let mut at = dir.to_path_buf();
    for part in Path::new(relative).components() {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.file_type().is_symlink() || m.file_attributes() & REPARSE_POINT != 0 => return None,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            // What cannot be looked at cannot be shown to be inside.
            Err(_) => return None,
        }
    }
    Some(path)
}

/// The payload's files already in `dir` that are not this app's (PROJECTS-34): not in the last install's record, and not
/// the payload's own bytes. An install would put them aside and then remove them.
fn collisions(dir: &Path, index: &payload::Index, previous: Option<&InstallRecord>) -> Vec<String> {
    use sha2::{Digest, Sha256};
    let owned = |path: &str| previous.is_some_and(|r| r.files.iter().any(|f| f.eq_ignore_ascii_case(path)));
    let same = |file: &Path, sha256: &str| {
        let Ok(mut f) = std::fs::File::open(file) else { return false };
        let mut hasher = Sha256::new();
        std::io::copy(&mut f, &mut hasher).is_ok() && format!("{:x}", hasher.finalize()) == sha256.to_ascii_lowercase()
    };
    index
        .files
        .iter()
        .filter(|e| !owned(&e.path))
        .filter(|e| inside(dir, &e.path).is_some_and(|dest| dest.exists() && !(dest.is_file() && same(&dest, &e.sha256))))
        .map(|e| e.path.clone())
        .collect()
}

/// The payload's and the last record's files reached through a link or a junction ([`contained`]).
fn redirected<'a>(dir: &Path, paths: impl Iterator<Item = &'a str>) -> Vec<String> {
    paths.filter(|p| inside(dir, p).is_some() && contained(dir, p).is_none()).map(str::to_string).collect()
}

/// What a cut install left (F15): files the last install had that are missing in `dir` and waiting in the backup folder
/// go back first; then the backup folder (its other files may be the only copy of something) and a staging folder
/// holding anything but payload files are kept under a name of their own instead of being removed.
fn recover_leftovers(dir: &Path, staging: &Path, backup: &Path, previous: Option<&InstallRecord>) -> anyhow::Result<()> {
    if real_dir(backup) {
        for path in previous.map(|r| r.files.as_slice()).unwrap_or_default() {
            let (Some(dest), Some(from)) = (contained(dir, path), contained(backup, path)) else { continue };
            if !dest.exists() && from.is_file() && make_parent(&dest).is_ok() {
                let _ = rename(&from, &dest);
            }
        }
    }
    // What is left in either is kept, under a name of its own; neither is ever removed with its contents (they may be
    // the person's, or the only copy of a file). When it cannot be kept aside, the install stops here.
    for folder in [backup, staging] {
        if std::fs::symlink_metadata(folder).is_err() {
            continue;
        }
        if real_dir(folder) {
            remove_empty_dirs(folder);
            if !folder.exists() {
                continue;
            }
        }
        anyhow::ensure!(
            keep_aside(dir, folder).is_ok(),
            "{}",
            tr!("{folder} is left from an earlier install and could not be set aside: nothing was changed.", folder = folder.display())
        );
    }
    Ok(())
}

/// What an earlier install left at `path` in `dir`, which says nothing that can be acted on, moved beside it under a name of its
/// own (`<name>-<time>-<n>`): kept as it is, never removed, and its name free for this install. Where it went.
pub(crate) fn keep_aside(dir: &Path, path: &Path) -> std::io::Result<PathBuf> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stamp = time::OffsetDateTime::now_utc().unix_timestamp();
    let free = (0..100).map(|n| dir.join(format!("{name}-{stamp}-{n}"))).find(|p| std::fs::symlink_metadata(p).is_err());
    let to = free.ok_or_else(|| std::io::Error::from(std::io::ErrorKind::AlreadyExists))?;
    rename(path, &to)?;
    Ok(to)
}

/// Removes the empty folders of `folder` (and it, once empty), deepest first; never a file, never through a link.
pub(crate) fn remove_empty_dirs(folder: &Path) {
    if !real_dir(folder) {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(folder) {
        for e in entries.flatten() {
            if real_dir(&e.path()) {
                remove_empty_dirs(&e.path());
            }
        }
    }
    let _ = std::fs::remove_dir(folder);
}


/// Installs the payload with `options`. `fresh` is a choice the person just made (the setup's window); an update
/// keeps what they changed since (a desktop shortcut they deleted stays deleted).
pub fn install(bytes: &[u8], options: &Options, progress: &Progress, silent: bool, fresh: bool) -> anyhow::Result<()> {
    install_with(bytes, options, progress, silent, fresh, &Windows)
}

/// What an install asks of Windows beside its files.
trait Surroundings {
    /// The entries and shortcuts of the install in `dir`: what could not be done, in words. Everything is tried.
    fn integrate(&self, dir: &Path, record: &InstallRecord, previous: Option<&InstallRecord>, size: u64, fresh: bool) -> Vec<String>;
    /// Whether a swap in `dir` is under way, which the next sign-in is to put back if it is cut.
    fn swapping(&self, dir: &Path, under_way: bool);
}

struct Windows;

impl Surroundings for Windows {
    fn integrate(&self, dir: &Path, record: &InstallRecord, previous: Option<&InstallRecord>, size: u64, fresh: bool) -> Vec<String> {
        integrate(dir, record, previous, size, fresh)
    }

    fn swapping(&self, dir: &Path, under_way: bool) {
        let command = std::env::current_exe().ok().filter(|_| under_way).and_then(|me| crate::swap::recover_command(&me, dir));
        let told = match command {
            Some(command) => system::schedule_recovery(&command),
            None => {
                system::cancel_recovery();
                Ok(())
            }
        };
        if let Err(e) = told {
            eprintln!("the next sign-in could not be told of the swap: {e:#}"); // i18n: not shown (a log line)
        }
    }
}

fn install_with(bytes: &[u8], options: &Options, progress: &Progress, silent: bool, fresh: bool, windows: &dyn Surroundings) -> anyhow::Result<()> {
    let dir = &options.dir;
    let index = payload::read_index(bytes)?;
    if let Some(free) = system::free_space(dir) {
        anyhow::ensure!(
            free > index.total() + MARGIN,
            "{}",
            orbiters_i18n::tr!("Not enough free space on the drive of {folder}: {size} MB are needed.", folder = dir.display(), size = (index.total() + MARGIN) >> 20)
        );
    }
    std::fs::create_dir_all(dir).with_context(|| orbiters_i18n::tr!("{path} cannot be created", path = dir.display()))?;
    let staging = dir.join(STAGING);
    let backup = dir.join(BACKUP);
    // What a swap that was cut left, before anything else is looked at.
    let left = crate::swap::left_programs(dir)?;
    if !left.is_empty() {
        close_app(dir, &own_programs(dir, left.iter().map(String::as_str)), progress, silent)?;
    }
    crate::swap::recover(dir)?;
    // A record there that cannot be read now is not "no install" (W1-1): that would take this app's files for the person's.
    let previous = InstallRecord::load(dir).map_err(|e| crate::swap::unreadable(&dir.join(RECORD_FILE), e))?;
    // Nothing is changed when a file of the payload would replace one of the person's, or would be reached through a
    // folder redirected elsewhere.
    let mut taken = collisions(dir, &index, previous.as_ref());
    // The record's own names (PROJECTS-34): without a record of ours, a file there is the person's. Except the record's draft:
    // a plain file at that exact name is what an older setup, which wrote it in place, left when it was cut before renaming it
    // (V2-2); it is kept beside below, never a reason to refuse the folder for good. (The journal's, the same, is kept beside
    // by the recovery above: what is still at its name is a folder or a link.)
    let mut drafts = vec![];
    if previous.is_none() {
        for name in [RECORD_FILE, crate::swap::RECORD_DRAFT, crate::swap::JOURNAL] {
            let file = dir.join(name);
            match std::fs::symlink_metadata(&file) {
                Ok(m) if name == crate::swap::RECORD_DRAFT && m.is_file() && std::os::windows::fs::MetadataExt::file_attributes(&m) & 0x400 == 0 => drafts.push(file),
                Ok(_) => taken.push(name.to_string()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(crate::swap::unreadable(&file, e)),
            }
        }
    }
    anyhow::ensure!(
        taken.is_empty(),
        "{}",
        tr!("These files are already in {folder} and are not Orbiters - N4N0's: {files}. Nothing was changed: choose another folder.", folder = dir.display(), files = taken.join(", "))
    );
    for draft in drafts {
        keep_aside(dir, &draft).with_context(|| tr!("{path} cannot be written", path = draft.display()))?;
    }
    let new_and_old = index.files.iter().map(|f| f.path.as_str()).chain(previous.iter().flat_map(|r| r.files.iter().map(String::as_str)));
    let linked = redirected(dir, new_and_old);
    anyhow::ensure!(
        linked.is_empty(),
        "{}",
        tr!("These files of {folder} are reached through a link to another folder: {files}. Nothing was changed.", folder = dir.display(), files = linked.join(", "))
    );
    // What a cut install left.
    recover_leftovers(dir, &staging, &backup, previous.as_ref())?;

    progress.set(0.02, tr("Unpacking…"));
    let unpacked = payload::unpack(bytes, &staging, |done, total, file| {
        progress.set(0.02 + 0.78 * (done as f32 / total.max(1) as f32), tr!("Unpacking {file}", file = file));
    });
    if let Err(e) = unpacked {
        remove_tree(&staging);
        return Err(e);
    }
    let programs = own_programs(dir, index.files.iter().map(|f| f.path.as_str()).chain(previous.iter().flat_map(|r| r.files.iter().map(String::as_str))));
    if let Err(e) = close_app(dir, &programs, progress, silent) {
        remove_tree(&staging);
        return Err(e);
    }

    progress.set(0.82, tr("Putting the new version in place…"));
    let record = InstallRecord {
        product: PRODUCT.into(),
        version: index.version.clone(),
        channel: options.channel.clone(),
        files: index.files.iter().map(|f| f.path.clone()).collect(),
        desktop_shortcut: options.desktop_shortcut,
        start_with_windows: options.start_with_windows,
        start_with_steamvr: options.start_with_steamvr,
        installed: now(),
    };
    // (A first install that is cut has no version to put back at the next sign-in: the setup is simply run again.)
    if previous.is_some() {
        windows.swapping(dir, true);
    }
    let swapped = crate::swap::swap(dir, &staging, &backup, &index, previous.as_ref(), &record);
    if previous.is_some() && !dir.join(crate::swap::JOURNAL).exists() {
        windows.swapping(dir, false);
    }
    swapped?;
    // The new version is in place: from here nothing is a failure of the install.
    if !dir.join(crate::swap::JOURNAL).exists() {
        remove_tree(&staging);
        remove_tree(&backup);
    }
    progress.set(0.9, tr("Adding it to Windows…"));
    progress.note(windows.integrate(dir, &record, previous.as_ref(), index.total(), fresh));
    progress.set(1.0, tr("Installed"));
    Ok(())
}

/// Windows' side: the installed-apps entry, the Start menu (and desktop) shortcut, the start with Windows; and SteamVR's
/// start of the app, when SteamVR runs (else the app tells it, from the record, the next time it runs).
/// What could not be done, in words: each of these is tried whatever became of the others (PRJ-41).
fn integrate(dir: &Path, record: &InstallRecord, previous: Option<&InstallRecord>, size: u64, fresh: bool) -> Vec<String> {
    let mut notes = Vec::new();
    let mut tried = |what: &str, done: anyhow::Result<()>| {
        if let Err(e) = done {
            eprintln!("{what}: {e:#}");
            notes.push(what.to_string());
        }
    };
    tried(tr("The installed-apps entry cannot be written"), system::register(dir, &record.version, size));
    if let Some(link) = system::start_menu_shortcut() {
        tried(tr("The Start menu shortcut cannot be written"), system::write_shortcut(&link, dir));
    }
    if let Some(link) = system::desktop_shortcut() {
        if record.desktop_shortcut && (fresh || link.exists()) {
            tried(tr("The desktop shortcut cannot be written"), system::write_shortcut(&link, dir));
        } else if !record.desktop_shortcut && previous.is_some_and(|p| p.desktop_shortcut) {
            let _ = std::fs::remove_file(&link);
        }
    }
    if fresh {
        if record.start_with_windows {
            tried(tr("The start with Windows cannot be set"), system::set_start_with_windows(dir, true));
        } else if system::starts_with_windows_from(dir) {
            tried(tr("The start with Windows cannot be set"), system::set_start_with_windows(dir, false));
        }
        // What SteamVR could not be told now, the app tells it the next time SteamVR runs (its settings keep the choice).
        if record.start_with_steamvr || previous.is_some_and(|p| p.start_with_steamvr) {
            let told = system::set_start_with_steamvr(dir, record.start_with_steamvr).unwrap_or_else(|e| {
                eprintln!("SteamVR's start of the app: {e:#}");
                false
            });
            if let Err(e) = system::remember_start_with_steamvr(record.start_with_steamvr, told) {
                eprintln!("the app's settings: {e:#}");
            }
        }
    }
    system::refresh_shell();
    notes
}

/// Updates the install in `dir` to the payload, keeping its options and channel.
pub fn update(bytes: &[u8], dir: &Path, progress: &Progress, silent: bool) -> anyhow::Result<InstallRecord> {
    let record = InstallRecord::load(dir).map_err(|e| crate::swap::unreadable(&dir.join(RECORD_FILE), e))?.with_context(|| tr!("Orbiters - N4N0 is not installed in {folder}.", folder = dir.display()))?;
    let options = Options {
        dir: dir.to_path_buf(),
        desktop_shortcut: record.desktop_shortcut,
        start_with_windows: record.start_with_windows,
        start_with_steamvr: record.start_with_steamvr,
        channel: record.channel,
    };
    install(bytes, &options, progress, silent, false)?;
    InstallRecord::load(dir).map_err(|e| crate::swap::unreadable(&dir.join(RECORD_FILE), e))?.with_context(|| tr("The install record is missing after the update."))
}

/// How long an uninstall waits for SteamVR to be closed, for a value still owed to it.
const BOOSTER_PATIENCE: Duration = Duration::from_secs(120);

/// Uninstalls the app from `dir`: what it added to Windows, then the files its record lists (nothing else), and the
/// folder once empty. `remove_data` also removes its settings, caches and speech models; the person's sessions are
/// never touched.
pub fn uninstall(dir: &Path, remove_data: bool, progress: &Progress, silent: bool) -> anyhow::Result<()> {
    let record = InstallRecord::load(dir).map_err(|e| crate::swap::unreadable(&dir.join(RECORD_FILE), e))?.with_context(|| tr!("Orbiters - N4N0 is not installed in {folder}.", folder = dir.display()))?;
    progress.set(0.05, tr("Closing Orbiters - N4N0…"));
    close_app(dir, &own_programs(dir, record.files.iter().map(String::as_str)), progress, silent)?;
    // An update that was cut: what its journal names goes with the rest, whichever version it was.
    crate::swap::recover(dir)?;
    let record = InstallRecord::load(dir).map_err(|e| crate::swap::unreadable(&dir.join(RECORD_FILE), e))?.with_context(|| tr!("Orbiters - N4N0 is not installed in {folder}.", folder = dir.display()))?;
    system::cancel_recovery();

    finish_booster(progress, silent);

    progress.set(0.2, tr("Removing it from Windows…"));
    if system::registered_dir().is_some_and(|d| same_dir(&d, dir)) {
        system::unregister();
    }
    if system::starts_with_windows_from(dir) {
        let _ = system::set_start_with_windows(dir, false);
    }
    system::forget_steamvr();
    // Tracking smoothing's driver, which the app registered for SteamVR once: SteamVR no longer loads it (a DLL a
    // running SteamVR holds is moved out of the way). Never a reason for the uninstall to fail.
    if let Err(e) = orbiters_smooth::uninstall() {
        eprintln!("the smoothing driver: {e:#}");
    }
    if let Some(link) = system::start_menu_shortcut() {
        let _ = std::fs::remove_file(link);
    }
    if record.desktop_shortcut {
        if let Some(link) = system::desktop_shortcut() {
            let _ = std::fs::remove_file(link);
        }
    }
    system::refresh_shell();

    progress.set(0.35, tr("Removing its files…"));
    let roots = Roots {
        local: dirs::data_local_dir().unwrap_or_default(),
        config: dirs::config_dir().unwrap_or_default(),
    };
    let failed = remove_installation(dir, &record, remove_data, &roots, &mut |fraction, label| progress.set(fraction, label));
    anyhow::ensure!(failed.is_empty(), "{}", tr!("These files could not be removed (in use?): {files}", files = failed.join(", ")));
    progress.set(1.0, tr("Uninstalled"));
    Ok(())
}

/// The folders uninstall removes things under: the person's local and roaming application data (a test gives its own).
pub struct Roots {
    pub local: PathBuf,
    pub config: PathBuf,
}

/// The real folders from `top` down to `path`'s parent, each held for as long as the result lives (Windows: none can
/// be renamed or replaced meanwhile), when `top` and every folder between are real folders, never a link or junction
/// (verifier round 2: walk down from a verified root, follow no reparse point at any level). `None` otherwise.
fn pinned_way(top: &Path, path: &Path) -> Option<Vec<std::fs::File>> {
    let rel = path.strip_prefix(top).ok()?;
    let mut pins = vec![pin(top)?];
    let mut at = top.to_path_buf();
    let parts: Vec<_> = rel.components().collect();
    for part in parts.iter().take(parts.len().saturating_sub(1)) {
        if !matches!(part, std::path::Component::Normal(_)) {
            return None;
        }
        at.push(part);
        pins.push(pin(&at)?);
    }
    Some(pins)
}

/// `path` held open when it is a real folder (not a link), without letting anyone delete or rename it meanwhile.
fn pin(path: &Path) -> Option<std::fs::File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(0x1 | 0x2)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    (meta.is_dir() && meta.file_attributes() & 0x400 == 0).then_some(file)
}

/// Removes the folder `path` with all in it (a link there: the link only), when everything from `top` to it is real
/// and held; links inside are removed as links. What could not be, named.
fn remove_owned_tree(top: &Path, path: &Path) -> Option<String> {
    let Some(_pins) = pinned_way(top, path) else {
        return std::fs::symlink_metadata(path).is_ok().then(|| tr!("{path} is reached through a link to another folder: nothing in it was removed", path = path.display()));
    };
    match std::fs::symlink_metadata(path) {
        Err(_) => None,
        Ok(_) if !real_dir(path) => std::fs::remove_dir(path).or_else(|_| std::fs::remove_file(path)).err().map(|e| format!("{} ({e})", path.display())),
        Ok(_) => remove_dir_all_retrying(path).err().map(|e| format!("{} ({e})", path.display())),
    }
}

fn remove_dir_all_retrying(path: &Path) -> std::io::Result<()> {
    retrying(|| match std::fs::remove_dir_all(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    })
}

/// What uninstall removes once the app is closed and Windows was told: the record's files in `dir` (through real
/// folders only, `dir` held), its now empty folders, the downloaded updates, and with `remove_data` N4N0's settings,
/// caches, models and its own entries of the shared folders (by name). Setup's scratch folders and anything not
/// N4N0's stay. The paths that could not be removed (or were refused), named.
fn remove_installation(dir: &Path, record: &InstallRecord, remove_data: bool, roots: &Roots, progress: &mut dyn FnMut(f32, String)) -> Vec<String> {
    let mut failed = Vec::new();
    // The install folder itself must be a real one, held while its files go: a folder redirected elsewhere is not
    // emptied (its files there are not N4N0's to remove).
    match pin(dir) {
        None => failed.push(tr!("{path} is reached through a link to another folder: nothing in it was removed", path = dir.display())),
        Some(_held) => {
            let count = record.files.len().max(1);
            for (i, path) in record.files.iter().enumerate() {
                match contained(dir, path) {
                    Some(file) => {
                        // Every folder on the way held while it goes.
                        let pins = pinned_way(dir, &file);
                        if pins.is_none() {
                            failed.push(file.display().to_string());
                        } else if file.is_file() && remove_file(&file).is_err() {
                            failed.push(file.display().to_string());
                        }
                    }
                    // Reached through a link to another folder: never removed from there (PROJECTS-42).
                    None if inside(dir, path).is_some() => failed.push(dir.join(path).display().to_string()),
                    None => {}
                }
                progress(0.35 + 0.5 * (i + 1) as f32 / count as f32, tr!("Removing {path}", path = path));
            }
            // The record's temporary file, only when it is one of setup's (a person's file of that name stays).
            let tmp = dir.join(format!("{RECORD_FILE}.tmp"));
            let ours = std::fs::symlink_metadata(&tmp).is_ok_and(|m| m.is_file())
                && std::fs::read(&tmp).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()).is_some_and(|v| v["product"] == PRODUCT);
            if ours {
                let _ = std::fs::remove_file(&tmp);
            }
            if failed.is_empty() {
                let _ = remove_file(&dir.join(RECORD_FILE));
            }
            // The record's folders, deepest first: each only once empty, reached through real folders.
            let mut folders: Vec<PathBuf> =
                record.files.iter().filter_map(|p| contained(dir, p)).filter_map(|f| f.parent().map(Path::to_path_buf)).filter(|f| f != dir).collect();
            folders.sort_by_key(|f| std::cmp::Reverse(f.components().count()));
            folders.dedup();
            for folder in folders {
                if pinned_way(dir, &folder).is_some() && real_dir(&folder) {
                    let _ = std::fs::remove_dir(folder);
                }
            }
        }
    }
    // The install folder once empty (setup's scratch folders, left as they are, keep it).
    if real_dir(dir) {
        let _ = std::fs::remove_dir(dir);
    }

    progress(0.9, tr("Tidying up…").to_string());
    let local = &roots.local;
    let shared = local.join("Orbiters");
    failed.extend(remove_owned_tree(local, &shared.join("Desktop").join("updates")));
    if remove_data {
        let config_shared = roots.config.join("Orbiters");
        for (top, folder) in [(&roots.config, config_shared.join("Desktop")), (local, shared.join("Desktop")), (local, shared.join("Models"))] {
            failed.extend(remove_owned_tree(top, &folder));
        }
        // The FPS booster's reminder of what to change back by hand. Its journal is not touched here: `finish_booster`
        // removed it if nothing is owed.
        let booster = shared.join("booster");
        if pinned_way(local, &booster.join("to-put-back.json")).is_some() {
            let _ = std::fs::remove_file(booster.join("to-put-back.json"));
            let _ = std::fs::remove_dir(&booster);
        }
        // Its frame logs, written at every level and Off by default: large, and nobody else's.
        if let Some(_pins) = pinned_way(local, &shared.join("logs").join("x")) {
            orbiters_booster::log::remove_logs(&shared.join("logs"));
        }
        // Everything else N4N0 keeps in the folder it shares with the Orbiters Unity tools (PRIV-6), by name.
        if let Some(_pins) = pinned_way(local, &shared.join("x")) {
            let stems: Vec<String> = record.files.iter().filter_map(|f| f.strip_suffix(".exe")).filter(|s| !s.contains(['/', '\\'])).map(str::to_string).collect();
            failed.extend(remove_own_data(&shared, &stems));
        } else if std::fs::symlink_metadata(&shared).is_ok() {
            failed.push(tr!("{path} is a link to another folder: nothing in it was removed", path = shared.display()));
        }
        // The shared Orbiters folders, once nothing else of Orbiters' is left in them (a link is left as it is).
        for root in [&roots.config, local] {
            if real_dir(&root.join("Orbiters")) {
                let _ = std::fs::remove_dir(root.join("Orbiters"));
            }
        }
    }
    failed
}

/// Everything the FPS booster changed comes back, now that the app that could do it has closed. A SteamVR value can't be
/// restored while SteamVR runs: the window asks for it to be closed and waits a while. Past that, or silently, the journal
/// and `RunOnce` stay, with a copy of this program outside the install folder to do it at the next sign-in. Never a reason
/// for the uninstall to fail, as with the smoothing driver.
fn finish_booster(progress: &Progress, silent: bool) {
    use orbiters_booster::{programs, system as booster};
    let restorer = booster::restorer_copy_path();
    let store = booster::store_for_user(&restorer);
    if !store.has_entries() {
        // Used and nothing owed: its empty journal and copy go, or the data folders could never be removed.
        store.remove_if_empty();
        booster::remove_restorer_copy();
        return;
    }
    progress.set(0.1, tr("Putting back what the FPS booster changed…"));
    let closed = || !programs::steamvr_running(&programs::scan());
    let until = std::time::Instant::now() + BOOSTER_PATIENCE;
    let settled = booster::settle_for_uninstall(&store, &mut programs::RealOs, orbiters_booster::vrsettings::settings_path_for_user(), &closed, silent, &mut || {
        progress.set(0.1, tr("Close SteamVR to finish putting your settings back"));
        std::thread::sleep(Duration::from_millis(500));
        std::time::Instant::now() < until && !progress.forced() && !progress.cancelled()
    });
    if !settled {
        // RunOnce pointing at the removed program would do nothing: this copy stays where the uninstall does not go.
        match std::env::current_exe().ok().and_then(|me| booster::leave_a_restorer(&me)) {
            Some(copy) => eprintln!("the FPS booster still owes a setting: {} restores it at the next sign-in", copy.display()), // i18n: not shown
            None => eprintln!("the FPS booster still owes a setting, and no restorer could be left"), // i18n: not shown
        }
    } else {
        store.remove_if_empty();
        booster::remove_restorer_copy();
    }
}

/// What N4N0 itself writes at the top of the `Orbiters` folder of `%LOCALAPPDATA%` it shares with the Orbiters Unity
/// tools: the VRChat session (its cookies), the statistics' install id and queue, the menu reports, its SteamVR files,
/// the VR tuning, and the Unity projects page's caches. The account the tools share, their avatar manifests, Mixer and
/// the booster's journal (`finish_booster` decides on it) are not on it.
const OWN_DATA: &[(&str, Kind)] = &[
    ("vrchat-session.bin", Kind::File),
    // Its last good copy (r20), sealed the same way: a removal with data leaves no VRChat cookie behind.
    ("vrchat-session.bin.prev", Kind::File),
    ("usage", Kind::Folder),
    ("menu-reports", Kind::Folder),
    ("SteamVR", Kind::Folder),
    ("orbit-vr-tune.json", Kind::File),
    ("unity-projects.json", Kind::File),
    // The projects page's package repositories: what it took from Creator Companion's settings, the tokens of private
    // repositories among it, their listings and the archives of packages.
    ("vpm", Kind::Folder),
    ("project-sizes.json", Kind::File),
    ("hidden-projects.json", Kind::File),
    ("unity-cache-folders.json", Kind::File),
    // What a disk scan keeps for the next one, the records of copies and of projects being made, and the Unity 6 trials'
    // (the copies' and trials' own folders are the person's, and stay).
    ("unity-scenes.json", Kind::File),
    ("unity-folders.bin", Kind::File),
    ("project-copies", Kind::Folder),
    ("project-stage", Kind::Folder),
    ("unity6-trials.journal", Kind::File),
    ("unity6-trials.lock", Kind::File),
    ("unity6-leases", Kind::Folder),
    ("unity6-logs", Kind::Folder),
    ("unity6-data.json", Kind::File),
    ("unity6-data.high.json", Kind::File),
    ("unity6-data.lock", Kind::File),
    ("unity6-vcc.json", Kind::File),
    ("unity-editors-located.json", Kind::File),
    ("unity-install", Kind::Folder),
    ("unity-templates", Kind::Folder),
    ("package-icons", Kind::Folder),
    ("project-pictures", Kind::Folder),
    ("avatar-thumbnails", Kind::Folder),
    ("world-thumbnails", Kind::Folder),
    ("explorer-worlds.json", Kind::File),
    ("camera-panel-fit.json", Kind::File),
    // The VRChat groups page: what was uploaded, drafted and kept for each group.
    ("group-uploads", Kind::Folder),
    ("group-ledger", Kind::Folder),
    ("group-drafts", Kind::Folder),
    ("group-banners", Kind::Folder),
];

/// Lists of N4N0's that are kept with their copy before the last change, their lock, and what was put aside damaged
/// (`orbiters_projects::own`), or that a write was cut of: `<name>.json` and those beside it.
const OWN_LISTS: &[&str] = &["unity-gallery", "unity-gallery-shared", "unity-projects", "project-sizes", "hidden-projects", "unity-cache-folders"];

/// Whether `name` is one of `list`'s files.
fn of_list(list: &str, name: &str) -> bool {
    name.strip_prefix(list).and_then(|rest| rest.strip_prefix('.')).is_some_and(|rest| {
        let numbered = |text: &str, from: &str, to: &str| text.strip_prefix(from).and_then(|r| r.strip_suffix(to)).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'));
        matches!(rest, "json" | "previous.json" | "lock") || numbered(rest, "damaged-", ".json") || numbered(rest, "json.", ".tmp") || numbered(rest, "previous.json.", ".tmp")
    })
}

/// What an entry of N4N0's is: one of another kind under its name is not N4N0's, and stays.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    File,
    Folder,
}

/// N4N0's picture folders in `logs`; everything else of [`OWN_LOGS`] is a file.
const OWN_LOG_FOLDERS: &[&str] = &["camera-frames", "vr-pictures", "vr-test-frames"];

/// What N4N0 writes in the shared `logs` folder: these names, `perf-report-*.json`, and its programs' panic logs
/// (`<program>.log`, `<program>.old.log`). The booster's logs are removed by `orbiters_booster::log::remove_logs`.
const OWN_LOGS: &[&str] = &[
    "perf.jsonl",
    "perf.jsonl.1",
    "perf-old-publishing.flag",
    "vr-test.log",
    "vr-test.old.log",
    "shares.log",
    "shares.old.log",
    "publishes.log",
    "publishes.old.log",
    "translate.log",
    "camera-tiles.log",
    "camera-events.jsonl",
    "camera-truth.jsonl",
    "camera-reader.json",
    "camera-reader.png",
    "overlay-stalls.log",
    "overlay-worlds.jsonl",
    "overlay-worlds.old.jsonl",
    "walk-boost.log",
    "walk-boost.previous.log",
    "n4n0-pointer.log",
    "n4n0-pointer.old.log",
    "n4n0-pictures.log",
    "n4n0-pictures.old.log",
    "qr-test-sender.txt",
    "fps-test-report.txt",
    "mismatch-press.txt",
    // Folders of pictures only N4N0 writes.
    "camera-frames",
    "vr-pictures",
    "vr-test-frames",
];

/// Removes N4N0's own entries ([`OWN_DATA`], [`OWN_LOGS`], the panic logs of `programs`, the file stems of the
/// install's programs) from `root` (the shared `%LOCALAPPDATA%\Orbiters`), each by name: never the shared folder itself,
/// never anything through a link (a link is removed as a link). The paths that could not be removed.
fn remove_own_data(root: &Path, programs: &[String]) -> Vec<String> {
    // Every folder on the way must be a real one (a redirected shared folder, or `logs` on another disk, would lead the
    // names below to another tool's files): checked before anything in it is looked at.
    if !real_dir(root) {
        return if std::fs::symlink_metadata(root).is_ok() { vec![tr!("{path} is a link to another folder: nothing in it was removed", path = root.display())] } else { vec![] };
    }
    let logs = root.join("logs");
    let mut targets: Vec<(PathBuf, Kind)> = OWN_DATA.iter().map(|(n, k)| (root.join(n), *k)).collect();
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if OWN_LISTS.iter().any(|list| of_list(list, &name)) && !targets.iter().any(|(p, _)| *p == e.path()) {
            targets.push((e.path(), Kind::File));
        }
    }
    let mut failed = Vec::new();
    if real_dir(&logs) {
        targets.extend(OWN_LOGS.iter().map(|n| (logs.join(n), if OWN_LOG_FOLDERS.contains(n) { Kind::Folder } else { Kind::File })));
        for program in programs {
            targets.push((logs.join(format!("{program}.log")), Kind::File));
            targets.push((logs.join(format!("{program}.old.log")), Kind::File));
        }
        if let Ok(entries) = std::fs::read_dir(&logs) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with("perf-report-") && name.ends_with(".json") {
                    targets.push((e.path(), Kind::File));
                }
            }
        }
    } else if std::fs::symlink_metadata(&logs).is_ok() {
        failed.push(tr!("{path} is a link to another folder: nothing in it was removed", path = logs.display()));
    }
    for (path, kind) in targets {
        match remove_owned(&path, kind) {
            Ok(()) => {}
            Err(e) => failed.push(format!("{} ({e})", path.display())),
        }
    }
    // The logs folder once nothing is left in it (another tool's logs keep it); a link there stays a link.
    if real_dir(&logs) {
        let _ = std::fs::remove_dir(&logs);
    }
    failed
}

/// `path` removed: a link (or junction) as a link, never what it leads to; a folder with all in it, links inside removed
/// as links (`remove_dir_all` does not follow them); nothing there is fine.
fn remove_owned(path: &Path, kind: Kind) -> std::io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let link = meta.file_type().is_symlink() || meta.file_attributes() & 0x400 != 0;
    // A junction (or a link to a folder) is a folder to Windows: removed with `remove_dir`, which takes the link only.
    let folder = meta.is_dir() || meta.file_attributes() & 0x10 != 0;
    // Another kind of entry under N4N0's name is not N4N0's (a link is removed as a link whatever it leads to).
    if !link && folder != (kind == Kind::Folder) {
        return Err(std::io::Error::other(tr("not what N4N0 keeps under this name: left as it is")));
    }
    retrying(|| match (link, folder) {
        (true, true) => std::fs::remove_dir(path),
        (true, false) | (false, false) => std::fs::remove_file(path),
        (false, true) => std::fs::remove_dir_all(path),
    })
}


/// Whether two paths name the same folder (as Windows compares them).
pub fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    norm(a) == norm(b)
}

/// UTC now, RFC 3339.
fn now() -> String {
    time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default()
}

pub(crate) fn make_parent(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(parent) => std::fs::create_dir_all(parent),
        None => Ok(()),
    }
}

/// Retries a moment's lock by another program (an antivirus scanning a new file, a backup).
fn retrying<T>(mut op: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut wait = Duration::from_millis(20);
    for _ in 0..7 {
        match op() {
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(32) => {
                std::thread::sleep(wait);
                wait *= 2;
            }
            other => return other,
        }
    }
    op()
}

pub(crate) fn rename(from: &Path, to: &Path) -> std::io::Result<()> {
    before_move();
    retrying(|| std::fs::rename(from, to))
}

/// `path` as Windows takes it past its 260 characters, NUL-terminated: whole (`std::path::absolute`, which also turns `/`
/// into `\` and resolves `..`) and with the `\\?\` prefix that std's own file calls add to a long path (V2-1). A raw call
/// with the plain form fails in a deep install folder where every std call around it succeeds.
fn long_form(path: &Path) -> std::io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    let units = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
    let full: Vec<u16> = std::path::absolute(path)?.as_os_str().encode_wide().collect();
    let mut wide = if full.starts_with(&units(r"\\?\")) || full.starts_with(&units(r"\\.\")) {
        full
    } else if full.starts_with(&units(r"\\")) {
        [units(r"\\?\UNC\"), full[2..].to_vec()].concat()
    } else {
        [units(r"\\?\"), full].concat()
    };
    wide.push(0);
    Ok(wide)
}

/// Publish a transaction marker, waiting for the move to reach disk.
pub(crate) fn rename_marker(from: &Path, to: &Path, replace: bool) -> std::io::Result<()> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};
    let (from, to) = (long_form(from)?, long_form(to)?);
    let mut flags = MOVEFILE_WRITE_THROUGH;
    if replace {
        flags |= MOVEFILE_REPLACE_EXISTING;
    }
    before_move();
    retrying(|| {
        // SAFETY: both buffers are NUL-terminated and live through the call.
        unsafe { MoveFileExW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), flags) }
            .map_err(|e| std::io::Error::from_raw_os_error(e.code().0 & 0xffff))
    })
}

fn before_move() {
    #[cfg(test)]
    CUT.with(|cut| match cut.get() {
        Some(0) => std::panic::panic_any(Cut),
        left => cut.set(left.map(|n| n - 1)),
    });
}

pub(crate) fn remove_file(path: &Path) -> std::io::Result<()> {
    retrying(|| std::fs::remove_file(path))
}

/// Removes a folder and everything in it, quietly (a leftover is removed by the next install).
pub(crate) fn remove_tree(path: &Path) {
    let _ = retrying(|| match std::fs::remove_dir_all(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    });
}

#[cfg(test)]
thread_local!(static CUT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) });

#[cfg(test)]
struct Cut;

/// Runs `f` until its `moves`th move of a file, where it stops as a process that is killed does: nothing after that move is run,
/// nothing is put back. `None` when it was cut.
#[cfg(test)]
pub(crate) fn cut_after<T>(moves: Option<usize>, f: impl FnOnce() -> T) -> Option<T> {
    CUT.with(|cut| cut.set(moves));
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !info.payload().is::<Cut>() {
            hook(info)
        }
    }));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    let _ = std::panic::take_hook();
    CUT.with(|cut| cut.set(None));
    outcome.map_err(|e| if e.is::<Cut>() { } else { std::panic::resume_unwind(e) }).ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::swap::swap;
    use orbiters_update::{APP_EXE, STABLE};

    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-setup-{name}-{}", std::process::id()));
        remove_tree(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub(crate) fn index(files: &[&str]) -> payload::Index {
        payload::Index {
            product: PRODUCT.into(),
            version: "1.0.0".into(),
            files: files.iter().map(|p| payload::Entry { path: p.to_string(), size: 0, sha256: String::new() }).collect(),
        }
    }

    pub(crate) fn record(files: &[&str]) -> InstallRecord {
        InstallRecord {
            product: PRODUCT.into(),
            version: "0.9.0".into(),
            channel: STABLE.into(),
            files: files.iter().map(|f| f.to_string()).collect(),
            desktop_shortcut: false,
            start_with_windows: false,
            start_with_steamvr: false,
            installed: String::new(),
        }
    }

    /// Windows' side of an install, left out: nothing outside the folder.
    pub(crate) struct Quiet;
    impl Surroundings for Quiet {
        fn integrate(&self, _: &Path, _: &InstallRecord, _: Option<&InstallRecord>, _: u64, _: bool) -> Vec<String> {
            vec![]
        }
        fn swapping(&self, _: &Path, _: bool) {}
    }

    /// The payload of one program, `APP_EXE`, holding `text`, at `version`.
    pub(crate) fn one_program(version: &str, text: &str) -> Vec<u8> {
        static MADE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let source = scratch(&format!("one-program-{}", MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::write(source.join(APP_EXE), text).unwrap();
        let packed = source.join("payload.bin");
        payload::pack(version, &[(APP_EXE.to_string(), source.join(APP_EXE))], &packed).unwrap();
        let bytes = std::fs::read(&packed).unwrap();
        remove_tree(&source);
        bytes
    }

    pub(crate) fn options(dir: &Path) -> Options {
        Options { dir: dir.to_path_buf(), desktop_shortcut: false, start_with_windows: false, start_with_steamvr: false, channel: STABLE.into() }
    }

    /// V2-1: an install folder deep enough that its markers' paths pass Windows' 260 characters installs and updates, as every
    /// other file move of the setup (std's) does there: both names of each marker's move, the journal's (which replaces
    /// nothing) and the record's (which replaces the one there).
    #[test]
    fn a_deep_install_folder_installs_and_updates() {
        let base = scratch("deep-folder");
        let mut dir = base.clone();
        while dir.as_os_str().len() < 250 {
            dir.push("Project Archive");
        }
        assert!(dir.join(crate::swap::JOURNAL).as_os_str().len() > 260 && dir.join(STAGING).join(crate::swap::JOURNAL).as_os_str().len() > 260);
        for (version, text, fresh) in [("1.0.0", "first", true), ("2.0.0", "second", false)] {
            install_with(&one_program(version, text), &options(&dir), &Progress::default(), true, fresh, &Quiet).unwrap_or_else(|e| panic!("{version}: {e:#}"));
            assert_eq!(InstallRecord::load(&dir).unwrap().unwrap().version, version);
            assert_eq!(std::fs::read_to_string(dir.join(APP_EXE)).unwrap(), text);
            assert!(!dir.join(crate::swap::JOURNAL).exists() && crate::swap::whole(&dir));
        }
        remove_tree(&base);
    }

    #[test]
    fn a_swap_replaces_the_install_and_leaves_other_files() {
        let dir = scratch("swap");
        let (staging, backup) = (dir.join(STAGING), dir.join(BACKUP));
        std::fs::create_dir_all(staging.join("sub")).unwrap();
        std::fs::write(staging.join(APP_EXE), b"new").unwrap();
        std::fs::write(staging.join("sub/b.dll"), b"new b").unwrap();
        std::fs::write(dir.join(APP_EXE), b"old").unwrap();
        std::fs::write(dir.join("gone.dll"), b"old gone").unwrap();
        std::fs::write(dir.join("mine.txt"), b"the person's").unwrap();

        swap(&dir, &staging, &backup, &index(&[APP_EXE, "sub/b.dll"]), Some(&record(&[APP_EXE, "gone.dll"])), &record(&[APP_EXE, "sub/b.dll"])).unwrap();
        assert_eq!(std::fs::read(dir.join(APP_EXE)).unwrap(), b"new");
        assert_eq!(std::fs::read(dir.join("sub/b.dll")).unwrap(), b"new b");
        assert!(!dir.join("gone.dll").exists(), "a file of the last install that this one has not");
        assert_eq!(std::fs::read(dir.join("mine.txt")).unwrap(), b"the person's", "not the setup's");
        assert_eq!(std::fs::read(backup.join(APP_EXE)).unwrap(), b"old");
        remove_tree(&dir);
    }

    #[test]
    fn a_failed_swap_puts_the_old_files_back() {
        let dir = scratch("rollback");
        let (staging, backup) = (dir.join(STAGING), dir.join(BACKUP));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join(APP_EXE), b"new").unwrap();
        // "b.dll" is in the index but was never unpacked: its move fails after the first was made.
        std::fs::write(dir.join(APP_EXE), b"old").unwrap();
        std::fs::write(dir.join("b.dll"), b"old b").unwrap();

        let e = swap(&dir, &staging, &backup, &index(&[APP_EXE, "b.dll"]), Some(&record(&[APP_EXE, "b.dll"])), &record(&[APP_EXE, "b.dll"])).unwrap_err();
        assert!(format!("{e:#}").contains("previous version was put back"), "{e:#}");
        assert_eq!(std::fs::read(dir.join(APP_EXE)).unwrap(), b"old");
        assert_eq!(std::fs::read(dir.join("b.dll")).unwrap(), b"old b");
        remove_tree(&dir);
    }

    /// PROJECTS-34: a file of the person's where the payload puts one of its own is never put aside and removed; the
    /// app's own files (in the record, or the payload's very bytes) are not in the way.
    #[test]
    fn a_file_of_the_persons_in_the_payloads_place_stops_the_install_before_any_change() {
        use sha2::{Digest, Sha256};
        let dir = scratch("collide");
        std::fs::write(dir.join("readme.txt"), b"the person's").unwrap();
        std::fs::write(dir.join("same.dll"), b"payload bytes").unwrap();
        std::fs::write(dir.join(APP_EXE), b"old app").unwrap();
        let mut index = index(&[APP_EXE, "readme.txt", "same.dll", "new.dll"]);
        index.files[2].sha256 = format!("{:x}", Sha256::digest(b"payload bytes"));
        assert_eq!(collisions(&dir, &index, Some(&record(&[APP_EXE]))), ["readme.txt"]);
        assert_eq!(collisions(&dir, &index, None), [APP_EXE, "readme.txt"], "a first install owns nothing there");
        remove_tree(&dir);
    }

    fn junction(link: &Path, target: &Path) {
        let ok = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(link).arg(target).output().unwrap().status.success();
        assert!(ok, "mklink /J");
    }

    /// PROJECTS-42: a folder of the install that is a junction to another folder: an update moves nothing there and an
    /// uninstall removes nothing there.
    #[test]
    fn files_reached_through_a_junction_are_never_moved_or_removed() {
        let dir = scratch("junction");
        let elsewhere = scratch("junction-target");
        std::fs::write(elsewhere.join("b.dll"), b"someone else's").unwrap();
        junction(&dir.join("sub"), &elsewhere);
        assert!(contained(&dir, "sub/b.dll").is_none());
        assert!(contained(&dir, APP_EXE).is_some());
        assert_eq!(redirected(&dir, ["sub/b.dll", APP_EXE].into_iter()), ["sub/b.dll"]);
        let (staging, backup) = (dir.join(STAGING), dir.join(BACKUP));
        std::fs::create_dir_all(staging.join("sub")).unwrap();
        std::fs::write(staging.join("sub/b.dll"), b"new").unwrap();
        assert!(swap(&dir, &staging, &backup, &index(&["sub/b.dll"]), Some(&record(&["sub/b.dll"])), &record(&["sub/b.dll"])).is_err());
        assert_eq!(std::fs::read(elsewhere.join("b.dll")).unwrap(), b"someone else's");
        // (`uninstall` itself is not run here: it changes this computer's shortcuts, SteamVR and driver registrations.
        // Its loop removes only what `contained` gives.)
        let _ = std::fs::remove_dir(dir.join("sub"));
        remove_tree(&dir);
        remove_tree(&elsewhere);
    }

    /// F15: a cut install's backup is the only copy of what it moved aside: those files go back, and the rest is kept
    /// under a name of its own, never removed.
    #[test]
    fn a_cut_install_leaves_nothing_lost() {
        let dir = scratch("leftovers");
        let (staging, backup) = (dir.join(STAGING), dir.join(BACKUP));
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(backup.join(APP_EXE), b"old app").unwrap();
        std::fs::write(backup.join("other.txt"), b"unknown").unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join(APP_EXE), b"half").unwrap();
        recover_leftovers(&dir, &staging, &backup, Some(&record(&[APP_EXE]))).unwrap();
        assert_eq!(std::fs::read(dir.join(APP_EXE)).unwrap(), b"old app");
        assert!(!backup.exists() && !staging.exists(), "both set aside");
        let kept = |prefix: &str| std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(prefix)).map(|e| e.path()).collect::<Vec<_>>();
        assert_eq!(std::fs::read(kept(".setup-old-")[0].join("other.txt")).unwrap(), b"unknown");
        // A staging folder is kept too: payload names prove nothing about the bytes (verifier, point 2).
        assert_eq!(std::fs::read(kept(".setup-new-")[0].join(APP_EXE)).unwrap(), b"half");
        remove_tree(&dir);
    }

    /// A backup folder that is a junction to another folder: nothing is moved out of it, and it is set aside, not
    /// emptied.
    #[test]
    fn a_backup_folder_that_is_a_junction_gives_nothing_away() {
        let dir = scratch("backup-junction");
        let elsewhere = scratch("backup-junction-target");
        std::fs::write(elsewhere.join(APP_EXE), b"someone else's").unwrap();
        junction(&dir.join(BACKUP), &elsewhere);
        assert!(contained(&dir.join(BACKUP), APP_EXE).is_none(), "the base itself is checked");
        recover_leftovers(&dir, &dir.join(STAGING), &dir.join(BACKUP), Some(&record(&[APP_EXE]))).unwrap();
        assert!(!dir.join(APP_EXE).exists(), "nothing moved out of the junction");
        assert_eq!(std::fs::read(elsewhere.join(APP_EXE)).unwrap(), b"someone else's");
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let _ = std::fs::remove_dir(e.path());
        }
        remove_tree(&dir);
        remove_tree(&elsewhere);
    }

    /// The unchecked uninstall's scratch folders: only their empty folders go, a file in them stays.
    #[test]
    fn scratch_folders_lose_only_their_empty_folders() {
        let dir = scratch("scratch");
        std::fs::create_dir_all(dir.join(BACKUP).join("sub/empty")).unwrap();
        std::fs::write(dir.join(BACKUP).join("sub/only-copy.dll"), b"x").unwrap();
        std::fs::create_dir_all(dir.join(STAGING).join("a/b")).unwrap();
        remove_empty_dirs(&dir.join(BACKUP));
        remove_empty_dirs(&dir.join(STAGING));
        assert!(dir.join(BACKUP).join("sub/only-copy.dll").is_file());
        assert!(!dir.join(BACKUP).join("sub/empty").exists() && !dir.join(STAGING).exists());
        remove_tree(&dir);
    }

    /// A first install into a folder holding a file named like the install record refuses before any change.
    #[test]
    fn a_record_name_held_by_the_persons_file_stops_a_first_install() {
        let dir = scratch("record-name");
        std::fs::write(dir.join(RECORD_FILE), b"the person's").unwrap();
        let options = Options { dir: dir.clone(), desktop_shortcut: false, start_with_windows: false, start_with_steamvr: false, channel: STABLE.into() };
        let packed = scratch("record-name-payload").join("payload.bin");
        payload::pack("1.0.0", &[], &packed).unwrap();
        let bytes = std::fs::read(&packed).unwrap();
        let said = format!("{:#}", install(&bytes, &options, &Progress::default(), true, true).unwrap_err());
        assert!(said.contains(RECORD_FILE), "{said}");
        assert_eq!(std::fs::read(dir.join(RECORD_FILE)).unwrap(), b"the person's");
        remove_tree(&dir);
    }

    /// W1-3: a first install cut before its record is in place (at its journal's publication, at a file's move, during the
    /// record's own write) is installed by the next run of the setup in that folder, never refused as files of the person's;
    /// a journal's name held by a file of the person's still stops a first install.
    #[test]
    fn a_cut_first_install_installs_on_the_next_run() {
        let source = scratch("first-cut-source");
        std::fs::write(source.join(APP_EXE), b"app").unwrap();
        let packed = source.join("payload.bin");
        payload::pack("1.0.0", &[(APP_EXE.to_string(), source.join(APP_EXE))], &packed).unwrap();
        let bytes = std::fs::read(&packed).unwrap();
        let tmp = format!("{RECORD_FILE}.tmp");
        for cut in [0, 2] {
            let dir = scratch("first-cut");
            let options = Options { dir: dir.clone(), desktop_shortcut: false, start_with_windows: false, start_with_steamvr: false, channel: STABLE.into() };
            let run = || install_with(&bytes, &options, &Progress::default(), true, true, &Quiet);
            assert!(cut_after(Some(cut), run).is_none(), "cut at move {cut}");
            let draft = if cut == 0 { crate::swap::JOURNAL } else { &tmp };
            assert!(dir.join(STAGING).join(draft).is_file());
            assert_eq!(dir.join(crate::swap::JOURNAL).is_file(), cut != 0);
            assert!(!dir.join(&tmp).exists() && !dir.join(RECORD_FILE).exists());
            // Model a kill during the write as well as one after syncing the draft.
            std::fs::write(dir.join(STAGING).join(draft), b"{").unwrap();
            run().unwrap_or_else(|e| panic!("cut at move {cut}: {e:#}"));
            assert_eq!(InstallRecord::read(&dir).unwrap().version, "1.0.0");
            assert_eq!(std::fs::read(dir.join(APP_EXE)).unwrap(), b"app");
            assert!(!dir.join(crate::swap::JOURNAL).exists() && !dir.join(&tmp).exists());
            remove_tree(&dir);
        }
        // What is at the journal's name and is not a file this app writes (a folder) still stops a first install.
        let dir = scratch("first-persons-journal");
        std::fs::create_dir(dir.join(crate::swap::JOURNAL)).unwrap();
        let said = format!("{:#}", install_with(&bytes, &options(&dir), &Progress::default(), true, true, &Quiet).unwrap_err());
        assert!(said.contains(crate::swap::JOURNAL) && !dir.join(APP_EXE).exists() && dir.join(crate::swap::JOURNAL).is_dir(), "{said}");
        remove_tree(&dir);
        remove_tree(&source);
    }

    /// The files kept beside under `name`'s own name in `dir` (`<name>-<time>-<n>`), and what each holds.
    fn kept_beside(dir: &Path, name: &str) -> Vec<Vec<u8>> {
        let prefix = format!("{name}-");
        std::fs::read_dir(dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&prefix)).map(|e| std::fs::read(e.path()).unwrap()).collect()
    }

    /// V2-2: an older setup wrote its journal and its record's draft in place, at their names in the install folder; cut while it
    /// did, it left one empty or half written. That marker is this app's: the next update and the next first install keep it
    /// beside, as it is, and go on (they refused the folder every time, for good). Nothing of the person's is taken for one.
    #[test]
    fn an_older_setups_half_written_marker_is_kept_beside_and_the_setup_goes_on() {
        let dir = scratch("old-marker-update");
        install_with(&one_program("1.0.0", "first"), &options(&dir), &Progress::default(), true, true, &Quiet).unwrap();
        std::fs::write(dir.join("mine.txt"), "the person's").unwrap();
        for (version, left) in [("2.0.0", &b""[..]), ("3.0.0", br#"{"product":"orbiters-desktop","aside":["#)] {
            std::fs::write(dir.join(crate::swap::JOURNAL), left).unwrap();
            install_with(&one_program(version, version), &options(&dir), &Progress::default(), true, false, &Quiet).unwrap_or_else(|e| panic!("{version}: {e:#}"));
            assert_eq!(InstallRecord::load(&dir).unwrap().unwrap().version, version);
            assert_eq!(std::fs::read_to_string(dir.join(APP_EXE)).unwrap(), version);
            assert!(!dir.join(crate::swap::JOURNAL).exists() && crate::swap::whole(&dir));
            assert!(kept_beside(&dir, crate::swap::JOURNAL).iter().any(|kept| kept == left), "kept as it was");
        }
        assert_eq!(std::fs::read_to_string(dir.join("mine.txt")).unwrap(), "the person's");
        remove_tree(&dir);
        for marker in [crate::swap::JOURNAL, crate::swap::RECORD_DRAFT] {
            for left in [&b""[..], b"{"] {
                let dir = scratch("old-marker-first");
                std::fs::write(dir.join(marker), left).unwrap();
                install_with(&one_program("1.0.0", "first"), &options(&dir), &Progress::default(), true, true, &Quiet).unwrap_or_else(|e| panic!("{marker}: {e:#}"));
                assert_eq!(InstallRecord::load(&dir).unwrap().unwrap().version, "1.0.0");
                assert!(!dir.join(marker).exists() && kept_beside(&dir, marker) == vec![left.to_vec()], "{marker}");
                remove_tree(&dir);
            }
        }
    }

    /// PROJECTS-15: a program of someone else's running from the install folder (a folder in it) is neither waited for
    /// nor closed, and an automatic update closes nothing at all.
    #[test]
    fn only_this_installs_programs_are_waited_for_and_none_closed_silently() {
        let dir = scratch("programs");
        std::fs::create_dir_all(dir.join("tools")).unwrap();
        let ping = dir.join("tools/ping.exe");
        std::fs::copy(r"C:\Windows\System32\PING.EXE", &ping).unwrap();
        let mut other = std::process::Command::new(&ping).args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        let programs = own_programs(&dir, [APP_EXE, "orbiters-capture.exe", "data.bin"].into_iter());
        assert_eq!(programs, [dir.join("orbiters-capture.exe"), dir.join(APP_EXE)]);
        let started = std::time::Instant::now();
        close_app(&dir, &programs, &Progress::default(), true).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5), "not waited for");
        assert!(other.try_wait().unwrap().is_none(), "still running");
        // One of this install's own that will not quit: a silent update closes nothing, and says it.
        let own = dir.join("orbiters-capture.exe");
        std::fs::copy(&ping, &own).unwrap();
        let mut mine = std::process::Command::new(&own).args(["-n", "60", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        assert!(close_app(&dir, &programs, &Progress::default(), true).is_err());
        assert!(mine.try_wait().unwrap().is_none(), "not closed by an automatic update");
        let _ = mine.kill();
        let _ = other.kill();
        let _ = (mine.wait(), other.wait());
        remove_tree(&dir);
    }

    /// PRIV-6: the checked uninstall removes what N4N0 wrote in the shared Orbiters folder (the VRChat session, the
    /// statistics' id, its logs and caches) and nothing of the Unity tools', nor anything a link leads to.
    #[test]
    fn removing_the_data_takes_n4n0s_own_files_and_nothing_else() {
        let root = scratch("own-data");
        let elsewhere = scratch("own-data-elsewhere");
        std::fs::write(elsewhere.join("precious.txt"), b"not N4N0's").unwrap();
        let write = |rel: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        };
        for own in [
            "vrchat-session.bin",
            "vrchat-session.bin.prev",
            "usage/install-id",
            "usage/waiting.jsonl",
            "menu-reports/2026-11-19-a/report.json",
            "orbit-vr-tune.json",
            "unity-projects.json",
            "project-pictures/a.jpg",
            "avatar-thumbnails/direct/avtr_a.image",
            "world-thumbnails/wrld_a.image",
            "package-icons/0123.png",
            "vpm/settings.json",
            "vpm/their-settings.json",
            "vpm/their-settings.previous.json",
            "vpm/repositories.json",
            "vpm/Repos/vrc-official.json",
            "unity-gallery.json",
            "unity-gallery.previous.json",
            "unity-gallery.lock",
            "unity-gallery.damaged-0123abcd.json",
            "unity-gallery-shared.json",
            "unity-projects.json.4312-0.tmp",
            "unity-scenes.json",
            "unity-folders.bin",
            "project-copies/0123.json",
            "project-stage/0123.lock",
            "unity6-trials.journal",
            "unity6-logs/a/log.txt",
            "group-drafts/one/draft.json",
            "logs/perf.jsonl",
            "logs/shares.log",
            "logs/perf-report-20261119.json",
            "logs/orbiters-desktop.log",
            "logs/orbiters-desktop.old.log",
            "logs/vr-test-frames/1.png",
        ] {
            write(own);
        }
        for theirs in ["account.json", "Avatars/avtr_a.json", "Mixer/inbox.json", "booster/journal.json", "logs/toolkit.log", "VCC/settings.json", "unity-gallery.txt", "unity-gallery-of-mine.json"] {
            write(theirs);
        }
        // A link among N4N0's names: removed as a link, what it leads to stays.
        std::fs::remove_dir_all(root.join("project-pictures")).unwrap();
        junction(&root.join("project-pictures"), &elsewhere);
        // A folder where N4N0 keeps a file: not N4N0's, it stays and is named.
        std::fs::create_dir_all(root.join("hidden-projects.json/inside")).unwrap();
        let failed = remove_own_data(&root, &["orbiters-desktop".into()]);
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert!(failed[0].contains("hidden-projects.json") && root.join("hidden-projects.json/inside").is_dir());
        for gone in ["vrchat-session.bin", "vrchat-session.bin.prev", "usage", "menu-reports", "orbit-vr-tune.json", "unity-projects.json", "project-pictures", "avatar-thumbnails", "world-thumbnails", "package-icons", "vpm", "unity-gallery.json", "unity-gallery.previous.json", "unity-gallery.lock", "unity-gallery.damaged-0123abcd.json", "unity-gallery-shared.json", "unity-projects.json.4312-0.tmp", "unity-scenes.json", "unity-folders.bin", "project-copies", "project-stage", "unity6-trials.journal", "unity6-logs", "group-drafts", "logs/perf.jsonl", "logs/shares.log", "logs/perf-report-20261119.json", "logs/orbiters-desktop.log", "logs/orbiters-desktop.old.log", "logs/vr-test-frames"] {
            assert!(std::fs::symlink_metadata(root.join(gone)).is_err(), "{gone} left");
        }
        for kept in ["account.json", "Avatars/avtr_a.json", "Mixer/inbox.json", "booster/journal.json", "logs/toolkit.log", "VCC/settings.json", "unity-gallery.txt", "unity-gallery-of-mine.json"] {
            assert!(root.join(kept).is_file(), "{kept} removed");
        }
        assert_eq!(std::fs::read(elsewhere.join("precious.txt")).unwrap(), b"not N4N0's", "through a link");
        remove_tree(&root);
        remove_tree(&elsewhere);
    }

    /// PRJ-44: what the crates keep at the top of the shared folder is all on the list, or said here to be another's: a name
    /// added there and not here would survive "remove my data".
    #[test]
    fn every_name_n4n0_keeps_in_the_shared_folder_is_on_the_list() {
        const OTHERS: &[(&str, &str)] = &[
            ("Avatars", "the Toolkit's avatar manifests"),
            ("Desktop", "removed whole, by its own line"),
            ("Models", "removed whole, by its own line"),
            ("logs", "by name (OWN_LOGS)"),
            ("group-kills", "a test's"),
            ("u6-stand-ins", "the screenshot harness's, under the folders it gives itself"),
        ];
        fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    sources(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = Vec::new();
        for krate in ["orbiters-projects", "orbiters-core", "orbiters-desktop", "orbiters-n4n0"] {
            sources(&crates.join(krate).join("src"), &mut files);
        }
        let (mut named, mut missing) = (0, Vec::new());
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            for (call, roots) in [("data_dir().join(\"", "orbiters-projects"), ("own::dir(root).join(\"", "orbiters-projects"), ("local_data_root().join(\"", "")] {
                // (The other programs' folders have functions of their own: `hub_data_dir`.)
                for (at, _) in text.match_indices(call).filter(|(at, _)| !text[..*at].ends_with('_')).filter(|_| file.to_string_lossy().contains(roots)) {
                    let name = text[at + call.len()..].split(['"', '/']).next().unwrap();
                    named += 1;
                    let ours = OWN_DATA.iter().any(|(n, _)| *n == name) || OWN_LISTS.iter().any(|l| of_list(l, name)) || OTHERS.iter().any(|(n, _)| *n == name);
                    if !ours {
                        missing.push(format!("{name} ({})", file.display()));
                    }
                }
            }
        }
        assert!(named > 15, "the crates' sources are read: {named}");
        assert!(missing.is_empty(), "kept in the shared folder and on no list of what \"remove my data\" removes:\n{}", missing.join("\n"));
    }

    /// PRJ-41: the new version in place, what Windows would not take (a shortcut in a protected folder) is said and is no
    /// failure: the record is written, nothing is left of the old version, and the next sign-in has nothing to do.
    #[test]
    fn a_shortcut_that_cannot_be_written_is_not_a_failed_install() {
        struct Refusing(std::cell::RefCell<Vec<bool>>);
        impl Surroundings for Refusing {
            fn integrate(&self, dir: &Path, _: &InstallRecord, _: Option<&InstallRecord>, _: u64, _: bool) -> Vec<String> {
                assert!(!dir.join(BACKUP).exists() && !dir.join(STAGING).exists(), "the old version is gone by then");
                assert_eq!(InstallRecord::read(dir).unwrap().version, "1.0.0");
                vec!["The desktop shortcut cannot be written".into(), "The Start menu shortcut cannot be written".into()]
            }
            fn swapping(&self, _: &Path, under_way: bool) {
                self.0.borrow_mut().push(under_way);
            }
        }
        let dir = scratch("refused");
        std::fs::write(dir.join(APP_EXE), b"old").unwrap();
        std::fs::write(dir.join("gone.dll"), b"old").unwrap();
        record(&[APP_EXE, "gone.dll"]).write(&dir).unwrap();
        let build = scratch("refused-payload");
        std::fs::write(build.join("app"), b"new").unwrap();
        let packed = build.join("payload.bin");
        payload::pack("1.0.0", &[(APP_EXE.to_string(), build.join("app"))], &packed).unwrap();
        let options = Options { dir: dir.clone(), desktop_shortcut: true, start_with_windows: false, start_with_steamvr: false, channel: STABLE.into() };
        let (progress, windows) = (Progress::default(), Refusing(Default::default()));
        install_with(&std::fs::read(&packed).unwrap(), &options, &progress, true, false, &windows).unwrap();
        assert_eq!(progress.status().notes.len(), 2);
        assert_eq!(std::fs::read(dir.join(APP_EXE)).unwrap(), b"new");
        assert!(!dir.join("gone.dll").exists() && !dir.join(crate::swap::JOURNAL).exists());
        assert_eq!(*windows.0.borrow(), [true, false], "the next sign-in is told before the first move, and has nothing to do after the last");
        remove_tree(&dir);
        remove_tree(&build);
    }

    /// A `logs` folder redirected to another disk (or a shared folder that is a junction): nothing reached through it is
    /// removed, and it is said.
    #[test]
    fn nothing_is_removed_through_a_redirected_logs_folder() {
        let root = scratch("logs-redirect");
        let elsewhere = scratch("logs-redirect-target");
        for name in ["perf.jsonl", "perf-report-1.json", "orbiters-desktop.log", "theirs.log"] {
            std::fs::write(elsewhere.join(name), b"another disk").unwrap();
        }
        junction(&root.join("logs"), &elsewhere);
        let failed = remove_own_data(&root, &["orbiters-desktop".into()]);
        assert!(failed.iter().any(|f| f.contains("logs")), "{failed:?}");
        for name in ["perf.jsonl", "perf-report-1.json", "orbiters-desktop.log", "theirs.log"] {
            assert!(elsewhere.join(name).is_file(), "{name} removed through the junction");
        }
        let _ = std::fs::remove_dir(root.join("logs"));
        // The shared folder itself a junction.
        let shared = scratch("shared-redirect").join("Orbiters");
        junction(&shared, &elsewhere);
        assert_eq!(remove_own_data(&shared, &[]).len(), 1);
        assert!(elsewhere.join("perf.jsonl").is_file());
        let _ = std::fs::remove_dir(&shared);
        remove_tree(&root);
        remove_tree(&elsewhere);
    }

    /// Verifier round 2: the whole list of what uninstall removes, run on a temporary tree (Windows' own integration
    /// left out): N4N0's files and data go; a person's file named like setup's temporary record, an old scratch tree,
    /// another tool's entries, and everything reached through a redirected shared folder or install folder stay.
    #[test]
    fn uninstall_removes_only_what_is_n4n0s_through_real_folders() {
        let base = scratch("uninstall-list");
        let (dir, local, config, elsewhere) = (base.join("Install"), base.join("Local"), base.join("Roaming"), base.join("Elsewhere"));
        let write = |p: PathBuf| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        };
        for f in [APP_EXE, "sub/b.dll"] {
            write(dir.join(f));
        }
        record(&[APP_EXE, "sub/b.dll"]).write(&dir).unwrap();
        write(dir.join(format!("{RECORD_FILE}.tmp")));
        std::fs::create_dir_all(dir.join(BACKUP).join("mine/empty")).unwrap();
        for f in ["Orbiters/Desktop/settings.json"] {
            write(config.join(f));
        }
        // The shared local folder redirected to another disk: what is there is not removed.
        for f in ["Desktop/updates/setup.exe", "Desktop/cache/x", "Models/m.bin", "vrchat-session.bin", "Mixer/inbox.json"] {
            write(elsewhere.join(f));
        }
        std::fs::create_dir_all(&local).unwrap();
        junction(&local.join("Orbiters"), &elsewhere);
        let roots = Roots { local: local.clone(), config: config.clone() };
        let failed = remove_installation(&dir, &record(&[APP_EXE, "sub/b.dll"]), true, &roots, &mut |_, _| {});
        assert!(!dir.join(APP_EXE).exists() && !dir.join("sub").exists(), "the install's files go");
        assert!(dir.join(format!("{RECORD_FILE}.tmp")).is_file(), "a person's file of that name stays");
        assert!(dir.join(BACKUP).join("mine/empty").is_dir(), "an old scratch tree stays");
        assert!(!config.join("Orbiters/Desktop").exists(), "settings go");
        for f in ["Desktop/updates/setup.exe", "Desktop/cache/x", "Models/m.bin", "vrchat-session.bin", "Mixer/inbox.json"] {
            assert!(elsewhere.join(f).is_file(), "{f} removed through the redirected shared folder");
        }
        assert!(failed.iter().any(|f| f.contains("Orbiters")), "{failed:?}");
        assert!(std::fs::symlink_metadata(local.join("Orbiters")).is_ok(), "the link itself is left");
        // The install folder itself a junction: nothing in its target goes.
        let target = base.join("RealInstall");
        write(target.join(APP_EXE));
        let linked = base.join("LinkedInstall");
        junction(&linked, &target);
        let failed = remove_installation(&linked, &record(&[APP_EXE]), false, &roots, &mut |_, _| {});
        assert!(target.join(APP_EXE).is_file() && !failed.is_empty());
        let _ = std::fs::remove_dir(&linked);
        let _ = std::fs::remove_dir(local.join("Orbiters"));
        remove_tree(&base);
    }

    #[test]
    fn folders_compare_as_windows_does() {
        assert!(same_dir(Path::new("C:/Users/A/Programs/Orbiters - N4N0/"), Path::new(r"c:\users\a\programs\orbiters - n4n0")));
        assert!(!same_dir(Path::new("C:/Users/A/Programs/Orbiters"), Path::new("C:/Users/A/Programs/Orbiters - N4N0")));
    }

    /// The window shows an error as it came, and the setup speaks the language the app saved: the failure is worded when it happens.
    #[test]
    fn a_failed_uninstall_speaks_the_language_of_the_person() {
        use orbiters_i18n::{in_language, Language};
        let dir = scratch("not-installed");
        let said = |language| in_language(language, || format!("{:#}", uninstall(&dir, false, &Progress::default(), true).unwrap_err()));
        assert!(said(Language::En).starts_with("Orbiters - N4N0 is not installed in "));
        assert!(said(Language::Fr).starts_with("Orbiters - N4N0 n’est pas installé dans "));
        assert!(said(Language::Ja).contains("にインストールされていません"));
        assert!(said(Language::Es).contains("no está instalado en"));
        assert!(said(Language::PtBr).contains("não está instalado em"));
        remove_tree(&dir);
    }
}
