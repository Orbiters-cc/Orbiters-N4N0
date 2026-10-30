//! Automatic straightening (the crop's AUTO, as the iPhone's Photos levels a picture): the long straight lines of the
//! picture are found by a Hough transform of its strong edges (each edge pixel votes for the lines through it whose
//! direction its gradient allows), and the lean the near-level ones (the horizon, a roof, a table) and the near-upright
//! ones (walls, doors, poles) agree on is the picture's tilt. Upright lines count a little less: a building shot from
//! below leans its verticals together, which a tilt does not undo.

use super::Rgba8;

/// Lines leaning more than this many degrees are the picture's own slants, not its tilt.
pub const MAX_TILT: f32 = 25.0;
/// The picture is looked at this large (its long side, in pixels).
const WORK: u32 = 480;
/// Steps of the line angles, in degrees.
const STEP: f32 = 0.25;
/// An edge pixel votes for the lines within this many degrees of its gradient's.
const SPREAD: f32 = 3.0;

/// What [`detect`] found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Level {
    /// How far the picture's lines lean clockwise, in degrees.
    pub tilt: f32,
    /// How many of the straight lines found agree on it.
    pub lines: usize,
    /// The share of the lines' length agreeing (0 to 1).
    pub agreement: f32,
}

impl Level {
    /// The straightening that levels the picture ([`super::Geometry::straighten`], before flips: a flipped picture
    /// leans the other way).
    pub fn straighten(&self) -> f32 {
        -self.tilt
    }
}

