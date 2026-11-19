//! Right after a 3D picture, under VRChat's camera (REQUEST13, [`crate::controller::explorer`]): "Share to …" and "Share
//! publicly" ([`Zone::CameraShare`]), then how each share goes, what a public one earned and the way to the next title;
//! and in a headset the friends to choose for "Share to …" ([`Zone::CameraChooser`]), over the camera's top edge, large
//! rows for a laser, a page at a time. Every press names what it is for: the picture by its id, a friend by theirs
//! ([`person_key`]); nothing acts on a row's place.

use super::{fit, look, text, Action, Item, Look};
use crate::controller::explorer::{self as ex, Done, Offer, Step};
use egui::{pos2, vec2, Align2, Color32, Painter, Rect, Stroke};
use orbiters_i18n::{mark, tr};
use orbiters_ui::theme::{self, weight};
use orbiters_ui::{icons, widgets};

pub const SHARE_TO: &str = mark("Share to …");
pub const SHARE_PUBLICLY: &str = mark("Share publicly");
pub const ASK_PUBLIC: &str = mark("Everyone on Orbiters will see it, and it cannot really be taken back.");
pub const YES_PUBLIC: &str = mark("Yes, share publicly");
pub const CANCEL: &str = mark("Cancel");
pub const MAKING: &str = mark("Making its 3D view…");
pub const SENDING: &str = mark("Sending {percent}");
pub const SHARING: &str = mark("Sharing…");
pub const SHARED_ONE: &str = mark("Shared with 1 friend");
pub const SHARED_MANY: &str = mark("Shared with {count} friends");
pub const EXPLICIT: &str = mark("Confirm it in Photos & captures: it may be explicit");
pub const TRY_AGAIN: &str = mark("Try again");
pub const CHOOSE_TITLE: &str = mark("Share with friends");
pub const HERE: &str = mark("Here now");
pub const LOADING: &str = mark("Loading your friends…");
pub const NONE_YET: &str = mark("No VRChat friends to show yet.");
pub const PAGE_OF: &str = mark("{page} of {pages}");
pub const SEND_ONE: &str = mark("Share with 1 friend");
pub const SEND_MANY: &str = mark("Share with {count} friends");
pub const SEND_NONE: &str = mark("Choose friends");
pub const NOTHING_LEFT: &str = mark("No points left in this world");
pub const SHARE_TO_WHY: &str = mark("Choose friends who see this 3D picture.");
pub const SHARE_PUBLICLY_WHY: &str = mark("Everyone on Orbiters sees it, on the world's page too.");

/// Friends on one page of the chooser.
pub const ROWS: usize = 6;

