//! Cropping and turning. The picture is turned by quarter turns and flipped (the "turned" picture, `tw`×`th`), then
//! tilted in perspective (a homography: the picture turned in depth about its middle lines, [`Warp`]), then
//! straightened by a small angle about its middle, and cropped. The crop is asked in shares of the turned picture's
//! frame and drawn shrunk to fit inside the tilted and straightened picture ([`fit`], [`fit_warp`]): no empty corners,
//! and undoing the tilt and the straightening gives the crop asked again. [`transform`] makes the cropped picture at any
//! size by sampling the source at each pixel.

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

/// The focal length of the virtual camera the perspective corrections turn the picture in front of, in shares of the
/// frame's long side (about a 67° view across it: strong enough to undo a building shot from below).
pub const FOCAL: f32 = 0.75;

/// How the turned picture (`tw`×`th`) is drawn into its frame of the same size: tilted in perspective about its middle
/// by `vertical` and `horizontal` degrees (the plane of the picture turned in depth in front of a camera [`FOCAL`]
/// away, then seen again from that camera), then straightened by `degrees` about the middle. The middle stays where it
/// is. Points are pixels of the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Warp {
    pub tw: f32,
    pub th: f32,
    /// Sine and cosine of the straightening.
    rot: (f32, f32),
    /// From the turned picture to the frame (about the middle, before the straightening), and back; None: no tilt.
    tilt: Option<([[f64; 3]; 3], [[f64; 3]; 3])>,
}

impl Warp {
    /// Straightened only.
    pub fn straighten(tw: f32, th: f32, degrees: f32) -> Self {
        Self::new(tw, th, degrees, 0.0, 0.0)
    }

    /// Straightened by `degrees` after a tilt of `vertical` degrees (positive: the top comes nearer, wider) and
    /// `horizontal` degrees (positive: the right side comes nearer, taller).
    pub fn new(tw: f32, th: f32, degrees: f32, vertical: f32, horizontal: f32) -> Self {
        let tilt = (vertical != 0.0 || horizontal != 0.0).then(|| {
            let h = homography(tw.max(th) as f64 * FOCAL as f64, vertical as f64, horizontal as f64);
            (h, invert(&h))
        });
        Self { tw, th, rot: degrees.to_radians().sin_cos(), tilt }
    }

    /// The warp of `g` for a turned picture of `tw`×`th`.
    pub fn of(g: &Geometry, tw: f32, th: f32) -> Self {
        Self::new(tw, th, g.straighten, g.vertical, g.horizontal)
    }

    /// Nothing moves.
    pub fn is_identity(&self) -> bool {
        self.rot.0 == 0.0 && self.tilt.is_none()
    }

    /// The tilt as a matrix from the turned picture to the frame, both about their middle in pixels (homogeneous;
    /// identity without a tilt): for drawing it on a GPU.
    pub fn tilt_matrix(&self) -> [[f32; 3]; 3] {
        let m = self.tilt.map_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], |t| t.0);
        m.map(|r| r.map(|v| v as f32))
    }

    /// The point of the turned picture under `p` (pixels of the frame); None beyond the tilted picture's horizon.
    pub fn unwarp(&self, p: [f32; 2]) -> Option<[f32; 2]> {
        let (s, c) = (-self.rot.0, self.rot.1);
        let (dx, dy) = (p[0] - self.tw * 0.5, p[1] - self.th * 0.5);
        let q = [dx * c - dy * s, dx * s + dy * c];
        let q = match &self.tilt {
            Some((_, inv)) => apply(inv, q)?,
            None => q,
        };
        Some([self.tw * 0.5 + q[0], self.th * 0.5 + q[1]])
    }

    /// Where the turned picture's point `q` is drawn in the frame; None when it is behind the camera.
    pub fn warp(&self, q: [f32; 2]) -> Option<[f32; 2]> {
        let d = [q[0] - self.tw * 0.5, q[1] - self.th * 0.5];
        let d = match &self.tilt {
            Some((fwd, _)) => apply(fwd, d)?,
            None => d,
        };
        let (s, c) = self.rot;
        Some([self.tw * 0.5 + d[0] * c - d[1] * s, self.th * 0.5 + d[0] * s + d[1] * c])
    }

    /// Whether `p` (pixels of the frame) shows the picture, not an empty corner.
    pub fn inside(&self, p: [f32; 2]) -> bool {
        let eps = 1e-4 * self.tw.max(self.th);
        self.unwarp(p).is_some_and(|q| q[0] >= -eps && q[0] <= self.tw + eps && q[1] >= -eps && q[1] <= self.th + eps)
    }

    /// The outline of the picture in the frame (from its top-left corner round, `per_side` points along each side):
    /// what the crop must stay in.
    pub fn outline(&self, per_side: usize) -> Vec<[f32; 2]> {
        let n = per_side.max(1);
        let corners = [[0.0, 0.0], [self.tw, 0.0], [self.tw, self.th], [0.0, self.th]];
        let mut out = Vec::with_capacity(4 * n);
        for k in 0..4 {
            let (a, b) = (corners[k], corners[(k + 1) % 4]);
            for i in 0..n {
                let t = i as f32 / n as f32;
                if let Some(p) = self.warp([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]) {
                    out.push(p);
                }
            }
        }
        out
    }
}