/// The tilt of `p`'s straight lines; None when it has none long enough to tell (a face, a cloud, a blur).
pub fn detect(p: &Rgba8) -> Option<Level> {
    let small = p.fit(WORK);
    let (w, h) = (small.width as usize, small.height as usize);
    if w < 16 || h < 16 {
        return None;
    }
    // Encoded luma, transparent pixels left out (a VRChat layer's empty parts have no edges).
    let luma: Vec<f32> = small
        .pixels
        .chunks_exact(4)
        .map(|c| if c[3] < 128 { f32::NAN } else { (c[0] as f32 * 0.299 + c[1] as f32 * 0.587 + c[2] as f32 * 0.114) / 255.0 })
        .collect();
    let at = |x: usize, y: usize| luma[y * w + x];
    // Sobel gradients; the strong ones are edges.
    let mut edges: Vec<(f32, f32, f32, f32)> = Vec::new();
    let mut strengths = Vec::new();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let (a, b, c) = (at(x - 1, y - 1), at(x, y - 1), at(x + 1, y - 1));
            let (d, f) = (at(x - 1, y), at(x + 1, y));
            let (g, i, j) = (at(x - 1, y + 1), at(x, y + 1), at(x + 1, y + 1));
            let gx = (c + 2.0 * f + j) - (a + 2.0 * d + g);
            let gy = (g + 2.0 * i + j) - (a + 2.0 * b + c);
            let m = (gx * gx + gy * gy).sqrt();
            if !m.is_finite() || m < 0.25 {
                continue;
            }
            // The gradient's direction (the normal of the line through the pixel), 0 to 180 degrees.
            let angle = gy.atan2(gx).to_degrees().rem_euclid(180.0);
            let lean = (angle - 90.0).abs().min(angle).min(180.0 - angle);
            if lean > MAX_TILT + SPREAD {
                continue;
            }
            edges.push((x as f32 + 0.5 - w as f32 * 0.5, y as f32 + 0.5 - h as f32 * 0.5, angle, m));
            strengths.push(m);
        }
    }
    if edges.len() < 20 {
        return None;
    }
    // Not the faintest: below a tenth of the strongest edges' (the 95th percentile's) strength, an edge is texture.
    let cut = {
        let k = strengths.len() * 95 / 100;
        *strengths.select_nth_unstable_by(k, |a, b| a.total_cmp(b)).1 * 0.1
    };
    // The accumulator: angle steps by distances from the middle (one pixel each).
    let n_theta = (180.0 / STEP) as usize;
    let reach = ((w * w + h * h) as f32).sqrt() as i32 / 2 + 2;
    let n_rho = (2 * reach + 1) as usize;
    let mut acc = vec![0u32; n_theta * n_rho];
    let table: Vec<(f32, f32)> = (0..n_theta).map(|t| (t as f32 * STEP).to_radians().sin_cos()).collect();
    let spread = (SPREAD / STEP) as i32;
    for &(x, y, angle, m) in &edges {
        if m < cut {
            continue;
        }
        let base = (angle / STEP).round() as i32;
        for k in -spread..=spread {
            // Past 0 or 180 degrees the same line has its distance the other way.
            let (t, flip) = match base + k {
                t if t < 0 => (t + n_theta as i32, true),
                t if t >= n_theta as i32 => (t - n_theta as i32, true),
                t => (t, false),
            };
            let (s, c) = table[t as usize];
            let rho = x * c + y * s;
            let rho = if flip { -rho } else { rho };
            let r = rho.round() as i32 + reach;
            acc[t as usize * n_rho + r as usize] += 1;
        }
    }
    // The lines: the accumulator's peaks among level and upright angles, long enough (a sixth of the short side).
    let min_votes = ((w.min(h) as f32) / 6.0).max(12.0) as u32;
    let lean_of = |t: usize| {
        let a = t as f32 * STEP;
        if (a - 90.0).abs() <= MAX_TILT {
            Some((a - 90.0, true))
        } else if a <= MAX_TILT {
            Some((a, false))
        } else if a >= 180.0 - MAX_TILT {
            Some((a - 180.0, false))
        } else {
            None
        }
    };
    let votes = |t: i32, r: i32| -> u32 {
        let t = t.rem_euclid(n_theta as i32) as usize;
        if r < 0 || r >= n_rho as i32 {
            return 0;
        }
        acc[t * n_rho + r as usize]
    };
    let mut peaks: Vec<(f32, f32, bool)> = Vec::new();
    for t in 0..n_theta {
        let Some((lean, level)) = lean_of(t) else { continue };
        for r in 0..n_rho {
            let v = acc[t * n_rho + r];
            if v < min_votes {
                continue;
            }
            // The highest within 2 degrees and 3 pixels (the first of equals).
            let (ti, ri) = (t as i32, r as i32);
            let mut best = true;
            'around: for dt in -8..=8 {
                for dr in -3..=3 {
                    if (dt, dr) == (0, 0) {
                        continue;
                    }
                    let o = votes(ti + dt, ri + dr);
                    if o > v || (o == v && (dt, dr) < (0, 0)) {
                        best = false;
                        break 'around;
                    }
                }
            }
            if !best {
                continue;
            }
            // Between angle steps: a parabola through the peak and its neighbours.
            let (a, b) = (votes(ti - 1, ri) as f32, votes(ti + 1, ri) as f32);
            let curve = a - 2.0 * v as f32 + b;
            let shift = if curve < 0.0 { (0.5 * (a - b) / curve).clamp(-0.5, 0.5) } else { 0.0 };
            peaks.push((lean + shift * STEP, v as f32, level));
        }
    }
    peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
    peaks.truncate(40);
    if peaks.is_empty() {
        return None;
    }
    let weight = |p: &(f32, f32, bool)| p.1 * if p.2 { 1.0 } else { 0.75 };
    // The lean most of their length agrees on: a histogram of quarter degrees, each line spread over a degree around
    // its own; then the mean of the lines within a degree of it.
    let bins = (2.0 * MAX_TILT / STEP) as usize + 1;
    let mut hist = vec![0f32; bins];
    for p in &peaks {
        let c = (p.0 + MAX_TILT) / STEP;
        for b in 0..bins {
            let d = (b as f32 - c).abs();
            if d < 4.0 {
                hist[b] += weight(p) * (1.0 - d / 4.0);
            }
        }
    }
    let mode = hist.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(b, _)| b as f32 * STEP - MAX_TILT)?;
    let total: f32 = peaks.iter().map(weight).sum();
    let agreeing: Vec<&(f32, f32, bool)> = peaks.iter().filter(|p| (p.0 - mode).abs() <= 1.0).collect();
    let support: f32 = agreeing.iter().map(|p| weight(p)).sum();
    if agreeing.is_empty() || support < total * 0.2 {
        return None;
    }
    let tilt = agreeing.iter().map(|p| p.0 * weight(p)).sum::<f32>() / support;
    Some(Level { tilt, lines: agreeing.len(), agreement: support / total })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A light picture of `w`×`h` with dark straight lines (each through a point at an angle, degrees clockwise from
    /// level, `width` pixels wide), over a soft gradient and a little noise.
    fn lines(w: u32, h: u32, list: &[([f32; 2], f32, f32)]) -> Rgba8 {
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        let mut seed = 12345u32;
        for y in 0..h {
            for x in 0..w {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 24) as f32 / 255.0 * 10.0;
                let mut v = 200.0 - 50.0 * (x as f32 / w as f32) + noise;
                for &(p, angle, width) in list {
                    let (s, c) = angle.to_radians().sin_cos();
                    // Distance from the line through p along (cos, sin).
                    let d = (-(x as f32 + 0.5 - p[0]) * s + (y as f32 + 0.5 - p[1]) * c).abs();
                    v = v.min(40.0 + 160.0 * (d - width * 0.5).clamp(0.0, 1.0));
                }
                px.extend_from_slice(&[v as u8, v as u8, v as u8, 255]);
            }
        }
        Rgba8::new(w, h, px)
    }

    #[test]
    fn a_tilted_horizon_and_level_lines_are_found() {
        for tilt in [5.0f32, -3.5, 12.0, 0.0, -20.0] {
            let p = lines(960, 540, &[([480.0, 300.0], tilt, 3.0), ([480.0, 150.0], tilt, 2.0)]);
            let l = detect(&p).unwrap_or_else(|| panic!("{tilt}: nothing found"));
            assert!((l.tilt - tilt).abs() < 0.3, "{tilt}: {l:?}");
            assert!((l.straighten() + tilt).abs() < 0.3);
        }
    }

    #[test]
    fn upright_lines_lean_the_same_way() {
        // Walls leaning 4 degrees clockwise: an upright line turned clockwise points down-left.
        let p = lines(800, 600, &[([200.0, 300.0], 94.0, 3.0), ([420.0, 300.0], 94.0, 3.0), ([640.0, 300.0], 94.0, 2.0)]);
        let l = detect(&p).expect("walls");
        assert!((l.tilt - 4.0).abs() < 0.3, "{l:?}");
    }

    #[test]
    fn the_horizon_outweighs_walls_leaning_together() {
        // A level-ish horizon at -2 degrees, and verticals leaning together (a building from below): -2.
        let p = lines(960, 640, &[([480.0, 420.0], -2.0, 3.0), ([200.0, 300.0], 84.0, 2.0), ([760.0, 300.0], 96.0, 2.0), ([480.0, 200.0], 90.0, 2.0)]);
        let l = detect(&p).expect("a horizon");
        assert!((l.tilt + 2.0).abs() < 0.4, "{l:?}");
    }

    #[test]
    fn a_picture_without_lines_says_so() {
        // A soft blob and noise.
        let (w, h) = (640u32, 360u32);
        let mut px = Vec::new();
        let mut seed = 7u32;
        for y in 0..h {
            for x in 0..w {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let d = ((x as f32 - 320.0).powi(2) + (y as f32 - 180.0).powi(2)).sqrt();
                let v = 60.0 + 120.0 * (-(d / 120.0).powi(2)).exp() + (seed >> 24) as f32 / 255.0 * 6.0;
                px.extend_from_slice(&[v as u8, (v * 0.8) as u8, (v * 0.6) as u8, 255]);
            }
        }
        assert_eq!(detect(&Rgba8::new(w, h, px)), None);
        // Lines leaning too far to be a tilt (a diagonal) are not one either.
        let p = lines(640, 360, &[([320.0, 180.0], 40.0, 3.0)]);
        assert_eq!(detect(&p), None);
    }

    #[test]
    fn straightening_by_what_was_found_levels_the_line() {
        use crate::photo::{Crop, Geometry};
        let tilt = 7.0f32;
        let p = lines(960, 540, &[([480.0, 270.0], tilt, 3.0)]);
        let g = Geometry { straighten: detect(&p).unwrap().straighten(), ..Default::default() };
        // Two points on a level line of the straightened picture come from two points of the drawn line.
        let aspect = 960.0 / 540.0;
        let a = g.source_point(Crop::FULL, aspect, 0.3, 0.5);
        let b = g.source_point(Crop::FULL, aspect, 0.7, 0.5);
        let slope = ((b[1] - a[1]) * 540.0).atan2((b[0] - a[0]) * 960.0).to_degrees();
        assert!((slope - tilt).abs() < 0.3, "{slope}");
    }
}