/// What the share row and the chooser show now.
#[derive(Clone, Debug, PartialEq)]
pub struct ShareView {
    pub offer: Offer,
    /// "Share publicly" was pressed and asks first (the first time only).
    pub confirming: bool,
    /// Sharing publicly from here has not been confirmed yet.
    pub asks_first: bool,
    /// The chooser, while it is open (in a headset; on a screen "Share to …" opens the window's own).
    pub chooser: Option<ChooserView>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChooserView {
    /// This page's friends.
    pub rows: Vec<ChooserRow>,
    pub page: usize,
    pub pages: usize,
    pub chosen: usize,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChooserRow {
    /// The friend's identity ([`person_key`]).
    pub key: u64,
    pub name: String,
    /// In this instance now.
    pub here: bool,
    pub chosen: bool,
}

/// A friend's identity for a press: their VRChat user id, hashed (a press names who, never which row).
pub fn person_key(id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish() | 1
}

fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(pos2(x, y), vec2(w, h))
}

/// Its buttons' height, a laser's size.
const BUTTON: f32 = 58.0;
/// A slot that says how a share went: its reason may take two lines.
const DONE: f32 = 104.0;
const PUT_AWAY_W: f32 = 44.0;

/// The share row's items, in a zone `w` x `h`.
pub fn share_items(w: f32, _h: f32, view: &ShareView) -> Vec<Item> {
    let id = view.offer.id;
    if view.confirming {
        let yes = 206.0;
        let no = 108.0;
        return vec![
            Item { action: Action::Idle, rect: r(0.0, 0.0, w - yes - no - 16.0, BUTTON), look: Look::ShareQuestion },
            Item { action: Action::ShareCancelConfirm, rect: r(w - yes - no - 8.0, 0.0, no, BUTTON), look: Look::ShareCancel },
            Item { action: Action::ShareConfirm(id), rect: r(w - yes, 0.0, yes, BUTTON), look: Look::ShareYes },
        ];
    }
    let tall = |step: &Step| if matches!(step, Step::Done(_) | Step::Limited) { DONE } else { BUTTON };
    // A column (on a screen, beside the panel): the two over each other, the put-away under them.
    if w < 300.0 {
        let question = view.confirming;
        if question {
            return vec![
                Item { action: Action::Idle, rect: r(0.0, 0.0, w, 84.0), look: Look::ShareQuestion },
                Item { action: Action::ShareConfirm(id), rect: r(0.0, 92.0, w, BUTTON), look: Look::ShareYes },
                Item { action: Action::ShareCancelConfirm, rect: r(0.0, 92.0 + BUTTON + 8.0, w, 44.0), look: Look::ShareCancel },
            ];
        }
        let first = tall(&view.offer.people);
        let mut out = vec![
            Item { action: slot_action(&view.offer.people, Action::ShareTo(id)), rect: r(0.0, 0.0, w, first), look: Look::ShareSlot(false) },
            Item { action: slot_action(&view.offer.public, Action::SharePublic(id)), rect: r(0.0, first + 8.0, w, tall(&view.offer.public)), look: Look::ShareSlot(true) },
        ];
        if !view.offer.busy() {
            let y = first + 8.0 + tall(&view.offer.public) + 8.0;
            out.push(Item { action: Action::ShareDismiss(id), rect: r(w - PUT_AWAY_W, y, PUT_AWAY_W, PUT_AWAY_W), look: Look::ShareDismiss });
        }
        return out;
    }
    let half = (w - PUT_AWAY_W - 16.0) / 2.0;
    let people = slot_action(&view.offer.people, Action::ShareTo(id));
    let public = slot_action(&view.offer.public, Action::SharePublic(id));
    let mut out = vec![
        Item { action: people, rect: r(0.0, 0.0, half, tall(&view.offer.people)), look: Look::ShareSlot(false) },
        Item { action: public, rect: r(half + 8.0, 0.0, half, tall(&view.offer.public)), look: Look::ShareSlot(true) },
    ];
    if !view.offer.busy() {
        out.push(Item { action: Action::ShareDismiss(id), rect: r(w - PUT_AWAY_W, (BUTTON - PUT_AWAY_W) / 2.0, PUT_AWAY_W, PUT_AWAY_W), look: Look::ShareDismiss });
    }
    out
}

/// What a slot presses: its button while nothing runs or after a failure (it tries again), nothing otherwise.
fn slot_action(step: &Step, press: Action) -> Action {
    if matches!(step, Step::Idle | Step::Failed(_)) {
        press
    } else {
        Action::Idle
    }
}

/// The chooser's items, in a zone `w` x `h`: its head with the close, a page of friends, the page's arrows and Send.
pub fn chooser_items(w: f32, h: f32, view: &ShareView) -> Vec<Item> {
    let Some(c) = view.chooser.as_ref() else { return Vec::new() };
    let mut out = vec![
        Item { action: Action::Idle, rect: r(0.0, 0.0, w, h), look: Look::ChooserBack },
        Item { action: Action::ChooserClose, rect: r(w - 12.0 - 48.0, 8.0, 48.0, 48.0), look: Look::Close },
    ];
    if c.error.is_some() {
        out.push(Item { action: Action::ChooserRetry, rect: r((w - 200.0) / 2.0, h / 2.0 + 40.0, 200.0, 52.0), look: Look::WebButton { icon: icons::ARROW_CLOCKWISE, label: tr(TRY_AGAIN) } });
    }
    for (n, row) in c.rows.iter().enumerate() {
        out.push(Item { action: Action::ChooserPerson(row.key), rect: r(12.0, 66.0 + n as f32 * 62.0, w - 24.0, 56.0), look: Look::ChooserRow(n) });
    }
    let foot = h - 12.0 - 56.0;
    if c.pages > 1 {
        out.push(Item { action: if c.page > 0 { Action::ChooserPage(false) } else { Action::Idle }, rect: r(12.0, foot, 56.0, 56.0), look: Look::ChooserArrow(false) });
        out.push(Item { action: if c.page + 1 < c.pages { Action::ChooserPage(true) } else { Action::Idle }, rect: r(12.0 + 56.0 + 64.0, foot, 56.0, 56.0), look: Look::ChooserArrow(true) });
    }
    let send = 206.0;
    out.push(Item { action: if c.chosen > 0 { Action::ChooserSend(view.offer.id) } else { Action::Idle }, rect: r(w - 12.0 - send, foot, send, 56.0), look: Look::ChooserSend });
    out
}

/// What a public share's slot says while it is done: what it earned (or why nothing), then where the member stands.
pub fn done_lines(done: &Done) -> (String, Option<String>) {
    let earned = ex::earned_words(done);
    let standing = match (done.reached.as_ref(), done.after.as_ref()) {
        (Some(t), _) => Some(orbiters_i18n::tr!("New title: {title}", title = ex::title_name(t))),
        (None, Some(after)) => Some(ex::standing_words(after)),
        _ => None,
    };
    match earned {
        Some(e) => (e, standing),
        None => (standing.unwrap_or_default(), None),
    }
}

/// The hint under "Share publicly" before it is pressed: what it would earn, when the world's lookup said.
pub fn would_earn_words(offer: &Offer) -> Option<String> {
    match offer.would_earn? {
        e if e == 1.0 => Some(tr(ex::EARNED_ONE).into()),
        e if e > 0.0 => Some(orbiters_i18n::tr!("+{points} Explorer points", points = orbiters_ui::format::points(e))),
        _ => Some(tr(NOTHING_LEFT).into()),
    }
}

fn button_back(p: &Painter, rect: Rect, hovered: bool, pressed: bool, primary: bool) {
    let round = look::radius(14.0);
    let fill = match (primary, pressed, hovered) {
        (true, true, _) => theme::mix(look::accent(), Color32::BLACK, 0.2),
        (true, false, true) => theme::mix(look::accent(), Color32::WHITE, 0.12),
        (true, false, false) => look::accent(),
        (false, true, _) => look::TILE_DOWN,
        (false, false, true) => look::TILE_HOVER,
        (false, false, false) => look::PANEL,
    };
    p.rect_filled(rect, round, fill);
    if hovered && !primary {
        p.rect_stroke(rect.shrink(0.75), round, Stroke::new(1.5, look::accent()), egui::StrokeKind::Middle);
    } else if !primary {
        p.rect_stroke(rect.shrink(0.5), round, Stroke::new(1.0, theme::white(0.10)), egui::StrokeKind::Middle);
    }
}

/// An icon and a label (and a smaller line under it), centred in `rect`.
fn face(p: &Painter, rect: Rect, icon: &str, label: &str, under: Option<&str>, ink: Color32, under_ink: Color32) {
    let font = look::font(15.0, weight::BOLD);
    let words = fit(p, label, &font, rect.width() - 48.0);
    let wide = p.layout_no_wrap(words.clone(), font.clone(), ink).size().x + 26.0;
    let from = rect.center().x - wide / 2.0;
    let y = if under.is_some() { rect.center().y - 9.0 } else { rect.center().y };
    widgets::icon(p, pos2(from + 9.0, y), icon, 18.0, ink);
    p.text(pos2(from + 26.0, y), Align2::LEFT_CENTER, words, font, ink);
    if let Some(under) = under {
        let small = look::font(12.0, weight::SEMIBOLD);
        p.text(pos2(rect.center().x, rect.center().y + 13.0), Align2::CENTER_CENTER, fit(p, under, &small, rect.width() - 20.0), small, under_ink);
    }
}

/// A status card: an icon, a bold line, a second that may take two lines (a reason is said whole), a third, left
/// aligned and centred together; `line`: a thin bar along its foot (0 to 1).
fn card(p: &Painter, rect: Rect, icon: &str, tint: Color32, first: &str, second: Option<&str>, third: Option<&str>, line: Option<f32>) {
    let round = look::radius(14.0);
    p.rect_filled(rect, round, look::PANEL);
    p.rect_stroke(rect.shrink(0.5), round, Stroke::new(1.0, theme::alpha(tint, 0.45)), egui::StrokeKind::Middle);
    let x = rect.left() + 14.0;
    let room = rect.width() - 14.0 - 30.0 - 12.0;
    let bold = look::font(14.0, weight::BOLD);
    let small = look::font(12.0, weight::SEMIBOLD);
    let tinted = theme::mix(tint, Color32::WHITE, 0.35);
    let second = second.map(|w| p.layout(w.to_string(), small.clone(), tinted, room));
    let second_h = second.as_ref().map_or(0.0, |g| g.size().y.min(32.0) + 3.0);
    let tall = 18.0 + second_h + if third.is_some() { 19.0 } else { 0.0 };
    let top = rect.center().y - tall / 2.0 + 9.0 - if line.is_some() { 2.0 } else { 0.0 };
    widgets::icon(p, pos2(x + 9.0, top), icon, 18.0, tint);
    p.text(pos2(x + 28.0, top), Align2::LEFT_CENTER, fit(p, first, &bold, room), bold, look::LABEL);
    let mut y = top + 11.0;
    if let Some(g) = second {
        let clip = Rect::from_min_size(pos2(x + 28.0, y), vec2(room, 32.0));
        p.with_clip_rect(clip).galley(pos2(x + 28.0, y), g, tinted);
        y += second_h;
    }
    if let Some(words) = third {
        p.text(pos2(x + 28.0, y + 8.0), Align2::LEFT_CENTER, fit(p, words, &small, room), small.clone(), look::MUTED);
    }
    if let Some(k) = line {
        let track = Rect::from_min_max(pos2(rect.left() + 14.0, rect.bottom() - 7.0), pos2(rect.right() - 14.0, rect.bottom() - 4.0));
        p.rect_filled(track, look::radius(1.5), theme::white(0.10));
        p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * k.clamp(0.0, 1.0), track.height())), look::radius(1.5), tint);
    }
}

