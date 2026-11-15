//! Finished 3D pictures of a world as a map for the next picture: where the captures saw solid things (walls, furniture),
//! where they saw open space (and floor), what they never saw, where the earlier pictures stood, and where to go next.
//!
//! **The frame** (measured on the tour's captures, 2026-11-15, `done7-mapplan.md`): one world frame for everything,
//! Unity's (left-handed, metres, +Y up): `/usercamera/Pose` (aim_camera's `from`/`to`), the strip's camera-to-world in
//! `frames.bin`, and the hips in `skeleton.bin` are the same numbers (Prismic's Avatar Search 07-00-48 and 07-01-07 began at
//! exactly the poses aim_camera reported, to the centimetre). `reconstruction/splats.ply` holds Unity's world with Z negated
//! (its header says so; rendering the cloud from a recorded camera matches the frame only that way). Headings are
//! aim_camera's: forward = (sin h, cos h) in (x, z), positive turns right; the head bone's yaw in `skeleton.bin` is the
//! heading VRChat's camera opened with, 0.37 m ahead of the hips along it. Units are metres: the floor in the cloud lies
//! VRChat's eye height (`/avatar/eyeheight`, 1.66) below where the camera opened.
//!
//! **What it can know.** One 8 s flight sees what is in sight of its spot. Plain or dark surfaces give few splats, and the
//! reconstruction leaves floaters near the camera's path and stray points far out: a voxel needs two points to count as
//! solid, rays from the flight carve what they crossed as open, and what no ray crossed is unknown, never free.

use serde::{Deserialize, Serialize};
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::path::Path;

/// Side of a voxel, metres.
pub const VOXEL: f32 = 0.25;
/// Side of a plan cell, metres (two voxels).
pub const CELL: f32 = 0.5;
/// VRChat's eye height of the tour's avatar (`/avatar/eyeheight`), when a capture does not say.
pub const EYE_HEIGHT: f32 = 1.66;
/// Hips over the floor for that avatar, measured in four worlds (1.02 to 1.03; 0.615 of its eye height).
const HIPS_PER_EYE: f32 = 0.615;
/// How far the captured space reaches around a flight, horizontally and below/above it.
const AROUND_M: f32 = 35.0;
const BELOW_M: f32 = 12.0;
const ABOVE_M: f32 = 15.0;
/// Points a voxel needs to be solid.
const SOLID: u16 = 2;
/// The rule: a world's pictures stand at least this far apart.
pub const APART_M: f32 = 10.0;
/// A body's band over the ground: what blocks walking, metres.
const BODY: (f32, f32) = (0.4, 1.8);
/// The largest step from cell to cell that is still walking (stairs, slopes), metres.
const STEP_M: f32 = 0.7;
/// The most voxels a dense grid may have before it goes coarser.
const MOST_VOXELS: usize = 24_000_000;

/// The avatar as a recording saw it: hips (Unity world) and heading.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Avatar {
    pub hips: [f32; 3],
    pub heading: f32,
}

/// One picture's share of a map: small enough to keep after the picture's own files are deleted.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Capture {
    /// The session's folder name.
    pub session: String,
    pub world: Option<String>,
    pub world_name: Option<String>,
    /// Unix seconds it was taken (its manifest's time).
    pub taken: u64,
    /// Where the camera stood when the picture began: the picture's centre.
    pub centre: [f32; 3],
    /// The camera's path, at most a few hundred positions.
    pub path: Vec<[f32; 3]>,
    pub avatar: Option<Avatar>,
    pub eye_height: f32,
    /// Voxels with points: x, y, z (in [`VOXEL`]s, Unity world) and how many points.
    #[serde(with = "voxels_text")]
    pub voxels: Vec<[i32; 4]>,
}

