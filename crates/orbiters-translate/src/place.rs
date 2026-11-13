//! Where a line read in the left eye is in the room. Its distance comes from the right eye: the line's box, put at a
//! trial distance along the left eye's rays, is projected into the right eye's picture and compared there with what the
//! left eye saw (normalised cross-correlation on grey samples), over distances from 0.3 to 40 m; the best match, refined
//! between its neighbours, is the distance. Both eyes' own fields and places are used as SteamVR gives them, so canted
//! eyes and unequal fields need nothing special. Lines of one sign agree on a plane, which gives the text its tilt; a
//! line alone faces the eye. When the eyes do not agree (a repeated pattern, a line too small or too plain), the line
//! is still put on its ray, at the distance of its neighbours or [`FALLBACK_DISTANCE`], its size from its angular size.

use crate::geometry::{self, add, cross, dot, normalized, scale, sub, EyeView, M34, V3};
use crate::ocr::{Rgba, TextBox};

/// Where a line goes when its distance cannot be told, metres.
pub const FALLBACK_DISTANCE: f32 = 2.0;
const NEAREST: f32 = 0.3;
const FURTHEST: f32 = 40.0;
/// Trial distances, evenly spread in 1/distance (as the eyes' disparity is).
const STEPS: usize = 96;
/// The correlation a match needs, and how far above the best match elsewhere it must stand.
const GOOD_MATCH: f32 = 0.6;
const DISTINCT: f32 = 0.08;

/// A line placed in the room: a flat rectangle (SteamVR's overlays are rectangles) of `width` x `height` metres, at
/// `pose` (+X along the text, +Y up it, +Z out of its face towards the reader).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub pose: M34,
    pub width: f32,
    pub height: f32,
    /// Whether the distance came from the two eyes (else from neighbours or the fallback).
    pub measured: bool,
    /// Distance from the eye, metres.
    pub distance: f32,
}

impl Placed {
    pub fn centre(&self) -> V3 {
        geometry::origin(&self.pose)
    }
    /// The rectangle's corners: top-left, top-right, bottom-right, bottom-left.
    pub fn corners(&self) -> [V3; 4] {
        let (x, y) = (geometry::axis(&self.pose, 0), geometry::axis(&self.pose, 1));
        let c = self.centre();
        let (hw, hh) = (self.width / 2.0, self.height / 2.0);
        [add(add(c, scale(x, -hw)), scale(y, hh)), add(add(c, scale(x, hw)), scale(y, hh)), add(add(c, scale(x, hw)), scale(y, -hh)), add(add(c, scale(x, -hw)), scale(y, -hh))]
    }
}

/// The two eyes' pictures at one moment. Each picture may be a part of its eye (`from`: its top-left in the whole eye's
/// pixels); the views are of the whole eyes.
pub struct Pair<'a> {
    pub pictures: [Rgba<'a>; 2],
    pub from: [[u32; 2]; 2],
    pub views: [EyeView; 2],
}

impl Pair<'_> {
    fn grey(&self, k: usize, px: [f32; 2]) -> Option<f32> {
        let (x, y) = (px[0] - self.from[k][0] as f32, px[1] - self.from[k][1] as f32);
        let p = self.pictures[k];
        if x < 0.0 || y < 0.0 || x >= p.width as f32 || y >= p.height as f32 {
            return None;
        }
        let s = p.sample(x, y);
        Some(0.299 * s[0] + 0.587 * s[1] + 0.114 * s[2])
    }
}

/// The samples a line is compared on: across `ROWS`, along in proportion, at most `MOST_COLUMNS`.
const ROWS: usize = 12;
const MOST_COLUMNS: usize = 96;

/// A grid of points over a box (in the box's own 0..1 coordinates, along then across), for both halves of the line.
fn grid(b: &TextBox) -> Vec<[f32; 2]> {
    let cols = ((ROWS as f32 * b.length() / b.thickness().max(1.0)).round() as usize).clamp(8, MOST_COLUMNS);
    let mut out = Vec::with_capacity(cols * ROWS);
    for r in 0..ROWS {
        for c in 0..cols {
            out.push([(c as f32 + 0.5) / cols as f32, (r as f32 + 0.5) / ROWS as f32]);
        }
    }
    out
}

