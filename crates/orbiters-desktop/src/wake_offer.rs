//! "Talk with N4N0?" (N4N0-11, Enzo's decision of 2026-11-19): during a call the wake word starts nothing by itself (the
//! call's voices reach the microphone too); N4N0 puts this button up for a few seconds where its owner looks, and pressing
//! it starts the listen. On the desktop it is a card of the window's toasts (bottom right, the same panel, the same
//! countdown line, gone by itself); in VR a band before the eyes in the play space ([`crate::overlay::floating`]),
//! pressed with a controller's laser.

use crate::backend::Ctl;
use crate::ipc::N4n0Command;
use egui::{pos2, vec2, Align2, Context, Id, Order, Painter, Rect, Sense};
use orbiters_i18n::{mark, tr};
use orbiters_n4n0::status::WakeOffer;
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Variant};
use orbiters_ui::{icons, n4n0_logo};

/// Its words, in English (the catalog area `wakeoffer` has them in every language).
pub const TITLE: &str = mark("Talk with N4N0?");
pub const WHY: &str = mark("Heard during your call");
pub const TALK: &str = mark("Talk");
pub const NOT_NOW: &str = mark("Not now");

/// The time now, Unix ms (what an offer's end is said in).
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The offer while it is up (its time not over yet).
pub fn up(offer: Option<WakeOffer>, now_ms: u64) -> Option<WakeOffer> {
    offer.filter(|o| o.left(now_ms) > 0.0)
}

/// As wide as the window's toasts, and as far from its corner.
const WIDTH: f32 = 380.0;
pub(crate) const HEIGHT: f32 = 76.0;
const MARGIN: f32 = 20.0;

/// The window's card, while an offer is up: Talk starts the listen, the cross puts it away.
pub fn window(ctx: &Context, ctl: &Ctl, offer: Option<WakeOffer>) {
    let now = now_ms();
    let Some(offer) = up(offer, now) else { return };
    let screen = ctx.content_rect();
    let id = Id::unique(("orbiters-wake-offer", offer.id));
    let enter = anim::spring_from(ctx, id.with("enter"), [0.0], [1.0], Spring::BOUNCY)[0];
    let base = Rect::from_min_size(pos2(screen.right() - MARGIN - WIDTH, screen.bottom() - MARGIN - HEIGHT), vec2(WIDTH, HEIGHT));
    let rect = Rect::from_center_size(base.center() + vec2(60.0 * (1.0 - enter), 0.0), base.size() * (0.92 + 0.08 * enter.min(1.0)));
    egui::Area::new(id).order(Order::Foreground).fixed_pos(screen.min).interactable(true).show(ctx, |ui| {
        let mut p = ui.painter().clone();
        p.multiply_opacity(enter.clamp(0.0, 1.0));
        // The card itself takes the clicks around its buttons (nothing under it is pressed through it).
        ui.interact(rect, id.with("card"), Sense::click());
        widgets::panel(&p, rect, 16.0);
        let mark = Rect::from_center_size(pos2(rect.left() + 16.0 + 14.0, rect.center().y - 1.0), n4n0_logo::size_for_width(28.0));
        n4n0_logo::paint(&p, mark, theme::MARK);
        let close = Rect::from_min_size(pos2(rect.right() - 12.0 - 32.0, rect.center().y - 16.0), vec2(32.0, 32.0));
        let talk = Button::new(tr(TALK)).icon(icons::MICROPHONE).variant(Variant::Primary);
        let w = talk.desired_width(ui).max(96.0);
        let at = Rect::from_min_size(pos2(close.left() - 8.0 - w, rect.center().y - 19.0), vec2(w, 38.0));
        let words = mark.right() + 12.0;
        let room = at.left() - 10.0 - words;
        widgets::text_truncated(&p, pos2(words, rect.center().y - 10.0), Align2::LEFT_CENTER, &Txt::new(tr(TITLE), 15.5).weight(weight::SEMIBOLD), room);
        widgets::text_truncated(&p, pos2(words, rect.center().y + 12.0), Align2::LEFT_CENTER, &Txt::new(tr(WHY), 12.5).color(theme::FG_3), room);
        if talk.width(w).show_at(ui, at).clicked() {
            ctl.n4n0(N4n0Command::WakeOffer { accept: true, id: Some(offer.id) });
        }
        if Button::new("").icon(icons::X).variant(Variant::Ghost).hint(tr(NOT_NOW)).width(32.0).show_at(ui, close).clicked() {
            ctl.n4n0(N4n0Command::WakeOffer { accept: false, id: Some(offer.id) });
        }
        // What is left of its time, as the toasts' line: shrinking towards the left inside the rounded corners.
        let k = offer.left(now);
        let track = Rect::from_min_max(pos2(rect.left() + 14.0, rect.bottom() - 3.0), pos2(rect.right() - 14.0, rect.bottom() - 1.0));
        p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * k, track.height())), theme::radius(1.0), theme::white(0.35));
    });
    // The line moves (30 frames a second is plenty), and the card goes when its time is over.
    ctx.request_repaint_after(std::time::Duration::from_millis(33));
}

