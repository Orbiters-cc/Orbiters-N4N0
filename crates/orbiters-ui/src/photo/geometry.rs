//! Cropping and turning. The picture is turned by quarter turns and flipped (the "turned" picture, `tw`×`th`), then
//! straightened by a small angle about its middle and cropped. The crop is asked in shares of the turned picture and
//! drawn shrunk to fit inside the straightened one ([`fit`]): no empty corners, and straightening back gives the crop
//! asked again. [`transform`] makes the cropped picture at any size by sampling the source at each pixel.

use super::{Crop, Geometry, Image, Rgba8};
use rayon::prelude::*;

/// The turned picture's size for a source of `w`×`h`.
pub fn turned_size(w: u32, h: u32, turns: u8) -> (u32, u32) {
    if turns % 2 == 1 {
        (h, w)
    } else {
        (w, h)
    }
}

/// A point of the straightened picture (pixels of the `tw`×`th` frame) at the point of the turned picture under it.
fn unstraighten(p: [f32; 2], tw: f32, th: f32, degrees: f32) -> [f32; 2] {
    let (s, c) = (-degrees).to_radians().sin_cos();
    let (dx, dy) = (p[0] - tw * 0.5, p[1] - th * 0.5);
    [tw * 0.5 + dx * c - dy * s, th * 0.5 + dx * s + dy * c]
}

/// Whether `p` (pixels of the frame) shows the straightened picture, not an empty corner.
pub fn inside(p: [f32; 2], tw: f32, th: f32, degrees: f32) -> bool {
    let q = unstraighten(p, tw, th, degrees);
    let eps = 1e-4 * tw.max(th);
    q[0] >= -eps && q[0] <= tw + eps && q[1] >= -eps && q[1] <= th + eps
}

fn corners_inside(c: Crop, tw: f32, th: f32, degrees: f32) -> bool {
    [(c.x, c.y), (c.x + c.w, c.y), (c.x, c.y + c.h), (c.x + c.w, c.y + c.h)].iter().all(|&(u, v)| inside([u * tw, v * th], tw, th, degrees))
}

/// `crop` (shares of the turned picture) shrunk about its middle, keeping its shape, and moved towards the picture's
/// middle as needed, until it lies inside the picture straightened by `degrees`.
pub fn fit(crop: Crop, tw: f32, th: f32, degrees: f32) -> Crop {
    let mut c = clamp_to_frame(crop);
    if degrees == 0.0 || corners_inside(c, tw, th, degrees) {
        return c;
    }
    let mut center = c.center();
    if !inside([center[0] * tw, center[1] * th], tw, th, degrees) {
        let (mut lo, mut hi) = (0f32, 1f32);
        for _ in 0..24 {
            let t = (lo + hi) * 0.5;
            let p = [center[0] + (0.5 - center[0]) * t, center[1] + (0.5 - center[1]) * t];
            if inside([p[0] * tw, p[1] * th], tw, th, degrees) {
                hi = t;
            } else {
                lo = t;
            }
        }
        center = [center[0] + (0.5 - center[0]) * hi, center[1] + (0.5 - center[1]) * hi];
    }
    let scaled = |k: f32| Crop { x: center[0] - c.w * k * 0.5, y: center[1] - c.h * k * 0.5, w: c.w * k, h: c.h * k };
    let (mut lo, mut hi) = (0f32, 1f32);
    for _ in 0..28 {
        let k = (lo + hi) * 0.5;
        if corners_inside(scaled(k), tw, th, degrees) {
            lo = k;
        } else {
            hi = k;
        }
    }
    c = scaled(lo);
    c
}

/// Inside the frame and at least a little of it.
pub fn clamp_to_frame(c: Crop) -> Crop {
    let w = c.w.clamp(0.01, 1.0);
    let h = c.h.clamp(0.01, 1.0);
    Crop { x: c.x.clamp(0.0, 1.0 - w), y: c.y.clamp(0.0, 1.0 - h), w, h }
}

/// The largest crop of width over height `ratio` (in pixels) about `crop`'s middle that fits inside `crop`'s frame
/// (the turned picture `tw`×`th`).
pub fn with_ratio(crop: Crop, ratio: f32, tw: f32, th: f32) -> Crop {
    // In shares, w / h = ratio * th / tw.
    let r = ratio * th / tw;
    let (mut w, mut h) = (crop.w, crop.w / r);
    if h > crop.h {
        h = crop.h;
        w = h * r;
    }
    let center = crop.center();
    clamp_to_frame(Crop { x: center[0] - w * 0.5, y: center[1] - h * 0.5, w, h })
}

