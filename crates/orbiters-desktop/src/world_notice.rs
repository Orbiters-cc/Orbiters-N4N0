//! "No 3D picture of this world yet" (REQUEST13, [`crate::controller::explorer`]): on arriving in a world that has no
//! public 3D picture on Orbiters, once per world, a short invitation with one action, "Take a 3D picture" (N4N0's own
//! flow: it waits for VRChat's camera and says so). On the desktop a card of the window's toasts, above "Talk with
//! N4N0?" when both are up; in VR the band before the eyes ([`crate::overlay::floating::prompt`]), pressed with a laser.
//! It goes by itself, its line of time running out; Settings › Orbiters account turns these notices off.

use crate::backend::Ctl;
use crate::controller::explorer::{Command, WorldNotice, NOTICE_EARNS, NOTICE_TITLE, NOTICE_WHY, NOT_NOW, TAKE};
use egui::{pos2, vec2, Align2, Context, Id, Order, Painter, Rect, Sense};
use orbiters_i18n::tr;
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Variant};
use orbiters_ui::{icons, n4n0_logo};

/// Its second line: what a picture here earns when the server said it, else the plain invitation.
pub fn why(notice: &WorldNotice) -> &'static str {
    if notice.earns == Some(1.0) {
        tr(NOTICE_EARNS)
    } else {
        tr(NOTICE_WHY)
    }
}

const WIDTH: f32 = 460.0;
const HEIGHT: f32 = 76.0;
const MARGIN: f32 = 20.0;

/// The window's card while a notice is up; `above`: another card ("Talk with N4N0?") holds the corner, this one sits
/// over it.
pub fn window(ctx: &Context, ctl: &Ctl, notice: Option<&WorldNotice>, above: bool) {
    let now = crate::controller::explorer::now_ms();
    let Some(notice) = notice.filter(|n| n.up(now)) else { return };
    let screen = ctx.content_rect();
    let id = Id::unique(("orbiters-world-notice", notice.id));
    let enter = anim::spring_from(ctx, id.with("enter"), [0.0], [1.0], Spring::BOUNCY)[0];
    let lift = if above { HEIGHT + 12.0 } else { 0.0 };
    let base = Rect::from_min_size(pos2(screen.right() - MARGIN - WIDTH, screen.bottom() - MARGIN - HEIGHT - lift), vec2(WIDTH, HEIGHT));
    let rect = Rect::from_center_size(base.center() + vec2(60.0 * (1.0 - enter), 0.0), base.size() * (0.92 + 0.08 * enter.min(1.0)));
    egui::Area::new(id).order(Order::Foreground).fixed_pos(screen.min).interactable(true).show(ctx, |ui| {
        let mut p = ui.painter().clone();
        p.multiply_opacity(enter.clamp(0.0, 1.0));
        ui.interact(rect, id.with("card"), Sense::click());
        widgets::panel(&p, rect, 16.0);
        // A world, with N4N0's mark: what it is about, and who says it.
        let mark = Rect::from_center_size(pos2(rect.left() + 16.0 + 14.0, rect.center().y - 1.0), n4n0_logo::size_for_width(28.0));
        n4n0_logo::paint(&p, mark, theme::MARK);
        let close = Rect::from_min_size(pos2(rect.right() - 12.0 - 32.0, rect.center().y - 16.0), vec2(32.0, 32.0));
        let take = Button::new(tr(TAKE)).icon(icons::CUBE).variant(Variant::Primary);
        let w = take.desired_width(ui).max(96.0);
        let at = Rect::from_min_size(pos2(close.left() - 8.0 - w, rect.center().y - 19.0), vec2(w, 38.0));
        let words = mark.right() + 12.0;
        let room = at.left() - 10.0 - words;
        widgets::text_truncated(&p, pos2(words, rect.center().y - 10.0), Align2::LEFT_CENTER, &Txt::new(tr(NOTICE_TITLE), 15.0).weight(weight::SEMIBOLD), room);
        widgets::text_truncated(&p, pos2(words, rect.center().y + 12.0), Align2::LEFT_CENTER, &Txt::new(why(notice), 12.5).color(theme::FG_3), room);
        if take.width(w).show_at(ui, at).clicked() {
            ctl.explorer(Command::NoticeTake { id: notice.id });
        }
        if Button::new("").icon(icons::X).variant(Variant::Ghost).hint(tr(NOT_NOW)).width(32.0).show_at(ui, close).clicked() {
            ctl.explorer(Command::NoticeDismiss { id: notice.id });
        }
        let k = notice.left(now);
        let track = Rect::from_min_max(pos2(rect.left() + 14.0, rect.bottom() - 3.0), pos2(rect.right() - 14.0, rect.bottom() - 1.0));
        p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * k, track.height())), theme::radius(1.0), theme::white(0.35));
    });
    ctx.request_repaint_after(std::time::Duration::from_millis(33));
}

