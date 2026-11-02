//! VRChat's 3D views of a reconstruction: the splats as a tree of boxes (OrbitersContracts Docs/splat-tree.md). The
//! splats are the leaves of an octree; every box above them holds a merged stand-in for what it contains, about a
//! quarter as many splats, and the size of the detail that merge loses (its error, metres). A viewer draws, along
//! every path from the root to a leaf, the first box whose error looks small enough from where it stands: one pass
//! over the boxes, no walk of the tree. The merged levels are then tuned on the GPU to look like the leaves from the
//! distances they stand in for them ([`crate::tree_tune`]).

use crate::gaussians::{Gaussians, logit, rotation_matrix, sigmoid};
use rayon::prelude::*;
use std::collections::HashMap;

pub const MAGIC: &[u8; 8] = orbiters_contracts::splat_tree::MAGIC;
/// The file's version (crate::tree_file: 20-byte splat records, 64-byte boxes).
pub const VERSION: u32 = orbiters_contracts::splat_tree::VERSION;
/// A box with more splats than this is split in eight.
pub const LEAF_SPLATS: usize = 2048;
/// A merged box holds about this fraction of what its children hold.
const MERGE: usize = 4;
const MAX_DEPTH: u32 = 16;
/// Splats fainter than this never show and are left out.
const MIN_OPACITY: f32 = 1.0 / 255.0;
/// A box's sphere reaches this many standard deviations past its splats' centres.
const REACH: f32 = 2.0;
/// Header flag: a coarse file, the top of a tree whose deepest boxes stand in for everything below them.
pub const FLAG_COARSE: u32 = orbiters_contracts::splat_tree::FLAG_COARSE;

/// One box of a tree. Boxes are in breadth-first order (a parent before its children, the root first) and each one's
/// splats are contiguous, in box order.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// A sphere holding this box's splats and every box below it (a child's sphere lies inside its parent's).
    pub center: [f32; 3],
    pub radius: f32,
    /// The detail its splats leave out (metres): 0 for a leaf, never less than a child's.
    pub error: f32,
    pub parent: Option<u32>,
    pub depth: u32,
    pub start: u32,
    pub count: u32,
    pub leaf: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub nodes: Vec<Node>,
    /// Every box's splats, box after box (Unity's world, like a reconstruction's).
    pub splats: Gaussians,
}

/// A Gaussian as merging reads it: centre, covariance (xx, xy, xz, yy, yz, zz), colour and opacity.
#[derive(Clone, Copy, Debug)]
struct Moment {
    mean: [f64; 3],
    cov: [f64; 6],
    color: [f64; 3],
    alpha: f64,
}

impl Moment {
    fn of(g: &Gaussians, i: usize) -> Self {
        let r = rotation_matrix(g.rotations[i]);
        let v = g.log_scales[i].map(|s| ((2.0 * s) as f64).exp());
        let c = |a: usize, b: usize| (0..3).map(|k| r[a][k] as f64 * r[b][k] as f64 * v[k]).sum::<f64>();
        Self {
            mean: g.means[i].map(f64::from),
            cov: [c(0, 0), c(0, 1), c(0, 2), c(1, 1), c(1, 2), c(2, 2)],
            color: g.colors[i].map(f64::from),
            alpha: sigmoid(g.opacity_logits[i]) as f64,
        }
    }

    /// About the area it covers: the root of the sum of its principal 2×2 minors (σ² for a flat disc of σ).
    fn area(&self) -> f64 {
        let [xx, xy, xz, yy, yz, zz] = self.cov;
        (xx * yy - xy * xy + xx * zz - xz * xz + yy * zz - yz * yz).max(1e-16).sqrt()
    }

    /// Its largest standard deviation.
    fn sigma(&self) -> f64 {
        eigen(self.cov).0.iter().copied().fold(0.0, f64::max).sqrt()
    }

    fn push_to(&self, g: &mut Gaussians) {
        let (values, axes) = eigen(self.cov);
        let scales = values.map(|v| (v.max(1e-10).sqrt() as f32).max(1e-5).ln());
        g.push(self.mean.map(|v| v as f32), scales, quaternion(axes), logit(self.alpha.clamp(1e-4, 0.995) as f32), self.color.map(|v| v as f32));
    }
}

