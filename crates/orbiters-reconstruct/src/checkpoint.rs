//! Pausing a reconstruction and going on from where it stood: a person takes one 3D picture after another in VRChat, each
//! capture needs the GPU, and the reconstructions behind them must finish however often they are interrupted.
//!
//! A pause ends the worker process. Suspending it instead would keep its graphics memory: a suspended run of Silent Dawn
//! (2.6M Gaussians) held 4.9 GB of the card for all of a 30 s suspension, VRChat Home 2.4 GB for all of 60 s, memory a
//! capture and VRChat need; nothing evicted it, and a run that was suspended finished fine, so it is only the memory
//! that rules suspension out. So the worker reads back what the steps carry ([`crate::gpu::Gpu::training_state`]), lets the
//! device go, and only then writes the file: on Silent Dawn the card was back at its idle memory 0.35 s after the
//! pause was asked, the file (604 MB) written 0.7 s later, and a new worker at its next step 2 s after it started, having
//! read the session again. The capture waits for none of it: asking is a line on a pipe.
//!
//! Where a pause lands, and what the next start does:
//! - preparing the dataset, finding faces, seeding: on the CPU, nothing kept; that part starts over (seconds). A
//!   checkpoint from an earlier pause stays and is used.
//! - the steps: everything is kept ([`StepState`]); they go on at the next step.
//! - after the last step, up to the result's replacing the previous one: the final set is kept ([`TrainedState`]); the
//!   renders that follow it start over (seconds).
//! - the swap of the finished folder itself: never stopped (milliseconds): the run ends as finished.
//!
//! One file per session ([`rec::CHECKPOINT_FILE`], beside its result), replaced atomically and deleted when the run ends
//! whichever way, so old ones do not pile up. It survives the app closing and the PC restarting. One that does not
//! match this worker, the session or the run's settings ([`key`]), or fails its checksum, is discarded without harm: the
//! run starts over.

use crate::gaussians::{Gaussians, PACKED};
use crate::portraits::Portrait;
use crate::train::Start;
use anyhow::{bail, ensure, Context as _};
use orbiters_core::reconstruction as rec;
use serde::{Deserialize, Serialize};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const MAGIC: &[u8; 8] = b"N4CKPT01";
/// A checkpoint's header is small: anything larger is not one.
const MOST_HEADER: u32 = 1 << 20;
/// What a checkpoint leaves free on the session's disk: a capture started this very moment needs it far more than the
/// run does (it stops recording under 512 MB). Under this, the run starts over instead.
const LEAVE_FREE: u64 = 2 << 30;

/// What asks the worker to pause (a line on its standard input, [`listen`]).
#[derive(Debug)]
pub struct Pause {
    asked: AtomicBool,
    /// Still on the CPU, with nothing to keep: the process can simply leave.
    early: AtomicBool,
    /// When it was first asked, to say what a pause took.
    when: std::sync::OnceLock<std::time::Instant>,
}

impl Default for Pause {
    fn default() -> Self {
        Self { asked: AtomicBool::new(false), early: AtomicBool::new(true), when: std::sync::OnceLock::new() }
    }
}

impl Pause {
    pub fn ask(&self) {
        self.when.get_or_init(std::time::Instant::now);
        self.asked.store(true, Ordering::SeqCst);
    }

    /// How long ago it was asked.
    pub fn since_asked(&self) -> std::time::Duration {
        self.when.get().map_or(std::time::Duration::ZERO, |t| t.elapsed())
    }

    pub fn asked(&self) -> bool {
        self.asked.load(Ordering::SeqCst)
    }

    /// From now on a pause has something to keep: the run stops between two steps.
    pub fn begin_steps(&self) {
        self.early.store(false, Ordering::SeqCst);
    }
}

/// What the rounds of renders outside the steps fail with when a pause cuts them short (crate::gpu::Gpu::pause).
#[derive(Clone, Copy, Debug)]
pub struct Interrupted;

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("asked to pause")
    }
}

impl std::error::Error for Interrupted {}

/// The line on the worker's standard input that asks it to pause.
pub const PAUSE_LINE: &str = "pause";

