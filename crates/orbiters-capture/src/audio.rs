//! Audio of a session (contracts `Docs/performance.md`): the person's microphone and what VRChat plays, each
//! captured with WASAPI on its own thread and written to a 32-bit float WAVE file by another, so a slow disk never
//! makes the capture late. Every packet carries the QueryPerformanceCounter time of its first frame, which places it
//! on the session clock: `sync` keeps one (frame, time) pair a second, so the device clock's slow drift against the
//! host's is recorded rather than corrected, and a gap (loopback delivers nothing while VRChat is silent, a device
//! that went away and came back) is filled with silence so the file plays in step with the session. A device that
//! goes away is opened again, and the default microphone follows Windows' default. VRChat's sound comes from process
//! loopback (Windows 11), so music players, voice chat and notifications are not recorded.

use orbiters_audio::{Ended, Options};
use orbiters_contracts::performance::audio::{self, Track, TrackKind};
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub use orbiters_audio::{list_mics, AudioSource};

/// A jump of the packets' times by more than this since the previous packet is a gap, filled with silence; less is
/// timing jitter. Slow drift never jumps: it stays in the sync pairs.
const GAP_NS: u64 = 20_000_000;

/// The longest gap filled with silence. Past it (the machine slept) the sync pairs place what follows: an hour of
/// zeros would be 1.4 GB.
const MAX_FILL_NS: u64 = 60_000_000_000;

/// How much sound the device keeps for a capture thread that is late (a busy machine).
const DEVICE_BUFFER: Duration = Duration::from_millis(500);

/// Packets waiting for the writer thread: about ten seconds of them. A writer later than that loses packets, whose
/// gap is then filled with silence like any other.
const WRITER_QUEUE: usize = 1000;

/// How often a track's header gets the frames written so far ([`TrackWriter::checkpoint`]).
const CHECKPOINT_EVERY: Duration = Duration::from_secs(2);

/// The track as its last checkpoint left it on disk, for `audio.json` while recording (`file`, `kind` and `source`
/// are the recorder's).
type Published = Arc<Mutex<Option<Track>>>;

/// What the device said about a packet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Flags {
    /// Sound was lost just before it (the capture thread was late).
    discontinuity: bool,
    /// Its time is not to be trusted.
    timestamp_error: bool,
}

/// Places packets on the session clock and writes them (format-independent, so it is tested without a device).
struct TrackWriter {
    file: BufWriter<File>,
    rate: u32,
    channels: u16,
    frames: u64,
    first_ns: Option<u64>,
    sync: Vec<[i64; 2]>,
    last_sync_ns: u64,
    /// The last packet's time less the time the frames before it take at the nominal rate: the device clock's drift
    /// so far.
    offset_ns: i64,
    zeros: Vec<f32>,
    published: Published,
    checkpointed: Instant,
}

