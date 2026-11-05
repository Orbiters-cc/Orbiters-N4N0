//! Taking the wearer of the capture kit out of a session that should not show them, when the avatar itself could not hide
//! (an upload from before Toolkit 0.4.18: nothing from outside can change an uploaded avatar). A 3D picture of the world,
//! or a recording with Local User off in VRChat's camera, then has the wearer in its colour, its depth and the rig's faces;
//! this finds them there, and training never learns them.
//!
//! **What is known of the body.** The skeleton band gives the hips' world position and the world rotation of every humanoid
//! bone, frame by frame, and nothing else: no bone lengths and no rest pose. A rotation of a bone says nothing about where
//! the bone points without its rest rotation (those are whatever the avatar's rig made them), so bone capsules cannot be
//! placed from the band. What it does give, exactly: where the body stands, and when any part of it moved (a rotation's
//! change is meaningful, whatever the rest pose). VRChat's `/avatar/eyeheight` gives its size. Everything else is measured:
//! the exact depth frames see the body's real surface, ears, tail and all.
//!
//! **Finding it** ([`find_body`], one [`Slice`] of the session at a time, a stretch during which the body held still):
//! 1. Every trusted depth sample around the hips goes into voxels of [`VOXEL_PER_EYE`] of the eye height (3 cm on The 1MB
//!    Ultirex), with its normal.
//! 2. The world grows in from outside: voxels outside the envelope (a cylinder around the hips, [`ENVELOPE_RADIUS`] eye
//!    heights wide), and the flat floor under the hips, spread along surfaces that continue smoothly: floors and walls run
//!    under and behind the body and stop at the crease where it meets them.
//! 3. The body grows out from a core (the column between the hips and the eyes, which no world may enter) through what the
//!    world did not claim, across gaps up to a voxel wide. Whatever does not join it stays: a chair or another player a hand's
//!    width away is not the wearer, whatever the envelope says. Someone holding on to the wearer does join, up to the envelope.
//!
//! **Taking it out** ([`remove`]): depth samples on the body say nothing from then on (no seeds, no depth targets:
//! [`DepthView::unknown`](crate::dataset::DepthView)). A keyframe's or a rig face's pixels that show it get alpha 0
//! ([`is_out`]), which is otherwise unused and survives the aberration's realignment, and the colour steps leave them out
//! through the loss mask ([`crate::train`]), seeds take no colour from them. What was behind the wearer gets what other
//! frames saw of it. What stays inside the wearer's volume when training ends goes too ([`Volume`]), and so does whatever around
//! them only their own pixels showed ([`leave_out_unseen`]). What nothing saw (under the feet) stays empty, or takes the floor's
//! colour around it when the wearer stood on a floor ([`patch_floor`]).
//!
//! **What this cannot do.** Their shadow on the floor and their reflection in a mirror are the world's colour as far as
//! any of this knows (lifting a shadow from the floor's colours was tried on Fort Rosebay: its direction is found, the floor comes
//! out blotchy with the shadow still there, so it was dropped). Someone touching the wearer has no seam in the depth and goes with
//! them, up to the envelope. An upload with the current Toolkit hides the avatar's renderers, shadow included, and needs none of
//! it: that stays the clean way.
//!
//! **Measured** on Enzo's seven recordings of 2026-11-04 (reports/wearer-out.md of the lab, three runs of each, the same pixels
//! scored): held-out views lose about a tenth of a dB against the same session with the wearer left in, and take a little less time.

use crate::camera::Camera;
use crate::dataset::{Dataset, DepthView, View};
use crate::gaussians::{sigmoid, Gaussians};
use orbiters_contracts::performance::{self, skeleton_frames};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

/// Voxel size, in eye heights. 3 cm on an avatar with eyes at 1.8 m. A depth frame's samples are 8 px apart at 1080p: 2.5 cm
/// at 3 m at the 3D pictures' focal length, so the flight's nearer frames fill these voxels and its far ones add to them.
const VOXEL_PER_EYE: f32 = 1.0 / 60.0;
/// Voxels closer than this many steps along each axis belong together: a gap of one voxel joins, two do not.
const REACH: i32 = 2;
/// How far from the hips' vertical axis the body can reach, eye heights: The 1MB Ultirex's tail reaches about 0.5 m behind it
/// (0.3), and arms held straight out about 0.55. Past it, everything is world.
const ENVELOPE_RADIUS: f32 = 0.7;
/// How much more than the envelope is looked at: what reaches past the envelope seeds the world.
const SHELL: f32 = 0.25;
/// The body's top above the floor, eye heights: ears and hair over the eyes.
const ENVELOPE_TOP: f32 = 1.45;
/// The core: within the first of these many eye heights of the hips' axis that holds [`CORE_VOXELS`], between these heights
/// above the floor. The body is sure to be there, and no world may enter it, even along an arm that reaches past the envelope.
/// A surface is what depth sees: a stout avatar has none near the axis (a box 0.15 m around it shows nothing within 0.14 m).
const CORE_RADII: [f32; 4] = [0.08, 0.12, 0.17, 0.24];
const CORE_VOXELS: usize = 30;
const CORE_FROM: f32 = 0.3;
const CORE_TO: f32 = 1.0;
/// A voxel continues a surface of the world when its normal is within about 35° of that surface's, it is flat (its samples'
/// normals agree to this much), and it lies on it.
const SAME_SURFACE: f32 = 0.82;
const FLAT: f32 = 0.8;
/// A voxel lies on the plane the world came along within this many voxels.
const ON_PLANE: f32 = 1.0;
/// The ground: this many eye heights above the floor, anything facing up belongs to the world whatever its shape (grass,
/// uneven rock); less than a paw's top.
const GROUND_BAND: f32 = 0.035;
/// Floor: this many eye heights around the height most flat, level samples sit at, under the hips.
const FLOOR_BAND: f32 = 0.02;
const HIPS_PER_EYE: f32 = crate::ground::HIPS_PER_EYE_HEIGHT;
/// A new slice starts when the hips have moved this many eye heights, or turned this many degrees, or any of the big bones
/// (hips to neck, shoulders, arms, legs) this many degrees, since its first frame.
const SLICE_MOVE: f32 = 0.02;
const SLICE_HIPS_TURN: f32 = 6.0;
const SLICE_BONE_TURN: f32 = 15.0;
/// A pixel shows the body within this much world margin around a voxel's centre (past the half voxel, the fur tips whose
/// transparent parts never reach depth), and this many training pixels past that at 1080p (anti-aliasing, the world's bloom).
const MASK_MARGIN_PER_EYE: f32 = 0.012;
const MASK_DILATE_1080P: f32 = 3.0;
/// Most view's pixels an enclosed gap in the mask may cover to count as a hole in the body rather than world seen past it.
const HOLE_SHARE: f32 = 0.02;
/// So many views' scene pixels must fall on a Gaussian near where the wearer stood for it to have been trained.
const SEEN_BY: usize = 3;
/// How far under the floor [`leave_out_unseen`] looks, eye heights: a trained floor is a layer, not a sheet, and what lies under it
/// where the wearer stood is seen as soon as nothing covers it. (Taking all of that out whatever it saw, as a first try did, opened
/// dark holes at the feet.)
const UNDER_FLOOR: f32 = 0.3;
/// A body sample and a view's nearest moment: views take the bodies of the slices nearest them in time, this many.
const SLICES_PER_VIEW: usize = 3;

