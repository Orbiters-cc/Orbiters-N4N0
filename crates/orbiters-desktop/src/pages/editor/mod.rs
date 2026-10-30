//! The photo editor: Edit on a picture of Photos & captures (the picture window, [`super::sharing`]) or on a view of
//! a 3D capture saved as a picture ([`super::splats`]). The picture is edited as a copy: a stack of layers like
//! Lightroom's over it ([`orbiters_ui::photo`]), each shown, hidden, faded, moved up or down or removed, and for a VRChat
//! multi-layer picture a stack for each of its layers too (the people blurred apart from the world, the comic look on
//! the people only…). Crop and rotate with handles on the canvas, a straighten slider and shapes; curves with a curve
//! to drag per channel; the vignette's centre, a mask's centre and text dragged on the canvas. Undo and redo (Ctrl+Z,
//! Ctrl+Shift+Z), hold the eye to compare with the original. The preview is made on a thread of its own at about the
//! canvas's size ([`preview`]); Save makes the full picture and writes `<name> (edited).png` beside the original with
//! the edit next to it ([`files`]): opening that copy again edits it again, from the original, which is never changed.

mod canvas;
mod controls;
pub mod files;
mod layers;
mod preview;
mod widgets;

use super::Job;
pub use files::Source;
use egui::{pos2, vec2, Align2, Color32, Id, Rect, Sense, Ui, UiBuilder};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::photo::render::{self, Renderer, Stage};
use orbiters_ui::photo::{Channel, Edit, Layer, Rgba8};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{Button, IconButton, Size, Tone, Variant};
use orbiters_ui::{feedback, icons};
use preview::{Ask, Preview};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Pictures saved by the editor since the app started: a listing keyed by it shows a new copy at once.
static SAVED: AtomicU64 = AtomicU64::new(0);
/// Every opening gets springs of its own.
static OPENED: AtomicU64 = AtomicU64::new(0);

/// How many pictures the editor saved (Photos & captures lists its pictures again when it changes).
pub fn saved() -> u64 {
    SAVED.load(Ordering::Relaxed)
}

/// The picture of a Photos & captures entry, with VRChat's layers when it is a multi-layer picture; None for a capture
/// or an item only on Orbiters.
pub fn source_of(entry: &crate::controller::shares::Entry) -> Option<Source> {
    let Some(crate::controller::shares::Local::Picture(path)) = &entry.local else { return None };
    Some(Source::with_layers(path.clone(), entry.layers().iter().map(|l| (l.name.clone(), l.path.clone()))))
}

const TOP_H: f32 = 64.0;
const GAP: f32 = 16.0;

/// The layer list's and the controls' widths in a window `width` wide: the canvas gets the rest.
fn panel_widths(width: f32) -> (f32, f32) {
    ((width * 0.2).clamp(228.0, 280.0), (width * 0.23).clamp(270.0, 320.0))
}

/// What the panels and the canvas act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Selected {
    Nothing,
    /// Crop and rotate.
    Crop,
    /// One of a multi-layer picture's layers itself (its visibility and opacity).
    Source(usize),
    /// A layer of a stack (`None`: the whole picture's) by its id.
    Layer(Option<usize>, u32),
}

/// The picture, read and scaled for the preview.
struct Loaded {
    inputs: Arc<Vec<Rgba8>>,
    /// The first file's size.
    full: (u32, u32),
}

struct Saving {
    job: Job<Result<PathBuf, String>>,
    /// Thousandths done.
    progress: Arc<AtomicU32>,
}

/// Undo and redo: the edit as it was before each change. A change is one while a pointer is down (a slider or a handle
/// dragged) or while typing goes on.
#[derive(Default)]
struct History {
    past: Vec<Edit>,
    future: Vec<Edit>,
    committed: Edit,
    /// When the edit last changed.
    changed_at: f64,
    seen: Edit,
}

const HISTORY: usize = 200;