/// Reads the worker's standard input for [`PAUSE_LINE`]. While nothing is worth keeping, `leave` ends the process right
/// away. Standard input closing says nothing: a lab run has none, and the output's pipe tells of an app that went.
pub fn listen(pause: Arc<Pause>, leave: impl Fn() + Send + 'static) {
    let spawned = std::thread::Builder::new().name("pause".into()).spawn(move || {
        for line in std::io::stdin().lines().map_while(Result::ok) {
            if line.trim() == PAUSE_LINE {
                pause.ask();
                if pause.early.load(Ordering::SeqCst) {
                    leave();
                }
            }
        }
    });
    if let Err(e) = spawned {
        eprintln!("could not listen for pauses: {e}");
    }
}

/// Where in a run the lab asks it to pause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabStage {
    Loading,
    Seeding,
    Finish,
    Saving,
}

impl LabStage {
    pub fn parse(name: &str) -> anyhow::Result<Self> {
        Ok(match name {
            "loading" => Self::Loading,
            "seeding" => Self::Seeding,
            "finish" => Self::Finish,
            "saving" => Self::Saving,
            _ => bail!("--pause-in takes loading, seeding, finish or saving"),
        })
    }
}

/// The lab's pauses (`--pause-at`, `--pause-in`): what a capture starting at that moment asks.
#[derive(Clone, Debug, Default)]
pub struct Lab {
    /// Steps (1 or more) or shares of a run's steps (under 1).
    pub at: Vec<f32>,
    pub stage: Option<LabStage>,
}

impl Lab {
    /// Whether to pause with `done` steps done, in a start that began with `from`.
    pub fn pause_at_step(&self, done: u32, from: u32, iterations: u32) -> bool {
        done > from && self.at.iter().any(|&a| done == if a < 1.0 { (a * iterations as f32) as u32 } else { a as u32 })
    }

    pub fn entering(&self, stage: LabStage, pause: Option<&Arc<Pause>>) {
        if self.stage == Some(stage) {
            if let Some(p) = pause {
                p.ask();
            }
        }
    }
}

/// What the steps carry on the device ([`crate::gpu::Gpu::training_state`]).
pub struct TrainingBuffers {
    pub params: Vec<f32>,
    pub m1: Vec<f32>,
    pub m2: Vec<f32>,
    pub stats: Vec<f32>,
}

/// The sky map and its Adam moments ([`crate::gpu::Gpu::sky_learning`]).
pub struct SkyLearningState {
    pub width: u32,
    pub texels: Vec<[f32; 4]>,
    pub m1: Vec<[f32; 4]>,
    pub m2: Vec<[f32; 4]>,
}

/// A run paused between two steps: all that goes on to the next.
pub struct StepState {
    pub iteration: u32,
    pub iterations: u32,
    pub gaussians: usize,
    pub gpu_step: u32,
    pub rng: u64,
    pub noise: f32,
    pub opacity_reg: f32,
    pub scale_reg: f32,
    pub last_round: Option<(usize, usize)>,
    pub fading: bool,
    pub fade_again: bool,
    pub seconds: f32,
    pub psnr: Option<f32>,
    pub buffers: TrainingBuffers,
    pub sky: Option<SkyLearningState>,
}

/// A run paused after its last step: its final set.
pub struct TrainedState {
    pub gaussians: Gaussians,
    pub iterations: u32,
    pub seconds: f32,
    pub psnr: Option<f32>,
}

/// What the worker worked out before the steps, which a resumed run does not work out again.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extras {
    pub budget: usize,
    pub starved: bool,
    pub sky_fill: bool,
    pub portraits: Vec<Portrait>,
    /// The sky map's texels seen (by seeding; after the last step, by training too).
    pub sky_seen: Vec<bool>,
    /// After the last step: the sky map as training left it.
    pub trained_sky: Option<(u32, Vec<[f32; 3]>)>,
}

