//! Sessions page: the library as cards with thumbnails. A card morphs into the session's detail; raw captures
//! from Mixer's simulator are imported from a popover.

use super::storage::{self, Pending, Popover, PopoverAction, TrimDialog};
use super::{Cx, Job};
use crate::controller::{self, RawCaptureSummary};
use crate::state::ReconstructionJob;
use egui::{pos2, vec2, Align2, Color32, ColorImage, CornerRadius, CursorIcon, Id, Order, Rect, Sense, TextureHandle, TextureOptions, Ui, UiBuilder};
use orbiters_core::bundle::{self, SessionSummary};
use orbiters_core::reconstruction::{self, Report, Stage};
use orbiters_ui::anim::{self, ease, Spring};
use orbiters_ui::paint::{self, Gradient};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, SegOption, Variant};
use orbiters_ui::{format, icons, logo, toast, trail};
use std::collections::HashMap;
use std::path::PathBuf;

enum Thumb {
    Loading(Job<Option<ColorImage>>),
    Ready(TextureHandle),
    Missing,
}

/// A session's picture as the grid keeps it.
struct Kept {
    thumb: Thumb,
    /// The session's manifest time when it was read: a session written since gets its picture again.
    modified: u64,
    /// When a card last drew it (egui's clock), to let go of the oldest first.
    used: f64,
}

/// Session pictures kept as textures (about half a megabyte each): those last on screen. Others load again from
/// disk as they scroll back into view.
const KEEP_THUMBS: usize = 64;

struct Detail {
    dir: PathBuf,
    /// The card's rect when it was opened (the morph starts there).
    origin: Rect,
    serial: u64,
    closing: bool,
    /// The morph's progress (0 the card, 1 the detail), as the last frame drew it.
    progress: f32,
    trail: Job<Vec<[f32; 3]>>,
    confirm_trash: bool,
    /// "Keep only the 3D view" asks first, over the detail.
    trim: Option<TrimDialog>,
    view: View,
    /// The reconstruction's preview, keyed by the report's creation time (a new run replaces it).
    preview: Option<(String, Thumb)>,
    /// A reconstruction of this session was running last frame.
    reconstructing: bool,
}

/// What the area under the stats shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Path,
    Splats,
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The card and the detail never blink at the swap: near the card, the card is fully drawn under the still opaque
/// detail, which then fades out (closing) or in (opening) over it.
fn card_opacity(progress: f32) -> f32 {
    1.0 - smoothstep(0.15, 0.4, progress)
}

fn detail_opacity(progress: f32) -> f32 {
    smoothstep(0.02, 0.15, progress)
}

struct Import {
    serial: u64,
    raws: Job<Vec<RawCaptureSummary>>,
}

#[derive(Default)]
pub struct State {
    loaded: Option<(u64, PathBuf, u64)>,
    list: Job<Vec<SessionSummary>>,
    sessions: Option<Vec<SessionSummary>>,
    query: String,
    /// The sessions the search shows (indices into `sessions`), for the search it was worked out for: again only
    /// when the list or the search changes, not every frame.
    shown: Option<(String, Vec<usize>)>,
    thumbs: HashMap<PathBuf, Kept>,
    detail: Option<Detail>,
    /// The 3D view of a session's splats, over everything (the detail waits under it).
    viewer: Option<super::splats::Viewer>,
    import: Option<Import>,
    /// The storage popover under "Free up space".
    storage: Option<Popover>,
    /// Clean-ups asked and shown as done already ([`Pending`]).
    pending: Pending,
    serial: u64,
}

impl State {
    /// Opens the detail of the first session as if its card was clicked (screenshots).
    pub fn open_first(&mut self, ctx: &egui::Context, origin: Rect) {
        if let Some(s) = self.sessions.as_ref().and_then(|l| l.first()) {
            let dir = s.dir.clone();
            self.open(ctx, dir, origin);
        }
    }

    /// Opens the detail of the session in `dir` (the GPU bench).
    pub fn open_session(&mut self, ctx: &egui::Context, dir: PathBuf, origin: Rect) {
        self.open(ctx, dir, origin);
    }

    pub fn loaded(&mut self) -> bool {
        if let Some(list) = self.list.take() {
            self.set_sessions(list);
        }
        self.sessions.is_some()
    }

    /// Whether something is still being read (the list, pictures, what a clean-up would delete): the screenshots
    /// wait for it.
    pub fn busy(&self) -> bool {
        self.list.running()
            || self.thumbs.values().any(|k| matches!(k.thumb, Thumb::Loading(_)))
            || self.detail.as_ref().and_then(|d| d.trim.as_ref()).is_some_and(TrimDialog::loading)
    }

    /// Opens the storage popover, on its confirmation when `confirming` (screenshots).
    pub fn open_storage(&mut self, confirming: bool) {
        self.serial += 1;
        self.storage = Some(if confirming { Popover::confirming(self.serial) } else { Popover::new(self.serial) });
    }

    /// Asks the open detail's "Keep only the 3D view" (screenshots).
    pub fn open_trim(&mut self, ctx: &egui::Context) {
        self.serial += 1;
        let serial = self.serial;
        if let Some(d) = self.detail.as_mut() {
            d.trim = Some(TrimDialog::new(ctx, d.dir.clone(), serial));
        }
    }

    /// Opens the detail of the first session matching `pick` (screenshots).
    pub fn open_where(&mut self, ctx: &egui::Context, origin: Rect, pick: impl Fn(&SessionSummary) -> bool) {
        if let Some(dir) = self.sessions.as_ref().and_then(|l| l.iter().find(|s| pick(s))).map(|s| s.dir.clone()) {
            self.open(ctx, dir, origin);
        }
    }

    /// Frees space as the person asked: the service deletes, and the library shows it done at once.
    fn clean_up(&mut self, cx: &Cx, trim: Vec<PathBuf>, delete: Vec<PathBuf>, now: f64) {
        self.pending.ask(&trim, &delete, now);
        if let Some(l) = self.sessions.as_mut() {
            self.pending.apply(l);
        }
        let list = self.sessions.take().unwrap_or_default();
        self.set_sessions(list);
        cx.ctl.clean_up(trim, delete);
    }

    /// A new list: the pictures of sessions that went away (trashed, moved) go with them.
    fn set_sessions(&mut self, list: Vec<SessionSummary>) {
        self.thumbs.retain(|dir, _| list.iter().any(|s| &s.dir == dir));
        self.sessions = Some(list);
        self.shown = None;
    }

    fn open(&mut self, ctx: &egui::Context, dir: PathBuf, origin: Rect) {
        self.serial += 1;
        let reconstructed = self.sessions.as_ref().and_then(|l| l.iter().find(|s| s.dir == dir)).is_some_and(|s| s.reconstruction.is_some());
        let d = dir.clone();
        self.detail = Some(Detail {
            dir,
            origin,
            serial: self.serial,
            closing: false,
            progress: 0.0,
            trail: Job::spawn(ctx, move || controller::session_trail(&d).unwrap_or_default()),
            confirm_trash: false,
            trim: None,
            view: if reconstructed { View::Splats } else { View::Path },
            preview: None,
            reconstructing: false,
        });
    }
}

