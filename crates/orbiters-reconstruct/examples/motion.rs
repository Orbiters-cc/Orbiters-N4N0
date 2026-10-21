//! Lab: does the world hold still under the camera? Prints the camera's path, then aligns pairs of depth frames some
//! time apart (their exact depth as world points) with point-to-point ICP: in a static world the frames already agree
//! where they overlap (identity); on a platform the player rides, the best rigid motion moves one frame's points onto
//! the other's and differs from identity.
//!
//! ```text
//! cargo run --release --example motion -- <session> [gap seconds]...
//! ```

use anyhow::Context as _;
use orbiters_reconstruct::dataset;
use std::collections::HashMap;
use std::path::PathBuf;

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn norm(a: V3) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// World points of a depth frame (every second sample along each axis).
fn points(d: &dataset::DepthView) -> Vec<V3> {
    let (gw, gh) = (d.grid_width as usize, d.grid_height as usize);
    let mut out = Vec::new();
    for gy in (0..gh).step_by(2) {
        for gx in (0..gw).step_by(2) {
            let i = gy * gw + gx;
            let z = d.depth[i];
            if z > 0.0 && z < 60.0 {
                let p = d.camera.unproject(d.pixels[i][0], d.pixels[i][1], z);
                out.push([p[0] as f64, p[1] as f64, p[2] as f64]);
            }
        }
    }
    out
}

struct Grid {
    cell: f64,
    map: HashMap<(i64, i64, i64), Vec<usize>>,
}

impl Grid {
    fn new(pts: &[V3], cell: f64) -> Self {
        let mut map: HashMap<(i64, i64, i64), Vec<usize>> = HashMap::new();
        for (i, p) in pts.iter().enumerate() {
            map.entry(Self::key(p, cell)).or_default().push(i);
        }
        Self { cell, map }
    }
    fn key(p: &V3, cell: f64) -> (i64, i64, i64) {
        ((p[0] / cell).floor() as i64, (p[1] / cell).floor() as i64, (p[2] / cell).floor() as i64)
    }
    /// Nearest point within one cell's reach.
    fn nearest(&self, pts: &[V3], q: &V3) -> Option<(usize, f64)> {
        let (kx, ky, kz) = Self::key(q, self.cell);
        let mut best: Option<(usize, f64)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(list) = self.map.get(&(kx + dx, ky + dy, kz + dz)) {
                        for &i in list {
                            let d = norm(sub(pts[i], *q));
                            if best.is_none_or(|b| d < b.1) {
                                best = Some((i, d));
                            }
                        }
                    }
                }
            }
        }
        best.filter(|b| b.1 <= self.cell)
    }
}

/// Rotation (row-major) and translation minimising Σ|R·b + t − a|² (Horn's quaternion method).
fn kabsch(a: &[V3], b: &[V3]) -> ([[f64; 3]; 3], V3) {
    let n = a.len() as f64;
    let ca = a.iter().fold([0.0; 3], |s, p| [s[0] + p[0] / n, s[1] + p[1] / n, s[2] + p[2] / n]);
    let cb = b.iter().fold([0.0; 3], |s, p| [s[0] + p[0] / n, s[1] + p[1] / n, s[2] + p[2] / n]);
    let mut m = [[0.0f64; 3]; 3];
    for (p, q) in a.iter().zip(b) {
        let (pa, pb) = (sub(*p, ca), sub(*q, cb));
        for r in 0..3 {
            for c in 0..3 {
                m[r][c] += pb[r] * pa[c];
            }
        }
    }
    let (sxx, sxy, sxz, syx, syy, syz, szx, szy, szz) = (m[0][0], m[0][1], m[0][2], m[1][0], m[1][1], m[1][2], m[2][0], m[2][1], m[2][2]);
    let mut k = [
        [sxx + syy + szz, syz - szy, szx - sxz, sxy - syx],
        [syz - szy, sxx - syy - szz, sxy + syx, szx + sxz],
        [szx - sxz, sxy + syx, -sxx + syy - szz, syz + szy],
        [sxy - syx, szx + sxz, syz + szy, -sxx - syy + szz],
    ];
    // Largest eigenvector by Jacobi rotations.
    let mut v = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
    for _ in 0..50 {
        for p in 0..4 {
            for q in p + 1..4 {
                if k[p][q].abs() < 1e-15 {
                    continue;
                }
                let theta = (k[q][q] - k[p][p]) / (2.0 * k[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let (c, s) = (1.0 / (t * t + 1.0).sqrt(), t / (t * t + 1.0).sqrt());
                for r in 0..4 {
                    let (krp, krq) = (k[r][p], k[r][q]);
                    k[r][p] = c * krp - s * krq;
                    k[r][q] = s * krp + c * krq;
                }
                for r in 0..4 {
                    let (kpr, kqr) = (k[p][r], k[q][r]);
                    k[p][r] = c * kpr - s * kqr;
                    k[q][r] = s * kpr + c * kqr;
                }
                for r in 0..4 {
                    let (vrp, vrq) = (v[r][p], v[r][q]);
                    v[r][p] = c * vrp - s * vrq;
                    v[r][q] = s * vrp + c * vrq;
                }
            }
        }
    }
    let best = (0..4).max_by(|&i, &j| k[i][i].total_cmp(&k[j][j])).unwrap();
    let (w, x, y, z) = (v[0][best], v[1][best], v[2][best], v[3][best]);
    let r = [
        [w * w + x * x - y * y - z * z, 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)],
        [2.0 * (x * y + w * z), w * w - x * x + y * y - z * z, 2.0 * (y * z - w * x)],
        [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), w * w - x * x - y * y + z * z],
    ];
    let rcb = [0, 1, 2].map(|i| r[i][0] * cb[0] + r[i][1] * cb[1] + r[i][2] * cb[2]);
    (r, sub(ca, rcb))
}