/// What a checkpoint says about itself, read without its body ([`peek`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Header {
    key: String,
    /// Before the first section: everything after the last step is left to do.
    pub trained: bool,
    pub iteration: u32,
    pub iterations: u32,
    pub gaussians: usize,
    pub seconds: f32,
    pub psnr: Option<f32>,
    budget: usize,
    starved: bool,
    sky_fill: bool,
    gpu_step: u32,
    rng: u64,
    noise: u32,
    opacity_reg: u32,
    scale_reg: u32,
    last_round: Option<(usize, usize)>,
    fading: bool,
    fade_again: bool,
    sky_width: u32,
    sky_trained: u32,
    /// Sections' lengths in bytes, in the order they follow, and a checksum of them all.
    sections: Vec<(String, u64)>,
    checksum: String,
}

/// A checkpoint read back.
pub struct Checkpoint {
    pub extras: Extras,
    pub start: Start,
}

/// What a checkpoint must match to be used: the file's format, this worker (a version, and the program's length for the
/// builds between versions), the session's recording and the run's settings. A session that is reconstructed again
/// with other settings, or a worker updated meanwhile, starts over.
pub fn key(session: &Path, settings: &str) -> anyhow::Result<String> {
    use orbiters_core::contracts::manifest;
    let hash = |file: &str| orbiters_core::visited::sha256_file(&session.join(file)).with_context(|| format!("reading {file}"));
    let program = std::env::current_exe().and_then(std::fs::metadata).map_or(0, |m| m.len());
    Ok(format!("{}|{}|{program}|{}|{}|{settings}", String::from_utf8_lossy(MAGIC), env!("CARGO_PKG_VERSION"), hash(manifest::MANIFEST_FILE)?, hash(manifest::FRAMES_FILE)?))
}

/// The checkpoint of a session.
pub fn file(session: &Path) -> PathBuf {
    session.join(rec::CHECKPOINT_FILE)
}

/// Whether a session has a checkpoint, whatever it is worth.
pub fn exists(session: &Path) -> bool {
    file(session).is_file()
}

/// A session's checkpoint and any file a write cut short: gone.
pub fn discard(session: &Path) {
    let Ok(entries) = std::fs::read_dir(session) else { return };
    for e in entries.flatten() {
        if e.file_name().to_string_lossy().starts_with(rec::CHECKPOINT_FILE) {
            if let Err(error) = std::fs::remove_file(e.path()) {
                eprintln!("could not delete {}: {error}", e.path().display());
            }
        }
    }
}