pub(super) fn title(s: &SessionSummary) -> String {
    s.manifest.world.as_ref().and_then(|w| w.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| {
        if s.manifest.capture.source_name.is_empty() {
            "Session".into()
        } else {
            s.manifest.capture.source_name.clone()
        }
    })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    let page = ui.max_rect();

    // Reload when the library changes, the folder changes, or the page is entered again (sizes come from the
    // window's library cache: only sessions written since are walked again).
    let key = (cx.ctl.library_version(), cx.snap.settings.sessions_dir.clone(), cx.visit);
    if state.loaded.as_ref() != Some(&key) {
        if state.loaded.as_ref().is_some_and(|k| k.1 != key.1) {
            // Another folder: none of these pictures belong to it.
            state.thumbs.clear();
        }
        state.loaded = Some(key);
        let dir = cx.snap.settings.sessions_dir.clone();
        state.list = Job::spawn(&ctx, move || crate::worlds::sessions(&dir));
    }
    let now = anim::time(&ctx);
    if let Some(mut list) = state.list.take() {
        state.pending.on_list(&mut list, cx.snap.cleanup.as_ref(), now);
        state.set_sessions(list);
    }
    // The service is done with a clean-up asked here (or turned it down): the library, read again, says how it went.
    if state.pending.settled(cx.snap.cleanup.as_ref(), now) && !state.list.running() {
        state.loaded = None;
        ctx.request_repaint();
    } else if !state.pending.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }

    // Heading and actions, fixed above the grid (drawn after it, over the cards scrolled up under them).
    let head = Rect::from_min_size(pos2(page.left() + super::PAD_X, page.top() + cx.top + 8.0), vec2(page.width() - super::PAD_X * 2.0, 72.0));
    let actions_y = head.bottom() - 40.0;
    let folder_r = Rect::from_min_size(pos2(head.right() - 36.0, actions_y + 2.0), vec2(36.0, 36.0));
    let import_btn = Button::new("Import raw capture").icon(icons::DOWNLOAD_SIMPLE);
    let import_w = import_btn.desired_width(ui);
    let import_r = Rect::from_min_size(pos2(folder_r.left() - 8.0 - import_w, actions_y), vec2(import_w, 40.0));
    // What the library takes and freeing it; while the service deletes, how far it got (one width for both).
    let freeing = cx.snap.cleanup.as_ref().map(|j| format!("Freeing… {:.0}%", j.fraction() * 100.0));
    let space_label = freeing.as_deref().unwrap_or("Free up space");
    let space_w = Button::new("Free up space").icon(icons::BROOM).desired_width(ui).max(Button::new("Freeing… 100%").icon(icons::BROOM).desired_width(ui));
    let space_r = Rect::from_min_size(pos2(import_r.left() - 8.0 - space_w, actions_y), vec2(space_w, 40.0));
    let search_r = Rect::from_min_size(pos2(space_r.left() - 8.0 - 224.0, actions_y), vec2(224.0, 40.0));

    // The grid rests below the heading and scrolls on under it and the title bar.
    let grid_top = head.bottom() + 20.0;
    let grid_rect = Rect::from_min_max(page.min, page.max);
    let below_head = Rect::from_min_max(pos2(page.left(), grid_top), page.max);
    let grid_from = orbiters_ui::blur::next_shape(ui);
    let mut gu = ui.new_child(UiBuilder::new().max_rect(grid_rect).id_salt("sessions-grid"));
    let q = state.query.trim().to_lowercase();
    if state.shown.as_ref().is_none_or(|(asked, _)| *asked != q) {
        let shown = state
            .sessions
            .as_ref()
            .map(|l| {
                let matches = |s: &SessionSummary| q.is_empty() || title(s).to_lowercase().contains(&q) || s.dir.to_string_lossy().to_lowercase().contains(&q);
                l.iter().enumerate().filter(|(_, s)| matches(s)).map(|(i, _)| i).collect()
            })
            .unwrap_or_default();
        state.shown = Some((q.clone(), shown));
    }
    let filtered: &[usize] = state.shown.as_ref().map(|(_, v)| v.as_slice()).unwrap_or_default();
    let sessions = state.sessions.as_deref().unwrap_or_default();
    let mut clicked: Option<(PathBuf, Rect)> = None;
    egui::ScrollArea::vertical().id_salt("sessions").auto_shrink([false, false]).scroll_bar_rect(below_head).show(&mut gu, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 0, bottom: 32 }).show(ui, |ui| {
            ui.add_space(grid_top - grid_rect.top());
            let w = ui.available_width();
            if state.sessions.is_some() && filtered.is_empty() {
                empty_state(ui, cx, !q.is_empty(), below_head.height() * 0.6);
                return;
            }
            let cols = (((w + 16.0) / (270.0 + 16.0)).floor() as usize).max(1);
            let card_w = (w - 16.0 * (cols - 1) as f32) / cols as f32;
            let card_h = card_w * 9.0 / 16.0 + 96.0;
            let count = if state.sessions.is_none() { 6 } else { filtered.len() };
            let rows = count.div_ceil(cols);
            let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (card_h + 16.0) - 16.0), Sense::hover());
            for i in 0..count {
                let r = Rect::from_min_size(
                    pos2(area.left() + (i % cols) as f32 * (card_w + 16.0), area.top() + (i / cols) as f32 * (card_h + 16.0)),
                    vec2(card_w, card_h),
                );
                if !ui.is_rect_visible(r) {
                    continue;
                }
                match filtered.get(i).and_then(|&k| sessions.get(k)) {
                    Some(s) => {
                        let shown = state.detail.as_ref().filter(|d| d.dir == s.dir).map_or(1.0, |d| card_opacity(d.progress));
                        let tex = thumbnail(&ctx, &mut state.thumbs, s, now);
                        if session_card(ui, cx, i, s, r, tex.as_ref(), shown) {
                            clicked = Some((s.dir.clone(), r));
                        }
                    }
                    None => shimmer_card(ui, r),
                }
            }
        });
    });
    super::under_top(&gu, cx, "sessions", grid_from, grid_top);

    let total: u64 = state.sessions.as_ref().map(|l| l.iter().map(|s| s.size_bytes).sum()).unwrap_or(0);
    let sub = match &state.sessions {
        Some(l) => format!("{} session{} · {}", l.len(), if l.len() == 1 { "" } else { "s" }, format::bytes(total as f64)),
        None => "Loading…".into(),
    };
    let (head_r, head_a) = super::entering(ui, cx, 0, head);
    {
        let mut hu = ui.new_child(UiBuilder::new().max_rect(head_r).id_salt("sessions-head"));
        hu.multiply_opacity(head_a);
        super::heading(&hu, head_r, "Library", "Sessions", Some(&sub));
    }
    {
        let mut su = ui.new_child(UiBuilder::new().max_rect(search_r).id_salt("sessions-search"));
        widgets::search_field(&mut su, "sessions-search", &mut state.query, "Search worlds", 224.0);
    }
    let space =
        Button::new(space_label).icon(icons::BROOM).width(space_w).id_salt("free-up-space").hint("What the captures and 3D pictures take, and freeing it");
    if space.show_at(ui, space_r).clicked() {
        state.import = None;
        if state.storage.is_some() {
            state.storage = None;
        } else {
            state.serial += 1;
            state.storage = Some(Popover::new(state.serial));
        }
    }
    if import_btn.show_at(ui, import_r).clicked() {
        state.storage = None;
        if state.import.is_some() {
            state.import = None;
        } else {
            state.serial += 1;
            let dir = cx.snap.settings.raw_captures_dir.clone();
            state.import = Some(Import { serial: state.serial, raws: Job::spawn(&ctx, move || controller::list_raw_captures(&dir)) });
        }
    }
    if IconButton::new(icons::FOLDER_OPEN, "Open the sessions folder").show_at(ui, folder_r).clicked() {
        let r = controller::open_path(&cx.snap.settings.sessions_dir);
        cx.run(&ctx, r);
    }

    if let Some((dir, origin)) = clicked {
        state.open(&ctx, dir, origin);
    }

    if state.import.is_some() {
        import_popover(&ctx, cx, state, import_r);
    }
    if let Some(popover) = state.storage.as_mut() {
        let overview = storage::overview(state.sessions.as_deref().unwrap_or_default(), cx.snap);
        match storage::popover(&ctx, cx.snap, popover, &overview, space_r) {
            Some(PopoverAction::Close) => state.storage = None,
            Some(PopoverAction::CleanUp { trim, delete }) => state.clean_up(cx, trim, delete, now),
            None => {}
        }
    }
    if let Some(v) = state.viewer.as_mut() {
        let window = ctx.content_rect();
        let rect = Rect::from_min_max(pos2(window.left(), window.top() + crate::app::TITLE_H), window.max);
        let mut open = true;
        egui::Area::new(Id::unique(("splat-viewer-area", &v.session))).order(Order::Foreground).fixed_pos(rect.min).show(&ctx, |ui| {
            ui.set_min_size(rect.size());
            open = v.show(ui, rect);
        });
        if !open {
            state.viewer = None;
        }
    } else if state.detail.is_some() {
        detail(&ctx, cx, state, page);
    }
    trim_thumbs(&mut state.thumbs, now);
}

