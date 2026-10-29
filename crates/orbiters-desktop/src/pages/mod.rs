//! The pages, plus what they share: the page context, background jobs and staggered entrances.

pub mod capture;
pub mod form;
pub mod header;
pub mod home;
pub mod mic;
pub mod orb;
pub mod projects;
pub mod sessions;
pub mod gallery;
mod gallery_select;
pub mod sharing;
pub mod requests;
pub mod settings;
pub mod splat_worker;
pub mod splats;
pub mod states;
pub mod storage;
pub mod videos;
pub mod worlds;

use crate::backend::Ctl;
use crate::state::Snapshot;
use egui::{pos2, vec2, Align2, Id, Layout, Rect, Sense, Shape, Ui, UiBuilder};
use orbiters_ui::blur;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{toast, widgets};
use std::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// N4N0's menu: what the app does, side by side.
    Home,
    Capture,
    Projects,
    Orb,
    Sessions,
    Worlds,
    /// The videos the worlds played, and the bookmark lists.
    Videos,
    Settings,
}

/// What a page gets each frame.
pub struct Cx<'a> {
    pub ctl: &'a Ctl,
    pub snap: &'a Snapshot,
    pub toasts: &'a mut toast::Toasts,
    /// When the page was entered (staggered entrances count from here).
    pub entered: f64,
    /// Bumped on every visit, so entrances replay.
    pub visit: u64,
    pub windowed: bool,
    /// The window's GPU, when it has one.
    pub gpu: Option<&'a crate::gpu::Handles>,
    /// Height of the title bar over the top of the page: content starts below it and scrolls on under it.
    pub top: f32,
    /// Where the page sends the person next (after this frame).
    pub go: Option<Go>,
}

/// Another page a page sends the person to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Go {
    /// Sessions, its storage popover open.
    FreeSpace,
    /// Unity projects (Capture's avatar picker, when it has no avatar to offer).
    Projects,
}

impl Cx<'_> {
    /// Runs a controller action and turns its error into a toast.
    pub fn run(&mut self, ctx: &egui::Context, r: anyhow::Result<()>) -> bool {
        match r {
            Ok(()) => true,
            Err(e) => {
                self.toasts.push(ctx, toast::Kind::Error, e.to_string());
                false
            }
        }
    }
}

/// Where a page's scroll bar goes: below the title bar, which the content scrolls on under.
pub fn below_bar(ui: &Ui, cx: &Cx) -> Rect {
    let r = ui.max_rect();
    Rect::from_min_max(pos2(r.left(), r.top() + cx.top), r.max)
}

/// Content under the title bar and a page's heading: blurred and dimmed, the more the nearer the window's top, so
/// that it stays in view without getting in the way of the text over it. The full look is reached this far above
/// where the content rests.
const UNDER_TOP: blur::Look = blur::Look { blur: 12.0, solid: 0.0, darken: 0.6, fade: 0.0 };
const UNDER_TOP_RAMP: f32 = 32.0;

/// The shapes from `from` on that scrolled up under what the page keeps above its scrolling content (the title bar,
/// and down to `bottom` its heading) stay in view there, blurred and dimmed, and no longer take clicks. Call after
/// the content and before what sits on it. `name` tells the bands of one frame apart (the worlds' grid and a world's
/// page both have one while a card flies open): each keeps its own GPU targets.
pub fn under_top(ui: &Ui, cx: &Cx, name: &'static str, from: egui::layers::ShapeIdx, bottom: f32) {
    let page = ui.max_rect();
    let band = Rect::from_min_max(page.min, pos2(page.right(), bottom));
    let id = Id::unique(("orbiters-page-top", name));
    let _ = ui.interact(band, id, Sense::click());
    if let Some(gpu) = cx.gpu {
        let look = blur::Look { solid: (band.height() - UNDER_TOP_RAMP).max(0.0), ..UNDER_TOP };
        blur::under(ui, id, &gpu.textures, from, band, look);
    }
}

/// Work done on a background thread; the interface polls it every frame and repaints when it lands.
pub struct Job<T> {
    rx: Option<mpsc::Receiver<T>>,
    value: Option<T>,
}

impl<T> Default for Job<T> {
    fn default() -> Self {
        Self { rx: None, value: None }
    }
}

