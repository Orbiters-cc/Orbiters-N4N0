//! The capture element of Photos & captures (REQUEST21 5: "Merge all the content of the tab Capture into the tab Photo &
//! captures where the 3D capture related UI elements will expand dynamically to show all the options when needed,
//! otherwise it'll be more of a gallery"). At the top of the gallery's one scrolling list: a slim bar, the state and the
//! primary actions on it (a 3D picture of people, of the world; Stop while one is under way), that grows in place, eased
//! (never a spring: the gallery's rule), into what it needs to show:
//!
//! - **a status card**, by itself, when something is going on or wants a look: a capture under way (what it is doing,
//!   who it left out, Stop, a free recording's Pause and folder), a reconstruction (its stage, how far, Stop), what the
//!   last capture could not do (and Update the Toolkit), its error, N4N0 missing from the avatar worn, a finished capture
//!   to open and share;
//! - **every option**, when the person opens it (its Options button, or the bar itself), while a capture runs, or when a
//!   link of the app asks for one of its parts ([`Part`]): which VRChat pictures are taken in, the three big actions (or
//!   the capture notice, or the test pattern where VRChat does not run here), the picture's length, Scan and the test
//!   pattern, what capturing needs (VRChat, N4N0 on the avatar, Spout, the Action Menu, the storage), a recording's
//!   numbers and settings, the camera path and what it records.
//!
//! The person's Hide closes it again until something new comes. It is part of the page's flow, not a box that scrolls.

use super::{block, entering, Cx};
use crate::state::{CaptureState, VrchatDriverShown, VrchatRunShown};
use egui::{pos2, vec2, Align, Align2, Id, Layout, Rect, Sense, Ui, UiBuilder, Vec2};
use orbiters_contracts::manifest::{CaptureMode, StorageQuality};
use orbiters_core::picture::Left;
use orbiters_core::settings::{Photo3dLength, PictureMode, Settings};
use orbiters_i18n::{fill, mark, tr, tr_ctx, tr_n};
use orbiters_n4n0_core::menu::strings::{Strings, FLIGHTS};
use orbiters_ui::anim::{self, ease, Spring};
use orbiters_ui::format;
use orbiters_ui::record::{self, RecState, RecordButton};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, SegOption, Size, Tone, Variant};
use orbiters_ui::{icons, trail};
use std::path::{Path, PathBuf};

/// A part of the element a link of the app opens it on (what opened the Capture tab before: the sidebar's VRChats, the
/// assistant's "open the capture page", a window closed to the tray on it, the screenshots).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The three big actions (the capture under way, the capture notice).
    Actions,
    /// The VRChats on this computer, and the one pictures are taken in.
    Vrchats,
    /// What capturing needs: VRChat, N4N0 on the avatar, the Spout source, the Action Menu, the storage.
    Needs,
    /// A recording's numbers and settings, the camera path and what it records.
    Recording,
}

/// Why the element shows more than its bar by itself, strongest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// A capture or a 3D picture is under way (every option shows: its numbers and path matter now).
    Capturing,
    /// A capture is being reconstructed in 3D.
    Reconstructing,
    /// The last capture ended in an error.
    Error,
    /// What the last capture could not do (who it could not leave out).
    LeftOut,
    /// N4N0 is not on the avatar worn.
    Kit,
    /// A capture just ended: to open and share.
    Finished,
}

/// How much of the element shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Bar,
    Status,
    Full,
}

/// What the element asks of the page around it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    /// Open this capture's window (look at it in 3D, share it).
    Review(PathBuf),
}

/// A capture that just ended.
#[derive(Clone, Debug, PartialEq)]
struct Finished {
    dir: PathBuf,
    picture: Option<PictureMode>,
}

/// A height eased from one value to another over [`Tween::FOR`] seconds (no spring: the gallery around never bounces).
#[derive(Clone, Copy, Debug, Default)]
struct Tween {
    from: f32,
    to: f32,
    at: f64,
}

impl Tween {
    const FOR: f64 = 0.34;

    fn value(&self, now: f64) -> f32 {
        let k = (((now - self.at) / Self::FOR) as f32).clamp(0.0, 1.0);
        self.from + (self.to - self.from) * ease::out_cubic(k)
    }

    fn aim(&mut self, to: f32, now: f64) {
        if (to - self.to).abs() > 0.5 {
            self.from = self.value(now);
            self.to = to;
            self.at = now;
        }
    }

    fn moving(&self, now: f64) -> bool {
        now - self.at < Self::FOR && (self.to - self.from).abs() > 0.5
    }
}

#[derive(Default)]
pub struct State {
    /// The avatar worn, and whether its upload can hide itself, as its manifest says (read once per avatar).
    upload: Option<(String, Option<bool>)>,
    /// Update the Toolkit was pressed: what went wrong is told here, once it ended.
    toolkit_asked: bool,
    /// The person opened every option.
    open: bool,
    /// The person hid the element while this reason held (it comes back for the next one).
    closed_for: Option<Why>,
    /// A link asked for this part: scrolled into view once it stands still.
    reveal: Option<Part>,
    /// Where each part was drawn last (its top), for the scroll to it.
    tops: Vec<(Part, f32)>,
    /// The status card's and the options' heights: shown (eased) and measured (what they need).
    tween: [Tween; 2],
    measured: [f32; 2],
    /// The session being recorded, as last seen, and the capture that ended since (until opened or put away).
    recording: Option<Finished>,
    finished: Option<Finished>,
}

impl State {
    /// Opens the element on `part` (a link of the app), and scrolls to it once it stands still.
    pub fn reveal(&mut self, part: Part) {
        self.open = true;
        self.closed_for = None;
        self.reveal = Some(part);
    }

    /// Every option shows.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The part a link asked for, until it was scrolled to (tests).
    pub fn revealing(&self) -> Option<Part> {
        self.reveal
    }

    /// How much shows for `snap` (what the person chose, and why it would open by itself).
    pub fn level(&self, snap: &crate::state::Snapshot) -> Level {
        level(self.open, self.closed_for, why(snap, self.finished.is_some(), kit_missing(snap)))
    }

    /// Screenshots: the person's Hide pressed for what shows now (a recording under way: the bar alone).
    pub fn stage_hidden(&mut self) {
        self.open = false;
        self.closed_for = Some(Why::Capturing);
    }

    /// Screenshots: a capture that just ended, in `dir`.
    pub fn stage_finished(&mut self, dir: PathBuf, picture: Option<PictureMode>) {
        self.finished = Some(Finished { dir, picture });
    }

    /// The element's height now, before it is drawn (the gallery keeps what is in view in place as it changes).
    pub fn height(&self, now: f64) -> f32 {
        BAR_H + self.tween[0].value(now) + self.tween[1].value(now) + AFTER
    }
}

/// The bar's height, and the room under the whole element before the gallery's first group.
pub const BAR_H: f32 = 72.0;
const AFTER: f32 = 28.0;
/// The room between the bar, the status card and the options.
const GAP: f32 = 16.0;

const GRID_H: f32 = 212.0;

/// What each of a 3D picture's camera flights does (1 s, 4 s, 8 s), here and on Home's 3D picture page.
pub(super) const FLIGHT_HINTS: [&str; 3] = [
    mark("A gentle sweep around what the camera aims at"),
    mark("Looks around, then goes around the people or through the place, as far as it can see it goes"),
    mark("As 4 s, with more views of each person, or more of the place"),
];

/// The three flights' names (1 s, 4 s, 8 s) in the language now, as the menu on the avatar and in VR writes them (`menu-strings`).
pub(super) fn flight_labels() -> [&'static str; 3] {
    let strings = Strings::builtin(orbiters_i18n::language());
    [strings.get(FLIGHTS[0]), strings.get(FLIGHTS[1]), strings.get(FLIGHTS[2])]
}

/// Whether N4N0 is missing from the avatar worn (an avatar is known, VRChat can be captured here, the notice was accepted).
fn kit_missing(snap: &crate::state::Snapshot) -> bool {
    orbiters_core::here::camera_frames()
        && snap.vrchat.running
        && snap.settings.consent_acknowledged
        && snap.n4n0.avatar_id.is_some()
        && !(snap.n4n0.manifest && snap.n4n0.prop_sync.is_some())
}

/// Why the element opens by itself now, strongest first.
pub fn why(snap: &crate::state::Snapshot, finished: bool, kit: bool) -> Option<Why> {
    let c = &snap.capture;
    if snap.is_capturing() || c.state == CaptureState::Stopping {
        return Some(Why::Capturing);
    }
    if snap.reconstruction.is_some() {
        return Some(Why::Reconstructing);
    }
    if c.error.is_some() {
        return Some(Why::Error);
    }
    if c.left_out.why.is_some() {
        return Some(Why::LeftOut);
    }
    if kit {
        return Some(Why::Kit);
    }
    finished.then_some(Why::Finished)
}

/// How much shows: every option when the person opened it or a capture runs; the status card alone for any other
/// reason; the bar alone when there is none, or the person hid it while that reason held.
pub fn level(open: bool, closed_for: Option<Why>, why: Option<Why>) -> Level {
    if open {
        return Level::Full;
    }
    match why {
        Some(w) if closed_for == Some(w) => Level::Bar,
        Some(Why::Capturing) => Level::Full,
        Some(_) => Level::Status,
        None => Level::Bar,
    }
}

/// The element, at the top of Photos & captures' list: the bar, then what it needs to show. `name_of` names a session
/// by its folder (the gallery's entries), `head` is how much of the page stays over the list (a part scrolled to is
/// put under it).
pub fn element(ui: &mut Ui, cx: &mut Cx, state: &mut State, projects: &mut super::projects::State, name_of: &dyn Fn(&Path) -> Option<String>, head: f32) -> Option<Out> {
    let ctx = ui.ctx().clone();
    let now = anim::time(&ctx);
    let snap = cx.snap;
    follow_finished(state, snap);
    let kit = kit_missing(snap);
    let reason = why(snap, state.finished.is_some(), kit);
    // A hide holds for the reason it was pressed for: the next one opens it again.
    if state.closed_for.is_some() && state.closed_for != reason {
        state.closed_for = None;
    }
    let level = level(state.open, state.closed_for, reason);
    let mut out = None;
    let mut toggle = false;
    // A link to the three big actions shows the element from its bar down (they are right under it).
    let bar_top = ui.cursor().top();
    bar(ui, cx, state, level, reason, name_of, &mut toggle);
    if toggle {
        if level > Level::Bar {
            state.open = false;
            state.closed_for = reason;
            // A finished capture put away is put away.
            if reason == Some(Why::Finished) {
                state.finished = None;
            }
        } else {
            state.open = true;
        }
    }
    let width = ui.available_width();
    // The status card.
    let status = reason.filter(|_| level >= Level::Status);
    grow(ui, now, &mut state.tween[0], &mut state.measured[0], status.is_some(), width, "capture-status", |ui| {
        if let Some(w) = status {
            match w {
                Why::Capturing => under_way(ui, cx),
                Why::Reconstructing => {
                    if let Some(o) = reconstruction(ui, cx, name_of) {
                        out = Some(o);
                    }
                }
                Why::Error => error_card(ui, cx),
                Why::LeftOut => why_last(ui, cx, &mut state.toolkit_asked, projects),
                Why::Kit => kit_card(ui, cx, projects),
                Why::Finished => {
                    let (review, dismiss) = finished_card(ui, cx, state.finished.as_ref(), name_of);
                    if review {
                        out = state.finished.take().map(|f| Out::Review(f.dir));
                    } else if dismiss {
                        state.finished = None;
                    }
                }
            }
        }
    });
    // Every option.
    let full = level == Level::Full;
    let mut tops = Vec::new();
    {
        let (upload, toolkit_asked) = (&mut state.upload, &mut state.toolkit_asked);
        grow(ui, now, &mut state.tween[1], &mut state.measured[1], full, width, "capture-options", |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 20.0);
            options(ui, cx, upload, toolkit_asked, projects, reason, &mut tops);
        });
    }
    if !tops.is_empty() {
        tops.retain(|(p, _)| *p != Part::Actions);
        tops.push((Part::Actions, bar_top));
        state.tops = tops;
    }
    // A part asked for, once it stands still: put under what stays over the list.
    if let Some(part) = state.reveal {
        if !state.tween[1].moving(now) && full {
            if let Some((_, y)) = state.tops.iter().find(|(p, _)| *p == part) {
                let r = Rect::from_min_size(pos2(ui.max_rect().left(), y - head - 12.0), vec2(1.0, 1.0));
                ui.scroll_to_rect(r, Some(Align::TOP));
            }
            state.reveal = None;
        }
    }
    ui.add_space(AFTER);
    if state.tween.iter().any(|t| t.moving(now)) {
        ctx.request_repaint();
    }
    out
}

