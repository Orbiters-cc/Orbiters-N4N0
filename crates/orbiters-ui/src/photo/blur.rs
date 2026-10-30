//! Blurs of a working picture (premultiplied, so transparent edges do not bleed dark): a gaussian (an exact kernel
//! when small, three box blurs of the right widths when large, which is as close and costs the same at any radius), a
//! lens's disc (each row's running sums give a disc row by row, its edge antialiased, bright points weighted up for
//! bokeh) and a directional one along a line. Radii are in pixels here; the layer gives them as shares of the height.

use super::Image;
use rayon::prelude::*;

/// The gaussian of deviation `sigma` pixels.
pub fn gaussian(img: &mut Image, sigma: f32) {
    if sigma < 0.3 || img.width == 0 || img.height == 0 {
        return;
    }
    if sigma <= 3.0 {
        let kernel = gaussian_kernel(sigma);
        rows(img, |row, out| convolve(row, out, &kernel));
        transpose(img);
        rows(img, |row, out| convolve(row, out, &kernel));
        transpose(img);
    } else {
        let widths = box_widths(sigma, 3);
        // Along the rows, then (turned) down the columns, then turned back.
        for _ in 0..2 {
            for &w in &widths {
                rows(img, |row, out| box_row(row, out, (w - 1) / 2));
            }
            transpose(img);
        }
    }
}

/// The weights of a gaussian of deviation `sigma`, out to three deviations, summing to one.
pub fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let r = (sigma * 3.0).ceil().max(1.0) as i32;
    let mut k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    k
}

/// The odd widths of `n` box blurs in a row whose result is closest to a gaussian of deviation `sigma` (Kovesi, "Fast
/// almost-Gaussian filtering"): their variances add up to about `sigma`².
pub fn box_widths(sigma: f32, n: usize) -> Vec<usize> {
    let ideal = ((12.0 * sigma * sigma / n as f32) + 1.0).sqrt();
    let mut lower = ideal.floor() as usize;
    if lower.is_multiple_of(2) {
        lower -= 1;
    }
    let lower = lower.max(1);
    let upper = lower + 2;
    let l = lower as f32;
    let m_ideal = (12.0 * sigma * sigma - n as f32 * l * l - 4.0 * n as f32 * l - 3.0 * n as f32) / (-4.0 * l - 4.0);
    let m = m_ideal.round().clamp(0.0, n as f32) as usize;
    (0..n).map(|i| if i < m { lower } else { upper }).collect()
}

/// Each row through `f` (row in, row out), in parallel.
fn rows(img: &mut Image, f: impl Fn(&[[f32; 4]], &mut [[f32; 4]]) + Sync) {
    let w = img.width as usize;
    let mut out = vec![[0f32; 4]; img.data.len()];
    out.par_chunks_mut(w).zip(img.data.par_chunks(w)).for_each(|(o, i)| f(i, o));
    img.data = out;
}

/// Swaps rows and columns (so that a pass along rows also runs down the columns).
fn transpose(img: &mut Image) {
    let (w, h) = (img.width as usize, img.height as usize);
    let mut out = vec![[0f32; 4]; img.data.len()];
    let data = &img.data;
    out.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        for (y, p) in col.iter_mut().enumerate() {
            *p = data[y * w + x];
        }
    });
    img.data = out;
    std::mem::swap(&mut img.width, &mut img.height);
}

fn convolve(row: &[[f32; 4]], out: &mut [[f32; 4]], kernel: &[f32]) {
    let r = (kernel.len() / 2) as i64;
    let n = row.len() as i64;
    for (x, o) in out.iter_mut().enumerate() {
        let mut s = [0f32; 4];
        for (k, w) in kernel.iter().enumerate() {
            let p = row[(x as i64 + k as i64 - r).clamp(0, n - 1) as usize];
            for c in 0..4 {
                s[c] += p[c] * w;
            }
        }
        *o = s;
    }
}

/// A box of `r` pixels each side, by a running sum (the edges held).
fn box_row(row: &[[f32; 4]], out: &mut [[f32; 4]], r: usize) {
    let n = row.len() as i64;
    let r = r as i64;
    let at = |i: i64| row[i.clamp(0, n - 1) as usize];
    let scale = 1.0 / (2 * r + 1) as f32;
    let mut sum = [0f32; 4];
    for i in -r..=r {
        let p = at(i);
        for c in 0..4 {
            sum[c] += p[c];
        }
    }
    for x in 0..n {
        out[x as usize] = sum.map(|v| v * scale);
        let (add, sub) = (at(x + r + 1), at(x - r));
        for c in 0..4 {
            sum[c] += add[c] - sub[c];
        }
    }
}

