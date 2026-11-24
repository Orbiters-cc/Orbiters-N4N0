//! "Talk with N4N0?" in the play space ([`crate::wake_offer`], N4N0-11): while the wake word heard during a call has it up,
//! a band before the eyes, a little below their line, where they looked when it came (it stays there: a laser can aim at
//! it, and it does not swim with the head), pressed as the strip is (either hand's laser on it, a pull of its trigger).
//! It shows for the offer's few seconds only, its line of time left shrinking, and goes by itself.

use super::space::{self, Quad};
use crate::overlay::{Canvas, Presenter};
use crate::wake_offer::{paint_vr, VR_PX, VR_WIDTH_M};
use orbiters_openvr::{self as vr, overlay_flag, Matrix34};

/// How far before the eyes, and how far under their line, it comes.
const DISTANCE_M: f32 = 0.6;
const BELOW_M: f32 = 0.16;
/// Over the panel and its bar, under the lasers, as the watch.
const SORT_ORDER: u32 = 12;
/// Steps of its line of time left that are painted (about ten a second over six seconds).
const STEPS: f32 = 60.0;


pub struct Prompt {
    handle: vr::OverlayHandle,
    canvas: Canvas,
    presented: crate::overlay::presented::Presented,
    /// Its picture's size and its width in the play space, as its face takes ([`Face::size`]).
    px: [u32; 2],
    width_m: f32,
    /// The offer it shows, and where it was put for it.
    placed: Option<(u64, Matrix34)>,
    /// The offer pressed here: it goes at once, before the app's word that it was taken.
    pressed: Option<u64>,
    visible: bool,
    /// What its picture shows: which face, hovered, the language, the step of time left.
    drawn: Option<(Face, bool, orbiters_i18n::Language, u32)>,
    /// Or a face of the shared 3D view's ([`Self::paint_with`]): a digest of what it shows.
    drawn_with: Option<u64>,
    /// What SteamVR confirmed of the offer placed: the only ground on which it says it shows it.
    evidence: Evidence,
    /// Where the face it shows goes before the eyes ([`Face::spot`]).
    spot: (f32, f32, f32),
    /// Its alpha as SteamVR has it (the world notice fades in and out at it, with no new picture).
    alpha: f32,
}

/// What SteamVR confirmed of the band for the offer placed (night4, verify2 SWEEP-7): put where it goes, made visible,
/// its picture taken. A refused call means it does not show that offer, and the window's countdown does not run on it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Evidence {
    placed: bool,
    shown: bool,
    pictured: bool,
}

impl Evidence {
    fn confirms(&self) -> bool {
        self.placed && self.shown && self.pictured
    }
}

/// What the band asks: "Talk with N4N0?" (`crate::wake_offer`), or "Orbiters - N4N0 will minimize to save memory" with
/// Keep open and its seconds left (`crate::window_rest`). One band, one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Talk,
    KeepOpen { seconds: u64 },
    /// "No 3D picture of this world yet" with "Take a 3D picture" and its cross (`crate::world_notice`, REQUEST13, made
    /// discreet by Request20); whether it says the point a picture there earns, the part a laser is on.
    World { earns_one: bool, on: Option<crate::world_notice::Part> },
    /// The avatar gun's card (`crate::avatar_gun_ui`): its question (`ask`) straight ahead, its notices low and to the right;
    /// the first change's notice (`tall`) with its hint under it.
    Gun { ask: bool, tall: bool },
}

impl Face {
    /// Its picture's size and width: the world notice has a row for its words and one for its action.
    pub fn size(self) -> ([u32; 2], f32) {
        match self {
            Face::World { .. } => (crate::world_notice::VR_PX, crate::world_notice::VR_WIDTH_M),
            Face::Gun { tall: false, .. } => (crate::avatar_gun_ui::VR_PX, crate::avatar_gun_ui::VR_WIDTH_M),
            Face::Gun { tall: true, .. } => (crate::avatar_gun_ui::VR_TALL_PX, crate::avatar_gun_ui::VR_WIDTH_M),
            _ => (VR_PX, VR_WIDTH_M),
        }
    }

