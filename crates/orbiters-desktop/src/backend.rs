//! What the interface talks to: the tray service over the IPC link (the window process), or a controller in the
//! same process (screenshots, the end-to-end test). Actions are fire-and-forget: the interface shows them at once
//! and failures come back as notices.

use crate::controller::{self, Notice, NoticeKind};
use crate::ipc::{Client, N4n0Command, ToService, UpdateAction};
use crate::state::{AccountState, MapsMine, Snapshot};
use orbiters_contracts::performance::calibration::Calibration;
use orbiters_core::settings::Settings;
use orbiters_i18n::tr;
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
    fn start_camera_recording(&self) {}
    fn camera_qr(&self, _active: bool) {}
    fn switch_demo_avatar(&self) {}
    fn stop_capture(&self);
    /// The Orbiters window to the front.
    fn show_window(&self) {}
    /// The Orbiters window, on one of its pages (`"projects"`: where My Avatar is).
    fn show_page(&self, _page: &str) {
        self.show_window();
    }
    /// The page the service asked this window to show, once.
    fn take_show_page(&self) -> Option<String> {
        None
    }
    /// What the panels around VRChat's menus say of themselves, for Settings.
    fn orbit_status(&self, _status: crate::state::OrbitStatus) {}
    /// What live translation in VR is doing, for the menus and Settings.
    fn translate_status(&self, _status: crate::state::TranslateStatus) {}
    /// The beta's usage statistics noted in this process since the last call (`usage`): only a process linked to the service sends them.
    fn usage(&self, _steps: Vec<crate::usage::Step>) {}
    /// What VRChat said over OSC of how the player moves since the last call (the service forwards it while live
    /// translation is on): when it came, the name (`VelocityX`, `VelocityY`, `VelocityZ`, `/avatar/eyeheight`), the value.
    fn take_locomotion(&self) -> Vec<(std::time::Instant, String, f32)> {
        Vec::new()
    }
    /// Lines translated by Orbiters' service (blocking: call it from a thread of its own).
    fn translate(&self, _batch: orbiters_translate::provider::Batch) -> controller::translate::Answer {
        Err(controller::translate::Refused::SignedOut)
    }
    fn pause_capture(&self, paused: bool);
    fn start_test_capture(&self);
    /// VRChat's camera flies around where it stands, at three heights, then the recording is reconstructed.
    fn scan(&self) {}
    fn import_raw_capture(&self, dir: PathBuf);
    /// Changes only the fields in `patch` ([`Settings::diff`], [`Settings::patched`]), on the settings as they are when
    /// it arrives.
    fn patch_settings(&self, patch: serde_json::Value);
    fn patch_smoothing(&self, patch: serde_json::Value) { self.patch_settings(patch); }
    /// Saves what `settings` changed against `base`, the copy it was changed from (a snapshot's): only those fields,
    /// so that whatever changed meanwhile (the avatar's radial, N4N0, the other window) stays.
    fn save_changes(&self, base: &Settings, settings: &Settings) {
        if base != settings {
            self.patch_settings(base.diff(settings));
        }
    }
    fn trash_session(&self, dir: PathBuf);
    /// Frees space for good: the capture files of `trim` (reconstructed sessions, their 3D views kept), and the whole
    /// of `delete`. The snapshot's `cleanup` follows it.
    fn clean_up(&self, trim: Vec<PathBuf>, delete: Vec<PathBuf>);
    /// Trains splats from a session in the reconstruction worker, after the ones waiting before it.
    fn reconstruct(&self, dir: PathBuf);
    fn stop_reconstruction(&self);
    /// Takes a waiting session off the reconstruction list.
    fn unqueue_reconstruction(&self, dir: PathBuf);
    /// Sends a session's splats to the Unity editor running Mixer.
    fn send_to_mixer(&self, dir: PathBuf, title: String);
    /// Signs in to Orbiters (the account the Unity tools share) through the browser: Connect on the page it opens.
    fn sign_in(&self);
    fn reopen_sign_in(&self);
    /// What Settings asks of the VRChat bridge.
    fn bridge_command(&self, _command: crate::bridge::Command) {}
    fn cancel_sign_in(&self);
    fn sign_out(&self);
    fn vrchat_login(&self, _command: crate::controller::vrchat_login::Command) {}
    /// Blocking read-only request. Call only from a page job or image loader.
    fn vrchat_data(&self, _scope: controller::vrchat_data::Scope, _request: controller::vrchat_data::Request) -> controller::vrchat_data::Result {
        Err(tr("VRChat data is unavailable.").into())
    }
    /// Uploads the picture file at `path` as the VRChat picture of `id`, an avatar or world of the signed-in VRChat
    /// account. Its step and outcome come in the snapshot's `vrchat_login.upload`, the outcome as a notice too.
    fn vrchat_picture(&self, _scope: controller::vrchat_data::Scope, _id: String, _path: PathBuf) {}
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
    fn capture_share(&self, _request: controller::shares::Request) {}
    /// Share to … opened on a 3D capture not uploaded yet: the tray makes its upload files while friends are chosen.
    fn prepare_share(&self, _session: PathBuf) {}
    /// A T-pose calibration taken in the overlay.
    fn calibrated(&self, calibration: Calibration);
    /// N4N0: a question, the Talk button, the panel, saved videos.
    fn n4n0(&self, command: N4n0Command);
    /// VRChat's frames the overlay saw, for world performance statistics.
    fn frames(&self, chunk: crate::state::FrameChunk);
    /// Whether someone wears the headset (the overlay reads SteamVR's activity level).
    fn headset(&self, in_use: bool);
    /// What each of SteamVR's device indices is (the overlay reads it), for tracking smoothing's strengths.
    fn steamvr_devices(&self, _kinds: Vec<crate::tracking::DeviceKind>) {}
    /// How Fast walk carries the person now (the overlay decides it).
    fn walk_path(&self, _path: crate::state::WalkPath) {}
    /// What the overlay concluded of the frames.
    fn booster_report(&self, _report: crate::booster::OverlayReport) {}
    /// What a surface asks of the FPS booster.
    fn booster(&self, _command: crate::booster::Command) {}
    /// Sets the walk boost's strength (0 to 1), saved and sent to the avatar's `N4N0/WalkBoost`.
    fn walk_boost(&self, strength: f32);
    /// Makes VRChat find the OSC service again (it sends avatar parameters only to services it found).
    fn osc_reconnect(&self);
    /// Updates: look now, restart into the downloaded one, or not after all.
    fn updates(&self, action: UpdateAction);
    /// Quits the whole app (the tray service and every window).
    fn quit(&self);
    /// The service asked this window to come to the front.
    fn take_show(&self) -> bool {
        false
    }
    /// The window is behind VRChat (VRChat runs and the window does not have the focus): the service's news is drawn
    /// at most a few times a second.
    fn set_background(&self, _background: bool) {}
    /// This window should close (the app quits, or the service went away).
    fn should_close(&self) -> bool {
        false
    }
}

