//! A capsule walking the recorded path on a collider: the direct test of whether people can use it.
//!
//! The capsule (VRChat's player is about this: radius 0.2 m, as tall as the eyes plus 0.1 m) follows the walked path
//! horizontally, every [`STEP`] metres (a teleport is not walked: crate::mesh::roi::TELEPORT). At each point it stands on the highest walkable surface (normal within
//! [`Capsule::slope`] of vertical) under its centre or under four points halfway to its rim, from
//! [`Capsule::drop`] below its feet to [`Capsule::step`] above them, as a character controller steps up a stair and
//! down a curb. No such surface: it would fall through the collider there (counted, then put back at the path's own
//! feet height to go on). Then its body, from a step above its feet to its top, must not touch the collider closer
//! than its radius: the person walked there in the game, so a touch is a false wall or snagging junk (counted, the walk
//! goes on). Off level: standing more than 0.3 m away from the path's own feet height (on a bench, under a floor),
//! which only means something when the path is the camera at eye height.

use super::Mesh;
use crate::camera::{cross, dot};

/// Path sampling, metres.
pub const STEP: f32 = 0.1;
const CELL: f32 = 0.5;

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Capsule {
    pub radius: f32,
    pub height: f32,
    pub step: f32,
    /// Steepest walkable slope, degrees.
    pub slope: f32,
    pub drop: f32,
}

impl Capsule {
    pub fn for_eye_height(eye_height: f32) -> Self {
        Self { radius: 0.2, height: eye_height + 0.1, step: 0.35, slope: 45.0, drop: 1.0 }
    }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Report {
    /// Path length walked, metres.
    pub length: f32,
    /// Shares of that length: standing on the collider, body touching it, off the path's level.
    pub supported: f32,
    pub blocked: f32,
    pub off_level: f32,
    /// Separate places (runs of points) where the capsule found no ground within reach (it would fall through, or it
    /// could not climb) or was blocked.
    pub falls: usize,
    pub blocks: usize,
    /// Longest run without support, metres.
    pub longest_fall: f32,
    /// Where the falls and blocks begin (at most 20 each), for renders.
    pub fall_at: Vec<[f32; 3]>,
    pub block_at: Vec<[f32; 3]>,
}

struct Grid<'a> {
    m: &'a Mesh,
    x0: f32,
    z0: f32,
    w: usize,
    h: usize,
    cells: Vec<Vec<u32>>,
    normals: Vec<[f32; 3]>,
}

impl<'a> Grid<'a> {
    fn new(m: &'a Mesh) -> Self {
        let lo = |i: usize| m.positions.iter().map(|p| p[i]).fold(f32::INFINITY, f32::min);
        let hi = |i: usize| m.positions.iter().map(|p| p[i]).fold(f32::NEG_INFINITY, f32::max);
        let (x0, z0) = (lo(0) - CELL, lo(2) - CELL);
        let w = (((hi(0) - x0) / CELL).ceil() as usize + 2).max(1);
        let h = (((hi(2) - z0) / CELL).ceil() as usize + 2).max(1);
        let mut cells = vec![Vec::new(); w * h];
        let mut normals = Vec::with_capacity(m.triangles.len());
        for (i, t) in m.triangles.iter().enumerate() {
            let p = t.map(|v| m.positions[v as usize]);
            let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
            let l = dot(n, n).sqrt().max(1e-20);
            normals.push(n.map(|v| v / l));
            let cx = |x: f32| (((x - x0) / CELL).floor().max(0.0) as usize).min(w - 1);
            let cz = |z: f32| (((z - z0) / CELL).floor().max(0.0) as usize).min(h - 1);
            let (ax, bx) = (cx(p.iter().map(|q| q[0]).fold(f32::INFINITY, f32::min)), cx(p.iter().map(|q| q[0]).fold(f32::NEG_INFINITY, f32::max)));
            let (az, bz) = (cz(p.iter().map(|q| q[2]).fold(f32::INFINITY, f32::min)), cz(p.iter().map(|q| q[2]).fold(f32::NEG_INFINITY, f32::max)));
            for z in az..=bz {
                for x in ax..=bx {
                    cells[z * w + x].push(i as u32);
                }
            }
        }
        Self { m, x0, z0, w, h, cells, normals }
    }

    fn cell(&self, x: f32, z: f32) -> Option<&[u32]> {
        let (cx, cz) = (((x - self.x0) / CELL).floor(), ((z - self.z0) / CELL).floor());
        (cx >= 0.0 && cz >= 0.0 && (cx as usize) < self.w && (cz as usize) < self.h).then(|| self.cells[cz as usize * self.w + cx as usize].as_slice())
    }

