//! Freeing space in the library, for good (the recycle bin would free nothing): the capture files of reconstructed
//! sessions (their 3D views stay, `orbiters_core::cleanup::keep_only_view`) and whole sessions, one after the other on a
//! thread of their own. The snapshot's `cleanup` says how far it got; once it is done, a notice says what was freed and
//! the library reloads. A session being recorded, reconstructed or waiting to be is left alone, checked when asked and
//! again when its turn comes.

use super::{CaptureState, Controller, NoticeKind, Shared};
use orbiters_core::{bundle, cleanup};
use orbiters_i18n::{mark, tr};
use orbiters_ui::format;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// What the running job did so far (the snapshot's `cleanup` holds what it still has to do).
#[derive(Default)]
pub struct CleanupRuntime {
    running: bool,
    trimmed: usize,
    deleted: usize,
    /// Sessions left alone, and why; sessions that could not be cleaned, and why.
    skipped: Vec<String>,
    failed: Vec<String>,
}

/// The job's progress goes out this often while it deletes (every file freed counts).
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

impl Controller {
    /// Frees space for good: the capture files of the `trim` sessions (reconstructed: their 3D views stay), and the
    /// whole of the `delete` ones. Only session folders right inside the sessions folder are taken, and none being
    /// recorded or reconstructed; the rest waits its turn after a job already running.
    pub fn clean_up(self: &Shared, trim: Vec<PathBuf>, delete: Vec<PathBuf>) -> anyhow::Result<()> {
        let root = self.settings().sessions_dir;
        let asked = trim.len() + delete.len();
        let (trim, delete): (Vec<_>, Vec<_>) = (
            trim.into_iter().filter(|d| session_folder(&root, d) && cleanup::has_view(d)).collect(),
            delete.into_iter().filter(|d| session_folder(&root, d)).collect(),
        );
        let busy: Vec<String> = trim.iter().chain(&delete).filter_map(|d| self.cleanup_blocked(d).map(|why| format!("{} ({})", name(d), tr(why)))).collect();
        let (trim, delete): (Vec<_>, Vec<_>) = (
            trim.into_iter().filter(|d| self.cleanup_blocked(d).is_none()).collect(),
            delete.into_iter().filter(|d| self.cleanup_blocked(d).is_none()).collect(),
        );
        if trim.is_empty() && delete.is_empty() {
            // Nothing taken: the window's "under way" goes with the next snapshot.
            self.publish();
            anyhow::ensure!(busy.is_empty(), "{}", tr!("Left alone: {list}.", list = busy.join(", ")));
            anyhow::bail!("{}", if asked == 1 { tr("This session cannot be cleaned up.") } else { tr("These sessions cannot be cleaned up.") });
        }
        // What it frees, from the sizes the library keeps (measured now for a session not listed yet).
        let bytes = |d: &PathBuf, whole: bool| {
            let video = bundle::read_manifest(d).ok().and_then(|m| m.capture.video);
            let sizes = bundle::session_sizes(d, video.as_deref());
            if whole {
                sizes.total
            } else {
                sizes.capture
            }
        };
        let total: u64 = trim.iter().map(|d| bytes(d, false)).sum::<u64>() + delete.iter().map(|d| bytes(d, true)).sum::<u64>();
        let start = {
            let mut rt = self.cleanup.lock().unwrap();
            if !busy.is_empty() {
                rt.skipped.extend(busy);
            }
            self.update(|s| {
                let job = s.cleanup.get_or_insert_with(Default::default);
                job.add(&trim, &delete);
                job.total += total;
            });
            !std::mem::replace(&mut rt.running, true)
        };
        if start {
            let ctl = self.clone();
            if let Err(e) = std::thread::Builder::new().name("cleanup".into()).spawn(move || ctl.run_cleanup()) {
                // Nothing runs it: not "running" for good (audit F14, CTL-11 b). What was asked stays listed and goes
                // with the next clean-up asked for; the error says why it did not start.
                self.cleanup.lock().unwrap().running = false;
                return Err(e.into());
            }
        }
        Ok(())
    }

