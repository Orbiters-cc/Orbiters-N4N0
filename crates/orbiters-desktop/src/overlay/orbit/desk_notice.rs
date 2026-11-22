//! "No 3D picture of this world yet" in desktop mode (Request20): the card of [`crate::world_notice`] in the bottom right
//! corner of VRChat's window (VRChat's own hints are in its top corners, the menu mismatch corner in its bottom left), a
//! window of its own as the panels are: it never takes the focus, and only its card's pixels take the mouse, everything
//! around it goes to the game. It shows only with nothing in its way: not while one of our pages is open, nor VRChat's
//! wheel (it holds the pointer) or camera panel, nor while VRChat's Quick Menu or a part of ours lies on its place; held
//! back, it tells the service, which keeps its time waiting ([`Command::NoticeHeld`], as the headset's band does). A click
//! on the card takes a 3D picture, on its cross closes it (that world is not told again). It fades in and out at its
//! window's alpha: no new picture for that.

use super::desktop::PanelWindow;
use crate::backend::Ctl;
use crate::controller::explorer::{Command, WorldNotice};
use crate::world_notice::{self, Part, CARD};
use egui::{pos2, Pos2, Rect, Vec2};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::POINT;

/// Its margin from the window's edges, in its units.
const MARGIN: f32 = 14.0;

/// Where it lies in a VRChat window of `client` pixels (window pixels), and its zoom (pixels a unit): in the bottom right
/// corner, by the window's height and a little smaller than the panels (1.35 at 1080 high), so that with VRChat's Quick
/// Menu open it lies right of the menu's foot and under our wing in every shape of window (`tests`).
pub(crate) fn rect(client: Vec2) -> (Rect, f32) {
    let zoom = (client.y / 800.0).min(client.x / 1100.0).clamp(0.7, 2.0);
    let size = CARD * zoom;
    let m = MARGIN * zoom;
    (Rect::from_min_size(pos2(client.x - m - size.x, client.y - m - size.y), size), zoom)
}

/// VRChat's Quick Menu itself in a window of `client` pixels: its panels and its row of icons under them (in its units
/// from its centre, as `zones` has them), a little room around.
pub(crate) fn quick_menu_rect(client: Vec2) -> Rect {
    let (center, unit) = super::zones::quick_menu(client);
    let units = Rect::from_min_max(pos2(-352.0, -300.0), pos2(352.0, super::zones::QM_ICONS_Y + 22.0)).expand(4.0);
    Rect::from_min_max(center + units.min.to_vec2() * unit, center + units.max.to_vec2() * unit)
}

/// Whether the card at `card` is clear of everything in `parts` (window pixels).
pub(crate) fn clear(card: Rect, parts: &[Rect]) -> bool {
    !parts.iter().any(|p| p.intersects(card.expand(4.0)))
}

#[derive(Default)]
pub(crate) struct Card {
    pub(super) window: Option<PanelWindow>,
    canvas: Option<crate::overlay::Canvas>,
    px: [u32; 2],
    /// What its picture shows: the notice, the part under the pointer, whether it says the point, the language.
    drawn: Option<(u64, Option<Part>, bool, orbiters_i18n::Language)>,
    /// A button went down on this part of this notice's card.
    down: Option<(u64, Part)>,
    /// The notice pressed here: it goes at once, before the service's word.
    gone: Option<u64>,
    /// When the service last heard that it is held back.
    held_said: Option<Instant>,
}

impl Card {
    pub(super) fn owns(&self, id: isize) -> bool {
        self.window.as_ref().is_some_and(|w| w.id() == id)
    }

    pub(super) fn hide(&mut self) {
        if let Some(w) = self.window.as_mut() {
            w.hide();
        }
        self.down = None;
    }

    pub(super) fn raise(&self) {
        if let Some(w) = self.window.as_ref().filter(|w| w.presented()) {
            w.raise();
        }
    }