impl State {
    /// Whether the 3D view shows its splats (screenshots).
    pub fn viewer_loaded(&self) -> bool {
        self.viewer.as_ref().is_some_and(|v| v.loaded())
    }

    /// The session whose detail is open (screenshots).
    pub fn open_dir(&self) -> Option<PathBuf> {
        self.detail.as_ref().map(|d| d.dir.clone())
    }

    /// When the reconstruction the library shows for `dir` was made, if it has one (screenshots).
    pub fn reconstruction_of(&self, dir: &std::path::Path) -> Option<String> {
        self.sessions.as_ref()?.iter().find(|s| s.dir == dir)?.reconstruction.as_ref().map(|r| r.created_utc.clone())
    }

    /// Opens the 3D view of the open detail's session (screenshots).
    pub fn open_viewer(&mut self, ctx: &egui::Context, gpu: &crate::gpu::Handles) {
        let Some(s) = self.detail.as_ref().and_then(|d| self.sessions.as_ref()?.iter().find(|s| s.dir == d.dir)) else { return };
        self.viewer = Some(super::splats::Viewer::open(ctx, gpu, &s.dir, title(s)));
    }
}

fn thumbnail(ctx: &egui::Context, thumbs: &mut HashMap<PathBuf, Kept>, s: &SessionSummary, now: f64) -> Option<TextureHandle> {
    if !s.has_thumbnail {
        return None;
    }
    if thumbs.get(&s.dir).is_some_and(|k| k.modified != s.modified) {
        thumbs.remove(&s.dir);
    }
    let kept = thumbs.entry(s.dir.clone()).or_insert_with(|| {
        let dir = s.dir.clone();
        let job = Job::spawn(ctx, move || bundle::read_thumbnail(&dir).map(|(w, h, rgba)| ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba)));
        Kept { thumb: Thumb::Loading(job), modified: s.modified, used: now }
    });
    kept.used = now;
    if let Thumb::Loading(job) = &mut kept.thumb {
        if let Some(result) = job.take() {
            kept.thumb = match result {
                Some(image) => Thumb::Ready(ctx.load_texture(format!("session-thumb-{}", s.dir.display()), image, TextureOptions::LINEAR)),
                None => Thumb::Missing,
            };
        }
    }
    match &kept.thumb {
        Thumb::Ready(t) => Some(t.clone()),
        _ => None,
    }
}

/// Lets go of the pictures drawn longest ago beyond [`KEEP_THUMBS`] (their textures are freed with them).
fn trim_thumbs(thumbs: &mut HashMap<PathBuf, Kept>, now: f64) {
    let ready = |k: &Kept| matches!(k.thumb, Thumb::Ready(_));
    let count = thumbs.values().filter(|k| ready(k)).count();
    if count <= KEEP_THUMBS {
        return;
    }
    let mut old: Vec<(f64, PathBuf)> = thumbs.iter().filter(|(_, k)| ready(k) && k.used < now).map(|(dir, k)| (k.used, dir.clone())).collect();
    old.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, dir) in old.into_iter().take(count - KEEP_THUMBS) {
        thumbs.remove(&dir);
    }
}

/// The reconstruction's preview render as a texture (loaded off the interface thread, at most 960 px wide).
fn preview(ctx: &egui::Context, d: &mut Detail, s: &SessionSummary) -> Option<TextureHandle> {
    let key = s.reconstruction.as_ref()?.created_utc.clone();
    if d.preview.as_ref().is_none_or(|(k, _)| *k != key) {
        let dir = s.dir.clone();
        let job = Job::spawn(ctx, move || {
            let (w, h, rgba) = reconstruction::read_preview(&dir)?;
            let f = w.div_ceil(960).max(1) as usize;
            let (w, h) = (w as usize, h as usize);
            if w == 0 || h == 0 || rgba.len() < w * h * 4 {
                return None;
            }
            // Every pixel counts, the last row and column of blocks included when `f` does not divide the size.
            let (sw, sh) = (w.div_ceil(f), h.div_ceil(f));
            let mut small = vec![0u8; sw * sh * 4];
            for y in 0..sh {
                let (y0, y1) = (y * f, ((y + 1) * f).min(h));
                for x in 0..sw {
                    let (x0, x1) = (x * f, ((x + 1) * f).min(w));
                    let mut acc = [0u32; 4];
                    for yy in y0..y1 {
                        let row = (yy * w + x0) * 4;
                        for px in rgba[row..row + (x1 - x0) * 4].as_chunks::<4>().0 {
                            for c in 0..4 {
                                acc[c] += px[c] as u32;
                            }
                        }
                    }
                    let n = ((x1 - x0) * (y1 - y0)) as u32;
                    for c in 0..4 {
                        small[(y * sw + x) * 4 + c] = (acc[c] / n) as u8;
                    }
                }
            }
            Some(ColorImage::from_rgba_unmultiplied([sw, sh], &small))
        });
        d.preview = Some((key, Thumb::Loading(job)));
    }
    let (_, entry) = d.preview.as_mut()?;
    if let Thumb::Loading(job) = entry {
        if let Some(result) = job.take() {
            *entry = match result {
                Some(image) => Thumb::Ready(ctx.load_texture(format!("splats-preview-{}", s.dir.display()), image, TextureOptions::LINEAR)),
                None => Thumb::Missing,
            };
        }
    }
    match entry {
        Thumb::Ready(t) => Some(t.clone()),
        _ => None,
    }
}

