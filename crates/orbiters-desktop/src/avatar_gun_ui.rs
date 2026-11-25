//! The avatar gun's card for the hit player ([`crate::controller::avatar_gun::Card`]): "{name} wants to change your avatar
//! to {avatar}" with Allow and Not now, then "{name} changed your avatar" with Undo, or "Could not change your avatar"
//! and why. One card with one face ([`paint_card`]), in the three places the app has for such things:
//! - on N4N0's window, in its bottom right corner over the other cards ([`window`]), the avatar's picture in it;
//! - in a headset, a face of the band before the eyes (`crate::overlay::floating::prompt`), pressed with a laser on the
//!   trigger's release; the question straight ahead, the notice low and to the right as the world notice;
//! - in desktop mode, in the bottom right corner of VRChat's window (`crate::overlay::orbit::desk_notice`).
//! Its ground is the world notice's (see-through over the game, nearly whole on the window); the question has its line
//! of time left. The menus' pictures have no texture of their own: there the avatar's place shows its mark.

use crate::backend::Ctl;
use crate::controller::avatar_gun::{Card, CardKind, Command};
use egui::{pos2, vec2, Align2, Context, Id, Order, Painter, Pos2, Rect, Sense, TextureId, Vec2};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets;

/// The card's size in its units.
pub const CARD: Vec2 = vec2(344.0, 96.0);
const RADIUS: f32 = 16.0;
const PAD: f32 = 14.0;
/// The avatar's picture, a square at the card's left.
const THUMB: f32 = 68.0;
/// Where its words start, and its pills.
const WORDS: f32 = PAD + THUMB + 12.0;
const PILL_Y: f32 = 62.0;
const PILL_H: f32 = 24.0;
const PILL_W: f32 = 104.0;
/// The cross's part, at the right end, its whole height (as the world notice's).
pub const CLOSE_W: f32 = 48.0;

/// The parts that can be pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    /// Allow, Undo.
    Main,
    /// Not now, or the cross.
    Other,
}

/// Whether the card has a cross at its right end (a notice), or two pills (the question).
fn has_cross(kind: &CardKind) -> bool {
    matches!(kind, CardKind::Changed | CardKind::Failed(_))
}

/// Where its parts are, in its units.
fn parts(kind: &CardKind) -> (Option<Rect>, Option<Rect>) {
    let pill = |i: f32| Rect::from_min_size(pos2(WORDS + i * (PILL_W + 8.0), PILL_Y), vec2(PILL_W, PILL_H));
    let cross = Rect::from_min_max(pos2(CARD.x - CLOSE_W, 0.0), CARD.to_pos2());
    match kind {
        CardKind::Ask => (Some(pill(0.0)), Some(pill(1.0))),
        CardKind::Changed => (Some(pill(0.0)), Some(cross)),
        CardKind::Failed(_) => (None, Some(cross)),
        CardKind::Changing | CardKind::Undoing => (None, None),
    }
}

/// The part at `p`, in the card's units from its top left. A pill takes a little room around it.
pub fn part_at(kind: &CardKind, p: Pos2) -> Option<Part> {
    if !Rect::from_min_size(Pos2::ZERO, CARD).contains(p) {
        return None;
    }
    let (main, other) = parts(kind);
    if other.is_some_and(|r| r.expand(if has_cross(kind) { 0.0 } else { 4.0 }).contains(p)) {
        return Some(Part::Other);
    }
    main.filter(|r| r.expand(4.0).contains(p)).map(|_| Part::Main)
}

pub fn part_at_uv(kind: &CardKind, uv: [f32; 2]) -> Option<Part> {
    part_at(kind, pos2(uv[0] * CARD.x, uv[1] * CARD.y))
}

/// The card as it shows now: its number and its kind (a question allowed becomes a notice with the same number, and a
/// place that hid it on the press shows the notice).
pub fn shown_id(card: &Card) -> u64 {
    let kind = match card.kind {
        CardKind::Ask => 1,
        CardKind::Changing => 2,
        CardKind::Changed => 3,
        CardKind::Undoing => 4,
        CardKind::Failed(_) => 5,
    };
    (card.id << 3) | kind
}

/// What a press on `part` of `card` asks of the service.
pub fn command(card: &Card, part: Part) -> Command {
    match part {
        Part::Main => Command::Main { card: card.id },
        Part::Other => Command::Close { card: card.id },
    }
}

