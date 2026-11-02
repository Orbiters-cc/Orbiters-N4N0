//! A collider people can walk on in VRChat or Unity, from the capture's exact depth (lab: `--mesh`).
//!
//! The first attempt fused the whole world, distant city and mountains included, into one triangle budget and merged
//! vertices into 0.64–0.8 m clusters to fit it: floors had holes, a third of the walked path had no floor under it,
//! and vertex clustering left tens of thousands of non-manifold edges. This one keeps what a person can reach and
//! nothing else, at the resolution a foot needs:
//!
//! 1. [`roi`]: the walked path (the avatar's hips from the capture kit, else the recorded camera at eye height) and the
//!    region a person can reach from it: [`Settings::radius`] metres around it horizontally, from
//!    [`Settings::below`] under the lowest feet to [`Settings::above`] over the highest. Far geometry is not a collider.
//! 2. [`tsdf`]: a sparse (hashed 8³-voxel blocks) truncated signed distance field at [`Settings::voxel`], allocated
//!    only near observed surfaces in that region. Depth samples are interpolated inside their sample grid's quads
//!    (inverse depth, exact for planes) only where the four corners lie on one surface seen at least
//!    [`Settings::min_angle`] off grazing, so a depth edge never becomes a skin.
//! 3. [`fill`]: the capturing person's own body is carved out where the skeleton says it stood; floor holes are
//!    closed only where evidence says floor: along the walked path (a person walked there), small holes enclosed by
//!    observed floor at one height and occlusion shadows bounded by it, never where the capture saw through the floor,
//!    and never walls.
//! 4. [`mc`]: marching cubes with consistent (asymptotic decider) face decisions: a manifold surface.
//! 5. [`simplify`]: quadric-error edge collapses that keep the surface manifold (link condition), boundaries in place
//!    and faces unflipped, to [`Settings::error`], then to the triangle budget.
//! 6. Floating debris smaller than [`Settings::debris`] across (particles, a passing avatar's ghost) is dropped.
//!
//! [`walk`] then walks a capsule along the recorded path on the result: the measure of whether people can use it.
//! Output: GLB in metres, +Y up, X reflected with reversed winding (UnityGLTF's and glTFast's convention: importing it
//! gives back the capture's Unity world exactly).

pub mod fill;
pub mod mc;
pub mod roi;
pub mod simplify;
pub mod tsdf;
pub mod walk;

use crate::{
    camera::{cross, dot, Camera},
    dataset::Dataset,
};
use anyhow::{ensure, Context};
use std::{io::Write, path::Path, time::Instant};

pub use roi::{Roi, WalkPath};

#[derive(Default, Clone, Debug, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

/// The collider's settings. Defaults are the measured choices (lab report `colliders-v2.md`).
#[derive(Clone, Debug)]
pub struct Settings {
    /// Voxel edge, metres. 4 cm resolves 15–20 cm stair risers in four voxels and a 5 cm bench board.
    pub voxel: f32,
    /// Truncation band, in voxels: past this behind a measured surface a voxel is not updated (occluded), past it in
    /// front it is free space.
    pub trunc_voxels: f32,
    /// Horizontal reach around the walked path, metres.
    pub radius: f32,
    /// Below the lowest feet and above the highest feet, metres.
    pub below: f32,
    pub above: f32,
    /// Depth samples farther than this are not fused: 16-bit inverse depth steps 6 cm at 20 m and 14 cm at 30 m.
    pub max_depth: f32,
    /// Truncation band at least this many of the depth code's steps at the sample's depth (0: fixed band). Lab switch
    /// for fusing far walls (see crate::mesh::tsdf).
    pub quant_trunc: f32,
    /// A sample quad seen this close to grazing (degrees) is a depth edge or too oblique to trust.
    pub min_angle: f32,
    /// Simplification error, metres (area-weighted RMS distance to the fused surface).
    pub error: f32,
    /// Triangle budget: past it the error doubles until the collider fits.
    pub budget: usize,
    /// Floor hole filling (crate::mesh::fill).
    pub fill: bool,
    /// Floating components smaller than this across (metres) are dropped.
    pub debris: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            voxel: 0.04,
            trunc_voxels: 3.0,
            radius: 40.0,
            below: 2.0,
            above: 4.0,
            max_depth: 30.0,
            quant_trunc: 0.0,
            min_angle: 4.0,
            error: 0.02,
            budget: PC_BUDGET,
            fill: true,
            debris: 0.6,
        }
    }
}

