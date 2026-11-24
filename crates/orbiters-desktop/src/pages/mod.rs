//! The pages, plus what they share: the page context, background jobs and staggered entrances.

mod beta_feedback;
mod booster_card;
mod launch_card;
mod translate_card;
pub mod capture;
pub mod community;
pub mod community_talk;
pub mod editor;
pub mod events;
pub mod experimental;
pub mod form;
pub mod groups;
pub mod header;
pub mod home;
pub mod language_card;
pub mod launcher;
pub mod mic;
pub mod mic_grid;
pub mod n4n0;
mod perf_card;
pub mod network;
pub mod projects;
pub mod reconstruct_again;
pub mod sessions;
pub mod gallery;
mod gallery_layers;
pub(crate) mod gallery_layout;
mod gallery_select;
pub mod sharing;
pub mod requests;
pub mod settings;
mod settings_n4n0;
pub mod settings_registry;
pub mod splat_export;
pub mod splat_worker;
pub mod splats;
pub mod states;
pub mod storage;
pub mod usage_notice;
pub mod videos;
pub mod vr_audio;
pub mod watch;
pub mod worlds;

pub use watch::Watch;

use crate::backend::Ctl;
use crate::state::Snapshot;
use egui::{pos2, vec2, Align2, Id, Layout, Rect, Sense, Shape, Ui, UiBuilder};
use orbiters_i18n::{tr, tr_ctx};
use orbiters_ui::blur;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{toast, widgets};
use std::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// N4N0's menu: what the app does, side by side.
    Home,
    Projects,
    N4n0,
    Sessions,
    Worlds,
    /// Orbiters' events, soonest first, and their windows (Invite me).
    Events,
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
    /// Unity projects (the capture element's avatar picker, when it has no avatar to offer).
    Projects,
    /// Settings, where the VRChat account is signed in (the launcher's "Sign in to VRChat").
    VrchatAccount,
    /// Settings, where the Explorer points are with the Orbiters account (Home's points, REQUEST19 1).
    ExplorerPoints,
    /// Settings on a setting (its registry id, `n4n0.calls`) or a sub-tab (`n4n0`): shown, brought into view, lit.
    Settings(&'static str),
}

impl Go {
    /// The setting (or sub-tab) of Settings it opens, for those that open Settings.
    pub fn setting(self) -> Option<&'static str> {
        match self {
            Go::VrchatAccount => Some("account.vrchat"),
            Go::ExplorerPoints => Some("account.explorer-points"),
            Go::Settings(target) => Some(target),
            Go::FreeSpace | Go::Projects => None,
        }
    }
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

/// Whether the text searched for differs from the last frame's (`last`, kept by the page): a new search starts at the
/// top of its results (Night PAGES-UI-24: scrolled down, a page kept its place and showed the end of the results).
/// The first frame is no new search; results arriving for the same search are none either.
pub fn new_search(last: &mut Option<String>, query: &str) -> bool {
    let query = query.trim();
    if last.as_deref() == Some(query) {
        return false;
    }
    let fresh = last.is_some();
    *last = Some(query.to_owned());
    fresh
}

/// The cache the pages keep what they fetch or make in (the editor's downloads, the drag's files): the app's own, or
/// in a test build the test process's own (`orbiters_core::paths::test_roots`, night6: every per-user folder of every
/// test is there, by construction).
pub fn cache_root() -> std::path::PathBuf {
    orbiters_core::paths::cache_dir()
}

/// Tests: this process's own folder for what they write.
#[cfg(test)]
pub fn test_root() -> std::path::PathBuf {
    orbiters_core::paths::test_roots().config.parent().expect("the test root").to_path_buf()
}

/// Who a page's work for Orbiters is for: the environment and the member's key, their id there and never their name
/// ([`AccountFollow::see`], verify4-r21 P3).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Owner {
    pub development: bool,
    pub account: String,
}

