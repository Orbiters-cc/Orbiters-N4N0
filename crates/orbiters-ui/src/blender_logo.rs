//! Blender's logo as an icon, in one colour (the Blender Foundation's mark, which its logo guidelines let a program
//! show to link to Blender: "Open in Blender"). Its outlines are those of the logo's SVG; it is rasterised with
//! tiny-skia at the exact pixel size, as N4N0's mark is ([`crate::n4n0_logo`]), white, and tinted when drawn.

use egui::{pos2, vec2, Color32, ColorImage, Id, Painter, Pos2, Rect, TextureHandle, TextureOptions};
use tiny_skia::{FillRule, Paint, Path, PathBuilder, Pixmap, Transform};

/// The logo's body (its three prongs and the ring round its eye, the eye cut out), as the SVG has it.
const BODY: &str = "m231-330c0.013 0.74 0.249 2.18 0.603 3.3 0.744 2.38 2.01 4.58 3.76 6.51 1.8 1.99 4.02 3.59 6.58 4.73 2.69 1.19 5.61 1.8 8.64 1.8 3.03-4e-3 5.95-0.624 8.64-1.83 2.56-1.15 4.78-2.75 6.58-4.75 1.76-1.95 3.02-4.15 3.76-6.53 0.375-1.2 0.612-2.42 0.707-3.64 0.093-1.2 0.054-2.41-0.117-3.62-0.334-2.35-1.15-4.56-2.4-6.56-1.14-1.85-2.62-3.46-4.38-4.82l4e-3 -3e-3 -17.7-13.6c-0.016-0.012-0.029-0.025-0.046-0.036-1.16-0.892-3.12-0.889-4.39 5e-3 -1.29 0.904-1.44 2.4-0.29 3.34l-5e-3 5e-3 7.39 6.01-22.5 0.024h-0.03c-1.86 2e-3 -3.65 1.22-4 2.77-0.364 1.57 0.9 2.88 2.84 2.88l-3e-3 7e-3 11.4-0.022-20.4 15.6c-0.026 0.019-0.054 0.039-0.078 0.058-1.92 1.47-2.54 3.92-1.33 5.46 1.23 1.57 3.84 1.58 5.78 9e-3l11.1-9.1s-0.162 1.23-0.149 1.96zm28.6 4.11c-2.29 2.33-5.5 3.66-8.96 3.66-3.47 6e-3 -6.68-1.3-8.97-3.63-1.12-1.14-1.94-2.44-2.45-3.83-0.497-1.37-0.69-2.82-0.562-4.28 0.121-1.43 0.547-2.8 1.23-4.03 0.668-1.21 1.59-2.31 2.72-3.24 2.23-1.81 5.06-2.8 8.02-2.8 2.97-4e-3 5.8 0.969 8.03 2.78 1.13 0.924 2.05 2.02 2.72 3.23 0.683 1.23 1.11 2.59 1.23 4.03 0.126 1.46-0.067 2.91-0.564 4.28-0.508 1.4-1.33 2.7-2.45 3.84z";
/// The eye's pupil.
const PUPIL: &str = "m243-334c0.106-1.89 1.03-3.56 2.43-4.74 1.37-1.16 3.21-1.87 5.23-1.87 2.01 0 3.85 0.709 5.22 1.87 1.4 1.18 2.32 2.85 2.43 4.74 0.106 1.94-0.675 3.75-2.04 5.09-1.4 1.36-3.38 2.22-5.61 2.22s-4.22-0.854-5.61-2.22c-1.37-1.34-2.15-3.14-2.04-5.08z";

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b',' | b'\n' | b'\r' | b'\t')
}

/// The next number of SVG path data from `i` (`-0.6`, `4e-3`, `.5` ...).
fn number(d: &str, i: &mut usize) -> Option<f32> {
    let b = d.as_bytes();
    while *i < b.len() && is_space(b[*i]) {
        *i += 1;
    }
    let start = *i;
    if *i < b.len() && matches!(b[*i], b'-' | b'+') {
        *i += 1;
    }
    let mut dot = false;
    while *i < b.len() {
        match b[*i] {
            c if c.is_ascii_digit() => *i += 1,
            b'.' if !dot => {
                dot = true;
                *i += 1;
            }
            b'e' | b'E' => {
                *i += 1;
                if *i < b.len() && matches!(b[*i], b'-' | b'+') {
                    *i += 1;
                }
            }
            _ => break,
        }
    }
    d.get(start..*i)?.parse().ok()
}