/// A part of the element whose height eases to what it needs (`want`) or to nothing, its content fading with it and
/// cut at its edge meanwhile. Drawn while it shows or closes; measured each time.
#[allow(clippy::too_many_arguments)]
fn grow(ui: &mut Ui, now: f64, tween: &mut Tween, measured: &mut f32, want: bool, width: f32, salt: &str, draw: impl FnOnce(&mut Ui)) {
    tween.aim(if want { *measured } else { 0.0 }, now);
    let h = tween.value(now);
    if !want && h < 0.5 {
        return;
    }
    let (rect, _) = ui.allocate_exact_size(vec2(width, h.max(0.0)), Sense::hover());
    let mut child = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(rect.min, vec2(width, 100_000.0))).layout(Layout::top_down(Align::Min)).id_salt(salt));
    child.set_clip_rect(rect.expand2(vec2(12.0, 0.0)).intersect(ui.clip_rect()));
    let k = if *measured > 1.0 { (h / *measured).clamp(0.0, 1.0) } else { 0.0 };
    child.multiply_opacity(k * k);
    child.spacing_mut().item_spacing = vec2(0.0, 20.0);
    child.add_space(GAP);
    draw(&mut child);
    let content = child.min_rect().height();
    if want && (content - *measured).abs() > 0.5 {
        *measured = content;
        tween.aim(content, now);
    }
}

/// Follows the session being recorded: when its capture ends, it is the finished one (to open and share).
fn follow_finished(state: &mut State, snap: &crate::state::Snapshot) {
    let c = &snap.capture;
    let under_way = snap.is_capturing() || c.state == CaptureState::Stopping;
    if under_way {
        if let Some(dir) = &c.session_dir {
            state.recording = Some(Finished { dir: dir.clone(), picture: c.picture });
        }
        state.finished = None;
    } else if let Some(done) = state.recording.take() {
        // A test pattern's recording is not one of the person's captures (it shows in Free up space only).
        if c.error.is_none() && !done.dir.to_string_lossy().to_lowercase().contains("test") {
            state.finished = Some(done);
        }
    }
}

/// The bar: what capturing stands at, in a line, and its primary actions; pressing it (or Options) opens every option.
#[allow(clippy::too_many_arguments)]
fn bar(ui: &mut Ui, cx: &mut Cx, state: &mut State, level: Level, reason: Option<Why>, name_of: &dyn Fn(&Path) -> Option<String>, toggle: &mut bool) {
    let ctx = ui.ctx().clone();
    let snap = cx.snap;
    let c = &snap.capture;
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, BAR_H), Sense::hover());
    let (rect, alpha) = entering(ui, cx, 1, rect);
    let mut bu = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("capture-bar"));
    bu.multiply_opacity(alpha);
    let id = Id::unique("capture-bar");
    let resp = bu.interact(rect, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    widgets::note_control(&ctx, tr("Capture"), rect, true);
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SNAPPY);
    let capturing = matches!(reason, Some(Why::Capturing));
    let (glyph, tint) = match reason {
        Some(Why::Capturing) => match c.picture {
            Some(PictureMode::People) => (icons::USERS, theme::VIOLET),
            Some(PictureMode::World) => (icons::GLOBE_HEMISPHERE_WEST, theme::accent()),
            None => (icons::RECORD, theme::REC),
        },
        Some(Why::Reconstructing) => (icons::CUBE_FOCUS, theme::accent()),
        Some(Why::Error | Why::LeftOut | Why::Kit) => (icons::WARNING, theme::WARN),
        Some(Why::Finished) => (icons::CHECK_CIRCLE, theme::OK),
        None => (icons::APERTURE, theme::accent()),
    };
    let p = bu.painter().clone();
    widgets::frame(&p, rect, theme::CARD_RADIUS, theme::mix(theme::surface(), tint, if capturing { 0.10 } else { 0.03 + 0.03 * hover }));
    let tile = Rect::from_min_size(pos2(rect.left() + 16.0, rect.center().y - 20.0), vec2(40.0, 40.0));
    p.rect_filled(tile, theme::radius(13.0), theme::alpha(tint, 0.16));
    if reason == Some(Why::Reconstructing) {
        let j = snap.reconstruction.as_ref();
        let f = j.map_or(0.0, super::sessions::overall_progress);
        orbiters_ui::paint::ring(&p, tile.center(), 11.0, 2.5, theme::alpha(tint, 0.3));
        orbiters_ui::paint::arc(&p, tile.center(), 11.0, -std::f32::consts::FRAC_PI_2, std::f32::consts::TAU * f.clamp(0.0, 1.0), 2.5, if j.is_some_and(|j| j.paused) { theme::fg_3() } else { tint });
    } else if capturing && matches!(c.state, CaptureState::Recording) {
        widgets::status_dot(&p, tile.center(), Tone::Rec, true);
    } else {
        widgets::icon(&p, tile.center(), glyph, 20.0, tint);
    }

    // The buttons on the right, from the right: Options (or Hide), then the primary actions.
    let open = level > Level::Bar;
    let more = Button::new(if open { tr("Hide") } else { tr("Options") })
        .icon(if open { icons::CARET_UP } else { icons::CARET_DOWN })
        .variant(Variant::Ghost)
        .size(Size::Sm)
        .id_salt("capture-options-toggle")
        .hint(if open { tr("Back to the gallery: the capture options close") } else { tr("Every capture option: VRChat, the picture's length, what capturing needs, a recording's settings") });
    let mw = more.desired_width(&bu);
    let mut right = rect.right() - 14.0;
    let more_r = Rect::from_min_size(pos2(right - mw, rect.center().y - 16.0), vec2(mw, 32.0));
    if widgets::activated(&more.show_at(&mut bu, more_r)) {
        *toggle = true;
    }
    right = more_r.left() - 8.0;
    let x = tile.right() + 14.0;
    let consent = snap.settings.consent_acknowledged;
    let here = orbiters_core::here::camera_frames();
    if capturing && !open {
        let stop = match c.state {
            CaptureState::Stopping => tr("Saving…"),
            CaptureState::Importing => tr("Importing…"),
            CaptureState::Starting => tr("Starting…"),
            _ if c.picture.is_some() => tr("Stop this picture"),
            _ => tr("Stop recording"),
        };
        let can = snap.is_capturing() && !matches!(c.state, CaptureState::Importing | CaptureState::Starting);
        let b = Button::new(stop).icon(icons::SQUARE).variant(Variant::Stop).size(Size::Sm).enabled(can).id_salt("capture-bar-stop");
        let bw = b.desired_width(&bu);
        let r = Rect::from_min_size(pos2(right - bw, rect.center().y - 16.0), vec2(bw, 32.0));
        if widgets::activated(&b.show_at(&mut bu, r)) {
            cx.ctl.stop_capture();
        }
        right = r.left() - 8.0;
    } else if here && !capturing && level < Level::Full {
        // A 3D picture of people, of the world: in words where they fit, else their icons (every option shows them
        // large).
        let vrchat = snap.vrchat.running;
        let why_not = if !vrchat { tr("Start VRChat first: its camera takes the picture") } else { "" };
        let people = tr("3D picture of people");
        let world = tr("3D picture of the world");
        let wide = Button::new(people).icon(icons::USERS).size(Size::Sm).desired_width(&bu) + Button::new(world).icon(icons::GLOBE_HEMISPHERE_WEST).size(Size::Sm).desired_width(&bu) + 8.0;
        let words_room = right - x - 220.0;
        let mut chosen = None;
        if wide <= words_room {
            for (mode, label, glyph, salt) in [(PictureMode::World, world, icons::GLOBE_HEMISPHERE_WEST, "capture-bar-world"), (PictureMode::People, people, icons::USERS, "capture-bar-people")] {
                let b = Button::new(label).icon(glyph).variant(Variant::Secondary).size(Size::Sm).enabled(vrchat).hint(why_not).id_salt(salt);
                let bw = b.desired_width(&bu);
                let r = Rect::from_min_size(pos2(right - bw, rect.center().y - 16.0), vec2(bw, 32.0));
                if widgets::activated(&b.show_at(&mut bu, r)) {
                    chosen = Some(mode);
                }
                right = r.left() - 8.0;
            }
        } else if right - x > 200.0 {
            for (mode, label, glyph, salt) in [(PictureMode::World, world, icons::GLOBE_HEMISPHERE_WEST, "capture-bar-world"), (PictureMode::People, people, icons::USERS, "capture-bar-people")] {
                let r = Rect::from_min_size(pos2(right - 34.0, rect.center().y - 17.0), vec2(34.0, 34.0));
                let b = IconButton::new(glyph, label).size(34.0, 16.0).id_salt(salt);
                let resp = if vrchat { b.show_at(&mut bu, r) } else { bu.interact(r, Id::unique(salt), Sense::hover()).on_hover_text(why_not) };
                if !vrchat {
                    widgets::icon(&p, r.center(), glyph, 16.0, theme::fg_4());
                }
                if vrchat && widgets::activated(&resp) {
                    chosen = Some(mode);
                }
                right = r.left() - 6.0;
            }
        }
        if let Some(mode) = chosen {
            if consent {
                cx.ctl.n4n0(crate::ipc::N4n0Command::Photo3DOf { mode });
            } else {
                // The notice comes first: every option opens on it.
                state.open = true;
                state.reveal = Some(Part::Actions);
            }
        }
    }

    // What it stands at: its name, and a line.
    let room = (right - x - 12.0).max(40.0);
    let world = &snap.vrchat.world;
    let title: String = match reason {
        Some(Why::Capturing) => match (c.picture, c.state) {
            (_, CaptureState::Stopping) => tr("Saving the session…").into(),
            (_, CaptureState::Importing) => tr("Importing a raw capture").into(),
            (Some(PictureMode::People), _) => tr("Taking a 3D picture of people").into(),
            (Some(PictureMode::World), _) => tr("Taking a 3D picture of the world").into(),
            (None, CaptureState::Paused) => tr("Recording paused").into(),
            _ => tr("Recording").into(),
        },
        _ => tr("Capture").into(),
    };
    // Collapsed, the bar sums up why it would open; open, the card under it says that, and the bar what capturing
    // stands at.
    let summary = if open && reason != Some(Why::Capturing) { None } else { reason };
    let line: (String, egui::Color32) = match summary {
        Some(Why::Capturing) if open => (world.world_name.clone().filter(|_| world.in_world).unwrap_or_else(|| tr("VRChat world").into()), theme::fg_2()),
        Some(Why::Capturing) => {
            let elapsed = format::duration(c.stats.as_ref().map(|s| s.elapsed_s).unwrap_or(0.0));
            let kept = c.stats.as_ref().map_or(0, |s| s.keyframes);
            (fill(&tr_n("{elapsed}   ·   {count} keyframe", "{elapsed}   ·   {count} keyframes", kept as u64), &[("elapsed", &elapsed)]), theme::fg_2())
        }
        Some(Why::Reconstructing) => {
            let j = snap.reconstruction.as_ref().expect("a reconstruction");
            let (badge, color) = super::sessions::job_badge(j, snap);
            let name = name_of(&j.session_dir).unwrap_or_else(|| format::folder_name(&j.session_dir));
            (format!("{badge} · {name}"), color)
        }
        Some(Why::Error) => (c.error.clone().unwrap_or_default(), theme::DANGER),
        Some(Why::LeftOut) => (c.left_out.why.clone().unwrap_or_default(), if c.left_out.wearer == Left::InPicture { theme::fg_2() } else { theme::WARN }),
        Some(Why::Kit) => (tr("N4N0 is not on the avatar you wear yet: add it to capture with it").into(), theme::WARN),
        Some(Why::Finished) => {
            let name = state.finished.as_ref().and_then(|f| name_of(&f.dir)).unwrap_or_else(|| tr("Session").into());
            (orbiters_i18n::tr!("Saved · {name}", name = name), theme::OK)
        }
        None if !here && cfg!(target_os = "macos") => (tr("VRChat's camera comes to a Mac with Obelisk's engine 0.1.15 or later. Captures made elsewhere open here, in Photos & captures.").into(), theme::fg_3()),
        None if !here => (tr("VRChat's camera comes over Spout, on Windows. Captures made there open here, in Photos & captures.").into(), theme::fg_3()),
        None if !consent => (tr("Read the capture notice before your first capture").into(), theme::fg_3()),
        None if !snap.vrchat.running => (tr("Start VRChat first: its camera takes the picture").into(), theme::fg_3()),
        None => match world.world_name.clone().filter(|_| world.in_world) {
            Some(name) => (orbiters_i18n::tr!("Ready · {world}", world = name), theme::fg_2()),
            // (a Mac's own game's world is given to its captures, and shown nowhere else)
            None if snap.launcher.desktop_game.is_some() => (tr("Ready: a capture is tagged with the world you are in").into(), theme::fg_2()),
            None => (tr("VRChat is running; join a world to tag captures with it.").into(), theme::fg_3()),
        },
    };
    widgets::text_truncated(&p, pos2(x, rect.center().y - 10.0), Align2::LEFT_CENTER, &Txt::new(title, 15.0).weight(weight::SEMIBOLD).tracking(-0.01), room);
    widgets::text_truncated(&p, pos2(x, rect.center().y + 11.0), Align2::LEFT_CENTER, &Txt::new(line.0, 12.5).color(line.1), room);
    if widgets::activated(&resp) {
        *toggle = true;
    }
}

