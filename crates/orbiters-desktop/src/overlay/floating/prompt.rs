//! The band before the eyes in the play space: "Talk with N4N0?" ([`crate::wake_offer`], N4N0-11) while the wake word
//! heard during a call has it up, else "No 3D picture of this world yet" ([`crate::world_notice`], REQUEST13). A little
//! below the eyes' line, where they looked when it came (it stays there: a laser can aim at it, and it does not swim with
//! the head), pressed as the strip is (either hand's laser on it, a pull of its trigger). It shows for its few seconds
//! only, its line of time left shrinking, and goes by itself.

use super::space::{self, Quad};
use crate::controller::explorer::WorldNotice;
use crate::overlay::{Canvas, Presenter};
use orbiters_n4n0::status::WakeOffer;
use orbiters_openvr::{self as vr, overlay_flag, Matrix34};

/// How far before the eyes, and how far under their line, it comes.
const DISTANCE_M: f32 = 0.6;
const BELOW_M: f32 = 0.16;
/// Over the panel and its bar, under the lasers, as the watch.
const SORT_ORDER: u32 = 12;
/// Steps of its line of time left that are painted (about ten a second over six seconds).
const STEPS: f32 = 60.0;

/// What the band shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Face {
    Talk(WakeOffer),
    World(WorldNotice),
}

impl Face {
    /// Which one it is, as a key (a Talk offer and a notice may have the same number).
    pub fn key(&self) -> u64 {
        match self {
            Face::Talk(o) => o.id << 1,
            Face::World(n) => (n.id << 1) | 1,
        }
    }
    fn px(&self) -> [u32; 2] {
        match self {
            Face::Talk(_) => crate::wake_offer::VR_PX,
            Face::World(_) => crate::world_notice::VR_PX,
        }
    }
    fn width_m(&self) -> f32 {
        match self {
            Face::Talk(_) => crate::wake_offer::VR_WIDTH_M,
            Face::World(_) => crate::world_notice::VR_WIDTH_M,
        }
    }
    fn paint(&self, p: &egui::Painter, hover: bool, left: f32) {
        match self {
            Face::Talk(_) => crate::wake_offer::paint_vr(p, hover, left),
            Face::World(n) => crate::world_notice::paint_vr(p, n, hover, left),
        }
    }
}

pub fn height_m(face: &Face) -> f32 {
    let px = face.px();
    face.width_m() * px[1] as f32 / px[0] as f32
}

pub struct Prompt {
    handle: vr::OverlayHandle,
    canvas: Canvas,
    /// The canvas's size in pixels.
    px: [u32; 2],
    /// The face it shows (its key), and where it was put for it.
    placed: Option<(u64, Matrix34)>,
    /// The face pressed here: it goes at once, before the app's word that it was taken.
    pressed: Option<u64>,
    visible: bool,
    /// What its picture shows: the face, hovered, the language, the step of time left.
    drawn: Option<(u64, bool, orbiters_i18n::Language, u32)>,
}

impl Prompt {
    pub fn new(overlay: &vr::Overlay, key: &str) -> anyhow::Result<Self> {
        let handle = crate::overlay::slots::create(overlay, &format!("{key}.wakeoffer"), "Orbiters talk with N4N0").map_err(|c| anyhow::anyhow!("talk with N4N0: SteamVR overlay error {c}"))?; // i18n: not shown (a name for SteamVR)
        let _ = overlay.set_width_in_meters(handle, crate::wake_offer::VR_WIDTH_M);
        let _ = overlay.set_flag(handle, overlay_flag::IS_PREMULTIPLIED, true);
        let _ = overlay.set_sort_order(handle, SORT_ORDER);
        let px = crate::wake_offer::VR_PX;
        Ok(Self { handle, canvas: Canvas::new(px[0], px[1]), px, placed: None, pressed: None, visible: false, drawn: None })
    }

    pub fn destroy(&self, overlay: &vr::Overlay) {
        let _ = overlay.destroy(self.handle);
    }

    /// Where it is this turn: for `face` (none hides it), before the eyes at `head` when it came. Returns it as a target.
    pub fn place(&mut self, overlay: &vr::Overlay, face: Option<&Face>, head: Option<&Matrix34>) -> Option<Quad> {
        let Some(face) = face.filter(|f| self.pressed != Some(f.key())) else {
            self.placed = None;
            if self.visible {
                let _ = overlay.hide(self.handle);
                self.visible = false;
            }
            return None;
        };
        if self.placed.is_none_or(|(key, _)| key != face.key()) {
            // A new face: where the eyes look now (a word heard again moves it there again), at its own size.
            let at = space::before_eyes(head?, DISTANCE_M, BELOW_M);
            let _ = overlay.set_transform_absolute(self.handle, &at);
            let _ = overlay.set_width_in_meters(self.handle, face.width_m());
            let px = face.px();
            if self.px != px {
                self.canvas = Canvas::new(px[0], px[1]);
                self.px = px;
            }
            self.placed = Some((face.key(), at));
            self.drawn = None;
        }
        let (_, at) = self.placed?;
        if !self.visible {
            let _ = overlay.set_alpha(self.handle, 1.0);
            let _ = overlay.show(self.handle);
            self.visible = true;
        }
        Some(Quad { transform: at, width: face.width_m(), height: height_m(face) })
    }

    /// Pressed: it goes now.
    pub fn pressed(&mut self, overlay: &vr::Overlay, face: &Face) {
        self.pressed = Some(face.key());
        self.place(overlay, None, None);
    }

    /// Its picture, painted when what it shows changed: lit under a laser (`hover`), `left` of its time.
    pub(in crate::overlay) fn paint(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, face: &Face, hover: bool, left: f32) {
        if !self.visible {
            return;
        }
        let look = (face.key(), hover, orbiters_i18n::language(), (left.clamp(0.0, 1.0) * STEPS).ceil() as u32);
        if self.drawn == Some(look) {
            return;
        }
        let first = self.drawn.is_none_or(|d| d.0 != look.0);
        for _ in 0..if first { 2 } else { 1 } {
            self.canvas.paint(0.0, |p| face.paint(p, hover, left));
        }
        let px = face.px();
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
    fn its_faces_paint_in_every_language() {
        let notice = WorldNotice { id: 3, world_id: "wrld_x".into(), world_name: String::new(), earns: Some(1.0), until_ms: 10, for_ms: 10 };
        for face in [Face::Talk(WakeOffer { id: 3, until_ms: 10, for_ms: 10 }), Face::World(notice)] {
            let px = face.px();
            let mut canvas = Canvas::new(px[0], px[1]);
            for language in orbiters_i18n::Language::TRANSLATED {
                orbiters_i18n::in_language(language, || {
                    for _ in 0..2 {
                        canvas.paint(0.0, |p| face.paint(p, true, 0.5));
                    }
                });
                assert!(canvas.pixels().iter().any(|&b| b != 0), "{language:?}: something was painted");
            }
        }
        let talk = Face::Talk(WakeOffer { id: 3, until_ms: 10, for_ms: 10 });
        assert_ne!(talk.key(), Face::World(WorldNotice { id: 3, world_id: String::new(), world_name: String::new(), earns: None, until_ms: 0, for_ms: 1 }).key(), "same number, other faces");
    }
}