    /// One turn: `notice` the service's, `in_the_way` one of our pages or VRChat's wheel or camera panel, `parts` what else
    /// lies over VRChat's window now (its Quick Menu, parts of ours), VRChat's window at `origin` (screen pixels) of `client`
    /// pixels, `clicks` the card window's buttons since the last turn (down or up), the pointer at `cursor`. Says whether a
    /// press acted.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn update(&mut self, backend: &Ctl, notice: Option<&WorldNotice>, in_the_way: bool, parts: &[Rect], origin: Pos2, client: Vec2, clicks: &[bool], cursor: (i32, i32), now: Instant) -> bool {
        let now_ms = crate::controller::explorer::now_ms();
        let Some(n) = notice.filter(|n| n.up(now_ms) && self.gone != Some(n.id)) else {
            self.hide();
            return false;
        };
        let (rect, zoom) = rect(client);
        if in_the_way || !clear(rect, parts) {
            self.hide();
            if self.held_said.is_none_or(|at| now.duration_since(at) >= Duration::from_secs(1)) {
                self.held_said = Some(now);
                backend.explorer(Command::NoticeHeld { id: n.id });
            }
            return false;
        }
        let px = [rect.width().ceil() as u32, rect.height().ceil() as u32];
        if self.px != px || self.window.is_none() {
            self.window = PanelWindow::new(px[0], px[1], true).map_err(|e| eprintln!("world notice window: {e}")).ok(); // i18n: not shown (a log's line)
            self.canvas = Some(crate::overlay::Canvas::new(px[0], px[1]));
            self.px = px;
            self.drawn = None;
        }
        let screen = rect.translate(origin.to_vec2());
        let c = pos2(cursor.0 as f32, cursor.1 as f32);
        let over = screen.contains(c).then(|| world_notice::part_at(((c - screen.min) / zoom).to_pos2())).flatten();
        let mut acted = false;
        for &down in clicks {
            if down {
                self.down = over.map(|p| (n.id, p));
            } else if let Some((id, part)) = self.down.take() {
                if id == n.id && over == Some(part) {
                    backend.explorer(match part {
                        Part::Take => Command::NoticeTake { id },
                        Part::Close => Command::NoticeDismiss { id },
                    });
                    self.gone = Some(id);
                    acted = true;
                }
            }
        }
        if acted {
            self.hide();
            return true;
        }
        let (Some(window), Some(canvas)) = (self.window.as_mut(), self.canvas.as_mut()) else { return false };
        let earns_one = n.earns == Some(1.0);
        let look = (n.id, over, earns_one, orbiters_i18n::language());
        if self.drawn != Some(look) {
            // The first paint loads the fonts.
            for _ in 0..if self.drawn.is_none() { 2 } else { 1 } {
                canvas.paint(0.0, |p| world_notice::paint_card(p, Pos2::ZERO, zoom, earns_one, over));
            }
            window.picture(canvas.pixels(), [0, 0, px[0], px[1]]);
            self.drawn = Some(look);
        }
        let alpha = (world_notice::opacity(n, now_ms) * 255.0).round() as u8;
        window.place_if_changed(POINT { x: screen.min.x.round() as i32, y: screen.min.y.round() as i32 }, alpha);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    const SIZES: [(f32, f32); 7] = [(800.0, 600.0), (1024.0, 768.0), (1280.0, 720.0), (1600.0, 900.0), (1920.0, 1080.0), (2560.0, 1440.0), (3440.0, 1440.0)];

    /// Request20: in every shape of VRChat's window the card lies whole inside it, in its bottom right corner, clear of
    /// the menu mismatch corner (bottom left) and of VRChat's hints (its top corners), and with VRChat's Quick Menu open
    /// it lies clear of the menu and of our wing (it may stay up then, where the pointer is free to press it).
    #[test]
    fn the_card_lies_in_the_bottom_right_corner_of_every_window() {
        for (w, h) in SIZES {
            let client = vec2(w, h);
            let (card, zoom) = rect(client);
            let window = Rect::from_min_size(Pos2::ZERO, client);
            assert!(window.contains_rect(card), "{w}x{h}: {card:?}");
            assert!(card.left() > w / 2.0 && card.top() > h * 0.7, "{w}x{h}: bottom right: {card:?}");
            assert!((w - card.right() - MARGIN * zoom).abs() < 0.5 && (h - card.bottom() - MARGIN * zoom).abs() < 0.5, "{w}x{h}: in the corner");
            assert!(card.width() <= w * 0.34, "{w}x{h}: small: {}", card.width());
            let (corner, _) = super::super::desktop::Corner::rect(client);
            assert!(!corner.intersects(card), "{w}x{h}: the mismatch corner");
            let qm = quick_menu_rect(client);
            let wing: Vec<Rect> = super::super::zones::layout(super::super::zones::Around::QuickMenu, client)
                .into_iter()
                .filter(|p| !p.zone.only_while_needed())
                .map(|p| p.rect)
                .collect();
            assert!(clear(card, &[qm]), "{w}x{h}: the Quick Menu {qm:?} {card:?}");
            assert!(clear(card, &wing), "{w}x{h}: our wing");
        }
    }

    /// What lies on its place holds it back; what does not, does not.
    #[test]
    fn only_what_lies_on_its_place_holds_it_back() {
        let client = vec2(1920.0, 1080.0);
        let (card, _) = rect(client);
        assert!(clear(card, &[]));
        assert!(!clear(card, &[Rect::from_center_size(card.center(), vec2(10.0, 10.0))]));
        assert!(clear(card, &[Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0))]));
    }
}
