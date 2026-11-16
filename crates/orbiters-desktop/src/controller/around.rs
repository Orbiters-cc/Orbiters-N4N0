//! The 3D picture's flight around the people in it, and through the room they stand in, as far as depth says the room
//! goes.
//!
//! Why this exists: Enzo's picture of 2026-11-04 (ちま部屋, 4 s) looked perfect from where it was taken and terrible once
//! turned to the side. Nothing moved (the avatar's hips and head bone were constant through 172 skeleton frames) and the
//! splats were no worse shaped than in a good model: the flight simply never saw him from there. Of its 37 full-resolution
//! views, none showed him from more than about 34° off the start's direction, and the rig's low-resolution faces from
//! 70° at the most. The flight circled the camera's own place (where the wearer stands when the camera is held in front of
//! them, but he stood 2.6 m ahead of it) and then spent 1.4 of its 4 s outside the building, through its walls. On a
//! real ring around a still avatar (VRChat Home, 2026-10-21, a held-out view every 18°), views from one side only left the
//! body at 18.5 dB from the other sides (LPIPS 0.39), and a ring of views 13° apart at 23.4 dB (LPIPS 0.15), the room with it:
//! the desktop repository's docs/3d-picture-angles-2026-11-04.md.
//!
//! What it does, in the order the camera does it:
//!
//! 1. **Look**: one level turn where the camera stands. It costs 0.7 s and is what tells the flight where it can go: every
//!    depth frame says what is free up to the surface it measured ([`Seen`]). VRChat's camera passes through walls, and a
//!    view from inside one, or with something between it and the person, is wasted time.
//! 2. **People**: for each person ([`people`]), sweeps around them ([`Kind`]): their whole body at chest height, then head
//!    and chest closer and a little from above, and with an 8 s picture once more from higher up. Radii and arcs are
//!    limited to what depth has seen free ([`Sweep`]), so a person with their back to a wall gets the arc in front of them,
//!    there and back. Between sweeps the depth frames of the last one extend what is known.
//! 3. **Room**: the rig on, hops between the places farthest from where the camera has been, each straight line checked
//!    against depth, then home: the exact pose it started from. VRChat's zoom is never touched.
//!
//! Two pictures, and the split of their seconds ([`Split`]). **People:** of a 4 s flight 0.7 s look, 2.9 s people, 0.4 s room; of
//! an 8 s flight 0.7 s look, 5.8 s people, 1.5 s room. **World:** the look, then only the place: 3.3 s and 7.3 s, nobody flown
//! around. Why these: on a real ring around a still avatar (docs/3d-picture-angles-2026-11-04.md), going from views of one
//! side to views 13° apart gave the body 5 dB from the sides, and from 13° to 4° apart another 0.6 dB and the room around it 1.5 dB.
//! A person gains most from every angle covered, so People gives them what the room had, at 6° between colour frames (the first
//! version, one split for all, had 2.2 s and 4 s).
//!
//! A picture of the world goes in a ring around the middle of what the opening turn saw ([`middle_of`]), looking across it, then
//! turns where it is looking up (and down: [`TILTED_TURNS`]), which is what tells it the floor and the ceiling and whether the
//! 8 s picture's rings above and below ([`RINGS`]) can fly, then hops as above, filling in what lies closer together
//! ([`WORLD_LEAST_NEW_M`]). Hops alone saw half the surface the sweeps around a person did on Enzo's recordings of 2026-11-04 (Dia
//! de Muertos), and rings looking outward saw little more: a wall a metre away fills the frame. Looking across, every frame sees
//! the far side of the place, from places metres apart, which is what depth and colour need of it.
//!
//! The flight's rig stays off around people, so every colour frame is a whole 1080p picture of them, with six times the pixels
//! per degree of the rig's faces (420 px over 90° against 1234 px focal length). A sweep needs no pauses: every frame
//! carries its exact pose, and VRChat's stream gave about 37 frames a second during flights, 3 in 4 of them colour, so 360°
//! in 1.2 s is a whole picture every 8°.

use super::dolly::axes;
use orbiters_contracts::depth::{self, DepthLayout};
use orbiters_contracts::frame_record::{record_flags, FrameRecord, RECORD_SIZE};
use orbiters_contracts::manifest::{depth_file, DEPTH_DIR, FRAMES_FILE};
use orbiters_core::settings::{Photo3dLength, PictureMode};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

pub type Pose = [f32; 6];
type V = [f32; 3];

fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: V, b: V) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn length(a: V) -> f32 {
    dot(a, a).sqrt()
}

fn flat(a: V, b: V) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn lerp(a: V, b: V, t: f32) -> V {
    std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
}

/// A depth frame's samples go into bins this many across and down, about 40 pixels each at 1080p: the nearest surface in
/// each. A bin's nearest surface errs on the safe side, and the grid's sparser samples towards the frame's edges still fill
/// their bins (contracts' Docs/depth-frames.md).
const BINS_X: usize = 48;
const BINS_Y: usize = 27;

/// What depth frames say about a place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clear {
    /// A depth frame's ray went on past it.
    Free,
    /// A surface was measured there.
    Surface,
    /// No depth frame saw it, or only from behind something.
    Unknown,
}

/// One depth frame, in bins.
#[derive(Clone, Debug)]
pub struct SeenFrame {
    pub eye: V,
    right: V,
    up: V,
    forward: V,
    /// P00, P11, P02, P12.
    projection: [f32; 4],
    /// The nearest surface in each bin (eye depth, metres): infinity when only sky was seen, NaN when nothing was read.
    bins: Vec<f32>,
    /// For a bin of sky, the nearest surface around it ([`rims_of`]): how far that sky vouches for.
    rims: Vec<f32>,
}

/// How many bins around a bin of sky are looked at for what surrounds it: about 11°, a hole of 2.2 m at 7 m.
const RIM_BINS: usize = 8;

/// Each bin's nearest surface within [`RIM_BINS`], for those that read sky. Depth has no surface where a world's material writes
/// none: glass, and on Rainy Skyline, a closed door. "Sky" in a hole in a wall says nothing of what lies past that wall, and the
/// flight of 2026-11-06 went through such a door 7 m away, out of the building into the night, and came back with the city in its
/// frames. Real open sky has no rim close by, or one far off.
fn rims_of(bins: &[f32]) -> Vec<f32> {
    let mut rims = vec![f32::INFINITY; bins.len()];
    for (bin, rim) in rims.iter_mut().enumerate() {
        if !bins[bin].is_infinite() {
            continue;
        }
        let (x, y) = (bin % BINS_X, bin / BINS_X);
        for ny in y.saturating_sub(RIM_BINS)..(y + RIM_BINS + 1).min(BINS_Y) {
            for nx in x.saturating_sub(RIM_BINS)..(x + RIM_BINS + 1).min(BINS_X) {
                let z = bins[ny * BINS_X + nx];
                if z.is_finite() && z < *rim {
                    *rim = z;
                }
            }
        }
    }
    rims
}