/// What [`AccountFollow::see`] saw this frame.
#[derive(Clone, Debug, Default)]
pub struct Seen {
    /// Who the pages work for now.
    pub owner: Owner,
    /// Orbiters has just come to answer for this account again (after being unreachable, an expired sign-in renewed):
    /// what failed may be asked again, nothing shown needs to go.
    pub answered: bool,
    /// Another owner than at the last frame: whatever the page keeps of the old one goes now. Followed on its own,
    /// never through what a page asked last (night4, verify2: an answer under way left Requests without a last key,
    /// and another account then kept the old one's list).
    pub changed: bool,
}

/// Follows the account for what a page asked Orbiters for it: WHO it is, not how its check with Orbiters is going.
/// `Checking`, `Offline` and `SignedIn` of the same member are the same person (the check runs again every minute while
/// Orbiters cannot be reached, and whenever a tool rewrites the saved account): what was shown for them stays. Another
/// member (or none), another environment, or a sign-in Orbiters refused (`Expired`), is another owner.
#[derive(Default)]
pub struct AccountFollow {
    /// Orbiters answered for this account at its last answer (`SignedIn`); a check under way keeps the last answer.
    answered: bool,
    last: Option<Owner>,
}

impl AccountFollow {
    pub fn see(&mut self, status: &crate::state::AccountStatus, development: bool) -> Seen {
        use crate::state::AccountState as A;
        let state = &status.state;
        let answered = match state {
            A::SignedIn { .. } => true,
            A::Checking { .. } | A::SigningIn => self.answered,
            A::Offline { .. } | A::Expired { .. } | A::SignedOut | A::Unknown => false,
        };
        let now = answered && !self.answered;
        self.answered = answered;
        // Nobody known (being read, signed out): nobody's.
        let member = status.member.as_deref().unwrap_or_default();
        // A sign-in Orbiters refused is no longer that account's access: what was asked with it goes.
        let account = if matches!(state, A::Expired { .. }) { format!("{member}\u{0}expired") } else { member.to_owned() };
        let owner = Owner { development, account };
        let changed = self.last.as_ref().is_some_and(|l| *l != owner);
        self.last = Some(owner.clone());
        Seen { owner, answered: now, changed }
    }

    /// Who the pages work for, as last seen.
    pub fn owner(&self) -> Option<&Owner> {
        self.last.as_ref()
    }
}

/// A result of work done for an [`Owner`]: THE rule of every page that shows what Orbiters says for an account
/// (night4, verify2 PAGES-UI-23): it is taken in only while that owner is still the one the page works for, and
/// dropped otherwise, whenever it lands. Dropping a page's jobs when the account changes is not enough: work asked a
/// moment before the change can land after it.
#[derive(Clone, Debug, PartialEq)]
pub struct ForAccount<T> {
    owner: Owner,
    value: T,
}

impl<T> ForAccount<T> {
    pub fn new(owner: &Owner, value: T) -> Self {
        Self { owner: owner.clone(), value }
    }

    /// The result, when it was for `current`; `None` (dropped) when it was for another owner.
    pub fn take_for(self, current: Option<&Owner>) -> Option<T> {
        (current == Some(&self.owner)).then_some(self.value)
    }
}

/// A [`Job`] for `owner`: its result says who it was for ([`ForAccount::take_for`]).
pub fn job_for<T: Send + 'static>(ctx: &egui::Context, owner: &Owner, work: impl FnOnce() -> T + Send + 'static) -> Job<ForAccount<T>> {
    let owner = owner.clone();
    Job::spawn(ctx, move || ForAccount { owner, value: work() })
}

/// Work the window process is doing on a thread of its own, counted: every [`Job`] for as long as it runs, and the few
/// other threads that write something (they hold a [`Working`]). While any is counted the window does not close to the
/// tray (`crate::window_rest`): a job added later blocks it by default, without anyone having to think of it.
static WORKING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// One piece of work of the window counted in [`working_now`] while this lives.
pub struct Working(());

impl Working {
    pub fn begin() -> Self {
        WORKING.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Self(())
    }
}

impl Drop for Working {
    fn drop(&mut self) {
        // The last work ended: the window looks again (its countdown to the tray waits for this, verify2 SWEEP-9; a
        // writer that is not a `Job` wakes nothing by itself).
        if WORKING.fetch_sub(1, std::sync::atomic::Ordering::AcqRel) == 1 {
            if let Some(ctx) = WAKE.get() {
                ctx.request_repaint();
            }
        }
    }
}