impl TrackWriter {
    fn create(path: &Path, rate: u32, channels: u16, published: Published) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = BufWriter::with_capacity(1 << 16, File::create(path)?);
        file.write_all(&audio::wav_header(rate, channels, 0))?;
        Ok(Self {
            file,
            rate,
            channels,
            frames: 0,
            first_ns: None,
            sync: Vec::new(),
            last_sync_ns: 0,
            offset_ns: 0,
            zeros: Vec::new(),
            published,
            checkpointed: Instant::now(),
        })
    }

    /// The time `frames` frames take at the nominal rate.
    fn nominal_ns(&self, frames: u64) -> u64 {
        (frames as u128 * 1_000_000_000 / self.rate as u128) as u64
    }

    fn write_zeros(&mut self, frames: u64) -> anyhow::Result<()> {
        let mut left = frames * self.channels as u64;
        while left > 0 {
            let n = left.min(48_000) as usize;
            if self.zeros.len() < n {
                self.zeros.resize(n, 0.0);
            }
            self.file.write_all(bytemuck_f32(&self.zeros[..n]))?;
            left -= n as u64;
        }
        Ok(())
    }

    /// One packet whose first frame was captured at session time `t_ns`; `samples` is `None` for silence.
    fn packet(&mut self, t_ns: u64, frames: u32, samples: Option<&[f32]>, flags: Flags) -> anyhow::Result<()> {
        let first = *self.first_ns.get_or_insert(t_ns);
        let expected = first + self.nominal_ns(self.frames);
        // A packet the device could not time follows the last one, at the drift so far.
        let t = if flags.timestamp_error { (expected as i64 + self.offset_ns).max(0) as u64 } else { t_ns };
        let offset = t as i64 - expected as i64;
        let jump = offset - self.offset_ns;
        if jump > GAP_NS as i64 || (flags.discontinuity && jump > 0) {
            // Nothing came for a while (or sound was lost to a late reader): that much silence, the drift before it
            // kept.
            let fill_ns = (jump as u64).min(MAX_FILL_NS);
            if fill_ns < jump as u64 {
                // Longer than is filled: where the sound stopped, so the pairs place what follows by its time.
                self.sync.push([self.frames as i64, expected as i64 + self.offset_ns]);
            }
            let missing = fill_ns * self.rate as u64 / 1_000_000_000;
            self.write_zeros(missing)?;
            self.frames += missing;
            self.sync.push([self.frames as i64, t as i64]);
            self.last_sync_ns = t;
            self.offset_ns = t as i64 - (first + self.nominal_ns(self.frames)) as i64;
        } else {
            self.offset_ns = offset;
            if self.sync.is_empty() || t >= self.last_sync_ns + 1_000_000_000 {
                self.sync.push([self.frames as i64, t as i64]);
                self.last_sync_ns = t;
            }
        }
        match samples {
            Some(s) => self.file.write_all(bytemuck_f32(&s[..frames as usize * self.channels as usize]))?,
            None => self.write_zeros(frames as u64)?,
        }
        self.frames += frames as u64;
        if self.checkpointed.elapsed() >= CHECKPOINT_EVERY {
            self.checkpoint()?;
        }
        Ok(())
    }

    /// Makes the file readable as it stands: its header gets the frames written so far, and `published` the track
    /// as `audio.json` gives it (the session writes that file at its own checkpoints). A recording that never
    /// finishes (the worker killed, Windows shut down) keeps its sound up to here.
    fn checkpoint(&mut self) -> anyhow::Result<()> {
        self.checkpointed = Instant::now();
        self.file.flush()?;
        let f = self.file.get_mut();
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&audio::wav_header(self.rate, self.channels, self.frames))?;
        f.seek(SeekFrom::End(0))?;
        *self.published.lock().unwrap() = Some(self.track());
        Ok(())
    }

    /// The track so far, without its file, kind and source.
    fn track(&self) -> Track {
        Track {
            sample_rate: self.rate,
            channels: self.channels,
            frames: self.frames,
            first_frame_ns: self.first_ns.unwrap_or(0) as i64,
            sync: self.sync.clone(),
            ..Default::default()
        }
    }

    fn finish(mut self, file: String, kind: TrackKind, source: String) -> anyhow::Result<Track> {
        self.file.flush()?;
        let track = self.track();
        let mut f = self.file.into_inner().map_err(|e| e.into_error())?;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&audio::wav_header(self.rate, self.channels, self.frames))?;
        Ok(Track { file, kind, source, ..track })
    }
}

fn bytemuck_f32(s: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding and any byte pattern is a valid u8; WAVE data is little-endian like the host.
    unsafe { std::slice::from_raw_parts(s.as_ptr().cast::<u8>(), std::mem::size_of_val(s)) }
}

/// A packet on its way to the writer thread: its time on the session clock, its frames, its samples (`None`:
/// silence) and what the device said about it.
struct ToWrite {
    t_ns: u64,
    frames: u32,
    samples: Option<Vec<f32>>,
    flags: Flags,
}

