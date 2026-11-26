//! 3D picture: a short recording during which VRChat's camera (`/usercamera/Pose`, VRChat 2025.3.3 and later) moves
//! on its own, then comes back where it was. The capture kit stamps every frame with the camera's pose, so the session
//! reconstructs into splats like any recording; the reconstruction starts as soon as the recording is saved. Three
//! lengths ([`Photo3dLength`]): a gentle 1 s sweep around what the camera aims at; 4 s and 8 s that look around, go around
//! the people in the picture and through the room, as far as depth shows it goes (controller/around.rs). Without depth frames
//! to go by, or with the camera dolly's pre-planned paths, the older blind flight: 4 s that circle the starting point looking at
//! it then turn in place looking straight up and down and fly 10 m forward, to each side and up and back; 8 s that add four
//! circles around points 8 m away.
//!
//! N4N0's camera rig (contracts' Docs/rig-frames.md) follows the camera, from the avatar's hips: on for the flight's room
//! part and for the whole blind flight (controller/rig.rs), about every other colour frame then carries six more views, one
//! along each axis around it.
//!
//! **VRChat's zoom is only set back to its default for a picture taken zoomed in** ([`zoom_for_flight`], since 2026-11-16), once
//! before the flight and once after it. Close-ups through `/usercamera/Zoom` were tried in 0.3.4-dev.27 and removed: the
//! slider is a focal length, not a field of view (the projection's P11 is Zoom / 19.7 on the two values ever recorded: 45
//! gives 47.3°, 20 gives 89°, so "zoomed in to 20" zoomed out, which Enzo saw), each change takes VRChat 0.8 s to finish, and,
//! worst of all, VRChat takes no `/usercamera/Pose` from the first Zoom message until 0.6 to 0.9 s after the last: in the three
//! recordings of 2026-11-04 the camera stood still for the whole of the close-ups and of the faces' arcs after them, and for the
//! first 0.2 to 0.5 s of the flight. Those recordings kept 5 keyframes from that time, all from one place, at
//! lower resolution than the frame before them, for 1.8 s of everybody's patience (the desktop repository's
//! docs/3d-picture-angles-2026-11-04.md).

use orbiters_core::locks::*;
use super::{around, NoticeKind, Shared};
use crate::state::CaptureState;
use orbiters_contracts::rig;
use orbiters_core::osc::{OscValue, PARAMETERS_PREFIX};
use orbiters_core::settings::{Photo3dLength, PictureMode};
use orbiters_i18n::tr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const POSE: &str = "/usercamera/Pose";
const FOCAL_DISTANCE: &str = "/usercamera/FocalDistance";
pub(super) const SMOOTH: &str = "/usercamera/SmoothMovement";
/// How far the flight goes from where the camera stands, each way.
pub const REACH_M: f32 = 10.0;
/// Straight up or down, short of the pole where yaw means nothing.
const STRAIGHT: f32 = 88.0;
/// Poses a second.
const RATE_HZ: u32 = 90;
/// After the recording started, before the camera moves: Spout and the kit's first frames.
const SETTLE: Duration = Duration::from_millis(700);
/// Eases in and out.
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// The yaw equal to `target` (mod 360) nearest to `from`, so turning never goes the long way round.
fn near(from: f32, target: f32) -> f32 {
    from + (target - from + 180.0).rem_euclid(360.0) - 180.0
}

/// A camera path being built at `rate` poses a second: each piece starts where the last one ended. Poses are VRChat's
/// (x, y, z in metres, then pitch, yaw, roll in degrees, Unity's conventions: +X right, +Y up, +Z forward, a positive
/// pitch looks down, a yaw turns forward towards +X).
struct Path {
    rate: u32,
    poses: Vec<[f32; 6]>,
    start: [f32; 6],
    forward: [f32; 3],
    right: [f32; 3],
}

impl Path {
    fn new(start: [f32; 6], rate: u32) -> Self {
        let w = start[4].to_radians();
        Self { rate, poses: vec![start], start, forward: [w.sin(), 0.0, w.cos()], right: [w.cos(), 0.0, -w.sin()] }
    }

    fn last(&self) -> [f32; 6] {
        *self.poses.last().expect("starts with a pose")
    }

    /// A point `right`, `up` and `forward` metres from the start, as the camera faced then.
    fn local(&self, right: f32, up: f32, forward: f32) -> [f32; 3] {
        let s = self.start;
        [s[0] + self.right[0] * right + self.forward[0] * forward, s[1] + up, s[2] + self.right[2] * right + self.forward[2] * forward]
    }

    fn steps(&self, seconds: f32) -> usize {
        ((seconds * self.rate as f32).round() as usize).max(1)
    }

    /// Moves to `at`, eased, turning to `pitch` and `yaw` (yaw as given: several turns are turned).
    fn move_to(&mut self, at: [f32; 3], pitch: f32, yaw: f32, seconds: f32) {
        let from = self.last();
        let n = self.steps(seconds);
        for i in 1..=n {
            let t = smooth(i as f32 / n as f32);
            let lerp = |a: f32, b: f32| a + (b - a) * t;
            self.poses.push([lerp(from[0], at[0]), lerp(from[1], at[1]), lerp(from[2], at[2]), lerp(from[3], pitch), lerp(from[4], yaw), from[5]]);
        }
    }

    /// Once around `center`, looking at it, rising and dipping `bob` metres twice. With `grow`, the circle grows from
    /// the camera's place to `radius` and back (circling the starting point); without, the camera is on the circle
    /// already (it starts and ends at the point facing the centre with its current heading).
    fn orbit(&mut self, center: [f32; 3], radius: f32, bob: f32, seconds: f32, grow: bool) {
        let from = self.last();
        let n = self.steps(seconds);
        // Facing the centre with the current heading: the camera is on the side the heading points away from.
        let start_angle = from[4] - 180.0;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let r = if grow { radius * (1.0 - (std::f32::consts::TAU * t).cos()) / 2.0 } else { radius };
            let h = bob * (2.0 * std::f32::consts::TAU * t).sin();
            let angle = (start_angle + 360.0 * t).to_radians();
            let at = [center[0] + r * angle.sin(), center[1] + h, center[2] + r * angle.cos()];
            // Looking at the centre: down when above it (a short distance keeps it from flipping straight down).
            let pitch = (at[1] - center[1]).atan2(r.max(0.8)).to_degrees();
            self.poses.push([at[0], at[1], at[2], pitch, from[4] + 360.0 * t, from[5]]);
        }
    }
}

/// The 1 s sweep: ±15° around what the camera aims at, `focus` metres ahead, rising a little.
fn sweep(path: &mut Path, focus: f32) {
    let [x, y, z, pitch, yaw, roll] = path.start;
    let (p, w) = (pitch.to_radians(), yaw.to_radians());
    let forward = [w.sin() * p.cos(), -p.sin(), w.cos() * p.cos()];
    let subject = [x + forward[0] * focus, y + forward[1] * focus, z + forward[2] * focus];
    let back = [x - subject[0], y - subject[1], z - subject[2]];
    let n = path.steps(1.0);
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let angle = (15.0 * (std::f32::consts::TAU * t).sin()).to_radians();
        let lift = 0.12 * (std::f32::consts::PI * t).sin();
        let (s, c) = angle.sin_cos();
        let offset = [back[0] * c + back[2] * s, back[1] + lift, -back[0] * s + back[2] * c];
        let flat = (offset[0] * offset[0] + offset[2] * offset[2]).sqrt();
        let look_yaw = (-offset[0]).atan2(-offset[2]).to_degrees();
        let look_pitch = offset[1].atan2(flat).to_degrees();
        path.poses.push([subject[0] + offset[0], subject[1] + offset[1], subject[2] + offset[2], look_pitch, near(yaw, look_yaw), roll]);
    }
}