/// Every option of capturing, as the Capture tab had them: which VRChat, the three big actions (or what is under way's
/// place: the status card shows it), the picture's length with Scan and the test pattern, what capturing needs, a
/// recording's numbers and settings, the camera path and what it records. `tops` gets where each part starts.
fn options(
    ui: &mut Ui,
    cx: &mut Cx,
    upload: &mut Option<(String, Option<bool>)>,
    toolkit_asked: &mut bool,
    projects: &mut super::projects::State,
    reason: Option<Why>,
    tops: &mut Vec<(Part, f32)>,
) {
    // The world's id (verify-r21 Minor 4: the Capture tab's header showed it; two worlds can share a name), to select.
    if let Some(id) = cx.snap.vrchat.world.world_id.clone().filter(|_| cx.snap.vrchat.world.in_world) {
        world_id_row(ui, &id);
    }
    tops.push((Part::Vrchats, ui.cursor().top()));
    vrchat_picker(ui, cx);
    tops.push((Part::Actions, ui.cursor().top()));
    // While a capture runs its card is the status card, above.
    if reason != Some(Why::Capturing) {
        actions(ui, cx, upload, toolkit_asked, projects, reason == Some(Why::LeftOut));
    }
    tops.push((Part::Needs, ui.cursor().top()));
    chips(ui, cx, projects);
    tops.push((Part::Recording, ui.cursor().top()));
    // A recording's numbers and what it records: chosen and read where recording happens, the test pattern's aside.
    let here = orbiters_core::here::camera_frames();
    if here || cx.snap.is_capturing() {
        grid(ui, cx);
        camera_path(ui, cx);
    }
    // (the voice, VRChat's sound and the trackers: Windows')
    if orbiters_core::here::HERE.camera_frames {
        performance(ui, cx);
    }
}

/// The current world's id in the open options: its caption, then the id in mono, selectable to copy.
fn world_id_row(ui: &mut Ui, id: &str) {
    let label = tr("World ID");
    let r = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 0.0);
            ui.add(egui::Label::new(egui::RichText::new(label.to_uppercase()).size(11.0).color(theme::fg_3())).selectable(false));
            ui.add(egui::Label::new(egui::RichText::new(id).monospace().size(12.5).color(theme::fg_2())).selectable(true));
        })
        .response
        .rect;
    widgets::note_control(ui.ctx(), label, r, true);
}

/// A reconstruction under way: whose, its stage, how far, why it waits, and its Stop. Its card in the gallery says the
/// same; this is the one in view at the top.
fn reconstruction(ui: &mut Ui, cx: &mut Cx, name_of: &dyn Fn(&Path) -> Option<String>) -> Option<Out> {
    let snap = cx.snap;
    let j = snap.reconstruction.as_ref()?;
    let w = ui.available_width();
    let waiting = snap.reconstruct_queue.len();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 104.0), Sense::hover());
    let p = ui.painter().clone();
    widgets::card(&p, rect, theme::CARD_RADIUS);
    let name = name_of(&j.session_dir).unwrap_or_else(|| format::folder_name(&j.session_dir));
    let x = rect.left() + 22.0;
    let f = super::sessions::overall_progress(j);
    widgets::text(&p, pos2(x, rect.top() + 24.0), Align2::LEFT_CENTER, &Txt::caps(tr("Reconstructing in 3D")));
    let stop = Button::new(tr("Stop reconstruction")).icon(icons::X).variant(Variant::Secondary).size(Size::Sm).id_salt("capture-stop-reconstruction");
    let sw = stop.desired_width(ui);
    let open = Button::new(tr("Open")).icon(icons::ARROWS_OUT).variant(Variant::Ghost).size(Size::Sm).id_salt("capture-open-reconstruction");
    let ow = open.desired_width(ui);
    let stop_r = Rect::from_min_size(pos2(rect.right() - 20.0 - sw, rect.top() + 18.0), vec2(sw, 32.0));
    let open_r = Rect::from_min_size(pos2(stop_r.left() - 8.0 - ow, rect.top() + 18.0), vec2(ow, 32.0));
    let room = open_r.left() - x - 16.0;
    widgets::text_truncated(&p, pos2(x, rect.top() + 48.0), Align2::LEFT_CENTER, &Txt::new(name, 17.0).weight(weight::SEMIBOLD).tracking(-0.01), room);
    let detail = if j.paused {
        tr(super::sessions::pause_why(snap)).to_string()
    } else {
        let mut d = format!("{} · {:.0}%", tr(super::sessions::stage_label(j.stage)), f * 100.0);
        if waiting > 0 {
            d = format!("{d} · {}", orbiters_i18n::tr!("In line after it: {count}", count = waiting));
        }
        d
    };
    widgets::text_truncated(&p, pos2(rect.right() - 20.0, rect.top() + 72.0), Align2::RIGHT_CENTER, &Txt::new(&detail, 12.5).color(theme::fg_3()), rect.width() * 0.45);
    let track = Rect::from_min_size(pos2(x, rect.top() + 70.0), vec2((rect.width() * 0.5 - 22.0).max(80.0), 6.0));
    p.rect_filled(track, theme::radius(3.0), theme::white(0.07));
    p.rect_filled(Rect::from_min_size(track.min, vec2((track.width() * f.clamp(0.0, 1.0)).max(6.0), 6.0)), theme::radius(3.0), super::sessions::job_bar(j));
    if j.paused {
        widgets::text_truncated(&p, pos2(x, rect.top() + 88.0), Align2::LEFT_CENTER, &Txt::new(tr("Paused"), 12.0).color(theme::fg_3()), room);
    }
    if widgets::activated(&stop.show_at(ui, stop_r)) {
        cx.ctl.stop_reconstruction();
    }
    widgets::activated(&open.show_at(ui, open_r)).then(|| Out::Review(j.session_dir.clone()))
}

/// The last capture's error, until the next one starts.
fn error_card(ui: &mut Ui, cx: &mut Cx) {
    let Some(error) = cx.snap.capture.error.clone() else { return };
    let w = ui.available_width();
    let galley = ui.painter().layout_job(Txt::new(&error, 13.5).color(theme::fg()).line_height(19.0).job(w - 68.0));
    let (rect, _) = ui.allocate_exact_size(vec2(w, galley.size().y + 24.0 + 18.0), Sense::hover());
    let p = ui.painter();
    widgets::frame(p, rect, 14.0, theme::alpha(theme::DANGER, 0.10));
    widgets::icon(p, pos2(rect.left() + 26.0, rect.top() + 12.0 + 9.5), icons::WARNING_CIRCLE, 18.0, theme::DANGER);
    widgets::text(p, pos2(rect.left() + 48.0, rect.top() + 21.0), Align2::LEFT_CENTER, &Txt::caps(tr("The last capture stopped")));
    p.galley(pos2(rect.left() + 48.0, rect.top() + 34.0), galley, theme::fg());
}

/// N4N0 missing from the avatar worn: what it is for, and the card that puts it on (Unity projects' own).
fn kit_card(ui: &mut Ui, cx: &mut Cx, projects: &mut super::projects::State) {
    let w = ui.available_width();
    let card_w = 300.0f32.min(w * 0.45);
    let (rect, _) = ui.allocate_exact_size(vec2(w, 86.0), Sense::hover());
    let p = ui.painter().clone();
    widgets::card(&p, rect, theme::CARD_RADIUS);
    let text_room = rect.width() - card_w - 66.0;
    widgets::text(&p, pos2(rect.left() + 22.0, rect.top() + 26.0), Align2::LEFT_CENTER, &Txt::caps(tr("Capture kit")));
    let galley = p.layout_job(
        Txt::new(tr("Put N4N0 on the avatar you capture with: Orbiters installs it in its Unity project, you confirm in Unity"), 13.0)
            .color(theme::fg_2())
            .line_height(18.0)
            .job(text_room),
    );
    p.galley(pos2(rect.left() + 22.0, rect.top() + 40.0), galley, theme::fg_2());
    let at = Rect::from_min_size(pos2(rect.right() - 12.0 - card_w, rect.top() + 12.0), vec2(card_w, 62.0));
    let mut cu = ui.new_child(UiBuilder::new().max_rect(at).id_salt("capture-kit"));
    super::projects::capture_card(&mut cu, cx, projects, at);
}

/// A capture that just ended: open it (its window: look at it in 3D, share it), or put this away. Returns (open, away).
fn finished_card(ui: &mut Ui, cx: &mut Cx, finished: Option<&Finished>, name_of: &dyn Fn(&Path) -> Option<String>) -> (bool, bool) {
    let Some(f) = finished else { return (false, false) };
    let _ = cx;
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 76.0), Sense::hover());
    let p = ui.painter().clone();
    widgets::frame(&p, rect, theme::CARD_RADIUS, theme::mix(theme::surface(), theme::OK, 0.06));
    let name = name_of(&f.dir).unwrap_or_else(|| format::folder_name(&f.dir));
    let what = if f.picture.is_some() { tr("3D picture saved") } else { tr("Recording saved") };
    let x = rect.left() + 22.0;
    let away = IconButton::new(icons::X, mark("Dismiss")).size(30.0, 13.0).id_salt("capture-finished-away");
    let away_r = Rect::from_min_size(pos2(rect.right() - 16.0 - 30.0, rect.center().y - 15.0), vec2(30.0, 30.0));
    let open = Button::new(tr("Open and share")).icon(icons::ARROWS_OUT).variant(Variant::Primary).size(Size::Sm).id_salt("capture-finished-open");
    let ow = open.desired_width(ui);
    let open_r = Rect::from_min_size(pos2(away_r.left() - 10.0 - ow, rect.center().y - 16.0), vec2(ow, 32.0));
    let room = open_r.left() - x - 16.0;
    widgets::text_truncated(&p, pos2(x, rect.top() + 26.0), Align2::LEFT_CENTER, &Txt::caps(what), room);
    widgets::text_truncated(&p, pos2(x, rect.top() + 50.0), Align2::LEFT_CENTER, &Txt::new(name, 16.0).weight(weight::SEMIBOLD).tracking(-0.01), room);
    let review = widgets::activated(&open.show_at(ui, open_r));
    let dismiss = widgets::activated(&away.show_at(ui, away_r));
    (review, dismiss)
}

/// The height of the three big actions, and of the card that takes their place while something is under way.
const ACTION_H: f32 = 176.0;

fn rec_state(s: CaptureState) -> RecState {
    match s {
        CaptureState::Idle => RecState::Idle,
        CaptureState::Starting => RecState::Starting,
        CaptureState::Recording => RecState::Recording,
        CaptureState::Paused => RecState::Paused,
        CaptureState::Stopping => RecState::Stopping,
        CaptureState::Importing => RecState::Importing,
    }
}

