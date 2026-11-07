//! What says how to handle a capture standing small, beside it in the room (Enzo: "a little joystick 3D animation and text
//! showing it pushed up / down"): first to take it, then to zoom it. Once a stick zoomed it, it goes until a new capture opens
//! from a new opening of the menu ([`super::Worlds`]).

use super::super::{Canvas, Presenter};
use super::stereo::{self, V3};
use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::{pos2, vec2, Align2, Color32, Painter, Pos2, Rect, Shape};
use orbiters_openvr as vr;
use orbiters_ui::theme::{self, weight};
use super::super::orbit::panels::look as orbit_look;
use orbiters_ui::{icons, widgets};
use std::time::Instant;

pub const PX: [u32; 2] = [448, 256];
pub const WIDTH_M: f32 = 0.28;
/// How high above what it speaks of it floats.
const ABOVE_M: f32 = 0.06;
const FRAME_S: f32 = 1.0 / 45.0;
/// One push up and pull down, in seconds.
const CYCLE_S: f32 = 2.6;

/// What it says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Grip to take it; `near`: a hand is near enough.
    Grip { near: bool },
    /// That hand's stick, up and down: 0 the left, 1 the right.
    Stick { hand: usize },
}

/// How far the stick is pushed `seconds` in: up positive, resting a moment at each end and in the middle.
fn stick_at(seconds: f32) -> f32 {
    let phase = (seconds / CYCLE_S).fract() * std::f32::consts::TAU;
    (phase.sin() * 1.7).clamp(-1.0, 1.0)
}

/// A filled quad of two colours along its width.
fn shaded_quad(p: &Painter, corners: [Pos2; 4], left: Color32, middle: Color32, right: Color32) {
    let [a, b, c, d] = corners;
    let mid_top = a + (b - a) * 0.42;
    let mid_foot = d + (c - d) * 0.42;
    let mut mesh = Mesh::default();
    for (at, color) in [(a, left), (mid_top, middle), (b, right), (c, right), (mid_foot, middle), (d, left)] {
        mesh.vertices.push(Vertex { pos: at, uv: WHITE_UV, color });
    }
    mesh.indices.extend_from_slice(&[0, 1, 4, 0, 4, 5, 1, 2, 3, 1, 3, 4]);
    p.add(Shape::mesh(mesh));
}