/// One of the share row's slots: its button while it can be pressed, else how its share goes.
fn slot(p: &Painter, rect: Rect, view: &ShareView, public: bool, hovered: bool, pressed: bool) {
    let offer = &view.offer;
    let step = if public { &offer.public } else { &offer.people };
    let good = theme::mix(look::accent(), Color32::WHITE, 0.1);
    match step {
        Step::Idle => {
            button_back(p, rect, hovered, pressed, public);
            let ink = if public { look::on_accent() } else { look::LABEL };
            if public {
                let hint = would_earn_words(offer);
                face(p, rect, icons::GLOBE, tr(SHARE_PUBLICLY), hint.as_deref(), ink, theme::alpha(ink, 0.8));
            } else {
                face(p, rect, icons::USERS, tr(SHARE_TO), None, ink, ink);
            }
        }
        Step::Busy { making, sent } => {
            let words = if *making {
                tr(MAKING).to_string()
            } else if let Some(k) = sent {
                orbiters_i18n::tr!("Sending {percent}", percent = orbiters_ui::format::percent(*k as f64, 1.0))
            } else {
                tr(SHARING).to_string()
            };
            let what = if public { tr(SHARE_PUBLICLY) } else { tr(SHARE_TO) };
            card(p, rect, icons::CIRCLE_NOTCH, theme::INDIGO, &words, Some(what), None, *sent);
        }
        Step::Done(done) if public => {
            let (second, third) = done_lines(done);
            let line = done.after.as_ref().map(|a| a.progress());
            let tint = if done.earned.is_some_and(|e| e > 0.0) { good } else { theme::FG_3 };
            card(p, rect, icons::CHECK_CIRCLE, tint, tr(ex::SHARED_PUBLICLY), Some(&second).filter(|s| !s.is_empty()).map(|s| s.as_str()), third.as_deref(), line);
        }
        Step::Done(done) => {
            let words = if done.people == 1 { tr(SHARED_ONE).to_string() } else { orbiters_i18n::tr!("Shared with {count} friends", count = done.people) };
            card(p, rect, icons::CHECK_CIRCLE, good, &words, None, None, None);
        }
        Step::Explicit => card(p, rect, icons::WARNING, theme::WARN, tr(EXPLICIT), None, None, None),
        // Said plainly; "Share to …" beside it still works.
        Step::Limited => card(p, rect, icons::CLOCK, theme::WARN, tr(ex::LIMIT_TITLE), Some(tr(ex::LIMIT_WHY)), None, None),
        Step::Failed(message) => {
            // Pressed again, it tries again.
            let round = look::radius(14.0);
            p.rect_filled(rect, round, if pressed { look::TILE_DOWN } else if hovered { look::TILE_HOVER } else { look::PANEL });
            p.rect_stroke(rect.shrink(0.5), round, Stroke::new(if hovered { 1.5 } else { 1.0 }, theme::alpha(theme::DANGER, 0.7)), egui::StrokeKind::Middle);
            let x = rect.left() + 14.0;
            widgets::icon(p, pos2(x + 9.0, rect.center().y - 9.0), icons::WARNING, 18.0, theme::DANGER);
            let small = look::font(12.0, weight::SEMIBOLD);
            let room = rect.width() - 52.0;
            p.text(pos2(x + 28.0, rect.center().y - 9.0), Align2::LEFT_CENTER, fit(p, message, &small, room), small.clone(), look::LABEL);
            p.text(pos2(x + 28.0, rect.center().y + 11.0), Align2::LEFT_CENTER, fit(p, tr(TRY_AGAIN), &small, room), small, theme::mix(theme::DANGER, Color32::WHITE, 0.4));
        }
    }
}

