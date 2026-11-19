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
mod crop;
pub mod files;
mod layers;
mod looks;
mod preview;
mod widgets;

use super::Job;
pub use files::Source;
use orbiters_i18n::{mark, tr};
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

/// The picture of a Photos & captures entry, with VRChat's layers when it is a multi-layer picture, or a picture on
/// Orbiters only (shared with the member, or theirs from another computer), downloaded to edit; None for a capture.
pub fn source_of(entry: &crate::controller::shares::Entry) -> Option<Source> {
    use crate::controller::shares::Local;
    match &entry.local {
        Some(Local::Picture(path)) => Some(Source::with_layers(path.clone(), entry.layers().iter().map(|l| (l.name.clone(), l.path.clone())))),
        Some(Local::Session(_)) => None,
        None => {
            // A 3D capture's picture is its preview, not a picture to edit.
            if entry.remote.as_ref().is_some_and(|i| i.kind != orbiters_core::shares::Kind::Picture) {
                return None;
            }
            // Fetched as Photos & captures fetches what is dragged out of it: a shared picture's file, a gallery
            // picture's renewed link, an asset's public link.
            let (kind, asset, id, development, url) = match super::gallery_select::remote(entry, development())? {
                super::gallery_select::Remote::Shared { server, id } => (files::RemoteKind::Shared, 0, id, server == orbiters_core::account::Server::Development, String::new()),
                super::gallery_select::Remote::Gallery { server, asset, id, url } => (files::RemoteKind::Gallery, asset, id, server == orbiters_core::account::Server::Development, url),
                super::gallery_select::Remote::Link(url) => (files::RemoteKind::Link, 0, 0, development(), url),
            };
            let by = entry.remote.as_ref().map(|i| (i.from.as_ref().unwrap_or(&i.owner).name.clone(), i.mine)).filter(|(n, mine)| !n.is_empty() && !mine).map(|(n, _)| n);
            let world = (!entry.world_name.is_empty()).then(|| entry.world_name.clone());
            let name = [world.clone(), by.map(|b| format!("by {b}"))].into_iter().flatten().collect::<Vec<_>>().join(" ");
            Some(Source::remote(files::Remote {
                kind,
                asset,
                id,
                development,
                world_id: entry.world_id.starts_with("wrld_").then(|| entry.world_id.clone()),
                world_name: world,
                taken: entry.taken as i64,
                name,
                revision: files::Remote::revision_of(entry.remote.as_ref().and_then(|i| i.source_key.as_deref()), &url),
                url,
            }))
        }
    }
}

/// A picture of a community's or an event's gallery (`event`: the event it was taken at, with its world), downloaded
/// to edit; `development`: listed by the development environment's server.
pub fn gallery_source(p: &orbiters_core::community::Picture, event: Option<&orbiters_core::community::Card>, development: bool) -> Source {
    use time::format_description::well_known::Rfc3339;
    let world = event.and_then(|e| e.world.clone()).unwrap_or_default();
    let when = p.posted_at.as_deref().or(event.and_then(|e| e.starts_at.as_deref()));
    let taken = when.and_then(|t| time::OffsetDateTime::parse(t, &Rfc3339).ok()).map_or(0, |t| t.unix_timestamp());
    let by = p.author.as_ref().map(|a| a.username.clone()).filter(|n| !n.is_empty());
    let place = event.map(|e| e.title.clone()).filter(|t| !t.is_empty()).or_else(|| world.name.clone());
    let name = [place, by.map(|b| format!("by {b}"))].into_iter().flatten().collect::<Vec<_>>().join(" ");
    // An asset's picture (no gallery id) by its link alone.
    let url = p.full_url.clone().or(p.src.clone()).filter(|u| u.starts_with("https://")).unwrap_or_default();
    let kind = if p.id == 0 { files::RemoteKind::Link } else { files::RemoteKind::Gallery };
    let revision = files::Remote::revision_of(None, &url);
    Source::remote(files::Remote { kind, asset: p.asset_id, id: p.id, development, world_id: world.id, world_name: world.name, taken, name, url, revision })
}

/// Whether the app is on the development environment (what a shared picture's id is of).
fn development() -> bool {
    orbiters_core::settings::Settings::load().development
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
    /// The edit being written: what the copy holds once it is done (the person may go on editing meanwhile).
    doc: Edit,
    /// Its revision ([`Editor::revision`]): the one confirmed written once it is done.
    revision: u64,
}

/// The editor over a picture that is drawn live by its host instead of read from a file: the 3D view's, Edit in 3D
/// ([`super::splats`]). The canvas leaves the picture's place for the host to fill and hands it the pointer where no
/// handle takes it (the camera moves with it); saving is asked of the host.
pub struct Live {
    /// Where the canvas left the picture's place this frame.
    pub slot: Option<Slot>,
    /// The canvas's pointer where no handle took it.
    pub free: Option<egui::Response>,
    /// What the top bar asked for.
    pub asked: Option<LiveAsk>,
    /// A depth blur's focus distance as the host draws it now (metres; the auto focus's, eased).
    pub focus: Option<f32>,
    /// A manual focus distance is being set (its slider held), and whether the splats at it are tinted meanwhile.
    pub setting_focus: bool,
    pub show_plane: bool,
}