/// One Gaussian standing in for `group`: the same weighted mean and spread (the moments of the mixture, each weighted by
/// opacity times area), the weighted colour, and the opacity that keeps the covered area's weight.
fn merge(group: &[Moment]) -> Moment {
    if group.len() == 1 {
        return group[0];
    }
    let weights: Vec<f64> = group.iter().map(|m| m.alpha * m.area()).collect();
    let total = weights.iter().sum::<f64>().max(1e-20);
    let mut mean = [0.0; 3];
    let mut color = [0.0; 3];
    for (m, w) in group.iter().zip(&weights) {
        for k in 0..3 {
            mean[k] += w * m.mean[k] / total;
            color[k] += w * m.color[k] / total;
        }
    }
    let mut cov = [0.0; 6];
    const PAIRS: [(usize, usize); 6] = [(0, 0), (0, 1), (0, 2), (1, 1), (1, 2), (2, 2)];
    for (m, w) in group.iter().zip(&weights) {
        let d = [m.mean[0] - mean[0], m.mean[1] - mean[1], m.mean[2] - mean[2]];
        for (k, (a, b)) in PAIRS.iter().enumerate() {
            cov[k] += w * (m.cov[k] + d[*a] * d[*b]) / total;
        }
    }
    let mut merged = Moment { mean, cov, color, alpha: 0.0 };
    let covered: f64 = group.iter().map(|m| m.alpha * m.area()).sum();
    merged.alpha = (covered / merged.area()).min(0.995);
    merged
}

/// Eigenvalues and eigenvectors (columns, a rotation) of a symmetric 3×3 matrix (xx, xy, xz, yy, yz, zz), by Jacobi.
fn eigen(c: [f64; 6]) -> ([f64; 3], [[f64; 3]; 3]) {
    let mut a = [[c[0], c[1], c[2]], [c[1], c[3], c[4]], [c[2], c[4], c[5]]];
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..24 {
        let off = a[0][1].abs() + a[0][2].abs() + a[1][2].abs();
        if off < 1e-18 * (a[0][0].abs() + a[1][1].abs() + a[2][2].abs()).max(1e-30) {
            break;
        }
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            if a[p][q].abs() < 1e-30 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = if theta >= 0.0 { 1.0 } else { -1.0 } / (theta.abs() + (theta * theta + 1.0).sqrt());
            let (cs, sn) = (1.0 / (t * t + 1.0).sqrt(), t / (t * t + 1.0).sqrt());
            for k in 0..3 {
                let (akp, akq) = (a[k][p], a[k][q]);
                a[k][p] = cs * akp - sn * akq;
                a[k][q] = sn * akp + cs * akq;
            }
            for k in 0..3 {
                let (apk, aqk) = (a[p][k], a[q][k]);
                a[p][k] = cs * apk - sn * aqk;
                a[q][k] = sn * apk + cs * aqk;
            }
            for row in &mut v {
                let (vp, vq) = (row[p], row[q]);
                row[p] = cs * vp - sn * vq;
                row[q] = sn * vp + cs * vq;
            }
        }
    }
    // A rotation: the third axis is the cross product of the first two.
    let (c0, c1) = ([v[0][0], v[1][0], v[2][0]], [v[0][1], v[1][1], v[2][1]]);
    let c2 = [c0[1] * c1[2] - c0[2] * c1[1], c0[2] * c1[0] - c0[0] * c1[2], c0[0] * c1[1] - c0[1] * c1[0]];
    for k in 0..3 {
        v[k][2] = c2[k];
    }
    ([a[0][0], a[1][1], a[2][2]], v)
}

/// The quaternion (w, x, y, z) of a rotation matrix whose columns are the Gaussian's axes.
fn quaternion(m: [[f64; 3]; 3]) -> [f32; 4] {
    let trace = m[0][0] + m[1][1] + m[2][2];
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [0.25 * s, (m[2][1] - m[1][2]) / s, (m[0][2] - m[2][0]) / s, (m[1][0] - m[0][1]) / s]
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        [(m[2][1] - m[1][2]) / s, 0.25 * s, (m[0][1] + m[1][0]) / s, (m[0][2] + m[2][0]) / s]
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        [(m[0][2] - m[2][0]) / s, (m[0][1] + m[1][0]) / s, 0.25 * s, (m[1][2] + m[2][1]) / s]
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        [(m[1][0] - m[0][1]) / s, (m[0][2] + m[2][0]) / s, (m[1][2] + m[2][1]) / s, 0.25 * s]
    };
    q.map(|v| v as f32)
}

