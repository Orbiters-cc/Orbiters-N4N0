//! The menu mismatch strip (Settings › Beta feedback, [`crate::mismatch`]): a band of three buttons on the watch's wrist, just over the
//! watch as the eyes see it, facing them and its words right way up whatever way the wrist is turned ([`space::over_watch`]), "Menu should be off", "Wheel should be on", "Quick menu should be on". It is there whenever the switch is on, whatever
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

/// Its size in the play space, and its picture's: rows 3 cm tall, as large as the watch, for a laser or a fingertip at arm's length.
pub const WIDTH_M: f32 = 0.18;
pub const PX: [u32; 2] = [560, 300];
/// How far it stands from the watch's disc, and before the wrist towards the eyes, in metres.
pub const GAP_M: f32 = 0.015;
pub const LIFT_M: f32 = 0.03;
/// Moved less than this (metres, and 1 − cosine of a turn) since SteamVR was last told, it is not told again.
const STILL_M: f32 = 0.002;
const STILL_TURN: f32 = 0.0004;
/// Over the panel and its bar, under the lasers, as the watch.
const SORT_ORDER: u32 = 12;
/// How long a row says "Noted" after its press.
pub const NOTED: Duration = Duration::from_millis(1600);
/// A fingertip this near it is on it.
pub const TOUCH_M: f32 = 0.03;

pub fn height_m() -> f32 {
    WIDTH_M * PX[1] as f32 / PX[0] as f32
}

