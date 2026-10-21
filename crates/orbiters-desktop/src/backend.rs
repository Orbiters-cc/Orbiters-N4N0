//! What the interface talks to: the tray service over the IPC link (the window process), or a controller in the
//! same process (screenshots, the end-to-end test). Actions are fire-and-forget: the interface shows them at once
//! and failures come back as notices.

use crate::controller::{self, Notice, NoticeKind};
use crate::ipc::{Client, OrbCommand, ToService};
use crate::state::{MapsMine, Snapshot};
use orbiters_contracts::performance::calibration::Calibration;
use orbiters_core::settings::Settings;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub trait Backend: Send + Sync {
    fn view(&self) -> Arc<Snapshot>;
    fn take_notices(&self) -> Vec<Notice>;
    /// Bumped when the session library changes.
    fn library_version(&self) -> u64;
    /// Repaint `ctx` whenever something changes.
    fn attach(&self, ctx: &egui::Context);
    fn start_capture(&self);
    fn stop_capture(&self);
    fn pause_capture(&self, paused: bool);
    fn start_test_capture(&self);
    /// VRChat's camera flies around where it stands, at three heights, then the recording is reconstructed.
    fn scan(&self) {}
    fn import_raw_capture(&self, dir: PathBuf);
    fn save_settings(&self, settings: Settings);
    fn trash_session(&self, dir: PathBuf);
    /// Trains splats from a session in the reconstruction worker, after the ones waiting before it.
    fn reconstruct(&self, dir: PathBuf);
    fn stop_reconstruction(&self);
    /// Takes a waiting session off the reconstruction list.
    fn unqueue_reconstruction(&self, dir: PathBuf);
    /// Sends a session's splats to the Unity editor running Mixer.
    fn send_to_mixer(&self, dir: PathBuf, title: String);
    /// Signs in to Orbiters (the account the Unity tools share) through the browser.
    fn sign_in(&self, provider: &str);
    fn reopen_sign_in(&self);
    fn cancel_sign_in(&self);
    fn sign_out(&self);
    /// Asks for the worker's log; [`Backend::worker_log`] has it once it arrived.
    fn request_worker_log(&self);
    fn worker_log(&self) -> Option<Vec<String>>;
    /// Shares pictures of a world on the Map explorer (the tray service sends them).
    fn upload_pictures(&self, world_id: String, world_name: String, paths: Vec<PathBuf>);
    /// Shares a session's reconstruction as a world's 3D preview.
    fn upload_preview(&self, world_id: String, world_name: String, session: PathBuf);
    /// Asks what this member added to the Map explorer; [`Backend::maps`] has it once it arrived.
    fn request_maps(&self);
    fn maps(&self) -> Option<MapsMine>;
    /// A T-pose calibration taken in the overlay.
    fn calibrated(&self, calibration: Calibration);
    /// Orb: a question, the Talk button, the panel, saved videos.
    fn orb(&self, command: OrbCommand);
    /// VRChat's frames the overlay saw, for world performance statistics.
    fn frames(&self, chunk: crate::state::FrameChunk);
    /// Sets the walk boost's strength (0 to 1), saved and sent to the avatar's `Orb/WalkBoost`.
    fn walk_boost(&self, strength: f32);
    /// Quits the whole app (the tray service and every window).
    fn quit(&self);
    /// The service asked this window to come to the front.
    fn take_show(&self) -> bool {
        false
    }
    /// This window should close (the app quits, or the service went away).
    fn should_close(&self) -> bool {
        false
    }
}

pub type Ctl = Arc<dyn Backend>;

/// The tray service, over the link.
pub struct Remote(pub Arc<Client>);

impl Backend for Remote {
    fn view(&self) -> Arc<Snapshot> {
        self.0.snapshot()
    }

    fn take_notices(&self) -> Vec<Notice> {
        self.0.take_notices()
    }

    fn library_version(&self) -> u64 {
        self.0.library_version()
    }

    fn attach(&self, ctx: &egui::Context) {
        self.0.attach(ctx);
    }

    fn start_capture(&self) {
        self.0.send(&ToService::StartCapture);
    }

    fn stop_capture(&self) {
        self.0.send(&ToService::StopCapture);
    }

    fn pause_capture(&self, paused: bool) {
        self.0.send(&ToService::Pause { paused });
    }

    fn start_test_capture(&self) {
        self.0.send(&ToService::TestPattern);
    }

    fn scan(&self) {
        self.0.send(&ToService::Scan);
    }

    fn import_raw_capture(&self, dir: PathBuf) {
        self.0.send(&ToService::ImportRaw { dir });
    }

    fn save_settings(&self, settings: Settings) {
        self.0.update_local(|s| s.settings = settings.clone());
        self.0.send(&ToService::SaveSettings { settings: Box::new(settings) });
    }

    fn trash_session(&self, dir: PathBuf) {
        self.0.send(&ToService::Trash { dir });
    }

    fn reconstruct(&self, dir: PathBuf) {
        self.0.send(&ToService::Reconstruct { dir });
    }

    fn stop_reconstruction(&self) {
        self.0.send(&ToService::StopReconstruction);
    }

    fn unqueue_reconstruction(&self, dir: PathBuf) {
        self.0.send(&ToService::Unqueue { dir });
    }

    fn send_to_mixer(&self, dir: PathBuf, title: String) {
        self.0.send(&ToService::SendToMixer { dir, title });
    }

    fn sign_in(&self, provider: &str) {
        self.0.send(&ToService::SignIn { provider: provider.to_string() });
    }

    fn reopen_sign_in(&self) {
        self.0.send(&ToService::ReopenSignIn);
    }

    fn cancel_sign_in(&self) {
        self.0.send(&ToService::CancelSignIn);
    }

