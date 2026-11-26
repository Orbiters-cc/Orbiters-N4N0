//! Floaters: the haze a capture's training leaves in empty air, which a capture shown small (a world's side panel's
//! hole, a hand, a sheet on the desktop) shows as clouds over its rooms and fringes under its floors.
//!
//! A floater is told by what it is, never by where it is. Two kinds of splat may be one: a faint one that is large
//! against the capture's usual splat, and a blob (round, not the flat disc a surface trains to) larger than usual. Either
//! goes only when it lies in space that is nearly empty around it at its own size. What is real is a surface, and a
//! surface, however dark or blurred, is opaque: around any of its splats, at that splat's own size, the others cover a
//! plane through it. Haze can be seen through. So the test is how much of a disc through a splat's middle, as wide as
//! the splat, the others around it cover, each by its opacity times its footprint (no more than the disc); and a cloud
//! of blobs cannot vouch for itself, as only splats that are no blobs count. A wall, a roof, a floor, a figure or a
//! tree keeps every splat, whatever its height, its colour or how faint the splat itself is. A needle (a line, such as
//! a railing or a wire, whose splats are long and thin) is never large here, as its size is that of its two longest
//! axes; nor is a small splat, nor one nearly opaque, alone or not.
//!
//! Counted on cells of a grid sized to each splat (a pyramid of Morton-ordered cells, each holding what the splats in it
//! cover): one sort of the splats, then a few binary searches per candidate. Deterministic, with no sampling of what is
//! counted.

use orbiters_core::locks::*;
use orbiters_reconstruct::gaussians::Gaussians;
use std::f32::consts::PI;

/// What makes a floater. [`Rule::default`] is what the small view uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rule {
    /// Under this opacity a splat is faint...
    pub faint: f32,
    /// ...and a faint one may be haze when its size (√(σ₁σ₂) of its two longest axes) is over this many times the
    /// capture's usual size (the median).
    pub large: f32,
    /// A blob: its shortest axis at least this share of its longest...
    pub round: f32,
    /// ...and its size over this many times the usual.
    pub blob: f32,
    /// A splat at least this opaque is never haze, blob or not.
    pub solid: f32,
    /// How far around its middle is looked at, in its longest sigmas.
    pub around: f32,
    /// What the others there cover of a disc that wide through its middle, under which it is nearly empty.
    pub empty: f32,
}

impl Default for Rule {
    fn default() -> Self {
        Self { faint: 0.35, large: 3.0, round: 0.25, blob: 2.0, solid: 0.9, around: 1.0, empty: 0.35 }
    }
}

/// Which of `g`'s splats are floaters, by [`Rule::default`].
pub fn floating(g: &Gaussians) -> Vec<bool> {
    floating_by(g, &Rule::default())
}

/// Bits per axis of a Morton key.
const BITS: u32 = 21;
const MAX_CELL: u32 = (1 << BITS) - 1;

fn spread(v: u32) -> u64 {
    let mut x = v as u64 & 0x1f_ffff;
    x = (x | x << 32) & 0x1f00000000ffff;
    x = (x | x << 16) & 0x1f0000ff0000ff;
    x = (x | x << 8) & 0x100f00f00f00f00f;
    x = (x | x << 4) & 0x10c30c30c30c30c3;
    x = (x | x << 2) & 0x1249249249249249;
    x
}

