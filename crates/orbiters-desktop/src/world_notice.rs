//! "No 3D picture of this world yet" (REQUEST13, [`crate::controller::explorer`]): on arriving in a world that has no
//! public 3D picture on Orbiters, once per world, a short invitation with one action, "Take a 3D picture" (N4N0's own
//! flow: it waits for VRChat's camera and says so).
//!
//! Request20 (Enzo, 2026-11-22: "a bit more discrete, like on the bottom right side with a more transparent background
//! and a close button"): one small card, the same face everywhere ([`paint_card`]): its words on one line, the action
//! as a small pill under them, a cross at its right end. Over the game its ground is see-through (a third of what is
//! behind shows; on N4N0's own window, over the page's words, nearly whole) and it has no frame, as nothing floating has
//! one. A press on the card is "Take a 3D picture"; a press on the cross closes
//! it, and that world is never told again for this account (the controller keeps it with the worlds told). It fades in,
//! stays [`crate::controller::explorer::NOTICE_MS`], and fades out by itself; frames are asked for only while it fades.
//! - On N4N0's window: in its bottom right corner, over "Talk with N4N0?" when both are up ([`window`]).
//! - In a headset: before the eyes, low and to the right of where they looked when it came ([`VR_SPOT`], the band of
//!   [`crate::overlay::floating::prompt`]), pressed with a laser; never over a menu, our panel or SteamVR's dashboard.
//! - In desktop mode: in the bottom right corner of VRChat's window ([`crate::overlay::orbit::desk_notice`]).
//! Settings › Orbiters account turns these notices off.

use crate::backend::Ctl;
use crate::controller::explorer::{Command, WorldNotice, EARNED_ONE, NOTICE_EARNS, NOTICE_TITLE, NOTICE_WHY, NOT_NOW, TAKE};
use egui::{pos2, vec2, Align2, Context, Id, Order, Painter, Pos2, Rect, Sense, Vec2};
use orbiters_i18n::tr;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets;
use orbiters_ui::icons;

/// What a picture here earns, when the server said it, else the plain invitation: the window's card says it on hover.
pub fn why(notice: &WorldNotice) -> &'static str {
    why_of(notice.earns == Some(1.0))
}

fn why_of(earns_one: bool) -> &'static str {
    if earns_one {
        tr(NOTICE_EARNS)
    } else {
        tr(NOTICE_WHY)
    }
}

/// The card's size in its units (points on the window, scaled for a headset and VRChat's window).
pub const CARD: Vec2 = vec2(288.0, 66.0);
const RADIUS: f32 = 16.0;
const PAD: f32 = 14.0;
/// The cross's part: the card's right end, its whole height (in a headset 4 x 5 cm, a laser's easy target).
pub const CLOSE_W: f32 = 52.0;
/// How much of its ground is dark: a third of what is behind shows through (it was 94 % to 96 %). White words on it keep
/// more than 5:1 over a white world (the ground then reads as a mid grey) and far more over a dark one.
pub const GROUND: f32 = 0.66;
/// On N4N0's own window it lies over the page's own words: there its ground is nearly whole, so they do not show through.
pub const WINDOW_GROUND: f32 = 0.92;
/// Under a laser or the pointer, a little darker.
const DARKER: f32 = 0.14;
/// How long it takes to come and to go.
pub const FADE_IN_MS: u64 = 300;
pub const FADE_OUT_MS: u64 = 900;

/// The two parts of the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    /// The card: "Take a 3D picture".
    Take,
    /// Its cross: closed, that world never told again.
    Close,
}

/// The part at `p`, in the card's units from its top left.
pub fn part_at(p: Pos2) -> Option<Part> {
    if !Rect::from_min_size(Pos2::ZERO, CARD).contains(p) {
        return None;
    }
    Some(if p.x >= CARD.x - CLOSE_W { Part::Close } else { Part::Take })
}

/// The part at `uv` (0 to 1 from the top left, as a laser hits a picture of the card alone).
pub fn part_at_uv(uv: [f32; 2]) -> Option<Part> {
    part_at(pos2(uv[0] * CARD.x, uv[1] * CARD.y))
}

