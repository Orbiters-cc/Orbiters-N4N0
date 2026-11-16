//! The menu mismatch strip (Settings › Beta feedback, [`crate::mismatch`]): a band of three buttons on the watch's wrist, along the forearm
//! beside the watch, "Menu should be off", "Wheel should be on", "Quick menu should be on". It is there whenever the switch is on, whatever
//! N4N0's menus or VRChat's do, since the moments it is for are the ones where those are wrong; it rides its controller as the watch does,
//! is pressed as the watch is (the other hand's laser on it, or its fingertip, and a pull of its trigger), and says "Noted" on the row
//! pressed for a moment. A press reports; it changes nothing of the menus.
//!
//! Why the wrist and not "a corner of the screen" as on the desktop: a headset has no corner that stays out of the way, a band fixed in
//! the view would be over the world all the time and in the way of VRChat's own menus, which open in front; the wrist is always one glance
//! away, is reached by the other hand in the same gesture as the watch, and needs nothing of N4N0's menus (they are what may be wrong).

use super::space::{self, Quad};
use crate::mismatch::Kind;
use crate::overlay::{Canvas, Presenter};
use egui::{pos2, vec2, Align2, Rect};
use orbiters_openvr::{self as vr, overlay_flag, Matrix34};
use orbiters_ui::theme::{self, weight, Txt};
use std::time::{Duration, Instant};

/// Its size in the play space, and its picture's.
pub const WIDTH_M: f32 = 0.11;
pub const PX: [u32; 2] = [440, 300];
/// How far its middle lies from the watch's, along the forearm, in metres.
pub const BESIDE_M: f32 = 0.085;
/// Over the panel and its bar, under the lasers, as the watch.
const SORT_ORDER: u32 = 12;
/// How long a row says "Noted" after its press.
pub const NOTED: Duration = Duration::from_millis(1600);
/// A fingertip this near it is on it.
pub const TOUCH_M: f32 = 0.03;

pub fn height_m() -> f32 {
    WIDTH_M * PX[1] as f32 / PX[0] as f32
}

/// Where it lies on the controller of the `right` hand or the left: in the watch's plane, along the forearm.
pub fn on_wrist(right: bool) -> Matrix34 {
    let watch = space::on_wrist(right);
    // The watch's +X runs toward the elbow on the left wrist and toward the hand on the right one.
    let along = if right { -BESIDE_M } else { BESIDE_M };
    let p = space::apply(&watch, [along, 0.0, 0.0]);
    space::from_axes(space::axis(&watch, 0), space::axis(&watch, 1), space::axis(&watch, 2), p)
}

/// The row at `v` (0 at its top, 1 at its bottom), as [`paint`] lays them.
pub fn row_at(v: f32) -> Option<Kind> {
    let rows = Kind::ALL.len() as f32;
    let (top, bottom) = (PAD / PX[1] as f32, 1.0 - PAD / PX[1] as f32);
    if !(top..bottom).contains(&v) {
        return None;
    }
    let i = ((v - top) / (bottom - top) * rows).floor() as usize;
    Kind::ALL.get(i).copied()
}

const PAD: f32 = 10.0;

pub struct Strip {
    handle: vr::OverlayHandle,
    canvas: Canvas,
    on: Option<(u32, bool)>,
    visible: bool,
    /// What its picture shows: the row under a laser, the row that says Noted, the language.
    drawn: Option<(Option<Kind>, Option<Kind>, orbiters_i18n::Language)>,
    noted: Option<(Kind, Instant)>,
}

impl Strip {
    pub fn new(overlay: &vr::Overlay, key: &str) -> anyhow::Result<Self> {
        let handle = crate::overlay::slots::create(overlay, &format!("{key}.mismatch"), "Orbiters menu mismatch").map_err(|c| anyhow::anyhow!("mismatch strip: SteamVR overlay error {c}"))?; // i18n: not shown (a name for SteamVR)
        let _ = overlay.set_width_in_meters(handle, WIDTH_M);
        let _ = overlay.set_flag(handle, overlay_flag::IS_PREMULTIPLIED, true);
        let _ = overlay.set_sort_order(handle, SORT_ORDER);
        Ok(Self { handle, canvas: Canvas::new(PX[0], PX[1]), on: None, visible: false, drawn: None, noted: None })
    }

    pub fn destroy(&self, overlay: &vr::Overlay) {
        let _ = overlay.destroy(self.handle);
    }

    /// Where it is this turn: on controller `device` of the `right` hand or the left, whose pose is `grip`; `None` hides it. Returns it as
    /// a target.
    pub fn place(&mut self, overlay: &vr::Overlay, at: Option<(u32, bool, Matrix34)>) -> Option<Quad> {
        let Some((device, right, grip)) = at else {
            self.on = None;
            if self.visible {
                let _ = overlay.hide(self.handle);
                self.visible = false;
            }
            return None;
        };
        if self.on != Some((device, right)) {
            let _ = overlay.set_transform_device_relative(self.handle, device, &on_wrist(right));
            self.on = Some((device, right));
        }
        if !self.visible {
            let _ = overlay.set_alpha(self.handle, 0.94);
            let _ = overlay.show(self.handle);
            self.visible = true;
        }
        Some(Quad { transform: space::compose(&grip, &on_wrist(right)), width: WIDTH_M, height: height_m() })
    }

    /// A row was pressed: it says Noted for a moment.
    pub fn pressed(&mut self, kind: Kind, now: Instant) {
        self.noted = Some((kind, now));
    }