impl SeenFrame {
    /// A frame's bins from its grid of depth codes. `None` when the codes do not fit the frame's depth layout.
    pub fn new(camera_to_world: &[f32; 12], projection: &[f32; 4], layout: &DepthLayout, codes: &[u16]) -> Option<Self> {
        let (gw, gh) = layout.grid_size();
        if codes.len() != (gw * gh) as usize {
            return None;
        }
        // Each bin's two nearest samples: the second is what counts, since a sample on a silhouette's edge can read anywhere
        // between the person and the wall behind them (the stars around objects in the capture's top view,
        // docs/3d-picture-angles-2026-11-04.md), and a real surface fills more than one of a bin's samples.
        let mut near = vec![(f32::INFINITY, f32::INFINITY); BINS_X * BINS_Y];
        let mut read = vec![0u16; BINS_X * BINS_Y];
        for gy in 0..gh {
            for gx in 0..gw {
                let code = codes[(gy * gw + gx) as usize];
                if code == depth::UNKNOWN {
                    continue;
                }
                let (px, py) = layout.grid_pixel(gx, gy);
                let (bx, by) = (px as usize * BINS_X / layout.width as usize, py as usize * BINS_Y / layout.height as usize);
                let bin = by.min(BINS_Y - 1) * BINS_X + bx.min(BINS_X - 1);
                let z = depth::dequantize(code).unwrap_or(f32::INFINITY);
                read[bin] += 1;
                let n = &mut near[bin];
                if z < n.0 {
                    *n = (z, n.0);
                } else if z < n.1 {
                    n.1 = z;
                }
            }
        }
        let bins = near
            .iter()
            .zip(&read)
            .map(|(n, r)| {
                if *r == 0 {
                    f32::NAN
                } else if *r == 1 {
                    n.0
                } else {
                    n.1
                }
            })
            .collect();
        let m = camera_to_world;
        let bins: Vec<f32> = bins;
        let rims = rims_of(&bins);
        Some(Self { eye: [m[3], m[7], m[11]], right: [m[0], m[4], m[8]], up: [m[1], m[5], m[9]], forward: [m[2], m[6], m[10]], projection: *projection, bins, rims })
    }

    /// A frame's bins as they stand (test data).
    #[cfg(test)]
    pub fn from_bins(camera_to_world: &[f32; 12], projection: &[f32; 4], bins: Vec<f32>) -> Self {
        let m = camera_to_world;
        assert_eq!(bins.len(), BINS_X * BINS_Y);
        let rims = rims_of(&bins);
        Self { eye: [m[3], m[7], m[11]], right: [m[0], m[4], m[8]], up: [m[1], m[5], m[9]], forward: [m[2], m[6], m[10]], projection: *projection, bins, rims }
    }

    /// The bin a place falls in, its eye depth: `None` behind the camera or outside the frame.
    fn bin_of(&self, p: V) -> Option<(usize, f32)> {
        let rel = sub(p, self.eye);
        let z = dot(rel, self.forward);
        if z < 0.1 {
            return None;
        }
        let [p00, p11, p02, p12] = self.projection;
        let u = 0.5 * p00 * dot(rel, self.right) / z + 0.5 * (1.0 - p02);
        let v = 0.5 * p11 * -dot(rel, self.up) / z + 0.5 * (1.0 + p12);
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            return None;
        }
        Some(((v * BINS_Y as f32) as usize * BINS_X + (u * BINS_X as f32) as usize, z))
    }

    /// What this frame says about a place, within `margin` of a surface counting as on it.
    fn at(&self, p: V, margin: f32) -> Clear {
        let Some((bin, z)) = self.bin_of(p) else { return Clear::Unknown };
        let surface = self.bins[bin];
        if surface.is_nan() {
            Clear::Unknown
        } else if surface.is_infinite() {
            // Sky: free up to what surrounds it, and past that nobody knows.
            if z < self.rims[bin] - margin {
                Clear::Free
            } else {
                Clear::Unknown
            }
        } else if z < surface - margin {
            Clear::Free
        } else if z <= surface + margin {
            Clear::Surface
        } else {
            Clear::Unknown
        }
    }

    /// The world point of a bin's nearest surface.
    fn surface_point(&self, bin: usize) -> Option<V> {
        let z = self.bins[bin];
        if !z.is_finite() {
            return None;
        }
        let [p00, p11, p02, p12] = self.projection;
        let u = ((bin % BINS_X) as f32 + 0.5) / BINS_X as f32;
        let v = ((bin / BINS_X) as f32 + 0.5) / BINS_Y as f32;
        let x = (u - 0.5 * (1.0 - p02)) * 2.0 / p00 * z;
        let y = -(v - 0.5 * (1.0 + p12)) * 2.0 / p11 * z;
        Some(std::array::from_fn(|k| self.eye[k] + self.right[k] * x + self.up[k] * y + self.forward[k] * z))
    }
}

/// Everything the depth frames have shown so far. Queries ask every frame: a place is free when any of them measured past
/// it and none measured a surface there.
#[derive(Clone, Debug, Default)]
pub struct Seen {
    pub frames: Vec<SeenFrame>,
    /// A wearer who stays in the frames and leaves the picture afterwards (crate wearer): no obstacle.
    pub left_out: Option<LeftOut>,
    /// What no depth frame has seen counts as free: a picture of people whose opening turns gave too few depth frames still
    /// goes around them, as the blind flight went everywhere, and stops only at what a frame did measure. Without this, Enzo's
    /// pictures of people on 2026-11-15 (4 of 10) flew the blind flight of a picture of the world: around the camera's own
    /// place and 10 m out, never around the friends they were of.
    pub blind: bool,
}

/// Where a wearer stands who is in the depth now and will not be in the picture. In the corridor of the first real picture
/// (2026-11-06, 1.7 m wide) their body closed the way behind the camera: the flight went the other way, twice, and half the place
/// stayed unseen. A wearer the avatar hides is in no depth, and flights around them already go there.
#[derive(Clone, Copy, Debug)]
pub struct LeftOut {
    pub hips: V,
    pub eye_height: f32,
}

impl LeftOut {
    /// Wide enough for arms held close and narrow enough to leave the walls of a corridor: in eye heights.
    const RADIUS: f32 = 0.35;
    const BELOW: f32 = 0.7;
    const ABOVE: f32 = 0.9;

    fn holds(&self, p: V) -> bool {
        flat(p, self.hips) < Self::RADIUS * self.eye_height && p[1] > self.hips[1] - Self::BELOW * self.eye_height && p[1] < self.hips[1] + Self::ABOVE * self.eye_height
    }
}

impl Seen {
    /// A frame's evidence, unless a frame from the same place looking the same way already gave it: the picture's pause
    /// makes dozens of those, whose votes would all be one.
    pub fn add(&mut self, mut frame: SeenFrame) {
        if let Some(wearer) = self.left_out {
            // As if that bin read nothing: what is behind the wearer is unknown from here, and other frames say what it is.
            for bin in 0..frame.bins.len() {
                if frame.surface_point(bin).is_some_and(|p| wearer.holds(p)) {
                    frame.bins[bin] = f32::NAN;
                }
            }
            frame.rims = rims_of(&frame.bins);
        }
        let same = |f: &SeenFrame| length(sub(f.eye, frame.eye)) < 0.02 && dot(f.forward, frame.forward) > 0.9995 && dot(f.up, frame.up) > 0.9995;
        if !self.frames.iter().any(same) {
            self.frames.push(frame);
        }
    }

    pub fn at(&self, p: V, margin: f32) -> Clear {
        let (mut free, mut surface) = (0, 0);
        for f in &self.frames {
            match f.at(p, margin) {
                Clear::Surface => surface += 1,
                Clear::Free => free += 1,
                Clear::Unknown => {}
            }
        }
        // A real surface hides what is behind it from every side, and a frame cannot measure past it. A surface only one frame
        // in several saw is a depth sample torn off an edge (the depth frames' stars around objects in the top view of
        // the capture, docs/3d-picture-angles-2026-11-04.md), in the air.
        if surface > 0 && free < TORN_OFF * surface {
            Clear::Surface
        } else if free > 0 || self.blind {
            Clear::Free
        } else {
            Clear::Unknown
        }
    }