/// What the page is for: a 3D picture of people, a 3D picture of the world, or a free recording. While one of them is under way,
/// what it is doing and how to stop it take their place. Before the capture notice is accepted, the notice does.
#[allow(clippy::too_many_arguments)]
fn actions(ui: &mut Ui, cx: &mut Cx, upload: &mut Option<(String, Option<bool>)>, toolkit_asked: &mut bool, projects: &mut super::projects::State, why_above: bool) {
    let snap = cx.snap;
    if !snap.settings.consent_acknowledged {
        return consent(ui, cx);
    }
    if snap.is_capturing() || snap.capture.state == CaptureState::Stopping {
        return under_way(ui, cx);
    }
    if !orbiters_core::here::camera_frames() {
        return recording_elsewhere(ui, cx);
    }
    // The worn avatar's upload, read once per avatar: a picture of the world says beforehand that it would show.
    let worn = snap.n4n0.avatar_id.clone();
    if upload.as_ref().map(|(id, _)| id) != worn.as_ref() {
        *upload = worn.map(|id| {
            let can = crate::controller::upload_can_hide(&id);
            (id, can)
        });
    }
    let cannot_hide = upload.as_ref().is_some_and(|(_, can)| *can == Some(false));
    let vrchat = snap.vrchat.running;
    let w = ui.available_width();
    let gap = 16.0;
    let card_w = (w - 2.0 * gap) / 3.0;
    let (rect, _) = ui.allocate_exact_size(vec2(w, ACTION_H), Sense::hover());
    let (rect, alpha) = entering(ui, cx, 1, rect);
    let mut row = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("capture-actions"));
    row.multiply_opacity(alpha);
    let cell = |i: usize| Rect::from_min_size(rect.min + vec2(i as f32 * (card_w + gap), 0.0), vec2(card_w, ACTION_H));
    let not_running = tr("Start VRChat first: its camera takes the picture");
    let world_note = if cannot_hide { tr("Goes through the place; your avatar is taken out afterwards") } else { tr("Goes through the place, avatars left out") };
    // (A text written as `field: tr("…")` would not be read as a key by tests/coverage.rs's scan: a `let` is.)
    let people_title = tr("3D picture of people");
    let people_note = tr("Goes around the people here");
    let place_title = tr("3D picture of the world");
    // A recording starts beside VRChat's camera in VR alone, the picture it records (Enzo, F15): here it says how, and is its Stop while it runs.
    let record_title = tr("Record a 3D scene");
    let record_note = tr("In VR, open VRChat's camera: Record is beside it.");
    let people = BigAction {
        glyph: icons::USERS,
        tint: theme::VIOLET,
        title: people_title,
        note: people_note,
        warn: false,
        enabled: vrchat,
        why_not: not_running,
    };
    let place = BigAction {
        glyph: icons::GLOBE_HEMISPHERE_WEST,
        tint: theme::accent(),
        title: place_title,
        note: world_note,
        warn: false,
        enabled: vrchat,
        why_not: not_running,
    };
    let record = BigAction {
        glyph: icons::RECORD,
        tint: theme::REC,
        title: record_title,
        note: record_note,
        warn: false,
        enabled: false,
        why_not: record_note,
    };
    let chosen = [
        people.show(&mut row, cell(0), "capture-people").then_some(PictureMode::People),
        place.show(&mut row, cell(1), "capture-world").then_some(PictureMode::World),
    ];
    if let Some(mode) = chosen.into_iter().flatten().next() {
        cx.ctl.n4n0(crate::ipc::N4n0Command::Photo3DOf { mode });
    }
    record.show(&mut row, cell(2), record::LABEL);
    // What the last capture could not do is the status card's, above, when it shows.
    if !why_above {
        why_last(ui, cx, toolkit_asked, projects);
    }
    quiet_row(ui, cx);
}

/// In place of the three big actions where VRChat does not run: they all fly or read its camera. The test pattern stays, and
/// tries the whole path from the capture worker to a reconstruction.
fn recording_elsewhere(ui: &mut Ui, cx: &mut Cx) {
    let mut test = false;
    // VRChat runs on a Linux computer, and its camera cannot be read there yet.
    let (sub, note) = if orbiters_core::here::LINUX {
        (
            tr("Not there yet on Linux"),
            tr("Orbiters - N4N0 cannot read VRChat's camera on Linux yet. The test pattern records a generated picture through the capture worker, to try the whole path. It shows in Free up space, not among your pictures."),
        )
    } else {
        (
            tr("Taken on the computer that runs VRChat"),
            tr("Without a VRChat here, the test pattern records a generated picture through the capture worker, to try the whole path. It shows in Free up space, not among your pictures."),
        )
    };
    super::section(ui, cx, 1, icons::CUBE_FOCUS, tr("3D pictures and recordings"), Some(sub), |ui| {
        widgets::field(ui, tr("Test pattern"), Some(note), 150.0, |ui| {
            test = Button::new(tr("Test pattern")).icon(icons::FLASK).size(Size::Sm).show(ui).clicked();
        });
    });
    if test {
        cx.ctl.start_test_capture();
    }
}

/// A big, clear button: a filled card in its colour, its icon, what it does in a few words.
struct BigAction<'a> {
    glyph: &'a str,
    tint: egui::Color32,
    title: &'a str,
    note: &'a str,
    warn: bool,
    enabled: bool,
    why_not: &'a str,
}

impl BigAction<'_> {
    fn show(&self, ui: &mut Ui, rect: Rect, label: &str) -> bool {
        let ctx = ui.ctx().clone();
        let id = Id::unique(("capture-action", label));
        let resp = ui.interact(rect, id, if self.enabled { Sense::click() } else { Sense::hover() });
        widgets::note_control(&ctx, label, rect, self.enabled);
        let hover = anim::toggle(&ctx, id.with("hover"), self.enabled && resp.hovered(), Spring::SOFT);
        let press = anim::toggle(&ctx, id.with("press"), self.enabled && resp.is_pointer_button_down_on(), Spring::SNAPPY);
        let clicked = self.enabled && widgets::pressed(&resp);
        if clicked {
            widgets::note_press(label);
        }
        if self.enabled {
            resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        } else if !self.why_not.is_empty() {
            resp.on_hover_text(self.why_not);
        }
        let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.02 * press));
        let mut p = ui.painter().clone();
        if !self.enabled {
            p.multiply_opacity(0.5);
        }
        widgets::frame(&p, r, theme::CARD_RADIUS, theme::mix(theme::surface(), self.tint, 0.10 + 0.08 * hover));
        let chip = Rect::from_min_size(r.min + vec2(22.0, 22.0), vec2(56.0, 56.0));
        p.rect_filled(chip, theme::radius(17.0), theme::alpha(self.tint, 0.18));
        widgets::icon(&p, chip.center(), self.glyph, 28.0, self.tint);
        widgets::text_truncated(
            &p,
            pos2(r.left() + 22.0, r.bottom() - 62.0),
            Align2::LEFT_CENTER,
            &Txt::new(self.title, 20.0).weight(weight::SEMIBOLD),
            r.width() - 44.0,
        );
        let note = if self.enabled { self.note } else { self.why_not };
        let (color, line) = if self.warn && self.enabled { (theme::WARN, 18.0) } else { (theme::fg_2(), 18.0) };
        let galley = p.layout_job(Txt::new(note, 13.5).color(color).line_height(line).job(r.width() - 44.0));
        p.galley(pos2(r.left() + 22.0, r.bottom() - 46.0), galley, color);
        clicked
    }
}

/// What the last capture could not do, until the next one starts: the toast that said so is gone, and the person may have been
/// in a headset.
fn why_last(ui: &mut Ui, cx: &mut Cx, toolkit_asked: &mut bool, projects: &mut super::projects::State) {
    let ctx = ui.ctx().clone();
    if *toolkit_asked && !projects.toolkit_running() {
        *toolkit_asked = false;
        if let Some(e) = projects.take_error() {
            cx.toasts.push(&ctx, orbiters_ui::toast::Kind::Error, e);
        }
    }
    let Some(why) = cx.snap.capture.left_out.why.clone() else { return };
    // The worn avatar could not hide: Update the Toolkit, where its project is on this computer.
    let wearer = cx.snap.capture.left_out.wearer == Left::InPicture;
    let worn = cx.snap.n4n0.avatar_id.clone().filter(|_| wearer);
    if worn.is_some() {
        projects.toolkit_look(&ctx);
    }
    let offer = worn.as_deref().map(|id| projects.toolkit_offer(id));
    let name = worn.as_deref().and_then(crate::controller::uploaded_avatar_name);
    let tone = if wearer { Tone::Info.color() } else { theme::WARN };
    let (button, note) = match &offer {
        Some(super::projects::ToolkitOffer::Project(name)) => (Some(tr!("Update the Toolkit in {name}", name = name)), None),
        Some(super::projects::ToolkitOffer::NotHere) => (None, Some(super::projects::toolkit_refusal(name.as_deref()))),
        Some(super::projects::ToolkitOffer::Refused(why)) => (None, Some(why.clone())),
        _ => (None, None),
    };
    let text = note.map_or(why.clone(), |n| format!("{why} {n}"));
    let w = ui.available_width();
    let ink = if wearer { theme::fg() } else { tone };
    let galley = ui.painter().layout_job(Txt::new(text, 13.5).color(ink).line_height(19.0).job(w - 68.0));
    let button_h = if button.is_some() { 44.0 } else { 0.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(w, galley.size().y + 24.0 + button_h), Sense::hover());
    let p = ui.painter();
    widgets::frame(p, rect, 14.0, theme::alpha(tone, 0.10));
    widgets::icon(p, pos2(rect.left() + 26.0, rect.top() + 12.0 + 9.5), if wearer { icons::INFO } else { icons::WARNING }, 18.0, tone);
    let text_bottom = rect.top() + 12.0 + galley.size().y;
    p.galley(pos2(rect.left() + 48.0, rect.top() + 12.0), galley, ink);
    if let (Some(label), Some(id)) = (button, worn) {
        let running = projects.toolkit_running();
        let b = Button::new(&label)
            .icon(icons::ARROW_UP)
            .variant(Variant::Secondary)
            .size(Size::Sm)
            .enabled(!running)
            .id_salt("update-toolkit")
            .hint(tr("Updates Orbiters Toolkit in that project and opens it in Unity. You upload from VRChat's SDK panel: N4N0 uploads nothing by itself."));
        let bw = b.desired_width(ui);
        let at = Rect::from_min_size(pos2(rect.left() + 48.0, text_bottom + 8.0), vec2(bw, 32.0));
        if widgets::activated(&b.show_at(ui, at)) {
            projects.update_toolkit(&ctx, &id, name.as_deref());
            *toolkit_asked = true;
        }
    }
}

/// What rarely changes: how long a picture flies, the longer scan and the test pattern. In reach and out of the way.
fn quiet_row(ui: &mut Ui, cx: &mut Cx) {
    let snap = cx.snap;
    let mut scan = false;
    let mut test = false;
    block(ui, cx, 2, 36.0, |ui, rect| {
        let p = ui.painter();
        let label = widgets::text(p, pos2(rect.left() + 2.0, rect.center().y), Align2::LEFT_CENTER, &Txt::new(tr("Picture length"), 13.0).color(theme::fg_3()));
        let mut length = snap.settings.photo_3d;
        let lengths = [Photo3dLength::Quick, Photo3dLength::Standard, Photo3dLength::Full]
            .into_iter()
            .zip(flight_labels().into_iter().zip(FLIGHT_HINTS))
            .map(|(length, (label, hint))| SegOption::new(length, label).hint(tr(hint)))
            .collect::<Vec<_>>();
        let seg_rect = Rect::from_min_size(pos2(label.right() + 12.0, rect.top()), vec2(150.0, 36.0));
        let mut seg = ui.new_child(UiBuilder::new().max_rect(seg_rect).layout(Layout::left_to_right(Align::Center)));
        if widgets::segmented(&mut seg, "photo-3d-length", &mut length, &lengths, true) {
            super::form::save(cx.ctl, &snap.settings, &Settings { photo_3d: length, ..Settings::clone(&snap.settings) });
        }
        let consent = snap.settings.consent_acknowledged;
        let t = Button::new(tr("Test pattern"))
            .icon(icons::FLASK)
            .variant(Variant::Ghost)
            .size(Size::Sm)
            .enabled(consent)
            .hint(tr("Record a generated test pattern (no VRChat needed)"));
        let tw = t.desired_width(ui);
        let test_at = Rect::from_min_size(pos2(rect.right() - tw, rect.top() + 2.0), vec2(tw, 32.0));
        test = t.show_at(ui, test_at).clicked();
        // The camera looks around, then circles at three heights, looking in and out: views a walk never gives.
        let s = Button::new(tr("Scan"))
            .icon(icons::PATH)
            .variant(Variant::Ghost)
            .size(Size::Sm)
            .enabled(consent && snap.vrchat.running)
            .hint(tr("VRChat's camera looks around, then circles where it stands at three heights, looking in and out (about a minute); the recording is then reconstructed in 3D"));
        let sw = s.desired_width(ui);
        let sr = Rect::from_min_size(pos2(test_at.left() - 8.0 - sw, rect.top() + 2.0), vec2(sw, 32.0));
        scan = s.show_at(ui, sr).clicked();
    });
    if test {
        cx.ctl.start_test_capture();
    }
    if scan {
        cx.ctl.scan();
    }
}