    /// Where it goes before the eyes when it comes: how far ahead, how far under their line, how far right of where the
    /// head faces, in metres. The world notice low and to the right, out of the way (Request20); the rest straight ahead.
    pub fn spot(self) -> (f32, f32, f32) {
        match self {
            Face::World { .. } | Face::Gun { ask: false, .. } => {
                let s = crate::world_notice::VR_SPOT;
                (s.distance, s.below, s.right)
            }
            _ => (DISTANCE_M, BELOW_M, 0.0),
        }
    }
}

impl Prompt {
    pub fn new(overlay: &vr::Overlay, key: &str) -> anyhow::Result<Self> {
        let handle = crate::overlay::slots::create(overlay, &format!("{key}.wakeoffer"), "Orbiters talk with N4N0").map_err(|c| anyhow::anyhow!("talk with N4N0: SteamVR overlay error {c}"))?; // i18n: not shown (a name for SteamVR)
        let _ = overlay.set_width_in_meters(handle, VR_WIDTH_M);
        let _ = overlay.set_flag(handle, overlay_flag::IS_PREMULTIPLIED, true);
        let _ = overlay.set_sort_order(handle, SORT_ORDER);
        Ok(Self { handle, canvas: Canvas::new(VR_PX[0], VR_PX[1]), presented: Default::default(), px: VR_PX, width_m: VR_WIDTH_M, placed: None, pressed: None, visible: false, drawn: None, drawn_with: None, evidence: Evidence::default(), spot: Face::Talk.spot(), alpha: 1.0 })
    }

    /// At the size `face` takes (before [`Prompt::place`]): another size is a new picture, placed again.
    pub fn fit(&mut self, overlay: &vr::Overlay, face: Face) {
        let (px, width_m) = face.size();
        if face.spot() != self.spot {
            self.spot = face.spot();
            self.placed = None;
        }
        if px != self.px || width_m != self.width_m {
            self.canvas = Canvas::new(px[0], px[1]);
            self.px = px;
            self.width_m = width_m;
            let _ = overlay.set_width_in_meters(self.handle, width_m);
            self.placed = None;
            self.drawn = None;
            self.drawn_with = None;
        }
    }

    fn height_m(&self) -> f32 {
        self.width_m * self.px[1] as f32 / self.px[0] as f32
    }

    pub fn destroy(&self, overlay: &vr::Overlay) {
        let _ = overlay.destroy(self.handle);
    }

    /// Where it is this turn: for `offer` (none hides it), before the eyes at `head` when the offer came. Returns it as a target.
    /// `offer`: the id of what it asks (ids of both kinds kept apart by the caller).
    pub fn place(&mut self, overlay: &vr::Overlay, offer: Option<u64>, head: Option<&Matrix34>) -> Option<Quad> {
        let Some(id) = offer.filter(|id| self.pressed != Some(*id)) else {
            self.placed = None;
            self.evidence = Evidence::default();
            if self.visible {
                let _ = overlay.hide(self.handle);
                self.visible = false;
            }
            return None;
        };
        if self.placed.is_none_or(|(placed, _)| placed != id) {
            // A new offer: where the eyes look now (a word heard again moves it there again), whole at once.
            let (distance, below, right) = self.spot;
            let at = space::before_eyes_aside(head?, distance, below, right);
            if self.alpha != 1.0 && overlay.set_alpha(self.handle, 1.0).is_ok() {
                self.alpha = 1.0;
            }
            self.evidence = Evidence { placed: overlay.set_transform_absolute(self.handle, &at).is_ok(), ..Evidence::default() };
            self.placed = Some((id, at));
            self.drawn = None;
            self.drawn_with = None;
        } else if !self.evidence.placed {
            // Refused before: asked again each turn.
            let (_, at) = self.placed?;
            self.evidence.placed = overlay.set_transform_absolute(self.handle, &at).is_ok();
        }
        let (_, at) = self.placed?;
        if !self.visible {
            if overlay.set_alpha(self.handle, 1.0).is_ok() {
                self.alpha = 1.0;
            }
            // Refused: not visible, tried again next turn.
            self.visible = overlay.show(self.handle).is_ok();
            self.evidence.shown = self.visible;
        }
        Some(Quad { transform: at, width: self.width_m, height: self.height_m() })
    }

