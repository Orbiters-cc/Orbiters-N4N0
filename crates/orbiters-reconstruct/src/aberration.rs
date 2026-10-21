//! Lab: the world's chromatic aberration in a session's colour frames (Post Processing Stack v2, what VRChat worlds
//! use). PPv2 takes each pixel's colour from the segment between the pixel and `diff = −c·|c|²·0.05·intensity` towards
//! the centre (`c = 2·uv − 1`), weighting red near the pixel, green in the middle and blue at the far end (3 to 16
//! taps, its default spectral LUT). A model trained on such frames sees every scene point with view-dependent colour
//! fringes towards the frame's edges.
//!
//! - [`measure`]: the intensity, from the pictures alone: the blue channel's shift against the red one in textured
//!   patches follows `0.64·diff`.
//! - [`realign`]: each channel resampled from where PPv2 took it (first order: shifts, not the blur along the
//!   segment); the [`offsets`] are the same for every frame.
//! - [`aberrate`]: PPv2's effect on a render, to score a model of the realigned world against the captured frames.
//!
//! Offsets are in training pixels; the tap count uses the training size (the capture's at full width).

use crate::dataset::{channel, View};
use orbiters_contracts::color;
use rayon::prelude::*;

/// PPv2's default spectral LUT (red, green, blue texels; bilinear, clamped) at t.
pub fn spectral(t: f32) -> [f32; 3] {
    let x = (t * 3.0 - 0.5).clamp(0.0, 2.0);
    let i = (x.floor() as usize).min(1);
    let f = x - i as f32;
    let mut out = [0.0; 3];
    out[i] += 1.0 - f;
    out[i + 1] += f;
    out
}

/// `diff` at pixel position (x, y) (pixel centres at integers + 0.5), in pixels.
fn diff(x: f32, y: f32, w: usize, h: usize, intensity: f32) -> (f32, f32) {
    let (cx, cy) = (2.0 * x / w as f32 - 1.0, 2.0 * y / h as f32 - 1.0);
    let d2 = cx * cx + cy * cy;
    let amount = 0.05 * intensity;
    (-cx * d2 * amount * w as f32, -cy * d2 * amount * h as f32)
}

/// PPv2's taps at a pixel: (fraction of `diff`, spectral weights).
fn taps(dx: f32, dy: f32) -> impl Iterator<Item = (f32, [f32; 3])> {
    let n = (((dx * dx + dy * dy).sqrt() / 2.0) as i32).clamp(3, 16);
    (0..n).map(move |i| (i as f32 / n as f32, spectral((i as f32 + 0.5) / n as f32)))
}

/// Where channel `ch` (0 red, 1 green, 2 blue) at (x, y) takes its colour from, relative to (x, y): the weighted mean
/// of its taps.
pub fn channel_offset(ch: usize, x: f32, y: f32, w: usize, h: usize, intensity: f32) -> (f32, f32) {
    let (dx, dy) = diff(x, y, w, h, intensity);
    let (mut t, mut sum) = (0.0f32, 0.0f32);
    for (f, s) in taps(dx, dy) {
        t += s[ch] * f;
        sum += s[ch];
    }
    let t = t / sum.max(1e-6);
    (t * dx, t * dy)
}

/// Bilinear sample of one channel of a packed keyframe at pixel coordinates (centres at integers + 0.5), clamped.
fn sample(image: &[u32], w: usize, h: usize, ch: usize, x: f32, y: f32) -> f32 {
    let (x, y) = ((x - 0.5).clamp(0.0, (w - 1) as f32), (y - 0.5).clamp(0.0, (h - 1) as f32));
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let at = |xx: usize, yy: usize| channel(image[yy * w + xx], ch);
    let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
    let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// Per pixel, where each channel takes its colour from (the same for every frame of a session): `channel_offset`
/// for red, green and blue.
pub fn offsets(w: usize, h: usize, intensity: f32) -> Vec<[(f32, f32); 3]> {
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            [0, 1, 2].map(|ch| channel_offset(ch, x, y, w, h, intensity))
        })
        .collect()
}

/// A keyframe with each channel moved back to where PPv2 took it from (alpha kept), given [`offsets`].
pub fn realign(image: &[u32], w: usize, h: usize, offsets: &[[(f32, f32); 3]]) -> Vec<u32> {
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            let mut packed = image[i] & 0xFF00_0000;
            for (ch, &(ox, oy)) in offsets[i].iter().enumerate() {
                let v = sample(image, w, h, ch, x - ox, y - oy);
                packed |= ((v * 255.0 + 0.5).clamp(0.0, 255.0) as u32) << (8 * ch);
            }
            packed
        })
        .collect()
}

