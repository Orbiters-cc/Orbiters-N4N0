//! N4N0's face, its logo as an icon: two side "wings" and the arc over them, as the companion's own face is drawn. Three
//! filled outlines in a 986 × 872 view box, rasterised with tiny-skia at the exact pixel size so they stay crisp at icon
//! sizes, in the colour asked (the theme's icon colour where it stands for a Phosphor glyph: [`crate::icons::N4N0`] draws
//! it through [`crate::widgets::icon`]).

use egui::{pos2, vec2, Color32, ColorImage, Context, Id, Painter, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

/// The face's view box.
pub const VIEW: Vec2 = Vec2::new(986.0, 872.0);

/// A piece of an outline: a line or a cubic curve to its last point.
#[derive(Clone, Copy)]
enum Step {
    Line(f32, f32),
    Cubic([f32; 6]),
}

/// The outlines, each from its first point: the right wing, the left wing, the arc over them.
const OUTLINES: [((f32, f32), &[Step]); 3] = [
    (
        (969.949, 331.795),
        &[
            Step::Line(572.95, 410.295),
            Step::Line(846.449, 455.295),
            Step::Cubic([854.283, 474.295, 865.249, 566.595, 846.449, 659.795]),
            Step::Cubic([827.649, 752.995, 790.116, 824.629, 765.449, 871.296]),
            Step::Cubic([823.949, 827.796, 931.397, 700.253, 956.448, 619.795]),
            Step::Cubic([991.01, 508.795, 992.282, 403.962, 969.949, 331.795]),
        ],
    ),
    (
        (412.447, 410.295),
        &[
            Step::Line(15.4477, 331.795),
            Step::Cubic([-6.88567, 403.962, -5.61351, 508.795, 28.9482, 619.795]),
            Step::Cubic([53.9999, 700.253, 161.448, 827.795, 219.948, 871.295]),
            Step::Cubic([195.281, 824.629, 157.747, 752.995, 138.947, 659.795]),
            Step::Cubic([120.147, 566.595, 131.114, 474.295, 138.947, 455.295]),
            Step::Line(412.447, 410.295),
        ],
    ),
    (
        (138.947, 232.404),
        &[
            Step::Cubic([166.037, 151.552, 267.673, -0.999, 489.532, 0.511129]),
            Step::Cubic([711.39, 2.02126, 819.918, 153.077, 846.449, 232.404]),
            Step::Cubic([769.649, 197.571, 662.868, 79.4406, 489.532, 79.4406]),
            Step::Cubic([316.195, 79.4406, 216.307, 197.571, 138.947, 232.404]),
        ],
    ),
];

fn face_path() -> &'static tiny_skia::Path {
    static PATH: std::sync::OnceLock<tiny_skia::Path> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let mut pb = PathBuilder::new();
        for ((x, y), steps) in OUTLINES {
            pb.move_to(x, y);
            for step in steps {
                match *step {
                    Step::Line(x, y) => pb.line_to(x, y),
                    Step::Cubic([x1, y1, x2, y2, x, y]) => pb.cubic_to(x1, y1, x2, y2, x, y),
                }
            }
            pb.close();
        }
        pb.finish().expect("N4N0's face")
    })
}

/// The face alone, `w` × `h` pixels (its view box stretched to them), in `color`.
pub fn rasterize(w: u32, h: u32, color: Color32) -> ColorImage {
    let mut pixmap = Pixmap::new(w.max(1), h.max(1)).expect("pixmap");
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r(), color.g(), color.b(), color.a());
    paint.anti_alias = true;
    pixmap.fill_path(face_path(), &paint, FillRule::Winding, Transform::from_scale(w as f32 / VIEW.x, h as f32 / VIEW.y), None);
    ColorImage::from_rgba_premultiplied([pixmap.width() as usize, pixmap.height() as usize], pixmap.data())
}

/// A texture by its pixel size and colour.
type Entry = (([u32; 2], [u8; 4]), TextureHandle);