mod voxels_text {
    //! Voxels as base64 of little-endian i16 x, y, z and u16 count: about 11 bytes a voxel in the file.
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[[i32; 4]], s: S) -> Result<S::Ok, S::Error> {
        let mut bytes = Vec::with_capacity(v.len() * 8);
        for [x, y, z, n] in v {
            for c in [*x, *y, *z] {
                bytes.extend_from_slice(&(c.clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_le_bytes());
            }
            bytes.extend_from_slice(&((*n).clamp(0, u16::MAX as i32) as u16).to_le_bytes());
        }
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<[i32; 4]>, D::Error> {
        let text = String::deserialize(d)?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(text).map_err(serde::de::Error::custom)?;
        Ok(bytes
            .chunks_exact(8)
            .map(|c| {
                let i = |k: usize| i16::from_le_bytes([c[k], c[k + 1]]) as i32;
                [i(0), i(2), i(4), u16::from_le_bytes([c[6], c[7]]) as i32]
            })
            .collect())
    }
}

/// Heading of a direction in (x, z): aim_camera's (0 is +z, 90 is +x).
pub fn heading_of(dx: f32, dz: f32) -> f32 {
    dx.atan2(dz).to_degrees().rem_euclid(360.0)
}

/// An angle in -180..180.
pub fn wrap(a: f32) -> f32 {
    (a + 180.0).rem_euclid(360.0) - 180.0
}

/// The level unit vector of a heading, (x, z).
pub fn forward(heading: f32) -> [f32; 2] {
    let (s, c) = heading.to_radians().sin_cos();
    [s, c]
}

impl Capture {
    /// One picture's share from its session folder: the reconstruction's splats, the camera's poses and the avatar.
    pub fn read(dir: &Path) -> Result<Capture, String> {
        let session = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let manifest: serde_json::Value = std::fs::read(dir.join("manifest.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let path = camera_path(dir)?;
        let centre = path[0];
        let avatar = read_avatar(dir);
        let eye_height = read_eye_height(dir).unwrap_or(EYE_HEIGHT);
        let ply = std::fs::read(dir.join("reconstruction").join("splats.ply")).map_err(|e| format!("no finished reconstruction ({e})"))?;
        let points = ply_points(&ply)?;
        let voxels = voxelize(&points, &path);
        let taken = std::fs::metadata(dir.join("manifest.json")).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        Ok(Capture {
            session,
            world: manifest["world"]["id"].as_str().map(str::to_lowercase),
            world_name: manifest["world"]["name"].as_str().map(str::to_string),
            taken,
            centre,
            path: thin(&path, 240),
            avatar,
            eye_height,
            voxels,
        })
    }
}

/// The camera's positions over the recording, from `frames.bin`: frames whose strip read and that were not gated, without
/// the rig's faces (they repeat their frame's position).
fn camera_path(dir: &Path) -> Result<Vec<[f32; 3]>, String> {
    use orbiters_contracts::frame_record::record_flags::{GATED, STRIP_OK};
    let records = orbiters_core::bundle::read_records(dir).map_err(|e| format!("no camera poses ({e})"))?;
    let path: Vec<[f32; 3]> = records
        .iter()
        .filter(|r| r.flags & STRIP_OK != 0 && r.flags & GATED == 0 && (r.strip_flags >> 8) & 0xF == 0)
        .map(|r| [r.camera_to_world[3], r.camera_to_world[7], r.camera_to_world[11]])
        .filter(|p| p.iter().all(|v| v.is_finite() && v.abs() < 100_000.0))
        .collect();
    if path.is_empty() {
        return Err("no camera poses in its recording".into());
    }
    Ok(path)
}

/// `n` of `all`, evenly, the first kept.
fn thin(all: &[[f32; 3]], n: usize) -> Vec<[f32; 3]> {
    if all.len() <= n {
        return all.to_vec();
    }
    (0..n).map(|i| all[i * all.len() / n]).collect()
}

/// The avatar from `skeleton.bin` (`performance.md`): the last record's hips, and the head's yaw (the hips' without a head).
fn read_avatar(dir: &Path) -> Option<Avatar> {
    const HEADER: usize = 16;
    const RECORD: usize = 916;
    let bytes = std::fs::read(dir.join("skeleton.bin")).ok()?;
    if bytes.len() < HEADER + RECORD || &bytes[..4] != b"OSKL" {
        return None;
    }
    let count = (bytes.len() - HEADER) / RECORD;
    let r = &bytes[HEADER + (count - 1) * RECORD..HEADER + count * RECORD];
    let f = |at: usize| f32::from_le_bytes(r[at..at + 4].try_into().unwrap());
    let hips = [f(16), f(20), f(24)];
    let mask = u64::from_le_bytes(r[28..36].try_into().unwrap());
    let bone = if mask & (1 << 10) != 0 { 10 } else { 0 };
    let q = [f(36 + bone * 16), f(40 + bone * 16), f(44 + bone * 16), f(48 + bone * 16)];
    let [x, y, z, w] = q;
    let (fx, fz) = (2.0 * (x * z + w * y), 1.0 - 2.0 * (x * x + y * y));
    (hips.iter().all(|v| v.is_finite()) && (fx != 0.0 || fz != 0.0)).then(|| Avatar { hips, heading: heading_of(fx, fz) })
}

/// `/avatar/eyeheight` from the recorded parameters, metres.
fn read_eye_height(dir: &Path) -> Option<f32> {
    let names: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("parameters.json")).ok()?).ok()?;
    let id = names["parameters"].as_array()?.iter().find(|p| p["address"] == "/avatar/eyeheight")?["id"].as_u64()? as u16;
    let bytes = std::fs::read(dir.join("parameters.bin")).ok()?;
    bytes.get(16..)?.chunks_exact(16).find(|r| u16::from_le_bytes([r[8], r[9]]) == id && r[10] == 0).map(|r| f32::from_le_bytes(r[12..16].try_into().unwrap())).filter(|h| (0.2..20.0).contains(h))
}

/// The splats worth a map from a reconstruction's PLY, in Unity's world: opaque enough, small (not the background shell or a
/// blur), not the sky, seen by two keyframes or more when the file says.
pub fn ply_points(data: &[u8]) -> Result<Vec<[f32; 3]>, String> {
    let end = data.windows(11).position(|w| w == b"end_header\n").ok_or("not a PLY")? + 11;
    let header = std::str::from_utf8(&data[..end]).map_err(|_| "not a PLY")?;
    let count: usize = header.lines().find_map(|l| l.strip_prefix("element vertex ")).and_then(|c| c.trim().parse().ok()).ok_or("no vertex count")?;
    let props: Vec<&str> = header.lines().filter_map(|l| l.strip_prefix("property float ")).collect();
    if props.len() != header.lines().filter(|l| l.starts_with("property ")).count() {
        return Err("a PLY of other than floats".into());
    }
    let at = |name: &str| props.iter().position(|p| *p == name);
    let (Some(x), Some(y), Some(z), Some(opacity)) = (at("x"), at("y"), at("z"), at("opacity")) else { return Err("a PLY without positions".into()) };
    let scales = [at("scale_0"), at("scale_1"), at("scale_2")];
    let (sky, views) = (at("sky"), at("seen_views"));
    let stride = props.len() * 4;
    if data.len() < end + count * stride {
        return Err("a PLY cut short".into());
    }
    let mut out = Vec::with_capacity(count / 2);
    for i in 0..count {
        let row = &data[end + i * stride..end + (i + 1) * stride];
        let f = |k: usize| f32::from_le_bytes(row[k * 4..k * 4 + 4].try_into().unwrap());
        // Opacity is a logit: 0.3 is -0.847.
        if f(opacity) < -0.847 || sky.is_some_and(|k| f(k) > 0.5) || views.is_some_and(|k| f(k) < 2.0) {
            continue;
        }
        if scales.iter().flatten().any(|&k| f(k).exp() > 0.6) {
            continue;
        }
        let p = [f(x), f(y), -f(z)];
        if p.iter().all(|v| v.is_finite()) {
            out.push(p);
        }
    }
    Ok(out)
}

/// The points around the flight (stray ones at thousands of metres left out), counted per [`VOXEL`].
fn voxelize(points: &[[f32; 3]], path: &[[f32; 3]]) -> Vec<[i32; 4]> {
    let (lo, hi) = around(path);
    let mut counts: HashMap<[i32; 3], i32> = HashMap::new();
    for p in points {
        if (0..3).all(|k| p[k] > lo[k] && p[k] < hi[k]) {
            *counts.entry(p.map(|v| (v / VOXEL).floor() as i32)).or_default() += 1;
        }
    }
    let mut v: Vec<[i32; 4]> = counts.into_iter().map(|(k, n)| [k[0], k[1], k[2], n]).collect();
    v.sort_unstable();
    v
}

/// The box a flight's captured space can be in.
fn around(path: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in path {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    ([lo[0] - AROUND_M, lo[1] - BELOW_M, lo[2] - AROUND_M], [hi[0] + AROUND_M, hi[1] + ABOVE_M, hi[2] + AROUND_M])
}

/// What a plan cell is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Cell {
    /// Nothing seen there.
    Unknown,
    /// Something solid in a body's height over the ground: a wall, furniture, a plant.
    Blocked,
    /// Floor seen, and open over it.
    Floor,
    /// Open space seen, its floor not (a plain or dark floor, a hole or a drop).
    Open,
}

impl Cell {
    fn walkable(self) -> bool {
        matches!(self, Cell::Floor | Cell::Open)
    }
}

/// A world's captures as one map.
pub struct Map {
    /// Voxel side (VOXEL, or coarser for a large world).
    pub voxel: f32,
    /// Corner of voxel (0, 0, 0), metres.
    lo: [f32; 3],
    dims: [usize; 3],
    /// Points per voxel, saturated.
    points: Vec<u16>,
    /// Rays from the flights that crossed a voxel, saturated.
    rays: Vec<u8>,
    /// Plan: cells of `cell` metres over x and z.
    pub cell: f32,
    pub plan_dims: [usize; 2],
    pub cells: Vec<Cell>,
    /// The ground's height under each cell (NaN where none was seen).
    pub ground: Vec<f32>,
    /// A seen surface over head height to stand on (a balcony, a roof, the top of something large), metres.
    pub raised: Vec<f32>,
    /// The floor level the avatar stands on.
    pub floor: f32,
    pub captures: Vec<std::sync::Arc<Capture>>,
}

impl Map {
    pub fn build(captures: Vec<std::sync::Arc<Capture>>) -> Map {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for c in &captures {
            let (l, h) = around(&c.path);
            for k in 0..3 {
                lo[k] = lo[k].min(l[k]);
                hi[k] = hi[k].max(h[k]);
            }
        }
        // Not beyond what was seen.
        let mut seen_lo = [i32::MAX; 3];
        let mut seen_hi = [i32::MIN; 3];
        for v in captures.iter().flat_map(|c| &c.voxels) {
            for k in 0..3 {
                seen_lo[k] = seen_lo[k].min(v[k]);
                seen_hi[k] = seen_hi[k].max(v[k]);
            }
        }
        if seen_lo[0] <= seen_hi[0] {
            for k in 0..3 {
                lo[k] = lo[k].max(seen_lo[k] as f32 * VOXEL - 1.0);
                hi[k] = hi[k].min((seen_hi[k] + 1) as f32 * VOXEL + 1.0);
            }
        }
        for c in &captures {
            for p in &c.path {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k] - 1.0);
                    hi[k] = hi[k].max(p[k] + 1.0);
                }
            }
        }
        let mut scale = 1;
        let dims = loop {
            let v = VOXEL * scale as f32;
            let d = [0, 1, 2].map(|k| ((hi[k] - lo[k]) / v).ceil().max(1.0) as usize + 1);
            if d[0] * d[1] * d[2] <= MOST_VOXELS || scale >= 8 {
                break d;
            }
            scale *= 2;
        };
        let voxel = VOXEL * scale as f32;
        let lo = lo.map(|v| (v / voxel).floor() * voxel);
        let mut map = Map {
            voxel,
            lo,
            dims,
            points: vec![0; dims[0] * dims[1] * dims[2]],
            rays: vec![0; dims[0] * dims[1] * dims[2]],
            cell: CELL.max(voxel),
            plan_dims: [0, 0],
            cells: Vec::new(),
            ground: Vec::new(),
            raised: Vec::new(),
            floor: 0.0,
            captures,
        };
        for c in map.captures.clone() {
            for v in &c.voxels {
                let p = [0, 1, 2].map(|k| (v[k] as f32 + 0.5) * VOXEL);
                if let Some(i) = map.index_of(p) {
                    map.points[i] = map.points[i].saturating_add(v[3].clamp(0, u16::MAX as i32) as u16);
                }
            }
        }
        map.carve();
        map.plan();
        map
    }

    fn index(&self, i: [usize; 3]) -> usize {
        (i[0] * self.dims[1] + i[1]) * self.dims[2] + i[2]
    }

    fn voxel_of(&self, p: [f32; 3]) -> Option<[usize; 3]> {
        let mut out = [0; 3];
        for k in 0..3 {
            let v = ((p[k] - self.lo[k]) / self.voxel).floor();
            if !(v >= 0.0 && (v as usize) < self.dims[k]) {
                return None;
            }
            out[k] = v as usize;
        }
        Some(out)
    }

    fn index_of(&self, p: [f32; 3]) -> Option<usize> {
        self.voxel_of(p).map(|i| self.index(i))
    }

    fn centre_of(&self, i: [usize; 3]) -> [f32; 3] {
        [0, 1, 2].map(|k| self.lo[k] + (i[k] as f32 + 0.5) * self.voxel)
    }

    pub fn solid_at(&self, i: [usize; 3]) -> bool {
        self.points[self.index(i)] >= SOLID
    }

    fn open_at(&self, i: [usize; 3]) -> bool {
        self.rays[self.index(i)] > 0 && !self.solid_at(i)
    }

    /// Rays from each flight to what it saw (from the two nearest camera positions to each solid voxel), stopped by anything
    /// solid on the way (walls one voxel thicker, so a sparse wall still stops them): what they crossed was open.
    fn carve(&mut self) {
        let cameras: Vec<[f32; 3]> = self.captures.iter().flat_map(|c| thin(&c.path, 160)).collect();
        if cameras.is_empty() {
            return;
        }
        let [dx, dy, dz] = self.dims;
        let solid: Vec<bool> = self.points.iter().map(|&n| n >= SOLID).collect();
        let mut stop = solid.clone();
        for x in 0..dx {
            for y in 0..dy {
                for z in 0..dz {
                    if !solid[(x * dy + y) * dz + z] {
                        continue;
                    }
                    for (ox, oy, oz) in [(-1i64, 0i64, 0i64), (1, 0, 0), (0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1)] {
                        let (nx, ny, nz) = (x as i64 + ox, y as i64 + oy, z as i64 + oz);
                        if nx >= 0 && ny >= 0 && nz >= 0 && (nx as usize) < dx && (ny as usize) < dy && (nz as usize) < dz {
                            stop[(nx as usize * dy + ny as usize) * dz + nz as usize] = true;
                        }
                    }
                }
            }
        }
        let targets: Vec<usize> = (0..solid.len()).filter(|&i| solid[i]).collect();
        let every = (targets.len() / 200_000).max(1);
        let step = self.voxel * 0.5;
        for &t in targets.iter().step_by(every) {
            let (x, rest) = (t / (dy * dz), t % (dy * dz));
            let p = self.centre_of([x, rest / dz, rest % dz]);
            let mut best = [(f32::MAX, 0usize); 2];
            for (k, c) in cameras.iter().enumerate() {
                let d = (0..3).map(|j| (p[j] - c[j]).powi(2)).sum::<f32>();
                if d < best[0].0 {
                    best = [(d, k), best[0]];
                } else if d < best[1].0 {
                    best[1] = (d, k);
                }
            }
            for &(d2, k) in &best {
                if d2 == f32::MAX {
                    continue;
                }
                let c = cameras[k];
                let length = d2.sqrt();
                let dir = [0, 1, 2].map(|j| (p[j] - c[j]) / length.max(1e-6));
                let mut s = 0.0;
                while length - s > self.voxel * 1.5 {
                    let q = [0, 1, 2].map(|j| c[j] + dir[j] * s);
                    let Some(i) = self.index_of(q) else { break };
                    if stop[i] && s > self.voxel {
                        break;
                    }
                    self.rays[i] = self.rays[i].saturating_add(1);
                    s += step;
                }
            }
        }
    }

    /// The plan: per cell the ground (grown from where the avatar stood, step by step, so stairs and slopes keep their own),
    /// then what blocks a body over it, else floor, open space or nothing known.
    fn plan(&mut self) {
        let per = (self.cell / self.voxel).round().max(1.0) as usize;
        let pw = self.dims[0].div_ceil(per);
        let pd = self.dims[2].div_ceil(per);
        self.plan_dims = [pw, pd];
        let dy = self.dims[1];
        // Seen tops in each cell: a solid voxel with no solid one over it.
        let mut tops: Vec<Vec<f32>> = vec![Vec::new(); pw * pd];
        for cx in 0..pw {
            for cz in 0..pd {
                let mut heights = Vec::new();
                for x in cx * per..((cx + 1) * per).min(self.dims[0]) {
                    for z in cz * per..((cz + 1) * per).min(self.dims[2]) {
                        for y in 0..dy {
                            if self.solid_at([x, y, z]) && (y + 1 >= dy || !self.solid_at([x, y + 1, z])) {
                                heights.push(self.lo[1] + (y + 1) as f32 * self.voxel);
                            }
                        }
                    }
                }
                heights.sort_by(f32::total_cmp);
                heights.dedup_by(|a, b| (*a - *b).abs() < self.voxel * 1.5);
                tops[cx * pd + cz] = heights;
            }
        }
        self.floor = self.floor_level(&tops);
        // Grow the ground from the seed: the avatar's cell, else the first picture's.
        let seed_at = self.captures.iter().rev().find_map(|c| c.avatar.map(|a| a.hips)).unwrap_or_else(|| self.captures.first().map_or([0.0; 3], |c| c.centre));
        let mut ground = vec![f32::NAN; pw * pd];
        let near = |list: &[f32], to: f32, within: f32| list.iter().copied().filter(|h| (h - to).abs() <= within).min_by(|a, b| (a - to).abs().total_cmp(&(b - to).abs()));
        if let Some(seed) = self.plan_cell([seed_at[0], seed_at[2]]) {
            let mut queue = VecDeque::new();
            let first = near(&tops[seed], self.floor, 0.5).unwrap_or(self.floor);
            ground[seed] = first;
            queue.push_back(seed);
            while let Some(c) = queue.pop_front() {
                let (cx, cz) = (c / pd, c % pd);
                for (ox, oz) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                    let (nx, nz) = (cx as i64 + ox, cz as i64 + oz);
                    if nx < 0 || nz < 0 || nx as usize >= pw || nz as usize >= pd {
                        continue;
                    }
                    let n = nx as usize * pd + nz as usize;
                    if !ground[n].is_nan() {
                        continue;
                    }
                    if let Some(h) = near(&tops[n], ground[c], STEP_M) {
                        // Not onto something a body could not stand beside: a top with solid right over it is a wall's side.
                        ground[n] = h;
                        queue.push_back(n);
                    }
                }
            }
        }
        // Cells the growing did not reach: the seen top nearest the floor level, within a storey.
        for c in 0..pw * pd {
            if ground[c].is_nan() {
                if let Some(h) = near(&tops[c], self.floor, 1.2) {
                    ground[c] = h;
                }
            }
        }
        let mut cells = vec![Cell::Unknown; pw * pd];
        let mut raised = vec![f32::NAN; pw * pd];
        for cx in 0..pw {
            for cz in 0..pd {
                let c = cx * pd + cz;
                let g = if ground[c].is_nan() { self.floor } else { ground[c] };
                let (mut blocked, mut open) = (false, 0);
                for x in cx * per..((cx + 1) * per).min(self.dims[0]) {
                    for z in cz * per..((cz + 1) * per).min(self.dims[2]) {
                        let y0 = ((g + BODY.0 - self.lo[1]) / self.voxel).floor().max(0.0) as usize;
                        let y1 = (((g + BODY.1 - self.lo[1]) / self.voxel).ceil().max(0.0) as usize).min(dy);
                        for y in y0..y1 {
                            if self.solid_at([x, y, z]) {
                                blocked = true;
                            } else if self.open_at([x, y, z]) {
                                open += 1;
                            }
                        }
                    }
                }
                // A seen floor with nothing solid over it is floor; open space seen without its floor is open.
                cells[c] = if blocked {
                    Cell::Blocked
                } else if !ground[c].is_nan() {
                    Cell::Floor
                } else if open >= per {
                    Cell::Open
                } else {
                    Cell::Unknown
                };
                if let Some(h) = tops[c].iter().copied().filter(|h| *h > g + 2.2).find(|h| {
                    let i = self.voxel_of([self.lo[0] + (cx * per) as f32 * self.voxel, h + 1.0, self.lo[2] + (cz * per) as f32 * self.voxel]);
                    i.is_some_and(|i| !self.solid_at(i))
                }) {
                    raised[c] = h;
                }
            }
        }
        self.cells = cells;
        self.ground = ground;
        self.raised = raised;
    }

    /// The floor the avatar stands on: under its hips the most solid layer between half a metre and 1.6 m below them
    /// (around the hips, not the avatar's own body), else hips less their usual height; with no avatar, the most common
    /// seen top under the first picture's flight.
    fn floor_level(&self, tops: &[Vec<f32>]) -> f32 {
        if let Some((a, eye)) = self.captures.iter().rev().find_map(|c| c.avatar.map(|a| (a, c.eye_height))) {
            let mut layers: HashMap<i32, u32> = HashMap::new();
            for c in &self.captures {
                for v in &c.voxels {
                    let p = [0, 1, 2].map(|k| (v[k] as f32 + 0.5) * VOXEL);
                    let r = ((p[0] - a.hips[0]).powi(2) + (p[2] - a.hips[2]).powi(2)).sqrt();
                    if (0.8..4.0).contains(&r) && p[1] < a.hips[1] - 0.5 && p[1] > a.hips[1] - 1.6 {
                        *layers.entry(v[1]).or_default() += v[3] as u32;
                    }
                }
            }
            if let Some((&y, &n)) = layers.iter().max_by_key(|(y, n)| (**n, **y)) {
                if n >= 8 {
                    return (y + 1) as f32 * VOXEL;
                }
            }
            return a.hips[1] - HIPS_PER_EYE * eye;
        }
        let Some(first) = self.captures.first() else { return 0.0 };
        let below = first.path.iter().map(|p| p[1]).fold(f32::MAX, f32::min) - first.eye_height;
        let mut counts: HashMap<i32, u32> = HashMap::new();
        if let Some(c) = self.plan_cell([first.centre[0], first.centre[2]]) {
            let (cx, cz) = (c / self.plan_dims[1], c % self.plan_dims[1]);
            for x in cx.saturating_sub(10)..(cx + 10).min(self.plan_dims[0]) {
                for z in cz.saturating_sub(10)..(cz + 10).min(self.plan_dims[1]) {
                    for h in &tops[x * self.plan_dims[1] + z] {
                        if *h < first.centre[1] - 0.5 {
                            *counts.entry((h / VOXEL).round() as i32).or_default() += 1;
                        }
                    }
                }
            }
        }
        counts.into_iter().max_by_key(|(h, n)| (*n, -(((*h as f32 * VOXEL) - below).abs() * 100.0) as i64)).map_or(below, |(h, _)| h as f32 * VOXEL)
    }

    /// The plan cell of a point (x, z).
    pub fn plan_cell(&self, p: [f32; 2]) -> Option<usize> {
        let x = ((p[0] - self.lo[0]) / self.cell).floor();
        let z = ((p[1] - self.lo[2]) / self.cell).floor();
        (x >= 0.0 && z >= 0.0 && (x as usize) < self.plan_dims[0] && (z as usize) < self.plan_dims[1]).then(|| x as usize * self.plan_dims[1] + z as usize)
    }

    /// The middle of a plan cell (x, z).
    pub fn cell_centre(&self, c: usize) -> [f32; 2] {
        let (x, z) = (c / self.plan_dims[1], c % self.plan_dims[1]);
        [self.lo[0] + (x as f32 + 0.5) * self.cell, self.lo[2] + (z as f32 + 0.5) * self.cell]
    }

    /// The ground under a cell, or the floor level.
    pub fn ground_at(&self, c: usize) -> f32 {
        if self.ground[c].is_nan() {
            self.floor
        } else {
            self.ground[c]
        }
    }
}

