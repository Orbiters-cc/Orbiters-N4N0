//! The watch: the N4N0 mark on a wrist, as XSOverlay's wrist button. It rides its controller (SteamVR moves it with the
//! hand at the headset's rate), fades in as the wrist turns to the eyes and out otherwise ([`space::watch_shown`]), and
//! one pull of the other hand's trigger on it (its laser, or its fingertip) opens N4N0's menu, or closes it, as the
//! controller binding does (in the play space, or on the avatar's screen when it has one; the floating panel opens from
//! the dashboard tab's Float button). Its ring is the accent while the menu is open. It paints again only when that or
//! its hover changes.

use super::space::{self, Follow, Quad};
use crate::overlay::{Canvas, Presenter};
use egui::{pos2, Stroke};
use orbiters_openvr::{self as vr, overlay_flag, Matrix34};
use orbiters_ui::anim::Spring;
use orbiters_ui::{n4n0_logo, theme};

/// Its width in the play space (a watch's face), and its picture.
pub const WIDTH_M: f32 = 0.045;
pub const PX: u32 = 160;
/// Over the panel and its bar, under the lasers.
const SORT_ORDER: u32 = 12;
/// Pressable once it shows this much.
const PRESSABLE: f32 = 0.5;

pub struct Watch {
    handle: vr::OverlayHandle,
    canvas: Canvas,
    presented: crate::overlay::presented::Presented,
    /// The controller it is on, and where on it SteamVR was last told, while it is placed there.
    on: Option<(u32, Matrix34)>,
    fade: Follow,
    /// The alpha SteamVR was last told, and whether it shows.
    alpha: f32,
    visible: bool,
    /// What its picture shows (hovered, the panel open).
    drawn: Option<(bool, bool)>,
}

impl Watch {
    pub fn new(overlay: &vr::Overlay, key: &str) -> anyhow::Result<Self> {
        let handle = crate::overlay::slots::create(overlay, &format!("{key}.watch"), "Orbiters watch").map_err(|c| anyhow::anyhow!("watch: SteamVR overlay error {c}"))?; // i18n: not shown (a name for SteamVR)
        let _ = overlay.set_width_in_meters(handle, WIDTH_M);
        let _ = overlay.set_flag(handle, overlay_flag::IS_PREMULTIPLIED, true);
        let _ = overlay.set_sort_order(handle, SORT_ORDER);
        Ok(Self { handle, canvas: Canvas::new(PX, PX), presented: Default::default(), on: None, fade: Follow::at(0.0), alpha: 0.0, visible: false, drawn: None })
    }

    pub fn destroy(&self, overlay: &vr::Overlay) {
        let _ = overlay.destroy(self.handle);
    }

    /// Where it is this turn: on controller `device` (the `right` hand's or the left's) whose pose is `grip`, the eyes
    /// at `head`; `None` hides it (turned off, its hand not tracked, SteamVR's dashboard open). `dt` since the last
    /// turn, `live` false to jump instead of fading (reduce motion). Returns it as a target while it shows enough to
    /// be pressed.
    pub fn place(&mut self, overlay: &vr::Overlay, at: Option<(u32, bool, Matrix34)>, head: Option<&Matrix34>, dt: f32, live: bool) -> Option<Quad> {
        let Some((device, right, grip)) = at else {
            self.fade = Follow::at(0.0);
            self.on = None;
            self.set_alpha(overlay, 0.0);
            return None;
        };
        // On the back of the wrist, its mark turned right way up for the eyes whatever way the wrist is (palm down, its top would be
        // the thumb's side, towards the chest: upside down, Enzo's headset 2026-11-16).
        let up = head.map_or_else(|| space::eyes_before_watch(right).1, |h| space::eyes_in(&grip, h).1);
        let relative = space::watch_upright(right, up);
        if !self.on.is_some_and(|(d, told)| d == device && space::near(&told, &relative, 0.001, 0.0004)) {
            let _ = overlay.set_transform_device_relative(self.handle, device, &relative);
            self.on = Some((device, relative));
        }
        let transform = space::compose(&grip, &relative);
        let target = head.map_or(0.0, |h| space::watch_shown(&transform, h));
        if live {
            self.fade.step(target, dt, Spring::SNAPPY);
        } else {
            self.fade = Follow::at(target);
        }
        self.set_alpha(overlay, self.fade.value.clamp(0.0, 1.0));
        (self.fade.value >= PRESSABLE).then_some(Quad { transform, width: WIDTH_M, height: WIDTH_M })
    }

