//! The N4N0 mark: N, 4, N and 0 in a square of four glyphs cut from blocks (straight lines only), rasterised with
//! tiny-skia at the exact pixel size so it stays crisp; and the app's icon built around it (its rounded square, the
//! glow, the mark with its cyan and violet echoes), which `examples/n4n0_icon.rs` writes out as the app's PNGs and ICO.

use crate::anim::{self, ease};
use crate::theme;
use egui::{pos2, vec2, Color32, ColorImage, Context, Id, Painter, Rect, TextureHandle, TextureOptions, Vec2};
use tiny_skia::{FillRule, GradientStop, Paint, PathBuilder, Pixmap, Point, RadialGradient, SpreadMode, Transform};

/// The mark's glyphs as closed outlines in its view box; holes are outlines inside another (filled even-odd).
const SHAPES: &[&[(f32, f32)]] = &[
    // 0, bottom right: a block with two slanted slots.
    &[(513.0, 455.5), (939.5, 455.5), (939.5, 821.5), (513.0, 821.5)],
    &[(857.0, 528.231), (597.0, 528.231), (597.0, 635.45), (857.0, 569.054)],
    &[(597.0, 742.669), (857.0, 742.669), (857.0, 655.862), (597.0, 717.331)],
    // N, bottom left, and its corner.
    &[(0.5, 821.5), (0.5, 455.5), (192.0, 455.5), (407.0, 761.0), (407.0, 821.5), (338.0, 821.5), (161.5, 559.5), (82.5, 559.5), (82.5, 821.5)],
    &[(407.0, 455.5), (407.0, 616.5), (302.5, 455.5)],
    // N, top left, and its corner.
    &[(0.5, 366.5), (0.5, 0.5), (192.0, 0.5), (407.0, 306.0), (407.0, 366.5), (338.0, 366.5), (161.5, 104.5), (82.5, 104.5), (82.5, 366.5)],
    &[(407.0, 0.5), (407.0, 161.5), (302.5, 0.5)],
    // 4, top right, and its counter.
    &[
        (513.0, 366.5),
        (513.0, 138.0),
        (615.5, 0.5),
        (832.0, 0.5),
        (832.0, 245.5),
        (939.5, 245.5),
        (939.5, 326.5),
        (832.0, 326.5),
        (832.0, 366.5),
        (751.5, 366.5),
        (751.5, 326.5),
        (597.0, 326.5),
        (597.0, 366.5),
    ],
    &[(597.0, 245.5), (751.5, 245.5), (751.5, 82.5), (657.5, 82.5), (597.0, 161.5)],
];

/// The mark's view box.
pub const VIEW: Vec2 = Vec2::new(940.0, 822.0);

/// Size of the mark for a given width (keeps its 940:822 aspect).
pub fn size_for_width(width: f32) -> Vec2 {
    vec2(width, width * VIEW.y / VIEW.x)
}

fn mark_path() -> &'static tiny_skia::Path {
    static PATH: std::sync::OnceLock<tiny_skia::Path> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let mut pb = PathBuilder::new();
        for shape in SHAPES {
            pb.move_to(shape[0].0, shape[0].1);
            for &(x, y) in &shape[1..] {
                pb.line_to(x, y);
            }
            pb.close();
        }
        pb.finish().expect("the N4N0 mark")
    })
}

/// Fills the mark into `pixmap` in `color`, its view box mapped by `transform`.
fn fill_mark(pixmap: &mut Pixmap, color: Color32, alpha: f32, transform: Transform) {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r(), color.g(), color.b(), (alpha.clamp(0.0, 1.0) * 255.0).round() as u8);
    paint.anti_alias = true;
    pixmap.fill_path(mark_path(), &paint, FillRule::EvenOdd, transform, None);
}

/// The mark alone, `w` × `h` pixels, in `color`.
pub fn rasterize(w: u32, h: u32, color: Color32) -> ColorImage {
    let mut pixmap = Pixmap::new(w.max(1), h.max(1)).expect("pixmap");
    fill_mark(&mut pixmap, color, 1.0, Transform::from_scale(w as f32 / VIEW.x, h as f32 / VIEW.y));
    ColorImage::from_rgba_premultiplied([pixmap.width() as usize, pixmap.height() as usize], pixmap.data())
}

/// The textures by pixel size and colour, the most recently drawn first.
#[derive(Clone, Default)]
struct Cache(Vec<(([u32; 2], [u8; 4]), TextureHandle)>);

/// Sizes kept (the marks on screen at once, icons in their colours included); a size seen once is let go.
const KEEP: usize = 10;

/// Paints the mark into `rect` in `color`.
pub fn paint(painter: &Painter, rect: Rect, color: Color32) {
    draw(painter, rect, rect, color, Color32::WHITE);
}

