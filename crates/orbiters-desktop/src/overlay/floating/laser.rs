//! The controllers' own lasers for the floating panel and the watch. SteamVR's laser comes with its dashboard, or with
//! an overlay that turns its laser mouse on, and either takes the controllers from VRChat; these are drawn here
//! instead: a beam from a controller's tip to where it points and a dot there, only while it points at the panel, its
//! bar or the watch. The beam rides its controller (SteamVR moves it with the controller at the headset's rate); only
//! its length and its turn about itself (towards the eyes, never seen edge on) change from turn to turn.

use super::space::{self, V3};
use crate::overlay::{Canvas, Presenter};
use egui::{pos2, vec2, Color32, Rect, Stroke};
use orbiters_openvr::{self as vr, overlay_flag, Matrix34};
use orbiters_ui::theme;

/// The beam's width, and its picture (stretched to any length through the texel aspect).
pub const BEAM_WIDTH_M: f32 = 0.004;
const BEAM_PX: [u32; 2] = [8, 256];
/// The dot where it ends: its width, and its picture.
pub const DOT_WIDTH_M: f32 = 0.016;
const DOT_PX: u32 = 64;
/// Above the panel and the watch, under nothing of the overlay's.
const SORT_ORDER: u32 = 30;

/// A laser to show this turn.
#[derive(Clone, Copy, Debug)]
pub struct Laser {
    /// Its controller (the beam rides it) and that controller's pose.
    pub device: u32,
    pub grip: Matrix34,
    /// Its tip (where it starts, pointing along its −Z), the eyes, where it ends and the face it ends on.
    pub tip: Matrix34,
    pub eyes: V3,
    pub end: V3,
    pub surface: Matrix34,
    /// Its trigger is held (the dot shrinks a little).
    pub pressed: bool,
}

/// What a laser's overlays were last told.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shown {
    device: u32,
    beam: Matrix34,
    aspect: f32,
    pressed: bool,
}

pub struct Lasers {
    beams: [vr::OverlayHandle; 2],
    dots: [vr::OverlayHandle; 2],
    shown: [Option<Shown>; 2],
    /// The pictures went to SteamVR (once, the first time a laser shows).
    painted: bool,
}

impl Lasers {
    pub fn new(overlay: &vr::Overlay, key: &str) -> anyhow::Result<Self> {
        let fail = |what: &str, code: i32| anyhow::anyhow!("{what}: SteamVR overlay error {code}");
        let mut beams = [0; 2];
        let mut dots = [0; 2];
        for (i, side) in ["left", "right"].into_iter().enumerate() {
            beams[i] = overlay.create(&format!("{key}.laser.{side}"), &format!("Orbiters laser ({side} hand)")).map_err(|c| fail("laser", c))?; // i18n: not shown (a name for SteamVR)
            dots[i] = overlay.create(&format!("{key}.laser-dot.{side}"), &format!("Orbiters laser's dot ({side} hand)")).map_err(|c| fail("laser dot", c))?; // i18n: not shown (a name for SteamVR)
            let _ = overlay.set_width_in_meters(beams[i], BEAM_WIDTH_M);
            let _ = overlay.set_width_in_meters(dots[i], DOT_WIDTH_M);
            for h in [beams[i], dots[i]] {
                let _ = overlay.set_flag(h, overlay_flag::IS_PREMULTIPLIED, true);
                let _ = overlay.set_sort_order(h, SORT_ORDER);
            }
        }
        Ok(Self { beams, dots, shown: [None; 2], painted: false })
    }

    pub fn destroy(&self, overlay: &vr::Overlay) {
        for h in self.beams.into_iter().chain(self.dots) {
            let _ = overlay.destroy(h);
        }
    }

    /// Shows hand `hand`'s laser as `laser` says, or hides it.
    pub(in crate::overlay) fn update(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, hand: usize, laser: Option<Laser>) {
        let Some(laser) = laser else {
            if self.shown[hand].take().is_some() {
                let _ = overlay.hide(self.beams[hand]);
                let _ = overlay.hide(self.dots[hand]);
            }
            return;
        };
        if !self.painted {
            self.painted = true;
            self.paint(overlay, presenter);
        }
        // The beam in its controller's space, from the tip to the end.
        let to_controller = space::inverse(&laser.grip);
        let tip = space::compose(&to_controller, &laser.tip);
        let length = space::length(space::sub(laser.end, space::position(&laser.tip))).max(0.01);
        let beam = space::beam(&tip, length, space::apply(&to_controller, laser.eyes));
        let aspect = beam_aspect(length);
        let was = self.shown[hand];
        // Only what moved enough goes to SteamVR (the last sent is what the next is measured against).
        let send_beam = was.is_none_or(|w| w.device != laser.device || !space::near(&beam, &w.beam, 0.001, 0.0005));
        if send_beam {
            let _ = overlay.set_transform_device_relative(self.beams[hand], laser.device, &beam);
        }
        let send_aspect = was.is_none_or(|w| (w.aspect / aspect - 1.0).abs() > 0.01);
        if send_aspect {
            let _ = overlay.set_texel_aspect(self.beams[hand], aspect);
        }
        if was.is_none_or(|w| w.pressed != laser.pressed) {
            let _ = overlay.set_width_in_meters(self.dots[hand], if laser.pressed { DOT_WIDTH_M * 0.75 } else { DOT_WIDTH_M });
        }
        // The dot flat on the face it ends on, a little in front of it (on the eyes' side).
        let normal = space::axis(&laser.surface, 2);
        let side = if space::dot(space::sub(laser.eyes, laser.end), normal) >= 0.0 { 1.0 } else { -1.0 };
        let mut dot = laser.surface;
        let at = space::add(laser.end, space::scale(normal, 0.002 * side));
        (0..3).for_each(|i| dot.m[i][3] = at[i]);
        let _ = overlay.set_transform_absolute(self.dots[hand], &dot);
        if was.is_none() {
            let _ = overlay.show(self.beams[hand]);
            let _ = overlay.show(self.dots[hand]);
        }
        let (beam, aspect) = match was {
            Some(w) => (if send_beam { beam } else { w.beam }, if send_aspect { aspect } else { w.aspect }),
            None => (beam, aspect),
        };
        self.shown[hand] = Some(Shown { device: laser.device, beam, aspect, pressed: laser.pressed });
    }