fn morton(c: [u32; 3]) -> u64 {
    spread(c[0]) | spread(c[1]) << 1 | spread(c[2]) << 2
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// A cell of one level: its key, the opacity of the splats in it wider than the widest disc looked at with cells of
/// this level (each covers that disc whole), and the opacity-weighted footprint of the narrower ones.
#[derive(Clone, Copy)]
struct Cell {
    key: u64,
    whole: f32,
    footprint: f32,
}

/// The widest disc looked at with cells `cell` wide (its radius twice that), whose area caps a splat's footprint.
fn cap(cell: f32) -> f32 {
    PI * 4.0 * cell * cell
}

/// Which of `g`'s splats are floaters by `rule`.
pub fn floating_by(g: &Gaussians, rule: &Rule) -> Vec<bool> {
    let n = g.len();
    let mut out = vec![false; n];
    let finite: Vec<u32> = (0..n).filter(|&i| g.is_finite(i)).map(|i| i as u32).collect();
    if finite.len() < 64 {
        return out;
    }
    // Each splat's opacity, longest sigma, size (√(σ₁σ₂) of its two longest), how round it is (its shortest over its
    // longest), and footprint: 2π σ₁σ₂, what a disc of it covers at its full opacity.
    let shape: Vec<Shape> = in_parallel(n, |i| {
        let mut s = g.log_scales[i].map(f32::exp);
        s.sort_by(f32::total_cmp);
        Shape {
            alpha: sigmoid(g.opacity_logits[i]),
            sigma: s[2],
            size: (s[1] * s[2]).sqrt(),
            round: s[0] / s[2].max(f32::MIN_POSITIVE),
            footprint: std::f32::consts::TAU * s[1] * s[2],
        }
    });
    let alpha: Vec<f32> = shape.iter().map(|s| s.alpha).collect();
    let sigma: Vec<f32> = shape.iter().map(|s| s.sigma).collect();
    let size: Vec<f32> = shape.iter().map(|s| s.size).collect();
    let round: Vec<f32> = shape.iter().map(|s| s.round).collect();
    let footprint: Vec<f32> = shape.iter().map(|s| s.footprint).collect();
    drop(shape);
    let usual = {
        let step = (finite.len() / 100_000).max(1);
        let mut sample: Vec<f32> = finite.iter().step_by(step).map(|&i| size[i as usize]).collect();
        let middle = sample.len() / 2;
        *sample.select_nth_unstable_by(middle, f32::total_cmp).1
    };
    // Blobs, which do not vouch for what is around them.
    let blob: Vec<bool> = (0..n).map(|i| round[i] >= rule.round && size[i] > rule.blob * usual).collect();
    let candidates: Vec<u32> = finite
        .iter()
        .copied()
        .filter(|&i| {
            let i = i as usize;
            alpha[i] < rule.solid && (blob[i] || (alpha[i] < rule.faint && size[i] > rule.large * usual))
        })
        .collect();
    if candidates.is_empty() {
        return out;
    }
    // The finest cells: half the narrowest candidate's radius (its longest sigma is at least its size). A candidate
    // looks at the cells at least half its radius wide.
    let base = (rule.around * rule.large.min(rule.blob) * usual / 2.0).max(1e-4);
    let mut low = [f32::MAX; 3];
    for &i in &finite {
        for a in 0..3 {
            low[a] = low[a].min(g.means[i as usize][a]);
        }
    }
    let at = |p: [f32; 3]| [0, 1, 2].map(|a| ((p[a] - low[a]) / base).clamp(0.0, MAX_CELL as f32) as u32);
    let level_of = |i: usize| level_for(sigma[i] * rule.around / 2.0, base);
    let levels = (candidates.iter().map(|&i| level_of(i as usize)).max().unwrap_or(0) + 1).min(BITS);
    // The level from which a splat no longer covers the widest disc whole.
    let narrow_from = |area: f32| (0..levels).find(|&k| cap(base * (1u64 << k) as f32) > area).unwrap_or(levels);

    // Level 0: every finite splat that is no blob in its cell, in Morton order.
    let counted: Vec<u32> = finite.iter().copied().filter(|&i| !blob[i as usize]).collect();
    let mut keyed: Vec<(u64, u32)> = in_parallel(counted.len(), |k| (morton(at(g.means[counted[k] as usize])), counted[k]));
    drop(counted);
    keyed.sort_unstable();
    let mut narrowing: Vec<Vec<(u64, f32, f32)>> = vec![Vec::new(); levels as usize];
    let mut level: Vec<Cell> = Vec::with_capacity(keyed.len() / 2);
    for &(key, i) in &keyed {
        let i = i as usize;
        let from = narrow_from(footprint[i]);
        let (whole, part) = if from == 0 { (0.0, alpha[i] * footprint[i]) } else { (alpha[i], 0.0) };
        if from > 0 && from < levels {
            narrowing[from as usize].push((key >> (3 * from), alpha[i] * footprint[i], alpha[i]));
        }
        match level.last_mut() {
            Some(c) if c.key == key => {
                c.whole += whole;
                c.footprint += part;
            }
            _ => level.push(Cell { key, whole, footprint: part }),
        }
    }
    let mut pyramid = vec![level];
    for k in 1..levels as usize {
        let mut next: Vec<Cell> = Vec::with_capacity(pyramid[k - 1].len() / 4 + 1);
        for c in &pyramid[k - 1] {
            let key = c.key >> 3;
            match next.last_mut() {
                Some(n) if n.key == key => {
                    n.whole += c.whole;
                    n.footprint += c.footprint;
                }
                _ => next.push(Cell { key, ..*c }),
            }
        }
        // Splats that no longer cover the widest disc here count by their footprint.
        for &(key, part, a) in &narrowing[k] {
            if let Ok(at) = next.binary_search_by_key(&key, |c| c.key) {
                next[at].whole -= a;
                next[at].footprint += part;
            }
        }
        pyramid.push(next);
    }

    // The cells around one, nearest first.
    let mut offsets: Vec<[i32; 3]> = (0..125).map(|i| [i % 5 - 2, i / 5 % 5 - 2, i / 25 - 2]).collect();
    offsets.sort_by_key(|d| d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
    let judge = |i: usize| -> bool {
        let k = level_of(i).min(levels - 1);
        let width = base * (1u64 << k) as f32;
        let radius = sigma[i] * rule.around;
        let disc = PI * radius * radius;
        let cells = &pyramid[k as usize];
        let p = g.means[i];
        let home = at(p).map(|v| (v >> k) as i32);
        // Less its own share, when it counts.
        let own = if blob[i] { 0.0 } else { alpha[i] * footprint[i].min(disc) };
        let enough = rule.empty * disc + own;
        let mut covered = 0.0f32;
        for d in &offsets {
            let c = [home[0] + d[0], home[1] + d[1], home[2] + d[2]];
            if c.iter().any(|v| *v < 0 || *v > (MAX_CELL >> k) as i32) {
                continue;
            }
            // A cell counts when its middle is within the radius.
            let middle = [0, 1, 2].map(|a| low[a] + (c[a] as f32 + 0.5) * width);
            let far = (0..3).map(|a| (middle[a] - p[a]) * (middle[a] - p[a])).sum::<f32>();
            if far > radius * radius {
                continue;
            }
            if let Ok(found) = cells.binary_search_by_key(&morton(c.map(|v| v as u32)), |c| c.key) {
                covered += cells[found].whole * disc + cells[found].footprint;
                if covered >= enough {
                    return false;
                }
            }
        }
        true
    };
    for (k, floats) in in_parallel(candidates.len(), |k| judge(candidates[k] as usize)).into_iter().enumerate() {
        out[candidates[k] as usize] = floats;
    }
    out
}

#[derive(Clone, Copy, Default)]
struct Shape {
    alpha: f32,
    sigma: f32,
    size: f32,
    round: f32,
    footprint: f32,
}

/// `f` of 0..`n` in order, on a few threads that take blocks of it as they are free (what a block costs varies: the
/// farthest splats come last and are the loneliest).
fn in_parallel<T: Send + Default + Clone>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    const BLOCK: usize = 4096;
    let threads = std::thread::available_parallelism().map_or(1, |t| t.get()).min(8);
    if n < BLOCK * 2 || threads == 1 {
        return (0..n).map(f).collect();
    }
    let mut out = vec![T::default(); n];
    let next = std::sync::atomic::AtomicUsize::new(0);
    let blocks: Vec<std::sync::Mutex<&mut [T]>> = out.chunks_mut(BLOCK).map(std::sync::Mutex::new).collect();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let b = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(block) = blocks.get(b) else { break };
                let mut block = block.locked();
                for (k, v) in block.iter_mut().enumerate() {
                    *v = f(b * BLOCK + k);
                }
            });
        }
    });
    drop(blocks);
    out
}

