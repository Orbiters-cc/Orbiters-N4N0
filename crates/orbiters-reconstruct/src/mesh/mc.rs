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
use std::collections::HashMap;

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
}
