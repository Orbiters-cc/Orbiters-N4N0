//! What the shared 3D view shows the person (`controller/shared_view.rs`, contracts `Docs/n4n0-shared-view.md`).
//! Stage 1 is looking at the same capture together, each copy in front of its own viewer; the words never promise a
//! shared place.
//!
//! - The friend who is offered one is asked first, with the sharer's name and the capture's world: a card of the
//!   window's toasts (Show, and a cross for Not now), and in VR a face of the band before the eyes
//!   ([`crate::overlay::floating::prompt`]), its Show and Not now pressed with a laser, on the trigger's release.
//! - The sharer with several friends here picks one on the band too ([`Pick`]). With one friend here, the share chip on
//!   the small view shows it to that friend at once.

use orbiters_core::locks::*;
use crate::backend::Ctl;
use crate::ipc::SharedViewCommand;
use crate::state::ShareOffer;
use egui::{pos2, vec2, Align2, Context, Id, Order, Painter, Rect, Sense};
use orbiters_i18n::{mark, tr};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Variant};
use orbiters_ui::{icons, n4n0_logo};
use std::sync::Mutex;

/// The offer, in English (the catalog area `sharedview` has them in every language): the friend's name stands above it.
pub const ASK: &str = mark("Look at a 3D picture together?");
pub const WHERE: &str = mark("{world} · it opens in front of you");
pub const SHOW: &str = mark("Show");
pub const NOT_NOW: &str = mark("Not now");
/// The sharer's choice of friend.
pub const WITH_WHOM: &str = mark("Look at it together with");

fn where_(offer: &ShareOffer) -> String {
    let world = if offer.title.trim().is_empty() { "VRChat" } else { offer.title.trim() };
    orbiters_i18n::fill(tr(WHERE), &[("world", &world)])
}

/// As wide as "Talk with N4N0?"'s card, a line taller: its buttons on the name's row, the question and where under them
/// across the card, whole in every language.
const WIDTH: f32 = 380.0;
pub(crate) const HEIGHT: f32 = 104.0;
const MARGIN: f32 = 20.0;

/// The window's card while an offer is up: Show takes it, the cross says not now. Above the "Talk with N4N0?" card's
/// place when both are up.
pub fn window(ctx: &Context, ctl: &Ctl, offer: Option<&ShareOffer>, below: bool) {
    let now = crate::wake_offer::now_ms();
    let Some(offer) = offer.filter(|o| o.until_ms > now) else { return };
    let screen = ctx.content_rect();
    let id = Id::unique(("orbiters-shared-view-offer", offer.id));
    let enter = anim::spring_from(ctx, id.with("enter"), [0.0], [1.0], Spring::BOUNCY)[0];
    // Over "Talk with N4N0?"'s card when it is up: its height, and the toasts' 12 between them.
    let lift = if below { crate::wake_offer::HEIGHT + 12.0 } else { 0.0 };
    let base = Rect::from_min_size(pos2(screen.right() - MARGIN - WIDTH, screen.bottom() - MARGIN - HEIGHT - lift), vec2(WIDTH, HEIGHT));
    let rect = Rect::from_center_size(base.center() + vec2(60.0 * (1.0 - enter), 0.0), base.size() * (0.92 + 0.08 * enter.min(1.0)));
    orbiters_ui::front::area(ctx, id, Order::Foreground, false).fixed_pos(screen.min).interactable(true).show(ctx, |ui| {
        let mut p = ui.painter().clone();
        p.multiply_opacity(enter.clamp(0.0, 1.0));
        ui.interact(rect, id.with("card"), Sense::click());
        widgets::panel(&p, rect, 16.0);
        let mark = Rect::from_center_size(pos2(rect.left() + 16.0 + 14.0, rect.center().y - 1.0), n4n0_logo::size_for_width(28.0));
        n4n0_logo::paint(&p, mark, theme::mark());
        let row = rect.top() + 30.0;
        let close = Rect::from_min_size(pos2(rect.right() - 12.0 - 32.0, row - 16.0), vec2(32.0, 32.0));
        let show = Button::new(tr(SHOW)).icon(icons::EYE).variant(Variant::Primary);
        let w = show.desired_width(ui).max(96.0);
        let at = Rect::from_min_size(pos2(close.left() - 8.0 - w, row - 17.0), vec2(w, 34.0));
        let words = mark.right() + 12.0;
        // Who first, beside the buttons; the question and where under them, across the card.
        widgets::text_truncated(&p, pos2(words, row), Align2::LEFT_CENTER, &Txt::new(&offer.from, 15.5).weight(weight::SEMIBOLD), at.left() - 10.0 - words);
        let room = rect.right() - 16.0 - words;
        let question = tr(ASK);
        let size = crate::mismatch::fitting(&p, question, &[13.5, 12.5, 11.5], room);
        widgets::text_truncated(&p, pos2(words, rect.top() + 60.0), Align2::LEFT_CENTER, &Txt::new(question, size), room);
        let place = where_(offer);
        let size = crate::mismatch::fitting(&p, &place, &[12.5, 11.5, 10.5], room);
        widgets::text_truncated(&p, pos2(words, rect.top() + 80.0), Align2::LEFT_CENTER, &Txt::new(&place, size).color(theme::fg_3()), room);
        if show.width(w).confirm_at(ui, at) {
            ctl.shared_view(SharedViewCommand::Answer { id: offer.id, accept: true });
        }
        if Button::new("").icon(icons::X).variant(Variant::Ghost).hint(tr(NOT_NOW)).width(32.0).show_at(ui, close).clicked() {
            ctl.shared_view(SharedViewCommand::Answer { id: offer.id, accept: false });
        }
        let k = offer.left(now);
        let track = Rect::from_min_max(pos2(rect.left() + 14.0, rect.bottom() - 3.0), pos2(rect.right() - 14.0, rect.bottom() - 1.0));
        p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * k, track.height())), theme::radius(1.0), theme::white(0.35));
    });
    ctx.request_repaint_after(std::time::Duration::from_millis(33));
}

