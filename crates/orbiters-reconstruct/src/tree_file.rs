//! A splat tree's file (OrbitersContracts Docs/splat-tree.md, version 2): what VRChat worlds download, written from a
//! [`Tree`] and read back into one. The records are the contract's (`orbiters_contracts::splat_tree`): a 32-byte
//! header, 64 bytes a box (its sphere, error, parent and splat range, and the bounds its splats' positions are stored
//! in) and 20 bytes a splat (16-bit steps of its box's bounds, half-float log scales, the smallest three components of
//! its rotation, colour and opacity bytes), right-handed (Unity's world with Z negated).
//!
//! The 20-byte record replaced version 1's 32 bytes (the web `.splat` record) in 2026-11: 1.6 times smaller downloads
//! for the same picture (held-out PSNR +0.0009 / −0.0155 / −0.0011 dB on Home, the plaza walk and Needle, −0.0010 dB
//! over the plaza's 104 off-path views; lab report compact-formats.md). What the file rounds is what the merged depths
//! are tuned against ([`Tree::select_stored`], crate::tree_tune), so the tuning sees what a world draws.

use crate::gaussians::{Gaussians, logit, sigmoid};
use crate::tree::{Node, Tree};
use orbiters_i18n::tr;
use orbiters_contracts::splat_tree::{self as st, Bounds, BoxRecord, Header, Splat};
use rayon::prelude::*;
use std::ops::Range;