/// What a capture under way is doing, who it left out, and how to stop it: in the place of the three actions.
fn under_way(ui: &mut Ui, cx: &mut Cx) {
    let ctx = ui.ctx().clone();
    let snap = cx.snap;
    let c = &snap.capture;
    let recording = matches!(c.state, CaptureState::Recording | CaptureState::Paused);
    // Who is out of the picture, or why not, or why no keyframes come.
    let warning = c.left_out.why.clone().or_else(|| c.no_strip.map(|n| n.message().to_string()));
    let w = ui.available_width();
    // Room for that as well as a free recording's Pause.
    let extra = if warning.is_some() && recording && c.picture.is_none() { 36.0 } else { 0.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(w, ACTION_H + extra), Sense::hover());
    let (rect, alpha) = entering(ui, cx, 1, rect);
    let mut ui = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("capture-under-way"));
    ui.multiply_opacity(alpha);
    let tint = match c.picture {
        Some(PictureMode::People) => theme::VIOLET,
        Some(PictureMode::World) => theme::accent(),
        None => theme::REC,
    };
    widgets::frame(ui.painter(), rect, theme::CARD_RADIUS, theme::mix(theme::surface(), tint, 0.10));

    // The button at the right: the same one, now stopping.
    let size = ACTION_H - 40.0;
    let button = Rect::from_min_size(pos2(rect.right() - size - 28.0, rect.top() + 12.0), Vec2::splat(size));
    let health = c.stats.as_ref().map(|s| if s.frames > 0 { s.strip_ok as f32 / s.frames as f32 } else { 1.0 }).unwrap_or(1.0);
    let rb = RecordButton { state: rec_state(c.state), disabled: c.state == CaptureState::Importing, health };
    let (_, toggled) = rb.show_at(&mut ui, button);
    if toggled && snap.is_capturing() {
        cx.ctl.stop_capture();
    }
    let stop = match c.state {
        CaptureState::Stopping => tr("Saving…"),
        CaptureState::Importing => tr("Importing…"),
        CaptureState::Starting => tr("Starting…"),
        _ if c.picture.is_some() => tr("Stop this picture"),
        _ => tr("Stop recording"),
    };
    widgets::text(
        ui.painter(),
        pos2(button.center().x, rect.bottom() - 16.0),
        Align2::CENTER_CENTER,
        &Txt::new(stop, 13.0).weight(weight::MEDIUM).color(theme::fg_2()),
    );

    // What is happening.
    let x = rect.left() + 26.0;
    let room = button.left() - 24.0 - x;
    let title = match c.picture {
        Some(PictureMode::People) => tr("Taking a 3D picture of people"),
        Some(PictureMode::World) => tr("Taking a 3D picture of the world"),
        None if c.state == CaptureState::Importing => tr("Importing a raw capture"),
        None if c.state == CaptureState::Paused => tr("Recording paused"),
        None => tr("Recording"),
    };
    let p = ui.painter().clone();
    if recording {
        let dot = pos2(x + 5.0, rect.top() + 28.0);
        widgets::status_dot(&p, dot, Tone::Rec, c.state == CaptureState::Recording);
        let elapsed = format::duration(c.stats.as_ref().map(|s| s.elapsed_s).unwrap_or(0.0));
        let kept = c.stats.as_ref().map_or(0, |s| s.keyframes);
        let line = fill(&tr_n("{elapsed}   ·   {count} keyframe", "{elapsed}   ·   {count} keyframes", kept as u64), &[("elapsed", &elapsed)]);
        widgets::text(&p, pos2(x + 18.0, rect.top() + 28.0), Align2::LEFT_CENTER, &Txt::new(line, 13.0).weight(weight::MEDIUM).color(theme::fg()));
    }
    widgets::text_truncated(&p, pos2(x, rect.top() + 60.0), Align2::LEFT_CENTER, &Txt::new(title, 24.0).weight(weight::SEMIBOLD).tracking(-0.01), room);
    let doing = match (c.picture, c.state) {
        (_, CaptureState::Stopping) => tr("Saving the session…").to_string(),
        (Some(PictureMode::People), _) => tr("VRChat's camera goes around the people here, then comes back where it was").to_string(),
        (Some(PictureMode::World), _) => tr("VRChat's camera goes through the place, then comes back where it was").to_string(),
        // Nothing comes before VRChat's camera streams (the worker names the sender once it is connected).
        _ if snap.sources.is_empty() && c.stats.as_ref().is_none_or(|s| s.frames == 0) => {
            if snap.settings.osc.enabled && snap.settings.osc.drive_camera {
                tr("Waiting for VRChat's camera: Orbiters asked for Stream mode with Spout Stream on (OSC must be on in VRChat)").to_string()
            } else {
                tr("Waiting for VRChat's camera: open it in Stream mode and turn Spout Stream on").to_string()
            }
        }
        _ => c.source.clone().unwrap_or_default(),
    };
    widgets::text_truncated(&p, pos2(x, rect.top() + 88.0), Align2::LEFT_CENTER, &Txt::new(doing, 13.5).color(theme::fg_2()), room);
    if let Some(warning) = warning {
        // The wearer comes out afterwards: nothing is going wrong, so it reads as a note.
        let ink = if c.left_out.wearer == Left::InPicture { theme::fg_2() } else { theme::WARN };
        let galley = p.layout_job(Txt::new(warning, 13.0).color(ink).line_height(17.0).job(room));
        p.galley(pos2(x, rect.top() + 104.0), galley, ink);
    } else {
        // Whole sentences for each case: a joined list could not be put in order or punctuated in every language.
        let out = match (c.left_out.wearer == Left::Out, c.left_out.others == Left::Out) {
            (true, true) => Some(tr("Your avatar is hidden on your side, other players are hidden from the camera")),
            (true, false) => Some(tr("Your avatar is hidden on your side")),
            (false, true) => Some(tr("other players are hidden from the camera")),
            (false, false) => None,
        };
        if let Some(out) = out {
            widgets::text_truncated(&p, pos2(x, rect.top() + 114.0), Align2::LEFT_CENTER, &Txt::new(out, 13.5).color(theme::accent()), room);
        }
    }
    // A free recording's pause, and its folder.
    if recording && c.picture.is_none() {
        let paused = c.state == CaptureState::Paused;
        let pause = Button::new(if paused { tr("Resume") } else { tr("Pause") }).icon(if paused { icons::PLAY } else { icons::PAUSE }).size(Size::Sm);
        let folder = c.session_dir.as_ref().map(|d| (d.clone(), format::folder_name(d)));
        let folder_btn = folder.as_ref().map(|(_, name)| Button::new(name).icon(icons::FOLDER_OPEN).variant(Variant::Ghost).size(Size::Sm));
        let pw = pause.desired_width(&ui);
        let y = rect.bottom() - 46.0;
        if pause.show_at(&mut ui, Rect::from_min_size(pos2(x, y), vec2(pw, 32.0))).clicked() {
            cx.ctl.pause_capture(!paused);
        }
        if let (Some(b), Some((dir, _))) = (folder_btn, folder.as_ref()) {
            let fw = b.desired_width(&ui).min(220.0).min(room - pw - 8.0);
            if b.width(fw).show_at(&mut ui, Rect::from_min_size(pos2(x + pw + 8.0, y), vec2(fw, 32.0))).clicked() {
                let r = crate::controller::open_path(dir);
                cx.run(&ctx, r);
            }
        }
    }
}

