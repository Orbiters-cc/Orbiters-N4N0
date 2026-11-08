//! What says how to handle a capture standing small, beside it in the room (Enzo: "a little joystick 3D animation and text
//! showing it pushed up / down"): first to take it, then to zoom it. Once a stick zoomed it, it goes until a new capture opens
//! from a new opening of the menu ([`super::Worlds`]).

use super::super::{Canvas, Presenter};
use super::stereo::{self, V3};
use super::stick3d;
use egui::{pos2, vec2, Align2, Color32, Painter, Pos2, Rect, Vec2};
use orbiters_i18n::{language, tr, Language};
use orbiters_openvr as vr;
use orbiters_ui::theme::{self, weight};
use super::super::orbit::panels::look as orbit_look;
use orbiters_ui::{icons, widgets};
use std::time::Instant;

pub const PX: [u32; 2] = [560, 256];
pub const WIDTH_M: f32 = 0.35;
/// How high above what it speaks of it floats.
const ABOVE_M: f32 = 0.06;
const FRAME_S: f32 = 1.0 / 45.0;
/// One push up and pull down, in seconds.
const CYCLE_S: f32 = stick3d::CYCLE_S;
/// How often the stick is drawn again: it is traced for each eye.
const STICK_FRAME_S: f32 = 1.0 / 36.0;
/// Where it stands against the words' middle: in the part of their page left for it.
/// How far round it someone may go before it turns to follow them, in radians.
const STICK_TURNS_BY: f32 = 0.7;
pub const STICK_AT: [f32; 2] = [-WIDTH_M / 2.0 + STICK_COLUMN_PX / PX[0] as f32 * WIDTH_M / 2.0, -0.004];
/// The part of the page the stick stands in, at its left, its name under it.
const STICK_COLUMN_PX: f32 = 236.0;

/// What it says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Grip to take it; `near`: a hand is near enough.
    Grip { near: bool },
    /// That hand's stick, up and down: 0 the left, 1 the right.
    Stick { hand: usize },
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// The offsets of a dark edge round what floats on the world with nothing behind it (Enzo: no frame), to read on a bright world as on a dark one.
const EDGE: [Vec2; 8] = [vec2(-2.0, 0.0), vec2(2.0, 0.0), vec2(0.0, -2.0), vec2(0.0, 2.0), vec2(-1.5, -1.5), vec2(1.5, -1.5), vec2(-1.5, 1.5), vec2(1.5, 1.5)];

fn edged_text(p: &Painter, at: Pos2, align: Align2, text: &str, font: egui::FontId, color: Color32) {
    for by in EDGE {
        p.text(at + by, align, text, font.clone(), theme::alpha(Color32::BLACK, 0.85));
    }
    p.text(at, align, text, font, color);
}

fn edged_icon(p: &Painter, at: Pos2, glyph: &str, size: f32, color: Color32) {
    for by in EDGE {
        widgets::icon(p, at + by, glyph, size, theme::alpha(Color32::BLACK, 0.85));
    }
    widgets::icon(p, at, glyph, size, color);
}

fn words(p: &Painter, at: Pos2, glyph: &str, first: &str, second: &str, on: f32) {
    let lit = mix(theme::FG, orbit_look::accent(), on);
    let text = mix(theme::FG, Color32::WHITE, on);
    edged_icon(p, at + vec2(16.0, 32.0), glyph, 30.0, lit);
    edged_text(p, at + vec2(42.0, 18.0), Align2::LEFT_CENTER, first, theme::font(30.0, weight::BOLD), text);
    edged_text(p, at + vec2(42.0, 49.0), Align2::LEFT_CENTER, second, theme::font(22.0, weight::SEMIBOLD), text);
}