pub type Ctl = Arc<dyn Backend>;

/// The tray service, over the link.
pub struct Remote(pub Arc<Client>);

impl Backend for Remote {
    fn camera_qr(&self, active: bool) { self.0.send(&ToService::CameraQr { active }); }
    fn switch_demo_avatar(&self) { self.0.send(&ToService::SwitchDemoAvatar); }
    fn show_window(&self) {
        self.0.send(&ToService::ShowWindow);
    }
    fn show_page(&self, page: &str) {
        self.0.send(&ToService::ShowPage { page: page.to_string() });
    }
    fn orbit_status(&self, status: crate::state::OrbitStatus) {
        self.0.send(&ToService::Orbit { status });
    }
    fn translate_status(&self, status: crate::state::TranslateStatus) {
        self.0.send(&ToService::TranslateStatus { status });
    }
    fn usage(&self, steps: Vec<crate::usage::Step>) {
        self.0.send(&ToService::Usage { steps });
    }
    fn translate(&self, batch: orbiters_translate::provider::Batch) -> controller::translate::Answer {
        self.0.translate(batch)
    }
    fn take_locomotion(&self) -> Vec<(std::time::Instant, String, f32)> {
        self.0.take_locomotion()
    }
    fn vrchat_data(&self, scope: controller::vrchat_data::Scope, request: controller::vrchat_data::Request) -> controller::vrchat_data::Result {
        self.0.vrchat_data(scope, request)
    }
    fn vrchat_picture(&self, scope: controller::vrchat_data::Scope, id: String, path: PathBuf) {
        self.0.send(&ToService::VrchatPicture { scope, id, path });
    }
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
        self.0.send(&ToService::StartCapture { origin: crate::ipc::RecordingOrigin::OtherUi });
    }

    fn start_camera_recording(&self) {
        self.0.send(&ToService::StartCapture { origin: crate::ipc::RecordingOrigin::CameraOverlay });
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

    fn patch_smoothing(&self, patch: serde_json::Value) { self.0.patch_smoothing(patch); }

    fn patch_settings(&self, patch: serde_json::Value) {
        self.0.update_local(|s| s.settings = s.settings.patched(&patch).0.into());
        self.0.send(&ToService::PatchSettings { patch });
    }

    fn trash_session(&self, dir: PathBuf) {
        self.0.send(&ToService::Trash { dir });
    }

    fn clean_up(&self, trim: Vec<PathBuf>, delete: Vec<PathBuf>) {
        // Under way at once; the service's snapshots carry it on (and end it, a refusal too).
        self.0.update_local(|s| s.cleanup.get_or_insert_with(Default::default).add(&trim, &delete));
        self.0.send(&ToService::CleanUp { trim, delete });
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

    fn sign_in(&self) {
        // Finishing in the browser at once: the service asks Orbiters for the link, then opens it.
        self.0.update_local(|s| {
            s.account.state = AccountState::SigningIn;
            s.account.error = None;
        });
        self.0.send(&ToService::SignIn);
    }

    fn reopen_sign_in(&self) {
        self.0.send(&ToService::ReopenSignIn);
    }

    fn bridge_command(&self, command: crate::bridge::Command) {
        self.0.send(&ToService::Bridge { command });
    }

    fn cancel_sign_in(&self) {
        self.0.send(&ToService::CancelSignIn);
    }

    fn sign_out(&self) {
        self.0.send(&ToService::SignOut);
    }

    fn vrchat_login(&self, command: crate::controller::vrchat_login::Command) {
        self.0.update_local(|s| {
            s.vrchat_login.busy = true;
            s.vrchat_login.error = None;
        });
        self.0.send(&ToService::VrchatLogin { command });
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

    fn capture_share(&self, request: controller::shares::Request) {
        self.0.send(&ToService::CaptureShare { request: Box::new(request) });
    }

    fn prepare_share(&self, session: PathBuf) {
        self.0.send(&ToService::PrepareShare { session });
    }

    fn frames(&self, chunk: crate::state::FrameChunk) {
        self.0.send(&ToService::Frames { chunk });
    }

    fn headset(&self, in_use: bool) {
        self.0.send(&ToService::Headset { in_use });
    }

    fn steamvr_devices(&self, kinds: Vec<crate::tracking::DeviceKind>) {
        self.0.send(&ToService::SteamVrDevices { kinds });
    }

    fn walk_path(&self, path: crate::state::WalkPath) {
        self.0.send(&ToService::WalkPath { path });
    }

    fn booster_report(&self, report: crate::booster::OverlayReport) {
        self.0.send(&ToService::BoosterReport { report: Box::new(report) });
    }

    fn booster(&self, command: crate::booster::Command) {
        self.0.send(&ToService::Booster { command });
    }

    fn walk_boost(&self, strength: f32) {
        self.0.send(&ToService::WalkBoost { strength });
    }

    fn osc_reconnect(&self) {
        self.0.send(&ToService::OscReconnect);
    }

    fn calibrated(&self, calibration: Calibration) {
        self.0.send(&ToService::Calibrated { calibration: Box::new(calibration) });
    }

    fn n4n0(&self, command: N4n0Command) {
        self.0.send(&ToService::N4n0 { command });
    }

    fn updates(&self, action: UpdateAction) {
        self.0.send(&ToService::Updates { action });
    }

    fn quit(&self) {
        self.0.send(&ToService::Quit);
    }

    fn take_show(&self) -> bool {
        self.0.take_show()
    }
    fn take_show_page(&self) -> Option<String> {
        self.0.take_show_page()
    }

    fn set_background(&self, background: bool) {
        self.0.set_background(background);
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
    fn camera_qr(&self, active: bool) { self.ctl.camera_qr(active); }
    fn translate_status(&self, status: crate::state::TranslateStatus) { self.ctl.set_translate_status(status); }
    fn translate(&self, batch: orbiters_translate::provider::Batch) -> controller::translate::Answer { self.ctl.translate_remote(&batch) }
    fn switch_demo_avatar(&self) { self.ctl.switch_demo_avatar(); }
    fn vrchat_data(&self, scope: controller::vrchat_data::Scope, request: controller::vrchat_data::Request) -> controller::vrchat_data::Result {
        self.ctl.vrchat_data(scope, request)
    }
    fn vrchat_picture(&self, scope: controller::vrchat_data::Scope, id: String, path: PathBuf) {
        self.ctl.vrchat_picture(scope, id, path);
    }
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
        self.report(self.ctl.start_capture_from(crate::ipc::RecordingOrigin::OtherUi));
    }

    fn start_camera_recording(&self) {
        self.report(self.ctl.start_capture_from(crate::ipc::RecordingOrigin::CameraOverlay));
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

    fn patch_settings(&self, patch: serde_json::Value) {
        self.ctl.patch_settings(&patch);
    }

    fn trash_session(&self, dir: PathBuf) {
        let r = self.ctl.trash_session(&dir);
        if r.is_ok() {
            self.ctl.notify(NoticeKind::Success, tr!("Session moved to the {bin}.", bin = tr(crate::words::BIN)));
        }
        self.report(r);
    }

    fn clean_up(&self, trim: Vec<PathBuf>, delete: Vec<PathBuf>) {
        self.report(self.ctl.clean_up(trim, delete));
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

    fn sign_in(&self) {
        let ctl = self.ctl.clone();
        std::thread::spawn(move || {
            if let Err(e) = ctl.sign_in() {
                ctl.notify(crate::controller::NoticeKind::Error, tr!("Could not start the sign-in: {error}", error = e));
            }
        });
    }

    fn reopen_sign_in(&self) {
        self.report(self.ctl.reopen_sign_in());
    }

    fn bridge_command(&self, command: crate::bridge::Command) {
        self.ctl.bridge_command(command);
    }

    fn cancel_sign_in(&self) {
        self.ctl.cancel_sign_in();
    }

    fn sign_out(&self) {
        self.report(self.ctl.sign_out());
    }

    fn vrchat_login(&self, command: crate::controller::vrchat_login::Command) {
        self.ctl.vrchat_command(command);
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

    fn capture_share(&self, request: controller::shares::Request) {
        self.ctl.capture_share(request);
    }

    fn prepare_share(&self, session: PathBuf) {
        self.ctl.prepare_share(session);
    }

    fn frames(&self, chunk: crate::state::FrameChunk) {
        self.ctl.frames(chunk);
    }

    fn headset(&self, in_use: bool) {
        self.ctl.set_headset(Some(in_use));
    }

    fn walk_boost(&self, strength: f32) {
        self.ctl.set_walk_boost(strength, false);
    }

    fn osc_reconnect(&self) {
        self.ctl.osc_rediscover();
    }

    fn calibrated(&self, calibration: Calibration) {
        self.ctl.calibrated(calibration);
    }

    fn n4n0(&self, command: N4n0Command) {
        self.ctl.n4n0_command(command);
    }

    fn updates(&self, action: UpdateAction) {
        self.ctl.update_action(action);
    }

    fn quit(&self) {
        self.ctl.shutdown();
    }
}
