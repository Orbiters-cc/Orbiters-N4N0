//! Discord's logo as an icon, in one colour: its own mark (the outline of Discord's published logo, given by Enzo on
//! 2026-11-23), not a look-alike glyph, for what opens or names Discord. Rasterised with tiny-skia at the exact pixel
//! size, as Blender's logo is ([`crate::blender_logo`], whose path reader it uses), white, and tinted when drawn.

use egui::{pos2, vec2, Color32, ColorImage, Id, Painter, Pos2, Rect, TextureHandle, TextureOptions};
use tiny_skia::{FillRule, Paint, Path, Pixmap, Transform};

/// The mark with its two eyes cut out, as the SVG has it (one path, filled by the even-odd rule).
const MARK: &str = "M216.856339,16.5966031 C200.285002,8.84328665 182.566144,3.2084988 164.041564,0 C161.766523,4.11318106 159.108624,9.64549908 157.276099,14.0464379 C137.583995,11.0849896 118.072967,11.0849896 98.7430163,14.0464379 C96.9108417,9.64549908 94.1925838,4.11318106 91.8971895,0 C73.3526068,3.2084988 55.6133949,8.86399117 39.0420583,16.6376612 C5.61752293,67.146514 -3.4433191,116.400813 1.08711069,164.955721 C23.2560196,181.510915 44.7403634,191.567697 65.8621325,198.148576 C71.0772151,190.971126 75.7283628,183.341335 79.7352139,175.300261 C72.104019,172.400575 64.7949724,168.822202 57.8887866,164.667963 C59.7209612,163.310589 61.5131304,161.891452 63.2445898,160.431257 C105.36741,180.133187 151.134928,180.133187 192.754523,160.431257 C194.506336,161.891452 196.298154,163.310589 198.110326,164.667963 C191.183787,168.842556 183.854737,172.420929 176.223542,175.320965 C180.230393,183.341335 184.861538,190.991831 190.096624,198.16893 C211.238746,191.588051 232.743023,181.531619 254.911949,164.955721 C260.227747,108.668201 245.831087,59.8662432 216.856339,16.5966031 Z M85.4738752,135.09489 C72.8290281,135.09489 62.4592217,123.290155 62.4592217,108.914901 C62.4592217,94.5396472 72.607595,82.7145587 85.4738752,82.7145587 C98.3405064,82.7145587 108.709962,94.5189427 108.488529,108.914901 C108.508531,123.290155 98.3405064,135.09489 85.4738752,135.09489 Z M170.525237,135.09489 C157.88039,135.09489 147.510584,123.290155 147.510584,108.914901 C147.510584,94.5396472 157.658606,82.7145587 170.525237,82.7145587 C183.391518,82.7145587 193.761324,94.5189427 193.539891,108.914901 C193.539891,123.290155 183.391518,135.09489 170.525237,135.09489 Z";

fn path() -> &'static Path {
    static PATH: std::sync::OnceLock<Path> = std::sync::OnceLock::new();
    PATH.get_or_init(|| crate::blender_logo::parse(MARK).expect("Discord's logo"))
}

/// The mark's box in the SVG's own units: from the origin, 256 wide and as high as its lowest point (the path's
/// bounds would count the curves' control points, which reach outside it).
const BOX: (f32, f32) = (256.0, 198.17);

/// The logo's width over its height.
pub fn aspect() -> f32 {
    BOX.0 / BOX.1
}

/// The logo filling `w` × `h` pixels (centred, its proportions kept), white on transparent.
pub fn rasterize(w: u32, h: u32) -> ColorImage {
    let mut pixmap = Pixmap::new(w.max(1), h.max(1)).expect("pixmap");
    let scale = (w as f32 / BOX.0).min(h as f32 / BOX.1);
    let (dx, dy) = ((w as f32 - BOX.0 * scale) * 0.5, (h as f32 - BOX.1 * scale) * 0.5);
    let t = Transform::from_row(scale, 0.0, 0.0, scale, dx, dy);
    let mut paint = Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.anti_alias = true;
    pixmap.fill_path(path(), &paint, FillRule::EvenOdd, t, None);
    ColorImage::from_rgba_premultiplied([pixmap.width() as usize, pixmap.height() as usize], pixmap.data())
}

/// The textures by pixel size, the most recently drawn first.
#[derive(Clone, Default)]
struct Cache(Vec<([u32; 2], TextureHandle)>);
const KEEP: usize = 6;

/// Paints the logo as an icon of `size` (a Phosphor glyph's font size) centred on `center`, in `color`, on whole
/// pixels so that it stays sharp at small sizes.
pub fn glyph(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    if color.a() == 0 {
        return;
    }
    let ctx = painter.ctx().clone();
    let ppp = ctx.pixels_per_point();
    // As wide as a Phosphor glyph's drawing, which keeps a little of its box free on each side.
    let w = size * 0.9;
    let rect = Rect::from_center_size(center, vec2(w, w / aspect()));
    let px = [(rect.width() * ppp).round().clamp(1.0, 1024.0) as u32, (rect.height() * ppp).round().clamp(1.0, 1024.0) as u32];
    let min = pos2((rect.min.x * ppp).round() / ppp, (rect.min.y * ppp).round() / ppp);
    let shown = Rect::from_min_size(min, vec2(px[0] as f32, px[1] as f32) / ppp);
    let id = Id::unique("discord-logo");
    let found = ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<Cache>(id);
        let i = cache.0.iter().position(|(k, _)| *k == px)?;
        let entry = cache.0.remove(i);
        let t = entry.1.clone();
        cache.0.insert(0, entry);
        Some(t)
    });
    let texture = found.unwrap_or_else(|| {
        let t = ctx.load_texture(format!("discord-logo-{}x{}", px[0], px[1]), rasterize(px[0], px[1]), TextureOptions::LINEAR);
        ctx.data_mut(|d| {
            let cache = d.get_temp_mut_or_default::<Cache>(id);
            cache.0.insert(0, (px, t.clone()));
            cache.0.truncate(KEEP);
        });
        t
    });
    painter.image(texture.id(), shown, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logo_is_read_whole_with_its_eyes_open() {
        assert!((aspect() - 1.29).abs() < 0.01, "256 wide, about 198 high ({})", aspect());
        let (w, h) = (256u32, (256.0 / aspect()).round() as u32);
        let image = rasterize(w, h);
        let alpha = |x: f32, y: f32| image.pixels[(y as usize) * w as usize + x as usize].a();
        // The SVG's own coordinates are these pixels, nearly (256 wide).
        assert_eq!(alpha(128.0, 60.0), 255, "the forehead");
        assert_eq!(alpha(85.0, 109.0), 0, "the left eye is cut out");
        assert_eq!(alpha(170.0, 109.0), 0, "the right eye is cut out");
        assert_eq!(alpha(128.0, 109.0), 255, "between the eyes");
        assert_eq!(alpha(2.0, 2.0), 0, "outside, above the shoulder");
        assert_eq!(alpha(128.0, 190.0), 0, "outside, under the chin");
    }
}