pub(super) fn stage_label(stage: Stage) -> &'static str {
    match stage {
        Stage::Loading => "Reading frames",
        Stage::Seeding => "Seeding splats from depth",
        Stage::Training => "Training splats",
        Stage::Saving => "Saving",
    }
}

/// The whole run's progress: reading and seeding are short, training is most of it.
pub(super) fn overall_progress(j: &ReconstructionJob) -> f32 {
    match j.stage {
        Stage::Loading => 0.05 * j.progress,
        Stage::Seeding => 0.05,
        Stage::Training => 0.06 + 0.92 * j.progress,
        Stage::Saving => 0.98,
    }
}

/// The reconstruction: its preview (a render of a keyframe it never trained on) with its scores, or the run's
/// progress over the previous result.
fn splats_view(ui: &Ui, rect: Rect, tex: Option<&TextureHandle>, report: Option<&Report>, job: Option<&ReconstructionJob>) {
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    widgets::frame(&p, rect, theme::TILE_RADIUS, theme::surface_soft());
    if let Some(t) = tex {
        let size = t.size_vec2();
        let uv = cover_uv(rect, size.x / size.y.max(1.0), 1.0);
        let tint = if job.is_some() { theme::white(0.25) } else { Color32::WHITE };
        paint::squircle_texture(&p, rect, [theme::squircle(theme::TILE_RADIUS); 4], t.id(), uv, tint);
    }
    if let Some(j) = job {
        let c = rect.center();
        widgets::text(&p, pos2(c.x, c.y - 26.0), Align2::CENTER_CENTER, &Txt::new(stage_label(j.stage), 15.0).weight(weight::SEMIBOLD));
        let bar = Rect::from_center_size(c, vec2((rect.width() * 0.6).min(360.0), 6.0));
        let f = anim::spring_from(ui.ctx(), Id::unique(("splats-progress", &j.session_dir)), [0.0], [overall_progress(j)], Spring::SOFT)[0].clamp(0.0, 1.0);
        p.rect_filled(bar, theme::radius(3.0), theme::white(0.08));
        if f > 0.001 {
            p.rect_filled(Rect::from_min_size(bar.min, vec2((bar.width() * f).max(6.0), 6.0)), theme::radius(3.0), theme::ACCENT);
        }
        let detail = if j.stage == Stage::Training && j.iteration > 0 {
            let left = j.seconds as f64 / j.iteration as f64 * j.iterations.saturating_sub(j.iteration) as f64;
            let quality = j.psnr.map(|q| format!(" · {q:.1} dB")).unwrap_or_default();
            let on = if j.integrated { " · on the integrated GPU" } else { "" };
            format!("{} splats{quality} · {} left{on}", format::integer(j.gaussians as f64), format::duration(left))
        } else {
            format!("{:.0}%", overall_progress(j) * 100.0)
        };
        widgets::text(&p, pos2(c.x, c.y + 24.0), Align2::CENTER_CENTER, &Txt::new(detail, 12.0).color(theme::FG_3));
        return;
    }
    let Some(r) = report else { return };
    badge(&p, ui, pos2(rect.left() + 12.0, rect.bottom() - 12.0 - 22.0), "A view it never trained on", theme::black(0.55), theme::FG_2, false);
    let quality = r.psnr_held_out.map(|q| format!(" · {q:.1} dB")).unwrap_or_default();
    let line = format!("{} splats{quality} · {}", format::integer(r.gaussians as f64), format::duration(r.seconds as f64));
    badge(&p, ui, pos2(rect.right() - 12.0, rect.bottom() - 12.0 - 22.0), &line, theme::black(0.55), theme::FG_2, true);
}

/// UV rect that covers `target` with an image of `image_aspect`, zoomed by `zoom` around the centre.
pub(super) fn cover_uv(target: Rect, image_aspect: f32, zoom: f32) -> Rect {
    let ta = target.width() / target.height().max(1.0);
    let (mut w, mut h) = if ta > image_aspect { (1.0, image_aspect / ta) } else { (ta / image_aspect, 1.0) };
    w /= zoom;
    h /= zoom;
    Rect::from_center_size(pos2(0.5, 0.5), vec2(w, h))
}

pub(super) fn thumb_shape(painter: &egui::Painter, rect: Rect, radius: CornerRadius, tex: Option<&TextureHandle>, zoom: f32) {
    match tex {
        Some(t) => {
            let size = t.size_vec2();
            let uv = cover_uv(rect, size.x / size.y.max(1.0), zoom);
            let radii = [radius.nw, radius.ne, radius.se, radius.sw].map(|r| theme::squircle(r as f32));
            paint::squircle_texture(painter, rect, radii, t.id(), uv, Color32::WHITE);
        }
        None => {
            let pts = paint::squircle_points_corners(rect, [radius.nw, radius.ne, radius.se, radius.sw].map(|r| theme::squircle(r as f32)));
            paint::fill_convex(painter, &pts, |_| theme::INK_750);
            let lr = Rect::from_center_size(rect.center(), logo::size_for_width(40.0));
            logo::paint(painter, lr, logo::Look::default());
        }
    }
}