/// PPv2's chromatic aberration applied to a render (encoded sRGB colours; blended in linear light).
pub fn aberrate(img: &[[f32; 4]], w: usize, h: usize, intensity: f32) -> Vec<[f32; 4]> {
    let lin: Vec<[f32; 3]> = img.iter().map(|p| [0, 1, 2].map(|c| color::srgb_to_linear(p[c].clamp(0.0, 1.0)))).collect();
    let at = |ch: usize, x: f32, y: f32| {
        let (x, y) = ((x - 0.5).clamp(0.0, (w - 1) as f32), (y - 0.5).clamp(0.0, (h - 1) as f32));
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let top = lin[y0 * w + x0][ch] * (1.0 - fx) + lin[y0 * w + x1][ch] * fx;
        let bottom = lin[y1 * w + x0][ch] * (1.0 - fx) + lin[y1 * w + x1][ch] * fx;
        top * (1.0 - fy) + bottom * fy
    };
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            let (dx, dy) = diff(x, y, w, h, intensity);
            let (mut sum, mut wsum) = ([0f32; 3], [0f32; 3]);
            for (f, s) in taps(dx, dy) {
                for ch in 0..3 {
                    sum[ch] += s[ch] * at(ch, x + f * dx, y + f * dy);
                    wsum[ch] += s[ch];
                }
            }
            let c = [0, 1, 2].map(|ch| color::linear_to_srgb(sum[ch] / wsum[ch].max(1e-6)));
            [c[0], c[1], c[2], img[i][3]]
        })
        .collect()
}

/// A measurement: PPv2 intensity (least squares over all reliable patches), the median of the outer patches' own
/// estimates, the patch count, and per radius band (0.2 wide from the centre) the mean radial blue−red shift.
pub struct Measurement {
    pub intensity: f32,
    pub median_outer: f32,
    pub patches: usize,
    pub profile: Vec<(f32, usize, f32)>,
}

/// Estimates the intensity from every `every`-th keyframe: in a grid of 24 × 14 textured 49×49 patches inside the
/// scene, the sub-pixel shift that best maps red onto blue (normalised cross-correlation ≥ 0.8, no other peak within
/// 0.03 three or more pixels away), fitted to `blue − red = 0.64·diff`.
pub fn measure(views: &[View], mask: &[f32], w: usize, h: usize, every: usize) -> Measurement {
    let (gx, gy, half, search) = (24usize, 14usize, 24i64, 14i64);
    let mut rows: Vec<(f32, f32, f32, f32)> = Vec::new();
    for v in views.iter().step_by(every.max(1)) {
        let found: Vec<Option<(f32, f32, f32, f32)>> = (0..gx * gy)
            .into_par_iter()
            .map(|k| {
                let (x, y) = ((((k % gx) as f32 + 0.5) * w as f32 / gx as f32) as i64, (((k / gx) as f32 + 0.5) * h as f32 / gy as f32) as i64);
                let r = half + search + 1;
                if x - r < 0 || y - r < 0 || x + r >= w as i64 || y + r >= h as i64 {
                    return None;
                }
                for yy in (y - r..=y + r).step_by(4) {
                    for xx in (x - r..=x + r).step_by(4) {
                        if mask[yy as usize * w + xx as usize] < 0.5 {
                            return None;
                        }
                    }
                }
                let s = best_shift(&v.image, w, x, y, half, search)?;
                Some((2.0 * x as f32 / w as f32 - 1.0, 2.0 * y as f32 / h as f32 - 1.0, s.0, s.1))
            })
            .collect();
        rows.extend(found.into_iter().flatten());
    }
    let (mut num, mut den) = (0f64, 0f64);
    let mut outer: Vec<f32> = Vec::new();
    for &(cx, cy, sx, sy) in &rows {
        let d2 = cx * cx + cy * cy;
        // Per unit intensity: 0.64 · diff.
        let (ex, ey) = (-cx * d2 * 0.05 * w as f32 * 0.64, -cy * d2 * 0.05 * h as f32 * 0.64);
        let ee = ex * ex + ey * ey;
        num += (sx * ex + sy * ey) as f64;
        den += ee as f64;
        if d2.sqrt() > 0.6 {
            outer.push((sx * ex + sy * ey) / ee);
        }
    }
    outer.sort_by(f32::total_cmp);
    let mut profile = Vec::new();
    for b in 0..7 {
        let lo = b as f32 * 0.2;
        let sel: Vec<_> = rows.iter().filter(|r| (lo..lo + 0.2).contains(&(r.0 * r.0 + r.1 * r.1).sqrt())).collect();
        if sel.is_empty() {
            continue;
        }
        let radial: f32 = sel
            .iter()
            .map(|&&(cx, cy, sx, sy)| {
                let (ux, uy) = (cx * w as f32, cy * h as f32);
                (sx * ux + sy * uy) / (ux * ux + uy * uy).sqrt().max(1e-6)
            })
            .sum();
        profile.push((lo, sel.len(), radial / sel.len() as f32));
    }
    Measurement { intensity: (num / den.max(1e-12)) as f32, median_outer: outer.get(outer.len() / 2).copied().unwrap_or(0.0), patches: rows.len(), profile }
}