/// Where it lies on the controller of the `right` hand or the left whose pose is `grip`, the eyes at `head`: just over the watch as they
/// see it, facing them, its top their up (in the controller's space).
pub fn on_wrist(right: bool, grip: &Matrix34, head: Option<&Matrix34>) -> Matrix34 {
    let (eyes, up) = head.map_or_else(|| space::eyes_before_watch(right), |h| space::eyes_in(grip, h));
    space::over_watch(right, eyes, up, height_m(), GAP_M, LIFT_M)
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
    /// The controller it is on, and where on it SteamVR was last told.
    on: Option<(u32, Matrix34)>,
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

    /// Where it is this turn: on controller `device` of the `right` hand or the left, whose pose is `grip`, turned to the eyes at `head`;
    /// `None` hides it. It rides the controller (SteamVR moves it with the hand at the headset's rate) and turns to the eyes each turn.
    /// Returns it as a target.
    pub fn place(&mut self, overlay: &vr::Overlay, at: Option<(u32, bool, Matrix34)>, head: Option<&Matrix34>) -> Option<Quad> {
        let Some((device, right, grip)) = at else {
            self.on = None;
            if self.visible {
                let _ = overlay.hide(self.handle);
                self.visible = false;
            }
            return None;
        };
        let relative = on_wrist(right, &grip, head);
        if !self.on.is_some_and(|(d, told)| d == device && space::near(&told, &relative, STILL_M, STILL_TURN)) {
            let _ = overlay.set_transform_device_relative(self.handle, device, &relative);
            self.on = Some((device, relative));
        }
        if !self.visible {
            let _ = overlay.set_alpha(self.handle, 0.94);
            let _ = overlay.show(self.handle);
            self.visible = true;
        }
        Some(Quad { transform: space::compose(&grip, &relative), width: WIDTH_M, height: height_m() })
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
        // Every language's words whole: the size comes down until they fit (pt-BR and French are the longest).
        let room = r.right() - dot.x - 44.0;
        let size = crate::mismatch::fitting(p, words, &[30.0, 28.0, 26.0, 24.0, 22.0], room);
        let text = Txt::new(words, size).weight(weight::SEMIBOLD).color(colour);
        orbiters_ui::widgets::text_truncated(p, pos2(dot.x + 26.0, r.center().y), Align2::LEFT_CENTER, &text, room + 4.0);
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

    use space::{add, axis, cross, dot, from_axes, position, scale, sub, unit, V3};

    /// A controller of the `right` hand or the left at `at`, pointing `forward` (its −Z), its palm towards `palm` (a left hand's palm is
    /// the controller's +X, a right hand's its −X; the thumb is its +Y).
    fn controller(at: V3, forward: V3, palm: V3, right: bool) -> Matrix34 {
        let z = unit(scale(forward, -1.0)).unwrap();
        let px = if right { scale(palm, -1.0) } else { palm };
        let x = unit(sub(px, scale(z, dot(px, z)))).unwrap();
        from_axes(x, cross(z, x), z, at)
    }

    /// Eyes at `eye` looking at `target`, the head level (its −Z towards the target).
    fn looking(eye: V3, target: V3) -> Matrix34 {
        let z = unit(sub(eye, target)).unwrap();
        let x = unit(cross([0.0, 1.0, 0.0], z)).unwrap();
        from_axes(x, cross(z, x), z, eye)
    }

    const EYE: V3 = [0.0, 1.6, 0.0];

    /// Enzo's pose in his pictures B and C (2026-11-16, Quest Pro): the left hand ahead, a little to the right, palm down, the forearm
    /// rising from the chest towards the hand at the upper right, the eyes on the watch.
    fn his_pose_c() -> (Matrix34, Matrix34) {
        let grip = controller([0.10, 1.12, -0.40], [0.55, 0.1, -0.83], [0.0, -1.0, 0.0], false);
        let head = looking(EYE, position(&space::compose(&grip, &space::on_wrist(false))));
        (grip, head)
    }

    /// Picture B: the forearm more across the body, the hand to the right.
    fn his_pose_b() -> (Matrix34, Matrix34) {
        let grip = controller([0.12, 1.15, -0.36], [0.9, 0.05, -0.42], [0.05, -1.0, 0.1], false);
        let head = looking(EYE, position(&space::compose(&grip, &space::on_wrist(false))));
        (grip, head)
    }

    /// The previous stream's sandbox pose: the arm ahead, thumb up, seen from behind.
    fn sandbox_pose() -> (Matrix34, Matrix34) {
        let grip = controller([-0.12, 1.2, -0.45], [0.05, 0.15, -1.0], [1.0, 0.0, 0.0], false);
        let head = looking(EYE, [0.0, 1.3, -0.6]);
        (grip, head)
    }

    /// Where the band was until 0.3.6-dev.10: in the watch's plane, its top the thumb's side, 10.5 cm along the forearm, 14 cm wide.
    fn before(right: bool) -> (Matrix34, f32) {
        let watch = space::on_wrist(right);
        (space::offset(&watch, [if right { -0.105 } else { 0.105 }, 0.0, 0.0]), 0.14)
    }

    /// How a quad reads from the eyes: its picture's right and top along the eyes' right and up (1 is right way up, −1 upside down).
    fn reading(quad: &Matrix34, head: &Matrix34) -> (f32, f32) {
        (dot(axis(quad, 0), axis(head, 0)), dot(axis(quad, 1), axis(head, 1)))
    }

    /// The cause, in his pose: the band (and the watch) took their top from the controller's +Y, the thumb's side. Palm down the thumb
    /// points at the chest, so both read upside down from his eyes, as in his pictures; the sandbox's arm was held thumb up, where the same
    /// rule reads right way up. Now both turn to the eyes.
    #[test]
    fn in_his_pose_it_was_upside_down_and_now_reads_right_way_up() {
        for (name, (grip, head)) in [("C", his_pose_c()), ("B", his_pose_b())] {
            let (old, _) = before(false);
            let (right, up) = reading(&space::compose(&grip, &old), &head);
            assert!(right < -0.5 && up < -0.3, "{name}: the old band read upside down ({right}, {up})");
            let (right, up) = reading(&space::compose(&grip, &space::on_wrist(false)), &head);
            assert!(right < -0.3 && up < -0.3, "{name}: the old watch too ({right}, {up})");
            let band = space::compose(&grip, &on_wrist(false, &grip, Some(&head)));
            let (right, up) = reading(&band, &head);
            assert!(right > 0.95 && up > 0.9, "{name}: the band right way up ({right}, {up})");
            let watch = space::compose(&grip, &space::watch_upright(false, space::eyes_in(&grip, &head).1));
            let (_, up) = reading(&watch, &head);
            assert!(up > 0.5, "{name}: the watch's top up ({up})");
            assert!((dot(axis(&watch, 2), axis(&space::compose(&grip, &space::on_wrist(false)), 2)) - 1.0).abs() < 1e-4, "the watch's face stays where it looks");
        }
        let (grip, head) = sandbox_pose();
        let (right, up) = reading(&space::compose(&grip, &before(false).0), &head);
        assert!(right > 0.3 && up > 0.3, "the sandbox's thumb-up arm read the old band right way up ({right}, {up})");
    }

    /// Whatever way either wrist is turned (palm down, in, up, towards the eyes; the arm ahead, across, raised), the band faces the eyes,
    /// reads right way up, is large enough for a laser and lies clear of the watch as the eyes see it.
    #[test]
    fn it_reads_right_way_up_and_stays_clear_of_the_watch_however_the_wrist_turns() {
        let mut cases = 0;
        for right in [false, true] {
            let side = if right { 1.0 } else { -1.0 };
            for forward in [[0.0, 0.0, -1.0], [-side * 0.8, 0.0, -0.6], [side * 0.3, 0.6, -0.7], [-side, 0.1, -0.1]] {
                for palm in [[0.0, -1.0, 0.0], [-side, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [side, 0.0, 0.0]] {
                    let at = [side * 0.15, 1.15, -0.38];
                    let Some(z) = unit(scale(forward, -1.0)) else { continue };
                    if unit(sub(palm, scale(z, dot(palm, z)))).is_none() {
                        continue;
                    }
                    let grip = controller(at, forward, palm, right);
                    let watch = space::compose(&grip, &space::on_wrist(right));
                    let head = looking(EYE, position(&watch));
                    let band = space::compose(&grip, &on_wrist(right, &grip, Some(&head)));
                    let what = format!("right {right} forward {forward:?} palm {palm:?}");
                    let to_eyes = unit(sub(EYE, position(&band))).unwrap();
                    assert!(dot(axis(&band, 2), to_eyes) > 0.999, "{what}: faces the eyes");
                    let (r, u) = reading(&band, &head);
                    assert!(r > 0.95 && u > 0.9, "{what}: right way up ({r}, {u})");
                    let quad = Quad { transform: band, width: WIDTH_M, height: height_m() };
                    for k in 0..24 {
                        let a = k as f32 / 24.0 * std::f32::consts::TAU;
                        let rim = space::apply(&watch, [a.cos() * super::super::watch::WIDTH_M / 2.0, a.sin() * super::super::watch::WIDTH_M / 2.0, 0.0]);
                        let d = unit(sub(rim, EYE)).unwrap();
                        assert!(quad.hit(EYE, d, 0.005).is_none(), "{what}: the watch's rim {k} is seen clear of the band");
                    }
                    // Each row at least 2.5 cm tall and the band within reach of the other hand: 12 cm from the watch at most.
                    assert!((height_m() - 2.0 * PAD / PX[1] as f32 * height_m()) / 3.0 > 0.025);
                    assert!(space::length(sub(position(&band), position(&watch))) < 0.12);
                    cases += 1;
                }
            }
        }
        assert!(cases >= 30, "{cases}");
        // No head pose from SteamVR: still over the watch, facing out of its face.
        let grip = controller([0.0, 1.1, -0.3], [0.0, 0.0, -1.0], [0.0, -1.0, 0.0], false);
        let band = space::compose(&grip, &on_wrist(false, &grip, None));
        assert!(position(&band).iter().all(|v| v.is_finite()));
    }

    /// Renders his poses B and C and the sandbox's before and after (`ORBITERS_WRIST_RENDER=<folder>`; run with `--ignored`): the forearm
    /// and hand as grey capsules, the watch's and the band's real pictures where SteamVR would draw them (over the scene).
    #[test]
    #[ignore]
    fn render_his_poses_before_and_after() {
        let Some(dir) = std::env::var_os("ORBITERS_WRIST_RENDER").map(std::path::PathBuf::from) else { return };
        std::fs::create_dir_all(&dir).unwrap();
        let picture = |w: u32, h: u32, paint_it: &dyn Fn(&egui::Painter)| {
            let mut canvas = Canvas::new(w, h);
            for _ in 0..2 {
                canvas.paint(0.0, |p| paint_it(p));
            }
            canvas.pixels().to_vec()
        };
        let watch_px = picture(super::super::watch::PX, super::super::watch::PX, &|p| super::super::watch::paint(p, false, false));
        let band_px = picture(PX[0], PX[1], &|p| paint(p, None, None));
        for (name, (grip, head)) in [("c", his_pose_c()), ("b", his_pose_b()), ("sandbox", sandbox_pose())] {
            for after in [false, true] {
                let (watch_rel, band_rel, band_w) = if after {
                    (space::watch_upright(false, space::eyes_in(&grip, &head).1), on_wrist(false, &grip, Some(&head)), WIDTH_M)
                } else {
                    let (b, w) = before(false);
                    (space::on_wrist(false), b, w)
                };
                let quads = [
                    (Quad { transform: space::compose(&grip, &watch_rel), width: super::super::watch::WIDTH_M, height: super::super::watch::WIDTH_M }, &watch_px, [super::super::watch::PX, super::super::watch::PX]),
                    (Quad { transform: space::compose(&grip, &band_rel), width: band_w, height: band_w * PX[1] as f32 / PX[0] as f32 }, &band_px, PX),
                ];
                let limb = |a: V3, b: V3| (space::apply(&grip, a), space::apply(&grip, b));
                let capsules = [(limb([0.0, -0.025, 0.12], [0.0, -0.05, 0.42]), 0.04), (limb([0.0, 0.0, 0.10], [0.0, 0.0, -0.06]), 0.045)];
                let rgba = render(&head, &quads, &capsules, [960, 540]);
                let file = dir.join(format!("{name}-{}.png", if after { "after" } else { "before" }));
                crate::overlay::menu_pages::write_png(&file, 960, 540, &rgba).unwrap();
            }
        }
    }

    /// A picture from `head` (100° across): the capsules lit grey, then the quads' pictures over them.
    fn render(head: &Matrix34, quads: &[(Quad, &Vec<u8>, [u32; 2])], capsules: &[((V3, V3), f32)], size: [u32; 2]) -> Vec<u8> {
        let (w, h) = (size[0] as usize, size[1] as usize);
        let tan = (50.0f32).to_radians().tan();
        let eye = position(head);
        let mut out = vec![0u8; w * h * 4];
        for py in 0..h {
            for px in 0..w {
                let u = (px as f32 + 0.5) / w as f32 * 2.0 - 1.0;
                let v = 1.0 - (py as f32 + 0.5) / h as f32 * 2.0;
                let d = unit(add(add(scale(axis(head, 2), -1.0), scale(axis(head, 0), u * tan)), scale(axis(head, 1), v * tan * h as f32 / w as f32))).unwrap();
                let mut c = [28.0f32, 20.0, 34.0];
                // The arm: the nearest capsule the ray meets (marched).
                let mut t = 0.05;
                while t < 2.0 {
                    let p = add(eye, scale(d, t));
                    let near = capsules
                        .iter()
                        .map(|((a, b), r)| {
                            let ab = sub(*b, *a);
                            let k = (dot(sub(p, *a), ab) / dot(ab, ab)).clamp(0.0, 1.0);
                            let q = add(*a, scale(ab, k));
                            (space::length(sub(p, q)) - r, q)
                        })
                        .fold((f32::MAX, [0.0; 3]), |m, x| if x.0 < m.0 { x } else { m });
                    if near.0 < 0.001 {
                        let n = unit(sub(p, near.1)).unwrap_or([0.0, 1.0, 0.0]);
                        let light = 0.35 + 0.65 * dot(n, unit([0.3, 1.0, 0.4]).unwrap()).max(0.0);
                        c = [70.0 * light + 20.0, 66.0 * light + 18.0, 72.0 * light + 22.0];
                        break;
                    }
                    t += near.0.max(0.0005);
                }
                // Overlays over the scene, the band over the watch as SteamVR sorts them alike, nearest last.
                for (quad, pixels, [qw, qh]) in quads {
                    if let Some(hit) = quad.hit(eye, d, 0.0) {
                        let x = ((hit.uv[0] * *qw as f32) as u32).min(qw - 1);
                        let y = ((hit.uv[1] * *qh as f32) as u32).min(qh - 1);
                        let i = ((y * qw + x) * 4) as usize;
                        let a = pixels[i + 3] as f32 / 255.0 * 0.94;
                        for k in 0..3 {
                            c[k] = pixels[i + k] as f32 * 0.94 + c[k] * (1.0 - a);
                        }
                    }
                }
                let o = (py * w + px) * 4;
                out[o..o + 4].copy_from_slice(&[c[0].min(255.0) as u8, c[1].min(255.0) as u8, c[2].min(255.0) as u8, 255]);
            }
        }
        out
    }
}