fn ellipse(p: &Painter, center: Pos2, radius: egui::Vec2, color: Color32) {
    p.add(Shape::ellipse_filled(center, radius, color));
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// A thumbstick seen from a little above, tilted away from who looks by `push` (1) or towards them (-1).
pub fn joystick(p: &Painter, base: Pos2, push: f32) {
    const HEIGHT: f32 = 92.0;
    let elevation = 26f32.to_radians();
    let tilt = push * 46f32.to_radians();
    // The top of its stem, projected: away goes up the picture, and smaller; towards comes down, and larger.
    let top = base - vec2(0.0, HEIGHT * (tilt.cos() * elevation.cos() + tilt.sin() * elevation.sin()));
    let near = 1.0 - 0.14 * push;
    // Its housing: a shadow, a rim catching the light, a dark well.
    ellipse(p, base + vec2(0.0, 12.0), vec2(82.0, 30.0), Color32::from_black_alpha(90));
    ellipse(p, base + vec2(0.0, 6.0), vec2(78.0, 28.0), Color32::from_rgb(20, 23, 30));
    ellipse(p, base, vec2(78.0, 28.0), Color32::from_rgb(112, 118, 134));
    ellipse(p, base + vec2(0.0, 1.5), vec2(72.0, 25.0), Color32::from_rgb(70, 75, 88));
    ellipse(p, base + vec2(0.0, 3.0), vec2(60.0, 20.0), Color32::from_rgb(46, 50, 60));
    // Its rubber boot, leaning with the stem.
    let lean = (top.y - (base.y - HEIGHT * elevation.cos())) * 0.1;
    for i in 0..6 {
        let t = i as f32 / 5.0;
        let c = mix(Color32::from_rgb(64, 69, 82), Color32::from_rgb(104, 110, 124), t);
        ellipse(p, base + vec2(0.0, -2.0 - t * 15.0 + lean * t), vec2(38.0 - t * 18.0, 14.0 - t * 7.0), c);
    }
    // The stem, lit from the left.
    let (wide, narrow) = (11.0, 9.0 * near);
    let foot = base - vec2(0.0, 12.0);
    shaded_quad(
        p,
        [top - vec2(narrow, -4.0), top + vec2(narrow, 4.0), foot + vec2(wide, 0.0), foot - vec2(wide, 0.0)],
        Color32::from_rgb(150, 158, 174),
        Color32::from_rgb(200, 206, 218),
        Color32::from_rgb(92, 98, 112),
    );
    // Its cap: a side, then a face rounded by rings of light towards its upper left.
    let (rx, ry) = (38.0 * near, 22.0 * near * (0.86 + 0.14 * push));
    let side = 11.0 * near;
    ellipse(p, top + vec2(0.0, side), vec2(rx, ry), Color32::from_rgb(96, 102, 116));
    for i in 0..6 {
        let t = i as f32 / 5.0;
        ellipse(p, top + vec2(0.0, side * (1.0 - t)), vec2(rx, ry), mix(Color32::from_rgb(110, 116, 130), Color32::from_rgb(150, 156, 170), t));
    }
    for i in 0..10 {
        let t = i as f32 / 9.0;
        let color = mix(Color32::from_rgb(176, 182, 196), Color32::from_rgb(244, 246, 250), t * t);
        ellipse(p, top + vec2(-5.0 * near * t, -2.5 * near * t), vec2(rx * (1.0 - t * 0.62), ry * (1.0 - t * 0.62)), color);
    }
    // Its dished top's grip.
    ellipse(p, top + vec2(0.0, 1.0), vec2(rx * 0.42, ry * 0.42), Color32::from_rgba_unmultiplied(70, 76, 90, 70));
}

fn words(p: &Painter, at: Pos2, glyph: &str, first: &str, second: &str, on: f32) {
    let lit = mix(theme::FG_3, orbit_look::accent(), on);
    let text = mix(theme::FG_2, Color32::WHITE, on);
    let row = Rect::from_min_size(at - vec2(8.0, 10.0), vec2(226.0, 84.0));
    if on > 0.01 {
        p.rect_filled(row, theme::radius(18.0), theme::alpha(orbit_look::accent(), 0.18 * on));
    }
    widgets::icon(p, at + vec2(16.0, 32.0), glyph, 30.0, lit);
    p.text(at + vec2(42.0, 18.0), Align2::LEFT_CENTER, first, theme::font(30.0, weight::BOLD), text);
    p.text(at + vec2(42.0, 49.0), Align2::LEFT_CENTER, second, theme::font(22.0, weight::SEMIBOLD), mix(theme::FG_2, Color32::WHITE, on));
}

/// A tail from the card's foot towards what it speaks of.
fn tail(p: &Painter, card: Rect) {
    let middle = card.center_bottom();
    p.add(Shape::convex_polygon(vec![middle + vec2(-18.0, -1.0), middle + vec2(18.0, -1.0), middle + vec2(0.0, 18.0)], theme::alpha(theme::INK_900, 0.95), egui::Stroke::NONE));
}

/// The hint on a page of [`PX`], `seconds` into its animation.
pub fn paint(p: &Painter, stage: Stage, seconds: f32) {
    let page = Rect::from_min_size(Pos2::ZERO, vec2(PX[0] as f32, PX[1] as f32));
    match stage {
        Stage::Stick { hand } => {
            let card = Rect::from_min_max(page.min + vec2(2.0, 2.0), page.max - vec2(2.0, 20.0));
            p.rect_filled(card, theme::radius(30.0), theme::alpha(theme::INK_900, 0.95));
            tail(p, card);
            let push = stick_at(seconds);
            joystick(p, pos2(104.0, 186.0), push);
            let which = if hand == 0 { "Left stick" } else { "Right stick" };
            p.text(pos2(104.0, 28.0), Align2::CENTER_CENTER, which, theme::font(19.0, weight::SEMIBOLD), theme::FG_2);
            let up = ((push - 0.2) / 0.8).clamp(0.0, 1.0);
            let down = ((-push - 0.2) / 0.8).clamp(0.0, 1.0);
            words(p, pos2(208.0, 26.0), icons::CARET_UP, "Push up", "to zoom in", up);
            words(p, pos2(208.0, 126.0), icons::CARET_DOWN, "Pull down", "to zoom out", down);
            // Off the stick, the way it goes: in time with the words that light up.
            if up > 0.0 {
                widgets::icon(p, pos2(104.0, 60.0), icons::CARET_UP, 46.0, theme::alpha(orbit_look::accent(), up));
            }
            if down > 0.0 {
                widgets::icon(p, pos2(166.0, 196.0), icons::CARET_DOWN, 46.0, theme::alpha(orbit_look::accent(), down));
            }
        }
        Stage::Grip { near } => {
            let card = Rect::from_center_size(page.center() + vec2(0.0, 30.0), vec2(PX[0] as f32 - 4.0, 140.0));
            p.rect_filled(card, theme::radius(30.0), theme::alpha(theme::INK_900, 0.95));
            if near {
                p.rect_filled(card, theme::radius(30.0), theme::alpha(orbit_look::accent(), 0.24));
                p.rect_stroke(card.shrink(1.5), theme::radius(28.0), egui::Stroke::new(3.0, orbit_look::accent()), egui::StrokeKind::Inside);
            }
            tail(p, card);
            let phase = (seconds / 1.3).fract() * std::f32::consts::TAU;
            let beat = if near { 1.0 + 0.1 * phase.sin() } else { 1.0 + 0.05 * phase.sin() };
            widgets::icon(p, card.left_center() + vec2(66.0, 0.0), icons::HAND_GRABBING, 66.0 * beat, if near { Color32::WHITE } else { orbit_look::accent() });
            let (first, second) = if near { ("Close your grip", "It comes with your hand") } else { ("Grip to take it", "With either hand") };
            p.text(card.left_center() + vec2(122.0, -20.0), Align2::LEFT_CENTER, first, theme::font(32.0, weight::BOLD), Color32::WHITE);
            p.text(card.left_center() + vec2(122.0, 22.0), Align2::LEFT_CENTER, second, theme::font(22.0, weight::MEDIUM), theme::FG_2);
        }
    }
}

/// The small Hide under a view taken in a hand, so that it can go where it is, with no menu to reopen.
pub const CHIP_PX: [u32; 2] = [280, 88];
pub const CHIP_WIDTH_M: f32 = 0.115;

/// [`HideChip`]'s picture.
pub fn paint_chip(p: &Painter, pointed: bool) {
    let page = Rect::from_min_size(Pos2::ZERO, vec2(CHIP_PX[0] as f32, CHIP_PX[1] as f32)).shrink(2.0);
    p.rect_filled(page, theme::radius(page.height() / 2.0), if pointed { orbit_look::accent() } else { theme::INK_750 });
    p.rect_stroke(page.shrink(1.0), theme::radius(page.height() / 2.0 - 1.0), egui::Stroke::new(2.5, if pointed { orbit_look::accent() } else { Color32::from_white_alpha(120) }), egui::StrokeKind::Inside);
    let ink = if pointed { orbit_look::on_accent() } else { Color32::WHITE };
    widgets::icon(p, page.left_center() + vec2(46.0, 0.0), icons::EYE_SLASH, 38.0, ink);
    p.text(page.left_center() + vec2(78.0, 0.0), Align2::LEFT_CENTER, "Hide 3D", theme::font(38.0, weight::BOLD), ink);
}

pub struct HideChip {
    handle: vr::OverlayHandle,
    canvas: Canvas,
    shown: Option<bool>,
}

impl HideChip {
    pub fn new(overlay: &vr::Overlay, key: &str) -> anyhow::Result<Self> {
        let handle = overlay.create(&format!("{key}.small-hide"), "Hide a small 3D view").map_err(|c| anyhow::anyhow!("the small view's Hide: {c:?}"))?;
        let _ = overlay.set_width_in_meters(handle, CHIP_WIDTH_M);
        let _ = overlay.set_flag(handle, vr::overlay_flag::IS_PREMULTIPLIED, true);
        let _ = overlay.set_sort_order(handle, 3);
        Ok(Self { handle, canvas: Canvas::new(CHIP_PX[0], CHIP_PX[1]), shown: None })
    }

    /// Where it shows, and whether a hand points at it; or nowhere.
    pub(super) fn update(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, at: Option<(vr::Matrix34, bool)>) {
        let Some((place, pointed)) = at else {
            if self.shown.take().is_some() {
                let _ = overlay.hide(self.handle);
            }
            return;
        };
        if self.shown != Some(pointed) {
            if self.shown.is_none() {
                self.canvas.invalidate();
            }
            for _ in 0..2 {
                self.canvas.paint(0.0, |p| paint_chip(p, pointed));
            }
            if let Some(changed) = self.canvas.damage() {
                presenter.show_part(overlay, self.handle, self.canvas.pixels(), CHIP_PX[0], CHIP_PX[1], changed);
            }
        }
        let _ = overlay.set_transform_absolute(self.handle, &place);
        if self.shown.is_none() {
            let _ = overlay.show(self.handle);
        }
        self.shown = Some(pointed);
    }

    pub fn destroy(&self, overlay: &vr::Overlay) {
        let _ = overlay.destroy(self.handle);
    }
}

/// Where a view's Hide lies, facing `eye`: just past the plate's rim on the side towards who looks, and under it. `plate` is the plate's middle.
pub fn chip_place(plate: V3, radius: f32, eye: V3) -> vr::Matrix34 {
    let to = stereo::sub(eye, plate);
    let level = stereo::length([to[0], 0.0, to[2]]).max(1e-3);
    let towards = [to[0] / level, 0.0, to[2] / level];
    facing(stereo::add(stereo::add(plate, stereo::scale(towards, radius * 1.02)), [0.0, -0.055, 0.0]), eye)
}

/// Whether a tool at `tip`, pointing along `along`, is on a chip at `place`: its ray within a generous margin, or itself close by.
pub fn on_chip(place: &vr::Matrix34, tip: V3, along: V3) -> bool {
    let middle = stereo::position(place);
    if stereo::length(stereo::sub(tip, middle)) < 0.09 {
        return true;
    }
    let normal = stereo::axis(place, 2);
    let facing = stereo::dot(along, normal);
    if facing.abs() < 1e-3 {
        return false;
    }
    let distance = stereo::dot(stereo::sub(middle, tip), normal) / facing;
    if distance <= 0.0 {
        return false;
    }
    let hit = stereo::sub(stereo::add(tip, stereo::scale(along, distance)), middle);
    stereo::dot(hit, stereo::axis(place, 0)).abs() <= CHIP_WIDTH_M * 0.8 && stereo::dot(hit, stereo::axis(place, 1)).abs() <= CHIP_WIDTH_M * 0.4
}

/// Its overlay.
pub struct SmallHint {
    handle: vr::OverlayHandle,
    canvas: Canvas,
    born: Instant,
    painted: Option<Instant>,
    shown: Option<Stage>,
}

impl SmallHint {
    pub fn new(overlay: &vr::Overlay, key: &str) -> anyhow::Result<Self> {
        let handle = overlay.create(&format!("{key}.small-hint"), "How to handle a small 3D view").map_err(|c| anyhow::anyhow!("the small view's hint: {c:?}"))?;
        let _ = overlay.set_width_in_meters(handle, WIDTH_M);
        let _ = overlay.set_flag(handle, vr::overlay_flag::IS_PREMULTIPLIED, true);
        // Over the capture, and under N4N0's menu and these panels.
        let _ = overlay.set_sort_order(handle, 3);
        Ok(Self { handle, canvas: Canvas::new(PX[0], PX[1]), born: Instant::now(), painted: None, shown: None })
    }

    /// Shows what is wanted, above where what it speaks of is and how far that reaches, facing `head`; or nothing.
    pub(super) fn update(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, head: &vr::Matrix34, wanted: Option<(Stage, V3, f32)>, moves: bool) {
        let Some((stage, at, radius)) = wanted else {
            if self.shown.take().is_some() {
                let _ = overlay.hide(self.handle);
            }
            return;
        };
        let due = self.painted.is_none_or(|t| t.elapsed().as_secs_f32() >= FRAME_S);
        if self.shown != Some(stage) || (moves && due) {
            if self.shown.is_none() {
                self.canvas.invalidate();
                self.born = Instant::now();
            }
            self.painted = Some(Instant::now());
            // With Reduce motion it stands still, pushed up.
            let seconds = if moves { self.born.elapsed().as_secs_f32() } else { CYCLE_S * 0.25 };
            self.canvas.paint(seconds as f64, |p| paint(p, stage, seconds));
            if let Some(changed) = self.canvas.damage() {
                presenter.show_part(overlay, self.handle, self.canvas.pixels(), PX[0], PX[1], changed);
            }
        }
        let _ = overlay.set_transform_absolute(self.handle, &facing(hint_place(at, radius), stereo::position(head)));
        if self.shown.is_none() {
            let _ = overlay.show(self.handle);
        }
        self.shown = Some(stage);
    }

    pub fn destroy(&self, overlay: &vr::Overlay) {
        let _ = overlay.destroy(self.handle);
    }
}

/// The plate a capture standing small stands on, under it: a soft shadow, a dark disc, a clean rim where the capture ends. It is
/// in the room, tipped as the capture is, so that it gives the capture its shape from any side.
pub struct Plate {
    handle: vr::OverlayHandle,
    pixels: Vec<u8>,
    painted: bool,
    shown: bool,
}

/// The plate's picture, premultiplied, `side` pixels square.
pub fn plate_pixels(side: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((side * side * 4) as usize);
    let rim = stereo::SMALL_PLATE_RIM;
    let smooth = |a: f32, b: f32, x: f32| {
        let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    for y in 0..side {
        for x in 0..side {
            let (u, v) = ((x as f32 + 0.5) / side as f32 * 2.0 - 1.0, (y as f32 + 0.5) / side as f32 * 2.0 - 1.0);
            let at = u.hypot(v);
            // From the edge in: shadow, then the disc under the capture, a little lighter in its middle.
            let shadow = 0.6 * (1.0 - smooth(rim, 1.0, at));
            let disc = 1.0 - smooth(rim - 0.006, rim + 0.002, at);
            let ring = (1.0 - ((at - rim).abs() / 0.012).clamp(0.0, 1.0)).powf(0.7);
            let (mut r, mut g, mut b, mut a) = (0.0f32, 0.0f32, 0.0f32, shadow);
            if disc > 0.0 {
                let light = (1.0 - at / rim).clamp(0.0, 1.0) * 0.5;
                let (dr, dg, db) = (0.09 + 0.05 * light, 0.1 + 0.055 * light, 0.125 + 0.07 * light);
                let da = 0.94;
                r = r * (1.0 - disc) + dr * da * disc;
                g = g * (1.0 - disc) + dg * da * disc;
                b = b * (1.0 - disc) + db * da * disc;
                a = a * (1.0 - disc) + da * disc;
            }
            // Its rim, Orbiters' own green, soft on the disc.
            let (rr, rg, rb) = (0.18, 0.95, 0.56);
            r = r * (1.0 - ring) + rr * ring;
            g = g * (1.0 - ring) + rg * ring;
            b = b * (1.0 - ring) + rb * ring;
            a = a * (1.0 - ring) + ring;
            pixels.extend_from_slice(&[(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, (a * 255.0) as u8]);
        }
    }
    pixels
}

impl Plate {
    const SIDE: u32 = 512;

    pub fn new(overlay: &vr::Overlay, key: &str) -> anyhow::Result<Self> {
        let handle = overlay.create(&format!("{key}.small-plate"), "Under a small 3D view").map_err(|c| anyhow::anyhow!("the small view's plate: {c:?}"))?;
        let _ = overlay.set_flag(handle, vr::overlay_flag::IS_PREMULTIPLIED, true);
        // Over a panel's pane, under the capture.
        let _ = overlay.set_sort_order(handle, 1);
        Ok(Self { handle, pixels: plate_pixels(Self::SIDE), painted: false, shown: false })
    }

    pub(super) fn update(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, place: Option<(vr::Matrix34, f32)>) {
        let Some((at, width)) = place else {
            if std::mem::take(&mut self.shown) {
                let _ = overlay.hide(self.handle);
            }
            return;
        };
        if !self.painted {
            self.painted = true;
            presenter.show_part(overlay, self.handle, &self.pixels, Self::SIDE, Self::SIDE, [0, 0, Self::SIDE, Self::SIDE]);
        }
        let _ = overlay.set_width_in_meters(self.handle, width);
        let _ = overlay.set_transform_absolute(self.handle, &at);
        if !std::mem::replace(&mut self.shown, true) {
            let _ = overlay.show(self.handle);
        }
    }

    pub fn destroy(&self, overlay: &vr::Overlay) {
        let _ = overlay.destroy(self.handle);
    }
}

/// What lies behind a capture in a panel's hole, under it, so that VRChat's menu does not show through around it: a dark
/// pane in the panel's plane, a little larger than the hole, whose edge the panel covers.
pub struct Backdrop {
    handle: vr::OverlayHandle,
    pixels: Vec<u8>,
    painted: bool,
    shown: bool,
}

/// The pane's size and picture.
pub const BACKDROP_PX: [u32; 2] = [64, 36];
pub fn backdrop_pixels() -> Vec<u8> {
    let mut pixels = Vec::with_capacity((BACKDROP_PX[0] * BACKDROP_PX[1] * 4) as usize);
    for y in 0..BACKDROP_PX[1] {
        for x in 0..BACKDROP_PX[0] {
            // Lightest in its middle, as a lit stage.
            let (u, v) = (x as f32 / (BACKDROP_PX[0] - 1) as f32 * 2.0 - 1.0, y as f32 / (BACKDROP_PX[1] - 1) as f32 * 2.0 - 1.0);
            let light = (1.0 - (u * u + v * v) * 0.5).clamp(0.0, 1.0);
            pixels.extend_from_slice(&[(26.0 + 44.0 * light) as u8, (30.0 + 48.0 * light) as u8, (38.0 + 58.0 * light) as u8, 255]);
        }
    }
    pixels
}

impl Backdrop {
    const PX: [u32; 2] = BACKDROP_PX;

    pub fn new(overlay: &vr::Overlay, key: &str) -> anyhow::Result<Self> {
        let handle = overlay.create(&format!("{key}.small-backdrop"), "Behind a small 3D view").map_err(|c| anyhow::anyhow!("the small view's backdrop: {c:?}"))?;
        let _ = overlay.set_flag(handle, vr::overlay_flag::IS_PREMULTIPLIED, true);
        // Under the capture.
        let _ = overlay.set_sort_order(handle, 0);
        Ok(Self { handle, pixels: backdrop_pixels(), painted: false, shown: false })
    }

    /// At `place` (the middle of the hole), `width` across; or nowhere.
    pub(super) fn update(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, place: Option<(vr::Matrix34, f32)>) {
        let Some((at, width)) = place else {
            if std::mem::take(&mut self.shown) {
                let _ = overlay.hide(self.handle);
            }
            return;
        };
        if !self.painted {
            self.painted = true;
            presenter.show_part(overlay, self.handle, &self.pixels, Self::PX[0], Self::PX[1], [0, 0, Self::PX[0], Self::PX[1]]);
        }
        let _ = overlay.set_width_in_meters(self.handle, width);
        let _ = overlay.set_transform_absolute(self.handle, &at);
        if !std::mem::replace(&mut self.shown, true) {
            let _ = overlay.show(self.handle);
        }
    }

    pub fn destroy(&self, overlay: &vr::Overlay) {
        let _ = overlay.destroy(self.handle);
    }
}

/// Where the hint's middle lies, for what it speaks of at `at` reaching `radius`: just over its middle, as most captures are flatter than they are wide, its tail towards it.
pub fn hint_place(at: V3, radius: f32) -> V3 {
    let height = WIDTH_M * PX[1] as f32 / PX[0] as f32;
    stereo::add(at, [0.0, radius * 0.5 + height / 2.0 + ABOVE_M, 0.0])
}

/// Upright at `place`, its face towards `eye`.
pub fn facing(place: V3, eye: V3) -> vr::Matrix34 {
    let to = stereo::sub(eye, place);
    let level = [to[0], 0.0, to[2]];
    let z = stereo::scale(level, 1.0 / stereo::length(level).max(1e-3));
    let y = [0.0, 1.0, 0.0];
    let x = stereo::cross(y, z);
    vr::Matrix34 { m: [[x[0], y[0], z[0], place[0]], [x[1], y[1], z[1], place[1]], [x[2], y[2], z[2], place[2]]] }
}

/// For the offline review: `small-hint-<what>.png`.
pub fn shots(out: &std::path::Path) -> anyhow::Result<()> {
    let stages = [
        ("stick-up", Stage::Stick { hand: 1 }, CYCLE_S * 0.25),
        ("stick-middle", Stage::Stick { hand: 0 }, 0.0),
        ("stick-down", Stage::Stick { hand: 1 }, CYCLE_S * 0.75),
        ("grip", Stage::Grip { near: false }, 0.0),
        ("grip-near", Stage::Grip { near: true }, 0.0),
    ];
    for (name, stage, seconds) in stages {
        let mut canvas = Canvas::new(PX[0], PX[1]);
        for _ in 0..2 {
            canvas.paint(seconds as f64, |p| paint(p, stage, seconds));
        }
        super::shots::write_png(&out.join(format!("small-hint-{name}.png")), PX[0], PX[1], canvas.pixels())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_stick_goes_up_and_down_and_rests_at_each_end() {
        assert_eq!(stick_at(CYCLE_S * 0.25), 1.0);
        assert_eq!(stick_at(CYCLE_S * 0.75), -1.0);
        assert_eq!(stick_at(CYCLE_S * 0.2), 1.0, "a moment at the top");
        assert!(stick_at(0.0).abs() < 1e-3);
    }

    #[test]
    fn its_hide_is_found_by_a_laser_with_room_to_spare_or_by_a_hand_near_it() {
        let place = chip_place([0.0, 1.4, -0.6], 0.3, [0.0, 1.6, 0.0]);
        let middle = stereo::position(&place);
        // From the chest, straight at it.
        let tip = [0.0, 1.0, -0.2];
        let along = stereo::sub(middle, tip);
        let along = stereo::scale(along, 1.0 / stereo::length(along));
        assert!(on_chip(&place, tip, along));
        // A little off is still on it, well off is not, and pointing away is not.
        assert!(on_chip(&place, stereo::add(tip, [0.05, 0.0, 0.0]), along));
        assert!(!on_chip(&place, stereo::add(tip, [0.3, 0.0, 0.0]), along));
        assert!(!on_chip(&place, tip, stereo::scale(along, -1.0)));
        // A hand there needs no aim.
        assert!(on_chip(&place, stereo::add(middle, [0.03, 0.0, 0.0]), [0.0, 1.0, 0.0]));
    }

    #[test]
    fn it_faces_who_looks_and_stands_upright() {
        let m = facing([1.0, 1.5, 0.0], [1.0, 1.7, 2.0]);
        assert!(m.m[2][2] > 0.99, "towards the eye");
        assert_eq!(m.m[1][1], 1.0);
        assert!((m.m[0][0] - 1.0).abs() < 1e-3, "its right is on the right of who looks");
    }
}