impl History {
    fn new(doc: &Edit) -> Self {
        Self { committed: doc.clone(), seen: doc.clone(), ..Default::default() }
    }

    /// Notes the edit at the end of a frame.
    fn settle(&mut self, doc: &Edit, holding: bool, now: f64) {
        if *doc != self.seen {
            self.seen = doc.clone();
            self.changed_at = now;
        }
        if *doc != self.committed && (!holding || now - self.changed_at > 1.0) {
            self.past.push(std::mem::replace(&mut self.committed, doc.clone()));
            if self.past.len() > HISTORY {
                self.past.remove(0);
            }
            self.future.clear();
        }
    }

    fn undo(&mut self, doc: &mut Edit) {
        self.settle(doc, false, self.changed_at);
        if let Some(prev) = self.past.pop() {
            self.future.push(std::mem::replace(&mut self.committed, prev.clone()));
            *doc = prev;
            self.seen = doc.clone();
        }
    }

    fn redo(&mut self, doc: &mut Edit) {
        self.settle(doc, false, self.changed_at);
        if let Some(next) = self.future.pop() {
            self.past.push(std::mem::replace(&mut self.committed, next.clone()));
            *doc = next;
            self.seen = doc.clone();
        }
    }
}

pub struct Editor {
    id: Id,
    source: Source,
    title: String,
    /// The copy saving writes (the one saved before, or the edited copy opened).
    target: Option<PathBuf>,
    doc: Edit,
    /// What the copy on disk has (nothing to save while the edit is the same).
    saved_doc: Option<Edit>,
    history: History,
    selected: Selected,
    channel: Channel,
    adding: bool,
    loading: Option<Job<Result<Loaded, String>>>,
    loaded: Option<Loaded>,
    error: Option<String>,
    preview: Option<Preview>,
    texture: Option<egui::TextureHandle>,
    /// What the texture shows, and its luma histogram.
    shown: Option<Ask>,
    histogram: Vec<u32>,
    saving: Option<Saving>,
    notice: Option<(String, Option<PathBuf>)>,
    fonts: controls::Fonts,
    canvas: canvas::State,
    rows: layers::State,
    /// The pressed eye: the original shown while it is held.
    comparing: bool,
    closing: Option<f64>,
    /// Close was asked with changes not saved: asked again, they are dropped.
    confirm_close: bool,
}

impl Editor {
    /// Edits `source` (an edited copy opens its edit, from its original), titled `title` (its world).
    pub fn open(ctx: &egui::Context, source: Source, title: impl Into<String>) -> Self {
        let (source, doc, target) = match files::reopen(&source.picture) {
            Some(saved) => {
                let copy = source.picture.clone();
                (saved.source, saved.edit, Some(copy))
            }
            None => {
                let edit = source.new_edit();
                (source, edit, None)
            }
        };
        let files = source.files();
        let c = ctx.clone();
        let loading = Job::spawn(ctx, move || load(&files).inspect(|_| c.request_repaint()));
        let serial = OPENED.fetch_add(1, Ordering::Relaxed);
        Self {
            id: Id::unique(("photo-editor", serial)),
            title: title.into(),
            target: target.clone(),
            saved_doc: target.is_some().then(|| doc.clone()),
            history: History::new(&doc),
            selected: Selected::Nothing,
            channel: Channel::Rgb,
            adding: false,
            loading: Some(loading),
            loaded: None,
            error: None,
            preview: None,
            texture: None,
            shown: None,
            histogram: Vec::new(),
            saving: None,
            notice: None,
            fonts: controls::Fonts::default(),
            canvas: canvas::State::default(),
            rows: layers::State::default(),
            comparing: false,
            closing: None,
            confirm_close: false,
            source,
            doc,
        }
    }

    /// The edit as it is (screenshots stage one).
    pub fn edit_mut(&mut self) -> &mut Edit {
        &mut self.doc
    }

    /// Selects `selected` (screenshots).
    pub(crate) fn select(&mut self, selected: Selected) {
        self.selected = selected;
    }

