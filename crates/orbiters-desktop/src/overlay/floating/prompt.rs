//! "Talk with N4N0?" in the play space ([`crate::wake_offer`], N4N0-11): while the wake word heard during a call has it up,
//! a band before the eyes, a little below their line, where they looked when it came (it stays there: a laser can aim at
//! it, and it does not swim with the head), pressed as the strip is (either hand's laser on it, a pull of its trigger).
//! It shows for the offer's few seconds only, its line of time left shrinking, and goes by itself.

use super::space::{self, Quad};
use crate::overlay::{Canvas, Presenter};
use crate::wake_offer::{paint_vr, VR_PX, VR_WIDTH_M};
use orbiters_n4n0::status::WakeOffer;
use orbiters_openvr::{self as vr, overlay_flag, Matrix34};

/// How far before the eyes, and how far under their line, it comes.
const DISTANCE_M: f32 = 0.6;
const BELOW_M: f32 = 0.16;
/// Over the panel and its bar, under the lasers, as the watch.
const SORT_ORDER: u32 = 12;
/// Steps of its line of time left that are painted (about ten a second over six seconds).
const STEPS: f32 = 60.0;

pub fn height_m() -> f32 {
    VR_WIDTH_M * VR_PX[1] as f32 / VR_PX[0] as f32
}

pub struct Prompt {
    handle: vr::OverlayHandle,
    canvas: Canvas,
    /// The offer it shows, and where it was put for it.
    placed: Option<(u64, Matrix34)>,
    /// The offer pressed here: it goes at once, before the app's word that it was taken.
    pressed: Option<u64>,
    visible: bool,
    /// What its picture shows: hovered, the language, the step of time left.
    drawn: Option<(bool, orbiters_i18n::Language, u32)>,
}

impl Prompt {
    pub fn new(overlay: &vr::Overlay, key: &str) -> anyhow::Result<Self> {
        let handle = crate::overlay::slots::create(overlay, &format!("{key}.wakeoffer"), "Orbiters talk with N4N0").map_err(|c| anyhow::anyhow!("talk with N4N0: SteamVR overlay error {c}"))?; // i18n: not shown (a name for SteamVR)
        let _ = overlay.set_width_in_meters(handle, VR_WIDTH_M);
        let _ = overlay.set_flag(handle, overlay_flag::IS_PREMULTIPLIED, true);
        let _ = overlay.set_sort_order(handle, SORT_ORDER);
        Ok(Self { handle, canvas: Canvas::new(VR_PX[0], VR_PX[1]), placed: None, pressed: None, visible: false, drawn: None })
    }

    pub fn destroy(&self, overlay: &vr::Overlay) {
        let _ = overlay.destroy(self.handle);
    }

    /// Where it is this turn: for `offer` (none hides it), before the eyes at `head` when the offer came. Returns it as a target.
    pub fn place(&mut self, overlay: &vr::Overlay, offer: Option<WakeOffer>, head: Option<&Matrix34>) -> Option<Quad> {
        let Some(offer) = offer.filter(|o| self.pressed != Some(o.id)) else {
            self.placed = None;
            if self.visible {
                let _ = overlay.hide(self.handle);
                self.visible = false;
            }
            return None;
        };
        if self.placed.is_none_or(|(id, _)| id != offer.id) {
            // A new offer: where the eyes look now (a word heard again moves it there again).
            let at = space::before_eyes(head?, DISTANCE_M, BELOW_M);
            let _ = overlay.set_transform_absolute(self.handle, &at);
            self.placed = Some((offer.id, at));
            self.drawn = None;
        }
        let (_, at) = self.placed?;
        if !self.visible {
            let _ = overlay.set_alpha(self.handle, 1.0);
            let _ = overlay.show(self.handle);
            self.visible = true;
        }
        Some(Quad { transform: at, width: VR_WIDTH_M, height: height_m() })
    }

    /// Pressed: it goes now.
    pub fn pressed(&mut self, overlay: &vr::Overlay, offer: WakeOffer) {
        self.pressed = Some(offer.id);
        self.place(overlay, None, None);
    }

    /// Its picture, painted when what it shows changed: lit under a laser (`hover`), `left` of its time.
    pub(in crate::overlay) fn paint(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, hover: bool, left: f32) {
        if !self.visible {
            return;
        }
        let look = (hover, orbiters_i18n::language(), (left.clamp(0.0, 1.0) * STEPS).ceil() as u32);
        if self.drawn == Some(look) {
            return;
        }
        let first = self.drawn.is_none();
        for _ in 0..if first { 2 } else { 1 } {
            self.canvas.paint(0.0, |p| paint_vr(p, hover, left));
        }
        let changed = if first { Some([0, 0, VR_PX[0], VR_PX[1]]) } else { self.canvas.damage() };
        if let Some(changed) = changed {
            presenter.show_part(overlay, self.handle, self.canvas.pixels(), VR_PX[0], VR_PX[1], changed);
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
            });
            assert!(canvas.pixels().iter().any(|&b| b != 0), "{language:?}: something was painted");
        }
    }
}