impl Live {
    fn new() -> Self {
        Self { slot: None, free: None, asked: None, focus: None, setting_focus: false, show_plane: true }
    }
}

/// The picture's place on the canvas: a shape to set ([`egui::Painter::set`]) on its layer, within its clip, the
/// rectangle the picture fills and what it shows.
pub struct Slot {
    pub layer: egui::LayerId,
    pub clip: Rect,
    pub shape: egui::layers::ShapeIdx,
    pub frame: Rect,
    pub stage: Stage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveAsk {
    /// The picture with its layers, at full size, into the gallery.
    Picture,
    /// The view without them (Save this view).
    View,
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
    /// What the copy on disk has.
    saved_doc: Option<Edit>,
    /// The edit's revision: one more at each change of it, undo and redo included (night4, verify2: an edit compared
    /// by value could come back equal to what was saved or armed, and lose what was between).
    revision: u64,
    /// The edit as it was at `revision` (a change is seen against it).
    tracked: Edit,
    /// The last revision confirmed written to the copy (`None`: none yet). Unsaved is the current revision differing
    /// from this one, and nothing else: a save under way is not written yet.
    written: Option<u64>,
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
    /// Crop and rotate's tools, view and dials.
    crop: crop::State,
    rows: layers::State,
    /// The pressed eye: the original shown while it is held.
    comparing: bool,
    closing: Option<f64>,
    /// Close was asked with changes not saved (the edit as it was then): asked again with the same edit, they are
    /// dropped. Edited or saved since, the next close asks again (it was armed once per editor, and the warning stayed).
    confirm_close: Option<u64>,
    /// Drawn live by its host (Edit in 3D).
    live: Option<Live>,
    /// The 3D view the copy opened was made of, and whether editing it there again was asked.
    view3d: Option<files::View3d>,
    to_3d: bool,
    /// The 3D view the copy's edit names, as read (its capture there or not): written back on each save, so a copy
    /// made in Edit in 3D keeps its way back there (a save from here dropped it).
    saved_view: Option<files::View3d>,
}

impl Editor {
    /// Edits `source` (an edited copy opens its edit, from its original), titled `title` (its world).
    pub fn open(ctx: &egui::Context, source: Source, title: impl Into<String>) -> Self {
        let (source, doc, target, view3d) = match files::reopen(&source.picture) {
            Some(saved) => {
                let copy = source.picture.clone();
                (saved.source, saved.edit, Some(copy), saved.view)
            }
            None => {
                let edit = source.new_edit();
                (source, edit, None, None)
            }
        };
        let files = source.files();
        let c = ctx.clone();
        let fetch = source.clone();
        let loading = Job::spawn(ctx, move || {
            // A picture of Orbiters is downloaded first.
            // (Never deletes the picture it was given, even unreadable: its path may come from a saved edit's file
            // and name any file; the verifier of night 2026-11-19 had it reverted.)
            files::fetch(&fetch).map_err(|e| tr!("The picture could not be downloaded: {error}", error = format!("{e:#}")))?;
            load(&files).inspect(|_| c.request_repaint())
        });
        let mut e = Self::with(source, title.into(), doc, target, Some(loading));
        e.saved_view = view3d.clone();
        e.view3d = view3d.filter(|v| v.capture.exists());
        e
    }

    /// Edits `doc` over a picture of `full` pixels that its host draws ([`Live`]): Edit in 3D.
    pub fn live(title: impl Into<String>, full: (u32, u32), doc: Edit) -> Self {
        let mut e = Self::with(Source::picture("3D view"), title.into(), doc, None, None);
        e.loaded = Some(Loaded { inputs: Arc::new(Vec::new()), full });
        e.live = Some(Live::new());
        e
    }

    fn with(source: Source, title: String, doc: Edit, target: Option<PathBuf>, loading: Option<Job<Result<Loaded, String>>>) -> Self {
        let serial = OPENED.fetch_add(1, Ordering::Relaxed);
        Self {
            id: Id::unique(("photo-editor", serial)),
            title,
            target: target.clone(),
            saved_doc: target.is_some().then(|| doc.clone()),
            revision: 0,
            tracked: doc.clone(),
            written: target.is_some().then_some(0),
            history: History::new(&doc),
            selected: Selected::Nothing,
            channel: Channel::Rgb,
            adding: false,
            loading,
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
            crop: crop::State::default(),
            rows: layers::State::default(),
            comparing: false,
            closing: None,
            confirm_close: None,
            live: None,
            view3d: None,
            saved_view: None,
            to_3d: false,
            source,
            doc,
        }
    }