/// The level whose cells are at least `width` wide.
fn level_for(width: f32, base: f32) -> u32 {
    if width <= base {
        0
    } else {
        (width / base).log2().ceil() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(g: &mut Gaussians, at: [f32; 3], sigma: [f32; 3], alpha: f32) {
        let logit = (alpha / (1.0 - alpha)).ln();
        g.push(at, sigma.map(f32::ln), [1.0, 0.0, 0.0, 0.0], logit, [0.3, 0.2, 0.1]);
    }

    /// A room: a floor, four walls and a roof, dense sheets of small splats, some of them faint and dark, a railing of
    /// long thin faint splats, a lamp; a few large faint blobs alone in the air inside, above and under it, and a dark
    /// cloud of fairly opaque blobs hanging under the roof (the cloud the judges saw over ちま部屋). The blobs and the
    /// cloud go; every splat of the room stays.
    #[test]
    fn haze_goes_and_every_wall_and_the_roof_stay() {
        let mut g = Gaussians::default();
        let mut room = 0;
        let mut sheet = |g: &mut Gaussians, point: &dyn Fn(f32, f32) -> [f32; 3], flat: usize| {
            for i in 0..120 {
                for j in 0..120 {
                    let (u, v) = (i as f32 / 120.0, j as f32 / 120.0);
                    let mut s = [0.03, 0.03, 0.03];
                    s[flat] = 0.002;
                    // Every fifth faint, as dark parts of a real surface are.
                    let alpha = if (i + j) % 5 == 0 { 0.08 } else { 0.7 };
                    push(g, point(u, v), s, alpha);
                    room += 1;
                }
            }
        };
        // Floor at 0, roof at 2.7 (and a roof's top at 2.9), walls at x, z = ±3.
        sheet(&mut g, &|u, v| [u * 6.0 - 3.0, 0.0, v * 6.0 - 3.0], 1);
        sheet(&mut g, &|u, v| [u * 6.0 - 3.0, 2.7, v * 6.0 - 3.0], 1);
        sheet(&mut g, &|u, v| [u * 6.0 - 3.0, 2.9, v * 6.0 - 3.0], 1);
        sheet(&mut g, &|u, v| [-3.0, v * 2.7, u * 6.0 - 3.0], 0);
        sheet(&mut g, &|u, v| [3.0, v * 2.7, u * 6.0 - 3.0], 0);
        sheet(&mut g, &|u, v| [u * 6.0 - 3.0, v * 2.7, -3.0], 2);
        sheet(&mut g, &|u, v| [u * 6.0 - 3.0, v * 2.7, 3.0], 2);
        // Large, dim, flat splats lying on the roof (a blurred part of it): they are the roof, and stay.
        for i in 0..6 {
            push(&mut g, [-2.0 + i as f32 * 0.8, 2.7, 0.5], [0.4, 0.01, 0.4], 0.15);
            room += 1;
        }
        // A small opaque fragment alone in the air, and a lamp's globe, round and nearly opaque: not haze.
        push(&mut g, [0.5, 1.5, -0.5], [0.02; 3], 0.9);
        push(&mut g, [-2.0, 1.9, -2.0], [0.1; 3], 0.95);
        room += 2;
        // A railing across the room: long thin faint splats along a line, which covers no plane.
        for i in 0..50 {
            push(&mut g, [-2.5 + i as f32 * 0.1, 1.0, 2.0], [0.15, 0.008, 0.008], 0.2);
            room += 1;
        }
        let blobs: Vec<[f32; 3]> =
            vec![[0.0, 2.2, 0.0], [1.0, 1.4, 1.0], [-1.5, 0.8, 1.2], [0.4, 4.0, -0.3], [2.0, 3.6, 1.0], [-1.0, 2.25, -1.6], [0.5, -0.6, 2.0]];
        for b in &blobs {
            push(&mut g, *b, [0.3, 0.25, 0.28], 0.12);
        }
        // The cloud: dark blobs, half opaque, close together a little under the roof.
        let mut cloud = Vec::new();
        for i in 0..4 {
            for j in 0..3 {
                let at = [1.0 + i as f32 * 0.15, 2.2 + (i + j) as f32 % 3.0 * 0.08, -1.5 + j as f32 * 0.15];
                push(&mut g, at, [0.12, 0.09, 0.1], 0.5);
                cloud.push(at);
            }
        }
        let floating = floating(&g);
        assert!(floating[..room].iter().all(|f| !f), "{} of the room's splats were taken", floating[..room].iter().filter(|f| **f).count());
        for (k, b) in blobs.iter().chain(&cloud).enumerate() {
            assert!(floating[room + k], "the blob at {b:?} stays");
        }
    }

    /// A surface made only of large faint splats (a ceiling the capture saw blurred) is still a surface: it covers itself.
    #[test]
    fn a_blurred_surface_of_large_faint_splats_stays() {
        let mut g = Gaussians::default();
        for i in 0..40 {
            for j in 0..40 {
                push(&mut g, [i as f32 * 0.15, 2.7, j as f32 * 0.15], [0.3, 0.01, 0.3], 0.2);
            }
        }
        // Small ones elsewhere, which set what usual is.
        for i in 0..20000 {
            push(&mut g, [10.0 + (i % 141) as f32 * 0.02, 0.0, (i / 141) as f32 * 0.02], [0.02, 0.002, 0.02], 0.8);
        }
        let floating = floating(&g);
        assert_eq!(floating.iter().filter(|f| **f).count(), 0);
    }

    #[test]
    fn morton_keys_keep_the_order_of_coarser_cells() {
        let a = morton([5, 9, 2]);
        assert_eq!(a >> 3, morton([2, 4, 1]));
        assert!(morton([0, 0, 0]) < morton([1, 0, 0]));
        assert_eq!(morton([MAX_CELL; 3]), (1u64 << 63) - 1);
    }
}
