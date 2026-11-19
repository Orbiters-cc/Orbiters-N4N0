//! Installing, updating and uninstalling, without the window: the window ([`crate::ui`]) runs these on a thread and
//! shows their [`Progress`]; `--silent` runs them as they are.
//!
//! An install unpacks the payload into a folder of its own inside the install folder (`.setup-new`, checked file by
//! file), waits for nothing to run from the install folder any more, then moves each file in place, the old ones
//! aside (`.setup-old`). Moves within one folder cannot run out of space halfway, and if one fails anyway (a file held
//! by another program), the ones made are moved back and the install is as it was, with the error shown. Only the
//! files of the payload and of the last install's record are touched: the folder may be one the person chose, with
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
const STAGING: &str = ".setup-new";
/// Where the files an install replaces wait until it has finished, inside the install folder.
const BACKUP: &str = ".setup-old";
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
        if !silent {
            progress.stuck(true);
            while !system::wait_for_programs(programs, Duration::from_secs(1), stop) && !stop() {}
            progress.stuck(false);
            anyhow::ensure!(!progress.cancelled(), "{}", tr("Cancelled: Orbiters - N4N0 was still running."));
            system::terminate_programs(programs);
        }
    }
    anyhow::ensure!(system::processes_of(programs).is_empty(), "{}", orbiters_i18n::tr!("Orbiters - N4N0 is still running from {folder}.", folder = dir.display()));
    Ok(())
}

