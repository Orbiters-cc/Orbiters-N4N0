//! The Toolkit photoshoot's effects of the whole frame (`PhotoshootEffects.shader`, its final pass), ported to the
//! CPU: the vignette (around a centre that moves here), chromatic aberration and the comic look. Sizes are shares of
//! the picture or scale with its height, as there, so the preview looks like the full picture.

use super::{decode, encode, lerp, luma, smoothstep, Aberration, Comic, Image, Vignette};
use rayon::prelude::*;

/// How much light the vignette leaves at `(u, v)` (shares of the frame, `aspect` its width over height): the
/// Toolkit's `pow(saturate(1 - dot(d, d) * 1.8), 2.2)`, its reach scaled by `size` and its fall-off by `feather`.
pub fn vignette_shade(v: &Vignette, u: f32, w: f32, aspect: f32) -> f32 {
    let d = [(u - v.center[0]) * aspect, w - v.center[1]];
    let size = v.size.clamp(0.2, 3.0);
    let exponent = 0.6 + v.feather.clamp(0.0, 1.0) * 3.2;
    (1.0 - (d[0] * d[0] + d[1] * d[1]) * 1.8 / (size * size)).clamp(0.0, 1.0).powf(exponent)
}

/// Darker edges, or lighter ones below zero (the Toolkit's: in linear light).
pub fn vignette(img: &mut Image, v: &Vignette) {
    let (w, h) = (img.width as usize, img.height as usize);
    let aspect = w as f32 / h as f32;
    let amount = v.amount.clamp(-1.0, 1.0);
    img.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let fy = (y as f32 + 0.5) / h as f32;
        for (x, p) in row.iter_mut().enumerate() {
            let shade = vignette_shade(v, (x as f32 + 0.5) / w as f32, fy, aspect);
            if amount > 0.0 {
                let k = lerp(1.0, shade, amount);
                p[0] *= k;
                p[1] *= k;
                p[2] *= k;
            } else {
                let t = (1.0 - shade) * -amount;
                for c in 0..3 {
                    p[c] = lerp(p[c], p[3], t);
                }
            }
        }
    });
}

/// Where the red and the blue of the pixel at `(u, v)` come from: away from and towards the centre by the Toolkit's
/// `amount × 0.012` of the distance to it.
pub fn aberration_offsets(a: &Aberration, u: f32, v: f32) -> ([f32; 2], [f32; 2]) {
    let shift = a.amount.clamp(0.0, 3.0) * 0.012;
    let from = [u - a.center[0], v - a.center[1]];
    ([u - from[0] * shift, v - from[1] * shift], [u + from[0] * shift, v + from[1] * shift])
}

pub fn aberration(img: &mut Image, a: &Aberration) {
    if a.amount <= 0.0 {
        return;
    }
    let src = img.clone();
    let (w, h) = (img.width as usize, img.height as usize);
    img.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let v = (y as f32 + 0.5) / h as f32;
        for (x, p) in row.iter_mut().enumerate() {
            let u = (x as f32 + 0.5) / w as f32;
            let (r, b) = aberration_offsets(a, u, v);
            p[0] = src.sample(r[0] * w as f32, r[1] * h as f32)[0];
            p[2] = src.sample(b[0] * w as f32, b[1] * h as f32)[2];
        }
    });
}

/// The ink of the comic's lines and dots (the Toolkit's `float3(0.06, 0.05, 0.08)`).
const INK: [f32; 3] = [0.06, 0.05, 0.08];

/// The comic's flat colour for an encoded colour: its brightness stepped to one of `tones`, its hue kept, a little
/// more saturated (the Toolkit's `Comic`, for the subject).
pub fn comic_tone(c: [f32; 3], tones: u32) -> [f32; 3] {
    let l = luma(c);
    let steps = (tones.clamp(Comic::MIN_TONES, Comic::MAX_TONES) - 1).max(1) as f32;
    let level = (l * steps + 0.5).floor() / steps;
    let k = level.max(0.05) / l.max(1e-3);
    let flat = c.map(|v| (v * k).clamp(0.0, 1.0));
    let fl = luma(flat);
    flat.map(|v| lerp(fl, v, 1.2).clamp(0.0, 1.0))
}