fn pill(p: &Painter, rect: Rect, words: &str, glyph: &str, primary: bool, lit: bool, s: f32) {
    let fill = match (primary, lit) {
        (true, false) => theme::accent(),
        (true, true) => theme::mix(theme::accent(), theme::fg(), 0.25),
        (false, false) => theme::white(0.12),
        (false, true) => theme::white(0.24),
    };
    let ink = if primary { theme::ink_950() } else { theme::fg() };
    p.rect_filled(rect, rect.height() / 2.0, fill);
    let lead = 11.0 * s + 13.0 * s + 5.0 * s;
    widgets::icon(p, pos2(rect.left() + 11.0 * s + 6.5 * s, rect.center().y), glyph, 13.0 * s, ink);
    let room = rect.width() - lead - 10.0 * s;
    let size = crate::mismatch::fitting(p, words, &[12.0 * s, 11.5 * s, 11.0 * s, 10.5 * s], room);
    widgets::text_truncated(p, pos2(rect.left() + lead, rect.center().y), Align2::LEFT_CENTER, &Txt::new(words, size).weight(weight::SEMIBOLD).color(ink), room + 2.0 * s);
}

/// Wrapped words at `at` (their top left), at most `rows` rows of `width`, the size coming down until they fit.
fn words(p: &Painter, at: Pos2, text: &str, sizes: &[f32], width: f32, rows: usize, strong: bool, color: egui::Color32) -> f32 {
    let job_at = |size: f32| {
        let mut txt = Txt::new(text, size).color(color).line_height(size * 1.25);
        if strong {
            txt = txt.weight(weight::SEMIBOLD);
        }
        let mut job = txt.job(width);
        job.wrap.max_rows = rows;
        job.wrap.overflow_character = Some('\u{2026}');
        job
    };
    let fits = |size: f32| {
        let mut job = job_at(size);
        job.wrap.max_rows = usize::MAX;
        p.layout_job(job).rows.len() <= rows
    };
    let size = sizes.iter().copied().find(|&z| fits(z)).unwrap_or(*sizes.last().unwrap_or(&12.0));
    let galley = p.layout_job(job_at(size));
    let high = galley.size().y;
    p.galley(at, galley, color);
    high
}

/// The card's face at `at` (its top left), `s` pixels a unit. `ground`: how dark its ground is (the world notice's
/// [`crate::world_notice::GROUND`] over a game, [`crate::world_notice::WINDOW_GROUND`] on the window); `picture`: the
/// avatar's thumbnail where the place has one; `hover`: the part under a laser or the pointer; `left`: the share of the
/// question's time left.
pub fn paint_card(p: &Painter, at: Pos2, s: f32, ground: f32, card: &Card, picture: Option<TextureId>, hover: Option<Part>, left: f32) {
    let u = |x: f32, y: f32| at + vec2(x, y) * s;
    let rect = Rect::from_min_size(at, CARD * s);
    let ground = if hover.is_some() { (ground + 0.14).min(0.96) } else { ground };
    p.rect_filled(rect, RADIUS * s, theme::alpha(theme::ink_950(), ground));
    // The avatar: its picture, else its place with the gun's mark (or what is happening).
    let thumb = Rect::from_min_size(u(PAD, PAD), vec2(THUMB, THUMB) * s);
    let radii = [12.0 * s; 4];
    match (picture, &card.kind) {
        (Some(id), CardKind::Ask | CardKind::Changing | CardKind::Changed) => {
            orbiters_ui::paint::squircle_texture(p, thumb, radii, id, cover_uv(thumb.size(), 4.0 / 3.0), egui::Color32::WHITE);
        }
        (_, kind) => {
            orbiters_ui::paint::fill_convex(p, &orbiters_ui::paint::squircle_points_corners(thumb, radii), |_| theme::white(0.08));
            let (glyph, ink) = match kind {
                CardKind::Failed(_) => (icons::WARNING_CIRCLE, theme::WARN),
                CardKind::Changing | CardKind::Undoing => (icons::CIRCLE_NOTCH, theme::accent()),
                _ => (icons::USER_CIRCLE, theme::fg_2()),
            };
            widgets::icon(p, thumb.center(), glyph, 30.0 * s, ink);
        }
    }
    let right = if has_cross(&card.kind) { CARD.x - CLOSE_W - 6.0 } else { CARD.x - PAD };
    let room = (right - WORDS) * s;
    let title = card.title();
    let title_high = words(p, u(WORDS, PAD - 1.0), &title, &[13.5 * s, 13.0 * s, 12.5 * s, 12.0 * s, 11.5 * s], room, 2, true, theme::fg()) / s;
    let (main, other) = parts(&card.kind);
    let (main_words, other_words) = card.buttons();
    let scaled = |r: Rect| Rect::from_min_max(u(r.min.x, r.min.y), u(r.max.x, r.max.y));
    match &card.kind {
        CardKind::Ask | CardKind::Changed => {
            if let (Some(r), Some(w)) = (main, main_words) {
                let glyph = if card.kind == CardKind::Ask { icons::CHECK } else { icons::ARROW_COUNTER_CLOCKWISE };
                pill(p, scaled(r), w, glyph, true, hover == Some(Part::Main), s);
            }
            if let (Some(r), Some(w), false) = (other, other_words, has_cross(&card.kind)) {
                pill(p, scaled(r), w, icons::X, false, hover == Some(Part::Other), s);
            }
        }
        CardKind::Failed(_) | CardKind::Changing | CardKind::Undoing => {
            let detail = card.detail();
            if !detail.is_empty() {
                // Right under the title, as many rows as the card has room for.
                let top = PAD - 1.0 + title_high + 5.0;
                let rows = (((CARD.y - 8.0 - top) / 14.0).floor() as usize).max(1);
                words(p, u(WORDS, top), &detail, &[11.5 * s, 11.0 * s, 10.5 * s], room, rows, false, theme::fg_3());
            }
        }
    }
    if has_cross(&card.kind) {
        let zone = Rect::from_min_max(u(CARD.x - CLOSE_W, 0.0), rect.max);
        p.line_segment([u(CARD.x - CLOSE_W, 18.0), u(CARD.x - CLOSE_W, CARD.y - 18.0)], egui::Stroke::new(1.0 * s, theme::white(0.12)));
        let on = hover == Some(Part::Other);
        if on {
            p.circle_filled(zone.center(), 15.0 * s, theme::white(0.16));
        }
        widgets::icon(p, zone.center(), icons::X, 15.0 * s, if on { theme::fg() } else { theme::fg_2() });
    }
    if card.kind == CardKind::Ask {
        let track = Rect::from_min_max(u(RADIUS, CARD.y - 4.0), u(CARD.x - RADIUS, CARD.y - 2.0));
        p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * left.clamp(0.0, 1.0), track.height())), theme::radius(1.0), theme::white(0.35));
    }
}