/// Files a write cut short (the process killed halfway); the checkpoint itself stays.
pub fn sweep(session: &Path) {
    let Ok(entries) = std::fs::read_dir(session) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with(rec::CHECKPOINT_FILE) && name != rec::CHECKPOINT_FILE {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// A session's checkpoint's header, with no judgement on its body: how far the run got, for the app to show.
pub fn peek(session: &Path) -> Option<Header> {
    read_header(&mut std::fs::File::open(file(session)).ok()?).ok()
}

fn read_header(f: &mut std::fs::File) -> anyhow::Result<Header> {
    let mut start = [0u8; 12];
    f.read_exact(&mut start).context("too short")?;
    ensure!(&start[..8] == MAGIC, "another format");
    let length = u32::from_le_bytes(start[8..12].try_into().expect("four bytes"));
    ensure!(length <= MOST_HEADER, "a header of {length} bytes");
    let mut text = vec![0u8; length as usize];
    f.read_exact(&mut text).context("too short")?;
    serde_json::from_slice(&text).context("its header")
}

/// A session's checkpoint when it matches `key`. One that does not is discarded, saying why: the run starts over.
pub fn matching_header(session: &Path, key: &str) -> Option<Header> {
    let found = match std::fs::File::open(file(session)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => Err(anyhow::Error::from(e)),
        Ok(mut f) => read_header(&mut f),
    };
    let why = match found {
        Ok(h) if h.key == key => return Some(h),
        Ok(_) => "made by another version of the worker, of another recording or with other settings".to_string(),
        Err(e) => format!("{e:#}"),
    };
    eprintln!("the paused run's checkpoint is not used ({why}): starting over");
    discard(session);
    None
}

/// A fast checksum (a checkpoint is hundreds of megabytes, and a capture is starting): it finds a damaged file, and
/// nothing about it needs to withstand anyone's effort.
#[derive(Clone, Copy)]
struct Checksum(u64);

impl Checksum {
    fn add(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        let mut h = self.0;
        for w in words {
            h = (h ^ u64::from_le_bytes(*w)).wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(29);
        }
        for b in rest {
            h = (h ^ *b as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(29);
        }
        self.0 = h ^ bytes.len() as u64;
    }
}

const SEED: Checksum = Checksum(0x4e34_434b_5054_3031);

fn bits(flags: &[bool]) -> Vec<u32> {
    let mut words = vec![0u32; flags.len().div_ceil(32)];
    for (i, _) in flags.iter().enumerate().filter(|(_, f)| **f) {
        words[i / 32] |= 1 << (i % 32);
    }
    words
}

fn flags(words: &[u32], n: usize) -> Vec<bool> {
    (0..n).map(|i| words.get(i / 32).is_some_and(|w| w >> (i % 32) & 1 == 1)).collect()
}

fn portrait_words(p: &[Portrait]) -> Vec<u32> {
    p.iter().flat_map(|p| [p.view as u32, p.x0.to_bits(), p.y0.to_bits(), p.x1.to_bits(), p.y1.to_bits(), p.distance.to_bits(), p.time.to_bits(), u32::from(p.still)]).collect()
}

fn portraits_of(words: &[u32]) -> Vec<Portrait> {
    let f = f32::from_bits;
    words.as_chunks::<8>().0.iter().map(|w| Portrait { view: w[0] as usize, x0: f(w[1]), y0: f(w[2]), x1: f(w[3]), y1: f(w[4]), distance: f(w[5]), time: f(w[6]), still: w[7] != 0 }).collect()
}

/// Free space on the disk holding `dir` (Windows; elsewhere unknown).
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

/// The bytes of a checkpoint's sections, in the order [`read_body`] takes them.
type Sections<'a> = Vec<(&'static str, std::borrow::Cow<'a, [u8]>)>;

fn borrowed<T: bytemuck::Pod>(v: &[T]) -> std::borrow::Cow<'_, [u8]> {
    std::borrow::Cow::Borrowed(bytemuck::cast_slice(v))
}

fn owned<T: bytemuck::Pod>(v: &[T]) -> std::borrow::Cow<'static, [u8]> {
    std::borrow::Cow::Owned(bytemuck::cast_slice(v).to_vec())
}

/// Writes a session's checkpoint: to a file beside it, flushed to the disk, then moved over the previous one, so a crash at
/// any moment leaves the previous one or the new one whole. Not enough room to leave a capture what it needs: an error,
/// and the previous one stays.
pub fn write(session: &Path, key: &str, state: &Start, extras: &Extras) -> anyhow::Result<u64> {
    let mut header = Header {
        key: key.to_string(),
        trained: false,
        iteration: 0,
        iterations: 0,
        gaussians: 0,
        seconds: 0.0,
        psnr: None,
        budget: extras.budget,
        starved: extras.starved,
        sky_fill: extras.sky_fill,
        gpu_step: 0,
        rng: 0,
        noise: 0,
        opacity_reg: 0,
        scale_reg: 0,
        last_round: None,
        fading: true,
        fade_again: false,
        sky_width: 0,
        sky_trained: 0,
        sections: Vec::new(),
        checksum: String::new(),
    };
    let mut sections: Sections<'_> = vec![("portraits", owned(&portrait_words(&extras.portraits))), ("sky seen", owned(&bits(&extras.sky_seen)))];
    match state {
        Start::Seeds(_) => bail!("a run that has not started has nothing to keep"),
        Start::Steps(s) => {
            header.iteration = s.iteration;
            header.iterations = s.iterations;
            header.gaussians = s.gaussians;
            header.seconds = s.seconds;
            header.psnr = s.psnr;
            header.gpu_step = s.gpu_step;
            header.rng = s.rng;
            header.noise = s.noise.to_bits();
            header.opacity_reg = s.opacity_reg.to_bits();
            header.scale_reg = s.scale_reg.to_bits();
            header.last_round = s.last_round;
            header.fading = s.fading;
            header.fade_again = s.fade_again;
            sections.push(("params", borrowed(&s.buffers.params)));
            sections.push(("moment 1", borrowed(&s.buffers.m1)));
            sections.push(("moment 2", borrowed(&s.buffers.m2)));
            sections.push(("stats", borrowed(&s.buffers.stats)));
            if let Some(sky) = &s.sky {
                // Most of a map is never looked at: colours for all of it (a fourth float it never uses is left out),
                // moments for what training moved (their second is never zero there).
                header.sky_width = sky.width;
                let trained: Vec<bool> = sky.m2.iter().map(|m| m[0] != 0.0 || m[1] != 0.0 || m[2] != 0.0).collect();
                let rgb: Vec<[f32; 3]> = sky.texels.iter().map(|t| [t[0], t[1], t[2]]).collect();
                let pick = |m: &[[f32; 4]]| -> Vec<[f32; 4]> { m.iter().zip(&trained).filter(|(_, t)| **t).map(|(v, _)| *v).collect() };
                let (m1, m2) = (pick(&sky.m1), pick(&sky.m2));
                header.sky_trained = m2.len() as u32;
                sections.push(("sky colours", owned(&rgb)));
                sections.push(("sky trained", owned(&bits(&trained))));
                sections.push(("sky moment 1", owned(&m1)));
                sections.push(("sky moment 2", owned(&m2)));
            }
        }
        Start::Trained(t) => {
            header.trained = true;
            header.iteration = t.iterations;
            header.iterations = t.iterations;
            header.gaussians = t.gaussians.len();
            header.seconds = t.seconds;
            header.psnr = t.psnr;
            sections.push(("gaussians", owned(&t.gaussians.pack())));
            if let Some((width, rgb)) = &extras.trained_sky {
                header.sky_width = *width;
                sections.push(("sky colours", owned(rgb)));
            }
        }
    }
    let total: u64 = sections.iter().map(|(_, b)| b.len() as u64).sum();
    if let Some(free) = free_space(session) {
        ensure!(free >= total + LEAVE_FREE, "only {} MB is free on the disk: a checkpoint of {} MB would leave a capture too little", free >> 20, total >> 20);
    }
    let mut sum = SEED;
    for (name, bytes) in &sections {
        sum.add(bytes);
        header.sections.push((name.to_string(), bytes.len() as u64));
    }
    header.checksum = format!("{:016x}", sum.0);
    let text = serde_json::to_vec(&header)?;
    let path = file(session);
    let tmp = session.join(format!("{}.{}.tmp", rec::CHECKPOINT_FILE, std::process::id()));
    let written = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(MAGIC)?;
        f.write_all(&(text.len() as u32).to_le_bytes())?;
        f.write_all(&text)?;
        // In pieces: a capture writing to the same disk gets its turn between them.
        for (_, bytes) in &sections {
            for piece in bytes.chunks(16 << 20) {
                f.write_all(piece)?;
            }
        }
        f.sync_all()?;
        drop(f);
        orbiters_core::rename_retrying(&tmp, &path)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("writing {}", path.display()));
    }
    Ok(total + text.len() as u64 + 12)
}