    /// Whether the camera can stand at `p`: seen free, with no surface [`EYE_MARGIN`] around it any way.
    fn eye_clear(&self, p: V) -> Clear {
        match self.at(p, EYE_MARGIN) {
            Clear::Free => {}
            other => return other,
        }
        let e = EYE_MARGIN;
        for d in [[e, 0.0, 0.0], [-e, 0.0, 0.0], [0.0, e, 0.0], [0.0, -e, 0.0], [0.0, 0.0, e], [0.0, 0.0, -e]] {
            if self.at([p[0] + d[0], p[1] + d[1], p[2] + d[2]], SIGHT_MARGIN) == Clear::Surface {
                return Clear::Surface;
            }
        }
        Clear::Free
    }

    /// Whether a straight line is free: no surface on it, and most of it seen free (what a person hides from earlier frames
    /// is unknown, not blocked). `Unknown` when too little of it was seen.
    fn line_clear(&self, a: V, b: V, margin: f32) -> Clear {
        let steps = ((length(sub(b, a)) / LINE_STEP).ceil() as usize).max(1);
        let mut free = 0;
        for i in 0..=steps {
            match self.at(lerp(a, b, i as f32 / steps as f32), margin) {
                Clear::Surface => return Clear::Surface,
                Clear::Free => free += 1,
                Clear::Unknown => {}
            }
        }
        if free * 2 >= steps + 1 {
            Clear::Free
        } else {
            Clear::Unknown
        }
    }

    /// The distance along a ray from the frames taken where `eye` is: the median of their nearest surfaces around it, at
    /// `spread` (a tangent) on either side. `None` where they read nothing.
    fn along(&self, eye: V, direction: V) -> Option<f32> {
        let mut found: Vec<f32> = Vec::new();
        for f in self.frames.iter().filter(|f| length(sub(f.eye, eye)) < 0.05) {
            let at: V = std::array::from_fn(|k| f.eye[k] + direction[k]);
            if let Some((bin, _)) = f.bin_of(at) {
                if f.bins[bin].is_finite() {
                    found.push(f.bins[bin]);
                }
            }
        }
        if found.is_empty() {
            return None;
        }
        found.sort_by(f32::total_cmp);
        Some(found[found.len() / 2])
    }

    /// The ground under a person, from the lowest surfaces measured beside their axis, at least [`ABOVE_GROUND_LEAST`] under
    /// the head. `None` when no frame saw any there (a framing that cut off the legs).
    fn ground_under(&self, head: V) -> Option<f32> {
        let mut heights = Vec::new();
        for f in &self.frames {
            for bin in 0..f.bins.len() {
                if let Some(p) = f.surface_point(bin) {
                    if flat(p, head) < 0.7 && p[1] < head[1] - ABOVE_GROUND_LEAST && p[1] > head[1] - ABOVE_GROUND_MOST {
                        heights.push(p[1]);
                    }
                }
            }
        }
        if heights.len() < 3 {
            return None;
        }
        heights.sort_by(f32::total_cmp);
        Some(heights[heights.len() / 20])
    }
}

/// How many frames must have measured past a place for one that measured a surface there to count as a stray.
const TORN_OFF: usize = 3;
/// A camera keeps this far from anything, metres: the sweeps' margin. VRChat's near plane is 5 cm, but a surface a few
/// centimetres in front of the camera fills the frame.
const EYE_MARGIN: f32 = 0.25;
/// What may graze a line of sight, metres.
const SIGHT_MARGIN: f32 = 0.08;
/// What may graze a line of flight, metres (a door's lintel at the height it flies, say).
const FLIGHT_MARGIN: f32 = 0.2;
/// Spacing of a line's checks, metres.
const LINE_STEP: f32 = 0.2;
/// A person's ground lies at least this under their head (a sitting avatar's is about this), metres...
const ABOVE_GROUND_LEAST: f32 = 0.4;
/// ...and at most this: anything lower is another floor.
const ABOVE_GROUND_MOST: f32 = 3.0;

/// Reads the depth frames of a recording under way: each one's record reaches `frames.bin` once its file is written
/// (orbiters_core::bundle flushes it then).
pub struct LiveDepth {
    dir: PathBuf,
    read: u64,
    layout: Option<DepthLayout>,
}

impl LiveDepth {
    pub fn new(dir: &Path) -> Self {
        Self { dir: dir.to_path_buf(), read: 0, layout: None }
    }

    /// Takes what has come since the last call into `seen`.
    pub fn poll(&mut self, seen: &mut Seen) {
        use std::io::{Read as _, Seek as _, SeekFrom};
        let Ok(mut file) = std::fs::File::open(self.dir.join(FRAMES_FILE)) else { return };
        let mut bytes = Vec::new();
        if file.seek(SeekFrom::Start(self.read)).is_err() || file.read_to_end(&mut bytes).is_err() {
            return;
        }
        let whole = bytes.len() - bytes.len() % RECORD_SIZE;
        self.read += whole as u64;
        for chunk in bytes[..whole].chunks_exact(RECORD_SIZE) {
            let Ok(record) = FrameRecord::from_bytes(chunk) else { continue };
            if !record.has(record_flags::STRIP_OK) || !record.has(record_flags::DEPTH) || record.depth_index < 0 {
                continue;
            }
            let path = self.dir.join(DEPTH_DIR).join(orbiters_core::bundle::depth_file(record.depth_index));
            let Ok(data) = std::fs::read(path) else { continue };
            let Ok(file) = depth_file::read_for_frame(&data, record.width, record.height) else { continue };
            if self.layout.as_ref().is_none_or(|l| (l.width, l.height) != (record.width, record.height)) {
                self.layout = DepthLayout::new(record.width, record.height);
            }
            let Some(layout) = &self.layout else { continue };
            if let Some(frame) = SeenFrame::new(&record.camera_to_world, &record.projection, layout, &file.codes) {
                seen.add(frame);
            }
        }
    }
}

/// Someone a 3D picture goes around.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Person {
    /// The head's middle, world metres.
    pub head: V,
    /// The height of the ground they stand on.
    pub ground: f32,
}

impl Person {
    /// From the ground to the top of the head.
    #[cfg(test)]
    fn height(&self) -> f32 {
        self.head[1] + CROWN - self.ground
    }
}

/// What the head's middle is under the top of the head, metres.
const CROWN: f32 = 0.2;
/// How wide a head is taken to be, metres, when depth does not tell how far a box is.
const HEAD_M: f32 = 0.25;
/// What a head's surface is in front of its middle, metres.
const HEAD_DEPTH: f32 = 0.08;
/// People flown around at most.
const MOST_PEOPLE: usize = 3;
/// A box narrower than this is too far to be the picture's subject.
const LEAST_FACE_PX: f32 = 40.0;
/// Detections weaker than this do not make the camera go anywhere.
const FACE_CONFIDENCE: f32 = 0.5;
/// The wearer is flown around when their head is within this of the camera, metres. A picture of something else from
/// farther away keeps its seconds: at the 2.6 m of the picture that asked for this they still fill the frame.
const WEARER_REACH_M: f32 = 6.0;
/// A face this close to the wearer's expected head is the wearer's, sideways metres.
const SAME_PERSON_M: f32 = 0.6;
/// What the avatar's hips are of its eye height above the ground (orbiters_reconstruct::ground::HIPS_PER_EYE_HEIGHT).
const HIPS_PER_EYE_HEIGHT: f32 = 0.62;
/// A person whose ground nothing tells: their head's middle is about this far above it.
const HEAD_ABOVE_GROUND: f32 = 1.55;
/// A face whose surroundings all lie within this of its depth, metres, is flat on a wall: a poster. FacesV1 scores those up
/// to 0.84, and going around one is a second lost.
const POSTER_M: f32 = 0.07;