    /// Its picture for the row under a laser (`hover`), painted when what it shows changed.
    pub(in crate::overlay) fn paint(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, hover: Option<Kind>, now: Instant) {
        if !self.visible {
            return;
        }
        let noted = self.noted.filter(|(_, at)| now.duration_since(*at) < NOTED).map(|(k, _)| k);
        let look = (hover, noted, orbiters_i18n::language());
        if self.drawn == Some(look) {
            return;
        }
        for _ in 0..if self.drawn.is_none() { 2 } else { 1 } {
            self.canvas.paint(0.0, |p| paint(p, hover, noted));
        }
        let changed = if self.drawn.is_none() { Some([0, 0, PX[0], PX[1]]) } else { self.canvas.damage() };
        if let Some(changed) = changed {
            presenter.show_part(overlay, self.handle, self.canvas.pixels(), PX[0], PX[1], changed);
        }
        self.drawn = Some(look);
    }
}

/// The strip's face: a row a kind, the hovered one lighter, the one just pressed saying Noted with a tick.
pub fn paint(p: &egui::Painter, hover: Option<Kind>, noted: Option<Kind>) {
    let all = Rect::from_min_size(pos2(0.0, 0.0), vec2(PX[0] as f32, PX[1] as f32));
    let rows = Kind::ALL.len() as f32;
    let h = (all.height() - 2.0 * PAD) / rows;
    for (i, kind) in Kind::ALL.into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(PAD, PAD + i as f32 * h), vec2(all.width() - 2.0 * PAD, h)).shrink2(vec2(0.0, 5.0));
        let is_noted = noted == Some(kind);
        let fill = if is_noted {
            theme::mix(theme::INK_950, theme::ACCENT, 0.35)
        } else if hover == Some(kind) {
            theme::mix(theme::INK_950, theme::FG, 0.18)
        } else {
            theme::alpha(theme::INK_950, 0.92)
        };
        p.rect_filled(r, r.height() / 2.0, fill);
        p.rect_stroke(r, r.height() / 2.0, egui::Stroke::new(3.0, if is_noted { theme::ACCENT } else { theme::white(0.16) }), egui::StrokeKind::Inside);
        let (words, colour) = if is_noted { (orbiters_i18n::tr(crate::mismatch::NOTED_WORDS), theme::ACCENT) } else { (orbiters_i18n::tr(kind.words()), theme::FG) };
        // A mark of what it says: a struck circle for "off", a filled one for "on".
        let dot = pos2(r.left() + r.height() / 2.0 + 4.0, r.center().y);
        if is_noted {
            p.line_segment([dot + vec2(-11.0, 0.0), dot + vec2(-3.0, 9.0)], egui::Stroke::new(5.0, colour));
            p.line_segment([dot + vec2(-3.0, 9.0), dot + vec2(13.0, -10.0)], egui::Stroke::new(5.0, colour));
        } else if kind == Kind::MenuShouldBeOff {
            p.circle_stroke(dot, 13.0, egui::Stroke::new(4.0, theme::FG_3));
            p.line_segment([dot + vec2(-9.0, 9.0), dot + vec2(9.0, -9.0)], egui::Stroke::new(4.0, theme::FG_3));
        } else {
            p.circle_filled(dot, 13.0, theme::ACCENT);
        }
        let text = Txt::new(words, 30.0).weight(weight::SEMIBOLD).color(colour);
        orbiters_ui::widgets::text_truncated(p, pos2(dot.x + 26.0, r.center().y), Align2::LEFT_CENTER, &text, r.right() - dot.x - 40.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_rows_are_found_where_they_are_painted() {
        let rows: Vec<Option<Kind>> = [0.01, 0.1, 0.3, 0.5, 0.7, 0.9, 0.99].into_iter().map(row_at).collect();
        assert_eq!(rows, [None, Some(Kind::MenuShouldBeOff), Some(Kind::MenuShouldBeOff), Some(Kind::WheelShouldBeOn), Some(Kind::QuickMenuShouldBeOn), Some(Kind::QuickMenuShouldBeOn), None]);
        let mut canvas = Canvas::new(PX[0], PX[1]);
        for _ in 0..2 {
            canvas.paint(0.0, |p| paint(p, None, None));
        }
        let px = canvas.pixels().to_vec();
        let alpha = |x: u32, y: u32| px[((y * PX[0] + x) * 4 + 3) as usize];
        for (i, _) in Kind::ALL.iter().enumerate() {
            let v = (PAD + (i as f32 + 0.5) * (PX[1] as f32 - 2.0 * PAD) / 3.0) / PX[1] as f32;
            assert_eq!(row_at(v), Some(Kind::ALL[i]));
            assert!(alpha(PX[0] / 2, (v * PX[1] as f32) as u32) > 200, "row {i} painted at its middle");
        }
        assert_eq!(alpha(2, 2), 0, "clear in its corner");
    }

    #[test]
    fn it_lies_beside_the_watch_never_on_it() {
        for right in [false, true] {
            let watch = space::position(&space::on_wrist(right));
            let strip = space::position(&on_wrist(right));
            let gap = space::length(space::sub(strip, watch));
            assert!(gap > super::super::watch::WIDTH_M / 2.0 + height_m().min(WIDTH_M) / 2.0 - 0.01, "{gap}");
            // Toward the elbow, as the forearm runs: behind the watch along the controller's +Z (the controller points along −Z).
            assert!(strip[2] > watch[2], "right {right}: toward the elbow {strip:?} {watch:?}");
        }
    }
}