/// Clusters `group` on a grid over the cube (`lo`, `size`) into at most `target` cells, the finest grid that does:
/// each cell's splats as one, and the cell's size.
fn cluster(group: &[Moment], lo: [f64; 3], size: f64, target: usize) -> (Vec<Moment>, f64) {
    let cells_at = |r: u64| {
        let cell = size / r as f64;
        let key = |m: &Moment| {
            let c = |k: usize| (((m.mean[k] - lo[k]) / cell).floor() as i64).clamp(0, r as i64 - 1) as u64;
            (c(0) * r + c(1)) * r + c(2)
        };
        let mut cells: HashMap<u64, Vec<usize>> = HashMap::new();
        for (i, m) in group.iter().enumerate() {
            cells.entry(key(m)).or_default().push(i);
        }
        cells
    };
    let (mut low, mut high) = (1u64, 1u64 << 12);
    while low < high {
        let mid = (low + high).div_ceil(2);
        if cells_at(mid).len() <= target { low = mid } else { high = mid - 1 }
    }
    let mut cells: Vec<(u64, Vec<usize>)> = cells_at(low).into_iter().collect();
    cells.sort_unstable_by_key(|c| c.0);
    let merged = cells.iter().map(|(_, members)| merge(&members.iter().map(|&i| group[i]).collect::<Vec<_>>())).collect();
    (merged, size / low as f64)
}

/// Importance: opacity times about the area covered (the splats a smaller tree keeps).
fn importance(g: &Gaussians, i: usize) -> f32 {
    let s = g.log_scales[i].map(f32::exp);
    sigmoid(g.opacity_logits[i]) * (s[0] * s[1] + s[1] * s[2] + s[0] * s[2])
}

/// Every splat's [`importance`] (opacity times area), for [`leaves_by`].
pub fn importances(g: &Gaussians) -> Vec<f32> {
    (0..g.len()).into_par_iter().map(|i| importance(g, i)).collect()
}

/// The splats that can be in a tree: visible and finite.
pub fn eligible(g: &Gaussians) -> usize {
    (0..g.len()).filter(|&i| g.is_finite(i) && sigmoid(g.opacity_logits[i]) >= MIN_OPACITY).count()
}

/// The splats worth a tree: visible and finite, at most `max` of them (the most important).
pub fn leaves(g: &Gaussians, max: usize) -> Vec<usize> {
    leaves_by(g, max, &importances(g))
}

/// The splats worth a tree: visible and finite, at most `max` of them, those `score` (one a splat) ranks highest.
pub fn leaves_by(g: &Gaussians, max: usize, score: &[f32]) -> Vec<usize> {
    let mut keep: Vec<usize> = (0..g.len()).filter(|&i| g.is_finite(i) && sigmoid(g.opacity_logits[i]) >= MIN_OPACITY).collect();
    if keep.len() > max {
        keep.select_nth_unstable_by(max, |&a, &b| score[b].total_cmp(&score[a]));
        keep.truncate(max);
        keep.sort_unstable();
    }
    keep
}

/// The octree while it is built: a cube, its children or (a leaf) its splats.
struct Cell {
    lo: [f64; 3],
    size: f64,
    depth: u32,
    children: Vec<usize>,
    splats: Vec<usize>,
}

fn split(cells: &mut Vec<Cell>, at: usize, means: &[[f64; 3]]) {
    let Cell { lo, size, depth, .. } = cells[at];
    if cells[at].splats.len() <= LEAF_SPLATS || depth >= MAX_DEPTH {
        return;
    }
    let half = size / 2.0;
    let mut octants: [Vec<usize>; 8] = Default::default();
    for &i in &cells[at].splats {
        let m = means[i];
        let o = (0..3).map(|k| usize::from(m[k] >= lo[k] + half) << k).sum::<usize>();
        octants[o].push(i);
    }
    cells[at].splats.clear();
    for (o, splats) in octants.into_iter().enumerate().filter(|(_, s)| !s.is_empty()) {
        let child_lo = [0, 1, 2].map(|k| lo[k] + if o >> k & 1 == 1 { half } else { 0.0 });
        cells.push(Cell { lo: child_lo, size: half, depth: depth + 1, children: Vec::new(), splats });
        let child = cells.len() - 1;
        cells[at].children.push(child);
        split(cells, child, means);
    }
}