/// Several VRChats on this computer: which is which (account, world, who drives it, desktop or VR, its camera), and which
/// one pictures are taken in. One click on another chooses it. Nothing shows with one VRChat.
fn vrchat_picker(ui: &mut Ui, cx: &mut Cx) {
    let snap = cx.snap;
    let runs = &snap.vrchat.runs;
    if runs.len() < 2 || !orbiters_core::here::camera_frames() {
        return;
    }
    let target = runs.iter().find(|r| r.target);
    let note = target.and_then(|t| match &t.driver {
        VrchatDriverShown::Agent { .. } => Some(tr("An agent drives this VRChat through the bridge. While you take a picture here, it waits, and its own pictures wait for you.")),
        VrchatDriverShown::Left => Some(tr("An agent started this VRChat and is gone: nobody drives it.")),
        VrchatDriverShown::Person => None,
    });
    let busy = snap.is_capturing();
    const CARD_H: f32 = 96.0;
    let height = 30.0 + CARD_H + if note.is_some() { 30.0 } else { 0.0 };
    let mut chosen = None;
    block(ui, cx, 20, height, |ui, rect| {
        let p = ui.painter().clone();
        let caption = tr_n("{count} VRChat on this computer", "{count} VRChats on this computer", runs.len() as u64);
        widgets::text(&p, pos2(rect.left(), rect.top() + 8.0), Align2::LEFT_CENTER, &Txt::caps(&caption));
        let row = Rect::from_min_size(pos2(rect.left(), rect.top() + 26.0), vec2(rect.width(), CARD_H));
        let gap = 12.0;
        // The row filled (two or three side by side), never one card too wide to read across.
        let width = ((row.width() - gap * (runs.len() as f32 - 1.0)) / runs.len() as f32).min(560.0);
        for (i, run) in runs.iter().enumerate() {
            let card = Rect::from_min_size(pos2(row.left() + i as f32 * (width + gap), row.top()), vec2(width, CARD_H));
            let id = Id::unique(("vrchat-pick", run.pid));
            let resp = ui.interact(card, id, Sense::click());
            let can = !run.target && !busy;
            let resp = if can { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
            if can && widgets::pressed(&resp) {
                chosen = Some(run.pid);
            }
            vrchat_card(ui, card, id, run, can && resp.hovered());
        }
        let hint = if busy { tr("Finish the current capture first.") } else { tr("Pictures are taken in the one you pick") };
        let right = row.left() + runs.len() as f32 * (width + gap) - gap;
        widgets::text(&p, pos2(right, rect.top() + 8.0), Align2::RIGHT_CENTER, &Txt::new(hint, 12.5).color(theme::fg_3()));
        if let Some(note) = note {
            let y = row.bottom() + 18.0;
            widgets::icon(&p, pos2(rect.left() + 8.0, y), icons::INFO, 14.0, theme::fg_3());
            widgets::text_truncated(&p, pos2(rect.left() + 24.0, y), Align2::LEFT_CENTER, &Txt::new(note, 12.5).color(theme::fg_2()), rect.width() - 24.0);
        }
    });
    if let Some(pid) = chosen {
        cx.ctl.choose_vrchat(Some(pid));
    }
}

/// One VRChat of the picker: the one pictures are taken in lit, with its mark.
fn vrchat_card(ui: &Ui, card: Rect, id: Id, run: &VrchatRunShown, hovered: bool) {
    let p = ui.painter();
    let lit = anim::toggle(ui.ctx(), id.with("lit"), run.target, Spring::SOFT);
    let hover = anim::toggle(ui.ctx(), id.with("hover"), hovered, Spring::SNAPPY);
    let fill = theme::alpha(theme::accent(), 0.08 * lit);
    widgets::frame(p, card, 16.0, theme::surface());
    p.rect_filled(card, theme::radius(16.0), fill);
    p.rect_filled(card, theme::radius(16.0), theme::white(0.03 * hover));
    if lit > 0.01 {
        p.rect_stroke(card, theme::radius(16.0), egui::Stroke::new(1.5, theme::alpha(theme::accent(), 0.85 * lit)), egui::StrokeKind::Inside);
    }
    let (glyph, tint) = match run.driver {
        VrchatDriverShown::Person => (icons::USER, theme::accent()),
        VrchatDriverShown::Agent { .. } => (icons::SPARKLE, theme::CYAN),
        VrchatDriverShown::Left => (icons::SPARKLE, theme::fg_4()),
    };
    let tile = Rect::from_min_size(pos2(card.left() + 14.0, card.top() + 14.0), vec2(36.0, 36.0));
    p.rect_filled(tile, theme::radius(12.0), theme::alpha(tint, 0.12));
    widgets::icon(p, tile.center(), glyph, 16.0, tint);
    let x = tile.right() + 12.0;
    // Its mark on the right: where pictures are taken, or the way to choose it.
    let mark_w = if run.target {
        let label = Txt::new(tr("Pictures here"), 12.0).weight(weight::MEDIUM).color(theme::accent());
        let w = widgets::text_size(ui, &label).x + 30.0;
        let pill = Rect::from_min_size(pos2(card.right() - 12.0 - w, card.top() + 12.0), vec2(w, 24.0));
        p.rect_filled(pill, theme::radius(12.0), theme::alpha(theme::accent(), 0.14));
        widgets::icon(p, pos2(pill.left() + 13.0, pill.center().y), icons::CHECK, 12.0, theme::accent());
        widgets::text(p, pos2(pill.left() + 23.0, pill.center().y), Align2::LEFT_CENTER, &label);
        w + 12.0
    } else if hovered {
        let label = Txt::new(tr("Take pictures here"), 12.0).weight(weight::MEDIUM).color(theme::fg_2());
        let w = widgets::text_size(ui, &label).x;
        widgets::text(p, pos2(card.right() - 14.0, card.top() + 24.0), Align2::RIGHT_CENTER, &label);
        w + 14.0
    } else {
        0.0
    };
    let room = card.right() - x - 12.0 - mark_w;
    let name = crate::controller::vrchats::who(run);
    widgets::text_truncated(p, pos2(x, card.top() + 24.0), Align2::LEFT_CENTER, &Txt::new(name, 15.0).weight(weight::SEMIBOLD), room);
    let world = run.world.clone().unwrap_or_else(|| tr("Not in a world yet").into());
    widgets::text_truncated(p, pos2(x, card.top() + 43.0), Align2::LEFT_CENTER, &Txt::new(world, 12.5).color(theme::fg_2()), card.right() - x - 14.0);
    // Who drives it, how it runs, and its camera.
    let driver = match &run.driver {
        VrchatDriverShown::Person => tr("Your own").to_string(),
        VrchatDriverShown::Agent { session } => tr!("Agent · {session}", session = session),
        VrchatDriverShown::Left => tr("Left by an agent").into(),
    };
    let mode = match run.desktop {
        Some(true) => Some(tr("Desktop")),
        Some(false) => Some(tr("VR")),
        None => None,
    };
    let line = match mode {
        Some(mode) => format!("{driver}  ·  {mode}"),
        None => driver,
    };
    let y = card.bottom() - 22.0;
    let left = card.left() + 16.0;
    let used = widgets::text_size(ui, &Txt::new(&line, 12.0)).x;
    widgets::text_truncated(p, pos2(left, y), Align2::LEFT_CENTER, &Txt::new(&line, 12.0).color(theme::fg_3()), card.width() * 0.6);
    if let Some(open) = run.camera {
        let cx_ = left + used.min(card.width() * 0.6) + 18.0;
        // Closed on the one pictures are taken in: what to do before a picture, in amber.
        let (tone, ink) = match (open, run.target) {
            (true, _) => (Tone::Ok, theme::fg_2()),
            (false, true) => (Tone::Warn, theme::WARN),
            (false, false) => (Tone::Off, theme::fg_3()),
        };
        widgets::status_dot(p, pos2(cx_, y), tone, false);
        let words = if open { tr("Camera open") } else { tr("Camera closed") };
        widgets::text_truncated(p, pos2(cx_ + 10.0, y), Align2::LEFT_CENTER, &Txt::new(words, 12.0).color(ink), card.right() - cx_ - 24.0);
    }
}

/// What capturing needs, as status cards: VRChat, N4N0 on the avatar (the one card to act on: it opens the avatar
/// picker), the Spout source, the Action Menu and the storage. Five on a row, or three then two on narrow windows.
fn chips(ui: &mut Ui, cx: &mut Cx, projects: &mut super::projects::State) {
    let snap = cx.snap;
    let world = &snap.vrchat.world;
    let source = if snap.settings.spout_sender.is_empty() {
        snap.sources.iter().find(|s| s.name.to_lowercase().contains("vrchat")).or(snap.sources.first())
    } else {
        snap.sources.iter().find(|s| s.name == snap.settings.spout_sender)
    };
    let osc = &snap.osc;
    let storage = &snap.storage;
    // Several VRChats: each line is about the one pictures are taken in, and says which.
    let vrchat_label = match snap.vrchat.several_target() {
        Some(target) => tr!("VRChat · {who}", who = crate::controller::vrchats::who(target)),
        None => "VRChat".to_string(),
    };
    let chips = [
        if orbiters_core::osc::VRCHAT_RUNS_HERE {
            (
                icons::MONITOR_PLAY,
                vrchat_label.as_str(),
                if snap.vrchat.running {
                    if world.in_world {
                        tr("In a world").to_string()
                    } else {
                        tr("Running").into()
                    }
                } else {
                    tr("Not running").into()
                },
                if snap.vrchat.running { Tone::Ok } else { Tone::Off },
            )
        } else {
            let (text, tone) = super::network::sidebar_status(&snap.lan);
            (icons::MONITOR_PLAY, "VRChat", text, tone)
        },
        if orbiters_core::here::camera_frames() {
            (
                icons::BROADCAST,
                tr("Spout source"),
                // Without a sender, what to do, not a state: "VRChat's camera: Stream mode, Spout Stream on" read as one (2026-11-15).
                source.map(|s| format!("{} · {}×{}", s.name, s.width, s.height)).unwrap_or_else(|| tr("Nothing yet: VRChat's camera, Stream mode").into()),
                if source.is_some() { Tone::Ok } else { Tone::Warn },
            )
        } else {
            (icons::BROADCAST, tr_ctx("the noun", "Recording"), tr("Where VRChat runs").into(), Tone::Off)
        },
        (
            icons::SPARKLE,
            tr("Action Menu"),
            osc.last_event
                .clone()
                .unwrap_or_else(|| if osc.listening { tr("N4N0 › Record").into() } else { osc.error.clone().unwrap_or_else(|| tr("OSC off").into()) }),
            if osc.listening {
                Tone::Ok
            } else if osc.enabled {
                Tone::Warn
            } else {
                Tone::Off
            },
        ),
        (
            icons::HARD_DRIVE,
            tr("Storage"),
            match storage.free_bytes {
                Some(free) => tr!("{used} used · {free} free", used = format::bytes(storage.used_bytes as f64), free = format::bytes(free as f64)),
                None => tr!("{used} used", used = format::bytes(storage.used_bytes as f64)),
            },
            if storage.free_bytes.is_some_and(|f| f < 20_000_000_000) { Tone::Warn } else { Tone::Ok },
        ),
    ];
    // Each card as wide as its words want, the row filled; three then two on windows too narrow for that.
    let width = ui.available_width();
    let want = |label: &str, value: &str| {
        14.0 + 36.0 + 12.0 + widgets::text_size(ui, &Txt::caps(label)).x.max(widgets::text_size(ui, &Txt::new(value, 13.0).weight(weight::MEDIUM)).x) + 14.0
    };
    let mut wanted: Vec<f32> = chips.iter().map(|(_, label, value, _)| want(label, value)).collect();
    wanted.insert(1, want("N4N0", tr("Add to my avatar")) + 26.0);
    // One row only while the words fit whole (below 1 they are cut): a language with longer words (French) takes two rows rather than cut them.
    let one_row = orbiters_ui::layout::share_scale(&wanted, width, 12.0) >= 1.0;
    let rows: Vec<std::ops::Range<usize>> = if one_row { vec![0..5] } else { vec![0..3, 3..5] };
    let (rect, _) = ui.allocate_exact_size(vec2(width, rows.len() as f32 * 74.0 - 12.0), Sense::hover());
    let (rect, alpha) = entering(ui, cx, 1, rect);
    let mut cells = Vec::with_capacity(5);
    for (r, range) in rows.iter().enumerate() {
        // Each row filled, as wide as its cards' words want (the second row's two cards included).
        let widths = orbiters_ui::layout::share(&wanted[range.clone()], rect.width(), 12.0);
        let mut x = rect.left();
        for w in widths {
            cells.push(Rect::from_min_size(pos2(x, rect.top() + r as f32 * 74.0), vec2(w, 62.0)));
            x += w + 12.0;
        }
    }
    let cell = |i: usize| cells[i];
    let mut cu = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("capture-chips"));
    cu.multiply_opacity(alpha);
    // N4N0 second, after VRChat.
    for (i, (glyph, label, value, tone)) in chips.iter().enumerate() {
        let slot = if i == 0 { 0 } else { i + 1 };
        widgets::status_chip(&mut cu, cell(slot), Id::unique(("capture-chip", i)), glyph, label, value, *tone);
    }
    super::projects::capture_card(&mut cu, cx, projects, cell(1));
}

fn grid(ui: &mut Ui, cx: &mut Cx) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), GRID_H), Sense::hover());
    tiles(ui, cx, rect);
    recording_settings(ui, cx);
}

/// How a free recording is kept: what it does while recording, and how much of it stays. On one row: it rarely changes.
fn recording_settings(ui: &mut Ui, cx: &mut Cx) {
    let snap = cx.snap;
    let settings = &snap.settings;
    block(ui, cx, 9, 76.0, |ui, card| {
        widgets::card(ui.painter(), card, theme::CARD_RADIUS);
        let p = ui.painter().clone();
        widgets::text(&p, pos2(card.left() + 22.0, card.top() + 28.0), Align2::LEFT_CENTER, &Txt::caps(tr_ctx("the noun", "Recording")));
        let sub = if snap.osc.listening { tr("Also from your Action Menu") } else { tr("Saved as a session") };
        widgets::text_truncated(&p, pos2(card.left() + 22.0, card.top() + 50.0), Align2::LEFT_CENTER, &Txt::new(sub, 12.5).color(theme::fg_3()), 180.0);
        let mut mode = settings.capture_mode;
        let mut quality = settings.storage_quality;
        let modes = [
            SegOption::new(CaptureMode::RecordOnly, tr("Record")).hint(tr("Record now, reconstruct afterwards with the whole GPU")),
            SegOption::new(CaptureMode::LiveRefine, tr("Live + refine")).hint(tr("Map live while playing, refine afterwards")).disabled(tr("soon")),
            SegOption::new(CaptureMode::LiveOnly, tr("Live only")).hint(tr("The live map is the result")).disabled(tr("soon")),
        ];
        let qualities = [
            SegOption::new(StorageQuality::Lossless, tr("Lossless")).hint(tr("Every frame saved losslessly (very large)")),
            SegOption::new(StorageQuality::High, tr("High")).hint(tr("Lossless keyframes, poses and depth")),
            SegOption::new(StorageQuality::Compact, tr("Compact")).hint(tr("Fewer keyframes, smaller sessions")),
        ];
        let area = Rect::from_min_max(pos2(card.left() + 200.0, card.top()), pos2(card.right() - 20.0, card.bottom()));
        let mut seg = ui.new_child(UiBuilder::new().max_rect(area).layout(Layout::right_to_left(Align::Center)));
        seg.spacing_mut().item_spacing = vec2(16.0, 0.0);
        let quality_changed = widgets::segmented(&mut seg, "storage-quality", &mut quality, &qualities, true);
        let mode_changed = widgets::segmented(&mut seg, "capture-mode", &mut mode, &modes, false);
        if mode_changed || quality_changed {
            super::form::save(cx.ctl, settings, &Settings { capture_mode: mode, storage_quality: quality, ..Settings::clone(settings) });
        }
    });
}

/// The capture notice, in the place of the actions until it is accepted.
fn consent(ui: &mut Ui, cx: &mut Cx) {
    let w = ui.available_width();
    let mut bullets = vec![
        tr("Capture your own worlds, or worlds whose creators agreed, and keep the results private."),
        tr("Other players are left out: a 3D picture of the world hides them from the Stream camera, or turn Remote Players off."),
    ];
    // With VRChat sign-in open to the member, the app can hold their VRChat session: the promise would not hold.
    if !cx.snap.vrchat_login.allowed {
        bullets.push(tr("Orbiters - N4N0 never asks for your VRChat login and only reads VRChat's local log, OSC and Spout."));
    }
    let galleys: Vec<_> = bullets.iter().map(|b| ui.painter().layout_job(Txt::new(*b, 13.0).color(theme::fg_2()).line_height(20.0).job(w - 300.0))).collect();
    let text_h: f32 = galleys.iter().map(|g| g.size().y + 8.0).sum();
    let h = (24.0 + 36.0 + 16.0 + text_h + 16.0).max(ACTION_H);
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let (rect, alpha) = entering(ui, cx, 1, rect);
    let mut ui = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("capture-consent"));
    ui.multiply_opacity(alpha);
    let p = ui.painter().clone();
    widgets::card(&p, rect, theme::CARD_RADIUS);
    let tile = Rect::from_min_size(rect.min + vec2(24.0, 24.0), vec2(36.0, 36.0));
    p.rect_filled(tile, theme::radius(12.0), theme::alpha(theme::accent(), 0.12));
    widgets::icon(&p, tile.center(), icons::SHIELD_CHECK, 18.0, theme::accent());
    widgets::text(&p, pos2(tile.right() + 10.0, tile.center().y), Align2::LEFT_CENTER, &Txt::new(tr("Before you capture"), 16.0).weight(weight::SEMIBOLD));
    let mut y = tile.bottom() + 16.0;
    for g in galleys {
        p.circle_filled(pos2(rect.left() + 30.0, y + 10.0), 2.0, theme::fg_3());
        let gh = g.size().y;
        p.galley(pos2(rect.left() + 24.0 + 16.0, y), g, theme::fg_2());
        y += gh + 8.0;
    }
    let button = Rect::from_min_size(pos2(rect.right() - 24.0 - 200.0, rect.center().y - 22.0), vec2(200.0, 44.0));
    if Button::new(tr("I understand")).variant(Variant::Primary).width(button.width()).show_at(&mut ui, button).clicked() {
        super::form::save(cx.ctl, &cx.snap.settings, &Settings { consent_acknowledged: true, ..Settings::clone(&cx.snap.settings) });
    }
}