/// How much of it shows at `now_ms`: it fades in after it came (or came back from behind a menu), and out over the end
/// of its time.
pub fn opacity(notice: &WorldNotice, now_ms: u64) -> f32 {
    let came = notice.until_ms.saturating_sub(notice.for_ms);
    let since = now_ms.saturating_sub(came) as f32 / FADE_IN_MS as f32;
    let left = notice.until_ms.saturating_sub(now_ms) as f32 / FADE_OUT_MS as f32;
    since.min(left).clamp(0.0, 1.0)
}

/// How long it stays as it is from `now_ms` (no frame needed meanwhile): none while it fades.
pub fn still_for_ms(notice: &WorldNotice, now_ms: u64) -> u64 {
    let o = opacity(notice, now_ms);
    if o > 0.0 && o < 1.0 {
        return 0;
    }
    notice.until_ms.saturating_sub(now_ms).saturating_sub(FADE_OUT_MS)
}

/// The card's face at `at` (its top left), `s` pixels a unit: its words on one line, "Take a 3D picture" as a pill under
/// them (and "+1 Explorer point" beside it when a picture here earns one, or "+1" where that does not fit), the cross at
/// its right end. `ground`: how dark its ground is ([`GROUND`] over a game, [`WINDOW_GROUND`] on N4N0's window); `hover`:
/// the part under a laser or the pointer.
pub fn paint_card(p: &Painter, at: Pos2, s: f32, ground: f32, earns_one: bool, hover: Option<Part>) {
    let u = |x: f32, y: f32| at + vec2(x, y) * s;
    let card = Rect::from_min_size(at, CARD * s);
    let ground = if hover.is_some() { (ground + DARKER).min(0.96) } else { ground };
    p.rect_filled(card, RADIUS * s, theme::alpha(theme::ink_950(), ground));
    let close_left = CARD.x - CLOSE_W;
    let room = (close_left - 6.0 - PAD) * s;
    let title = tr(NOTICE_TITLE);
    let size = crate::mismatch::fitting(p, title, &[13.5 * s, 13.0 * s, 12.5 * s, 12.0 * s, 11.5 * s], room);
    widgets::text_truncated(p, u(PAD, 21.0), Align2::LEFT_CENTER, &Txt::new(title, size).weight(weight::SEMIBOLD).color(theme::fg()), room + 2.0 * s);
    // The action, a small pill under the words.
    let take = tr(TAKE);
    let lead = 11.0 * s + 13.0 * s + 5.0 * s;
    let words_room = room - lead - 11.0 * s;
    let tsize = crate::mismatch::fitting(p, take, &[12.0 * s, 11.5 * s, 11.0 * s, 10.5 * s], words_room);
    let words_w = p.layout_no_wrap(take.to_string(), egui::FontId::proportional(tsize), theme::ink_950()).size().x.min(words_room);
    let pill = Rect::from_min_size(u(PAD - 3.0, 35.0), vec2(lead + words_w + 11.0 * s, 22.0 * s));
    let lit = hover == Some(Part::Take);
    p.rect_filled(pill, pill.height() / 2.0, if lit { theme::mix(theme::accent(), theme::fg(), 0.25) } else { theme::accent() });
    widgets::icon(p, pos2(pill.left() + 11.0 * s + 6.5 * s, pill.center().y), icons::CUBE, 13.0 * s, theme::ink_950());
    widgets::text_truncated(p, pos2(pill.left() + lead, pill.center().y), Align2::LEFT_CENTER, &Txt::new(take, tsize).weight(weight::SEMIBOLD).color(theme::ink_950()), words_room + 2.0 * s);
    if earns_one {
        let from = pill.right() + 8.0 * s;
        let left = u(close_left - 12.0, 0.0).x - from;
        let whole = tr(EARNED_ONE);
        let fits = |t: &str, size: f32| p.layout_no_wrap(t.to_string(), egui::FontId::proportional(size), theme::fg()).size().x <= left;
        let said = [11.5 * s, 11.0 * s, 10.5 * s].into_iter().find(|&z| fits(whole, z)).map(|z| (whole, z)).or_else(|| fits("+1", 12.0 * s).then_some(("+1", 12.0 * s)));
        if let Some((t, z)) = said {
            widgets::text(p, pos2(from, pill.center().y), Align2::LEFT_CENTER, &Txt::new(t, z).weight(weight::SEMIBOLD).color(theme::fg()));
        }
    }
    // The cross, at the card's right end, its part the whole height.
    let zone = Rect::from_min_max(u(close_left, 0.0), card.max);
    p.line_segment([u(close_left, 17.0), u(close_left, CARD.y - 17.0)], egui::Stroke::new(1.0 * s, theme::white(0.12)));
    let on_close = hover == Some(Part::Close);
    if on_close {
        p.circle_filled(zone.center(), 15.0 * s, theme::white(0.16));
    }
    widgets::icon(p, zone.center(), icons::X, 15.0 * s, if on_close { theme::fg() } else { theme::fg_2() });
}