fn apply(r: &[[f64; 3]; 3], t: &V3, p: &V3) -> V3 {
    [0, 1, 2].map(|i| r[i][0] * p[0] + r[i][1] * p[1] + r[i][2] * p[2] + t[i])
}

/// Share of `b`'s points within `tol` of `a` after moving them by (r, t).
fn inliers(a: &[V3], grid: &Grid, b: &[V3], r: &[[f64; 3]; 3], t: &V3, tol: f64) -> f64 {
    let n = b.iter().filter(|p| grid.nearest(a, &apply(r, t, p)).is_some_and(|(_, d)| d <= tol)).count();
    n as f64 / b.len().max(1) as f64
}

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let session = PathBuf::from(it.next().context("usage: motion <session> [gap seconds]...")?);
    let gaps: Vec<f32> = it.map(|g| g.parse()).collect::<Result<_, _>>()?;
    let gaps = if gaps.is_empty() { vec![1.0, 5.0] } else { gaps };
    let data = dataset::load(&session, 1920, |_| {})?;
    let dv = &data.depth_views;
    let pos: Vec<[f32; 3]> = dv.iter().map(|d| d.camera.position()).collect();
    let path: f64 = pos.windows(2).map(|w| norm(sub(w[1].map(f64::from), w[0].map(f64::from)))).sum();
    let span = dv.last().map_or(0.0, |d| d.game_time) - dv.first().map_or(0.0, |d| d.game_time);
    println!("{} depth frames over {span:.1} s; camera path {path:.1} m, start to end {:.1} m", dv.len(), norm(sub(pos[pos.len() - 1].map(f64::from), pos[0].map(f64::from))));
    for k in (0..dv.len()).step_by((dv.len() / 12).max(1)) {
        println!("  t {:7.2} s  camera {:8.2} {:8.2} {:8.2}", dv[k].game_time, pos[k][0], pos[k][1], pos[k][2]);
    }
    let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for &gap in &gaps {
        let mut rows = Vec::new();
        for a in (0..dv.len()).step_by((dv.len() / 10).max(1)) {
            let Some(b) = (a + 1..dv.len()).find(|&b| dv[b].game_time - dv[a].game_time >= gap) else { continue };
            let (pa, pb) = (points(&dv[a]), points(&dv[b]));
            if pa.len() < 200 || pb.len() < 200 {
                continue;
            }
            let grid = Grid::new(&pa, 0.25);
            let still = inliers(&pa, &grid, &pb, &identity, &[0.0; 3], 0.05);
            // ICP from identity, correspondences within 25 cm.
            let (mut r, mut t) = (identity, [0.0; 3]);
            for _ in 0..20 {
                let (mut ma, mut mb) = (Vec::new(), Vec::new());
                for p in &pb {
                    if let Some((i, _)) = grid.nearest(&pa, &apply(&r, &t, p)) {
                        ma.push(pa[i]);
                        mb.push(*p);
                    }
                }
                if ma.len() < 100 {
                    break;
                }
                (r, t) = kabsch(&ma, &mb);
            }
            let moved = inliers(&pa, &grid, &pb, &r, &t, 0.05);
            let angle = ((r[0][0] + r[1][1] + r[2][2] - 1.0) / 2.0).clamp(-1.0, 1.0).acos().to_degrees();
            let cam = norm(sub(pos[b].map(f64::from), pos[a].map(f64::from)));
            rows.push((dv[a].game_time, still, moved, norm(t), angle, cam));
        }
        println!("frames {gap} s apart: share of the later frame's points within 5 cm of the earlier one's");
        for (t0, still, moved, tn, ang, cam) in rows {
            println!("  t {t0:7.2} s  as recorded {:5.1} %  best rigid motion {:5.1} % (moves {tn:6.2} m, turns {ang:5.1}°; camera moved {cam:5.2} m)", 100.0 * still, 100.0 * moved);
        }
    }
    Ok(())
}