/// What the frame the recording started on shows, in its pixels: faces as x0, y0, x1, y1, confidence.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Framing {
    pub width: u32,
    pub height: u32,
    pub p00: f32,
    pub p11: f32,
}

/// Who a 3D picture from `start` goes around: the faces the start frame shows, the largest first, placed by what depth
/// measured through their boxes (else by a box's width, a head being [`HEAD_M`]), posters left out; and the wearer, from
/// the capture kit's hips and the avatar's eye height, when they stand near and are not among them. A wearer hidden from
/// the picture (Local User off, hide_self.rs) is not looked for: pass no eye height.
pub fn people(start: Pose, framing: Framing, boxes: &[[f32; 5]], hips: Option<V>, eye_height: Option<f32>, seen: &Seen) -> Vec<Person> {
    let [right, up, forward] = axes(&start);
    let eye = [start[0], start[1], start[2]];
    let (w, h) = (framing.width as f32, framing.height as f32);
    let (fx, fy) = (framing.p00 * w / 2.0, framing.p11 * h / 2.0);
    let wearer = match (hips, eye_height) {
        (Some(hips), Some(height)) => Some(([hips[0], hips[1] + (1.0 - HIPS_PER_EYE_HEIGHT) * height, hips[2]], hips[1] - HIPS_PER_EYE_HEIGHT * height)),
        _ => None,
    };
    let mut out: Vec<Person> = Vec::new();
    if fx > 0.0 && fy > 0.0 {
        let mut faces: Vec<[f32; 5]> = boxes.iter().copied().filter(|b| b[4] >= FACE_CONFIDENCE && b[2] - b[0] >= LEAST_FACE_PX).collect();
        faces.sort_by(|a, b| (b[2] - b[0]).total_cmp(&(a[2] - a[0])));
        for b in faces {
            if out.len() == MOST_PEOPLE {
                break;
            }
            // A ray through a pixel: forward plus the sideways share, so distances along it are eye depths.
            let ray = |px: f32, py: f32| -> V {
                let (x, y) = ((px - w / 2.0) / fx, (h / 2.0 - py) / fy);
                std::array::from_fn(|k| forward[k] + right[k] * x + up[k] * y)
            };
            let (mid_x, mid_y, width) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0, b[2] - b[0]);
            let measured = seen.along(eye, ray(mid_x, mid_y));
            let by_box = HEAD_M * fx / width.max(1.0);
            let depth = measured.filter(|z| (by_box / 2.5..=by_box * 2.5).contains(z));
            if let Some(z) = depth {
                let around = [
                    (mid_x - 1.3 * width, mid_y),
                    (mid_x + 1.3 * width, mid_y),
                    (mid_x, b[1] - 0.8 * width),
                    (mid_x - 1.3 * width, b[1] - 0.8 * width),
                    (mid_x + 1.3 * width, b[1] - 0.8 * width),
                ];
                let flat_around = around.iter().filter_map(|&(x, y)| seen.along(eye, ray(x, y))).filter(|s| (s - z).abs() < POSTER_M).count();
                if flat_around >= 4 {
                    continue;
                }
            }
            let z = depth.map_or(by_box, |z| z + HEAD_DEPTH);
            let d = ray(mid_x, mid_y);
            let head: V = std::array::from_fn(|k| eye[k] + d[k] * z);
            // The wearer's face: their own ground. Anyone else's from depth, else an average head's height.
            let own = wearer.filter(|(w, _)| flat(*w, head) < SAME_PERSON_M).map(|(_, g)| g);
            let ground = own.or_else(|| seen.ground_under(head)).unwrap_or(head[1] - HEAD_ABOVE_GROUND);
            out.push(Person { head, ground });
        }
    }
    if let Some((head, ground)) = wearer {
        if out.len() < MOST_PEOPLE && length(sub(head, eye)) <= WEARER_REACH_M && out.iter().all(|p| flat(p.head, head) >= SAME_PERSON_M) {
            out.push(Person { head, ground });
        }
    }
    out
}

/// People this close together, sideways metres, are one sweep's subject: going around each would see the other from a metre
/// away, in the way.
const SAME_GROUP_M: f32 = 1.4;

/// Who one set of sweeps goes around.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Group {
    /// The middle of their heads.
    pub head: V,
    pub ground: f32,
    /// From the ground to the top of the tallest head.
    pub height: f32,
    /// How far the farthest head is from the middle, sideways.
    pub spread: f32,
}

/// The people in groups, the first person's first.
pub fn groups(people: &[Person]) -> Vec<Group> {
    let mut lists: Vec<Vec<Person>> = Vec::new();
    for p in people {
        match lists.iter_mut().find(|list| list.iter().any(|q| flat(q.head, p.head) < SAME_GROUP_M)) {
            Some(list) => list.push(*p),
            None => lists.push(vec![*p]),
        }
    }
    lists
        .iter()
        .map(|list| {
            let n = list.len() as f32;
            let head: V = std::array::from_fn(|k| list.iter().map(|p| p.head[k]).sum::<f32>() / n);
            let ground = list.iter().map(|p| p.ground).fold(f32::MAX, f32::min);
            let height = list.iter().map(|p| p.head[1] + CROWN - ground).fold(0.0, f32::max);
            let spread = list.iter().map(|p| flat(p.head, head)).fold(0.0, f32::max);
            Group { head, ground, height, spread }
        })
        .collect()
}

/// A sweep's framing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The whole body at chest height. What Enzo's turned view lacked most.
    Whole,
    /// Head and chest, closer and a little from above: more than twice the pixels of the whole body, on what people look at.
    Close,
    /// The whole body from higher up: the top of the head and the shoulders. An 8 s picture's third sweep.
    Above,
}

/// Of a frame's height, what lies between the capture strip (down to 22 % of it, contracts' Docs/capture-strip.md) and the
/// skeleton band (from 97 %): a body is framed within it.
const USABLE: f32 = 0.75;
/// The usable part's middle is this far under the frame's middle, in half-heights of the frame.
const USABLE_BELOW: f32 = 0.19;
/// The usable part's top is this far above the frame's middle, in half-heights of the frame.
const USABLE_ABOVE: f32 = 0.56;
/// The usable part's bottom is this far under the frame's middle, in half-heights of the frame.
const USABLE_BOTTOM: f32 = 0.94;
/// How far a sweep goes between two swings of the aim from head to feet and back when they do not fit together, degrees.
const SWING_DEG: f32 = 70.0;
/// How much of a body a close sweep frames, metres from the top of the head down.
const CLOSE_M: f32 = 0.95;
/// Each sweep's height over the head's middle, metres, the highest the world allows.
const CLOSE_RISE: [f32; 2] = [0.35, 0.1];
const ABOVE_RISE: [f32; 3] = [1.0, 0.7, 0.45];
/// Radii tried at each angle, of the one that frames the subject: nearer is cropped but is still a view, and from a wall's
/// foot nothing else is possible.
const RADII: [f32; 4] = [1.0, 0.85, 0.7, 0.55];
/// No nearer than this to a person's axis, metres: closer the view is all chest.
const NEAREST_M: f32 = 0.9;
/// Angles a sweep considers around a person, 15° apart.
const ANGLES: usize = 24;
/// What a person's own silhouette hides from a frame is unknown rather than free: at most this many angles in a row may
/// be, between two that were seen, so a ring passes behind someone. Walls are seen on either side of such a gap; what
/// stands exactly in it costs a frame or two.
const UNKNOWN_BRIDGE: usize = 2;
/// A radius changes by at most this per angle, metres, so the framing does not jump.
const RADIUS_SLOPE: f32 = 0.35;
/// An arc shorter than this is not worth flying, degrees: the start's views already have it.
const LEAST_ARC: f32 = 60.0;
/// How fast a sweep turns at most, degrees a second: about 20° between colour frames at VRChat's 37 frames a second.
const MOST_TURN: f32 = 720.0;
/// How fast the camera moves between sweeps and places, metres a second (the flight before this one reached 16).
const TRANSIT_SPEED: f32 = 8.0;
const LEAST_TRANSIT_S: f32 = 0.06;

