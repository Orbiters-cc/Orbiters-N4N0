//! Marching cubes over the sparse field, as a manifold surface.
//!
//! No 256-case table: each cube's surface is traced from its faces. On a face with two sign changes the two crossings
//! join; on a face with four (an ambiguous face) the asymptotic decider joins the crossings around the corners whose
//! sign differs from the bilinear saddle's. Both cubes sharing a face see its four values and decide alike, so the
//! contours of neighbouring cubes meet edge for edge, and each crossing (a vertex, identified by its lattice edge) is
//! used by exactly two faces of a cube: every closed contour is one polygon. Polygons are fanned from a corner whose
//! diagonals never join two crossings of one face (such a diagonal could also be made by the neighbouring cube: an edge
//! of four triangles); if no corner qualifies, from a new centre vertex. Each polygon faces free space (the field's
//! gradient). A cube is skipped when a corner was never observed or a crossing joins values more than one apart
//! (a jump from the free side of the band to the occluded side is a discontinuity, not a surface).

use super::{
    tsdf::{local, Volume, N, SIDE},
    Mesh,
};
use rayon::prelude::*;

const CORNERS: [[i32; 3]; 8] = [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0], [0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]];
const EDGES: [[usize; 2]; 12] = [[0, 1], [1, 2], [2, 3], [3, 0], [4, 5], [5, 6], [6, 7], [7, 4], [0, 4], [1, 5], [2, 6], [3, 7]];
/// Each face's edges in cyclic order; edge `k` of a face joins its corners `k` and `k + 1` ([`FACE_CORNERS`]).
const FACES: [[usize; 4]; 6] = [[0, 1, 2, 3], [4, 5, 6, 7], [0, 9, 4, 8], [2, 10, 6, 11], [3, 11, 7, 8], [1, 10, 5, 9]];
const FACE_CORNERS: [[usize; 4]; 6] = [[0, 1, 2, 3], [4, 5, 6, 7], [0, 1, 5, 4], [3, 2, 6, 7], [0, 3, 7, 4], [1, 2, 6, 5]];
/// Largest value step across a crossing edge (values are distances over the truncation band, −1..1).
const MAX_JUMP: f32 = 1.0;
const OFF: i64 = 1 << 19;

/// The id of the lattice edge from voxel `g` along `axis` (20 bits a coordinate: ±21 km at 4 cm).
fn edge_id(g: [i32; 3], axis: usize) -> u64 {
    let c = g.map(|v| ((v as i64 + OFF) as u64) & 0xf_ffff);
    (c[0] << 42) | (c[1] << 22) | (c[2] << 2) | axis as u64
}

/// Faces each cube edge lies on, as a bit mask.
fn edge_faces() -> [u8; 12] {
    let mut m = [0u8; 12];
    for (f, face) in FACES.iter().enumerate() {
        for &e in face {
            m[e] |= 1 << f;
        }
    }
    m
}

/// One block's cubes: every vertex a polygon emitted, in emission order (a crossing shared by several polygons is
/// emitted once by each), and the triangles as indices into them.
struct Part {
    ids: Vec<u64>,
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    skipped_jumps: usize,
}