/// How wide the mark is drawn for an icon of `size` (a Phosphor glyph's font size): the glyph's whole width, its cuts need it.
pub const ICON_WIDTH: f32 = 1.0;

/// Paints the mark as an icon of `size` (a Phosphor glyph's font size) centred on `center`, in `color` (its opacity
/// through the tint, one texture per colour), on whole pixels so its cuts stay sharp at small sizes.
pub fn glyph(painter: &Painter, center: egui::Pos2, size: f32, color: Color32) {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    if a == 0 {
        return;
    }
    let ppp = painter.ctx().pixels_per_point();
    let rect = Rect::from_center_size(center, size_for_width(size * ICON_WIDTH));
    let px = vec2((rect.width() * ppp).round().max(1.0), (rect.height() * ppp).round().max(1.0));
    let min = pos2((rect.min.x * ppp).round() / ppp, (rect.min.y * ppp).round() / ppp);
    let shown = Rect::from_min_size(min, px / ppp);
    draw(painter, shown, shown, Color32::from_rgb(r, g, b), Color32::WHITE.gamma_multiply(a as f32 / 255.0));
}

/// The mark rasterised at `raster`'s pixel size in `color`, drawn into `shown` through `tint` (its opacity, without
/// another texture per opacity).
fn draw(painter: &Painter, raster: Rect, shown: Rect, color: Color32, tint: Color32) {
    let ctx: Context = painter.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let size = [(raster.width() * ppp).round().clamp(1.0, 4096.0) as u32, (raster.height() * ppp).round().clamp(1.0, 4096.0) as u32];
    let key = (size, color.to_array());
    let id = Id::unique("n4n0-logo");
    let texture = ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<Cache>(id);
        let i = cache.0.iter().position(|(k, _)| *k == key)?;
        let entry = cache.0.remove(i);
        let t = entry.1.clone();
        cache.0.insert(0, entry);
        Some(t)
    });
    let texture = texture.unwrap_or_else(|| {
        let t = ctx.load_texture(format!("n4n0-logo-{}x{}-{:?}", size[0], size[1], key.1), rasterize(size[0], size[1], color), TextureOptions::LINEAR);
        ctx.data_mut(|d| {
            let cache = d.get_temp_mut_or_default::<Cache>(id);
            cache.0.insert(0, (key, t.clone()));
            cache.0.truncate(KEEP);
        });
        t
    });
    painter.image(texture.id(), shown, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
}

/// Paints the mark into `rect` in the interface's white with the signal glitch started at `glitch` ([`anim::time`], a
/// strength: 1.2 for the splash, 0.8 on hover): cyan and violet echoes sliding apart and back, as the Orbiters mark's.
pub fn paint_glitched(painter: &Painter, rect: Rect, glitch: Option<(f64, f32)>) {
    paint_zoomed(painter, rect, 1.0, glitch);
}

/// [`paint_glitched`] rasterised for `rect` and shown `zoom` times as large around its centre: a mark whose size changes
/// every frame (the splash's) keeps one set of textures instead of making one per size.
pub fn paint_zoomed(painter: &Painter, rect: Rect, zoom: f32, glitch: Option<(f64, f32)>) {
    let zoom = if zoom.is_finite() { zoom.max(0.0) } else { 1.0 };
    let shown = Rect::from_center_size(rect.center(), rect.size() * zoom);
    if let Some((start, strength)) = glitch {
        let ctx = painter.ctx().clone();
        let duration = 0.6 + strength as f64 * 0.3;
        let t = anim::since(&ctx, start, duration);
        if t < duration {
            let k = (t / duration) as f32;
            let times = [0.0, 0.3, 0.65, 1.0];
            let dx = ease::keyframes_at(k, &times, &[0.0, -14.0 * strength, 8.0 * strength, 0.0], ease::out_cubic) * shown.width() / 34.0;
            let alpha = Color32::WHITE.gamma_multiply(ease::keyframes_at(k, &times, &[0.0, 0.9, 0.55, 0.0], ease::out_cubic));
            draw(painter, rect, shown.translate(vec2(dx, 0.0)), theme::CYAN, alpha);
            draw(painter, rect, shown.translate(vec2(-dx, 0.0)), theme::VIOLET, alpha);
        }
    }
    draw(painter, rect, shown, theme::mark(), Color32::WHITE);
}