/// A sweep's geometry for a group, or for a ring around a place looking outward.
struct Sweep {
    group: Group,
    /// A ring: the camera looks across the middle at the far side, and nothing need show.
    across: bool,
    radius: f32,
    eye_y: f32,
    /// What it frames: the height of the body's middle, of the top of what must show, of the bottom, and whether it is to
    /// see the whole body.
    middle: f32,
    crown: f32,
    floor: f32,
    whole: bool,
    p11: f32,
}

impl Sweep {
    fn new(group: Group, kind: Kind, rise: f32, p11: f32) -> Self {
        let fit = |metres: f32| metres * p11 / (2.0 * USABLE);
        let middle = group.ground + group.height / 2.0;
        let (radius, eye_y, aim) = match kind {
            Kind::Whole => (fit(group.height) + group.spread, group.ground + 0.62 * group.height, middle),
            Kind::Close => (fit(CLOSE_M) + group.spread, group.head[1] + rise, group.head[1] + CROWN - CLOSE_M / 2.0),
            Kind::Above => (0.9 * fit(group.height) + group.spread, group.head[1] + rise, middle),
        };
        let radius = radius.max(NEAREST_M + group.spread);
        Self { group, across: false, radius, eye_y, middle: aim, crown: group.ground + group.height, floor: group.ground, whole: kind != Kind::Close, p11 }
    }

    /// A ring around `center` at the height `eye_y`, looking across it and `tilt` degrees down.
    fn ring(center: V, eye_y: f32, tilt: f32, p11: f32) -> Self {
        let group = Group { head: center, ground: eye_y, height: 0.0, spread: 0.0 };
        Self { group, across: true, radius: RING_M, eye_y, middle: eye_y - tilt.to_radians().tan() * RING_M, crown: eye_y, floor: eye_y, whole: false, p11 }
    }

    /// The camera's place at an angle and a radius.
    fn eye(&self, angle: f32, radius: f32) -> V {
        let a = angle.to_radians();
        [self.group.head[0] + radius * a.sin(), self.eye_y, self.group.head[2] + radius * a.cos()]
    }

    /// The aim from `radius` away, `walked` degrees into the sweep: the subject's axis, its middle in the middle of the frame's usable part. Nearer
    /// than what fits it all (a room can be too small for that), the aim swings between the top under the capture strip and the
    /// feet above the skeleton band, every [`SWING_DEG`] walked, so that the sides of the legs get seen too, in passing, and an
    /// arc flown there and back shows at each place what it did not show the first time.
    fn aim(&self, radius: f32, walked: f32) -> V {
        let half = radius / self.p11;
        let fits = self.middle + USABLE_BELOW * half;
        let (high, low) = (self.crown - USABLE_ABOVE * half, self.floor + USABLE_BOTTOM * half);
        let y = if !self.whole || high <= low {
            fits.max(high)
        } else {
            let swing = 0.5 + 0.5 * (walked / SWING_DEG * 360.0).to_radians().cos();
            high + (low - high) * (1.0 - swing)
        };
        [self.group.head[0], y, self.group.head[2]]
    }

    /// What the camera looks at from `radius`, `walked` degrees in: its subject, or on a ring, the middle.
    fn aim_at(&self, radius: f32, walked: f32) -> V {
        if self.across {
            return [self.group.head[0], self.middle, self.group.head[2]];
        }
        self.aim(radius, walked)
    }

    /// The largest radius at this angle the camera can stand and see from, or what stops it. The line of sight stops short
    /// of the person: they are the surface it ends on.
    fn radius_at(&self, seen: &Seen, angle: f32) -> Result<f32, Clear> {
        let mut unknown = false;
        if self.across {
            let middle = [self.group.head[0], self.eye_y, self.group.head[2]];
            for share in RADII {
                let r = self.radius * share;
                let e = self.eye(angle, r);
                match (seen.eye_clear(e), seen.line_clear(middle, e, FLIGHT_MARGIN)) {
                    (Clear::Free, Clear::Free) => return Ok(r),
                    (Clear::Surface, _) | (_, Clear::Surface) => {}
                    _ => unknown = true,
                }
            }
            return Err(if unknown { Clear::Unknown } else { Clear::Surface });
        }
        let nearest = NEAREST_M + self.group.spread;
        for share in RADII {
            let r = self.radius * share;
            if r < nearest {
                break;
            }
            let e = self.eye(angle, r);
            let stop = lerp(e, self.aim(r, 0.0), 1.0 - (nearest * 0.8 / r).min(0.95));
            match (seen.eye_clear(e), seen.line_clear(e, stop, SIGHT_MARGIN)) {
                (Clear::Free, Clear::Free) => return Ok(r),
                (Clear::Surface, _) | (_, Clear::Surface) => {}
                _ => unknown = true,
            }
        }
        Err(if unknown { Clear::Unknown } else { Clear::Surface })
    }

    /// The angles to fly, each with its radius, as a path that starts on the angle nearest `from`: all the way round when
    /// everything is free, else over the free arc there and back. `None` when no arc of [`LEAST_ARC`] can be reached from there.
    fn route(&self, seen: &Seen, from: V, turn: f32) -> Option<Vec<(f32, f32)>> {
        let first = (from[0] - self.group.head[0]).atan2(from[2] - self.group.head[2]).to_degrees();
        let step = 360.0 / ANGLES as f32;
        let mut radii: Vec<Result<f32, Clear>> = (0..ANGLES).map(|i| self.radius_at(seen, first + step * i as f32)).collect();
        // Short unknown runs between two seen angles are bridged.
        for i in 0..ANGLES {
            if radii[i].is_ok() || radii[(i + ANGLES - 1) % ANGLES].is_err() {
                continue;
            }
            let run = (0..ANGLES).take_while(|k| radii[(i + k) % ANGLES] == Err(Clear::Unknown)).count();
            if run >= 1 && run <= UNKNOWN_BRIDGE {
                if let Ok(next) = radii[(i + run) % ANGLES] {
                    let before = radii[(i + ANGLES - 1) % ANGLES].unwrap_or(next);
                    for k in 0..run {
                        radii[(i + k) % ANGLES] = Ok(before.min(next));
                    }
                }
            }
        }
        // The framing changes slowly: a small radius pulls its neighbours in.
        let mut r: Vec<Option<f32>> = radii.iter().map(|v| v.ok()).collect();
        for _ in 0..ANGLES {
            for i in 0..ANGLES {
                let side = [r[(i + 1) % ANGLES], r[(i + ANGLES - 1) % ANGLES]];
                if let Some(mine) = r[i] {
                    let least = side.iter().flatten().fold(mine, |m, s| m.min(s + RADIUS_SLOPE));
                    r[i] = Some(least);
                }
            }
        }
        // The nearest usable angle to where the camera is, and how far free either way from it.
        let start = (0..=ANGLES / 2)
            .flat_map(|k| [k, (ANGLES - k) % ANGLES])
            .find(|&i| r[i].is_some() && seen.line_clear(from, self.eye(first + step * i as f32, r[i].unwrap_or(0.0)), FLIGHT_MARGIN) != Clear::Surface)?;
        let ahead = (1..ANGLES).take_while(|k| r[(start + k) % ANGLES].is_some()).count();
        let back = (1..ANGLES).take_while(|k| r[(start + ANGLES - k) % ANGLES].is_some()).count();
        let angle_of = |i: isize| first + step * i as f32;
        let at = |i: isize| r[i.rem_euclid(ANGLES as isize) as usize].unwrap_or(self.radius);
        let s = start as isize;
        let mut path: Vec<(f32, f32)> = Vec::new();
        let dir: isize = if turn >= 0.0 { 1 } else { -1 };
        if ahead + 1 >= ANGLES {
            for k in 0..=ANGLES as isize {
                path.push((angle_of(s + dir * k), at(s + dir * k)));
            }
        } else {
            if (ahead + back) as f32 * step < LEAST_ARC {
                return None;
            }
            let (far, near) = (ahead as isize, back as isize);
            for k in 0..=far {
                path.push((angle_of(s + k), at(s + k)));
            }
            for k in (-near..far).rev() {
                path.push((angle_of(s + k), at(s + k)));
            }
            for k in -near + 1..=0 {
                path.push((angle_of(s + k), at(s + k)));
            }
        }
        Some(path)
    }

