//! "Share 3D capture" under the Quick Menu's side tiles (REQUEST18 3, [`Zone::QmShare`]): Enzo, "when a 3D capture have
//! been done but none uploaded publicly, I would like to have a button "Share 3D capture" on the side panel with a small
//! thumbnail of the capture". The service says which capture ([`crate::controller::explorer::share_here`]); a press shares
//! it publicly as "Share publicly" beside VRChat's camera does (the same first time's question, the same Photos & captures
//! job, the same confirmation by Orbiters' answer: [`crate::controller::explorer::Command::ShareCapture`]). It goes once the
//! capture is public, says how far its share went while it runs, and what stopped it when it did not go.
//!
//! [`Zone::QmShare`]: super::Zone::QmShare

use super::share_sheet::{ASK_PUBLIC, MAKING, SHARING, YES_PUBLIC};
use super::{cover_uv, fitted, keep_colour, look, tile_back, Action, Item, Look};
use crate::controller::explorer::{self as ex, Status, Step};
use egui::epaint::RectShape;
use egui::{pos2, vec2, Align2, Color32, Painter, Rect, Stroke, TextureId};
use orbiters_i18n::{mark, tr};
use orbiters_ui::theme::{self, weight};
use orbiters_ui::{icons, widgets};

pub const SHARE_3D_CAPTURE: &str = mark("Share 3D capture");
pub const SHARE_3D_CAPTURE_WHY: &str = mark("No 3D capture of this world is public yet: share yours, everyone on Orbiters sees it.");
pub const NOT_SHARED: &str = mark("Not shared: try again");

/// What the tile shows.
#[derive(Clone, Debug, PartialEq)]
pub struct ShareTile {
    /// The capture's identity ([`ex::offer_id`]): what its presses name.
    pub id: u64,
    pub state: TileState,
    /// Its thumbnail, as a texture of the canvas being painted (added there).
    pub picture: Option<TextureId>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TileState {
    /// "Share 3D capture": a press shares it, or asks first.
    Ready,
    /// The first time's question, up on the tile.
    Asking,
    /// On its way: its 3D view being made, how far its files went.
    Busy { making: bool, sent: Option<f32> },
    /// It did not go: why; a press tries again.
    Failed(String),
    /// Thirty first public shares today.
    Limited,
    /// Orbiters found it may be explicit: confirmed in Photos & captures.
    Explicit,
}

/// The tile from what the service says (`status`) and the question up here (`asking`, the capture it is for): none when no
/// capture is offered, or once its public share is confirmed.
pub fn tile(status: &Status, asking: Option<u64>) -> Option<ShareTile> {
    let here = status.share_here.as_ref()?;
    let id = ex::offer_id(&here.key);
    let step = status.offer.as_ref().filter(|o| o.id == id).map(|o| o.public.clone()).filter(|s| *s != Step::Idle).or_else(|| here.last.clone());
    let state = match step {
        Some(Step::Done(_)) => return None,
        Some(Step::Busy { making, sent }) => TileState::Busy { making, sent },
        _ if asking == Some(id) => TileState::Asking,
        Some(Step::Failed(why)) => TileState::Failed(why),
        Some(Step::Limited) => TileState::Limited,
        Some(Step::Explicit) => TileState::Explicit,
        Some(Step::Idle) | None => TileState::Ready,
    };
    Some(ShareTile { id, state, picture: None })
}

/// The zone's items, `w` x `h`: the tile, or the question with its two buttons (on top: the last is found first).
pub fn items(w: f32, h: f32, tile: &ShareTile) -> Vec<Item> {
    let whole = Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h));
    match tile.state {
        TileState::Asking => {
            let pad = 6.0;
            let tall = 40.0;
            vec![
                Item { action: Action::Idle, rect: whole, look: Look::ShareHereAsk },
                Item { action: Action::ShareHereNo, rect: Rect::from_min_size(pos2(pad, h - pad - tall), vec2(tall, tall)), look: Look::ShareHereNo },
                Item { action: Action::ShareHereYes(tile.id), rect: Rect::from_min_max(pos2(pad + tall + pad, h - pad - tall), pos2(w - pad, h - pad)), look: Look::ShareHereYes },
            ]
        }
        // Pressed: shared (or asked first); after a failure, again.
        TileState::Ready | TileState::Failed(_) => vec![Item { action: Action::ShareHere(tile.id), rect: whole, look: Look::ShareHere }],
        TileState::Busy { .. } | TileState::Limited | TileState::Explicit => vec![Item { action: Action::Idle, rect: whole, look: Look::ShareHere }],
    }
}