impl Geometry {
    /// The crop drawn for a source of `w`×`h`: [`Geometry::crop`] fitted inside the straightened picture.
    pub fn effective_crop(&self, w: u32, h: u32) -> Crop {
        let (tw, th) = turned_size(w, h, self.turns);
        fit(self.crop, tw as f32, th as f32, self.straighten)
    }

    /// The edited picture's size in pixels for a source of `w`×`h` (at its full size).
    pub fn output_size(&self, w: u32, h: u32) -> (u32, u32) {
        let (tw, th) = turned_size(w, h, self.turns);
        let c = self.effective_crop(w, h);
        (((c.w * tw as f32).round() as u32).max(1), ((c.h * th as f32).round() as u32).max(1))
    }

    /// The source point (shares of the source picture) under `(u, v)`, shares of the frame `crop` of the turned
    /// picture (for a source whose width over height is `aspect`).
    pub fn source_point(&self, crop: Crop, aspect: f32, u: f32, v: f32) -> [f32; 2] {
        // Work in a frame of the source's shape: one unit high.
        let (tw, th) = if self.turns % 2 == 1 { (1.0, aspect) } else { (aspect, 1.0) };
        let p = [(crop.x + u * crop.w) * tw, (crop.y + v * crop.h) * th];
        let mut q = unstraighten(p, tw, th, self.straighten);
        if self.flip_h {
            q[0] = tw - q[0];
        }
        if self.flip_v {
            q[1] = th - q[1];
        }
        let (x, y) = (q[0] / tw, q[1] / th);
        // Undo the quarter turns (clockwise): shares of the turned picture back to shares of the source.
        match self.turns % 4 {
            1 => [y, 1.0 - x],
            2 => [1.0 - x, 1.0 - y],
            3 => [1.0 - y, x],
            _ => [x, y],
        }
    }
}