/// What the map says of a point (a camera pose).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PointCheck {
    /// `free`: the pictures saw open space there; `near_solid`: open but within half a metre of something solid; `solid`:
    /// inside something the pictures saw (a wall, an object); `unknown`: inside the captured area but no picture saw it;
    /// `outside`: beyond everything captured (the sky, the void, or a part of the world no picture reached).
    pub at: &'static str,
    /// Metres down to the first solid thing under it (none: nothing seen under it).
    #[serde(serialize_with = "tenths::option")]
    pub ground_below_m: Option<f32>,
    /// Metres up to the first solid thing over it.
    #[serde(serialize_with = "tenths::option")]
    pub ceiling_above_m: Option<f32>,
    /// The nearest solid thing, when within 2 m.
    #[serde(serialize_with = "tenths::option")]
    pub nearest_solid_m: Option<f32>,
    /// Something solid stands between the nearest picture's centre and it: it is behind a wall or an object from where the
    /// pictures were taken (another room, the outside), which the camera may pass through.
    pub behind_something: bool,
    pub note: String,
}

fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

/// A metre or degree figure as an answer gives it: to a tenth, without f32's tail (1.6, not 1.600000023841858).
pub fn tenth(v: f32) -> f64 {
    (f64::from(v) * 10.0).round() / 10.0
}

