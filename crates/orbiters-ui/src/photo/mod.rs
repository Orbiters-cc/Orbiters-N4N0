//! Photo editing: a picture (or each layer of a VRChat multi-layer picture) cropped and turned, then a stack of
//! layers over it like Lightroom's: light (exposure, brightness, contrast, highlights, shadows, black point), colour,
//! curves, a vignette with a movable centre, chromatic aberration, the Toolkit photoshoot's comic look (flat tones,
//! ink lines, a print screen of dots), blurs (gaussian, lens, directional) and text in the Toolkit's styles. Each layer
//! can be hidden, faded and limited to a part of the picture ([`Mask`]). Nothing here touches files: the document is
//! [`Edit`] (small, serialisable), the pictures come in as [`Rgba8`] and [`render::Renderer`] makes the result at any
//! size, a preview or the full picture, sizes being shares of the picture so both look alike.
//!
//! Pixels are worked on in linear light with premultiplied alpha ([`Image`]); what reads like a print (curves, the
//! comic's tones, black point) is done on the encoded values, as a picture is seen.

pub mod blur;
pub mod edit;
pub mod effects;
pub mod geometry;
pub mod gpu;
pub mod level;
pub mod render;
pub mod text;
pub mod tone;

pub use edit::*;

use rayon::prelude::*;
use std::sync::OnceLock;

/// A picture as decoded: RGBA8, encoded sRGB, straight alpha, rows packed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba8 {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Rgba8 {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        assert_eq!(pixels.len(), width as usize * height as usize * 4, "RGBA8 of {width}x{height}");
        Self { width, height, pixels }
    }

    /// Scaled down with a box filter so that its long side is at most `max` pixels (a copy when it already is): the
    /// preview's working picture, sampled at about one of its pixels per pixel shown.
    pub fn fit(&self, max: u32) -> Rgba8 {
        let long = self.width.max(self.height);
        if long <= max || max == 0 {
            return self.clone();
        }
        let k = max as f64 / long as f64;
        let (w, h) = (((self.width as f64 * k).round() as u32).max(1), ((self.height as f64 * k).round() as u32).max(1));
        let (sx, sy) = (self.width as f64 / w as f64, self.height as f64 / h as f64);
        let mut out = vec![0u8; w as usize * h as usize * 4];
        let lut = decode_lut();
        out.par_chunks_mut(w as usize * 4).enumerate().for_each(|(y, row)| {
            let (y0, y1) = ((y as f64 * sy) as usize, (((y + 1) as f64 * sy).ceil() as usize).min(self.height as usize));
            for x in 0..w as usize {
                let (x0, x1) = ((x as f64 * sx) as usize, (((x + 1) as f64 * sx).ceil() as usize).min(self.width as usize));
                // Averaged in linear light, weighted by alpha, as a premultiplied picture would be.
                let mut sum = [0f32; 4];
                for yy in y0..y1.max(y0 + 1) {
                    let line = &self.pixels[yy * self.width as usize * 4..];
                    for xx in x0..x1.max(x0 + 1) {
                        let p = &line[xx * 4..xx * 4 + 4];
                        let a = p[3] as f32 / 255.0;
                        sum[0] += lut[p[0] as usize] * a;
                        sum[1] += lut[p[1] as usize] * a;
                        sum[2] += lut[p[2] as usize] * a;
                        sum[3] += a;
                    }
                }
                let n = ((y1.max(y0 + 1) - y0) * (x1.max(x0 + 1) - x0)) as f32;
                let px = unpremultiplied_rgba8([sum[0] / n, sum[1] / n, sum[2] / n, sum[3] / n]);
                row[x * 4..x * 4 + 4].copy_from_slice(&px);
            }
        });
        Rgba8::new(w, h, out)
    }
}

/// A working picture: linear light, premultiplied alpha.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub data: Vec<[f32; 4]>,
}

impl Image {
    /// Transparent.
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, data: vec![[0.0; 4]; width as usize * height as usize] }
    }

    /// One colour all over (linear light, straight alpha).
    pub fn filled(width: u32, height: u32, rgba: [f32; 4]) -> Self {
        let a = rgba[3];
        Self { width, height, data: vec![[rgba[0] * a, rgba[1] * a, rgba[2] * a, a]; width as usize * height as usize] }
    }

    pub fn from_rgba8(p: &Rgba8) -> Self {
        let lut = decode_lut();
        let data = p
            .pixels
            .par_chunks_exact(4)
            .map(|c| {
                let a = c[3] as f32 / 255.0;
                [lut[c[0] as usize] * a, lut[c[1] as usize] * a, lut[c[2] as usize] * a, a]
            })
            .collect();
        Self { width: p.width, height: p.height, data }
    }

    /// As RGBA8, encoded sRGB, straight alpha (what PNG files and egui's textures take).
    pub fn to_rgba8(&self) -> Rgba8 {
        let mut pixels = vec![0u8; self.data.len() * 4];
        pixels.par_chunks_exact_mut(4).zip(self.data.par_iter()).for_each(|(out, p)| out.copy_from_slice(&unpremultiplied_rgba8(*p)));
        Rgba8::new(self.width, self.height, pixels)
    }

    pub fn at(&self, x: i64, y: i64) -> [f32; 4] {
        let x = x.clamp(0, self.width as i64 - 1) as usize;
        let y = y.clamp(0, self.height as i64 - 1) as usize;
        self.data[y * self.width as usize + x]
    }

    /// Bilinear between the pixels around `(x, y)` (pixel centres at half pixels), the edges held beyond the picture.
    pub fn sample(&self, x: f32, y: f32) -> [f32; 4] {
        let (fx, fy) = (x - 0.5, y - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let (a, b, c, d) = (self.at(x0, y0), self.at(x0 + 1, y0), self.at(x0, y0 + 1), self.at(x0 + 1, y0 + 1));
        let mut out = [0.0; 4];
        for k in 0..4 {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            out[k] = top + (bottom - top) * ty;
        }
        out
    }
}