/// The VR band's picture (as "Talk with N4N0?"'s).
const PX: [f32; 2] = [crate::wake_offer::VR_PX[0] as f32, crate::wake_offer::VR_PX[1] as f32];
/// Its two pills, stacked at its right: Show over Not now (picture pixels).
const PILL_W: f32 = 168.0;
const PILL_H: f32 = 64.0;

fn band() -> Rect {
    Rect::from_min_size(pos2(0.0, 0.0), vec2(PX[0], PX[1])).shrink(6.0)
}

fn pill_rect(i: usize) -> Rect {
    let b = band();
    let gap = 8.0;
    let top = b.center().y - PILL_H - gap / 2.0;
    Rect::from_min_size(pos2(b.right() - 22.0 - PILL_W, top + i as f32 * (PILL_H + gap)), vec2(PILL_W, PILL_H))
}

fn hit(rect: Rect, uv: [f32; 2]) -> bool {
    rect.expand(6.0).contains(pos2(uv[0] * PX[0], uv[1] * PX[1]))
}

/// Which pill of the offer's face a laser is on (`uv` from the band's top left): Some(true) Show, Some(false) Not now.
pub fn answer_at(uv: [f32; 2]) -> Option<bool> {
    if hit(pill_rect(0), uv) {
        Some(true)
    } else if hit(pill_rect(1), uv) {
        Some(false)
    } else {
        None
    }
}

/// The friends' pills of the picker's face, side by side under its words: at most three.
pub const PICK_MOST: usize = 3;

fn name_rect(i: usize, count: usize) -> Rect {
    let b = band();
    let count = count.clamp(1, PICK_MOST) as f32;
    let gap = 10.0;
    let left = b.left() + 26.0;
    let w = (b.width() - 52.0 - gap * (count - 1.0)) / count;
    Rect::from_min_size(pos2(left + i as f32 * (w + gap), b.bottom() - 18.0 - PILL_H), vec2(w, PILL_H))
}

/// Which friend of the picker a laser is on.
pub fn pick_at(uv: [f32; 2], count: usize) -> Option<usize> {
    (0..count.min(PICK_MOST)).find(|&i| hit(name_rect(i, count), uv))
}