    /// Opens the add list (screenshots).
    pub fn show_add(&mut self) {
        self.adding = true;
    }

    /// Waits, at most `seconds`, until the picture is read and the preview asked last is made and shown (screenshots
    /// shoot the edit as staged).
    pub fn wait(&mut self, ctx: &egui::Context, seconds: f64) {
        let start = std::time::Instant::now();
        while start.elapsed().as_secs_f64() < seconds {
            self.poll(ctx);
            if self.error.is_some() || (self.loaded.is_some() && self.preview.as_ref().is_some_and(|p| !p.busy())) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn dirty(&self) -> bool {
        match &self.saved_doc {
            Some(s) => *s != self.doc,
            None => !self.doc.is_identity(),
        }
    }

    /// Starts closing; false while changes are not saved and this is the first time asked.
    fn close(&mut self, ctx: &egui::Context) {
        if self.dirty() && !self.confirm_close && self.saving.is_none() {
            self.confirm_close = true;
            return;
        }
        self.closing = Some(anim::time(ctx));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(job) = self.loading.as_mut() {
            if let Some(r) = job.take() {
                self.loading = None;
                match r {
                    Ok(l) => {
                        self.preview = Some(Preview::start(ctx, l.inputs.clone(), l.full));
                        self.loaded = Some(l);
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        }
        if let Some(made) = self.preview.as_ref().and_then(Preview::take) {
            let img = egui::ColorImage::from_rgba_unmultiplied([made.image.width as usize, made.image.height as usize], &made.image.pixels);
            match self.texture.as_mut() {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => self.texture = Some(ctx.load_texture(format!("photo-editor-{:?}", self.id), img, egui::TextureOptions::LINEAR)),
            }
            self.histogram = made.histogram;
            self.shown = Some(made.ask);
        }
        if let Some(s) = self.saving.as_mut() {
            if let Some(r) = s.job.take() {
                self.saving = None;
                match r {
                    Ok(path) => {
                        SAVED.fetch_add(1, Ordering::Relaxed);
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        self.notice = Some((format!("Saved as “{name}” beside the original: it shows in Photos & captures."), Some(path.clone())));
                        self.target = Some(path);
                        self.saved_doc = Some(self.doc.clone());
                        self.error = None;
                    }
                    Err(e) => self.error = Some(format!("The picture could not be saved: {e}")),
                }
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
    }

    /// What the canvas shows now.
    fn stage(&self) -> Stage {
        if self.comparing {
            Stage::Plain
        } else if self.selected == Selected::Crop {
            Stage::Uncropped
        } else {
            Stage::Full
        }
    }

    /// Saves the full picture as the copy.
    fn save(&mut self, ctx: &egui::Context) {
        if self.saving.is_some() || self.loaded.is_none() {
            return;
        }
        let target = self.target.clone().unwrap_or_else(|| files::copy_path(&self.source.picture));
        let progress = Arc::new(AtomicU32::new(0));
        let (p, edit, source) = (progress.clone(), self.doc.clone(), self.source.clone());
        let job = Job::spawn(ctx, move || save(&source, &edit, &target, &p).map(|_| target).map_err(|e| format!("{e:#}")));
        self.saving = Some(Saving { job, progress });
        self.notice = None;
    }

    /// Draws the editor over `rect`; false once it closed.
    pub fn show(&mut self, ui: &mut Ui, rect: Rect) -> bool {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        let now = anim::time(&ctx);
        let enter = anim::spring_from(&ctx, self.id.with("in"), [0.0], [1.0], Spring::SOFT)[0].clamp(0.0, 1.0);
        let leave = match self.closing {
            Some(at) => 1.0 - ((now - at) / 0.18).clamp(0.0, 1.0) as f32,
            None => 1.0,
        };
        if leave <= 0.0 {
            return false;
        }
        if self.closing.is_some() {
            ctx.request_repaint();
        }
        let alpha = enter.min(leave);
        // Over the page: nothing under it takes the pointer.
        let _ = ui.interact(rect, self.id.with("backdrop"), Sense::click_and_drag());
        ui.painter().rect_filled(rect, 0.0, theme::alpha(theme::INK_950, alpha));
        let rect = rect.translate(vec2(0.0, 14.0 * (1.0 - enter)));
        let mut ui = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(self.id.with("ui")));
        ui.set_clip_rect(rect.intersect(ui.clip_rect()));
        ui.multiply_opacity(alpha);
        if self.closing.is_some() {
            ui.disable();
        }

        let top = Rect::from_min_size(rect.min, vec2(rect.width(), TOP_H));
        let body = Rect::from_min_max(pos2(rect.left() + GAP, top.bottom()), pos2(rect.right() - GAP, rect.bottom() - GAP));
        let (left_w, right_w) = panel_widths(rect.width());
        let left = Rect::from_min_size(body.min, vec2(left_w, body.height()));
        let right = Rect::from_min_max(pos2(body.right() - right_w, body.top()), body.max);
        let canvas = Rect::from_min_max(pos2(left.right() + GAP, body.top()), pos2(right.left() - GAP, body.bottom()));

        self.top_bar(&mut ui, top);
        let title = self.title.clone();
        layers::show(self, &mut ui, left, &title);
        controls::show(self, &mut ui, right);
        canvas::show(self, &mut ui, canvas);
        self.keys(&ctx);

        let holding = ctx.input(|i| i.pointer.any_down()) || ctx.memory(|m| m.focused().is_some());
        self.history.settle(&self.doc, holding, now);
        self.ask(&ctx, canvas);
        true
    }

    /// Asks for the preview the canvas needs now.
    fn ask(&mut self, ctx: &egui::Context, canvas: Rect) {
        let stage = self.stage();
        let (Some(loaded), Some(preview)) = (self.loaded.as_ref(), self.preview.as_mut()) else { return };
        let ppp = ctx.pixels_per_point();
        let full = render::full_size(&self.doc, loaded.full, stage);
        // At most a pixel of the scaled picture a pixel of the canvas.
        let proxy = loaded.inputs.first().map_or(1.0, |p| p.width as f32 / loaded.full.0.max(1) as f32);
        let max_w = ((canvas.width() * ppp) as u32).min((full.0 as f32 * proxy).ceil() as u32).max(16);
        let max_h = ((canvas.height() * ppp) as u32).min((full.1 as f32 * proxy).ceil() as u32).max(16);
        let size = render::fit_size(full, max_w, max_h);
        preview.ask(Ask { edit: self.doc.clone(), stage, size });
    }

    fn keys(&mut self, ctx: &egui::Context) {
        let typing = ctx.memory(|m| m.focused().is_some());
        let (undo, redo, delete, escape) = ctx.input_mut(|i| {
            let redo = i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z) || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y);
            let undo = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
            let delete = !typing && i.key_pressed(egui::Key::Delete);
            let escape = i.key_pressed(egui::Key::Escape);
            (undo, redo, delete, escape)
        });
        if undo && !typing {
            self.history.undo(&mut self.doc);
        }
        if redo && !typing {
            self.history.redo(&mut self.doc);
        }
        if delete {
            layers::delete_selected(self);
        }
        if escape {
            if typing {
                ctx.memory_mut(|m| m.stop_text_input());
            } else if self.adding {
                self.adding = false;
            } else if self.selected != Selected::Nothing {
                self.selected = Selected::Nothing;
            } else {
                self.close(ctx);
            }
        }
    }

    fn top_bar(&mut self, ui: &mut Ui, bar: Rect) {
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();
        let mid = bar.center().y;
        let back = Rect::from_center_size(pos2(bar.left() + GAP + 20.0, mid), vec2(40.0, 40.0));
        if orbiters_ui::widgets::activated(&IconButton::new(icons::ARROW_LEFT, "Close the editor").size(40.0, 18.0).backdrop(theme::white(0.04)).show_at(ui, back)) {
            self.close(&ctx);
        }
        let x = back.right() + 14.0;
        orbiters_ui::widgets::text(&p, pos2(x, mid - 10.0), Align2::LEFT_CENTER, &Txt::caps(if self.target.is_some() { "Edited copy" } else { "Edit a copy" }));
        let name = self.source.picture.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let title = if self.title.is_empty() { name.clone() } else { format!("{} · {name}", self.title) };
        orbiters_ui::widgets::text_truncated(&p, pos2(x, mid + 9.0), Align2::LEFT_CENTER, &Txt::new(title, 15.0).weight(weight::SEMIBOLD), bar.width() * 0.34);

        // Undo, redo and compare in the middle.
        let c = pos2(bar.center().x, mid);
        let can_undo = !self.history.past.is_empty() || self.doc != self.history.committed;
        let can_redo = !self.history.future.is_empty();
        for (i, glyph, label, enabled) in [(-1.0, icons::ARROW_U_UP_LEFT, "Undo (Ctrl+Z)", can_undo), (0.0, icons::EYE, "Hold to see the original", true), (1.0, icons::ARROW_U_UP_RIGHT, "Redo (Ctrl+Shift+Z)", can_redo)] {
            let r = Rect::from_center_size(c + vec2(i * 46.0, 0.0), vec2(40.0, 40.0));
            let mut bu = ui.new_child(UiBuilder::new().max_rect(r).id_salt(("editor-history", label)));
            if !enabled {
                bu.disable();
                bu.multiply_opacity(0.35);
            }
            let resp = IconButton::new(glyph, label).size(40.0, 18.0).show_at(&mut bu, r);
            if i == 0.0 {
                self.comparing = resp.is_pointer_button_down_on();
            } else if orbiters_ui::widgets::activated(&resp) && enabled {
                if i < 0.0 {
                    self.history.undo(&mut self.doc);
                } else {
                    self.history.redo(&mut self.doc);
                }
            }
        }

        // Save on the right, saying how far it is.
        let saving = self.saving.as_ref().map(|s| s.progress.load(Ordering::Relaxed) as f32 / 1000.0);
        let label = match saving {
            Some(k) => format!("Saving {:.0} %", k * 100.0),
            None if self.target.is_some() => "Save".to_string(),
            None => "Save a copy".to_string(),
        };
        let enabled = self.loaded.is_some() && saving.is_none() && self.dirty();
        let b = Button::new(&label).icon(icons::FLOPPY_DISK).variant(Variant::Primary).enabled(enabled).id_salt("editor-save");
        let w = b.desired_width(ui).max(132.0);
        let r = Rect::from_min_size(pos2(bar.right() - GAP - w, mid - 20.0), vec2(w, 40.0));
        if super::sharing::pressed(b.width(w).show_at(ui, r)) {
            self.save(&ctx);
        }
        if let Some(k) = saving {
            let track = Rect::from_min_max(pos2(r.left() + 16.0, r.bottom() - 5.0), pos2(r.right() - 16.0, r.bottom() - 3.0));
            p.rect_filled(track, 1.0, theme::black(0.25));
            p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * k, track.height())), 1.0, theme::ON_ACCENT);
        }
        // What happened, beside it.
        let msg_right = r.left() - 12.0;
        let msg = if self.confirm_close && self.closing.is_none() {
            Some(("Not saved: close again to leave without saving", theme::WARN))
        } else if self.error.is_some() {
            Some(("Something went wrong (see the canvas)", theme::DANGER))
        } else {
            None
        };
        if let Some((m, color)) = msg {
            orbiters_ui::widgets::text_truncated(&p, pos2(msg_right, mid), Align2::RIGHT_CENTER, &Txt::new(m, 12.5).color(color), (msg_right - c.x - 90.0).max(40.0));
        }
        p.hline(bar.x_range(), bar.bottom() - 0.5, egui::Stroke::new(1.0, theme::white(0.05)));
    }

    /// The selected layer (of whichever stack).
    fn layer(&self) -> Option<&Layer> {
        let Selected::Layer(stack, id) = self.selected else { return None };
        self.doc.stack(stack).iter().find(|l| l.id == id)
    }


    /// The notice under the canvas: what saving did, with the copy to show in its folder.
    fn notice_ui(&mut self, ui: &mut Ui, at: Rect) {
        let ctx = ui.ctx().clone();
        let Some((text, path)) = self.notice.clone() else { return };
        let mut nu = ui.new_child(UiBuilder::new().max_rect(at).layout(egui::Layout::top_down(egui::Align::Min)).id_salt(self.id.with("notice")));
        if feedback::banner(&mut nu, self.id.with(("notice", &text)), Tone::Ok, &text, true) {
            self.notice = None;
        }
        if let Some(path) = path {
            nu.add_space(8.0);
            let b = Button::new("Show in its folder").icon(icons::FOLDER_OPEN).size(Size::Sm).id_salt("editor-reveal");
            let w = b.desired_width(&nu);
            let (r, _) = nu.allocate_exact_size(vec2(w, 32.0), Sense::hover());
            if super::sharing::pressed(b.show_at(&mut nu, r)) {
                if let Err(e) = files::reveal(&path) {
                    self.error = Some(e.to_string());
                }
            }
        }
        let _ = ctx;
    }
}

/// The layer `selected` names in `doc`.
pub(crate) fn find_mut(doc: &mut Edit, selected: Selected) -> Option<&mut Layer> {
    let Selected::Layer(stack, id) = selected else { return None };
    doc.stack_mut(stack).iter_mut().find(|l| l.id == id)
}

/// Reads the files and scales them for the preview.
fn load(files: &[PathBuf]) -> Result<Loaded, String> {
    let mut inputs = Vec::new();
    let mut full = (0, 0);
    for (i, f) in files.iter().enumerate() {
        let img = files::load(f).map_err(|e| format!("{} could not be read: {e:#}", f.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()))?;
        if i == 0 {
            full = (img.width, img.height);
        }
        inputs.push(img.fit(preview::PROXY));
    }
    Ok(Loaded { inputs: Arc::new(inputs), full })
}

/// Makes the full picture and writes the copy and its edit.
fn save(source: &Source, edit: &Edit, target: &std::path::Path, progress: &AtomicU32) -> anyhow::Result<()> {
    let files = source.files();
    let mut inputs = Vec::new();
    for f in &files {
        inputs.push(files::load(f)?);
    }
    progress.store(100, Ordering::Relaxed);
    let full = inputs.first().map(|i| (i.width, i.height)).unwrap_or((1, 1));
    let size = render::full_size(edit, full, Stage::Full);
    let refs: Vec<&Rgba8> = inputs.iter().collect();
    let img = Renderer::default()
        .render(edit, &refs, full, Stage::Full, size, &mut |k| {
            progress.store(100 + (k * 800.0) as u32, Ordering::Relaxed);
            true
        })
        .ok_or_else(|| anyhow::anyhow!("stopped"))?;
    drop(inputs);
    let rgba = img.to_rgba8();
    drop(img);
    progress.store(920, Ordering::Relaxed);
    let xmp = files::xmp_chunk(&source.picture).or_else(|| files.first().and_then(|f| files::xmp_chunk(f)));
    files::write_png(target, &rgba, xmp.as_deref())?;
    files::write_sidecar(target, &files::Saved::new(source.clone(), edit.clone()))?;
    progress.store(1000, Ordering::Relaxed);
    Ok(())
}

/// A colour for each kind of layer's icon tile.
fn tint(kind: orbiters_ui::photo::Kind) -> Color32 {
    use orbiters_ui::photo::Kind;
    match kind {
        Kind::Light => theme::WARN,
        Kind::Color => theme::VIOLET,
        Kind::Curves => theme::CYAN,
        Kind::Vignette => theme::FG_2,
        Kind::Aberration => theme::DANGER,
        Kind::Comic => theme::BLUE,
        Kind::Blur => theme::INDIGO,
        Kind::Text => theme::ACCENT,
    }
}

/// Each kind of layer's icon.
fn glyph(kind: orbiters_ui::photo::Kind) -> &'static str {
    use orbiters_ui::photo::Kind;
    match kind {
        Kind::Light => icons::SUN,
        Kind::Color => icons::PALETTE,
        Kind::Curves => icons::CHART_LINE,
        Kind::Vignette => icons::CIRCLE_HALF,
        Kind::Aberration => icons::CIRCLES_THREE,
        Kind::Comic => icons::DOTS_NINE,
        Kind::Blur => icons::DROP,
        Kind::Text => icons::TEXT_T,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_ui::photo::{Effect, Light};

    #[test]
    fn saving_writes_the_whole_copy_and_its_edit_and_leaves_the_original() {
        let dir = std::env::temp_dir().join(format!("orbiters-editor-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("VRChat_2026-10-29_21-04-11.532_64x32.png");
        let px = Rgba8::new(64, 32, (0..64 * 32).flat_map(|i| [(i % 64 * 4) as u8, (i / 64 * 8) as u8, 120, 255]).collect());
        let taken = time::OffsetDateTime::from_unix_timestamp(1_792_348_661).unwrap();
        files::write_png(&original, &px, Some(&files::xmp_for(Some("wrld_1"), Some("Rooftop"), taken))).unwrap();
        let source = Source::picture(&original);
        let mut edit = source.new_edit();
        edit.geometry.crop = orbiters_ui::photo::Crop { x: 0.5, y: 0.0, w: 0.5, h: 1.0 };
        for effect in [Effect::Light(Light { exposure: 0.5, ..Default::default() }), Effect::Text(orbiters_ui::photo::Text { value: "Hi".into(), ..Default::default() })] {
            let l = edit.layer(effect);
            edit.layers.push(l);
        }
        let target = files::copy_path(&original);
        let progress = AtomicU32::new(0);
        save(&source, &edit, &target, &progress).unwrap();
        assert_eq!(progress.load(Ordering::Relaxed), 1000);
        let copy = files::load(&target).unwrap();
        assert_eq!((copy.width, copy.height), (32, 32), "the crop at full size");
        assert_eq!(files::reopen(&target).map(|s| s.edit), Some(edit));
        assert_eq!(files::load(&original).unwrap(), px, "the original as it was");
        let info = orbiters_core::visited::read_shot(&target).unwrap();
        assert_eq!((info.world_id.as_deref(), info.taken), (Some("wrld_1"), 1_792_348_661));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn undo_steps_back_through_each_change_once_settled() {
        let mut doc = Edit::default();
        let mut h = History::new(&doc);
        let l = doc.layer(Effect::Light(Light::default()));
        doc.layers.push(l);
        h.settle(&doc, false, 1.0);
        // A drag: many changes while held make one step.
        for i in 0..10 {
            if let Effect::Light(l) = &mut doc.layers[0].effect {
                l.exposure = i as f32 * 0.1;
            }
            h.settle(&doc, true, 2.0 + i as f64 * 0.01);
        }
        h.settle(&doc, false, 2.5);
        assert_eq!(h.past.len(), 2);
        h.undo(&mut doc);
        assert!(matches!(&doc.layers[0].effect, Effect::Light(l) if l.exposure == 0.0));
        h.undo(&mut doc);
        assert!(doc.layers.is_empty());
        h.redo(&mut doc);
        h.redo(&mut doc);
        assert!(matches!(&doc.layers[0].effect, Effect::Light(l) if (l.exposure - 0.9).abs() < 1e-6));
        // A change after an undo drops what could be redone.
        h.undo(&mut doc);
        doc.layers.clear();
        h.settle(&doc, false, 3.0);
        assert!(h.future.is_empty());
    }
}