    /// The poses of a route over `seconds`, from `from`: an eased move to its first place, then a steady sweep.
    fn poses(&self, route: &[(f32, f32)], from: Pose, seconds: f32, rate: u32) -> Vec<Pose> {
        let lens: f32 = route.windows(2).map(|w| (w[1].0 - w[0].0).abs()).sum();
        let begin = looking_at(self.eye(route[0].0, route[0].1), self.aim_at(route[0].1, 0.0), from[4], from[5]);
        let mut poses = ease(from, begin, transit_seconds(from, begin), rate);
        let last = poses.last().copied().unwrap_or(from);
        let spent = poses.len() as f32 / rate as f32;
        let sweep = (seconds - spent).max(lens / MOST_TURN);
        let n = ((sweep * rate as f32).round() as usize).max(1);
        let mut yaw = last[4];
        let mut walk = (0usize, 0.0f32);
        for i in 1..=n {
            let target = lens * i as f32 / n as f32;
            while walk.0 + 2 < route.len() && walk.1 + (route[walk.0 + 1].0 - route[walk.0].0).abs() < target {
                walk.1 += (route[walk.0 + 1].0 - route[walk.0].0).abs();
                walk.0 += 1;
            }
            let (a, b) = (route[walk.0], route[walk.0 + 1]);
            let piece = (b.0 - a.0).abs().max(1e-3);
            let t = ((target - walk.1) / piece).clamp(0.0, 1.0);
            let (angle, radius) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            let pose = looking_at(self.eye(angle, radius), self.aim_at(radius, target), yaw, from[5]);
            yaw = pose[4];
            poses.push(pose);
        }
        poses
    }
}

/// The camera at `eye` looking at `at`, `near_yaw` the yaw it turns from, so it never goes the long way round.
fn looking_at(eye: V, at: V, near_yaw: f32, roll: f32) -> Pose {
    let d = sub(at, eye);
    let across = (d[0] * d[0] + d[2] * d[2]).sqrt();
    [eye[0], eye[1], eye[2], -d[1].atan2(across).to_degrees(), near(near_yaw, d[0].atan2(d[2]).to_degrees()), roll]
}

/// The yaw equal to `target` (mod 360) nearest to `from`.
fn near(from: f32, target: f32) -> f32 {
    from + (target - from + 180.0).rem_euclid(360.0) - 180.0
}

fn place(p: &Pose) -> V {
    [p[0], p[1], p[2]]
}

fn transit_seconds(from: Pose, to: Pose) -> f32 {
    (length(sub(place(&to), place(&from))) / TRANSIT_SPEED).max(LEAST_TRANSIT_S)
}

/// An eased move between two poses (the yaw as given: several turns are turned).
fn ease(from: Pose, to: Pose, seconds: f32, rate: u32) -> Vec<Pose> {
    let n = ((seconds * rate as f32).round() as usize).max(1);
    (1..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let t = t * t * (3.0 - 2.0 * t);
            std::array::from_fn(|k| from[k] + (to[k] - from[k]) * t)
        })
        .collect()
}

/// How long the flight's opening turn takes, seconds: at 37 frames a second, a depth frame every 40° or so on average, which
/// the frame's 75° width covers with an overlap, however they fall.
pub const LOOK_S: f32 = 0.7;

/// The opening turn, level, from `start` back to `start`.
pub fn look(start: Pose, rate: u32) -> Vec<Pose> {
    let n = ((LOOK_S * rate as f32).round() as usize).max(1);
    (1..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let tilt = (std::f32::consts::PI * t).sin();
            [start[0], start[1], start[2], start[3] * (1.0 - tilt), start[4] + 360.0 * t, start[5]]
        })
        .collect()
}

/// With fewer depth frames than this after the opening turns (a world's post-processing, a capture kit without them, a stream at
/// a dozen frames a second), nothing tells where the camera can go. About 9 are usual.
pub const LEAST_DEPTH_FRAMES: usize = 3;
/// Opening turns at most while depth frames are fewer than [`LEAST_DEPTH_FRAMES`]. On 2026-11-15 the stream gave 11 to 20 frames
/// a second in some worlds, one in eight or so a depth frame: one turn of [`LOOK_S`] brought 1 to 4, and the picture flew blind.
pub const MOST_LOOKS: usize = 3;

/// Whether the camera turns once more where it stands before flying: after `looks` turns, depth has given `depth_frames`.
pub fn look_again(depth_frames: usize, looks: usize) -> bool {
    depth_frames < LEAST_DEPTH_FRAMES && looks < MOST_LOOKS
}

/// What a 3D picture flies after its opening turns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// [`Flight`], where depth says the camera can go.
    Around,
    /// [`Flight`] around the people with what depth never saw taken as free ([`Seen::blind`]).
    AroundBlind,
    /// The older blind flight (controller/burst.rs `path`): around the camera's own place and out through the room.
    Blind,
}

/// The flight for a picture of `mode` with `depth_frames` after its opening turns and `people` to go around. A picture of people
/// goes around them with or without depth: the blind flight is a picture of the room, and that is what made people and world
/// look alike (Enzo, 2026-11-16).
pub fn choose(mode: PictureMode, depth_frames: usize, people: usize) -> Choice {
    if depth_frames >= LEAST_DEPTH_FRAMES {
        Choice::Around
    } else if mode == PictureMode::People && people > 0 {
        Choice::AroundBlind
    } else {
        Choice::Blind
    }
}

/// The pitches of a picture of the world's further turns, degrees: a positive one looks down.
const TILTED_TURNS: [f32; 2] = [-40.0, 40.0];

/// A whole turn where the camera is, its pitch going to `pitch` and back: it ends on the pose it began with, a turn later.
fn turn_tilted(from: Pose, pitch: f32, rate: u32) -> Vec<Pose> {
    let n = ((LOOK_S * rate as f32).round() as usize).max(1);
    (1..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let tilt = if i == n { 0.0 } else { (std::f32::consts::PI * t).sin() };
            [from[0], from[1], from[2], from[3] + (pitch - from[3]) * tilt, from[4] + 360.0 * t, from[5]]
        })
        .collect()
}

