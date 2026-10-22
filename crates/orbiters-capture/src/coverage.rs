//! What the depth frames have seen so far, from above: the capture page's coverage map and its hints. Every known depth
//! sample (every second one along both axes) lands in a 0.5 m cell of the horizontal plane (Unity world x, z), which
//! remembers from which of 16 compass sectors and from how high above it the camera saw it, and at which heights its
//! points lie. A surface seen from one side only, or a floor seen only at grazing angles from afar, reconstructs into
//! smears from any other viewpoint: the hints name the nearest such place, and a scan flight
//! (orbiters-desktop's controller) keeps clear of what the cells say is in the way.

use orbiters_contracts::coords::intrinsics_from_projection;
use orbiters_contracts::depth::{self, DepthLayout};
use orbiters_core::protocol::{coverage as cell_bits, CoverageMap};
use std::collections::HashMap;

/// Cell size on the horizontal plane, metres.
pub const CELL_M: f32 = 0.5;
const SECTORS: u32 = 16;
/// Height slices of 0.25 m, from 4 m below the first camera position to 4 m above it.
const SLICE_M: f32 = 0.25;
const SLICES: i32 = 32;
/// The largest map sent: cells are merged two by two until the seen area fits.
pub const MAX_CELLS: u32 = 64;

#[derive(Clone, Copy, Default)]
struct Cell {
    points: u32,
    /// Bit s: seen from compass sector s (the direction from the point towards the camera).
    sectors: u16,
    /// Bit k: seen from elevation class k ([`cell_bits`]).
    angles: u8,
    /// Bit k: a point in height slice k.
    slices: u32,
}

pub struct Coverage {
    cells: HashMap<(i32, i32), Cell>,
    /// Height of slice 0's bottom (4 m under the first camera position).
    base_y: Option<f32>,
    /// Camera positions of the depth frames, every one.
    path: Vec<[f32; 3]>,
}

fn cell_of(x: f32, z: f32) -> (i32, i32) {
    ((x / CELL_M).floor() as i32, (z / CELL_M).floor() as i32)
}

impl Coverage {
    pub fn new() -> Self {
        Self { cells: HashMap::new(), base_y: None, path: Vec::new() }
    }

    /// Adds a depth frame's known samples: `camera_to_world` and `projection` from its strip, `codes` its decoded
    /// grid (0 = no depth, [`depth::UNKNOWN`] = unread).
    pub fn add_depth(&mut self, camera_to_world: &[f32; 12], projection: &[f32; 4], layout: &DepthLayout, grid_width: u32, codes: &[u16]) {
        let m = camera_to_world;
        let (right, up, forward, pos) = ([m[0], m[4], m[8]], [m[1], m[5], m[9]], [m[2], m[6], m[10]], [m[3], m[7], m[11]]);
        let k = intrinsics_from_projection(projection, layout.width, layout.height);
        let base = *self.base_y.get_or_insert(pos[1] - SLICES as f32 * SLICE_M / 2.0);
        self.path.push(pos);
        let grid_height = codes.len() as u32 / grid_width.max(1);
        for gy in (0..grid_height).step_by(2) {
            for gx in (0..grid_width).step_by(2) {
                let Some(z) = depth::dequantize(codes[(gy * grid_width + gx) as usize]) else { continue };
                let (u, v) = layout.grid_pixel(gx, gy);
                let (x, y) = ((u as f32 - k.cx) / k.fx * z, (v as f32 - k.cy) / k.fy * z);
                let p = [0, 1, 2].map(|i| pos[i] + right[i] * x - up[i] * y + forward[i] * z);
                let (dx, dz) = (pos[0] - p[0], pos[2] - p[2]);
                let flat = (dx * dx + dz * dz).sqrt();
                // Compass sector of the camera seen from the point (0 = +Z, clockwise towards +X).
                let sector = ((dx.atan2(dz).to_degrees().rem_euclid(360.0) / (360.0 / SECTORS as f32)) as u32).min(SECTORS - 1);
                let elevation = (pos[1] - p[1]).atan2(flat.max(1e-3)).to_degrees();
                let angle = if elevation < -10.0 {
                    cell_bits::FROM_BELOW
                } else if elevation < 10.0 {
                    cell_bits::GRAZING
                } else if elevation < 35.0 {
                    cell_bits::LOW
                } else if elevation < 60.0 {
                    cell_bits::HIGH
                } else {
                    cell_bits::OVERHEAD
                };
                let slice = ((p[1] - base) / SLICE_M).floor() as i32;
                let c = self.cells.entry(cell_of(p[0], p[2])).or_default();
                c.points += 1;
                c.sectors |= 1 << sector;
                c.angles |= angle;
                if (0..SLICES).contains(&slice) {
                    c.slices |= 1 << slice;
                }
            }
        }
    }