/// Serialising f32 figures as [`tenth`]s.
mod tenths {
    use serde::ser::{SerializeSeq, Serializer};

    pub fn one<S: Serializer>(v: &f32, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(super::tenth(*v))
    }

    pub fn option<S: Serializer>(v: &Option<f32>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(v) => s.serialize_some(&super::tenth(*v)),
            None => s.serialize_none(),
        }
    }

    pub fn array<S: Serializer, const N: usize>(v: &[f32; N], s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(N))?;
        for x in v {
            seq.serialize_element(&super::tenth(*x))?;
        }
        seq.end()
    }
}

impl Map {
    pub fn check_point(&self, p: [f32; 3]) -> PointCheck {
        let Some(i) = self.voxel_of(p) else {
            return PointCheck {
                at: "outside",
                ground_below_m: None,
                ceiling_above_m: None,
                nearest_solid_m: None,
                behind_something: false,
                note: "Beyond everything the pictures captured: the sky, the void, or a part of the world no picture reached. Look with a screenshot.".into(),
            };
        };
        let column = |up: bool| -> Option<f32> {
            let mut y = i[1] as i64;
            loop {
                y += if up { 1 } else { -1 };
                if y < 0 || y as usize >= self.dims[1] {
                    return None;
                }
                if self.solid_at([i[0], y as usize, i[2]]) {
                    let edge = self.lo[1] + (y as f32 + if up { 0.0 } else { 1.0 }) * self.voxel;
                    return Some(round1((edge - p[1]).abs()));
                }
            }
        };
        let reach = (2.0 / self.voxel).ceil() as i64;
        let mut nearest = f32::MAX;
        for x in -reach..=reach {
            for y in -reach..=reach {
                for z in -reach..=reach {
                    let j = [i[0] as i64 + x, i[1] as i64 + y, i[2] as i64 + z];
                    if (0..3).any(|k| j[k] < 0 || j[k] as usize >= self.dims[k]) {
                        continue;
                    }
                    let j = j.map(|v| v as usize);
                    if self.solid_at(j) {
                        let c = self.centre_of(j);
                        let d = ((0..3).map(|k| (c[k] - p[k]).powi(2)).sum::<f32>()).sqrt() - self.voxel * 0.5;
                        nearest = nearest.min(d.max(0.0));
                    }
                }
            }
        }
        let behind = self.captures.iter().map(|c| c.centre).min_by(|a, b| dist3(*a, p).total_cmp(&dist3(*b, p))).is_some_and(|c| self.solid_between(c, p));
        let (at, note) = if self.solid_at(i) {
            ("solid", "Inside something the pictures saw as solid (a wall, an object, the ground): the camera would show its inside. Move it.")
        } else if self.open_at(i) && nearest < 0.5 {
            ("near_solid", "Open, but within half a metre of something solid: the view may be filled by it.")
        } else if self.open_at(i) {
            ("free", "The pictures saw open space here.")
        } else {
            ("unknown", "Inside the captured area, but no picture saw this spot: it may be open, or inside something. Look with a screenshot.")
        };
        PointCheck {
            at,
            ground_below_m: column(false),
            ceiling_above_m: column(true),
            nearest_solid_m: (nearest < 2.0).then(|| round1(nearest)),
            behind_something: behind,
            note: note.into(),
        }
    }

    /// Whether something solid lies on the straight line from `a` to `b` (their own ends left out).
    pub fn solid_between(&self, a: [f32; 3], b: [f32; 3]) -> bool {
        let length = dist3(a, b);
        let mut s = self.voxel * 2.0;
        while s < length - self.voxel * 2.0 {
            let q = [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * s / length);
            if self.voxel_of(q).is_some_and(|i| self.solid_at(i)) {
                return true;
            }
            s += self.voxel * 0.5;
        }
        false
    }

    /// Walking `distance` metres from `from` (x, z) along `heading`, over the plan: how far it is clear, and what stops it.
    pub fn check_walk(&self, from: [f32; 2], heading: f32, distance: f32) -> WalkCheck {
        let f = forward(heading);
        let mut s = 0.0;
        let mut last = from;
        while s <= distance {
            let p = [from[0] + f[0] * s, from[1] + f[1] * s];
            let cell = self.plan_cell(p).map(|c| self.cells[c]);
            match cell {
                Some(Cell::Floor | Cell::Open) => last = p,
                stop => {
                    let what = match stop {
                        Some(Cell::Blocked) => "blocked",
                        Some(_) => "unknown",
                        None => "outside",
                    };
                    // The avatar's own cell is never what stops it.
                    if s > 0.3 {
                        return WalkCheck { clear_m: round1(s), stops: Some(what), end: [round1(last[0]), round1(last[1])] };
                    }
                }
            }
            s += self.cell * 0.5;
        }
        let end = [from[0] + f[0] * distance, from[1] + f[1] * distance];
        WalkCheck { clear_m: round1(distance), stops: None, end: [round1(end[0]), round1(end[1])] }
    }
}

