//! Updates of an installed app (`orbiters_update`). The tray service looks for a newer release of the install's channel
//! a few seconds after it starts and every half hour (every two minutes in the development environment, whose feed is
//! the development server on this computer); a newer one's setup is downloaded in the background and checked
//! against the size and SHA-256 its signed release gives, then waits for the person. "Restart to update" (the window:
//! Settings' About, the sidebar) has the service start that setup and quit: the setup replaces the files once nothing
//! runs from the install folder any more, then starts the app again. While a recording, a reconstruction, a call or an
//! upload runs ([`Snapshot::busy_with`](crate::state::Snapshot::busy_with)), the restart waits for it to end, unless
//! the person restarts at once. A setup that was ready when the app quit is installed when it starts next, before
//! anything runs ([`pending_setup`]), once: one that failed is left to the person's restart. A copy the setup did not
//! install (a development build) never updates itself.

use super::{Controller, NoticeKind, Shared};
use crate::ipc::UpdateAction;
use crate::state::UpdateState;
use orbiters_update::{InstallRecord, Release};
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The first look, a moment after the app starts: a newer version shows beside its name at once.
const FIRST_LOOK: Duration = Duration::from_secs(5);
/// Then every so often (a request of a few hundred bytes), and more often in the development environment, whose
/// builds come one after another while they are being tried.
const EVERY: Duration = Duration::from_secs(30 * 60);
const EVERY_DEVELOPMENT: Duration = Duration::from_secs(2 * 60);
/// A restart waiting for the work to end looks this often.
const IDLE_POLL: Duration = Duration::from_secs(5);
/// Beside a downloaded setup ([`Ready`]).
const READY_FILE: &str = "ready.json";

/// What restarts the app into a setup (the service: it starts the setup, then quits).
pub type Restarter = Box<dyn Fn(&Path, &Path) + Send + Sync>;

#[derive(Default)]
pub(super) struct UpdateRuntime {
    /// The install this app runs from; `None` for a copy the setup did not install.
    install: Option<PathBuf>,
    /// Wakes the updates thread to look now.
    wake: Option<mpsc::Sender<()>>,
    restarter: Option<Restarter>,
}

/// A downloaded setup, as noted beside it: which release it installs, and whether it was started once.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Ready {
    version: String,
    sha256: String,
    size: u64,
    #[serde(default)]
    notes: String,
    /// Started to install it: an app that starts and is still older than it finds it failed, and leaves it to the
    /// person.
    #[serde(default)]
    started: bool,
}

fn read_ready() -> Option<Ready> {
    serde_json::from_slice(&std::fs::read(orbiters_update::updates_dir().join(READY_FILE)).ok()?).ok()
}

fn write_ready(ready: &Ready) {
    let path = orbiters_update::updates_dir().join(READY_FILE);
    if let Err(e) = serde_json::to_vec_pretty(ready).map_err(std::io::Error::other).and_then(|json| orbiters_core::write_atomic(&path, &json)) {
        eprintln!("updates: {}: {e}", path.display());
    }
}

/// Whether `setup` is the whole file `ready` names.
fn verified(setup: &Path, ready: &Ready) -> bool {
    std::fs::metadata(setup).is_ok_and(|m| m.len() == ready.size) && orbiters_update::sha256_file(setup).is_ok_and(|h| h == ready.sha256)
}