/// The window, woken when the last of its work ends ([`Working`]).
static WAKE: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();

/// Called once the window's context exists.
pub fn wake_when_work_ends(ctx: &egui::Context) {
    let _ = WAKE.set(ctx.clone());
}

/// How many pieces of work of the window run now ([`Working`]).
pub fn working_now() -> usize {
    WORKING.load(std::sync::atomic::Ordering::Acquire)
}

/// Work done on a background thread; the interface polls it every frame and repaints when it lands.
///
/// A job whose work panicked ends too (Night PAGES-UI-16): it is no longer `running`, brings no value, and says so
/// once through [`Job::died`]; its page woke for it. (It stayed "running" for ever: a spinner and buttons off until
/// the window was closed.) The panic itself goes to the panic log as before; nothing of it is shown.
pub struct Job<T> {
    rx: Option<mpsc::Receiver<T>>,
    value: Option<T>,
    /// Set by the worker when its work panicked (no value will come).
    panicked: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    died: bool,
}

impl<T> Default for Job<T> {
    fn default() -> Self {
        Self { rx: None, value: None, panicked: None, died: false }
    }
}

impl<T: Send + 'static> Job<T> {
    pub fn spawn(ctx: &egui::Context, f: impl FnOnce() -> T + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let panicked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = panicked.clone();
        // Counted as work of the window from now until its work ended (`crate::window_rest`: the window never closes to
        // the tray in the middle of it, whatever it is: a project copy, a Library move, a package update, a save).
        let working = Working::begin();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
            drop(working);
            match result {
                Ok(v) => {
                    let _ = tx.send(v);
                }
                Err(_) => flag.store(true, std::sync::atomic::Ordering::Release),
            }
            ctx.request_repaint();
        });
        Self { rx: Some(rx), value: None, panicked: Some(panicked), died: false }
    }

    /// The result once it arrived.
    pub fn poll(&mut self) -> Option<&mut T> {
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(v) => {
                    self.value = Some(v);
                    self.rx = None;
                    self.panicked = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    // The work ended without a value: it panicked.
                    self.rx = None;
                    self.panicked = None;
                    self.died = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        self.value.as_mut()
    }

    /// Still working: not once its value came, nor once its work panicked (even before a poll saw it).
    pub fn running(&self) -> bool {
        self.rx.is_some() && !self.panicked.as_ref().is_some_and(|p| p.load(std::sync::atomic::Ordering::Acquire))
    }

    pub fn take(&mut self) -> Option<T> {
        self.poll();
        self.value.take()
    }

    /// Whether the work panicked, told once.
    pub fn died(&mut self) -> bool {
        self.poll();
        std::mem::take(&mut self.died)
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
    heading_in(ui, rect, caption, title, sub, rect.width() * 0.6);
}

/// [`heading`] whose line under the title stops at `room` points (what stands on the head's right leaves it).
pub fn heading_in(ui: &Ui, rect: Rect, caption: &str, title: &str, sub: Option<&str>, room: f32) {
    let p = ui.painter();
    widgets::text(p, pos2(rect.left(), rect.top() + 7.0), Align2::LEFT_CENTER, &Txt::caps(caption));
    widgets::text(p, pos2(rect.left(), rect.top() + 34.0), Align2::LEFT_CENTER, &Txt::new(title, 30.0).weight(weight::SEMIBOLD).tracking(-0.015));
    if let Some(sub) = sub {
        widgets::text_truncated(p, pos2(rect.left(), rect.top() + 62.0), Align2::LEFT_CENTER, &Txt::new(sub, 13.0).color(theme::FG_3), room);
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

/// An ISO 8601 UTC time ("2026-10-20T01:55:12Z") in local time: "20 Oct 2026, 01:55". Each language puts the day, month and year in its
/// own order, but the date and the time always stay apart by ", " (the pages of the gallery and community split them there to show the time alone).
pub fn local_time(iso: &str) -> String {
    use time::format_description::well_known::Rfc3339;
    let Ok(t) = time::OffsetDateTime::parse(iso, &Rfc3339) else { return iso.to_string() };
    let t = time::UtcOffset::current_local_offset().map(|o| t.to_offset(o)).unwrap_or(t);
    // The whole date is one text, so each language puts day, month and year in its own order (Japanese: 2026年10月20日, 01:55).
    let month = match t.month() as u8 {
        1 => tr_ctx("month", "Jan"),
        2 => tr_ctx("month", "Feb"),
        3 => tr_ctx("month", "Mar"),
        4 => tr_ctx("month", "Apr"),
        5 => tr_ctx("month", "May"),
        6 => tr_ctx("month", "Jun"),
        7 => tr_ctx("month", "Jul"),
        8 => tr_ctx("month", "Aug"),
        9 => tr_ctx("month", "Sep"),
        10 => tr_ctx("month", "Oct"),
        11 => tr_ctx("month", "Nov"),
        _ => tr_ctx("month", "Dec"),
    };
    tr!("{day} {month} {year}, {time}", day = t.day(), month = month, year = t.year(), time = format!("{:02}:{:02}", t.hour(), t.minute()))
}

/// "5 min ago" for the moment `epoch` at `now` (Unix seconds; `orbiters_ui::format::relative_time`), and a frame asked
/// for when that would read otherwise: the pages draw only when something changed, and this text changes by the clock
/// alone. For text on screen: ask it for the rows in view.
pub fn ago(ctx: &egui::Context, epoch: u64, now: u64) -> String {
    if let Some(wait) = orbiters_ui::format::relative_time_changes_in(epoch, now) {
        ctx.request_repaint_after(after_seconds(wait));
    }
    orbiters_ui::format::relative_time(epoch, now)
}

/// Asks for a frame when something is due, `left` seconds from now, only while that is still to come. The rule for
/// every deadline a page waits on: what is due is taken up on the frame it is due on, and a deadline already past
/// asks for nothing. Asked for with a floor instead ("in 50 ms at the soonest"), a past deadline that nothing takes
/// up, because another page shows or a job stands in its way, asks for a frame at every frame, for ever: the page
/// never rests. What holds a past deadline up brings its own frame when it ends.
pub fn frame_in(ctx: &egui::Context, left: f64) {
    if left > 0.0 {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(left + 0.05));
    }
}

/// The wait for something due `seconds` after a clock read in whole seconds: a little more, so that the frame comes
/// once the second has turned.
pub fn after_seconds(seconds: u64) -> std::time::Duration {
    std::time::Duration::from_millis(seconds * 1000 + 50)
}

/// The local day `now` (Unix seconds) is in, as a number that grows by one at each local midnight, and the seconds
/// until what is said by the day ("Today", "Yesterday", "at 21:03" or a date) is to be worked out again: the next
/// local midnight, or the moment the zone's offset changes if that comes first (see [`local_day_in`]).
pub fn local_day(now: u64) -> (i64, u64) {
    local_day_in(now, &|at| {
        let local = time::OffsetDateTime::from_unix_timestamp(at).ok().and_then(|t| time::UtcOffset::local_offset_at(t).ok());
        local.or_else(|| time::UtcOffset::current_local_offset().ok()).map_or(0, |o| i64::from(o.whole_seconds()))
    })
}

/// [`local_day`] in a zone whose offset from UTC at a moment (Unix seconds) is `offset_at`, in seconds.
///
/// Midnight is reckoned with the offset now. When the offset changes before that (daylight saving time starts or
/// ends), midnight comes an hour sooner or later than reckoned, or the day turns at the change itself where a zone
/// changes at midnight: the wait is then until the change, to the second, and the frame asked for there reckons
/// again with the new offset. Reckoned with today's offset alone, the day's headings were an hour late on the night
/// the clocks go forward.
fn local_day_in(now: u64, offset_at: &dyn Fn(i64) -> i64) -> (i64, u64) {
    let now = now as i64;
    let offset = offset_at(now);
    let local = now + offset;
    let midnight = now + 86_400 - local.rem_euclid(86_400);
    let until = if offset_at(midnight) == offset {
        midnight
    } else {
        // The first second with another offset: the offset holds at `still`, and no longer at `changed`.
        let (mut still, mut changed) = (now, midnight);
        while changed - still > 1 {
            let middle = still + (changed - still) / 2;
            if offset_at(middle) == offset {
                still = middle;
            } else {
                changed = middle;
            }
        }
        changed
    };
    (local.div_euclid(86_400), (until - now).max(1) as u64)
}

/// Unix seconds in local time, as [`local_time`].
pub fn local_time_unix(secs: u64) -> String {
    let Ok(t) = time::OffsetDateTime::from_unix_timestamp(secs as i64) else { return String::new() };
    local_time(&t.format(&time::format_description::well_known::Rfc3339).unwrap_or_default())
}

/// What a page costs while nothing happens, for the pages' tests.
#[cfg(test)]
pub(crate) mod rest {
    use std::time::Duration;

    /// The frames a page draws by itself in a minute once it came to rest, from `from` seconds on: `frame(t)` draws
    /// one at `t` and says how soon it asked for the next (`Duration::MAX`: it asked for none). Forty seconds pass
    /// first, for entrances to end and looping decorations to rest (`orbiters_ui::anim::REST_AFTER`); then each frame
    /// comes when the last one asked, as a window's would. What a worker asks for between frames is not a frame the
    /// page asked for, and is not counted.
    pub fn frames_a_minute(from: f64, mut frame: impl FnMut(f64) -> Duration) -> usize {
        let mut t = from;
        while t < from + 40.0 {
            t += frame(t).as_secs_f64().clamp(1.0 / 60.0, 0.5);
        }
        let end = t + 60.0;
        let mut frames = 0;
        loop {
            let wait = frame(t);
            if wait == Duration::MAX {
                break;
            }
            t += wait.as_secs_f64().max(1.0 / 60.0);
            if t >= end {
                break;
            }
            frames += 1;
        }
        frames
    }

    /// How soon the frame that gave `output` asked for the next.
    pub fn asked(output: &egui::FullOutput) -> Duration {
        output.viewport_output.get(&egui::ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay)
    }

    /// Every text the frame that gave `output` painted, a line each.
    pub fn texts(output: &egui::FullOutput) -> String {
        fn of(shape: &egui::Shape, out: &mut String) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push_str(text.galley.text());
                    out.push('\n');
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| of(s, out)),
                _ => {}
            }
        }
        let mut out = String::new();
        output.shapes.iter().for_each(|clipped| of(&clipped.shape, &mut out));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_i18n::{in_language, Language};

    /// Night PAGES-UI-24: only a change of the text is a new search.
    #[test]
    fn a_new_search_is_a_change_of_its_text() {
        let mut last = None;
        assert!(!new_search(&mut last, ""), "the first frame");
        assert!(!new_search(&mut last, " "), "the same, trimmed");
        assert!(new_search(&mut last, "w"));
        assert!(!new_search(&mut last, "w"), "results coming for it");
        assert!(new_search(&mut last, "wo"));
        assert!(new_search(&mut last, ""), "cleared");
    }

    /// Night PAGES-UI-16: a job whose work panics ends (not "running" for ever), wakes its page and says so once.
    #[test]
    fn a_job_that_panics_ends_and_says_so_once() {
        let ctx = egui::Context::default();
        let woke = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let w = woke.clone();
        ctx.set_request_repaint_callback(move |_| {
            w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        let mut job: Job<u32> = Job::spawn(&ctx, || panic!("a worker's own unwrap"));
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while job.running() {
            assert!(std::time::Instant::now() < until, "still running after its work panicked");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(job.take().is_none());
        assert!(job.died(), "said once");
        assert!(!job.died(), "and only once");
        assert!(woke.load(std::sync::atomic::Ordering::SeqCst) > 0, "its page woke for it");
        // A job that works still brings its value.
        let mut ok = Job::spawn(&ctx, || 7u32);
        while ok.running() && ok.poll().is_none() {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(ok.take(), Some(7));
        assert!(!ok.died());
    }

    /// Night PAGES-UI-23: the account's key is who it is; "answered" rises once when Orbiters comes to answer.
    #[test]
    fn an_account_is_who_it_is_not_how_its_check_goes() {
        use crate::state::AccountState as A;
        let x = || Some("x".to_owned());
        let mut f = AccountFollow::default();
        let mut see = |s: A, member: Option<&str>| {
            let seen = f.see(&crate::state::AccountStatus { state: s, member: member.map(str::to_string), ..Default::default() }, false);
            (seen.owner.account, seen.answered)
        };
        let m = Some("production/1");
        assert_eq!(see(A::Unknown, None), (String::new(), false));
        assert_eq!(see(A::Checking { username: x() }, m), ("production/1".into(), false));
        assert_eq!(see(A::Offline { username: x() }, m), ("production/1".into(), false));
        assert_eq!(see(A::Checking { username: x() }, m), ("production/1".into(), false));
        assert_eq!(see(A::SignedIn { username: "x".into() }, m), ("production/1".into(), true));
        assert_eq!(see(A::Checking { username: x() }, m), ("production/1".into(), false), "a check under way keeps the last answer");
        assert_eq!(see(A::SignedIn { username: "renamed".into() }, m), ("production/1".into(), false));
        assert_eq!(see(A::Expired { username: x() }, m), ("production/1\u{0}expired".into(), false), "a refused sign-in is not the same access");
        assert_eq!(see(A::SignedIn { username: "x".into() }, m), ("production/1".into(), true), "a renewed sign-in asks again");
        assert_eq!(see(A::SignedIn { username: "x".into() }, None), (String::new(), false), "nobody known, whatever the name");
        assert_eq!(see(A::SignedOut, None), (String::new(), false));
    }

    /// Night4, verify2 PAGES-UI-23, verify4-r21 P3: another owner is said the frame it comes, whatever a page asked, a member of
    /// the same name too; a result is taken in only for the owner it was asked for.
    #[test]
    fn a_result_is_taken_only_for_the_owner_it_was_asked_for() {
        use crate::state::AccountState as A;
        let of = |state: A, member: &str| crate::state::AccountStatus { state, member: Some(member.to_string()), ..Default::default() };
        let sam = || A::SignedIn { username: "Sam".into() };
        let mut f = AccountFollow::default();
        let a = f.see(&of(sam(), "production/1"), false);
        assert!(!a.changed, "the first look is no change");
        let late = ForAccount::new(&a.owner, vec!["a's picture"]);
        let b = f.see(&of(sam(), "production/2"), false);
        assert!(b.changed, "another member, of the same name");
        assert_eq!(late.take_for(f.owner()), None, "a's answer landing after b came is dropped");
        assert_eq!(ForAccount::new(&b.owner, 1).take_for(f.owner()), Some(1));
        assert!(!f.see(&of(A::SignedIn { username: "Renamed".into() }, "production/2"), false).changed, "the same member, renamed");
        assert!(f.see(&of(sam(), "development/2"), true).changed, "another environment");
        assert!(f.see(&of(A::Expired { username: Some("Sam".into()) }, "development/2"), true).changed, "a refused sign-in");
        assert!(!f.see(&of(A::Expired { username: Some("Sam".into()) }, "development/2"), true).changed);
    }

    /// Verify4-r21 P3: no page of a window (nor its pictures) reads the saved account: what it asks is the service's to send, as the
    /// member on show.
    #[test]
    fn no_window_page_reads_the_saved_account() {
        fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let path = e.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    file(&path, out);
                }
            }
        }
        fn file(path: &std::path::Path, out: &mut Vec<String>) {
            let source = std::fs::read_to_string(path).unwrap();
            for (n, line) in source.lines().enumerate().filter(|(_, l)| !l.contains("contains(") && !l.trim_start().starts_with("//")) {
                if ["account::read(", "account::read_saved(", "shares::api(", "controller::api("].iter().any(|w| line.contains(w)) {
                    out.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        for dir in ["pages", "chat_bar", "overlay", "worlds"] {
            walk(&src.join(dir), &mut found);
        }
        for name in ["worlds.rs", "app.rs", "window.rs"] {
            file(&src.join(name), &mut found);
        }
        assert!(found.is_empty(), "{}", found.join("\n"));
    }

    /// What is said by the day is worked out again at each wait [`local_day_in`] gives. Followed from wait to wait
    /// through a change of the zone's offset, the day believed must be the day it is at every moment in between:
    /// with made-up zones and clocks, not this computer's zone today (whose offset is the same tomorrow most days of
    /// the year, which is how reckoning midnight with today's offset passed its test).
    #[test]
    fn the_day_is_right_at_every_moment_through_a_change_of_the_zones_offset() {
        // A day in each zone: the true local day at a moment.
        let day_at = |zone: &dyn Fn(i64) -> i64, at: i64| (at + zone(at)).div_euclid(86_400);
        // 2027-03-28 01:00 UTC, when Central Europe's clocks go forward; any moment does for the made-up ones.
        let change = 1_806_195_600_i64;
        type Zone = Box<dyn Fn(i64) -> i64>;
        let zones: [(&str, Zone); 5] = [
            ("no change", Box::new(|_| 3600)),
            ("forward at 02:00 local", Box::new(move |at| if at < change { 3600 } else { 7200 })),
            ("back at 03:00 local", Box::new(move |at| if at < change { 7200 } else { 3600 })),
            // Zones that change at midnight: 00:00 becomes 01:00 (that midnight never comes), or 24:00 becomes 23:00.
            ("forward at midnight", Box::new(move |at| if at < change { -3600 } else { 0 })),
            ("back at midnight", Box::new(move |at| if at < change { -3600 } else { -7200 })),
        ];
        for (name, zone) in &zones {
            // From two days before the change to two days after, from wait to wait.
            let (mut at, end) = (change - 2 * 86_400 - 1234, change + 2 * 86_400);
            let mut waits = 0;
            while at < end {
                let (day, wait) = local_day_in(at as u64, zone.as_ref());
                assert!((1..=90_000).contains(&wait), "{name}: {wait}");
                assert_eq!(day, day_at(zone.as_ref(), at), "{name}");
                // The day believed holds until the wait is over, second by second.
                for second in (at..at + wait as i64).step_by(61).chain([at + wait as i64 - 1]) {
                    assert_eq!(day_at(zone.as_ref(), second), day, "{name}: at {second}, {} s after a look at {at} that waits {wait} s", second - at);
                }
                at += wait as i64;
                waits += 1;
            }
            // One wait a midnight, and one more for the change where there is one: nothing like a frame a second.
            assert!(waits <= 6, "{name}: {waits} waits in four days");
        }
        // The case the review gave: just past midnight on the night the clocks go forward, the next midnight is 23
        // hours away, not 24. The wait ends at the change (02:00 local), and the one after it at midnight.
        let forward = |at: i64| if at < change { 3600 } else { 7200 };
        let just_past_midnight = change - 2 * 3600 + 60;
        let (_, to_change) = local_day_in(just_past_midnight as u64, &forward);
        assert_eq!(to_change as i64, change - just_past_midnight);
        let (_, to_midnight) = local_day_in(change as u64, &forward);
        assert_eq!(to_change + to_midnight, 23 * 3600 - 60, "23 hours from midnight to midnight");
    }

    /// Each language writes the date its own way, but keeps the date and the time apart by ", " (the gallery and community pages split them there),
    /// and leaves no `{name}` of its template unfilled.
    #[test]
    fn a_date_follows_the_language_and_keeps_its_time_after_a_comma() {
        for language in Language::ALL {
            in_language(language, || {
                let text = local_time("2026-10-20T12:30:00Z");
                assert!(!text.contains('{') && !text.contains('}'), "{language:?}: {text}");
                let (date, clock) = text.rsplit_once(", ").unwrap_or_else(|| panic!("{language:?}: no comma in {text}"));
                assert!(clock.len() == 5 && clock.as_bytes()[2] == b':', "{language:?}: {clock}");
                assert!(date.contains("2026"), "{language:?}: {date}");
                if language == Language::Ja {
                    assert!(date.contains('年') && date.contains('月') && date.contains('日'), "{date}");
                }
            });
        }
    }
}