/// The pose of the wearer over a session, from `skeleton.bin`, and how big they are.
pub struct Track {
    pub frames: Vec<skeleton_frames::Frame>,
    pub eye_height: f32,
}

impl Track {
    /// A session's: `None` without skeleton frames (no kit, or a band nothing could read): then nothing can say where the
    /// wearer stands.
    pub fn read(session: &Path) -> Option<Self> {
        let mut frames = skeleton_frames::read(&std::fs::read(session.join(performance::SKELETON_FILE)).ok()?).ok()?;
        frames.retain(|f| f.hips.iter().all(|c| c.is_finite()));
        frames.sort_by(|a, b| a.game_time.total_cmp(&b.game_time));
        if frames.is_empty() {
            return None;
        }
        let eye_height = crate::ground::eye_height(session, None).filter(|e| *e > 0.05 && *e < 50.0).unwrap_or(crate::ground::EYE_HEIGHT);
        Some(Self { frames, eye_height })
    }

    fn nearest(&self, time: f32) -> &skeleton_frames::Frame {
        let at = self.frames.partition_point(|f| f.game_time < time);
        let later = self.frames.get(at);
        let earlier = at.checked_sub(1).and_then(|i| self.frames.get(i));
        match (earlier, later) {
            (Some(a), Some(b)) => {
                if (time - a.game_time).abs() <= (b.game_time - time).abs() {
                    a
                } else {
                    b
                }
            }
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => unreachable!("a track has frames"),
        }
    }
}

/// What was done, for the log and the session's report.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    /// Stretches during which the body held still, and voxels of body found (in the largest).
    pub slices: usize,
    pub voxels: usize,
    /// Depth samples on the body, of those that hit something.
    pub depth_samples: usize,
    pub depth_hits: usize,
    /// Views (keyframes and rig faces) with any pixel left out, of all, and the share of all their pixels.
    pub views: usize,
    pub views_total: usize,
    pub pixel_share: f32,
    /// The floor's height under the wearer, when flat samples showed it.
    pub floor: Option<f32>,
}

/// A stretch of the session during which the body held still: its depth frames (indices into `Dataset::depth_views`), the
/// hips, and the voxels the body fills.
struct Slice {
    time: f32,
    hips: [f32; 3],
    rotation: [f32; 4],
    frames: Vec<usize>,
    body: Vec<[f32; 3]>,
}

/// Which slice of `times` each depth frame is in, in time order: a new one whenever the body has moved since the last began.
fn slices_of(depth: &[DepthView], track: &Track) -> Vec<Slice> {
    let mut order: Vec<usize> = (0..depth.len()).collect();
    order.sort_by(|&a, &b| depth[a].game_time.total_cmp(&depth[b].game_time));
    let mut out: Vec<Slice> = Vec::new();
    let mut first: Option<&skeleton_frames::Frame> = None;
    for d in order {
        let f = track.nearest(depth[d].game_time);
        let same = first.is_some_and(|s| !moved(s, f, track.eye_height));
        if same {
            out.last_mut().expect("a slice").frames.push(d);
        } else {
            first = Some(f);
            out.push(Slice { time: f.game_time, hips: f.hips, rotation: rotation_of(f), frames: vec![d], body: Vec::new() });
        }
    }
    out
}

/// The bones that move a lot of the body: hips to head (0, 7, 8, 9, 10), legs (1–6), shoulders and arms (11–18).
const BIG_BONES: [usize; 19] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18];

fn moved(a: &skeleton_frames::Frame, b: &skeleton_frames::Frame, eye: f32) -> bool {
    let d = sub(a.hips, b.hips);
    if dot(d, d).sqrt() > SLICE_MOVE * eye {
        return true;
    }
    BIG_BONES.iter().any(|&k| match (a.rotations[k], b.rotations[k]) {
        (Some(p), Some(q)) => angle_between(p, q) > if k == 0 { SLICE_HIPS_TURN } else { SLICE_BONE_TURN },
        _ => false,
    })
}

fn rotation_of(f: &skeleton_frames::Frame) -> [f32; 4] {
    f.rotations[0].unwrap_or([0.0, 0.0, 0.0, 1.0])
}

/// Degrees between two rotations (x, y, z, w).
fn angle_between(p: [f32; 4], q: [f32; 4]) -> f32 {
    let d = (p[0] * q[0] + p[1] * q[1] + p[2] * q[2] + p[3] * q[3]).abs().min(1.0);
    2.0 * d.acos().to_degrees()
}

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