    /// Why `dir` cannot be cleaned up now: it is being recorded, reconstructed, or waits to be.
    pub(super) fn cleanup_blocked(&self, dir: &Path) -> Option<&'static str> {
        let s = self.snapshot.lock().unwrap();
        if s.capture.state != CaptureState::Idle && s.capture.session_dir.as_deref().is_some_and(|d| same(d, dir)) {
            Some(mark("it is being recorded"))
        } else if s.reconstruction.as_ref().is_some_and(|j| same(&j.session_dir, dir)) {
            Some(mark("it is being reconstructed"))
        } else if s.reconstruct_queue.iter().any(|d| same(d, dir)) {
            Some(mark("it waits to be reconstructed"))
        } else {
            None
        }
    }

    /// Whether the clean-up has `dir` still to do or doing.
    pub(super) fn cleaning(&self, dir: &Path) -> bool {
        self.snapshot.lock().unwrap().cleanup.as_ref().is_some_and(|j| j.of(dir).is_some())
    }

    /// The job: one session after the other until none waits, then what came of it.
    fn run_cleanup(self: &Shared) {
        let summary = loop {
            // Taken, or the job ended, under the lock a new request takes too: none comes between the last session
            // and the end, unseen.
            let step = {
                let mut rt = self.cleanup.lock().unwrap();
                let step = self.update(|s| {
                    let Some(job) = s.cleanup.as_mut() else { return Err(0) };
                    let next = if !job.trim.is_empty() {
                        Some((job.trim.remove(0), false))
                    } else if !job.delete.is_empty() {
                        Some((job.delete.remove(0), true))
                    } else {
                        None
                    };
                    job.current = next.clone();
                    next.ok_or_else(|| s.cleanup.take().map_or(0, |job| job.freed))
                });
                match step {
                    Ok(next) => Ok(next),
                    Err(freed) => {
                        rt.running = false;
                        Err((freed, std::mem::take(&mut *rt)))
                    }
                }
            };
            let (dir, whole) = match step {
                Ok(next) => next,
                Err(summary) => break summary,
            };
            if let Some(why) = self.cleanup_blocked(&dir) {
                self.cleanup.lock().unwrap().skipped.push(format!("{} ({})", name(&dir), tr(why)));
                continue;
            }
            let mut published = Instant::now();
            let mut freed = |n: u64| {
                if let Some(job) = self.snapshot.lock().unwrap().cleanup.as_mut() {
                    job.freed += n;
                }
                if published.elapsed() >= PROGRESS_EVERY {
                    published = Instant::now();
                    self.publish();
                }
            };
            if whole {
                // Its card on Orbiters keeps its picture once the folder is gone (shares.log says what was given, or
                // what could not be: the person asked for the folder to go, so it goes all the same).
                let _ = super::shares::keep_pictures_on_orbiters(&dir);
            }
            let result = if whole { cleanup::delete_session(&dir, &mut freed) } else { cleanup::keep_only_view(&dir, &mut freed) };
            let mut rt = self.cleanup.lock().unwrap();
            match result {
                Ok(_) if whole => rt.deleted += 1,
                Ok(_) => rt.trimmed += 1,
                Err(e) => rt.failed.push(format!("{}: {e}", name(&dir))),
            }
        };
        self.cleanup_done(summary);
    }

    /// The job is over (it left the snapshot): the library reloads (the window takes the list it reads then for how
    /// things are), and notices say what was freed and what was not.
    fn cleanup_done(self: &Shared, (freed, done): (u64, CleanupRuntime)) {
        super::refresh_storage(self);
        self.bump_library();
        if done.trimmed + done.deleted > 0 {
            self.notify(NoticeKind::Success, freed_message(freed, done.trimmed, done.deleted));
        }
        if !done.skipped.is_empty() {
            self.notify(NoticeKind::Warning, tr!("Left alone: {list}.", list = done.skipped.join(tr(", "))));
        }
        if !done.failed.is_empty() {
            self.notify(NoticeKind::Error, tr!("Not everything could be deleted: {list}", list = done.failed.join("; ")));
        }
    }
}