/// How the tile says its state, under its picture: an icon, its colour, the words.
fn label(state: &TileState) -> (&'static str, Color32, String) {
    match state {
        TileState::Ready | TileState::Asking => (icons::GLOBE, look::accent(), tr(SHARE_3D_CAPTURE).into()),
        TileState::Busy { making: true, .. } => (icons::CIRCLE_NOTCH, theme::INDIGO, tr(MAKING).into()),
        TileState::Busy { sent: Some(k), .. } => (icons::CIRCLE_NOTCH, theme::INDIGO, orbiters_i18n::tr!("Sending {percent}", percent = orbiters_ui::format::percent(*k as f64, 1.0))),
        TileState::Busy { .. } => (icons::CIRCLE_NOTCH, theme::INDIGO, tr(SHARING).into()),
        TileState::Failed(_) => (icons::WARNING, theme::WARN, tr(NOT_SHARED).into()),
        TileState::Limited => (icons::WARNING, theme::WARN, tr(ex::LIMIT_TITLE).into()),
        TileState::Explicit => (icons::WARNING, theme::WARN, tr(SHARE_3D_CAPTURE).into()),
    }
}

/// What lies over its picture when the share stopped: why.
fn reason(state: &TileState) -> Option<String> {
    match state {
        TileState::Failed(why) => Some(why.clone()),
        TileState::Limited => Some(tr(ex::LIMIT_WHY).into()),
        TileState::Explicit => Some(tr(super::share_sheet::EXPLICIT).into()),
        _ => None,
    }
}

/// Words wrapped in `width`, as large as fit `high` (from `size` down to `least`).
fn wrapped(p: &Painter, s: &str, size: f32, w: f32, width: f32, high: f32, least: f32, color: Color32) -> std::sync::Arc<egui::Galley> {
    let mut at = size;
    loop {
        let galley = p.layout(s.to_string(), look::font(at, w), color, width);
        if galley.size().y <= high || at - 0.5 < least {
            return galley;
        }
        at -= 0.5;
    }
}

/// One of the zone's items.
pub fn draw(p: &Painter, item: &Item, tile: &ShareTile, hovered: bool, pressed: bool) {
    let rect = item.rect;
    match item.look {
        Look::ShareHere => {
            let live = item.action != Action::Idle;
            tile_back(p, rect, hovered && live, pressed && live, false);
            let ink = look::accent();
            if hovered && live && !look::retro() {
                p.rect_filled(rect, look::radius(look::TILE_RADIUS), theme::alpha(ink, if pressed { 0.26 } else { 0.16 }));
                p.rect_stroke(rect.shrink(0.75), look::radius(look::TILE_RADIUS), Stroke::new(2.0, ink), egui::StrokeKind::Middle);
            }
            // Its picture, small, over its words.
            let picture = Rect::from_min_max(rect.min + vec2(6.0, 6.0), pos2(rect.right() - 6.0, rect.bottom() - 32.0));
            match tile.picture {
                Some(id) => {
                    keep_colour(p, picture);
                    p.add(RectShape::filled(picture, look::radius(6.0), Color32::WHITE).with_texture(id, cover_uv(picture.size())));
                }
                None => {
                    p.rect_filled(picture, look::radius(6.0), look::HEADER);
                    widgets::icon(p, picture.center(), icons::CUBE, 22.0, look::MUTED);
                }
            }
            // A capture in 3D, said as the gallery's tiles say it.
            let chip = Rect::from_min_size(picture.min + vec2(5.0, 5.0), vec2(22.0, 14.0));
            p.rect_filled(chip, look::radius(7.0), look::accent());
            p.text(chip.center(), Align2::CENTER_CENTER, "3D", look::font(9.0, weight::BOLD), look::on_accent());
            // While it goes: the picture dimmed, its files' share along its foot.
            if let TileState::Busy { sent, .. } = tile.state {
                p.rect_filled(picture, look::radius(6.0), Color32::from_black_alpha(120));
                let bar = Rect::from_min_max(pos2(picture.left() + 8.0, picture.bottom() - 9.0), pos2(picture.right() - 8.0, picture.bottom() - 5.0));
                p.rect_filled(bar, look::radius(2.0), Color32::from_white_alpha(40));
                let done = sent.unwrap_or(0.0).clamp(0.0, 1.0);
                if done > 0.0 {
                    p.rect_filled(Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * done, bar.bottom())), look::radius(2.0), theme::INDIGO);
                }
            }
            // Stopped: why, over its picture.
            if let Some(why) = reason(&tile.state) {
                p.rect_filled(picture, look::radius(6.0), Color32::from_black_alpha(185));
                let galley = wrapped(p, &why, 10.5, weight::MEDIUM, picture.width() - 14.0, picture.height() - 8.0, 8.5, look::LABEL);
                let at = pos2(picture.left() + 7.0, picture.center().y - galley.size().y / 2.0);
                p.with_clip_rect(picture).galley(at, galley, look::LABEL);
            }
            let (glyph, tint, words) = label(&tile.state);
            let row = Rect::from_min_max(pos2(rect.left() + 8.0, picture.bottom() + 2.0), pos2(rect.right() - 8.0, rect.bottom() - 2.0));
            let (shown, font) = fitted(p, &words, 11.5, weight::SEMIBOLD, row.width() - 20.0, 9.0);
            let wide = p.layout_no_wrap(shown.clone(), font.clone(), Color32::WHITE).size().x + 20.0;
            let x = row.center().x - wide / 2.0;
            widgets::icon(p, pos2(x + 7.0, row.center().y), glyph, 14.0, tint);
            p.text(pos2(x + 20.0, row.center().y), Align2::LEFT_CENTER, shown, font, if live || matches!(tile.state, TileState::Busy { .. }) { look::LABEL } else { look::MUTED });
        }
        Look::ShareHereAsk => {
            // The first time only, as beside the camera: what sharing publicly means, then Cancel and Yes.
            p.rect_filled(rect, look::radius(look::TILE_RADIUS), look::PANEL);
            p.rect_stroke(rect.shrink(0.75), look::radius(look::TILE_RADIUS), Stroke::new(1.5, look::accent()), egui::StrokeKind::Middle);
            let room = Rect::from_min_max(rect.min + vec2(8.0, 7.0), pos2(rect.right() - 8.0, rect.bottom() - 6.0 - 40.0 - 6.0));
            let galley = wrapped(p, tr(ASK_PUBLIC), 11.0, weight::MEDIUM, room.width(), room.height(), 8.5, look::LABEL);
            p.with_clip_rect(room).galley(pos2(room.left(), room.center().y - galley.size().y / 2.0), galley, look::LABEL);
        }
        Look::ShareHereNo => {
            p.rect_filled(rect, look::radius(10.0), if pressed { look::TILE_DOWN } else if hovered { look::TILE_HOVER } else { look::TILE });
            widgets::icon(p, rect.center(), icons::X, 16.0, look::LABEL);
        }
        Look::ShareHereYes => {
            let fill = if pressed { theme::mix(look::accent(), Color32::BLACK, 0.2) } else if hovered { theme::mix(look::accent(), Color32::WHITE, 0.12) } else { look::accent() };
            p.rect_filled(rect, look::radius(10.0), fill);
            let galley = wrapped(p, tr(YES_PUBLIC), 11.0, weight::SEMIBOLD, rect.width() - 10.0, rect.height() - 6.0, 8.5, look::on_accent());
            let at = pos2(rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0);
            p.galley(at, galley, look::on_accent());
        }
        _ => {}
    }
}