    /// Whether it shows the offer `id` now: only when SteamVR took its place, its visibility and its picture.
    pub fn showing(&self, id: u64) -> bool {
        self.visible && self.placed.is_some_and(|(placed, _)| placed == id) && self.evidence.confirms()
    }

    /// The band's picture that shows is of what it offers now (it may be pointed at and pressed).
    pub fn current(&self) -> bool {
        self.presented.current()
    }

    /// How much of it shows (the world notice's fade, [`crate::world_notice::opacity`]): SteamVR's alpha, said only when it
    /// changed by a step that shows (no new picture for it).
    pub fn fade(&mut self, overlay: &vr::Overlay, alpha: f32) {
        let alpha = (alpha.clamp(0.0, 1.0) * 64.0).round() / 64.0;
        if self.visible && alpha != self.alpha && overlay.set_alpha(self.handle, alpha).is_ok() {
            self.alpha = alpha;
        }
    }

    /// Pressed: it goes now.
    pub fn pressed(&mut self, overlay: &vr::Overlay, id: u64) {
        self.pressed = Some(id);
        self.place(overlay, None, None);
    }

    /// Its picture, painted when what it shows changed: lit under a laser (`hover`), `left` of its time.
    pub(in crate::overlay) fn paint(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, face: Face, hover: bool, left: f32) {
        if !self.visible {
            return;
        }
        let look = (face, hover, orbiters_i18n::language(), (left.clamp(0.0, 1.0) * STEPS).ceil() as u32);
        if self.drawn == Some(look) {
            return;
        }
        let first = self.drawn.is_none();
        self.drawn_with = None;
        for _ in 0..if first { 2 } else { 1 } {
            self.canvas.paint(0.0, |p| match face {
                Face::Talk => paint_vr(p, hover, left),
                Face::KeepOpen { seconds } => crate::window_rest::paint_band(p, hover, left, seconds),
                Face::World { earns_one, on } => crate::world_notice::paint_vr(p, earns_one, on),
                // Painted with its card ([`Prompt::paint_with`]).
                Face::Gun { .. } => {}
            });
        }
        let px = self.px;
        let changed = if first { Some([0, 0, px[0], px[1]]) } else { self.canvas.damage() };
        if let Some(changed) = changed {
            if !self.presented.hand_over(Some(changed), std::time::Instant::now(), |r| presenter.show_part(overlay, self.handle, self.canvas.pixels(), px[0], px[1], r)) {
                // Refused: the band does not show this picture; painted whole again next turn.
                self.evidence.pictured = false;
                self.drawn = None;
                return;
            }
        }
        // Its first picture taken (a later one only changes it), the band shows its offer.
        self.evidence.pictured |= first;
        self.drawn = Some(look);
    }

    /// A face of the shared 3D view's (`crate::shared_view_ui`), painted by `draw` when `look` (a digest of what it shows:
    /// its words, the pill under a laser, the language, the step of time left) changed.
    pub(in crate::overlay) fn paint_with(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, look: u64, draw: impl Fn(&egui::Painter)) {
        if !self.visible || self.drawn_with == Some(look) {
            return;
        }
        let first = self.drawn_with.is_none();
        self.drawn = None;
        for _ in 0..if first { 2 } else { 1 } {
            self.canvas.paint(0.0, |p| draw(p));
        }
        // At the band's size now (the caller fits it to "Talk with N4N0?"'s first: [`Self::fit`]).
        let px = self.px;
        let changed = if first { Some([0, 0, px[0], px[1]]) } else { self.canvas.damage() };
        if let Some(changed) = changed {
            // Refused: this face is not shown; painted whole again next turn (as the offers' own picture). It is never
            // the countdown's evidence: that is only the countdown's own picture.
            if !self.presented.hand_over(Some(changed), std::time::Instant::now(), |r| presenter.show_part(overlay, self.handle, self.canvas.pixels(), px[0], px[1], r)) {
                self.drawn_with = None;
                return;
            }
        }
        self.drawn_with = Some(look);
    }
}