/// On the window: its size in points, and how far from the window's edges.
const WINDOW_S: f32 = 1.1;
const MARGIN: f32 = 20.0;

/// The window's card while a notice is up, in its bottom right corner. `lift`: how much of the corner the cards under it
/// take ("Talk with N4N0?", a friend's offer to look at a 3D picture together), their gaps included. Only its own
/// rectangle takes the pointer.
pub fn window(ctx: &Context, ctl: &Ctl, notice: Option<&WorldNotice>, lift: f32) {
    let now = crate::controller::explorer::now_ms();
    let Some(notice) = notice.filter(|n| n.up(now)) else { return };
    let screen = ctx.content_rect();
    let size = CARD * WINDOW_S;
    let rect = Rect::from_min_size(pos2(screen.right() - MARGIN - size.x, screen.bottom() - MARGIN - size.y - lift), size);
    let id = Id::unique(("orbiters-world-notice", notice.id));
    let shown = opacity(notice, now);
    orbiters_ui::front::area(ctx, id, Order::Foreground, false).fixed_pos(screen.min).interactable(true).show(ctx, |ui| {
        let mut p = ui.painter().clone();
        p.multiply_opacity(shown);
        let card = ui.interact(rect, id.with("card"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let part = |at: Option<Pos2>| at.and_then(|h| part_at(((h - rect.min) / WINDOW_S).to_pos2()));
        let hover = part(card.hover_pos());
        let card = match hover {
            Some(Part::Close) => card.on_hover_text(tr(NOT_NOW)),
            _ => card.on_hover_text(why(notice)),
        };
        paint_card(&p, rect.min, WINDOW_S, WINDOW_GROUND, notice.earns == Some(1.0), hover);
        if card.clicked() {
            match part(card.interact_pointer_pos()) {
                Some(Part::Close) => ctl.explorer(Command::NoticeDismiss { id: notice.id }),
                _ => ctl.explorer(Command::NoticeTake { id: notice.id }),
            }
        }
    });
    // Frames only while it fades; else the next is when its fade out starts.
    match still_for_ms(notice, now) {
        0 => ctx.request_repaint_after(std::time::Duration::from_millis(16)),
        ms => ctx.request_repaint_after(std::time::Duration::from_millis(ms)),
    }
}

/// In a headset: pixels a unit of its picture, the picture's size (the card alone, so a laser's place on it is a place on
/// the card), its width in the play space, and where it is put: `distance` ahead of the eyes, `below` their line and
/// `right` of it, in metres (about 17° right and 19° down: in view, out of the way of what is looked at). Its words are
/// about 1° high there, as VRChat's own small labels.
pub const VR_S: f32 = 2.2;
pub const VR_PX: [u32; 2] = [634, 146];
pub const VR_WIDTH_M: f32 = 0.22;
pub struct Spot {
    pub distance: f32,
    pub below: f32,
    pub right: f32,
}
pub const VR_SPOT: Spot = Spot { distance: 0.62, below: 0.21, right: 0.19 };

/// The headset's picture: the card at [`VR_S`]. `hover`: the part a laser is on.
pub fn paint_vr(p: &Painter, earns_one: bool, hover: Option<Part>) {
    paint_card(p, Pos2::ZERO, VR_S, GROUND, earns_one, hover);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(until_ms: u64, for_ms: u64) -> WorldNotice {
        WorldNotice { id: 1, world_id: "wrld_x".into(), world_name: String::new(), earns: Some(1.0), until_ms, for_ms }
    }

    #[test]
    fn its_words_are_in_every_language() {
        for language in orbiters_i18n::Language::TRANSLATED {
            for words in [NOTICE_TITLE, NOTICE_WHY, NOTICE_EARNS, TAKE, EARNED_ONE, NOT_NOW] {
                let said = orbiters_i18n::in_language(language, || tr(words).to_string());
                assert!(said != words, "{language:?}: {words}");
            }
        }
    }

    #[test]
    fn it_says_the_point_only_when_the_server_said_a_picture_here_earns_one() {
        let mut n = notice(10, 10);
        orbiters_i18n::in_language(orbiters_i18n::Language::En, || {
            assert_eq!(why(&n), NOTICE_EARNS);
            n.earns = None;
            assert_eq!(why(&n), NOTICE_WHY);
            n.earns = Some(0.5);
            assert_eq!(why(&n), NOTICE_WHY, "a world with none earns 1 (ranks may be used by unshared ones): no promise of less");
        });
    }

    /// Request20: the cross is the card's right end, its whole height; anywhere else on the card takes a 3D picture;
    /// off the card, nothing (what is behind it gets the press).
    #[test]
    fn the_cross_is_the_right_end_and_the_rest_takes_a_picture() {
        assert_eq!(part_at(pos2(20.0, 20.0)), Some(Part::Take));
        assert_eq!(part_at(pos2(CARD.x - CLOSE_W - 1.0, CARD.y - 2.0)), Some(Part::Take));
        assert_eq!(part_at(pos2(CARD.x - CLOSE_W + 1.0, 1.0)), Some(Part::Close));
        assert_eq!(part_at(pos2(CARD.x - 1.0, CARD.y - 1.0)), Some(Part::Close));
        assert_eq!(part_at(pos2(CARD.x + 1.0, 10.0)), None);
        assert_eq!(part_at(pos2(10.0, -1.0)), None);
        assert_eq!(part_at_uv([0.98, 0.5]), Some(Part::Close));
        assert_eq!(part_at_uv([0.3, 0.5]), Some(Part::Take));
        // In a headset its picture is the card alone, and the cross is a laser's size there.
        assert!((VR_PX[0] as f32 - CARD.x * VR_S).abs() < 1.0 && (VR_PX[1] as f32 - CARD.y * VR_S).abs() < 1.0);
        let metres = VR_WIDTH_M / CARD.x;
        assert!(CLOSE_W * metres >= 0.039 && CARD.y * metres >= 0.05, "{} x {} m", CLOSE_W * metres, CARD.y * metres);
        // Its title about a degree high where it is put.
        let title_m = 13.5 * metres;
        let degrees = (title_m / VR_SPOT.distance).atan().to_degrees();
        assert!((0.85..1.3).contains(&degrees), "{degrees}°");
    }

    /// Request20: it fades in, stays, fades out over the end of its time and is gone after it; frames are needed only while
    /// it fades. Its time is the controller's 12 s.
    #[test]
    fn it_goes_by_itself_after_its_time_with_a_fade() {
        use crate::controller::explorer::NOTICE_MS;
        assert_eq!(NOTICE_MS, 12_000);
        let came = 1_000_000;
        let n = notice(came + NOTICE_MS, NOTICE_MS);
        assert_eq!(opacity(&n, came), 0.0);
        assert!((opacity(&n, came + FADE_IN_MS / 2) - 0.5).abs() < 0.01);
        assert_eq!(opacity(&n, came + FADE_IN_MS), 1.0);
        assert_eq!(opacity(&n, came + 6_000), 1.0);
        assert!((opacity(&n, came + NOTICE_MS - FADE_OUT_MS / 2) - 0.5).abs() < 0.01);
        assert_eq!(opacity(&n, came + NOTICE_MS), 0.0);
        assert!(n.up(came + NOTICE_MS - 1) && !n.up(came + NOTICE_MS));
        // No frame while it stands still: the next is when its fade out starts.
        assert_eq!(still_for_ms(&n, came + 2_000), NOTICE_MS - 2_000 - FADE_OUT_MS);
        assert_eq!(still_for_ms(&n, came + 100), 0, "fading in");
        assert_eq!(still_for_ms(&n, came + NOTICE_MS - 300), 0, "fading out");
    }

    #[test]
    fn its_face_paints_in_every_language_and_fits() {
        for language in orbiters_i18n::Language::TRANSLATED {
            let mut canvas = crate::overlay::Canvas::new(VR_PX[0], VR_PX[1]);
            orbiters_i18n::in_language(language, || {
                for hover in [None, Some(Part::Take), Some(Part::Close)] {
                    for _ in 0..2 {
                        canvas.paint(0.0, |p| paint_vr(p, true, hover));
                    }
                }
            });
            assert!(canvas.pixels().iter().any(|&b| b != 0), "{language:?}: painted");
        }
    }
}