/// Triangle budgets. PhysX keeps a static triangle mesh in a BVH, so a query costs O(log n): triangles cost memory
/// (about 50 bytes each cooked) and cooking time rather than frame time. PC: 150 000 (≈ 7 MB cooked, well under any
/// world's other meshes). Quest (Android): 50 000, a third of that, for its tighter memory and the 100 MB world cap.
pub const PC_BUDGET: usize = 150_000;
pub const QUEST_BUDGET: usize = 50_000;

/// A built collider and what it was built from.
pub struct Built {
    pub mesh: Mesh,
    pub roi: Roi,
    pub path: WalkPath,
    pub volume: tsdf::Volume,
    pub stats: serde_json::Value,
}

/// The collider of a prepared session.
pub fn build(session: &Path, data: &Dataset, s: &Settings) -> anyhow::Result<Built> {
    ensure!(s.voxel.is_finite() && (0.01..=0.5).contains(&s.voxel), "voxel must be finite, 0.01 to 0.5 metre");
    let start = Instant::now();
    let mut stage = Instant::now();
    let mut lap = |name: &str, times: &mut serde_json::Map<String, serde_json::Value>| {
        times.insert(name.into(), stage.elapsed().as_secs_f64().into());
        stage = Instant::now();
    };
    let mut times = serde_json::Map::new();
    let path = roi::walked(session, data);
    ensure!(!path.feet.is_empty(), "no camera path in this session");
    let roi = Roi::new(&path, s.radius, s.below, s.above);
    let mut volume = tsdf::fuse(data, &roi, s);
    lap("fuse", &mut times);
    // The capturing person's own body is not the world (crate::mesh::fill::carve_walker). Only where the skeleton
    // says where it was: a camera path is not a body.
    let carved = if path.source == "hips" { fill::carve_walker(&mut volume, &path) } else { 0 };
    lap("carve", &mut times);
    let fill = if s.fill { fill::floors(&mut volume, &path, &roi) } else { fill::Stats::default() };
    lap("fill", &mut times);
    let mut mesh = mc::extract(&volume);
    let extracted = mesh.triangles.len();
    lap("extract", &mut times);
    let before = mesh.drop_debris(s.debris, &path);
    lap("debris", &mut times);
    let passes = simplify::collider(&mut mesh, s);
    lap("simplify", &mut times);
    let after = mesh.drop_debris(s.debris, &path);
    lap("debris2", &mut times);
    let stats = serde_json::json!({
        "seconds": start.elapsed().as_secs_f64(),
        "stages": times,
        "path_source": path.source,
        "path_points": path.feet.len(),
        "eye_height": path.eye_height,
        "roi_cells": roi.mask.iter().filter(|m| **m).count(),
        "roi_y": [roi.y_min, roi.y_max],
        "blocks": volume.keys.len(),
        "fill": fill,
        "carved_voxels": carved,
        "extracted_triangles": extracted,
        "debris_before": before,
        "debris_after": after,
        "simplify_passes": passes,
        "triangles": mesh.triangles.len(),
        "vertices": mesh.positions.len(),
    });
    Ok(Built { mesh, roi, path, volume, stats })
}