/// Three seconds: a turn in place looking straight up then down, out 10 m forward and back looking around, to the right
/// and across to the left, up looking down, and home, three full turns in all.
fn flight(path: &mut Path) {
    // (right, up, forward, turn, pitch, share of the time); the last key keeps the camera's own pitch.
    const KEYS: [(f32, f32, f32, f32, f32, f32); 11] = [
        (0.0, 0.0, 0.0, 120.0, -STRAIGHT, 0.45),
        (0.0, 0.0, 0.0, 240.0, STRAIGHT, 0.45),
        (0.0, 0.0, 0.0, 360.0, 0.0, 0.45),
        (0.0, 0.0, REACH_M, 360.0, 0.0, 0.6),
        (0.0, 0.0, REACH_M, 540.0, 15.0, 0.45),
        (0.0, 0.0, 0.0, 540.0, 0.0, 0.6),
        (REACH_M, 0.0, 0.0, 630.0, 0.0, 0.6),
        (-REACH_M, 0.0, 0.0, 810.0, 0.0, 1.0),
        (0.0, 0.0, 0.0, 900.0, 0.0, 0.6),
        (0.0, REACH_M, 0.0, 990.0, STRAIGHT, 0.7),
        (0.0, 0.0, 0.0, 1080.0, f32::NAN, 0.7),
    ];
    let shares: f32 = KEYS.iter().map(|k| k.5).sum();
    let yaw = path.last()[4];
    for (right, up, forward, turn, pitch, share) in KEYS {
        let at = path.local(right, up, forward);
        let pitch = if pitch.is_nan() { path.start[3] } else { pitch };
        path.move_to(at, pitch, yaw + turn, share / shares * 3.0);
    }
}

/// The camera's poses for a 3D picture of `length`, from `start` (`focus`: how far ahead the camera aims, for the 1 s
/// sweep). The last pose is the first one, maybe some turns later.
pub fn path(start: [f32; 6], length: Photo3dLength, focus: f32, rate: u32) -> Vec<[f32; 6]> {
    let mut path = Path::new(start, rate);
    match length {
        Photo3dLength::Quick => sweep(&mut path, focus),
        Photo3dLength::Standard | Photo3dLength::Full => {
            // Around the person taking the picture first: the camera's own place, about 2 m out.
            let me = [start[0], start[1], start[2]];
            path.orbit(me, 2.0, 0.6, 1.0, true);
            flight(&mut path);
            if length == Photo3dLength::Full {
                // Four circles around points 8 m away, in front, to the right, behind and to the left.
                for (right, forward) in [(0.0, 8.0), (8.0, 0.0), (0.0, -8.0), (-8.0, 0.0)] {
                    let center = path.local(right, 0.0, forward);
                    let heading = path.last()[4];
                    let w = heading.to_radians();
                    let entry = [center[0] - 1.5 * w.sin(), center[1], center[2] - 1.5 * w.cos()];
                    path.move_to(entry, 0.0, heading, 0.25);
                    path.orbit(center, 1.5, 0.6, 0.7, false);
                }
                let home = path.local(0.0, 0.0, 0.0);
                let heading = path.last()[4];
                path.move_to(home, start[3], heading, 0.2);
            }
        }
    }
    let mut poses = path.poses;
    // Exactly where it started (the yaw some turns later).
    if let Some(last) = poses.last_mut() {
        let turns = ((last[4] - start[4]) / 360.0).round();
        *last = [start[0], start[1], start[2], start[3], start[4] + 360.0 * turns, start[5]];
    }
    poses
}

/// What the worker read from the frame the recording started on (`orbiters-reconstruct --find-faces`).
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct StartFrame {
    pub width: u32,
    pub height: u32,
    /// The strip's projection terms: the frame's exact field of view.
    pub p00: f32,
    pub p11: f32,
    /// The faces' boxes in the frame's pixels: x0, y0, x1, y1, confidence.
    pub boxes: Vec<[f32; 5]>,
    /// The hips of the avatar wearing the capture kit, from its skeleton band.
    pub hips: Option<[f32; 3]>,
}

/// `/usercamera/Zoom` for a projection's P11, what the camera dolly's paths ask for: 45 gave P11 2.2857 and 20 gave 1.0159 in
/// the recordings of 2026-11-04, a focal length in other words.
const ZOOM_PER_P11: f32 = 19.69;