/// The tree of `g`'s splats `keep` (from [`leaves`]): an octree of at most [`LEAF_SPLATS`] a leaf, merged boxes
/// above.
pub fn build(g: &Gaussians, keep: &[usize]) -> Tree {
    let moments: Vec<Moment> = keep.par_iter().map(|&i| Moment::of(g, i)).collect();
    if moments.is_empty() {
        return Tree::default();
    }
    let means: Vec<[f64; 3]> = moments.iter().map(|m| m.mean).collect();
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for m in &means {
        for k in 0..3 {
            lo[k] = lo[k].min(m[k]);
            hi[k] = hi[k].max(m[k]);
        }
    }
    let size = (0..3).map(|k| hi[k] - lo[k]).fold(1e-3, f64::max) * 1.0001;
    let mut cells = vec![Cell { lo, size, depth: 0, children: Vec::new(), splats: (0..moments.len()).collect() }];
    split(&mut cells, 0, &means);

    // Bottom up, a depth at a time: each box's own splats, error and sphere.
    let max_depth = cells.iter().map(|c| c.depth).max().unwrap_or(0);
    let mut own: Vec<Vec<Moment>> = vec![Vec::new(); cells.len()];
    let mut error = vec![0.0f64; cells.len()];
    let mut sphere = vec![([0.0f64; 3], 0.0f64); cells.len()];
    for depth in (0..=max_depth).rev() {
        let at: Vec<usize> = (0..cells.len()).filter(|&c| cells[c].depth == depth).collect();
        let done: Vec<(usize, Vec<Moment>, f64, ([f64; 3], f64))> = at
            .par_iter()
            .map(|&c| {
                let cell = &cells[c];
                let (splats, err) = if cell.children.is_empty() {
                    (cell.splats.iter().map(|&i| moments[i]).collect::<Vec<_>>(), 0.0)
                } else {
                    let below: Vec<Moment> = cell.children.iter().flat_map(|&k| own[k].iter().copied()).collect();
                    let (merged, cell_size) = cluster(&below, cell.lo, cell.size, below.len().div_ceil(MERGE).max(1));
                    (merged, cell.children.iter().map(|&k| error[k]).fold(cell_size, f64::max))
                };
                let s = bound(&splats, cell.children.iter().map(|&k| sphere[k]));
                (c, splats, err, s)
            })
            .collect();
        for (c, splats, err, s) in done {
            own[c] = splats;
            error[c] = err;
            sphere[c] = s;
        }
    }

    // Breadth first: the root, then each depth's boxes in their parents' order.
    let mut order = vec![0usize];
    let mut parent_of = vec![None; cells.len()];
    let mut next = 0;
    while next < order.len() {
        let c = order[next];
        for &k in &cells[c].children {
            parent_of[k] = Some(c);
            order.push(k);
        }
        next += 1;
    }
    let mut index = vec![0u32; cells.len()];
    for (i, &c) in order.iter().enumerate() {
        index[c] = i as u32;
    }
    let mut tree = Tree::default();
    for &c in &order {
        let start = tree.splats.len() as u32;
        for m in &own[c] {
            m.push_to(&mut tree.splats);
        }
        let (center, radius) = sphere[c];
        tree.nodes.push(Node {
            center: center.map(|v| v as f32),
            radius: radius as f32,
            error: error[c] as f32,
            parent: parent_of[c].map(|p| index[p]),
            depth: cells[c].depth,
            start,
            count: own[c].len() as u32,
            leaf: cells[c].children.is_empty(),
        });
    }
    tree
}

