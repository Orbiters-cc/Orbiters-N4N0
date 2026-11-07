//! What a recording has covered, in VR while you record: the Capture page's camera path (the path, its cells by how many
//! directions saw them, what to film next) on a small SteamVR overlay above a controller, as a watch shows the time.
//! Shown while a recording runs (not during a 3D picture, whose camera flies on its own) and the setting
//! (`Settings::coverage_overlay`) names a hand.
//!
//! It can never be in what Orbiters records. The frames come from VRChat's camera through Spout (`VRCSender1`): VRChat
//! renders its camera into a texture of its own and shares that texture, in its own process, before it hands its eye
//! pictures to SteamVR. SteamVR's compositor draws overlays afterwards, into the headset's eyes only (and SteamVR's
//! own mirror window); nothing it draws goes back into VRChat's camera or its Spout texture. The capture worker reads
//! nothing else in VR (`SourceSpec`: a Spout sender, a raw capture folder or the test pattern).

use super::{Canvas, Presenter, KEY};
use crate::state::{CaptureState, Snapshot};
use egui::{pos2, vec2, Align2, Id, Rect, Ui};
use orbiters_core::settings::CoverageOverlay;
use orbiters_i18n::{mark, tr};
use orbiters_openvr::{self as vr, overlay_flag, ControllerRole, Matrix34};
use orbiters_ui::theme::{self, Txt};
use orbiters_ui::widgets::{self, Tone};
use orbiters_ui::{format, trail};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The panel's pixels and its width in the play space: 20 cm, its text about 6 mm tall.
pub const PX: [u32; 2] = [600, 450];
pub const WIDTH_M: f32 = 0.2;
/// Pictures a second at most: the coverage changes once a second, the path a little more often.
const FRAME: Duration = Duration::from_millis(100);

/// Whether the panel shows for this snapshot, and on which hand.
pub fn wanted(snap: &Snapshot) -> Option<CoverageOverlay> {
    let recording = matches!(snap.capture.state, CaptureState::Recording | CaptureState::Paused);
    (recording && !snap.capture.photo_3d && snap.settings.coverage_overlay != CoverageOverlay::Off).then_some(snap.settings.coverage_overlay)
}

/// Where the panel sits on a controller (SteamVR's controller space: X right, Y up, Z back towards the wrist): 9 cm
/// towards the wrist and 4 cm up, its face turned 50° up from facing the wrist, towards the eyes of someone looking at
/// the back of their hand.
pub fn on_hand() -> Matrix34 {
    let (s, c) = (-50f32).to_radians().sin_cos();
    Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, c, -s, 0.04], [0.0, s, c, 0.09]] }
}