fn frame(p: &Painter, hover: bool) -> (Rect, f32) {
    let b = band();
    let radius = 36.0;
    let fill = if hover { theme::mix(theme::ink_950(), theme::fg(), 0.08) } else { theme::alpha(theme::ink_950(), 0.94) };
    p.rect_filled(b, radius, fill);
    p.rect_stroke(b, radius, egui::Stroke::new(3.0, if hover { theme::accent() } else { theme::white(0.16) }), egui::StrokeKind::Inside);
    (b, radius)
}

fn pill(p: &Painter, rect: Rect, words: &str, glyph: Option<&str>, primary: bool, hover: bool) {
    let fill = match (primary, hover) {
        (true, false) => theme::accent(),
        (true, true) => theme::mix(theme::accent(), theme::fg(), 0.2),
        (false, false) => theme::white(0.10),
        (false, true) => theme::white(0.22),
    };
    let ink = if primary { theme::ink_950() } else { theme::fg() };
    p.rect_filled(rect, rect.height() / 2.0, fill);
    let (text_left, room) = match glyph {
        Some(g) => {
            widgets::icon(p, pos2(rect.left() + 34.0, rect.center().y), g, 26.0, ink);
            (rect.left() + 56.0, rect.width() - 68.0)
        }
        None => (rect.left() + 18.0, rect.width() - 36.0),
    };
    let size = crate::mismatch::fitting(p, words, &[26.0, 24.0, 22.0, 20.0], room);
    let at = if glyph.is_some() { pos2(text_left, rect.center().y) } else { rect.center() };
    let align = if glyph.is_some() { Align2::LEFT_CENTER } else { Align2::CENTER_CENTER };
    widgets::text_truncated(p, at, align, &Txt::new(words, size).weight(weight::SEMIBOLD).color(ink), room + 4.0);
}

fn time_left(p: &Painter, b: Rect, radius: f32, left: f32) {
    let track = Rect::from_min_max(pos2(b.left() + radius, b.bottom() - 9.0), pos2(b.right() - radius, b.bottom() - 5.0));
    p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * left.clamp(0.0, 1.0), track.height())), 2.0, theme::white(0.35));
}

/// The offer's face: N4N0's mark, who and where, Show and Not now. `hover`: the pill a laser is on.
pub fn paint_offer(p: &Painter, from: &str, title: &str, hover: Option<bool>, left: f32) {
    let offer = ShareOffer { from: from.into(), title: title.into(), ..Default::default() };
    let (b, radius) = frame(p, false);
    let mark = Rect::from_center_size(pos2(b.left() + 30.0 + 22.0, b.center().y - 2.0), n4n0_logo::size_for_width(44.0));
    n4n0_logo::paint(p, mark, theme::mark());
    pill(p, pill_rect(0), tr(SHOW), Some(icons::EYE), true, hover == Some(true));
    pill(p, pill_rect(1), tr(NOT_NOW), None, false, hover == Some(false));
    // Who first, whole (the size comes down until it fits), then the question, then where it opens.
    let words = mark.right() + 18.0;
    let room = pill_rect(0).left() - 16.0 - words;
    let size = crate::mismatch::fitting(p, &offer.from, &[32.0, 29.0, 26.0, 23.0], room);
    widgets::text_truncated(p, pos2(words, b.center().y - 34.0), Align2::LEFT_CENTER, &Txt::new(&offer.from, size).weight(weight::SEMIBOLD), room + 4.0);
    let question = tr(ASK);
    let size = crate::mismatch::fitting(p, question, &[23.0, 21.0, 19.0, 17.0], room);
    widgets::text_truncated(p, pos2(words, b.center().y + 2.0), Align2::LEFT_CENTER, &Txt::new(question, size), room + 4.0);
    let place = where_(&offer);
    let size = crate::mismatch::fitting(p, &place, &[20.0, 18.0, 16.0], room);
    widgets::text_truncated(p, pos2(words, b.center().y + 34.0), Align2::LEFT_CENTER, &Txt::new(&place, size).color(theme::fg_3()), room + 4.0);
    time_left(p, b, radius, left);
}