/// Components of a mesh (shared vertices), each as its triangles' indices.
pub fn components(m: &Mesh) -> Vec<Vec<usize>> {
    let mut parent: Vec<u32> = (0..m.positions.len() as u32).collect();
    fn root(p: &mut [u32], mut i: u32) -> u32 {
        while p[i as usize] != i {
            p[i as usize] = p[p[i as usize] as usize];
            i = p[i as usize];
        }
        i
    }
    for t in &m.triangles {
        let a = root(&mut parent, t[0]);
        for &v in &t[1..] {
            let b = root(&mut parent, v);
            if a != b {
                parent[b as usize] = a;
            }
        }
    }
    let mut groups: std::collections::HashMap<u32, Vec<usize>> = std::collections::HashMap::new();
    for (i, t) in m.triangles.iter().enumerate() {
        groups.entry(root(&mut parent, t[0])).or_default().push(i);
    }
    let mut out: Vec<Vec<usize>> = groups.into_values().collect();
    out.sort_by_key(|g| std::cmp::Reverse(g.len()));
    out
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}

/// A triangle's unit normal (zero when degenerate).
pub fn normal(p: [[f32; 3]; 3]) -> [f32; 3] {
    let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
    let l = dot(n, n).sqrt();
    if l > 1e-20 {
        n.map(|v| v / l)
    } else {
        [0.0; 3]
    }
}

/// Edge audit: boundary edges (one face), non-manifold edges (more than two).
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct Audit {
    pub boundary_edges: usize,
    pub non_manifold_edges: usize,
    pub degenerate_triangles: usize,
    pub components: usize,
}

impl Mesh {
    pub fn area(&self, t: &[u32; 3]) -> f32 {
        let p = t.map(|i| self.positions[i as usize]);
        let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        0.5 * dot(n, n).sqrt()
    }

    /// Drops floating debris: a component is kept if its bounding box is at least `debris` metres along some axis, or
    /// it is the largest (strictly the most triangles). Smaller ones (particles, leaves in the air, the cut-off hand of
    /// the carved walker) go.
    /// Returns (components before, components dropped, triangles dropped).
    pub fn drop_debris(&mut self, debris: f32, _path: &WalkPath) -> [usize; 3] {
        let groups = components(self);
        let largest = groups.first().is_some_and(|g| g.len() > groups.get(1).map_or(0, |h| h.len()));
        let mut keep = vec![false; self.triangles.len()];
        let mut dropped = 0;
        let mut dropped_triangles = 0;
        for (k, g) in groups.iter().enumerate() {
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            for &t in g {
                for &v in &self.triangles[t] {
                    let p = self.positions[v as usize];
                    for i in 0..3 {
                        lo[i] = lo[i].min(p[i]);
                        hi[i] = hi[i].max(p[i]);
                    }
                }
            }
            let extent = (0..3).map(|i| hi[i] - lo[i]).fold(0.0, f32::max);
            if (k == 0 && largest) || extent >= debris {
                for &t in g {
                    keep[t] = true;
                }
            } else {
                dropped += 1;
                dropped_triangles += g.len();
            }
        }
        let mut i = 0;
        self.triangles.retain(|_| {
            i += 1;
            keep[i - 1]
        });
        self.compact();
        [groups.len(), dropped, dropped_triangles]
    }

    /// Removes unreferenced vertices, keeping order.
    pub fn compact(&mut self) {
        let mut map = vec![u32::MAX; self.positions.len()];
        let mut positions = Vec::new();
        for t in &mut self.triangles {
            for v in t.iter_mut() {
                if map[*v as usize] == u32::MAX {
                    map[*v as usize] = positions.len() as u32;
                    positions.push(self.positions[*v as usize]);
                }
                *v = map[*v as usize];
            }
        }
        self.positions = positions;
    }

    pub fn audit(&self) -> Audit {
        let mut edges: Vec<u64> = self
            .triangles
            .iter()
            .flat_map(|t| (0..3).map(move |k| (t[k].min(t[(k + 1) % 3]) as u64) << 32 | t[k].max(t[(k + 1) % 3]) as u64))
            .collect();
        edges.sort_unstable();
        let mut a = Audit { components: components(self).len(), ..Default::default() };
        let mut i = 0;
        while i < edges.len() {
            let mut j = i;
            while j < edges.len() && edges[j] == edges[i] {
                j += 1;
            }
            match j - i {
                1 => a.boundary_edges += 1,
                2 => {}
                _ => a.non_manifold_edges += 1,
            }
            i = j;
        }
        a.degenerate_triangles = self.triangles.iter().filter(|t| t[0] == t[1] || t[1] == t[2] || t[0] == t[2] || self.area(t) < 1e-10).count();
        a
    }