    /// Heights of walkable surfaces under (x, z) between `lo` and `hi`: the highest.
    fn ground(&self, x: f32, z: f32, lo: f32, hi: f32, min_ny: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        for &t in self.cell(x, z)? {
            if self.normals[t as usize][1].abs() < min_ny {
                continue;
            }
            let p = self.m.triangles[t as usize].map(|v| self.m.positions[v as usize]);
            // Barycentric coordinates in the xz plane.
            let d = (p[1][2] - p[2][2]) * (p[0][0] - p[2][0]) + (p[2][0] - p[1][0]) * (p[0][2] - p[2][2]);
            if d.abs() < 1e-12 {
                continue;
            }
            let a = ((p[1][2] - p[2][2]) * (x - p[2][0]) + (p[2][0] - p[1][0]) * (z - p[2][2])) / d;
            let b = ((p[2][2] - p[0][2]) * (x - p[2][0]) + (p[0][0] - p[2][0]) * (z - p[2][2])) / d;
            let c = 1.0 - a - b;
            if a < -1e-5 || b < -1e-5 || c < -1e-5 {
                continue;
            }
            let y = a * p[0][1] + b * p[1][1] + c * p[2][1];
            if y >= lo && y <= hi && best.is_none_or(|b| y > b) {
                best = Some(y);
            }
        }
        best
    }