fn dist3(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>()).sqrt()
}

fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WalkCheck {
    /// Metres the plan shows clear ahead (up to the distance asked).
    #[serde(serialize_with = "tenths::one")]
    pub clear_m: f32,
    /// What ends it: `blocked` (a wall, furniture, a plant), `unknown` (no picture saw past there), `outside` (beyond
    /// everything captured); none: clear all the way.
    pub stops: Option<&'static str>,
    /// Where the clear part ends, (x, z).
    #[serde(serialize_with = "tenths::array")]
    pub end: [f32; 2],
}

/// A place worth the next picture.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Suggestion {
    pub id: char,
    /// What makes it promising, in a few words.
    pub why: String,
    /// The camera's pose there: x, y, z, pitch (positive looks down), heading.
    #[serde(serialize_with = "tenths::array")]
    pub pose: [f32; 5],
    /// The floor cell under it (x, z), where the avatar would stand.
    #[serde(serialize_with = "tenths::array")]
    pub stand: [f32; 2],
    /// From the nearest earlier picture's centre, metres.
    #[serde(serialize_with = "tenths::one")]
    pub from_centres_m: f32,
    pub kind: &'static str,
    #[serde(skip)]
    pub score: f32,
}

/// What the plan says of the world's size and simplicity.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Size {
    /// Width (x) and depth (z) of what was seen, metres.
    #[serde(serialize_with = "tenths::array")]
    pub extent_m: [f32; 2],
    /// From the floor to the highest thing seen, metres.
    #[serde(serialize_with = "tenths::one")]
    pub height_m: f32,
    /// Floor and open space seen, square metres.
    #[serde(serialize_with = "tenths::one")]
    pub walkable_m2: f32,
    /// Open areas apart from each other (rooms, or parts joined only by narrow doors).
    pub areas: usize,
    /// Openings from seen open space into what no picture saw: doors, corridors, the edge of an outdoor space.
    pub openings: usize,
    /// How much of the seen space's edge is wall rather than unknown (1: closed all round).
    #[serde(serialize_with = "tenths::one")]
    pub closed: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Verdict {
    /// How many pictures the world deserves by the rule: 1 for a very simple world, else 3 (and more when there is clearly more).
    pub pictures: u32,
    pub simple: bool,
    pub why: String,
}

impl Map {
    /// Open areas: walkable cells shrunk by half a metre (so a door narrower than about 1.5 m splits two rooms), connected.
    /// Returns each walkable cell's area (usize::MAX: none) and the areas' sizes in cells.
    pub fn areas(&self) -> (Vec<usize>, Vec<usize>) {
        let [pw, pd] = self.plan_dims;
        let walk: Vec<bool> = self.cells.iter().map(|c| c.walkable()).collect();
        let core: Vec<bool> = (0..pw * pd)
            .map(|c| {
                let (x, z) = (c / pd, c % pd);
                walk[c] && neighbours8(x, z, pw, pd).all(|n| walk[n])
            })
            .collect();
        let mut area = vec![usize::MAX; pw * pd];
        let mut sizes = Vec::new();
        for start in 0..pw * pd {
            if !core[start] || area[start] != usize::MAX {
                continue;
            }
            let id = sizes.len();
            let mut n = 0;
            let mut queue = VecDeque::from([start]);
            area[start] = id;
            while let Some(c) = queue.pop_front() {
                n += 1;
                for m in neighbours4(c / pd, c % pd, pw, pd) {
                    if core[m] && area[m] == usize::MAX {
                        area[m] = id;
                        queue.push_back(m);
                    }
                }
            }
            sizes.push(n);
        }
        // The rim cells take their nearest core's area.
        let mut queue: VecDeque<usize> = (0..pw * pd).filter(|&c| area[c] != usize::MAX).collect();
        while let Some(c) = queue.pop_front() {
            for m in neighbours4(c / pd, c % pd, pw, pd) {
                if walk[m] && area[m] == usize::MAX {
                    area[m] = area[c];
                    queue.push_back(m);
                }
            }
        }
        (area, sizes)
    }

    /// Unknown cells beside walkable ones, grouped into openings (by touch); each opening's cells.
    pub fn openings(&self) -> Vec<Vec<usize>> {
        let [pw, pd] = self.plan_dims;
        let frontier: Vec<bool> = (0..pw * pd).map(|c| self.cells[c] == Cell::Unknown && neighbours4(c / pd, c % pd, pw, pd).any(|n| self.cells[n].walkable())).collect();
        let mut seen = vec![false; pw * pd];
        let mut out = Vec::new();
        for start in 0..pw * pd {
            if !frontier[start] || seen[start] {
                continue;
            }
            let mut group = Vec::new();
            let mut queue = VecDeque::from([start]);
            seen[start] = true;
            while let Some(c) = queue.pop_front() {
                group.push(c);
                for m in neighbours8(c / pd, c % pd, pw, pd) {
                    if frontier[m] && !seen[m] {
                        seen[m] = true;
                        queue.push_back(m);
                    }
                }
            }
            // A metre at least: one stray hole in a floor is not a way anywhere.
            if group.len() as f32 * self.cell >= 1.0 {
                out.push(group);
            }
        }
        out
    }