/// "Freed 3.1 GB: 12 captures keep only their 3D view and 5 were deleted."
fn freed_message(freed: u64, trimmed: usize, deleted: usize) -> String {
    let bytes = format::bytes(freed as f64);
    // Whole sentences for each case: the order of the parts and the plural are each language's own.
    match (trimmed, deleted) {
        (0, 0) => tr!("Freed {bytes}.", bytes = bytes),
        (1, 0) => tr!("Freed {bytes}: 1 capture keeps only its 3D view.", bytes = bytes),
        (n, 0) => tr!("Freed {bytes}: {trimmed} captures keep only their 3D view.", bytes = bytes, trimmed = n),
        (0, 1) => tr!("Freed {bytes}: 1 capture deleted.", bytes = bytes),
        (0, n) => tr!("Freed {bytes}: {deleted} captures deleted.", bytes = bytes, deleted = n),
        (1, 1) => tr!("Freed {bytes}: 1 capture keeps only its 3D view and 1 was deleted.", bytes = bytes),
        (1, n) => tr!("Freed {bytes}: 1 capture keeps only its 3D view and {deleted} were deleted.", bytes = bytes, deleted = n),
        (n, 1) => tr!("Freed {bytes}: {trimmed} captures keep only their 3D view and 1 was deleted.", bytes = bytes, trimmed = n),
        (t, d) => tr!("Freed {bytes}: {trimmed} captures keep only their 3D view and {deleted} were deleted.", bytes = bytes, trimmed = t, deleted = d),
    }
}

/// A session's folder name, as notices name it.
fn name(dir: &Path) -> String {
    dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.display().to_string())
}