/// A point of the box at (along, across), in the whole eye's pixels.
fn in_box(b: &TextBox, at: [f32; 2]) -> [f32; 2] {
    let [tl, tr, br, bl] = b.corners;
    let top = [tl[0] + (tr[0] - tl[0]) * at[0], tl[1] + (tr[1] - tl[1]) * at[0]];
    let bottom = [bl[0] + (br[0] - bl[0]) * at[0], bl[1] + (br[1] - bl[1]) * at[0]];
    [top[0] + (bottom[0] - top[0]) * at[1], top[1] + (bottom[1] - top[1]) * at[1]]
}

fn ncc(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len() as f32;
    let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (x - ma, y - mb);
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa < 1e-3 * n || bb < 1e-3 * n {
        return -1.0;
    }
    ab / (aa * bb).sqrt()
}

/// How far along the left eye's ray through the box's centre the line is: the distance at which the right eye sees the
/// same, with the correlation of that match. `None` when no distance matches well and distinctly.
pub fn stereo_distance(pair: &Pair, b: &TextBox, part: [f32; 2]) -> Option<(f32, f32)> {
    let points: Vec<[f32; 2]> = grid(b).into_iter().filter(|p| p[0] >= part[0] && p[0] <= part[1]).map(|p| in_box(b, p)).collect();
    if points.len() < 16 {
        return None;
    }
    let left: Vec<f32> = points.iter().map(|&p| pair.grey(0, p)).collect::<Option<Vec<f32>>>()?;
    let centre = in_box(b, [(part[0] + part[1]) / 2.0, 0.5]);
    let (o, axis) = pair.views[0].ray(centre);
    let rays: Vec<V3> = points.iter().map(|&p| pair.views[0].ray(p).1).collect();
    // At a trial distance, the box lies on the plane facing the eye at that distance along its centre ray.
    let score_at = |d: f32| -> f32 {
        let mut right = Vec::with_capacity(rays.len());
        for r in &rays {
            let t = d / dot(*r, axis).max(1e-3);
            let Some(px) = pair.views[1].project(add(o, scale(*r, t))) else { return -1.0 };
            match pair.grey(1, px) {
                Some(g) => right.push(g),
                None => return -1.0,
            }
        }
        ncc(&left, &right)
    };
    let inv = |i: f32| 1.0 / NEAREST + (1.0 / FURTHEST - 1.0 / NEAREST) * i / (STEPS - 1) as f32;
    let scores: Vec<f32> = (0..STEPS).map(|i| score_at(1.0 / inv(i as f32))).collect();
    let (best, &peak) = scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    if peak < GOOD_MATCH {
        return None;
    }
    // The best match away from the peak's own hill.
    let elsewhere = scores.iter().enumerate().filter(|(i, _)| (*i as i32 - best as i32).abs() > 3).map(|(_, s)| *s).fold(-1.0f32, f32::max);
    if peak - elsewhere < DISTINCT {
        return None;
    }
    // Between its neighbours: a parabola through the three.
    let at = if best > 0 && best + 1 < STEPS {
        let (a, c) = (scores[best - 1], scores[best + 1]);
        let den = a - 2.0 * peak + c;
        best as f32 + if den.abs() > 1e-6 { (0.5 * (a - c) / den).clamp(-0.5, 0.5) } else { 0.0 }
    } else {
        best as f32
    };
    // A finer look about it.
    let (lo, hi) = (inv(at - 0.5), inv(at + 0.5));
    let (mut best_d, mut best_s) = (1.0 / inv(at), peak);
    for k in 0..=8 {
        let d = 1.0 / (lo + (hi - lo) * k as f32 / 8.0);
        let s = score_at(d);
        if s > best_s {
            (best_d, best_s) = (d, s);
        }
    }
    Some((best_d, best_s))
}