/// The part of a picture of shape `aspect` (width over height) that covers a rect of `size`, centred.
pub fn cover_uv(size: Vec2, aspect: f32) -> Rect {
    let want = size.x / size.y.max(1.0);
    if aspect > want {
        let w = want / aspect;
        Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
    } else {
        let h = aspect / want;
        Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
    }
}

/// How much of it shows at `now_ms`: it fades in when it came and out over the end of its time.
pub fn opacity(card: &Card, now_ms: u64) -> f32 {
    let came = card.until_ms.saturating_sub(card.for_ms);
    let since = now_ms.saturating_sub(came) as f32 / crate::world_notice::FADE_IN_MS as f32;
    let left = card.until_ms.saturating_sub(now_ms) as f32 / crate::world_notice::FADE_OUT_MS as f32;
    since.min(left).clamp(0.0, 1.0)
}

/// On the window: its size in points, and how far from the window's edges.
const WINDOW_S: f32 = 1.1;
const MARGIN: f32 = 20.0;

/// The window's card while one is up, in its bottom right corner, `lift` above it (the cards under it). `picture`: the
/// avatar's thumbnail, once loaded.
pub fn window(ctx: &Context, ctl: &Ctl, card: Option<&Card>, lift: f32, picture: Option<TextureId>) {
    let now = crate::controller::unix_ms();
    let Some(card) = card.filter(|c| c.up(now)) else { return };
    let screen = ctx.content_rect();
    let size = CARD * WINDOW_S;
    let rect = Rect::from_min_size(pos2(screen.right() - MARGIN - size.x, screen.bottom() - MARGIN - size.y - lift), size);
    let id = Id::unique(("orbiters-avatar-gun-card", card.id));
    let shown = opacity(card, now);
    orbiters_ui::front::area(ctx, id, Order::Foreground, false).fixed_pos(screen.min).interactable(true).show(ctx, |ui| {
        let mut p = ui.painter().clone();
        p.multiply_opacity(shown);
        let response = ui.interact(rect, id.with("card"), Sense::click());
        let part = |at: Option<Pos2>| at.and_then(|h| part_at(&card.kind, ((h - rect.min) / WINDOW_S).to_pos2()));
        let hover = part(response.hover_pos());
        let response = if hover.is_some() { response.on_hover_cursor(egui::CursorIcon::PointingHand) } else { response };
        paint_card(&p, rect.min, WINDOW_S, crate::world_notice::WINDOW_GROUND, card, picture, hover, card.left(now));
        if response.clicked() {
            if let Some(pressed) = part(response.interact_pointer_pos()) {
                ctl.avatar_gun(command(card, pressed));
            }
        }
    });
    ctx.request_repaint_after(std::time::Duration::from_millis(if card.kind == CardKind::Ask || shown < 1.0 { 33 } else { 250 }));
}

