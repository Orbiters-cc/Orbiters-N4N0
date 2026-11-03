//! Unity's built-in meshes (`{fileID: 102xx, guid: 0000000000000000e000000000000000}`), which worlds use everywhere:
//! made here at Unity's sizes (a 1 m cube, a sphere of 0.5 m radius, a 10 m plane, a 1 m quad facing -Z, a cylinder
//! and a capsule 2 m tall), wound as Unity's front faces (clockwise in its left-handed space).

use super::model::{normalize, ModelMesh};

type Vertices = Vec<([f32; 3], [f32; 3], [f32; 2])>;

pub fn builtin(file_id: i64) -> Option<ModelMesh> {
    let (verts, idx) = match file_id {
        10202 => cube(),
        10206 => cylinder(),
        10207 => sphere(0.5, 0.0, 32, 16),
        10208 => capsule(),
        10209 => plane(),
        10210 => quad(),
        _ => return None,
    };
    Some(ModelMesh {
        name: "Built-in".into(),
        positions: verts.iter().map(|v| v.0).collect(),
        normals: verts.iter().map(|v| v.1).collect(),
        uvs: verts.iter().map(|v| v.2).collect(),
        submeshes: vec![idx],
        skin: None,
        blend_shapes: vec![],
    })
}

/// Turns each triangle to face the way its first vertex's normal does.
fn wind(verts: &Vertices, idx: &mut [u32]) {
    for t in idx.chunks_mut(3) {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| verts[i as usize].0);
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
        let vn = verts[t[0] as usize].1;
        if n[0] * vn[0] + n[1] * vn[1] + n[2] * vn[2] < 0.0 {
            t.swap(1, 2);
        }
    }
}

fn grid(cols: u32, rows: u32, f: impl Fn(f32, f32) -> ([f32; 3], [f32; 3])) -> (Vertices, Vec<u32>) {
    let mut verts = vec![];
    for j in 0..=rows {
        for i in 0..=cols {
            let (u, v) = (i as f32 / cols as f32, j as f32 / rows as f32);
            let (p, n) = f(u, v);
            verts.push((p, n, [u, v]));
        }
    }
    let mut idx = vec![];
    for j in 0..rows {
        for i in 0..cols {
            let a = j * (cols + 1) + i;
            let (b, c, d) = (a + 1, a + cols + 1, a + cols + 2);
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    // Degenerate triangles at a sphere's poles are dropped.
    let mut kept = Vec::with_capacity(idx.len());
    for t in idx.chunks(3) {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| verts[i as usize].0);
        if a != b && b != c && a != c {
            kept.extend_from_slice(t);
        }
    }
    wind(&verts, &mut kept);
    (verts, kept)
}

fn cube() -> (Vertices, Vec<u32>) {
    let mut verts = vec![];
    let mut idx = vec![];
    for (n, u, v) in [
        ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, 0.0, 1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ] {
        let base = verts.len() as u32;
        for (a, b) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            let p: [f32; 3] = std::array::from_fn(|k| (n[k] + u[k] * a + v[k] * b) * 0.5);
            verts.push((p, n, [(a + 1.0) * 0.5, (b + 1.0) * 0.5]));
        }
        idx.extend_from_slice(&[base, base + 2, base + 1, base + 1, base + 2, base + 3]);
    }
    wind(&verts, &mut idx);
    (verts, idx)
}

/// A sphere of radius `r`, its halves `stretch` apart along Y (a capsule's ends).
fn sphere(r: f32, stretch: f32, cols: u32, rows: u32) -> (Vertices, Vec<u32>) {
    use std::f32::consts::{PI, TAU};
    grid(cols, rows, |u, v| {
        let (t, p) = (u * TAU, (v - 0.5) * PI);
        let n = [p.cos() * t.cos(), p.sin(), p.cos() * t.sin()];
        let shift = if n[1] >= 0.0 { stretch * 0.5 } else { -stretch * 0.5 };
        ([n[0] * r, n[1] * r + shift, n[2] * r], n)
    })
}

fn capsule() -> (Vertices, Vec<u32>) {
    // Rows at the equator twice so the straight part is drawn by the stretch.
    sphere(0.5, 1.0, 32, 17)
}

fn cylinder() -> (Vertices, Vec<u32>) {
    use std::f32::consts::TAU;
    let (mut verts, mut idx) = grid(32, 1, |u, v| {
        let a = u * TAU;
        ([0.5 * a.cos(), -1.0 + 2.0 * v, 0.5 * a.sin()], [a.cos(), 0.0, a.sin()])
    });
    for (y, n) in [(1.0f32, 1.0f32), (-1.0, -1.0)] {
        let base = verts.len() as u32;
        verts.push(([0.0, y, 0.0], [0.0, n, 0.0], [0.5, 0.5]));
        for i in 0..=32 {
            let a = i as f32 / 32.0 * TAU;
            verts.push(([0.5 * a.cos(), y, 0.5 * a.sin()], [0.0, n, 0.0], [0.5 + 0.5 * a.cos(), 0.5 + 0.5 * a.sin()]));
        }
        let mut cap = vec![];
        for i in 0..32 {
            cap.extend_from_slice(&[base, base + 1 + i, base + 2 + i]);
        }
        wind(&verts, &mut cap);
        idx.extend(cap);
    }
    (verts, idx)
}

fn plane() -> (Vertices, Vec<u32>) {
    grid(10, 10, |u, v| ([5.0 - 10.0 * u, 0.0, 5.0 - 10.0 * v], [0.0, 1.0, 0.0]))
}

fn quad() -> (Vertices, Vec<u32>) {
    grid(1, 1, |u, v| ([u - 0.5, v - 0.5, 0.0], normalize([0.0, 0.0, -1.0])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_meshes_have_unitys_sizes_and_face_out() {
        for id in [10202, 10206, 10207, 10208, 10209, 10210] {
            let m = builtin(id).unwrap();
            assert!(m.triangles() > 0, "{id}");
            for t in m.submeshes[0].chunks(3) {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| m.positions[i as usize]);
                let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                let vn = m.normals[t[0] as usize];
                assert!(n[0] * vn[0] + n[1] * vn[1] + n[2] * vn[2] >= -1e-9, "{id} has a face turned in");
            }
        }
        let cube = super::super::build::bounds_of(&builtin(10202).unwrap().positions);
        assert_eq!((cube.min, cube.max), ([-0.5; 3], [0.5; 3]));
        let plane = super::super::build::bounds_of(&builtin(10209).unwrap().positions);
        assert_eq!((plane.min[0], plane.max[0]), (-5.0, 5.0));
        assert!(builtin(1).is_none());
    }
}