/// A line as found: its box in the left eye and, if the eyes agreed, the room points of its two halves' middles.
#[derive(Clone, Debug)]
pub struct Found {
    pub boxed: TextBox,
    /// The room points of the line's left and right halves (on its middle line), when measured.
    pub halves: Option<[V3; 2]>,
}

/// Measures a line's halves (and, failing that, the line whole) with both eyes.
pub fn measure(pair: &Pair, b: &TextBox) -> Found {
    let point = |part: [f32; 2], d: f32| {
        let c = in_box(b, [(part[0] + part[1]) / 2.0, 0.5]);
        let (o, r) = pair.views[0].ray(c);
        let (_, axis) = pair.views[0].ray(c);
        add(o, scale(r, d / dot(r, axis).max(1e-3)))
    };
    let halves = [[0.0, 0.5], [0.5, 1.0]].map(|part| stereo_distance(pair, b, part).map(|(d, _)| point(part, d)));
    let halves = match halves {
        [Some(a), Some(c)] => Some([a, c]),
        _ => stereo_distance(pair, b, [0.0, 1.0]).map(|(d, _)| [point([0.25, 0.25], d), point([0.75, 0.75], d)]),
    };
    Found { boxed: b.clone(), halves }
}

/// A plane: a point and a unit normal facing the eye.
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub point: V3,
    pub normal: V3,
}

/// The plane through measured points (least squares), its normal towards `eye`; with fewer than three points or points
/// all in a line, the plane through their middle facing the eye but holding their line.
pub fn fit_plane(points: &[V3], eye: V3) -> Option<Plane> {
    if points.is_empty() {
        return None;
    }
    let n = points.len() as f32;
    let c = scale(points.iter().fold([0.0; 3], |a, p| add(a, *p)), 1.0 / n);
    let toward = normalized(sub(eye, c));
    let mut normal = toward;
    if points.len() >= 3 {
        // The smallest axis of the points' spread: power iteration on the inverse is overkill; take the cross of the
        // two largest spreads instead.
        let mut cov = [[0f32; 3]; 3];
        for p in points {
            let d = sub(*p, c);
            for i in 0..3 {
                for j in 0..3 {
                    cov[i][j] += d[i] * d[j];
                }
            }
        }
        let mul = |m: &[[f32; 3]; 3], v: V3| -> V3 { std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2]) };
        let mut a = normalized(sub(points[0], c));
        if geometry::norm(a) < 1e-6 {
            a = [1.0, 0.0, 0.0];
        }
        for _ in 0..30 {
            a = normalized(mul(&cov, a));
        }
        // The second axis: power iteration with the first removed.
        let mut b = normalized(cross(a, toward));
        for _ in 0..30 {
            let v = mul(&cov, b);
            b = normalized(sub(v, scale(a, dot(v, a))));
        }
        let spread_b = dot(mul(&cov, b), b) / n;
        // Points nearly in a line (one row of text): keep facing the eye about that line.
        if spread_b > 0.0004 {
            normal = normalized(cross(a, b));
        } else {
            let side = normalized(sub(toward, scale(a, dot(toward, a))));
            normal = side;
        }
    } else if points.len() == 2 {
        let a = normalized(sub(points[1], points[0]));
        normal = normalized(sub(toward, scale(a, dot(toward, a))));
    }
    if dot(normal, toward) < 0.0 {
        normal = scale(normal, -1.0);
    }
    // A plane seen nearly edge-on is a wrong fit: lean it back towards the eye.
    if dot(normal, toward) < 0.25 {
        normal = normalized(add(normal, scale(toward, 0.5)));
    }
    Some(Plane { point: c, normal })
}