/// One card; returns true when clicked.
fn session_card(ui: &mut Ui, cx: &Cx, i: usize, s: &SessionSummary, rect: Rect, tex: Option<&TextureHandle>, shown: f32) -> bool {
    let ctx = ui.ctx().clone();
    let id = Id::unique(("session-card", &s.dir));
    let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    // Entrance: up and in, 35 ms apart.
    let delay = 0.05 + i.min(12) as f64 * 0.035;
    let elapsed = anim::time(&ctx) - cx.entered;
    if elapsed < delay {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay - elapsed));
    }
    let enter = anim::spring_from(&ctx, id.with(("enter", cx.visit)), [0.0], [if elapsed >= delay { 1.0 } else { 0.0 }], Spring::SOFT)[0];
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SOFT);
    let press = anim::toggle(&ctx, id.with("press"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
    let scale = (0.97 + 0.03 * enter) * (1.0 - 0.03 * press);
    let r = Rect::from_center_size(rect.center() + vec2(0.0, 20.0 * (1.0 - enter) - 4.0 * hover), rect.size() * scale);
    let mut p = ui.painter().clone();
    p.multiply_opacity(enter.clamp(0.0, 1.0) * shown);

    widgets::frame(&p, r, theme::CARD_RADIUS, theme::surface());
    let thumb = Rect::from_min_size(r.min, vec2(r.width(), r.width() * 9.0 / 16.0));
    let top = CornerRadius { nw: 20, ne: 20, sw: 0, se: 0 };
    thumb_shape(&p, thumb, top, tex, 1.0 + 0.06 * ease::out_cubic(hover));
    let shade = paint::rounded_rect_points_corners(thumb, [20.0, 20.0, 0.0, 0.0]);
    paint::fill_gradient(
        &p,
        &shade,
        &Gradient::vertical(thumb, &[(0.0, Color32::TRANSPARENT), (0.55, theme::alpha(theme::INK_950, 0.1)), (1.0, theme::alpha(theme::INK_950, 0.9))]),
    );
    let w = badge(&p, ui, pos2(thumb.left() + 12.0, thumb.top() + 12.0), &format::duration(s.manifest.stats.duration_s), theme::black(0.55), theme::FG_2, false);
    let job = cx.snap.reconstruction.as_ref().filter(|j| j.session_dir == s.dir);
    let cleaning = cx.snap.cleanup.as_ref().is_some_and(|j| j.of(&s.dir).is_some());
    let splats = match job {
        Some(j) => Some((format!("Reconstructing {:.0}%", overall_progress(j) * 100.0), theme::ACCENT)),
        None if cx.snap.reconstruct_queue.contains(&s.dir) => Some(("Waiting to reconstruct".to_string(), theme::FG_2)),
        None if cleaning => Some(("Freeing space".to_string(), theme::FG_2)),
        None => s.reconstruction.as_ref().map(|_| ("Splats".to_string(), theme::ACCENT)),
    };
    if let Some((label, color)) = splats {
        badge(&p, ui, pos2(thumb.left() + 12.0 + w + 6.0, thumb.top() + 12.0), &label, theme::black(0.55), color, false);
    }
    if s.manifest.consent.own_world {
        badge(&p, ui, pos2(thumb.right() - 12.0, thumb.top() + 12.0), "Own world", theme::alpha(theme::OK, 0.2), theme::OK, true);
    }

    let x = r.left() + 16.0;
    widgets::text_truncated(&p, pos2(x, thumb.bottom() + 22.0), Align2::LEFT_CENTER, &Txt::new(title(s), 15.0).weight(weight::SEMIBOLD), r.width() - 32.0);
    widgets::text(&p, pos2(x, thumb.bottom() + 42.0), Align2::LEFT_CENTER, &Txt::new(format::relative_time(s.modified, now_secs()), 12.0).color(theme::FG_3));
    let st = &s.manifest.stats;
    let mut px = x;
    let py = thumb.bottom() + 68.0;
    // A session reduced to its 3D view has no frames left to count.
    let pills = if s.trimmed && s.reconstruction.is_some() {
        vec![(Some(icons::CUBE_FOCUS), "3D view only".to_string()), (None, format::bytes(s.size_bytes as f64))]
    } else {
        vec![
            (Some(icons::STACK), format::integer(st.frames as f64)),
            (Some(icons::CUBE), format::integer(st.keyframes as f64)),
            (Some(icons::CPU), format::integer(st.depth_frames as f64)),
            (None, format::bytes(s.size_bytes as f64)),
        ]
    };
    for (glyph, label) in pills {
        px += widgets::pill_at(&p, ui, pos2(px, py), glyph, &label) + 6.0;
    }
    resp.clicked()
}

/// A small label on a picture; returns its width.
pub(super) fn badge(p: &egui::Painter, ui: &Ui, at: egui::Pos2, label: &str, bg: Color32, fg: Color32, right: bool) -> f32 {
    let t = Txt::new(label, 10.5).weight(weight::SEMIBOLD).tracking(0.025).color(fg);
    let s = widgets::text_size(ui, &t);
    let w = s.x + 20.0;
    let r = if right { Rect::from_min_size(pos2(at.x - w, at.y), vec2(w, 22.0)) } else { Rect::from_min_size(at, vec2(w, 22.0)) };
    p.rect_filled(r, theme::radius(11.0), bg);
    widgets::text(p, r.center(), Align2::CENTER_CENTER, &t);
    w
}

pub(super) fn shimmer_card(ui: &Ui, rect: Rect) {
    let p = ui.painter();
    widgets::frame(p, rect, theme::CARD_RADIUS, theme::surface());
    let t = anim::clock(ui.ctx(), 30.0);
    let k = ease::out_expo(((t % 1.6) / 1.6) as f32);
    let band =
        Rect::from_center_size(pos2(rect.left() - rect.width() * 0.5 + k * rect.width() * 2.0, rect.center().y), vec2(rect.width() * 0.6, rect.height()));
    let clip = p.with_clip_rect(rect.intersect(p.clip_rect()));
    let g = Gradient::horizontal(band, &[(0.2, Color32::TRANSPARENT), (0.5, theme::white(0.06)), (0.8, Color32::TRANSPARENT)]);
    paint::fill_gradient(&clip, &paint::rounded_rect_points(band, 0.0), &g);
}

fn empty_state(ui: &mut Ui, cx: &Cx, searching: bool, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height.max(240.0)), Sense::hover());
    let (rect, alpha) = super::entering(ui, cx, 1, rect);
    let mut p = ui.painter().clone();
    p.multiply_opacity(alpha);
    let t = anim::clock(ui.ctx(), 30.0);
    let bob = -8.0 * ease::in_out(ease::mirror(t, 2.0));
    let lr = Rect::from_center_size(rect.center() + vec2(0.0, -44.0 + bob), logo::size_for_width(70.0));
    logo::paint(&p, lr, logo::Look::default());
    let (title, text) = if searching {
        ("No session matches", "Try another world name.")
    } else {
        ("No sessions yet", "Record in VRChat from the Capture page, or import a raw capture from Mixer's simulator.")
    };
    widgets::text(&p, rect.center() + vec2(0.0, 18.0), Align2::CENTER_CENTER, &Txt::new(title, 17.0).weight(weight::SEMIBOLD));
    let g = p.layout_job({
        let mut job = Txt::new(text, 13.0).color(theme::FG_3).line_height(20.0).job(380.0);
        job.halign = egui::Align::Center;
        job
    });
    p.galley(pos2(rect.center().x, rect.center().y + 34.0), g, theme::FG_3);
}