/// The frame `crop` of the turned and straightened `src` at `out_w`×`out_h`: each pixel sampled (bilinear, in linear
/// light) where it falls on the source; the empty corners of a straightened picture with an uncropped frame are
/// transparent.
pub fn transform(src: &Rgba8, g: &Geometry, crop: Crop, out_w: u32, out_h: u32) -> Image {
    let mut out = Image::new(out_w, out_h);
    if src.width == 0 || src.height == 0 {
        return out;
    }
    let lut = super::decode_lut();
    let (sw, sh) = (src.width as i64, src.height as i64);
    let aspect = src.width as f32 / src.height as f32;
    let px = |x: i64, y: i64| -> [f32; 4] {
        if x < 0 || y < 0 || x >= sw || y >= sh {
            return [0.0; 4];
        }
        let i = ((y * sw + x) * 4) as usize;
        let p = &src.pixels[i..i + 4];
        let a = p[3] as f32 / 255.0;
        [lut[p[0] as usize] * a, lut[p[1] as usize] * a, lut[p[2] as usize] * a, a]
    };
    let plain = g.straighten == 0.0;
    out.data.par_chunks_mut(out_w as usize).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let u = (x as f32 + 0.5) / out_w as f32;
            let v = (y as f32 + 0.5) / out_h as f32;
            let s = g.source_point(crop, aspect, u, v);
            let (fx, fy) = (s[0] * sw as f32 - 0.5, s[1] * sh as f32 - 0.5);
            // Within half a pixel of the edge the edge pixel is held; beyond, an empty corner.
            if !plain && (fx < -1.0 || fy < -1.0 || fx > sw as f32 || fy > sh as f32) {
                continue;
            }
            let (fx, fy) = (fx.clamp(0.0, (sw - 1) as f32), fy.clamp(0.0, (sh - 1) as f32));
            let (x0, y0) = (fx.floor() as i64, fy.floor() as i64);
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            let (a, b, c, d) = (px(x0, y0), px((x0 + 1).min(sw - 1), y0), px(x0, (y0 + 1).min(sh - 1)), px((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1)));
            for k in 0..4 {
                let top = a[k] + (b[k] - a[k]) * tx;
                let bottom = c[k] + (d[k] - c[k]) * tx;
                o[k] = top + (bottom - top) * ty;
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4
    }

    #[test]
    fn quarter_turns_and_flips_put_the_corners_where_they_go() {
        let full = Crop::FULL;
        let at = |g: &Geometry, u, v| g.source_point(full, 1.5, u, v);
        let g = Geometry::default();
        assert!(close(at(&g, 0.25, 0.75), [0.25, 0.75]));
        // A clockwise turn: the turned picture's top left is the source's bottom left.
        let g = Geometry { turns: 1, ..Default::default() };
        assert!(close(at(&g, 0.0, 0.0), [0.0, 1.0]));
        assert!(close(at(&g, 1.0, 0.0), [0.0, 0.0]));
        assert!(close(at(&g, 1.0, 1.0), [1.0, 0.0]));
        let g = Geometry { turns: 2, ..Default::default() };
        assert!(close(at(&g, 0.0, 0.0), [1.0, 1.0]));
        let g = Geometry { turns: 3, ..Default::default() };
        assert!(close(at(&g, 0.0, 0.0), [1.0, 0.0]));
        let g = Geometry { flip_h: true, ..Default::default() };
        assert!(close(at(&g, 0.1, 0.2), [0.9, 0.2]));
        let g = Geometry { flip_v: true, turns: 1, ..Default::default() };
        // Flipped down after the turn: its top left is the turned picture's bottom left, the source's bottom right.
        assert!(close(at(&g, 0.0, 0.0), [1.0, 1.0]));
        assert_eq!(turned_size(1920, 1080, 1), (1080, 1920));
        assert_eq!(Geometry { turns: 3, ..Default::default() }.output_size(1920, 1080), (1080, 1920));
    }

    #[test]
    fn a_crop_is_the_part_of_the_picture_it_frames() {
        let g = Geometry { crop: Crop { x: 0.25, y: 0.5, w: 0.5, h: 0.25 }, ..Default::default() };
        assert!(close(g.source_point(g.crop, 1.0, 0.0, 0.0), [0.25, 0.5]));
        assert!(close(g.source_point(g.crop, 1.0, 1.0, 1.0), [0.75, 0.75]));
        assert_eq!(g.output_size(400, 200), (200, 50));
        // Pixels come from where they are.
        let pixels: Vec<u8> = (0..16u32).flat_map(|i| [(i * 16) as u8, 0, 0, 255]).collect();
        let src = Rgba8::new(4, 4, pixels);
        let g = Geometry { crop: Crop { x: 0.5, y: 0.5, w: 0.5, h: 0.5 }, ..Default::default() };
        let out = transform(&src, &g, g.crop, 2, 2).to_rgba8();
        assert_eq!(out.pixels.chunks(4).map(|p| p[0]).collect::<Vec<_>>(), vec![10 * 16, 11 * 16, 14 * 16, 15 * 16]);
        // A quarter turn: the top row of the result is the source's left column, bottom to top.
        let g = Geometry { turns: 1, ..Default::default() };
        let out = transform(&src, &g, Crop::FULL, 4, 4).to_rgba8();
        assert_eq!(out.pixels.chunks(4).take(4).map(|p| p[0] / 16).collect::<Vec<_>>(), vec![12, 8, 4, 0]);
    }

    #[test]
    fn straightening_keeps_the_crop_inside_the_picture() {
        let (tw, th) = (1600f32, 900f32);
        for degrees in [3.0f32, -8.0, 20.0, 45.0] {
            let c = fit(Crop::FULL, tw, th, degrees);
            assert!(corners_inside(c, tw, th, degrees), "{degrees}: {c:?}");
            // The largest centred crop of the picture's shape: s = min(W / (W cos + H sin), H / (W sin + H cos)).
            let (s, co) = degrees.abs().to_radians().sin_cos();
            let want = (tw / (tw * co + th * s)).min(th / (tw * s + th * co));
            assert!((c.w - want).abs() < 2e-3 && (c.h - want).abs() < 2e-3, "{degrees}: {} vs {want}", c.w);
            assert!(close(c.center(), [0.5, 0.5]));
        }
        // Straightened back, the crop asked is drawn again.
        let asked = Crop { x: 0.1, y: 0.2, w: 0.5, h: 0.5 };
        assert_eq!(fit(asked, tw, th, 0.0), asked);
        // A crop in a corner moves in as it shrinks.
        let corner = fit(Crop { x: 0.0, y: 0.0, w: 0.3, h: 0.3 }, tw, th, 15.0);
        assert!(corners_inside(corner, tw, th, 15.0) && corner.w > 0.1, "{corner:?}");
        let g = Geometry { straighten: 10.0, ..Default::default() };
        let (w, h) = g.output_size(1600, 900);
        assert!(w < 1600 && h < 900 && ((w as f32 / h as f32) - 16.0 / 9.0).abs() < 0.01, "{w}x{h}");
    }

    #[test]
    fn crop_shapes_keep_their_ratio_in_pixels() {
        let c = with_ratio(Crop::FULL, 1.0, 1600.0, 900.0);
        assert!((c.w * 1600.0 - c.h * 900.0).abs() < 0.5, "{c:?}");
        assert!((c.h - 1.0).abs() < 1e-6 && close(c.center(), [0.5, 0.5]));
        let c = with_ratio(Crop::FULL, 9.0 / 16.0, 1600.0, 900.0);
        assert!(((c.w * 1600.0) / (c.h * 900.0) - 9.0 / 16.0).abs() < 1e-3);
    }
}