/// The line on `plane`: its box's corners cast from the left eye onto it, as a rectangle (+X along the text, +Y up it).
/// `None` when a corner's ray misses the plane.
pub fn on_plane(view: &EyeView, b: &TextBox, plane: &Plane, measured: bool) -> Option<Placed> {
    let o = geometry::origin(&view.eye);
    let mut corners = [[0f32; 3]; 4];
    for (k, c) in b.corners.iter().enumerate() {
        let (_, d) = view.ray(*c);
        let t = geometry::ray_plane(o, d, plane.point, plane.normal)?;
        corners[k] = add(o, scale(d, t));
    }
    let [tl, tr, br, bl] = corners;
    let (x_raw, y_raw) = if b.vertical {
        // A vertical line's reading direction goes down: its rectangle stands upright, +X across it.
        (sub(add(tl, tr), add(bl, br)), sub(add(tl, bl), add(tr, br)))
    } else {
        (sub(add(tr, br), add(tl, bl)), sub(add(tl, tr), add(bl, br)))
    };
    let (width, height) = (geometry::norm(x_raw) / 2.0, geometry::norm(y_raw) / 2.0);
    let z = plane.normal;
    let x = normalized(sub(x_raw, scale(z, dot(x_raw, z))));
    let y = normalized(cross(z, x));
    // Keep +Y up the text as the picture showed it.
    let y = if dot(y, y_raw) < 0.0 { scale(y, -1.0) } else { y };
    let x = normalized(cross(y, z));
    let centre = scale(add(add(tl, tr), add(br, bl)), 0.25);
    let distance = geometry::norm(sub(centre, o));
    Some(Placed { pose: geometry::from_axes(x, y, z, centre), width, height, measured, distance })
}

/// Places the lines found in one look: lines whose halves were measured define planes with their neighbours (lines of
/// one sign: close in the picture, alike in height and angle); a line not measured takes its neighbours' plane, else the
/// plane facing the eye at the distance of the nearest measured line, else at [`FALLBACK_DISTANCE`].
pub fn place_all(view: &EyeView, found: &[Found]) -> Vec<Placed> {
    let eye = geometry::origin(&view.eye);
    let near = |a: &TextBox, b: &TextBox| {
        let (ca, cb) = (a.centre(), b.centre());
        let h = a.thickness().max(b.thickness());
        let gap = ((ca[0] - cb[0]).powi(2) + (ca[1] - cb[1]).powi(2)).sqrt();
        let alike = (a.thickness() / b.thickness().max(1.0)).clamp(0.0, 10.0);
        let angle = |t: &TextBox| (t.corners[1][1] - t.corners[0][1]).atan2(t.corners[1][0] - t.corners[0][0]);
        gap < h * 4.0 + (a.length().max(b.length())) * 0.6 && (0.6..1.7).contains(&alike) && (angle(a) - angle(b)).abs() < 0.2 && a.vertical == b.vertical
    };
    let measured: Vec<usize> = (0..found.len()).filter(|&i| found[i].halves.is_some()).collect();
    found
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let neighbours: Vec<V3> = measured.iter().filter(|&&j| j == i || near(&f.boxed, &found[j].boxed)).flat_map(|&j| found[j].halves.unwrap()).collect();
            let own = f.halves.is_some();
            let plane = if !neighbours.is_empty() {
                fit_plane(&neighbours, eye)
            } else {
                // The distance of the nearest measured line in the picture, else the fallback, facing the eye.
                let c = f.boxed.centre();
                let d = measured
                    .iter()
                    .min_by(|&&a, &&b| {
                        let da = found[a].boxed.centre();
                        let db = found[b].boxed.centre();
                        ((da[0] - c[0]).powi(2) + (da[1] - c[1]).powi(2)).total_cmp(&((db[0] - c[0]).powi(2) + (db[1] - c[1]).powi(2)))
                    })
                    .map(|&j| {
                        let h = found[j].halves.unwrap();
                        geometry::norm(sub(scale(add(h[0], h[1]), 0.5), eye))
                    })
                    .unwrap_or(FALLBACK_DISTANCE);
                let (o, r) = view.ray(c);
                Some(Plane { point: add(o, scale(r, d)), normal: scale(r, -1.0) })
            };
            plane.and_then(|p| on_plane(view, &f.boxed, &p, own)).unwrap_or_else(|| {
                let (o, r) = view.ray(f.boxed.centre());
                let plane = Plane { point: add(o, scale(r, FALLBACK_DISTANCE)), normal: scale(r, -1.0) };
                on_plane(view, &f.boxed, &plane, false).expect("a plane facing the eye meets its rays")
            })
        })
        .collect()
}