impl<T: Send + 'static> Job<T> {
    pub fn spawn(ctx: &egui::Context, f: impl FnOnce() -> T + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        });
        Self { rx: Some(rx), value: None }
    }

    /// The result once it arrived.
    pub fn poll(&mut self) -> Option<&mut T> {
        if let Some(rx) = &self.rx {
            if let Ok(v) = rx.try_recv() {
                self.value = Some(v);
                self.rx = None;
            }
        }
        self.value.as_mut()
    }

    pub fn running(&self) -> bool {
        self.rx.is_some()
    }

    pub fn take(&mut self) -> Option<T> {
        self.poll();
        self.value.take()
    }
}

/// Allocates a block of `height` and runs `f` in it with the staggered entrance (`index` × 50 ms).
pub fn block(ui: &mut Ui, cx: &Cx, index: usize, height: f32, f: impl FnOnce(&mut Ui, Rect)) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    if !ui.is_rect_visible(rect.expand(24.0)) {
        return;
    }
    let (alpha, dy) = widgets::entrance(ui, Id::unique(("orbiters-enter", cx.visit, index)), cx.entered, index);
    let r = rect.translate(vec2(0.0, dy));
    let mut child = ui.new_child(UiBuilder::new().max_rect(r).id_salt(("block", index)));
    child.multiply_opacity(alpha);
    f(&mut child, r);
}

/// Staggered entrance of a rect inside a block (tiles, cards).
pub fn entering(ui: &Ui, cx: &Cx, index: usize, rect: Rect) -> (Rect, f32) {
    let (alpha, dy) = widgets::entrance(ui, Id::unique(("orbiters-enter", cx.visit, index)), cx.entered, index);
    (rect.translate(vec2(0.0, dy)), alpha)
}

/// Page heading: caption, title, and a sub line; returns the space to its right for actions.
pub fn heading(ui: &Ui, rect: Rect, caption: &str, title: &str, sub: Option<&str>) {
    let p = ui.painter();
    widgets::text(p, pos2(rect.left(), rect.top() + 7.0), Align2::LEFT_CENTER, &Txt::caps(caption));
    widgets::text(p, pos2(rect.left(), rect.top() + 34.0), Align2::LEFT_CENTER, &Txt::new(title, 30.0).weight(weight::SEMIBOLD).tracking(-0.015));
    if let Some(sub) = sub {
        widgets::text_truncated(p, pos2(rect.left(), rect.top() + 62.0), Align2::LEFT_CENTER, &Txt::new(sub, 13.0).color(theme::FG_3), rect.width() * 0.6);
    }
}

/// Section cards side by side on wide pages ([`orbiters_ui::layout::section_columns`]), one column on narrow ones. A
/// card goes to the shortest column the first time it shows at that many columns and keeps that column after, so cards
/// that grow (a list loading, a sign-in step) never shuffle the others.
pub struct Masonry {
    id: Id,
    top: egui::Pos2,
    width: f32,
    columns: Vec<Ui>,
    /// The column of each card, by its key.
    placed: Vec<(usize, usize)>,
}

/// Space between the columns.
const MASONRY_GAP: f32 = 24.0;

impl Masonry {
    /// Columns over the rest of `ui`'s width, from its cursor down.
    pub fn new(ui: &mut Ui, id_salt: &str) -> Self {
        let id = Id::unique(("orbiters-masonry", id_salt));
        let width = ui.available_width();
        let (n, w) = orbiters_ui::layout::section_columns(width, MASONRY_GAP);
        let top = ui.cursor().min;
        let columns = (0..n)
            .map(|c| {
                let r = Rect::from_min_size(top + vec2(c as f32 * (w + MASONRY_GAP), 0.0), vec2(w, 1.0e6));
                ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::top_down(egui::Align::Min)).id_salt((id, c)))
            })
            .collect();
        let placed = ui.ctx().data(|d| d.get_temp::<(usize, Vec<(usize, usize)>)>(id)).filter(|(m, _)| *m == n).map(|(_, p)| p).unwrap_or_default();
        Self { id, top, width, columns, placed }
    }

    /// Adds the card `key` (its section's index), drawn by `f` in its column.
    pub fn add<R>(&mut self, key: usize, f: impl FnOnce(&mut Ui) -> R) -> R {
        let column = match self.placed.iter().find(|(k, _)| *k == key) {
            Some(&(_, c)) => c.min(self.columns.len() - 1),
            None => {
                let c = (0..self.columns.len()).min_by(|&a, &b| self.columns[a].cursor().top().total_cmp(&self.columns[b].cursor().top())).unwrap_or(0);
                self.placed.push((key, c));
                c
            }
        };
        f(&mut self.columns[column])
    }

    /// Takes the columns' height in `ui` and keeps where each card went.
    pub fn finish(self, ui: &mut Ui) {
        let bottom = self.columns.iter().map(|c| c.min_rect().bottom()).fold(self.top.y, f32::max);
        let n = self.columns.len();
        let id = self.id;
        let placed = self.placed;
        ui.ctx().data_mut(|d| d.insert_temp(id, (n, placed)));
        ui.allocate_exact_size(vec2(self.width, bottom - self.top.y), Sense::hover());
    }
}