/// Whether two paths name one folder (as written, or once resolved).
fn same(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// Whether `dir` is a session folder right inside the sessions folder `root` (a folder of its own there, not a link out
/// of it, with a manifest): the only folders the clean-up deletes from.
fn session_folder(root: &Path, dir: &Path) -> bool {
    let (Ok(root), Ok(real)) = (root.canonicalize(), dir.canonicalize()) else { return false };
    real.parent() == Some(root.as_path()) && std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) && dir.join("manifest.json").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::Controller;
    use crate::state::{CaptureState, ReconstructionJob};
    use orbiters_contracts::manifest::SessionManifest;
    use orbiters_core::settings::Settings;
    use std::fs;

    /// A library of four sessions under a temporary folder: two reconstructed, two not; one of each "busy" below.
    fn library(name: &str) -> (PathBuf, Vec<PathBuf>) {
        let root = std::env::temp_dir().join(format!("orbiters-cleanup-job-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dirs: Vec<PathBuf> = ["Viewed", "Viewed busy", "Raw", "Raw busy"]
            .iter()
            .map(|n| {
                let (dir, _) = bundle::BundleWriter::create(&root, n, SessionManifest::new(n.to_string(), orbiters_core::now_utc_iso(), "0.2.1"))
                    .unwrap()
                    .finish()
                    .unwrap();
                fs::write(dir.join("keyframes").join("000000.qoi"), vec![1u8; 4000]).unwrap();
                fs::write(dir.join("parameters.bin"), vec![1u8; 500]).unwrap();
                if n.starts_with("Viewed") {
                    fs::create_dir_all(dir.join("reconstruction")).unwrap();
                    fs::write(dir.join("reconstruction").join("splats.ply"), vec![1u8; 2000]).unwrap();
                    fs::write(dir.join("reconstruction").join("report.json"), "{}").unwrap();
                }
                dir
            })
            .collect();
        (root, dirs)
    }

    /// Waits for the clean-up to end, and for what it tells (it tells once the job left the snapshot).
    fn finished(ctl: &Shared) -> Vec<String> {
        let until = Instant::now() + Duration::from_secs(10);
        let mut notices = Vec::new();
        while (ctl.view().cleanup.is_some() || notices.is_empty()) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
            notices.extend(ctl.take_notices().into_iter().map(|n| format!("{n:?}")));
        }
        assert!(ctl.view().cleanup.is_none(), "the clean-up ended");
        notices
    }

    #[test]
    fn frees_space_for_good_and_leaves_busy_sessions_alone() {
        let (root, dirs) = library("busy");
        let ctl = Controller::new(Settings { sessions_dir: root.clone(), ..Default::default() });
        ctl.update(|s| {
            s.capture.state = CaptureState::Recording;
            s.capture.session_dir = Some(dirs[3].clone());
            s.reconstruction = Some(ReconstructionJob::new(dirs[1].clone(), false));
        });
        ctl.clean_up(vec![dirs[0].clone(), dirs[1].clone()], vec![dirs[2].clone(), dirs[3].clone()]).unwrap();
        let mut notices = finished(&ctl);
        // The viewed one keeps its view, the raw one is gone, not in the recycle bin; the busy ones are untouched.
        assert!(cleanup::trimmed(&dirs[0]) && dirs[0].join("reconstruction/splats.ply").is_file());
        assert!(!dirs[2].exists());
        assert!(!cleanup::trimmed(&dirs[1]) && dirs[3].join("keyframes").is_dir());
        // The second notice follows the first.
        std::thread::sleep(Duration::from_millis(50));
        notices.extend(ctl.take_notices().into_iter().map(|n| format!("{n:?}")));
        assert!(notices.iter().any(|n| n.contains("Freed") && n.contains("1 capture keeps only its 3D view and 1 was deleted")), "{notices:?}");
        assert!(
            notices
                .iter()
                .any(|n| n.contains("Left alone") && n.contains("Viewed busy (it is being reconstructed)") && n.contains("Raw busy (it is being recorded)")),
            "{notices:?}"
        );
        // The library says so: the sizes kept for the trimmed one were measured again. Of what stays, the busy viewed
        // one's capture files can still go (the one recording counts through its own stats).
        let listed = bundle::list_sessions(&root);
        let viewed = listed.iter().find(|s| s.dir == dirs[0]).unwrap();
        assert!(viewed.trimmed && viewed.capture_bytes == 0 && viewed.size_bytes < 4000);
        let storage = ctl.view().storage.clone();
        assert_eq!((storage.sessions, storage.capture_sessions, storage.capture_bytes), (2, 1, 4500));
        ctl.shutdown();
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn only_session_folders_of_the_library_are_deleted() {
        let (root, dirs) = library("outside");
        let elsewhere = std::env::temp_dir().join(format!("orbiters-cleanup-elsewhere-{}", std::process::id()));
        let _ = fs::remove_dir_all(&elsewhere);
        let (stranger, _) = bundle::BundleWriter::create(&elsewhere, "Stranger", SessionManifest::default()).unwrap().finish().unwrap();
        let ctl = Controller::new(Settings { sessions_dir: root.clone(), ..Default::default() });
        // A session of another folder, the sessions folder itself, a folder inside a session, a folder without a
        // manifest, and keeping only the 3D view of a session that has none.
        fs::create_dir_all(root.join("Not a session")).unwrap();
        for (trim, delete) in [
            (vec![], vec![stranger.clone()]),
            (vec![], vec![root.clone()]),
            (vec![], vec![dirs[2].join("keyframes")]),
            (vec![], vec![root.join("Not a session")]),
            (vec![dirs[2].clone()], vec![]),
        ] {
            assert!(ctl.clean_up(trim.clone(), delete.clone()).is_err(), "{trim:?} {delete:?}");
            assert!(ctl.view().cleanup.is_none());
        }
        assert!(stranger.join("manifest.json").is_file() && dirs[2].join("keyframes").is_dir() && root.join("Not a session").is_dir());
        // Spelled through `..`, the library's own session is still one of its folders.
        ctl.clean_up(vec![root.join("..").join(root.file_name().unwrap()).join("Viewed")], vec![]).unwrap();
        finished(&ctl);
        assert!(cleanup::trimmed(&dirs[0]));
        ctl.shutdown();
        fs::remove_dir_all(&root).ok();
        fs::remove_dir_all(&elsewhere).ok();
    }

    #[test]
    fn notices_say_what_was_freed() {
        assert_eq!(freed_message(3_100_000_000, 12, 5), "Freed 3.1 GB: 12 captures keep only their 3D view and 5 were deleted.");
        assert_eq!(freed_message(757_000_000, 1, 0), "Freed 757 MB: 1 capture keeps only its 3D view.");
        assert_eq!(freed_message(820_000_000, 0, 1), "Freed 820 MB: 1 capture deleted.");
        assert_eq!(freed_message(820_000_000, 0, 5), "Freed 820 MB: 5 captures deleted.");
    }
}