/// Reads the frame the recording started on through the worker (its own process: the app itself carries no detector),
/// `None` when that fails or takes too long: the picture then goes on without.
fn read_start_frame(session: &std::path::Path) -> Option<StartFrame> {
    let file = session.join(orbiters_contracts::manifest::KEYFRAMES_DIR).join("000000.qoi");
    let begin = Instant::now();
    while !file.is_file() {
        if begin.elapsed() > Duration::from_millis(500) {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // The file may be a moment from whole: the capture writes it on a thread of its own.
    while begin.elapsed() < Duration::from_millis(READ_START_MS) {
        let mut command = std::process::Command::new(crate::worker::sibling_exe("orbiters-reconstruct"));
        command.arg("--find-faces").arg(&file).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().ok()?;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    let output = child.wait_with_output().ok()?;
                    if let Ok(frame) = serde_json::from_slice(&output.stdout) {
                        return Some(frame);
                    }
                    break;
                }
                Ok(None) if begin.elapsed() < Duration::from_millis(READ_START_MS) => std::thread::sleep(Duration::from_millis(10)),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                Err(_) => return None,
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// How long the picture waits for [`read_start_frame`], milliseconds: the people in it are waiting too. It reads while the
/// camera makes its opening turn, so nobody waits for the first second of this.
const READ_START_MS: u64 = 1500;
/// From a frame's arrival to its record and depth file on disk: frames wait for the keyframes before them to be written.
const DEPTH_LATENCY: Duration = Duration::from_millis(120);
use around::LEAST_DEPTH_FRAMES;
/// VRChat's camera at its default zoom, 47.3° vertically, when the start frame could not be read.
const DEFAULT_P11: f32 = 2.2857;
pub const ZOOM: &str = "/usercamera/Zoom";
/// VRChat's default zoom ([`DEFAULT_P11`]).
const DEFAULT_ZOOM: f32 = 45.0;
/// A camera zoomed in further than this (as its frames' P11) flies at [`DEFAULT_ZOOM`] and is zoomed back after: 15 % past the
/// default, so a slider left near it is left alone.
const ZOOMED_P11: f32 = DEFAULT_P11 * 1.15;
/// From a `/usercamera/Zoom` message until VRChat takes poses again: 0.6 to 0.9 s measured on 2026-11-04, so the opening turn
/// waits this long instead of [`SETTLE`].
const ZOOM_FREEZE: Duration = Duration::from_millis(1000);

/// The zoom a picture flies at and the one it puts back, for a camera whose frames have `p11`: `None` when it is left as it is.
///
/// Why a picture touches the zoom again (the module's notes say why it stopped): Enzo's pictures of people in ESCAPISM and Sunset
/// Bay (2026-11-15/16) were zoomed in to 21° (P11 5.26). Each frame then saw a fifth of what it does at 47°, the opening turn
/// measured too little to fly by, and sweeps sized for that framing stood 5 to 6 m from the people: four of the five flew the
/// blind flight or went around nobody. One change before the flight costs one freeze, while the camera stands anyway, and one
/// after it, when nothing moves any more; the first frames, zoomed as he framed them, are where the 3D view opens.
pub fn zoom_for_flight(p11: f32) -> Option<(f32, f32)> {
    (p11.is_finite() && p11 > ZOOMED_P11).then(|| (DEFAULT_ZOOM, (p11 * ZOOM_PER_P11).round()))
}

/// The P11 of the first colour frame recorded in `session` (its strip's projection), waiting at most `wait` for it.
fn first_p11(session: &std::path::Path, wait: Duration) -> Option<f32> {
    use orbiters_contracts::frame_record::{record_flags, FrameRecord, RECORD_SIZE};
    use orbiters_contracts::strip::flags;
    let begin = Instant::now();
    loop {
        if let Ok(bytes) = std::fs::read(session.join(orbiters_contracts::manifest::FRAMES_FILE)) {
            let first = bytes.chunks_exact(RECORD_SIZE).filter_map(|c| FrameRecord::from_bytes(c).ok()).find(|r| {
                r.has(record_flags::STRIP_OK) && flags::kind(r.strip_flags) == flags::KIND_COLOUR && flags::face(r.strip_flags) == 0
            });
            if let Some(r) = first {
                return Some(r.projection[1]);
            }
        }
        if begin.elapsed() >= wait {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// What a picture's flight did, kept in its session for whoever looks into a picture afterwards (the installed app's error
/// output goes nowhere).
pub const FLIGHT_FILE: &str = "flight.txt";

/// N4N0's camera rig's parameters for a camera pose, with the avatar's hips at `hips`: following it, where it stands
/// from the hips (each axis over [`rig::REACH_M`]) and its turn from them (over [`rig::YAW_DEG`]), level on upright
/// hips (`rig::follow_values`). VRChat takes OSC floats from -1 to 1.
pub fn rig_parameters(pose: &[f32; 6], hips: &super::rig::Hips) -> Vec<(String, Vec<rosc::OscType>)> {
    let [x, y, z, yaw] = super::rig::values(pose, hips);
    [
        (rig::PARAMETER_FOLLOW, rosc::OscType::Bool(true)),
        (rig::PARAMETER_X, rosc::OscType::Float(x)),
        (rig::PARAMETER_Y, rosc::OscType::Float(y)),
        (rig::PARAMETER_Z, rosc::OscType::Float(z)),
        (rig::PARAMETER_YAW, rosc::OscType::Float(yaw)),
    ]
    .into_iter()
    .map(|(name, value)| (format!("{PARAMETERS_PREFIX}{name}"), vec![value]))
    .collect()
}

/// The capture rig on or off.
pub fn rig_switch(on: bool) -> (String, Vec<rosc::OscType>) {
    (format!("{PARAMETERS_PREFIX}{}", rig::PARAMETER), vec![rosc::OscType::Bool(on)])
}

/// What a 3D picture's save needs of its recording ([`super::Controller::take_picture_ended`]).
pub(super) struct PictureEnded {
    mode: PictureMode,
    length: Photo3dLength,
    left_out: crate::state::LeftOut,
}

/// What a 3D picture waits for once its recording stops.
#[derive(Default)]
pub struct BurstRuntime {
    /// A 3D picture's recording is under way (its reconstruction starts when it is saved).
    pub(super) recording: bool,
    /// What it is of.
    pub(super) mode: PictureMode,
    /// How long it flies.
    pub(super) length: Photo3dLength,
}

impl BurstRuntime {
    /// What the recording under way is a picture of, `None` for a free recording.
    pub(super) fn picture_mode(&self) -> Option<PictureMode> {
        self.recording.then_some(self.mode)
    }
}

pub(super) fn floats(values: &[OscValue]) -> Vec<f32> {
    values.iter().filter_map(|v| if let OscValue::Float(f) = v { Some(*f) } else { None }).collect()
}

impl super::Controller {
    /// Whether VRChat's camera is open (`/usercamera/Mode` not 0): `None` when VRChat does not say (not found, or before
    /// 2025.3.3, whose tree has no camera).
    pub fn camera_open(&self) -> Option<bool> {
        // VRChat's camera node alone (its tree), else what VRChat sent when it last changed.
        let mode = self.camera_values().into_iter().find(|(p, _)| p == super::CAMERA_MODE).map(|(_, v)| v);
        let mode = mode.or_else(|| self.osc_values.locked().latest.get(super::CAMERA_MODE).and_then(|(v, _)| v.first().cloned()));
        match mode {
            Some(OscValue::Int(mode)) => Some(mode != 0),
            _ => None,
        }
    }

    /// VRChat's camera's values as its OSCQuery tree has them now (`/usercamera/Mode`, `/usercamera/Zoom`…): empty when
    /// VRChat does not say (not found, or before 2025.3.3, whose tree has no camera).
    pub(super) fn camera_values(&self) -> Vec<(String, OscValue)> {
        let query = self.with_osc(|o| o.vrchat_query()).flatten();
        query.map(|addr| orbiters_core::osc::vrchat_camera_values(addr, Duration::from_millis(500))).unwrap_or_default()
    }

    /// Takes a 3D picture with VRChat's camera where it stands now.
    pub fn take_3d_picture(self: &Shared) -> anyhow::Result<()> {
        let (length, mode) = self.with_settings(|s| (s.photo_3d, s.photo_3d_mode));
        self.take_3d_picture_of(length, mode).map(|_| ())
    }

    /// [`Self::take_3d_picture`] of a given length and kind (the VRChat bridge says which; a person's settings say otherwise).
    ///
    /// What stops it is said in English, whatever the language: the two who ask are not the ones who read it in the person's language.
    /// N4N0 (a toast, and the line of the 3D picture page, which its avatar's screen draws from a Latin-only atlas) keeps the English
    /// as the key it translates where it draws, as it does for [`orbiters_n4n0::platform::CAMERA_CLOSED`]; the VRChat bridge hands
    /// it to an AI, which is told in English. Only the errors are made inside: the recording itself runs on a thread of its own.
    /// The number of the recording it started ([`super::StopOf::Recording`] stops that one only).
    pub fn take_3d_picture_of(self: &Shared, length: Photo3dLength, mode: PictureMode) -> anyhow::Result<u64> {
        orbiters_i18n::in_language(orbiters_i18n::Language::En, || self.start_3d_picture(length, mode))
    }

    fn start_3d_picture(self: &Shared, length: Photo3dLength, mode: PictureMode) -> anyhow::Result<u64> {
        anyhow::ensure!(self.snapshot.locked().vrchat.running, "{}", tr("Start VRChat and open its camera first."));
        self.ensure_capture_idle()?;
        let (pose, focus, smooth, eye_height) = {
            let osc = self.osc_values.locked();
            let pose = osc.latest.get(POSE).map(|(v, _)| floats(v)).filter(|v| v.len() == 6 && v.iter().any(|f| *f != 0.0));
            let focus = osc.latest.get(FOCAL_DISTANCE).and_then(|(v, _)| floats(v).first().copied()).unwrap_or(2.0).clamp(0.5, 8.0);
            let smooth = osc.latest.get(SMOOTH).and_then(|(v, _)| v.first().map(OscValue::truthy));
            let eye_height = osc.latest.get("/avatar/eyeheight").and_then(|(v, _)| floats(v).first().copied());
            (pose, focus, smooth, eye_height)
        };
        // VRChat's camera must be open: OSC switches its mode once it is (to Stream, for Spout), nothing opens it
        // (VRChat's `/usercamera/Mode` changes nothing while the camera is closed, measured again 2026-11-06: it only stores what
        // the tree then says, no camera shows and a recording gets no frame; there is no endpoint to open it).
        anyhow::ensure!(self.camera_open() != Some(false), orbiters_n4n0::platform::CAMERA_CLOSED);
        let Some(pose) = pose else {
            anyhow::bail!("{}", tr("Move VRChat's camera once so Orbiters knows where it is, then try again."));
        };
        let pose: [f32; 6] = pose.try_into().expect("six values");
        // N4N0's menu and link card stand in front of their owner: while the capture kit records (N4N0/Capture/Record),
        // the avatar hides them on its owner's client, out of the picture, and shows them again after it.
        // What it is, set under the start gate the moment the start is accepted, and the number of the recording that
        // start issued: its flight flies, puts back and stops that one only (audit F06, CTL2-7, verify-controller).
        // A start that fails after it was accepted takes its flag back itself, under the gate; one refused took none.
        let owner = self.start_capture_owned(
            super::CapturePurpose::Short,
            || *self.burst.locked() = BurstRuntime { recording: true, mode, length },
            || self.burst.locked().recording = false,
        )?;
        let ctl = self.clone();
        let flight = std::thread::Builder::new().name("3d-picture".into()).spawn(move || {
            let started = Instant::now();
            while ctl.snapshot.locked().capture.state != CaptureState::Recording {
                if started.elapsed() > Duration::from_secs(8) || !ctl.owns_recording(owner) {
                    ctl.give_up_picture_start(owner);
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            // The frame the recording started on is read while the camera already looks around: the people wait less.
            let session = ctl.snapshot.locked().capture.session_dir.clone();
            let picture = matches!(length, Photo3dLength::Standard | Photo3dLength::Full);
            // Nobody to go around in a picture of the world: its first frame says nothing the flight needs.
            let reading = session.clone().filter(|_| picture && mode == PictureMode::People).map(|dir| std::thread::spawn(move || read_start_frame(&dir)));
            // Remote User hides the other players from VRChat's camera; the rig's six cameras have no such mask.
            let room_rig = mode == PictureMode::People || ctl.others_here() == 0;
            let send = |address: &str, args: Vec<rosc::OscType>| {
                let _ = ctl.with_osc(|o| o.send(address, args));
            };
            let mut told: Vec<String> = Vec::new();
            // A camera zoomed in flies at the usual zoom (zoom_for_flight), and gets its own back at the end.
            let zoom = (picture && !ctl.with_settings(|s| s.dolly_streams))
                .then(|| session.as_deref().and_then(|dir| first_p11(dir, Duration::from_millis(400))))
                .flatten()
                .and_then(|p11| zoom_for_flight(p11).map(|z| (p11, z)));
            let zoomed = zoom.is_some_and(|(_, (to, _))| ctl.for_recording(owner, || send(ZOOM, vec![rosc::OscType::Float(to)])));
            if let Some((p11, (to, back))) = zoom.filter(|_| zoomed) {
                told.push(format!("zoomed in (P11 {p11:.2}): flies at zoom {to}, back to {back} after")); // i18n: not shown
                std::thread::sleep(ZOOM_FREEZE);
            } else {
                std::thread::sleep(SETTLE);
            }
            // Smoothing would trail the swing behind.
            ctl.for_recording(owner, || send(SMOOTH, vec![rosc::OscType::Bool(false)]));
            let camera = |p: &[f32; 6]| (POSE.to_string(), p.iter().map(|v| rosc::OscType::Float(*v)).collect::<Vec<_>>());
            // Sends poses at the flight's rate; `true` when they were all sent (the recording may stop under them).
            let fly = |poses: &[[f32; 6]], rig: bool| -> bool {
                let tick = Duration::from_secs_f32(1.0 / RATE_HZ as f32);
                let begin = Instant::now();
                for (i, p) in poses.iter().enumerate() {
                    {
                        // Checked and sent under the start gate: no other recording can begin in between.
                        let _gate = ctl.capture_start_gate.locked();
                        if !ctl.owns_recording(owner) {
                            return false;
                        }
                        if rig {
                            // The camera and the rig in one bundle: VRChat applies them together.
                            ctl.place_rig(Some(camera(p)), p);
                        } else {
                            send(POSE, p.iter().map(|v| rosc::OscType::Float(*v)).collect());
                        }
                    }
                    if let Some(wait) = (tick * (i as u32 + 1)).checked_sub(begin.elapsed()) {
                        std::thread::sleep(wait);
                    }
                }
                true
            };
            let dolly_wanted = ctl.with_settings(|s| s.dolly_streams);
            let mut around = false;
            let frame;
            if picture && !dolly_wanted {
                // Where it can go: one turn where it stands, for the depth frames.
                let mut flying = fly(&around::look(pose, RATE_HZ), false);
                frame = reading.and_then(|r| r.join().ok()).flatten();
                let mut seen = around::Seen::default();
                // A wearer the frames show is in nobody's way: in a picture of the world they leave it afterwards, in one of people they
                // are what it goes around. Held up by their own body, a camera held at their chest (ESCAPISM and Trash Compactor,
                // 2026-11-15/16) found no way out to the ring around them and went around nobody.
                if !ctl.hiding_self() {
                    seen.left_out = ctl.hips_now().zip(eye_height).map(|(hips, eye_height)| around::LeftOut { hips, eye_height });
                }
                if let Some(dir) = &session {
                    let mut live = around::LiveDepth::new(dir);
                    // Up to what the capture's threads still hold of the turn.
                    std::thread::sleep(DEPTH_LATENCY);
                    live.poll(&mut seen);
                    // No depth with Remote User off is the one thing no recording shows the answer to: it may be what took
                    // the kit away. The blind flight goes through walls, so it comes back on, and the camera looks again.
                    let mut looks = 1;
                    // Its own recording's mask only: after its flight broke, the hiding state may be the next picture's.
                    let mut cost = false;
                    if seen.frames.len() < LEAST_DEPTH_FRAMES {
                        ctl.for_recording(owner, || cost = ctl.hiding_others() && ctl.others_cost_the_strip());
                    }
                    if cost {
                        flying = flying && fly(&around::look(pose, RATE_HZ), false);
                        looks += 1;
                        std::thread::sleep(DEPTH_LATENCY);
                        live.poll(&mut seen);
                    }
                    // A slow stream gives a turn few depth frames: the camera turns again, a little longer to wait for a flight that
                    // knows where it goes.
                    while flying && around::look_again(seen.frames.len(), looks) {
                        flying = fly(&around::look(pose, RATE_HZ), false);
                        looks += 1;
                        std::thread::sleep(DEPTH_LATENCY);
                        live.poll(&mut seen);
                    }
                    let hips = frame.as_ref().and_then(|f| f.hips).or_else(|| ctl.hips_now());
                    let framing = frame.as_ref().map_or_else(Default::default, |f| around::Framing { width: f.width, height: f.height, p00: f.p00, p11: f.p11 });
                    let boxes = frame.as_ref().map_or(&[][..], |f| &f.boxes[..]);
                    // A wearer hidden from the picture (Local User off, hide_self.rs) has nobody to go and see.
                    let people = around::people(pose, framing, boxes, hips, eye_height.filter(|_| !ctl.hiding_self()), &seen);
                    let choice = around::choose(mode, seen.frames.len(), people.len());
                    told.push(format!("{mode:?} {length:?}: {looks} opening turns, {} depth frames, {} people ({} faces in the first frame), {choice:?}", seen.frames.len(), people.len(), boxes.len())); // i18n: not shown
                    if choice != around::Choice::Blind {
                        around = true;
                        seen.blind = choice == around::Choice::AroundBlind;
                        // The frames' own zoom, unless the flight set it back to the usual one.
                        let p11 = if zoom.is_some() { DEFAULT_P11 } else { frame.as_ref().map_or(DEFAULT_P11, |f| f.p11).max(0.1) };
                        let mut flight = around::Flight::new(pose, p11, RATE_HZ, length, mode, &people, room_rig);
                        flight.spend((looks - 1) as f32 * around::LOOK_S);
                        while let Some(piece) = flying.then(|| flight.next(&seen)).flatten() {
                            if piece.rig {
                                // The rig where the camera is, then on: every other colour frame gets its six views.
                                let placed = ctl.for_recording(owner, || {
                                    if let Some(first) = piece.poses.first() {
                                        ctl.place_rig(None, first);
                                    }
                                    ctl.hold_rig(true);
                                });
                                if !placed {
                                    break;
                                }
                            }
                            flying = fly(&piece.poses, piece.rig);
                            live.poll(&mut seen);
                        }
                        told.extend(flight.log.iter().cloned());
                    }
                }
            } else {
                frame = reading.and_then(|r| r.join().ok()).flatten();
            }
            // Stopped during the opening turn (too few depth frames: "blind"), the blind flight does not fly.
            // The rig on for the blind flight; put back off when VRChat is, whatever ends the recording.
            if !around && ctl.for_recording(owner, || ctl.hold_rig(room_rig)) {
                let flight = path(pose, length, focus, RATE_HZ);
                // Experimental (VRC+): the camera dolly flies the camera and three more cameras around it (controller/dolly.rs).
                let dolly = dolly_wanted && ctl.start_dolly(owner, &flight, frame.as_ref().map_or(DEFAULT_P11, |f| f.p11) * ZOOM_PER_P11, focus);
                let begin = Instant::now();
                let tick = Duration::from_secs_f32(1.0 / RATE_HZ as f32);
                for (i, p) in flight.into_iter().enumerate() {
                    {
                        let _gate = ctl.capture_start_gate.locked();
                        if !ctl.owns_recording(owner) {
                            break;
                        }
                        // The dolly moves the camera itself: the rig only follows.
                        let camera = (!dolly).then(|| camera(&p));
                        ctl.place_rig(camera, &p);
                    }
                    if let Some(wait) = (tick * (i as u32 + 1)).checked_sub(begin.elapsed()) {
                        std::thread::sleep(wait);
                    }
                }
                if dolly {
                    // Its own dolly only: one a recording begun since plays is not this picture's to stop.
                    let gate = ctl.capture_start_gate.locked();
                    let ours = ctl.no_recording_since(owner);
                    // Its Play(false) and what it reads of the dolly's streams under the gate too: a picture begun
                    // between the check and the send would have its dolly stopped and its counts told (verify2).
                    if ours {
                        ctl.stop_dolly();
                    }
                    drop(gate);
                }
            }
            // What the flight changed of the camera goes back, once, unless another recording began since: that one's camera
            // is not this picture's to move. Under the start gate, so none can begin in between.
            let gate = ctl.capture_start_gate.locked();
            let restore = ctl.no_recording_since(owner);
            if restore {
                send(POSE, pose.iter().map(|v| rosc::OscType::Float(*v)).collect());
                if let Some((_, (_, back))) = zoom {
                    send(ZOOM, vec![rosc::OscType::Float(back)]);
                }
            }
            drop(gate);
            if !around {
                told.push("the older flight: blind, or the 1 s sweep".into()); // i18n: not shown
            }
            for line in &told {
                eprintln!("3d picture: {line}");
            }
            if let Some(dir) = &session {
                let _ = std::fs::write(dir.join(FLIGHT_FILE), told.join("\n") + "\n");
            }
            if restore {
                let _gate = ctl.capture_start_gate.locked();
                if ctl.no_recording_since(owner) {
                    ctl.hold_rig(false);
                    if let Some(smooth) = smooth.filter(|s| *s) {
                        send(SMOOTH, vec![rosc::OscType::Bool(smooth)]);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(250));
            // Its own recording only: a start asked since, or a recording begun since, is left alone.
            ctl.stop_recording_of(owner);
        });
        if let Err(e) = flight {
            // Accepted, but nothing would fly it or end it: it ends here, as an ordinary recording.
            self.give_up_start_of(owner, || self.burst.locked().recording = false);
            return Err(e.into());
        }
        Ok(owner)
    }

    /// A 3D picture whose recording did not reach Recording in time (or ended first): its flag goes, and a start still
    /// under way is stopped, or it would become a recording nobody flies or stops (CTL2-7). Neither when another recording
    /// began since: that one's flag and recording are not this picture's.
    pub(super) fn give_up_picture_start(self: &Shared, owner: u64) {
        self.give_up_start_of(owner, || self.burst.locked().recording = false);
    }

    /// Hands the flight to VRChat's camera dolly as four paths and plays them (`/dolly/Import`, `/dolly/Play`): false
    /// when that could not be done (the flight then moves the camera alone).
    fn start_dolly(&self, owner: u64, flight: &[[f32; 6]], zoom: f32, focus: f32) -> bool {
        let seconds = flight.len() as f32 / RATE_HZ as f32;
        let file = match super::dolly::write(&super::dolly::paths(flight, seconds, zoom, focus)) {
            Ok(file) => file,
            Err(e) => {
                self.notify(NoticeKind::Warning, tr!("The camera dolly's paths could not be written ({error}): the 3D picture uses the camera alone.", error = e));
                return false;
            }
        };
        let mut imported = false;
        self.for_recording(owner, || {
            imported = self.with_osc(|o| o.send(super::dolly::IMPORT, vec![rosc::OscType::String(file.display().to_string())])).is_some_and(|r| r.is_ok());
        });
        if !imported {
            return false;
        }
        // VRChat reads the file before it plays (not under the gate: the wait is not the picture's to make others wait).
        std::thread::sleep(Duration::from_millis(300));
        let mut playing = false;
        self.for_recording(owner, || playing = self.with_osc(|o| o.send(super::dolly::PLAY, vec![rosc::OscType::Bool(true)])).is_some_and(|r| r.is_ok()));
        playing
    }

    /// Stops the dolly's animation and tells what its extra streams gave.
    fn stop_dolly(&self) {
        let _ = self.with_osc(|o| o.send(super::dolly::PLAY, vec![rosc::OscType::Bool(false)]));
        let stats = self.snapshot.locked().capture.stats.clone().unwrap_or_default();
        let message = if stats.extra_frames == 0 {
            tr("The camera dolly's other cameras sent nothing: in VRChat set Camera › Dolly › Streaming to Multi (VRC+), then try again.").to_string()
        } else if stats.extra_keyframes == 0 {
            tr!(
                "The camera dolly's other cameras sent {frames} frames, none with your capture kit's strip: VRChat does not draw it in them, so they add nothing.",
                frames = stats.extra_frames
            )
        } else {
            tr!("The camera dolly's other cameras sent {frames} frames: {views} more views for the reconstruction.", frames = stats.extra_frames, views = stats.extra_keyframes)
        };
        self.notify(NoticeKind::Info, message);
    }

    /// What a 3D picture's save needs, taken from the ending recording before its Idle is published (after it, they may
    /// be the next picture's): `None` when it was no picture.
    pub(super) fn take_picture_ended(&self) -> Option<PictureEnded> {
        // The snapshot first, then the picture's state: the order `update`'s closures take them in.
        let left_out = self.snapshot.locked().capture.left_out.clone();
        let mut burst = self.burst.locked();
        std::mem::take(&mut burst.recording).then(|| PictureEnded { mode: burst.mode, length: burst.length, left_out })
    }

    /// A recording was saved: a 3D picture's (`ended`, taken before Idle) goes on to its reconstruction.
    pub(super) fn burst_saved(self: &Shared, session: Option<PathBuf>, ended: Option<PictureEnded>) {
        let Some((dir, PictureEnded { mode, length, left_out })) = session.zip(ended) else { return };
        let picture = orbiters_core::picture::Picture { mode, length, wearer: left_out.wearer, others: left_out.others };
        // Right after it, under the camera: "Share to …" and "Share publicly" for this picture (REQUEST13).
        self.explorer_offer(&dir);
        if let Err(e) = picture.save(&dir) {
            self.notify(NoticeKind::Warning, tr!("Could not note what this 3D picture is of: {error}", error = e));
        }
        match self.queue_reconstruction(dir) {
            Ok(false) => self.notify(NoticeKind::Success, tr("3D picture taken: it is being reconstructed.")),
            Ok(true) => self.notify(NoticeKind::Success, tr!("3D picture taken: its reconstruction starts {when}.", when = tr(self.reconstruction_waits_for()))),
            Err(e) => self.notify(NoticeKind::Warning, tr!("3D picture taken, not reconstructed: {error}", error = e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_i18n::{in_language, Language};

    fn stops(ctl: &Shared) -> usize {
        ctl.test_commands.lock().unwrap().as_ref().unwrap().iter().filter(|c| matches!(c, orbiters_core::protocol::WorkerCommand::Stop)).count()
    }

    /// verify-controller: a picture's or a scan's start while another recording runs is refused: its flag is not set and
    /// it takes no number, so it can never adopt that recording, fly it or stop it.
    #[test]
    fn an_owned_start_never_adopts_a_recording_under_way() {
        use std::sync::atomic::Ordering;
        let ctl = recording_controller();
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        let number = ctl.recording_number();
        let mut claimed = false;
        let refused = ctl.start_capture_owned(super::super::CapturePurpose::Short, || claimed = true, || {});
        assert!(refused.is_err(), "it would have adopted the recording under way");
        assert!(!claimed, "no picture flag set for the other recording");
        assert_eq!(ctl.capture_serial.load(Ordering::Acquire), number, "no number taken");
        ctl.update(|s| s.capture.state = CaptureState::Stopping);
        assert!(ctl.start_capture_owned(super::super::CapturePurpose::Short, || claimed = true, || {}).is_err(), "nor while it is saved");
        assert!(!claimed);
    }

    /// verify2-controller: a picture request refused while another picture is being saved (it passed the first Idle
    /// check before that one started) takes back nothing: the saved picture keeps its flag, so it is still reconstructed.
    #[test]
    fn a_refused_picture_request_leaves_the_saved_pictures_flag() {
        let ctl = recording_controller();
        *ctl.burst.lock().unwrap() = BurstRuntime { recording: true, mode: PictureMode::People, length: Photo3dLength::Standard };
        ctl.update(|s| s.capture.state = CaptureState::Stopping);
        let refused = ctl.start_capture_owned(
            super::super::CapturePurpose::Short,
            || *ctl.burst.lock().unwrap() = BurstRuntime { recording: true, mode: PictureMode::World, length: Photo3dLength::Full },
            || ctl.burst.lock().unwrap().recording = false,
        );
        assert!(refused.is_err());
        let saved = ctl.burst.lock().unwrap();
        assert!(saved.recording && saved.mode == PictureMode::People, "the picture being saved keeps its flag");
    }

    static NEXT_PICTURE: std::sync::Mutex<Option<std::sync::Weak<super::super::Controller>>> = std::sync::Mutex::new(None);

    /// A next picture that starts the very moment Idle is published (its flag set by whoever sees Idle first).
    fn next_picture_at_idle(s: &crate::state::Snapshot) -> bool {
        if s.capture.state == CaptureState::Idle {
            if let Some(ctl) = NEXT_PICTURE.lock().unwrap().take().and_then(|w| w.upgrade()) {
                *ctl.burst.lock().unwrap() = BurstRuntime { recording: true, mode: PictureMode::People, length: Photo3dLength::Standard };
            }
        }
        s.capture.state == CaptureState::Idle
    }

    /// verify-controller: the save of picture A takes what it is of before Idle is published; a picture B started at
    /// Idle keeps its own flag, and A's folder gets A's metadata, not B's.
    #[test]
    fn a_saved_picture_takes_its_own_metadata_before_the_next_one_starts() {
        let ctl = recording_controller();
        let dir = std::env::temp_dir().join(format!("orbiters-picture-a-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        *ctl.burst.lock().unwrap() = BurstRuntime { recording: true, mode: PictureMode::World, length: Photo3dLength::Full };
        ctl.update(|s| {
            s.capture.state = CaptureState::Recording;
            s.capture.session_dir = Some(dir.clone());
        });
        *NEXT_PICTURE.lock().unwrap() = Some(std::sync::Arc::downgrade(&ctl));
        ctl.subscribe(crate::mailbox::Mailbox::watching(next_picture_at_idle));
        ctl.on_worker_event(orbiters_core::protocol::WorkerEvent::Stopped { session_dir: dir.clone(), manifest: Box::default(), reason: "stopped".into() });
        assert_eq!(orbiters_core::picture::Picture::read(&dir).map(|p| (p.mode, p.length)), Some((PictureMode::World, Photo3dLength::Full)), "A's own metadata");
        let next = ctl.burst.lock().unwrap();
        assert!(next.recording && next.mode == PictureMode::People, "B's flag is B's");
        drop(next);
        std::fs::remove_dir_all(dir).ok();
    }

    fn recording_controller() -> Shared {
        let ctl = super::super::Controller::new(orbiters_core::settings::Settings::default());
        *ctl.test_commands.lock().unwrap() = Some(Vec::new());
        ctl
    }

    /// CTL2-7 (audit F06): a picture stopped during its opening turn, then a recording started: the old flight owns
    /// nothing of it (no pose, no camera put back over it) and its final Stop leaves it running.
    #[test]
    fn a_stopped_picture_neither_flies_nor_stops_the_next_recording() {
        use std::sync::atomic::Ordering;
        let ctl = recording_controller();
        ctl.capture_serial.store(7, Ordering::Release);
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        let picture = ctl.recording_number();
        assert!(ctl.owns_recording(picture));
        // Stop pressed during the opening turn, then another recording.
        ctl.update(|s| s.capture.state = CaptureState::Idle);
        assert!(!ctl.owns_recording(picture), "no pose after the stop");
        assert!(ctl.no_recording_since(picture), "its camera is still its own to put back");
        ctl.capture_serial.fetch_add(1, Ordering::AcqRel);
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        assert!(!ctl.owns_recording(picture), "no pose over the new recording");
        assert!(!ctl.no_recording_since(picture), "nor its camera put back over the new one");
        let generation = ctl.stop_generation();
        ctl.stop_recording_of(picture);
        assert_eq!(stops(&ctl), 0, "the old picture's last Stop");
        assert_eq!(ctl.stop_generation(), generation, "calls off no start either");
        assert_eq!(ctl.view().capture.state, CaptureState::Recording, "the new recording still runs");
    }

    /// CTL2-7 (3): a start still under way when the picture's wait gives up is stopped (no recording nobody flies or
    /// stops) and its flag goes; a recording begun since keeps both.
    #[test]
    fn a_picture_start_given_up_is_stopped_unless_another_recording_began() {
        use std::sync::atomic::Ordering;
        let ctl = recording_controller();
        ctl.update(|s| s.capture.state = CaptureState::Starting);
        ctl.burst.lock().unwrap().recording = true;
        let picture = ctl.recording_number();
        ctl.give_up_picture_start(picture);
        assert_eq!(stops(&ctl), 1, "its slow start is stopped");
        assert!(!ctl.burst.lock().unwrap().recording);
        // Another picture began before the old wait gave up.
        let ctl = recording_controller();
        let old = ctl.recording_number();
        ctl.capture_serial.fetch_add(1, Ordering::AcqRel);
        ctl.update(|s| s.capture.state = CaptureState::Starting);
        ctl.burst.lock().unwrap().recording = true;
        ctl.give_up_picture_start(old);
        assert_eq!(stops(&ctl), 0);
        assert!(ctl.burst.lock().unwrap().recording, "the new picture's flag stays");
    }

    /// A camera zoomed in flies at VRChat's default zoom and gets its own back; one at (or near) the default, or wider, is left alone.
    #[test]
    fn a_zoomed_in_camera_flies_at_the_usual_zoom_and_gets_its_own_back() {
        assert_eq!(zoom_for_flight(5.26), Some((45.0, 104.0)), "ESCAPISM's and Sunset Bay's 21°");
        assert_eq!(zoom_for_flight(DEFAULT_P11), None);
        assert_eq!(zoom_for_flight(2.5), None, "near the default");
        assert_eq!(zoom_for_flight(1.0159), None, "zoomed out to 89°");
        assert_eq!(zoom_for_flight(f32::NAN), None);
    }

    /// The first colour frame's zoom is read from the recording's own records, rig faces and depth frames aside.
    #[test]
    fn the_zoom_is_read_from_the_first_colour_frame() {
        use orbiters_contracts::frame_record::{record_flags, FrameRecord};
        use orbiters_contracts::strip::flags;
        let dir = std::env::temp_dir().join(format!("orbiters-first-p11-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(first_p11(&dir, Duration::ZERO), None, "nothing recorded yet");
        let record = |kind: u32, face: u32, p11: f32| {
            FrameRecord {
                flags: record_flags::STRIP_OK,
                frame_index: 0,
                capture_time_ns: 0,
                game_time: 0.0,
                strip_flags: kind | face << flags::FACE_SHIFT,
                camera_to_world: [0.0; 12],
                projection: [1.0, p11, 0.0, 0.0],
                near: 0.05,
                far: 1000.0,
                width: 1920,
                height: 1080,
                keyframe_index: -1,
                depth_index: -1,
                video_frame: -1,
                corrected_bits: 0,
                patch: [[0.0; 3]; 6],
                min_contrast: 0.0,
            }
            .to_bytes()
        };
        let mut bytes = Vec::new();
        bytes.extend(record(flags::KIND_DEPTH, 0, 9.0));
        bytes.extend(record(flags::KIND_RIG, 3, 1.0));
        bytes.extend(record(flags::KIND_COLOUR, 0, 5.26));
        bytes.extend(record(flags::KIND_COLOUR, 0, 2.2857));
        std::fs::write(dir.join(orbiters_contracts::manifest::FRAMES_FILE), bytes).unwrap();
        assert_eq!(first_p11(&dir, Duration::ZERO), Some(5.26));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// What stops a 3D picture is told in English in any language: N4N0 keys its translation on that text where it draws it (its screen has no
    /// Japanese), and the bridge hands it to an AI. A scan has no such reader: a toast says it in the person's language.
    #[test]
    fn a_picture_that_cannot_start_says_why_in_english_and_a_scan_in_the_language() {
        let ctl = crate::controller::Controller::new(orbiters_core::settings::Settings::default());
        for language in [Language::Ja, Language::Fr, Language::Es, Language::PtBr] {
            let picture = in_language(language, || ctl.take_3d_picture().unwrap_err().to_string());
            assert_eq!(picture, "Start VRChat and open its camera first.", "{language:?}");
            let scan = in_language(language, || ctl.take_scan().unwrap_err().to_string());
            assert_ne!(scan, "Start VRChat and open its camera first.", "{language:?}");
        }
    }

    fn ends_where_it_started(poses: &[[f32; 6]], pose: [f32; 6]) {
        assert_eq!(poses.first(), Some(&pose));
        let end = poses.last().unwrap();
        assert!(end.iter().zip(pose).enumerate().all(|(i, (a, b))| if i == 4 { ((a - b) % 360.0).abs() < 1e-3 } else { (a - b).abs() < 1e-4 }), "{end:?}");
    }

    /// No jump between two poses (90 a second): under a metre and 20°.
    fn smooth_all_along(poses: &[[f32; 6]]) {
        for w in poses.windows(2) {
            let d = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2) + (w[1][2] - w[0][2]).powi(2)).sqrt();
            assert!(d < 1.0 && (w[1][4] - w[0][4]).abs() < 20.0 && (w[1][3] - w[0][3]).abs() < 20.0, "{w:?}");
        }
    }

    fn headings(poses: &[[f32; 6]]) -> usize {
        let mut h: Vec<i32> = poses.iter().map(|p| (p[4].rem_euclid(360.0) / 30.0) as i32).collect();
        h.sort();
        h.dedup();
        h.len()
    }

    // At (1, 1.5, 0), facing +X (yaw 90), slightly down.
    const POSE: [f32; 6] = [1.0, 1.5, 0.0, 10.0, 90.0, 0.0];

    #[test]
    fn the_rig_follows_the_camera_from_the_hips_within_vrchats_range() {
        // Hips at the world's origin, upright, facing +Z: the values are the camera's world place.
        let origin: super::super::rig::Hips = ([0.0; 3], [0.0, 0.0, 0.0, 1.0]);
        let values = |pose: [f32; 6], hips: &super::super::rig::Hips| -> Vec<f32> {
            rig_parameters(&pose, hips).into_iter().skip(1).map(|(_, v)| if let rosc::OscType::Float(f) = v[0] { f } else { panic!() }).collect()
        };
        assert_eq!(
            rig_parameters(&POSE, &origin).iter().map(|(a, _)| a.as_str()).collect::<Vec<_>>(),
            [
                "/avatar/parameters/N4N0/Capture/Follow",
                "/avatar/parameters/N4N0/Capture/RigX",
                "/avatar/parameters/N4N0/Capture/RigY",
                "/avatar/parameters/N4N0/Capture/RigZ",
                "/avatar/parameters/N4N0/Capture/RigYaw"
            ]
        );
        assert_eq!(rig_parameters(&POSE, &origin)[0].1, vec![rosc::OscType::Bool(true)], "following the camera");
        let v = values([2048.0, 1.5, -409.6, 10.0, 270.0, 0.0], &origin);
        assert_eq!(v[0], 0.5);
        assert!((v[1] * rig::REACH_M - 1.5).abs() < 1e-4 && (v[2] + 0.1).abs() < 1e-6);
        // 270° is -90°: half a turn left of straight ahead.
        assert!((v[3] + 0.5).abs() < 1e-6);
        // Beyond the reach: at its edge.
        assert_eq!(values([9000.0, 0.0, -9000.0, 0.0, -540.0, 0.0], &origin)[..3], [1.0, 0.0, -1.0]);
        assert!((values([0.0; 6], &origin)[3]).abs() < 1e-6);
        // Hips 15 m away, turned 90° (facing +X): the camera 2 m in front of them is 2 m along their +Z, heading 0.
        let (s, c) = (45f32.to_radians().sin(), 45f32.to_radians().cos());
        let hips = ([15.0, 1.0, 6.0], [0.0, s, 0.0, c]);
        let v = values([17.0, 2.5, 6.0, 0.0, 90.0, 0.0], &hips);
        let m = |k: usize| v[k] * rig::REACH_M;
        assert!(m(0).abs() < 1e-3 && (m(1) - 1.5).abs() < 1e-3 && (m(2) - 2.0).abs() < 1e-3, "{v:?}");
        assert!(v[3].abs() < 1e-5, "the hips' heading");
    }

    /// What the reconstruction and the gallery read of a picture: its mode, and who it left out.
    #[test]
    fn a_saved_picture_says_what_it_is_of() {
        use orbiters_core::picture::{Left, Picture};
        let dir = std::env::temp_dir().join(format!("orbiters-burst-picture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ctl = super::super::Controller::new(orbiters_core::settings::Settings::default());
        *ctl.burst.lock().unwrap() = BurstRuntime { recording: true, mode: PictureMode::World, length: Photo3dLength::Full };
        assert_eq!(ctl.burst.lock().unwrap().picture_mode(), Some(PictureMode::World));
        ctl.update(|s| {
            s.capture.left_out.wearer = Left::InPicture;
            s.capture.left_out.others = Left::Out;
        });
        ctl.burst_saved(Some(dir.clone()), ctl.take_picture_ended());
        assert_eq!(Picture::read(&dir), Some(Picture { mode: PictureMode::World, length: Photo3dLength::Full, wearer: Left::InPicture, others: Left::Out }));
        assert!(!Picture::trains_faces(&dir));
        // A free recording writes nothing, and no longer counts as a picture.
        assert_eq!(ctl.burst.lock().unwrap().picture_mode(), None);
        std::fs::remove_file(dir.join(orbiters_core::picture::FILE)).unwrap();
        ctl.burst_saved(Some(dir.clone()), ctl.take_picture_ended());
        assert!(Picture::read(&dir).is_none() && Picture::trains_faces(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn one_second_sweeps_gently() {
        let poses = path(POSE, Photo3dLength::Quick, 2.0, 90);
        ends_where_it_started(&poses, POSE);
        smooth_all_along(&poses);
        assert!((poses.len() as i32 - 91).abs() <= 1);
        let side = poses.iter().map(|p| (p[2] - POSE[2]).abs()).fold(0.0, f32::max);
        assert!(side > 0.4 && side < 0.7, "about half a metre each way: {side}");
    }

    #[test]
    fn four_seconds_circle_me_then_look_every_way_and_fly_ten_metres_out() {
        let poses = path(POSE, Photo3dLength::Standard, 2.0, 90);
        ends_where_it_started(&poses, POSE);
        smooth_all_along(&poses);
        assert!((poses.len() as f32 - 4.0 * 90.0).abs() <= 12.0, "{} poses", poses.len());
        // The first second circles the camera's place about 2 m out, looking at it.
        let first: Vec<_> = poses[..=90].to_vec();
        let far = first.iter().map(|p| ((p[0] - POSE[0]).powi(2) + (p[2] - POSE[2]).powi(2)).sqrt()).fold(0.0, f32::max);
        assert!((far - 2.0).abs() < 0.05, "{far}");
        assert_eq!(headings(&first), 12, "looks at it from every side");
        // Straight up and down, every heading, 10 m forward (+X), to each side (±Z) and up.
        let pitches = poses.iter().map(|p| p[3]);
        assert!(pitches.clone().fold(f32::MAX, f32::min) <= -85.0 && pitches.fold(f32::MIN, f32::max) >= 85.0);
        assert_eq!(headings(&poses), 12);
        let reach = |f: &dyn Fn(&[f32; 6]) -> f32| poses.iter().map(f).fold(f32::MIN, f32::max);
        assert!(reach(&|p| p[0] - POSE[0]) > 9.9, "forward");
        assert!(reach(&|p| POSE[2] - p[2]) > 9.9, "right");
        assert!(reach(&|p| p[2] - POSE[2]) > 9.9, "left");
        assert!(reach(&|p| p[1] - POSE[1]) > 9.9, "up");
    }

    #[test]
    fn eight_seconds_add_four_distant_circles() {
        let poses = path(POSE, Photo3dLength::Full, 2.0, 90);
        ends_where_it_started(&poses, POSE);
        smooth_all_along(&poses);
        assert!((poses.len() as f32 - 8.0 * 90.0).abs() <= 20.0, "{} poses", poses.len());
        // Behind the start (−X) too, 8 m out plus the circle.
        let back = poses.iter().map(|p| POSE[0] - p[0]).fold(f32::MIN, f32::max);
        assert!(back > 9.0, "{back}");
    }
}