    fn sign_out(&self) {
        self.0.send(&ToService::SignOut);
    }

    fn request_worker_log(&self) {
        self.0.send(&ToService::WorkerLog);
    }

    fn worker_log(&self) -> Option<Vec<String>> {
        self.0.worker_log()
    }

    fn upload_pictures(&self, world_id: String, world_name: String, paths: Vec<PathBuf>) {
        self.0.send(&ToService::UploadPictures { world_id, world_name, paths });
    }

    fn upload_preview(&self, world_id: String, world_name: String, session: PathBuf) {
        self.0.send(&ToService::UploadPreview { world_id, world_name, session });
    }

    fn request_maps(&self) {
        self.0.send(&ToService::Maps);
    }

    fn maps(&self) -> Option<MapsMine> {
        self.0.maps()
    }

    fn frames(&self, chunk: crate::state::FrameChunk) {
        self.0.send(&ToService::Frames { chunk });
    }

    fn walk_boost(&self, strength: f32) {
        self.0.send(&ToService::WalkBoost { strength });
    }

    fn calibrated(&self, calibration: Calibration) {
        self.0.send(&ToService::Calibrated { calibration: Box::new(calibration) });
    }

    fn orb(&self, command: OrbCommand) {
        self.0.send(&ToService::Orb { command });
    }

    fn quit(&self) {
        self.0.send(&ToService::Quit);
    }

    fn take_show(&self) -> bool {
        self.0.take_show()
    }

    fn should_close(&self) -> bool {
        self.0.should_close()
    }
}

/// A controller in this process.
pub struct Local {
    ctl: controller::Shared,
    log: Mutex<Option<Vec<String>>>,
}

impl Local {
    pub fn new(ctl: controller::Shared) -> Self {
        Self { ctl, log: Mutex::new(None) }
    }

    fn report(&self, r: anyhow::Result<()>) {
        if let Err(e) = r {
            self.ctl.notify(NoticeKind::Error, e.to_string());
        }
    }
}

impl Backend for Local {
    fn view(&self) -> Arc<Snapshot> {
        self.ctl.view()
    }

    fn take_notices(&self) -> Vec<Notice> {
        self.ctl.take_notices()
    }

    fn library_version(&self) -> u64 {
        self.ctl.library_version()
    }

    fn attach(&self, ctx: &egui::Context) {
        self.ctl.attach(ctx);
    }

    fn start_capture(&self) {
        self.report(self.ctl.start_capture(false));
    }

    fn stop_capture(&self) {
        self.report(self.ctl.stop_capture());
    }

    fn pause_capture(&self, paused: bool) {
        self.report(self.ctl.pause_capture(paused));
    }

    fn start_test_capture(&self) {
        self.report(self.ctl.start_test_capture());
    }

    fn scan(&self) {
        self.report(self.ctl.take_scan());
    }

    fn import_raw_capture(&self, dir: PathBuf) {
        self.report(self.ctl.import_raw_capture(dir));
    }

    fn save_settings(&self, settings: Settings) {
        self.ctl.save_settings(settings);
    }

    fn trash_session(&self, dir: PathBuf) {
        let r = self.ctl.trash_session(&dir);
        if r.is_ok() {
            self.ctl.notify(NoticeKind::Success, "Session moved to the recycle bin.");
        }
        self.report(r);
    }

    fn reconstruct(&self, dir: PathBuf) {
        self.report(self.ctl.reconstruct(dir));
    }

    fn stop_reconstruction(&self) {
        self.ctl.stop_reconstruction();
    }

    fn unqueue_reconstruction(&self, dir: PathBuf) {
        self.ctl.unqueue_reconstruction(&dir);
    }

    fn send_to_mixer(&self, dir: PathBuf, title: String) {
        self.report(self.ctl.send_to_mixer(dir, title));
    }

    fn sign_in(&self, provider: &str) {
        let ctl = self.ctl.clone();
        let provider = provider.to_string();
        std::thread::spawn(move || {
            if let Err(e) = ctl.sign_in(&provider) {
                ctl.notify(crate::controller::NoticeKind::Error, format!("Could not start the sign-in: {e}"));
            }
        });
    }

    fn reopen_sign_in(&self) {
        self.report(self.ctl.reopen_sign_in());
    }

    fn cancel_sign_in(&self) {
        self.ctl.cancel_sign_in();
    }

    fn sign_out(&self) {
        self.report(self.ctl.sign_out());
    }

    fn request_worker_log(&self) {
        *self.log.lock().unwrap() = Some(self.ctl.worker_log());
    }

    fn worker_log(&self) -> Option<Vec<String>> {
        self.log.lock().unwrap().clone()
    }

    fn upload_pictures(&self, world_id: String, world_name: String, paths: Vec<PathBuf>) {
        self.report(self.ctl.upload_pictures(world_id, world_name, paths));
    }

    fn upload_preview(&self, world_id: String, world_name: String, session: PathBuf) {
        self.report(self.ctl.upload_preview(world_id, world_name, session));
    }

    fn request_maps(&self) {
        self.ctl.refresh_maps();
    }

    fn maps(&self) -> Option<MapsMine> {
        self.ctl.maps()
    }

    fn frames(&self, chunk: crate::state::FrameChunk) {
        self.ctl.frames(chunk);
    }

    fn walk_boost(&self, strength: f32) {
        self.ctl.set_walk_boost(strength);
    }

    fn calibrated(&self, calibration: Calibration) {
        self.ctl.calibrated(calibration);
    }

    fn orb(&self, command: OrbCommand) {
        self.ctl.orb_command(command);
    }

    fn quit(&self) {
        self.ctl.shutdown();
    }
}