    /// Floor components use only faces within 45 degrees of horizontal. A vertical ray under each walked camera
    /// must land on the same component; this does not test capsule clearance or step height (crate::mesh::walk does).
    pub fn floor_support(&self, path: &[[f32; 3]]) -> (usize, usize) {
        let mut parent: Vec<_> = (0..self.positions.len()).collect();
        fn root(p: &mut [usize], mut i: usize) -> usize {
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i]
            }
            i
        }
        let floor: Vec<_> = self
            .triangles
            .iter()
            .filter(|t| {
                let p = t.map(|i| self.positions[i as usize]);
                let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
                n[1] * n[1] > 0.5 * dot(n, n)
            })
            .collect();
        for t in &floor {
            let a = root(&mut parent, t[0] as usize);
            for &v in &t[1..] {
                let b = root(&mut parent, v as usize);
                parent[b] = a;
            }
        }
        let mut hits = std::collections::HashMap::new();
        for eye in path {
            let mut closest = 3.0;
            let mut hit = None;
            for t in &floor {
                let p = t.map(|i| self.positions[i as usize]);
                let e1 = sub(p[1], p[0]);
                let e2 = sub(p[2], p[0]);
                let h = cross([0., -1., 0.], e2);
                let det = dot(e1, h);
                if det.abs() < 1e-8 {
                    continue;
                }
                let d = sub(*eye, p[0]);
                let u = dot(d, h) / det;
                let q = cross(d, e1);
                let v = dot([0., -1., 0.], q) / det;
                let distance = dot(e2, q) / det;
                if u >= 0. && v >= 0. && u + v <= 1. && distance > 0.2 && distance < closest {
                    closest = distance;
                    hit = Some(root(&mut parent, t[0] as usize));
                }
            }
            if let Some(id) = hit {
                *hits.entry(id).or_insert(0usize) += 1;
            }
        }
        (hits.values().sum(), hits.values().copied().max().unwrap_or(0))
    }

    /// GLB (or OBJ) in UnityGLTF's and glTFast's convention: right-handed, +Y up, metres: world X reflected and the
    /// winding reversed, so an import gives back the capture's Unity world. One mesh named "N4N0 collider", 16-bit
    /// indices when it has at most 65 535 vertices.
    pub fn write(&self, path: &Path) -> anyhow::Result<()> {
        ensure!(!self.triangles.is_empty(), "no measured surface survived extraction");
        let positions: Vec<_> = self.positions.iter().map(|p| [-p[0], p[1], p[2]]).collect();
        let triangles: Vec<_> = self.triangles.iter().map(|t| [t[0], t[2], t[1]]).collect();
        if path.extension().and_then(|s| s.to_str()) == Some("obj") {
            let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
            for p in positions {
                writeln!(out, "v {} {} {}", p[0], p[1], p[2])?;
            }
            for t in triangles {
                writeln!(out, "f {} {} {}", t[0] + 1, t[1] + 1, t[2] + 1)?;
            }
            out.flush()?;
            return Ok(());
        }
        ensure!(path.extension().and_then(|s| s.to_str()) == Some("glb"), "output extension must be .glb or .obj");
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for p in &positions {
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
        let short = positions.len() <= 65_535;
        let vb = positions.len() * 12;
        let ib = triangles.len() * if short { 6 } else { 12 };
        let pad = (4 - ib % 4) % 4;
        let json = serde_json::json!({
            "asset": {"version": "2.0", "generator": "Orbiters N4N0 collider"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0, "name": "N4N0 collider"}],
            "meshes": [{"name": "N4N0 collider", "primitives": [{"attributes": {"POSITION": 0}, "indices": 1, "mode": 4}]}],
            "buffers": [{"byteLength": vb + ib + pad}],
            "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": vb, "target": 34962}, {"buffer": 0, "byteOffset": vb, "byteLength": ib, "target": 34963}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": positions.len(), "type": "VEC3", "min": min, "max": max},
                {"bufferView": 1, "componentType": if short { 5123 } else { 5125 }, "count": triangles.len() * 3, "type": "SCALAR"}
            ]
        });
        let mut j = serde_json::to_vec(&json)?;
        while j.len() % 4 != 0 {
            j.push(b' ')
        }
        let bin = vb + ib + pad;
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        for v in [0x46546c67, 2, (28 + j.len() + bin) as u32, j.len() as u32, 0x4e4f534a] {
            out.write_all(&v.to_le_bytes())?;
        }
        out.write_all(&j)?;
        out.write_all(&(bin as u32).to_le_bytes())?;
        out.write_all(&0x004e4942u32.to_le_bytes())?;
        for p in positions {
            for v in p {
                out.write_all(&v.to_le_bytes())?;
            }
        }
        for t in triangles {
            for v in t {
                if short {
                    out.write_all(&(v as u16).to_le_bytes())?;
                } else {
                    out.write_all(&v.to_le_bytes())?;
                }
            }
        }
        out.write_all(&vec![0u8; pad])?;
        out.flush()?;
        Ok(())
    }

    /// Reads a GLB of one mesh in one untransformed node, as Unity imports it (`reflect`: X reflected and winding
    /// reversed, UnityGLTF/glTFast) or raw.
    pub fn read_glb(path: &Path, reflect: bool) -> anyhow::Result<Mesh> {
        let b = std::fs::read(path)?;
        let word = |at: usize| -> anyhow::Result<u32> { Ok(u32::from_le_bytes(b.get(at..at + 4).context("truncated GLB word")?.try_into()?)) };
        ensure!(b.get(..4) == Some(b"glTF") && word(4)? == 2 && word(8)? as usize == b.len(), "GLB header");
        let n = word(12)? as usize;
        ensure!(word(16)? == 0x4e4f534a, "JSON chunk required");
        let j: serde_json::Value = serde_json::from_slice(b.get(20..20 + n).context("truncated JSON")?)?;
        let binary = 28 + n;
        ensure!(word(24 + n)? == 0x004e4942 && binary + word(20 + n)? as usize == b.len(), "BIN chunk required");
        ensure!(j["meshes"].as_array().is_some_and(|v| v.len() == 1) && j["meshes"][0]["primitives"].as_array().is_some_and(|v| v.len() == 1), "one mesh, one primitive");
        for field in ["matrix", "translation", "rotation", "scale", "children"] {
            ensure!(j["nodes"][0][field].is_null(), "transformed or hierarchical GLB is unsupported");
        }
        let primitive = &j["meshes"][0]["primitives"][0];
        ensure!(primitive["mode"].is_null() || primitive["mode"] == 4, "triangle list required");
        let pa = &j["accessors"][primitive["attributes"]["POSITION"].as_u64().context("position accessor")? as usize];
        let ia = &j["accessors"][primitive["indices"].as_u64().context("index accessor")? as usize];
        let data = |a: &serde_json::Value, element: usize| -> anyhow::Result<(&[u8], usize, usize)> {
            let v = &j["bufferViews"][a["bufferView"].as_u64().context("buffer view")? as usize];
            let start = binary + v["byteOffset"].as_u64().unwrap_or(0) as usize + a["byteOffset"].as_u64().unwrap_or(0) as usize;
            let count = a["count"].as_u64().context("accessor count")? as usize;
            let stride = v["byteStride"].as_u64().unwrap_or(element as u64) as usize;
            ensure!(start + count.saturating_sub(1) * stride + element <= b.len(), "accessor out of range");
            Ok((&b[start..], count, stride))
        };
        ensure!(pa["componentType"] == 5126 && pa["type"] == "VEC3", "float positions required");
        let (bytes, count, stride) = data(pa, 12)?;
        let positions: Vec<[f32; 3]> = (0..count)
            .map(|i| {
                let mut p: [f32; 3] = std::array::from_fn(|c| f32::from_le_bytes(bytes[i * stride + c * 4..i * stride + c * 4 + 4].try_into().unwrap()));
                if reflect {
                    p[0] = -p[0];
                }
                p
            })
            .collect();
        let ty = ia["componentType"].as_u64().context("index type")?;
        ensure!(ty == 5125 || ty == 5123, "unsigned int or short indices required");
        let element = if ty == 5125 { 4 } else { 2 };
        let (bytes, count, stride) = data(ia, element)?;
        let indices: Vec<u32> = (0..count)
            .map(|i| if ty == 5125 { u32::from_le_bytes(bytes[i * stride..i * stride + 4].try_into().unwrap()) } else { u16::from_le_bytes(bytes[i * stride..i * stride + 2].try_into().unwrap()) as u32 })
            .collect();
        ensure!(indices.iter().all(|&i| (i as usize) < positions.len()) && count % 3 == 0, "indices");
        let triangles = indices.chunks_exact(3).map(|t| if reflect { [t[0], t[2], t[1]] } else { [t[0], t[1], t[2]] }).collect();
        Ok(Mesh { positions, triangles })
    }

    /// Two-sided CPU z-buffer with perspective-correct eye depth and near-plane clipping; also each pixel's face
    /// normal (world), for shaded renders.
    pub fn render(&self, c: &Camera) -> (Vec<f32>, Vec<[f32; 3]>) {
        let mut out = vec![f32::INFINITY; (c.width * c.height) as usize];
        let mut normals = vec![[0.0; 3]; out.len()];
        for t in &self.triangles {
            let world = t.map(|i| self.positions[i as usize]);
            let n = cross(sub(world[1], world[0]), sub(world[2], world[0]));
            let l = dot(n, n).sqrt().max(1e-20);
            let n = n.map(|v| v / l);
            let vertices = world.map(|p| c.to_camera(p));
            let mut polygon = Vec::new();
            for i in 0..3 {
                let a = vertices[i];
                let b = vertices[(i + 1) % 3];
                let inside = a[2] >= 0.001;
                let next = b[2] >= 0.001;
                if inside {
                    polygon.push(a);
                }
                if inside != next {
                    let u = (0.001 - a[2]) / (b[2] - a[2]);
                    polygon.push(std::array::from_fn(|j| a[j] + u * (b[j] - a[j])));
                }
            }
            for fan in 1..polygon.len().saturating_sub(1) {
                let p = [polygon[0], polygon[fan], polygon[fan + 1]].map(|v| (c.fx * v[0] / v[2] + c.cx, c.fy * v[1] / v[2] + c.cy, v[2]));
                let edge = |a: (f32, f32, f32), b: (f32, f32, f32), x: f32, y: f32| (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0);
                let area = edge(p[0], p[1], p[2].0, p[2].1);
                if area.abs() < 1e-8 {
                    continue;
                }
                let lo_x = p.iter().map(|p| p.0).fold(f32::INFINITY, f32::min).floor().max(0.) as u32;
                let hi_x = p.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max).ceil().min(c.width as f32 - 1.).max(0.) as u32;
                let lo_y = p.iter().map(|p| p.1).fold(f32::INFINITY, f32::min).floor().max(0.) as u32;
                let hi_y = p.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max).ceil().min(c.height as f32 - 1.).max(0.) as u32;
                for y in lo_y..=hi_y {
                    for x in lo_x..=hi_x {
                        let a = edge(p[1], p[2], x as f32, y as f32) / area;
                        let b = edge(p[2], p[0], x as f32, y as f32) / area;
                        let d = 1. - a - b;
                        if a >= 0. && b >= 0. && d >= 0. {
                            let z = 1. / (a / p[0].2 + b / p[1].2 + d / p[2].2);
                            let k = (y * c.width + x) as usize;
                            if z < out[k] {
                                out[k] = z;
                                normals[k] = n;
                            }
                        }
                    }
                }
            }
        }
        (out, normals)
    }

    pub fn depth(&self, c: &Camera) -> Vec<f32> {
        self.render(c).0
    }
}