/// A lens's blur: every pixel spread over a disc of `radius` pixels, a bright one (`bokeh` 0 to 1) more than the rest,
/// as out-of-focus highlights show as discs.
pub fn lens(img: &mut Image, radius: f32, bokeh: f32) {
    if radius < 0.5 || img.width == 0 || img.height == 0 {
        return;
    }
    let (w, h) = (img.width as usize, img.height as usize);
    let r = radius.ceil() as usize;
    // Each pixel weighted by its brightness: [colour × weight, alpha × weight] and the weight.
    let boost = bokeh.clamp(0.0, 1.0) * 12.0;
    let weighted: Vec<[f32; 5]> = img
        .data
        .par_iter()
        .map(|p| {
            let a = p[3].max(1e-6);
            let bright = (p[0].max(p[1]).max(p[2]) / a - 0.6).max(0.0);
            let k = 1.0 + boost * bright * bright;
            [p[0] * k, p[1] * k, p[2] * k, p[3] * k, k]
        })
        .collect();
    // Running sums along each row, the row held past its ends by `r` pixels.
    let span = w + 2 * r;
    let mut sums = vec![[0f32; 5]; (span + 1) * h];
    sums.par_chunks_mut(span + 1).enumerate().for_each(|(y, s)| {
        let row = &weighted[y * w..(y + 1) * w];
        let mut acc = [0f32; 5];
        for i in 0..span {
            let p = row[(i as i64 - r as i64).clamp(0, w as i64 - 1) as usize];
            for c in 0..5 {
                acc[c] += p[c];
            }
            s[i + 1] = acc;
        }
    });
    // The disc row by row: its half width at each height, the part pixel at its ends counted in part.
    let rows: Vec<(i64, usize, f32)> = (-(r as i64)..=r as i64)
        .filter_map(|dy| {
            let half = (radius * radius - (dy * dy) as f32).max(0.0).sqrt();
            if half <= 0.0 && dy != 0 {
                return None;
            }
            Some((dy, half.floor() as usize, half - half.floor()))
        })
        .collect();
    let row_of = |y: i64| y.clamp(0, h as i64 - 1) as usize * (span + 1);
    let value = |base: usize, i: usize| -> [f32; 5] {
        // The pixel `i` of the held row (prefix difference).
        let (a, b) = (sums[base + i + 1], sums[base + i]);
        std::array::from_fn(|c| a[c] - b[c])
    };
    img.data.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
        for (x, o) in out.iter_mut().enumerate() {
            let mut acc = [0f32; 5];
            for &(dy, k, frac) in &rows {
                let base = row_of(y as i64 + dy);
                // Held row coordinates: pixel x is at x + r.
                let (lo, hi) = (x + r - k, x + r + k);
                let (a, b) = (sums[base + hi + 1], sums[base + lo]);
                for c in 0..5 {
                    acc[c] += a[c] - b[c];
                }
                if frac > 0.0 && k < r {
                    let (left, right) = (value(base, lo - 1), value(base, hi + 1));
                    for c in 0..5 {
                        acc[c] += (left[c] + right[c]) * frac;
                    }
                }
            }
            let k = acc[4].max(1e-12);
            *o = [acc[0] / k, acc[1] / k, acc[2] / k, acc[3] / k];
        }
    });
}