/// The VR band's picture size (taller than "Talk with N4N0?": its words and its action each have a row), and its width
/// in the play space.
pub const VR_PX: [u32; 2] = [640, 232];
pub const VR_WIDTH_M: f32 = 0.27;

/// The VR band's face: the mark and the two lines, then the whole-width "Take a 3D picture" (the whole band presses it),
/// the line of time left. `hover`: a laser is on it.
pub fn paint_vr(p: &Painter, notice: &WorldNotice, hover: bool, left: f32) {
    let all = Rect::from_min_size(pos2(0.0, 0.0), vec2(VR_PX[0] as f32, VR_PX[1] as f32));
    let band = all.shrink(6.0);
    let radius = 36.0;
    let fill = if hover { theme::mix(theme::INK_950, theme::FG, 0.14) } else { theme::alpha(theme::INK_950, 0.94) };
    p.rect_filled(band, radius, fill);
    p.rect_stroke(band, radius, egui::Stroke::new(3.0, if hover { theme::ACCENT } else { theme::white(0.16) }), egui::StrokeKind::Inside);
    let mark = Rect::from_center_size(pos2(band.left() + 30.0 + 24.0, band.top() + 60.0), n4n0_logo::size_for_width(48.0));
    n4n0_logo::paint(p, mark, theme::MARK);
    let words = mark.right() + 20.0;
    let room = band.right() - 30.0 - words;
    let title = tr(NOTICE_TITLE);
    let size = crate::mismatch::fitting(p, title, &[31.0, 28.0, 25.0, 23.0], room);
    widgets::text_truncated(p, pos2(words, band.top() + 42.0), Align2::LEFT_CENTER, &Txt::new(title, size).weight(weight::SEMIBOLD), room + 4.0);
    let why = why(notice);
    let size = crate::mismatch::fitting(p, why, &[22.0, 20.0, 18.0, 16.5], room);
    widgets::text_truncated(p, pos2(words, band.top() + 80.0), Align2::LEFT_CENTER, &Txt::new(why, size).color(theme::FG_3), room + 4.0);
    // The action, a pill across the band's foot.
    let pill = Rect::from_min_max(pos2(band.left() + 26.0, band.bottom() - 26.0 - 64.0), pos2(band.right() - 26.0, band.bottom() - 26.0));
    p.rect_filled(pill, 32.0, if hover { theme::mix(theme::ACCENT, theme::FG, 0.2) } else { theme::ACCENT });
    let take = tr(TAKE);
    let size = crate::mismatch::fitting(p, take, &[28.0, 26.0, 24.0, 22.0], pill.width() - 110.0);
    let wide = p.layout_no_wrap(take.to_string(), egui::FontId::proportional(size), theme::INK_950).size().x + 40.0;
    let from = pill.center().x - wide / 2.0;
    widgets::icon(p, pos2(from + 14.0, pill.center().y), icons::CUBE, 28.0, theme::INK_950);
    widgets::text_truncated(p, pos2(from + 40.0, pill.center().y), Align2::LEFT_CENTER, &Txt::new(take, size).weight(weight::SEMIBOLD).color(theme::INK_950), pill.width() - 100.0);
    let track = Rect::from_min_max(pos2(band.left() + radius, band.bottom() - 11.0), pos2(band.right() - radius, band.bottom() - 7.0));
    p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * left.clamp(0.0, 1.0), track.height())), 2.0, theme::white(0.35));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_words_are_in_every_language() {
        for language in orbiters_i18n::Language::TRANSLATED {
            for words in [NOTICE_TITLE, NOTICE_WHY, NOTICE_EARNS, TAKE] {
                let said = orbiters_i18n::in_language(language, || tr(words).to_string());
                assert!(said != words, "{language:?}: {words}");
            }
        }
    }

    #[test]
    fn it_says_the_point_only_when_the_server_said_a_picture_here_earns_one() {
        let mut n = WorldNotice { id: 1, world_id: "wrld_x".into(), world_name: String::new(), earns: Some(1.0), until_ms: 10, for_ms: 10 };
        orbiters_i18n::in_language(orbiters_i18n::Language::En, || {
            assert_eq!(why(&n), NOTICE_EARNS);
            n.earns = None;
            assert_eq!(why(&n), NOTICE_WHY);
            n.earns = Some(0.5);
            assert_eq!(why(&n), NOTICE_WHY, "a world with none earns 1 (ranks may be used by unshared ones): no promise of less");
        });
    }
}
