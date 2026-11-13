//! A translation drawn as the world's own text, the way Word Lens did it: the original's place filled with the colour
//! around it (its edges fading into the surface, no frame), the translation in the original's text colour, as large as
//! the original letters and fitted into its box: smaller letters first, then a longer box, then two lines.

use crate::track::Colours;

/// A translation ready for its overlay: premultiplied RGBA, and the size its quad takes in the room (a box made longer
/// or taller than the original's when the translation needed room), metres.
#[derive(Clone, Debug, PartialEq)]
pub struct Painted {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub quad_width: f32,
    pub quad_height: f32,
}

/// Pixels across the original box's height: enough for crisp letters at the sizes signs are read at.
const BOX_PIXELS: f32 = 72.0;
/// The original letters' height against their box's: a run's box is [`crate::runs::RUN_HEIGHT`] letters high.
const LETTERS_IN_BOX: f32 = 1.0 / crate::runs::RUN_HEIGHT;
/// The smallest letters against the original's, before the box grows.
const SMALLEST: f32 = 0.62;
/// How much longer than the original the box may grow before the translation takes two lines.
const LONGER: f32 = 1.7;
/// Widest texture.
const MOST_PIXELS: f32 = 2048.0;

pub struct Painter {
    ctx: egui::Context,
}

impl Default for Painter {
    fn default() -> Self {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        // egui has no fonts to measure with before a first frame. Its atlas goes to each picture's painter whole
        // ([`Painter::draw`]), so what this frame changed is not needed.
        let mut first = ctx.run_ui(egui::RawInput::default(), |_| {});
        first.textures_delta.clear();
        Self { ctx }
    }
}

/// The colour to write in: the original's text colour, unless it would not read on the background (then black or white).
pub fn text_colour(c: Colours) -> [u8; 3] {
    let lum = |c: [u8; 3]| {
        let f = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2])
    };
    let contrast = |a: [u8; 3], b: [u8; 3]| {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    };
    if contrast(c.text, c.background) >= 3.0 {
        c.text
    } else if contrast([0; 3], c.background) > contrast([255; 3], c.background) {
        [16, 16, 16]
    } else {
        [250, 250, 250]
    }
}

impl Painter {
    /// `text` over a box of `width` x `height` metres coloured as `colours` say.
    pub fn paint(&mut self, text: &str, colours: Colours, width: f32, height: f32) -> Painted {
        let px_per_m = BOX_PIXELS / height.max(1e-4);
        let (box_w, box_h) = ((width * px_per_m).max(8.0), BOX_PIXELS);
        let ink = text_colour(colours);
        let ink32 = egui::Color32::from_rgb(ink[0], ink[1], ink[2]);
        // Letters as tall as the original's, never taller: a font's size is about its CJK letters' height, Latin
        // capitals are seven tenths of it.
        let start = box_h * LETTERS_IN_BOX;
        let family = orbiters_ui::fonts::family(500.0);
        let layout = |ctx: &egui::Context, size: f32, wrap: f32| ctx.fonts_mut(|f| f.layout(text.to_string(), egui::FontId::new(size, family.clone()), ink32, wrap));
        // Twice: letters of a script not drawn before open their font on the first layout.
        let _ = layout(&self.ctx, start, f32::INFINITY);
        let margin = box_h * 0.18;
        let (mut size, mut quad_w, mut quad_h) = (start, box_w, box_h);
        let mut wrap = f32::INFINITY;
        let fits = |g: &egui::Galley, w: f32, h: f32| g.size().x <= w - 2.0 * margin * 0.5 && g.size().y <= h;
        let mut galley = layout(&self.ctx, size, wrap);
        while !fits(&galley, quad_w, quad_h) && size > start * SMALLEST {
            size *= 0.94;
            galley = layout(&self.ctx, size, wrap);
        }
        if !fits(&galley, quad_w, quad_h) {
            let needed = galley.size().x + margin;
            if needed <= box_w * LONGER {
                quad_w = needed;
            } else {
                // Two lines (or more), the box as long as it may grow and taller.
                quad_w = (box_w * LONGER).min(MOST_PIXELS);
                wrap = quad_w - margin;
                galley = layout(&self.ctx, size, wrap);
                quad_h = quad_h.max(galley.size().y + margin);
            }
        }
        let (w, h) = (quad_w.min(MOST_PIXELS).ceil() as u32, quad_h.ceil() as u32);
        let rgba = self.draw(w, h, colours.background, &galley, ink32);
        Painted { width: w, height: h, rgba, quad_width: w as f32 / px_per_m, quad_height: h as f32 / px_per_m }
    }