/// The plane of a picture turned about its middle lines in front of a camera `f` away, as seen by that camera: a
/// homography from points about the picture's middle to points about the frame's middle. `vertical` turns it about
/// the horizontal line (positive: its top nearer), then `horizontal` about the vertical line (positive: its right side
/// nearer). The middle stays.
fn homography(f: f64, vertical: f64, horizontal: f64) -> [[f64; 3]; 3] {
    let (st, ct) = vertical.to_radians().sin_cos();
    let (sp, cp) = horizontal.to_radians().sin_cos();
    // A point (x, y, 0) of the picture (y down): about the horizontal line, (x, y·cosθ, y·sinθ) (the top, y < 0, nearer
    // for θ > 0); about the vertical line, x' = x·cosφ + z·sinφ, z' = z·cosφ − x·sinφ (the right side nearer for
    // φ > 0); seen from the camera, f·(x', y') / (f + z').
    [[f * cp, f * sp * st, 0.0], [0.0, f * ct, 0.0], [-sp, cp * st, f]]
}

fn apply(m: &[[f64; 3]; 3], p: [f32; 2]) -> Option<[f32; 2]> {
    let (x, y) = (p[0] as f64, p[1] as f64);
    let w = m[2][0] * x + m[2][1] * y + m[2][2];
    if w <= 1e-9 {
        return None;
    }
    Some([((m[0][0] * x + m[0][1] * y + m[0][2]) / w) as f32, ((m[1][0] * x + m[1][1] * y + m[1][2]) / w) as f32])
}

/// The inverse of a 3×3 matrix (by its adjugate), its sign chosen so that its last entry is positive: a homography's
/// inverse, whose points in front of the camera keep a positive weight.
fn invert(m: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let c = |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let adj = [
        [c(1, 1, 2, 2), -c(0, 1, 2, 2), c(0, 1, 1, 2)],
        [-c(1, 0, 2, 2), c(0, 0, 2, 2), -c(0, 0, 1, 2)],
        [c(1, 0, 2, 1), -c(0, 0, 2, 1), c(0, 0, 1, 1)],
    ];
    let det = m[0][0] * adj[0][0] + m[0][1] * adj[1][0] + m[0][2] * adj[2][0];
    let k = if det.abs() > 1e-12 { 1.0 / det } else { 1.0 };
    let k = if adj[2][2] * k < 0.0 { -k } else { k };
    adj.map(|r| r.map(|v| v * k))
}

/// Whether `p` (pixels of the frame) shows the straightened picture, not an empty corner.
pub fn inside(p: [f32; 2], tw: f32, th: f32, degrees: f32) -> bool {
    Warp::straighten(tw, th, degrees).inside(p)
}

