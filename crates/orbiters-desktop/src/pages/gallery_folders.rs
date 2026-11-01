//! The gallery's additional local picture sources. Folder I/O and the native folder picker stay on workers.
use super::super::{Cx, Job};
use egui::{Rect, Ui};
use orbiters_ui::{icons, widgets::{Button, Variant}};
use std::path::PathBuf;

/// The button's label (Enzo, 2026-10-30: "Manage folders", it was "Folders").
pub const LABEL: &str = "Manage folders";

/// The native folder picker, run on a worker (it blocks until the member chooses).
fn pick_folder() -> Option<PathBuf> {
    rfd::FileDialog::new().set_title("Add a picture folder").pick_folder()
}

#[derive(Default)]
pub struct State {
    open: bool,
    /// Shrinking back into its button: drawn (not clickable) until it landed.
    closing: bool,
    picker: Job<Option<PathBuf>>,
    /// Stands in for [`pick_folder`] (tests).
    pick: Option<fn() -> Option<PathBuf>>,
    status: Job<Vec<(PathBuf, bool)>>,
    folders: Vec<(PathBuf, bool)>,
    /// The settings' folders the shown list was checked for.
    listed: Option<Vec<PathBuf>>,
    checked: f64,
    revision: u64,
    scanned: f64,
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

    /// Where the button was drawn on the last frame.
    pub fn button(&self) -> Option<Rect> {
        self.button
    }

    pub fn tick(&mut self, ui: &Ui, cx: &Cx) -> (bool, bool) {
        let now = ui.input(|i| i.time);
        let revision = crate::worlds::picture_revision();
        let images_changed = revision != self.revision;
        let changed = images_changed || self.saved != cx.snap.settings.picture_folders || now - self.scanned > 15.0;
        if changed {
            self.revision = revision;
            self.saved = cx.snap.settings.picture_folders.clone();
            self.scanned = now;
        }
        self.take_picked(cx);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        (changed, images_changed)
    }

    /// The picker's folder, once chosen, saved among the settings' (once).
    fn take_picked(&mut self, cx: &Cx) {
        if let Some(Some(path)) = self.picker.take() {
            let mut folders = cx.snap.settings.picture_folders.clone();
            if !folders.iter().any(|p| p.to_string_lossy().eq_ignore_ascii_case(&path.to_string_lossy())) {
                folders.push(path);
                cx.ctl.patch_settings(serde_json::json!({"picture_folders": folders}));
                self.checked = f64::NEG_INFINITY;
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, cx: &Cx, anchor: Rect) {
        self.button = Some(anchor);
        // On the press, as the other buttons of the page's head (Manage captures).
        if super::super::sharing::pressed(Button::new(LABEL).icon(icons::FOLDER_OPEN).show_at(ui, anchor)) {
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
        let now = ui.input(|i| i.time);
        // Checked again when the settings' folders changed (a folder just added or removed), else every ten seconds.
        let folders = &cx.snap.settings.picture_folders;
        let stale = self.listed.as_ref() != Some(folders);
        if self.open && !self.fixture && !self.status.running() && (stale || now - self.checked > 10.0) {
            self.checked = now;
            self.listed = Some(folders.clone());
            let extra = folders.clone();
            self.status = Job::spawn(ui.ctx(), move || {
                orbiters_core::visited::screenshot_dirs_with(&extra).into_iter().map(|p| {
                    let online = std::fs::read_dir(&p).is_ok();
                    (p, online)
                }).collect()
            });
        }
        if let Some(folders) = self.status.take() { self.folders = folders; }
        let open = self.open;
        let shown = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(pop.rect.min).interactable(open).show(ui.ctx(), |ui| {
            ui.set_min_size(pop.rect.size());
            ui.multiply_opacity(pop.opacity);
            if !open {
                ui.disable();
            }
            orbiters_ui::widgets::panel(ui.painter(), pop.rect, 18.0);
            let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(pop.rect.shrink(20.0)));
            let ui = &mut inner;
            ui.heading("Picture folders");
            ui.add_space(8.0);
            ui.label("VRChat's picture folder is always included. Add archives to keep them in your library.");
            ui.add_space(12.0);
            egui::ScrollArea::vertical().max_height(190.0).show(ui, |ui| {
                for (path, online) in &self.folders {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(path.to_string_lossy());
                            ui.weak(if *online { "Available" } else { "Offline: showing remembered pictures" });
                        });
                        if cx.snap.settings.picture_folders.contains(path) && Button::new("Remove").variant(Variant::Ghost).show(ui).clicked() {
                            let folders: Vec<_> = cx.snap.settings.picture_folders.iter().filter(|p| *p != path).collect();
                            cx.ctl.patch_settings(serde_json::json!({"picture_folders": folders}));
                            self.checked = f64::NEG_INFINITY;
                        }
                    });
                    ui.separator();
                }
            });
            if self.status.running() { ui.weak("Checking folders..."); }
            if self.picker.running() {
                ui.weak("Choose a folder in the window that opened...");
            } else {
                let add = Button::new("Add folder").icon(icons::PLUS).show(ui);
                self.add = Some(add.rect);
                if add.clicked() {
                    let pick = self.pick.unwrap_or(pick_folder);
                    self.picker = Job::spawn(ui.ctx(), pick);
                }
            }
            ui.weak("Removing a source leaves its original files untouched.");
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
            self.time += 1.0 / 60.0;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1100.0, 760.0))), time: Some(self.time), events, ..Default::default() };
            let mut toasts = orbiters_ui::toast::Toasts::default();
            let (ctl, snap, state) = (&self.ctl, &self.snap, &mut self.state);
            let mut output = self.ctx.run_ui(input, |ui| {
                let cx = Cx { ctl, snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 40.0, go: None };
                state.tick(ui, &cx);
                state.show(ui, &cx, BUTTON);
            });
            output.textures_delta.clear();
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
