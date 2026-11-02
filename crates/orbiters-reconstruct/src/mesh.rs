//! Experimental, CPU-only colliders from trusted metric depth; no explicit unknown-space filling.
//! Sparse samples contribute projective TSDF values only within their measured pixel footprint and a
//! two-voxel truncation band. Unobserved cubes are skipped, but subsequent vertex clustering can
//! bridge nearby surfaces and does not preserve the topology of unknown regions.
use crate::{
    camera::{cross, dot, Camera},
    dataset::Dataset,
};
use anyhow::{ensure, Context};
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::Path,
};
type Key = [i32; 3];
#[derive(Default, Clone)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
}
fn key(p: [f32; 3], s: f32) -> Key {
    p.map(|v| (v / s).floor() as i32)
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
const CORNERS: [Key; 8] = [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0], [0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]];
const EDGES: [[usize; 2]; 12] = [[0, 1], [1, 2], [2, 3], [3, 0], [4, 5], [5, 6], [6, 7], [7, 4], [0, 4], [1, 5], [2, 6], [3, 7]];
const FACES: [[usize; 4]; 6] = [[0, 1, 2, 3], [4, 5, 6, 7], [0, 9, 4, 8], [2, 10, 6, 11], [3, 11, 7, 8], [1, 10, 5, 9]];
/// Sparse marching cubes using face contours instead of a 256-case table. Ambiguous faces use a fixed
/// pairing; this is a lab extractor, not a guarantee of manifold topology. Unobserved corners skip a cube.
pub fn marching_cubes(field: &HashMap<Key, f32>, size: f32) -> Mesh {
    let mut mesh = Mesh::default();
    let mut shared = HashMap::new();
    let mut cells: Vec<_> = field.keys().copied().collect();
    cells.sort_unstable();
    for k in cells {
        let ks = CORNERS.map(|c| std::array::from_fn(|i| k[i] + c[i]));
        let Some(vals) = ks.iter().map(|k| field.get(k).copied()).collect::<Option<Vec<_>>>() else { continue };
        let active = EDGES.map(|[a, b]| (vals[a] < 0.) != (vals[b] < 0.));
        let mut adj = vec![Vec::new(); 12];
        for face in FACES {
            let e: Vec<_> = face.into_iter().filter(|&e| active[e]).collect();
            for pair in e.chunks_exact(2) {
                adj[pair[0]].push(pair[1]);
                adj[pair[1]].push(pair[0]);
            }
        }
        let mut used = [false; 12];
        for start in 0..12 {
            if used[start] || adj[start].len() != 2 {
                continue;
            }
            let mut ring = Vec::new();
            let mut e = start;
            let mut prev = usize::MAX;
            loop {
                if used[e] {
                    break;
                }
                used[e] = true;
                let [mut a, mut b] = EDGES[e];
                if ks[a] > ks[b] {
                    std::mem::swap(&mut a, &mut b);
                }
                let mut pair = [ks[a], ks[b]];
                pair.sort_unstable();
                let id = *shared.entry(pair).or_insert_with(|| {
                    let t = vals[a] / (vals[a] - vals[b]);
                    let p = std::array::from_fn(|i| (ks[a][i] as f32 + t * (ks[b][i] - ks[a][i]) as f32) * size);
                    let id = mesh.positions.len() as u32;
                    mesh.positions.push(p);
                    id
                });
                ring.push(id);
                let next = adj[e].iter().copied().find(|&v| v != prev);
                prev = e;
                let Some(n) = next else { break };
                e = n;
            }
            for j in 1..ring.len().saturating_sub(1) {
                let mut tri = [ring[0], ring[j], ring[j + 1]];
                let p = tri.map(|i| mesh.positions[i as usize]);
                let normal = cross(sub(p[1], p[0]), sub(p[2], p[0]));
                let grad = [
                    vals[1] + vals[2] + vals[5] + vals[6] - vals[0] - vals[3] - vals[4] - vals[7],
                    vals[2] + vals[3] + vals[6] + vals[7] - vals[0] - vals[1] - vals[4] - vals[5],
                    vals[4] + vals[5] + vals[6] + vals[7] - vals[0] - vals[1] - vals[2] - vals[3],
                ];
                if dot(normal, grad) < 0. {
                    tri.swap(1, 2)
                }
                mesh.triangles.push(tri);
            }
        }
    }
    mesh
}
// Nested vertex clusters bound extraction memory before the final collider-budget pass. Doubling
// the grid reuses weighted position sums, so earlier blocks are not repeatedly averaged as one vote.
struct StreamMesh {
    size: f32,
    bins: HashMap<Key, u32>,
    sums: Vec<([f64; 3], u64)>,
    triangles: Vec<[u32; 3]>,
    faces: HashSet<[u32; 3]>,
}
impl StreamMesh {
    fn new(size: f32) -> Self {
        Self { size, bins: HashMap::new(), sums: Vec::new(), triangles: Vec::new(), faces: HashSet::new() }
    }
    fn vertex(&mut self, p: [f32; 3], sum: [f64; 3], weight: u64) -> u32 {
        let id = *self.bins.entry(key(p, self.size)).or_insert_with(|| {
            self.sums.push(([0.; 3], 0));
            (self.sums.len() - 1) as u32
        });
        let s = &mut self.sums[id as usize];
        for i in 0..3 {
            s.0[i] += sum[i]
        }
        s.1 += weight;
        id
    }
    fn add(&mut self, m: Mesh) {
        let map: Vec<_> = m.positions.iter().map(|&p| self.vertex(p, p.map(|v| v as f64), 1)).collect();
        for t in m.triangles {
            let t = t.map(|i| map[i as usize]);
            let mut k = t;
            k.sort_unstable();
            if k[0] != k[1] && k[1] != k[2] && self.faces.insert(k) {
                self.triangles.push(t);
            }
        }
        while self.sums.len() > 200_000 || self.triangles.len() > 300_000 {
            self.size *= 2.;
            let old = std::mem::take(&mut self.sums);
            self.bins.clear();
            let map: Vec<_> = old.iter().map(|&(sum, n)| self.vertex(sum.map(|v| (v / n as f64) as f32), sum, n)).collect();
            let mut seen = HashSet::new();
            self.triangles = self
                .triangles
                .iter()
                .filter_map(|t| {
                    let t = t.map(|i| map[i as usize]);
                    let mut k = t;
                    k.sort_unstable();
                    (k[0] != k[1] && k[1] != k[2] && seen.insert(k)).then_some(t)
                })
                .collect();
            self.faces = seen;
        }
    }
    fn finish(self) -> Mesh {
        let mut seen = HashSet::new();
        let triangles = self
            .triangles
            .into_iter()
            .filter(|t| {
                let mut k = *t;
                k.sort_unstable();
                k[0] != k[1] && k[1] != k[2] && seen.insert(k)
            })
            .collect();
        Mesh { positions: self.sums.iter().map(|(p, n)| p.map(|v| (v / *n as f64) as f32)).collect(), triangles }
    }
}
/// Footprints are capped at four voxels to bound work and avoid inventing large surfaces from distant samples.
pub fn fuse(data: &Dataset, size: f32) -> anyhow::Result<Mesh> {
    ensure!(size.is_finite() && (0.01..=1.).contains(&size), "voxel must be finite, 0.01 to 1 metre");
    // Process overlapping 32-cell blocks, rather than keeping the entire distant landscape's field
    // resident. The plaza exceeded twelve million observed nodes even at 10 cm in the first trial.
    // Every block includes its positive boundary nodes; only complete cubes are extracted. Shared
    // boundary nodes receive identical contributions before globally aligned vertex clustering.
    let sample = |vi: usize, i: usize| {
        let dv = &data.depth_views[vi];
        let z = dv.depth[i];
        if !z.is_finite() || z <= 0. || dv.is_unknown(i) || dv.grid.unused.contains(i) {
            return None;
        }
        let [u, v] = dv.grid.pixels[i];
        let x = u.round() as i32;
        let y = v.round() as i32;
        if x < 0 || y < 0 || x >= data.width as i32 || y >= data.height as i32 || data.mask[(y as u32 * data.width + x as u32) as usize] <= 0. {
            return None;
        }
        let half = (dv.grid.spacing * dv.grid.step(i) as f32 * 0.55).max(0.5 / data.geometry.factor.max(1) as f32);
        let radius = ((z * half / dv.camera.fx.min(dv.camera.fy) / size).ceil() as i32 + 2).clamp(2, 4);
        Some((u, v, z, half, radius, key(dv.camera.unproject(u, v, z), size)))
    };
    let mut blocks: std::collections::BTreeMap<Key, Vec<(usize, usize)>> = std::collections::BTreeMap::new();
    for (vi, dv) in data.depth_views.iter().enumerate() {
        for i in 0..dv.depth.len() {
            let Some((_, _, _, _, radius, k)) = sample(vi, i) else { continue };
            let lo = k.map(|c| (c - radius - 1).div_euclid(32));
            let hi = k.map(|c| (c + radius).div_euclid(32));
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        blocks.entry([x, y, z]).or_default().push((vi, i));
                    }
                }
            }
        }
    }
    let mut mesh = StreamMesh::new(size * 0.25);
    let mut raw_triangles = 0usize;
    for (block, samples) in blocks {
        let lo = block.map(|v| v * 32);
        let mut field: HashMap<Key, (f32, u32)> = HashMap::new();
        for (vi, i) in samples {
            let Some((u, v, z, half, radius, k)) = sample(vi, i) else { continue };
            let camera = &data.depth_views[vi].camera;
            for x in (k[0] - radius).max(lo[0])..=(k[0] + radius).min(lo[0] + 32) {
                for y in (k[1] - radius).max(lo[1])..=(k[1] + radius).min(lo[1] + 32) {
                    for zz in (k[2] - radius).max(lo[2])..=(k[2] + radius).min(lo[2] + 32) {
                        let q = [x, y, zz];
                        let Some((px, py, pz)) = camera.project(q.map(|v| v as f32 * size)) else { continue };
                        let d = z - pz;
                        if (px - u).abs() > half || (py - v).abs() > half || d.abs() > 2. * size {
                            continue;
                        }
                        let mx = px.round() as i32;
                        let my = py.round() as i32;
                        if mx < 0
                            || my < 0
                            || mx >= data.width as i32
                            || my >= data.height as i32
                            || data.mask[(my as u32 * data.width + mx as u32) as usize] <= 0.
                        {
                            continue;
                        }
                        let entry = field.entry(q).or_default();
                        entry.0 += d / (2. * size);
                        entry.1 += 1;
                    }
                }
            }
        }
        let field = field.into_iter().map(|(k, (v, n))| (k, v / n as f32)).collect();
        let part = marching_cubes(&field, size);
        raw_triangles += part.triangles.len();
        mesh.add(part);
    }
    eprintln!("extracted {raw_triangles} triangles; streaming cluster cell {} m", mesh.size);
    Ok(mesh.finish())
}