/// The VR band's picture size, and its size in the play space.
pub const VR_PX: [u32; 2] = [640, 168];
pub const VR_WIDTH_M: f32 = 0.26;

/// The VR band's face: N4N0's mark, the question and why, a Talk pill (the whole band presses it), the line of time
/// left. `hover`: a laser is on it.
pub fn paint_vr(p: &Painter, hover: bool, left: f32) {
    let all = Rect::from_min_size(pos2(0.0, 0.0), vec2(VR_PX[0] as f32, VR_PX[1] as f32));
    let band = all.shrink(6.0);
    let radius = 36.0;
    let fill = if hover { theme::mix(theme::INK_950, theme::FG, 0.14) } else { theme::alpha(theme::INK_950, 0.94) };
    p.rect_filled(band, radius, fill);
    p.rect_stroke(band, radius, egui::Stroke::new(3.0, if hover { theme::ACCENT } else { theme::white(0.16) }), egui::StrokeKind::Inside);
    let mark = Rect::from_center_size(pos2(band.left() + 30.0 + 26.0, band.center().y - 2.0), n4n0_logo::size_for_width(52.0));
    n4n0_logo::paint(p, mark, theme::MARK);
    // The Talk pill on the right.
    let pill = Rect::from_min_size(pos2(band.right() - 26.0 - 170.0, band.center().y - 32.0), vec2(170.0, 64.0));
    p.rect_filled(pill, 32.0, if hover { theme::mix(theme::ACCENT, theme::FG, 0.2) } else { theme::ACCENT });
    widgets::icon(p, pos2(pill.left() + 40.0, pill.center().y), icons::MICROPHONE, 28.0, theme::INK_950);
    let talk = tr(TALK);
    let size = crate::mismatch::fitting(p, talk, &[28.0, 26.0, 24.0, 22.0], pill.width() - 76.0);
    widgets::text_truncated(p, pos2(pill.left() + 62.0, pill.center().y), Align2::LEFT_CENTER, &Txt::new(talk, size).weight(weight::SEMIBOLD).color(theme::INK_950), pill.width() - 72.0);
    // The question and why, whole in every language (the size comes down until they fit).
    let words = mark.right() + 22.0;
    let room = pill.left() - 18.0 - words;
    let title = tr(TITLE);
    let size = crate::mismatch::fitting(p, title, &[34.0, 31.0, 28.0, 25.0], room);
    widgets::text_truncated(p, pos2(words, band.center().y - 18.0), Align2::LEFT_CENTER, &Txt::new(title, size).weight(weight::SEMIBOLD), room + 4.0);
    let why = tr(WHY);
    let size = crate::mismatch::fitting(p, why, &[23.0, 21.0, 19.0], room);
    widgets::text_truncated(p, pos2(words, band.center().y + 22.0), Align2::LEFT_CENTER, &Txt::new(why, size).color(theme::FG_3), room + 4.0);
    // Time left, along its bottom edge.
    let track = Rect::from_min_max(pos2(band.left() + radius, band.bottom() - 9.0), pos2(band.right() - radius, band.bottom() - 5.0));
    p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * left.clamp(0.0, 1.0), track.height())), 2.0, theme::white(0.35));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_offer_is_up_until_its_time_is_over() {
        let offer = WakeOffer { id: 1, until_ms: 10_000, for_ms: 6_000 };
        assert_eq!(up(Some(offer), 4_000), Some(offer));
        assert_eq!(up(Some(offer), 9_999), Some(offer));
        assert_eq!(up(Some(offer), 10_000), None);
        assert_eq!(up(None, 0), None);
    }

    #[test]
    fn its_words_are_in_every_language() {
        for language in orbiters_i18n::Language::TRANSLATED {
            for words in [TITLE, WHY, TALK, NOT_NOW] {
                let said = orbiters_i18n::in_language(language, || tr(words).to_string());
                assert!(said != words, "{language:?}: {words}");
            }
        }
    }
}