/// The panel: a dark card with the recording's state and length, the camera path over what the depth frames have
/// seen, and what to film next (or what the colours mean). Painted for the overlay and for the desktop panel alike.
pub fn paint_panel(ui: &mut Ui, rect: Rect, snap: &Snapshot) {
    let p = ui.painter().clone();
    widgets::frame(&p, rect, 26.0, theme::INK_950);
    let c = &snap.capture;
    let pad = 18.0;
    let top = rect.top() + 26.0;
    let (label, tone) = if c.state == CaptureState::Paused { (tr("PAUSED"), Tone::Warn) } else { (tr("REC"), Tone::Rec) };
    widgets::status_dot(&p, pos2(rect.left() + pad + 6.0, top), tone, c.state == CaptureState::Recording);
    let elapsed = c.stats.as_ref().map(|s| format::duration(s.elapsed_s)).unwrap_or_default();
    widgets::text(&p, pos2(rect.left() + pad + 20.0, top), Align2::LEFT_CENTER, &Txt::new(format!("{label}  {elapsed}"), 18.0).color(theme::FG));
    widgets::text(&p, pos2(rect.right() - pad, top), Align2::RIGHT_CENTER, &Txt::caps(tr("Camera path")).color(theme::FG_3));
    let hint_h = 50.0;
    let map = Rect::from_min_max(pos2(rect.left() + pad, rect.top() + 48.0), pos2(rect.right() - pad, rect.bottom() - pad - hint_h));
    let coverage = c.coverage.as_ref().map(|m| trail::Coverage {
        x0: m.x0,
        z0: m.z0,
        cell: m.cell_m,
        width: m.width,
        depth: m.depth,
        cells: &m.cells,
        obstacle: orbiters_core::protocol::coverage::OBSTACLE,
    });
    let empty = tr("Your camera's path appears here");
    trail::show_with(
        ui,
        map,
        Id::unique("coverage-overlay-trail"),
        &trail::Trail {
            points: &c.trail,
            heading: c.stats.as_ref().and_then(|s| s.heading_deg),
            live: c.state == CaptureState::Recording,
            empty_text: empty,
        },
        coverage.as_ref(),
    );
    let hints = Rect::from_min_max(pos2(map.left(), map.bottom() + 8.0), pos2(map.right(), rect.bottom() - pad));
    match c.coverage.as_ref().and_then(|m| m.hints.first()) {
        Some(hint) => {
            p.circle_filled(pos2(hints.left() + 5.0, hints.top() + 12.0), 4.0, theme::WARN);
            let galley = p.layout(hint.clone(), egui::FontId::proportional(16.0), theme::FG, hints.width() - 18.0);
            p.galley(pos2(hints.left() + 16.0, hints.top()), galley, theme::FG);
        }
        None => {
            let y = hints.top() + 14.0;
            let mut x = hints.left();
            widgets::text(&p, pos2(x, y), Align2::LEFT_CENTER, &Txt::new(tr("Seen from"), 15.0).color(theme::FG_3));
            x += 80.0;
            for (directions, label) in [(1u8, mark("1–2 directions")), (3, mark("3–5")), (6, mark("6 or more"))] {
                p.rect_filled(Rect::from_center_size(pos2(x + 6.0, y), vec2(12.0, 12.0)), 3.0, theme::alpha(trail::coverage_colour(directions), 0.7));
                let r = widgets::text(&p, pos2(x + 18.0, y), Align2::LEFT_CENTER, &Txt::new(tr(label), 15.0).color(theme::FG_3));
                x = r.right() + 16.0;
            }
        }
    }
}

/// The panel in SteamVR, on the chosen hand's controller.
pub struct CoverageOverlayPanel {
    handle: vr::OverlayHandle,
    canvas: Canvas,
    /// The controller it is on and for which hand, while shown.
    on: Option<(u32, CoverageOverlay)>,
    shown: Option<Arc<Snapshot>>,
    last_paint: Option<Instant>,
}

impl CoverageOverlayPanel {
    pub fn new(overlay: &vr::Overlay) -> anyhow::Result<Self> {
        let handle =
            overlay.create(&format!("{KEY}.coverage"), "Orbiters coverage").map_err(|c| anyhow::anyhow!("coverage overlay: SteamVR overlay error {c}"))?; // i18n: not shown (a name for SteamVR)
        let _ = overlay.set_width_in_meters(handle, WIDTH_M);
        // Painted premultiplied, as the menu.
        let _ = overlay.set_flag(handle, overlay_flag::IS_PREMULTIPLIED, true);
        Ok(Self { handle, canvas: Canvas::new(PX[0], PX[1]), on: None, shown: None, last_paint: None })
    }

    pub(super) fn update(&mut self, overlay: &vr::Overlay, steamvr: &vr::Client, presenter: &mut Presenter, snap: &Arc<Snapshot>, now: f64) {
        let device = wanted(snap).and_then(|hand| {
            let role = if hand == CoverageOverlay::RightHand { ControllerRole::RightHand } else { ControllerRole::LeftHand };
            steamvr.controller_index(role).map(|d| (d, hand))
        });
        let Some(device) = device else {
            if self.on.take().is_some() {
                let _ = overlay.hide(self.handle);
            }
            return;
        };
        let changed = !self.shown.as_ref().is_some_and(|s| Arc::ptr_eq(s, snap));
        // The path's view springs towards new points: a few more pictures after the last change.
        let settling = self.last_paint.is_some_and(|t| t.elapsed() < Duration::from_secs(2));
        if (changed || settling || self.on.is_none()) && self.last_paint.is_none_or(|t| t.elapsed() >= FRAME) {
            self.last_paint = Some(Instant::now());
            self.shown = Some(snap.clone());
            if self.on.is_none() {
                self.canvas.invalidate();
            }
            orbiters_ui::anim::set_live(self.canvas.ctx(), !snap.settings.reduce_motion);
            let size = vec2(PX[0] as f32, PX[1] as f32);
            self.canvas.paint_ui(now, |ui| paint_panel(ui, Rect::from_min_size(pos2(0.0, 0.0), size), snap));
            if let Some(changed) = self.canvas.damage() {
                presenter.show_part(overlay, self.handle, self.canvas.pixels(), PX[0], PX[1], changed);
            }
        }
        if self.on != Some(device) {
            let _ = overlay.set_transform_device_relative(self.handle, device.0, &on_hand());
            if self.on.is_none() {
                let _ = overlay.show(self.handle);
            }
            self.on = Some(device);
        }
    }