/// A sphere holding `splats` (to [`REACH`] standard deviations) and every sphere of `inner`.
fn bound(splats: &[Moment], inner: impl Iterator<Item = ([f64; 3], f64)> + Clone) -> ([f64; 3], f64) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    let reach: Vec<f64> = splats.iter().map(|m| REACH as f64 * m.sigma()).collect();
    for (m, r) in splats.iter().zip(&reach) {
        for k in 0..3 {
            lo[k] = lo[k].min(m.mean[k] - r);
            hi[k] = hi[k].max(m.mean[k] + r);
        }
    }
    for (c, r) in inner.clone() {
        for k in 0..3 {
            lo[k] = lo[k].min(c[k] - r);
            hi[k] = hi[k].max(c[k] + r);
        }
    }
    let center = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let distance = |p: [f64; 3]| ((p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2) + (p[2] - center[2]).powi(2)).sqrt();
    let radius = splats.iter().zip(&reach).map(|(m, r)| distance(m.mean) + r).chain(inner.map(|(c, r)| distance(c) + r)).fold(0.0, f64::max);
    (center, radius)
}

impl Tree {
    pub fn len(&self) -> usize {
        self.splats.len()
    }

    pub fn is_empty(&self) -> bool {
        self.splats.is_empty()
    }

    /// The splats of the leaves only: what the tree shows from up close.
    pub fn leaf_splats(&self) -> Gaussians {
        self.select(|n| n.leaf)
    }

    /// The splats of the boxes `pick` keeps, in box order.
    pub fn select(&self, pick: impl Fn(&Node) -> bool) -> Gaussians {
        let mut g = Gaussians::default();
        for n in self.nodes.iter().filter(|n| pick(n)) {
            for i in n.start as usize..(n.start + n.count) as usize {
                g.push(self.splats.means[i], self.splats.log_scales[i], self.splats.rotations[i], self.splats.opacity_logits[i], self.splats.colors[i]);
            }
        }
        g
    }

    /// The top of the tree, as many depths as fit in `max` splats (one at least): its deepest boxes become its leaves.
    pub fn top(&self, max: usize) -> Tree {
        let mut depth = 0;
        let deepest = self.nodes.iter().map(|n| n.depth).max().unwrap_or(0);
        while depth < deepest && self.nodes.iter().filter(|n| n.depth <= depth + 1).map(|n| n.count as usize).sum::<usize>() <= max {
            depth += 1;
        }
        let mut top = Tree { nodes: Vec::new(), splats: self.select(|n| n.depth <= depth) };
        let mut start = 0;
        for n in self.nodes.iter().filter(|n| n.depth <= depth) {
            let leaf = n.leaf || n.depth == depth;
            top.nodes.push(Node { error: if leaf { 0.0 } else { n.error }, start, leaf, ..n.clone() });
            start += n.count;
        }
        top
    }

    /// The file VRChat worlds load (OrbitersContracts Docs/splat-tree.md, version 2: crate::tree_file): a header, the
    /// boxes with their splats' bounds, and the splats in 20 bytes each, right-handed like the web `.splat` (Unity's Z
    /// negated). `key` names the upload the file belongs to.
    pub fn file(&self, key: u32, flags: u32) -> Vec<u8> {
        crate::tree_file::write(self, key, flags)
    }

