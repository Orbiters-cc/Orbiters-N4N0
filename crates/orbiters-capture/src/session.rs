//! One capture session: decodes every frame's strip (and depth symbols on depth frames), chooses keyframes,
//! and writes the session bundle. Keyframe images are encoded on a small thread pool so capture never waits
//! on disk or compression; when the pool falls behind, keyframes are dropped (and counted) instead.

use crate::audio::{AudioRecorder, AudioSource};
use crate::coverage::Coverage;
use crate::performance::{ParameterStream, SkeletonStream};
use crate::tracking::{self, TrackingRecorder};
use crossbeam_channel::{bounded, Receiver as ChanReceiver, Sender as ChanSender, TrySendError};
use orbiters_contracts::frame_record::{record_flags, FrameRecord};
use orbiters_contracts::image::{ImageView, PixelOrder};
use orbiters_contracts::manifest::{depth_file, SessionManifest, StorageQuality};
use orbiters_contracts::performance::calibration::Calibration;
use orbiters_contracts::performance::tracking::Device;
use orbiters_contracts::performance::{self as perf, audio::TrackKind};
use orbiters_contracts::{depth, qoi, skeleton, strip};
use orbiters_core::bundle::{self, BundleWriter};
use orbiters_core::clock;
use orbiters_core::keyframes::{self, KeyframeSelector};
use orbiters_core::protocol::{CaptureStats, CoverageMap, OscSample, StartCapture};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

enum Job {
    Keyframe { path: PathBuf, data: Vec<u8>, width: u32, height: u32, bgra: bool },
    Depth { path: PathBuf, bytes: Vec<u8> },
}

struct Pool {
    tx: Option<ChanSender<Job>>,
    threads: Vec<JoinHandle<()>>,
    written: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
}