/// How much ink a dot of the print screen puts at pixel `(x, y)` for a tone of luma `l`: dots on a 45° grid of `cell`
/// pixels, each covering as much of its cell as the tone is dark.
pub fn dot(x: f32, y: f32, cell: f32, l: f32) -> f32 {
    let g = [(x + y) * std::f32::consts::FRAC_1_SQRT_2 / cell, (y - x) * std::f32::consts::FRAC_1_SQRT_2 / cell];
    let f = [g[0] - g[0].floor() - 0.5, g[1] - g[1].floor() - 0.5];
    let distance = (f[0] * f[0] + f[1] * f[1]).sqrt();
    // The Toolkit's `sqrt(saturate(1 - luma) * 0.3183) * 1.06`: an area of the darkness.
    let radius = ((1.0 - l).clamp(0.0, 1.0) * std::f32::consts::FRAC_1_PI).sqrt() * 1.06;
    let edge = 1.0 / cell;
    1.0 - smoothstep(radius - edge, radius + edge, distance)
}

/// The comic look over the picture. Its tones and dots are chosen on the picture as seen (encoded), like a print.
pub fn comic(img: &mut Image, c: &Comic) {
    let (w, h) = (img.width as usize, img.height as usize);
    if w == 0 || h == 0 {
        return;
    }
    // Straight encoded colours, and the luma the lines are found in, softened a little so that grain draws no lines.
    let shown: Vec<[f32; 4]> = img
        .data
        .par_iter()
        .map(|p| {
            let a = p[3];
            if a <= 0.0 {
                [0.0; 4]
            } else {
                [encode(p[0] / a), encode(p[1] / a), encode(p[2] / a), a]
            }
        })
        .collect();
    let line = Comic::line_width(h as f32);
    let edges = if c.ink > 0.0 { Some(edge_map(&shown, w, h, line)) } else { None };
    let cell = c.dot_cell(h as f32);
    let ink = c.ink.clamp(0.0, 1.0);
    img.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let s = shown[i];
            let col = [s[0], s[1], s[2]];
            let l = luma(col);
            let mut flat = comic_tone(col, c.tones);
            if c.dots {
                let m = dot(x as f32 + 0.5, y as f32 + 0.5, cell, l);
                flat = [0, 1, 2].map(|k| lerp((flat[k] * 1.25 + 0.06).clamp(0.0, 1.0), flat[k] * 0.12, m));
            }
            if let Some(e) = &edges {
                // Fewer, stronger edges at a low ink; most edges at 1.
                let threshold = lerp(0.42, 0.1, ink);
                let k = ((e[i] - threshold) * 7.0).clamp(0.0, 1.0);
                flat = [0, 1, 2].map(|j| lerp(flat[j], INK[j], k));
            }
            // A layer's transparent pixels stay so; its outline (where alpha changes) is inked over what it holds.
            let a = s[3].max(p[3]);
            *p = [decode(flat[0]) * a, decode(flat[1]) * a, decode(flat[2]) * a, a];
        }
    });
}