/// The writer thread's queue and handle.
type WriterThread = (crossbeam_channel::Sender<ToWrite>, JoinHandle<anyhow::Result<TrackWriter>>);

/// Writes packets as they come; the writer back when the capture thread is done with it.
fn write_packets(mut writer: TrackWriter, packets: crossbeam_channel::Receiver<ToWrite>) -> anyhow::Result<TrackWriter> {
    for p in packets {
        writer.packet(p.t_ns, p.frames, p.samples.as_deref(), p.flags)?;
    }
    Ok(writer)
}

/// Reopening a device that went away: the first pause, and the longest while it stays away.
const REOPEN_FIRST: Duration = Duration::from_millis(500);
const REOPEN_MOST: Duration = Duration::from_secs(5);

/// Opens a device and captures it until told to stop: [`orbiters_audio::capture_packets`], a stand-in in tests.
type Capture = Box<
    dyn FnMut(
            &AudioSource,
            &Options,
            &AtomicBool,
            &mut dyn FnMut(&str, orbiters_audio::Format) -> anyhow::Result<()>,
            &mut dyn FnMut(&orbiters_audio::Packet<'_>) -> anyhow::Result<()>,
        ) -> anyhow::Result<(String, Ended)>
        + Send,
>;

fn wasapi() -> Capture {
    Box::new(|source, options, stop, ready, packet| orbiters_audio::capture_packets(source, options, stop, ready, packet))
}

/// What the recorder asks of the capture thread. Changed under its lock together with `interrupt`, and taken by
/// the thread under the same lock, so that no request is lost between the two.
struct Control {
    /// The source to capture: the game track follows VRChat when it restarts.
    source: AudioSource,
    /// `source` changed: the device is opened again.
    retarget: bool,
    stop: bool,
}

/// What the capture thread shares with the recorder.
struct Shared {
    control: Mutex<Control>,
    /// Ends the capture under way (a stop, or a new source to open), looked at by the capture loop.
    interrupt: AtomicBool,
    paused: AtomicBool,
    /// Peak level since the last [`AudioRecorder::take_level`], as f32 bits.
    level: AtomicU32,
    /// What happened to the device since the last look (it went away, it is recorded again).
    trouble: Mutex<Option<String>>,
    /// The device recorded now.
    name: Mutex<String>,
}

impl Shared {
    /// Asks the capture thread to stop.
    fn stop(&self) {
        let mut control = self.control.lock().unwrap();
        control.stop = true;
        self.interrupt.store(true, Ordering::Relaxed);
    }
}

/// A track being recorded. Dropping it without [`AudioRecorder::finish`] stops the threads too.
pub struct AudioRecorder {
    pub kind: TrackKind,
    pub source_name: String,
    /// Relative to the bundle (`audio/mic.wav`).
    file: String,
    published: Published,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<anyhow::Result<Track>>>,
}

impl AudioRecorder {
    /// Opens the device and starts recording into `<dir>/audio/<kind>.wav`; `epoch_ns` is the session start on
    /// [`orbiters_core::clock`]. Fails when the device cannot be opened; once it was, a device that goes away is
    /// opened again into the same file, the gap silent.
    pub fn start(kind: TrackKind, source: AudioSource, dir: &Path, epoch_ns: u64) -> anyhow::Result<Self> {
        Self::start_with(kind, source, dir, epoch_ns, wasapi())
    }

    fn start_with(kind: TrackKind, source: AudioSource, dir: &Path, epoch_ns: u64, mut capture: Capture) -> anyhow::Result<Self> {
        let name = match kind {
            TrackKind::Mic => "mic",
            TrackKind::Game => "game",
        };
        let file = format!("{}/{name}.wav", orbiters_contracts::performance::AUDIO_DIR);
        let path = dir.join(&file);
        let shared = Arc::new(Shared {
            control: Mutex::new(Control { source, retarget: false, stop: false }),
            interrupt: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            level: AtomicU32::new(0),
            trouble: Mutex::new(None),
            name: Mutex::new(String::new()),
        });
        let published = Published::default();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = {
            let (shared, published, file, kind_name) = (shared.clone(), published.clone(), file.clone(), name);
            std::thread::Builder::new().name(format!("audio-{name}")).spawn(move || {
                let result = run(&shared, &path, epoch_ns, &published, &ready_tx, kind_name, &mut capture);
                if let Err(e) = &result {
                    let _ = ready_tx.try_send(Err(format!("{e:#}")));
                }
                let (writer, source_name) = result?;
                writer.finish(file, kind, source_name)
            })?
        };
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(source_name)) => Ok(Self { kind, source_name, file, published, shared, thread: Some(thread) }),
            Ok(Err(e)) => {
                let _ = thread.join();
                anyhow::bail!("{e}")
            }
            Err(_) => {
                shared.stop();
                anyhow::bail!("the audio device did not start")
            }
        }
    }

    /// While paused, silence is written instead of the sound (the timeline stays continuous).
    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    /// Peak level (0–1) since the last call.
    pub fn take_level(&self) -> f32 {
        f32::from_bits(self.shared.level.swap(0, Ordering::Relaxed))
    }

    /// What happened to the device since the last call (it went away, it is recorded again), for the person.
    pub fn take_trouble(&self) -> Option<String> {
        self.shared.trouble.lock().unwrap().take()
    }

    /// Records `source` from now on, into the same file (VRChat restarted as another process).
    pub fn retarget(&self, source: AudioSource) {
        let mut control = self.shared.control.lock().unwrap();
        if control.source == source {
            return;
        }
        (control.source, control.retarget) = (source, true);
        self.shared.interrupt.store(true, Ordering::Relaxed);
    }

    /// The track as its file on disk holds it (its last checkpoint), for `audio.json` while recording; `None`
    /// before the first.
    pub fn track(&self) -> Option<Track> {
        let track = self.published.lock().unwrap().clone()?;
        let source = self.shared.name.lock().unwrap().clone();
        Some(Track { file: self.file.clone(), kind: self.kind, source, ..track })
    }

    /// Stops recording and completes the file.
    pub fn finish(mut self) -> anyhow::Result<Track> {
        self.shared.stop();
        match self.thread.take().expect("running").join() {
            Ok(r) => r,
            Err(_) => anyhow::bail!("the audio thread panicked"),
        }
    }
}