    /// Its picture for `hover` (a laser or a fingertip on it) and `open` (N4N0's menu is), painted when that changed.
    pub(in crate::overlay) fn paint(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, hover: bool, open: bool) {
        if self.visible {
            self.presented.hand_over(None, std::time::Instant::now(), |r| presenter.show_part(overlay, self.handle, self.canvas.pixels(), PX, PX, r));
        }
        if !self.visible || self.drawn == Some((hover, open)) {
            return;
        }
        // The mark's texture loads on the first frame: paint until it comes.
        for _ in 0..if self.drawn.is_none() { 2 } else { 1 } {
            self.canvas.paint(0.0, |p| paint(p, hover, open));
        }
        let changed = if self.drawn.is_none() { Some([0, 0, PX, PX]) } else { self.canvas.damage() };
        self.presented.hand_over(changed, std::time::Instant::now(), |r| presenter.show_part(overlay, self.handle, self.canvas.pixels(), PX, PX, r));
        self.drawn = Some((hover, open));
    }

    /// The watch's picture that shows is of its current look (it may be pressed).
    pub fn current(&self) -> bool {
        self.presented.current()
    }

    fn set_alpha(&mut self, overlay: &vr::Overlay, alpha: f32) {
        let alpha = if alpha < 0.01 { 0.0 } else { alpha };
        if (alpha - self.alpha).abs() >= 0.01 || (alpha == 0.0) != (self.alpha == 0.0) {
            let _ = overlay.set_alpha(self.handle, alpha);
            self.alpha = alpha;
        }
        // Faded out it is hidden: SteamVR then has nothing of it to draw.
        let visible = alpha > 0.0;
        if visible != self.visible {
            let _ = if visible { overlay.show(self.handle) } else { overlay.hide(self.handle) };
            self.visible = visible;
        }
    }
}

/// The watch's face: the N4N0 mark on ink in a ring, the accent while N4N0's menu is open, lighter under a laser.
pub fn paint(p: &egui::Painter, hover: bool, open: bool) {
    let c = PX as f32 / 2.0;
    let centre = pos2(c, c);
    let ring = if open { theme::ACCENT } else { theme::FG_3 };
    let ring = if hover { theme::mix(ring, theme::FG, 0.45) } else { ring };
    p.circle_filled(centre, c - 4.0, theme::alpha(theme::INK_950, 0.94));
    if hover {
        p.circle_filled(centre, c - 10.0, theme::white(0.08));
    }
    // (egui strokes a circle outside its radius: the ring is its rim.)
    p.circle_stroke(centre, c - 9.0, Stroke::new(if hover { 9.0 } else { 7.0 }, ring));
    n4n0_logo::paint(p, egui::Rect::from_center_size(centre, n4n0_logo::size_for_width(PX as f32 * 0.5)), theme::MARK);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_face_paints_round_with_the_mark() {
        let mut canvas = Canvas::new(PX, PX);
        for _ in 0..2 {
            canvas.paint(0.0, |p| paint(p, false, true));
        }
        let px = canvas.pixels().to_vec();
        let at = |x: u32, y: u32| &px[((y * PX + x) * 4) as usize..((y * PX + x) * 4 + 4) as usize];
        assert_eq!(at(1, 1)[3], 0, "round: clear in its corner");
        let ring = at(PX / 2, 5);
        assert!(ring[1] > 150 && ring[0] < 80, "an accent ring while the panel is open: {ring:?}");
        let mark = (PX / 2 - 20..PX / 2 + 20).flat_map(|y| (PX / 2 - 30..PX / 2 + 30).map(move |x| (x, y))).filter(|&(x, y)| at(x, y)[0] > 180).count();
        assert!(mark > 100, "the mark in its middle: {mark}");
        canvas.paint(0.0, |p| paint(p, false, false));
        let closed = canvas.pixels()[((5 * PX + PX / 2) * 4) as usize..((5 * PX + PX / 2) * 4 + 4) as usize].to_vec();
        assert!(closed[1] < 160, "grey while closed: {closed:?}");
    }
}