/// The textures by pixel size and colour, the most recently drawn first.
#[derive(Clone, Default)]
struct Cache(Vec<Entry>);

/// Sizes kept (the faces on screen at once: the sidebar's, a section's, both hovered and not).
const KEEP: usize = 8;

/// How wide the face is drawn for an icon of `size` (a Phosphor glyph's font size): as wide as the glyphs' drawings.
pub const ICON_WIDTH: f32 = 0.9;

/// The rect the face fills as an icon of `size` centred on `center`.
pub fn icon_rect(center: Pos2, size: f32) -> Rect {
    let w = size * ICON_WIDTH;
    Rect::from_center_size(center, vec2(w, w * VIEW.y / VIEW.x))
}

/// Paints the face as an icon of `size` (a Phosphor glyph's font size) centred on `center`, in `color`.
pub fn icon(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    paint(painter, icon_rect(center, size), color);
}

/// Paints the face into `rect` (keep its 986:872 aspect), in `color`: rasterised at the rect's pixel size, the rect put on
/// whole pixels, so its edges stay sharp.
pub fn paint(painter: &Painter, rect: Rect, color: Color32) {
    if !rect.is_positive() || color.a() == 0 {
        return;
    }
    let ctx: Context = painter.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let size = [(rect.width() * ppp).round().clamp(1.0, 2048.0) as u32, (rect.height() * ppp).round().clamp(1.0, 2048.0) as u32];
    // On whole pixels: the texture's texels fall on the screen's.
    let min = pos2((rect.min.x * ppp).round() / ppp, (rect.min.y * ppp).round() / ppp);
    let shown = Rect::from_min_size(min, vec2(size[0] as f32 / ppp, size[1] as f32 / ppp));
    let key = (size, color.to_array());
    let id = Id::unique("n4n0-face");
    let texture = ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<Cache>(id);
        let i = cache.0.iter().position(|(k, _)| *k == key)?;
        let entry = cache.0.remove(i);
        let t = entry.1.clone();
        cache.0.insert(0, entry);
        Some(t)
    });
    let texture = texture.unwrap_or_else(|| {
        let t = ctx.load_texture(format!("n4n0-face-{}x{}-{:?}", size[0], size[1], key.1), rasterize(size[0], size[1], color), TextureOptions::NEAREST);
        ctx.data_mut(|d| {
            let cache = d.get_temp_mut_or_default::<Cache>(id);
            cache.0.insert(0, (key, t.clone()));
            cache.0.truncate(KEEP);
        });
        t
    });
    painter.image(texture.id(), shown, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_face_has_its_wings_and_arc_and_room_between_them() {
        let image = rasterize(986, 872, Color32::WHITE);
        let alpha = |x: usize, y: usize| image.pixels[y * 986 + x].a();
        // Inside the left wing, the right wing and the arc.
        for (x, y) in [(60, 560), (930, 560), (490, 40), (300, 90)] {
            assert!(alpha(x, y) > 200, "drawn at {x}, {y}");
        }
        // The face's middle, under the arc, between the wings' tips, and the corners: empty.
        for (x, y) in [(493, 500), (493, 200), (493, 800), (5, 5), (980, 860), (600, 600)] {
            assert!(alpha(x, y) < 30, "empty at {x}, {y}");
        }
    }

    #[test]
    fn an_icon_keeps_the_aspect_and_its_size() {
        let r = icon_rect(pos2(100.0, 50.0), 18.0);
        assert!((r.width() - 18.0 * ICON_WIDTH).abs() < 1e-4);
        assert!((r.width() / r.height() - VIEW.x / VIEW.y).abs() < 1e-4);
        assert_eq!(r.center(), pos2(100.0, 50.0));
        // Coloured as asked: the theme's icon colour stays its own.
        let small = rasterize(18, 16, Color32::from_rgb(0, 218, 109));
        let solid = small.pixels.iter().find(|p| p.a() == 255).expect("a solid pixel at 18 pixels");
        assert_eq!((solid.r(), solid.g(), solid.b()), (0, 218, 109));
    }
}