fn byte(v: f32) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Splat `i` of `g` as its record holds it: right-handed (positions' z negated; a rotation R becomes S R S, the
/// quaternion (w, −x, −y, z)), colour and opacity in bytes. The record's codec normalises the rotation.
fn stored(g: &Gaussians, i: usize) -> Splat {
    let [x, y, z] = g.means[i];
    let q = g.rotations[i];
    let c = g.colors[i];
    Splat {
        position: [x, y, -z],
        log_scale: g.log_scales[i],
        rotation: [q[0], -q[1], -q[2], q[3]],
        rgba: [byte(c[0]), byte(c[1]), byte(c[2]), byte(sigmoid(g.opacity_logits[i]))],
    }
}

/// The opacity logit of an opacity byte (as crate::web::read_splat reads one: 0 and 255 just inside the open interval).
fn opacity_logit(alpha: u8) -> f32 {
    logit((alpha as f32 / 255.0).clamp(1e-6, 1.0 - 1e-6))
}

/// A decoded record back in Unity's world.
fn unity(s: &Splat) -> ([f32; 3], [f32; 3], [f32; 4], f32, [f32; 3]) {
    let [x, y, z] = s.position;
    let q = s.rotation;
    (
        [x, y, -z],
        s.log_scale,
        [q[0], -q[1], -q[2], q[3]],
        opacity_logit(s.rgba[3]),
        [s.rgba[0] as f32 / 255.0, s.rgba[1] as f32 / 255.0, s.rgba[2] as f32 / 255.0],
    )
}

/// The bounds a box's splats `range` of `g` are stored in (right-handed).
fn bounds_of(g: &Gaussians, range: Range<usize>) -> Bounds {
    Bounds::of(range.map(|i| {
        let [x, y, z] = g.means[i];
        [x, y, -z]
    }))
}

/// The splats `range` of `g`, one box's, as the file stores them: each one replaced by what its record decodes to in
/// that box's bounds (the position within half a step of 1/65535 of the box's extent, the scales within half a half
/// float's step of their log, the rotation within 6.3 mrad, colour and opacity in bytes).
pub fn round_box(g: &mut Gaussians, range: Range<usize>) {
    let bounds = bounds_of(g, range.clone());
    for i in range {
        let (mean, log_scale, rotation, opacity_logit, color) = unity(&st::decode(&st::encode(&stored(g, i), &bounds), &bounds));
        g.means[i] = mean;
        g.log_scales[i] = log_scale;
        g.rotations[i] = rotation;
        g.opacity_logits[i] = opacity_logit;
        g.colors[i] = color;
    }
}

/// Every splat of `g` as a record rounds it, but for its position (which depends on the box it ends up in; in a leaf
/// box it barely moves: steps of 0.012–0.056 mm at the median on two production trees, at most 0.23 mm within 20 m of
/// the start view): log scales as half floats, the rotation's smallest three components, colour and opacity bytes. For
/// work on leaves before their tree is built.
pub fn round_attributes(g: &mut Gaussians) {
    let origin = Bounds { min: [0.0; 3], step: [0.0; 3] };
    for i in 0..g.len() {
        let (_, log_scale, rotation, opacity_logit, color) = unity(&st::decode(&st::encode(&stored(g, i), &origin), &origin));
        g.log_scales[i] = log_scale;
        g.rotations[i] = rotation;
        g.opacity_logits[i] = opacity_logit;
        g.colors[i] = color;
    }
}

/// The upload key a file's header names (after the checks of [`Header::read`]: a version 2 tree of the right length).
pub fn key(bytes: &[u8]) -> anyhow::Result<u32> {
    Ok(Header::read(bytes).map_err(|e| anyhow::anyhow!("{e}"))?.key)
}

/// `tree`'s file: its header (`key`, `flags`), its boxes and their splats.
pub fn write(tree: &Tree, key: u32, flags: u32) -> Vec<u8> {
    let g = &tree.splats;
    let range = |n: &Node| n.start as usize..(n.start + n.count) as usize;
    let bounds: Vec<Bounds> = tree.nodes.par_iter().map(|n| bounds_of(g, range(n))).collect();
    let records: Vec<Vec<u8>> = tree
        .nodes
        .par_iter()
        .zip(&bounds)
        .map(|(n, b)| {
            let mut out = Vec::with_capacity(n.count as usize * st::SPLAT_BYTES);
            for i in range(n) {
                debug_assert!(g.is_finite(i), "a tree's splats are finite");
                out.extend_from_slice(&st::encode(&stored(g, i), b));
            }
            out
        })
        .collect();
    let mut bytes = Vec::with_capacity(st::file_len(tree.nodes.len(), tree.len()).expect("a tree's size fits memory"));
    Header { boxes: tree.nodes.len() as u32, splats: tree.len() as u32, key, flags }.write_to(&mut bytes);
    for (n, b) in tree.nodes.iter().zip(&bounds) {
        let [x, y, z] = n.center;
        BoxRecord { center: [x, y, -z], radius: n.radius, error: n.error, parent: n.parent, first: n.start, count: n.count, bounds: *b }.write_to(&mut bytes);
    }
    for r in records {
        bytes.extend_from_slice(&r);
    }
    bytes
}

/// A tree back from [`write`]'s bytes (what a world loads: checks of what a shared file holds), the boxes' depths and
/// leaves worked out from their parents. A version 1 file, a file of another length than its counts give, a box out of
/// order, ranges that do not follow each other, non-finite bounds or a non-finite scale are errors.
pub fn parse(bytes: &[u8]) -> anyhow::Result<Tree> {
    let header = Header::read(bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
    let (count, splats) = (header.boxes as usize, header.splats as usize);
    let mut nodes: Vec<Node> = Vec::with_capacity(count);
    let mut bounds = Vec::with_capacity(count);
    let mut next = 0usize;
    for i in 0..count {
        let r = BoxRecord::read(bytes, i);
        anyhow::ensure!(r.parent.is_none_or(|p| (p as usize) < i) && (i == 0) == r.parent.is_none(), "{}", tr("a box's parent comes after it"));
        anyhow::ensure!(r.first as usize == next && r.count >= 1, "{}", tr("a splat tree's boxes do not cover its splats in order"));
        next += r.count as usize;
        anyhow::ensure!(next <= splats, "{}", tr("a box's splats run past the file's"));
        anyhow::ensure!(
            r.bounds.min.iter().chain(&r.bounds.step).all(|v| v.is_finite()) && r.bounds.step.iter().all(|&s| s >= 0.0),
            "{}",
            tr("a box's bounds are not finite")
        );
        let depth = r.parent.map_or(0, |p| nodes[p as usize].depth + 1);
        let [x, y, z] = r.center;
        nodes.push(Node { center: [x, y, -z], radius: r.radius, error: r.error, parent: r.parent, depth, start: r.first, count: r.count, leaf: true });
        if let Some(p) = r.parent {
            nodes[p as usize].leaf = false;
        }
        bounds.push(r.bounds);
    }
    anyhow::ensure!(next == splats, "{}", tr("a splat tree's boxes do not cover its splats"));
    let first = st::splat_offset(count, 0);
    let decoded: Vec<Vec<Splat>> = nodes
        .par_iter()
        .zip(&bounds)
        .map(|(n, b)| {
            (n.start as usize..(n.start + n.count) as usize)
                .map(|i| st::decode(&bytes[first + i * st::SPLAT_BYTES..first + (i + 1) * st::SPLAT_BYTES], b))
                .collect()
        })
        .collect();
    let mut g = Gaussians::default();
    for s in decoded.iter().flatten() {
        anyhow::ensure!(s.log_scale.iter().all(|v| v.is_finite()), "{}", tr("a splat's scale is not finite"));
        let (mean, log_scale, rotation, opacity_logit, color) = unity(s);
        g.push(mean, log_scale, rotation, opacity_logit, color);
    }
    Ok(Tree { nodes, splats: g })
}

impl Tree {
    /// The splats of the boxes `pick` keeps, in box order, as the file stores them ([`round_box`] per box): what a world
    /// draws of them.
    pub fn select_stored(&self, pick: impl Fn(&Node) -> bool) -> Gaussians {
        let picked: Vec<&Node> = self.nodes.iter().filter(|n| pick(n)).collect();
        let mut g = self.select(|n| pick(n));
        let mut at = 0usize;
        for n in picked {
            round_box(&mut g, at..at + n.count as usize);
            at += n.count as usize;
        }
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{build, leaves};

    /// A floor of `n` small splats, a few big ones above it, turned every way.
    fn scene(n: usize) -> Gaussians {
        let mut g = Gaussians::default();
        let mut rng = fastrand::Rng::with_seed(3);
        for _ in 0..n {
            let q = [rng.f32() - 0.5, rng.f32() - 0.5, rng.f32() - 0.5, rng.f32() - 0.5];
            g.push(
                [rng.f32() * 20.0 - 10.0, rng.f32() * 0.1, rng.f32() * 20.0 - 10.0],
                [-3.0 - rng.f32() * 4.0, -6.0, -3.5],
                q,
                rng.f32() * 6.0 - 1.0,
                [rng.f32() * 1.2 - 0.1, 0.5, 0.2],
            );
        }
        for k in 0..8 {
            g.push([k as f32, 3.0, 0.0], [-1.0; 3], [0.9, 0.1, 0.3, 0.2], 0.0, [1.0, 0.0, 0.0]);
        }
        g
    }

    fn unit(q: [f32; 4]) -> [f32; 4] {
        let n = q.iter().map(|v| v * v).sum::<f32>().sqrt();
        q.map(|v| v / n)
    }

    fn angle(a: [f32; 4], b: [f32; 4]) -> f32 {
        let (a, b) = (unit(a), unit(b));
        2.0 * (0..4).map(|k| a[k] * b[k]).sum::<f32>().abs().min(1.0).acos()
    }

    #[test]
    fn the_file_is_a_header_the_boxes_and_the_splats() {
        let g = scene(5000);
        let tree = build(&g, &leaves(&g, 3000));
        assert_eq!(tree.leaf_splats().len(), 3000, "the most important ones");
        let bytes = tree.file(0xABCD, 0);
        assert_eq!(bytes.len(), 32 + 64 * tree.nodes.len() + 20 * tree.len());
        let u = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        let f = |o: usize| f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        assert_eq!(&bytes[..8], crate::tree::MAGIC);
        assert_eq!((u(8), u(12) as usize, u(16) as usize, u(20), u(24), u(28)), (2, tree.nodes.len(), tree.len(), 0xABCD, 0, 0));
        // The root: its sphere right-handed, no parent, the first splats; its bounds hold its splats.
        let b = 32;
        assert_eq!((u(b + 20), u(b + 24), u(b + 28)), (u32::MAX, 0, tree.nodes[0].count));
        assert_eq!(f(b + 8), -tree.nodes[0].center[2]);
        let root = 0..tree.nodes[0].count as usize;
        let lowest_z = root.clone().map(|i| -tree.splats.means[i][2]).fold(f32::INFINITY, f32::min);
        assert_eq!(f(b + 40), lowest_z, "min z of the root's own splats, right-handed");
        assert_eq!(&bytes[b + 56..b + 64], &[0; 8]);
        let first = 32 + 64 * tree.nodes.len();
        let r = &bytes[first..first + 20];
        let bounds = BoxRecord::read(&bytes, 0).bounds;
        let back = st::decode(r, &bounds);
        assert!((back.position[2] + tree.splats.means[0][2]).abs() <= bounds.step[2] * 0.5 + 1e-5);
        assert_eq!(back.rgba[3], byte(sigmoid(tree.splats.opacity_logits[0])));
        assert_eq!(key(&bytes).unwrap(), 0xABCD);
        assert_eq!(tree.top(1000).file(1, crate::tree::FLAG_COARSE)[24], 1, "a coarse file says so");
    }

    /// The round trip loses no more than the records' steps: every position within half a step of its own box's bounds
    /// (and inside them), every scale within half a half float's step of its log, every rotation within 6.3 mrad,
    /// colour and opacity as bytes; the boxes come back exactly.
    #[test]
    fn a_file_reads_back_as_its_tree_within_the_records_steps() {
        let g = scene(9000);
        let tree = build(&g, &leaves(&g, usize::MAX));
        let bytes = tree.file(5, 0);
        let back = parse(&bytes).unwrap();
        assert_eq!(back.nodes, tree.nodes);
        assert_eq!(back.len(), tree.len());
        let (s, t) = (&back.splats, &tree.splats);
        for (b, n) in tree.nodes.iter().enumerate() {
            let bounds = BoxRecord::read(&bytes, b).bounds;
            for i in n.start as usize..(n.start + n.count) as usize {
                let right = [s.means[i][0], s.means[i][1], -s.means[i][2]];
                let original = [t.means[i][0], t.means[i][1], -t.means[i][2]];
                for k in 0..3 {
                    assert!((right[k] - original[k]).abs() <= bounds.step[k] * 0.5 + original[k].abs() * 1e-6 + 1e-6, "box {b} splat {i} axis {k}");
                    assert!(right[k] >= bounds.min[k] && right[k] <= bounds.min[k] + bounds.step[k] * 65535.0 * (1.0 + 1e-6) + 1e-6);
                    // A half float's relative step below 2^-10: half of it on the log.
                    assert!((s.log_scales[i][k] - t.log_scales[i][k]).abs() <= t.log_scales[i][k].abs() * 2f32.powi(-11) + 1e-7);
                }
                assert!(angle(s.rotations[i], t.rotations[i]) < 0.0063, "{:?} vs {:?}", s.rotations[i], t.rotations[i]);
                assert!((sigmoid(s.opacity_logits[i]) - sigmoid(t.opacity_logits[i]).min(1.0)).abs() <= 0.5 / 255.0 + 1e-5);
                assert!(s.colors[i].iter().zip(t.colors[i]).all(|(a, b)| (a - b.clamp(0.0, 1.0)).abs() <= 0.5 / 255.0 + 1e-6));
            }
        }
        // Leaf boxes are a few metres at most here: their steps are tens of micrometres.
        let leaf_step = tree
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.leaf)
            .map(|(b, _)| BoxRecord::read(&bytes, b).bounds.step.into_iter().fold(0.0, f32::max))
            .fold(0.0, f32::max);
        assert!(leaf_step < 1e-4, "{leaf_step} m");
        // What a stored selection draws is exactly what the file holds.
        let stored = tree.select_stored(|n| n.leaf);
        assert!(stored == back.leaf_splats(), "select_stored rounds as the file does");
    }

    #[test]
    fn a_damaged_or_older_file_is_refused() {
        let g = scene(3000);
        let tree = build(&g, &leaves(&g, usize::MAX));
        let bytes = tree.file(5, 0);
        assert!(parse(&bytes).is_ok());
        assert!(parse(&bytes[..bytes.len() - 20]).is_err(), "cut short");
        let mut longer = bytes.clone();
        longer.extend_from_slice(&[0; 20]);
        assert!(parse(&longer).is_err(), "longer than its counts");
        let mut v1 = bytes.clone();
        v1[8..12].copy_from_slice(&1u32.to_le_bytes());
        assert!(parse(&v1).unwrap_err().to_string().contains("version 1"), "no reader of version 1");
        assert!(key(&v1).is_err());
        let box1 = 32 + 64;
        let mut order = bytes.clone();
        order[box1 + 24..box1 + 28].copy_from_slice(&(tree.nodes[1].start + 1).to_le_bytes());
        assert!(parse(&order).is_err(), "ranges follow each other");
        let mut parent = bytes.clone();
        parent[box1 + 20..box1 + 24].copy_from_slice(&1u32.to_le_bytes());
        assert!(parse(&parent).is_err(), "a parent before its child");
        let mut nan = bytes.clone();
        nan[box1 + 44..box1 + 48].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(parse(&nan).is_err(), "bounds are finite");
        let mut scale = bytes.clone();
        let first = 32 + 64 * tree.nodes.len();
        scale[first + 6..first + 8].copy_from_slice(&0x7c00u16.to_le_bytes());
        assert!(parse(&scale).is_err(), "an infinite scale");
    }

    #[test]
    fn rounding_attributes_leaves_positions_and_matches_the_records() {
        let g = scene(500);
        let mut rounded = g.clone();
        round_attributes(&mut rounded);
        assert_eq!(rounded.means, g.means);
        let tree = Tree {
            nodes: vec![Node { center: [0.0; 3], radius: 100.0, error: 0.0, parent: None, depth: 0, start: 0, count: g.len() as u32, leaf: true }],
            splats: g.clone(),
        };
        let back = parse(&tree.file(1, 0)).unwrap().splats;
        assert_eq!(rounded.log_scales, back.log_scales);
        assert_eq!(rounded.rotations, back.rotations);
        assert_eq!(rounded.colors, back.colors);
        assert_eq!(rounded.opacity_logits, back.opacity_logits);
        // Rounding a box twice changes nothing more.
        let mut once = g.clone();
        round_box(&mut once, 0..g.len());
        let mut twice = once.clone();
        round_box(&mut twice, 0..g.len());
        assert!(twice.log_scales == once.log_scales && twice.colors == once.colors);
        for i in 0..g.len() {
            assert!((0..3).all(|k| (twice.means[i][k] - once.means[i][k]).abs() <= 1e-6 * (1.0 + once.means[i][k].abs())));
            assert!(angle(twice.rotations[i], once.rotations[i]) < 0.0063);
        }
    }
}