    /// A tree back from [`Tree::file`]'s bytes (what a world loads: checks of what a shared file holds), the boxes'
    /// depths and leaves worked out from their parents, the splats as their records decode. Anything else is an error,
    /// a version 1 file too (crate::tree_file::parse).
    pub fn parse(bytes: &[u8]) -> anyhow::Result<Tree> {
        crate::tree_file::parse(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A floor of `n` small splats and a few big ones above it.
    fn scene(n: usize) -> Gaussians {
        let mut g = Gaussians::default();
        let mut rng = fastrand::Rng::with_seed(7);
        for _ in 0..n {
            g.push([rng.f32() * 20.0 - 10.0, 0.0, rng.f32() * 20.0 - 10.0], [-3.0, -6.0, -3.0], [1.0, 0.0, 0.0, 0.0], 2.0, [rng.f32(), 0.5, 0.2]);
        }
        for k in 0..8 {
            g.push([k as f32, 3.0, 0.0], [-1.0; 3], [0.9, 0.1, 0.3, 0.2], 0.0, [1.0, 0.0, 0.0]);
        }
        g
    }

    #[test]
    fn every_path_from_the_root_meets_one_leaf_and_errors_and_spheres_nest() {
        let g = scene(20_000);
        let tree = build(&g, &leaves(&g, usize::MAX));
        assert_eq!(tree.leaf_splats().len(), g.len());
        assert!(tree.len() < g.len() * 3 / 2, "{} splats for {}", tree.len(), g.len());
        assert!(tree.nodes.len() > 9);
        let mut start = 0;
        for (i, n) in tree.nodes.iter().enumerate() {
            assert_eq!(n.start, start, "contiguous splats in box order");
            start += n.count;
            assert!(n.count > 0);
            assert_eq!(n.leaf, n.error == 0.0 && !tree.nodes.iter().any(|c| c.parent == Some(i as u32)));
            if let Some(p) = n.parent {
                let p = &tree.nodes[p as usize];
                assert!(p.depth + 1 == n.depth && p.error >= n.error && !p.leaf);
                let d = (0..3).map(|k| (n.center[k] - p.center[k]).powi(2)).sum::<f32>().sqrt();
                assert!(d + n.radius <= p.radius * 1.0001 + 1e-4, "a child's sphere inside its parent's");
            } else {
                assert_eq!(i, 0, "the root first");
            }
        }
        assert_eq!(start as usize, tree.len());
        // Breadth first: a parent before its children.
        assert!(tree.nodes.iter().enumerate().all(|(i, n)| n.parent.is_none_or(|p| (p as usize) < i)));
    }

    #[test]
    fn merging_keeps_the_mixture_moments() {
        let mut g = Gaussians::default();
        g.push([-1.0, 0.0, 0.0], [-2.0; 3], [1.0, 0.0, 0.0, 0.0], 3.0, [1.0, 0.0, 0.0]);
        g.push([1.0, 0.0, 0.0], [-2.0; 3], [1.0, 0.0, 0.0, 0.0], 3.0, [0.0, 0.0, 1.0]);
        let m = merge(&[Moment::of(&g, 0), Moment::of(&g, 1)]);
        assert!(m.mean.iter().all(|v| v.abs() < 1e-9));
        assert!((m.cov[0] - (1.0 + (-4.0f64).exp())).abs() < 1e-6, "spread along x from the centres");
        assert!((m.color[0] - 0.5).abs() < 1e-9 && (m.color[2] - 0.5).abs() < 1e-9);
        let mut back = Gaussians::default();
        m.push_to(&mut back);
        let again = Moment::of(&back, 0);
        assert!(again.cov.iter().zip(m.cov).all(|(a, b)| (a - b).abs() < 1e-4 * b.abs().max(1e-3)), "{:?} vs {:?}", again.cov, m.cov);
    }

    #[test]
    fn the_top_of_a_tree_fits_its_budget_and_ends_in_leaves() {
        let g = scene(30_000);
        let tree = build(&g, &leaves(&g, usize::MAX));
        let top = tree.top(4000);
        assert!(top.len() <= 4000 && !top.is_empty());
        assert!(top.nodes.iter().all(|n| n.leaf == (n.error == 0.0)));
        assert!(top.nodes.iter().all(|n| n.leaf || top.nodes.iter().any(|c| c.parent.is_some_and(|p| top.nodes[p as usize] == *n))));
    }

    #[test]
    fn a_ranking_picks_the_leaves_and_invisible_splats_never_count() {
        let mut g = scene(1000);
        g.opacity_logits[3] = -20.0;
        // Ranked by index: the last ones first, but the invisible one never.
        let score: Vec<f32> = (0..g.len()).map(|i| i as f32).collect();
        let keep = leaves_by(&g, 100, &score);
        assert_eq!(keep, (g.len() - 100..g.len()).collect::<Vec<_>>());
        let reversed: Vec<f32> = score.iter().map(|s| -s).collect();
        let keep = leaves_by(&g, 5, &reversed);
        assert_eq!(keep, vec![0, 1, 2, 4, 5]);
        assert_eq!(eligible(&g), g.len() - 1);
        assert_eq!(leaves(&g, usize::MAX).len(), g.len() - 1);
    }
}