/// Sub-pixel s with blue(x) ≈ red(x + s) around (x, y); None for flat or ambiguous patches.
fn best_shift(image: &[u32], w: usize, x: i64, y: i64, half: i64, search: i64) -> Option<(f32, f32)> {
    let n = (2 * half + 1) as usize;
    let patch = |ch: usize, ox: i64, oy: i64| -> Vec<f32> {
        let mut v = Vec::with_capacity(n * n);
        for yy in -half..=half {
            for xx in -half..=half {
                v.push(channel(image[((y + yy + oy) as usize) * w + (x + xx + ox) as usize], ch));
            }
        }
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        v.iter_mut().for_each(|a| *a -= mean);
        v
    };
    let blue = patch(2, 0, 0);
    let bn = blue.iter().map(|a| a * a).sum::<f32>().sqrt();
    if bn / (n as f32) < 0.02 {
        return None;
    }
    let size = (2 * search + 1) as usize;
    let mut score = vec![f32::MIN; size * size];
    for sy in -search..=search {
        for sx in -search..=search {
            let red = patch(0, sx, sy);
            let rn = red.iter().map(|a| a * a).sum::<f32>().sqrt();
            if rn / (n as f32) < 0.02 {
                continue;
            }
            let dot: f32 = red.iter().zip(&blue).map(|(a, b)| a * b).sum();
            score[((sy + search) as usize) * size + (sx + search) as usize] = dot / (rn * bn);
        }
    }
    let (best, &peak) = score.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    let (bx, by) = ((best % size) as i64 - search, (best / size) as i64 - search);
    if peak < 0.8 || bx.abs() == search || by.abs() == search {
        return None;
    }
    for (i, &s) in score.iter().enumerate() {
        let (ix, iy) = ((i % size) as i64 - search, (i / size) as i64 - search);
        if (ix - bx).abs().max((iy - by).abs()) >= 3 && s > peak - 0.03 {
            return None;
        }
    }
    let at = |dx: i64, dy: i64| score[((by + dy + search) as usize) * size + (bx + dx + search) as usize];
    let parabola = |a: f32, b: f32, c: f32| {
        let d = a - 2.0 * b + c;
        if d.abs() < 1e-9 {
            0.0
        } else {
            (0.5 * (a - c) / d).clamp(-0.5, 0.5)
        }
    };
    Some((bx as f32 + parabola(at(-1, 0), peak, at(1, 0)), by as f32 + parabola(at(0, -1), peak, at(0, 1))))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture of random blocks aberrated by PPv2 measures back as its intensity, and realigning it brings red, green
    /// and blue back over each other.
    #[test]
    fn measures_and_realigns_ppv2() {
        let (w, h) = (640usize, 360usize);
        let mut rng = fastrand::Rng::with_seed(3);
        let blocks: Vec<f32> = (0..(w / 8) * (h / 8)).map(|_| rng.f32()).collect();
        let grey: Vec<[f32; 4]> = (0..w * h).map(|i| { let v = blocks[(i / w / 8) * (w / 8) + (i % w) / 8]; [v, v, v, 1.0] }).collect();
        let intensity = 0.3;
        let ab = aberrate(&grey, w, h, intensity);
        let packed: Vec<u32> = ab.iter().map(|p| [0, 1, 2].iter().fold(255u32 << 24, |acc, &c| acc | (((p[c] * 255.0 + 0.5) as u32) << (8 * c)))).collect();
        let views = vec![View { camera: crate::camera::Camera::look([0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 1.0, w as u32, h as u32), image: packed.clone(), game_time: 0.0, keyframe: 0 }];
        let m = measure(&views, &vec![1.0; w * h], w, h, 1);
        assert!(m.patches > 20, "{} patches", m.patches);
        assert!((m.intensity - intensity).abs() < 0.05, "measured {}", m.intensity);
        // Red against blue, away from the centre, before and after realigning.
        let fixed = realign(&packed, w, h, &offsets(w, h, intensity));
        let spread = |img: &[u32]| -> f32 {
            let mut e = 0.0;
            for y in 0..h {
                for x in 0..w {
                    let (cx, cy) = (2.0 * x as f32 / w as f32 - 1.0, 2.0 * y as f32 / h as f32 - 1.0);
                    if (cx * cx + cy * cy).sqrt() > 0.7 {
                        e += (channel(img[y * w + x], 0) - channel(img[y * w + x], 2)).abs();
                    }
                }
            }
            e
        };
        assert!(spread(&fixed) < 0.5 * spread(&packed), "{} vs {}", spread(&fixed), spread(&packed));
    }
}
