//! SteamVR device poses of a session (contracts `Docs/performance.md`): headset, controllers and trackers
//! (including the virtual ones of full-body estimation tools) sampled at a fixed rate on their own thread, in the
//! standing play space converted to Unity conventions. A T-pose held still while recording (VRChat's own
//! calibration asks for one) is kept as a calibration, so the avatar's proportions can be matched later.

use orbiters_contracts::performance::calibration::Calibration;
use orbiters_contracts::performance::tracking::{self, Device, DeviceClass, Devices, Sample};
use orbiters_core::pose::{calibration_of, TPoseDetector};
use orbiters_openvr::{self as vr, sample_of, Pose, MAX_DEVICES};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// What the recorder needs from SteamVR (a stand-in drives the tests).
pub trait PoseSource: Send {
    /// Current poses in the standing universe; `false` once the source is gone (SteamVR quit).
    fn sample(&mut self, out: &mut [Pose; MAX_DEVICES]) -> bool;
    /// What a device is; `None` while it is not known yet.
    fn describe(&mut self, index: u32) -> Option<Device>;
    /// Whether devices changed roles (or properties) since the last call: they are described again.
    fn take_roles_changed(&mut self) -> bool {
        false
    }
}

/// SteamVR as a background application: it never starts SteamVR.
pub struct SteamVr {
    client: vr::Client,
    /// Roles changed: devices are described again.
    pub roles_changed: bool,
}

impl SteamVr {
    pub fn connect() -> Result<Self, vr::Error> {
        Ok(Self { client: vr::Client::connect(vr::AppType::Background)?, roles_changed: false })
    }
}

impl PoseSource for SteamVr {
    fn sample(&mut self, out: &mut [Pose; MAX_DEVICES]) -> bool {
        while let Some(e) = self.client.poll_event() {
            match e.event_type {
                vr::event::QUIT => {
                    self.client.acknowledge_quit();
                    return false;
                }
                vr::event::TRACKED_DEVICE_ROLE_CHANGED | vr::event::PROPERTY_CHANGED => self.roles_changed = true,
                _ => {}
            }
        }
        self.client.poses(vr::Universe::Standing, out);
        true
    }

    fn describe(&mut self, index: u32) -> Option<Device> {
        self.client.describe(index)
    }

    fn take_roles_changed(&mut self) -> bool {
        std::mem::take(&mut self.roles_changed)
    }
}

/// What a finished recording leaves for the manifest.
pub struct TrackingResult {
    pub samples: u64,
    pub devices: Vec<Device>,
    pub calibrations: Vec<Calibration>,
}

/// Connects to the pose source; `None` while it is not available (SteamVR not running).
pub type Connect = Box<dyn FnMut() -> Option<Box<dyn PoseSource>> + Send>;

/// SteamVR, retried until it runs. Why it cannot be reached is logged when that changes, not at every attempt
/// ("not installed" every 3 s filled the worker's log in twenty minutes).
pub fn steamvr() -> Connect {
    let mut logged: Option<String> = None;
    Box::new(move || match SteamVr::connect() {
        Ok(s) => {
            logged = None;
            Some(Box::new(s) as Box<dyn PoseSource>)
        }
        Err(e) => {
            let why = e.to_string();
            if !e.not_running() && logged.as_deref() != Some(why.as_str()) {
                eprintln!("SteamVR: {why}");
                logged = Some(why);
            }
            None
        }
    })
}

/// Records poses on its own thread until [`TrackingRecorder::finish`]. Waits for SteamVR when it is not running
/// and reconnects when it restarts.
pub struct TrackingRecorder {
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    counters: Arc<Counters>,
    thread: Option<JoinHandle<anyhow::Result<TrackingResult>>>,
}

#[derive(Default)]
struct Counters {
    samples: AtomicU64,
    devices: AtomicU64,
    calibrations: AtomicU64,
    connected: AtomicBool,
    /// The devices and the T-poses caught so far, for the session's checkpoints.
    found: Mutex<(Vec<Device>, Vec<Calibration>)>,
}

/// What the interface shows while recording.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrackingCounts {
    pub connected: bool,
    pub devices: u64,
    pub samples: u64,
    pub calibrations: u64,
}

