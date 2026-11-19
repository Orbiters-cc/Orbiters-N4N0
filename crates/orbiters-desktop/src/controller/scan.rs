//! Scan: a recording during which VRChat's camera (`/usercamera/Pose`, as for a 3D picture) flies a path planned for
//! reconstruction, then comes back where it was. First a slow turn looking a little down, so the depth frames show what
//! is near; then, kept clear of what the coverage map (orbiters-capture's coverage) says is in the way, three rings
//! around the starting point: at eye height looking out, high looking in and down at the middle, low looking out and
//! a little up. Heights and gazes that one walk at eye height never gives constrain floors and surfaces from more
//! sides, so they hold together from more viewpoints. The reconstruction starts once the recording is saved.

use super::{NoticeKind, Shared};
use crate::state::CaptureState;
use orbiters_core::protocol::{coverage, CoverageMap};
use orbiters_i18n::tr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Poses a second.
const RATE_HZ: u32 = 90;
/// After the recording started, before the camera moves: Spout and the kit's first frames.
const SETTLE: Duration = Duration::from_millis(700);
/// Slow enough for sharp frames and many keyframes.
const MAX_SPEED: f32 = 1.0;
const MAX_TURN: f32 = 60.0;
/// Directions the free space is measured in, around the starting point.
const DIRECTIONS: usize = 32;
/// Farthest a ring reaches, and how close it comes to what is in the way (share of the free distance).
const REACH_M: f32 = 8.0;
const CLEARANCE: f32 = 0.7;

/// Eases in and out.
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Poses are VRChat's: x, y, z in metres, then pitch, yaw, roll in degrees (Unity's conventions: +Y up, a positive
/// pitch looks down, yaw 0 looks along +Z and turns towards +X).
struct Flight {
    poses: Vec<[f32; 6]>,
}

impl Flight {
    fn last(&self) -> [f32; 6] {
        *self.poses.last().expect("starts with a pose")
    }

    /// To `to`, eased, no faster than [`MAX_SPEED`] and [`MAX_TURN`] (yaw as given: several turns are turned).
    fn move_to(&mut self, to: [f32; 6], at_least: f32) {
        let from = self.last();
        let distance = ((to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2) + (to[2] - from[2]).powi(2)).sqrt();
        let turn = (to[3] - from[3]).abs().max((to[4] - from[4]).abs());
        // Easing peaks at 1.5 × the mean speed.
        let seconds = (1.5 * distance / MAX_SPEED).max(1.5 * turn / MAX_TURN).max(at_least);
        let n = ((seconds * RATE_HZ as f32).round() as usize).max(1);
        for i in 1..=n {
            let t = smooth(i as f32 / n as f32);
            self.poses.push(std::array::from_fn(|k| from[k] + (to[k] - from[k]) * t));
        }
    }

    /// Once around `centre` at height `y`, `radius(direction)` out (direction in degrees, as yaw), each pose's gaze
    /// from `gaze(direction)` (pitch, yaw), starting where the ring is at `from_direction`.
    fn ring(&mut self, centre: [f32; 3], y: f32, from_direction: f32, radius: &dyn Fn(f32) -> f32, gaze: &dyn Fn(f32) -> (f32, f32)) {
        // Long enough for the slower of the turn and the path at its widest.
        let widest = (0..DIRECTIONS).map(|k| radius(from_direction + 360.0 * k as f32 / DIRECTIONS as f32)).fold(0.0f32, f32::max);
        let seconds = (std::f32::consts::TAU * widest / MAX_SPEED).max(360.0 / MAX_TURN);
        let n = ((seconds * RATE_HZ as f32).round() as usize).max(DIRECTIONS);
        let roll = self.last()[5];
        for i in 1..=n {
            let a = from_direction + 360.0 * i as f32 / n as f32;
            let r = radius(a);
            let w = a.to_radians();
            let (pitch, yaw) = gaze(a);
            self.poses.push([centre[0] + r * w.sin(), y, centre[2] + r * w.cos(), pitch, yaw, roll]);
        }
    }
}

/// How far the camera can go from `from` in each of [`DIRECTIONS`] directions (yaw degrees `360 k / DIRECTIONS`) before
/// something in the way, or a place the depth frames never saw beyond the first metre, capped at [`REACH_M`].
pub fn free_distances(map: Option<&CoverageMap>, from: [f32; 3]) -> [f32; DIRECTIONS] {
    let Some(map) = map else { return [1.0; DIRECTIONS] };
    let step = map.cell_m * 0.5;
    std::array::from_fn(|k| {
        let w = (360.0 * k as f32 / DIRECTIONS as f32).to_radians();
        let mut free = 0.0;
        let mut d = step;
        while d <= REACH_M {
            let blocked = match map.at(from[0] + d * w.sin(), from[2] + d * w.cos()) {
                Some((sectors, bits)) => bits & coverage::OBSTACLE != 0 || (sectors == 0 && d > 1.0),
                None => d > 1.0,
            };
            if blocked {
                break;
            }
            free = d;
            d += step;
        }
        free
    })
}

