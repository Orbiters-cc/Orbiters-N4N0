//! The gallery's additional local picture sources. Folder I/O and the native folder picker stay on workers.
use super::super::{Cx, Job};
use egui::{Rect, Ui};
use orbiters_i18n::{mark, tr};
use orbiters_ui::{icons, widgets::{Button, Variant}};
use std::path::PathBuf;

/// The button's label (Enzo, 2026-10-30: "Manage folders", it was "Folders").
pub const LABEL: &str = mark("Manage folders");

/// What a look of the page's watch saw of what its list is made from: its revision, and when the look began. One look
/// is the same as another when the revision is: the watch then asks for no frame.
#[derive(Clone, Copy, Debug)]
struct Looked {
    at: std::time::Instant,
    revision: u64,
}

impl PartialEq for Looked {
    fn eq(&self, other: &Self) -> bool {
        self.revision == other.revision
    }
}

/// How often the popover looks whether each folder can be read.
const STATUS_EVERY: std::time::Duration = std::time::Duration::from_secs(10);
/// How often the picture folders are walked again while Photos & captures shows.
const RESCAN_EVERY: std::time::Duration = std::time::Duration::from_secs(15);

/// The native folder picker, run on a worker (it blocks until the member chooses).
fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().set_title(tr("Add a picture folder")).pick_folder()
}

#[derive(Default)]
pub struct State {
    open: bool,
    /// Shrinking back into its button: drawn (not clickable) until it landed.
    closing: bool,
    picker: Job<Option<PathBuf>>,
    /// Stands in for [`pick_folder`] (tests).
    pick: Option<fn() -> Option<PathBuf>>,
    /// Whether each folder can be read, looked at on a thread every [`STATUS_EVERY`] while the popover is open (a
    /// disk plugged in or gone): drawn again when that changed.
    status: super::super::Watch<Vec<PathBuf>, Vec<(PathBuf, bool)>>,
    folders: Vec<(PathBuf, bool)>,
    revision: u64,
    /// The folders walked again every [`RESCAN_EVERY`] while the page shows, on a thread, for the pictures taken
    /// meanwhile: the page is drawn again only when a walk found something (the pictures' revision changes). The
    /// same look gives the revision of what else the page's list is made from (the capture folders, the visit history,
    /// the upload records: `controller::shares::LocalRevision`), for the settings' picture folders, captures' folder and
    /// environment; and the last one seen ([`State::local_seen`]).
    rescan: super::super::Watch<(Vec<PathBuf>, PathBuf, bool), Looked>,
    local: Option<Looked>,
    /// The history and the upload records at these files, and no picture folder walked (tests).
    #[cfg(test)]
    sources: Option<(PathBuf, Option<PathBuf>)>,
    /// The folder scan was told which interface to wake.
    woken: bool,
    saved: Vec<PathBuf>,
    fixture: bool,
    /// Where the button was drawn last.
    button: Option<Rect>,
    /// Where Add folder was drawn last.
    add: Option<Rect>,
}
impl State {
    pub fn stage(&mut self) {
        self.open = true;
        self.fixture = true;
        self.folders = vec![(PathBuf::from("C:/Users/Member/Pictures/VRChat"), true), (PathBuf::from("I:/VRChat pics"), false)];
    }

