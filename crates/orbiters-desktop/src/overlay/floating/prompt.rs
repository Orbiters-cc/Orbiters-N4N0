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
}

/// What the band asks: "Talk with N4N0?" (`crate::wake_offer`), or "Orbiters - N4N0 will minimize to save memory" with
/// Keep open and its seconds left (`crate::window_rest`). One band, one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Talk,
    KeepOpen { seconds: u64 },
    /// "No 3D picture of this world yet" with "Take a 3D picture" (`crate::world_notice`, REQUEST13); whether it says the
    /// point a picture there earns.
    World { earns_one: bool },
}

impl Face {
    /// Its picture's size and width: the world notice has a row for its words and one for its action.
    pub fn size(self) -> ([u32; 2], f32) {
        match self {
            Face::World { .. } => (crate::world_notice::VR_PX, crate::world_notice::VR_WIDTH_M),
            _ => (VR_PX, VR_WIDTH_M),
        }
    }
}

impl Prompt {
    pub fn new(overlay: &vr::Overlay, key: &str) -> anyhow::Result<Self> {
        let handle = crate::overlay::slots::create(overlay, &format!("{key}.wakeoffer"), "Orbiters talk with N4N0").map_err(|c| anyhow::anyhow!("talk with N4N0: SteamVR overlay error {c}"))?; // i18n: not shown (a name for SteamVR)
        let _ = overlay.set_width_in_meters(handle, VR_WIDTH_M);
        let _ = overlay.set_flag(handle, overlay_flag::IS_PREMULTIPLIED, true);
        let _ = overlay.set_sort_order(handle, SORT_ORDER);
        Ok(Self { handle, canvas: Canvas::new(VR_PX[0], VR_PX[1]), px: VR_PX, width_m: VR_WIDTH_M, placed: None, pressed: None, visible: false, drawn: None })
    }

    /// At the size `face` takes (before [`Prompt::place`]): another size is a new picture, placed again.
    pub fn fit(&mut self, overlay: &vr::Overlay, face: Face) {
        let (px, width_m) = face.size();
        if px != self.px || width_m != self.width_m {
            self.canvas = Canvas::new(px[0], px[1]);
            self.px = px;
            self.width_m = width_m;
            let _ = overlay.set_width_in_meters(self.handle, width_m);
            self.placed = None;
            self.drawn = None;
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
            if self.visible {
                let _ = overlay.hide(self.handle);
                self.visible = false;
            }
            return None;
        };
        if self.placed.is_none_or(|(placed, _)| placed != id) {
            // A new offer: where the eyes look now (a word heard again moves it there again).
            let at = space::before_eyes(head?, DISTANCE_M, BELOW_M);
            let _ = overlay.set_transform_absolute(self.handle, &at);
            self.placed = Some((id, at));
            self.drawn = None;
        }
        let (_, at) = self.placed?;
        if !self.visible {
            let _ = overlay.set_alpha(self.handle, 1.0);
            let _ = overlay.show(self.handle);
            self.visible = true;
        }
        Some(Quad { transform: at, width: self.width_m, height: self.height_m() })
    }

    /// Whether it shows the offer `id` now.
    pub fn showing(&self, id: u64) -> bool {
        self.visible && self.placed.is_some_and(|(placed, _)| placed == id)
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
        for _ in 0..if first { 2 } else { 1 } {
            self.canvas.paint(0.0, |p| match face {
                Face::Talk => paint_vr(p, hover, left),
                Face::KeepOpen { seconds } => crate::window_rest::paint_band(p, hover, left, seconds),
                Face::World { earns_one } => crate::world_notice::paint_vr(p, earns_one, hover, left),
            });
        }
        let px = self.px;
        let changed = if first { Some([0, 0, px[0], px[1]]) } else { self.canvas.damage() };
        if let Some(changed) = changed {
            presenter.show_part(overlay, self.handle, self.canvas.pixels(), px[0], px[1], changed);
        }
        self.drawn = Some(look);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let (px, _) = Face::World { earns_one: true }.size();
            let mut world = Canvas::new(px[0], px[1]);
            orbiters_i18n::in_language(language, || {
                for _ in 0..2 {
                    world.paint(0.0, |p| crate::world_notice::paint_vr(p, true, true, 0.5));
                }
            });
            assert!(world.pixels().iter().any(|&b| b != 0), "{language:?}: the world notice was painted");
        }
    }
}