/// How fast a ring's radius may change with its direction, metres per radian: at most a few centimetres between two
/// poses, never a jump into a gap.
const RADIUS_SLOPE: f32 = 0.5;

/// A ring's radius towards `direction` (degrees): at most `wanted` and [`CLEARANCE`] of the free distance in every
/// measured direction, changing no faster than [`RADIUS_SLOPE`] (the lower envelope of those limits, each lowered by
/// what the envelope can rise between two measured directions, so it never exceeds them in between either).
fn ring_radius(free: &[f32; DIRECTIONS], wanted: f32, direction: f32) -> f32 {
    let half_gap = (180.0 / DIRECTIONS as f32).to_radians();
    (0..DIRECTIONS)
        .map(|k| {
            let towards = 360.0 * k as f32 / DIRECTIONS as f32;
            let apart = ((direction - towards).rem_euclid(360.0)).min((towards - direction).rem_euclid(360.0)).to_radians();
            wanted.min(CLEARANCE * free[k]) - RADIUS_SLOPE * half_gap + RADIUS_SLOPE * apart
        })
        .fold(f32::MAX, f32::min)
        .max(0.0)
}

/// The yaw equal to `target` (mod 360) nearest to `from`, so turning never goes the long way round.
fn near(from: f32, target: f32) -> f32 {
    from + (target - from + 180.0).rem_euclid(360.0) - 180.0
}

/// The look-around before the scan: a slow turn from `start`, looking 22° down (with VRChat's 47° camera, the floor
/// from a metre and a half out to the horizon), back to the starting heading.
pub fn look_around(start: [f32; 6]) -> Vec<[f32; 6]> {
    let mut f = Flight { poses: vec![start] };
    let [x, y, z, _, yaw, roll] = start;
    f.move_to([x, y, z, 22.0, yaw, roll], 0.0);
    f.move_to([x, y, z, 22.0, yaw + 360.0, roll], 0.0);
    f.move_to([x, y, z, start[3], yaw + 360.0, roll], 0.0);
    f.poses
}

/// The scan's rings from where the look-around ended (`start`, the camera's own pose), kept clear of what `map` says is
/// in the way; `floor_y` sets the low and high rings' heights when known. Ends exactly at `start`, some turns later.
pub fn rings(start: [f32; 6], map: Option<&CoverageMap>) -> Vec<[f32; 6]> {
    let centre = [start[0], start[1], start[2]];
    let free = free_distances(map, centre);
    let floor = map.and_then(|m| m.floor_y).filter(|f| *f < start[1] - 0.8).unwrap_or(start[1] - 1.6);
    // Eye height is the camera's own; low is a crouch, high a raised arm, under 2.3 m above the floor.
    let (low, high) = (floor + 0.55, (start[1] + 0.8).min(floor + 2.3));
    let middle = [centre[0], floor + 0.5, centre[2]];
    let mut f = Flight { poses: vec![start] };
    let roll = start[5];
    let point = |r: f32, a: f32, y: f32| {
        let w = a.to_radians();
        [centre[0] + r * w.sin(), y, centre[2] + r * w.cos()]
    };
    // The rings start in front of the camera.
    let a0 = start[4];
    // 1. Eye height, 1.5 m out, looking out and a little down.
    let r1 = |a: f32| ring_radius(&free, 1.5, a);
    let p = point(r1(a0), a0, start[1]);
    let base = start[4];
    f.move_to([p[0], p[1], p[2], 8.0, base, roll], 0.0);
    f.ring(centre, start[1], a0, &r1, &|a| (8.0, base + (a - a0)));
    // 2. High, 2.5 m out, looking in and down at the middle.
    let r2 = |a: f32| ring_radius(&free, 2.5, a);
    let down_to_middle = |a: f32| (high - middle[1]).atan2(r2(a).max(0.5)).to_degrees();
    let p = point(r2(a0), a0, high);
    let base = near(base + 360.0, a0 + 180.0);
    f.move_to([p[0], p[1], p[2], down_to_middle(a0), base, roll], 0.0);
    f.ring(centre, high, a0, &r2, &|a| (down_to_middle(a), base + (a - a0)));
    // 3. Low, 1.2 m out, looking out and a little up.
    let r3 = |a: f32| ring_radius(&free, 1.2, a);
    let p = point(r3(a0), a0, low);
    let base = near(base + 360.0, a0);
    f.move_to([p[0], p[1], p[2], -10.0, base, roll], 0.0);
    f.ring(centre, low, a0, &r3, &|a| (-10.0, base + (a - a0)));
    // Home.
    let home_yaw = near(base + 360.0, start[4]);
    f.move_to([start[0], start[1], start[2], start[3], home_yaw, roll], 0.0);
    let mut poses = f.poses;
    if let Some(last) = poses.last_mut() {
        *last = [start[0], start[1], start[2], start[3], home_yaw, roll];
    }
    poses
}