/// How sharply the picture changes across each pixel, `line` pixels either side (the comic's ink lines): the jumps of
/// its luma and, more strongly, of its alpha (a layer's outline), over a slightly softened copy.
fn edge_map(shown: &[[f32; 4]], w: usize, h: usize, line: f32) -> Vec<f32> {
    let mut soft = Image { width: w as u32, height: h as u32, data: shown.iter().map(|s| [luma([s[0], s[1], s[2]]) * s[3], s[3], 0.0, 0.0]).collect() };
    super::blur::gaussian(&mut soft, (line * 0.6).max(0.6));
    let t = line.round().max(1.0) as i64;
    let at = |x: i64, y: i64| soft.data[(y.clamp(0, h as i64 - 1) as usize) * w + x.clamp(0, w as i64 - 1) as usize];
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let y = y as i64;
        for (x, o) in row.iter_mut().enumerate() {
            let x = x as i64;
            let (l, r, u, d) = (at(x - t, y), at(x + t, y), at(x, y - t), at(x, y + t));
            let jump = (l[0] - r[0]).abs() + (u[0] - d[0]).abs();
            let outline = (l[1] - r[1]).abs() + (u[1] - d[1]).abs();
            *o = jump.max(outline * 1.5);
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::photo::{to_linear, to_srgb};

    #[test]
    fn the_vignette_darkens_the_edges_around_its_centre() {
        let v = Vignette { amount: 1.0, ..Default::default() };
        assert!((vignette_shade(&v, 0.5, 0.5, 1.0) - 1.0).abs() < 1e-6);
        assert!(vignette_shade(&v, 0.95, 0.5, 1.0) < vignette_shade(&v, 0.7, 0.5, 1.0));
        // Moved: the light follows its centre.
        let moved = Vignette { center: [0.2, 0.3], ..v.clone() };
        assert!((vignette_shade(&moved, 0.2, 0.3, 1.6) - 1.0).abs() < 1e-6);
        assert!(vignette_shade(&moved, 0.8, 0.8, 1.6) < 0.2);
        let mut img = Image::filled(40, 20, [0.5, 0.5, 0.5, 1.0]);
        vignette(&mut img, &moved);
        let at = |x: usize, y: usize| img.data[y * 40 + x][0];
        assert!(at(8, 6) > at(30, 15) && at(8, 6) > 0.45);
        // Below zero, the edges go light.
        let mut img = Image::filled(40, 20, [0.2, 0.2, 0.2, 1.0]);
        vignette(&mut img, &Vignette { amount: -1.0, ..Default::default() });
        assert!(img.data[0][0] > 0.9 && (img.data[10 * 40 + 20][0] - 0.2).abs() < 0.01);
    }

    #[test]
    fn aberration_pulls_red_out_and_blue_in_by_its_share() {
        let a = Aberration { amount: 1.0, center: [0.5, 0.5] };
        let (r, b) = aberration_offsets(&a, 0.9, 0.5);
        assert!((r[0] - (0.9 - 0.4 * 0.012)).abs() < 1e-6 && (b[0] - (0.9 + 0.4 * 0.012)).abs() < 1e-6);
        assert_eq!(r[1], 0.5);
        let (r, b) = aberration_offsets(&a, 0.5, 0.5);
        assert_eq!((r, b), ([0.5, 0.5], [0.5, 0.5]));
        let (r, _) = aberration_offsets(&Aberration { amount: 3.0, center: [0.0, 0.0] }, 1.0, 1.0);
        assert!((r[0] - (1.0 - 0.036)).abs() < 1e-6);
        // A white bar right of the centre: its red shows farther out, its blue nearer the middle.
        let mut img = Image::filled(200, 10, [0.0, 0.0, 0.0, 1.0]);
        for y in 0..10 {
            for x in 150..160 {
                img.data[y * 200 + x] = [1.0, 1.0, 1.0, 1.0];
            }
        }
        aberration(&mut img, &Aberration { amount: 3.0, center: [0.5, 0.5] });
        let row = |x: usize| img.data[5 * 200 + x];
        // Shift 0.036 of 0.5 of the width: about 3.6 pixels at x = 155.
        assert!(row(161)[0] > 0.9 && row(161)[2] < 0.1, "{:?}", row(161));
        assert!(row(149)[2] > 0.9 && row(149)[0] < 0.1, "{:?}", row(149));
    }

    #[test]
    fn comic_tones_step_brightness_and_keep_hue() {
        // Four tones: 0, 1/3, 2/3, 1. A grey of 0.4 falls to a third.
        let g = comic_tone([0.4, 0.4, 0.4], 4);
        assert!(g.iter().all(|v| (v - 1.0 / 3.0).abs() < 1e-5), "{g:?}");
        let g = comic_tone([0.9, 0.9, 0.9], 4);
        assert!(g.iter().all(|v| (v - 1.0).abs() < 1e-5), "{g:?}");
        let g = comic_tone([0.55, 0.55, 0.55], 2);
        assert!(g.iter().all(|v| (v - 1.0).abs() < 1e-5), "{g:?}");
        // Black keeps a little light (the Toolkit's 0.05), a colour its hue.
        assert!(comic_tone([0.0, 0.0, 0.0], 4).iter().all(|v| *v < 0.01));
        let red = comic_tone([0.6, 0.2, 0.2], 4);
        assert!(red[0] > red[1] * 2.0 && (red[1] - red[2]).abs() < 1e-5, "{red:?}");
        // Every tone count gives at most that many greys (black aside).
        for tones in [2u32, 5, 12] {
            let mut levels: Vec<i32> = (1..=100).map(|i| (comic_tone([i as f32 / 100.0; 3], tones)[0] * 1000.0).round() as i32).collect();
            levels.dedup();
            levels.sort();
            levels.dedup();
            assert!(levels.len() <= tones as usize, "{tones}: {levels:?}");
        }
    }

    #[test]
    fn dots_cover_as_much_as_the_tone_is_dark() {
        let cell = 12.0;
        let coverage = |l: f32| {
            let mut sum = 0.0;
            for y in 0..120 {
                for x in 0..120 {
                    sum += dot(x as f32 + 0.5, y as f32 + 0.5, cell, l);
                }
            }
            sum / (120.0 * 120.0)
        };
        assert!(coverage(1.0) < 0.02);
        let mid = coverage(0.5);
        assert!((mid - 0.5).abs() < 0.12, "{mid}");
        assert!(coverage(0.2) > mid && coverage(0.8) < mid);
        // The Toolkit's dot spacing: a 260th to a 36th of the height, at least 3 pixels.
        assert!((Comic { dot_size: 0.0, ..Default::default() }.dot_cell(2160.0) - 2160.0 / 260.0).abs() < 1e-3);
        assert!((Comic { dot_size: 1.0, ..Default::default() }.dot_cell(2160.0) - 60.0).abs() < 1e-3);
        assert_eq!(Comic::default().dot_cell(100.0), 3.0);
    }

    #[test]
    fn the_comic_inks_edges_and_leaves_flat_parts_flat() {
        let (w, h) = (60usize, 40usize);
        let mut img = Image::filled(w as u32, h as u32, [to_linear(0.4), to_linear(0.4), to_linear(0.4), 1.0]);
        for y in 0..h {
            for x in 30..w {
                img.data[y * w + x] = [to_linear(0.95), to_linear(0.95), to_linear(0.95), 1.0];
            }
        }
        comic(&mut img, &Comic { ink: 1.0, ..Default::default() });
        let shown = |x: usize| to_srgb(img.data[20 * w + x][0]);
        assert!((shown(5) - 1.0 / 3.0).abs() < 0.01, "{}", shown(5));
        assert!((shown(55) - 1.0).abs() < 0.01, "{}", shown(55));
        assert!(shown(30) < 0.15 || shown(29) < 0.15, "an ink line at the edge: {} {}", shown(29), shown(30));
        // A transparent layer's outline is inked, its empty parts left empty.
        let mut layer = Image::new(40, 40);
        for y in 10..30 {
            for x in 10..30 {
                layer.data[y * 40 + x] = [0.8, 0.8, 0.8, 1.0];
            }
        }
        comic(&mut layer, &Comic { ink: 0.6, ..Default::default() });
        assert_eq!(layer.data[0], [0.0; 4]);
        assert!(to_srgb(layer.data[20 * 40 + 10][0]) < 0.2, "{:?}", layer.data[20 * 40 + 10]);
        assert!(to_srgb(layer.data[20 * 40 + 20][0]) > 0.9);
    }
}