fn tiles(ui: &mut Ui, cx: &mut Cx, rect: Rect) {
    let snap = cx.snap;
    let s = snap.capture.stats.as_ref();
    let health = s.map(|s| if s.frames > 0 { s.strip_ok as f64 / s.frames as f64 } else { 1.0 }).unwrap_or(1.0);
    let w = (rect.width() - 24.0) / 3.0;
    let h = (rect.height() - 12.0) / 2.0;
    let n = |ui: &Ui, i: usize, v: f64| widgets::animated_number(ui, Id::unique(("capture-number", i)), v);
    let frames = n(ui, 0, s.map(|s| s.frames as f64).unwrap_or(0.0));
    let decoded = n(ui, 1, health * 100.0);
    let keyframes = n(ui, 2, s.map(|s| s.keyframes as f64).unwrap_or(0.0));
    let depth = n(ui, 3, s.map(|s| s.depth_frames as f64).unwrap_or(0.0));
    let fps = n(ui, 4, s.map(|s| s.fps as f64).unwrap_or(0.0));
    let written = n(ui, 5, s.map(|s| s.bytes as f64).unwrap_or(0.0));
    let accent = s.map(|_| {
        if health > 0.97 {
            theme::OK
        } else if health > 0.8 {
            theme::WARN
        } else {
            theme::DANGER
        }
    });
    let tiles: [(&str, &str, String, String, Option<egui::Color32>); 6] = [
        (icons::STACK, tr("Frames"), format::integer(frames), s.map(|s| format!("{}×{}", s.width, s.height)).unwrap_or_else(|| "–".into()), None),
        (
            icons::SCAN,
            tr("Strip decoded"),
            s.map(|_| if decoded > 99.95 { format!("{decoded:.0}%") } else { format!("{decoded:.1}%") }).unwrap_or_else(|| "–".into()),
            match s {
                // Gone for a while, and why: no keyframes come meanwhile.
                Some(s) if s.strip_missing_s >= 2.0 => {
                    tr!(
                        "none for {seconds} s: {reason}",
                        seconds = format!("{:.0}", s.strip_missing_s),
                        reason = snap.capture.no_strip.unwrap_or(crate::state::NoStrip::Unknown).short()
                    )
                }
                Some(s) => tr!("{count} unreadable", count = format::integer(s.strip_failed as f64)),
                None => tr("pose strip in every frame").into(),
            },
            if s.is_some_and(|s| s.strip_missing_s >= 2.0) { Some(theme::WARN) } else { accent },
        ),
        (
            icons::CUBE,
            tr("Keyframes"),
            format::integer(keyframes),
            match s {
                Some(s) if s.dropped > 0 => tr!("{count} dropped", count = s.dropped),
                _ => tr("lossless images").into(),
            },
            None,
        ),
        (
            icons::CPU,
            tr("Depth frames"),
            format::integer(depth),
            match s {
                // Decoding fell behind (a slow disk or a busy processor): the recording went on without those.
                Some(s) if s.depth_dropped > 0 => tr!("{count} dropped", count = format::integer(s.depth_dropped as f64)),
                Some(s) => tr!("{percent} of frames", percent = format::percent(s.depth_frames as f64, s.frames as f64)),
                None => tr("exact depth samples").into(),
            },
            s.filter(|s| s.depth_dropped > 0).map(|_| theme::WARN),
        ),
        (
            icons::GAUGE,
            tr("Frame rate"),
            s.map(|_| format!("{fps:.1} fps")).unwrap_or_else(|| "–".into()),
            s.map(|s| tr!("decode {ms} ms", ms = format!("{:.1}", s.decode_ms))).unwrap_or_else(|| tr("per frame").into()),
            None,
        ),
        (
            icons::HARD_DRIVE,
            tr("Written"),
            s.map(|_| format::bytes(written)).unwrap_or_else(|| "–".into()),
            s.map(|s| tr!("{duration} recorded", duration = format::duration(s.elapsed_s))).unwrap_or_else(|| tr("this session").into()),
            None,
        ),
    ];
    for (i, (glyph, label, number, sub, accent)) in tiles.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + (i % 3) as f32 * (w + 12.0), rect.top() + (i / 3) as f32 * (h + 12.0)), vec2(w, h));
        let (r, alpha) = entering(ui, cx, 3 + i, r);
        let mut child = ui.new_child(UiBuilder::new().max_rect(r.expand(8.0)).id_salt(("tile", i)));
        child.multiply_opacity(alpha);
        widgets::stat_tile(&mut child, r, Id::unique(("capture-tile", i)), &widgets::Stat { glyph, label, number, sub, accent: *accent });
    }
}

/// What the session records besides the frames: avatar parameters, voice, VRChat's sound, body tracking, and the
/// SteamVR tab with the latest T-pose calibration.
fn performance(ui: &mut Ui, cx: &mut Cx) {
    let snap = cx.snap;
    let capturing = snap.is_capturing();
    let mut changed = None;
    block(ui, cx, 9, 164.0, |ui, rect| {
        widgets::card(ui.painter(), rect, theme::CARD_RADIUS);
        let p = ui.painter().clone();
        widgets::text(&p, pos2(rect.left() + 20.0, rect.top() + 26.0), Align2::LEFT_CENTER, &Txt::caps(tr("Performance")));
        let vr = &snap.steamvr;
        let right = match (&vr.calibration, vr.running, vr.overlay) {
            _ if capturing => tr("Switches apply to the next recording").to_string(),
            (Some(c), _, _) => tr!("T-pose: eyes at {eyes} m, hands {hands} m apart", eyes = format!("{:.2}", c.head_height_m), hands = format!("{:.2}", c.arm_span_m)),
            // VRChat sends its head and hand poses: Mixer needs no T-pose for its sessions.
            _ if snap.vrchat.running => tr("No T-pose needed: VRChat sends its head and hand poses").into(),
            (None, true, true) => tr("Calibrate in the Orbiters tab of SteamVR's dashboard").into(),
            (None, true, false) => tr("SteamVR is running").into(),
            (None, false, _) => tr("Not calibrated: start SteamVR, then calibrate in its dashboard").into(),
        };
        widgets::text_truncated(
            &p,
            pos2(rect.right() - 20.0, rect.top() + 26.0),
            Align2::RIGHT_CENTER,
            &Txt::new(right, 12.5).color(theme::fg_3()),
            rect.width() * 0.6,
        );
        let rows = crate::streams::streams(snap);
        let gap = 12.0;
        let w = (rect.width() - 32.0 - gap * 3.0) / 4.0;
        for (i, row) in rows.iter().enumerate() {
            let cell = Rect::from_min_size(pos2(rect.left() + 16.0 + i as f32 * (w + gap), rect.top() + 48.0), vec2(w, rect.height() - 64.0));
            widgets::frame(&p, cell, 14.0, theme::white(0.03));
            let tile = Rect::from_min_size(cell.min + vec2(14.0, 14.0), vec2(36.0, 36.0));
            p.rect_filled(tile, theme::radius(11.0), theme::alpha(row.tone.color(), 0.12));
            widgets::icon(&p, tile.center(), row.glyph, 17.0, if row.tone == Tone::Off { theme::fg_3() } else { row.tone.color() });
            widgets::text(&p, pos2(cell.left() + 14.0, tile.bottom() + 18.0), Align2::LEFT_CENTER, &Txt::new(row.name, 14.0).weight(weight::SEMIBOLD));
            widgets::text_truncated(
                &p,
                pos2(cell.left() + 14.0, tile.bottom() + 38.0),
                Align2::LEFT_CENTER,
                &Txt::new(&row.status, 12.0).color(theme::fg_3()),
                cell.width() - 28.0,
            );
            if let Some(level) = row.level {
                crate::streams::meter(
                    ui,
                    Id::unique(("capture-meter", i)),
                    Rect::from_min_size(pos2(tile.right() + 12.0, tile.center().y - 2.5), vec2(cell.width() - 36.0 - 14.0 - 12.0 - 62.0, 5.0)),
                    level,
                );
            }
            if let Some(on) = row.switch {
                let mut value = on;
                let mut sw = ui.new_child(
                    UiBuilder::new()
                        .max_rect(Rect::from_min_size(pos2(cell.right() - 14.0 - 48.0, tile.center().y - 14.0), vec2(48.0, 28.0)))
                        .id_salt(("capture-switch", i)),
                );
                if capturing {
                    sw.disable();
                }
                if widgets::toggle(&mut sw, ("capture-switch", i), &mut value) {
                    changed = Some(crate::streams::with_switch(&snap.settings, i, value));
                }
            }
        }
    });
    if let Some(s) = changed {
        super::form::save(cx.ctl, &snap.settings, &s);
    }
}