/// In a headset: pixels a unit of its picture, the picture's size (the card alone) and its width in the play space.
pub const VR_S: f32 = 2.2;
pub const VR_PX: [u32; 2] = [757, 211];
pub const VR_WIDTH_M: f32 = 0.26;

/// The headset's picture.
pub fn paint_vr(p: &Painter, card: &Card, hover: Option<Part>, left: f32) {
    paint_card(p, Pos2::ZERO, VR_S, crate::world_notice::GROUND, card, None, hover, left);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::avatar_gun::Failure;

    fn card(kind: CardKind) -> Card {
        Card { id: 3, kind, from: "Kite".into(), avatar_id: "avtr_x".into(), avatar: "Paper Robot".into(), thumbnail: String::new(), until_ms: 20_000, for_ms: 20_000 }
    }

    /// Allow and Not now are where they are drawn; a notice's cross is its right end; nothing else presses.
    #[test]
    fn its_parts_are_where_they_are_drawn() {
        let ask = CardKind::Ask;
        let (main, other) = parts(&ask);
        assert_eq!(part_at(&ask, main.unwrap().center()), Some(Part::Main));
        assert_eq!(part_at(&ask, other.unwrap().center()), Some(Part::Other));
        assert_eq!(part_at(&ask, pos2(30.0, 30.0)), None, "the picture presses nothing");
        assert!(main.unwrap().right() < other.unwrap().left() && other.unwrap().right() <= CARD.x - PAD);
        let changed = CardKind::Changed;
        assert_eq!(part_at(&changed, pos2(CARD.x - 5.0, 5.0)), Some(Part::Other));
        assert_eq!(part_at(&changed, parts(&changed).0.unwrap().center()), Some(Part::Main));
        assert_eq!(part_at(&CardKind::Failed(Failure::NotPublic), pos2(WORDS + 10.0, PILL_Y + 10.0)), None, "a failure has its cross only");
        assert_eq!(part_at(&CardKind::Changing, pos2(CARD.x - 5.0, 5.0)), None, "nothing to press while VRChat is asked");
        assert_eq!(part_at_uv(&changed, [0.99, 0.5]), Some(Part::Other));
        assert_eq!(command(&card(ask), Part::Main), Command::Main { card: 3 });
        assert!((VR_PX[0] as f32 - CARD.x * VR_S).abs() < 1.0 && (VR_PX[1] as f32 - CARD.y * VR_S).abs() < 1.0);
        // A laser's size in a headset.
        let metres = VR_WIDTH_M / CARD.x;
        assert!(PILL_W * metres >= 0.07 && (PILL_H + 8.0) * metres >= 0.024, "{} x {} m", PILL_W * metres, PILL_H * metres);
    }

    #[test]
    fn every_face_paints_in_every_language() {
        for language in orbiters_i18n::Language::TRANSLATED.into_iter().chain([orbiters_i18n::Language::En]) {
            let mut canvas = crate::overlay::Canvas::new(VR_PX[0], VR_PX[1]);
            orbiters_i18n::in_language(language, || {
                for kind in [CardKind::Ask, CardKind::Changing, CardKind::Changed, CardKind::Undoing, CardKind::Failed(Failure::DidNotFollow { signed_in: false })] {
                    for hover in [None, Some(Part::Main), Some(Part::Other)] {
                        for _ in 0..2 {
                            canvas.paint(0.0, |p| paint_vr(p, &card(kind.clone()), hover, 0.5));
                        }
                    }
                }
            });
            assert!(canvas.pixels().iter().any(|&b| b != 0), "{language:?}");
        }
    }

    #[test]
    fn it_fades_in_and_out() {
        let c = card(CardKind::Changed);
        assert_eq!(opacity(&c, 0), 0.0);
        assert_eq!(opacity(&c, 5_000), 1.0);
        assert_eq!(opacity(&c, 20_000), 0.0);
    }
}