    /// The floor under the walked path: the lowest height slice under the cameras that most cells near them share.
    fn floor_slice(&self) -> Option<i32> {
        let base = self.base_y?;
        let mut votes = [0u32; SLICES as usize];
        let mut seen = std::collections::HashSet::new();
        for p in self.path.iter().step_by((self.path.len() / 200).max(1)) {
            let (cx, cz) = cell_of(p[0], p[2]);
            let camera_slice = ((p[1] - base) / SLICE_M).floor() as i32;
            for dz in -4..=4 {
                for dx in -4..=4 {
                    if !seen.insert((cx + dx, cz + dz)) {
                        continue;
                    }
                    let Some(c) = self.cells.get(&(cx + dx, cz + dz)) else { continue };
                    let below = c.slices & ((1u64 << camera_slice.clamp(0, SLICES)) - 1) as u32;
                    if below != 0 {
                        votes[below.trailing_zeros() as usize] += 1;
                    }
                }
            }
        }
        let (slice, &n) = votes.iter().enumerate().max_by_key(|(_, n)| **n)?;
        (n > 0).then_some(slice as i32)
    }

    /// The map around what was seen and walked, at most [`MAX_CELLS`] a side, with hints for a camera at `camera`
    /// heading `heading_deg` (0 = +Z, clockwise towards +X).
    pub fn map(&self, camera: Option<[f32; 3]>, heading_deg: Option<f32>, seconds: f64) -> Option<CoverageMap> {
        if self.cells.is_empty() {
            return None;
        }
        let floor = self.floor_slice();
        let base = self.base_y.unwrap_or(0.0);
        // Obstacles: points from 0.5 m to 2.5 m above the floor (where a camera at eye height moves).
        let obstacle_mask: u32 = match floor {
            Some(f) => (f + 2..(f + 10).min(SLICES)).fold(0u32, |m, s| if s >= 0 { m | 1 << s } else { m }),
            None => 0,
        };
        let floor_mask: u32 = floor.map_or(0, |f| (f.max(0)..(f + 2).min(SLICES)).fold(0u32, |m, s| m | 1 << s));
        let (mut x0, mut z0, mut x1, mut z1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        let mut grow = |c: (i32, i32)| {
            x0 = x0.min(c.0);
            z0 = z0.min(c.1);
            x1 = x1.max(c.0);
            z1 = z1.max(c.1);
        };
        for (&c, cell) in &self.cells {
            if cell.points >= 2 {
                grow(c);
            }
        }
        for p in &self.path {
            grow(cell_of(p[0], p[2]));
        }
        let mut merge = 1i32;
        while ((x1 - x0 + 1) / merge > MAX_CELLS as i32) || ((z1 - z0 + 1) / merge > MAX_CELLS as i32) {
            merge *= 2;
        }
        // Cropped around the camera when even merged cells do not fit (they always do below 1 km).
        let (gx0, gz0) = (x0.div_euclid(merge), z0.div_euclid(merge));
        let width = (x1.div_euclid(merge) - gx0 + 1).min(MAX_CELLS as i32) as u32;
        let depth = (z1.div_euclid(merge) - gz0 + 1).min(MAX_CELLS as i32) as u32;
        let mut sectors = vec![0u16; (width * depth) as usize];
        let mut bits = vec![0u8; (width * depth) as usize];
        for (&(cx, cz), cell) in &self.cells {
            if cell.points < 2 {
                continue;
            }
            let (ix, iz) = (cx.div_euclid(merge) - gx0, cz.div_euclid(merge) - gz0);
            if ix < 0 || iz < 0 || ix >= width as i32 || iz >= depth as i32 {
                continue;
            }
            let i = (iz as u32 * width + ix as u32) as usize;
            sectors[i] |= cell.sectors;
            bits[i] |= cell.angles;
            if cell.slices & obstacle_mask != 0 {
                bits[i] |= cell_bits::OBSTACLE;
            }
            if cell.slices & floor_mask != 0 {
                bits[i] |= cell_bits::FLOOR;
            }
        }
        let mut cells = Vec::with_capacity(2 * sectors.len());
        for (s, b) in sectors.iter().zip(&bits) {
            cells.push(s.count_ones() as u8);
            cells.push(*b);
        }
        let cell_m = CELL_M * merge as f32;
        Some(CoverageMap {
            cell_m,
            x0: gx0 as f32 * cell_m,
            z0: gz0 as f32 * cell_m,
            width,
            depth,
            cells,
            floor_y: floor.map(|f| base + f as f32 * SLICE_M),
            hints: self.hints(camera, heading_deg, seconds, floor_mask),
        })
    }

    /// Up to three things to do next, the most useful first.
    fn hints(&self, camera: Option<[f32; 3]>, heading_deg: Option<f32>, seconds: f64, floor_mask: u32) -> Vec<String> {
        let mut out = Vec::new();
        let (Some(cam), Some(heading)) = (camera, heading_deg) else { return out };
        // Things within 12 m (more than floor) seen from at most two of the 16 compass sectors, by direction from the
        // camera. (A floor seen along the walk is one-sided too; the grazing test below covers floors.)
        let mut one_sided = [(0u32, 0f32); 8];
        // Floor near the walked path seen only at grazing angles.
        let mut grazing_floor = 0u32;
        let near_path = |x: f32, z: f32| self.path.iter().step_by((self.path.len() / 100).max(1)).any(|p| (p[0] - x).powi(2) + (p[2] - z).powi(2) < 9.0);
        for (&(cx, cz), cell) in &self.cells {
            if cell.points < 8 {
                continue;
            }
            let (x, z) = ((cx as f32 + 0.5) * CELL_M, (cz as f32 + 0.5) * CELL_M);
            let (dx, dz) = (x - cam[0], z - cam[2]);
            let distance = (dx * dx + dz * dz).sqrt();
            if cell.sectors.count_ones() <= 2 && (1.0..12.0).contains(&distance) && cell.slices & !floor_mask != 0 {
                let bearing = (dx.atan2(dz).to_degrees() - heading).rem_euclid(360.0);
                let k = (((bearing + 22.5) / 45.0) as usize) % 8;
                one_sided[k].0 += 1;
                one_sided[k].1 += distance;
            }
            if floor_mask != 0 && cell.slices & floor_mask != 0 && cell.angles & !cell_bits::GRAZING == 0 && near_path(x, z) {
                grazing_floor += 1;
            }
        }
        const WAYS: [&str; 8] = [
            "ahead of you",
            "ahead to your right",
            "to your right",
            "behind you to the right",
            "behind you",
            "behind you to the left",
            "to your left",
            "ahead to your left",
        ];
        if let Some((k, &(n, total))) = one_sided.iter().enumerate().max_by_key(|(_, v)| v.0) {
            // At least 1.5 m² of it.
            if n >= 6 {
                out.push(format!(
                    "What is {} (about {:.0} m away) was only seen from one direction: look at it from another angle too.",
                    WAYS[k],
                    total / n as f32
                ));
            }
        }
        if grazing_floor >= 10 {
            out.push("Look down at the floor around you: so far it was only seen from afar, at a grazing angle.".into());
        }
        if seconds >= 10.0 && self.path.len() >= 10 {
            let (lo, hi) = self.path.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[1]), hi.max(p[1])));
            if hi - lo < 0.3 {
                out.push("Crouch or raise the camera for a moment: everything so far was seen from one height.".into());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_contracts::depth::{quantize, DepthLayout};

    /// A camera at eye height over a floor (y = 0), with a wall 6 m ahead (+Z), yaw `yaw_deg`: the depth frame's
    /// grid, as decoded codes.
    fn frame(pos: [f32; 3], yaw_deg: f32) -> ([f32; 12], [f32; 4], DepthLayout, u32, Vec<u16>) {
        let layout = DepthLayout::new(960, 540).unwrap();
        let (gw, gh) = layout.grid_size();
        let w = yaw_deg.to_radians();
        let (right, up, forward) = ([w.cos(), 0.0, -w.sin()], [0.0, 1.0, 0.0], [w.sin(), 0.0, w.cos()]);
        let m = [right[0], up[0], forward[0], pos[0], right[1], up[1], forward[1], pos[1], right[2], up[2], forward[2], pos[2]];
        let projection = [1.0, 1.0 * 960.0 / 540.0, 0.0, 0.0];
        let k = intrinsics_from_projection(&projection, 960, 540);
        let mut codes = vec![0u16; (gw * gh) as usize];
        for gy in 0..gh {
            for gx in 0..gw {
                let (u, v) = layout.grid_pixel(gx, gy);
                let (x, y) = ((u as f32 - k.cx) / k.fx, (v as f32 - k.cy) / k.fy);
                // The ray at eye depth 1.
                let d = [0, 1, 2].map(|i| right[i] * x - up[i] * y + forward[i]);
                let floor = if d[1] < -1e-3 { -pos[1] / d[1] } else { f32::INFINITY };
                let wall = if d[2] > 1e-3 { (6.0 - pos[2]) / d[2] } else { f32::INFINITY };
                codes[(gy * gw + gx) as usize] = quantize(floor.min(wall));
            }
        }
        (m, projection, layout, gw, codes)
    }

    #[test]
    fn a_straight_walk_sees_the_wall_from_one_side_and_one_height() {
        let mut c = Coverage::new();
        for i in 0..20 {
            let (m, p, l, gw, codes) = frame([0.0, 1.6, -2.0 + 0.1 * i as f32], 0.0);
            c.add_depth(&m, &p, &l, gw, &codes);
        }
        let map = c.map(Some([0.0, 1.6, 0.0]), Some(0.0), 15.0).unwrap();
        assert!((map.floor_y.unwrap() - 0.0).abs() <= 0.25, "floor at {:?}", map.floor_y);
        // The wall's cells are obstacles, seen from one side.
        let at = |x: f32, z: f32| {
            let (ix, iz) = (((x - map.x0) / map.cell_m) as u32, ((z - map.z0) / map.cell_m) as u32);
            let i = (iz * map.width + ix) as usize;
            (map.cells[2 * i], map.cells[2 * i + 1])
        };
        let (sectors, bits) = at(0.25, 6.1);
        assert!((1..=2).contains(&sectors) && bits & cell_bits::OBSTACLE != 0, "wall cell {sectors} {bits:#x}");
        let (_, floor_bits) = at(0.25, 2.25);
        assert!(floor_bits & cell_bits::FLOOR != 0 && floor_bits & cell_bits::OBSTACLE == 0, "floor cell {floor_bits:#x}");
        assert!(map.hints.iter().any(|h| h.contains("ahead") && h.contains("one direction")), "{:?}", map.hints);
        assert!(map.hints.iter().any(|h| h.contains("one height")), "{:?}", map.hints);
    }

    #[test]
    fn turning_and_crouching_silence_the_hints() {
        let mut c = Coverage::new();
        for i in 0..72 {
            // Around a point 3 m from the wall, low and high, looking every way.
            let a = (i as f32 * 20.0).to_radians();
            let pos = [1.5 * a.sin(), if i % 2 == 0 { 0.6 } else { 2.4 }, 3.0 + 1.5 * a.cos()];
            let (m, p, l, gw, codes) = frame(pos, i as f32 * 25.0);
            c.add_depth(&m, &p, &l, gw, &codes);
        }
        let map = c.map(Some([0.0, 1.6, 3.0]), Some(0.0), 30.0).unwrap();
        assert!(!map.hints.iter().any(|h| h.contains("one height")), "{:?}", map.hints);
        assert!(!map.hints.iter().any(|h| h.contains("grazing")), "{:?}", map.hints);
    }
}
