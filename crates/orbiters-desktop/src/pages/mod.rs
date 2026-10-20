//! The three pages, plus what they share: the page context, background jobs and staggered entrances.

pub mod capture;
pub mod sessions;
pub mod splats;
pub mod settings;

use crate::backend::Ctl;
use crate::state::Snapshot;
use egui::{pos2, vec2, Align2, Id, Rect, Sense, Ui, UiBuilder};
use orbiters_ui::blur;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{toast, widgets};
use std::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Capture,
    Sessions,
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
/// the content and before what sits on it.
pub fn under_top(ui: &Ui, cx: &Cx, from: egui::layers::ShapeIdx, bottom: f32) {
    let page = ui.max_rect();
    let band = Rect::from_min_max(page.min, pos2(page.right(), bottom));
    let _ = ui.interact(band, Id::unique("orbiters-page-top"), Sense::click());
    if let Some(gpu) = cx.gpu {
        let look = blur::Look { solid: (band.height() - UNDER_TOP_RAMP).max(0.0), ..UNDER_TOP };
        blur::under(ui, Id::unique("orbiters-page-top"), &gpu.textures, from, band, look);
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
