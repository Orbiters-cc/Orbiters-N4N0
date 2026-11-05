//! One capture session: decodes every frame's strip (and the skeleton band of colour frames), chooses keyframes,
//! and writes the session bundle. Keyframes are encoded, and depth frames decoded, on threads of their own, so
//! capture never waits on the disk, compression or the depth decoder: when they fall behind, keyframes and depth
//! frames are dropped (and counted) instead. A frame's record reaches `frames.bin` once the files it names are
//! written, in frame order: a session cut short (a full disk, a crash) never names a keyframe that is not there.

use crate::audio::{AudioRecorder, AudioSource};
use crate::coverage::Coverage;
use crate::performance::{ParameterStream, SkeletonStream};
use crate::sources::FrameAccess;
use crate::tracking::{self, TrackingRecorder};
use crossbeam_channel::{bounded, unbounded, Receiver as ChanReceiver, Sender as ChanSender};
use orbiters_contracts::frame_record::{record_flags, FrameRecord};
use orbiters_contracts::image::{ImageView, PixelOrder};
use orbiters_contracts::manifest::{self as bundle_files, depth_file, SessionManifest, StorageQuality};
use orbiters_contracts::performance::calibration::Calibration;
use orbiters_contracts::performance::tracking::Device;
use orbiters_contracts::performance::{self as perf, audio::TrackKind};
use orbiters_contracts::{depth, qoi, rig, skeleton, strip};
use orbiters_core::bundle::{self, BundleWriter};
use orbiters_core::clock;
use orbiters_core::keyframes::{self, KeyframeSelector};
use orbiters_core::protocol::{CaptureStats, CoverageMap, OscSample, StartCapture};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Something the person should know about a recording that goes on, by what it concerns: the worker tells each kind
/// once, and again when its message changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Warning {
    Microphone,
    GameSound,
    Tracking,
    /// The avatar parameters' and the skeleton band's streams.
    Streams,
    /// Keyframes and depth frames that could not be written, or were dropped.
    Pictures,
    /// The session's files as a whole: its checkpoints, `frames.bin`, the free space left.
    Disk,
    /// The frame source (a Spout sender that cannot be opened, frames of a raw capture that cannot be read).
    Source,
}

/// A session's rig faces so far, when not one of them could be read: what to tell the person. A face camera that does not project as
/// its frame's strip says draws no stamp (contracts' Docs/rig-frames.md, for an upload whose cameras' physical properties did not reach
/// VRChat), and a 3D picture without its faces loses what it shows away from the camera's path, 5.9 dB on views of the rig
/// (reports/rig-pixels.md). Thirty faces is five rig frames: a few late cameras at the start of the rig are normal.
fn rig_faces_unread(read: u64, late: u64) -> Option<&'static str> {
    (read == 0 && late >= 30).then_some("N4N0's rig pictures cannot be read in this recording, so the 3D picture will look worse from anywhere but the camera's path. Upload the avatar again; if this stays, the cameras of its rig are not working in VRChat.")
}

/// A stretch without the strip shorter than this is a moment's trouble (a fast turn, a flash): not counted.
const STRIP_GAP_NS: u64 = 1_000_000_000;

/// A picture for the encoders: a keyframe (its frame's record waits for it), or a depth frame kept as received.
struct Picture {
    /// The record waiting for it; `None` for `depth-raw/` frames, which no record names.
    frame: Option<u64>,
    path: PathBuf,
    /// Upright and tightly packed, in the source's channel order.
    data: Vec<u8>,
    width: u32,
    height: u32,
    bgra: bool,
    /// The session's thumbnail, made from this picture below the strip (rows from `.1` down).
    thumbnail: Option<(PathBuf, u32)>,
}

/// A rig frame's face for a record of its own: read with its frame, its picture sent to the encoders once its record is
/// queued (a picture finished before its record would have nothing to tell).
struct FaceView {
    record: FrameRecord,
    picture: Option<Picture>,
}

/// An extra stream's state (a camera dolly's stream): the last pose read (game time bits, pose, time) and its keyframes'
/// choice, as the main stream's.
struct ExtraStream {
    previous: Option<(u32, [f32; 12], f32)>,
    selector: KeyframeSelector,
}

/// A depth frame for the decoder, upright and tightly packed.
struct DepthJob {
    frame: u64,
    index: i32,
    path: PathBuf,
    data: Vec<u8>,
    width: u32,
    height: u32,
    order: PixelOrder,
}

/// A depth frame decoded and written.
struct Decoded {
    layout: depth::DepthLayout,
    grid_width: u32,
    codes: Vec<u16>,
    tiles_rejected: u32,
    bytes: u64,
}

/// What a thread finished, with the frame buffer it had (back for the next copy).
enum Done {
    Picture { frame: Option<u64>, written: Result<u64, String>, thumbnail_failed: bool, buffer: Vec<u8> },
    Depth { frame: u64, index: i32, decoded: Result<Option<Decoded>, String>, buffer: Vec<u8>, width: u32, height: u32, bgra: bool },
}

/// Threads taking jobs from a bounded queue.
struct Workers<J> {
    tx: Option<ChanSender<J>>,
    threads: Vec<JoinHandle<()>>,
}