/// The hint on a page of [`PX`], `seconds` into its animation.
pub fn paint(p: &Painter, stage: Stage, seconds: f32) {
    let page = Rect::from_min_size(Pos2::ZERO, vec2(PX[0] as f32, PX[1] as f32));
    match stage {
        Stage::Stick { hand } => {
            // The stick itself is a thing in the room beside these ([`super::stick3d`]).
            let push = stick3d::push_at(seconds);
            let which = if hand == 0 { tr("Left stick") } else { tr("Right stick") };
            // Its name under the stick, as large as what it does (judges could hardly read it at 19).
            edged_text(p, pos2(STICK_COLUMN_PX / 2.0, 236.0), Align2::CENTER_CENTER, which, theme::font(25.0, weight::BOLD), Color32::WHITE);
            let up = ((push - 0.2) / 0.8).clamp(0.0, 1.0);
            let down = ((-push - 0.2) / 0.8).clamp(0.0, 1.0);
            words(p, pos2(STICK_COLUMN_PX + 4.0, 20.0), icons::CARET_UP, tr("Push up"), tr("to zoom in"), up);
            words(p, pos2(STICK_COLUMN_PX + 4.0, 104.0), icons::CARET_DOWN, tr("Pull down"), tr("to zoom out"), down);
            edged_icon(p, pos2(STICK_COLUMN_PX + 20.0, 214.0), icons::ARROWS_OUT, 26.0, theme::FG);
            let room = PX[0] as f32 - STICK_COLUMN_PX - 50.0;
            let font = theme::font(22.0, weight::SEMIBOLD);
            let said = super::super::orbit::panels::fit(p, tr("Other stick: move around"), &font, room);
            edged_text(p, pos2(STICK_COLUMN_PX + 46.0, 214.0), Align2::LEFT_CENTER, &said, font, theme::FG);
        }
        Stage::Grip { near } => {
            let card = Rect::from_center_size(page.center() + vec2(0.0, 30.0), vec2(PX[0] as f32 - 4.0, 140.0));
            let phase = (seconds / 1.3).fract() * std::f32::consts::TAU;
            let beat = if near { 1.0 + 0.1 * phase.sin() } else { 1.0 + 0.05 * phase.sin() };
            edged_icon(p, card.left_center() + vec2(66.0, 0.0), icons::HAND_GRABBING, 66.0 * beat, if near { Color32::WHITE } else { orbit_look::accent() });
            let (first, second) = if near { (tr("Close your grip"), tr("It comes with your hand")) } else { (tr("Grip to take it"), tr("With either hand")) };
            let ink = if near { orbit_look::accent() } else { Color32::WHITE };
            edged_text(p, card.left_center() + vec2(122.0, -20.0), Align2::LEFT_CENTER, first, theme::font(32.0, weight::BOLD), ink);
            edged_text(p, card.left_center() + vec2(122.0, 22.0), Align2::LEFT_CENTER, second, theme::font(22.0, weight::MEDIUM), theme::FG);
        }
    }
}

/// The small X above a view, so that it can go where it is, with no menu to reopen.
pub const CHIP_PX: [u32; 2] = [128, 128];
pub const CHIP_WIDTH_M: f32 = 0.05;
/// How far off its middle a laser still has it: the X is small, and with the menu off there is no beam to aim by.
const CHIP_AIM_M: f32 = 0.08;

/// [`HideChip`]'s picture.
pub fn paint_chip(p: &Painter, pointed: bool) {
    let page = Rect::from_min_size(Pos2::ZERO, vec2(CHIP_PX[0] as f32, CHIP_PX[1] as f32));
    let radius = page.height() / 2.0 - 2.0;
    p.circle_filled(page.center(), radius, theme::alpha(theme::INK_900, 0.95));
    p.circle_stroke(page.center(), radius - 1.5, egui::Stroke::new(3.0, if pointed { orbit_look::accent() } else { theme::white(0.25) }));
    widgets::icon(p, page.center(), icons::X, radius * 1.05, if pointed { orbit_look::accent() } else { theme::FG });
}

pub struct HideChip {
    handle: vr::OverlayHandle,
    canvas: Canvas,
    shown: Option<bool>,
    /// The language its picture was drawn in: a picture kept on the overlay does not change with the language, so it is drawn again.
    drawn_in: Language,
}

impl HideChip {
    /// For `vr-test.log`.
    pub fn report(&self) -> String {
        format!("X {:?}", self.shown) // i18n: not shown (a log's line)
    }

    pub fn new(overlay: &vr::Overlay, key: &str) -> anyhow::Result<Self> {
        let handle = overlay.create(&format!("{key}.small-hide"), "Hide a small 3D view").map_err(|c| anyhow::anyhow!("the small view's Hide: {c:?}"))?; // i18n: not shown (the overlay's name in SteamVR)
        let _ = overlay.set_width_in_meters(handle, CHIP_WIDTH_M);
        let _ = overlay.set_flag(handle, vr::overlay_flag::IS_PREMULTIPLIED, true);
        let _ = overlay.set_sort_order(handle, 3);
        Ok(Self { handle, canvas: Canvas::new(CHIP_PX[0], CHIP_PX[1]), shown: None, drawn_in: language() })
    }