/// Parses SVG path data with the commands the logo uses (`m`, `l`, `h`, `c`, `s` and `z`, relative or absolute).
fn parse(d: &str) -> Option<Path> {
    let b = d.as_bytes();
    let mut pb = PathBuilder::new();
    let (mut i, mut cmd) = (0, b'm');
    let (mut cur, mut start, mut ctrl) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32), None::<(f32, f32)>);
    loop {
        while i < b.len() && is_space(b[i]) {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        if b[i].is_ascii_alphabetic() {
            cmd = b[i];
            i += 1;
            if cmd.eq_ignore_ascii_case(&b'z') {
                pb.close();
                (cur, ctrl) = (start, None);
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let point = |i: &mut usize, cur: (f32, f32)| -> Option<(f32, f32)> {
            let (x, y) = (number(d, i)?, number(d, i)?);
            Some(if rel { (cur.0 + x, cur.1 + y) } else { (x, y) })
        };
        match cmd.to_ascii_lowercase() {
            b'm' => {
                let p = point(&mut i, cur)?;
                pb.move_to(p.0, p.1);
                (cur, start, ctrl) = (p, p, None);
                // The pairs after a move are lines.
                cmd = if rel { b'l' } else { b'L' };
            }
            b'l' => {
                let p = point(&mut i, cur)?;
                pb.line_to(p.0, p.1);
                (cur, ctrl) = (p, None);
            }
            b'h' => {
                let x = number(d, &mut i)?;
                cur.0 = if rel { cur.0 + x } else { x };
                pb.line_to(cur.0, cur.1);
                ctrl = None;
            }
            b'c' | b's' => {
                let c1 = if cmd.eq_ignore_ascii_case(&b'c') {
                    point(&mut i, cur)?
                } else {
                    // The first control point mirrors the last curve's second.
                    ctrl.map_or(cur, |c| (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1))
                };
                let c2 = point(&mut i, cur)?;
                let p = point(&mut i, cur)?;
                pb.cubic_to(c1.0, c1.1, c2.0, c2.1, p.0, p.1);
                (cur, ctrl) = (p, Some(c2));
            }
            _ => return None,
        }
    }
    pb.finish()
}

fn paths() -> &'static (Path, Path) {
    static PATHS: std::sync::OnceLock<(Path, Path)> = std::sync::OnceLock::new();
    PATHS.get_or_init(|| (parse(BODY).expect("Blender's logo"), parse(PUPIL).expect("Blender's logo")))
}

/// The logo's width over its height.
pub fn aspect() -> f32 {
    let b = paths().0.bounds();
    b.width() / b.height()
}

/// The logo filling `w` × `h` pixels (centred, its proportions kept), white on transparent.
pub fn rasterize(w: u32, h: u32) -> ColorImage {
    let mut pixmap = Pixmap::new(w.max(1), h.max(1)).expect("pixmap");
    let (body, pupil) = paths();
    let b = body.bounds();
    let scale = (w as f32 / b.width()).min(h as f32 / b.height());
    let (dx, dy) = ((w as f32 - b.width() * scale) * 0.5, (h as f32 - b.height() * scale) * 0.5);
    let t = Transform::from_row(scale, 0.0, 0.0, scale, dx - b.left() * scale, dy - b.top() * scale);
    let mut paint = Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.anti_alias = true;
    pixmap.fill_path(body, &paint, FillRule::Winding, t, None);
    pixmap.fill_path(pupil, &paint, FillRule::Winding, t, None);
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
    let id = Id::unique("blender-logo");
    let found = ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<Cache>(id);
        let i = cache.0.iter().position(|(k, _)| *k == px)?;
        let entry = cache.0.remove(i);
        let t = entry.1.clone();
        cache.0.insert(0, entry);
        Some(t)
    });
    let texture = found.unwrap_or_else(|| {
        let t = ctx.load_texture(format!("blender-logo-{}x{}", px[0], px[1]), rasterize(px[0], px[1]), TextureOptions::LINEAR);
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
    fn the_logo_is_read_whole_with_its_eye_open() {
        let (body, pupil) = paths();
        assert!((aspect() - 1.24).abs() < 0.1, "about 181:148 ({})", aspect());
        let (b, p) = (body.bounds(), pupil.bounds());
        assert!(p.left() > b.left() + b.width() * 0.45 && p.right() < b.right(), "the pupil sits in the ring, at the right");
        let (w, h) = (200u32, (200.0 / aspect()).round() as u32);
        let image = rasterize(w, h);
        let alpha = |x: f32, y: f32| image.pixels[(y as usize) * w as usize + x as usize].a();
        let to_px = |x: f32, y: f32| ((x - b.left()) / b.width() * w as f32, (y - b.top()) / b.height() * h as f32);
        let mid = ((p.left() + p.right()) * 0.5, (p.top() + p.bottom()) * 0.5);
        let (cx, cy) = to_px(mid.0, mid.1);
        assert_eq!(alpha(cx, cy), 255, "the pupil is drawn");
        let (ex, ey) = to_px(p.right() + p.width() * 0.2, mid.1);
        assert_eq!(alpha(ex, ey), 0, "the eye round it is cut out");
        assert_eq!(alpha(2.0, h as f32 - 2.0), 0, "outside");
    }
}