    /// Whether a vertical capsule body from `bottom` to `top` (sphere centres) at (x, z) touches the collider.
    fn touches(&self, x: f32, z: f32, bottom: f32, top: f32, r: f32) -> bool {
        let mut seen: Vec<u32> = Vec::new();
        for (dx, dz) in [(-r, -r), (r, -r), (-r, r), (r, r)] {
            if let Some(c) = self.cell(x + dx, z + dz) {
                for &t in c {
                    if !seen.contains(&t) {
                        seen.push(t);
                    }
                }
            }
        }
        let n = ((top - bottom) / r).ceil().max(1.0) as usize;
        for &t in &seen {
            let p = self.m.triangles[t as usize].map(|v| self.m.positions[v as usize]);
            if p.iter().all(|q| q[1] < bottom - r) || p.iter().all(|q| q[1] > top + r) {
                continue;
            }
            for k in 0..=n {
                let c = [x, bottom + (top - bottom) * k as f32 / n as f32, z];
                let q = closest_point(c, p);
                let d = sub(q, c);
                if dot(d, d) < r * r {
                    return true;
                }
            }
        }
        false
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}

/// The point of triangle `t` closest to `p` (Ericson, Real-Time Collision Detection 5.1.5).
fn closest_point(p: [f32; 3], t: [[f32; 3]; 3]) -> [f32; 3] {
    let [a, b, c] = t;
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return std::array::from_fn(|i| a[i] + v * ab[i]);
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return std::array::from_fn(|i| a[i] + w * ac[i]);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return std::array::from_fn(|i| b[i] + w * (c[i] - b[i]));
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    std::array::from_fn(|i| a[i] + ab[i] * v + ac[i] * w)
}

/// Walks `cap` along `feet` (the path's feet, any spacing) on `m`.
pub fn walk(m: &Mesh, feet: &[[f32; 3]], cap: &Capsule) -> Report {
    let mut r = Report::default();
    if m.triangles.is_empty() || feet.is_empty() {
        return r;
    }
    let grid = Grid::new(m);
    // Resample every STEP metres horizontally, interpolating the path's own feet height.
    let points = super::WalkPath { feet: feet.to_vec(), eye_height: cap.height, source: "walk" }.resampled(STEP);
    let min_ny = cap.slope.to_radians().cos();
    let probes = [(0.0, 0.0), (0.5, 0.0), (-0.5, 0.0), (0.0, 0.5), (0.0, -0.5)].map(|(a, b)| (a * cap.radius, b * cap.radius));
    let stand = |x: f32, z: f32, f: f32| probes.iter().filter_map(|(dx, dz)| grid.ground(x + dx, z + dz, f - cap.drop, f + cap.step, min_ny)).reduce(f32::max);
    let mut f = stand(points[0][0], points[0][2], points[0][1] + 0.5).unwrap_or(points[0][1]);
    let (mut falling, mut blocking) = (false, false);
    let mut run = 0.0f32;
    for (i, p) in points.iter().enumerate() {
        let mut d = if i == 0 { 0.0 } else { ((p[0] - points[i - 1][0]).powi(2) + (p[2] - points[i - 1][2]).powi(2)).sqrt() };
        if d > super::roi::TELEPORT {
            // A teleport: not walked; stand again where it lands.
            d = 0.0;
            f = stand(p[0], p[2], p[1] + 0.5).unwrap_or(p[1]);
            falling = false;
        }
        r.length += d;
        match stand(p[0], p[2], f) {
            Some(g) => {
                f = g;
                r.supported += d;
                falling = false;
                run = 0.0;
            }
            None => {
                if !falling {
                    r.falls += 1;
                    if r.fall_at.len() < 20 {
                        r.fall_at.push([p[0], f, p[2]]);
                    }
                }
                falling = true;
                run += d;
                r.longest_fall = r.longest_fall.max(run);
                f = p[1];
            }
        }
        if (f - p[1]).abs() > 0.3 {
            r.off_level += d;
        }
        let touch = grid.touches(p[0], p[2], f + cap.step + cap.radius, f + cap.height - cap.radius, cap.radius);
        if touch {
            r.blocked += d;
            if !blocking {
                r.blocks += 1;
                if r.block_at.len() < 20 {
                    r.block_at.push([p[0], f, p[2]]);
                }
            }
        }
        blocking = touch;
    }
    if r.length > 0.0 {
        r.supported /= r.length;
        r.blocked /= r.length;
        r.off_level /= r.length;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path through `points`, recorded every 0.25 m (as a walk is: no teleports).
    fn line(points: &[[f32; 3]]) -> Vec<[f32; 3]> {
        let mut out = vec![points[0]];
        for w in points.windows(2) {
            let n = (((w[1][0] - w[0][0]).powi(2) + (w[1][2] - w[0][2]).powi(2)).sqrt() / 0.25).ceil().max(1.0) as usize;
            out.extend((1..=n).map(|k| std::array::from_fn(|i| w[0][i] + k as f32 / n as f32 * (w[1][i] - w[0][i]))));
        }
        out
    }

    fn quad(m: &mut Mesh, a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) {
        let k = m.positions.len() as u32;
        m.positions.extend([a, b, c, d]);
        m.triangles.extend([[k, k + 1, k + 2], [k, k + 2, k + 3]]);
    }

    #[test]
    fn walks_up_stairs_falls_through_holes_bumps_walls() {
        let mut m = Mesh::default();
        // Floor from x = 0 to 4, then three 0.2 m steps 0.3 m deep, then a landing to x = 8.
        quad(&mut m, [0., 0., -1.], [4., 0., -1.], [4., 0., 1.], [0., 0., 1.]);
        for s in 0..3 {
            let (x, y) = (4.0 + 0.3 * s as f32, 0.2 * (s + 1) as f32);
            quad(&mut m, [x, y, -1.], [x + 0.3, y, -1.], [x + 0.3, y, 1.], [x, y, 1.]);
        }
        quad(&mut m, [4.9, 0.6, -1.], [8., 0.6, -1.], [8., 0.6, 1.], [4.9, 0.6, 1.]);
        let cap = Capsule::for_eye_height(1.6);
        let path = line(&[[0.5, 0., 0.], [7.5, 0.6, 0.]]);
        let r = walk(&m, &path, &cap);
        assert!(r.supported > 0.99 && r.blocked == 0.0 && r.falls == 0, "{r:?}");
        // A 1 m hole in the floor: it falls through once.
        let mut holed = Mesh::default();
        quad(&mut holed, [0., 0., -1.], [2., 0., -1.], [2., 0., 1.], [0., 0., 1.]);
        quad(&mut holed, [3., 0., -1.], [5., 0., -1.], [5., 0., 1.], [3., 0., 1.]);
        let r = walk(&holed, &line(&[[0.5, 0., 0.], [4.5, 0., 0.]]), &cap);
        assert_eq!(r.falls, 1);
        assert!((r.supported - 0.8).abs() < 0.06 && r.longest_fall > 0.5, "{r:?}");
        // A wall across the path.
        quad(&mut holed, [2.5, 0., -1.], [2.5, 0., 1.], [2.5, 2., 1.], [2.5, 2., -1.]);
        let r = walk(&holed, &line(&[[0.5, 0., 0.], [4.5, 0., 0.]]), &cap);
        assert_eq!(r.blocks, 1, "{r:?}");
        // A 0.6 m ledge is not a step.
        let mut ledge = Mesh::default();
        quad(&mut ledge, [0., 0., -1.], [2., 0., -1.], [2., 0., 1.], [0., 0., 1.]);
        quad(&mut ledge, [2., 0.6, -1.], [4., 0.6, -1.], [4., 0.6, 1.], [2., 0.6, 1.]);
        let r = walk(&ledge, &line(&[[0.5, 0., 0.], [3.5, 0., 0.]]), &cap);
        assert!(r.falls == 1 && r.blocks == 1, "{r:?}");
        // A teleport across the hole is not walked: no fall.
        let r = walk(&holed, &[line(&[[0.2, 0., 0.5], [1.2, 0., 0.5]]), line(&[[3.5, 0., 0.5], [4.5, 0., 0.5]])].concat(), &cap);
        assert!(r.falls == 0 && (r.length - 2.0).abs() < 0.11, "{r:?}");
    }

    #[test]
    fn closest_point_on_a_triangle() {
        let t = [[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]];
        let c = closest_point([0.2, 1., 0.2], t);
        assert!((c[0] - 0.2).abs() < 1e-6 && c[1] == 0.0 && (c[2] - 0.2).abs() < 1e-6, "{c:?}");
        assert_eq!(closest_point([-1., 0., -1.], t), [0., 0., 0.]);
        let e = closest_point([1., 0., 1.], t);
        assert!((e[0] - 0.5).abs() < 1e-6 && (e[2] - 0.5).abs() < 1e-6);
    }
}