/// What the wing's caption says while one of its items is pointed at.
pub fn caption(action: Action, tile: Option<&ShareTile>) -> Option<(String, String)> {
    match action {
        Action::ShareHere(_) => Some(match tile.map(|t| &t.state) {
            Some(TileState::Failed(why)) => (tr(NOT_SHARED).into(), why.clone()),
            _ => (tr(SHARE_3D_CAPTURE).into(), tr(SHARE_3D_CAPTURE_WHY).into()),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::explorer::{Done, Offer, ShareHere};

    fn status(offer: Option<Step>, last: Option<Step>) -> Status {
        let key = "C:/Sessions/2026-11-20_10-00-00".to_string();
        Status {
            share_here: Some(ShareHere { key: key.clone(), world_id: "wrld_sample".into(), world_name: "Sample world".into(), last }),
            offer: offer.map(|public| Offer { id: ex::offer_id(&key), key, world_id: "wrld_sample".into(), would_earn: None, until_ms: u64::MAX, public, people: Step::Idle }),
            ..Default::default()
        }
    }

    /// REQUEST18 3: the tile is there while a capture is offered, says its share while it goes, and goes once it is public.
    #[test]
    fn the_tile_follows_its_capture_s_share() {
        assert_eq!(tile(&Status::default(), None), None, "no capture offered: no tile");
        let ready = tile(&status(None, None), None).expect("a tile");
        assert_eq!(ready.state, TileState::Ready);
        assert_eq!(items(162.0, 102.0, &ready).iter().map(|i| i.action).collect::<Vec<_>>(), [Action::ShareHere(ready.id)]);
        // Asked first: the question and its two buttons; nothing shares from the tile itself.
        let asking = tile(&status(None, None), Some(ready.id)).unwrap();
        assert_eq!(asking.state, TileState::Asking);
        assert_eq!(items(162.0, 102.0, &asking).iter().map(|i| i.action).collect::<Vec<_>>(), [Action::Idle, Action::ShareHereNo, Action::ShareHereYes(ready.id)]);
        // Another capture's question does not show on this one.
        assert_eq!(tile(&status(None, None), Some(ready.id ^ 2)).unwrap().state, TileState::Ready);
        // On its way: said, and a press does nothing.
        let busy = tile(&status(Some(Step::Busy { making: false, sent: Some(0.4) }), None), None).unwrap();
        assert_eq!(busy.state, TileState::Busy { making: false, sent: Some(0.4) });
        assert_eq!(items(162.0, 102.0, &busy)[0].action, Action::Idle);
        // Confirmed public: gone.
        assert_eq!(tile(&status(Some(Step::Done(Done { people: 0, earned: Some(1.0), nothing: None, after: None, reached: None })), None), None), None);
        // It did not go: why, and a press tries again; also once the offer has gone (what the service kept).
        let failed = tile(&status(None, Some(Step::Failed("Sign in to Orbiters in Settings first.".into()))), None).unwrap();
        assert_eq!(failed.state, TileState::Failed("Sign in to Orbiters in Settings first.".into()));
        assert_eq!(items(162.0, 102.0, &failed)[0].action, Action::ShareHere(failed.id));
        assert_eq!(tile(&status(Some(Step::Limited), None), None).unwrap().state, TileState::Limited);
    }
}
