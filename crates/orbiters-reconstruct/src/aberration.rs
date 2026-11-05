//! The world's chromatic aberration in a session's colour frames (Post Processing Stack v2, what VRChat worlds use).
//! PPv2 takes each pixel's colour from the segment between the pixel and `diff = −c·|c|²·0.05·intensity` towards
//! the centre (`c = 2·uv − 1`), weighting red near the pixel, green in the middle and blue at the far end (3 to 16
//! taps, its default spectral LUT). A model trained on such frames sees every scene point with view-dependent colour
//! fringes towards the frame's edges, and bakes them into Gaussians as colour noise.
//!
//! - [`measure`]: the intensity, from the pictures alone: the blue channel's shift against the red one in textured
//!   patches follows `0.64·diff`.
//! - [`realign`]: each channel resampled from where PPv2 took it (first order: shifts, not the blur along the
//!   segment); the [`offsets`] are the same for every frame.
//! - [`undo`]: both, on a session's keyframes, when the measurement holds together. Measured +0.24–0.30 dB and up to
//!   +0.022 SSIM on two aberrated worlds (held-out renders passed through the same aberration against the captured
//!   frames), and the speckle in foliage gone. Pixels whose colour realigning would take from outside the frame leave
//!   the loss mask.
//! - [`aberrate`]: the aberration itself, to score renders against the keyframes as captured (crate::train).
//!
//! Offsets are in training pixels; PPv2 counts its taps in captured pixels. It all runs on the loading threads
//! (crate::priority), not on every core while VRChat plays.

use crate::dataset::{channel, Dataset, View};
use orbiters_contracts::color;
use rayon::prelude::*;