/// How a flight spends its seconds after the opening turn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Split {
    pub people: f32,
    pub room: f32,
}

impl Split {
    /// [`LOOK_S`] plus these make the picture's length. Around people first: they hold their pose for the first seconds best.
    pub fn of(length: Photo3dLength, mode: PictureMode, groups: usize) -> Self {
        let total = match length {
            Photo3dLength::Quick => 1.0,
            Photo3dLength::Standard => 4.0,
            Photo3dLength::Full => 8.0,
        } - LOOK_S;
        let people = match (mode, length, groups) {
            (PictureMode::World, _, _) | (_, _, 0) | (_, Photo3dLength::Quick, _) => 0.0,
            (_, Photo3dLength::Standard, _) => 2.9,
            (_, Photo3dLength::Full, _) => 5.8,
        };
        Self { people, room: total - people }
    }
}

/// The sweeps a group gets in `seconds`: each at least [`LEAST_SWEEP_S`], the whole body first.
fn sweeps_of(seconds: f32) -> Vec<(Kind, f32)> {
    if seconds >= 3.0 {
        vec![(Kind::Whole, 0.4 * seconds), (Kind::Close, 0.35 * seconds), (Kind::Above, 0.25 * seconds)]
    } else if seconds >= 2.0 * LEAST_SWEEP_S {
        vec![(Kind::Whole, 0.55 * seconds), (Kind::Close, 0.45 * seconds)]
    } else {
        vec![(Kind::Whole, seconds)]
    }
}

/// A sweep in less than this leaves more than about 15° between colour frames.
const LEAST_SWEEP_S: f32 = 0.6;

/// The narrowest framing a flight is planned for, as a projection's P11 (about 42° vertically). Sweeps stand as far from a
/// person as frames them: zoomed in to 21° (P11 5.26, Enzo's pictures of ESCAPISM and Sunset Bay, 2026-11-15/16) that was
/// 5 to 6 m, a ring no room had free, so nobody was gone around. The camera is set back to its usual zoom for the flight
/// (controller/burst.rs); should it stay zoomed, a nearer sweep that frames part of a person still sees them from every side.
pub const PLAN_P11_MOST: f32 = 2.6;

/// A part of a flight: its poses, and whether the rig goes with them.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub poses: Vec<Pose>,
    pub rig: bool,
}

enum Task {
    Sweep(Group, Kind, f32),
    /// A ring around where the camera started: how far it rises, how far down it looks, seconds.
    Ring(f32, f32, f32),
    Room,
    /// A picture of the world's further turn where it is, looking up or down: a level turn says little about the ceiling and the
    /// floor.
    TurnTilted(f32),
}

/// A flight after its opening turn, planned a piece at a time: each piece is planned from what depth has shown up to then.
pub struct Flight {
    start: Pose,
    p11: f32,
    /// The seconds the people have, whatever else is cut.
    people_s: f32,
    rate: u32,
    tasks: VecDeque<Task>,
    at: Pose,
    /// Seconds flown of `total`.
    flown: f32,
    total: f32,
    /// Where the camera has been, for the room's hops.
    been: Vec<V>,
    /// A person's sweeps turn alternately each way.
    turn: f32,
    /// The room's hops tilt alternately up and down, degrees.
    tilt: f32,
    /// The rig goes with the room's hops (not with a picture of the world among other players: its cameras would show them).
    room_rig: bool,
    /// What the picture is of.
    mode: PictureMode,
    pub log: Vec<String>,
}

impl Flight {
    pub fn new(start: Pose, p11: f32, rate: u32, length: Photo3dLength, mode: PictureMode, people: &[Person], room_rig: bool) -> Self {
        // A picture of the world goes around nobody, whoever its first frame shows.
        let groups = if mode == PictureMode::World { Vec::new() } else { groups(people) };
        let split = Split::of(length, mode, groups.len());
        let mut tasks = VecDeque::new();
        for g in &groups {
            for (kind, seconds) in sweeps_of(split.people / groups.len() as f32) {
                tasks.push_back(Task::Sweep(*g, kind, seconds));
            }
        }
        if mode == PictureMode::World && length != Photo3dLength::Quick {
            let full = length == Photo3dLength::Full;
            tasks.push_back(Task::Ring(RINGS[0].0, RINGS[0].1, RING_S));
            for &pitch in if full { &TILTED_TURNS[..] } else { &TILTED_TURNS[..1] } {
                tasks.push_back(Task::TurnTilted(pitch));
            }
            for &(rise, tilt) in if full { &RINGS[1..] } else { &RINGS[..0] } {
                tasks.push_back(Task::Ring(rise, tilt, RING_S));
            }
        }
        tasks.push_back(Task::Room);
        Self {
            start,
            p11: p11.min(PLAN_P11_MOST),
            people_s: split.people,
            rate,
            tasks,
            at: start,
            flown: 0.0,
            total: split.people + split.room,
            been: vec![place(&start)],
            turn: 1.0,
            tilt: ROOM_TILT,
            room_rig,
            mode,
            log: Vec::new(),
        }
    }

    /// `seconds` more went to opening turns than [`LOOK_S`]: they come off the room's part, never off the people's, so the
    /// picture keeps about its length.
    pub fn spend(&mut self, seconds: f32) {
        self.total = (self.total - seconds.max(0.0)).max(self.people_s);
    }

    /// The next piece, planned from what `seen` knows now. `None`: the flight is over and the camera is home.
    pub fn next(&mut self, seen: &Seen) -> Option<Piece> {
        loop {
            match self.tasks.pop_front()? {
                Task::Sweep(group, kind, seconds) => {
                    let Some(poses) = self.sweep(seen, group, kind, seconds) else {
                        self.log.push(format!("no {kind:?} sweep around {:?}: depth shows no way", group.head)); // i18n: not shown
                        continue;
                    };
                    return Some(self.flew(poses, false));
                }
                Task::Ring(rise, tilt, seconds) => {
                    let ring = Sweep::ring(middle_of(seen, place(&self.start)), self.start[1] + rise, tilt, self.p11);
                    let Some(route) = ring.route(seen, place(&self.at), self.turn) else {
                        self.log.push(format!("no ring {rise} m up: depth shows no way")); // i18n: not shown
                        continue;
                    };
                    self.turn = -self.turn;
                    let poses = ring.poses(&route, self.at, seconds, self.rate);
                    let rig = self.room_rig;
                    return Some(self.flew(poses, rig));
                }
                Task::TurnTilted(pitch) => {
                    let poses = turn_tilted(self.at, pitch, self.rate);
                    let rig = self.room_rig;
                    return Some(self.flew(poses, rig));
                }
                Task::Room => {
                    let (poses, home) = self.hop(seen, self.total - self.flown);
                    if !home {
                        self.tasks.push_front(Task::Room);
                    }
                    let rig = self.room_rig;
                    return Some(self.flew(poses, rig));
                }
            }
        }
    }

    fn flew(&mut self, poses: Vec<Pose>, rig: bool) -> Piece {
        self.flown += poses.len() as f32 / self.rate as f32;
        if let Some(last) = poses.last() {
            self.at = *last;
        }
        self.been.extend(poses.iter().step_by(10).map(place));
        Piece { poses, rig }
    }

    fn sweep(&mut self, seen: &Seen, group: Group, kind: Kind, seconds: f32) -> Option<Vec<Pose>> {
        let rises: &[f32] = match kind {
            Kind::Whole => &[0.0],
            Kind::Close => &CLOSE_RISE,
            Kind::Above => &ABOVE_RISE,
        };
        for &rise in rises {
            let sweep = Sweep::new(group, kind, rise, self.p11);
            if let Some(route) = sweep.route(seen, place(&self.at), self.turn) {
                self.turn = -self.turn;
                return Some(sweep.poses(&route, self.at, seconds, self.rate));
            }
        }
        None
    }