/// Takes a checkpoint's sections off its file, in the order they were written.
struct Reader<'a> {
    f: std::io::BufReader<std::fs::File>,
    sections: std::slice::Iter<'a, (String, u64)>,
    sum: Checksum,
}

impl Reader<'_> {
    fn next<T: bytemuck::Pod>(&mut self, name: &str) -> anyhow::Result<Vec<T>> {
        let (found, bytes) = self.sections.next().with_context(|| format!("no {name}"))?;
        ensure!(found == name, "{found} where {name} should be");
        let size = std::mem::size_of::<T>() as u64;
        ensure!(bytes % size == 0, "{name} of {bytes} bytes");
        let mut v = vec![T::zeroed(); (bytes / size) as usize];
        self.f.read_exact(bytemuck::cast_slice_mut(&mut v)).with_context(|| format!("reading {name}"))?;
        self.sum.add(bytemuck::cast_slice(&v));
        Ok(v)
    }
}

/// A checkpoint's body, checked against its checksum.
pub fn read_body(session: &Path, header: &Header) -> anyhow::Result<Checkpoint> {
    let mut f = std::fs::File::open(file(session))?;
    let again = read_header(&mut f)?;
    ensure!(again == *header, "it changed since its header was read");
    let mut r = Reader { f: std::io::BufReader::with_capacity(1 << 20, f), sections: header.sections.iter(), sum: SEED };
    let portraits = portraits_of(&r.next::<u32>("portraits")?);
    let seen = r.next::<u32>("sky seen")?;
    let texels = header.sky_width as usize * (header.sky_width as usize / 2);
    let mut extras = Extras { budget: header.budget, starved: header.starved, sky_fill: header.sky_fill, portraits, sky_seen: flags(&seen, texels), trained_sky: None };
    let start = if header.trained {
        let packed = r.next::<f32>("gaussians")?;
        ensure!(packed.len() == header.gaussians * PACKED, "{} floats for {} Gaussians", packed.len(), header.gaussians);
        let gaussians = Gaussians::unpack(&packed);
        drop(packed);
        if header.sky_width > 0 {
            let rgb = r.next::<[f32; 3]>("sky colours")?;
            ensure!(rgb.len() == texels, "{} sky texels for {} across", rgb.len(), header.sky_width);
            extras.trained_sky = Some((header.sky_width, rgb));
        }
        Start::Trained(Box::new(TrainedState { gaussians, iterations: header.iterations, seconds: header.seconds, psnr: header.psnr }))
    } else {
        let buffers = TrainingBuffers { params: r.next("params")?, m1: r.next("moment 1")?, m2: r.next("moment 2")?, stats: r.next("stats")? };
        let n = header.gaussians;
        ensure!(buffers.params.len() == n * PACKED && buffers.m1.len() == n * PACKED && buffers.m2.len() == n * PACKED && buffers.stats.len() == n * 8, "sections of another size than {n} Gaussians");
        let sky = if header.sky_width > 0 {
            let rgb = r.next::<[f32; 3]>("sky colours")?;
            let trained = flags(&r.next::<u32>("sky trained")?, texels);
            let (few1, few2) = (r.next::<[f32; 4]>("sky moment 1")?, r.next::<[f32; 4]>("sky moment 2")?);
            let count = header.sky_trained as usize;
            ensure!(rgb.len() == texels && few1.len() == count && few2.len() == count && trained.iter().filter(|t| **t).count() == count, "a sky map of another size");
            let spread = |few: &[[f32; 4]]| -> Vec<[f32; 4]> {
                let mut values = few.iter();
                trained.iter().map(|t| if *t { *values.next().expect("counted") } else { [0.0; 4] }).collect()
            };
            Some(SkyLearningState { width: header.sky_width, texels: rgb.iter().map(|c| [c[0], c[1], c[2], 0.0]).collect(), m1: spread(&few1), m2: spread(&few2) })
        } else {
            None
        };
        Start::Steps(Box::new(StepState {
            iteration: header.iteration,
            iterations: header.iterations,
            gaussians: n,
            gpu_step: header.gpu_step,
            rng: header.rng,
            noise: f32::from_bits(header.noise),
            opacity_reg: f32::from_bits(header.opacity_reg),
            scale_reg: f32::from_bits(header.scale_reg),
            last_round: header.last_round,
            fading: header.fading,
            fade_again: header.fade_again,
            seconds: header.seconds,
            psnr: header.psnr,
            buffers,
            sky,
        }))
    };
    let mut rest = [0u8; 1];
    ensure!(r.f.read(&mut rest)? == 0, "more than its sections say");
    ensure!(format!("{:016x}", r.sum.0) == header.checksum, "its checksum differs: the file is damaged");
    Ok(Checkpoint { extras, start })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-checkpoint-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn steps() -> Start {
        let n = 3;
        let floats = |k: f32, c: usize| (0..c).map(|i| k + i as f32 * 0.5).collect::<Vec<f32>>();
        // A sky map of 4 × 2 texels, of which the second and fourth were trained.
        let touched = |k: f32| (0..8).map(|i| if i == 1 || i == 3 { [k, k + 1.0, k + 2.0, 0.0] } else { [0.0; 4] }).collect::<Vec<_>>();
        Start::Steps(Box::new(StepState {
            iteration: 1234,
            iterations: 9000,
            gaussians: n,
            gpu_step: 1000,
            rng: 0xdead_beef_0123_4567,
            noise: 5e5,
            opacity_reg: 0.1,
            scale_reg: 0.2,
            last_round: Some((7, 9)),
            fading: false,
            fade_again: true,
            seconds: 12.5,
            psnr: Some(30.0),
            buffers: TrainingBuffers { params: floats(1.0, n * PACKED), m1: floats(2.0, n * PACKED), m2: floats(3.0, n * PACKED), stats: floats(4.0, n * 8) },
            sky: Some(SkyLearningState { width: 4, texels: (0..8).map(|i| [i as f32, 0.5, 0.25, 0.0]).collect(), m1: touched(1.0), m2: touched(2.0) }),
        }))
    }

    fn extras() -> Extras {
        Extras {
            budget: 2_000_000,
            starved: true,
            sky_fill: false,
            portraits: vec![Portrait { view: 3, x0: 1.0, y0: 2.0, x1: 3.0, y1: 4.0, distance: 5.0, time: 6.0, still: true }],
            sky_seen: vec![true, false, false, true, false, false, false, true],
            trained_sky: None,
        }
    }

    #[test]
    fn a_paused_run_comes_back_as_it_was_written() {
        let dir = session("steps");
        let bytes = write(&dir, "the key", &steps(), &extras()).unwrap();
        assert_eq!(std::fs::metadata(file(&dir)).unwrap().len(), bytes);
        let header = matching_header(&dir, "the key").unwrap();
        assert_eq!((header.iteration, header.iterations, header.gaussians, header.trained), (1234, 9000, 3, false));
        assert_eq!(peek(&dir).as_ref(), Some(&header));
        let cp = read_body(&dir, &header).unwrap();
        assert_eq!(cp.extras, extras());
        let (Start::Steps(read), Start::Steps(wrote)) = (cp.start, steps()) else { panic!("in the middle of its steps") };
        assert_eq!((read.iteration, read.gpu_step, read.rng, read.last_round, read.fading, read.fade_again), (1234, 1000, wrote.rng, Some((7, 9)), false, true));
        assert_eq!((read.noise.to_bits(), read.opacity_reg.to_bits(), read.scale_reg.to_bits()), (wrote.noise.to_bits(), wrote.opacity_reg.to_bits(), wrote.scale_reg.to_bits()));
        assert_eq!((read.buffers.params, read.buffers.m1, read.buffers.m2, read.buffers.stats), (wrote.buffers.params, wrote.buffers.m1, wrote.buffers.m2, wrote.buffers.stats));
        let (r, w) = (read.sky.unwrap(), wrote.sky.unwrap());
        assert_eq!((r.width, r.texels, r.m1, r.m2), (w.width, w.texels, w.m1, w.m2));
        discard(&dir);
        assert!(!exists(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_run_paused_after_its_steps_keeps_its_final_set_and_sky() {
        let dir = session("trained");
        let mut g = Gaussians::default();
        g.push([1.0, 2.0, 3.0], [0.1, 0.2, 0.3], [1.0, 0.0, 0.0, 0.0], 0.5, [0.4, 0.5, 0.6]);
        g.sky[0] = true;
        let state = Start::Trained(Box::new(TrainedState { gaussians: g.clone(), iterations: 900, seconds: 40.0, psnr: None }));
        let sky = (4, vec![[0.25, 0.5, 0.75]; 8]);
        let e = Extras { trained_sky: Some(sky.clone()), ..extras() };
        write(&dir, "k", &state, &e).unwrap();
        let header = peek(&dir).unwrap();
        assert!(header.trained);
        assert_eq!((header.iteration, header.iterations), (900, 900));
        let cp = read_body(&dir, &header).unwrap();
        assert_eq!(cp.extras.trained_sky, Some(sky));
        let Start::Trained(t) = cp.start else { panic!("its steps are done") };
        assert_eq!((t.gaussians, t.iterations, t.seconds), (g, 900, 40.0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Another key (the worker updated, the session changed, other settings) and rubbish: gone, and the run starts over.
    #[test]
    fn a_stale_or_damaged_checkpoint_is_discarded_without_harm() {
        let dir = session("stale");
        write(&dir, "the key", &steps(), &extras()).unwrap();
        assert!(matching_header(&dir, "another").is_none());
        assert!(!exists(&dir), "discarded");
        assert!(matching_header(&dir, "another").is_none(), "none at all is no failure");
        std::fs::write(file(&dir), b"not a checkpoint").unwrap();
        assert!(matching_header(&dir, "the key").is_none());
        assert!(!exists(&dir), "discarded");
        // A byte of the body flipped, cut short, or longer than it says: the header reads, the body does not.
        write(&dir, "the key", &steps(), &extras()).unwrap();
        let good = std::fs::read(file(&dir)).unwrap();
        let header = peek(&dir).unwrap();
        let last = good.len() - 1;
        let mut bad = good.clone();
        bad[last] ^= 1;
        std::fs::write(file(&dir), &bad).unwrap();
        assert!(format!("{:#}", read_body(&dir, &header).err().unwrap()).contains("checksum"));
        std::fs::write(file(&dir), &good[..last]).unwrap();
        assert!(read_body(&dir, &header).is_err());
        let mut long = good.clone();
        long.push(0);
        std::fs::write(file(&dir), &long).unwrap();
        assert!(read_body(&dir, &header).is_err());
        std::fs::write(file(&dir), &good).unwrap();
        assert!(read_body(&dir, &header).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A crash while writing leaves a file beside the checkpoint: the previous one still answers, the next start clears
    /// what was cut short, and one writer's checkpoint replaces another's whole.
    #[test]
    fn a_write_cut_short_leaves_the_previous_checkpoint_whole() {
        let dir = session("crash");
        write(&dir, "k", &steps(), &extras()).unwrap();
        let before = peek(&dir).unwrap();
        let cut = dir.join(format!("{}.999.tmp", rec::CHECKPOINT_FILE));
        std::fs::write(&cut, b"half a checkpoint").unwrap();
        assert_eq!(matching_header(&dir, "k").as_ref(), Some(&before));
        assert!(read_body(&dir, &before).is_ok());
        sweep(&dir);
        assert!(!cut.exists());
        assert!(exists(&dir), "the checkpoint stays");
        write(&dir, "k", &steps(), &extras()).unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "one file a session, however often it paused");
        std::fs::write(&cut, b"half a checkpoint").unwrap();
        discard(&dir);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "stopped: nothing left");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_key_follows_the_recording_and_the_settings() {
        let dir = session("key");
        use orbiters_core::contracts::manifest;
        std::fs::write(dir.join(manifest::MANIFEST_FILE), b"manifest").unwrap();
        std::fs::write(dir.join(manifest::FRAMES_FILE), b"frames").unwrap();
        let k = key(&dir, "settings").unwrap();
        assert_eq!(key(&dir, "settings").unwrap(), k);
        assert_ne!(key(&dir, "other settings").unwrap(), k);
        std::fs::write(dir.join(manifest::FRAMES_FILE), b"frames again").unwrap();
        assert_ne!(key(&dir, "settings").unwrap(), k);
        std::fs::remove_file(dir.join(manifest::FRAMES_FILE)).unwrap();
        assert!(key(&dir, "settings").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_lab_pauses_once_at_each_point() {
        let lab = Lab { at: vec![0.5, 800.0], stage: None };
        assert!(lab.pause_at_step(500, 0, 1000) && lab.pause_at_step(800, 500, 1000));
        assert!(!lab.pause_at_step(500, 500, 1000), "just resumed from there");
        assert!(!lab.pause_at_step(499, 0, 1000));
    }
}