/// Draws one of the share row's or the chooser's items.
pub fn draw(p: &Painter, item: &Item, view: &ShareView, hovered: bool, pressed: bool) {
    let rect = item.rect;
    match item.look {
        Look::ShareSlot(public) => slot(p, rect, view, public, hovered, pressed),
        Look::ShareDismiss => {
            let round = look::radius(rect.height() / 2.0);
            p.rect_filled(rect, round, if pressed { look::TILE_DOWN } else if hovered { look::TILE_HOVER } else { look::PANEL });
            widgets::icon(p, rect.center(), icons::X, 18.0, if hovered { look::LABEL } else { look::MUTED });
        }
        Look::ShareQuestion => {
            let round = look::radius(14.0);
            p.rect_filled(rect, round, look::PANEL);
            widgets::icon(p, pos2(rect.left() + 22.0, rect.center().y), icons::GLOBE, 18.0, theme::WARN);
            let words = tr(ASK_PUBLIC);
            let font = look::font(12.5, weight::SEMIBOLD);
            let galley = p.layout(words.to_string(), font, look::LABEL, rect.width() - 52.0);
            let at = pos2(rect.left() + 40.0, rect.center().y - galley.size().y / 2.0);
            p.galley(at, galley, look::LABEL);
        }
        Look::ShareCancel => {
            button_back(p, rect, hovered, pressed, false);
            face(p, rect, icons::X, tr(CANCEL), None, look::LABEL, look::LABEL);
        }
        Look::ShareYes => {
            button_back(p, rect, hovered, pressed, true);
            face(p, rect, icons::GLOBE, tr(YES_PUBLIC), would_earn_words(&view.offer).as_deref(), look::on_accent(), theme::alpha(look::on_accent(), 0.8));
        }
        Look::ChooserBack => {
            let Some(c) = view.chooser.as_ref() else { return };
            let round = look::radius(look::RADIUS);
            p.rect_filled(rect, round, look::OPAQUE);
            p.rect_stroke(rect.shrink(0.5), round, Stroke::new(1.0, theme::white(0.12)), egui::StrokeKind::Middle);
            widgets::icon(p, pos2(rect.left() + 28.0, rect.top() + 32.0), icons::USERS, 20.0, look::accent());
            let head = look::font(17.0, weight::BOLD);
            p.text(pos2(rect.left() + 48.0, rect.top() + 32.0), Align2::LEFT_CENTER, fit(p, tr(CHOOSE_TITLE), &head, rect.width() - 130.0), head, look::LABEL);
            let middle = pos2(rect.center().x, rect.top() + 66.0 + 3.0 * 62.0 - 30.0);
            if c.loading {
                widgets::icon(p, middle - vec2(0.0, 24.0), icons::CIRCLE_NOTCH, 26.0, look::MUTED);
                text(p, middle + vec2(0.0, 12.0), Align2::CENTER_CENTER, tr(LOADING), 14.0, weight::SEMIBOLD, look::MUTED);
            } else if let Some(error) = c.error.as_deref() {
                widgets::icon(p, middle - vec2(0.0, 40.0), icons::WARNING, 26.0, theme::WARN);
                let galley = p.layout(error.to_string(), look::font(13.0, weight::SEMIBOLD), look::LABEL, rect.width() - 60.0);
                p.galley(pos2(rect.center().x - galley.size().x / 2.0, middle.y - 14.0), galley, look::LABEL);
            } else if c.rows.is_empty() {
                text(p, middle, Align2::CENTER_CENTER, tr(NONE_YET), 14.0, weight::SEMIBOLD, look::MUTED);
            }
            if c.pages > 1 {
                let foot = rect.bottom() - 12.0 - 28.0;
                let words = orbiters_i18n::tr!("{page} of {pages}", page = c.page + 1, pages = c.pages);
                text(p, pos2(rect.left() + 12.0 + 56.0 + 32.0, foot), Align2::CENTER_CENTER, &words, 13.0, weight::SEMIBOLD, look::MUTED);
            }
        }
        Look::ChooserRow(n) => {
            let Some(row) = view.chooser.as_ref().and_then(|c| c.rows.get(n)) else { return };
            let round = look::radius(14.0);
            let fill = if pressed {
                look::TILE_DOWN
            } else if row.chosen {
                theme::mix(look::TILE, look::accent(), if hovered { 0.34 } else { 0.24 })
            } else if hovered {
                look::TILE_HOVER
            } else {
                look::TILE
            };
            p.rect_filled(rect, round, fill);
            if hovered || row.chosen {
                p.rect_stroke(rect.shrink(0.75), round, Stroke::new(1.5, theme::alpha(look::accent(), if row.chosen { 0.9 } else { 0.6 })), egui::StrokeKind::Middle);
            }
            // The check on the left, the name, "Here now" on the right for a friend in this instance.
            let check = Rect::from_center_size(pos2(rect.left() + 28.0, rect.center().y), vec2(26.0, 26.0));
            if row.chosen {
                p.rect_filled(check, look::radius(8.0), look::accent());
                widgets::icon(p, check.center(), icons::CHECK, 16.0, look::on_accent());
            } else {
                p.rect_stroke(check, look::radius(8.0), Stroke::new(1.5, theme::white(0.35)), egui::StrokeKind::Inside);
            }
            let here = row.here.then(|| tr(HERE));
            let tag_font = look::font(11.5, weight::BOLD);
            let tag_w = here.map_or(0.0, |h| p.layout_no_wrap(h.to_string(), tag_font.clone(), Color32::WHITE).size().x + 20.0);
            let name_font = look::font(16.0, weight::SEMIBOLD);
            p.text(pos2(rect.left() + 54.0, rect.center().y), Align2::LEFT_CENTER, fit(p, &row.name, &name_font, rect.width() - 70.0 - tag_w), name_font, look::LABEL);
            if let Some(here) = here {
                let tag = Rect::from_min_size(pos2(rect.right() - 12.0 - tag_w, rect.center().y - 11.0), vec2(tag_w, 22.0));
                p.rect_filled(tag, look::radius(11.0), theme::alpha(theme::VIOLET, 0.25));
                p.text(tag.center(), Align2::CENTER_CENTER, here, tag_font, theme::mix(theme::VIOLET, Color32::WHITE, 0.45));
            }
        }
        Look::ChooserArrow(next) => {
            let on = item.action != Action::Idle;
            let round = look::radius(rect.height() / 2.0);
            p.rect_filled(rect, round, if !on { look::PANEL } else if pressed { look::TILE_DOWN } else if hovered { look::TILE_HOVER } else { look::TILE });
            widgets::icon(p, rect.center(), if next { icons::CARET_RIGHT } else { icons::CARET_LEFT }, 20.0, if on { look::LABEL } else { theme::FG_4 });
        }
        Look::ChooserSend => {
            let n = view.chooser.as_ref().map_or(0, |c| c.chosen);
            let on = n > 0;
            if on {
                button_back(p, rect, hovered, pressed, true);
            } else {
                p.rect_filled(rect, look::radius(14.0), look::TILE);
            }
            let words = match n {
                0 => tr(SEND_NONE).to_string(),
                1 => tr(SEND_ONE).to_string(),
                _ => orbiters_i18n::tr!("Share with {count} friends", count = n),
            };
            let ink = if on { look::on_accent() } else { theme::FG_4 };
            face(p, rect, icons::PAPER_PLANE_TILT, &words, None, ink, ink);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer() -> Offer {
        Offer { id: 7, key: "C:/S/a".into(), world_id: "wrld_x".into(), would_earn: Some(0.5), until_ms: u64::MAX, public: Step::Idle, people: Step::Idle }
    }

    fn view() -> ShareView {
        ShareView { offer: offer(), confirming: false, asks_first: true, chooser: None }
    }

    fn actions(items: &[Item]) -> Vec<Action> {
        items.iter().map(|i| i.action).collect()
    }

    /// Each button names the picture it is for; what runs or is done cannot be pressed again, what failed can.
    #[test]
    fn the_buttons_name_their_picture_and_follow_their_shares() {
        let mut v = view();
        assert_eq!(actions(&share_items(520.0, 84.0, &v)), [Action::ShareTo(7), Action::SharePublic(7), Action::ShareDismiss(7)]);
        v.offer.public = Step::Busy { making: true, sent: None };
        assert_eq!(actions(&share_items(520.0, 84.0, &v)), [Action::ShareTo(7), Action::Idle], "nothing put away while a share runs");
        v.offer.public = Step::Failed("No".into());
        v.offer.people = Step::Done(Done { people: 2, earned: None, nothing: None, after: None, reached: None });
        assert_eq!(actions(&share_items(520.0, 84.0, &v)), [Action::Idle, Action::SharePublic(7), Action::ShareDismiss(7)]);
        // The daily limit: public cannot be pressed again today, "Share to …" still can.
        let mut limited = view();
        limited.offer.public = Step::Limited;
        assert_eq!(actions(&share_items(520.0, 84.0, &limited)), [Action::ShareTo(7), Action::Idle, Action::ShareDismiss(7)]);
        v.confirming = true;
        assert_eq!(actions(&share_items(520.0, 84.0, &v)), [Action::Idle, Action::ShareCancelConfirm, Action::ShareConfirm(7)]);
        // On a screen, the column beside the panel: the same presses, over each other, none overlapping.
        let column = share_items(210.0, 268.0, &v);
        assert_eq!(actions(&column), [Action::Idle, Action::ShareConfirm(7), Action::ShareCancelConfirm]);
        v.confirming = false;
        let column = share_items(210.0, 268.0, &v);
        assert_eq!(actions(&column), [Action::Idle, Action::SharePublic(7), Action::ShareDismiss(7)]);
        for pair in column.windows(2) {
            assert!(pair[0].rect.bottom() <= pair[1].rect.top() && pair[1].rect.bottom() <= 268.0, "{pair:?}");
        }
    }

    /// A row presses its friend, by who they are; Send is off until someone is chosen; arrows only where there is a page.
    #[test]
    fn the_chooser_presses_friends_by_identity() {
        let mut v = view();
        let rows = ["usr_a", "usr_b"].map(|id| ChooserRow { key: person_key(id), name: id.into(), here: false, chosen: false }).to_vec();
        v.chooser = Some(ChooserView { rows, page: 0, pages: 1, chosen: 0, loading: false, error: None });
        let items = chooser_items(410.0, 516.0, &v);
        let a = actions(&items);
        assert!(a.contains(&Action::ChooserPerson(person_key("usr_a"))) && a.contains(&Action::ChooserPerson(person_key("usr_b"))));
        assert!(!a.iter().any(|x| matches!(x, Action::ChooserPage(_) | Action::ChooserSend(_))));
        v.chooser.as_mut().unwrap().chosen = 1;
        v.chooser.as_mut().unwrap().pages = 3;
        let a = actions(&chooser_items(410.0, 516.0, &v));
        assert!(a.contains(&Action::ChooserSend(7)) && a.contains(&Action::ChooserPage(true)) && !a.contains(&Action::ChooserPage(false)));
        assert_ne!(person_key("usr_a"), person_key("usr_b"));
    }

    fn at(points: f64, title: Option<(&str, f64)>, next: Option<(&str, f64)>) -> orbiters_core::explorer::Explorer {
        orbiters_core::explorer::Explorer {
            points,
            half_points: None,
            title: title.map(|(k, p)| orbiters_core::explorer::Title { key: k.into(), points: p, name: String::new() }),
            next: next.map(|(k, p)| orbiters_core::explorer::Next { key: k.into(), points: p, name: String::new(), remaining: p - points }),
            titles: Vec::new(),
        }
    }

    /// What a public share says at each threshold and in each case: a whole point, a half, nothing and why, below zero,
    /// a title reached, the last title.
    #[test]
    fn a_public_share_says_what_it_earned_and_the_way_to_the_next_title() {
        orbiters_i18n::in_language(orbiters_i18n::Language::En, || {
            let done = |earned: Option<f64>, nothing, after: orbiters_core::explorer::Explorer, reached: Option<orbiters_core::explorer::Title>| Done { people: 0, earned, nothing, after: Some(after), reached };
            let lines = |d: Done| done_lines(&d);
            assert_eq!(lines(done(Some(1.0), None, at(1.0, None, Some(("explorer", 10.0))), None)), ("+1 Explorer point".into(), Some("1 point · 9 to Explorer".into())));
            assert_eq!(lines(done(Some(0.5), None, at(12.5, Some(("explorer", 10.0)), Some(("navigator", 50.0))), None)), ("+0.5 Explorer points".into(), Some("12.5 points · 37.5 to Navigator".into())));
            assert_eq!(
                lines(done(Some(0.0), Some(ex::Nothing::WorldHasThree), at(-1.0, None, Some(("explorer", 10.0))), None)),
                ("No points: this world already has three public 3D pictures".into(), Some("-1 points · 11 to Explorer".into()))
            );
            assert_eq!(lines(done(Some(0.0), Some(ex::Nothing::SharedBefore), at(3.0, None, Some(("explorer", 10.0))), None)).0, "No points: this picture was shared publicly before");
            let reached = Some(orbiters_core::explorer::Title { key: "navigator".into(), points: 50.0, name: String::new() });
            assert_eq!(lines(done(Some(1.0), None, at(50.0, Some(("navigator", 50.0)), Some(("discoverer", 100.0))), reached)).1, Some("New title: Navigator".into()));
            for (points, title, next, says) in [
                (100.0, ("discoverer", 100.0), Some(("marcoPolo", 500.0)), "100 points · 400 to Marco polo"),
                (500.0, ("marcoPolo", 500.0), Some(("multiverseTraveler", 1000.0)), "500 points · 500 to Multiverse traveler"),
                (1000.5, ("multiverseTraveler", 1000.0), None, "1,000.5 points · Multiverse traveler"),
            ] {
                assert_eq!(ex::standing_words(&at(points, Some(title), next)), says);
            }
            assert_eq!(done_lines(&Done { people: 0, earned: None, nothing: None, after: None, reached: None }), (String::new(), None), "points not read: the share alone");
        });
        orbiters_i18n::in_language(orbiters_i18n::Language::Fr, || {
            assert_eq!(ex::standing_words(&at(12.5, Some(("explorer", 10.0)), Some(("navigator", 50.0)))), "12,5 points · 37,5 avant Navigateur");
        });
    }

    #[test]
    fn the_hint_says_what_a_public_share_would_earn() {
        orbiters_i18n::in_language(orbiters_i18n::Language::En, || {
            let mut o = offer();
            assert_eq!(would_earn_words(&o).as_deref(), Some("+0.5 Explorer points"));
            o.would_earn = Some(1.0);
            assert_eq!(would_earn_words(&o).as_deref(), Some("+1 Explorer point"));
            o.would_earn = Some(0.0);
            assert_eq!(would_earn_words(&o).as_deref(), Some("No points left in this world"));
            o.would_earn = None;
            assert_eq!(would_earn_words(&o), None);
        });
    }
}