impl<J: Send + 'static> Workers<J> {
    /// `threads` threads named `name-<i>`, each running the job function `make` gives it (its own state: the depth
    /// decoder learns how each tile reads), with room for `queue` jobs waiting.
    fn spawn(name: &str, threads: usize, queue: usize, make: impl Fn() -> Box<dyn FnMut(J) + Send>) -> Self {
        let (tx, rx) = bounded::<J>(queue);
        let threads = (0..threads)
            .map(|i| {
                let (rx, mut work) = (rx.clone(), make());
                std::thread::Builder::new()
                    .name(format!("{name}-{i}"))
                    .spawn(move || {
                        for job in rx {
                            work(job);
                        }
                    })
                    .expect("spawn a capture thread")
            })
            .collect();
        Self { tx: Some(tx), threads }
    }

    /// Whether a job would be taken now: looked at before a frame is copied for one.
    fn has_room(&self) -> bool {
        self.tx.as_ref().is_some_and(|tx| !tx.is_full())
    }

    /// Every thread ended while jobs could still come (a panic): nothing will be taken.
    fn dead(&self) -> bool {
        self.tx.is_some() && self.threads.iter().all(|t| t.is_finished())
    }

    /// Queues a job; it comes back when it could not be (no room after all, the threads gone).
    fn send(&self, job: J) -> Result<(), J> {
        match &self.tx {
            Some(tx) => tx.try_send(job).map_err(|e| e.into_inner()),
            None => Err(job),
        }
    }

    /// No more jobs: waits for the threads to finish theirs.
    fn finish(&mut self) {
        self.tx = None;
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// Files written whole by a thread of their own (the checkpoints' `manifest.json`, `audio.json`, `calibration.json`),
/// each through a temporary file flushed to the disk and a rename ([`orbiters_core::write_atomic`], which also waits
/// out a moment's lock on the file): the capture thread hands them over and goes on. A file handed over again before
/// it was written is written once, as it was last.
struct Saver {
    shared: Arc<(Mutex<SaverState>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct SaverState {
    /// Files waiting, the latest of each.
    waiting: Vec<(PathBuf, Vec<u8>)>,
    /// One is being written.
    busy: bool,
    /// Files written, and why others could not be, since the session last looked.
    written: u64,
    errors: Vec<String>,
    closed: bool,
}

impl Saver {
    fn spawn() -> Self {
        let shared = Arc::new((Mutex::new(SaverState::default()), Condvar::new()));
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new().name("save".into()).spawn(move || Self::run(&shared)).expect("spawn the session's saver")
        };
        Self { shared, thread: Some(thread) }
    }

    fn run(shared: &(Mutex<SaverState>, Condvar)) {
        let (lock, wake) = shared;
        let mut state = lock.lock().unwrap();
        loop {
            if !state.waiting.is_empty() {
                let (path, bytes) = state.waiting.remove(0);
                state.busy = true;
                drop(state);
                let result = orbiters_core::write_atomic(&path, &bytes);
                state = lock.lock().unwrap();
                state.busy = false;
                match result {
                    Ok(()) => state.written += 1,
                    Err(e) => state.errors.push(format!("{}: {e}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default())),
                }
                wake.notify_all();
                continue;
            }
            if state.closed {
                return;
            }
            state = wake.wait(state).unwrap();
        }
    }

    /// Hands a file over (a version of it still waiting is replaced).
    fn save(&self, path: PathBuf, bytes: Vec<u8>) {
        let (lock, wake) = &*self.shared;
        let mut state = lock.lock().unwrap();
        match state.waiting.iter_mut().find(|(p, _)| *p == path) {
            Some(waiting) => waiting.1 = bytes,
            None => state.waiting.push((path, bytes)),
        }
        wake.notify_all();
    }

    /// Files written since the last call, and why others could not be.
    fn outcome(&self) -> (u64, Vec<String>) {
        let mut state = self.shared.0.lock().unwrap();
        (std::mem::take(&mut state.written), std::mem::take(&mut state.errors))
    }

    /// Writes what waits, and ends the thread.
    fn finish(&mut self) {
        self.shared.0.lock().unwrap().closed = true;
        self.shared.1.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// Nothing waits, nothing is being written.
    #[cfg(test)]
    fn idle(&self) -> bool {
        let state = self.shared.0.lock().unwrap();
        state.waiting.is_empty() && !state.busy
    }
}

impl Drop for Saver {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Writes a file through a temporary one renamed into place: a reader, or a crash, finds all of it or nothing. The
/// rename waits out a moment's lock (an antivirus or an indexer looking at the new file).
fn write_whole(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let written = std::fs::write(&tmp, bytes).and_then(|()| orbiters_core::rename_retrying(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Threads one picture is encoded on (the same file as on one): a keyframe's buffer and its record wait less.
const ENCODE_THREADS: usize = 2;

/// Encodes and writes a picture (and the session's thumbnail when it carries one).
fn encode(job: Picture, done: &ChanSender<Done>) {
    let bytes = qoi::encode_bgra_strided_parallel(&job.data, job.width, job.height, job.width as usize * 4, job.bgra, true, ENCODE_THREADS);
    let written = write_whole(&job.path, &bytes).map(|()| bytes.len() as u64).map_err(|e| {
        eprintln!("write {}: {e}", job.path.display());
        e.to_string()
    });
    let thumbnail_failed = job.thumbnail.as_ref().is_some_and(|(path, top)| {
        let row = job.width as usize * 4;
        let bottom = bundle::capture_picture_rect(job.width, job.height).y1.max(*top + 1).min(job.height);
        let mut rgba = job.data[(*top as usize * row).min(job.data.len())..bottom as usize * row].to_vec();
        if job.bgra {
            for px in rgba.as_chunks_mut::<4>().0 {
                px.swap(0, 2);
            }
        }
        let (tw, th, small) = bundle::downscale(&rgba, job.width, (rgba.len() / row.max(1)) as u32, 480);
        let bytes = qoi::encode(&small, tw, th, 3);
        write_whole(path, &bytes).and_then(|()| write_whole(&path.with_file_name(bundle::PICTURE_THUMBNAIL_FILE), &bytes)).is_err()
    });
    let _ = done.send(Done::Picture { frame: job.frame, written, thumbnail_failed, buffer: job.data });
}

/// Decodes a depth frame and writes its codes. `None` when the frame is too small for depth symbols.
fn decode_depth(decoder: &mut depth::Decoder, job: &DepthJob) -> Result<Option<Decoded>, String> {
    let view = ImageView::try_new(job.width, job.height, job.width as usize * 4, &job.data, job.order).map_err(|e| e.to_string())?;
    // The copy is upright: read as it is.
    let Some(d) = decoder.decode(&view, false) else { return Ok(None) };
    let bytes = depth_file::write(d.grid_width, d.grid_height, job.width, job.height, &d.codes);
    write_whole(&job.path, &bytes).map_err(|e| {
        eprintln!("write {}: {e}", job.path.display());
        e.to_string()
    })?;
    Ok(Some(Decoded { layout: d.layout, grid_width: d.grid_width, codes: d.codes, tiles_rejected: d.tiles_rejected, bytes: bytes.len() as u64 }))
}

/// Frame-sized buffers the threads hand back, for the next copies (a fresh 8–33 MB buffer per keyframe costs its page
/// faults on every frame copied), and the bytes out with the threads: frames copied for them stay under a budget
/// whatever the picture's size (an 8K frame is 133 MB).
#[derive(Default)]
struct Buffers {
    free: Vec<Vec<u8>>,
    out: usize,
}

impl Buffers {
    /// Bytes of frames the threads may hold at once.
    const OUT_MOST: usize = 768 << 20;
    /// Bytes of buffers kept for the next copies.
    const KEEP_MOST: usize = 256 << 20;

    /// Whether a frame of `bytes` may go to a thread now.
    fn room(&self, bytes: usize) -> bool {
        self.out + bytes <= Self::OUT_MOST
    }

    fn take(&mut self) -> Vec<u8> {
        self.free.pop().unwrap_or_default()
    }

    /// A buffer of `bytes` went to a thread.
    fn lent(&mut self, bytes: usize) {
        self.out += bytes;
    }

    /// A lent buffer back from its thread.
    fn give(&mut self, buffer: Vec<u8>) {
        self.out = self.out.saturating_sub(buffer.len());
        self.keep(buffer);
    }

    /// A buffer for the next copies, if there is room for it.
    fn keep(&mut self, buffer: Vec<u8>) {
        let kept: usize = self.free.iter().map(Vec::capacity).sum();
        if kept + buffer.capacity() <= Self::KEEP_MOST {
            self.free.push(buffer);
        }
    }
}

/// Copies the part `tile` of a frame (top-down, as it shows upright) into `out`, tightly packed and upright.
fn copy_tile_upright(view: &ImageView, flipped: bool, tile: orbiters_contracts::image::Rect, out: &mut Vec<u8>) {
    let row = (tile.x1 - tile.x0) as usize * 4;
    out.clear();
    out.reserve(row * (tile.y1 - tile.y0) as usize);
    for y in tile.y0 as usize..tile.y1 as usize {
        let src = if flipped { view.height as usize - 1 - y } else { y };
        let at = src * view.stride + tile.x0 as usize * 4;
        out.extend_from_slice(&view.data[at..at + row]);
    }
}

/// Copies a frame into `out`, tightly packed and upright (4 bytes per pixel, the source's channel order).
fn copy_upright(view: &ImageView, flipped: bool, out: &mut Vec<u8>) {
    let row = view.width as usize * 4;
    out.clear();
    out.reserve(row * view.height as usize);
    for y in 0..view.height as usize {
        let src = if flipped { view.height as usize - 1 - y } else { y };
        out.extend_from_slice(&view.data[src * view.stride..src * view.stride + row]);
    }
}

/// A frame's record waiting for its files (its keyframe, its depth frame) before it goes to `frames.bin`.
struct Pending {
    record: FrameRecord,
    waiting: u8,
    since: Instant,
}

/// How long a record waits for its files (a disk that hangs, a thread that died) before it goes without them.
const RECORD_PATIENCE: Duration = Duration::from_secs(20);
/// How long failures must keep coming, with nothing written meanwhile, before the recording stops; failures closer
/// than `FAILURE_RUN` belong to one run (a checkpoint every few seconds keeps a full disk's run going).
const WRITE_PATIENCE: Duration = Duration::from_secs(3);
const FAILURE_RUN: Duration = Duration::from_secs(6);

/// Writing failures in a row: since when, the last one, how many, and the last error.
struct Failing {
    since: Instant,
    last: Instant,
    count: u32,
    error: String,
}

impl Failing {
    /// Failures that keep coming, not one now and then: nothing gets written any more.
    fn persistent(&self) -> bool {
        self.count >= 3 && self.since.elapsed() >= WRITE_PATIENCE && self.last.elapsed() < Duration::from_secs(1)
    }
}
/// Free space under which the recording stops, and under which it is told it soon will.
const FREE_STOP: u64 = 512 << 20;
const FREE_WARN: u64 = 2 << 30;
/// How often the session so far reaches the disk ([`CaptureSession::checkpoint`]).
const CHECKPOINT_EVERY: Duration = Duration::from_millis(4500);
/// `audio.json` grows by a sync pair a second per track: written this often while recording.
const AUDIO_INDEX_EVERY: Duration = Duration::from_secs(30);

/// The record of frame `frame`, while it waits.
fn pending_at(pending: &mut VecDeque<Pending>, frame: u64) -> Option<&mut Pending> {
    let first = pending.front()?.record.frame_index;
    pending.get_mut(frame.checked_sub(first)? as usize)
}

/// Where the decoders read, for frames of one size: the strip's rows and the skeleton band's (with the reach of the
/// band decoder's shift search), for either orientation as stored.
struct Areas {
    width: u32,
    height: u32,
    strip: Option<strip::StripLayout>,
    /// Upright, flipped: the strip's rows and the band's, read together.
    rows: [Vec<(u32, u32)>; 2],
    band: [Option<(u32, u32)>; 2],
}

impl Areas {
    fn new(width: u32, height: u32) -> Self {
        let strip = strip::StripLayout::new(width, height);
        let flip = |(a, b): (u32, u32)| (height.saturating_sub(b), height.saturating_sub(a));
        let strip_rows = strip.map(|l| (l.reserved.y0, l.reserved.y1.min(height)));
        let band = skeleton::BandLayout::new(width, height).map(|b| (b.reserved().y0.saturating_sub(3 * b.cell + 2), height));
        let up: Vec<(u32, u32)> = strip_rows.into_iter().chain(band).collect();
        let down = up.iter().map(|&r| flip(r)).collect();
        Self { width, height, strip, rows: [up, down], band: [band, band.map(flip)] }
    }
}

/// Free bytes on the disk holding `dir`.
#[cfg(windows)]
fn free_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, available: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let mut available = 0u64;
    // SAFETY: a NUL-terminated path and an out-parameter; the totals are not asked for.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(available)
}

#[cfg(not(windows))]
fn free_space(_dir: &Path) -> Option<u64> {
    None
}

/// What [`CaptureSession::process`] made of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Processed {
    Frame,
    /// The frame is of another size than the session's (the Stream camera's resolution changed): it was left for a
    /// new session, since a bundle holds frames of one size.
    SizeChanged {
        width: u32,
        height: u32,
    },
}

pub struct CaptureSession {
    writer: BundleWriter,
    selector: KeyframeSelector,
    /// The rig's faces each have keyframes of their own, chosen as the frames' are (by face number - 1).
    face_selectors: [KeyframeSelector; rig::FACES],
    /// The faces of the frame being read, until its record is queued.
    faces_read: Vec<FaceView>,
    /// Keyframes (and `depth-raw/` frames) to encode and write; depth frames to decode and write; what they finished.
    encoder: Workers<Picture>,
    depth: Workers<DepthJob>,
    done: ChanReceiver<Done>,
    buffers: Buffers,
    /// The checkpoints' files, written on a thread of their own.
    saver: Saver,
    /// Records waiting for their files, in frame order.
    pending: VecDeque<Pending>,
    band: skeleton::Decoder,
    /// The hips the last band gave (world position, rotation), until taken (`take_hips`).
    hips: Option<([f32; 3], [f32; 4])>,
    areas: Option<Areas>,
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
    /// When `audio.json` was last written while recording.
    audio_indexed: Option<Instant>,
    frame_index: u64,
    prefer_flipped: bool,
    previous: Option<(u32, [f32; 12], f32)>,
    /// The extra streams' own (the camera dolly's other cameras, [`FrameAccess::stream`] 1 and on): their last pose,
    /// for gating, and their keyframe choice. The main stream's are `previous` and `selector`.
    extra: Vec<ExtraStream>,
    thumbnail_done: bool,
    decode_time_total: f64,
    fps_window: (Instant, u64),
    /// Depth frames kept as received (`depth-raw/`), at most [`RAW_DEPTH_FRAMES`].
    raw_depth_kept: u32,
    /// What the depth frames have seen, for the capture page's coverage map.
    coverage: Coverage,
    /// Bytes of the pictures written, and the files that could not be.
    written: u64,
    write_errors: u64,
    /// Writing that has been failing, with nothing written meanwhile.
    failing: Option<Failing>,
    /// When the free space was last looked at, and whether running low was told.
    free_checked: Option<Instant>,
    free_warned: bool,
    /// Warnings not yet handed to the worker, and the latest of each kind (the stats show them).
    warnings: Vec<(Warning, String)>,
    latest: Vec<(Warning, String)>,
    /// Why the recording must stop (the disk is full).
    stop: Option<String>,
    /// When a frame's strip was last read and since when it has not been, on the session clock; the time without it, in
    /// stretches of at least [`STRIP_GAP_NS`], once it was seen; the latest frame's time.
    strip_first_ns: Option<u64>,
    strip_seen_ns: Option<u64>,
    strip_lost_ns: Option<u64>,
    strip_missing_ns: u64,
    last_frame_ns: u64,
    /// The worker paused the recording (a game track started meanwhile starts paused).
    performance_paused: bool,
    /// The source does not run on its own (a raw capture imported as fast as it reads, a test pattern): the threads
    /// are waited for, rather than frames dropped.
    offline: bool,
}

/// Numbers JSON can carry, in the stats sent four times a second: serde writes NaN and infinities as `null`, which
/// costs the reader the field (and, strictly read, the whole event).
fn finite_stats(s: &mut CaptureStats) {
    let finite = |v: f32| if v.is_finite() { v } else { 0.0 };
    (s.fps, s.decode_ms) = (finite(s.fps), finite(s.decode_ms));
    if !s.elapsed_s.is_finite() {
        s.elapsed_s = 0.0;
    }
    if s.position.is_some_and(|p| p.iter().any(|v| !v.is_finite())) {
        s.position = None;
    }
    if s.heading_deg.is_some_and(|h| !h.is_finite()) {
        s.heading_deg = None;
    }
    (s.mic_level, s.game_level) = (s.mic_level.map(finite), s.game_level.map(finite));
}

/// The same for a finished session's manifest (the `Stopped` event carries it).
fn finite_manifest(m: &mut SessionManifest) {
    let s = &mut m.stats;
    if !s.duration_s.is_finite() {
        s.duration_s = 0.0;
    }
    s.first_game_time = s.first_game_time.filter(|t| t.is_finite());
    s.last_game_time = s.last_game_time.filter(|t| t.is_finite());
}

/// The warning kind and the words for an audio track.
fn audio_warning(kind: TrackKind) -> (Warning, &'static str) {
    match kind {
        TrackKind::Mic => (Warning::Microphone, "microphone"),
        TrackKind::Game => (Warning::GameSound, "VRChat sound"),
    }
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
            orbiters_core::protocol::SourceSpec::Spout { extra, .. } if !extra.is_empty() => "spout_streams",
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
        let (done_tx, done) = unbounded::<Done>();
        let encoder = {
            let done = done_tx.clone();
            // Room for a rig frame's faces besides the frames' keyframes.
            Workers::spawn("encode", threads, threads * 2 + 2 * rig::FACES, move || {
                let done = done.clone();
                Box::new(move |job: Picture| encode(job, &done))
            })
        };
        // One thread decodes depth frames in order, with a decoder that learns how each tile reads (it splits a frame
        // over the cores itself).
        let depth = Workers::spawn("depth", 1, 3, move || {
            let (done, mut decoder) = (done_tx.clone(), depth::Decoder::new());
            Box::new(move |job: DepthJob| {
                let decoded = decode_depth(&mut decoder, &job);
                let DepthJob { frame, index, data, width, height, order, .. } = job;
                let _ = done.send(Done::Depth { frame, index, decoded, buffer: data, width, height, bgra: order == PixelOrder::Bgra });
            })
        });
        let parameters = ParameterStream::new(writer.dir());
        let skeleton = SkeletonStream::new(writer.dir());
        let epoch_ns = clock::now_ns();
        let mut warnings = Vec::new();
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
                    let (warning, what) = audio_warning(kind);
                    eprintln!("{what}: {e:#}");
                    warnings.push((warning, format!("The {what} is not recorded: {e:#}")));
                }
            }
        }
        let tracking = match p.tracking_hz {
            Some(hz) => match TrackingRecorder::start(tracking::steamvr(), writer.dir(), epoch_ns, hz) {
                Ok(r) => Some(r),
                Err(e) => {
                    warnings.push((Warning::Tracking, format!("SteamVR tracking is not recorded: {e:#}")));
                    None
                }
            },
            None => None,
        };
        let mut session = Self {
            writer,
            selector: KeyframeSelector::new(cmd.keyframes.clone(), cmd.quality == StorageQuality::Lossless),
            face_selectors: std::array::from_fn(|_| KeyframeSelector::new(cmd.keyframes.clone(), cmd.quality == StorageQuality::Lossless)),
            faces_read: Vec::new(),
            encoder,
            depth,
            done,
            buffers: Buffers::default(),
            saver: Saver::spawn(),
            pending: VecDeque::new(),
            band: skeleton::Decoder::new(),
            hips: None,
            areas: None,
            stats: CaptureStats::default(),
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
            audio_indexed: None,
            frame_index: 0,
            prefer_flipped: false,
            previous: None,
            extra: Vec::new(),
            thumbnail_done: false,
            decode_time_total: 0.0,
            raw_depth_kept: 0,
            coverage: Coverage::new(),
            fps_window: (Instant::now(), 0),
            written: 0,
            write_errors: 0,
            failing: None,
            free_checked: None,
            free_warned: false,
            warnings: Vec::new(),
            latest: Vec::new(),
            stop: None,
            strip_first_ns: None,
            strip_seen_ns: None,
            strip_lost_ns: None,
            strip_missing_ns: 0,
            last_frame_ns: 0,
            performance_paused: false,
            offline: matches!(
                cmd.source,
                orbiters_core::protocol::SourceSpec::RawSequence { realtime: false, .. } | orbiters_core::protocol::SourceSpec::TestPattern { .. }
            ),
        };
        for (kind, message) in warnings {
            session.warn(kind, message);
        }
        session.check_free_space();
        Ok(session)
    }

    pub fn dir(&self) -> PathBuf {
        self.writer.dir().to_path_buf()
    }

    pub fn set_world(&mut self, world: Option<orbiters_contracts::manifest::WorldInfo>) {
        self.writer.manifest_mut().world = world;
    }

    /// What the source turned out to be (a Spout source names its sender once it connected).
    pub fn set_source_name(&mut self, name: String) {
        self.writer.manifest_mut().capture.source_name = name;
    }

    /// Nanoseconds since the session started.
    pub fn time_ns(&self) -> u64 {
        clock::now_ns().saturating_sub(self.epoch_ns)
    }

    /// Hands the warnings out (each once).
    pub fn take_warnings(&mut self) -> Vec<(Warning, String)> {
        std::mem::take(&mut self.warnings)
    }

    /// Why the recording must stop now (the disk is full, or nothing can be written any more).
    pub fn must_stop(&self) -> Option<&str> {
        self.stop.as_deref()
    }

    /// Notes something the person should know (the stats keep the latest of each kind).
    pub fn warn(&mut self, kind: Warning, message: String) {
        match self.latest.iter_mut().find(|(k, _)| *k == kind) {
            Some(latest) => latest.1 = message.clone(),
            None => self.latest.push((kind, message.clone())),
        }
        self.stats.warnings = self.latest.iter().map(|(_, m)| m.clone()).collect();
        self.warnings.push((kind, message));
    }

    /// A frame without a readable strip at `now`: a stretch without it starts or goes on, from the recording's first
    /// frame when no strip came yet (with Local User off in VRChat's camera, or on an avatar without the kit, none ever
    /// does). The service reads `strip_missing_s` and says why: only it knows VRChat's camera and the avatar.
    fn strip_missed(&mut self, now: u64) {
        let lost = *self.strip_lost_ns.get_or_insert(now);
        self.stats.strip_missing_s = now.saturating_sub(lost) as f32 / 1e9;
    }

    /// A frame whose strip was read at `now`: a stretch without it ends, counted when long enough.
    fn strip_read(&mut self, now: u64) {
        if let Some(lost) = self.strip_lost_ns.take() {
            let gap = now.saturating_sub(lost);
            if gap >= STRIP_GAP_NS {
                self.strip_missing_ns += gap;
            }
        }
        self.strip_first_ns.get_or_insert(now);
        self.strip_seen_ns = Some(now);
        self.stats.strip_missing_s = 0.0;
    }

    /// Time without the strip after it was first seen, the stretch still open at the end included; and the time since
    /// it was first seen.
    fn strip_missing(&self) -> (u64, u64) {
        let open = self.strip_lost_ns.map_or(0, |lost| self.last_frame_ns.saturating_sub(lost));
        let open = if open >= STRIP_GAP_NS { open } else { 0 };
        let since_seen = self.strip_first_ns.map_or(0, |first| self.last_frame_ns.saturating_sub(first));
        (self.strip_missing_ns + open, since_seen)
    }

    /// A file was written: writing works.
    fn wrote(&mut self) {
        self.failing = None;
    }

    /// A file could not be written. The recording goes on (an antivirus holding a file, a moment's trouble) unless
    /// failures keep coming, with nothing written, for a while ([`Failing::persistent`]).
    fn write_failed(&mut self, what: &str, error: String) {
        self.write_errors += 1;
        let now = Instant::now();
        match &mut self.failing {
            Some(f) if f.last.elapsed() <= FAILURE_RUN => (f.last, f.count, f.error) = (now, f.count + 1, error.clone()),
            _ => self.failing = Some(Failing { since: now, last: now, count: 1, error: error.clone() }),
        }
        self.warn(Warning::Disk, format!("Files of the session could not be written ({what}: {error})"));
    }

    /// Stops the recording while a little space is left (files cut short by a full disk are worse than a session
    /// that ends), and says so before.
    fn check_free_space(&mut self) {
        if self.free_checked.is_some_and(|t| t.elapsed() < CHECKPOINT_EVERY) {
            return;
        }
        self.free_checked = Some(Instant::now());
        let Some(free) = free_space(self.writer.dir()) else { return };
        let mb = free >> 20;
        if free < FREE_STOP && self.stop.is_none() {
            self.stop = Some(format!("Recording stopped: only {mb} MB is free on the disk the sessions are saved to."));
        } else if free < FREE_WARN && !self.free_warned {
            self.free_warned = true;
            self.warn(Warning::Disk, format!("Only {mb} MB is free on the disk the sessions are saved to: the recording stops below 512 MB."));
        }
    }

    /// Records OSC values from VRChat (avatar parameters, face tracking, head and wrist poses).
    pub fn osc(&mut self, samples: &[OscSample], paused: bool) {
        for sample in samples {
            if let Err(e) = self.parameters.add(self.epoch_ns, sample, paused) {
                self.warn(Warning::Streams, format!("Avatar parameters are not recorded: {e:#}"));
                break;
            }
        }
    }

    /// VRChat runs as process `pid` now (it started after the session, or restarted): the session's VRChat sound
    /// follows it, or starts with it.
    pub fn game_audio(&mut self, pid: u32) {
        let source = AudioSource::Process(pid);
        if let Some(r) = self.audio.iter().find(|a| a.kind == TrackKind::Game) {
            r.retarget(source);
            return;
        }
        match AudioRecorder::start(TrackKind::Game, source, self.writer.dir(), self.epoch_ns) {
            Ok(r) => {
                eprintln!("recording {}", r.source_name);
                r.set_paused(self.performance_paused);
                self.audio.push(r);
            }
            Err(e) => self.warn(Warning::GameSound, format!("The VRChat sound is not recorded: {e:#}")),
        }
    }

    /// A calibration made now in the overlay (`time_ns` read on the shared clock).
    pub fn add_calibration(&mut self, mut calibration: Calibration) {
        calibration.time_ns -= self.epoch_ns as i64;
        self.calibrations.push(calibration);
    }

    /// While paused, the audio tracks record silence and no poses are written.
    pub fn set_performance_paused(&mut self, paused: bool) {
        self.performance_paused = paused;
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
            self.warn(Warning::Streams, format!("Avatar parameters are not recorded: {e:#}"));
        }
    }

    /// Processes one frame: its strip (and the skeleton band) first; the whole picture is read only for a depth frame
    /// or a keyframe, and only when there is room to take it.
    pub fn process(&mut self, frame: &mut dyn FrameAccess) -> anyhow::Result<Processed> {
        let t0 = Instant::now();
        self.collect();
        let (width, height) = (frame.width(), frame.height());
        let stream = frame.stream();
        let capture = &mut self.writer.manifest_mut().capture;
        if stream > 0 && (capture.width, capture.height) != (width, height) {
            // An extra stream (a camera dolly's) never sets nor changes the session's size: until the main stream's
            // first frame, and at another size, its frames are left out.
            self.stats.extra_other_size += 1;
            return Ok(Processed::Frame);
        }
        if stream > 0 {
            self.stats.extra_frames += 1;
        }
        if capture.width == 0 {
            capture.width = width;
            capture.height = height;
        } else if (capture.width, capture.height) != (width, height) {
            return Ok(Processed::SizeChanged { width, height });
        }
        if self.areas.as_ref().is_none_or(|a| (a.width, a.height) != (width, height)) {
            self.areas = Some(Areas::new(width, height));
        }
        let mut record = FrameRecord {
            frame_index: self.frame_index,
            // When the frame was seen, not when it was read back: in step with the sound and the poses.
            capture_time_ns: frame.seen_ns().saturating_sub(self.epoch_ns),
            width,
            height,
            ..Default::default()
        };
        self.frame_index += 1;
        self.stats.frames += 1;
        self.stats.width = width;
        self.stats.height = height;
        let mut waiting = 0u8;
        // The record goes out whatever reading the frame met (frame indices stay consecutive).
        let read = self.read_frame(frame, &mut record, &mut waiting);
        if record.has(record_flags::GATED) {
            self.stats.gated += 1;
        }
        self.pending.push_back(Pending { record, waiting, since: Instant::now() });
        self.queue_faces();
        self.flush_records();
        self.decode_time_total += t0.elapsed().as_secs_f64();
        read.map(|()| Processed::Frame)
    }

    /// Decodes a frame into its record, and queues its keyframe or depth frame (`waiting` counts them). An error is the
    /// source's: a part of the frame could not be read.
    fn read_frame(&mut self, frame: &mut dyn FrameAccess, record: &mut FrameRecord, waiting: &mut u8) -> anyhow::Result<()> {
        let (width, height) = (record.width, record.height);
        let bytes = width as usize * height as usize * 4;
        let now = record.capture_time_ns;
        let stream = frame.stream();
        // The main stream's: what tells the person their camera's strip is read, its camera's place on the map, the
        // skeleton band, the rig's faces and the thumbnail. Extra streams (a camera dolly's) add views.
        let main = stream == 0;
        if !main && self.extra.len() < stream {
            // The main stream's choice, from scratch.
            let mut selector = self.selector.clone();
            selector.reset();
            self.extra.resize_with(stream, || ExtraStream { previous: None, selector: selector.clone() });
        }
        if main {
            self.last_frame_ns = now;
        }
        match self.decode_strip(frame)? {
            Err(e) if !main => {
                let _ = e;
            }
            Err(e) => {
                self.stats.strip_failed += 1;
                self.stats.last_error = Some(e.to_string());
                self.strip_missed(now);
            }
            Ok(d) => {
                if main {
                    self.strip_read(now);
                    self.prefer_flipped = d.flipped;
                    self.stats.flipped = d.flipped;
                    self.stats.strip_ok += 1;
                }
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
                // The capture kit's skeleton band rides on an avatar's colour frames (depth frames use the whole
                // picture; the simulator draws none; the test pattern draws one like the kit).
                let source = strip::flags::source(p.flags);
                if main && !p.is_depth_frame() && !p.is_rig_frame() && (source == strip::flags::SOURCE_AVATAR || source == strip::flags::SOURCE_TEST) {
                    self.read_band(frame, d.flipped, p.time, record.capture_time_ns)?;
                }
                if main {
                    let pos = p.position();
                    self.stats.position = Some(pos);
                    self.stats.heading_deg = Some(p.camera_to_world[2].atan2(p.camera_to_world[10]).to_degrees());
                }

                // Gating: a repeated frame (same game time) or an impossible jump (teleport, respawn). A rig frame's pose
                // is the rig's, wherever it stands: it is gated on its time only, and leaves the camera's last pose as it was.
                // Each stream against its own last pose: the camera dolly's cameras stand elsewhere in the same frames.
                let previous = if main { self.previous } else { self.extra[stream - 1].previous };
                let gated = match &previous {
                    Some((bits, _, _)) if *bits == p.time.to_bits() => true,
                    Some(_) if p.is_rig_frame() => false,
                    Some((_, prev_pose, prev_time)) => {
                        let dt = (p.time - prev_time).abs().max(1e-3);
                        let moved = keyframes::translation(&p.camera_to_world, prev_pose);
                        if moved > 3.0 && moved / dt > 40.0 {
                            self.selector_of(stream).reset();
                            true
                        } else {
                            false
                        }
                    }
                    None => false,
                };
                let previous = match previous {
                    Some((_, pose, time)) if p.is_rig_frame() => Some((p.time.to_bits(), pose, time)),
                    _ => Some((p.time.to_bits(), p.camera_to_world, p.time)),
                };
                if main {
                    self.previous = previous;
                } else {
                    self.extra[stream - 1].previous = previous;
                }
                if gated {
                    record.flags |= record_flags::GATED;
                } else if p.is_rig_frame() && !main {
                    // The rig's faces are the same in every stream: the main stream's are kept.
                } else if p.is_rig_frame() {
                    self.read_faces(frame, &p, d.flipped, record)?;
                } else if p.is_depth_frame() {
                    self.wait_for(|s| s.depth.has_room() && s.buffers.room(bytes));
                    if self.depth.has_room() && self.buffers.room(bytes) {
                        let mut data = self.buffers.take();
                        let order = {
                            let view = frame.full()?;
                            copy_upright(&view, d.flipped, &mut data);
                            view.order
                        };
                        let (index, path) = self.writer.reserve_depth();
                        match self.depth.send(DepthJob { frame: record.frame_index, index, path, data, width, height, order }) {
                            Ok(()) => {
                                self.buffers.lent(bytes);
                                *waiting += 1;
                            }
                            Err(job) => {
                                self.buffers.keep(job.data);
                                self.stats.depth_dropped += 1;
                            }
                        }
                    } else {
                        // The decoder is behind: this one is dropped rather than waited for.
                        self.stats.depth_dropped += 1;
                    }
                } else if self.selector_of(stream).clone().consider_view(&p.camera_to_world, p.projection[1], p.time) {
                    // A keyframe it is (asked on a copy of the selector, which keeps waiting for one until it is taken).
                    self.wait_for(|s| s.encoder.has_room() && s.buffers.room(bytes));
                    if !self.encoder.has_room() || !self.buffers.room(bytes) {
                        // The encoders are behind: nothing is copied, and the keyframe is counted as dropped.
                        self.stats.dropped += 1;
                    } else {
                        let mut data = self.buffers.take();
                        let bgra = {
                            let view = frame.full()?;
                            copy_upright(&view, d.flipped, &mut data);
                            view.order == PixelOrder::Bgra
                        };
                        self.selector_of(stream).consider_view(&p.camera_to_world, p.projection[1], p.time);
                        // The first keyframe makes the thumbnail, from below the strip and its patches.
                        let thumbnail = (main && !self.thumbnail_done).then(|| {
                            let top = self.areas.as_ref().and_then(|a| a.strip).map_or(0, |l| l.reserved.y1.min(height - 1));
                            (self.writer.dir().join(bundle_files::THUMBNAIL_FILE), top)
                        });
                        let (index, path) = self.writer.reserve_keyframe();
                        match self.encoder.send(Picture { frame: Some(record.frame_index), path, data, width, height, bgra, thumbnail }) {
                            Ok(()) => {
                                self.buffers.lent(bytes);
                                record.flags |= record_flags::KEYFRAME;
                                record.keyframe_index = index;
                                self.stats.keyframes += 1;
                                if main {
                                    self.thumbnail_done = true;
                                } else {
                                    self.stats.extra_keyframes += 1;
                                }
                                *waiting += 1;
                            }
                            Err(job) => {
                                self.buffers.keep(job.data);
                                self.stats.dropped += 1;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// The keyframe choice of stream `stream` (0 the main one).
    fn selector_of(&mut self, stream: usize) -> &mut KeyframeSelector {
        if stream == 0 {
            &mut self.selector
        } else {
            &mut self.extra[stream - 1].selector
        }
    }

    /// A rig frame's faces (contracts' Docs/rig-frames.md): each one its camera rendered in this frame (its stamp is the
    /// strip's time) gets a record of its own, the next frame indices after its frame's, with the face's pose and size,
    /// and its tile as a keyframe when the face's selector takes one. The frame's own record keeps the rig's pose and
    /// names no picture. The records and pictures wait in `faces_read` for [`Self::queue_faces`].
    fn read_faces(&mut self, frame: &mut dyn FrameAccess, p: &strip::StripPayload, flipped: bool, record: &FrameRecord) -> anyhow::Result<()> {
        // The layout the frame's strip names: an avatar uploaded with an older Toolkit keeps drawing the square one.
        let Some(layout) = rig::RigLayout::of_strip(record.width, record.height, &p.projection) else { return Ok(()) };
        let face_bytes = layout.tile_width as usize * layout.tile_height as usize * 4;
        let wanted: Vec<bool> = rig::TILE_FACES
            .iter()
            .map(|&face| self.face_selectors[face as usize - 1].clone().consider(&rig::face_camera_to_world(&p.camera_to_world, face), p.time))
            .collect();
        let bytes = face_bytes * wanted.iter().filter(|w| **w).count();
        self.wait_for(|s| s.buffers.room(bytes));
        let view = frame.full()?;
        let Some(faces) = rig::faces(p, &view, flipped) else { return Ok(()) };
        for (face, wanted) in faces.iter().zip(wanted) {
            if !face.current(p.time) {
                self.stats.rig_faces_late += 1;
                continue;
            }
            self.stats.rig_faces += 1;
            let shift = strip::flags::FACE_SHIFT;
            let face_record = FrameRecord {
                frame_index: self.frame_index,
                flags: record.flags & (record_flags::STRIP_OK | record_flags::FLIPPED),
                strip_flags: (p.flags & !(strip::flags::FACE_MASK << shift)) | face.face << shift,
                camera_to_world: face.camera_to_world,
                projection: face.projection,
                width: layout.tile_width,
                height: layout.tile_height,
                keyframe_index: -1,
                depth_index: -1,
                ..*record
            };
            self.frame_index += 1;
            let picture = (wanted && self.buffers.room(face_bytes)).then(|| {
                let mut data = self.buffers.take();
                copy_tile_upright(&view, flipped, face.tile, &mut data);
                self.buffers.lent(face_bytes);
                self.face_selectors[face.face as usize - 1].consider(&face.camera_to_world, p.time);
                Picture {
                    frame: Some(face_record.frame_index),
                    path: PathBuf::new(),
                    data,
                    width: layout.tile_width,
                    height: layout.tile_height,
                    bgra: view.order == PixelOrder::Bgra,
                    thumbnail: None,
                }
            });
            if wanted && picture.is_none() {
                self.stats.dropped += 1;
            }
            self.faces_read.push(FaceView { record: face_record, picture });
        }
        if let Some(message) = rig_faces_unread(self.stats.rig_faces, self.stats.rig_faces_late) {
            self.warn(Warning::Pictures, message.into());
        }
        Ok(())
    }

    /// Queues the faces read with the frame just queued: their records, then their pictures to the encoders.
    fn queue_faces(&mut self) {
        for FaceView { mut record, picture } in std::mem::take(&mut self.faces_read) {
            let mut waiting = 0u8;
            if let Some(mut picture) = picture {
                let (index, path) = self.writer.reserve_keyframe();
                picture.path = path;
                match self.encoder.send(picture) {
                    Ok(()) => {
                        record.flags |= record_flags::KEYFRAME;
                        record.keyframe_index = index;
                        self.stats.keyframes += 1;
                        waiting = 1;
                    }
                    Err(job) => {
                        self.buffers.give(job.data);
                        self.stats.dropped += 1;
                    }
                }
            }
            self.pending.push_back(Pending { record, waiting, since: Instant::now() });
        }
    }

    /// For a source that waits (`offline`): until there is `room`, taking what the threads finish meanwhile, or
    /// [`RECORD_PATIENCE`] at most (a thread that died).
    fn wait_for(&mut self, room: impl Fn(&Self) -> bool) {
        if !self.offline {
            return;
        }
        let until = Instant::now() + RECORD_PATIENCE;
        while !room(self) && Instant::now() < until && !self.encoder.dead() && !self.depth.dead() {
            std::thread::sleep(Duration::from_millis(1));
            self.collect();
        }
    }

    /// Waits for every queued file to be written, its record to go out, and the saver to be done (tests read the
    /// bundle meanwhile).
    #[cfg(test)]
    fn settle(&mut self) {
        let until = Instant::now() + RECORD_PATIENCE;
        while (self.pending.iter().any(|p| p.waiting > 0) || !self.saver.idle()) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(2));
            self.collect();
        }
    }

    /// Decodes the strip, in the last frame's orientation first; the band's rows of that orientation are read along
    /// with it (a colour frame's band is decoded next).
    fn decode_strip(&mut self, frame: &mut dyn FrameAccess) -> anyhow::Result<Result<strip::StripDecode, strip::StripError>> {
        let Some(areas) = &self.areas else { return Ok(Err(strip::StripError::ImageTooSmall)) };
        let Some(layout) = areas.strip else { return Ok(Err(strip::StripError::ImageTooSmall)) };
        let first = self.prefer_flipped;
        let tried = {
            let view = frame.rows(&areas.rows[first as usize])?;
            strip::decode_oriented(&view, &layout, first)
        };
        match tried {
            Ok(d) => Ok(Ok(d)),
            Err(error) => {
                let view = frame.rows(&areas.rows[!first as usize])?;
                // The first error tells more, unless it only says there was no strip that way up.
                Ok(strip::decode_oriented(&view, &layout, !first).map_err(|second| match error {
                    strip::StripError::NoStrip { .. } => second,
                    _ => error,
                }))
            }
        }
    }

    /// The capture kit's skeleton band of a colour frame, into `skeleton.bin`: only a band of this frame (the strip's
    /// game time, bit for bit), not another kit wearer's in view.
    fn read_band(&mut self, frame: &mut dyn FrameAccess, flipped: bool, time: f32, time_ns: u64) -> anyhow::Result<()> {
        let Some(rows) = self.areas.as_ref().and_then(|a| a.band[flipped as usize]) else { return Ok(()) };
        let pose = {
            let view = frame.rows(&[rows])?;
            match self.band.decode(&view, flipped).map(|b| b.result) {
                Some(Ok(pose)) if pose.is_of_frame(time) => pose,
                _ => return Ok(()),
            }
        };
        // Bone 0 is the hips (HumanBodyBones.Hips).
        if let Some(rotation) = pose.bones[0] {
            self.hips = Some((pose.hips, rotation));
        }
        if let Err(e) = self.skeleton.add(time_ns, &pose) {
            self.warn(Warning::Streams, format!("The skeleton band is not recorded: {e:#}"));
        }
        Ok(())
    }

    /// Where the avatar's hips were in the last frame whose skeleton band was read, once: the app hangs N4N0's camera
    /// rig from them (contracts' `Docs/rig-frames.md`).
    pub fn take_hips(&mut self) -> Option<([f32; 3], [f32; 4])> {
        self.hips.take()
    }

    /// Takes what the threads finished: records get their files, or lose the flags of those that could not be
    /// written; the coverage map gets the depth frames; buffers come back. Then the records that are ready go out.
    fn collect(&mut self) {
        while let Ok(done) = self.done.try_recv() {
            match done {
                Done::Picture { frame, written, thumbnail_failed, buffer } => {
                    self.buffers.give(buffer);
                    if thumbnail_failed {
                        self.thumbnail_done = false;
                    }
                    match written {
                        Ok(bytes) => {
                            self.written += bytes;
                            self.wrote();
                        }
                        Err(error) => {
                            self.write_failed(if frame.is_some() { "a keyframe" } else { "a depth frame as received" }, error);
                            if let Some(p) = frame.and_then(|f| pending_at(&mut self.pending, f)) {
                                p.record.flags &= !record_flags::KEYFRAME;
                                p.record.keyframe_index = -1;
                                self.stats.keyframes = self.stats.keyframes.saturating_sub(1);
                            }
                        }
                    }
                    if let Some(p) = frame.and_then(|f| pending_at(&mut self.pending, f)) {
                        p.waiting = p.waiting.saturating_sub(1);
                    }
                }
                Done::Depth { frame, index, decoded, buffer, width, height, bgra } => {
                    let mut buffer = Some(buffer);
                    match decoded {
                        Ok(Some(d)) => {
                            self.written += d.bytes;
                            self.wrote();
                            if let Some(p) = pending_at(&mut self.pending, frame) {
                                p.record.flags |= record_flags::DEPTH;
                                p.record.depth_index = index;
                                self.stats.depth_frames += 1;
                                self.coverage.add_depth(&p.record.camera_to_world, &p.record.projection, &d.layout, d.grid_width, &d.codes);
                            }
                            // A few frames with tiles that could not be read, as received: what the world's
                            // post-processing did to the symbols, for whoever improves the decoder next.
                            if d.tiles_rejected > 0 && self.raw_depth_kept < RAW_DEPTH_FRAMES && self.encoder.has_room() {
                                if let Ok(path) = self.writer.raw_depth_path(index) {
                                    // The buffer stays lent: from the decoder to an encoder.
                                    let data = buffer.take().expect("buffer");
                                    match self.encoder.send(Picture { frame: None, path, data, width, height, bgra, thumbnail: None }) {
                                        Ok(()) => self.raw_depth_kept += 1,
                                        Err(job) => buffer = Some(job.data),
                                    }
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(error) => self.write_failed("a depth frame", error),
                    }
                    if let Some(buffer) = buffer {
                        self.buffers.give(buffer);
                    }
                    if let Some(p) = pending_at(&mut self.pending, frame) {
                        p.waiting = p.waiting.saturating_sub(1);
                    }
                }
            }
        }
        self.flush_records();
    }

    /// Appends the records whose files are done, in frame order.
    fn flush_records(&mut self) {
        while let Some(front) = self.pending.front_mut() {
            if front.waiting > 0 {
                if front.since.elapsed() < RECORD_PATIENCE {
                    break;
                }
                // Its files are taking too long (a disk that hangs, a thread that died): the record goes without them.
                front.record.flags &= !(record_flags::KEYFRAME | record_flags::DEPTH);
                front.record.keyframe_index = -1;
                front.record.depth_index = -1;
                front.waiting = 0;
                self.warn(Warning::Pictures, "Keyframes or depth frames took too long to write and were left out.".into());
                continue;
            }
            let p = self.pending.pop_front().expect("a record");
            if let Err(e) = self.writer.append_record(&p.record) {
                self.write_failed("frames.bin", format!("{e:#}"));
            }
        }
    }

    /// The coverage map so far, with hints for where the camera is now (none while one of its numbers is not one JSON
    /// can carry).
    pub fn coverage_map(&self) -> Option<CoverageMap> {
        self.coverage
            .map(self.stats.position, self.stats.heading_deg, self.stats.elapsed_s)
            .filter(|m| [m.cell_m, m.x0, m.z0].iter().all(|v| v.is_finite()) && m.floor_y.is_none_or(f32::is_finite))
    }

    /// Refreshes rates and byte counts, takes what the threads finished and saves the session so far when due; call
    /// before sending stats.
    pub fn refresh_stats(&mut self) -> &CaptureStats {
        self.collect();
        let (since, frames_then) = self.fps_window;
        let dt = since.elapsed().as_secs_f32();
        if dt >= 1.0 {
            self.stats.fps = (self.stats.frames - frames_then) as f32 / dt;
            self.fps_window = (Instant::now(), self.stats.frames);
        }
        self.stats.decode_ms = if self.stats.frames > 0 { (self.decode_time_total / self.stats.frames as f64 * 1000.0) as f32 } else { 0.0 };
        // Every file the session writes, the sound and the skeleton band's frames too (the storage limit counts them).
        let sound: u64 = self.audio.iter().filter_map(AudioRecorder::track).map(|t| 58 + t.frames * t.channels as u64 * 4).sum();
        let skeleton = self.skeleton.frames * perf::skeleton_frames::RECORD_SIZE as u64;
        self.stats.bytes = self.writer.manifest().stats.bytes + self.written + self.parameters.changes * 16 + sound + skeleton;
        self.stats.elapsed_s = self.time_ns() as f64 / 1e9;
        self.stats.parameter_changes = self.parameters.changes;
        self.stats.skeleton_frames = self.skeleton.frames;
        if let Err(e) = self.skeleton.flush() {
            self.warn(Warning::Streams, format!("The skeleton band is not recorded: {e:#}"));
        }
        self.stats.mic_level = self.audio.iter().find(|a| a.kind == TrackKind::Mic).map(AudioRecorder::take_level);
        self.stats.game_level = self.audio.iter().find(|a| a.kind == TrackKind::Game).map(AudioRecorder::take_level);
        let troubles: Vec<(TrackKind, String)> = self.audio.iter().filter_map(|a| a.take_trouble().map(|t| (a.kind, t))).collect();
        for (kind, trouble) in troubles {
            self.warn(audio_warning(kind).0, trouble);
        }
        if let Some(t) = &self.tracking {
            let c = t.counts();
            self.stats.tracking_connected = Some(c.connected);
            (self.stats.tracking_devices, self.stats.tracking_samples, self.stats.calibrations) = (c.devices, c.samples, c.calibrations);
            self.stats.bytes += c.samples * 64;
        }
        if let Err(e) = self.parameters.flush() {
            self.warn(Warning::Streams, format!("Avatar parameters are not recorded: {e:#}"));
        }
        if self.checkpointed.elapsed() >= CHECKPOINT_EVERY {
            if let Err(e) = self.checkpoint() {
                self.write_failed("the session so far", format!("{e:#}"));
            }
        }
        // What the saver wrote since, or could not.
        let (saved, errors) = self.saver.outcome();
        if saved > 0 {
            self.wrote();
        }
        for error in errors {
            self.write_failed("the session so far", error);
        }
        if let Some(f) = self.failing.as_ref().filter(|f| f.persistent()) {
            if self.stop.is_none() {
                self.stop = Some(format!("Recording stopped: the session could not be written ({}).", f.error));
            }
        }
        self.check_free_space();
        finite_stats(&mut self.stats);
        &self.stats
    }

    /// Writes what the session holds so far, every few seconds: the streams' tables (`parameters.json`,
    /// `audio.json` as the WAVE headers stand, `calibration.json`; `tracking.json` is kept by its thread), the
    /// manifest with its performance block, and the frame records. A session that never finishes (the worker
    /// killed or crashed, Windows shut down) stays readable up to here. The files written whole (each flushed to the
    /// disk before it replaces the last) go to the saver: the capture thread does not wait for the disk.
    fn checkpoint(&mut self) -> anyhow::Result<()> {
        self.checkpointed = Instant::now();
        self.parameters.flush()?;
        self.skeleton.flush()?;
        let tracks: Vec<_> = self.audio.iter().filter_map(AudioRecorder::track).collect();
        // `audio.json` grows by a sync pair a second per track: compact, and not at every checkpoint.
        if !tracks.is_empty() && self.audio_indexed.is_none_or(|t| t.elapsed() >= AUDIO_INDEX_EVERY) {
            self.audio_indexed = Some(Instant::now());
            let mut index = perf::audio::Audio::new();
            index.tracks = tracks.clone();
            self.saver.save(self.writer.dir().join(perf::AUDIO_FILE), serde_json::to_vec(&index)?);
        }
        let (devices, detected) = self.tracking.as_ref().map(TrackingRecorder::found).unwrap_or_default();
        let calibrations = session_calibrations(&self.calibrations, detected, &devices);
        let written = (calibrations.len(), devices.len());
        if written != self.calibrations_written {
            if let Some(json) = tracking::calibrations_json(calibrations)? {
                self.saver.save(self.writer.dir().join(perf::CALIBRATION_FILE), json);
            }
            self.calibrations_written = written;
        }
        let performance = &mut self.writer.manifest_mut().performance;
        performance.parameter_changes = self.parameters.changes;
        performance.skeleton_frames = self.skeleton.frames;
        performance.tracking_samples = self.tracking.as_ref().map_or(0, |t| t.counts().samples);
        performance.calibrations = written.0 as u32;
        performance.audio = tracks.into_iter().map(|t| t.file).collect();
        let (path, manifest) = self.writer.checkpoint_deferred()?;
        self.saver.save(path, manifest);
        Ok(())
    }

    /// Waits for pending files and writes the final manifest. Every step runs even when one fails (a full disk, a
    /// file another program holds): whatever can be written is, the manifest last, and what went wrong comes back
    /// with it, so the service always learns that the session ended.
    pub fn finish(mut self) -> Finished {
        // Every queued file first: the depth decoder's (whose frames may still ask for a `depth-raw/` picture), then
        // the encoders'. Their records follow them into frames.bin.
        self.depth.finish();
        self.collect();
        self.encoder.finish();
        self.collect();
        // A record whose file never came (a thread that died) goes without it.
        for p in &mut self.pending {
            if p.waiting > 0 {
                p.record.flags &= !(record_flags::KEYFRAME | record_flags::DEPTH);
                (p.record.keyframe_index, p.record.depth_index, p.waiting) = (-1, -1, 0);
            }
        }
        self.flush_records();
        // The last checkpoint's files before the final ones (written here), which must come after them.
        self.saver.finish();
        for error in self.saver.outcome().1 {
            eprintln!("finishing the session: the last checkpoint: {error}");
        }
        let (strip_missing_ns, strip_since_ns) = self.strip_missing();
        let Self { mut writer, parameters, skeleton, audio, tracking, calibrations, calibrations_written, written, write_errors, .. } = self;
        let mut warnings = Vec::new();
        // Long stretches without the strip leave parts of the walk without keyframes: how much. (No strip at all is the
        // service's to tell, knowing why: the manifest's keyframes say so.)
        if strip_since_ns > 0 && strip_missing_ns >= 5_000_000_000 && strip_missing_ns * 10 >= strip_since_ns {
            warnings.push(format!(
                "no camera pose for {:.0} s of {:.0} s: the capture strip was not seen, so those parts have no keyframes",
                strip_missing_ns as f64 / 1e9,
                strip_since_ns as f64 / 1e9
            ));
        }
        writer.add_bytes(written);
        if write_errors > 0 {
            warnings.push(format!("{write_errors} file(s) could not be written"));
        }
        match parameters.finish() {
            Ok((changes, bytes)) => {
                writer.manifest_mut().performance.parameter_changes = changes;
                writer.add_bytes(bytes);
            }
            Err(e) => warnings.push(format!("avatar parameters: {e:#}")),
        }
        match skeleton.finish() {
            Ok((frames, bytes)) => {
                writer.manifest_mut().performance.skeleton_frames = frames;
                writer.add_bytes(bytes);
            }
            Err(e) => warnings.push(format!("skeleton: {e:#}")),
        }
        let mut tracks = perf::audio::Audio::new();
        for recorder in audio {
            match recorder.finish() {
                Ok(track) => {
                    writer.add_bytes(58 + track.frames * track.channels as u64 * 4);
                    tracks.tracks.push(track);
                }
                Err(e) => warnings.push(format!("audio: {e:#}")),
            }
        }
        // The list is the tracks', whatever the checkpoints wrote before (they listed the same files).
        writer.manifest_mut().performance.audio = tracks.tracks.iter().map(|t| t.file.clone()).collect();
        if !tracks.tracks.is_empty() {
            let index = serde_json::to_vec_pretty(&tracks).map_err(anyhow::Error::from);
            if let Err(e) = index.and_then(|bytes| Ok(orbiters_core::write_atomic(&writer.dir().join(perf::AUDIO_FILE), &bytes)?)) {
                warnings.push(format!("audio.json: {e:#}"));
            }
        }
        let (mut devices, mut detected, mut lost_devices) = (Vec::new(), Vec::new(), false);
        if let Some(t) = tracking {
            // What the recorder knew at its last write: the devices to number calibrations by if it fails now.
            let found = t.found();
            match t.finish() {
                Ok(r) => {
                    eprintln!("tracking: {} devices, {} samples, {} T-poses", r.devices.len(), r.samples, r.calibrations.len());
                    writer.manifest_mut().performance.tracking_samples = r.samples;
                    writer.add_bytes(perf::HEADER_SIZE as u64 * u64::from(r.samples > 0) + r.samples * 64);
                    detected = r.calibrations;
                    devices = r.devices;
                }
                Err(e) => {
                    warnings.push(format!("tracking: {e:#}"));
                    (devices, detected) = found;
                    lost_devices = devices.is_empty();
                }
            }
        }
        // Calibrations are numbered by tracking's devices: when tracking failed knowing none, the last checkpoint's
        // file stays as it was rather than being replaced by one with every device unknown. Otherwise (tracking off
        // included) the file gets every calibration, those made since the last checkpoint too.
        if !(lost_devices && calibrations_written.0 > 0) {
            let calibrations = session_calibrations(&calibrations, detected, &devices);
            writer.manifest_mut().performance.calibrations = calibrations.len() as u32;
            match tracking::write_calibrations(writer.dir(), calibrations) {
                Ok(bytes) => writer.add_bytes(bytes),
                Err(e) => warnings.push(format!("calibration.json: {e:#}")),
            }
        }
        let (dir, mut manifest, error) = writer.finish_keeping();
        if let Some(e) = error {
            warnings.push(format!("the session's manifest: {e:#}"));
        }
        finite_manifest(&mut manifest);
        for w in &warnings {
            eprintln!("finishing the session: {w}");
        }
        Finished { dir, manifest, warnings }
    }
}

/// A finished session: where it is, its manifest, and what could not be written (empty when all was).
pub struct Finished {
    pub dir: PathBuf,
    pub manifest: SessionManifest,
    pub warnings: Vec<String>,
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
    #[test]
    fn encoded_picture_thumbnail_excludes_both_data_bands() {
        let dir = std::env::temp_dir().join(format!("orbiters-encoded-thumb-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let (width, height) = (1920, 1080);
        let crop = bundle::capture_picture_rect(width, height);
        let mut pixels = vec![255; width as usize * height as usize * 4];
        for p in pixels[crop.y0 as usize * width as usize * 4..crop.y1 as usize * width as usize * 4].chunks_exact_mut(4) {
            p.copy_from_slice(&[30, 60, 90, 255]); // BGRA input.
        }
        let (tx, rx) = bounded(1);
        encode(
            Picture {
                frame: None,
                path: dir.join("frame.qoi"),
                data: pixels,
                width,
                height,
                bgra: true,
                thumbnail: Some((dir.join(bundle_files::THUMBNAIL_FILE), crop.y0)),
            },
            &tx,
        );
        assert!(matches!(rx.recv().unwrap(), Done::Picture { thumbnail_failed: false, written: Ok(_), .. }));
        let (w, h, rgba) = bundle::read_thumbnail(&dir).unwrap();
        assert_eq!((w, h), (480, 196));
        assert!(rgba.chunks_exact(4).all(|p| p == [90, 60, 30, 255]));
        std::fs::remove_dir_all(dir).unwrap();
    }
    use crate::sources::{FrameSource, Next, TestPatternSource, Whole};
    use orbiters_contracts::manifest::{CaptureMode, ConsentInfo};
    use orbiters_contracts::performance::skeleton_frames;
    use orbiters_core::protocol::SourceSpec;
    use orbiters_core::settings::KeyframeSettings;

    /// A test-pattern session of `frames` 720p frames under a fresh folder of the temporary directory.
    fn pattern_session(name: &str, frames: u32) -> (PathBuf, CaptureSession, TestPatternSource) {
        let root = std::env::temp_dir().join(format!("orbiters-capture-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cmd = StartCapture {
            source: SourceSpec::TestPattern { width: 1280, height: 720, fps: 30.0, depth_every: 4, frames },
            sessions_dir: root.clone(),
            mode: CaptureMode::RecordOnly,
            quality: StorageQuality::High,
            keyframes: KeyframeSettings::default(),
            world: None,
            consent: ConsentInfo::default(),
            folder_name: Some(name.into()),
            performance: Default::default(),
        };
        let src = TestPatternSource::new(1280, 720, 30.0, 4, frames, false);
        let session = CaptureSession::start(&cmd, src.describe(), &src.folder_hint()).unwrap();
        (root, session, src)
    }

    /// The camera dolly's streams (a multi-stream animation): three more cameras of the same frames, elsewhere. Their
    /// frames join the session as views of their own, each stream gated and choosing keyframes against its own last
    /// pose (the same game times in every stream are no repeats); the skeleton band and the camera's place come from
    /// the main stream only; a stream of another size is left out, never a change of the session's size.
    #[test]
    fn the_camera_dollys_streams_join_the_session_as_views_of_their_own() {
        use crate::sources::MultiSource;
        let (root, mut session, src) = pattern_session("dolly-streams", 48);
        let extra: Vec<Box<dyn FrameSource>> = vec![
            Box::new(TestPatternSource::new(1280, 720, 30.0, 4, 48, false).with_stream(1)),
            Box::new(TestPatternSource::new(1280, 720, 30.0, 4, 48, false).with_stream(2)),
            // Another size: left out.
            Box::new(TestPatternSource::new(960, 540, 30.0, 4, 48, false).with_stream(3)),
        ];
        let mut multi = MultiSource::new(Box::new(src), extra);
        assert!(multi.describe().starts_with("Test pattern 1280x720 + "));
        let mut streams = [0u32; 4];
        loop {
            let next = multi
                .next(Duration::from_millis(10), &mut |v| {
                    streams[v.stream()] += 1;
                    assert_eq!(session.process(v).unwrap(), Processed::Frame);
                })
                .unwrap();
            if matches!(next, Next::End) {
                break;
            }
        }
        assert_eq!(streams[0], 48, "the main stream whole");
        let stats = session.refresh_stats().clone();
        assert_eq!((stats.extra_other_size, stats.gated), (u64::from(streams[3]), 0), "no repeats across streams, the odd size left out");
        assert_eq!(stats.extra_frames, u64::from(streams[1] + streams[2]));
        assert!(stats.extra_keyframes >= 4, "views from the other cameras: {}", stats.extra_keyframes);
        assert_eq!(stats.skeleton_frames, 36, "the band of the main stream's colour frames only");
        let finished = session.finish();
        assert!(finished.warnings.is_empty(), "{:?}", finished.warnings);
        let records = bundle::read_records(&finished.dir).unwrap();
        assert_eq!(records.len() as u32, streams[0] + streams[1] + streams[2]);
        assert!(records.windows(2).all(|w| w[1].frame_index == w[0].frame_index + 1), "consecutive frame indices");
        // Each stream's keyframes carry its own camera's pose.
        let poses: Vec<[f32; 12]> = records.iter().filter(|r| r.has(record_flags::KEYFRAME)).map(|r| r.camera_to_world).collect();
        for stream in 0..3 {
            let t = records.iter().find(|r| r.has(record_flags::KEYFRAME) && r.camera_to_world == TestPatternSource::stream_pose(r.game_time, stream));
            assert!(t.is_some(), "a keyframe of stream {stream} among {}", poses.len());
        }
        assert_eq!(finished.manifest.capture.source, "test");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Keyframes and depth frames that cannot be written are named by no record: the session stays readable whole.
    #[test]
    fn records_never_name_files_that_were_not_written() {
        let (root, mut session, mut src) = pattern_session("unwritable", 24);
        // The pictures' folders vanish (a drive unplugged, a cleaner): every picture fails to write.
        std::fs::remove_dir_all(session.dir().join("keyframes")).unwrap();
        std::fs::remove_dir_all(session.dir().join("depth")).unwrap();
        while !matches!(src.next(std::time::Duration::from_millis(10), &mut |v| assert_eq!(session.process(v).unwrap(), Processed::Frame)), Ok(Next::End)) {}
        let finished = session.finish();
        assert!(finished.warnings.iter().any(|w| w.contains("could not be written")), "{:?}", finished.warnings);
        let records = bundle::read_records(&finished.dir).unwrap();
        assert_eq!(records.len(), 24);
        assert!(records.iter().all(|r| !r.has(record_flags::KEYFRAME) && !r.has(record_flags::DEPTH)));
        assert!(records.iter().all(|r| r.keyframe_index == -1 && r.depth_index == -1));
        assert_eq!((finished.manifest.stats.frames, finished.manifest.stats.keyframes, finished.manifest.stats.depth_frames), (24, 0, 0));
        std::fs::remove_dir_all(&root).ok();
    }

    /// The Stream camera set to another resolution: its frames are left for a new session (a bundle has one size).
    #[test]
    fn a_new_frame_size_is_left_for_a_new_session() {
        let (root, mut session, _) = pattern_session("resized", 0);
        let big = TestPatternSource::new(1280, 720, 30.0, 0, 1, false).render(0);
        let small = TestPatternSource::new(640, 360, 30.0, 0, 1, false).render(0);
        assert_eq!(session.process(&mut Whole::new(big.view())).unwrap(), Processed::Frame);
        assert_eq!(session.process(&mut Whole::new(small.view())).unwrap(), Processed::SizeChanged { width: 640, height: 360 });
        let finished = session.finish();
        let m = &finished.manifest;
        assert_eq!((m.capture.width, m.capture.height, m.stats.frames), (1280, 720, 1));
        std::fs::remove_dir_all(&root).ok();
    }

    /// Odd sizes record like any other, and a frame too small for a strip is a frame without one (not a panic).
    #[test]
    fn odd_and_tiny_frame_sizes_are_recorded() {
        for (name, w, h, strip) in [("odd", 1365, 767, true), ("tiny", 96, 54, false)] {
            let (root, mut session, _) = pattern_session(name, 0);
            let img = TestPatternSource::new(w, h, 30.0, 0, 1, false).render(0);
            assert_eq!(session.process(&mut Whole::new(img.view())).unwrap(), Processed::Frame);
            let finished = session.finish();
            assert!(finished.warnings.is_empty(), "{:?}", finished.warnings);
            let records = bundle::read_records(&finished.dir).unwrap();
            assert_eq!((records.len(), records[0].has(record_flags::STRIP_OK), records[0].has(record_flags::KEYFRAME)), (1, strip, strip), "{name}");
            if strip {
                let kf = finished.dir.join("keyframes").join(bundle::keyframe_file(records[0].keyframe_index));
                let (head, _) = qoi::decode_rgba(&std::fs::read(kf).unwrap()).unwrap();
                assert_eq!((head.width, head.height), (w, h));
            }
            std::fs::remove_dir_all(&root).ok();
        }
    }

    /// A frame's time is when the source saw it, not when it was decoded (later, after its readback).
    #[test]
    fn frames_are_timed_when_they_were_seen() {
        let (root, mut session, _) = pattern_session("timed", 0);
        let img = TestPatternSource::new(1280, 720, 30.0, 0, 1, false).render(0);
        let seen_ns = session.epoch_ns + 5_000_000;
        session.process(&mut Whole { view: img.view(), seen_ns }).unwrap();
        let finished = session.finish();
        assert_eq!(bundle::read_records(&finished.dir).unwrap()[0].capture_time_ns, 5_000_000);
        std::fs::remove_dir_all(&root).ok();
    }

    /// What the worker sends reads back strictly: no NaN or infinity, which JSON would carry as null.
    #[test]
    fn events_carry_only_numbers_json_can_hold() {
        let mut s = CaptureStats {
            frames: 3,
            fps: f32::NAN,
            decode_ms: f32::INFINITY,
            elapsed_s: f64::NAN,
            position: Some([0.0, f32::NAN, 1.0]),
            heading_deg: Some(f32::NEG_INFINITY),
            mic_level: Some(f32::INFINITY),
            ..Default::default()
        };
        finite_stats(&mut s);
        let line = orbiters_core::protocol::WorkerEvent::Stats(s).to_line();
        assert!(serde_json::from_str::<orbiters_core::protocol::WorkerEvent>(&line).is_ok(), "{line}");
        let mut m = SessionManifest::default();
        (m.stats.duration_s, m.stats.first_game_time, m.stats.last_game_time) = (f64::NAN, Some(f32::NAN), Some(2.0));
        finite_manifest(&mut m);
        assert_eq!((m.stats.duration_s, m.stats.first_game_time, m.stats.last_game_time), (0.0, None, Some(2.0)));
    }

    /// The checkpoints' files go to their own thread: the latest version of each is written, and what could not be is
    /// told.
    #[test]
    fn the_saver_writes_the_latest_of_each_file() {
        let dir = std::env::temp_dir().join(format!("orbiters-saver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut saver = Saver::spawn();
        let manifest = dir.join("manifest.json");
        saver.save(manifest.clone(), b"1".to_vec());
        saver.save(manifest.clone(), b"2".to_vec());
        saver.save(dir.join("no such folder").join("audio.json"), b"x".to_vec());
        saver.finish();
        assert_eq!(std::fs::read(&manifest).unwrap(), b"2");
        let (written, errors) = saver.outcome();
        assert!((1..=2).contains(&written), "{written}");
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].starts_with("audio.json"), "{errors:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn frames_lent_to_the_threads_stay_under_a_budget() {
        let mut b = Buffers::default();
        // Six 8K frames fit, not seven.
        let frame = 7680 * 4320 * 4;
        assert_eq!((0..10).take_while(|_| b.room(frame).then(|| b.lent(frame)).is_some()).count(), 6);
        b.out = Buffers::OUT_MOST - 16;
        assert!(b.room(16) && !b.room(17));
        b.give(vec![0u8; 16]);
        assert_eq!(b.out, Buffers::OUT_MOST - 32);
        assert_eq!(b.take().len(), 16, "kept for the next copy");
    }

    /// Writing that fails for a moment is told once and waited out; writing that keeps failing stops the recording.
    #[test]
    fn a_disk_that_keeps_failing_stops_the_recording() {
        let (root, mut session, _) = pattern_session("full", 0);
        // Whatever the temporary folder's disk has free: only the failing writes count here.
        (session.stop, session.free_checked) = (None, Some(Instant::now()));
        session.take_warnings();
        let full = "There is not enough space on the disk. (os error 112)";
        session.write_failed("frames.bin", full.into());
        session.write_failed("frames.bin", full.into());
        session.refresh_stats();
        assert_eq!(session.must_stop(), None, "a moment's trouble");
        let told: Vec<_> = session.take_warnings().into_iter().filter(|(k, _)| *k == Warning::Disk).collect();
        assert!(!told.is_empty() && told.iter().all(|(_, w)| w.contains("not enough space")), "{told:?}");
        assert_eq!(session.stats.warnings.len(), 1, "the stats keep the latest of each kind");
        // Something written meanwhile: writing works.
        session.wrote();
        assert!(session.failing.is_none());
        // One failure, however long ago and with nothing written since (paused, no keyframe due): no reason to stop.
        session.write_failed("a keyframe", full.into());
        let earlier = Instant::now().checked_sub(WRITE_PATIENCE).unwrap();
        session.failing.as_mut().unwrap().since = earlier;
        session.refresh_stats();
        assert_eq!(session.must_stop(), None, "a single failure");
        // A failure long after the last one starts a run of its own.
        session.failing.as_mut().unwrap().last = Instant::now().checked_sub(FAILURE_RUN * 2).unwrap();
        session.write_failed("a keyframe", full.into());
        assert_eq!(session.failing.as_ref().map(|f| f.count), Some(1));
        // Failures that keep coming for three seconds: the recording stops.
        session.failing.as_mut().unwrap().since = earlier;
        session.write_failed("a depth frame", full.into());
        session.write_failed("frames.bin", full.into());
        session.refresh_stats();
        assert!(session.must_stop().is_some_and(|why| why.contains("not enough space")), "{:?}", session.must_stop());
        drop(session);
        std::fs::remove_dir_all(&root).ok();
    }

    /// The strip lost while recording (a camera that no longer sees an avatar uploaded with an older N4N0, a world's
    /// depth of field): the stats say for how long, a long stretch is told once, and the end of the session says how
    /// much had no pose.
    /// What a recording gets with Local User off in VRChat's camera, or on an avatar without the kit: real frames, none
    /// with a strip (Enzo's sessions of 2026-11-03 at 22:56, 22:59 and 23:00). The time without it runs from the first frame,
    /// for the service to tell why, and the session says plainly that it has nothing.
    #[test]
    fn frames_that_never_have_a_strip_are_timed_from_the_first_and_leave_no_keyframes() {
        let (root, mut session, _) = pattern_session("never-a-strip", 0);
        let (w, h) = (1280u32, 720u32);
        let pixels = vec![90u8; (w * h * 4) as usize];
        let t0 = session.epoch_ns;
        for i in 0..90u64 {
            let view = ImageView::new(w, h, (w * 4) as usize, &pixels, PixelOrder::Rgba);
            let mut frame = crate::sources::Whole { view, seen_ns: t0 + i * 33_000_000 };
            assert_eq!(session.process(&mut frame).unwrap(), Processed::Frame);
        }
        assert!(session.stats.strip_missing_s > 2.9, "{}", session.stats.strip_missing_s);
        assert_eq!((session.stats.frames, session.stats.strip_ok, session.stats.keyframes), (90, 0, 0));
        let finished = session.finish();
        assert_eq!((finished.manifest.stats.frames, finished.manifest.stats.keyframes, finished.manifest.stats.depth_frames), (90, 0, 0));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn stretches_without_the_strip_are_counted() {
        let (root, mut session, _) = pattern_session("strip-gaps", 0);
        session.take_warnings();
        let s = |seconds: f64| (seconds * 1e9) as u64;
        // No strip yet: the service reads how long (Local User off, or an avatar without the kit, never give one).
        session.strip_missed(s(0.5));
        session.last_frame_ns = s(3.0);
        session.strip_missed(s(3.0));
        assert_eq!(session.stats.strip_missing_s, 2.5);
        session.strip_read(s(3.5));
        assert_eq!(session.stats.strip_missing_s, 0.0);
        session.strip_missing_ns = 0;
        session.strip_first_ns = Some(s(1.0));
        // A blink of half a second is no stretch.
        session.strip_missed(s(2.0));
        session.strip_read(s(2.5));
        assert_eq!(session.strip_missing().0, 0);
        // Eight seconds without it, read by the service as they go.
        for tenth in 30..110 {
            session.last_frame_ns = s(tenth as f64 / 10.0);
            session.strip_missed(session.last_frame_ns);
        }
        assert!((session.stats.strip_missing_s - 7.9).abs() < 0.01, "{}", session.stats.strip_missing_s);
        session.strip_read(s(11.0));
        assert_eq!(session.stats.strip_missing_s, 0.0);
        // A stretch still open at the end counts too: 8 s, then 4 of the last 4.
        session.last_frame_ns = s(15.0);
        session.strip_missed(s(11.0));
        session.strip_missed(s(15.0));
        assert_eq!(session.strip_missing(), (s(8.0) + s(4.0), s(14.0)));
        let finished = session.finish();
        assert!(finished.warnings.iter().any(|w| w.starts_with("no camera pose for 12 s of 14 s")), "{:?}", finished.warnings);
        std::fs::remove_dir_all(&root).ok();
    }

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
        while !matches!(src.next(std::time::Duration::from_millis(10), &mut |v| assert_eq!(session.process(v).unwrap(), Processed::Frame)), Ok(Next::End)) {}
        // The files written, a few seconds on: the stats refresh writes a checkpoint. Then the worker is killed: no
        // finish.
        session.settle();
        session.checkpointed = Instant::now().checked_sub(CHECKPOINT_EVERY).unwrap();
        session.refresh_stats();
        // The saver writes the checkpoint's files a moment later, on its own thread.
        session.settle();
        session.refresh_stats();
        assert_eq!(session.stats.last_error, None);
        assert!(session.stats.warnings.is_empty(), "{:?}", session.stats.warnings);
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

    /// A rig frame's six faces become views of their own: records after their frame's, with the face's pose and size,
    /// their tiles as keyframes; a face rendered in another frame is left out; the frame's own record names no picture.
    #[test]
    fn rig_frames_record_their_faces_as_views() {
        let (root, mut session, src) = pattern_session("rig", 24);
        let mut src = src.with_rig(Some(4));
        while !matches!(src.next(std::time::Duration::from_millis(10), &mut |v| assert_eq!(session.process(v).unwrap(), Processed::Frame)), Ok(Next::End)) {}
        let (late, faces) = (session.stats.rig_faces_late, session.stats.rig_faces);
        let finished = session.finish();
        assert!(finished.warnings.is_empty(), "{:?}", finished.warnings);
        let records = bundle::read_records(&finished.dir).unwrap();
        // 24 frames: 6 depth frames (index % 4 == 3), 6 rig frames (the other odd ones), each with 5 current faces.
        let rig_frames: Vec<_> =
            records.iter().filter(|r| strip::flags::kind(r.strip_flags) == strip::flags::KIND_RIG && strip::flags::face(r.strip_flags) == 0).collect();
        assert_eq!(rig_frames.len(), 6);
        assert_eq!((faces, late), (30, 6));
        assert_eq!(records.len(), 24 + 30);
        assert!(records.windows(2).all(|w| w[1].frame_index == w[0].frame_index + 1), "consecutive frame indices");
        let layout = rig::RigLayout::new(1280, 720).unwrap();
        for frame in &rig_frames {
            assert!(!frame.has(record_flags::KEYFRAME) && !frame.has(record_flags::DEPTH));
            let faces: Vec<_> =
                records.iter().skip_while(|r| r.frame_index <= frame.frame_index).take_while(|r| strip::flags::face(r.strip_flags) != 0).collect();
            assert_eq!(faces.iter().map(|r| strip::flags::face(r.strip_flags)).collect::<Vec<_>>(), vec![2, 5, 1, 6, 3]);
            for r in faces {
                let face = strip::flags::face(r.strip_flags);
                assert_eq!((r.width, r.height, r.game_time.to_bits()), (layout.tile_width, layout.tile_height, frame.game_time.to_bits()));
                assert_eq!(r.camera_to_world, rig::face_camera_to_world(&frame.camera_to_world, face));
                assert_eq!(r.projection, layout.generation.projection());
            }
        }
        // Each face's first view is a keyframe: its tile, at the face's size, showing its face's colour.
        for face in [1u32, 2, 3, 5, 6] {
            let r = records.iter().find(|r| strip::flags::face(r.strip_flags) == face && r.has(record_flags::KEYFRAME)).unwrap();
            let (hdr, rgb) = qoi::decode_rgba(&std::fs::read(finished.dir.join("keyframes").join(bundle::keyframe_file(r.keyframe_index))).unwrap()).unwrap();
            assert_eq!((hdr.width, hdr.height), (layout.tile_width, layout.tile_height));
            let mid = ((layout.tile_height / 2 * layout.tile_width + 5) * 4) as usize;
            assert_eq!(rgb[mid], (face * 40) as u8, "face {face}");
        }
        assert!(records.iter().all(|r| strip::flags::face(r.strip_flags) != 4));
        std::fs::remove_dir_all(&root).ok();
    }

    /// Faces that all come without a stamp are told about, once a few rig frames have shown it is no moment's trouble.
    #[test]
    fn rig_faces_none_of_which_can_be_read_are_told() {
        assert!(rig_faces_unread(0, 29).is_none());
        assert!(rig_faces_unread(0, 30).is_some());
        assert!(rig_faces_unread(1, 3000).is_none());
    }

    /// A rig frame from an avatar uploaded with an older Toolkit: its strip names the square layout, and its faces are read
    /// and stored as what they are, 90° squares of their tile's size.
    #[test]
    fn an_older_uploads_rig_frames_keep_their_layout() {
        let (root, mut session, src) = pattern_session("rig-square", 24);
        let mut src = src.with_old_rig();
        while !matches!(src.next(std::time::Duration::from_millis(10), &mut |v| assert_eq!(session.process(v).unwrap(), Processed::Frame)), Ok(Next::End)) {}
        assert_eq!((session.stats.rig_faces, session.stats.rig_faces_late), (36, 0));
        let finished = session.finish();
        let records = bundle::read_records(&finished.dir).unwrap();
        let layout = rig::RigLayout::of_generation(1280, 720, rig::Generation::Square).unwrap();
        let faces: Vec<_> = records.iter().filter(|r| strip::flags::face(r.strip_flags) != 0).collect();
        assert_eq!(faces.len(), 36);
        for r in faces {
            assert_eq!((r.width, r.height, r.projection), (layout.tile_width, layout.tile_height, rig::SQUARE_PROJECTION));
        }
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
        let mut hips = Vec::new();
        loop {
            let mut outcome = None;
            match src.next(std::time::Duration::from_millis(10), &mut |v| outcome = Some(session.process(v))) {
                Ok(Next::Frame) => assert_eq!(outcome.map(|o| o.unwrap()), Some(Processed::Frame)),
                Ok(Next::End) => break,
                Ok(Next::Timeout) => {}
                Err(e) => panic!("{e}"),
            }
            hips.extend(session.take_hips());
        }
        // Each band's hips, once, for N4N0's camera rig (the app hangs it from them).
        assert_eq!(hips.len(), 45, "the colour frames' bands");
        assert!(session.take_hips().is_none(), "taken");
        assert_eq!((session.stats.dropped, session.stats.depth_dropped), (0, 0), "a test pattern waits for the encoders");
        let finished = session.finish();
        assert!(finished.warnings.is_empty(), "{:?}", finished.warnings);
        let (dir, manifest) = (finished.dir, finished.manifest);
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
        let last = frames.last().unwrap();
        assert_eq!(hips.last().copied(), Some((last.hips, last.rotations[0].unwrap())), "the last band's");
        let colour: Vec<_> = records.iter().filter(|r| !r.has(record_flags::DEPTH)).collect();
        for (frame, record) in frames.iter().zip(&colour) {
            let want = TestPatternSource::skeleton(frame.game_time);
            assert_eq!(frame.time_ns, record.capture_time_ns);
            assert_eq!((frame.hips, frame.avatar, frame.mask()), (want.hips, want.avatar, want.mask()));
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