fn camera_path(ui: &mut Ui, cx: &mut Cx) {
    let snap = cx.snap;
    let c = &snap.capture;
    let s = c.stats.as_ref();
    let empty_text = tr("The camera path appears here while you record");
    block(ui, cx, 10, 314.0, |ui, rect| {
        widgets::card(ui.painter(), rect, theme::CARD_RADIUS);
        let p = ui.painter();
        widgets::text(p, pos2(rect.left() + 20.0, rect.top() + 26.0), Align2::LEFT_CENTER, &Txt::caps(tr("Camera path")));
        let sub = match s.and_then(|s| s.position) {
            Some([x, y, z]) => format!("x {x:.1}   y {y:.1}   z {z:.1} m"),
            None => tr("Exact positions from the capture strip, top-down").into(),
        };
        widgets::text(p, pos2(rect.left() + 20.0, rect.top() + 46.0), Align2::LEFT_CENTER, &Txt::new(sub, 12.5).color(theme::fg_3()));
        // What went wrong: the capture's error, else what the recording goes on without (a microphone that could not
        // be opened, files that could not be written), as the worker last said them.
        let warnings = s.map(|s| s.warnings.join(" · ")).filter(|w| !w.is_empty());
        if let Some((text, color)) = c.error.clone().map(|e| (e, theme::DANGER)).or_else(|| warnings.map(|w| (w, theme::WARN))) {
            widgets::text_truncated(
                p,
                pos2(rect.right() - 20.0, rect.top() + 36.0),
                Align2::RIGHT_CENTER,
                &Txt::new(text, 12.0).color(color),
                rect.width() * 0.55,
            );
        }
        let map = Rect::from_min_max(pos2(rect.left() + 16.0, rect.top() + 68.0), pos2(rect.right() - 16.0, rect.bottom() - 16.0));
        let coverage = c.coverage.as_ref().map(|m| trail::Coverage {
            x0: m.x0,
            z0: m.z0,
            cell: m.cell_m,
            width: m.width,
            depth: m.depth,
            cells: &m.cells,
            obstacle: orbiters_core::protocol::coverage::OBSTACLE,
        });
        trail::show_with(
            ui,
            map,
            Id::unique("capture-trail"),
            &trail::Trail {
                points: &c.trail,
                heading: s.and_then(|s| s.heading_deg),
                live: c.state == CaptureState::Recording,
                empty_text,
            },
            coverage.as_ref(),
        );
        // What to do next, over the map's top; else what the colours mean.
        if let Some(m) = &c.coverage {
            let p = ui.painter();
            if m.hints.is_empty() {
                let mut x = map.left() + 12.0;
                let y = map.top() + 14.0;
                widgets::text(p, pos2(x, y), Align2::LEFT_CENTER, &Txt::new(tr("Seen from"), 11.5).color(theme::fg_3()));
                x += 62.0;
                for (directions, label) in [(1u8, tr("1–2 directions")), (3, "3–5"), (6, tr("6 or more"))] {
                    p.rect_filled(Rect::from_center_size(pos2(x + 5.0, y), vec2(10.0, 10.0)), 2.0, theme::alpha(trail::coverage_colour(directions), 0.7));
                    widgets::text(p, pos2(x + 14.0, y), Align2::LEFT_CENTER, &Txt::new(label, 11.5).color(theme::fg_3()));
                    // Wide letters (Japanese) take about twice a Latin letter's room.
                    x += 24.0 + label.chars().map(|ch| if ch.is_ascii() || ch.len_utf8() < 3 { 6.5 } else { 11.5 }).sum::<f32>();
                }
            } else {
                for (k, hint) in m.hints.iter().take(2).enumerate() {
                    let y = map.top() + 16.0 + 22.0 * k as f32;
                    p.circle_filled(pos2(map.left() + 16.0, y), 3.0, theme::WARN);
                    widgets::text_truncated(p, pos2(map.left() + 26.0, y), Align2::LEFT_CENTER, &Txt::new(hint, 12.5).color(theme::fg()), map.width() - 40.0);
                }
            }
        }
    });
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::backend::{Backend, Ctl};
    use crate::state::Snapshot;
    use egui::{Event, Modifiers, PointerButton, Pos2, RawInput};
    use std::sync::{Arc, Mutex};

    /// What the capture element asked of the service, as words (nothing reaches one).
    #[derive(Default)]
    pub(crate) struct Recorder {
        pub said: Mutex<Vec<String>>,
    }

    impl Recorder {
        fn push(&self, what: impl Into<String>) {
            self.said.lock().unwrap().push(what.into());
        }
    }

    impl Backend for Recorder {
        fn n4n0(&self, command: crate::ipc::N4n0Command) {
            self.push(format!("{command:?}"));
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
        fn stop_capture(&self) {
            self.push("stop");
        }
        fn pause_capture(&self, paused: bool) {
            self.push(format!("pause {paused}"));
        }
        fn start_test_capture(&self) {
            self.push("test pattern");
        }
        fn scan(&self) {
            self.push("scan");
        }
        fn choose_vrchat(&self, pid: Option<u32>) {
            self.push(format!("choose {pid:?}"));
        }
        fn import_raw_capture(&self, _dir: PathBuf) {}
        fn patch_settings(&self, patch: serde_json::Value) {
            self.push(format!("settings {patch}"));
        }
        fn trash_session(&self, _dir: PathBuf) {}
        fn clean_up(&self, _trim: Vec<PathBuf>, _delete: Vec<PathBuf>) {}
        fn reconstruct(&self, _dir: PathBuf) {}
        fn stop_reconstruction(&self) {
            self.push("stop reconstruction");
        }
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
        fn updates(&self, _action: crate::ipc::UpdateAction) {}
        fn quit(&self) {}
    }

    /// The element at the top of a list that scrolls, as Photos & captures draws it (one scroll area: the list's), in a
    /// 1000 × 800 page, frame by frame.
    struct Harness {
        ctx: egui::Context,
        recorder: Arc<Recorder>,
        ctl: Ctl,
        snap: Snapshot,
        state: State,
        projects: crate::pages::projects::State,
        toasts: orbiters_ui::toast::Toasts,
        time: f64,
        out: Option<Out>,
    }

    impl Harness {
        /// VRChat running here in a world, the capture notice accepted.
        fn new() -> Self {
            let ctx = egui::Context::default();
            orbiters_ui::install(&ctx);
            let recorder = Arc::new(Recorder::default());
            let ctl: Ctl = recorder.clone();
            let mut snap = Snapshot::new(Settings { consent_acknowledged: true, ..Settings::default() });
            snap.vrchat.running = true;
            snap.vrchat.world.in_world = true;
            snap.vrchat.world.world_name = Some("Lantern Garden".into());
            Self { ctx, recorder, ctl, snap, state: State::default(), projects: Default::default(), toasts: Default::default(), time: 0.0, out: None }
        }

        fn frame(&mut self, events: Vec<Event>) {
            self.time += 1.0 / 60.0;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0))), time: Some(self.time), events, ..Default::default() };
            let (ctl, snap, toasts, state, projects) = (&self.ctl, &self.snap, &mut self.toasts, &mut self.state, &mut self.projects);
            let mut out = None;
            let mut output = self.ctx.run_ui(input, |ui| {
                egui::ScrollArea::vertical().id_salt("photos-captures").auto_shrink([false, false]).show(ui, |ui| {
                    let mut cx = Cx { ctl, snap, toasts: &mut *toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
                    out = element(ui, &mut cx, state, projects, &|_| Some("Lantern Garden".to_string()), 0.0);
                    // The gallery under it.
                    ui.allocate_exact_size(vec2(ui.available_width(), 2000.0), Sense::hover());
                });
            });
            output.textures_delta.clear();
            if out.is_some() {
                self.out = out;
            }
        }

        fn settle(&mut self) {
            for _ in 0..60 {
                self.frame(Vec::new());
            }
        }

        fn drawn(&self, label: &str) -> bool {
            widgets::control(&self.ctx, label).is_some()
        }

        /// Presses the control labelled `label` where it was drawn (it must take presses), and lets it settle.
        fn press(&mut self, label: &str) {
            let (r, on) = widgets::control(&self.ctx, label).unwrap_or_else(|| panic!("{label} is drawn"));
            assert!(on, "{label} takes a press");
            let at = r.center();
            self.frame(vec![Event::PointerMoved(at), Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
            self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
            self.settle();
        }

        fn said(&self) -> Vec<String> {
            self.recorder.said.lock().unwrap().clone()
        }

        fn scroll_areas(&self) -> usize {
            self.ctx.data(|d| d.count::<egui::scroll_area::State>())
        }
    }

    /// REQUEST21 5: by default a bar over the gallery (its two pictures on it, nothing more); Options opens every option
    /// the Capture tab had, in the list's own flow (no scroll area of its own); Hide closes them again.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn every_function_of_the_capture_tab_is_in_photos_and_captures() {
        widgets::watch_controls();
        let mut h = Harness::new();
        h.settle();
        assert_eq!(h.state.level(&h.snap), Level::Bar, "a gallery first");
        assert!(h.drawn("3D picture of people") && h.drawn("3D picture of the world"), "the primary actions on the bar");
        for hidden in ["capture-people", "Test pattern", "Scan", "Lossless"] {
            assert!(!h.drawn(hidden), "{hidden}: only once opened");
        }
        let bar_only = h.state.height(h.time);
        assert!((bar_only - BAR_H - AFTER).abs() < 1.0, "the bar alone: {bar_only}");
        // The bar's own: a 3D picture of people, of the world, straight away.
        h.press("3D picture of people");
        h.press("3D picture of the world");
        assert_eq!(h.said(), ["Photo3DOf { mode: People }", "Photo3DOf { mode: World }"]);

        // Every option, in the page's flow.
        h.press("Options");
        assert_eq!(h.state.level(&h.snap), Level::Full);
        let open = h.state.height(h.time);
        assert!(open > bar_only + 600.0, "it grew in place: {open}");
        for label in [
            "capture-people",
            "capture-world",
            orbiters_ui::record::LABEL,
            "Test pattern",
            "Scan",
            // The picture's length, a recording's mode and how much of it stays.
            flight_labels()[0],
            flight_labels()[2],
            "Record",
            "Lossless",
            "Compact",
            "Hide",
        ] {
            assert!(h.drawn(label), "{label} is in the open element");
        }
        assert_eq!(h.scroll_areas(), 1, "one scroll area: the page's list");
        assert!(!h.drawn("World ID"), "no world's id without one");
        h.recorder.said.lock().unwrap().clear();
        h.press("capture-people");
        h.press("capture-world");
        h.press("Test pattern");
        h.press("Scan");
        h.press("Compact");
        let said = h.said();
        assert_eq!(said[..4], ["Photo3DOf { mode: People }", "Photo3DOf { mode: World }", "test pattern", "scan"], "{said:?}");
        assert!(said[4].starts_with("settings") && said[4].contains("compact"), "{said:?}");
        // Hide: the gallery again.
        h.press("Hide");
        assert_eq!(h.state.level(&h.snap), Level::Bar);
        assert!(!h.drawn("capture-people"));
        assert!((h.state.height(h.time) - bar_only).abs() < 1.0);
    }

    /// A recording opens every option by itself (its numbers, its path), with its Pause and Stop; hidden by the person,
    /// the bar keeps its Stop, and the next capture opens it again. A capture notice not accepted: the bar's pictures
    /// open the element on it instead of taking one.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn what_runs_opens_it_by_itself_and_the_person_closes_it() {
        widgets::watch_controls();
        let mut h = Harness::new();
        h.snap.capture.state = CaptureState::Recording;
        h.snap.capture.session_dir = Some(PathBuf::from("Lantern Garden 2026-11-22 21-00"));
        h.settle();
        assert_eq!(h.state.level(&h.snap), Level::Full, "a recording: every option");
        assert!(h.drawn("Pause"));
        assert!(!h.drawn("Stop recording"), "open, the card's own Stop: not a second one on the bar");
        assert_eq!(h.scroll_areas(), 1);
        h.press("Pause");
        assert_eq!(h.said(), ["pause true"]);
        h.press("Hide");
        assert_eq!(h.state.level(&h.snap), Level::Bar, "the person's Hide holds while it records");
        assert!(h.drawn("Stop recording"), "its Stop stays on the bar");
        h.press("Stop recording");
        assert_eq!(h.said().last().map(String::as_str), Some("stop"));
        // Ended: the capture is there to open and share; Open asks the page to open its window.
        h.snap.capture.state = CaptureState::Idle;
        h.settle();
        assert_eq!(h.state.level(&h.snap), Level::Status, "a finished capture wants a look");
        h.press("Open and share");
        assert_eq!(h.out, Some(Out::Review(PathBuf::from("Lantern Garden 2026-11-22 21-00"))));
        assert_eq!(h.state.level(&h.snap), Level::Bar);
        // A reconstruction: its card, its Stop.
        h.snap.reconstruction = Some(crate::state::ReconstructionJob::new(PathBuf::from("Lantern Garden 2026-11-22 21-00"), false));
        h.settle();
        assert_eq!(h.state.level(&h.snap), Level::Status);
        h.press("Stop reconstruction");
        assert_eq!(h.said().last().map(String::as_str), Some("stop reconstruction"));
        h.snap.reconstruction = None;
        // The last capture's error, until the next one.
        h.snap.capture.error = Some("made up".into());
        h.settle();
        assert_eq!(h.state.level(&h.snap), Level::Status);
        h.snap.capture.error = None;
        // The notice first: the bar's picture opens the element on it.
        h.snap.settings.consent_acknowledged = false;
        h.settle();
        h.recorder.said.lock().unwrap().clear();
        h.press("3D picture of people");
        assert!(h.said().is_empty(), "no picture before the notice is accepted");
        assert_eq!(h.state.level(&h.snap), Level::Full);
        assert!(h.drawn("I understand"));
    }

    #[test]
    fn it_opens_by_itself_for_the_strongest_reason_and_a_hide_holds_for_that_one() {
        assert_eq!(level(false, None, None), Level::Bar);
        assert_eq!(level(true, None, None), Level::Full);
        assert_eq!(level(false, None, Some(Why::Capturing)), Level::Full);
        for why in [Why::Reconstructing, Why::Error, Why::LeftOut, Why::Kit, Why::Finished] {
            assert_eq!(level(false, None, Some(why)), Level::Status, "{why:?}");
            assert_eq!(level(false, Some(why), Some(why)), Level::Bar, "{why:?} hidden");
            assert_eq!(level(false, Some(Why::Capturing), Some(why)), Level::Status, "a hide of another reason");
        }
        let mut snap = Snapshot::new(Settings::default());
        assert_eq!(why(&snap, false, false), None);
        assert_eq!(why(&snap, true, false), Some(Why::Finished));
        assert_eq!(why(&snap, true, true), Some(Why::Kit));
        snap.capture.left_out.why = Some("x".into());
        assert_eq!(why(&snap, true, true), Some(Why::LeftOut));
        snap.capture.error = Some("x".into());
        assert_eq!(why(&snap, true, true), Some(Why::Error));
        snap.reconstruction = Some(crate::state::ReconstructionJob::new(PathBuf::from("s"), false));
        assert_eq!(why(&snap, true, true), Some(Why::Reconstructing));
        snap.capture.state = CaptureState::Recording;
        assert_eq!(why(&snap, true, true), Some(Why::Capturing));
    }

    /// The owner's rule (never a scroll area inside another): the element draws none of its own.
    #[test]
    fn the_element_has_no_scroll_area_of_its_own() {
        let code = orbiters_core::source::production(include_str!("capture.rs"));
        assert!(!code.contains(concat!("Scroll", "Area")), "the capture element scrolls with the page's list");
    }

    /// verify-r21 Minor 4: the open options carry the current world's id (selectable), as the Capture tab's header did.
    #[test]
    fn the_open_options_show_the_worlds_id() {
        widgets::watch_controls();
        let mut h = Harness::new();
        h.snap.vrchat.world.world_id = Some("wrld_00000000-0000-4000-8000-000000000123".into());
        h.settle();
        assert!(!h.drawn("World ID"), "the gallery first");
        h.press("Options");
        assert!(h.drawn("World ID"));
    }
}
