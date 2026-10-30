//! The gallery's additional local picture sources. Folder I/O stays on a worker.
use super::super::{Cx, Job};
use egui::{Rect, Ui};
use orbiters_ui::{icons, widgets::{Button, Variant}};
use std::path::PathBuf;

#[derive(Default)]
pub struct State {
    open: bool,
    picker: Job<Option<PathBuf>>,
    status: Job<Vec<(PathBuf, bool)>>,
    folders: Vec<(PathBuf, bool)>,
    checked: f64,
    revision: u64,
    scanned: f64,
    saved: Vec<PathBuf>,
    fixture: bool,
}
impl State {
    pub fn stage(&mut self) {
        self.open = true;
        self.fixture = true;
        self.folders = vec![(PathBuf::from("C:/Users/Member/Pictures/VRChat"), true), (PathBuf::from("I:/VRChat pics"), false)];
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
        if let Some(Some(path)) = self.picker.take() {
            let mut folders = cx.snap.settings.picture_folders.clone();
            if !folders.iter().any(|p| p.to_string_lossy().eq_ignore_ascii_case(&path.to_string_lossy())) {
                folders.push(path);
                cx.ctl.patch_settings(serde_json::json!({"picture_folders": folders}));
                self.checked = f64::NEG_INFINITY;
            }
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        (changed, images_changed)
    }
    pub fn show(&mut self, ui: &mut Ui, cx: &Cx, anchor: Rect) {
        if Button::new("Folders").icon(icons::FOLDER_OPEN).show_at(ui, anchor).clicked() { self.open = !self.open; }
        if !self.open { return; }
        let now = ui.input(|i| i.time);
        if !self.fixture && !self.status.running() && (self.folders.is_empty() || now - self.checked > 10.0) {
            self.checked = now;
            self.status = Job::spawn(ui.ctx(), || orbiters_core::visited::screenshot_dirs().into_iter().map(|p| {
                let online = std::fs::read_dir(&p).is_ok(); (p, online)
            }).collect());
        }
        if let Some(folders) = self.status.take() { self.folders = folders; }
        let id = egui::Id::unique("gallery-picture-folders");
        let width = 480.0_f32.min(ui.ctx().content_rect().width() - 32.0);
        let base = Rect::from_min_size(egui::pos2((anchor.right() - width).max(16.0), anchor.bottom() + 8.0), egui::vec2(width, 350.0));
        let pop = orbiters_ui::popup::popover(ui.ctx(), id, anchor, base, true);
        let shown = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(pop.rect.min).show(ui.ctx(), |ui| {
            ui.set_min_size(pop.rect.size());
            ui.multiply_opacity(pop.opacity);
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
            if !self.picker.running() && Button::new("Add folder").icon(icons::PLUS).show(ui).clicked() {
                self.picker = Job::spawn(ui.ctx(), || rfd::FileDialog::new().set_title("Add a picture folder").pick_folder());
            }
            ui.weak("Removing a source leaves its original files untouched.");
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) || shown.response.clicked_elsewhere() { self.open = false; }
    }
}