    /// A hop to the place farthest from where the camera has been, within what depth has seen free and from where it can come
    /// straight home. With no such place in reach from here it goes through home, and with none from home, or no time for one,
    /// it ends there: when the room has shown what it can, the people need not wait for the rest of the seconds. The rig goes
    /// with these: it adds six views around the path in every other colour frame, which is what the room's far corners get.
    /// `seconds`: what is left of the flight. Also tells whether this is the way home for good.
    fn hop(&mut self, seen: &Seen, seconds: f32) -> (Vec<Pose>, bool) {
        let (from, home) = (self.at, self.start);
        let world = self.mode == PictureMode::World;
        let speed = if world { WORLD_SPEED } else { ROOM_SPEED };
        let back = |to: V| (length(sub(to, place(&home))) / TRANSIT_SPEED).max(LEAST_HOME_S);
        let hop_of = |at: V, to: V| (length(sub(to, at)) / speed).max(ROOM_LEAST_HOP_S);
        let places: Vec<(V, f32)> = room_places(home)
            .into_iter()
            .filter(|p| seen.eye_clear(*p) == Clear::Free && seen.line_clear(*p, place(&home), FLIGHT_MARGIN) == Clear::Free)
            .map(|p| (p, self.been.iter().map(|b| length(sub(p, *b))).fold(f32::MAX, f32::min)))
            .collect();
        // A picture of the world has nobody waiting for it to end, and nothing better to do with its seconds than fill in
        // what lies between the places it went to.
        let far_apart = places.iter().filter(|(p, apart)| *apart >= ROOM_LEAST_NEW_M && hop_of(place(&from), *p) + back(*p) <= seconds).count() > 0;
        let least_new = if world && !far_apart { WORLD_LEAST_NEW_M } else { ROOM_LEAST_NEW_M };
        let new_places: Vec<(V, f32)> = places.into_iter().filter(|(_, apart)| *apart >= least_new).collect();
        let farthest = |list: &mut dyn Iterator<Item = &(V, f32)>| list.copied().max_by(|a, b| a.1.total_cmp(&b.1));
        let in_reach = farthest(
            &mut new_places
                .iter()
                .filter(|(p, _)| hop_of(place(&from), *p) + back(*p) <= seconds && seen.line_clear(place(&from), *p, FLIGHT_MARGIN) == Clear::Free),
        );
        if let Some((to, _)) = in_reach {
            // The main view turns as it goes, and tilts up and down in turn: the rig's faces have the floor and ceiling at a
            // sixth of the pixels.
            self.tilt = -self.tilt;
            let hop = hop_of(place(&from), to);
            let next = [to[0], to[1], to[2], self.tilt, from[4] + ROOM_TURN_PER_S * hop, from[5]];
            return (ease(from, next, hop, self.rate), false);
        }
        let away = length(sub(place(&from), place(&home)));
        let way_home = back(place(&from));
        // Through home to where it can go from there, if the time allows.
        let through = away > 0.5 && new_places.iter().any(|(p, _)| way_home + hop_of(place(&home), *p) + back(*p) <= seconds);
        let turns = ((from[4] + ROOM_TURN_PER_S * way_home - home[4]) / 360.0).round();
        let mut at_home = home;
        at_home[4] += 360.0 * turns;
        (ease(from, at_home, way_home, self.rate), !through)
    }
}

/// A picture of the world's speed on its hops: half as many again views a metre, for the people-free seconds it has.
const WORLD_SPEED: f32 = 8.0;
/// How far a ring goes from the middle at most, metres: nearer where the place is smaller.
const RING_M: f32 = 3.5;
/// How long a ring takes, seconds: 240° a second, 6° between colour frames.
const RING_S: f32 = 1.5;
/// A picture of the world's rings: how far each rises, and how far down it looks, degrees. An 8 s picture has them all.
const RINGS: [(f32, f32); 3] = [(0.0, 5.0), (1.0, 20.0), (-0.7, -10.0)];
/// A surface farther than this from the camera says nothing of the place it stands in, metres.
const MIDDLE_WITHIN_M: f32 = 12.0;
/// The middle may be this far from where the camera stands at most, metres: a ring around the far end of a street shows the
/// camera's own surroundings no better.
const MIDDLE_MOST_M: f32 = 4.0;

/// The middle of what depth has shown around `at`: sideways, the median of the surfaces measured near it. A ring around this
/// sees across a room from all sides, where one around the camera in a corner would be half a ring.
fn middle_of(seen: &Seen, at: V) -> V {
    let (mut xs, mut zs) = (Vec::new(), Vec::new());
    for f in &seen.frames {
        for p in (0..f.bins.len()).filter_map(|bin| f.surface_point(bin)) {
            if flat(p, at) < MIDDLE_WITHIN_M {
                xs.push(p[0]);
                zs.push(p[2]);
            }
        }
    }
    if xs.is_empty() {
        return at;
    }
    xs.sort_by(f32::total_cmp);
    zs.sort_by(f32::total_cmp);
    let middle = [xs[xs.len() / 2], at[1], zs[zs.len() / 2]];
    let away = flat(middle, at);
    if away <= MIDDLE_MOST_M {
        middle
    } else {
        lerp(at, middle, MIDDLE_MOST_M / away)
    }
}

/// A picture of the world, out of far places, fills in places at least this far from where it has been, metres.
const WORLD_LEAST_NEW_M: f32 = 0.75;
/// The hops' speed, metres a second, and the shortest one, seconds. The older flight's 10 m in 0.6 s kept every frame sharp:
/// VRChat's camera draws no motion blur.
const ROOM_SPEED: f32 = 12.0;
const ROOM_LEAST_HOP_S: f32 = 0.25;
/// How fast the main view turns on a hop, degrees a second: the older flight's 3 turns in 3 s.
const ROOM_TURN_PER_S: f32 = 360.0;
/// A hop must reach somewhere at least this far from anywhere the camera has been, metres.
const ROOM_LEAST_NEW_M: f32 = 1.5;
/// The main view's tilt on the hops, degrees.
const ROOM_TILT: f32 = 18.0;
/// The way home takes at least this, seconds.
const LEAST_HOME_S: f32 = 0.15;
/// How far the camera goes from where it stands: the older flight's reach, metres.
const ROOM_REACH_M: f32 = 10.0;

/// Where a room's hops may go: rings around the start at its own height and above.
fn room_places(start: Pose) -> Vec<V> {
    let mut out = Vec::new();
    for rise in [0.0, -0.8, 0.8, 3.0, 6.0, ROOM_REACH_M] {
        for distance in [1.5, 3.0, 4.5, 6.0, 8.0, ROOM_REACH_M] {
            for i in 0..16 {
                let a = (i as f32 * 22.5).to_radians();
                out.push([start[0] + distance * a.sin(), start[1] + rise, start[2] + distance * a.cos()]);
            }
        }
    }
    out
}

/// The whole flight at once when nothing new will be learnt: for what cannot be asked piece by piece (tests).
#[cfg(test)]
pub fn plan(start: Pose, p11: f32, rate: u32, length: Photo3dLength, people: &[Person], seen: &Seen) -> Vec<Piece> {
    let mut flight = Flight::new(start, p11, rate, length, PictureMode::People, people, true);
    std::iter::from_fn(|| flight.next(seen)).collect()
}

#[cfg(test)]
mod tests;