impl Pool {
    fn new(threads: usize) -> Self {
        let (tx, rx): (ChanSender<Job>, ChanReceiver<Job>) = bounded(threads * 2);
        let written = Arc::new(AtomicU64::new(0));
        let errors = Arc::new(AtomicU64::new(0));
        let handles = (0..threads)
            .map(|i| {
                let rx = rx.clone();
                let written = written.clone();
                let errors = errors.clone();
                std::thread::Builder::new()
                    .name(format!("encode-{i}"))
                    .spawn(move || {
                        for job in rx {
                            let (path, bytes) = match job {
                                Job::Keyframe { path, data, width, height, bgra } => {
                                    (path, qoi::encode_bgra_strided(&data, width, height, width as usize * 4, bgra, true))
                                }
                                Job::Depth { path, bytes } => (path, bytes),
                            };
                            match std::fs::write(&path, &bytes) {
                                Ok(()) => {
                                    written.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                                }
                                Err(e) => {
                                    eprintln!("write {}: {e}", path.display());
                                    errors.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    })
                    .expect("spawn encoder")
            })
            .collect();
        Self { tx: Some(tx), threads: handles, written, errors }
    }

    fn try_send(&self, job: Job) -> bool {
        match self.tx.as_ref().expect("pool open").try_send(job) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Depth files are small and must not be lost: block until queued.
    fn send(&self, job: Job) {
        let _ = self.tx.as_ref().expect("pool open").send(job);
    }

    fn finish(mut self) -> (u64, u64) {
        self.tx = None;
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        (self.written.load(Ordering::Relaxed), self.errors.load(Ordering::Relaxed))
    }
}

/// Copies a borrowed frame into a tightly packed, upright buffer (4 bytes per pixel, source channel order).
fn copy_upright(view: &ImageView, flipped: bool) -> Vec<u8> {
    let row = view.width as usize * 4;
    let mut out = vec![0u8; row * view.height as usize];
    for y in 0..view.height as usize {
        let src_row = if flipped { view.height as usize - 1 - y } else { y };
        out[y * row..(y + 1) * row].copy_from_slice(&view.data[src_row * view.stride..src_row * view.stride + row]);
    }
    out
}

pub struct CaptureSession {
    writer: BundleWriter,
    selector: KeyframeSelector,
    pool: Pool,
    pub stats: CaptureStats,
    /// Session start on the shared clock: every stream's `time_ns` counts from here.
    epoch_ns: u64,
    parameters: ParameterStream,
    skeleton: SkeletonStream,
    audio: Vec<AudioRecorder>,
    tracking: Option<TrackingRecorder>,
    /// Calibrations made in the overlay (before the session or during it), on the session clock.
    calibrations: Vec<Calibration>,
    /// When the side streams' tables and the manifest were last written ([`CaptureSession::checkpoint`]), and the
    /// calibrations and devices `calibration.json` was written with.
    checkpointed: Instant,
    calibrations_written: (usize, usize),
    frame_index: u64,
    prefer_flipped: bool,
    previous: Option<(u32, [f32; 12], f32)>,
    thumbnail_done: bool,
    decode_time_total: f64,
    fps_window: (Instant, u64),
    /// Depth frames kept as received (`depth-raw/`), at most [`RAW_DEPTH_FRAMES`].
    raw_depth_kept: u32,
    /// What the depth frames have seen, for the capture page's coverage map.
    coverage: Coverage,
}

/// Depth frames a session keeps as received when some of their tiles could not be read.
const RAW_DEPTH_FRAMES: u32 = 3;

impl CaptureSession {
    pub fn start(cmd: &StartCapture, source_description: String, folder_hint: &str) -> anyhow::Result<Self> {
        let mut manifest = SessionManifest::new(uuid::Uuid::new_v4().to_string(), orbiters_core::now_utc_iso(), orbiters_core::APP_VERSION);
        manifest.world = cmd.world.clone();
        manifest.consent = cmd.consent.clone();
        manifest.capture.mode = cmd.mode;
        manifest.capture.quality = cmd.quality;
        manifest.capture.source = match &cmd.source {
            orbiters_core::protocol::SourceSpec::Spout { .. } => "spout",
            orbiters_core::protocol::SourceSpec::RawSequence { .. } => "raw_sequence",
            orbiters_core::protocol::SourceSpec::TestPattern { .. } => "test",
        }
        .into();
        manifest.capture.source_name = source_description.clone();
        let base = cmd.folder_name.clone().unwrap_or_else(|| {
            let world = cmd.world.as_ref().and_then(|w| w.name.clone()).unwrap_or_else(|| folder_hint.to_string());
            format!("{world} {}", orbiters_core::now_local_for_names())
        });
        let mut writer = BundleWriter::create(&cmd.sessions_dir, &base, manifest)?;
        if matches!(cmd.source, orbiters_core::protocol::SourceSpec::RawSequence { realtime: false, .. }) {
            writer.use_game_time_duration();
        }
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 6) / 2 + 1;
        let parameters = ParameterStream::new(writer.dir());
        let skeleton = SkeletonStream::new(writer.dir());
        let epoch_ns = clock::now_ns();
        let mut stats = CaptureStats::default();
        let mut audio = Vec::new();
        let p = &cmd.performance;
        let wanted = [
            p.mic.as_ref().map(|id| (TrackKind::Mic, AudioSource::Mic((!id.is_empty()).then(|| id.clone())))),
            p.game_audio_pid.map(|pid| (TrackKind::Game, AudioSource::Process(pid))),
        ];
        for (kind, source) in wanted.into_iter().flatten() {
            match AudioRecorder::start(kind, source, writer.dir(), epoch_ns) {
                Ok(r) => {
                    eprintln!("recording {}", r.source_name);
                    audio.push(r);
                }
                Err(e) => {
                    let what = if kind == TrackKind::Mic { "microphone" } else { "VRChat sound" };
                    eprintln!("{what}: {e:#}");
                    stats.last_error = Some(format!("Could not record the {what}: {e:#}"));
                }
            }
        }
        let tracking = match p.tracking_hz {
            Some(hz) => match TrackingRecorder::start(tracking::steamvr(), writer.dir(), epoch_ns, hz) {
                Ok(r) => Some(r),
                Err(e) => {
                    stats.last_error = Some(format!("Could not record SteamVR tracking: {e:#}"));
                    None
                }
            },
            None => None,
        };
        Ok(Self {
            writer,
            selector: KeyframeSelector::new(cmd.keyframes.clone(), cmd.quality == StorageQuality::Lossless),
            pool: Pool::new(threads),
            stats,
            epoch_ns,
            parameters,
            skeleton,
            audio,
            tracking,
            calibrations: p
                .calibration
                .iter()
                .cloned()
                .map(|mut c| {
                    c.time_ns -= epoch_ns as i64;
                    c
                })
                .collect(),
            checkpointed: Instant::now(),
            calibrations_written: (0, 0),
            frame_index: 0,
            prefer_flipped: false,
            previous: None,
            thumbnail_done: false,
            decode_time_total: 0.0,
            raw_depth_kept: 0,
            coverage: Coverage::new(),
            fps_window: (Instant::now(), 0),
        })
    }

    pub fn dir(&self) -> PathBuf {
        self.writer.dir().to_path_buf()
    }

    pub fn set_world(&mut self, world: Option<orbiters_contracts::manifest::WorldInfo>) {
        self.writer.manifest_mut().world = world;
    }

    /// Nanoseconds since the session started.
    pub fn time_ns(&self) -> u64 {
        clock::now_ns().saturating_sub(self.epoch_ns)
    }

    /// Records OSC values from VRChat (avatar parameters, face tracking, head and wrist poses).
    pub fn osc(&mut self, samples: &[OscSample], paused: bool) {
        for sample in samples {
            if let Err(e) = self.parameters.add(self.epoch_ns, sample, paused) {
                self.stats.last_error = Some(format!("parameters: {e:#}"));
            }
        }
    }

    /// A calibration made now in the overlay (`time_ns` read on the shared clock).
    pub fn add_calibration(&mut self, mut calibration: Calibration) {
        calibration.time_ns -= self.epoch_ns as i64;
        self.calibrations.push(calibration);
    }

    /// While paused, the audio tracks record silence and no poses are written.
    pub fn set_performance_paused(&self, paused: bool) {
        for a in &self.audio {
            a.set_paused(paused);
        }
        if let Some(t) = &self.tracking {
            t.set_paused(paused);
        }
    }

    /// Recording goes on after a pause: writes what changed meanwhile.
    pub fn resume(&mut self) {
        if let Err(e) = self.parameters.resume(self.time_ns()) {
            self.stats.last_error = Some(format!("parameters: {e:#}"));
        }
    }

    /// Processes one frame (called with the source's borrowed view).
    pub fn process(&mut self, view: ImageView<'_>) -> anyhow::Result<()> {
        let t0 = Instant::now();
        let mut record =
            FrameRecord { frame_index: self.frame_index, capture_time_ns: self.time_ns(), width: view.width, height: view.height, ..Default::default() };
        self.frame_index += 1;
        self.stats.frames += 1;
        self.stats.width = view.width;
        self.stats.height = view.height;
        let capture = &mut self.writer.manifest_mut().capture;
        if capture.width == 0 {
            capture.width = view.width;
            capture.height = view.height;
        }

        match strip::decode_preferring(&view, self.prefer_flipped) {
            Err(e) => {
                self.stats.strip_failed += 1;
                self.stats.last_error = Some(e.to_string());
            }
            Ok(d) => {
                self.prefer_flipped = d.flipped;
                self.stats.flipped = d.flipped;
                self.stats.strip_ok += 1;
                let p = d.payload;
                record.flags |= record_flags::STRIP_OK;
                if d.flipped {
                    record.flags |= record_flags::FLIPPED;
                }
                record.game_time = p.time;
                record.strip_flags = p.flags;
                record.camera_to_world = p.camera_to_world;
                record.projection = p.projection;
                record.near = p.near;
                record.far = p.far;
                record.corrected_bits = d.corrected_bits;
                record.patch = d.patch;
                record.min_contrast = d.min_contrast;
                // The capture kit's skeleton band rides on colour frames (depth frames use the whole picture).
                if !p.is_depth_frame() {
                    if let Some(Ok(pose)) = skeleton::decode(&view, d.flipped).map(|b| b.result) {
                        if let Err(e) = self.skeleton.add(record.capture_time_ns, &pose) {
                            self.stats.last_error = Some(format!("skeleton: {e:#}"));
                        }
                    }
                }
                let pos = p.position();
                self.stats.position = Some(pos);
                self.stats.heading_deg = Some(p.camera_to_world[2].atan2(p.camera_to_world[10]).to_degrees());

                // Gating: a repeated frame (same game time) or an impossible jump (teleport, respawn).
                let gated = match &self.previous {
                    Some((bits, prev_pose, prev_time)) if *bits == p.time.to_bits() => true,
                    Some((_, prev_pose, prev_time)) => {
                        let dt = (p.time - prev_time).abs().max(1e-3);
                        let moved = keyframes::translation(&p.camera_to_world, prev_pose);
                        if moved > 3.0 && moved / dt > 40.0 {
                            self.selector.reset();
                            true
                        } else {
                            false
                        }
                    }
                    None => false,
                };
                self.previous = Some((p.time.to_bits(), p.camera_to_world, p.time));
                if gated {
                    record.flags |= record_flags::GATED;
                } else if p.is_depth_frame() {
                    if let Some(dd) = depth::decode(&view, d.flipped) {
                        let (index, path) = self.writer.reserve_depth();
                        let bytes = depth_file::write(dd.grid_width, dd.grid_height, view.width, view.height, &dd.codes);
                        self.pool.send(Job::Depth { path, bytes });
                        record.flags |= record_flags::DEPTH;
                        record.depth_index = index;
                        self.stats.depth_frames += 1;
                        self.coverage.add_depth(&p.camera_to_world, &p.projection, &dd.layout, dd.grid_width, &dd.codes);
                        // A few frames with tiles that could not be read, as received: what the world's
                        // post-processing did to the symbols, for whoever improves the decoder next.
                        if dd.tiles_rejected > 0 && self.raw_depth_kept < RAW_DEPTH_FRAMES {
                            if let Ok(path) = self.writer.raw_depth_path(index) {
                                let data = copy_upright(&view, d.flipped);
                                let bgra = view.order == PixelOrder::Bgra;
                                if self.pool.try_send(Job::Keyframe { path, data, width: view.width, height: view.height, bgra }) {
                                    self.raw_depth_kept += 1;
                                }
                            }
                        }
                    }
                } else if self.selector.consider(&p.camera_to_world, p.time) {
                    let data = copy_upright(&view, d.flipped);
                    let bgra = view.order == PixelOrder::Bgra;
                    if !self.thumbnail_done {
                        let mut rgba = data.clone();
                        if bgra {
                            for px in rgba.as_chunks_mut::<4>().0 {
                                px.swap(0, 2);
                            }
                        }
                        // Leave the pose strip (and its reference patches) out of the picture.
                        let top = strip::StripLayout::new(view.width, view.height).map(|l| l.reserved.y1.min(view.height - 1)).unwrap_or(0);
                        let below = &rgba[top as usize * view.width as usize * 4..];
                        if self.writer.write_thumbnail(below, view.width, view.height - top).is_ok() {
                            self.thumbnail_done = true;
                        }
                    }
                    let (index, path) = self.writer.reserve_keyframe();
                    if self.pool.try_send(Job::Keyframe { path, data, width: view.width, height: view.height, bgra }) {
                        record.flags |= record_flags::KEYFRAME;
                        record.keyframe_index = index;
                        self.stats.keyframes += 1;
                    } else {
                        self.stats.dropped += 1;
                    }
                }
            }
        }
        if record.has(record_flags::GATED) {
            self.stats.gated += 1;
        }
        self.writer.append_record(&record)?;
        self.decode_time_total += t0.elapsed().as_secs_f64();
        Ok(())
    }

    /// The coverage map so far, with hints for where the camera is now.
    pub fn coverage_map(&self) -> Option<CoverageMap> {
        self.coverage.map(self.stats.position, self.stats.heading_deg, self.stats.elapsed_s)
    }

    /// Refreshes rates and byte counts; call before sending stats.
    pub fn refresh_stats(&mut self) -> &CaptureStats {
        let (since, frames_then) = self.fps_window;
        let dt = since.elapsed().as_secs_f32();
        if dt >= 1.0 {
            self.stats.fps = (self.stats.frames - frames_then) as f32 / dt;
            self.fps_window = (Instant::now(), self.stats.frames);
        }
        self.stats.decode_ms = if self.stats.frames > 0 { (self.decode_time_total / self.stats.frames as f64 * 1000.0) as f32 } else { 0.0 };
        self.stats.bytes = self.writer.manifest().stats.bytes + self.pool.written.load(Ordering::Relaxed) + self.parameters.changes * 16;
        self.stats.elapsed_s = self.time_ns() as f64 / 1e9;
        self.stats.parameter_changes = self.parameters.changes;
        self.stats.skeleton_frames = self.skeleton.frames;
        if let Err(e) = self.skeleton.flush() {
            self.stats.last_error = Some(format!("skeleton: {e:#}"));
        }
        self.stats.mic_level = self.audio.iter().find(|a| a.kind == TrackKind::Mic).map(AudioRecorder::take_level);
        self.stats.game_level = self.audio.iter().find(|a| a.kind == TrackKind::Game).map(AudioRecorder::take_level);
        if let Some(t) = &self.tracking {
            let c = t.counts();
            self.stats.tracking_connected = Some(c.connected);
            (self.stats.tracking_devices, self.stats.tracking_samples, self.stats.calibrations) = (c.devices, c.samples, c.calibrations);
            self.stats.bytes += c.samples * 64;
        }
        if let Err(e) = self.parameters.flush() {
            self.stats.last_error = Some(format!("parameters: {e:#}"));
        }
        if self.checkpointed.elapsed() >= bundle::CHECKPOINT_EVERY {
            if let Err(e) = self.checkpoint() {
                self.stats.last_error = Some(format!("saving the session so far: {e:#}"));
            }
        }
        &self.stats
    }

    /// Writes what the session holds so far, every few seconds: the streams' tables (`parameters.json`,
    /// `audio.json` as the WAVE headers stand, `calibration.json`; `tracking.json` is kept by its thread), the
    /// manifest with its performance block, and the frame records. A session that never finishes (the worker
    /// killed or crashed, Windows shut down) stays readable up to here.
    fn checkpoint(&mut self) -> anyhow::Result<()> {
        self.checkpointed = Instant::now();
        self.parameters.flush()?;
        self.skeleton.flush()?;
        let tracks: Vec<_> = self.audio.iter().filter_map(AudioRecorder::track).collect();
        if !tracks.is_empty() {
            let mut index = perf::audio::Audio::new();
            index.tracks = tracks.clone();
            orbiters_core::write_atomic(&self.writer.dir().join(perf::AUDIO_FILE), &serde_json::to_vec_pretty(&index)?)?;
        }
        let (devices, detected) = self.tracking.as_ref().map(TrackingRecorder::found).unwrap_or_default();
        let calibrations = session_calibrations(&self.calibrations, detected, &devices);
        let written = (calibrations.len(), devices.len());
        if written != self.calibrations_written {
            tracking::write_calibrations(self.writer.dir(), calibrations)?;
            self.calibrations_written = written;
        }
        let performance = &mut self.writer.manifest_mut().performance;
        performance.parameter_changes = self.parameters.changes;
        performance.skeleton_frames = self.skeleton.frames;
        performance.tracking_samples = self.tracking.as_ref().map_or(0, |t| t.counts().samples);
        performance.calibrations = written.0 as u32;
        performance.audio = tracks.into_iter().map(|t| t.file).collect();
        self.writer.checkpoint()
    }

    /// Waits for pending files and writes the final manifest.
    pub fn finish(self) -> anyhow::Result<(PathBuf, SessionManifest)> {
        let Self { mut writer, pool, parameters, skeleton, audio, tracking, calibrations, .. } = self;
        let (written, errors) = pool.finish();
        writer.add_bytes(written);
        let (changes, bytes) = parameters.finish()?;
        writer.manifest_mut().performance.parameter_changes = changes;
        writer.add_bytes(bytes);
        let (frames, bytes) = skeleton.finish()?;
        writer.manifest_mut().performance.skeleton_frames = frames;
        writer.add_bytes(bytes);
        let mut tracks = perf::audio::Audio::new();
        for recorder in audio {
            match recorder.finish() {
                Ok(track) => {
                    writer.add_bytes(58 + track.frames * track.channels as u64 * 4);
                    writer.manifest_mut().performance.audio.push(track.file.clone());
                    tracks.tracks.push(track);
                }
                Err(e) => eprintln!("audio: {e:#}"),
            }
        }
        if !tracks.tracks.is_empty() {
            orbiters_core::write_atomic(&writer.dir().join(perf::AUDIO_FILE), &serde_json::to_vec_pretty(&tracks)?)?;
        }
        let (mut devices, mut detected) = (Vec::new(), Vec::new());
        if let Some(t) = tracking {
            match t.finish() {
                Ok(r) => {
                    eprintln!("tracking: {} devices, {} samples, {} T-poses", r.devices.len(), r.samples, r.calibrations.len());
                    writer.manifest_mut().performance.tracking_samples = r.samples;
                    writer.add_bytes(perf::HEADER_SIZE as u64 * u64::from(r.samples > 0) + r.samples * 64);
                    detected = r.calibrations;
                    devices = r.devices;
                }
                Err(e) => eprintln!("tracking: {e:#}"),
            }
        }
        let calibrations = session_calibrations(&calibrations, detected, &devices);
        writer.manifest_mut().performance.calibrations = calibrations.len() as u32;
        let bytes = tracking::write_calibrations(writer.dir(), calibrations)?;
        writer.add_bytes(bytes);
        if errors > 0 {
            eprintln!("{errors} file(s) could not be written");
        }
        writer.finish()
    }
}

/// A session's calibrations: those from the overlay (they name devices by serial: numbered like this session's
/// `tracking.json`) and the T-poses caught while recording, in time order.
fn session_calibrations(overlay: &[Calibration], detected: Vec<Calibration>, devices: &[Device]) -> Vec<Calibration> {
    let mut all: Vec<Calibration> = overlay.iter().cloned().chain(detected).collect();
    for c in &mut all {
        orbiters_core::pose::renumber(c, devices);
    }
    all.sort_by_key(|c| c.time_ns);
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{FrameSource, Next, TestPatternSource};
    use orbiters_contracts::manifest::{CaptureMode, ConsentInfo};
    use orbiters_contracts::performance::skeleton_frames;
    use orbiters_core::protocol::SourceSpec;
    use orbiters_core::settings::KeyframeSettings;

    #[test]
    fn a_session_that_never_finishes_reads_up_to_its_last_checkpoint() {
        let root = std::env::temp_dir().join(format!("orbiters-capture-unfinished-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // A T-pose from the overlay a second before the session.
        let calibration =
            Calibration { time_ns: clock::now_ns() as i64 - 1_000_000_000, pose: "t_pose".into(), source: "overlay".into(), ..Default::default() };
        let cmd = StartCapture {
            source: SourceSpec::TestPattern { width: 1280, height: 720, fps: 30.0, depth_every: 4, frames: 20 },
            sessions_dir: root.clone(),
            mode: CaptureMode::RecordOnly,
            quality: StorageQuality::High,
            keyframes: KeyframeSettings::default(),
            world: None,
            consent: ConsentInfo::default(),
            folder_name: Some("unfinished".into()),
            performance: orbiters_core::protocol::PerformanceSpec { calibration: Some(calibration), ..Default::default() },
        };
        let mut src = TestPatternSource::new(1280, 720, 30.0, 4, 20, false);
        let mut session = CaptureSession::start(&cmd, src.describe(), &src.folder_hint()).unwrap();
        let viseme = OscSample {
            at_ns: clock::now_ns(),
            address: "/avatar/parameters/Viseme".into(),
            index: None,
            value: orbiters_core::osc::OscValue::Int(3),
            initial: false,
        };
        session.osc(&[viseme], false);
        while !matches!(src.next(std::time::Duration::from_millis(10), &mut |v| session.process(v).unwrap()), Ok(Next::End)) {}
        // Five seconds on: the stats refresh writes a checkpoint. Then the worker is killed: no finish.
        session.checkpointed = Instant::now().checked_sub(bundle::CHECKPOINT_EVERY).unwrap();
        session.refresh_stats();
        assert_eq!(session.stats.last_error, None);
        let dir = session.dir();
        let manifest = bundle::read_manifest(&dir).unwrap();
        assert_eq!(manifest.stats.frames, 20);
        assert_eq!(bundle::read_records(&dir).unwrap().len(), 20);
        let p = &manifest.performance;
        assert_eq!((p.parameter_changes, p.skeleton_frames, p.calibrations), (1, 15, 1));
        let table: perf::parameters::Table = serde_json::from_slice(&std::fs::read(dir.join(perf::PARAMETERS_TABLE)).unwrap()).unwrap();
        assert_eq!(table.parameters[0].name, "Viseme");
        let calibrations: perf::calibration::Calibrations = serde_json::from_slice(&std::fs::read(dir.join(perf::CALIBRATION_FILE)).unwrap()).unwrap();
        assert_eq!(calibrations.calibrations.len(), 1);
        assert!(calibrations.calibrations[0].time_ns < 0, "made before the session");
        drop(session);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn test_pattern_session_end_to_end() {
        let root = std::env::temp_dir().join(format!("orbiters-capture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cmd = StartCapture {
            source: SourceSpec::TestPattern { width: 1280, height: 720, fps: 30.0, depth_every: 4, frames: 60 },
            sessions_dir: root.clone(),
            mode: CaptureMode::RecordOnly,
            quality: StorageQuality::High,
            keyframes: KeyframeSettings::default(),
            world: None,
            consent: ConsentInfo::default(),
            folder_name: Some("pattern".into()),
            performance: Default::default(),
        };
        let mut src = TestPatternSource::new(1280, 720, 30.0, 4, 60, false);
        let mut session = CaptureSession::start(&cmd, src.describe(), &src.folder_hint()).unwrap();
        loop {
            let mut err = None;
            match src.next(std::time::Duration::from_millis(10), &mut |v| {
                if let Err(e) = session.process(v) {
                    err = Some(e);
                }
            }) {
                Ok(Next::Frame) => assert!(err.is_none()),
                Ok(Next::End) => break,
                Ok(Next::Timeout) => {}
                Err(e) => panic!("{e}"),
            }
        }
        let (dir, manifest) = session.finish().unwrap();
        assert_eq!(manifest.stats.frames, 60);
        assert_eq!(manifest.stats.strip_ok, 60);
        assert_eq!(manifest.stats.depth_frames, 15);
        assert!(manifest.stats.keyframes >= 5, "keyframes {}", manifest.stats.keyframes);
        let records = orbiters_core::bundle::read_records(&dir).unwrap();
        assert_eq!(records.len(), 60);
        let k = records.iter().find(|r| r.has(record_flags::KEYFRAME)).unwrap();
        let kf = dir.join("keyframes").join(orbiters_core::bundle::keyframe_file(k.keyframe_index));
        let (h, _) = qoi::decode_rgba(&std::fs::read(kf).unwrap()).unwrap();
        assert_eq!((h.width, h.height, h.channels), (1280, 720, 3));
        let d = records.iter().find(|r| r.has(record_flags::DEPTH)).unwrap();
        let df = std::fs::read(dir.join("depth").join(orbiters_core::bundle::depth_file(d.depth_index))).unwrap();
        let (gw, gh, fw, _, codes) = depth_file::read(&df).unwrap();
        assert_eq!(fw, 1280);
        assert_eq!(codes.len() as u32, gw * gh);
        assert!(codes.iter().filter(|&&c| c != 0).count() > 1000);
        assert!(dir.join("thumbnail.qoi").is_file());
        // The test pattern draws the capture kit's skeleton band on its colour frames.
        assert_eq!(manifest.performance.skeleton_frames, 45);
        let frames = skeleton_frames::read(&std::fs::read(dir.join(orbiters_contracts::performance::SKELETON_FILE)).unwrap()).unwrap();
        assert_eq!(frames.len(), 45);
        let colour: Vec<_> = records.iter().filter(|r| !r.has(record_flags::DEPTH)).collect();
        for (frame, record) in frames.iter().zip(&colour) {
            let want = TestPatternSource::skeleton(frame.game_time);
            assert_eq!(frame.time_ns, record.capture_time_ns);
            assert_eq!((frame.hips, frame.avatar, frame.mask()), (want.hips, want.avatar, want.mask()));
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