    /// The fixture's folders, the popover left closed (a scenario opens it with a click).
    pub fn stage_closed(&mut self) {
        self.stage();
        self.open = false;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The visit history and the upload records at these files, and no picture folder walked (the page's tests).
    #[cfg(test)]
    pub fn set_sources(&mut self, history: PathBuf, records: Option<PathBuf>) {
        self.sources = Some((history, records));
    }

    /// The watch looks now, not at its next period (the page's tests).
    #[cfg(test)]
    pub fn look_now(&self) {
        self.rescan.look_now();
    }

    /// Where the button was drawn on the last frame.
    pub fn button(&self) -> Option<Rect> {
        self.button
    }

    /// The revision of what the page's list is made from beyond the pictures (a capture's folder, the visit history,
    /// the upload records) as the watch last saw it change, and when that look began: the page makes its list again
    /// when this is another revision than the one its list was made from, seen after it (`gallery::list_is_stale`).
    pub fn local_seen(&self) -> Option<(std::time::Instant, u64)> {
        self.local.map(|looked| (looked.at, looked.revision))
    }

    /// Whether the listing is to be made again (the scan found pictures, or the settings' folders changed), and whether
    /// pictures changed; and keeps the watch whose looks [`State::local_seen`] gives. Asks for no frame: the scan does
    /// when it finds something ([`crate::worlds::wake_on_pictures`]), and the watch when the rest changed.
    pub fn tick(&mut self, ui: &Ui, cx: &Cx) -> (bool, bool) {
        if !std::mem::replace(&mut self.woken, true) {
            crate::worlds::wake_on_pictures(ui.ctx());
        }
        let folders = &cx.snap.settings.picture_folders;
        #[cfg(not(test))]
        let sources: Option<(PathBuf, Option<PathBuf>)> = None;
        #[cfg(test)]
        let sources = self.sources.clone();
        // Made-up folders (screenshots, tests) are not walked.
        if !self.fixture || sources.is_some() {
            let (root, development) = (&cx.snap.settings.sessions_dir, cx.snap.settings.development);
            let key = (folders.clone(), root.clone(), development);
            let looked = self.rescan.keep(ui.ctx(), &key, RESCAN_EVERY, || {
                let (folders, root) = (folders.clone(), root.clone());
                let server = if development { orbiters_core::account::Server::Development } else { orbiters_core::account::Server::Production };
                let mut revision = crate::controller::shares::LocalRevision::default();
                move || {
                    // When the look began: what it sees is at least as new as that.
                    let at = std::time::Instant::now();
                    let revision = match &sources {
                        Some((history, records)) => revision.look_at(&root, history, records.as_deref()),
                        None => {
                            crate::worlds::rescan_pictures(&folders);
                            revision.look(&root, server)
                        }
                    };
                    Looked { at, revision }
                }
            });
            if looked.is_some() {
                self.local = looked;
            }
        }
        let revision = crate::worlds::picture_revision();
        let images_changed = revision != self.revision;
        let changed = images_changed || self.saved != *folders;
        if changed {
            self.revision = revision;
            self.saved = folders.clone();
        }
        self.take_picked(cx);
        (changed, images_changed)
    }

    /// The picker's folder, once chosen, saved among the settings' (once).
    fn take_picked(&mut self, cx: &Cx) {
        if let Some(Some(path)) = self.picker.take() {
            let mut folders = cx.snap.settings.picture_folders.clone();
            if !folders.iter().any(|p| p.to_string_lossy().eq_ignore_ascii_case(&path.to_string_lossy())) {
                folders.push(path);
                cx.ctl.patch_settings(serde_json::json!({"picture_folders": folders}));
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, cx: &Cx, anchor: Rect) {
        self.button = Some(anchor);
        // On the press, as the other buttons of the page's head (Manage captures).
        if super::super::sharing::pressed(Button::new(tr(LABEL)).icon(icons::FOLDER_OPEN).show_at(ui, anchor)) {
            if self.open {
                self.open = false;
                self.closing = true;
            } else {
                self.open = true;
                self.closing = false;
            }
        }
        if !self.open && !self.closing {
            return;
        }
        let id = egui::Id::unique("gallery-picture-folders");
        let width = 480.0_f32.min(ui.ctx().content_rect().width() - 32.0);
        let base = Rect::from_min_size(egui::pos2((anchor.right() - width).max(16.0), anchor.bottom() + 8.0), egui::vec2(width, 350.0));
        let pop = orbiters_ui::popup::popover(ui.ctx(), id, anchor, base, self.open);
        if self.closing && pop.gone {
            self.closing = false;
            return;
        }
        // Checked again when the settings' folders changed (a folder just added or removed: another watch), else every
        // ten seconds, on a thread. The popover rode on the page's ten frames a second to do it.
        let folders = &cx.snap.settings.picture_folders;
        if self.open && !self.fixture {
            let checked = self.status.keep(ui.ctx(), folders, STATUS_EVERY, || {
                let extra = folders.clone();
                move || {
                    orbiters_core::visited::screenshot_dirs_with(&extra).into_iter().map(|p| {
                        let online = std::fs::read_dir(&p).is_ok();
                        (p, online)
                    }).collect()
                }
            });
            if let Some(folders) = checked { self.folders = folders; }
        }
        let open = self.open;
        let shown = orbiters_ui::front::area(ui.ctx(), id, egui::Order::Foreground, open).fixed_pos(pop.rect.min).interactable(open).show(ui.ctx(), |ui| {
            ui.set_min_size(pop.rect.size());
            ui.multiply_opacity(pop.opacity);
            if !open {
                ui.disable();
            }
            orbiters_ui::widgets::panel(ui.painter(), pop.rect, 18.0);
            let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(pop.rect.shrink(20.0)));
            let ui = &mut inner;
            ui.heading(tr("Picture folders"));
            ui.add_space(8.0);
            ui.label(if cfg!(target_os = "macos") {
                tr("Pictures/VRChat is always included. Add the folders that hold your VRChat pictures to keep them in your library.")
            } else {
                tr("VRChat's picture folder is always included. Add archives to keep them in your library.")
            });
            ui.add_space(12.0);
            egui::ScrollArea::vertical().max_height(190.0).show(ui, |ui| {
                for (path, online) in &self.folders {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(path.to_string_lossy());
                            // VRChat does not run on a Mac, so its folder not being there says nothing of a disk gone away.
                            let added = cx.snap.settings.picture_folders.contains(path);
                            ui.weak(if *online {
                                tr("Available")
                            } else if cfg!(target_os = "macos") && !added {
                                tr("Not made yet: what you save here will show up")
                            } else {
                                tr("Offline: showing remembered pictures")
                            });
                        });
                        if cx.snap.settings.picture_folders.contains(path) && Button::new(tr("Remove")).variant(Variant::Ghost).show(ui).clicked() {
                            let folders: Vec<_> = cx.snap.settings.picture_folders.iter().filter(|p| *p != path).collect();
                            cx.ctl.patch_settings(serde_json::json!({"picture_folders": folders}));
                        }
                    });
                    ui.separator();
                }
            });
            if !self.fixture && self.status.seen().is_none() { ui.weak(tr("Checking folders...")); }
            if self.picker.running() {
                ui.weak(tr("Choose a folder in the window that opened..."));
            } else {
                let add = Button::new(tr("Add folder")).icon(icons::PLUS).show(ui);
                self.add = Some(add.rect);
                if add.clicked() {
                    let pick = self.pick.unwrap_or(pick_folder);
                    self.picker = Job::spawn(ui.ctx(), pick);
                }
            }
            ui.weak(tr("Removing a source leaves its original files untouched."));
        });
        // The press that opened it is outside it too (and so is its release, a click): the button's own presses toggle
        // it above, so a click elsewhere means one off both.
        let on_button = ui.input(|i| i.pointer.interact_pos().is_some_and(|p| anchor.contains(p)));
        let outside = shown.response.clicked_elsewhere() && !on_button;
        if open && (outside || ui.input(|i| i.key_pressed(egui::Key::Escape))) {
            self.open = false;
            self.closing = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Backend, Ctl};
    use crate::ipc::{N4n0Command, UpdateAction};
    use crate::state::Snapshot;
    use egui::{pos2, vec2, Event, Modifiers, PointerButton, RawInput};
    use orbiters_core::settings::Settings;
    use std::sync::{Arc, Mutex};