    /// The beam's and the dot's pictures, the same for both hands, painted once.
    fn paint(&self, overlay: &vr::Overlay, presenter: &mut Presenter) {
        let mut beam = Canvas::new(BEAM_PX[0], BEAM_PX[1]);
        let pixels = beam.paint(0.0, paint_beam).to_vec();
        let mut dot = Canvas::new(DOT_PX, DOT_PX);
        let dot_pixels = dot.paint(0.0, paint_dot).to_vec();
        for hand in 0..2 {
            presenter.show(overlay, self.beams[hand], &pixels, BEAM_PX[0], BEAM_PX[1]);
            presenter.show(overlay, self.dots[hand], &dot_pixels, DOT_PX, DOT_PX);
        }
    }
}

/// The texel aspect that stretches the beam's picture to `length` metres at its width.
pub fn beam_aspect(length: f32) -> f32 {
    BEAM_PX[1] as f32 * BEAM_WIDTH_M / (BEAM_PX[0] as f32 * length.max(0.01))
}

/// The beam: the accent, brightest at the hand (the picture's bottom) and fainter towards where it points, a soft
/// glow about a bright core.
pub fn paint_beam(p: &egui::Painter) {
    let (w, h) = (BEAM_PX[0] as f32, BEAM_PX[1] as f32);
    let strip = |x0: f32, x1: f32, far: f32, near: f32| {
        let mut mesh = egui::Mesh::default();
        let (top, bottom) = (theme::alpha(theme::ACCENT, far), theme::alpha(theme::ACCENT, near));
        mesh.colored_vertex(pos2(x0, 0.0), top);
        mesh.colored_vertex(pos2(x1, 0.0), top);
        mesh.colored_vertex(pos2(x1, h), bottom);
        mesh.colored_vertex(pos2(x0, h), bottom);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        p.add(egui::Shape::mesh(mesh));
    };
    strip(0.0, w, 0.08, 0.3);
    strip(w * 0.25, w * 0.75, 0.35, 0.95);
}

/// The dot: the accent with a dark rim, seen on light and dark alike.
pub fn paint_dot(p: &egui::Painter) {
    let c = DOT_PX as f32 / 2.0;
    let rect = Rect::from_center_size(pos2(c, c), vec2(DOT_PX as f32, DOT_PX as f32));
    p.circle_filled(rect.center(), c - 6.0, theme::ACCENT);
    p.circle_stroke(rect.center(), c - 4.0, Stroke::new(4.0, Color32::from_black_alpha(150)));
    p.circle_filled(rect.center(), c * 0.32, theme::white(0.85));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_beam_stretches_to_its_length() {
        // The overlay's height is its width × the picture's height / width / the texel aspect.
        for length in [0.05, 0.3, 1.0, 2.5] {
            let height = BEAM_WIDTH_M * BEAM_PX[1] as f32 / BEAM_PX[0] as f32 / beam_aspect(length);
            assert!((height - length).abs() < 1e-5, "{length}: {height}");
        }
        assert!(beam_aspect(0.0).is_finite());
    }

    #[test]
    fn its_pictures_paint() {
        let mut beam = Canvas::new(BEAM_PX[0], BEAM_PX[1]);
        let px = beam.paint(0.0, paint_beam).to_vec();
        let alpha = |x: u32, y: u32| px[((y * BEAM_PX[0] + x) * 4 + 3) as usize];
        assert!(alpha(4, BEAM_PX[1] - 2) > alpha(4, 1), "brighter at the hand");
        assert!(alpha(4, 128) > alpha(0, 128), "a core in a glow");
        let mut dot = Canvas::new(DOT_PX, DOT_PX);
        let px = dot.paint(0.0, paint_dot).to_vec();
        assert_eq!(px[((32 * DOT_PX + 32) * 4 + 3) as usize], 255, "opaque in its middle");
        assert_eq!(px[3], 0, "round: clear in its corner");
    }
}