impl TrackingRecorder {
    /// Starts sampling at `rate_hz` into `<dir>/tracking.bin` (+ `tracking.json` when it finishes).
    pub fn start(connect: Connect, dir: &Path, epoch_ns: u64, rate_hz: f32) -> anyhow::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(Counters::default());
        let thread = {
            let (stop, paused, counters, dir) = (stop.clone(), paused.clone(), counters.clone(), dir.to_path_buf());
            std::thread::Builder::new().name("tracking".into()).spawn(move || {
                let mut rec = Recording::new(&dir, epoch_ns, rate_hz, counters);
                rec.run(connect, &stop, &paused)?;
                rec.finish()
            })?
        };
        Ok(Self { stop, paused, counters, thread: Some(thread) })
    }

    /// While paused nothing is written (a pause means "not this part").
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    /// The devices numbered so far and the T-poses caught so far (the session's `calibration.json` while recording).
    pub fn found(&self) -> (Vec<Device>, Vec<Calibration>) {
        self.counters.found.lock().unwrap().clone()
    }

    pub fn counts(&self) -> TrackingCounts {
        let c = &self.counters;
        TrackingCounts {
            connected: c.connected.load(Ordering::Relaxed),
            devices: c.devices.load(Ordering::Relaxed),
            samples: c.samples.load(Ordering::Relaxed),
            calibrations: c.calibrations.load(Ordering::Relaxed),
        }
    }

    pub fn finish(mut self) -> anyhow::Result<TrackingResult> {
        self.stop.store(true, Ordering::Relaxed);
        match self.thread.take().expect("running").join() {
            Ok(r) => r,
            Err(_) => anyhow::bail!("{}", orbiters_i18n::tr("the tracking thread panicked")),
        }
    }
}

impl Drop for TrackingRecorder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The recording's state across connections.
struct Recording {
    dir: std::path::PathBuf,
    epoch_ns: u64,
    period: Duration,
    file: Option<BufWriter<File>>,
    devices: Devices,
    /// SteamVR index → device number in `tracking.json` (`None` for base stations), for the current connection.
    known: HashMap<u32, Option<u8>>,
    detector: TPoseDetector,
    calibrations: Vec<Calibration>,
    written: u64,
    counters: Arc<Counters>,
    /// A device got its number or a new SteamVR index since `tracking.json` was written.
    devices_changed: bool,
}

impl Recording {
    fn new(dir: &Path, epoch_ns: u64, rate_hz: f32, counters: Arc<Counters>) -> Self {
        Self {
            dir: dir.to_path_buf(),
            epoch_ns,
            period: Duration::from_secs_f32(1.0 / rate_hz.clamp(1.0, 1000.0)),
            file: None,
            devices: Devices::new(rate_hz),
            known: HashMap::new(),
            detector: TPoseDetector::default(),
            calibrations: Vec::new(),
            written: 0,
            counters,
            devices_changed: false,
        }
    }