    fn draw(&mut self, w: u32, h: u32, background: [u8; 3], galley: &std::sync::Arc<egui::Galley>, ink: egui::Color32) -> Vec<u8> {
        let mut painter = orbiters_ui::software::Software::new(w, h);
        let size = egui::vec2(w as f32, h as f32);
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() };
        let galley = galley.clone();
        let mut full = self.ctx.run_ui(input, |ui| {
            let p = ui.painter();
            // From the line's start, as the original was written (the box's padding before it), centred across.
            let at = egui::pos2(((size.y * 0.36).min((size.x - galley.size().x) / 2.0)).max(0.0), (size.y - galley.size().y) / 2.0);
            p.galley(at, galley.clone(), ink);
        });
        // A new painter has none of the fonts' atlas: the whole of it first (egui sends only what changed since its last
        // frame), then what this frame changed.
        let atlas = self.ctx.fonts(|f| f.image());
        let mut whole = egui::epaint::textures::TexturesDelta::default();
        whole.set.insert(egui::TextureId::default(), std::iter::once(egui::epaint::ImageDelta::full(atlas, egui::TextureOptions::LINEAR)).collect());
        painter.set_textures(&mut whole);
        painter.set_textures(&mut full.textures_delta);
        let primitives = self.ctx.tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
        let text = painter.paint(&primitives, full.pixels_per_point, egui::Color32::TRANSPARENT).to_vec();
        painter.free_textures(&mut full.textures_delta);
        // The cover: the background colour, its edges fading over a few pixels into the surface, the text over it.
        let feather = (h as f32 * 0.09).max(2.0);
        let mut out = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let edge = (x as f32 + 0.5).min(w as f32 - x as f32 - 0.5).min(y as f32 + 0.5).min(h as f32 - y as f32 - 0.5);
                let a = (edge / feather).clamp(0.0, 1.0);
                let a = a * a * (3.0 - 2.0 * a);
                let i = ((y * w + x) * 4) as usize;
                let ta = text[i + 3] as f32 / 255.0;
                for c in 0..3 {
                    // Premultiplied: text over the background, then the whole at the cover's alpha.
                    let v = text[i + c] as f32 + background[c] as f32 * (1.0 - ta);
                    out[i + c] = (v * a).round().clamp(0.0, 255.0) as u8;
                }
                out[i + 3] = (a * 255.0).round() as u8;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_colours_turn_black_or_white() {
        assert_eq!(text_colour(Colours { background: [240, 240, 240], text: [20, 30, 40] }), [20, 30, 40]);
        assert_eq!(text_colour(Colours { background: [240, 240, 240], text: [220, 220, 220] }), [16, 16, 16]);
        assert_eq!(text_colour(Colours { background: [20, 20, 30], text: [40, 40, 40] }), [250, 250, 250]);
    }

    #[test]
    fn a_long_translation_grows_its_box_and_short_ones_keep_it() {
        let mut p = Painter::default();
        let c = Colours { background: [235, 228, 210], text: [20, 20, 20] };
        let short = p.paint("Exit", c, 0.3, 0.05);
        assert!((short.quad_width - 0.3).abs() < 0.01 && (short.quad_height - 0.05).abs() < 0.001, "{short:?}");
        assert_eq!(short.rgba.len() as u32, short.width * short.height * 4);
        let long = p.paint("Authorized personnel only beyond this point, thank you", c, 0.3, 0.05);
        assert!(long.quad_width > 0.3 && long.quad_width <= 0.3 * LONGER + 0.01, "{}", long.quad_width);
        assert!(long.quad_height > 0.05, "two lines");
        // The middle is opaque background or ink, the edge fades out.
        let mid = ((short.height / 2 * short.width + 2) * 4) as usize;
        assert!(short.rgba[mid + 3] < 200, "edges fade");
        let centre = ((short.height / 2 * short.width + short.width / 2) * 4 + 3) as usize;
        assert_eq!(short.rgba[centre], 255);
    }
}