    pub fn size(&self) -> Size {
        let [pw, pd] = self.plan_dims;
        let known: Vec<usize> = (0..pw * pd).filter(|&c| self.cells[c] != Cell::Unknown).collect();
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for &c in &known {
            let p = self.cell_centre(c);
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let extent = if known.is_empty() { [0.0, 0.0] } else { [round1(hi[0] - lo[0] + self.cell), round1(hi[1] - lo[1] + self.cell)] };
        let walkable = self.cells.iter().filter(|c| c.walkable()).count() as f32 * self.cell * self.cell;
        let mut top = self.floor;
        for (i, &n) in self.points.iter().enumerate() {
            if n >= SOLID {
                let y = self.lo[1] + ((i / self.dims[2]) % self.dims[1] + 1) as f32 * self.voxel;
                top = top.max(y);
            }
        }
        let (_, sizes) = self.areas();
        let areas = sizes.iter().filter(|&&n| n as f32 * self.cell * self.cell >= 6.0).count();
        let openings = self.openings();
        // The edge of the walkable space: its cells' neighbours that are not walkable, wall or unknown.
        let (mut wall, mut unknown) = (0usize, 0usize);
        for c in 0..pw * pd {
            if !self.cells[c].walkable() {
                continue;
            }
            for n in neighbours4(c / pd, c % pd, pw, pd) {
                match self.cells[n] {
                    Cell::Blocked => wall += 1,
                    Cell::Unknown => unknown += 1,
                    _ => {}
                }
            }
        }
        Size {
            extent_m: extent,
            height_m: round1(top - self.floor),
            walkable_m2: walkable.round(),
            areas,
            openings: openings.len(),
            closed: if wall + unknown == 0 { 0.0 } else { (wall as f32 / (wall + unknown) as f32 * 100.0).round() / 100.0 },
        }
    }

    /// The rule for how many pictures: a small space closed all round with nothing in it is one picture; another area, an
    /// opening into the unseen, or a large space is more.
    pub fn verdict(&self, size: &Size) -> Verdict {
        let [pw, pd] = self.plan_dims;
        let walk = self.cells.iter().filter(|c| c.walkable()).count().max(1);
        // Things standing in the space (not its walls): blocked cells with walkable cells on two opposite sides.
        let furniture = (0..pw * pd)
            .filter(|&c| {
                let (x, z) = (c / pd, c % pd);
                self.cells[c] == Cell::Blocked
                    && x > 0
                    && z > 0
                    && x + 1 < pw
                    && z + 1 < pd
                    && ((self.cells[c - pd].walkable() && self.cells[c + pd].walkable()) || (self.cells[c - 1].walkable() && self.cells[c + 1].walkable()))
            })
            .count();
        let cluttered = furniture as f32 / walk as f32;
        let big = size.extent_m[0].max(size.extent_m[1]);
        let mut reasons = Vec::new();
        if size.areas > 1 {
            reasons.push(format!("{} separate areas", size.areas));
        }
        if size.openings > 0 && size.closed < 0.85 {
            reasons.push(format!("{} openings into parts no picture saw", size.openings));
        }
        if big > 30.0 {
            reasons.push(format!("{big:.0} m across"));
        }
        if size.height_m > 8.0 {
            reasons.push(format!("{:.0} m high", size.height_m));
        }
        if reasons.is_empty() && size.closed >= 0.7 && big <= 30.0 && cluttered < 0.03 {
            return Verdict {
                pictures: 1,
                simple: true,
                why: format!("A simple space: {:.0} by {:.0} m, closed all round ({:.0}% wall), little standing in it: one picture shows it.", size.extent_m[0], size.extent_m[1], size.closed * 100.0),
            };
        }
        if reasons.is_empty() {
            reasons.push(format!("{:.0} by {:.0} m with things standing in it", size.extent_m[0], size.extent_m[1]));
        }
        Verdict { pictures: 3, simple: false, why: format!("More to see: {}.", reasons.join(", ")) }
    }

    /// Up to three places for the next pictures, each at least [`APART_M`] from every earlier centre when the world is that
    /// large (else as far as it allows), apart from each other, in open space clear of walls: another area first, then an
    /// opening into the unseen, then a high point over the space.
    pub fn suggestions(&self, size: &Size) -> Vec<Suggestion> {
        let [pw, pd] = self.plan_dims;
        let centres: Vec<[f32; 3]> = self.captures.iter().map(|c| c.centre).collect();
        let (area, _) = self.areas();
        let pictured: Vec<usize> = centres.iter().filter_map(|c| self.nearest_walkable([c[0], c[2]])).map(|c| area[c]).collect();
        let openings = self.openings();
        let mut opening_of = vec![usize::MAX; pw * pd];
        for (k, g) in openings.iter().enumerate() {
            for &c in g {
                opening_of[c] = k;
            }
        }
        let clearance = self.clearance();
        let big = size.extent_m[0].max(size.extent_m[1]);
        let apart = if big >= APART_M * 1.4 { APART_M } else { (big * 0.5).max(2.0) };
        let mut candidates = Vec::new();
        for c in 0..pw * pd {
            if !self.cells[c].walkable() || clearance[c] < 2 {
                continue;
            }
            let at = self.cell_centre(c);
            let from = centres.iter().map(|k| dist2(at, [k[0], k[2]])).fold(f32::MAX, f32::min);
            if from < apart {
                continue;
            }
            let g = self.ground_at(c);
            let eye = g + self.captures.last().map_or(EYE_HEIGHT, |k| k.eye_height);
            let (seen, unseen, look) = self.view(at, eye, 20.0, &opening_of);
            let new_area = area[c] != usize::MAX && !pictured.contains(&area[c]);
            let mut score = seen as f32 * 0.05 + unseen.len() as f32 * 1.5 + from.min(20.0) * 0.5 + if new_area { 60.0 } else { 0.0 };
            if self.cells[c] == Cell::Open {
                score *= 0.8;
            }
            let (kind, why) = if new_area {
                ("another_area", "another area, apart from where the pictures were taken".to_string())
            } else if let Some(&k) = unseen.first() {
                let o = self.cell_centre(openings[k][openings[k].len() / 2]);
                ("opening", format!("faces an opening into unseen space {:.0} m away", dist2(at, o)))
            } else {
                ("far_side", "the far side of the space, looking back over it".to_string())
            };
            candidates.push(Suggestion { id: '?', why, pose: [at[0], eye, at[1], 3.0, look], stand: at, from_centres_m: round1(from), kind, score });
            // Over it: high enough to see over what stands around, where the pictures saw that space open.
            for lift in [4.0, 7.0] {
                let p = [at[0], g + lift, at[1]];
                if self.voxel_of(p).is_some_and(|i| self.open_at(i)) && self.check_point(p).nearest_solid_m.is_none_or(|d| d >= 1.0) {
                    let (seen, unseen, look) = self.view(at, g + lift, 30.0, &opening_of);
                    let score = seen as f32 * 0.08 + unseen.len() as f32 * 1.0 + from.min(20.0) * 0.5 + if new_area { 40.0 } else { 0.0 } + 5.0;
                    let pitch = (lift / 12.0).atan().to_degrees().clamp(10.0, 35.0).round();
                    candidates.push(Suggestion {
                        id: '?',
                        why: format!("a high point {lift:.0} m over the floor with a wide view"),
                        pose: [at[0], g + lift, at[1], pitch, look],
                        stand: at,
                        from_centres_m: round1(from),
                        kind: "high_point",
                        score,
                    });
                }
            }
        }
        candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
        let mut chosen: Vec<Suggestion> = Vec::new();
        let mut kinds_taken = Vec::new();
        // One of each kind first, then the best others.
        for pass in 0..2 {
            for c in &candidates {
                if chosen.len() >= 3 {
                    break;
                }
                if pass == 0 && kinds_taken.contains(&c.kind) {
                    continue;
                }
                if chosen.iter().any(|k| dist2(k.stand, c.stand) < apart.min(8.0)) {
                    continue;
                }
                kinds_taken.push(c.kind);
                chosen.push(c.clone());
            }
        }
        chosen.sort_by(|a, b| b.score.total_cmp(&a.score));
        for (k, s) in chosen.iter_mut().enumerate() {
            s.id = (b'A' + k as u8) as char;
            s.pose = s.pose.map(round1);
            s.stand = s.stand.map(round1);
        }
        chosen
    }

    /// From (x, z) at height `eye`: how many known cells it sees within `reach` metres (things lower than the eye do not
    /// block a view from over them), which openings it sees, and the heading to look along (towards the openings seen,
    /// else into the most of the space).
    fn view(&self, at: [f32; 2], eye: f32, reach: f32, opening_of: &[usize]) -> (usize, Vec<usize>, f32) {
        let mut seen = 0;
        let mut openings: Vec<(usize, usize)> = Vec::new();
        let mut best = (0usize, 0.0f32);
        for k in 0..72 {
            let h = k as f32 * 5.0;
            let f = forward(h);
            let mut along = 0;
            let mut s = self.cell;
            while s <= reach {
                let Some(c) = self.plan_cell([at[0] + f[0] * s, at[1] + f[1] * s]) else { break };
                match self.cells[c] {
                    Cell::Blocked if self.blocks_view(c, eye) => break,
                    Cell::Unknown => {
                        if opening_of[c] != usize::MAX {
                            match openings.iter_mut().find(|o| o.0 == opening_of[c]) {
                                Some(o) => o.1 += 1,
                                None => openings.push((opening_of[c], 1)),
                            }
                            along += 3;
                        }
                        break;
                    }
                    _ => {
                        seen += 1;
                        along += 1;
                    }
                }
                s += self.cell;
            }
            if along > best.0 {
                best = (along, h);
            }
        }
        openings.sort_by(|a, b| b.1.cmp(&a.1));
        (seen, openings.into_iter().map(|o| o.0).collect(), best.1)
    }

    /// A blocked cell hides what is behind it from `eye` when something in it stands higher than the eye.
    fn blocks_view(&self, c: usize, eye: f32) -> bool {
        let per = (self.cell / self.voxel).round().max(1.0) as usize;
        let (cx, cz) = (c / self.plan_dims[1], c % self.plan_dims[1]);
        let y0 = ((eye - 0.3 - self.lo[1]) / self.voxel).floor().max(0.0) as usize;
        for x in cx * per..((cx + 1) * per).min(self.dims[0]) {
            for z in cz * per..((cz + 1) * per).min(self.dims[2]) {
                if (y0..self.dims[1]).any(|y| self.solid_at([x, y, z])) {
                    return true;
                }
            }
        }
        false
    }

    /// Cells to the nearest blocked or unknown cell (4-neighbour steps), capped at 8.
    fn clearance(&self) -> Vec<u8> {
        let [pw, pd] = self.plan_dims;
        let mut d = vec![u8::MAX; pw * pd];
        let mut queue = VecDeque::new();
        for c in 0..pw * pd {
            if !self.cells[c].walkable() {
                d[c] = 0;
                queue.push_back(c);
            }
        }
        while let Some(c) = queue.pop_front() {
            if d[c] >= 8 {
                continue;
            }
            for n in neighbours4(c / pd, c % pd, pw, pd) {
                if d[n] == u8::MAX {
                    d[n] = d[c] + 1;
                    queue.push_back(n);
                }
            }
        }
        for c in 0..pw * pd {
            let (x, z) = (c / pd, c % pd);
            // The plan's edge is not known to be open.
            let edge = x.min(z).min(pw - 1 - x).min(pd - 1 - z) as u8;
            d[c] = d[c].min(edge + 1).min(8);
        }
        d
    }

    /// The walkable cell nearest a point (x, z), within 3 m.
    pub fn nearest_walkable(&self, p: [f32; 2]) -> Option<usize> {
        let c = self.plan_cell(p)?;
        if self.cells[c].walkable() {
            return Some(c);
        }
        let [pw, pd] = self.plan_dims;
        let r = (3.0 / self.cell) as i64;
        let (cx, cz) = ((c / pd) as i64, (c % pd) as i64);
        let mut best: Option<(i64, usize)> = None;
        for x in (cx - r).max(0)..=(cx + r).min(pw as i64 - 1) {
            for z in (cz - r).max(0)..=(cz + r).min(pd as i64 - 1) {
                let n = x as usize * pd + z as usize;
                let d = (x - cx).pow(2) + (z - cz).pow(2);
                if self.cells[n].walkable() && best.is_none_or(|b| d < b.0) {
                    best = Some((d, n));
                }
            }
        }
        best.map(|b| b.1)
    }

    /// A walk from (x, z) to (x, z) through cells the pictures saw walkable, kept clear of walls where it can be: the corners
    /// of the way, from the start. `None`: no seen way.
    pub fn route(&self, from: [f32; 2], to: [f32; 2]) -> Option<Vec<[f32; 2]>> {
        let start = self.nearest_walkable(from)?;
        let goal = self.nearest_walkable(to)?;
        let [pw, pd] = self.plan_dims;
        let clearance = self.clearance();
        let cost = |c: usize| -> Option<f32> {
            let base = match self.cells[c] {
                Cell::Floor => 1.0,
                Cell::Open => 1.6,
                _ => return None,
            };
            Some(base * if clearance[c] <= 1 { 4.0 } else { 1.0 })
        };
        let mut best = vec![f32::INFINITY; pw * pd];
        let mut came = vec![usize::MAX; pw * pd];
        let mut heap = BinaryHeap::new();
        best[start] = 0.0;
        let estimate = |c: usize| dist2(self.cell_centre(c), self.cell_centre(goal)) / self.cell;
        heap.push(Visit(-estimate(start), start));
        while let Some(Visit(_, c)) = heap.pop() {
            if c == goal {
                break;
            }
            let (x, z) = (c / pd, c % pd);
            for n in neighbours8(x, z, pw, pd) {
                let Some(step) = cost(n) else { continue };
                let (nx, nz) = (n / pd, n % pd);
                let diagonal = nx != x && nz != z;
                // No cutting a wall's corner.
                if diagonal && (!self.cells[nx * pd + z].walkable() || !self.cells[x * pd + nz].walkable()) {
                    continue;
                }
                let g = best[c] + step * if diagonal { std::f32::consts::SQRT_2 } else { 1.0 };
                if g < best[n] {
                    best[n] = g;
                    came[n] = c;
                    heap.push(Visit(-(g + estimate(n)), n));
                }
            }
        }
        if best[goal].is_infinite() {
            return None;
        }
        let mut cells = vec![goal];
        while *cells.last().unwrap() != start {
            cells.push(came[*cells.last().unwrap()]);
        }
        cells.reverse();
        // Straight lines where the plan shows them clear.
        let points: Vec<[f32; 2]> = cells.iter().map(|&c| self.cell_centre(c)).collect();
        let mut corners = vec![from];
        let mut i = 0;
        while i + 1 < points.len() {
            let mut j = points.len() - 1;
            while j > i + 1 && !self.clear_line(points[i], points[j]) {
                j -= 1;
            }
            corners.push(points[j]);
            i = j;
        }
        if dist2(*corners.last().unwrap(), to) > 0.1 && self.plan_cell(to) == Some(goal) {
            corners.push(to);
        }
        Some(corners)
    }

    fn clear_line(&self, a: [f32; 2], b: [f32; 2]) -> bool {
        let length = dist2(a, b);
        let mut s = 0.0;
        while s <= length {
            let p = [a[0] + (b[0] - a[0]) * s / length.max(1e-6), a[1] + (b[1] - a[1]) * s / length.max(1e-6)];
            // Half a body's width either side.
            for side in [-0.3, 0.0, 0.3] {
                let n = [-(b[1] - a[1]) / length.max(1e-6), (b[0] - a[0]) / length.max(1e-6)];
                let q = [p[0] + n[0] * side, p[1] + n[1] * side];
                if !self.plan_cell(q).is_some_and(|c| self.cells[c].walkable()) {
                    return false;
                }
            }
            s += self.cell * 0.5;
        }
        true
    }
}

#[derive(PartialEq)]
struct Visit(f32, usize);
impl Eq for Visit {}
impl PartialOrd for Visit {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Visit {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0).then(other.1.cmp(&self.1))
    }
}