impl Drop for AudioRecorder {
    fn drop(&mut self) {
        self.shared.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Records until stopped, opening the device again whenever it goes away; returns the writer to complete and the
/// last source's name. `what` names the track in messages ("mic", "game").
fn run(
    shared: &Shared,
    path: &Path,
    epoch_ns: u64,
    published: &Published,
    ready: &mpsc::SyncSender<Result<String, String>>,
    what: &str,
    capture: &mut Capture,
) -> anyhow::Result<(TrackWriter, String)> {
    // Each message is written whole for each track: a sentence cannot be built from the name of its subject in every language.
    let mic = what == "mic";
    // The writer thread, once the device first opened (its rate and channels make the file's), and its queue. Both
    // callbacks reach it, one after the other.
    let writer: std::cell::RefCell<Option<WriterThread>> = Default::default();
    let mut format: Option<orbiters_audio::Format> = None;
    let mut last_name = String::new();
    let mut pause = REOPEN_FIRST;
    let mut lost = 0u64;
    let mut failed: Option<anyhow::Error> = None;
    loop {
        // What was asked, taken under the lock that asking takes: a stop, or a new source to open (its interrupt is
        // cleared with it, and only then).
        let source = {
            let mut control = shared.control.lock().unwrap();
            if control.stop {
                break;
            }
            if std::mem::take(&mut control.retarget) {
                shared.interrupt.store(false, Ordering::Relaxed);
            }
            control.source.clone()
        };
        let options = Options {
            // Opened again, the device converts to the file's rate.
            rate: format.map(|f| f.rate),
            buffer: DEVICE_BUFFER,
            pro_audio: true,
            follow_default: matches!(source, AudioSource::Mic(None)),
        };
        let reopened = writer.borrow().is_some();
        let result = capture(
            &source,
            &options,
            &shared.interrupt,
            &mut |name, f| {
                if let Some(first) = format {
                    anyhow::ensure!(f.channels == first.channels, "{name} gives {} channels, the track has {}", f.channels, first.channels);
                } else {
                    let track = TrackWriter::create(path, f.rate, f.channels, published.clone())?;
                    let (tx, rx) = crossbeam_channel::bounded::<ToWrite>(WRITER_QUEUE);
                    let thread = std::thread::Builder::new().name("audio-writer".into()).spawn(move || write_packets(track, rx))?;
                    *writer.borrow_mut() = Some((tx, thread));
                    format = Some(f);
                    let _ = ready.try_send(Ok(name.to_string()));
                }
                last_name = name.to_string();
                *shared.name.lock().unwrap() = last_name.clone();
                pause = REOPEN_FIRST;
                if reopened {
                    *shared.trouble.lock().unwrap() = Some(if mic { orbiters_i18n::tr!("The microphone is recorded again ({name}).", name = name) } else { orbiters_i18n::tr!("VRChat's sound is recorded again ({name}).", name = name) });
                }
                Ok(())
            },
            &mut |p| {
                let writer = writer.borrow();
                let Some((tx, _)) = writer.as_ref() else { anyhow::bail!("no track to write") };
                let samples = match p.samples {
                    // While paused, silence keeps the timeline continuous.
                    Some(samples) if !shared.paused.load(Ordering::Relaxed) => {
                        // A level of at most 1 (NaN never wins `max`; an infinite sample from a broken driver would
                        // reach the stats as null).
                        let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs())).min(1.0);
                        // Non-negative floats order like their bits.
                        shared.level.fetch_max(peak.to_bits(), Ordering::Relaxed);
                        Some(samples.to_vec())
                    }
                    _ => None,
                };
                let flags = Flags { discontinuity: p.discontinuity, timestamp_error: p.timestamp_error };
                match tx.try_send(ToWrite { t_ns: p.at_ns.saturating_sub(epoch_ns), frames: p.frames, samples, flags }) {
                    Ok(()) => Ok(()),
                    // The writer is ten seconds behind: this packet's gap will be silent.
                    Err(crossbeam_channel::TrySendError::Full(_)) => {
                        lost += 1;
                        Ok(())
                    }
                    Err(crossbeam_channel::TrySendError::Disconnected(_)) => anyhow::bail!("the track's writer stopped"),
                }
            },
        );
        {
            let control = shared.control.lock().unwrap();
            if control.stop {
                break;
            }
            if control.retarget {
                // Another source (VRChat restarted): opened right away.
                continue;
            }
        }
        let why = match result {
            // Not even the first time: nothing to record into.
            Err(e) if writer.borrow().is_none() => return Err(e),
            Err(e) if mic => orbiters_i18n::tr!("The microphone could not be opened again ({error}): tried again shortly.", error = format!("{e:#}")),
            Err(e) => orbiters_i18n::tr!("VRChat's sound could not be opened again ({error}): tried again shortly.", error = format!("{e:#}")),
            Ok((name, Ended::Lost(e))) if mic => orbiters_i18n::tr!("The microphone went away ({name}: {error}): recorded again as soon as it comes back.", name = name, error = e),
            Ok((name, Ended::Lost(e))) => orbiters_i18n::tr!("VRChat's sound went away ({name}: {error}): recorded again as soon as it comes back.", name = name, error = e),
            Ok((_, Ended::DefaultChanged)) => {
                eprintln!("audio: the system's default microphone changed");
                pause = Duration::ZERO;
                if cfg!(target_os = "macos") { orbiters_i18n::tr!("The microphone follows the Mac's default microphone, which changed.") } else { orbiters_i18n::tr!("The microphone follows Windows' default microphone, which changed.") }
            }
            Ok((_, Ended::Refused(e))) => {
                // The writer stopped (the disk): its error ends this track.
                let error = writer.borrow_mut().take().map(|(tx, thread)| {
                    drop(tx);
                    thread.join().map_err(|_| anyhow::anyhow!("the track's writer panicked")).and_then(|r| r.map(|_| ()))
                });
                let e = match error {
                    Some(Err(e)) => e,
                    _ => anyhow::anyhow!("{e}"),
                };
                *shared.trouble.lock().unwrap() = Some(if mic { orbiters_i18n::tr!("The microphone is not recorded any more: {error}", error = format!("{e:#}")) } else { orbiters_i18n::tr!("VRChat's sound is not recorded any more: {error}", error = format!("{e:#}")) });
                failed = Some(e);
                break;
            }
            // Only a stop or a new source interrupts a capture, and neither is pending: nothing more to record.
            Ok((_, Ended::Stopped)) => break,
        };
        eprintln!("audio: {why}");
        *shared.trouble.lock().unwrap() = Some(why);
        // Wait before the next attempt, ready to stop or to switch meanwhile.
        let until = Instant::now() + pause;
        while Instant::now() < until && !shared.interrupt.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
        }
        pause = (pause * 2).clamp(REOPEN_FIRST, REOPEN_MOST);
    }
    if lost > 0 {
        eprintln!("audio: {lost} packet(s) left out while the writer was behind (the gaps are silent)");
    }
    if let Some(e) = failed {
        return Err(e);
    }
    let (tx, thread) = writer.into_inner().ok_or_else(|| anyhow::anyhow!("the audio device did not start"))?;
    drop(tx);
    let writer = thread.join().map_err(|_| anyhow::anyhow!("the track's writer panicked"))??;
    Ok((writer, last_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const PLAIN: Flags = Flags { discontinuity: false, timestamp_error: false };

    fn temp_track(name: &str, rate: u32, channels: u16) -> (PathBuf, TrackWriter) {
        let dir = std::env::temp_dir().join(format!("orbiters-audio-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("audio/track.wav");
        let w = TrackWriter::create(&path, rate, channels, Published::default()).unwrap();
        (dir, w)
    }

    #[test]
    fn packets_land_on_the_session_clock() {
        let dir = std::env::temp_dir().join(format!("orbiters-audio-{}", std::process::id()));
        let path = dir.join("audio/mic.wav");
        let mut w = TrackWriter::create(&path, 1000, 1, Published::default()).unwrap();
        let tone: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        // Continuous 100 ms packets from t = 5 ms (the second one silent): a sync point a second.
        for i in 0..12u64 {
            w.packet(5_000_000 + i * 100_000_000, 100, (i != 1).then_some(tone.as_slice()), PLAIN).unwrap();
        }
        // Then 300 ms without packets (loopback while VRChat is silent): filled with zeros.
        w.packet(1_505_000_000, 100, Some(&tone), PLAIN).unwrap();
        let track = w.finish("audio/mic.wav".into(), TrackKind::Mic, "Test mic".into()).unwrap();
        assert_eq!(track.first_frame_ns, 5_000_000);
        assert_eq!(track.frames, 1600);
        assert_eq!(track.sync, vec![[0, 5_000_000], [1000, 1_005_000_000], [1500, 1_505_000_000]]);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..58], &audio::wav_header(1000, 1, 1600));
        assert_eq!(bytes.len(), 58 + 1600 * 4);
        let sample = |i: usize| f32::from_le_bytes(bytes[58 + i * 4..62 + i * 4].try_into().unwrap());
        assert_eq!((sample(50), sample(150), sample(250), sample(1300), sample(1550)), (0.5, 0.0, 0.5, 0.0, 0.5));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_track_that_never_finishes_reads_up_to_its_last_checkpoint() {
        let dir = std::env::temp_dir().join(format!("orbiters-audio-unfinished-{}", std::process::id()));
        let path = dir.join("audio/game.wav");
        let published = Published::default();
        let mut w = TrackWriter::create(&path, 1000, 2, published.clone()).unwrap();
        let tone = vec![0.25f32; 200];
        for i in 0..12u64 {
            w.packet(i * 100_000_000, 100, Some(&tone), PLAIN).unwrap();
        }
        assert!(published.lock().unwrap().is_none(), "no checkpoint yet");
        w.checkpoint().unwrap();
        // Read while recording (or after the worker was killed): the header covers what is on disk.
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..58], &audio::wav_header(1000, 2, 1200));
        assert_eq!(bytes.len(), 58 + 1200 * 2 * 4);
        let track = published.lock().unwrap().clone().unwrap();
        assert_eq!((track.frames, track.sample_rate, track.channels, track.sync.len()), (1200, 1000, 2, 2));
        // More sound after the checkpoint: the file grows past its header, which still reads as the first 1.2 s.
        w.packet(1_200_000_000, 100, Some(&tone), PLAIN).unwrap();
        w.file.flush().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!((&bytes[..58], bytes.len()), (&audio::wav_header(1000, 2, 1200)[..], 58 + 1300 * 2 * 4));
        // Every couple of seconds on its own.
        w.checkpointed = Instant::now().checked_sub(CHECKPOINT_EVERY).unwrap();
        w.packet(1_300_000_000, 100, Some(&tone), PLAIN).unwrap();
        assert_eq!(published.lock().unwrap().as_ref().map(|t| t.frames), Some(1400));
        let track = w.finish("audio/game.wav".into(), TrackKind::Game, "VRChat".into()).unwrap();
        assert_eq!((track.frames, track.file.as_str()), (1400, "audio/game.wav"));
        assert_eq!(&std::fs::read(&path).unwrap()[..58], &audio::wav_header(1000, 2, 1400));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A USB headset's clock 200 ppm slow: its 10 ms packets come 2 µs late each. The drift stays in the sync pairs;
    /// no silence is inserted into continuous sound (it was, 20 ms of it every 100 s).
    #[test]
    fn clock_drift_is_recorded_not_filled() {
        let (dir, mut w) = temp_track("drift", 48_000, 1);
        let tone = vec![0.5f32; 480];
        let packets = 20_000u64; // 200 s
        for i in 0..packets {
            w.packet(1_000_000 + i * 10_002_000, 480, Some(&tone), PLAIN).unwrap();
        }
        assert_eq!(w.frames, packets * 480, "no frame was added");
        // A pair about every second, the last one carrying the 40 ms of drift.
        assert!((195..=205).contains(&w.sync.len()), "{} sync pairs", w.sync.len());
        let [frame, time] = *w.sync.last().unwrap();
        let nominal = 1_000_000 + frame * 1_000_000_000 / 48_000;
        assert!((time - nominal - 39_000_000).abs() < 2_000_000, "drift {} ns", time - nominal);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// One opening of a stand-in device: `packets` packets of 10 ms (1 kHz, mono) from `start_ns`, then the device goes
    /// away (`lost`) or runs until interrupted.
    struct Opening {
        packets: u64,
        start_ns: u64,
        lost: bool,
    }

    /// A stand-in for WASAPI that opens as `openings` say, and notes each source it was asked to open.
    fn stand_in(openings: Vec<Opening>, opened: Arc<Mutex<Vec<AudioSource>>>) -> Capture {
        let mut openings = openings.into_iter();
        Box::new(move |source, _options, stop, ready, packet| {
            let Some(o) = openings.next() else { anyhow::bail!("no such device") };
            opened.lock().unwrap().push(source.clone());
            let name = match source {
                AudioSource::Process(pid) => format!("VRChat (process {pid})"),
                AudioSource::Mic(_) => "Test mic".into(),
            };
            ready(&name, orbiters_audio::Format { rate: 1000, channels: 1 })?;
            let tone = vec![0.5f32; 10];
            for i in 0..o.packets {
                let p = orbiters_audio::Packet {
                    at_ns: o.start_ns + i * 10_000_000,
                    frames: 10,
                    samples: Some(&tone),
                    discontinuity: false,
                    timestamp_error: false,
                };
                packet(&p)?;
            }
            if o.lost {
                return Ok((name, Ended::Lost("the headset was unplugged".into())));
            }
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok((name, Ended::Stopped))
        })
    }

    fn eventually(what: &str, done: impl Fn() -> bool) {
        let until = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < until, "{what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The device goes away and comes back, then VRChat restarts as another process: one file throughout, in step
    /// with the session (the gaps silent), each change told.
    #[test]
    fn a_track_survives_its_device_going_away_and_follows_vrchat() {
        let dir = std::env::temp_dir().join(format!("orbiters-audio-reopen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let epoch = 1_000_000_000;
        let openings = vec![
            // 100 ms of sound, then the device goes away.
            Opening { packets: 10, start_ns: epoch, lost: true },
            // Back a second in, until VRChat restarts.
            Opening { packets: 10, start_ns: epoch + 1_000_000_000, lost: false },
            // The new VRChat, until the end.
            Opening { packets: 10, start_ns: epoch + 2_000_000_000, lost: false },
        ];
        let opened = Arc::new(Mutex::new(Vec::new()));
        let r = AudioRecorder::start_with(TrackKind::Game, AudioSource::Process(1), &dir, epoch, stand_in(openings, opened.clone())).unwrap();
        eventually("opened again", || opened.lock().unwrap().len() == 2);
        eventually("told", || r.take_trouble().is_some_and(|t| t.contains("recorded again")));
        r.retarget(AudioSource::Process(2));
        eventually("the new VRChat", || opened.lock().unwrap().len() == 3);
        // The same process again changes nothing.
        r.retarget(AudioSource::Process(2));
        std::thread::sleep(Duration::from_millis(50));
        let track = r.finish().unwrap();
        assert_eq!(*opened.lock().unwrap(), vec![AudioSource::Process(1), AudioSource::Process(1), AudioSource::Process(2)]);
        assert_eq!(track.source, "VRChat (process 2)");
        // 3 × 100 frames of sound, and 2 × 900 of silence between.
        assert_eq!(track.frames, 2100);
        assert_eq!(std::fs::read(dir.join("audio/game.wav")).unwrap().len(), 58 + 2100 * 4);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gaps_are_silence_long_ones_cut_short() {
        let (dir, mut w) = temp_track("gaps", 1000, 1);
        let tone = vec![0.5f32; 100];
        w.packet(0, 100, Some(&tone), PLAIN).unwrap();
        // 10 ms of sound lost to a late reader: filled, though shorter than a gap.
        w.packet(110_000_000, 100, Some(&tone), Flags { discontinuity: true, ..PLAIN }).unwrap();
        assert_eq!(w.frames, 210);
        // A packet the device could not time follows the last one, wherever its time says.
        w.packet(5_000_000_000_000, 100, Some(&tone), Flags { timestamp_error: true, ..PLAIN }).unwrap();
        assert_eq!(w.frames, 310);
        // Two hours asleep: a minute of silence, and the pairs place what follows.
        let woke = 310_000_000 + 7_200_000_000_000;
        w.packet(woke, 100, Some(&tone), PLAIN).unwrap();
        assert_eq!(w.frames, 310 + 60_000 + 100);
        let pairs = &w.sync[w.sync.len() - 2..];
        assert_eq!(pairs, &[[310, 310_000_000], [60_310, woke as i64]]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