/// `orbiters-reconstruct --mesh <session> <out.glb|out.obj> [--voxel M] [--quest] [--no-fill] [--radius M]`.
pub fn cli(args: &[String]) -> anyhow::Result<()> {
    ensure!(args.len() >= 2, "usage: --mesh <session> <out.glb|out.obj> [--voxel M] [--quest] [--no-fill] [--radius M]");
    let mut s = Settings::default();
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--voxel" => {
                s.voxel = args.get(i + 1).context("--voxel M")?.parse()?;
                i += 1
            }
            "--radius" => {
                s.radius = args.get(i + 1).context("--radius M")?.parse()?;
                i += 1
            }
            "--quest" => s.budget = QUEST_BUDGET,
            "--no-fill" => s.fill = false,
            other => anyhow::bail!("unknown option {other}"),
        }
        i += 1;
    }
    ensure!(s.voxel.is_finite() && (0.01..=0.5).contains(&s.voxel), "invalid voxel size");
    let start = Instant::now();
    let session = Path::new(&args[0]);
    let prepared = crate::prepare::prepare(session, &crate::prepare::Options { width: 640, aberration: false, ..Default::default() }, |_| {})?;
    let built = build(session, &prepared.data, &s)?;
    built.mesh.write(Path::new(&args[1])).context("writing mesh")?;
    let mut stats = built.stats;
    stats["total_seconds"] = start.elapsed().as_secs_f64().into();
    println!("{stats}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glb_round_trips_through_unity_import() {
        let m = Mesh { positions: vec![[0., 0., 1.], [1., 0., 1.], [0., 1., 2.]], triangles: vec![[0, 1, 2]] };
        let p = std::env::temp_dir().join(format!("orbiters-mesh-{}.glb", std::process::id()));
        m.write(&p).unwrap();
        let b = std::fs::read(&p).unwrap();
        assert_eq!(&b[..4], b"glTF");
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize, b.len());
        assert_eq!(b.len() % 4, 0);
        // UnityGLTF reflects X on import: reading it back that way gives the captured positions and winding.
        assert_eq!(Mesh::read_glb(&p, true).unwrap(), m);
        let raw = Mesh::read_glb(&p, false).unwrap();
        assert_eq!(raw.positions[1], [-1., 0., 1.]);
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn depth_and_floor() {
        let m = Mesh { positions: vec![[-2., 0., -2.], [2., 0., -2.], [2., 0., 2.], [-2., 0., 2.]], triangles: vec![[0, 1, 2], [0, 2, 3]] };
        assert_eq!(m.floor_support(&[[0., 1., 0.], [1., 1., 1.]]), (2, 2));
        let c = Camera::look([0., 1., 0.], [0., -1., 0.], [0., 0., 1.], 1., 3, 3);
        assert!(m.depth(&c).iter().all(|d| (*d - 1.).abs() < 1e-5));
        let a = m.audit();
        assert_eq!((a.boundary_edges, a.non_manifold_edges, a.components), (4, 0, 1));
    }

    #[test]
    fn debris_goes_structures_stay() {
        let mut m = Mesh::default();
        let mut quad = |o: [f32; 3], s: f32| {
            let b = m.positions.len() as u32;
            m.positions.extend([[o[0], o[1], o[2]], [o[0] + s, o[1], o[2]], [o[0] + s, o[1], o[2] + s], [o[0], o[1], o[2] + s]]);
            m.triangles.extend([[b, b + 1, b + 2], [b, b + 2, b + 3]]);
        };
        quad([0., 0., 0.], 10.);
        quad([3., 0.5, 3.], 1.);
        quad([5., 1.5, 5.], 0.2);
        let path = WalkPath { feet: vec![[1., 0., 1.]], eye_height: 1.6, source: "test" };
        assert_eq!(m.drop_debris(0.6, &path), [3, 1, 2]);
        assert_eq!(m.triangles.len(), 4);
        assert_eq!(m.positions.len(), 8);
    }
}