fn neighbours4(x: usize, z: usize, w: usize, d: usize) -> impl Iterator<Item = usize> {
    [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)].into_iter().filter_map(move |(ox, oz)| {
        let (nx, nz) = (x as i64 + ox, z as i64 + oz);
        (nx >= 0 && nz >= 0 && (nx as usize) < w && (nz as usize) < d).then(|| nx as usize * d + nz as usize)
    })
}

fn neighbours8(x: usize, z: usize, w: usize, d: usize) -> impl Iterator<Item = usize> {
    (-1i64..=1).flat_map(|ox| (-1i64..=1).map(move |oz| (ox, oz))).filter(|&o| o != (0, 0)).filter_map(move |(ox, oz)| {
        let (nx, nz) = (x as i64 + ox, z as i64 + oz);
        (nx >= 0 && nz >= 0 && (nx as usize) < w && (nz as usize) < d).then(|| nx as usize * d + nz as usize)
    })
}

/// What a walk plan says to do: turn to a heading, then walk so far.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WalkStep {
    /// Degrees for `turn` (positive right), from the heading before.
    #[serde(serialize_with = "tenths::one")]
    pub turn_deg: f32,
    #[serde(serialize_with = "tenths::one")]
    pub heading: f32,
    #[serde(serialize_with = "tenths::one")]
    pub walk_m: f32,
    /// Seconds for `walk` forward at the speed assumed.
    #[serde(serialize_with = "tenths::one")]
    pub walk_s: f32,
}

/// A route's corners as turns and walks from a heading, at `speed` metres a second.
pub fn walk_steps(corners: &[[f32; 2]], mut heading: f32, speed: f32) -> Vec<WalkStep> {
    let mut steps = Vec::new();
    for w in corners.windows(2) {
        let (dx, dz) = (w[1][0] - w[0][0], w[1][1] - w[0][1]);
        let m = (dx * dx + dz * dz).sqrt();
        if m < 0.2 {
            continue;
        }
        let h = heading_of(dx, dz);
        steps.push(WalkStep { turn_deg: wrap(h - heading).round(), heading: h.round(), walk_m: round1(m), walk_s: round1(m / speed.max(0.3)) });
        heading = h;
    }
    steps
}

/// aim_camera's arguments that take a camera at `from` (x, y, z, pitch, heading) to `to` (x, y, z, pitch, heading), in calls of
/// at most `reach` metres each way.
pub fn aim_steps(from: [f32; 5], to: [f32; 5], reach: f32) -> Vec<serde_json::Value> {
    let (dx, dy, dz) = (to[0] - from[0], to[1] - from[1], to[2] - from[2]);
    let (s, c) = from[4].to_radians().sin_cos();
    let (f, r) = (dx * s + dz * c, dx * c - dz * s);
    let calls = ((f.abs().max(r.abs()).max(dy.abs())) / reach).ceil().max(1.0) as usize;
    (0..calls)
        .map(|k| {
            let mut call = serde_json::json!({
                "forward_m": tenth(f / calls as f32),
                "right_m": tenth(r / calls as f32),
                "up_m": tenth(dy / calls as f32),
            });
            if k == calls - 1 {
                call["turn_deg"] = serde_json::json!(tenth(wrap(to[4] - from[4]).round()));
                call["pitch_deg"] = serde_json::json!(tenth(to[3].round()));
            }
            call
        })
        .collect()
}

/// What goes on the picture over the plan.
#[derive(Default)]
pub struct Marks {
    /// The earlier pictures' centres, numbered from 1 in this order.
    pub centres: Vec<[f32; 3]>,
    /// VRChat's camera now: position and heading.
    pub camera: Option<([f32; 3], f32)>,
    /// The avatar: (x, z) and heading.
    pub avatar: Option<([f32; 2], f32)>,
    pub suggestions: Vec<Suggestion>,
    /// A walk to draw, corners (x, z).
    pub route: Vec<[f32; 2]>,
}

/// Colours of the picture, as `map_plan` explains them.
pub const LEGEND: &str = "light grey: floor seen; blue-grey: open space seen, its floor not; red: blocked (walls, furniture, plants); green tint: a raised surface to stand on (balcony, roof); near black: nothing seen (unknown, not free); yellow numbers: earlier pictures' centres; blue letters: suggestions, with a tick towards their heading; cyan arrow: VRChat's camera; pink arrow: the avatar; dotted yellow: the walk plan; up is +z (heading 0), right is +x (heading 90); the white bar is 10 m (5 m on a small map).";

const UNKNOWN: [u8; 3] = [24, 26, 32];
const BLOCKED: [u8; 3] = [214, 96, 72];
const FLOOR: [u8; 3] = [232, 232, 224];
const OPEN: [u8; 3] = [140, 156, 186];
const CENTRE: [u8; 3] = [250, 204, 21];
const SUGGESTION: [u8; 3] = [59, 130, 246];
const CAMERA: [u8; 3] = [34, 211, 238];
const AVATAR: [u8; 3] = [236, 72, 153];