/// `relative` in `dir`, when nothing between them is a link or a junction (PROJECTS-42): a folder of the install
/// redirected elsewhere would make an update or uninstall move or remove files outside the install folder.
fn contained(dir: &Path, relative: &str) -> Option<PathBuf> {
    use std::os::windows::fs::MetadataExt;
    const REPARSE_POINT: u32 = 0x400;
    let path = inside(dir, relative)?;
    let mut at = dir.to_path_buf();
    for part in Path::new(relative).components() {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.file_type().is_symlink() || m.file_attributes() & REPARSE_POINT != 0 => return None,
            Ok(_) => {}
            Err(_) => break,
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
fn recover_leftovers(dir: &Path, staging: &Path, backup: &Path, index: &payload::Index, previous: Option<&InstallRecord>) {
    let stamp = time::OffsetDateTime::now_utc().unix_timestamp();
    if backup.is_dir() {
        for path in previous.map(|r| r.files.as_slice()).unwrap_or_default() {
            let (Some(dest), Some(from)) = (contained(dir, path), contained(backup, path)) else { continue };
            if !dest.exists() && from.is_file() && make_parent(&dest).is_ok() {
                let _ = rename(&from, &dest);
            }
        }
        if std::fs::read_dir(backup).is_ok_and(|mut d| d.next().is_some()) && !files_only_of(backup, &[]) {
            let _ = rename(backup, &dir.join(format!("{BACKUP}-{stamp}")));
        } else {
            remove_tree(backup);
        }
    }
    if staging.is_dir() {
        let payload: Vec<&str> = index.files.iter().map(|f| f.path.as_str()).collect();
        if files_only_of(staging, &payload) {
            remove_tree(staging);
        } else {
            let _ = rename(staging, &dir.join(format!("{STAGING}-{stamp}")));
        }
    }
}

/// Whether every file under `folder` is one of `paths` (relative, `/` between folders), with no link anywhere.
fn files_only_of(folder: &Path, paths: &[&str]) -> bool {
    fn walk(root: &Path, at: &Path, paths: &[&str]) -> bool {
        let Ok(entries) = std::fs::read_dir(at) else { return false };
        entries.flatten().all(|e| {
            let Ok(m) = std::fs::symlink_metadata(e.path()) else { return false };
            if m.file_type().is_symlink() {
                false
            } else if m.is_dir() {
                walk(root, &e.path(), paths)
            } else {
                let rel = e.path().strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
                paths.iter().any(|p| p.eq_ignore_ascii_case(&rel))
            }
        })
    }
    walk(folder, folder, paths)
}

/// Installs the payload with `options`. `fresh` is a choice the person just made (the setup's window); an update
/// keeps what they changed since (a desktop shortcut they deleted stays deleted).
pub fn install(bytes: &[u8], options: &Options, progress: &Progress, silent: bool, fresh: bool) -> anyhow::Result<()> {
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
    let previous = InstallRecord::read(dir);
    // Nothing is changed when a file of the payload would replace one of the person's, or would be reached through a
    // folder redirected elsewhere.
    let taken = collisions(dir, &index, previous.as_ref());
    anyhow::ensure!(
        taken.is_empty(),
        "{}",
        tr!("These files are already in {folder} and are not Orbiters - N4N0's: {files}. Nothing was changed: choose another folder.", folder = dir.display(), files = taken.join(", "))
    );
    let new_and_old = index.files.iter().map(|f| f.path.as_str()).chain(previous.iter().flat_map(|r| r.files.iter().map(String::as_str)));
    let linked = redirected(dir, new_and_old);
    anyhow::ensure!(
        linked.is_empty(),
        "{}",
        tr!("These files of {folder} are reached through a link to another folder: {files}. Nothing was changed.", folder = dir.display(), files = linked.join(", "))
    );
    // What a cut install left.
    recover_leftovers(dir, &staging, &backup, &index, previous.as_ref());

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
    swap(dir, &staging, &backup, &index, previous.as_ref())?;
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
    record.write(dir).with_context(|| orbiters_i18n::tr!("{path} cannot be written", path = dir.join(RECORD_FILE).display()))?;

    progress.set(0.9, tr("Adding it to Windows…"));
    integrate(dir, &record, previous.as_ref(), index.total(), fresh)?;
    remove_tree(&staging);
    remove_tree(&backup);
    progress.set(1.0, tr("Installed"));
    Ok(())
}

/// Moves the unpacked files of `index` from `staging` into `dir`, what they replace (and what the `previous` install
/// had that this one has not) into `backup`. A move that fails puts back the ones made.
fn swap(dir: &Path, staging: &Path, backup: &Path, index: &payload::Index, previous: Option<&InstallRecord>) -> anyhow::Result<()> {
    let new: Vec<&str> = index.files.iter().map(|f| f.path.as_str()).collect();
    let gone: Vec<&str> = previous.map(|r| r.files.iter().map(String::as_str).filter(|f| !new.contains(f)).collect()).unwrap_or_default();
    let mut aside: Vec<&str> = Vec::new();
    let mut placed: Vec<&str> = Vec::new();
    let result = (|| -> anyhow::Result<()> {
        for &path in new.iter().chain(&gone) {
            let (Some(dest), Some(to)) = (contained(dir, path), contained(backup, path)) else { continue };
            if dest.is_file() {
                make_parent(&to)?;
                rename(&dest, &to).with_context(|| orbiters_i18n::tr!("{path} is in use by another program", path = dest.display()))?;
                aside.push(path);
            }
        }
        for &path in &new {
            let (Some(from), Some(dest)) = (contained(staging, path), contained(dir, path)) else {
                anyhow::bail!("{}", tr!("{path} is reached through a link to another folder", path = path));
            };
            make_parent(&dest)?;
            rename(&from, &dest).with_context(|| orbiters_i18n::tr!("{path} cannot be written", path = dest.display()))?;
            placed.push(path);
        }
        Ok(())
    })();
    if let Err(e) = result {
        // As it was: the new files out, the old ones back. Each move checked (F16): what could not be put back is named,
        // and the backup folder then stays as it is.
        let mut stuck = Vec::new();
        for &path in placed.iter().rev() {
            if let (Some(dest), Some(from)) = (inside(dir, path), inside(staging, path)) {
                if rename(&dest, &from).is_err() {
                    stuck.push(path);
                }
            }
        }
        for &path in aside.iter().rev() {
            if let (Some(to), Some(dest)) = (inside(backup, path), inside(dir, path)) {
                if rename(&to, &dest).is_err() {
                    stuck.push(path);
                }
            }
        }
        if !stuck.is_empty() {
            return Err(e.context(tr!(
                "The previous version could not be fully put back: {files}. Its files are kept in {folder}.",
                files = stuck.join(", "),
                folder = backup.display()
            )));
        }
        remove_tree(staging);
        return Err(e.context(tr("The previous version was put back")));
    }
    Ok(())
}

/// Windows' side: the installed-apps entry, the Start menu (and desktop) shortcut, the start with Windows; and SteamVR's
/// start of the app, when SteamVR runs (else the app tells it, from the record, the next time it runs).
fn integrate(dir: &Path, record: &InstallRecord, previous: Option<&InstallRecord>, size: u64, fresh: bool) -> anyhow::Result<()> {
    system::register(dir, &record.version, size).with_context(|| tr("The installed-apps entry cannot be written"))?;
    if let Some(link) = system::start_menu_shortcut() {
        system::write_shortcut(&link, dir).with_context(|| tr("The Start menu shortcut cannot be written"))?;
    }
    if let Some(link) = system::desktop_shortcut() {
        if record.desktop_shortcut && (fresh || link.exists()) {
            system::write_shortcut(&link, dir).with_context(|| tr("The desktop shortcut cannot be written"))?;
        } else if !record.desktop_shortcut && previous.is_some_and(|p| p.desktop_shortcut) {
            let _ = std::fs::remove_file(&link);
        }
    }
    if fresh {
        if record.start_with_windows {
            system::set_start_with_windows(dir, true)?;
        } else if system::starts_with_windows_from(dir) {
            system::set_start_with_windows(dir, false)?;
        }
        // Never a reason for the install to fail: what SteamVR could not be told now, the app tells it the next time
        // SteamVR runs (its settings keep the choice).
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
    Ok(())
}

/// Updates the install in `dir` to the payload, keeping its options and channel.
pub fn update(bytes: &[u8], dir: &Path, progress: &Progress, silent: bool) -> anyhow::Result<InstallRecord> {
    let record = InstallRecord::read(dir).with_context(|| tr!("Orbiters - N4N0 is not installed in {folder}.", folder = dir.display()))?;
    let options = Options {
        dir: dir.to_path_buf(),
        desktop_shortcut: record.desktop_shortcut,
        start_with_windows: record.start_with_windows,
        start_with_steamvr: record.start_with_steamvr,
        channel: record.channel,
    };
    install(bytes, &options, progress, silent, false)?;
    InstallRecord::read(dir).with_context(|| tr("The install record is missing after the update."))
}

/// How long an uninstall waits for SteamVR to be closed, for a value still owed to it.
const BOOSTER_PATIENCE: Duration = Duration::from_secs(120);

/// Uninstalls the app from `dir`: what it added to Windows, then the files its record lists (nothing else), and the
/// folder once empty. `remove_data` also removes its settings, caches and speech models; the person's sessions are
/// never touched.
pub fn uninstall(dir: &Path, remove_data: bool, progress: &Progress, silent: bool) -> anyhow::Result<()> {
    let record = InstallRecord::read(dir).with_context(|| tr!("Orbiters - N4N0 is not installed in {folder}.", folder = dir.display()))?;
    progress.set(0.05, tr("Closing Orbiters - N4N0…"));
    close_app(dir, &own_programs(dir, record.files.iter().map(String::as_str)), progress, silent)?;

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
    let mut failed = Vec::new();
    let count = record.files.len().max(1);
    for (i, path) in record.files.iter().enumerate() {
        match contained(dir, path) {
            Some(file) => {
                if file.is_file() && remove_file(&file).is_err() {
                    failed.push(file.display().to_string());
                }
            }
            // Reached through a link to another folder: never removed from there (PROJECTS-42).
            None if inside(dir, path).is_some() => failed.push(dir.join(path).display().to_string()),
            None => {}
        }
        progress.set(0.35 + 0.5 * (i + 1) as f32 / count as f32, tr!("Removing {path}", path = path));
    }
    remove_tree(&dir.join(STAGING));
    remove_tree(&dir.join(BACKUP));
    let _ = std::fs::remove_file(dir.join(format!("{RECORD_FILE}.tmp")));
    if failed.is_empty() {
        let _ = remove_file(&dir.join(RECORD_FILE));
    }
    // The record's folders, deepest first, then the install folder: each only once empty.
    let mut folders: Vec<PathBuf> =
        record.files.iter().filter_map(|p| inside(dir, p)).filter_map(|f| f.parent().map(Path::to_path_buf)).filter(|f| f != dir).collect();
    folders.sort_by_key(|f| std::cmp::Reverse(f.components().count()));
    folders.dedup();
    for folder in folders {
        let _ = std::fs::remove_dir(folder);
    }
    let _ = std::fs::remove_dir(dir);

    progress.set(0.9, tr("Tidying up…"));
    remove_tree(&orbiters_update::updates_dir());
    if remove_data {
        for folder in data_dirs() {
            remove_tree(&folder);
        }
        // The FPS booster's reminder of what to change back by hand. Its journal is not touched here: `finish_booster` removed it if nothing is owed.
        let _ = std::fs::remove_file(orbiters_booster::data_dir().join("to-put-back.json"));
        let _ = std::fs::remove_dir(orbiters_booster::data_dir());
        // Its frame logs, written at every level and Off by default: large, and nobody else's.
        if let Some(local) = dirs::data_local_dir() {
            orbiters_booster::log::remove_logs(&local.join("Orbiters").join("logs"));
            // Everything else N4N0 keeps in the folder it shares with the Orbiters Unity tools (PRIV-6), by name.
            let stems: Vec<String> = record.files.iter().filter_map(|f| f.strip_suffix(".exe")).filter(|s| !s.contains(['/', '\\'])).map(str::to_string).collect();
            failed.extend(remove_own_data(&local.join("Orbiters"), &stems));
        }
        // The shared Orbiters folders, once nothing else of Orbiters' is left in them.
        for root in [dirs::config_dir(), dirs::data_local_dir()].into_iter().flatten() {
            let _ = std::fs::remove_dir(root.join("Orbiters"));
        }
    }
    anyhow::ensure!(failed.is_empty(), "{}", tr!("These files could not be removed (in use?): {files}", files = failed.join(", ")));
    progress.set(1.0, tr("Uninstalled"));
    Ok(())
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
const OWN_DATA: &[&str] = &[
    "vrchat-session.bin",
    "usage",
    "menu-reports",
    "SteamVR",
    "orbit-vr-tune.json",
    "unity-projects.json",
    "project-sizes.json",
    "hidden-projects.json",
    "unity-cache-folders.json",
    "unity-templates",
    "package-icons",
    "project-pictures",
    "avatar-thumbnails",
    "world-thumbnails",
];

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
    let logs = root.join("logs");
    let mut targets: Vec<PathBuf> = OWN_DATA.iter().map(|n| root.join(n)).collect();
    targets.extend(OWN_LOGS.iter().map(|n| logs.join(n)));
    for program in programs {
        targets.push(logs.join(format!("{program}.log")));
        targets.push(logs.join(format!("{program}.old.log")));
    }
    if let Ok(entries) = std::fs::read_dir(&logs) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("perf-report-") && name.ends_with(".json") {
                targets.push(e.path());
            }
        }
    }
    let mut failed = Vec::new();
    for path in targets {
        if let Err(e) = remove_owned(&path) {
            failed.push(format!("{} ({e})", path.display()));
        }
    }
    // The logs folder once nothing is left in it (another tool's logs keep it).
    let _ = std::fs::remove_dir(&logs);
    failed
}

/// `path` removed: a link (or junction) as a link, never what it leads to; a folder with all in it, links inside removed
/// as links (`remove_dir_all` does not follow them); nothing there is fine.
fn remove_owned(path: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let link = meta.file_type().is_symlink() || meta.file_attributes() & 0x400 != 0;
    // A junction (or a link to a folder) is a folder to Windows: removed with `remove_dir`, which takes the link only.
    let folder = meta.is_dir() || meta.file_attributes() & 0x10 != 0;
    retrying(|| match (link, folder) {
        (true, true) => std::fs::remove_dir(path),
        (true, false) | (false, false) => std::fs::remove_file(path),
        (false, true) => std::fs::remove_dir_all(path),
    })
}

/// What Orbiters - N4N0 keeps outside its install folder, its own only (`orbiters_core::paths`): settings
/// (`%APPDATA%\Orbiters\Desktop`), caches and downloaded updates (`%LOCALAPPDATA%\Orbiters\Desktop`), speech models
/// (`%LOCALAPPDATA%\Orbiters\Models`). What the Orbiters Unity tools share there (the account, Mixer's inbox, avatar
/// manifests, logs) stays, and so do the sessions.
pub fn data_dirs() -> Vec<PathBuf> {
    let mut dirs_found = Vec::new();
    if let Some(config) = dirs::config_dir() {
        dirs_found.push(config.join("Orbiters").join("Desktop"));
    }
    if let Some(local) = dirs::data_local_dir() {
        dirs_found.push(local.join("Orbiters").join("Desktop"));
        dirs_found.push(local.join("Orbiters").join("Models"));
    }
    dirs_found
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

fn make_parent(path: &Path) -> std::io::Result<()> {
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

fn rename(from: &Path, to: &Path) -> std::io::Result<()> {
    retrying(|| std::fs::rename(from, to))
}

fn remove_file(path: &Path) -> std::io::Result<()> {
    retrying(|| std::fs::remove_file(path))
}

/// Removes a folder and everything in it, quietly (a leftover is removed by the next install).
fn remove_tree(path: &Path) {
    let _ = retrying(|| match std::fs::remove_dir_all(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_update::{APP_EXE, STABLE};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-setup-{name}-{}", std::process::id()));
        remove_tree(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn index(files: &[&str]) -> payload::Index {
        payload::Index {
            product: PRODUCT.into(),
            version: "1.0.0".into(),
            files: files.iter().map(|p| payload::Entry { path: p.to_string(), size: 0, sha256: String::new() }).collect(),
        }
    }

    fn record(files: &[&str]) -> InstallRecord {
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

        swap(&dir, &staging, &backup, &index(&[APP_EXE, "sub/b.dll"]), Some(&record(&[APP_EXE, "gone.dll"]))).unwrap();
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

        let e = swap(&dir, &staging, &backup, &index(&[APP_EXE, "b.dll"]), Some(&record(&[APP_EXE, "b.dll"]))).unwrap_err();
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
        assert!(swap(&dir, &staging, &backup, &index(&["sub/b.dll"]), Some(&record(&["sub/b.dll"]))).is_err());
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
        recover_leftovers(&dir, &staging, &backup, &index(&[APP_EXE]), Some(&record(&[APP_EXE])));
        assert_eq!(std::fs::read(dir.join(APP_EXE)).unwrap(), b"old app");
        assert!(!backup.exists() && !staging.exists(), "the staging folder held payload files only");
        let kept: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(".setup-old-")).collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read(kept[0].path().join("other.txt")).unwrap(), b"unknown");
        remove_tree(&dir);
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
            "usage/install-id",
            "usage/waiting.jsonl",
            "menu-reports/2026-11-19-a/report.json",
            "orbit-vr-tune.json",
            "unity-projects.json",
            "project-pictures/a.jpg",
            "avatar-thumbnails/direct/avtr_a.image",
            "world-thumbnails/wrld_a.image",
            "package-icons/0123.png",
            "logs/perf.jsonl",
            "logs/shares.log",
            "logs/perf-report-20261119.json",
            "logs/orbiters-desktop.log",
            "logs/orbiters-desktop.old.log",
            "logs/vr-test-frames/1.png",
        ] {
            write(own);
        }
        for theirs in ["account.json", "Avatars/avtr_a.json", "Mixer/inbox.json", "booster/journal.json", "logs/toolkit.log", "VCC/settings.json"] {
            write(theirs);
        }
        // A link among N4N0's names: removed as a link, what it leads to stays.
        std::fs::remove_dir_all(root.join("project-pictures")).unwrap();
        junction(&root.join("project-pictures"), &elsewhere);
        let failed = remove_own_data(&root, &["orbiters-desktop".into()]);
        assert!(failed.is_empty(), "{failed:?}");
        for gone in ["vrchat-session.bin", "usage", "menu-reports", "orbit-vr-tune.json", "unity-projects.json", "project-pictures", "avatar-thumbnails", "world-thumbnails", "package-icons", "logs/perf.jsonl", "logs/shares.log", "logs/perf-report-20261119.json", "logs/orbiters-desktop.log", "logs/orbiters-desktop.old.log", "logs/vr-test-frames"] {
            assert!(std::fs::symlink_metadata(root.join(gone)).is_err(), "{gone} left");
        }
        for kept in ["account.json", "Avatars/avtr_a.json", "Mixer/inbox.json", "booster/journal.json", "logs/toolkit.log", "VCC/settings.json"] {
            assert!(root.join(kept).is_file(), "{kept} removed");
        }
        assert_eq!(std::fs::read(elsewhere.join("precious.txt")).unwrap(), b"not N4N0's", "through a link");
        remove_tree(&root);
        remove_tree(&elsewhere);
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
