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
    /// What its picture shows: which face, hovered, the language, the step of time left.
    drawn: Option<(Face, bool, orbiters_i18n::Language, u32)>,
    /// Or a face of the shared 3D view's ([`Self::paint_with`]): a digest of what it shows.
    drawn_with: Option<u64>,
}

/// What the band asks: "Talk with N4N0?" (`crate::wake_offer`), or "Orbiters - N4N0 will minimize to save memory" with
/// Keep open and its seconds left (`crate::window_rest`). One band, one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Talk,
    KeepOpen { seconds: u64 },
}

impl Prompt {
    pub fn new(overlay: &vr::Overlay, key: &str) -> anyhow::Result<Self> {
        let handle = crate::overlay::slots::create(overlay, &format!("{key}.wakeoffer"), "Orbiters talk with N4N0").map_err(|c| anyhow::anyhow!("talk with N4N0: SteamVR overlay error {c}"))?; // i18n: not shown (a name for SteamVR)
        let _ = overlay.set_width_in_meters(handle, VR_WIDTH_M);
        let _ = overlay.set_flag(handle, overlay_flag::IS_PREMULTIPLIED, true);
        let _ = overlay.set_sort_order(handle, SORT_ORDER);
        Ok(Self { handle, canvas: Canvas::new(VR_PX[0], VR_PX[1]), placed: None, pressed: None, visible: false, drawn: None, drawn_with: None })
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
            self.drawn_with = None;
        }
        let (_, at) = self.placed?;
        if !self.visible {
            let _ = overlay.set_alpha(self.handle, 1.0);
            let _ = overlay.show(self.handle);
            self.visible = true;
        }
        Some(Quad { transform: at, width: VR_WIDTH_M, height: height_m() })
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
        self.drawn_with = None;
        for _ in 0..if first { 2 } else { 1 } {
            self.canvas.paint(0.0, |p| match face {
                Face::Talk => paint_vr(p, hover, left),
                Face::KeepOpen { seconds } => crate::window_rest::paint_band(p, hover, left, seconds),
            });
        }
        let changed = if first { Some([0, 0, VR_PX[0], VR_PX[1]]) } else { self.canvas.damage() };
        if let Some(changed) = changed {
            presenter.show_part(overlay, self.handle, self.canvas.pixels(), VR_PX[0], VR_PX[1], changed);
        }
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
        let changed = if first { Some([0, 0, VR_PX[0], VR_PX[1]]) } else { self.canvas.damage() };
        if let Some(changed) = changed {
            presenter.show_part(overlay, self.handle, self.canvas.pixels(), VR_PX[0], VR_PX[1], changed);
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
        }
    }
}