    /// The settings patches the popover sent.
    #[derive(Default)]
    struct Recorder {
        patches: Mutex<Vec<serde_json::Value>>,
    }

    impl Backend for Recorder {
        fn n4n0(&self, _command: N4n0Command) {}
        fn patch_settings(&self, patch: serde_json::Value) {
            self.patches.lock().unwrap().push(patch);
        }
        fn view(&self) -> Arc<Snapshot> {
            Arc::new(Snapshot::new(Settings::default()))
        }
        fn take_notices(&self) -> Vec<crate::controller::Notice> {
            Vec::new()
        }
        fn library_version(&self) -> u64 {
            0
        }
        fn attach(&self, _ctx: &egui::Context) {}
        fn start_capture(&self) {}
        fn stop_capture(&self) {}
        fn pause_capture(&self, _paused: bool) {}
        fn start_test_capture(&self) {}
        fn import_raw_capture(&self, _dir: PathBuf) {}
        fn trash_session(&self, _dir: PathBuf) {}
        fn clean_up(&self, _trim: Vec<PathBuf>, _delete: Vec<PathBuf>) {}
        fn reconstruct(&self, _dir: PathBuf) {}
        fn stop_reconstruction(&self) {}
        fn unqueue_reconstruction(&self, _dir: PathBuf) {}
        fn send_to_mixer(&self, _dir: PathBuf, _title: String) {}
        fn sign_in(&self) {}
        fn reopen_sign_in(&self) {}
        fn cancel_sign_in(&self) {}
        fn sign_out(&self) {}
        fn request_worker_log(&self) {}
        fn worker_log(&self) -> Option<Vec<String>> {
            None
        }
        fn upload_pictures(&self, _world_id: String, _world_name: String, _paths: Vec<PathBuf>) {}
        fn upload_preview(&self, _world_id: String, _world_name: String, _session: PathBuf) {}
        fn request_maps(&self) {}
        fn maps(&self) -> Option<crate::state::MapsMine> {
            None
        }
        fn calibrated(&self, _calibration: orbiters_contracts::performance::calibration::Calibration) {}
        fn frames(&self, _chunk: crate::state::FrameChunk) {}
        fn headset(&self, _in_use: bool) {}
        fn walk_boost(&self, _strength: f32) {}
        fn osc_reconnect(&self) {}
        fn updates(&self, _action: UpdateAction) {}
        fn quit(&self) {}
    }