/// `count` rows of a card side by side when it is wide enough for rows of `min` points (`gap` apart), in reading order:
/// `row(ui, i, first)` draws row `i` in its column, `first` when it starts its column (no separator above it).
pub fn row_columns(ui: &mut Ui, id_salt: &str, min: f32, gap: f32, count: usize, mut row: impl FnMut(&mut Ui, usize, bool)) {
    let width = ui.available_width();
    let (n, w) = orbiters_ui::layout::columns(width, min, gap);
    let n = n.min(count.max(1));
    let w = if n == 1 { width } else { w };
    let top = ui.cursor().min;
    let mut columns: Vec<Ui> = (0..n)
        .map(|c| {
            let r = Rect::from_min_size(top + vec2(c as f32 * (w + gap), 0.0), vec2(w, 1.0e6));
            ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::top_down(egui::Align::Min)).id_salt((id_salt, c)))
        })
        .collect();
    for i in 0..count {
        row(&mut columns[i % n], i, i < n);
    }
    let bottom = columns.iter().map(|c| c.min_rect().bottom()).fold(top.y, f32::max);
    ui.allocate_exact_size(vec2(width, bottom - top.y), Sense::hover());
}

/// A card section: entrance, gradient icon tile, title and subtitle, then rows; the card is sized to its rows.
pub fn section(ui: &mut Ui, cx: &Cx, index: usize, glyph: &str, title: &str, subtitle: Option<&str>, body: impl FnOnce(&mut Ui)) {
    section_up_to(ui, cx, orbiters_ui::layout::SECTION_ALONE_MAX, index, glyph, title, subtitle, body);
}

/// A [`section`] at most `max_width` wide (one whose rows go side by side on wide pages).
#[allow(clippy::too_many_arguments)]
pub fn section_up_to(ui: &mut Ui, cx: &Cx, max_width: f32, index: usize, glyph: &str, title: &str, subtitle: Option<&str>, body: impl FnOnce(&mut Ui)) {
    let width = ui.available_width().min(max_width);
    let top_left = ui.cursor().min;
    let (alpha, dy) = widgets::entrance(ui, Id::unique(("orbiters-enter", cx.visit, index)), cx.entered, index);
    let bg = ui.painter().add(Shape::Noop);
    let inner = Rect::from_min_size(top_left + vec2(24.0, 20.0 + dy), vec2(width - 48.0, 100_000.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(egui::Align::Min)).id_salt(("section", index)));
    child.multiply_opacity(alpha);
    child.spacing_mut().item_spacing = vec2(0.0, 0.0);
    widgets::section_header(&mut child, glyph, title, subtitle);
    child.add_space(4.0);
    body(&mut child);
    let h = child.min_rect().height() + 20.0 + 12.0;
    let card = Rect::from_min_size(top_left + vec2(0.0, dy), vec2(width, h));
    ui.painter().set(
        bg,
        Shape::mesh(orbiters_ui::paint::squircle_mesh(
            card,
            theme::squircle(theme::CARD_RADIUS),
            theme::surface().gamma_multiply(alpha),
            1.0 / ui.ctx().pixels_per_point(),
        )),
    );
    ui.allocate_exact_size(vec2(width, h), Sense::hover());
}

/// Inner margins of every page.
pub const PAD_X: f32 = 32.0;

/// An ISO 8601 UTC time ("2026-10-20T01:55:12Z") in local time: "20 Oct 2026, 01:55".
pub fn local_time(iso: &str) -> String {
    use time::format_description::well_known::Rfc3339;
    let Ok(t) = time::OffsetDateTime::parse(iso, &Rfc3339) else { return iso.to_string() };
    let t = time::UtcOffset::current_local_offset().map(|o| t.to_offset(o)).unwrap_or(t);
    let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {} {}, {:02}:{:02}", t.day(), months[t.month() as usize - 1], t.year(), t.hour(), t.minute())
}

/// Unix seconds in local time, as [`local_time`].
pub fn local_time_unix(secs: u64) -> String {
    let Ok(t) = time::OffsetDateTime::from_unix_timestamp(secs as i64) else { return String::new() };
    local_time(&t.format(&time::format_description::well_known::Rfc3339).unwrap_or_default())
}
