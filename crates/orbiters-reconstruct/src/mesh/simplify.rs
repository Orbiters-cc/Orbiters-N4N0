//! Quadric-error simplification that keeps a collider a collider.
//!
//! Garland and Heckbert's quadrics (each face's plane, weighted by its area; each boundary edge adds the plane through
//! it perpendicular to its face, so boundaries stay put), with half-edge collapses: a vertex moves onto a neighbour,
//! so every kept vertex is a vertex of the fused surface. A collapse is taken only if
//! - the error (area-weighted mean squared distance to the merged faces' planes) stays under [`Options::error`]²,
//! - the moved vertex stays within [`Options::max_move`] of the planes of the faces around it, and it of the faces
//!   it leaves (a pole's tip, a stair's nosing or a curb cannot fold flat, however much floor shares their quadric),
//! - the link condition holds (the two vertices share exactly the neighbours of the faces on their edge): no edge
//!   ever gets a third face, no surface pinches,
//! - no face turns over or degenerates, a boundary vertex only slides along its boundary and its boundary stays
//!   within the same distance, and a boundary vertex never meets another boundary across the surface.
//!
//! Passes: each sorts all candidate collapses by error and takes them greedily, each vertex at most once a pass
//! (its neighbourhood is checked on current geometry), then rebuilds the adjacency. The first stage runs in parallel
//! on 8 m columns of the region with the vertices they share locked; a global stage then joins the seams and meets the
//! budget, raising the error by half again until the collider fits.

use super::{Mesh, Settings};
use rayon::prelude::*;

/// Weight of a boundary edge's plane, per squared edge length.
const BOUNDARY_WEIGHT: f64 = 2.0;
/// Columns of the parallel first stage, metres.
const CHUNK: f32 = 8.0;
/// A face may turn by at most acos(0.2) ≈ 78° in a collapse.
const MIN_COS: f64 = 0.2;

#[derive(Clone, Copy, Default, Debug)]
pub struct Quadric {
    a: [f64; 6],
    b: [f64; 3],
    c: f64,
    w: f64,
}

impl Quadric {
    fn plane(n: [f64; 3], p: [f64; 3], w: f64) -> Self {
        let d = -(n[0] * p[0] + n[1] * p[1] + n[2] * p[2]);
        Quadric {
            a: [n[0] * n[0] * w, n[0] * n[1] * w, n[0] * n[2] * w, n[1] * n[1] * w, n[1] * n[2] * w, n[2] * n[2] * w],
            b: [n[0] * d * w, n[1] * d * w, n[2] * d * w],
            c: d * d * w,
            w,
        }
    }

    fn add(&mut self, o: &Quadric) {
        for i in 0..6 {
            self.a[i] += o.a[i];
        }
        for i in 0..3 {
            self.b[i] += o.b[i];
        }
        self.c += o.c;
        self.w += o.w;
    }