/// 3 by 5 glyphs, rows top down, bits left to right.
fn glyph(c: char) -> Option<[u8; 5]> {
    Some(match c {
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [7, 1, 7, 4, 7],
        '3' => [7, 1, 7, 1, 7],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 7, 1, 7],
        '6' => [7, 4, 7, 5, 7],
        '7' => [7, 1, 2, 2, 2],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 7],
        'A' => [2, 5, 7, 5, 5],
        'B' => [6, 5, 6, 5, 6],
        'C' => [7, 4, 4, 4, 7],
        'D' => [6, 5, 5, 5, 6],
        'E' => [7, 4, 6, 4, 7],
        'F' => [7, 4, 6, 4, 4],
        'N' => [5, 7, 7, 7, 5],
        'm' => [0, 0, 7, 7, 5],
        _ => return None,
    })
}

struct Canvas {
    w: usize,
    h: usize,
    rgb: Vec<u8>,
}

impl Canvas {
    fn set(&mut self, x: i64, y: i64, c: [u8; 3]) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            let i = (y as usize * self.w + x as usize) * 3;
            self.rgb[i..i + 3].copy_from_slice(&c);
        }
    }

    fn disc(&mut self, x: i64, y: i64, r: i64, c: [u8; 3]) {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r * r {
                    self.set(x + dx, y + dy, c);
                }
            }
        }
    }

    fn line(&mut self, a: (f32, f32), b: (f32, f32), c: [u8; 3], dotted: bool) {
        let n = ((b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil() as usize).max(1);
        for k in 0..=n {
            if dotted && (k / 2) % 2 == 1 {
                continue;
            }
            let t = k as f32 / n as f32;
            let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            self.set(x.round() as i64, y.round() as i64, c);
            if !dotted {
                self.set(x.round() as i64 + 1, y.round() as i64, c);
            }
        }
    }

    fn text(&mut self, x: i64, y: i64, text: &str, scale: i64, c: [u8; 3]) {
        for (k, ch) in text.chars().enumerate() {
            let Some(g) = glyph(ch) else { continue };
            for (row, bits) in g.iter().enumerate() {
                for col in 0..3 {
                    if bits & (4 >> col) != 0 {
                        for sy in 0..scale {
                            for sx in 0..scale {
                                self.set(x + (k as i64 * 4 + col) * scale + sx, y + row as i64 * scale + sy, c);
                            }
                        }
                    }
                }
            }
        }
    }

    /// An arrow at (x, y) pointing along a heading (up is heading 0).
    fn arrow(&mut self, x: f32, y: f32, heading: f32, size: f32, c: [u8; 3]) {
        let f = forward(heading);
        let (fx, fy) = (f[0], -f[1]);
        let tip = (x + fx * size, y + fy * size);
        let back = (x - fx * size * 0.6, y - fy * size * 0.6);
        let side = (-fy * size * 0.6, fx * size * 0.6);
        self.line(back, tip, c, false);
        self.line(tip, (back.0 + side.0, back.1 + side.1), c, false);
        self.line(tip, (back.0 - side.0, back.1 - side.1), c, false);
        self.disc(x.round() as i64, y.round() as i64, 2, c);
    }
}

impl Map {
    /// A top-down picture of the plan with `marks`, PNG, its longer side about `size` pixels.
    pub fn picture(&self, marks: &Marks, size: u32) -> Vec<u8> {
        let [pw, pd] = self.plan_dims;
        // What was seen and what is marked, with a margin.
        let (mut lo, mut hi) = ([usize::MAX; 2], [0usize; 2]);
        let mut take = |c: usize| {
            let (x, z) = (c / pd, c % pd);
            lo = [lo[0].min(x), lo[1].min(z)];
            hi = [hi[0].max(x), hi[1].max(z)];
        };
        for c in 0..pw * pd {
            if self.cells[c] != Cell::Unknown {
                take(c);
            }
        }
        let marked: Vec<[f32; 2]> = marks
            .centres
            .iter()
            .map(|c| [c[0], c[2]])
            .chain(marks.camera.map(|(p, _)| [p[0], p[2]]))
            .chain(marks.avatar.map(|(p, _)| p))
            .chain(marks.suggestions.iter().map(|s| s.stand))
            .collect();
        for p in &marked {
            if let Some(c) = self.plan_cell(*p) {
                take(c);
            }
        }
        if lo[0] == usize::MAX {
            lo = [0, 0];
            hi = [pw.saturating_sub(1), pd.saturating_sub(1)];
        }
        let lo = [lo[0].saturating_sub(3), lo[1].saturating_sub(3)];
        let hi = [(hi[0] + 3).min(pw - 1), (hi[1] + 3).min(pd - 1)];
        let (cw, ch) = (hi[0] - lo[0] + 1, hi[1] - lo[1] + 1);
        let k = (size as usize / cw.max(ch)).clamp(1, 12);
        let mut canvas = Canvas { w: cw * k, h: ch * k, rgb: vec![0; cw * k * ch * k * 3] };
        for x in 0..cw {
            for z in 0..ch {
                let c = (lo[0] + x) * pd + lo[1] + z;
                let mut colour = match self.cells[c] {
                    Cell::Unknown => UNKNOWN,
                    Cell::Blocked => BLOCKED,
                    Cell::Floor => FLOOR,
                    Cell::Open => OPEN,
                };
                if !self.raised[c].is_nan() && self.cells[c] != Cell::Blocked {
                    colour = [colour[0] / 2 + 50, colour[1] / 2 + 100, colour[2] / 2 + 60];
                }
                let row = ch - 1 - z;
                for dy in 0..k {
                    for dx in 0..k {
                        canvas.set((x * k + dx) as i64, (row * k + dy) as i64, colour);
                    }
                }
            }
        }
        let x0 = self.lo[0] + lo[0] as f32 * self.cell;
        let z1 = self.lo[2] + (hi[1] + 1) as f32 * self.cell;
        let px = |p: [f32; 2]| ((p[0] - x0) / self.cell * k as f32, (z1 - p[1]) / self.cell * k as f32);
        let r = (k as i64 * 2).clamp(4, 9);
        let font = if r >= 7 { 2 } else { 1 };
        for w in marks.route.windows(2) {
            canvas.line(px(w[0]), px(w[1]), CENTRE, true);
        }
        for (n, c) in marks.centres.iter().enumerate() {
            let (x, y) = px([c[0], c[2]]);
            canvas.disc(x as i64, y as i64, r, CENTRE);
            let label = (n + 1).to_string();
            canvas.text(x as i64 - (label.len() as i64 * 4 * font) / 2 + font / 2, y as i64 - 2 * font, &label, font, [0, 0, 0]);
        }
        for s in &marks.suggestions {
            let (x, y) = px(s.stand);
            let f = forward(s.pose[4]);
            canvas.line((x, y), (x + f[0] * r as f32 * 2.2, y - f[1] * r as f32 * 2.2), SUGGESTION, false);
            canvas.disc(x as i64, y as i64, r, SUGGESTION);
            canvas.text(x as i64 - font * 3 / 2, y as i64 - 2 * font, &s.id.to_string(), font, [255, 255, 255]);
        }
        if let Some((p, h)) = marks.avatar {
            let (x, y) = px(p);
            canvas.arrow(x, y, h, r as f32 * 1.6, AVATAR);
        }
        if let Some((p, h)) = marks.camera {
            let (x, y) = px([p[0], p[2]]);
            canvas.arrow(x, y, h, r as f32 * 1.6, CAMERA);
        }
        // Scale bar and north.
        let metres = if cw.max(ch) as f32 * self.cell > 30.0 { 10.0 } else { 5.0 };
        let bar = (metres / self.cell * k as f32) as i64;
        let (bx, by) = (6, canvas.h as i64 - 8);
        for t in 0..3 {
            canvas.line((bx as f32, (by + t) as f32), ((bx + bar) as f32, (by + t) as f32), [255, 255, 255], false);
        }
        canvas.text(bx + bar + 4, by - 3, &format!("{metres:.0}m"), 1, [255, 255, 255]);
        canvas.text(canvas.w as i64 / 2 - 2, 3, "N", 2, [255, 255, 255]);
        encode_png(canvas.w as u32, canvas.h as u32, &canvas.rgb)
    }
}

fn encode_png(w: u32, h: u32, rgb: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, w, h);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    if let Ok(mut writer) = encoder.write_header() {
        let _ = writer.write_image_data(rgb);
    }
    bytes
}

#[cfg(test)]
#[path = "map_tests.rs"]
pub(crate) mod tests;