fn quat_conj(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let u = [q[0], q[1], q[2]];
    let t = cross(u, v).map(|c| c * 2.0);
    let c = cross(u, t);
    [v[0] + q[3] * t[0] + c[0], v[1] + q[3] * t[1] + c[1], v[2] + q[3] * t[2] + c[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn axis_distance(p: [f32; 3], hips: [f32; 3]) -> f32 {
    ((p[0] - hips[0]).powi(2) + (p[2] - hips[2]).powi(2)).sqrt()
}

type Key = [i32; 3];

fn key(p: [f32; 3], voxel: f32) -> Key {
    p.map(|c| (c / voxel).floor() as i32)
}

/// A depth frame's samples that hit something within `radius` of the hips' axis and between these heights, with their
/// normals (facing the camera): (sample, point, normal).
fn near_samples(dv: &DepthView, hips: [f32; 3], radius: f32, low: f32, high: f32) -> Vec<(u32, [f32; 3], [f32; 3])> {
    let mut out = Vec::new();
    for i in 0..dv.depth.len().min(dv.grid.len()) {
        let z = dv.depth[i];
        if z <= 0.0 || dv.is_unknown(i) {
            continue;
        }
        let [u, v] = dv.grid.pixels[i];
        let p = dv.camera.unproject(u, v, z);
        if p[1] < low || p[1] > high || axis_distance(p, hips) > radius {
            continue;
        }
        out.push((i as u32, p, crate::init::facing_normal(dv, i, p, false)));
    }
    out
}

/// The height of the floor under the hips: the most common height of the level samples under them within an eye height of
/// the axis, at least a tenth of an eye height below the hips (a sitting avatar's are low). `None`: no such samples.
fn find_floor(samples: &[(u32, [f32; 3], [f32; 3])], hips: [f32; 3], eye: f32) -> Option<f32> {
    let bin = 0.25 * FLOOR_BAND * eye;
    let mut counts: HashMap<i32, u32> = HashMap::new();
    for (_, p, n) in samples {
        if p[1] < hips[1] - 0.1 * eye && n[1].abs() > 0.9 && axis_distance(*p, hips) < eye {
            *counts.entry((p[1] / bin).floor() as i32).or_default() += 1;
        }
    }
    let (best, n) = counts.iter().max_by_key(|(k, n)| (**n, **k)).map(|(k, n)| (*k, *n))?;
    (n >= 8).then_some((best as f32 + 0.5) * bin)
}

#[derive(Clone, Copy, Default)]
struct Cell {
    position: [f32; 3],
    normal: [f32; 3],
    count: u32,
}

impl Cell {
    fn mean(&self) -> [f32; 3] {
        self.position.map(|c| c / self.count as f32)
    }

    /// How much the normals of the samples here agree: 1 when they all point the same way.
    fn flatness(&self) -> f32 {
        dot(self.normal, self.normal).sqrt() / self.count as f32
    }

    fn unit_normal(&self) -> [f32; 3] {
        let l = dot(self.normal, self.normal).sqrt().max(1e-9);
        self.normal.map(|c| c / l)
    }
}

/// What [`find_body`] tells: the body's voxels, what the world claimed, and the floor.
struct Found {
    body: HashSet<Key>,
    world: HashSet<Key>,
    floor: Option<f32>,
}

fn neighbours(k: Key) -> impl Iterator<Item = Key> {
    (-REACH..=REACH).flat_map(move |x| (-REACH..=REACH).flat_map(move |y| (-REACH..=REACH).map(move |z| [k[0] + x, k[1] + y, k[2] + z]))).filter(move |n| *n != k)
}

/// The body in `samples`, as the module says: the world grows in, then the body grows out through what it left.
fn find_body(samples: &[(u32, [f32; 3], [f32; 3])], hips: [f32; 3], eye: f32) -> Found {
    let voxel = VOXEL_PER_EYE * eye;
    let floor_at = find_floor(samples, hips, eye);
    let floor = floor_at.unwrap_or(hips[1] - HIPS_PER_EYE * eye);
    let mut cells: HashMap<Key, Cell> = HashMap::new();
    for (_, p, n) in samples {
        let c = cells.entry(key(*p, voxel)).or_default();
        for a in 0..3 {
            c.position[a] += p[a];
            c.normal[a] += n[a];
        }
        c.count += 1;
    }
    let radius = ENVELOPE_RADIUS * eye;
    let top = floor + ENVELOPE_TOP * eye;
    let in_core = |p: [f32; 3], r: f32| axis_distance(p, hips) < r * eye && p[1] > floor + CORE_FROM * eye && p[1] < floor + CORE_TO * eye;
    let core_radius = CORE_RADII.iter().copied().find(|r| cells.values().filter(|c| in_core(c.mean(), *r)).count() >= CORE_VOXELS).unwrap_or(CORE_RADII[0]);
    let core = |p: [f32; 3]| in_core(p, core_radius);
    let on_floor = |p: [f32; 3], c: &Cell| (p[1] - floor).abs() < FLOOR_BAND * eye && c.unit_normal()[1].abs() > 0.8;

    // The world, grown in from what lies outside the envelope and the floor. Each voxel it takes keeps the plane it came along:
    // only planes get in (a smooth step from voxel to voxel climbs from the floor over a paw, a leg and an arm, as the first
    // version did on The 1MB Ultirex, and took half the body), and the ground: anything low and facing up, for terrain.
    let mut world: HashSet<Key> = HashSet::new();
    let mut queue: VecDeque<(Key, [f32; 3], [f32; 3])> = VecDeque::new();
    // In a fixed order, as each voxel keeps the plane it came along: a run resumed after a pause finds the very mask it trained on.
    let mut ordered: Vec<Key> = cells.keys().copied().collect();
    ordered.sort_unstable();
    for k in &ordered {
        let c = &cells[k];
        let p = c.mean();
        if !core(p) && (axis_distance(p, hips) > radius || p[1] > top || p[1] < floor - FLOOR_BAND * eye || on_floor(p, c)) {
            world.insert(*k);
            queue.push_back((*k, p, c.unit_normal()));
        }
    }
    let ground = |c: &Cell, p: [f32; 3]| p[1] < floor + GROUND_BAND * eye && c.unit_normal()[1].abs() > 0.8 && c.flatness() >= FLAT;
    while let Some((k, origin, normal)) = queue.pop_front() {
        let a = cells[&k];
        for n in neighbours(k) {
            if world.contains(&n) {
                continue;
            }
            let Some(b) = cells.get(&n) else { continue };
            let pb = b.mean();
            if core(pb) {
                continue;
            }
            let along = a.flatness() >= FLAT && b.flatness() >= FLAT && dot(normal, b.unit_normal()) >= SAME_SURFACE && dot(normal, sub(pb, origin)).abs() <= ON_PLANE * voxel;
            if along || (ground(&a, a.mean()) && ground(b, pb)) {
                world.insert(n);
                queue.push_back((n, origin, normal));
            }
        }
    }

    // The body, grown out of the core through what the world left.
    let mut body: HashSet<Key> = HashSet::new();
    let mut growing: VecDeque<Key> = VecDeque::new();
    for (k, c) in &cells {
        if core(c.mean()) && !world.contains(k) {
            body.insert(*k);
            growing.push_back(*k);
        }
    }
    while let Some(k) = growing.pop_front() {
        // Along the ground it reaches no further: the voxels where the floor meets a foot are mixed, the world does not claim
        // them, and through them someone standing a hand away joined the body by their feet.
        if cells[&k].mean()[1] < floor + GROUND_BAND * eye {
            continue;
        }
        for n in neighbours(k) {
            if body.contains(&n) || world.contains(&n) {
                continue;
            }
            let Some(c) = cells.get(&n) else { continue };
            let p = c.mean();
            if axis_distance(p, hips) <= radius && p[1] <= top {
                body.insert(n);
                growing.push_back(n);
            }
        }
    }
    Found { body, world, floor: floor_at }
}

/// Fills a disc in a `width × height` mask.
fn disc(mask: &mut [bool], width: usize, height: usize, u: f32, v: f32, r: f32) {
    let (y0, y1) = ((v - r).floor().max(0.0) as usize, (v + r).ceil().min(height as f32 - 1.0));
    if y1 < 0.0 {
        return;
    }
    for y in y0..=y1 as usize {
        let dy = y as f32 - v;
        let half = (r * r - dy * dy).max(0.0).sqrt();
        let (x0, x1) = ((u - half).ceil().max(0.0) as usize, (u + half).floor().min(width as f32 - 1.0));
        if x1 < 0.0 || x0 as f32 > x1 {
            continue;
        }
        mask[y * width + x0..=y * width + x1 as usize].fill(true);
    }
}

/// `mask` grown by `r` pixels (a square, row then column).
fn dilate(mask: &mut Vec<bool>, width: usize, height: usize, r: usize) {
    if r == 0 {
        return;
    }
    let mut across = vec![false; mask.len()];
    for y in 0..height {
        let row = &mask[y * width..(y + 1) * width];
        let mut last = None;
        for x in 0..width {
            if row[x] {
                last = Some(x);
            }
            let ahead = (x..=(x + r).min(width - 1)).any(|q| row[q]);
            across[y * width + x] = ahead || last.is_some_and(|l| x - l <= r);
        }
    }
    for x in 0..width {
        let mut last = None;
        for y in 0..height {
            if across[y * width + x] {
                last = Some(y);
            }
            let ahead = (y..=(y + r).min(height - 1)).any(|q| across[q * width + x]);
            mask[y * width + x] = ahead || last.is_some_and(|l| y - l <= r);
        }
    }
}

/// Fills gaps in `mask` the rest of the picture does not reach, up to `largest` pixels: parts of the body no depth frame saw,
/// seen here through the voxels around them.
fn fill_holes(mask: &mut [bool], width: usize, height: usize, largest: usize) {
    let mut seen = vec![false; mask.len()];
    let mut stack = Vec::new();
    for start in 0..mask.len() {
        if mask[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut pixels = Vec::new();
        let mut reaches_edge = false;
        while let Some(p) = stack.pop() {
            pixels.push(p);
            let (x, y) = (p % width, p / width);
            reaches_edge |= x == 0 || y == 0 || x == width - 1 || y == height - 1;
            let mut go = |q: usize| {
                if !mask[q] && !seen[q] {
                    seen[q] = true;
                    stack.push(q);
                }
            };
            if x > 0 {
                go(p - 1);
            }
            if x + 1 < width {
                go(p + 1);
            }
            if y > 0 {
                go(p - width);
            }
            if y + 1 < height {
                go(p + width);
            }
        }
        if !reaches_edge && pixels.len() <= largest {
            for p in pixels {
                mask[p] = true;
            }
        }
    }
}

/// The pixels of a `camera` view that show `body` (points with their radius), at the view's size.
fn view_mask(camera: &Camera, bodies: &[(&[[f32; 3]], [f32; 3], [f32; 3], [f32; 4])], radius: f32, dilate_px: f32) -> Option<Vec<bool>> {
    let (w, h) = (camera.width as usize, camera.height as usize);
    let mut mask = vec![false; w * h];
    let mut any = false;
    for &(body, from, to, turn) in bodies {
        for p in body {
            let q = sub(*p, from);
            let q = rotate(turn, q);
            let at = [q[0] + to[0], q[1] + to[1], q[2] + to[2]];
            let Some((u, v, z)) = camera.project(at) else { continue };
            if z < 0.05 {
                continue;
            }
            let r = camera.fx.max(camera.fy) * radius / z.max(0.15);
            if u + r < 0.0 || v + r < 0.0 || u - r > w as f32 || v - r > h as f32 {
                continue;
            }
            disc(&mut mask, w, h, u, v, r);
            any = true;
        }
    }
    if !any {
        return None;
    }
    fill_holes(&mut mask, w, h, ((w * h) as f32 * HOLE_SHARE) as usize);
    let d = (dilate_px * h as f32 / 1080.0).ceil() as usize;
    dilate(&mut mask, w, h, d.max(1));
    Some(mask)
}

/// Whether a keyframe's or a rig face's pixel was left out as showing the wearer ([`remove`]).
pub fn is_out(pixel: u32) -> bool {
    pixel >> 24 == 0
}

fn leave_out(view: &mut View, mask: &[bool]) -> usize {
    let mut n = 0;
    for (p, m) in view.image.iter_mut().zip(mask) {
        if *m {
            *p &= 0x00FF_FFFF;
            n += 1;
        }
    }
    n
}

/// What the wearer filled while they held still: what is inside it when training ends goes ([`Volume::retain_outside`]).
#[derive(Clone, Debug, Default)]
pub struct Volume {
    voxel: f32,
    cells: HashSet<Key>,
    /// The floor under them, for [`patch_floor`].
    pub floor: Option<f32>,
    /// The hips over it.
    pub hips: [f32; 3],
    pub eye_height: f32,
}

impl Volume {
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn contains(&self, p: [f32; 3]) -> bool {
        self.cells.contains(&key(p, self.voxel))
    }

    /// Takes out the Gaussians whose centre lies in the volume (never the sky's), and says how many.
    pub fn retain_outside(&self, g: &mut Gaussians) -> usize {
        if self.is_empty() {
            return 0;
        }
        let keep: Vec<bool> = (0..g.len()).map(|i| g.sky.get(i).copied().unwrap_or(false) || !self.contains(g.means[i])).collect();
        let out = keep.iter().filter(|k| !**k).count();
        if out > 0 {
            g.retain(&keep);
        }
        out
    }
}

/// Takes out what lies around where the wearer stood and no pixel left in was there to train: with their pixels out of the loss, a
/// Gaussian only those pixels show keeps whatever colour it was seeded or copied with. On all seven of Enzo's recordings that was a
/// blotch at the feet, dark holes and pale or orange smears, seen from every side and in no score, since no keyframe's pixels
/// there count. A Gaussian stays when its centre falls on a scene pixel of at least [`SEEN_BY`] training views (rig faces too).
/// `held_out`: views that never trained. Returns how many went.
pub fn leave_out_unseen(data: &Dataset, held_out: &[usize], volume: &Volume, g: &mut Gaussians) -> usize {
    let (Some(floor), eye) = (volume.floor, volume.eye_height) else { return 0 };
    let (radius, top) = (ENVELOPE_RADIUS * eye, floor + ENVELOPE_TOP * eye);
    let low = floor - UNDER_FLOOR * eye;
    let near = |p: [f32; 3]| axis_distance(p, volume.hips) <= radius && p[1] <= top && p[1] >= low;
    let shows = |v: &View, mask: &[f32], p: [f32; 3]| -> bool {
        let Some((u, y, _)) = v.camera.project(p) else { return false };
        let (x, y) = (u.round() as i64, y.round() as i64);
        if x < 0 || y < 0 || x >= v.camera.width as i64 || y >= v.camera.height as i64 {
            return false;
        }
        let k = y as usize * v.camera.width as usize + x as usize;
        mask[k] > 0.5 && !is_out(v.image[k])
    };
    let views: Vec<&View> = data.views.iter().enumerate().filter(|(i, _)| !held_out.contains(i)).map(|(_, v)| v).collect();
    // Rig faces come in the sizes the session has (an old upload's and a new one's), each with its own mask.
    let faces: Vec<(&View, &[f32])> =
        data.faces.iter().flat_map(|f| f.views.iter().filter_map(|v| f.size_of(&v.camera).map(|s| (v, s.mask.as_slice())))).collect();
    let keep: Vec<bool> = (0..g.len())
        .into_par_iter()
        .map(|i| {
            let p = g.means[i];
            if g.sky.get(i).copied().unwrap_or(false) || !near(p) {
                return true;
            }
            let seen = views.iter().filter(|v| shows(v, &data.mask, p)).take(SEEN_BY).count();
            seen + faces.iter().filter(|(v, mask)| shows(v, mask, p)).take(SEEN_BY - seen).count() >= SEEN_BY
        })
        .collect();
    let out = keep.iter().filter(|k| !**k).count();
    if out > 0 {
        g.retain(&keep);
    }
    out
}

/// Fills the floor where the wearer stood and nothing saw it (under their feet and, when few views looked behind them, a
/// little around): flat discs in the colour of the floor around the patch, opaque enough to read as floor, so a viewer
/// looking down sees a floor and not what lies under it. Honest in the sense that it is a guess: the report counts them.
/// Only with a known floor: nothing is invented over stairs or in midair. Returns how many it added.
pub fn patch_floor(volume: &Volume, g: &mut Gaussians) -> usize {
    let Some(floor) = volume.floor else { return 0 };
    let eye = volume.eye_height;
    let cell = 2.0 * VOXEL_PER_EYE * eye;
    let reach = 0.5 * eye;
    let side = (2.0 * reach / cell).ceil() as i32;
    let at = |x: i32, z: i32| [volume.hips[0] + (x as f32 + 0.5) * cell - reach, floor, volume.hips[2] + (z as f32 + 0.5) * cell - reach];
    // Each patch cell: the visible floor splats in it, weighted by opacity.
    let (mut weight, mut color) = (vec![0f32; (side * side) as usize], vec![[0f32; 3]; (side * side) as usize]);
    let band = 2.5 * FLOOR_BAND * eye;
    for i in 0..g.len() {
        let p = g.means[i];
        if (p[1] - floor).abs() > band || g.sky.get(i).copied().unwrap_or(false) {
            continue;
        }
        let (x, z) = (((p[0] - volume.hips[0] + reach) / cell).floor() as i32, ((p[2] - volume.hips[2] + reach) / cell).floor() as i32);
        if x < 0 || z < 0 || x >= side || z >= side {
            continue;
        }
        let o = sigmoid(g.opacity_logits[i]);
        let k = (z * side + x) as usize;
        weight[k] += o;
        for a in 0..3 {
            color[k][a] += o * g.colors[i][a];
        }
    }
    // Empty cells, nearest filled first: the colour of the filled cells around them, spreading inward.
    let empty = |w: f32| w < 0.5;
    let mut added = 0;
    let first_filled: Vec<bool> = weight.iter().map(|w| !empty(*w)).collect();
    let mut grown = first_filled.clone();
    let s = side as usize;
    // The patch is only what the filled cells enclose: an empty cell that open empty space reaches is not under the wearer.
    let mut outside = vec![false; s * s];
    let mut stack: Vec<usize> = (0..s * s).filter(|&k| !first_filled[k] && (k % s == 0 || k % s == s - 1 || k / s == 0 || k / s == s - 1)).collect();
    while let Some(k) = stack.pop() {
        if outside[k] {
            continue;
        }
        outside[k] = true;
        let (x, z) = (k % s, k / s);
        for (dx, dz) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let (nx, nz) = (x as i32 + dx, z as i32 + dz);
            if nx >= 0 && nz >= 0 && nx < side && nz < side {
                let q = nz as usize * s + nx as usize;
                if !first_filled[q] && !outside[q] {
                    stack.push(q);
                }
            }
        }
    }
    loop {
        let mut changed = Vec::new();
        for k in 0..s * s {
            if grown[k] || outside[k] {
                continue;
            }
            let (x, z) = (k % s, k / s);
            let (mut sum, mut total) = ([0f32; 3], 0f32);
            for (dx, dz) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, -1), (1, -1), (-1, 1)] {
                let (nx, nz) = (x as i32 + dx, z as i32 + dz);
                if nx >= 0 && nz >= 0 && nx < side && nz < side {
                    let q = nz as usize * s + nx as usize;
                    if grown[q] {
                        for a in 0..3 {
                            sum[a] += color[q][a];
                        }
                        total += weight[q];
                    }
                }
            }
            if total > 0.0 {
                changed.push((k, sum.map(|c| c / total)));
            }
        }
        if changed.is_empty() {
            break;
        }
        for (k, c) in changed {
            grown[k] = true;
            weight[k] = 1.0;
            color[k] = c;
            let (x, z) = ((k % s) as i32, (k / s) as i32);
            let log = (0.75 * cell).ln();
            g.push(at(x, z), [log, log, (0.1 * cell).ln()], crate::gaussians::quat_from_z([0.0, 1.0, 0.0]), crate::gaussians::logit(0.9), c);
            added += 1;
        }
    }
    added
}

/// Takes the wearer out of `data` as the module says, and returns the volume they filled. `None`: nothing of them was found
/// (no depth frame saw them), which leaves `data` as it was.
pub fn remove(data: &mut Dataset, track: &Track) -> Option<(Volume, Stats)> {
    let eye = track.eye_height;
    let voxel = VOXEL_PER_EYE * eye;
    let radius = (ENVELOPE_RADIUS + SHELL) * eye;
    let mut slices = slices_of(&data.depth_views, track);
    let mut stats = Stats { slices: slices.len(), ..Default::default() };
    let mut per_depth: Vec<Vec<u32>> = vec![Vec::new(); data.depth_views.len()];
    let mut best: Option<(usize, HashSet<Key>, Option<f32>)> = None;
    for (index, slice) in slices.iter_mut().enumerate() {
        let hips = slice.hips;
        let (low, high) = (hips[1] - 1.3 * eye, hips[1] + 1.3 * eye);
        let found: Vec<(usize, Vec<(u32, [f32; 3], [f32; 3])>)> = slice.frames.par_iter().map(|&d| (d, near_samples(&data.depth_views[d], hips, radius, low, high))).collect();
        let all: Vec<(u32, [f32; 3], [f32; 3])> = found.iter().flat_map(|(_, s)| s.iter().copied()).collect();
        let body = find_body(&all, hips, eye);
        if body.floor.is_some() && stats.floor.is_none() {
            stats.floor = body.floor;
        }
        if body.body.is_empty() {
            continue;
        }
        // A sample on the body, or beside it and on no surface of the world (a fur tip across a gap).
        for (d, samples) in &found {
            for (i, p, _) in samples {
                let k = key(*p, voxel);
                let on = body.body.contains(&k) || (!body.world.contains(&k) && neighbours(k).any(|n| body.body.contains(&n)));
                if on {
                    per_depth[*d].push(*i);
                }
            }
        }
        slice.body = body.body.iter().map(|k| [(k[0] as f32 + 0.5) * voxel, (k[1] as f32 + 0.5) * voxel, (k[2] as f32 + 0.5) * voxel]).collect();
        stats.voxels = stats.voxels.max(slice.body.len());
        if best.as_ref().is_none_or(|(_, b, _)| b.len() < body.body.len()) {
            best = Some((index, body.body, body.floor));
        }
    }
    let (best_slice, cells, floor) = best?;
    stats.depth_hits = data.depth_views.iter().map(DepthView::valid).sum();
    for (d, samples) in per_depth.iter().enumerate() {
        for &i in samples {
            data.depth_views[d].unknown.insert(i as usize);
        }
        stats.depth_samples += samples.len();
    }

    // Each view's pixels: the bodies of the slices nearest it in time, carried to where the hips were at its moment.
    let radius_world = 0.87 * voxel + MASK_MARGIN_PER_EYE * eye;
    let posed: Vec<&Slice> = slices.iter().filter(|s| !s.body.is_empty()).collect();
    let mask_for = |v: &View, slices: &[&Slice]| -> Option<Vec<bool>> {
        let f = track.nearest(v.game_time);
        let turn_to = rotation_of(f);
        let mut near: Vec<&&Slice> = slices.iter().collect();
        near.sort_by(|a, b| (a.time - v.game_time).abs().total_cmp(&(b.time - v.game_time).abs()));
        let bodies: Vec<(&[[f32; 3]], [f32; 3], [f32; 3], [f32; 4])> =
            near.iter().take(SLICES_PER_VIEW).map(|s| (s.body.as_slice(), s.hips, f.hips, quat_mul(turn_to, quat_conj(s.rotation)))).collect();
        view_mask(&v.camera, &bodies, radius_world, MASK_DILATE_1080P)
    };
    let mut out = 0u64;
    let mut pixels = 0u64;
    let mut touched = 0usize;
    let counts: Vec<usize> = data.views.par_iter_mut().map(|v| mask_for(v, &posed).map_or(0, |m| leave_out(v, &m))).collect();
    for c in &counts {
        out += *c as u64;
        touched += usize::from(*c > 0);
        pixels += (data.width * data.height) as u64;
    }
    if let Some(f) = &mut data.faces {
        let counts: Vec<usize> = f.views.par_iter_mut().map(|v| mask_for(v, &posed).map_or(0, |m| leave_out(v, &m))).collect();
        for (c, v) in counts.iter().zip(&f.views) {
            out += *c as u64;
            touched += usize::from(*c > 0);
            pixels += (v.camera.width * v.camera.height) as u64;
        }
    }
    stats.views = touched;
    stats.views_total = data.views.len() + data.faces.as_ref().map_or(0, |f| f.views.len());
    stats.pixel_share = out as f32 / pixels.max(1) as f32;

    // The volume, a voxel fatter than what depth saw: splats sit on surfaces, and a little off them while they train. A body
    // that moved has no one volume: what lies where it once stood may well be the world.
    let still = posed.len() == 1 || slices[best_slice].frames.len() * 5 >= data.depth_views.len() * 4;
    let mut volume = Volume { voxel, cells: HashSet::new(), floor, hips: slices[best_slice].hips, eye_height: eye };
    if still {
        for k in &cells {
            volume.cells.insert(*k);
            volume.cells.extend(neighbours(*k).filter(|n| (n[0] - k[0]).abs() <= 1 && (n[1] - k[1]).abs() <= 1 && (n[2] - k[2]).abs() <= 1));
        }
    } else {
        volume.floor = None;
    }
    Some((volume, stats))
}

/// The loss mask of a session the wearer left. The device keeps one mask per image size (crate::gpu::Gpu::set_mask), and here
/// each view has its own: a step writes its view's before it renders. Writing them costs a pass over a view's pixels and an
/// upload as large as a keyframe's, so a session nobody left never does either: [`LossMask::new`] sees no pixel left out, and the
/// run is the same step for step as before.
pub struct LossMask {
    active: bool,
    buffer: Vec<f32>,
}

impl LossMask {
    pub fn new(data: &Dataset) -> Self {
        let any = |views: &[View]| views.par_iter().any(|v| v.image.par_iter().any(|p| is_out(*p)));
        let active = any(&data.views) || data.faces.as_ref().is_some_and(|f| any(&f.views));
        Self { active, buffer: Vec::new() }
    }

    /// Sets the mask for this step's `target` pixels of `width × height`: `base`, less what shows the wearer.
    pub fn show(&mut self, gpu: &mut crate::gpu::Gpu, width: u32, height: u32, base: &[f32], target: &[u32]) -> anyhow::Result<()> {
        if !self.active {
            return Ok(());
        }
        self.buffer.clear();
        self.buffer.extend(base.iter().zip(target).map(|(m, p)| if is_out(*p) { 0.0 } else { *m }));
        gpu.set_mask(width, height, &self.buffer)
    }

    /// Puts back the session's masks: what scores and visibility rounds read between the steps.
    pub fn settle(&mut self, gpu: &mut crate::gpu::Gpu, data: &Dataset) -> anyhow::Result<()> {
        if !self.active {
            return Ok(());
        }
        gpu.set_mask(data.width, data.height, &data.mask)?;
        for s in data.faces.iter().flat_map(|f| &f.sizes).filter(|s| (s.width, s.height) != (data.width, data.height)) {
            gpu.set_mask(s.width, s.height, &s.mask)?;
        }
        Ok(())
    }
}

/// `mask` less what shows the wearer in `image`, for a score on the CPU.
pub fn scene<'a>(mask: &'a [f32], image: &[u32]) -> std::borrow::Cow<'a, [f32]> {
    if image.iter().any(|p| is_out(*p)) {
        std::borrow::Cow::Owned(mask.iter().zip(image).map(|(m, p)| if is_out(*p) { 0.0 } else { *m }).collect())
    } else {
        std::borrow::Cow::Borrowed(mask)
    }
}