/// Along a line `length` pixels long at `angle` degrees through each pixel (a camera moving that way).
pub fn directional(img: &mut Image, length: f32, angle: f32) {
    if length < 1.0 || img.width == 0 || img.height == 0 {
        return;
    }
    let (s, c) = angle.to_radians().sin_cos();
    let taps = (length.ceil() as usize + 1).max(2);
    let src = img.clone();
    let w = img.width as usize;
    img.data.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
        for (x, o) in out.iter_mut().enumerate() {
            let mut acc = [0f32; 4];
            for t in 0..taps {
                let d = (t as f32 / (taps - 1) as f32 - 0.5) * length;
                let p = src.sample(x as f32 + 0.5 + c * d, y as f32 + 0.5 + s * d);
                for k in 0..4 {
                    acc[k] += p[k];
                }
            }
            *o = acc.map(|v| v / taps as f32);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One white pixel in the middle of a black picture.
    fn impulse(size: u32) -> Image {
        let mut img = Image::filled(size, size, [0.0, 0.0, 0.0, 1.0]);
        let c = (size / 2) as usize;
        img.data[c * size as usize + c] = [1.0, 1.0, 1.0, 1.0];
        img
    }

    fn red(img: &Image) -> Vec<f32> {
        img.data.iter().map(|p| p[0]).collect()
    }

    /// The impulse response's total and its variance along x.
    fn spread(img: &Image) -> (f32, f32) {
        let c = (img.width / 2) as f32;
        let (mut total, mut var) = (0f32, 0f32);
        for (i, p) in img.data.iter().enumerate() {
            let x = (i % img.width as usize) as f32 - c;
            total += p[0];
            var += p[0] * x * x;
        }
        (total, var / total)
    }

    #[test]
    fn gaussian_kernels_sum_to_one_with_their_deviation() {
        for sigma in [0.6f32, 1.0, 2.5] {
            let k = gaussian_kernel(sigma);
            assert!((k.iter().sum::<f32>() - 1.0).abs() < 1e-5);
            let r = (k.len() / 2) as f32;
            let var: f32 = k.iter().enumerate().map(|(i, w)| w * (i as f32 - r).powi(2)).sum();
            assert!((var.sqrt() - sigma).abs() < 0.05 * sigma, "{sigma}: {}", var.sqrt());
        }
        // Three boxes whose variances add up to sigma².
        for sigma in [4.0f32, 9.5, 30.0] {
            let w = box_widths(sigma, 3);
            let var: f32 = w.iter().map(|&w| ((w * w) as f32 - 1.0) / 12.0).sum();
            assert!(w.iter().all(|w| w % 2 == 1), "{w:?}");
            assert!((var.sqrt() - sigma).abs() < 0.08 * sigma, "{sigma}: {w:?} gives {}", var.sqrt());
        }
    }

    #[test]
    fn gaussian_blurs_keep_light_and_spread_it_by_sigma() {
        for sigma in [1.5f32, 6.0] {
            let mut img = impulse(81);
            gaussian(&mut img, sigma);
            let (total, var) = spread(&img);
            assert!((total - 1.0).abs() < 1e-3, "{total}");
            assert!((var.sqrt() - sigma).abs() < 0.1 * sigma, "{sigma}: {}", var.sqrt());
            // Round: as wide down as across.
            let c = 40usize;
            assert!((img.data[c * 81 + c + 4][0] - img.data[(c + 4) * 81 + c][0]).abs() < 1e-4);
        }
        // A flat picture stays flat.
        let mut flat = Image::filled(20, 10, [0.3, 0.2, 0.1, 1.0]);
        gaussian(&mut flat, 5.0);
        assert!(flat.data.iter().all(|p| (p[0] - 0.3).abs() < 1e-5 && (p[3] - 1.0).abs() < 1e-5));
    }

    #[test]
    fn a_lens_spreads_a_point_over_a_disc() {
        let mut img = impulse(41);
        lens(&mut img, 8.0, 0.0);
        let v = red(&img);
        let at = |dx: i32, dy: i32| v[((20 + dy) * 41 + 20 + dx) as usize];
        // Even inside the disc, nothing beyond it; the light kept.
        assert!((at(0, 0) - at(5, 0)).abs() < 1e-6 && (at(0, 0) - at(0, -5)).abs() < 1e-6 && (at(3, 4) - at(0, 0)).abs() < 1e-6);
        assert_eq!(at(9, 0), 0.0);
        assert_eq!(at(7, 7), 0.0);
        assert!((v.iter().sum::<f32>() - 1.0).abs() < 1e-3, "{}", v.iter().sum::<f32>());
        // Disc area: about pi r².
        let lit = v.iter().filter(|x| **x > 0.0).count() as f32;
        assert!((lit - std::f32::consts::PI * 64.0).abs() < 40.0, "{lit}");
    }

    #[test]
    fn bokeh_lets_bright_points_outweigh_the_rest() {
        let mut plain = Image::filled(21, 21, [0.2, 0.2, 0.2, 1.0]);
        plain.data[10 * 21 + 10] = [4.0, 4.0, 4.0, 1.0];
        let mut weighted = plain.clone();
        lens(&mut plain, 5.0, 0.0);
        lens(&mut weighted, 5.0, 1.0);
        assert!(weighted.data[10 * 21 + 12][0] > plain.data[10 * 21 + 12][0] * 2.0);
    }

    #[test]
    fn a_directional_blur_moves_light_along_its_line_only() {
        let mut img = impulse(31);
        directional(&mut img, 10.0, 0.0);
        let v = red(&img);
        assert!(v[15 * 31 + 19] > 0.05 && v[15 * 31 + 11] > 0.05, "along the width");
        assert!(v[19 * 31 + 15] < 1e-6 && v[11 * 31 + 15] < 1e-6, "not down");
        assert!((v.iter().sum::<f32>() - 1.0).abs() < 0.02);
        let mut img = impulse(31);
        directional(&mut img, 10.0, 90.0);
        let v = red(&img);
        assert!(v[19 * 31 + 15] > 0.05 && v[15 * 31 + 19] < 1e-6, "turned down");
    }
}