/// Polygonizes one cube: `vals` at [`CORNERS`], `origin` its first voxel.
///
/// Fixed-size arrays rather than vectors for the face crossings, the ring and its vertices (a ring has at most twelve
/// edges): the first version allocated nine small vectors for every surface cube, a measurable share of extraction.
/// The arithmetic is unchanged, operation for operation, so the vertices come out bit for bit as before.
fn cube(vals: [f32; 8], origin: [i32; 3], voxel: f32, faces_of: &[u8; 12], out: &mut Part) {
    const NONE: usize = usize::MAX;
    let inside = vals.map(|v| v < 0.0);
    let active: [bool; 12] = EDGES.map(|[a, b]| inside[a] != inside[b]);
    if !active.iter().any(|&a| a) {
        return;
    }
    if EDGES.iter().zip(active).any(|([a, b], on)| on && (vals[*a] - vals[*b]).abs() > MAX_JUMP) {
        out.skipped_jumps += 1;
        return;
    }
    let mut adj = [[NONE; 2]; 12];
    let mut link = |a: usize, b: usize| {
        let slot = |s: &mut [usize; 2], v: usize| {
            if s[0] == NONE {
                s[0] = v
            } else {
                s[1] = v
            }
        };
        slot(&mut adj[a], b);
        slot(&mut adj[b], a);
    };
    for (f, face) in FACES.iter().enumerate() {
        let mut on = [0usize; 4];
        let mut n = 0;
        for k in 0..4 {
            if active[face[k]] {
                on[n] = k;
                n += 1;
            }
        }
        match n {
            2 => link(face[on[0]], face[on[1]]),
            4 => {
                let c = FACE_CORNERS[f].map(|k| vals[k]);
                let den = c[0] + c[2] - c[1] - c[3];
                let saddle = if den.abs() > 1e-12 { (c[0] * c[2] - c[1] * c[3]) / den } else { 0.0 };
                if (saddle < 0.0) == inside[FACE_CORNERS[f][0]] {
                    // Corners 0 and 2 meet through the face: cut corners 1 (edges 0, 1) and 3 (edges 2, 3) off.
                    link(face[0], face[1]);
                    link(face[2], face[3]);
                } else {
                    link(face[3], face[0]);
                    link(face[1], face[2]);
                }
            }
            _ => {}
        }
    }
    let position = |e: usize| -> (u64, [f32; 3]) {
        let [a, b] = EDGES[e];
        let (ga, gb) = (CORNERS[a], CORNERS[b]);
        let axis = (0..3).find(|&i| ga[i] != gb[i]).unwrap();
        let lo = if ga[axis] < gb[axis] { ga } else { gb };
        let t = (vals[a] / (vals[a] - vals[b])).clamp(1e-3, 1.0 - 1e-3);
        let p = std::array::from_fn(|i| (origin[i] as f32 + ga[i] as f32 + t * (gb[i] - ga[i]) as f32) * voxel);
        (edge_id([origin[0] + lo[0], origin[1] + lo[1], origin[2] + lo[2]], axis), p)
    };
    let grad = [
        vals[1] + vals[2] + vals[5] + vals[6] - vals[0] - vals[3] - vals[4] - vals[7],
        vals[2] + vals[3] + vals[6] + vals[7] - vals[0] - vals[1] - vals[4] - vals[5],
        vals[4] + vals[5] + vals[6] + vals[7] - vals[0] - vals[1] - vals[2] - vals[3],
    ];
    let mut used = [false; 12];
    let mut ring_index = 0u64;
    for start in 0..12 {
        if !active[start] || used[start] || adj[start][1] == NONE {
            continue;
        }
        let mut ring = [0usize; 12];
        let mut m = 0;
        let mut e = start;
        let mut prev = NONE;
        while !used[e] {
            used[e] = true;
            ring[m] = e;
            m += 1;
            let next = if adj[e][0] != prev { adj[e][0] } else { adj[e][1] };
            prev = e;
            e = next;
        }
        if m < 3 {
            continue;
        }
        let mut verts = [(0u64, [0.0f32; 3]); 12];
        for k in 0..m {
            verts[k] = position(ring[k]);
        }
        let verts = &verts[..m];
        // Newell's normal of the polygon against the gradient (towards free space).
        let mut n = [0.0f32; 3];
        for k in 0..m {
            let (a, b) = (verts[k].1, verts[(k + 1) % m].1);
            n[0] += (a[1] - b[1]) * (a[2] + b[2]);
            n[1] += (a[2] - b[2]) * (a[0] + b[0]);
            n[2] += (a[0] - b[0]) * (a[1] + b[1]);
        }
        let flip = n[0] * grad[0] + n[1] * grad[1] + n[2] * grad[2] < 0.0;
        let order = |k: usize| if flip { m - 1 - k } else { k };
        // This polygon's vertices are emitted at `base..base + m` (its centre, if any, at `base + m`).
        let base = out.ids.len() as u32;
        for &(id, p) in verts {
            out.ids.push(id);
            out.positions.push(p);
        }
        let at = |k: usize| base + order(k) as u32;
        let safe = |s: usize| (2..m - 1).all(|k| faces_of[ring[order(s)]] & faces_of[ring[order((s + k) % m)]] == 0);
        if let Some(s) = (0..m).find(|&s| safe(s)) {
            for k in 1..m - 1 {
                out.triangles.push([at(s), at((s + k) % m), at((s + k + 1) % m)]);
            }
        } else {
            // A centre's id: the cube's first lattice edge id with the top bit set and the ring (at most four) as axis.
            let c = (1 << 63) | edge_id(origin, ring_index as usize & 3);
            let centre = std::array::from_fn(|i| verts.iter().map(|v| v.1[i]).sum::<f32>() / m as f32);
            out.ids.push(c);
            out.positions.push(centre);
            let centre = base + m as u32;
            for k in 0..m {
                out.triangles.push([centre, at(k), at((k + 1) % m)]);
            }
        }
        ring_index += 1;
    }
}