fn import_popover(ctx: &egui::Context, cx: &mut Cx, state: &mut State, anchor: Rect) {
    let Some(import) = state.import.as_mut() else { return };
    let id = Id::unique(("import-popover", import.serial));
    let k = anim::spring_from(ctx, id.with("in"), [0.0], [1.0], Spring::BOUNCY)[0];
    let width = 420.0;
    let raws = import.raws.poll().cloned();
    let rows = raws.as_ref().map(|r| r.len().max(1)).unwrap_or(1);
    let list_h = (rows as f32 * 58.0).min(320.0);
    let h = 8.0 + 62.0 + list_h + 8.0;
    let base = Rect::from_min_size(pos2(anchor.right() - width, anchor.bottom() + 8.0), vec2(width, h));
    // Grows from its top-right corner.
    let rect =
        Rect::from_min_max(base.right_top() + (base.left_top() - base.right_top()) * (0.96 + 0.04 * k), base.right_top() + vec2(0.0, h * (0.96 + 0.04 * k)))
            .translate(vec2(0.0, -8.0 * (1.0 - k)));
    let busy = cx.snap.capture.state != crate::state::CaptureState::Idle;
    let mut close = false;
    let mut start: Option<RawCaptureSummary> = None;
    let area = egui::Area::new(id).order(Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
        ui.set_min_size(rect.size());
        let mut p = ui.painter().clone();
        p.multiply_opacity(k.clamp(0.0, 1.0));
        widgets::panel(&p, rect, 16.0);
        widgets::text(&p, pos2(rect.left() + 20.0, rect.top() + 26.0), Align2::LEFT_CENTER, &Txt::new("Raw captures", 13.5).weight(weight::SEMIBOLD));
        widgets::text_truncated(
            &p,
            pos2(rect.left() + 20.0, rect.top() + 46.0),
            Align2::LEFT_CENTER,
            &Txt::new("Frames from Mixer's capture simulator, replayed through the Spout decoder.", 12.0).color(theme::FG_3),
            width - 40.0,
        );
        let list = Rect::from_min_size(pos2(rect.left() + 8.0, rect.top() + 70.0), vec2(width - 16.0, list_h));
        match &raws {
            None => {
                let r = Rect::from_min_size(list.min + vec2(8.0, 4.0), vec2(list.width() - 16.0, 50.0));
                shimmer_card(ui, r);
            }
            Some(raws) if raws.is_empty() => {
                widgets::text(&p, list.center(), Align2::CENTER_CENTER, &Txt::new("No raw captures in the raw captures folder yet.", 12.5).color(theme::FG_4));
            }
            Some(raws) => {
                let mut lu = ui.new_child(UiBuilder::new().max_rect(list).id_salt("raws"));
                egui::ScrollArea::vertical().max_height(list_h).show(&mut lu, |ui| {
                    for r in raws {
                        let (row, resp) = ui.allocate_exact_size(vec2(list.width(), 56.0), if busy { Sense::hover() } else { Sense::click() });
                        let hover = anim::toggle(ctx, Id::unique(("raw-hover", &r.dir)), resp.hovered() && !busy, Spring::SNAPPY);
                        let mut p = ui.painter().clone();
                        if busy {
                            p.multiply_opacity(0.5);
                        }
                        p.rect_filled(row, theme::radius(12.0), theme::white(0.06 * hover));
                        let tile = Rect::from_min_size(pos2(row.left() + 12.0, row.center().y - 18.0), vec2(36.0, 36.0));
                        p.rect_filled(tile, theme::radius(10.0), theme::alpha(theme::ACCENT, 0.12));
                        widgets::icon(&p, tile.center(), icons::DOWNLOAD_SIMPLE, 16.0, theme::ACCENT);
                        let x = tile.right() + 12.0;
                        widgets::text_truncated(
                            &p,
                            pos2(x, row.center().y - 9.0),
                            Align2::LEFT_CENTER,
                            &Txt::new(&r.name, 13.0).weight(weight::MEDIUM),
                            row.right() - x - 12.0,
                        );
                        let mut info = format!("{} frames · {}×{}", r.frames, r.width, r.height);
                        if r.depth_every > 0 {
                            info.push_str(&format!(" · depth every {}", r.depth_every));
                        }
                        if r.has_ground_truth {
                            info.push_str(" · ground truth");
                        }
                        widgets::text_truncated(
                            &p,
                            pos2(x, row.center().y + 10.0),
                            Align2::LEFT_CENTER,
                            &Txt::new(info, 11.5).color(theme::FG_3),
                            row.right() - x - 12.0,
                        );
                        if resp.clicked() {
                            start = Some(r.clone());
                        }
                        if !busy {
                            resp.on_hover_cursor(CursorIcon::PointingHand);
                        }
                    }
                });
            }
        }
    });
    if area.response.clicked_elsewhere() && !ctx.input(|i| i.pointer.interact_pos().is_some_and(|p| anchor.contains(p))) {
        close = true;
    }
    if let Some(r) = start {
        cx.ctl.import_raw_capture(r.dir.clone());
        cx.toasts.push(ctx, toast::Kind::Info, format!("Importing {}…", r.name));
        close = true;
    }
    if close {
        state.import = None;
    }
}