/// Whether all of `crop` (shares of the frame) shows the picture as `w` draws it (the picture's outline is convex: its
/// corners are enough).
pub fn crop_inside(crop: Crop, w: &Warp) -> bool {
    let (tw, th) = (w.tw, w.th);
    [(crop.x, crop.y), (crop.x + crop.w, crop.y), (crop.x, crop.y + crop.h), (crop.x + crop.w, crop.y + crop.h)].iter().all(|&(u, v)| w.inside([u * tw, v * th]))
}

/// `crop` (shares of the turned picture) shrunk about its middle, keeping its shape, and moved towards the picture's
/// middle as needed, until it lies inside the picture straightened by `degrees`.
pub fn fit(crop: Crop, tw: f32, th: f32, degrees: f32) -> Crop {
    fit_warp(crop, &Warp::straighten(tw, th, degrees))
}

/// [`fit`] for any warp (tilted and straightened): the crop asked, shrunk about its middle (moved towards the
/// picture's middle when that is off the picture) until it lies inside the picture as `w` draws it.
pub fn fit_warp(crop: Crop, w: &Warp) -> Crop {
    let (tw, th) = (w.tw, w.th);
    let c = clamp_to_frame(crop);
    if w.is_identity() || crop_inside(c, w) {
        return c;
    }
    let mut center = c.center();
    if !w.inside([center[0] * tw, center[1] * th]) {
        let (mut lo, mut hi) = (0f32, 1f32);
        for _ in 0..24 {
            let t = (lo + hi) * 0.5;
            let p = [center[0] + (0.5 - center[0]) * t, center[1] + (0.5 - center[1]) * t];
            if w.inside([p[0] * tw, p[1] * th]) {
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
        if crop_inside(scaled(k), w) {
            lo = k;
        } else {
            hi = k;
        }
    }
    scaled(lo)
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
    /// The warp drawing the turned picture of a source of `w`×`h` into its frame, in pixels.
    pub fn warp(&self, w: u32, h: u32) -> Warp {
        let (tw, th) = turned_size(w, h, self.turns);
        Warp::of(self, tw as f32, th as f32)
    }

    /// The crop drawn for a source of `w`×`h`: [`Geometry::crop`] fitted inside the tilted and straightened picture.
    pub fn effective_crop(&self, w: u32, h: u32) -> Crop {
        fit_warp(self.crop, &self.warp(w, h))
    }

    /// The edited picture's size in pixels for a source of `w`×`h` (at its full size).
    pub fn output_size(&self, w: u32, h: u32) -> (u32, u32) {
        let (tw, th) = turned_size(w, h, self.turns);
        let c = self.effective_crop(w, h);
        (((c.w * tw as f32).round() as u32).max(1), ((c.h * th as f32).round() as u32).max(1))
    }

    /// The source point (shares of the source picture) under `(u, v)`, shares of the frame `crop` of the turned
    /// picture (for a source whose width over height is `aspect`); far outside it beyond a tilted picture's horizon.
    pub fn source_point(&self, crop: Crop, aspect: f32, u: f32, v: f32) -> [f32; 2] {
        Sampler::new(self, aspect).source(crop, u, v).unwrap_or([-1.0, -1.0])
    }
}

/// [`Geometry::source_point`] with its warp made once (a picture's every pixel asks).
struct Sampler<'a> {
    g: &'a Geometry,
    warp: Warp,
}

impl<'a> Sampler<'a> {
    fn new(g: &'a Geometry, aspect: f32) -> Self {
        // Work in a frame of the source's shape: one unit high.
        let (tw, th) = if g.turns % 2 == 1 { (1.0, aspect) } else { (aspect, 1.0) };
        Self { g, warp: Warp::of(g, tw, th) }
    }

    fn source(&self, crop: Crop, u: f32, v: f32) -> Option<[f32; 2]> {
        let (tw, th) = (self.warp.tw, self.warp.th);
        let mut q = self.warp.unwarp([(crop.x + u * crop.w) * tw, (crop.y + v * crop.h) * th])?;
        if self.g.flip_h {
            q[0] = tw - q[0];
        }
        if self.g.flip_v {
            q[1] = th - q[1];
        }
        let (x, y) = (q[0] / tw, q[1] / th);
        // Undo the quarter turns (clockwise): shares of the turned picture back to shares of the source.
        Some(match self.g.turns % 4 {
            1 => [y, 1.0 - x],
            2 => [1.0 - x, 1.0 - y],
            3 => [1.0 - y, x],
            _ => [x, y],
        })
    }
}

/// The frame `crop` of the turned, tilted and straightened `src` at `out_w`×`out_h`: each pixel sampled (bilinear, in
/// linear light) where it falls on the source; the empty corners of a straightened or tilted picture with an uncropped
/// frame are transparent.
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
    let sampler = Sampler::new(g, aspect);
    let plain = sampler.warp.is_identity();
    out.data.par_chunks_mut(out_w as usize).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let u = (x as f32 + 0.5) / out_w as f32;
            let v = (y as f32 + 0.5) / out_h as f32;
            let Some(s) = sampler.source(crop, u, v) else { continue };
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
            assert!(crop_inside(c, &Warp::straighten(tw, th, degrees)), "{degrees}: {c:?}");
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
        assert!(crop_inside(corner, &Warp::straighten(tw, th, 15.0)) && corner.w > 0.1, "{corner:?}");
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

    #[test]
    fn presets_stand_and_lie_as_the_iphone_lists_them() {
        use crate::photo::Aspect;
        let wide = 16.0 / 9.0;
        let names: Vec<&str> = Aspect::PRESETS.iter().map(|a| a.name()).collect();
        assert_eq!(names, ["Original", "Freeform", "Square", "9:16", "4:5", "5:7", "3:4", "3:5", "2:3"]);
        let lying: Vec<&str> = Aspect::PRESETS.iter().map(|a| a.oriented(true, wide).name()).collect();
        assert_eq!(lying, ["Original", "Freeform", "Square", "16:9", "5:4", "7:5", "4:3", "5:3", "3:2"]);
        for a in Aspect::ALL {
            // Turned twice, the same; turned, the inverse shape; every form back to its preset.
            assert_eq!(a.turned().turned(), a);
            if let (Some(r), Some(t)) = (a.ratio(wide), a.turned().ratio(wide)) {
                assert!((r * t - 1.0).abs() < 1e-5 || a == Aspect::Square, "{a:?}");
            }
            assert!(Aspect::PRESETS.contains(&a.preset()), "{a:?}");
            assert!(a.oriented(true, wide).landscape(wide).unwrap_or(true), "{a:?}");
            assert!(!a.oriented(false, wide).landscape(wide).unwrap_or(false), "{a:?}");
        }
        // The original of a lying picture lies; stood up, it is the picture's shape turned.
        assert_eq!(Aspect::Original.landscape(wide), Some(true));
        assert_eq!(Aspect::Original.oriented(false, wide), Aspect::OriginalTurned);
        assert!((Aspect::OriginalTurned.ratio(wide).unwrap() - 9.0 / 16.0).abs() < 1e-6);
        assert_eq!(Aspect::Square.landscape(wide), None);
        assert_eq!(Aspect::Free.ratio(wide), None);
        // The largest of each shape, in pixels of a 1600×900 picture.
        for a in Aspect::ALL {
            let Some(r) = a.ratio(wide) else { continue };
            let c = with_ratio(Crop::FULL, r, 1600.0, 900.0);
            assert!(((c.w * 1600.0) / (c.h * 900.0) - r).abs() < 1e-3, "{a:?}");
            assert!((c.w - 1.0).abs() < 1e-5 || (c.h - 1.0).abs() < 1e-5, "{a:?}: the largest");
        }
    }

    /// Where the lines `a`–`b` and `c`–`d` meet.
    fn meet(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> [f32; 2] {
        let (r, s) = ([b[0] - a[0], b[1] - a[1]], [d[0] - c[0], d[1] - c[1]]);
        let t = ((c[0] - a[0]) * s[1] - (c[1] - a[1]) * s[0]) / (r[0] * s[1] - r[1] * s[0]);
        [a[0] + r[0] * t, a[1] + r[1] * t]
    }

    #[test]
    fn perspective_tilts_are_homographies_about_the_middle() {
        let (tw, th) = (1600f32, 900f32);
        let f = tw * FOCAL;
        // Untilted, nothing moves; tilted, the middle stays.
        let w = Warp::new(tw, th, 0.0, 0.0, 0.0);
        assert!(w.is_identity() && w.unwarp([100.0, 700.0]) == Some([100.0, 700.0]));
        for (v, h) in [(20.0f32, 0.0f32), (0.0, -30.0), (12.0, 25.0), (-45.0, 45.0)] {
            let w = Warp::new(tw, th, 7.0, v, h);
            let mid = w.unwarp([800.0, 450.0]).unwrap();
            assert!((mid[0] - 800.0).abs() < 0.01 && (mid[1] - 450.0).abs() < 0.01, "{v} {h}: {mid:?}");
            // There and back.
            for p in [[0.0, 0.0], [1600.0, 0.0], [300.0, 820.0], [1500.0, 880.0]] {
                // (Beyond the horizon of the strongest tilts a frame's corner has no picture at all.)
                let Some(q) = w.unwarp(p) else {
                    assert!(v.abs() + h.abs() > 60.0, "{v} {h}: {p:?}");
                    continue;
                };
                let back = w.warp(q).unwrap();
                assert!((back[0] - p[0]).abs() < 0.05 && (back[1] - p[1]).abs() < 0.05, "{v} {h}: {p:?} → {back:?}");
            }
            // Straight lines stay straight: three points in a line of the frame come from three in a line.
            let q: Vec<[f32; 2]> = [[500.0, 300.0], [800.0, 450.0], [1100.0, 600.0]].iter().map(|p| w.unwarp(*p).unwrap()).collect();
            let cross = (q[1][0] - q[0][0]) * (q[2][1] - q[0][1]) - (q[1][1] - q[0][1]) * (q[2][0] - q[0][0]);
            let scale = ((q[2][0] - q[0][0]).powi(2) + (q[2][1] - q[0][1]).powi(2)).sqrt();
            assert!(cross.abs() < 1e-4 * scale * scale, "{v} {h}: {cross}");
        }
        // A vertical tilt: the picture's verticals lean together and meet below the middle, f / tan θ away (its top
        // nearer, wider); the bottom narrower, so the frame's bottom corners show nothing.
        let theta = 20f32;
        let w = Warp::new(tw, th, 0.0, theta, 0.0);
        let (l0, l1) = (w.warp([400.0, 0.0]).unwrap(), w.warp([400.0, 900.0]).unwrap());
        let (r0, r1) = (w.warp([1200.0, 0.0]).unwrap(), w.warp([1200.0, 900.0]).unwrap());
        assert!(r0[0] - l0[0] > r1[0] - l1[0], "wider at the top");
        let v = meet(l0, l1, r0, r1);
        assert!((v[0] - 800.0).abs() < 0.5 && (v[1] - (450.0 + f / theta.to_radians().tan())).abs() < 1.0, "{v:?}");
        assert!(!w.inside([0.0, 900.0]) && !w.inside([1600.0, 900.0]) && w.inside([0.0, 0.0]));
        // A horizontal tilt: taller on the right, its horizontals meeting on the left.
        let w = Warp::new(tw, th, 0.0, 0.0, 20.0);
        let (t0, t1) = (w.warp([0.0, 200.0]).unwrap(), w.warp([1600.0, 200.0]).unwrap());
        let (b0, b1) = (w.warp([0.0, 700.0]).unwrap(), w.warp([1600.0, 700.0]).unwrap());
        assert!(b1[1] - t1[1] > b0[1] - t0[1], "taller on the right");
        let v = meet(t0, t1, b0, b1);
        assert!((v[1] - 450.0).abs() < 0.5 && (v[0] - (800.0 - f / 20f32.to_radians().tan())).abs() < 1.0, "{v:?}");
        // Undoing a building's lean: walls shot from below lean together towards a point above the picture (the camera
        // turned up by α puts it f / tan α above the middle); tilted by θ with sin θ = f / that distance, they stand
        // upright (the tilt sends that point to infinity).
        let alpha = 15f32.to_radians();
        let top = [800.0, 450.0 - f / alpha.tan()];
        let fix = Warp::new(tw, th, 0.0, alpha.tan().asin().to_degrees(), 0.0);
        for x in [300.0f32, 1250.0] {
            let (a, b) = (top, [x, 850.0]);
            let at = |t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            let (p, q) = (at(0.9), at(1.0));
            assert!((p[0] - q[0]).abs() > 5.0, "it leans");
            let (p, q) = (fix.warp(p).unwrap(), fix.warp(q).unwrap());
            assert!((p[0] - q[0]).abs() < 0.05, "{x}: {p:?} {q:?}");
        }
    }

    #[test]
    fn a_tilted_picture_keeps_its_crop_inside_and_its_pixels_in_place() {
        let (tw, th) = (1600f32, 900f32);
        for (v, h, s) in [(25.0f32, 0.0f32, 0.0f32), (0.0, -40.0, 0.0), (-30.0, 15.0, 8.0), (45.0, 45.0, -45.0)] {
            let w = Warp::new(tw, th, s, v, h);
            let c = fit_warp(Crop::FULL, &w);
            assert!(crop_inside(c, &w), "{v} {h} {s}: {c:?}");
            assert!(c.w > 0.2 && c.w < 1.0 && close(c.center(), [0.5, 0.5]), "{v} {h} {s}: {c:?}");
            // As large as it can be: a little more and a corner shows nothing.
            let more = Crop { x: c.x - c.w * 0.005, y: c.y - c.h * 0.005, w: c.w * 1.01, h: c.h * 1.01 };
            assert!(!crop_inside(more, &w), "{v} {h} {s}");
        }
        // The geometry's crop, its size and its pixels follow the tilt; with no tilt the crop asked is drawn again.
        let g = Geometry { vertical: 20.0, ..Default::default() };
        let c = g.effective_crop(1600, 900);
        assert!(c.w < 1.0 && c.h < 1.0, "{c:?}");
        let (w, h) = g.output_size(1600, 900);
        assert!(((w as f32 / h as f32) - 16.0 / 9.0).abs() < 0.01, "{w}x{h}");
        assert_eq!(Geometry { vertical: 0.0, ..g.clone() }.effective_crop(1600, 900), Crop::FULL);
        assert!(!g.is_identity());
        // A tilted picture's pixel is the source's pixel the warp says.
        let pixels: Vec<u8> = (0..64 * 36).flat_map(|i| [(i % 64 * 4) as u8, (i / 64 * 7) as u8, 0, 255]).collect();
        let src = Rgba8::new(64, 36, pixels);
        let g = Geometry { vertical: 15.0, horizontal: -10.0, ..Default::default() };
        let crop = g.effective_crop(64, 36);
        let out = transform(&src, &g, crop, 64, 36).to_rgba8();
        let warp = Warp::of(&g, 64.0, 36.0);
        for (x, y) in [(10u32, 5u32), (40, 20), (60, 33)] {
            let p = [(crop.x + (x as f32 + 0.5) / 64.0 * crop.w) * 64.0, (crop.y + (y as f32 + 0.5) / 36.0 * crop.h) * 36.0];
            let q = warp.unwarp(p).unwrap();
            let got = &out.pixels[((y * 64 + x) * 4) as usize..][..2];
            // The source's red steps by 4 a pixel across, its green by 7 down (encoded: compared loosely).
            let want = [((q[0] - 0.5).floor() * 4.0), ((q[1] - 0.5).floor() * 7.0)];
            assert!((got[0] as f32 - want[0]).abs() <= 6.0 && (got[1] as f32 - want[1]).abs() <= 9.0, "{x},{y}: {got:?} at {q:?}");
        }
        // Its uncropped frame shows nothing beyond the narrower bottom right.
        let whole = transform(&src, &g, Crop::FULL, 64, 36);
        assert_eq!(whole.data[35 * 64 + 63][3], 0.0);
        assert_eq!(whole.data[18 * 64 + 32][3], 1.0);
    }
}