/// Corner `k` of the cube at dense index `d` of a block's 9³ gather is at `d + GATHER[k]` ([`CORNERS`] in x, y, z
/// order with strides 1, 9, 81).
const GATHER: [usize; 8] = [0, 1, 1 + 9, 9, 81, 1 + 81, 1 + 9 + 81, 9 + 81];

/// Extracts the surface of every observed cube of the volume.
///
/// Blocks are polygonized in parallel, each into its own [`Part`]; the parts, in block order, then give the mesh:
/// every lattice edge's crossing becomes one vertex, at the position and in the order of its first emission, and
/// triangles keep their order. How that is computed was measured (lab report `mc-fast.md`, 2026-11-03):
/// - Each block first gathers the 9³ voxels its 8³ cubes read (its own and its +x/+y/+z neighbours' first layers)
///   once, instead of looking each cube's 8 corners up through the neighbour table (4 096 lookups a block, 729 now).
/// - The first version merged the crossings on one thread through a std `HashMap` (SipHash) keyed by edge id, then
///   looked every triangle corner up again: on the plaza walk (9 M emitted vertices) 1.8 s of a 2.1 s extraction, the
///   cubes themselves 0.27 s. Now the emitted vertices are sorted by (edge id, emission index) on every core: each
///   run of one id is one vertex, its first element the first emission. A scan in emission order numbers the vertices
///   and copies their positions; triangles index the emitted vertices of their part, so they are remapped through
///   that numbering with no lookups. The mesh is the same bit for bit (positions, order, triangles).
pub fn extract(v: &Volume) -> Mesh {
    let faces_of = edge_faces();
    let parts: Vec<Part> = (0..v.keys.len())
        .into_par_iter()
        .map(|b| {
            let key = v.keys[b];
            let nb: [Option<usize>; 8] = std::array::from_fn(|k| {
                if k == 0 {
                    Some(b)
                } else {
                    v.index.get(&[key[0] + (k as i32 & 1), key[1] + ((k as i32 >> 1) & 1), key[2] + ((k as i32 >> 2) & 1)]).map(|&i| i as usize)
                }
            });
            // The 9³ voxels this block's cubes read, and whether each was observed (its block exists, weight > 0).
            let mut value = [0.0f32; 729];
            let mut seen = [false; 729];
            for z in 0..=SIDE {
                for y in 0..=SIDE {
                    for x in 0..=SIDE {
                        let k = (x >= SIDE) as usize | ((y >= SIDE) as usize) << 1 | ((z >= SIDE) as usize) << 2;
                        let Some(blk) = nb[k] else { continue };
                        let i = blk * N + local([x % SIDE, y % SIDE, z % SIDE]);
                        let d = (x + 9 * (y + 9 * z)) as usize;
                        seen[d] = v.weight[i] > 0.0;
                        value[d] = v.sdf[i];
                    }
                }
            }
            let mut part = Part { ids: Vec::new(), positions: Vec::new(), triangles: Vec::new(), skipped_jumps: 0 };
            for z in 0..SIDE {
                for y in 0..SIDE {
                    for x in 0..SIDE {
                        let d = (x + 9 * (y + 9 * z)) as usize;
                        if GATHER.iter().all(|&o| seen[d + o]) {
                            let vals = GATHER.map(|o| value[d + o]);
                            cube(vals, [key[0] * SIDE + x, key[1] * SIDE + y, key[2] * SIDE + z], v.voxel, &faces_of, &mut part);
                        }
                    }
                }
            }
            part
        })
        .collect();
    merge(&parts)
}