/// A scan waiting for its recording to be saved.
#[derive(Default)]
pub struct ScanRuntime {
    pub(super) recording: bool,
}

impl super::Controller {
    /// Scans the place around VRChat's camera where it stands now.
    pub fn take_scan(self: &Shared) -> anyhow::Result<()> {
        anyhow::ensure!(self.snapshot.lock().unwrap().vrchat.running, "{}", tr("Start VRChat and open its camera first."));
        self.ensure_capture_idle()?;
        let (pose, smooth) = {
            let osc = self.osc_values.lock().unwrap();
            let pose = osc.latest.get(super::burst::POSE).map(|(v, _)| super::burst::floats(v)).filter(|v| v.len() == 6 && v.iter().any(|f| *f != 0.0));
            let smooth = osc.latest.get(super::burst::SMOOTH).and_then(|(v, _)| v.first().map(orbiters_core::osc::OscValue::truthy));
            (pose, smooth)
        };
        let Some(pose) = pose else {
            anyhow::bail!("{}", tr("Move VRChat's camera once so Orbiters knows where it is, then try again."));
        };
        let pose: [f32; 6] = pose.try_into().expect("six values");
        self.scan.lock().unwrap().recording = true;
        if let Err(e) = self.start_capture(super::CapturePurpose::Short) {
            self.scan.lock().unwrap().recording = false;
            return Err(e);
        }
        // The recording this scan started: everything the scan does later is for it alone (C-2).
        let owner = self.capture_serial.load(std::sync::atomic::Ordering::Acquire);
        let ctl = self.clone();
        let flight = std::thread::Builder::new().name("scan".into()).spawn(move || {
            let started = Instant::now();
            while ctl.snapshot.lock().unwrap().capture.state != CaptureState::Recording {
                if started.elapsed() > Duration::from_secs(8) || !ctl.scan_owns(owner) {
                    // Its flag only while no other recording began (that one may be a scan of its own).
                    if ctl.capture_serial.load(std::sync::atomic::Ordering::Acquire) == owner {
                        ctl.scan.lock().unwrap().recording = false;
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            std::thread::sleep(SETTLE);
            let send = |address: &str, args: Vec<rosc::OscType>| {
                let _ = ctl.with_osc(|o| o.send(address, args));
            };
            let fly = |poses: Vec<[f32; 6]>| {
                let tick = Duration::from_secs_f32(1.0 / RATE_HZ as f32);
                let begin = Instant::now();
                for (i, p) in poses.into_iter().enumerate() {
                    {
                        // Checked and sent under the start gate: no other recording can begin in between.
                        let _gate = ctl.capture_start_gate.lock().unwrap();
                        if !ctl.scan_owns(owner) {
                            return false;
                        }
                        // N4N0's camera rig follows in the same bundle (controller/rig.rs).
                        ctl.place_rig(Some((super::burst::POSE.to_string(), p.iter().map(|v| rosc::OscType::Float(*v)).collect())), &p);
                    }
                    if let Some(wait) = (tick * (i as u32 + 1)).checked_sub(begin.elapsed()) {
                        std::thread::sleep(wait);
                    }
                }
                true
            };
            // Smoothing would trail the path behind.
            {
                let _gate = ctl.capture_start_gate.lock().unwrap();
                if !ctl.scan_owns(owner) {
                    return;
                }
                send(super::burst::SMOOTH, vec![rosc::OscType::Bool(false)]);
            }
            let mut done = fly(look_around(pose));
            if done {
                // A coverage map with the look-around's depth in it (the worker sends one a second).
                std::thread::sleep(Duration::from_millis(1200));
                let map = ctl.snapshot.lock().unwrap().capture.coverage.clone();
                done = fly(rings(pose, map.as_ref()));
            }
            {
                // The camera goes back where it was, also after a scan stopped early, unless another recording
                // began since: that one's camera is not the scan's to move.
                let _gate = ctl.capture_start_gate.lock().unwrap();
                if ctl.capture_serial.load(std::sync::atomic::Ordering::Acquire) == owner {
                    send(super::burst::POSE, pose.iter().map(|v| rosc::OscType::Float(*v)).collect());
                    if let Some(smooth) = smooth.filter(|s| *s) {
                        send(super::burst::SMOOTH, vec![rosc::OscType::Bool(smooth)]);
                    }
                }
            }
            if done {
                std::thread::sleep(Duration::from_millis(250));
                ctl.scan_stop(owner);
            }
        });
        if let Err(e) = flight {
            // The recording was accepted, but nothing would fly it or end it: it ends here.
            self.scan_stop(owner);
            return Err(e.into());
        }
        Ok(())
    }

    /// Whether the recording a scan started (`owner`, `Controller::capture_serial`) is still the one under way.
    /// Asked under `capture_start_gate` by whoever acts on the answer.
    pub(super) fn scan_owns(&self, owner: u64) -> bool {
        self.capture_serial.load(std::sync::atomic::Ordering::Acquire) == owner && self.is_capturing()
    }

    /// Stops the scan's own recording, never one started after it.
    pub(super) fn scan_stop(self: &Shared, owner: u64) {
        let _gate = self.capture_start_gate.lock().unwrap();
        if !self.scan_owns(owner) {
            return;
        }
        if let Err(e) = self.stop_capture() {
            self.notify(NoticeKind::Error, e.to_string());
        }
    }

    /// A scan's recording was saved (the caller took its flag before the state went Idle): it goes on to its
    /// reconstruction.
    pub(super) fn scan_saved(self: &Shared, session: Option<PathBuf>) {
        let Some(dir) = session else { return };
        match self.queue_reconstruction(dir) {
            Ok(false) => self.notify(NoticeKind::Success, tr("Scan recorded: it is being reconstructed.")),
            Ok(true) => self.notify(NoticeKind::Success, tr!("Scan recorded: its reconstruction starts {when}.", when = tr(self.reconstruction_waits_for()))),
            Err(e) => self.notify(NoticeKind::Warning, tr!("Scan recorded, not reconstructed: {error}", error = e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Looking along +X (yaw 90) at eye height 1.6 m, slightly down.
    const POSE: [f32; 6] = [1.0, 1.6, 0.0, 10.0, 90.0, 0.0];

    fn smooth_all_along(poses: &[[f32; 6]]) {
        for w in poses.windows(2) {
            let d = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2) + (w[1][2] - w[0][2]).powi(2)).sqrt();
            // 90 poses a second: at most 1.5 × the speed and turn limits.
            assert!(d <= 1.5 * MAX_SPEED / RATE_HZ as f32 + 1e-3, "{d} m between {w:?}");
            assert!(
                (w[1][4] - w[0][4]).abs() <= 1.5 * MAX_TURN / RATE_HZ as f32 + 0.2 && (w[1][3] - w[0][3]).abs() <= 1.5 * MAX_TURN / RATE_HZ as f32 + 0.2,
                "{w:?}"
            );
        }
    }

    /// A room 10 × 10 m around the start whose east wall stands 2 m away: a coverage map of floor with that wall.
    fn room() -> CoverageMap {
        let (cell, n) = (0.5f32, 40u32);
        let (x0, z0) = (POSE[0] - 10.0, POSE[2] - 10.0);
        let mut cells = vec![0u8; (2 * n * n) as usize];
        for iz in 0..n {
            for ix in 0..n {
                let (x, z) = (x0 + (ix as f32 + 0.5) * cell, z0 + (iz as f32 + 0.5) * cell);
                let i = (2 * (iz * n + ix)) as usize;
                let wall = (x - POSE[0] >= 2.0 && x - POSE[0] < 2.5) || (x - POSE[0]).abs() > 5.0 || (z - POSE[2]).abs() > 5.0;
                cells[i] = 6;
                cells[i + 1] = if wall { coverage::OBSTACLE } else { coverage::FLOOR | coverage::HIGH };
            }
        }
        CoverageMap { cell_m: cell, x0, z0, width: n, depth: n, cells, floor_y: Some(0.0), hints: Vec::new() }
    }

    #[test]
    fn look_around_turns_once_looking_down_and_comes_back() {
        let poses = look_around(POSE);
        smooth_all_along(&poses);
        let last = poses.last().unwrap();
        assert!((last[4] - POSE[4] - 360.0).abs() < 1e-3 && (last[3] - POSE[3]).abs() < 1e-3);
        assert!(poses.iter().all(|p| p[..3] == POSE[..3]));
        assert!(poses.iter().any(|p| (p[3] - 22.0).abs() < 1e-3));
    }

    #[test]
    fn rings_keep_clear_of_the_wall_at_three_heights() {
        let map = room();
        let poses = rings(POSE, Some(&map));
        smooth_all_along(&poses);
        assert_eq!(poses.first(), Some(&POSE));
        let last = poses.last().unwrap();
        assert!(last[..4].iter().zip(&POSE[..4]).all(|(a, b)| (a - b).abs() < 1e-4) && ((last[4] - POSE[4]) % 360.0).abs() < 1e-3, "{last:?}");
        // Never past 70 % of the way to the east wall (2 m away), nor 5 m out anywhere.
        let east = poses.iter().map(|p| p[0] - POSE[0]).fold(f32::MIN, f32::max);
        assert!(east <= 0.7 * 2.0 + 0.05, "east {east}");
        let far = poses.iter().map(|p| ((p[0] - POSE[0]).powi(2) + (p[2] - POSE[2]).powi(2)).sqrt()).fold(0.0f32, f32::max);
        assert!(far > 2.3 && far <= 2.5 + 1e-3, "farthest {far}");
        // Low (a crouch over the floor at 0), eye height and high (under 2.3 m).
        let heights: Vec<f32> = poses.iter().map(|p| p[1]).collect();
        let (lo, hi) = (heights.iter().copied().fold(f32::MAX, f32::min), heights.iter().copied().fold(f32::MIN, f32::max));
        assert!((lo - 0.55).abs() < 1e-3 && (hi - 2.3).abs() < 1e-3, "heights {lo}..{hi}");
        // Looking down at the middle from high, a little up from low; every heading.
        assert!(poses.iter().any(|p| p[1] > 2.2 && p[3] > 20.0));
        assert!(poses.iter().any(|p| p[1] < 0.6 && p[3] < -5.0));
        let mut headings: Vec<i32> = poses.iter().map(|p| (p[4].rem_euclid(360.0) / 30.0) as i32).collect();
        headings.sort();
        headings.dedup();
        assert_eq!(headings.len(), 12);
        // Under a minute.
        assert!(poses.len() < 60 * RATE_HZ as usize, "{} s", poses.len() / RATE_HZ as usize);
    }

    #[test]
    fn without_a_map_the_rings_stay_within_reach_of_the_camera() {
        let poses = rings(POSE, None);
        smooth_all_along(&poses);
        let far = poses.iter().map(|p| ((p[0] - POSE[0]).powi(2) + (p[2] - POSE[2]).powi(2)).sqrt()).fold(0.0f32, f32::max);
        assert!(far <= CLEARANCE + 1e-3, "{far}");
    }

    /// C-2: a scan whose recording was stopped, followed by another recording, neither stops nor flies that one: its
    /// late Stop (after its last pose) is for its own recording only.
    #[test]
    fn an_old_scan_never_stops_the_next_recording() {
        use orbiters_core::protocol::WorkerCommand;
        use std::sync::atomic::Ordering;
        let ctl = super::super::Controller::new(orbiters_core::settings::Settings::default());
        *ctl.test_commands.lock().unwrap() = Some(Vec::new());
        let stops = |ctl: &super::super::Shared| ctl.test_commands.lock().unwrap().as_ref().unwrap().iter().filter(|c| matches!(c, WorkerCommand::Stop)).count();
        // The scan's recording.
        ctl.capture_serial.store(4, Ordering::Release);
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        let old = ctl.capture_serial.load(Ordering::Acquire);
        assert!(ctl.scan_owns(old));
        // Stopped by the person, then another recording starts before the scan's thread wakes.
        ctl.update(|s| s.capture.state = CaptureState::Idle);
        assert!(!ctl.scan_owns(old), "an ended recording is no scan's");
        ctl.capture_serial.fetch_add(1, Ordering::AcqRel);
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        let generation = ctl.stop_generation.load(Ordering::Acquire);
        ctl.scan_stop(old);
        assert_eq!(stops(&ctl), 0, "the old scan sent no Stop");
        assert_eq!(ctl.stop_generation.load(Ordering::Acquire), generation, "nor ended the new recording's generation");
        assert!(!ctl.scan_owns(old), "nor may it fly that recording's camera");
        // A scan stops its own recording.
        let new = ctl.capture_serial.load(Ordering::Acquire);
        ctl.scan_stop(new);
        assert_eq!(stops(&ctl), 1);
    }
}