    /// The thread the picker ran on.
    static PICKED_ON: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);

    fn fake_pick() -> Option<PathBuf> {
        *PICKED_ON.lock().unwrap() = Some(std::thread::current().id());
        Some(PathBuf::from("I:/VRChat archive"))
    }

    const BUTTON: Rect = Rect { min: pos2(900.0, 40.0), max: pos2(1040.0, 80.0) };

    struct Harness {
        ctx: egui::Context,
        recorder: Arc<Recorder>,
        ctl: Ctl,
        snap: Snapshot,
        state: State,
        time: f64,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            orbiters_ui::install(&ctx);
            let recorder = Arc::new(Recorder::default());
            let ctl: Ctl = recorder.clone();
            let mut state = State::default();
            state.stage_closed();
            state.pick = Some(fake_pick);
            Self { ctx, recorder, ctl, snap: Snapshot::new(Settings::default()), state, time: 0.0 }
        }

        fn frame(&mut self, events: Vec<Event>) {
            self.frame_at(self.time + 1.0 / 60.0, events);
        }

        /// A frame at `time`; how soon it asked for the next.
        fn frame_at(&mut self, time: f64, events: Vec<Event>) -> std::time::Duration {
            self.time = time;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1100.0, 760.0))), time: Some(self.time), events, ..Default::default() };
            let mut toasts = orbiters_ui::toast::Toasts::default();
            let (ctl, snap, state) = (&self.ctl, &self.snap, &mut self.state);
            let mut output = self.ctx.run_ui(input, |ui| {
                let cx = Cx { ctl, snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 40.0, go: None };
                state.tick(ui, &cx);
                state.show(ui, &cx, BUTTON);
            });
            output.textures_delta.clear();
            crate::pages::rest::asked(&output)
        }

        fn settle(&mut self) {
            for _ in 0..60 {
                self.frame(Vec::new());
            }
        }

        /// A real click: the pointer moves there, presses on one frame and releases on the next.
        fn click(&mut self, at: egui::Pos2) {
            self.frame(vec![Event::PointerMoved(at), button(at, true)]);
            self.frame(vec![button(at, false)]);
        }
    }

    fn button(at: egui::Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
    }

    /// Photos & captures asked for a frame every 100 ms to see whether the folder scan had found pictures (the
    /// architecture review's DUI-27): 600 frames a minute with nothing to show. The scan asks for the frame now.
    #[test]
    fn looking_for_new_pictures_asks_for_no_frame_and_the_scan_wakes_the_page() {
        let _pictures = crate::worlds::tests::pictures_lock();
        let mut h = Harness::new();
        assert_eq!(crate::pages::rest::frames_a_minute(0.0, |t| h.frame_at(t, Vec::new())), 0);
        // The scan found pictures: the page is asked to draw, and is told once that its listing changed.
        assert!(!h.ctx.has_requested_repaint());
        crate::worlds::tests::publish_for_test();
        assert!(h.ctx.has_requested_repaint(), "the scan wakes the page that looks at its pictures");
        let mut told = Vec::new();
        for _ in 0..3 {
            h.time += 1.0 / 60.0;
            let input = RawInput { time: Some(h.time), ..Default::default() };
            let mut toasts = orbiters_ui::toast::Toasts::default();
            let (ctl, snap, state) = (&h.ctl, &h.snap, &mut h.state);
            let mut output = h.ctx.run_ui(input, |ui| {
                let cx = Cx { ctl, snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 40.0, go: None };
                told.push(state.tick(ui, &cx));
            });
            output.textures_delta.clear();
        }
        assert_eq!(told, vec![(true, true), (false, false), (false, false)]);
    }

    /// The page made its whole list again every fifteen seconds, which also took in what changes outside the pictures'
    /// index. Made again only when the folder walk found pictures, it kept a ghost card for a capture another program
    /// had deleted, and did not file pictures under instances the visit history learned of meanwhile. The watch gives
    /// a revision of all the list reads: here the pictures' index never changes, and the revision the page is handed
    /// changes, with a frame asked for, exactly when a capture's folder, the visits or the upload records did. (That
    /// the page then makes its list again, and what the new list holds: `gallery`'s own test.)
    #[test]
    fn a_capture_deleted_elsewhere_new_visits_or_an_upload_change_the_revision_the_page_is_handed() {
        use orbiters_core::contracts::manifest::{SessionManifest, MANIFEST_FILE, MANIFEST_FORMAT};
        use orbiters_core::visited::{History, InstanceVisit};
        let root = std::env::temp_dir().join(format!("orbiters-gallery-revision-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let captures = root.join("Sessions");
        for name in ["2026-11-09 one", "2026-11-10 two"] {
            std::fs::create_dir_all(captures.join(name)).unwrap();
            let manifest = SessionManifest { format: MANIFEST_FORMAT.into(), ..Default::default() };
            std::fs::write(captures.join(name).join(MANIFEST_FILE), serde_json::to_vec(&manifest).unwrap()).unwrap();
        }
        let (history_file, records_file) = (root.join("visited-worlds.json"), root.join("capture-uploads.json"));
        let mut history = History::default();
        history.instances.push(InstanceVisit { world_id: "wrld_a".into(), instance: "1~private".into(), at: 100, account: None });
        history.save(&history_file).unwrap();

        let mut h = Harness::new();
        h.snap.settings.sessions_dir = captures.clone();
        h.state.set_sources(history_file.clone(), Some(records_file.clone()));
        let _pictures = crate::worlds::tests::pictures_lock();
        let pictures = crate::worlds::picture_revision();
        // One frame: whether the pictures or the folders say the list is to be made again, and the revision seen.
        let tick = |h: &mut Harness| -> ((bool, bool), Option<u64>) {
            h.time += 1.0 / 60.0;
            let input = RawInput { time: Some(h.time), ..Default::default() };
            let mut toasts = orbiters_ui::toast::Toasts::default();
            let (ctl, snap, state) = (&h.ctl, &h.snap, &mut h.state);
            let mut told = (false, false);
            let mut output = h.ctx.run_ui(input, |ui| {
                let cx = Cx { ctl, snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 40.0, go: None };
                told = state.tick(ui, &cx);
            });
            output.textures_delta.clear();
            (told, h.state.local_seen().map(|(_, revision)| revision))
        };
        let woken = |h: &Harness, what: &str| {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !h.ctx.has_requested_repaint() {
                assert!(std::time::Instant::now() < until, "{what}");
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        };
        // Frames until the page asks for no more (egui's own first frames ask for some): from then on a frame asked
        // for is the watch's.
        let rest = |h: &mut Harness| {
            for _ in 0..50 {
                if !h.ctx.has_requested_repaint() {
                    return;
                }
                tick(h);
            }
            panic!("the page does not come to rest");
        };
        // The first frame takes the pictures' revision as it stands (another test may have moved it before this one).
        tick(&mut h);
        let first = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while h.state.local_seen().is_none() {
            assert!(std::time::Instant::now() < first, "the first look");
            std::thread::sleep(std::time::Duration::from_millis(2));
            tick(&mut h);
        }
        rest(&mut h);
        let (_, before) = tick(&mut h);
        assert_eq!(crate::worlds::sessions(&captures).len(), 2);

        // Another program deletes a capture's folder: the page is woken, and handed another revision.
        std::fs::remove_dir_all(captures.join("2026-11-09 one")).unwrap();
        h.state.look_now();
        woken(&h, "a frame for the deleted capture");
        let (told, deleted) = tick(&mut h);
        assert_eq!(told, (false, false), "the pictures and the folders did not change");
        assert_ne!(deleted, before, "a capture's folder gone");
        rest(&mut h);

        // The service writes the history with nothing new but how far VRChat's log was read: no frame, the same revision.
        assert!(history.follow("output_log_2026-11-10_20-00-00.txt", 0, 4096, &[]));
        history.save(&history_file).unwrap();
        h.state.look_now();
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(!h.ctx.has_requested_repaint(), "the log read further is no change of the list");
        assert_eq!(tick(&mut h).1, deleted);
        rest(&mut h);

        // It learns of another instance joined (pictures are filed by instance): another revision, and a frame.
        history.instances.push(InstanceVisit { world_id: "wrld_b".into(), instance: "2~friends".into(), at: 200, account: None });
        history.save(&history_file).unwrap();
        h.state.look_now();
        woken(&h, "a frame for the new visit");
        let (_, visited) = tick(&mut h);
        assert_ne!(visited, deleted, "a new visit");
        rest(&mut h);

        // An upload recorded from elsewhere: the same.
        std::fs::write(&records_file, b"[]").unwrap();
        h.state.look_now();
        woken(&h, "a frame for the upload record");
        assert_ne!(tick(&mut h).1, visited, "an upload record");
        assert_eq!(crate::worlds::picture_revision(), pictures, "the pictures' index never changed");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_click_that_opens_it_leaves_it_open_and_add_folder_saves_the_picked_one() {
        let mut h = Harness::new();
        h.settle();
        assert!(!h.state.is_open());
        h.click(BUTTON.center());
        assert!(h.state.is_open(), "the opening click must not count as a click outside");
        h.settle();
        assert!(h.state.is_open(), "still open once it grew");

        // Add folder: the second-to-last row of the panel (under the list); found by its label.
        let add_at = h.state.add.expect("Add folder drawn").center();
        let ui_thread = std::thread::current().id();
        h.click(add_at);
        assert!(h.state.is_open(), "a click inside keeps it open");
        // The picker answers on its worker; the next frames save its folder.
        for _ in 0..200 {
            h.frame(Vec::new());
            if !h.recorder.patches.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let on = PICKED_ON.lock().unwrap().expect("the picker ran");
        assert_ne!(on, ui_thread, "the picker runs off the UI thread");
        let patches = h.recorder.patches.lock().unwrap().clone();
        assert_eq!(patches, vec![serde_json::json!({"picture_folders": ["I:/VRChat archive"]})]);

        // A click outside closes it; another on the button opens it again, and the button closes it.
        h.click(pos2(200.0, 600.0));
        assert!(!h.state.is_open(), "a click outside closes it");
        h.settle();
        h.click(BUTTON.center());
        assert!(h.state.is_open());
        h.settle();
        h.click(BUTTON.center());
        assert!(!h.state.is_open(), "its button closes it again");
    }
}