/// The mesh of the parts in order: one vertex per id at its first emission, in first-emission order (see [`extract`]).
fn merge(parts: &[Part]) -> Mesh {
    use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
    let mut offsets = Vec::with_capacity(parts.len() + 1);
    offsets.push(0usize);
    for p in parts {
        offsets.push(offsets.last().unwrap() + p.ids.len());
    }
    let total = *offsets.last().unwrap();
    assert!(total < u32::MAX as usize, "too many surface vertices for 32-bit indices");
    // Every emission as (id, emission index), sorted: each run of one id starts with its first emission.
    let mut emitted = vec![(0u64, 0u32); total];
    {
        let mut rest = &mut emitted[..];
        let mut slices = Vec::with_capacity(parts.len());
        for p in parts {
            let (head, tail) = rest.split_at_mut(p.ids.len());
            slices.push(head);
            rest = tail;
        }
        slices.into_par_iter().zip(parts.par_iter().zip(&offsets[..parts.len()])).for_each(|(out, (p, &at))| {
            for (k, (o, &id)) in out.iter_mut().zip(&p.ids).enumerate() {
                *o = (id, (at + k) as u32);
            }
        });
    }
    emitted.par_sort_unstable();
    // `index[e]`: the vertex emission `e` becomes. First the first emissions are marked, numbered in emission order
    // (copying their positions), then every other emission takes its first's number.
    const FIRST: u32 = u32::MAX;
    let index: Vec<AtomicU32> = (0..total).into_par_iter().map(|_| AtomicU32::new(0)).collect();
    emitted.par_chunk_by(|a, b| a.0 == b.0).for_each(|run| index[run[0].1 as usize].store(FIRST, Relaxed));
    let mut mesh = Mesh::default();
    for (p, &at) in parts.iter().zip(&offsets) {
        for (k, position) in p.positions.iter().enumerate() {
            let slot = &index[at + k];
            if slot.load(Relaxed) == FIRST {
                slot.store(mesh.positions.len() as u32, Relaxed);
                mesh.positions.push(*position);
            }
        }
    }
    emitted.par_chunk_by(|a, b| a.0 == b.0).for_each(|run| {
        let vertex = index[run[0].1 as usize].load(Relaxed);
        for &(_, e) in &run[1..] {
            index[e as usize].store(vertex, Relaxed);
        }
    });
    drop(emitted);
    let count: usize = parts.iter().map(|p| p.triangles.len()).sum();
    mesh.triangles = vec![[0u32; 3]; count];
    let mut rest = &mut mesh.triangles[..];
    let mut slices = Vec::with_capacity(parts.len());
    for p in parts {
        let (head, tail) = rest.split_at_mut(p.triangles.len());
        slices.push(head);
        rest = tail;
    }
    slices.into_par_iter().zip(parts.par_iter().zip(&offsets[..parts.len()])).for_each(|(out, (p, &at))| {
        for (o, t) in out.iter_mut().zip(&p.triangles) {
            *o = t.map(|e| index[at + e as usize].load(Relaxed));
        }
    });
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(f: impl Fn([f32; 3]) -> f32, extent: i32, voxel: f32) -> Volume {
        let mut v = Volume::empty(voxel, 3.0 * voxel);
        for bz in -extent..extent {
            for by in -extent..extent {
                for bx in -extent..extent {
                    let b = v.ensure([bx, by, bz]);
                    for z in 0..SIDE {
                        for y in 0..SIDE {
                            for x in 0..SIDE {
                                let g = [bx * SIDE + x, by * SIDE + y, bz * SIDE + z];
                                let s = f(g.map(|c| c as f32 * voxel)) / (3.0 * voxel);
                                v.sdf[b * N + local([x, y, z])] = s.clamp(-1.0, 1.0);
                                v.weight[b * N + local([x, y, z])] = 1.0;
                            }
                        }
                    }
                }
            }
        }
        v
    }

    #[test]
    fn tables_are_consistent() {
        for (f, face) in FACES.iter().enumerate() {
            for k in 0..4 {
                let [a, b] = EDGES[face[k]];
                let (c0, c1) = (FACE_CORNERS[f][k], FACE_CORNERS[f][(k + 1) % 4]);
                assert!((a, b) == (c0, c1) || (a, b) == (c1, c0), "face {f} edge {k}");
            }
        }
        assert!(edge_faces().iter().all(|m| m.count_ones() == 2));
    }

    #[test]
    fn a_sphere_is_closed_and_manifold() {
        let v = volume(|p| (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - 0.37, 1, 0.1);
        let m = extract(&v);
        let a = m.audit();
        assert!(m.triangles.len() > 100);
        assert_eq!((a.boundary_edges, a.non_manifold_edges, a.components), (0, 0, 1), "{a:?}");
        // Faces point outwards (free space, positive values).
        let outward = m.triangles.iter().filter(|t| {
            let p = t.map(|i| m.positions[i as usize]);
            let n = crate::camera::cross(super::super::sub(p[1], p[0]), super::super::sub(p[2], p[0]));
            crate::camera::dot(n, p[0]) > 0.0
        });
        assert_eq!(outward.count(), m.triangles.len());
        for p in &m.positions {
            let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((r - 0.37).abs() < 0.02, "{r}");
        }
    }

    #[test]
    fn saddles_stay_manifold() {
        // Two close spheres and a field full of ambiguous faces: still no edge of more than two faces.
        let v = volume(|p| ((p[0] * 7.3).sin() * (p[1] * 5.1).cos() + (p[2] * 6.7).sin() * 0.8) * 0.1, 1, 0.1);
        let a = extract(&v).audit();
        assert_eq!(a.non_manifold_edges, 0, "{a:?}");
    }

    #[test]
    fn unobserved_is_not_a_wall_and_jumps_are_not_surfaces() {
        let mut v = volume(|p| p[1] - 0.05, 1, 0.1);
        let floor = extract(&v).triangles.len();
        assert!(floor > 0);
        for w in v.weight.iter_mut().step_by(2) {
            *w = 0.0;
        }
        assert!(extract(&v).triangles.len() < floor / 4);
        let jump = volume(|p| if p[1] < 0.05 { -1.0 } else { 1.0 }, 1, 0.1);
        assert!(extract(&jump).triangles.is_empty());
    }

    /// The first version of the extraction (main 658a903), kept to prove that today's gives the same mesh bit for bit.
    mod reference {
        use super::super::*;
        use std::collections::HashMap;

        struct Part {
            vertices: Vec<(u64, [f32; 3])>,
            triangles: Vec<[u64; 3]>,
            skipped_jumps: usize,
        }

        /// Polygonizes one cube: `vals` at [`CORNERS`], `origin` its first voxel.
        fn cube(vals: [f32; 8], origin: [i32; 3], voxel: f32, faces_of: &[u8; 12], out: &mut Part) {
            let inside = vals.map(|v| v < 0.0);
            let active: [bool; 12] = EDGES.map(|[a, b]| inside[a] != inside[b]);
            if !active.iter().any(|&a| a) {
                return;
            }
            if EDGES.iter().zip(active).any(|([a, b], on)| on && (vals[*a] - vals[*b]).abs() > MAX_JUMP) {
                out.skipped_jumps += 1;
                return;
            }
            let mut adj = [[usize::MAX; 2]; 12];
            let mut link = |a: usize, b: usize| {
                let slot = |s: &mut [usize; 2], v: usize| {
                    if s[0] == usize::MAX {
                        s[0] = v
                    } else {
                        s[1] = v
                    }
                };
                slot(&mut adj[a], b);
                slot(&mut adj[b], a);
            };
            for (f, face) in FACES.iter().enumerate() {
                let on: Vec<usize> = (0..4).filter(|&k| active[face[k]]).collect();
                match on.len() {
                    2 => link(face[on[0]], face[on[1]]),
                    4 => {
                        let c = FACE_CORNERS[f].map(|k| vals[k]);
                        let den = c[0] + c[2] - c[1] - c[3];
                        let saddle = if den.abs() > 1e-12 { (c[0] * c[2] - c[1] * c[3]) / den } else { 0.0 };
                        if (saddle < 0.0) == inside[FACE_CORNERS[f][0]] {
                            // Corners 0 and 2 meet through the face: cut corners 1 (edges 0, 1) and 3 (edges 2, 3) off.
                            link(face[0], face[1]);
                            link(face[2], face[3]);
                        } else {
                            link(face[3], face[0]);
                            link(face[1], face[2]);
                        }
                    }
                    _ => {}
                }
            }
            let position = |e: usize| -> (u64, [f32; 3]) {
                let [a, b] = EDGES[e];
                let (ga, gb) = (CORNERS[a], CORNERS[b]);
                let axis = (0..3).find(|&i| ga[i] != gb[i]).unwrap();
                let lo = if ga[axis] < gb[axis] { ga } else { gb };
                let t = (vals[a] / (vals[a] - vals[b])).clamp(1e-3, 1.0 - 1e-3);
                let p = std::array::from_fn(|i| (origin[i] as f32 + ga[i] as f32 + t * (gb[i] - ga[i]) as f32) * voxel);
                (edge_id([origin[0] + lo[0], origin[1] + lo[1], origin[2] + lo[2]], axis), p)
            };
            let grad = [
                vals[1] + vals[2] + vals[5] + vals[6] - vals[0] - vals[3] - vals[4] - vals[7],
                vals[2] + vals[3] + vals[6] + vals[7] - vals[0] - vals[1] - vals[4] - vals[5],
                vals[4] + vals[5] + vals[6] + vals[7] - vals[0] - vals[1] - vals[2] - vals[3],
            ];
            let mut used = [false; 12];
            let mut ring_index = 0u64;
            for start in 0..12 {
                if !active[start] || used[start] || adj[start][1] == usize::MAX {
                    continue;
                }
                let mut ring = Vec::with_capacity(8);
                let mut e = start;
                let mut prev = usize::MAX;
                while !used[e] {
                    used[e] = true;
                    ring.push(e);
                    let next = if adj[e][0] != prev { adj[e][0] } else { adj[e][1] };
                    prev = e;
                    e = next;
                }
                if ring.len() < 3 {
                    continue;
                }
                let verts: Vec<(u64, [f32; 3])> = ring.iter().map(|&e| position(e)).collect();
                // Newell's normal of the polygon against the gradient (towards free space).
                let mut n = [0.0f32; 3];
                for k in 0..verts.len() {
                    let (a, b) = (verts[k].1, verts[(k + 1) % verts.len()].1);
                    n[0] += (a[1] - b[1]) * (a[2] + b[2]);
                    n[1] += (a[2] - b[2]) * (a[0] + b[0]);
                    n[2] += (a[0] - b[0]) * (a[1] + b[1]);
                }
                let mut order: Vec<usize> = (0..ring.len()).collect();
                if n[0] * grad[0] + n[1] * grad[1] + n[2] * grad[2] < 0.0 {
                    order.reverse();
                }
                out.vertices.extend(verts.iter().copied());
                let m = order.len();
                let safe = |s: usize| (2..m - 1).all(|k| faces_of[ring[order[s]]] & faces_of[ring[order[(s + k) % m]]] == 0);
                if let Some(s) = (0..m).find(|&s| safe(s)) {
                    for k in 1..m - 1 {
                        out.triangles.push([verts[order[s]].0, verts[order[(s + k) % m]].0, verts[order[(s + k + 1) % m]].0]);
                    }
                } else {
                    // A centre's id: the cube's first lattice edge id with the top bit set and the ring (at most four) as axis.
                    let c = (1 << 63) | edge_id(origin, ring_index as usize & 3);
                    let centre = std::array::from_fn(|i| verts.iter().map(|v| v.1[i]).sum::<f32>() / m as f32);
                    out.vertices.push((c, centre));
                    for k in 0..m {
                        out.triangles.push([c, verts[order[k]].0, verts[order[(k + 1) % m]].0]);
                    }
                }
                ring_index += 1;
            }
        }

        /// Extracts the surface of every observed cube of the volume.
        pub fn extract(v: &Volume) -> Mesh {
            let faces_of = edge_faces();
            let parts: Vec<Part> = (0..v.keys.len())
                .into_par_iter()
                .map(|b| {
                    let key = v.keys[b];
                    let nb: [Option<usize>; 8] = std::array::from_fn(|k| {
                        if k == 0 {
                            Some(b)
                        } else {
                            v.index.get(&[key[0] + (k as i32 & 1), key[1] + ((k as i32 >> 1) & 1), key[2] + ((k as i32 >> 2) & 1)]).map(|&i| i as usize)
                        }
                    });
                    let fetch = |l: [i32; 3]| -> Option<f32> {
                        let k = (l[0] >= SIDE) as usize | ((l[1] >= SIDE) as usize) << 1 | ((l[2] >= SIDE) as usize) << 2;
                        let blk = nb[k]?;
                        let i = blk * N + local(l.map(|c| c % SIDE));
                        (v.weight[i] > 0.0).then_some(v.sdf[i])
                    };
                    let mut part = Part { vertices: Vec::new(), triangles: Vec::new(), skipped_jumps: 0 };
                    for z in 0..SIDE {
                        for y in 0..SIDE {
                            for x in 0..SIDE {
                                let mut vals = [0.0f32; 8];
                                let mut ok = true;
                                for (k, c) in CORNERS.iter().enumerate() {
                                    match fetch([x + c[0], y + c[1], z + c[2]]) {
                                        Some(s) => vals[k] = s,
                                        None => {
                                            ok = false;
                                            break;
                                        }
                                    }
                                }
                                if ok {
                                    cube(vals, [key[0] * SIDE + x, key[1] * SIDE + y, key[2] * SIDE + z], v.voxel, &faces_of, &mut part);
                                }
                            }
                        }
                    }
                    part
                })
                .collect();
            let total: usize = parts.iter().map(|p| p.vertices.len()).sum();
            let mut ids: HashMap<u64, u32> = HashMap::with_capacity(total / 3);
            let mut mesh = Mesh::default();
            for p in &parts {
                for &(id, pos) in &p.vertices {
                    ids.entry(id).or_insert_with(|| {
                        mesh.positions.push(pos);
                        (mesh.positions.len() - 1) as u32
                    });
                }
            }
            for p in &parts {
                mesh.triangles.extend(p.triangles.iter().map(|t| t.map(|id| ids[&id])));
            }
            mesh
        }
    }

    /// splitmix64: a deterministic test generator.
    fn noise(state: &mut u64) -> f32 {
        *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
    }

    fn bits(m: &Mesh) -> (Vec<[u32; 3]>, Vec<[u32; 3]>) {
        (m.positions.iter().map(|p| p.map(f32::to_bits)).collect(), m.triangles.clone())
    }

    #[test]
    fn the_mesh_is_the_first_versions_bit_for_bit() {
        // Smooth fields (closed surfaces, saddles) and a sparse, noisy one: blocks missing, unobserved voxels, jumps,
        // values exactly 0, blocks in no particular order.
        let mut fields = vec![
            volume(|p| (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - 0.37, 1, 0.1),
            volume(|p| ((p[0] * 7.3).sin() * (p[1] * 5.1).cos() + (p[2] * 6.7).sin() * 0.8) * 0.1, 1, 0.1),
            volume(|p| p[1] - 0.05, 2, 0.04),
        ];
        let mut state = 7u64;
        let mut sparse = Volume::empty(0.05, 0.15);
        for bz in -2..2 {
            for by in -2..2 {
                for bx in [1, -2, 0, -1] {
                    if noise(&mut state) < 0.2 {
                        continue;
                    }
                    let b = sparse.ensure([bx, by, bz]);
                    for i in 0..N {
                        let l = [(i % 8) as i32, (i / 8 % 8) as i32, (i / 64) as i32];
                        let g = [bx * SIDE + l[0], by * SIDE + l[1], bz * SIDE + l[2]];
                        let smooth = ((g[0] as f32 * 0.31).sin() + (g[1] as f32 * 0.17).cos() + (g[2] as f32 * 0.23).sin()) * 0.6;
                        let r = noise(&mut state);
                        sparse.sdf[b * N + i] = if r < 0.03 { 0.0 } else if r < 0.06 { 1.0 - 2.0 * (r * 33.0).fract() } else { (smooth + 0.3 * (r - 0.5)).clamp(-1.0, 1.0) };
                        sparse.weight[b * N + i] = if noise(&mut state) < 0.05 { 0.0 } else { 1.0 };
                    }
                }
            }
        }
        fields.push(sparse);
        for (k, v) in fields.iter().enumerate() {
            let (new, old) = (extract(v), reference::extract(v));
            assert!(!old.triangles.is_empty(), "field {k}");
            assert!(bits(&new) == bits(&old), "field {k}: {} / {} vertices, {} / {} triangles", new.positions.len(), old.positions.len(), new.triangles.len(), old.triangles.len());
        }
    }
}