/// How many of a picture's pixels have each luma (of `bins` steps), from about a hundred thousand of them: what a
/// curve is drawn over. Transparent pixels are left out.
pub fn histogram(p: &Rgba8, bins: usize) -> Vec<u32> {
    let mut out = vec![0u32; bins.max(1)];
    let n = (p.width as usize * p.height as usize).max(1);
    let step = (n / 100_000).max(1);
    for i in (0..n).step_by(step) {
        let c = &p.pixels[i * 4..i * 4 + 4];
        if c[3] == 0 {
            continue;
        }
        let l = (c[0] as f32 * 0.299 + c[1] as f32 * 0.587 + c[2] as f32 * 0.114) / 255.0;
        out[((l * bins as f32) as usize).min(bins - 1)] += 1;
    }
    out
}

/// A linear premultiplied pixel as straight RGBA8, encoded sRGB.
fn unpremultiplied_rgba8(p: [f32; 4]) -> [u8; 4] {
    let a = p[3].clamp(0.0, 1.0);
    if a <= 1.0 / 512.0 {
        return [0, 0, 0, 0];
    }
    let byte = |v: f32| (encode((v / a).clamp(0.0, 1.0)) * 255.0 + 0.5) as u8;
    [byte(p[0]), byte(p[1]), byte(p[2]), (a * 255.0 + 0.5) as u8]
}

/// An encoded sRGB value in linear light.
pub fn to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// A linear value encoded as sRGB.
pub fn to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Each byte of encoded sRGB in linear light.
pub fn decode_lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| std::array::from_fn(|i| to_linear(i as f32 / 255.0)))
}

/// Steps of the tables between linear and encoded values.
const STEPS: usize = 4096;

fn encode_table() -> &'static [f32] {
    static T: OnceLock<Vec<f32>> = OnceLock::new();
    T.get_or_init(|| (0..=STEPS).map(|i| to_srgb(i as f32 / STEPS as f32)).collect())
}

fn decode_table() -> &'static [f32] {
    static T: OnceLock<Vec<f32>> = OnceLock::new();
    T.get_or_init(|| (0..=STEPS).map(|i| to_linear(i as f32 / STEPS as f32)).collect())
}

/// Between two entries of a table over 0 to 1.
fn lookup(table: &[f32], v: f32) -> f32 {
    let x = v.clamp(0.0, 1.0) * (table.len() - 1) as f32;
    let i = (x as usize).min(table.len() - 2);
    let t = x - i as f32;
    table[i] + (table[i + 1] - table[i]) * t
}

/// [`to_srgb`] from a table, for 0 to 1 (clamped).
pub fn encode(v: f32) -> f32 {
    lookup(encode_table(), v)
}

/// [`to_linear`] from a table, for 0 to 1 (clamped).
pub fn decode(v: f32) -> f32 {
    lookup(decode_table(), v)
}

/// Rec. 601 luma of encoded values (what the Toolkit's comic look reads).
pub fn luma(c: [f32; 3]) -> f32 {
    c[0] * 0.299 + c[1] * 0.587 + c[2] * 0.114
}

pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_tables_follow_the_formulas() {
        for i in 0..=100 {
            let v = i as f32 / 100.0;
            assert!((encode(v) - to_srgb(v)).abs() < 2e-4, "{v}");
            assert!((decode(v) - to_linear(v)).abs() < 2e-4, "{v}");
            assert!((to_srgb(to_linear(v)) - v).abs() < 1e-5);
        }
    }

    #[test]
    fn rgba8_round_trips_through_the_working_picture() {
        let pixels: Vec<u8> = (0..64u32).flat_map(|i| [(i * 4) as u8, (255 - i * 3) as u8, (i * 2) as u8, 255]).collect();
        let p = Rgba8::new(8, 8, pixels.clone());
        assert_eq!(Image::from_rgba8(&p).to_rgba8().pixels, pixels);
        // Transparent pixels lose their colour, half transparent ones keep it.
        let p = Rgba8::new(2, 1, vec![200, 100, 50, 0, 200, 100, 50, 128]);
        let back = Image::from_rgba8(&p).to_rgba8().pixels;
        assert_eq!(&back[..4], &[0, 0, 0, 0]);
        assert!(back[4..].iter().zip([200u8, 100, 50, 128]).all(|(a, b)| a.abs_diff(b) <= 1), "{back:?}");
    }

    #[test]
    fn fitting_averages_blocks_and_keeps_the_aspect() {
        // Black and white columns average to the grey of half the light, not to 128.
        let pixels: Vec<u8> = (0..16 * 8).flat_map(|i| if i % 2 == 0 { [0, 0, 0, 255] } else { [255, 255, 255, 255] }).collect();
        let small = Rgba8::new(16, 8, pixels).fit(8);
        assert_eq!((small.width, small.height), (8, 4));
        let want = (to_srgb(0.5) * 255.0).round() as u8;
        assert!(small.pixels.chunks(4).all(|p| p[0].abs_diff(want) <= 1 && p[3] == 255), "{:?}", &small.pixels[..8]);
    }
}