    fn run(&mut self, mut connect: Connect, stop: &AtomicBool, paused: &AtomicBool) -> anyhow::Result<()> {
        while !stop.load(Ordering::Relaxed) {
            let Some(mut source) = connect() else {
                // Not running: look again in a few seconds.
                for _ in 0..30 {
                    if stop.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                continue;
            };
            self.counters.connected.store(true, Ordering::Relaxed);
            self.known.clear();
            let result = self.sample_until_gone(source.as_mut(), stop, paused);
            self.counters.connected.store(false, Ordering::Relaxed);
            result?;
        }
        Ok(())
    }

    fn sample_until_gone(&mut self, source: &mut dyn PoseSource, stop: &AtomicBool, paused: &AtomicBool) -> anyhow::Result<()> {
        let mut poses = [Pose::default(); MAX_DEVICES];
        let mut next = std::time::Instant::now();
        let mut last_flush = std::time::Instant::now();
        while !stop.load(Ordering::Relaxed) {
            if !source.sample(&mut poses) {
                eprintln!("SteamVR quit; waiting for it to come back");
                return Ok(());
            }
            // A controller that gets its role late, hands swapped in SteamVR: every device is described again (the
            // numbers stay, by serial).
            if source.take_roles_changed() {
                self.known.clear();
            }
            let t = orbiters_core::clock::now_ns().saturating_sub(self.epoch_ns);
            let mut current: Vec<Sample> = Vec::new();
            for (i, p) in poses.iter().enumerate() {
                if !p.device_is_connected {
                    continue;
                }
                if let Some(number) = self.number_of(source, i as u32, t) {
                    current.push(sample_of(t, number as usize, p));
                }
            }
            // Before any sample uses a new number: tracking.bin never holds a device tracking.json does not name.
            if self.devices_changed {
                if let Err(e) = self.write_devices() {
                    eprintln!("tracking.json: {e:#}");
                }
            }
            if !paused.load(Ordering::Relaxed) {
                self.write(&current)?;
                // A held T-pose (VRChat's calibration, or one done for Orbiters) becomes a calibration.
                let in_pose = match (self.by_role("head", &current), self.by_role("left_hand", &current), self.by_role("right_hand", &current)) {
                    (Some(h), Some(l), Some(r)) => TPoseDetector::is_t_pose(h, l, r),
                    _ => false,
                };
                if self.detector.update(t, in_pose) {
                    self.calibrations.push(calibration_of(t as i64, "detected", &self.devices.devices, &current));
                    self.counters.calibrations.store(self.calibrations.len() as u64, Ordering::Relaxed);
                    self.counters.found.lock().unwrap().1 = self.calibrations.clone();
                }
            }
            if last_flush.elapsed() >= Duration::from_secs(1) {
                if let Some(f) = self.file.as_mut() {
                    f.flush()?;
                }
                last_flush = std::time::Instant::now();
            }
            next += self.period;
            let now = std::time::Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else if now - next > self.period * 4 {
                // Far behind (the machine stalled): skip ahead instead of sampling in a burst.
                next = now;
            }
        }
        Ok(())
    }

    /// The device's number in `tracking.json`, describing it the first time (a device seen before a SteamVR
    /// restart keeps its number: same serial), and again after a role change (its role follows).
    fn number_of(&mut self, source: &mut dyn PoseSource, index: u32, t: u64) -> Option<u8> {
        if let Some(n) = self.known.get(&index) {
            return *n;
        }
        let mut d = source.describe(index)?;
        if d.class == DeviceClass::TrackingReference {
            // Base stations stand still: not worth a stream.
            self.known.insert(index, None);
            return None;
        }
        let existing = self.devices.devices.iter().position(|e| !d.serial.is_empty() && e.serial == d.serial);
        let number = match existing {
            Some(n) => {
                let e = &mut self.devices.devices[n];
                // A role it lost for a moment (switched off) is kept.
                let role = if d.role.is_empty() { e.role.clone() } else { d.role };
                if e.index != index as u8 || e.role != role {
                    e.index = index as u8;
                    e.role = role;
                    self.devices_changed = true;
                }
                n
            }
            None => {
                d.first_seen_ns = t;
                self.devices.devices.push(d);
                self.devices_changed = true;
                self.devices.devices.len() - 1
            }
        };
        let number = u8::try_from(number).ok();
        self.known.insert(index, number);
        self.counters.devices.store(self.devices.devices.len() as u64, Ordering::Relaxed);
        number
    }

    fn by_role<'a>(&self, role: &str, current: &'a [Sample]) -> Option<&'a Sample> {
        let number = self.devices.devices.iter().position(|d| d.role == role)?;
        current.iter().find(|s| s.device as usize == number)
    }

    fn write(&mut self, current: &[Sample]) -> anyhow::Result<()> {
        if current.is_empty() {
            return Ok(());
        }
        if self.file.is_none() {
            let mut f = BufWriter::with_capacity(1 << 16, File::create(self.dir.join(orbiters_contracts::performance::TRACKING_FILE))?);
            f.write_all(&tracking::header())?;
            self.file = Some(f);
        }
        let f = self.file.as_mut().expect("open");
        for s in current {
            f.write_all(&s.to_bytes())?;
        }
        self.written += current.len() as u64;
        self.counters.samples.store(self.written, Ordering::Relaxed);
        Ok(())
    }

    /// Writes `tracking.json` as it stands.
    fn write_devices(&mut self) -> anyhow::Result<()> {
        self.devices_changed = false;
        orbiters_core::write_atomic(&self.dir.join(orbiters_contracts::performance::TRACKING_DEVICES), &serde_json::to_vec_pretty(&self.devices)?)?;
        self.counters.found.lock().unwrap().0 = self.devices.devices.clone();
        Ok(())
    }

    fn finish(mut self) -> anyhow::Result<TrackingResult> {
        if let Some(mut f) = self.file.take() {
            f.flush()?;
        }
        if !self.devices.devices.is_empty() {
            self.write_devices()?;
        }
        Ok(TrackingResult { samples: self.written, devices: self.devices.devices, calibrations: self.calibrations })
    }
}

/// `calibration.json` of a session: calibrations made before it (negative times) and those it caught.
pub fn write_calibrations(dir: &Path, calibrations: Vec<Calibration>) -> anyhow::Result<u64> {
    let Some(json) = calibrations_json(calibrations)? else { return Ok(0) };
    orbiters_core::write_atomic(&dir.join(orbiters_contracts::performance::CALIBRATION_FILE), &json)?;
    Ok(json.len() as u64)
}

