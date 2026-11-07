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

/// Makes sure nothing runs from `dir` any more. The app is asked to quit as its own Quit does, when it runs from
/// there (an instance from elsewhere, a development build, is left alone; its files are not these). Past
/// [`PATIENCE`] the window offers to close what is left, which `silent` does by itself.
fn close_app(dir: &Path, progress: &Progress, silent: bool) -> anyhow::Result<()> {
    if system::processes_in(dir).is_empty() {
        return Ok(());
    }
    progress.label(tr("Closing Orbiters - N4N0…"));
    if system::app_runs_from(dir) {
        system::ask_app_to_quit();
    }
    let stop = || progress.forced() || progress.cancelled();
    if !system::wait_for_folder(dir, PATIENCE, stop) {
        if !silent {
            progress.stuck(true);
            while !system::wait_for_folder(dir, Duration::from_secs(1), stop) && !stop() {}
            progress.stuck(false);
        }
        anyhow::ensure!(!progress.cancelled(), "Cancelled: Orbiters - N4N0 was still running.");
        system::terminate_folder(dir);
    }
    anyhow::ensure!(system::processes_in(dir).is_empty(), "Orbiters - N4N0 is still running from {}.", dir.display());
    Ok(())
}

/// Installs the payload with `options`. `fresh` is a choice the person just made (the setup's window); an update
/// keeps what they changed since (a desktop shortcut they deleted stays deleted).
pub fn install(bytes: &[u8], options: &Options, progress: &Progress, silent: bool, fresh: bool) -> anyhow::Result<()> {
    let dir = &options.dir;
    let index = payload::read_index(bytes)?;
    if let Some(free) = system::free_space(dir) {
        anyhow::ensure!(
            free > index.total() + MARGIN,
            "Not enough free space on the drive of {}: {} MB are needed.",
            dir.display(),
            (index.total() + MARGIN) >> 20
        );
    }
    std::fs::create_dir_all(dir).with_context(|| format!("{} cannot be created", dir.display()))?;
    let staging = dir.join(STAGING);
    let backup = dir.join(BACKUP);
    // What a cut install left.
    remove_tree(&staging);
    remove_tree(&backup);

    progress.set(0.02, tr("Unpacking…"));
    let unpacked = payload::unpack(bytes, &staging, |done, total, file| {
        progress.set(0.02 + 0.78 * (done as f32 / total.max(1) as f32), tr!("Unpacking {file}", file = file));
    });
    if let Err(e) = unpacked {
        remove_tree(&staging);
        return Err(e);
    }
    if let Err(e) = close_app(dir, progress, silent) {
        remove_tree(&staging);
        return Err(e);
    }

    progress.set(0.82, tr("Putting the new version in place…"));
    let previous = InstallRecord::read(dir);
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
    record.write(dir).with_context(|| format!("{} cannot be written", dir.join(RECORD_FILE).display()))?;

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
            let (Some(dest), Some(to)) = (inside(dir, path), inside(backup, path)) else { continue };
            if dest.is_file() {
                make_parent(&to)?;
                rename(&dest, &to).with_context(|| format!("{} is in use by another program", dest.display()))?;
                aside.push(path);
            }
        }
        for &path in &new {
            let (Some(from), Some(dest)) = (inside(staging, path), inside(dir, path)) else { continue };
            make_parent(&dest)?;
            rename(&from, &dest).with_context(|| format!("{} cannot be written", dest.display()))?;
            placed.push(path);
        }
        Ok(())
    })();
    if let Err(e) = result {
        // As it was: the new files out, the old ones back.
        for &path in placed.iter().rev() {
            if let (Some(dest), Some(from)) = (inside(dir, path), inside(staging, path)) {
                let _ = rename(&dest, &from);
            }
        }
        for &path in aside.iter().rev() {
            if let (Some(to), Some(dest)) = (inside(backup, path), inside(dir, path)) {
                let _ = rename(&to, &dest);
            }
        }
        remove_tree(staging);
        return Err(e.context("The previous version was put back"));
    }
    Ok(())
}

/// Windows' side: the installed-apps entry, the Start menu (and desktop) shortcut, the start with Windows; and SteamVR's
/// start of the app, when SteamVR runs (else the app tells it, from the record, the next time it runs).
fn integrate(dir: &Path, record: &InstallRecord, previous: Option<&InstallRecord>, size: u64, fresh: bool) -> anyhow::Result<()> {
    system::register(dir, &record.version, size).context("The installed-apps entry cannot be written")?;
    if let Some(link) = system::start_menu_shortcut() {
        system::write_shortcut(&link, dir).context("The Start menu shortcut cannot be written")?;
    }
    if let Some(link) = system::desktop_shortcut() {
        if record.desktop_shortcut && (fresh || link.exists()) {
            system::write_shortcut(&link, dir).context("The desktop shortcut cannot be written")?;
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
    let record = InstallRecord::read(dir).with_context(|| format!("Orbiters - N4N0 is not installed in {}.", dir.display()))?;
    let options = Options {
        dir: dir.to_path_buf(),
        desktop_shortcut: record.desktop_shortcut,
        start_with_windows: record.start_with_windows,
        start_with_steamvr: record.start_with_steamvr,
        channel: record.channel,
    };
    install(bytes, &options, progress, silent, false)?;
    InstallRecord::read(dir).context("The install record is missing after the update.")
}

/// How long an uninstall waits for SteamVR to be closed, for a value still owed to it.
const BOOSTER_PATIENCE: Duration = Duration::from_secs(120);

/// Uninstalls the app from `dir`: what it added to Windows, then the files its record lists (nothing else), and the
/// folder once empty. `remove_data` also removes its settings, caches and speech models; the person's sessions are
/// never touched.
pub fn uninstall(dir: &Path, remove_data: bool, progress: &Progress, silent: bool) -> anyhow::Result<()> {
    let record = InstallRecord::read(dir).with_context(|| format!("Orbiters - N4N0 is not installed in {}.", dir.display()))?;
    progress.set(0.05, tr("Closing Orbiters - N4N0…"));
    close_app(dir, progress, silent)?;

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
        if let Some(file) = inside(dir, path) {
            if file.is_file() && remove_file(&file).is_err() {
                failed.push(file.display().to_string());
            }
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
        }
        // The shared Orbiters folders, once nothing else of Orbiters' is left in them.
        for root in [dirs::config_dir(), dirs::data_local_dir()].into_iter().flatten() {
            let _ = std::fs::remove_dir(root.join("Orbiters"));
        }
    }
    anyhow::ensure!(failed.is_empty(), "These files could not be removed (in use?): {}", failed.join(", "));
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

    #[test]
    fn folders_compare_as_windows_does() {
        assert!(same_dir(Path::new("C:/Users/A/Programs/Orbiters - N4N0/"), Path::new(r"c:\users\a\programs\orbiters - n4n0")));
        assert!(!same_dir(Path::new("C:/Users/A/Programs/Orbiters"), Path::new("C:/Users/A/Programs/Orbiters - N4N0")));
    }
}