/// A digest of what a face shows, for [`Prompt::paint_with`].
pub fn look_of(parts: impl std::hash::Hash) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// verify2, SWEEP-7: each of SteamVR's refusals (place, show, picture) means the band does not show its offer.
    #[test]
    fn the_band_shows_its_offer_only_with_every_call_taken() {
        let all = Evidence { placed: true, shown: true, pictured: true };
        assert!(all.confirms());
        assert!(!Evidence { placed: false, ..all }.confirms(), "place refused");
        assert!(!Evidence { shown: false, ..all }.confirms(), "show refused");
        assert!(!Evidence { pictured: false, ..all }.confirms(), "picture refused");
        assert!(!Evidence::default().confirms(), "a new offer: nothing confirmed yet");
    }

    /// A head at `at` facing `yaw` degrees from -Z (level).
    fn head(yaw: f32, at: space::V3) -> Matrix34 {
        let (s, c) = yaw.to_radians().sin_cos();
        space::from_axes([c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c], at)
    }

    /// Request20: the world notice comes low and to the right of where the head faces, whatever way it faces: about 17°
    /// right and 19° down, its far corner well inside a headset's view (under 30° both ways), facing the eyes; every other
    /// face still comes straight ahead.
    #[test]
    fn the_world_notice_comes_low_and_right_of_the_gaze() {
        let face = Face::World { earns_one: false, on: None };
        let (distance, below, right) = face.spot();
        let (px, width) = face.size();
        let height = width * px[1] as f32 / px[0] as f32;
        for (yaw, at) in [(0.0, [0.0, 1.6, 0.0]), (40.0, [1.0, 1.2, -2.0]), (135.0, [-0.5, 1.7, 0.4]), (-90.0, [0.0, 0.9, 0.0])] {
            let h = head(yaw, at);
            let placed = space::before_eyes_aside(&h, distance, below, right);
            let d = space::sub(space::position(&placed), space::position(&h));
            let (x, y, ahead) = (space::dot(d, space::axis(&h, 0)), space::dot(d, space::axis(&h, 1)), -space::dot(d, space::axis(&h, 2)));
            assert!((ahead - distance).abs() < 1e-4 && (x - right).abs() < 1e-4 && (y + below).abs() < 1e-4, "yaw {yaw}: {x} {y} {ahead}");
            let (across, down) = ((x / ahead).atan().to_degrees(), (-y / ahead).atan().to_degrees());
            assert!((12.0..=24.0).contains(&across) && (12.0..=24.0).contains(&down), "yaw {yaw}: {across}° right, {down}° down");
            let (far_x, far_y) = (((x + width / 2.0) / ahead).atan().to_degrees(), ((-y + height / 2.0) / ahead).atan().to_degrees());
            assert!(far_x < 30.0 && far_y < 30.0, "yaw {yaw}: its far corner at {far_x}°, {far_y}°");
            let to_eyes = space::unit(space::scale(d, -1.0)).unwrap();
            assert!(space::dot(space::axis(&placed, 2), to_eyes) > 0.99, "yaw {yaw}: it faces the eyes");
        }
        assert_eq!(Face::Talk.spot(), (DISTANCE_M, BELOW_M, 0.0), "the other faces: straight ahead, as before");
        assert_eq!(Face::KeepOpen { seconds: 3 }.spot(), (DISTANCE_M, BELOW_M, 0.0));
    }

    #[test]
    fn its_face_paints_in_every_language() {
        let mut canvas = Canvas::new(VR_PX[0], VR_PX[1]);
        for language in orbiters_i18n::Language::TRANSLATED {
            orbiters_i18n::in_language(language, || {
                for _ in 0..2 {
                    canvas.paint(0.0, |p| paint_vr(p, true, 0.5));
                }
                assert!(canvas.pixels().iter().any(|&b| b != 0), "{language:?}: something was painted");
                for _ in 0..2 {
                    canvas.paint(0.0, |p| crate::window_rest::paint_band(p, false, 0.5, 4));
                }
            });
            assert!(canvas.pixels().iter().any(|&b| b != 0), "{language:?}: something was painted");
            let (px, _) = Face::World { earns_one: true, on: None }.size();
            let mut world = Canvas::new(px[0], px[1]);
            orbiters_i18n::in_language(language, || {
                for _ in 0..2 {
                    world.paint(0.0, |p| crate::world_notice::paint_vr(p, true, Some(crate::world_notice::Part::Close)));
                }
            });
            assert!(world.pixels().iter().any(|&b| b != 0), "{language:?}: the world notice was painted");
        }
    }
}