    /// Where it shows, and whether a hand points at it; or nowhere.
    pub(super) fn update(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, at: Option<(vr::Matrix34, bool)>) {
        let Some((place, pointed)) = at else {
            if self.shown.take().is_some() {
                let _ = overlay.hide(self.handle);
            }
            return;
        };
        if self.shown != Some(pointed) || self.drawn_in != language() {
            if self.shown.is_none() || self.drawn_in != language() {
                self.canvas.invalidate();
            }
            self.drawn_in = language();
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

/// Where a view's X lies, facing `eye`: floating just above it.
pub fn chip_place(middle: V3, radius: f32, eye: V3, over_the_hint: bool) -> vr::Matrix34 {
    let over = if over_the_hint { hint_place(middle, radius)[1] - middle[1] + WIDTH_M * PX[1] as f32 / PX[0] as f32 / 2.0 + 0.04 } else { radius + 0.05 };
    facing(stereo::add(middle, [0.0, over, 0.0]), eye)
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
    stereo::dot(hit, stereo::axis(place, 0)).abs() <= CHIP_AIM_M && stereo::dot(hit, stereo::axis(place, 1)).abs() <= CHIP_AIM_M
}

/// Its overlay.
pub struct SmallHint {
    handle: vr::OverlayHandle,
    /// The stick, as a picture for each eye.
    stick: vr::OverlayHandle,
    stick_drawn: Option<Instant>,
    stick_shown: bool,
    stick_faces: Option<V3>,
    canvas: Canvas,
    born: Instant,
    painted: Option<Instant>,
    shown: Option<Stage>,
    /// The language its picture was drawn in: it stands still with Reduce motion, so it is drawn again when the language changes.
    drawn_in: Language,
}

impl SmallHint {
    pub fn new(overlay: &vr::Overlay, key: &str) -> anyhow::Result<Self> {
        let handle = overlay.create(&format!("{key}.small-hint"), "How to handle a small 3D view").map_err(|c| anyhow::anyhow!("the small view's hint: {c:?}"))?; // i18n: not shown (the overlay's name in SteamVR)
        let _ = overlay.set_width_in_meters(handle, WIDTH_M);
        let _ = overlay.set_flag(handle, vr::overlay_flag::IS_PREMULTIPLIED, true);
        // Over the capture, and under N4N0's menu and these panels.
        let _ = overlay.set_sort_order(handle, 3);
        let stick = overlay.create(&format!("{key}.small-hint-stick"), "A thumbstick").map_err(|c| anyhow::anyhow!("the small view's stick: {c:?}"))?; // i18n: not shown (the overlay's name in SteamVR's own lists)
        let _ = overlay.set_width_in_meters(stick, stick3d::WIDTH_M);
        let _ = overlay.set_flag(stick, vr::overlay_flag::IS_PREMULTIPLIED, true);
        let _ = overlay.set_flag(stick, vr::overlay_flag::SIDE_BY_SIDE_PARALLEL, true);
        let _ = overlay.set_sort_order(stick, 3);
        Ok(Self { stick, stick_drawn: None, stick_shown: false, stick_faces: None, handle, canvas: Canvas::new(PX[0], PX[1]), born: Instant::now(), painted: None, shown: None, drawn_in: language() })
    }

    /// Shows what is wanted, above where what it speaks of is and how far that reaches, facing `head`; or nothing.
    pub(super) fn update(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, head: &vr::Matrix34, eyes: [V3; 2], wanted: Option<(Stage, V3, f32)>, moves: bool) {
        let Some((stage, at, radius)) = wanted else {
            if self.shown.take().is_some() {
                let _ = overlay.hide(self.handle);
            }
            if std::mem::take(&mut self.stick_shown) {
                let _ = overlay.hide(self.stick);
            }
            self.stick_faces = None;
            return;
        };
        let due = self.painted.is_none_or(|t| t.elapsed().as_secs_f32() >= FRAME_S);
        if self.shown != Some(stage) || (moves && due) || self.drawn_in != language() {
            if self.shown.is_none() {
                self.born = Instant::now();
            }
            if self.shown.is_none() || self.drawn_in != language() {
                self.canvas.invalidate();
            }
            self.drawn_in = language();
            self.painted = Some(Instant::now());
            // With Reduce motion it stands still, pushed up.
            let seconds = if moves { self.born.elapsed().as_secs_f32() } else { CYCLE_S * 0.25 };
            self.canvas.paint(seconds as f64, |p| paint(p, stage, seconds));
            if let Some(changed) = self.canvas.damage() {
                presenter.show_part(overlay, self.handle, self.canvas.pixels(), PX[0], PX[1], changed);
            }
        }
        let place = facing(hint_place(at, radius), stereo::position(head));
        let _ = overlay.set_transform_absolute(self.handle, &place);
        if self.shown.is_none() {
            let _ = overlay.show(self.handle);
        }
        self.shown = Some(stage);
        if matches!(stage, Stage::Stick { .. }) {
            let middle = stereo::add(stereo::add(stereo::position(&place), stereo::scale(stereo::axis(&place, 0), STICK_AT[0])), stereo::scale(stereo::axis(&place, 1), STICK_AT[1]));
            // A thing in the room: it stays turned as it is, and walking round it shows its side. Only so far round does it come after who looks, or there is nothing left to see through.
            let wanted = stereo::axis(&place, 2);
            let facing_now = self.stick_faces.get_or_insert(wanted);
            let off = stereo::dot(*facing_now, wanted).clamp(-1.0, 1.0).acos();
            if off > STICK_TURNS_BY {
                let k = ((off - STICK_TURNS_BY) / off).min(1.0);
                let towards = stereo::add(*facing_now, stereo::scale(stereo::sub(wanted, *facing_now), k));
                *facing_now = stereo::scale(towards, 1.0 / stereo::length(towards).max(1e-3));
            }
            let there = facing(middle, stereo::add(middle, *facing_now));
            let _ = overlay.set_transform_absolute(self.stick, &there);
            if self.stick_drawn.is_none_or(|t| t.elapsed().as_secs_f32() >= STICK_FRAME_S) {
                self.stick_drawn = Some(Instant::now());
                let seconds = if moves { self.born.elapsed().as_secs_f32() } else { CYCLE_S * 0.25 };
                let own = |eye: V3| {
                    let d = stereo::sub(eye, middle);
                    [stereo::dot(d, stereo::axis(&there, 0)), stereo::dot(d, stereo::axis(&there, 1)), stereo::dot(d, stereo::axis(&there, 2))]
                };
                let pixels = stick3d::picture(eyes.map(own), stick3d::push_at(seconds), orbit_look::accent());
                presenter.show(overlay, self.stick, &pixels, stick3d::EYE_PX[0] * 2, stick3d::EYE_PX[1]);
            }
            if !std::mem::replace(&mut self.stick_shown, true) {
                let _ = overlay.show(self.stick);
            }
        } else if std::mem::take(&mut self.stick_shown) {
            let _ = overlay.hide(self.stick);
        }
    }

    pub fn destroy(&self, overlay: &vr::Overlay) {
        let _ = overlay.destroy(self.handle);
        let _ = overlay.destroy(self.stick);
    }

    /// For `vr-test.log`.
    pub fn report(&self) -> String {
        format!("hint {:?} stick shown {}", self.shown, self.stick_shown) // i18n: not shown (a log's line)
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
        let handle = overlay.create(&format!("{key}.small-backdrop"), "Behind a small 3D view").map_err(|c| anyhow::anyhow!("the small view's backdrop: {c:?}"))?; // i18n: not shown (the overlay's name in SteamVR)
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
    let shot_folder = out.to_path_buf();
    // The stick as each eye has it, straight on and from well to the side, over a dark world and a bright one, its two eyes side by side.
    let world = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/overlay/orbit/testdata_camera/camera-headset-1.png");
    let dark = image::open(world)?.to_rgba8();
    // A white room, as a capture of one lit from a window has it.
    let bright = image::RgbaImage::from_fn(dark.width(), dark.height(), |x, y| {
        let light = 236.0 - (y as f32 / 720.0) * 30.0 - ((x as f32 / 40.0).sin() * 5.0);
        image::Rgba([light as u8, (light - 3.0) as u8, (light - 10.0) as u8, 255])
    });
    for (world, ground) in [("dark", dark), ("bright", bright)] {
        for (name, eyes) in [("straight", [[-0.032, 0.08, 0.42], [0.032, 0.08, 0.42]]), ("angle", [[0.24, 0.12, 0.34], [0.29, 0.12, 0.30]])] {
            for (when, push) in [("up", 1.0f32), ("down", -1.0)] {
                let pixels = stick3d::picture(eyes, push, orbit_look::accent());
                let (w, h) = (stick3d::EYE_PX[0] * 2, stick3d::EYE_PX[1]);
                let mut composed = Vec::with_capacity(pixels.len());
                for (i, p) in pixels.chunks_exact(4).enumerate() {
                    let (x, y) = (i as u32 % w, i as u32 / w);
                    let under = ground.get_pixel(300 + x, 200 + y);
                    let a = 1.0 - p[3] as f32 / 255.0;
                    for c in 0..3 {
                        composed.push((p[c] as f32 + under[c] as f32 * a).min(255.0) as u8);
                    }
                    composed.push(255);
                }
                super::shots::write_png(&shot_folder.join(format!("small-hint-3d-{world}-{name}-{when}.png")), w, h, &composed)?;
            }
        }
    }
    // The Hide under a view taken in a hand, away from and pointed at.
    for (name, pointed) in [("hide", false), ("hide-pointed", true)] {
        let mut canvas = Canvas::new(CHIP_PX[0], CHIP_PX[1]);
        for _ in 0..2 {
            canvas.paint(0.0, |p| paint_chip(p, pointed));
        }
        super::shots::write_png(&out.join(format!("small-hint-{name}.png")), CHIP_PX[0], CHIP_PX[1], canvas.pixels())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_hide_is_found_by_a_laser_with_room_to_spare_or_by_a_hand_near_it() {
        let place = chip_place([0.0, 1.4, -0.6], 0.3, [0.0, 1.6, 0.0], false);
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