/// Removes what the updates folder holds but the setup of `keep` and its note (an installed update's setup, a
/// download given up).
fn tidy(keep: Option<&str>) {
    let Ok(entries) = std::fs::read_dir(orbiters_update::updates_dir()) else { return };
    let kept = keep.map(orbiters_update::setup_file);
    for entry in entries.flatten() {
        let path = entry.path();
        let keep_this = kept.as_ref().is_some_and(|k| *k == path) || (keep.is_some() && path.file_name().is_some_and(|n| n == READY_FILE));
        if !keep_this {
            // One still running (the setup that just started this app) goes next time.
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// The setup to install before the app starts: one that was ready, newer than this app and never started; it is
/// marked started. Returns it and the install folder.
pub fn pending_setup() -> Option<(PathBuf, PathBuf)> {
    let (dir, _) = InstallRecord::of_this_program()?;
    let ready = read_ready()?;
    if ready.started || !orbiters_update::is_newer(&ready.version, orbiters_core::app_version()) {
        return None;
    }
    let setup = orbiters_update::setup_file(&ready.version);
    if !verified(&setup, &ready) {
        return None;
    }
    write_ready(&Ready { started: true, ..ready });
    Some((setup, dir))
}

/// Starts `setup` to update the install in `dir` once this app has quit, then start it again (`hidden`: in the tray,
/// as it was). Not in the service's job: it outlives the app.
#[cfg(target_os = "macos")]
pub fn start_setup(setup: &Path, dir: &Path, hidden: bool) -> std::io::Result<()> {
    crate::mac_update::start(setup, dir, hidden)
}

/// Starts `setup` to update the install in `dir` once this app has quit, then start it again (`hidden`: in the tray,
/// as it was). Not in the service's job: it outlives the app.
#[cfg(not(target_os = "macos"))]
pub fn start_setup(setup: &Path, dir: &Path, hidden: bool) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(setup);
    cmd.arg("--update").arg("--dir").arg(dir).arg("--relaunch");
    if hidden {
        cmd.arg("--hidden");
    }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    cmd.spawn().map(drop)
}

impl Controller {
    /// Starts looking for updates, for an app the setup installed.
    pub(super) fn start_updates(self: &Shared) {
        let Some((dir, record)) = InstallRecord::of_this_program() else { return };
        if orbiters_update::public_key().is_none() {
            eprintln!("updates: this build has no release key; it does not update itself");
            return;
        }
        // An update that installed (this app is its version or newer) leaves its setup behind.
        match read_ready() {
            Some(ready) if orbiters_update::is_newer(&ready.version, orbiters_core::app_version()) => {}
            _ => tidy(None),
        }
        self.update(|s| {
            s.update.enabled = true;
            s.update.channel = record.channel.clone();
        });
        let (tx, rx) = mpsc::channel();
        {
            let mut u = self.updates.lock().unwrap();
            u.install = Some(dir);
            u.wake = Some(tx);
        }
        let c = self.clone();
        let spawned = std::thread::Builder::new().name("updates".into()).spawn(move || {
            let mut next = Instant::now() + FIRST_LOOK;
            loop {
                let waiting = c.view().update.restart_when_idle;
                let wait = if waiting { IDLE_POLL.min(next.saturating_duration_since(Instant::now())) } else { next.saturating_duration_since(Instant::now()) };
                let asked = match rx.recv_timeout(wait) {
                    Ok(()) => true,
                    Err(mpsc::RecvTimeoutError::Timeout) => false,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                };
                if c.quitting.load(Ordering::Relaxed) {
                    return;
                }
                if asked || Instant::now() >= next {
                    c.look_for_update();
                    next = Instant::now() + if orbiters_update::development() { EVERY_DEVELOPMENT } else { EVERY };
                }
                if c.view().update.restart_when_idle && c.busy_with().is_none() {
                    c.restart_to_update(true);
                }
            }
        });
        if let Err(e) = spawned {
            eprintln!("updates: {e}");
        }
    }

    /// Lets the service restart the app into a setup.
    pub fn set_restarter(&self, restarter: Restarter) {
        self.updates.lock().unwrap().restarter = Some(restarter);
    }

    /// What a restart would cut short now.
    pub fn busy_with(&self) -> Option<&'static str> {
        self.snapshot.lock().unwrap().busy_with()
    }

    pub fn update_action(self: &Shared, action: UpdateAction) {
        match action {
            UpdateAction::Check => {
                if let Some(wake) = &self.updates.lock().unwrap().wake {
                    let _ = wake.send(());
                }
            }
            UpdateAction::Restart { now } => self.restart_to_update(now),
            UpdateAction::CancelRestart => {
                self.update_if(|s| std::mem::replace(&mut s.update.restart_when_idle, false));
            }
        }
    }

    /// Asks the channel's release; downloads its setup when it is newer.
    fn look_for_update(self: &Shared) {
        let Some(key) = orbiters_update::public_key() else { return };
        let (channel, ready) = {
            let s = self.snapshot.lock().unwrap();
            (s.update.channel.clone(), matches!(s.update.state, UpdateState::Ready { .. }))
        };
        if !ready {
            self.update(|s| s.update.state = UpdateState::Checking);
        }
        let release = match orbiters_update::net::fetch_release(&channel, &key) {
            Ok(release) => release,
            // Nothing out on this channel yet: this app is the newest there is.
            Err(e) if orbiters_update::net::none_published(&e) => {
                let checked = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default();
                self.update(|s| {
                    s.update.checked = Some(checked);
                    s.update.state = UpdateState::Idle;
                });
                return;
            }
            Err(e) => {
                eprintln!("updates: {e:#}");
                if !ready {
                    let error = format!("{e:#}");
                    let message = if orbiters_update::development() {
                        tr!("The development server could not be asked for updates ({error}).", error = error)
                    } else {
                        tr!("Orbiters could not be asked for updates ({error}).", error = error)
                    };
                    self.update(|s| s.update.state = UpdateState::Failed { message });
                }
                return;
            }
        };
        let checked = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default();
        self.update(|s| s.update.checked = Some(checked));
        if !orbiters_update::is_newer(&release.version, orbiters_core::app_version()) {
            self.update(|s| s.update.state = UpdateState::Idle);
            tidy(None);
            return;
        }
        let same = matches!(&self.view().update.state, UpdateState::Ready { version, .. } if *version == release.version);
        if !same {
            self.download_update(release);
        }
    }

    /// Downloads a release's setup (going on from an earlier try), notes it, and tells the person it is ready.
    fn download_update(self: &Shared, release: Release) {
        let version = release.version.clone();
        let setup = orbiters_update::setup_file(&version);
        let ready = Ready {
            version: version.clone(),
            sha256: release.installer.sha256.clone(),
            size: release.installer.size,
            notes: release.notes.clone(),
            started: false,
        };
        let noted = read_ready().filter(|r| r.version == ready.version && r.sha256 == ready.sha256);
        if !verified(&setup, &ready) {
            self.update(|s| s.update.state = UpdateState::Downloading { version: version.clone(), fraction: 0.0 });
            let mut shown = 0.0f32;
            let downloaded = orbiters_update::net::download(
                &release.installer,
                &setup,
                |have, total| {
                    let fraction = have as f32 / total.max(1) as f32;
                    if fraction - shown >= 0.01 || fraction >= 1.0 {
                        shown = fraction;
                        self.update(|s| {
                            if let UpdateState::Downloading { fraction: f, .. } = &mut s.update.state {
                                *f = fraction;
                            }
                        });
                    }
                },
                &self.quitting,
            );
            if let Err(e) = downloaded {
                eprintln!("updates: {e:#}");
                if !self.quitting.load(Ordering::Relaxed) {
                    let message = tr!("Version {version} could not be downloaded: {error}", version = version, error = e);
                    self.update(|s| s.update.state = UpdateState::Failed { message });
                }
                return;
            }
        }
        // Noted already (it was started once): kept as it is.
        if noted.is_none() {
            write_ready(&ready);
        }
        tidy(Some(&version));
        self.update(|s| s.update.state = UpdateState::Ready { version: version.clone(), notes: release.notes.clone() });
        self.notify(NoticeKind::Info, tr!("Orbiters - N4N0 {version} is ready: restart it to update (Settings, About).", version = version));
    }

    /// Restarts into the downloaded update: at once when nothing would be cut short (or `now`), otherwise once the
    /// running work ends.
    fn restart_to_update(self: &Shared, now: bool) {
        let UpdateState::Ready { version, .. } = self.view().update.state.clone() else {
            self.notify(NoticeKind::Warning, tr("No update is ready yet."));
            return;
        };
        if !now {
            if let Some(work) = self.busy_with() {
                let first = self.update_if(|s| !std::mem::replace(&mut s.update.restart_when_idle, true));
                if first {
                    self.notify(NoticeKind::Info, tr!("Orbiters - N4N0 restarts to update once {work} ends.", work = tr(work)));
                }
                return;
            }
        }
        self.update_if(|s| std::mem::replace(&mut s.update.restart_when_idle, false));
        let setup = orbiters_update::setup_file(&version);
        let Some(ready) = read_ready().filter(|r| r.version == version && verified(&setup, r)) else {
            // Gone or damaged since: it downloads again.
            tidy(None);
            let message = tr!("The downloaded update ({version}) was damaged: it downloads again.", version = version);
            self.update(|s| s.update.state = UpdateState::Failed { message });
            self.update_action(UpdateAction::Check);
            return;
        };
        write_ready(&Ready { started: true, ..ready });
        let u = self.updates.lock().unwrap();
        match (&u.restarter, &u.install) {
            (Some(restart), Some(dir)) => restart(&setup, dir),
            _ => self.notify(NoticeKind::Error, tr("This window cannot restart Orbiters - N4N0 to update.")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{CaptureState, Snapshot};

    #[test]
    fn a_restart_waits_for_work_it_would_cut_short() {
        let mut s = Snapshot::new(orbiters_core::settings::Settings::default());
        assert_eq!(s.busy_with(), None);
        s.reconstruct_queue.push(PathBuf::from("session"));
        assert_eq!(s.busy_with(), Some("the reconstruction"), "one waiting counts");
        s.reconstruct_queue.clear();
        s.capture.state = CaptureState::Stopping;
        assert_eq!(s.busy_with(), Some("the recording"), "still being saved");
        s.capture.state = CaptureState::Idle;
        s.n4n0.voice.downloading = Some(0.3);
        assert_eq!(s.busy_with(), Some("the speech models' download"));
    }

    #[test]
    fn a_ready_note_reads_back_and_is_checked_against_its_file() {
        let dir = std::env::temp_dir().join(format!("orbiters-desktop-ready-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let setup = dir.join("OrbitersSetup-9.9.9.exe");
        std::fs::write(&setup, b"setup").unwrap();
        let ready = Ready { version: "9.9.9".into(), sha256: orbiters_update::sha256_file(&setup).unwrap(), size: 5, notes: String::new(), started: false };
        assert!(verified(&setup, &ready));
        assert!(!verified(&setup, &Ready { size: 6, ..ready.clone() }), "another size");
        assert!(!verified(&setup, &Ready { sha256: "0".repeat(64), ..ready.clone() }), "another file");
        let json = serde_json::to_string(&ready).unwrap();
        assert_eq!(serde_json::from_str::<Ready>(&json).unwrap(), ready);
        let old: Ready = serde_json::from_str(r#"{"version":"1.0.0","sha256":"ab","size":1}"#).unwrap();
        assert!(!old.started, "a note without the field was never started");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