impl Mesh {
    /// Remove components with fewer than `minimum` faces; retain separate substantial objects.
    pub fn clean(&mut self, minimum: usize) -> Vec<usize> {
        let mut parent: Vec<_> = (0..self.positions.len()).collect();
        fn root(p: &mut [usize], mut i: usize) -> usize {
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i]
            }
            i
        }
        for t in &self.triangles {
            let a = root(&mut parent, t[0] as usize);
            for &v in &t[1..] {
                let b = root(&mut parent, v as usize);
                parent[b] = a;
            }
        }
        let mut counts = HashMap::new();
        for t in &self.triangles {
            *counts.entry(root(&mut parent, t[0] as usize)).or_insert(0usize) += 1;
        }
        self.triangles.retain(|t| counts[&root(&mut parent, t[0] as usize)] >= minimum);
        let mut remap = HashMap::new();
        let mut positions = Vec::new();
        for t in &mut self.triangles {
            for v in t {
                let id = *remap.entry(*v).or_insert_with(|| {
                    positions.push(self.positions[*v as usize]);
                    (positions.len() - 1) as u32
                });
                *v = id;
            }
        }
        self.positions = positions;
        let mut sizes: Vec<_> = counts.values().copied().filter(|&n| n >= minimum).collect();
        sizes.sort_unstable_by(|a, b| b.cmp(a));
        sizes
    }
    /// Deterministic vertex clustering, increasing the cell until the hard triangle budget is met.
    pub fn decimate(&mut self, budget: usize, size: f32) {
        let original = self.clone();
        let mut cell = size;
        for _ in 0..64 {
            if self.triangles.len() <= budget {
                break;
            }
            let mut bins: HashMap<Key, u32> = HashMap::new();
            let mut sums: Vec<([f32; 3], u32)> = Vec::new();
            let map: Vec<_> = original
                .positions
                .iter()
                .map(|&p| {
                    let id = *bins.entry(key(p, cell)).or_insert_with(|| {
                        sums.push(([0.; 3], 0));
                        (sums.len() - 1) as u32
                    });
                    let s = &mut sums[id as usize];
                    for j in 0..3 {
                        s.0[j] += p[j]
                    }
                    s.1 += 1;
                    id
                })
                .collect();
            self.positions = sums.into_iter().map(|(p, n)| p.map(|v| v / n as f32)).collect();
            let mut seen = HashSet::new();
            self.triangles = original
                .triangles
                .iter()
                .filter_map(|t| {
                    let t = t.map(|v| map[v as usize]);
                    let mut sorted = t;
                    sorted.sort_unstable();
                    (sorted[0] != sorted[1] && sorted[1] != sorted[2] && seen.insert(sorted)).then_some(t)
                })
                .collect();
            cell *= 1.2;
        }
    }
    /// Floor components use only faces within 45 degrees of horizontal. A vertical ray under each
    /// walked camera must land on the same component; this does not test capsule clearance or step height.
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
        let mut hits = HashMap::new();
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
    /// GLB is right-handed (+Y up): reflect world X and reverse winding. OBJ uses the same convention.
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
        let vb = positions.len() * 12;
        let ib = triangles.len() * 12;
        let json = serde_json::json!({"asset":{"version":"2.0","generator":"Orbiters metric depth lab"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],"meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1,"mode":4}]}],"buffers":[{"byteLength":vb+ib}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":vb,"target":34962},{"buffer":0,"byteOffset":vb,"byteLength":ib,"target":34963}],"accessors":[{"bufferView":0,"componentType":5126,"count":positions.len(),"type":"VEC3","min":min,"max":max},{"bufferView":1,"componentType":5125,"count":triangles.len()*3,"type":"SCALAR"}]});
        let mut j = serde_json::to_vec(&json)?;
        while j.len() % 4 != 0 {
            j.push(b' ')
        }
        let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
        for v in [0x46546c67, 2, (28 + j.len() + vb + ib) as u32, j.len() as u32, 0x4e4f534a] {
            out.write_all(&v.to_le_bytes())?;
        }
        out.write_all(&j)?;
        out.write_all(&((vb + ib) as u32).to_le_bytes())?;
        out.write_all(&0x004e4942u32.to_le_bytes())?;
        for p in positions {
            for v in p {
                out.write_all(&v.to_le_bytes())?;
            }
        }
        for t in triangles {
            for v in t {
                out.write_all(&v.to_le_bytes())?;
            }
        }
        out.flush()?;
        Ok(())
    }
    /// Two-sided CPU z-buffer with perspective-correct eye depth and near-plane clipping.
    pub fn depth(&self, c: &Camera) -> Vec<f32> {
        let mut out = vec![f32::INFINITY; (c.width * c.height) as usize];
        for t in &self.triangles {
            let vertices = t.map(|i| c.to_camera(self.positions[i as usize]));
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
                            out[k] = out[k].min(z);
                        }
                    }
                }
            }
        }
        out
    }
}
pub fn cli(args: &[String]) -> anyhow::Result<()> {
    ensure!(args.len() == 2 || args.len() == 4, "usage: --mesh <session> <out.glb|out.obj> [--voxel M]");
    let size: f32 = if args.len() == 4 {
        ensure!(args[2] == "--voxel", "expected --voxel");
        args[3].parse()?
    } else {
        0.05
    };
    ensure!(size.is_finite() && (0.01..=1.).contains(&size), "invalid voxel size");
    let start = std::time::Instant::now();
    let prepared = crate::prepare::prepare(Path::new(&args[0]), &crate::prepare::Options { width: 640, aberration: false, ..Default::default() }, |_| {})?;
    let mut mesh = fuse(&prepared.data, size)?;
    mesh.clean(16);
    mesh.decimate(100_000, size);
    let components = mesh.clean(16);
    mesh.write(Path::new(&args[1])).context("writing mesh")?;
    println!(
        "{}",
        serde_json::json!({"seconds":start.elapsed().as_secs_f64(),"triangles":mesh.triangles.len(),"vertices":mesh.positions.len(),"components":components.len(),"voxel":size})
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tsdf_rejects_unknown_sky_and_masks() {
        use crate::dataset::{DepthGrid, DepthView, Loaded, Marks};
        let camera = Camera::look([0., 0., 0.], [0., 0., 1.], [0., 1., 0.], 1.0, 8, 8);
        let pixels = (0..8).flat_map(|y| (0..8).map(move |x| [x as f32, y as f32])).collect();
        let grid = std::sync::Arc::new(DepthGrid { width: 8, height: 8, pixels, spacing: 1., ..Default::default() });
        let dv = DepthView { camera, grid, depth: vec![1.; 64], game_time: 0., unknown: Marks::default(), decoder_unknown: 0, zero_may_be_unread: false };
        let mut data = Dataset {
            dir: Default::default(),
            views: vec![],
            depth_views: vec![dv],
            faces: None,
            width: 8,
            height: 8,
            geometry: Default::default(),
            mask: vec![1.; 64],
            loaded: Loaded::default(),
        };
        let mut m = fuse(&data, 0.1).unwrap();
        assert!(!m.triangles.is_empty());
        assert_eq!(m.clean(1).len(), 1, "block boundaries must not split a measured plane");
        assert!(m.positions.iter().all(|p| (p[2] - 1.).abs() < 1e-5));
        data.mask.fill(0.);
        assert!(fuse(&data, 0.1).unwrap().triangles.is_empty());
        data.mask.fill(1.);
        for i in 0..64 {
            data.depth_views[0].unknown.insert(i);
        }
        assert!(fuse(&data, 0.1).unwrap().triangles.is_empty());
        data.depth_views[0].unknown = Marks::default();
        data.depth_views[0].depth.fill(0.);
        assert!(fuse(&data, 0.1).unwrap().triangles.is_empty());
        assert!(fuse(&data, f32::NAN).is_err());
    }
    #[test]
    fn streaming_duplicates_do_not_force_coarsening() {
        let mut stream = StreamMesh::new(0.01);
        let m = Mesh { positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]], triangles: vec![[0, 1, 2]] };
        for _ in 0..300_010 {
            stream.add(m.clone());
        }
        assert_eq!(stream.size, 0.01);
        assert_eq!(stream.finish().triangles.len(), 1);
    }
    #[test]
    fn depth_and_floor() {
        let m = Mesh { positions: vec![[-2., 0., -2.], [2., 0., -2.], [2., 0., 2.], [-2., 0., 2.]], triangles: vec![[0, 1, 2], [0, 2, 3]] };
        assert_eq!(m.floor_support(&[[0., 1., 0.], [1., 1., 1.]]), (2, 2));
        let c = Camera::look([0., 1., 0.], [0., -1., 0.], [0., 0., 1.], 1., 3, 3);
        assert!(m.depth(&c).iter().all(|d| (*d - 1.).abs() < 1e-5));
    }
    #[test]
    fn plane() {
        let mut f = HashMap::new();
        for x in 0..4 {
            for y in 0..4 {
                for z in 0..4 {
                    f.insert([x, y, z], z as f32 - 1.5);
                }
            }
        }
        let m = marching_cubes(&f, 1.);
        assert_eq!(m.triangles.len(), 18);
        assert!(m.positions.iter().all(|p| (p[2] - 1.5).abs() < 1e-6));
    }
    #[test]
    fn unknown_is_not_a_wall() {
        assert!(marching_cubes(&HashMap::from([([0, 0, 0], -1.)]), 1.).triangles.is_empty());
    }
    #[test]
    fn glb_structure() {
        let m = Mesh { positions: vec![[0., 0., 1.], [1., 0., 1.], [0., 1., 1.]], triangles: vec![[0, 1, 2]] };
        let p = std::env::temp_dir().join(format!("orbiters-mesh-{}.glb", std::process::id()));
        m.write(&p).unwrap();
        let b = std::fs::read(&p).unwrap();
        std::fs::remove_file(p).unwrap();
        assert_eq!(&b[..4], b"glTF");
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize, b.len());
        let n = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
        let j: serde_json::Value = serde_json::from_slice(&b[20..20 + n]).unwrap();
        assert_eq!(j["accessors"][0]["min"][0], -1.);
        assert_eq!(j["accessors"][0]["min"][2], 1.);
        // UnityGLTF reflects X on import. The exported bytes must restore the exact captured positions.
        for (i, original) in m.positions.iter().enumerate() {
            let offset = 28 + n + i * 12;
            let mut p: [f32; 3] = std::array::from_fn(|c| f32::from_le_bytes(b[offset + 4 * c..offset + 4 * c + 4].try_into().unwrap()));
            p[0] = -p[0];
            assert_eq!(&p, original);
        }
        assert_eq!(j["accessors"][1]["count"], 3);
    }
}