/// The picker's face: "Look at it together with" and a pill for each friend here (at most three).
pub fn paint_pick(p: &Painter, names: &[String], hover: Option<usize>, left: f32) {
    let (b, radius) = frame(p, false);
    let mark = Rect::from_center_size(pos2(b.left() + 26.0 + 16.0, b.top() + 40.0), n4n0_logo::size_for_width(32.0));
    n4n0_logo::paint(p, mark, theme::mark());
    let words = mark.right() + 14.0;
    let room = b.right() - 26.0 - words;
    let title = tr(WITH_WHOM);
    let size = crate::mismatch::fitting(p, title, &[28.0, 25.0, 22.0], room);
    widgets::text_truncated(p, pos2(words, b.top() + 40.0), Align2::LEFT_CENTER, &Txt::new(title, size).weight(weight::SEMIBOLD), room + 4.0);
    let count = names.len().min(PICK_MOST);
    for (i, name) in names.iter().take(PICK_MOST).enumerate() {
        pill(p, name_rect(i, count), name, Some(icons::USER), i == 0, hover == Some(i));
    }
    time_left(p, b, radius, left);
}

/// What the share chip asked the band for: pick a friend for this capture as its view is now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    pub id: u64,
    pub capture: u64,
    pub revision: u64,
    pub reach: f32,
    pub moved: [f32; 2],
    pub until_ms: u64,
}

/// How long the picker waits.
pub const PICK_FOR_MS: u64 = 15_000;

impl Pick {
    pub fn left(&self, now_ms: u64) -> f32 {
        (self.until_ms.saturating_sub(now_ms) as f32 / PICK_FOR_MS as f32).clamp(0.0, 1.0)
    }
}

/// The picker up now, between the small view (which asks) and the band (which shows it): both in the overlay's process.
static PICKING: Mutex<Option<Pick>> = Mutex::new(None);

pub fn ask_pick(capture: u64, revision: u64, reach: f32, moved: [f32; 2], now_ms: u64) {
    let mut p = PICKING.locked();
    let id = p.map_or(1, |p| p.id + 1);
    *p = Some(Pick { id, capture, revision, reach, moved, until_ms: now_ms + PICK_FOR_MS });
}

/// The picker while its time lasts.
pub fn picking(now_ms: u64) -> Option<Pick> {
    PICKING.locked().filter(|p| p.until_ms > now_ms)
}

pub fn clear_pick() {
    if let Some(p) = PICKING.locked().as_mut() {
        p.until_ms = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_words_are_in_every_language() {
        for language in orbiters_i18n::Language::TRANSLATED {
            for words in [ASK, WHERE, SHOW, NOT_NOW, WITH_WHOM] {
                let said = orbiters_i18n::in_language(language, || tr(words).to_string());
                assert!(said != words, "{language:?}: {words}");
            }
        }
    }

    #[test]
    fn its_pills_are_where_they_are_drawn_and_nowhere_else() {
        let centre = |r: Rect| [r.center().x / PX[0], r.center().y / PX[1]];
        assert_eq!(answer_at(centre(pill_rect(0))), Some(true));
        assert_eq!(answer_at(centre(pill_rect(1))), Some(false));
        // The words on the left press nothing.
        assert_eq!(answer_at([0.2, 0.5]), None);
        for count in 1..=3 {
            for i in 0..count {
                assert_eq!(pick_at(centre(name_rect(i, count)), count), Some(i), "{count}: {i}");
            }
            assert_eq!(pick_at([0.5, 0.15], count), None, "the title presses nothing");
        }
        // Pills never overlap or leave the band.
        for count in 1..=3 {
            for i in 0..count {
                assert!(band().contains_rect(name_rect(i, count)));
                if i > 0 {
                    assert!(name_rect(i - 1, count).right() < name_rect(i, count).left());
                }
            }
        }
        assert!(pill_rect(0).bottom() < pill_rect(1).top() && band().contains_rect(pill_rect(1)));
    }

    #[test]
    fn the_picker_lasts_its_time_and_goes_when_cleared() {
        ask_pick(4, 7, 10.0, [0.0, 0.0], 1_000);
        let p = picking(1_000).expect("up");
        assert_eq!((p.capture, p.revision), (4, 7));
        assert!(picking(1_000 + PICK_FOR_MS).is_none());
        ask_pick(5, 7, 10.0, [0.0, 0.0], 2_000);
        assert!(picking(2_000).unwrap().id > p.id, "a new one has a new id");
        clear_pick();
        assert!(picking(2_000).is_none());
    }
}