/// PPv2's default spectral LUT (red, green, blue texels; bilinear, clamped) at t.
fn spectral(t: f32) -> [f32; 3] {
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

/// PPv2's taps at a pixel: (fraction of `diff`, spectral weights). PPv2 counts them from the length of `diff` in the
/// captured frame's pixels: `factor` of them a training pixel.
fn taps(dx: f32, dy: f32, factor: u32) -> impl Iterator<Item = (f32, [f32; 3])> {
    let n = (((dx * dx + dy * dy).sqrt() * factor as f32 / 2.0) as i32).clamp(3, 16);
    (0..n).map(move |i| (i as f32 / n as f32, spectral((i as f32 + 0.5) / n as f32)))
}

/// Where channel `ch` (0 red, 1 green, 2 blue) at (x, y) takes its colour from, relative to (x, y): the weighted mean
/// of its taps.
fn channel_offset(ch: usize, x: f32, y: f32, w: usize, h: usize, intensity: f32, factor: u32) -> (f32, f32) {
    let (dx, dy) = diff(x, y, w, h, intensity);
    let (mut t, mut sum) = (0.0f32, 0.0f32);
    for (f, s) in taps(dx, dy, factor) {
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

/// Per pixel and channel (the same for every frame of a session, captured at `factor` times the training size), how
/// far back realigning reads: the colour PPv2 moved to pixel p came from p + o(p) (`channel_offset`), so the colour
/// that belongs at x sits at the p with p + o(p) = x. Found by fixed-point steps from p = x − o(x): the offset at x
/// itself is off by about 3|o|²/r (up to 1.7 px for blue in a strongly aberrated world's corners).
pub fn offsets(w: usize, h: usize, intensity: f32, factor: u32) -> Vec<[(f32, f32); 3]> {
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            [0, 1, 2].map(|ch| {
                let (mut px, mut py) = (x, y);
                for _ in 0..3 {
                    let (ox, oy) = channel_offset(ch, px, py, w, h, intensity, factor);
                    (px, py) = (x - ox, y - oy);
                }
                (x - px, y - py)
            })
        })
        .collect()
}

/// PPv2's chromatic aberration applied to a render (encoded sRGB colours, blended in linear light; depth kept), for
/// frames captured at `factor` times its size: what the world's camera would have shown, to score against keyframes
/// as captured.
pub fn aberrate(img: &[[f32; 4]], w: usize, h: usize, intensity: f32, factor: u32) -> Vec<[f32; 4]> {
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
            for (f, s) in taps(dx, dy, factor) {
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

/// A measurement: PPv2 intensity (least squares over all reliable patches), the median of the outer patches' own
/// estimates, the patch count, and per radius band (0.2 wide from the centre) the mean radial blue−red shift.
pub struct Measurement {
    pub intensity: f32,
    pub median_outer: f32,
    pub patches: usize,
    pub profile: Vec<(f32, usize, f32)>,
}

/// Below this measured intensity the keyframes are left as they are (clean worlds measure within ±0.005).
pub const MIN_INTENSITY: f32 = 0.02;
/// The strongest aberration PPv2's slider gives.
pub const MAX_INTENSITY: f32 = 1.0;
/// The least-squares fit and the outer patches' median agree at least this well in a measurement that holds
/// together (the lab saw 0.092 against 0.098).
const AGREEMENT: f32 = 0.25;
/// Fewest reliable patches a measurement stands on.
const MIN_PATCHES: usize = 30;

impl Measurement {
    /// The intensity to undo, or why the keyframes stay as captured: too weak to matter, too few textured patches to
    /// tell, a fit the outer patches (where the aberration is strongest) disagree with.
    pub fn verdict(&self) -> Result<f32, String> {
        let (fit, outer) = (self.intensity, self.median_outer);
        if !fit.is_finite() || fit < MIN_INTENSITY {
            return Err(format!("none measured ({fit:.3})"));
        }
        if self.patches < MIN_PATCHES {
            return Err(format!("{fit:.3} measured on only {} patches", self.patches));
        }
        if (fit - outer).abs() > AGREEMENT * fit.max(outer) {
            return Err(format!("the fit ({fit:.3}) and the frame's outer patches ({outer:.3}) disagree"));
        }
        Ok(fit.min(MAX_INTENSITY))
    }
}

/// An undone aberration: its intensity, the captured frames' size over the training size (the taps), and the loss mask
/// before the borders realigning cannot fill were cut from it (scoring against the frames as captured counts them).
#[derive(Clone, Debug)]
pub struct Undo {
    pub intensity: f32,
    pub factor: u32,
    pub mask: Vec<f32>,
}

/// Measures the world's chromatic aberration on about a dozen of the session's keyframes and, when there is any and
/// the measurement holds together ([`Measurement::verdict`]), realigns every keyframe and leaves the pixels whose
/// colour would come from outside the frame (stretched edge colours) out of the loss mask. Runs on the loading
/// threads (crate::priority).
pub fn undo(data: &mut Dataset) -> Option<Undo> {
    crate::priority::on_loading_threads(|| {
        let (w, h) = (data.width as usize, data.height as usize);
        let every = (data.views.len() / 12).max(1);
        let m = measure(&data.views, &data.mask, w, h, every);
        let intensity = match m.verdict() {
            Ok(i) => i,
            Err(why) => {
                if m.intensity >= MIN_INTENSITY {
                    eprintln!("chromatic aberration left in the keyframes: {why}");
                }
                return None;
            }
        };
        let factor = data.geometry.factor;
        let offsets = offsets(w, h, intensity, factor);
        let mask = data.mask.clone();
        for (i, o) in offsets.iter().enumerate() {
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            if o.iter().any(|&(ox, oy)| x - ox < 0.0 || y - oy < 0.0 || x - ox > w as f32 || y - oy > h as f32) {
                data.mask[i] = 0.0;
            }
        }
        for v in data.views.iter_mut() {
            v.image = realign(&v.image, w, h, &offsets);
        }
        Some(Undo { intensity, factor, mask })
    })
}

/// A keyframe's red channel with summed-area tables of it and its square: any window's mean and spread in four reads.
struct RedPlane {
    w: usize,
    red: Vec<f32>,
    sum: Vec<f64>,
    squares: Vec<f64>,
}

impl RedPlane {
    fn new(image: &[u32], w: usize, h: usize) -> Self {
        let red: Vec<f32> = image.iter().map(|&p| channel(p, 0)).collect();
        let (mut sum, mut squares) = (vec![0f64; (w + 1) * (h + 1)], vec![0f64; (w + 1) * (h + 1)]);
        for y in 0..h {
            let (mut row, mut row2) = (0f64, 0f64);
            for x in 0..w {
                let v = red[y * w + x] as f64;
                row += v;
                row2 += v * v;
                sum[(y + 1) * (w + 1) + x + 1] = sum[y * (w + 1) + x + 1] + row;
                squares[(y + 1) * (w + 1) + x + 1] = squares[y * (w + 1) + x + 1] + row2;
            }
        }
        Self { w, red, sum, squares }
    }

    /// Σ red and Σ red² over the `n × n` window from (x0, y0).
    fn window(&self, x0: usize, y0: usize, n: usize) -> (f64, f64) {
        let s = self.w + 1;
        let at = |t: &[f64], x: usize, y: usize| t[y * s + x];
        let area = |t: &[f64]| at(t, x0 + n, y0 + n) - at(t, x0, y0 + n) - at(t, x0 + n, y0) + at(t, x0, y0);
        (area(&self.sum), area(&self.squares))
    }
}

/// Σ a·b, in eight lanes (a fixed order: the same sum each time).
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut lanes = [0f32; 8];
    let ((ca, ra), (cb, rb)) = (a.as_chunks::<8>(), b.as_chunks::<8>());
    let tail: f32 = ra.iter().zip(rb).map(|(x, y)| x * y).sum();
    for (x, y) in ca.iter().zip(cb) {
        for ((l, a), b) in lanes.iter_mut().zip(x).zip(y) {
            *l += a * b;
        }
    }
    lanes.iter().sum::<f32>() + tail
}

/// Estimates the intensity from every `every`-th keyframe: in a grid of 24 × 14 textured 49×49 patches inside the
/// scene, the sub-pixel shift that best maps red onto blue (normalised cross-correlation ≥ 0.8, no other peak within
/// 0.03 three or more pixels away), fitted to `blue − red = 0.64·diff`.
pub fn measure(views: &[View], mask: &[f32], w: usize, h: usize, every: usize) -> Measurement {
    let (gx, gy, half, search) = (24usize, 14usize, 24i64, 14i64);
    let mut rows: Vec<(f32, f32, f32, f32)> = Vec::new();
    for v in views.iter().step_by(every.max(1)) {
        let plane = RedPlane::new(&v.image, w, h);
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
                let s = best_shift(&v.image, &plane, x, y, half, search)?;
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

/// Sub-pixel s with blue(x) ≈ red(x + s) around (x, y); None for flat or ambiguous patches. Each shift's red window
/// takes its mean and spread from the summed-area tables, and its correlation with the centred blue patch is one
/// dot product (Σ (r − r̄)·b = Σ r·b, as b sums to zero).
fn best_shift(image: &[u32], red: &RedPlane, x: i64, y: i64, half: i64, search: i64) -> Option<(f32, f32)> {
    let (w, n) = (red.w, (2 * half + 1) as usize);
    let mut blue = Vec::with_capacity(n * n);
    for yy in -half..=half {
        for xx in -half..=half {
            blue.push(channel(image[((y + yy) as usize) * w + (x + xx) as usize], 2));
        }
    }
    let mean = blue.iter().sum::<f32>() / blue.len() as f32;
    blue.iter_mut().for_each(|a| *a -= mean);
    let bn = blue.iter().map(|a| a * a).sum::<f32>().sqrt();
    if bn / (n as f32) < 0.02 {
        return None;
    }
    let size = (2 * search + 1) as usize;
    let mut score = vec![f32::MIN; size * size];
    for sy in -search..=search {
        for sx in -search..=search {
            let (x0, y0) = ((x + sx - half) as usize, (y + sy - half) as usize);
            let (s, s2) = red.window(x0, y0, n);
            let rn = (s2 - s * s / (n * n) as f64).max(0.0).sqrt() as f32;
            if rn / (n as f32) < 0.02 {
                continue;
            }
            let d: f32 = (0..n).map(|r| dot(&red.red[(y0 + r) * w + x0..][..n], &blue[r * n..][..n])).sum();
            score[((sy + search) as usize) * size + (sx + search) as usize] = d / (rn * bn);
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
    use crate::camera::{Camera, Geometry};

    const W: usize = 640;
    const H: usize = 360;

    /// Random grey blocks 8 px wide.
    fn blocks(seed: u64) -> Vec<[f32; 4]> {
        let mut rng = fastrand::Rng::with_seed(seed);
        let blocks: Vec<f32> = (0..(W / 8) * (H / 8)).map(|_| rng.f32()).collect();
        (0..W * H)
            .map(|i| {
                let v = blocks[(i / W / 8) * (W / 8) + (i % W) / 8];
                [v, v, v, 1.0]
            })
            .collect()
    }

    fn pack(img: &[[f32; 4]]) -> Vec<u32> {
        img.iter().map(|p| [0, 1, 2].iter().fold(255u32 << 24, |acc, &c| acc | (((p[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u32) << (8 * c)))).collect()
    }

    fn view(image: Vec<u32>) -> View {
        View { camera: Camera::look([0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 1.0, W as u32, H as u32), image, game_time: 0.0, keyframe: 0 }
    }

    /// Mean absolute difference of channel `a` of one picture and channel `b` of another, away from the centre.
    fn outer_error(x: &[u32], a: usize, y: &[u32], b: usize) -> f32 {
        let mut e = 0.0;
        for py in 0..H {
            for px in 0..W {
                let (cx, cy) = (2.0 * px as f32 / W as f32 - 1.0, 2.0 * py as f32 / H as f32 - 1.0);
                if (cx * cx + cy * cy).sqrt() > 0.7 {
                    e += (channel(x[py * W + px], a) - channel(y[py * W + px], b)).abs();
                }
            }
        }
        e
    }

    /// A picture of random blocks aberrated by PPv2 measures back as its intensity, and realigning it brings red, green
    /// and blue back over each other and over the picture before the aberration.
    #[test]
    fn measures_and_realigns_ppv2() {
        let grey = blocks(3);
        let intensity = 0.3;
        let packed = pack(&aberrate(&grey, W, H, intensity, 1));
        let m = measure(&[view(packed.clone())], &vec![1.0; W * H], W, H, 1);
        assert!(m.patches > 20, "{} patches", m.patches);
        assert!((m.intensity - intensity).abs() < 0.05, "measured {}", m.intensity);
        let fixed = realign(&packed, W, H, &offsets(W, H, intensity, 1));
        // Red against blue and green, away from the centre, before and after realigning.
        assert!(outer_error(&fixed, 0, &fixed, 2) < 0.5 * outer_error(&packed, 0, &packed, 2));
        assert!(outer_error(&fixed, 0, &fixed, 1) < 0.5 * outer_error(&packed, 0, &packed, 1));
        // Green and blue (which PPv2 moves most) against the picture as it was; red, which it hardly moves, no worse.
        let clean = pack(&grey);
        for ch in 1..3 {
            assert!(outer_error(&fixed, ch, &clean, ch) < 0.6 * outer_error(&packed, ch, &clean, ch), "channel {ch}");
        }
        assert!(outer_error(&fixed, 0, &clean, 0) <= 1.1 * outer_error(&packed, 0, &clean, 0));
    }

    /// The measuring shortcut (summed-area tables, one dot product a shift) finds what normalising each shifted patch
    /// found.
    #[test]
    fn the_fast_correlation_finds_the_same_shifts() {
        let packed = pack(&aberrate(&blocks(5), W, H, 0.4, 1));
        let direct = |x: i64, y: i64, half: i64, search: i64| -> Option<(f32, f32)> {
            let n = (2 * half + 1) as usize;
            let patch = |ch: usize, ox: i64, oy: i64| -> Vec<f32> {
                let mut v: Vec<f32> = (-half..=half).flat_map(|yy| (-half..=half).map(move |xx| (xx, yy))).map(|(xx, yy)| channel(packed[((y + yy + oy) as usize) * W + (x + xx + ox) as usize], ch)).collect();
                let mean = v.iter().sum::<f32>() / v.len() as f32;
                v.iter_mut().for_each(|a| *a -= mean);
                v
            };
            let blue = patch(2, 0, 0);
            let bn = blue.iter().map(|a| a * a).sum::<f32>().sqrt();
            if bn / (n as f32) < 0.02 {
                return None;
            }
            let (mut best, mut peak) = ((0, 0), f32::MIN);
            for sy in -search..=search {
                for sx in -search..=search {
                    let red = patch(0, sx, sy);
                    let rn = red.iter().map(|a| a * a).sum::<f32>().sqrt();
                    let s = red.iter().zip(&blue).map(|(a, b)| a * b).sum::<f32>() / (rn * bn);
                    if rn / (n as f32) >= 0.02 && s > peak {
                        (best, peak) = ((sx, sy), s);
                    }
                }
            }
            Some((best.0 as f32, best.1 as f32))
        };
        let plane = RedPlane::new(&packed, W, H);
        let mut compared = 0;
        for (x, y) in [(80i64, 60i64), (560, 300), (100, 280), (320, 180), (540, 70)] {
            let fast = best_shift(&packed, &plane, x, y, 24, 14);
            let slow = direct(x, y, 24, 14);
            if let (Some(f), Some(s)) = (fast, slow) {
                // The fast one adds a sub-pixel parabola fit to the same integer peak.
                assert!((f.0 - s.0).abs() <= 0.5 && (f.1 - s.1).abs() <= 0.5, "({x}, {y}): {f:?} vs {s:?}");
                compared += 1;
            }
        }
        assert!(compared >= 3, "{compared}");
    }

    /// A clean world is left as captured; an aberrated one is undone, and the borders whose colour would come from
    /// outside the frame leave the loss mask (the mask before that is kept for scoring).
    #[test]
    fn undo_leaves_clean_frames_alone_and_masks_what_it_cannot_fill() {
        let dataset = |images: Vec<Vec<u32>>| Dataset {
            faces: None,
            dir: Default::default(),
            views: images.into_iter().map(view).collect(),
            depth_views: Vec::new(),
            width: W as u32,
            height: H as u32,
            geometry: Geometry::new(W as u32, H as u32, 1),
            mask: vec![1.0; W * H],
            loaded: Default::default(),
        };
        let clean: Vec<Vec<u32>> = (0..3).map(|s| pack(&blocks(10 + s))).collect();
        let mut data = dataset(clean.clone());
        assert!(undo(&mut data).is_none());
        assert!(data.views.iter().zip(&clean).all(|(v, c)| v.image == *c));
        assert!(data.mask.iter().all(|m| *m == 1.0));
        let mut data = dataset((0..3).map(|s| pack(&aberrate(&blocks(20 + s), W, H, 0.3, 1))).collect());
        let u = undo(&mut data).expect("undone");
        assert!((u.intensity - 0.3).abs() < 0.05, "{}", u.intensity);
        assert!(u.mask.iter().all(|m| *m == 1.0));
        assert_eq!(data.mask[W * H / 2 + W / 2], 1.0, "the centre trains");
        assert_eq!(data.mask[0], 0.0, "a corner whose blue comes from outside the frame does not");
        let cut = data.mask.iter().filter(|m| **m == 0.0).count() as f32 / (W * H) as f32;
        assert!(cut > 0.001 && cut < 0.1, "{cut}");
    }

    /// PPv2 counts its taps over the captured frame's pixels: training at half the capture's width, the offsets are
    /// those of twice the length, not of half as many taps.
    #[test]
    fn taps_count_captured_pixels() {
        let (x, y) = (5.5f32, 5.5f32);
        // In the corner at intensity 0.3, diff is about 20 training pixels: 10 taps at factor 1, 16 at factor 2.
        let one = channel_offset(2, x, y, W, H, 0.3, 1);
        let two = channel_offset(2, x, y, W, H, 0.3, 2);
        assert!((one.0 - two.0).abs() > 0.05, "{one:?} {two:?}");
        // Near the centre both have the fewest taps (3).
        assert_eq!(channel_offset(2, 330.5, 180.5, W, H, 0.3, 1), channel_offset(2, 330.5, 180.5, W, H, 0.3, 2));
    }

    /// Realigning reads each channel at the p whose colour PPv2 moved onto the pixel (p + o(p) lands on it), not
    /// first-order at the pixel's own offset.
    #[test]
    fn realigning_inverts_the_offsets() {
        let (w, h, intensity) = (640usize, 360usize, 0.2);
        let all = offsets(w, h, intensity, 1);
        let (x, y) = (3.5f32, 2.5f32);
        let (ox, oy) = all[2 * w + 3][2];
        let (px, py) = (x - ox, y - oy);
        let (qx, qy) = channel_offset(2, px, py, w, h, intensity, 1);
        assert!((px + qx - x).abs() < 0.02 && (py + qy - y).abs() < 0.02, "lands {} px off", ((px + qx - x).powi(2) + (py + qy - y).powi(2)).sqrt());
        // The first-order read lands farther off.
        let (fx, fy) = channel_offset(2, x, y, w, h, intensity, 1);
        let (gx, gy) = channel_offset(2, x - fx, y - fy, w, h, intensity, 1);
        assert!(((x - fx + gx - x).powi(2) + (y - fy + gy - y).powi(2)).sqrt() > 0.3);
    }

    #[test]
    fn a_measurement_must_hold_together() {
        let m = |intensity: f32, median_outer: f32, patches: usize| Measurement { intensity, median_outer, patches, profile: Vec::new() };
        assert_eq!(m(0.1, 0.098, 200).verdict(), Ok(0.1));
        assert!(m(0.1, 0.05, 200).verdict().is_err(), "the outer patches disagree");
        assert!(m(0.1, 0.1, 10).verdict().is_err(), "too few patches");
        assert!(m(0.01, 0.01, 200).verdict().is_err(), "too weak to matter");
        assert!(m(f32::NAN, 0.1, 200).verdict().is_err());
        assert_eq!(m(1.5, 1.45, 200).verdict(), Ok(MAX_INTENSITY));
    }
}