    /// What its host draws and is asked for ([`Editor::live`]).
    pub fn live_mut(&mut self) -> Option<&mut Live> {
        self.live.as_mut()
    }

    /// The edit as it is.
    pub fn edit(&self) -> &Edit {
        &self.doc
    }

    /// Saving the host started for [`LiveAsk`]: its progress shows on Save, its result under the canvas.
    pub fn start_saving(&mut self, job: Job<Result<PathBuf, String>>, progress: Arc<AtomicU32>) {
        let revision = self.track();
        self.saving = Some(Saving { job, progress, doc: self.doc.clone(), revision });
        self.notice = None;
    }

    /// It edits a picture of Orbiters (downloaded with the account), not one of this computer.
    pub fn is_remote(&self) -> bool {
        self.source.remote.is_some()
    }

    pub fn saving(&self) -> bool {
        self.saving.is_some()
    }

    /// Editing in its 3D view again was asked (a copy saved from Edit in 3D): the view and the edit, once.
    pub fn take_3d(&mut self) -> Option<(files::View3d, Edit)> {
        if !std::mem::take(&mut self.to_3d) {
            return None;
        }
        self.view3d.clone().map(|v| (v, self.doc.clone()))
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

    /// Crop and rotate with the dial named `dial` chosen, its shapes shown or not, and the picture levelled by AUTO
    /// (once the picture is read: screenshots).
    pub(crate) fn stage_crop(&mut self, dial: &str, shapes: bool, auto: bool) {
        self.selected = Selected::Crop;
        crop::stage(self, dial, shapes, auto);
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

    /// The edit's revision now: a change since the last look makes a new one.
    fn track(&mut self) -> u64 {
        if self.tracked != self.doc {
            self.revision += 1;
            self.tracked = self.doc.clone();
        }
        self.revision
    }

    fn dirty(&self) -> bool {
        // A live picture's edit stays with its host when the editor closes.
        if self.live.is_some() {
            return false;
        }
        // Changed since the last look: a revision of its own, not written.
        let revision = if self.tracked != self.doc { self.revision + 1 } else { self.revision };
        match self.written {
            Some(written) => written != revision,
            // Never written: only an edit that does something is lost.
            None => !self.doc.is_identity(),
        }
    }

    /// Starts closing; asks first while the edit on screen is not confirmed written (a save under way included: it
    /// writes what it was given, and may fail), once for each revision.
    fn close(&mut self, ctx: &egui::Context) {
        let revision = self.track();
        if self.dirty() && self.confirm_close != Some(revision) {
            self.confirm_close = Some(revision);
            return;
        }
        self.closing = Some(anim::time(ctx));
    }

    /// Tests: the edit on screen as if written to the copy.
    #[cfg(test)]
    fn written_now(&mut self) {
        self.written = Some(self.track());
        self.saved_doc = Some(self.doc.clone());
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(job) = self.loading.as_mut() {
            // A reading whose worker ended without a word (it panicked) ends as an error, not as a picture for ever
            // loading.
            let r = job.take().or_else(|| job.died().then(|| Err(tr("The picture could not be read.").to_string())));
            if let Some(r) = r {
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
            // A save whose worker ended without a word (it panicked) ends as a failed save: Save works again.
            let r = s.job.take().or_else(|| s.job.died().then(|| Err(tr("it stopped unexpectedly").to_string())));
            if let Some(r) = r {
                let written = self.saving.take().map(|s| (s.doc, s.revision));
                match r {
                    Ok(path) if self.live.is_some() => {
                        // Each save of a live picture is a picture of its own.
                        SAVED.fetch_add(1, Ordering::Relaxed);
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        self.notice = Some((tr!("Saved as “{name}”: it shows in Photos & captures.", name = name), Some(path)));
                        self.error = None;
                    }
                    Ok(path) => {
                        SAVED.fetch_add(1, Ordering::Relaxed);
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        // A sentence for each place, never a piece put into one.
                        let text = if self.source.remote.is_some() {
                            tr!("Saved as “{name}” in your VRChat pictures: it shows in Photos & captures.", name = name)
                        } else {
                            tr!("Saved as “{name}” beside the original: it shows in Photos & captures.", name = name)
                        };
                        self.notice = Some((text, Some(path.clone())));
                        self.target = Some(path);
                        // What the copy holds is the edit that was written, not what is on screen now (edited during
                        // the save, it is not saved: Back asks again).
                        if let Some((doc, revision)) = written {
                            self.saved_doc = Some(doc);
                            self.written = Some(revision);
                        }
                        // A close armed before the save asked about what was not written then.
                        self.confirm_close = None;
                        self.error = None;
                    }
                    Err(e) => self.error = Some(tr!("The picture could not be saved: {error}", error = e)),
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
        if let Some(live) = self.live.as_mut() {
            live.asked = Some(LiveAsk::Picture);
            return;
        }
        let target = self.target.clone().unwrap_or_else(|| self.source.copy_path());
        let progress = Arc::new(AtomicU32::new(0));
        let (p, edit, source, view) = (progress.clone(), self.doc.clone(), self.source.clone(), self.saved_view.clone());
        let job = Job::spawn(ctx, move || save(&source, &edit, &target, view.as_ref(), &p).map(|_| target).map_err(|e| format!("{e:#}")));
        let revision = self.track();
        self.saving = Some(Saving { job, progress, doc: self.doc.clone(), revision });
        self.notice = None;
    }

    /// Draws the editor over `rect`; false once it closed.
    pub fn show(&mut self, ui: &mut Ui, rect: Rect) -> bool {
        let ctx = ui.ctx().clone();
        self.track();
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
        if self.selected != Selected::Crop {
            self.crop.leave();
        }
        self.keys(&ctx);

        let holding = ctx.input(|i| i.pointer.any_down()) || ctx.memory(|m| m.focused().is_some()) || self.crop.moving();
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
        // At most a pixel of the scaled picture a pixel of the canvas; the crop's picture as large as it shows zoomed
        // in, in steps (a zoom does not ask for a new picture every frame).
        let proxy = loaded.inputs.first().map_or(1.0, |p| p.width as f32 / loaded.full.0.max(1) as f32);
        let shown = match (stage, self.crop.shown_size()) {
            (Stage::Uncropped, Some(s)) => {
                let k = (s.x / canvas.width().max(1.0)).max(s.y / canvas.height().max(1.0)).max(1.0);
                canvas.size() * 2f32.powf((k.log2() * 2.0).ceil() / 2.0)
            }
            _ => canvas.size(),
        };
        let max_w = ((shown.x * ppp) as u32).min((full.0 as f32 * proxy).ceil() as u32).max(16);
        let max_h = ((shown.y * ppp) as u32).min((full.1 as f32 * proxy).ceil() as u32).max(16);
        let size = render::fit_size(full, max_w, max_h);
        preview.ask(Ask { edit: self.doc.clone(), stage, size });
    }

    fn keys(&mut self, ctx: &egui::Context) {
        let typing = ctx.memory(|m| m.focused().is_some());
        // An open menu (Looks, Crop's More) takes Escape: it closes itself and nothing behind it.
        let menu = orbiters_ui::popup::any_open(ctx);
        let (undo, redo, delete, escape) = ctx.input_mut(|i| {
            let redo = i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z) || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y);
            let undo = i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
            // A Mac keyboard's delete key sends Backspace.
            let delete = !typing && (i.key_pressed(egui::Key::Delete) || (cfg!(target_os = "macos") && i.key_pressed(egui::Key::Backspace)));
            let escape = !menu && i.key_pressed(egui::Key::Escape);
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
        if orbiters_ui::widgets::activated(&IconButton::new(icons::ARROW_LEFT, mark("Close the editor")).size(40.0, 18.0).backdrop(theme::white(0.04)).show_at(ui, back)) {
            self.close(&ctx);
        }
        let x = back.right() + 14.0;
        let caps = if self.live.is_some() {
            tr("Edit in 3D")
        } else if self.target.is_some() {
            tr("Edited copy")
        } else {
            tr("Edit a copy")
        };
        orbiters_ui::widgets::text(&p, pos2(x, mid - 10.0), Align2::LEFT_CENTER, &Txt::caps(caps));
        let name = match &self.source.remote {
            // A picture of Orbiters: its name, not its download's.
            Some(r) if !r.name.is_empty() => r.name.clone(),
            _ => self.source.picture.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        };
        let title = if self.title.is_empty() || name.starts_with(&self.title) { name.clone() } else { format!("{} · {name}", self.title) };
        orbiters_ui::widgets::text_truncated(&p, pos2(x, mid + 9.0), Align2::LEFT_CENTER, &Txt::new(title, 15.0).weight(weight::SEMIBOLD), bar.width() * 0.34);

        // Undo, redo and compare in the middle.
        let c = pos2(bar.center().x, mid);
        let can_undo = !self.history.past.is_empty() || self.doc != self.history.committed;
        let can_redo = !self.history.future.is_empty();
        for (i, glyph, label, enabled) in [(-1.0, icons::ARROW_U_UP_LEFT, if cfg!(target_os = "macos") { mark("Undo (Cmd+Z)") } else { mark("Undo (Ctrl+Z)") }, can_undo), (0.0, icons::EYE, mark("Hold to see the original"), true), (1.0, icons::ARROW_U_UP_RIGHT, if cfg!(target_os = "macos") { mark("Redo (Cmd+Shift+Z)") } else { mark("Redo (Ctrl+Shift+Z)") }, can_redo)] {
            let r = Rect::from_center_size(c + vec2(i * 46.0, 0.0), vec2(40.0, 40.0));
            let mut bu = ui.new_child(UiBuilder::new().max_rect(r).id_salt(("editor-history", label)));
            if !enabled {
                bu.disable();
                bu.multiply_opacity(0.35);
            }
            // Its id from the English label: an open tooltip keeps its state when the language changes.
            let resp = IconButton::new(glyph, label).size(40.0, 18.0).id_salt(label).show_at(&mut bu, r);
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
            Some(k) => tr!("Saving {percent}", percent = format!("{:.0} %", k * 100.0)),
            None if self.live.is_some() => tr("Save picture").to_string(),
            None if self.target.is_some() => tr("Save").to_string(),
            None => tr("Save a copy").to_string(),
        };
        let enabled = self.loaded.is_some() && saving.is_none() && (self.dirty() || self.live.is_some());
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
        // Looks, Save this view (live) or Edit in 3D (a copy made there), left of it.
        let msg_right = self.more_buttons(ui, r) - 12.0;
        let msg = if self.confirm_close == Some(self.revision) && self.tracked == self.doc && self.dirty() && self.closing.is_none() {
            Some((tr("Not saved: close again to leave without saving"), theme::WARN))
        } else if self.error.is_some() {
            Some((tr("Something went wrong (see the canvas)"), theme::DANGER))
        } else {
            None
        };
        if let Some((m, color)) = msg {
            orbiters_ui::widgets::text_truncated(&p, pos2(msg_right, mid), Align2::RIGHT_CENTER, &Txt::new(m, 12.5).color(color), (msg_right - c.x - 90.0).max(40.0));
        }
        p.hline(bar.x_range(), bar.bottom() - 0.5, egui::Stroke::new(1.0, theme::white(0.05)));
    }

    /// The buttons left of Save (`save`): Looks, then Save this view for a live picture or Edit in 3D for a copy saved
    /// from one. Returns where they start.
    fn more_buttons(&mut self, ui: &mut Ui, save: Rect) -> f32 {
        let mut left = save.left() - 10.0;
        let busy = self.saving.is_some();
        if self.live.is_some() {
            let b = Button::new(tr("Save this view")).icon(icons::CAMERA).variant(Variant::Glass).enabled(!busy).hint(tr("Saves the view as it is, without the layers")).id_salt("editor-3d-view");
            let w = b.desired_width(ui);
            let r = Rect::from_min_size(pos2(left - w, save.top()), vec2(w, save.height()));
            if super::sharing::pressed(b.show_at(ui, r)) && !busy {
                if let Some(live) = self.live.as_mut() {
                    live.asked = Some(LiveAsk::View);
                }
            }
            left = r.left() - 10.0;
        } else if self.view3d.is_some() {
            let b = Button::new(tr("Edit in 3D")).icon(icons::CUBE).variant(Variant::Glass).hint(tr("Opens its capture's 3D view where it was made, with these layers")).id_salt("editor-to-3d");
            let w = b.desired_width(ui);
            let r = Rect::from_min_size(pos2(left - w, save.top()), vec2(w, save.height()));
            if super::sharing::pressed(b.show_at(ui, r)) {
                self.to_3d = true;
            }
            left = r.left() - 10.0;
        }
        let r = Rect::from_min_size(pos2(left - 40.0, save.center().y - 20.0), vec2(40.0, 40.0));
        looks::menu(self, ui, r);
        r.left()
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
            let b = Button::new(tr("Show in its folder")).icon(icons::FOLDER_OPEN).size(Size::Sm).id_salt("editor-reveal");
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
        let img = files::load(f).map_err(|e| tr!("{file} could not be read: {error}", file = f.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(), error = format!("{e:#}")))?;
        if i == 0 {
            full = (img.width, img.height);
        }
        inputs.push(img.fit(preview::PROXY));
    }
    Ok(Loaded { inputs: Arc::new(inputs), full })
}

/// Makes the full picture and writes the copy and its edit.
fn save(source: &Source, edit: &Edit, target: &std::path::Path, view: Option<&files::View3d>, progress: &AtomicU32) -> anyhow::Result<()> {
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
    let xmp = source.xmp();
    // A picture of Orbiters' copy goes into a month's folder that may not be there yet.
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    files::write_png(target, &rgba, xmp.as_deref())?;
    files::write_sidecar(target, &files::Saved { view: view.cloned(), ..files::Saved::new(source.clone(), edit.clone()) })?;
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
        save(&source, &edit, &target, None, &progress).unwrap();
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
    fn a_copy_saved_in_3d_asks_for_its_view_again_and_a_live_editor_asks_its_host_to_save() {
        let dir = std::env::temp_dir().join(format!("orbiters-editor-3d-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let capture = dir.join("Silent Dawn");
        std::fs::create_dir_all(&capture).unwrap();
        let original = dir.join("Orbiters 3D view 2026-10-30 12-00-00.png");
        let px = Rgba8::new(4, 2, vec![90; 32]);
        files::write_png(&original, &px, None).unwrap();
        let copy = files::copy_path(&original);
        files::write_png(&copy, &px, None).unwrap();
        let view = files::View3d { capture: capture.clone(), yaw: 0.5, distance: 3.0, fov_y: 1.0, size: [4, 2], ..Default::default() };
        let mut edit = Edit::default();
        let l = edit.layer(Effect::Light(Light { exposure: 0.5, ..Default::default() }));
        edit.layers.push(l);
        files::write_sidecar(&copy, &files::Saved { view: Some(view.clone()), ..files::Saved::new(Source::picture(&original), edit.clone()) }).unwrap();
        let ctx = egui::Context::default();
        let mut e = Editor::open(&ctx, Source::picture(&copy), "Silent Dawn");
        assert_eq!(e.take_3d(), None, "not asked");
        e.to_3d = true;
        assert_eq!(e.take_3d(), Some((view, edit.clone())));
        assert_eq!(e.take_3d(), None, "once");
        // Its capture gone, there is no 3D view to go back to.
        std::fs::remove_dir_all(&capture).unwrap();
        let mut e = Editor::open(&ctx, Source::picture(&copy), "Silent Dawn");
        e.to_3d = true;
        assert!(e.take_3d().is_none());
        // Night PG2-6: saved again from here (its capture gone or not), the copy keeps its way back to 3D.
        let target = files::copy_path(&original);
        save(&Source::picture(&original), &edit, &target, e.saved_view.as_ref(), &AtomicU32::new(0)).unwrap();
        assert_eq!(files::reopen(&target).and_then(|s| s.view).map(|v| v.capture), Some(capture.clone()), "the view is kept");
        // Live, saving is asked of the host, and closing keeps nothing to confirm (the host keeps the layers).
        let mut live = Editor::live("Silent Dawn", (2560, 1440), edit);
        assert!(!live.dirty());
        live.save(&ctx);
        assert_eq!(live.live_mut().and_then(|l| l.asked), Some(LiveAsk::Picture));
        assert!(live.saving.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Verifier (night3, point 3): a picture of Orbiters whose file cannot be read is an error; the file is never
    /// deleted (its path can come from a saved edit's file and name anything).
    #[test]
    fn an_unreadable_picture_is_never_deleted() {
        use files::{Remote, RemoteKind};
        let ctx = egui::Context::default();
        let path = std::env::temp_dir().join(format!("orbiters-editor-unreadable-{}.img", std::process::id()));
        std::fs::write(&path, b"not a picture").unwrap();
        let mut source = Source::remote(Remote { kind: RemoteKind::Gallery, id: 12, ..Default::default() });
        source.picture = path.clone();
        let mut e = Editor::open(&ctx, source, "World");
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while e.loading.is_some() {
            assert!(std::time::Instant::now() < until, "the reading never ended");
            e.poll(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(e.error.is_some(), "it says the picture could not be read");
        assert!(path.is_file(), "the file it was given is still there");
        let _ = std::fs::remove_file(path);
    }

    /// Verifier (night3, points 2 and 4): a save marks as saved the edit it wrote, not the one on screen when it ends
    /// (edited meanwhile, Back asks again); a save whose worker panicked ends as a failed save (Save works again); a
    /// close armed before a save does not stand for the edit after it.
    #[test]
    fn a_save_marks_saved_what_it_wrote_and_a_panicked_save_ends() {
        let ctx = egui::Context::default();
        let path = std::env::temp_dir().join(format!("orbiters-editor-saving-{}.png", std::process::id()));
        let mut e = Editor::open(&ctx, Source::picture(&path), "World");
        let with = |exposure: f32| {
            let mut doc = Edit::default();
            let l = doc.layer(Effect::Light(Light { exposure, ..Default::default() }));
            doc.layers.push(l);
            doc
        };
        let finish = |e: &mut Editor| {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while e.saving.is_some() {
                assert!(std::time::Instant::now() < until, "the save never ended");
                e.poll(&ctx);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        };
        // A is being written; the person edits to B meanwhile.
        e.doc = with(0.5);
        let written = path.clone();
        e.saving = Some(Saving { job: Job::spawn(&ctx, move || Ok(written)), progress: Arc::default(), doc: e.doc.clone(), revision: e.track() });
        e.doc = with(0.2);
        finish(&mut e);
        assert_eq!(e.saved_doc, Some(with(0.5)), "what the copy holds");
        assert!(e.dirty(), "B is not saved");
        e.close(&ctx);
        assert!(e.closing.is_none(), "Back asks first");
        // A save of B, with the close armed for B: once saved, the old question does not stand.
        let written = path.clone();
        e.saving = Some(Saving { job: Job::spawn(&ctx, move || Ok(written)), progress: Arc::default(), doc: e.doc.clone(), revision: e.track() });
        finish(&mut e);
        assert!(!e.dirty() && e.confirm_close.is_none());
        // A save whose worker panics.
        e.doc = with(0.9);
        e.saving = Some(Saving { job: Job::spawn(&ctx, || -> Result<PathBuf, String> { panic!("a writer's own unwrap") }), progress: Arc::default(), doc: e.doc.clone(), revision: e.track() });
        finish(&mut e);
        assert!(e.error.is_some(), "it says the save failed");
        assert!(e.dirty(), "and the edit is still unsaved");
    }

    /// Night4, verify2 PG2-4: Back pressed while a save writes A, with B on screen, asks (B closed silently: the guard
    /// stood aside for any save under way); and a close armed for an edit does not stand for the same edit come back by
    /// undo after another change. Unsaved is the revision on screen differing from the last one confirmed written.
    #[test]
    fn back_during_a_save_asks_and_an_armed_close_is_for_one_revision() {
        let ctx = egui::Context::default();
        let path = std::env::temp_dir().join(format!("orbiters-editor-back-{}.png", std::process::id()));
        let mut e = Editor::open(&ctx, Source::picture(&path), "World");
        let with = |exposure: f32| {
            let mut doc = Edit::default();
            let l = doc.layer(Effect::Light(Light { exposure, ..Default::default() }));
            doc.layers.push(l);
            doc
        };
        // A is being written (held); the person edits to B and presses Back before it ends.
        e.doc = with(0.5);
        let (finish, held) = std::sync::mpsc::channel::<()>();
        let written = path.clone();
        e.saving = Some(Saving {
            job: Job::spawn(&ctx, move || {
                let _ = held.recv();
                Ok(written)
            }),
            progress: Arc::default(),
            doc: e.doc.clone(),
            revision: e.track(),
        });
        e.doc = with(0.2);
        e.close(&ctx);
        assert!(e.closing.is_none(), "B is not written: Back asks, even with a save under way");
        // With A on screen alone, Back during its save asks too: A is not confirmed written yet.
        let mut a = Editor::open(&ctx, Source::picture(&path), "World");
        a.doc = with(0.5);
        let r = a.track();
        a.saving = Some(Saving { job: Job::spawn(&ctx, || -> Result<PathBuf, String> { Err("held".into()) }), progress: Arc::default(), doc: a.doc.clone(), revision: r });
        a.close(&ctx);
        assert!(a.closing.is_none(), "a save under way is not a save");
        drop(finish);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while e.saving.is_some() {
            assert!(std::time::Instant::now() < until);
            e.poll(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(e.dirty(), "A written, B not");
        // Armed on B; then C, then undo back to B's value: a new revision, asked again.
        e.close(&ctx);
        assert!(e.closing.is_none());
        e.close(&ctx);
        assert!(e.closing.is_some(), "asked twice for the same revision: it closes");
        let mut e = Editor::open(&ctx, Source::picture(&path), "World");
        e.doc = with(0.2);
        e.close(&ctx);
        e.doc = with(0.7);
        e.track();
        e.doc = with(0.2);
        e.close(&ctx);
        assert!(e.closing.is_none(), "the same value again after another change: asked again");
        // A written, then edited and undone back to A: the revision on screen is not the one written.
        let mut e = Editor::open(&ctx, Source::picture(&path), "World");
        e.doc = with(0.5);
        e.written_now();
        e.doc = with(0.6);
        e.track();
        e.doc = with(0.5);
        assert!(e.dirty(), "unsaved is a revision, not a value");
    }

    /// Night PG2-4: the "not saved" guard asks every time there is something new to lose, and its warning goes once
    /// the edit changed or was saved.
    #[test]
    fn closing_asks_again_after_more_changes() {
        let ctx = egui::Context::default();
        let mut e = Editor::open(&ctx, Source::picture(std::path::Path::new("Z:/orbiters-test/none.png")), "World");
        let change = |e: &mut Editor, exposure: f32| {
            let l = e.doc.layer(Effect::Light(Light { exposure, ..Default::default() }));
            e.doc.layers.push(l);
        };
        change(&mut e, 0.5);
        e.close(&ctx);
        assert!(e.closing.is_none(), "asked first");
        change(&mut e, 0.2);
        e.close(&ctx);
        assert!(e.closing.is_none(), "changed since: asked again, not dropped");
        e.close(&ctx);
        assert!(e.closing.is_some(), "asked twice for the same edit: closes");
        let mut e = Editor::open(&ctx, Source::picture(std::path::Path::new("Z:/orbiters-test/none.png")), "World");
        change(&mut e, 0.5);
        e.close(&ctx);
        e.written_now();
        assert!(!e.dirty());
        change(&mut e, 0.3);
        e.close(&ctx);
        assert!(e.closing.is_none(), "saved, then changed: asked again");
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

    /// Dragging a depth blur's focus dot, or its distance, is one step of undo each.
    #[test]
    fn moving_the_focus_is_one_undo_step() {
        use orbiters_ui::photo::{Blur, Focus};
        let mut doc = Edit::default();
        let l = doc.layer(Effect::Blur(Blur::depth()));
        doc.layers.push(l);
        let mut h = History::new(&doc);
        for i in 0..20 {
            if let Effect::Blur(b) = &mut doc.layers[0].effect {
                b.point = [0.5 + i as f32 * 0.01, 0.5];
            }
            h.settle(&doc, true, 1.0 + i as f64 * 0.01);
        }
        h.settle(&doc, false, 1.5);
        if let Effect::Blur(b) = &mut doc.layers[0].effect {
            b.focus = Focus::Manual;
            b.distance = 12.0;
        }
        h.settle(&doc, false, 2.0);
        assert_eq!(h.past.len(), 2);
        h.undo(&mut doc);
        assert!(matches!(&doc.layers[0].effect, Effect::Blur(b) if b.focus == Focus::Auto && (b.point[0] - 0.69).abs() < 1e-5));
        h.undo(&mut doc);
        assert!(matches!(&doc.layers[0].effect, Effect::Blur(b) if b.point == [0.5, 0.5]));
        h.redo(&mut doc);
        h.redo(&mut doc);
        assert!(matches!(&doc.layers[0].effect, Effect::Blur(b) if b.distance == 12.0));
    }

    #[test]
    fn a_picture_of_orbiters_saves_with_its_world_and_time() {
        let dir = std::env::temp_dir().join(format!("orbiters-editor-remote-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Downloaded (a JPEG or a WebP from Discord has no VRChat XMP): the copy says what Orbiters said of it.
        let downloaded = dir.join("gallery-prod-9.img");
        files::write_png(&downloaded, &Rgba8::new(8, 4, vec![200; 8 * 4 * 4]), None).unwrap();
        let remote = files::Remote { kind: files::RemoteKind::Gallery, id: 9, world_id: Some("wrld_9".into()), world_name: Some("Lobby".into()), taken: 1_792_000_000, name: "Lobby".into(), ..Default::default() };
        let source = Source { picture: downloaded.clone(), layers: Vec::new(), remote: Some(remote) };
        let mut edit = source.new_edit();
        edit.geometry.vertical = 10.0;
        let target = dir.join("2026-10").join("Lobby (edited).png");
        save(&source, &edit, &target, None, &AtomicU32::new(0)).unwrap();
        let info = orbiters_core::visited::read_shot(&target).unwrap();
        assert_eq!((info.world_id.as_deref(), info.world_name.as_deref(), info.taken), (Some("wrld_9"), Some("Lobby"), 1_792_000_000));
        // Opened again, the copy edits again from the picture its descriptor names (night4, verify2: never the path the
        // edit kept, which an earlier version may have shared between two pictures).
        let named = Source { picture: source.remote.as_ref().unwrap().file(), ..source.clone() };
        assert_eq!(files::reopen(&target).map(|s| (s.source, s.edit)), Some((named, edit)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pictures_from_orbiters_are_edited_from_where_photos_and_captures_fetches_them() {
        let feed: orbiters_core::shares::Feed = serde_json::from_str(include_str!("../../../../orbiters-core/tests/fixtures/captures.json")).unwrap();
        let base = crate::controller::shares::remote_entry(feed.items.into_iter().next().unwrap());
        let only = |picture: crate::worlds::Picture| {
            let mut e = base.clone();
            e.remote = None;
            e.local = None;
            e.picture = Some(picture);
            e
        };
        // A community gallery's picture: renewed by its id, its listed link kept.
        let g = source_of(&only(crate::worlds::Picture::Gallery { development: false, asset: 0, id: 31, url: "https://cdn.example/x.png".into(), large: false })).unwrap();
        let r = g.remote.unwrap();
        assert_eq!((r.kind, r.id, r.url.as_str()), (files::RemoteKind::Gallery, 31, "https://cdn.example/x.png"));
        // An asset's picture: its link alone.
        let a = source_of(&only(crate::worlds::Picture::Gallery { development: false, asset: 0, id: 0, url: "https://orbiters.cc/asset.webp".into(), large: false })).unwrap();
        assert_eq!(a.remote.map(|r| (r.kind, r.url)), Some((files::RemoteKind::Link, "https://orbiters.cc/asset.webp".to_string())));
        // A 3D capture's picture is no picture to edit.
        let mut capture = base.clone();
        capture.local = None;
        if let Some(item) = capture.remote.as_mut() {
            item.kind = orbiters_core::shares::Kind::Capture;
        }
        assert!(source_of(&capture).is_none());
    }
}