    pub fn destroy(&self, overlay: &vr::Overlay) {
        let _ = overlay.destroy(self.handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_shows_while_recording_but_not_for_a_3d_picture() {
        let mut snap = Snapshot::new(orbiters_core::settings::Settings::default());
        assert_eq!(wanted(&snap), None, "not recording");
        snap.capture.state = CaptureState::Recording;
        assert_eq!(wanted(&snap), Some(CoverageOverlay::LeftHand));
        snap.capture.state = CaptureState::Paused;
        assert_eq!(wanted(&snap), Some(CoverageOverlay::LeftHand));
        snap.capture.photo_3d = true;
        assert_eq!(wanted(&snap), None, "a 3D picture flies the camera on its own");
        snap.capture.photo_3d = false;
        snap.settings.coverage_overlay = CoverageOverlay::Off;
        assert_eq!(wanted(&snap), None);
    }

    #[test]
    fn it_faces_up_from_the_back_of_the_hand() {
        let m = on_hand().m;
        // The panel's face (its +Z) turned up and towards the wrist.
        let face = [m[0][2], m[1][2], m[2][2]];
        assert!(face[1] > 0.7 && face[2] > 0.6, "{face:?}");
        // Its rotation is a rotation.
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        assert!((det - 1.0).abs() < 1e-5);
    }

    /// The panel as it looks, for a person to check: `ORBITERS_COVERAGE_PNG=<file> cargo test -p orbiters-desktop
    /// paints_the_panel -- --ignored`.
    #[test]
    #[ignore]
    fn paints_the_panel() {
        let mut snap = Snapshot::new(orbiters_core::settings::Settings::default());
        snap.capture.state = CaptureState::Recording;
        snap.capture.trail = (0..60).map(|i| [i as f32 * 0.15, 1.5, (i as f32 * 0.2).sin() * 2.0]).collect();
        snap.capture.stats = Some(orbiters_core::protocol::CaptureStats { elapsed_s: 83.0, heading_deg: Some(70.0), ..Default::default() });
        let (width, depth) = (40u32, 30u32);
        let mut cells = vec![0u8; (width * depth * 2) as usize];
        for z in 8..22 {
            for x in 2..36 {
                let i = ((z * width + x) * 2) as usize;
                cells[i] = ((x + z) % 9) as u8;
            }
        }
        snap.capture.coverage = Some(orbiters_core::protocol::CoverageMap {
            cell_m: 0.25,
            x0: -1.0,
            z0: -3.75,
            width,
            depth,
            cells,
            floor_y: Some(0.0),
            hints: vec!["What is ahead of you (about 9 m away) was only seen from one direction".into()],
        });
        let mut canvas = Canvas::new(PX[0], PX[1]);
        let size = vec2(PX[0] as f32, PX[1] as f32);
        for k in 0..30 {
            canvas.paint_ui(k as f64 * 0.1, |ui| paint_panel(ui, Rect::from_min_size(pos2(0.0, 0.0), size), &snap));
        }
        let path = std::env::var("ORBITERS_COVERAGE_PNG").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("coverage-panel.png"));
        super::super::menu_pages::write_png(&path, PX[0], PX[1], canvas.pixels()).unwrap();
        println!("{}", path.display());
    }
}