/// Clears from `mask` the pixels of `image` that show the wearer.
pub fn leave_out_of(mask: &mut [f32], image: &[u32]) {
    for (m, p) in mask.iter_mut().zip(image) {
        if is_out(*p) {
            *m = 0.0;
        }
    }
}

/// For looking at it: every voxel around the first slice's hips, as (centre, how many samples, 0 neither, 1 world, 2 body).
pub fn labelled(data: &Dataset, track: &Track) -> Vec<([f32; 3], u32, u8)> {
    let eye = track.eye_height;
    let voxel = VOXEL_PER_EYE * eye;
    let slices = slices_of(&data.depth_views, track);
    let Some(slice) = slices.first() else { return Vec::new() };
    let hips = slice.hips;
    let samples: Vec<(u32, [f32; 3], [f32; 3])> = slice.frames.iter().flat_map(|&d| near_samples(&data.depth_views[d], hips, (ENVELOPE_RADIUS + SHELL) * eye, hips[1] - 1.3 * eye, hips[1] + 1.3 * eye)).collect();
    let found = find_body(&samples, hips, eye);
    let mut cells: HashMap<Key, u32> = HashMap::new();
    for (_, p, _) in &samples {
        *cells.entry(key(*p, voxel)).or_default() += 1;
    }
    cells.iter().map(|(k, n)| (k.map(|c| (c as f32 + 0.5) * voxel), *n, if found.body.contains(k) { 2 } else if found.world.contains(k) { 1 } else { 0 })).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Geometry;
    use crate::dataset::{DepthGrid, Marks, View};
    use orbiters_contracts::skeleton::BONES;
    use std::sync::Arc;

    const W: u32 = 320;
    const H: u32 = 180;

    fn grid() -> Arc<DepthGrid> {
        let (gw, gh) = (W / 2, H / 2);
        let pixels = (0..gh).flat_map(|y| (0..gw).map(move |x| [x as f32 * 2.0 + 0.5, y as f32 * 2.0 + 0.5])).collect();
        Arc::new(DepthGrid { width: gw, height: gh, pixels, spacing: 2.0, unused: Marks::default(), steps: Vec::new() })
    }

    /// A little world by ray casting: a floor at y = 0, a wall at z = 6, two standing boxes.
    struct Scene {
        boxes: Vec<([f32; 3], [f32; 3])>,
    }

    impl Scene {
        fn cast(&self, o: [f32; 3], d: [f32; 3]) -> f32 {
            let mut t = f32::INFINITY;
            if d[1] < -1e-6 {
                t = t.min(-o[1] / d[1]);
            }
            if d[2] > 1e-6 {
                t = t.min((6.0 - o[2]) / d[2]);
            }
            for (lo, hi) in &self.boxes {
                let (mut t0, mut t1) = (0f32, f32::INFINITY);
                let mut hit = true;
                for a in 0..3 {
                    if d[a].abs() < 1e-7 {
                        hit &= o[a] > lo[a] && o[a] < hi[a];
                        continue;
                    }
                    let (p, q) = ((lo[a] - o[a]) / d[a], (hi[a] - o[a]) / d[a]);
                    t0 = t0.max(p.min(q));
                    t1 = t1.min(p.max(q));
                }
                if hit && t0 < t1 {
                    t = t.min(t0);
                }
            }
            t
        }

        fn depth_view(&self, eye: [f32; 3], at: [f32; 3], time: f32, g: &Arc<DepthGrid>) -> DepthView {
            let forward = sub(at, eye);
            let camera = Camera::look(eye, forward, [0.0, 1.0, 0.0], 1.0, W, H);
            let (mut depth, c) = (Vec::with_capacity(g.len()), camera.position());
            for [u, v] in &g.pixels {
                let far = camera.unproject(*u, *v, 1.0);
                let d = sub(far, c);
                let t = self.cast(c, d);
                // `d` is a ray of depth 1: its parameter is the eye depth.
                depth.push(if t.is_finite() { t } else { 0.0 });
            }
            DepthView { camera, grid: g.clone(), depth, game_time: time, unknown: Marks::default(), decoder_unknown: 0, zero_may_be_unread: false }
        }
    }

    fn frame(time: f32, hips: [f32; 3]) -> skeleton_frames::Frame {
        let mut rotations = [None; BONES];
        rotations[0] = Some([0.0, 0.0, 0.0, 1.0]);
        skeleton_frames::Frame { time_ns: (time * 1e9) as u64, game_time: time, avatar: 1, hips, rotations }
    }

    /// Orbiting views around (0, 1, 2).
    fn eyes() -> Vec<[f32; 3]> {
        (0..16).map(|k| { let a = k as f32 * 0.39 - 3.0; [a.cos() * 2.5, 1.2 + 0.6 * (k % 3) as f32, 2.0 + a.sin() * 2.5] }).collect()
    }

    fn dataset(scene: &Scene) -> Dataset {
        let g = grid();
        let target = [0.0, 1.0, 2.0];
        let depth_views: Vec<DepthView> = eyes().iter().enumerate().map(|(k, e)| scene.depth_view(*e, target, k as f32, &g)).collect();
        let views: Vec<View> = eyes().iter().enumerate().map(|(k, e)| View { camera: Camera::look(*e, sub(target, *e), [0.0, 1.0, 0.0], 1.0, W, H), image: vec![0xFF80_8080; (W * H) as usize], game_time: k as f32, keyframe: k as i32 }).collect();
        Dataset { dir: Default::default(), views, depth_views, faces: None, width: W, height: H, geometry: Geometry::new(W, H, 1), mask: vec![1.0; (W * H) as usize], loaded: Default::default() }
    }

    fn track() -> Track {
        Track { frames: (0..20).map(|k| frame(k as f32, [0.0, 1.0, 2.0])).collect(), eye_height: 1.7 }
    }

    fn body_box() -> ([f32; 3], [f32; 3]) {
        ([-0.2, 0.0, 1.85], [0.2, 1.9, 2.15])
    }

    /// The wearer goes: their pixels and depth samples leave, the floor and wall keep theirs, and a box standing a hand's width
    /// beyond their reach stays whole.
    #[test]
    fn the_wearer_goes_and_what_stands_beside_them_stays() {
        let friend = ([0.55, 0.0, 1.8], [0.95, 1.9, 2.2]);
        let scene = Scene { boxes: vec![body_box(), friend] };
        let mut data = dataset(&scene);
        let (volume, stats) = remove(&mut data, &track()).expect("found");
        assert!(stats.depth_samples > 0 && stats.views > 0 && stats.floor.is_some_and(|f| f.abs() < 0.05), "{stats:?}");
        // Depth: on the body unknown, on the friend, floor and wall known.
        let (mut body_kept, mut world_lost, mut friend_lost, mut friend_all) = (0, 0, 0, 0);
        for d in &data.depth_views {
            for i in 0..d.depth.len() {
                if d.depth[i] <= 0.0 {
                    continue;
                }
                let [u, v] = d.grid.pixels[i];
                let p = d.camera.unproject(u, v, d.depth[i]);
                let inside = |b: ([f32; 3], [f32; 3]), m: f32| (0..3).all(|a| p[a] > b.0[a] - m && p[a] < b.1[a] + m);
                let high = p[1] > 0.1;
                if inside(body_box(), 1e-3) && high {
                    body_kept += usize::from(!d.is_unknown(i));
                } else if inside(friend, 1e-3) {
                    friend_all += 1;
                    friend_lost += usize::from(d.is_unknown(i));
                } else if !inside(body_box(), 0.1) {
                    world_lost += usize::from(d.is_unknown(i));
                }
            }
        }
        assert!(friend_all > 500);
        assert_eq!(friend_lost, 0, "the friend stays");
        assert!(body_kept * 200 < stats.depth_samples, "the body goes: {body_kept} of {}", stats.depth_samples);
        assert!(world_lost * 200 < stats.depth_hits, "the world stays: {world_lost} of {}", stats.depth_hits);
        // Pixels: a body point is out in a view that sees it; a point on the wall far from it is not.
        let v = &data.views[6];
        let px = |p: [f32; 3]| {
            let (u, vv, _) = v.camera.project(p).expect("in front");
            v.image[(vv.round() as u32 * W + u.round() as u32) as usize]
        };
        assert!(is_out(px([0.0, 1.0, 2.0])), "the middle of the body");
        assert!(!is_out(px([0.8, 0.0, 3.4])), "the floor well away");
        // The volume: the body in it, the floor and the friend's far side not.
        assert!(volume.contains([0.0, 1.5, 1.85]));
        assert!(!volume.contains([0.75, 1.0, 1.8]));
        assert!(!volume.contains([0.0, 0.0, 3.0]));
    }

    /// How close someone may stand: at a hand's width from the body they stay whole; touching, they go with it, as far as the
    /// envelope: depth has no seam between two bodies, and nothing else tells them apart (reports/wearer-out.md).
    #[test]
    fn someone_a_hand_away_stays_and_someone_touching_goes_with_the_wearer() {
        let share_kept = |friend: ([f32; 3], [f32; 3])| {
            let scene = Scene { boxes: vec![body_box(), friend] };
            let mut data = dataset(&scene);
            remove(&mut data, &track()).expect("found");
            let (mut all, mut lost) = (0, 0);
            for d in &data.depth_views {
                for i in 0..d.depth.len() {
                    if d.depth[i] <= 0.0 {
                        continue;
                    }
                    let [u, v] = d.grid.pixels[i];
                    let p = d.camera.unproject(u, v, d.depth[i]);
                    if p[1] > 0.1 && (0..3).all(|a| p[a] > friend.0[a] && p[a] < friend.1[a]) {
                        all += 1;
                        lost += usize::from(d.is_unknown(i));
                    }
                }
            }
            1.0 - lost as f32 / all as f32
        };
        let near = share_kept(([0.3, 0.0, 1.8], [0.7, 1.9, 2.2]));
        assert!(near > 0.99, "a 10 cm gap: {near}");
        assert!(share_kept(([0.2, 0.0, 1.8], [0.6, 1.9, 2.2])) < 0.5, "touching");
    }

    /// With no one there, nothing is touched.
    #[test]
    fn no_wearer_in_the_depth_leaves_everything() {
        let scene = Scene { boxes: Vec::new() };
        let mut data = dataset(&scene);
        assert!(remove(&mut data, &track()).is_none());
        assert!(data.views.iter().all(|v| v.image.iter().all(|p| !is_out(*p))));
        assert!(data.depth_views.iter().all(|d| d.unknown.is_empty()));
    }

    /// A wearer who walks: each slice has its own body, and no volume stands anywhere.
    #[test]
    fn a_moving_wearer_is_found_where_they_are_and_leaves_no_volume() {
        // The body moved half way through the session: the frames say so through the hips.
        let t = Track { frames: (0..20).map(|k| frame(k as f32, if k < 8 { [0.0, 1.0, 2.0] } else { [0.6, 1.0, 2.0] })).collect(), eye_height: 1.7 };
        let g = grid();
        let target = [0.0, 1.0, 2.0];
        let first = Scene { boxes: vec![body_box()] };
        let moved_box = ([0.4, 0.0, 1.85], [0.8, 1.9, 2.15]);
        let second = Scene { boxes: vec![moved_box] };
        let depth_views: Vec<DepthView> = eyes().iter().enumerate().map(|(k, e)| if k < 8 { first.depth_view(*e, target, k as f32, &g) } else { second.depth_view(*e, target, k as f32, &g) }).collect();
        let mut data = dataset(&first);
        data.depth_views = depth_views;
        let (volume, stats) = remove(&mut data, &t).expect("found");
        assert_eq!(stats.slices, 2);
        assert!(volume.is_empty(), "no one place held it for most of the session");
        // In a later view, the body is out where it stands then, and where it stood is not.
        let v = &data.views[12];
        let at = |p: [f32; 3]| {
            let (u, vv, _) = v.camera.project(p).expect("in front");
            is_out(v.image[(vv.round() as u32 * W + u.round() as u32) as usize])
        };
        assert!(at([0.6, 1.2, 2.0]));
    }

    /// What only the wearer's pixels showed goes when training ends; what other views saw stays, and so does what lies further off.
    #[test]
    fn what_no_pixel_left_in_saw_goes() {
        let scene = Scene { boxes: vec![body_box()] };
        let mut data = dataset(&scene);
        let (volume, _) = remove(&mut data, &track()).expect("found");
        let mut g = Gaussians::default();
        let at = |g: &mut Gaussians, p: [f32; 3]| g.push(p, [(-3f32); 3], [1.0, 0.0, 0.0, 0.0], crate::gaussians::logit(0.9), [0.5; 3]);
        at(&mut g, [0.6, 0.0, 2.0]); // the floor beside them: seen
        at(&mut g, [0.0, 0.0, 2.0]); // under their feet, in the middle of the box: every view has it behind them
        at(&mut g, [5.0, 0.0, 2.0]); // far off, out of every view: not this sweep's to judge
        let out = leave_out_unseen(&data, &[], &volume, &mut g);
        assert_eq!(out, 1);
        assert_eq!(g.means, vec![[0.6, 0.0, 2.0], [5.0, 0.0, 2.0]]);
    }

    #[test]
    fn floor_patches_fill_what_the_floor_encloses_and_nothing_else() {
        let mut g = Gaussians::default();
        let volume = Volume { voxel: 0.03, cells: HashSet::new(), floor: Some(0.0), hips: [0.0, 1.0, 0.0], eye_height: 1.8 };
        // A floor with a hole around the origin, of radius 0.2 m.
        let mut x = -1.0;
        while x < 1.0 {
            let mut z = -1.0;
            while z < 1.0 {
                if (x as f32).hypot(z as f32) > 0.2 {
                    g.push([x, 0.0, z], [(-3f32); 3], [1.0, 0.0, 0.0, 0.0], crate::gaussians::logit(0.9), [0.3, 0.4, 0.5]);
                }
                z += 0.03;
            }
            x += 0.03;
        }
        let before = g.len();
        let added = patch_floor(&volume, &mut g);
        assert!(added > 10, "{added}");
        // Only inside the hole, in the floor's colour.
        for i in before..g.len() {
            assert!(g.means[i][0].hypot(g.means[i][2]) < 0.3, "{:?}", g.means[i]);
            assert!((g.colors[i][1] - 0.4).abs() < 0.02, "{:?}", g.colors[i]);
        }
        // Over a floor with open sides, nothing.
        let mut g = Gaussians::default();
        g.push([0.2, 0.0, 0.2], [(-3f32); 3], [1.0, 0.0, 0.0, 0.0], crate::gaussians::logit(0.9), [0.3, 0.4, 0.5]);
        assert_eq!(patch_floor(&volume, &mut g), 0);
    }

    #[test]
    fn quaternions_carry_points() {
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let y90 = [0.0, half, 0.0, half];
        let v = rotate(y90, [1.0, 0.0, 0.0]);
        assert!((v[0]).abs() < 1e-5 && (v[2] + 1.0).abs() < 1e-5, "{v:?}");
        let back = quat_mul(y90, quat_conj(y90));
        assert!((back[3].abs() - 1.0).abs() < 1e-5);
        assert!((angle_between(y90, [0.0, 0.0, 0.0, 1.0]) - 90.0).abs() < 0.1);
    }
}