fn detail(ctx: &egui::Context, cx: &mut Cx, state: &mut State, page: Rect) {
    let Some(d) = state.detail.as_mut() else { return };
    let Some(session) = state.sessions.as_ref().and_then(|l| l.iter().find(|s| s.dir == d.dir)).cloned() else {
        state.detail = None;
        return;
    };
    let tex = match state.thumbs.get_mut(&d.dir) {
        Some(kept) => {
            // In use as long as the detail shows it, wherever the grid is scrolled.
            kept.used = anim::time(ctx);
            match &kept.thumb {
                Thumb::Ready(t) => Some(t.clone()),
                _ => None,
            }
        }
        None => None,
    };
    let id = Id::unique(("session-detail", d.serial));
    let t = anim::spring_from(ctx, id.with("morph"), [0.0], [if d.closing { 0.0 } else { 1.0 }], Spring::MORPH)[0];
    d.progress = t;
    if d.closing && t < 0.02 {
        state.detail = None;
        return;
    }
    let k = t.clamp(0.0, 1.0);
    let target_w = (page.width() - 64.0).min(980.0);
    let target_h = (page.height() - 64.0).min(640.0);
    let target = Rect::from_center_size(page.center(), vec2(target_w, target_h));
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    let rect = Rect::from_min_max(
        pos2(lerp(d.origin.left(), target.left()), lerp(d.origin.top(), target.top())),
        pos2(lerp(d.origin.right(), target.right()), lerp(d.origin.bottom(), target.bottom())),
    );
    let radius = lerp(20.0, 28.0);
    let content = ((t - 0.55) / 0.45).clamp(0.0, 1.0);
    let mut close = false;
    let mut trash = false;
    let mut trim = false;
    let mut asked = false;
    let mut open_folder = false;
    let mut open_viewer = false;
    let serial = state.serial + 1;
    // Escape and clicks outside go to the dialog first, when one asks (this frame closes it, not the detail).
    let asking = d.trim.is_some();

    // The whole window dims, sidebar and title bar included; a click anywhere but the title bar (which still moves,
    // minimises and closes the window) closes the detail.
    let window = ctx.content_rect();
    let catcher = Rect::from_min_max(pos2(window.left(), window.top() + crate::app::TITLE_H), window.max);
    egui::Area::new(id).order(Order::Foreground).fixed_pos(catcher.min).show(ctx, |ui| {
        let (_, backdrop) = ui.allocate_exact_size(catcher.size(), Sense::click());
        if backdrop.clicked() && !rect.contains(backdrop.interact_pointer_pos().unwrap_or_default()) && !asking {
            close = true;
        }
        ui.painter().with_clip_rect(window).rect_filled(window, 0.0, theme::alpha(theme::INK_950, 0.7 * k));
        ui.multiply_opacity(detail_opacity(t));
        let p = ui.painter();
        widgets::panel(p, rect, radius);
        let thumb_h = lerp(d.origin.width() * 9.0 / 16.0, 260.0);
        let thumb = Rect::from_min_size(rect.min, vec2(rect.width(), thumb_h));
        let r8 = radius.round().clamp(0.0, 255.0) as u8;
        thumb_shape(p, thumb, CornerRadius { nw: r8, ne: r8, sw: 0, se: 0 }, tex.as_ref(), 1.0);
        let shade = paint::rounded_rect_points_corners(thumb, [radius, radius, 0.0, 0.0]);
        paint::fill_gradient(
            p,
            &shade,
            &Gradient::vertical(thumb, &[(0.0, Color32::TRANSPARENT), (0.6, theme::alpha(theme::INK_750, 0.2)), (1.0, theme::INK_750)]),
        );
        if content <= 0.0 {
            return;
        }
        let mut cp = p.clone();
        cp.multiply_opacity(content);
        let dy = 14.0 * (1.0 - content);
        widgets::text_truncated(
            &cp,
            pos2(thumb.left() + 24.0, thumb.bottom() - 40.0 + dy),
            Align2::LEFT_CENTER,
            &Txt::new(title(&session), 26.0).weight(weight::SEMIBOLD).tracking(-0.015),
            thumb.width() - 48.0,
        );
        let ident = session.manifest.world.as_ref().and_then(|w| w.id.clone()).unwrap_or_else(|| session.dir.display().to_string());
        widgets::text_truncated(
            &cp,
            pos2(thumb.left() + 24.0, thumb.bottom() - 16.0 + dy),
            Align2::LEFT_CENTER,
            &Txt::new(ident, 11.5).mono().color(theme::FG_3),
            thumb.width() - 48.0,
        );

        let mut cu = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(("detail-body", d.serial)));
        cu.multiply_opacity(content);
        let close_r = Rect::from_min_size(pos2(thumb.right() - 16.0 - 36.0, thumb.top() + 16.0), vec2(36.0, 36.0));
        if IconButton::new(icons::X, "Close").size(36.0, 18.0).backdrop(theme::black(0.4)).show_at(&mut cu, close_r).clicked() {
            close = true;
        }

        // Body: stats and the path on the left, details and actions on the right.
        let body = Rect::from_min_max(pos2(rect.left() + 24.0, thumb.bottom() + 24.0 + dy), pos2(rect.right() - 24.0, rect.bottom() - 24.0 + dy));
        let side_w = 300.0;
        let main = Rect::from_min_max(body.min, pos2(body.right() - side_w - 20.0, body.bottom()));
        let side = Rect::from_min_max(pos2(main.right() + 20.0, body.top()), body.max);
        let st = &session.manifest.stats;
        let minis = [
            (icons::CLOCK, "Duration", format::duration(st.duration_s), None),
            (
                icons::STACK,
                "Frames",
                format::integer(st.frames as f64),
                Some(format!("{} decoded", format::percent(st.strip_ok as f64, st.frames.max(1) as f64))),
            ),
            (icons::CUBE, "Keyframes", format::integer(st.keyframes as f64), None),
            (icons::CPU, "Depth frames", format::integer(st.depth_frames as f64), None),
        ];
        let mw = (main.width() - 36.0) / 4.0;
        for (i, (glyph, label, value, sub)) in minis.iter().enumerate() {
            let r = Rect::from_min_size(pos2(main.left() + i as f32 * (mw + 12.0), main.top()), vec2(mw, 78.0));
            mini_stat(&cp, r, glyph, label, value, sub.as_deref());
        }
        let map = Rect::from_min_max(pos2(main.left(), main.top() + 78.0 + 16.0), main.max);
        let job = cx.snap.reconstruction.as_ref().filter(|j| j.session_dir == session.dir);
        // A run of this session starting brings its progress up.
        if job.is_some() && !d.reconstructing {
            d.view = View::Splats;
        }
        d.reconstructing = job.is_some();
        let splats = job.is_some() || session.reconstruction.is_some();
        if !splats {
            d.view = View::Path;
        }
        match d.view {
            View::Path => {
                let points = d.trail.poll().cloned().unwrap_or_default();
                trail::show(
                    &cu,
                    map,
                    id.with("trail"),
                    &trail::Trail {
                        points: &points,
                        heading: None,
                        live: false,
                        empty_text: if d.trail.running() { "Reading the camera path…" } else { "No camera path in this session" },
                    },
                );
            }
            View::Splats => {
                let tex = preview(ctx, d, &session);
                splats_view(&cu, map, tex.as_ref(), session.reconstruction.as_ref(), job);
                if session.reconstruction.is_some() && job.is_none() && cx.gpu.is_some() {
                    let r = Rect::from_min_size(pos2(map.left() + 10.0, map.top() + 12.0), vec2(124.0, 36.0));
                    if Button::new("View in 3D").icon(icons::CUBE_FOCUS).variant(Variant::Glass).size(widgets::Size::Sm).width(r.width()).show_at(&mut cu, r).clicked() {
                        open_viewer = true;
                    }
                }
            }
        }
        if splats {
            let bar = Rect::from_min_max(pos2(map.left() + 10.0, map.top() + 10.0), pos2(map.right() - 10.0, map.top() + 50.0));
            let mut bu = cu.new_child(UiBuilder::new().max_rect(bar).layout(egui::Layout::right_to_left(egui::Align::Min)).id_salt("view-switch"));
            widgets::segmented(&mut bu, "view", &mut d.view, &[SegOption::new(View::Path, "Path"), SegOption::new(View::Splats, "Splats")], true);
        }

        let c = &session.manifest.capture;
        let rows = [
            ("Captured", super::local_time(&session.manifest.created_utc)),
            ("Source", if c.source_name.is_empty() { c.source.clone() } else { c.source_name.clone() }),
            ("Resolution", if c.width > 0 { format!("{}×{}", c.width, c.height) } else { "–".into() }),
            ("Quality", format!("{:?}", c.quality).to_lowercase()),
            ("Size", format::bytes(session.size_bytes as f64)),
            ("Format", format!("session v{} · contracts {}", session.manifest.version, session.manifest.contracts_version)),
        ];
        let info = Rect::from_min_size(side.min, vec2(side.width(), 16.0 * 2.0 + rows.len() as f32 * 22.0));
        widgets::card(&cp, info, 16.0);
        for (i, (k, v)) in rows.iter().enumerate() {
            let y = info.top() + 16.0 + 11.0 + i as f32 * 22.0;
            widgets::text(&cp, pos2(info.left() + 16.0, y), Align2::LEFT_CENTER, &Txt::new(*k, 12.5).color(theme::FG_3));
            widgets::text_truncated(&cp, pos2(info.right() - 16.0, y), Align2::RIGHT_CENTER, &Txt::new(v, 12.5), info.width() - 120.0);
        }
        let mut y = info.bottom() + 12.0;
        let full = |y: f32| Rect::from_min_size(pos2(side.left(), y), vec2(side.width(), 40.0));
        // Reduced to its 3D view (or being): nothing left to reconstruct from.
        let cleaning = cx.snap.cleanup.as_ref().and_then(|j| j.of(&session.dir));
        let view_only = session.reconstruction.is_some() && (session.trimmed || cleaning == Some(false));
        let queued = cx.snap.reconstruct_queue.contains(&session.dir);
        let recording = cx.snap.capture.state != crate::state::CaptureState::Idle && cx.snap.capture.session_dir.as_ref() == Some(&session.dir);
        if view_only && job.is_none() && !queued {
            storage::view_only(&cp, full(y), cleaning.is_some());
        } else if job.is_some() {
            if Button::new("Stop reconstruction").icon(icons::X).width(side.width()).show_at(&mut cu, full(y)).clicked() {
                cx.ctl.stop_reconstruction();
            }
        } else if cx.snap.reconstruct_queue.contains(&session.dir) {
            let hint = if cx.snap.is_capturing() { "Starts when the capture ends" } else { "Starts after the reconstructions before it" };
            if Button::new("Waiting · cancel").icon(icons::X).variant(Variant::Secondary).hint(hint).width(side.width()).show_at(&mut cu, full(y)).clicked() {
                cx.ctl.unqueue_reconstruction(session.dir.clone());
            }
        } else {
            let blocked = (st.depth_frames == 0).then_some("This session has no depth frames: record with the capture kit's depth frames on");
            let again = session.reconstruction.is_some();
            let mut b = Button::new(if again { "Reconstruct again" } else { "Reconstruct" })
                .icon(icons::MAGIC_WAND)
                .variant(if again { Variant::Secondary } else { Variant::Primary })
                .enabled(blocked.is_none())
                .width(side.width());
            if let Some(h) = blocked {
                b = b.hint(h);
            }
            if b.show_at(&mut cu, full(y)).clicked() {
                cx.ctl.reconstruct(session.dir.clone());
                d.view = View::Splats;
            }
        }
        y += 52.0;
        let mut send = Button::new("Send to Mixer").icon(icons::SPARKLE).enabled(session.reconstruction.is_some() && job.is_none()).width(side.width());
        if session.reconstruction.is_none() {
            send = send.hint("Reconstruct first: Mixer gets the splats");
        }
        if send.show_at(&mut cu, full(y)).clicked() {
            cx.ctl.send_to_mixer(session.dir.clone(), title(&session));
        }
        y += 52.0;
        let bin = Rect::from_min_size(pos2(side.right() - 40.0, y), vec2(40.0, 40.0));
        // Its capture files can go, its 3D view staying (when nothing else needs them now).
        let can_trim = session.reconstruction.is_some() && session.capture_bytes > 0 && !view_only && job.is_none() && !queued && !recording;
        if d.confirm_trash {
            let w = 104.0;
            let r = Rect::from_min_size(pos2(side.right() - w, y), vec2(w, 40.0));
            let k = anim::spring_from(ctx, id.with("confirm"), [0.9], [1.0], Spring::SNAPPY)[0];
            let mut bu = cu.new_child(UiBuilder::new().max_rect(side).id_salt("confirm"));
            bu.multiply_opacity(((k - 0.9) * 10.0).clamp(0.0, 1.0));
            let cancel = Rect::from_min_size(pos2(side.left(), y), vec2(side.width() - w - 8.0, 40.0));
            if Button::new("Cancel").variant(Variant::Ghost).width(cancel.width()).id_salt("trash-cancel").show_at(&mut bu, cancel).clicked() {
                d.confirm_trash = false;
            }
            if Button::new("Confirm")
                .icon(icons::TRASH)
                .variant(Variant::Danger)
                .width(w)
                .show_at(&mut bu, Rect::from_center_size(r.center(), r.size() * k))
                .clicked()
            {
                trash = true;
            }
        } else {
            if can_trim {
                let keep = Rect::from_min_size(pos2(side.left(), y), vec2(side.width() - 96.0, 40.0));
                let hint = format!("Deletes its capture files ({}) for good; the splats stay", format::bytes(session.capture_bytes as f64));
                if Button::new("Keep only the 3D view").icon(icons::BROOM).width(keep.width()).hint(&hint).show_at(&mut cu, keep).clicked() {
                    d.trim = Some(TrimDialog::new(ctx, session.dir.clone(), serial));
                    asked = true;
                }
                let folder = Rect::from_min_size(pos2(bin.left() - 48.0, y), vec2(40.0, 40.0));
                if IconButton::new(icons::FOLDER_OPEN, "Open folder").size(40.0, 17.0).show_at(&mut cu, folder).clicked() {
                    open_folder = true;
                }
            } else {
                let open_r = Rect::from_min_size(pos2(side.left(), y), vec2(side.width() - 48.0, 40.0));
                if Button::new("Open folder").icon(icons::FOLDER_OPEN).width(open_r.width()).show_at(&mut cu, open_r).clicked() {
                    open_folder = true;
                }
            }
            if IconButton::new(icons::TRASH, "Move to the recycle bin").size(40.0, 17.0).danger().show_at(&mut cu, bin).clicked() {
                d.confirm_trash = true;
            }
        }
        // Asked before anything goes, over the whole detail.
        if let Some(dialog) = d.trim.as_mut() {
            match storage::trim_dialog(ui, dialog, rect, session.capture_bytes) {
                Some(true) => {
                    trim = true;
                    d.trim = None;
                }
                Some(false) => d.trim = None,
                None => {}
            }
        }
    });
    if asked {
        // A dialog of its own each time (its entrance plays again).
        state.serial = serial;
    }
    if trim {
        state.clean_up(cx, vec![session.dir.clone()], Vec::new(), anim::time(ctx));
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !asking && !trim {
        close = true;
    }
    if open_viewer {
        if let Some(gpu) = cx.gpu {
            state.viewer = Some(super::splats::Viewer::open(ctx, gpu, &session.dir, title(&session)));
        }
    }
    if open_folder {
        let r = controller::open_path(&session.dir);
        cx.run(ctx, r);
    }
    if trash {
        // Gone at once; the library reload brings it back if the recycle bin refused it.
        cx.ctl.trash_session(session.dir.clone());
        if let Some(l) = state.sessions.as_mut() {
            l.retain(|s| s.dir != session.dir);
        }
        state.thumbs.remove(&session.dir);
        state.shown = None;
        state.detail = None;
        return;
    }
    if close {
        if let Some(d) = state.detail.as_mut() {
            d.closing = true;
        }
    }
}

pub(super) fn mini_stat(p: &egui::Painter, r: Rect, glyph: &str, label: &str, value: &str, sub: Option<&str>) {
    widgets::frame(p, r, 16.0, theme::surface_soft());
    widgets::icon(p, pos2(r.left() + 19.0, r.top() + 20.0), glyph, 14.0, theme::FG_4);
    widgets::text(p, pos2(r.left() + 32.0, r.top() + 20.0), Align2::LEFT_CENTER, &Txt::caps(label));
    widgets::text_truncated(p, pos2(r.left() + 12.0, r.top() + 44.0), Align2::LEFT_CENTER, &Txt::new(value, 18.0).weight(weight::SEMIBOLD), r.width() - 24.0);
    if let Some(sub) = sub {
        widgets::text_truncated(p, pos2(r.left() + 12.0, r.top() + 64.0), Align2::LEFT_CENTER, &Txt::new(sub, 11.0).color(theme::FG_3), r.width() - 24.0);
    }
}