/// The bytes of `calibration.json`, `None` without calibrations (a session without any has no file).
pub fn calibrations_json(calibrations: Vec<Calibration>) -> anyhow::Result<Option<Vec<u8>>> {
    if calibrations.is_empty() {
        return Ok(None);
    }
    let mut all = orbiters_contracts::performance::calibration::Calibrations::new();
    all.calibrations = calibrations;
    Ok(Some(serde_json::to_vec_pretty(&all)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_openvr::Matrix34;

    /// A headset, two controllers and a base station: standing, a T-pose from 0.6 s to 2.7 s (stand-in time), then
    /// arms down.
    struct StandIn {
        t: f32,
        step: f32,
    }

    fn pose_at(x: f32, y: f32, z: f32) -> Pose {
        let mut m = Matrix34::IDENTITY;
        m.m[0][3] = x;
        m.m[1][3] = y;
        m.m[2][3] = z;
        Pose { device_to_absolute: m, tracking_result: 200, pose_is_valid: true, device_is_connected: true, ..Default::default() }
    }

    impl PoseSource for StandIn {
        fn sample(&mut self, out: &mut [Pose; MAX_DEVICES]) -> bool {
            self.t += self.step;
            let t_pose = (0.6..2.7).contains(&self.t);
            out[0] = pose_at(0.0, 1.62, 0.0);
            let (hy, hx) = if t_pose { (1.40, 0.70) } else { (0.85, 0.25) };
            out[1] = pose_at(-hx, hy, 0.0);
            out[2] = pose_at(hx, hy, 0.0);
            out[3] = pose_at(0.0, 0.0, 3.0);
            out[4] = Pose::default();
            true
        }

        fn describe(&mut self, index: u32) -> Option<Device> {
            let (class, role) = match index {
                0 => (DeviceClass::Hmd, "head"),
                1 => (DeviceClass::Controller, "left_hand"),
                2 => (DeviceClass::Controller, "right_hand"),
                3 => (DeviceClass::TrackingReference, ""),
                _ => return None,
            };
            Some(Device { index: index as u8, class, role: role.into(), serial: format!("SN{index}"), ..Default::default() })
        }
    }

    #[test]
    fn records_devices_and_catches_a_t_pose() {
        let dir = std::env::temp_dir().join(format!("orbiters-tracking-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 100 Hz sampling with a stand-in whose clock advances 30 ms a sample: 3 s of motion in ~1 s.
        let mut once = Some(Box::new(StandIn { t: 0.0, step: 0.03 }) as Box<dyn PoseSource>);
        let rec = TrackingRecorder::start(Box::new(move || once.take()), &dir, orbiters_core::clock::now_ns(), 100.0).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(rec.counts().connected);
        // While recording, tracking.json already names every device (a session that never finishes keeps them).
        let early: Devices = serde_json::from_slice(&std::fs::read(dir.join("tracking.json")).unwrap()).unwrap();
        assert_eq!(early.devices.len(), 3);
        std::thread::sleep(Duration::from_millis(1200));
        let (devices, caught) = rec.found();
        assert_eq!((devices.len(), caught.len()), (3, 1));
        let result = rec.finish().unwrap();
        assert_eq!(result.devices.len(), 3, "base station left out");
        let samples = tracking::read(&std::fs::read(dir.join("tracking.bin")).unwrap()).unwrap();
        assert_eq!(samples.len() as u64, result.samples);
        assert!(samples.len() >= 3 * 80, "{} samples", samples.len());
        assert!(samples.iter().all(|s| s.device <= 2 && s.pose_valid()), "devices are numbered 0..3, base station skipped");
        // Unity conventions: the headset at 1.62 m, OpenVR -Z forward becomes +Z.
        let head = samples.iter().find(|s| s.device == 0).unwrap();
        assert_eq!((head.position, head.rotation), ([0.0, 1.62, -0.0], [0.0, 0.0, 0.0, 1.0]));
        let devices: Devices = serde_json::from_slice(&std::fs::read(dir.join("tracking.json")).unwrap()).unwrap();
        assert_eq!(devices.devices.iter().map(|d| d.role.as_str()).collect::<Vec<_>>(), ["head", "left_hand", "right_hand"]);
        // The T-pose (held still for ~0.7 s of real time) was caught once, with every device.
        assert_eq!(result.calibrations.len(), 1, "{:?}", result.calibrations.len());
        let c = &result.calibrations[0];
        assert_eq!((c.source.as_str(), c.devices.len()), ("detected", 3));
        assert!((c.head_height_m - 1.62).abs() < 1e-5 && (c.arm_span_m - 1.4).abs() < 1e-5);
        assert!(write_calibrations(&dir, result.calibrations).unwrap() > 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Controllers that get their roles late (SteamVR names the hands once both are on): first described without
    /// one, then told of the change.
    struct LateRoles {
        inner: StandIn,
        samples: u32,
        changed: bool,
    }

    impl PoseSource for LateRoles {
        fn sample(&mut self, out: &mut [Pose; MAX_DEVICES]) -> bool {
            self.samples += 1;
            if self.samples == 10 {
                self.changed = true;
            }
            self.inner.sample(out)
        }

        fn describe(&mut self, index: u32) -> Option<Device> {
            let mut d = self.inner.describe(index)?;
            if self.samples < 10 && d.class == DeviceClass::Controller {
                d.role.clear();
            }
            Some(d)
        }

        fn take_roles_changed(&mut self) -> bool {
            std::mem::take(&mut self.changed)
        }
    }

    #[test]
    fn roles_given_late_are_followed() {
        let dir = std::env::temp_dir().join(format!("orbiters-tracking-roles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut once = Some(Box::new(LateRoles { inner: StandIn { t: 0.0, step: 0.03 }, samples: 0, changed: false }) as Box<dyn PoseSource>);
        let rec = TrackingRecorder::start(Box::new(move || once.take()), &dir, orbiters_core::clock::now_ns(), 100.0).unwrap();
        std::thread::sleep(Duration::from_millis(1300));
        let result = rec.finish().unwrap();
        assert_eq!(result.devices.iter().map(|d| d.role.as_str()).collect::<Vec<_>>(), ["head", "left_hand", "right_hand"]);
        // The T-pose came after the hands were named: caught.
        assert_eq!(result.calibrations.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// SteamVR restarting mid-session: the same devices come back with other indices and keep their numbers.
    struct Restarting {
        calls: u32,
        shift: u32,
    }

    impl PoseSource for Restarting {
        fn sample(&mut self, out: &mut [Pose; MAX_DEVICES]) -> bool {
            self.calls += 1;
            if self.calls > 5 {
                return false;
            }
            *out = [Pose::default(); MAX_DEVICES];
            out[(self.shift) as usize] = pose_at(0.0, 1.6, 0.0);
            out[(self.shift + 1) as usize] = pose_at(0.3, 1.0, 0.0);
            true
        }

        fn describe(&mut self, index: u32) -> Option<Device> {
            let serial = if index == self.shift { "HMD" } else { "TRACKER" };
            Some(Device { index: index as u8, class: DeviceClass::Tracker, serial: serial.into(), ..Default::default() })
        }
    }

    #[test]
    fn reconnects_and_keeps_device_numbers() {
        let dir = std::env::temp_dir().join(format!("orbiters-tracking-restart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut sources = vec![Box::new(Restarting { calls: 0, shift: 7 }) as Box<dyn PoseSource>, Box::new(Restarting { calls: 0, shift: 0 })];
        let rec = TrackingRecorder::start(Box::new(move || sources.pop()), &dir, orbiters_core::clock::now_ns(), 200.0).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let result = rec.finish().unwrap();
        assert_eq!((result.devices.len(), result.samples), (2, 20));
        let devices: Devices = serde_json::from_slice(&std::fs::read(dir.join("tracking.json")).unwrap()).unwrap();
        assert_eq!(devices.devices.iter().map(|d| (d.serial.as_str(), d.index)).collect::<Vec<_>>(), [("HMD", 7), ("TRACKER", 8)]);
        let samples = tracking::read(&std::fs::read(dir.join("tracking.bin")).unwrap()).unwrap();
        assert!(samples.iter().all(|s| (s.device == 0) == (s.position[1] == 1.6)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn velocities_follow_the_mirror() {
        let mut p = pose_at(1.0, 2.0, 3.0);
        p.velocity = [1.0, 2.0, 3.0];
        p.angular_velocity = [0.1, 0.2, 0.3];
        let s = sample_of(5, 1, &p);
        assert_eq!((s.position, s.velocity, s.angular_velocity), ([1.0, 2.0, -3.0], [1.0, 2.0, -3.0], [-0.1, -0.2, 0.3]));
        assert_eq!((s.time_ns, s.device, s.result, s.flags), (5, 1, 200, tracking::flags::POSE_VALID | tracking::flags::CONNECTED));
    }
}