/// The app's icon, `size` × `size` pixels: a rounded square of the night's violet with a blue glow, the mark in the
/// interface's white with a cyan echo to its left and a violet one to its right, as the Orbiters icon had them.
pub fn icon(size: u32) -> Pixmap {
    let mut pixmap = Pixmap::new(size, size).expect("pixmap");
    let k = size as f32 / 1024.0;
    // The square: 960 wide in a 1024 box, corners of 230.
    let (x0, y0, x1, y1, r) = (32.0 * k, 32.0 * k, 992.0 * k, 992.0 * k, 230.0 * k);
    let square = {
        let mut pb = PathBuilder::new();
        let c = 0.5523 * r;
        pb.move_to(x0 + r, y0);
        pb.line_to(x1 - r, y0);
        pb.cubic_to(x1 - r + c, y0, x1, y0 + r - c, x1, y0 + r);
        pb.line_to(x1, y1 - r);
        pb.cubic_to(x1, y1 - r + c, x1 - r + c, y1, x1 - r, y1);
        pb.line_to(x0 + r, y1);
        pb.cubic_to(x0 + r - c, y1, x0, y1 - r + c, x0, y1 - r);
        pb.line_to(x0, y0 + r);
        pb.cubic_to(x0, y0 + r - c, x0 + r - c, y0, x0 + r, y0);
        pb.close();
        pb.finish().expect("the icon's square")
    };
    let gradient = |cx: f32, cy: f32, radius: f32, stops: Vec<GradientStop>| {
        RadialGradient::new(Point::from_xy(cx, cy), 0.0, Point::from_xy(cx, cy), radius, stops, SpreadMode::Pad, Transform::identity()).expect("gradient")
    };
    let rgba = |r: u8, g: u8, b: u8, a: f32| tiny_skia::Color::from_rgba8(r, g, b, (a * 255.0).round() as u8);
    let mut paint = Paint { anti_alias: true, ..Paint::default() };
    paint.shader = gradient(
        x0 + 0.32 * 960.0 * k,
        y0 + 0.22 * 960.0 * k,
        0.95 * 960.0 * k,
        vec![GradientStop::new(0.0, rgba(0x2b, 0x1a, 0x52, 1.0)), GradientStop::new(0.55, rgba(0x12, 0x0e, 0x24, 1.0)), GradientStop::new(1.0, rgba(0x07, 0x07, 0x0b, 1.0))],
    );
    pixmap.fill_path(&square, &paint, FillRule::Winding, Transform::identity(), None);
    paint.shader = gradient(
        x0 + 0.72 * 960.0 * k,
        y0 + 0.85 * 960.0 * k,
        0.6 * 960.0 * k,
        vec![GradientStop::new(0.0, rgba(0x4f, 0x89, 0xff, 0.45)), GradientStop::new(1.0, rgba(0x4f, 0x89, 0xff, 0.0))],
    );
    pixmap.fill_path(&square, &paint, FillRule::Winding, Transform::identity(), None);
    // The mark, 560 wide in the 1024 box, in its middle; echoes 14 units of the mark's to either side.
    let scale = 560.0 * k / VIEW.x;
    let at = |dx: f32| Transform::from_row(scale, 0.0, 0.0, scale, 512.0 * k - VIEW.x * scale / 2.0 + dx * scale, 512.0 * k - VIEW.y * scale / 2.0);
    fill_mark(&mut pixmap, theme::CYAN, 0.55, at(-14.0));
    fill_mark(&mut pixmap, theme::VIOLET, 0.55, at(14.0));
    fill_mark(&mut pixmap, Color32::from_rgb(0xec, 0xec, 0xf4), 1.0, at(0.0));
    pixmap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_has_its_four_glyphs_and_their_holes() {
        // 2 × 2 glyphs: pixels inside each glyph are drawn, the 0's slots and the 4's counter are not.
        let image = rasterize(940, 822, Color32::WHITE);
        let alpha = |x: usize, y: usize| image.pixels[y * 940 + x].a();
        for (x, y) in [(40, 600), (40, 200), (700, 470), (560, 300), (900, 280)] {
            assert!(alpha(x, y) > 200, "glyph at {x}, {y}");
        }
        for (x, y) in [(700, 550), (700, 735), (700, 200), (470, 400), (470, 100)] {
            assert!(alpha(x, y) < 50, "hole or gap at {x}, {y}");
        }
    }

    #[test]
    fn the_icon_is_opaque_inside_its_square_and_clear_at_its_corners() {
        let icon = icon(256);
        let px = |x: u32, y: u32| icon.pixel(x, y).unwrap();
        assert_eq!(px(1, 1).alpha(), 0, "the corner outside the rounded square");
        assert_eq!(px(128, 20).alpha(), 255, "inside the square");
        // The mark's white over the middle of the bottom right glyph (the 0's block, left of its slots).
        let mark = px(128 + 24, 128 + 52);
        assert!(mark.red() > 200 && mark.green() > 200 && mark.blue() > 200, "{mark:?}");
    }
}