    fn eval(&self, p: [f64; 3]) -> f64 {
        let [x, y, z] = p;
        let a = &self.a;
        x * (a[0] * x + a[1] * y + a[2] * z) + y * (a[1] * x + a[3] * y + a[4] * z) + z * (a[2] * x + a[4] * y + a[5] * z) + 2.0 * (self.b[0] * x + self.b[1] * y + self.b[2] * z) + self.c
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn edge_key(a: u32, b: u32) -> u64 {
    (a.min(b) as u64) << 32 | a.max(b) as u64
}

/// Each vertex's quadric: its faces' planes and its boundary edges' perpendicular planes.
pub fn quadrics(m: &Mesh) -> Vec<Quadric> {
    let pos = |i: u32| m.positions[i as usize].map(|v| v as f64);
    let mut q = vec![Quadric::default(); m.positions.len()];
    let normals: Vec<Option<[f64; 3]>> = m
        .triangles
        .iter()
        .map(|t| {
            let p = t.map(pos);
            let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
            let l = dot(n, n).sqrt();
            (l > 1e-20).then(|| n.map(|v| v / l))
        })
        .collect();
    for (t, n) in m.triangles.iter().zip(&normals) {
        let Some(n) = n else { continue };
        let p = t.map(pos);
        let area = 0.5 * dot(cross(sub(p[1], p[0]), sub(p[2], p[0])), *n);
        let face = Quadric::plane(*n, p[0], area);
        for &v in t {
            q[v as usize].add(&face);
        }
    }
    let mut edges: Vec<(u64, u32)> = m.triangles.iter().enumerate().flat_map(|(i, t)| (0..3).map(move |k| (edge_key(t[k], t[(k + 1) % 3]), i as u32))).collect();
    edges.par_sort_unstable();
    let mut i = 0;
    while i < edges.len() {
        let mut j = i;
        while j < edges.len() && edges[j].0 == edges[i].0 {
            j += 1;
        }
        if j - i == 1 {
            let (a, b) = ((edges[i].0 >> 32) as u32, edges[i].0 as u32);
            if let Some(n) = normals[edges[i].1 as usize] {
                let e = sub(pos(b), pos(a));
                let len2 = dot(e, e);
                let mm = cross(e, n);
                let l = dot(mm, mm).sqrt();
                if l > 1e-20 {
                    let plane = Quadric::plane(mm.map(|v| v / l), pos(a), len2 * BOUNDARY_WEIGHT);
                    q[a as usize].add(&plane);
                    q[b as usize].add(&plane);
                }
            }
        }
        i = j;
    }
    q
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub error: f64,
    pub max_move: f64,
    /// Stop at this many triangles (0: only the error bounds).
    pub target: usize,
    /// Whether vertices may collapse onto locked ones.
    pub into_locked: bool,
}

/// Simplifies `triangles` (indices into `positions`) in place; returns the number of passes.
pub fn simplify(positions: &[[f32; 3]], triangles: &mut Vec<[u32; 3]>, q: &mut [Quadric], locked: &[bool], o: &Options) -> usize {
    let nv = positions.len();
    let pos = |i: u32| positions[i as usize].map(|v| v as f64);
    let mut passes = 0;
    while triangles.len() > o.target {
        passes += 1;
        let mut start = vec![0u32; nv + 1];
        for t in triangles.iter() {
            for &v in t {
                start[v as usize + 1] += 1;
            }
        }
        for i in 0..nv {
            start[i + 1] += start[i];
        }
        let mut fill = start.clone();
        let mut list = vec![0u32; triangles.len() * 3];
        for (ti, t) in triangles.iter().enumerate() {
            for &v in t {
                list[fill[v as usize] as usize] = ti as u32;
                fill[v as usize] += 1;
            }
        }
        drop(fill);
        let mut edges: Vec<u64> = triangles.iter().flat_map(|t| (0..3).map(move |k| edge_key(t[k], t[(k + 1) % 3]))).collect();
        edges.par_sort_unstable();
        let mut boundary_edges = vec![0u8; nv];
        let mut pinned = vec![false; nv];
        let mut unique: Vec<(u32, u32, bool)> = Vec::with_capacity(edges.len() / 2 + 1);
        let mut i = 0;
        while i < edges.len() {
            let mut j = i;
            while j < edges.len() && edges[j] == edges[i] {
                j += 1;
            }
            let (a, b) = ((edges[i] >> 32) as u32, edges[i] as u32);
            match j - i {
                1 => {
                    boundary_edges[a as usize] = boundary_edges[a as usize].saturating_add(1);
                    boundary_edges[b as usize] = boundary_edges[b as usize].saturating_add(1);
                    unique.push((a, b, true));
                }
                2 => unique.push((a, b, false)),
                _ => {
                    pinned[a as usize] = true;
                    pinned[b as usize] = true;
                }
            }
            i = j;
        }
        drop(edges);
        for v in 0..nv {
            if boundary_edges[v] != 0 && boundary_edges[v] != 2 {
                pinned[v] = true;
            }
        }
        let limit = o.error * o.error;
        let mut candidates: Vec<(f32, u32, u32)> = unique
            .par_iter()
            .filter_map(|&(a, b, on_boundary)| {
                let mut best: Option<(f64, u32, u32)> = None;
                for (from, to) in [(a, b), (b, a)] {
                    let (f, t) = (from as usize, to as usize);
                    if locked[f] || pinned[f] || pinned[t] || (!o.into_locked && locked[t]) {
                        continue;
                    }
                    if boundary_edges[f] > 0 && !on_boundary {
                        continue;
                    }
                    let mut qq = q[f];
                    qq.add(&q[t]);
                    let cost = qq.eval(pos(to)).max(0.0) / qq.w.max(1e-30);
                    if best.is_none_or(|b| cost < b.0) {
                        best = Some((cost, from, to));
                    }
                }
                best.filter(|b| b.0 <= limit).map(|(c, f, t)| (c as f32, f, t))
            })
            .collect();
        drop(unique);
        candidates.par_sort_unstable_by(|x, y| x.0.total_cmp(&y.0));
        let mut alive = vec![true; triangles.len()];
        let mut touched = vec![false; nv];
        let mut live = triangles.len();
        let mut collapsed = 0usize;
        let mut nf: Vec<u32> = Vec::new();
        let mut nt: Vec<u32> = Vec::new();
        for &(_, from, to) in &candidates {
            if live <= o.target {
                break;
            }
            if touched[from as usize] || touched[to as usize] {
                continue;
            }
            let tf = &list[start[from as usize] as usize..start[from as usize + 1] as usize];
            let tt = &list[start[to as usize] as usize..start[to as usize + 1] as usize];
            // Link condition.
            nf.clear();
            nt.clear();
            let mut shared = 0;
            for &t in tf {
                if !alive[t as usize] {
                    continue;
                }
                let tri = triangles[t as usize];
                if tri.contains(&to) {
                    shared += 1;
                }
                for v in tri {
                    if v != from && !nf.contains(&v) {
                        nf.push(v);
                    }
                }
            }
            for &t in tt {
                if !alive[t as usize] {
                    continue;
                }
                for v in triangles[t as usize] {
                    if v != to && !nt.contains(&v) {
                        nt.push(v);
                    }
                }
            }
            let common = nf.iter().filter(|v| **v != to && nt.contains(v)).count();
            if shared == 0 || common != shared {
                continue;
            }
            // Geometry: no flip, no large move, boundaries stay.
            let (pf, pt) = (pos(from), pos(to));
            let mut ok = true;
            for &t in tf {
                if !alive[t as usize] || triangles[t as usize].contains(&to) {
                    continue;
                }
                let tri = triangles[t as usize];
                let p = tri.map(pos);
                let n0 = cross(sub(p[1], p[0]), sub(p[2], p[0]));
                let moved = tri.map(|v| if v == from { pt } else { pos(v) });
                let n1 = cross(sub(moved[1], moved[0]), sub(moved[2], moved[0]));
                let (l0, l1) = (dot(n0, n0).sqrt(), dot(n1, n1).sqrt());
                if l1 < 1e-12 || dot(n0, n1) < MIN_COS * l0 * l1 || (l0 > 0.0 && dot(n0, sub(pt, p[0])).abs() > o.max_move * l0) || dot(n1, sub(pf, moved[0])).abs() > o.max_move * l1 {
                    ok = false;
                    break;
                }
            }
            if ok && boundary_edges[from as usize] > 0 {
                // The other boundary neighbour: an edge of `from` in exactly one live face.
                for &w in &nf {
                    if w == to {
                        continue;
                    }
                    let faces = tf.iter().filter(|&&t| alive[t as usize] && triangles[t as usize].contains(&w)).count();
                    if faces == 1 {
                        let pw = pos(w);
                        let d = sub(pt, pw);
                        let l = dot(d, d);
                        let s = if l > 0.0 { (dot(sub(pf, pw), d) / l).clamp(0.0, 1.0) } else { 0.0 };
                        let off = sub(pf, [pw[0] + s * d[0], pw[1] + s * d[1], pw[2] + s * d[2]]);
                        if dot(off, off) > o.max_move * o.max_move {
                            ok = false;
                        }
                    }
                }
            }
            if !ok {
                continue;
            }
            for &t in tf {
                let t = t as usize;
                if !alive[t] {
                    continue;
                }
                if triangles[t].contains(&to) {
                    alive[t] = false;
                    live -= 1;
                } else {
                    for v in triangles[t].iter_mut() {
                        if *v == from {
                            *v = to;
                        }
                    }
                }
            }
            let qf = q[from as usize];
            q[to as usize].add(&qf);
            touched[from as usize] = true;
            touched[to as usize] = true;
            collapsed += 1;
        }
        let mut k = 0;
        triangles.retain(|_| {
            k += 1;
            alive[k - 1]
        });
        if collapsed == 0 {
            break;
        }
    }
    passes
}

/// The collider's simplification (see the module's comment). Returns the passes of each stage.
pub fn collider(m: &mut Mesh, s: &Settings) -> [usize; 3] {
    let nv = m.positions.len();
    let mut q = quadrics(m);
    let o = Options { error: s.error as f64, max_move: 2.0 * s.error as f64, target: 0, into_locked: false };
    // Stage 1: columns in parallel, the vertices they share locked.
    let column = |t: &[u32; 3]| -> u64 {
        let c: [f32; 3] = std::array::from_fn(|i| t.iter().map(|&v| m.positions[v as usize][i]).sum::<f32>() / 3.0);
        let (x, z) = ((c[0] / CHUNK).floor() as i64 + (1 << 30), (c[2] / CHUNK).floor() as i64 + (1 << 30));
        (x as u64) << 32 | z as u64
    };
    let mut order: Vec<(u64, u32)> = m.triangles.iter().enumerate().map(|(i, t)| (column(t), i as u32)).collect();
    order.par_sort_unstable();
    let mut owner = vec![u64::MAX; nv];
    let mut locked = vec![false; nv];
    for &(k, t) in &order {
        for v in m.triangles[t as usize] {
            let o = &mut owner[v as usize];
            if *o == u64::MAX {
                *o = k;
            } else if *o != k {
                locked[v as usize] = true;
            }
        }
    }
    drop(owner);
    let mut groups = Vec::new();
    let mut i = 0;
    while i < order.len() {
        let mut j = i;
        while j < order.len() && order[j].0 == order[i].0 {
            j += 1;
        }
        groups.push(i..j);
        i = j;
    }
    let results: Vec<(Vec<[u32; 3]>, Vec<(u32, Quadric)>, usize)> = groups
        .par_iter()
        .map(|range| {
            let global: Vec<[u32; 3]> = order[range.clone()].iter().map(|&(_, t)| m.triangles[t as usize]).collect();
            let mut verts: Vec<u32> = global.iter().flatten().copied().collect();
            verts.sort_unstable();
            verts.dedup();
            let local = |v: u32| verts.binary_search(&v).unwrap() as u32;
            let mut tris: Vec<[u32; 3]> = global.iter().map(|t| t.map(local)).collect();
            let positions: Vec<[f32; 3]> = verts.iter().map(|&v| m.positions[v as usize]).collect();
            let mut lq: Vec<Quadric> = verts.iter().map(|&v| q[v as usize]).collect();
            let lock: Vec<bool> = verts.iter().map(|&v| locked[v as usize]).collect();
            let passes = simplify(&positions, &mut tris, &mut lq, &lock, &o);
            let back: Vec<(u32, Quadric)> = verts.iter().enumerate().filter(|(k, _)| !lock[*k]).map(|(k, &v)| (v, lq[k])).collect();
            (tris.into_iter().map(|t| t.map(|v| verts[v as usize])).collect(), back, passes)
        })
        .collect();
    m.triangles.clear();
    let mut first = 0;
    for (tris, back, passes) in results {
        m.triangles.extend(tris);
        for (v, qq) in back {
            q[v as usize] = qq;
        }
        first = first.max(passes);
    }
    // Stage 2: everything, seams included.
    let none = vec![false; nv];
    let second = simplify(&m.positions, &mut m.triangles, &mut q, &none, &Options { into_locked: true, ..o });
    // Stage 3: the budget.
    let mut third = 0;
    let mut e = o.error;
    while m.triangles.len() > s.budget && e < 0.5 {
        e *= 1.5;
        third += simplify(&m.positions, &mut m.triangles, &mut q, &none, &Options { error: e, max_move: 2.0 * e, target: s.budget, into_locked: true });
    }
    m.compact();
    [first, second, third]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A w × h grid of unit squares in the plane y = height(x, z), triangulated.
    fn grid(w: usize, h: usize, height: impl Fn(f32, f32) -> f32) -> Mesh {
        let mut m = Mesh::default();
        for z in 0..=h {
            for x in 0..=w {
                let (fx, fz) = (x as f32 * 0.1, z as f32 * 0.1);
                m.positions.push([fx, height(fx, fz), fz]);
            }
        }
        let id = |x: usize, z: usize| (z * (w + 1) + x) as u32;
        for z in 0..h {
            for x in 0..w {
                m.triangles.push([id(x, z), id(x, z + 1), id(x + 1, z)]);
                m.triangles.push([id(x + 1, z), id(x, z + 1), id(x + 1, z + 1)]);
            }
        }
        m
    }

    fn settings() -> Settings {
        Settings { error: 0.01, budget: usize::MAX, ..Default::default() }
    }

    #[test]
    fn a_flat_floor_collapses_and_keeps_its_outline() {
        let mut m = grid(40, 30, |_, _| 0.0);
        collider(&mut m, &settings());
        assert!(m.triangles.len() <= 8, "{}", m.triangles.len());
        let a = m.audit();
        assert_eq!(a.non_manifold_edges, 0);
        // The outline: still the 4 m × 3 m rectangle.
        let area: f32 = m.triangles.iter().map(|t| m.area(t)).sum();
        assert!((area - 12.0).abs() < 1e-3, "{area}");
        assert!(m.positions.iter().all(|p| p[1] == 0.0));
    }

    #[test]
    fn a_step_survives() {
        // A 20 cm step across the middle of a floor: its riser stays where it is.
        let mut m = grid(40, 20, |x, _| if x > 2.0 { 0.2 } else { 0.0 });
        collider(&mut m, &settings());
        assert_eq!(m.audit().non_manifold_edges, 0);
        let low = m.positions.iter().filter(|p| p[0] <= 2.0 + 1e-4 && p[1] > 0.1).count();
        assert_eq!(low, 0, "the upper tread spread over the lower one");
        assert!(m.positions.iter().any(|p| (p[0] - 2.0).abs() < 1e-4 && p[1] == 0.0));
        assert!(m.positions.iter().any(|p| (p[0] - 2.1).abs() < 1e-4 && p[1] == 0.2));
    }

    #[test]
    fn a_closed_surface_stays_closed_and_meets_the_budget() {
        // A sphere from marching cubes, then a budget.
        let mut m = Mesh::default();
        let n = 60;
        for i in 0..=n {
            for j in 0..n {
                let (th, ph) = (std::f32::consts::PI * i as f32 / n as f32, 2.0 * std::f32::consts::PI * j as f32 / n as f32);
                m.positions.push([th.sin() * ph.cos(), th.cos(), th.sin() * ph.sin()]);
            }
        }
        // Merge the poles' rings into single vertices.
        let pole = |i: usize, j: usize| -> u32 {
            if i == 0 {
                0
            } else if i == n {
                (n * n) as u32
            } else {
                (i * n + j % n) as u32
            }
        };
        for i in 0..n {
            for j in 0..n {
                let (a, b, c, d) = (pole(i, j), pole(i, j + 1), pole(i + 1, j), pole(i + 1, j + 1));
                if a != b {
                    m.triangles.push([a, c, b]);
                }
                if c != d {
                    m.triangles.push([b, c, d]);
                }
            }
        }
        m.compact();
        let a0 = m.audit();
        assert_eq!((a0.boundary_edges, a0.non_manifold_edges), (0, 0));
        collider(&mut m, &Settings { error: 0.005, budget: 300, ..Default::default() });
        let a = m.audit();
        assert!(m.triangles.len() <= 300, "{}", m.triangles.len());
        assert_eq!((a.boundary_edges, a.non_manifold_edges, a.components), (0, 0, 1), "{a:?}");
    }
}
