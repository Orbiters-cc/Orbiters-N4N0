//! Sound devices (WASAPI shared mode on Windows, audio queues on macOS): [`Output`] plays sounds on the default output, [`MicMonitor`]
//! sends a microphone straight to it (the settings' mic test) and [`Duplex`] carries a call's sound both ways.
//! [`Output`] opens the default output for each run of sounds; all three follow a default output that changes while
//! they play (SteamVR switching to the headset) and open again a device that went away. [`Duplex`] also follows the
//! default microphone, and takes it when the chosen one cannot open.
//!
//! The threads of [`MicMonitor`] and [`Duplex`] end when they are dropped, and when whoever started them stopped
//! waiting for their devices: a device that opens late never leaves a thread behind.

use crate::synth::Sound;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// The rate everything is rendered and played at (the system converts to the device's own).
pub const RATE: u32 = 48_000;
/// How long a start waits for its devices.
const START_WAIT: Duration = Duration::from_secs(3);

enum Command {
    Play(Arc<Sound>),
}

/// Plays sounds, mixed, on the default output device. The device is opened when a sound starts and let go a while
/// after the last one ends. Clones share the player.
#[derive(Clone)]
pub struct Output {
    tx: mpsc::Sender<Command>,
}

impl Output {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let _ = std::thread::Builder::new().name("sound-output".into()).spawn(move || imp::output(rx));
        Self { tx }
    }

    pub fn play(&self, sound: &Arc<Sound>) {
        let _ = self.tx.send(Command::Play(sound.clone()));
    }
}

impl Default for Output {
    fn default() -> Self {
        Self::new()
    }
}

/// What a sound thread shares with whoever started it.
struct Shared {
    stop: Arc<AtomicBool>,
    /// The microphone's loudest sample since the last look (its bits).
    level: Arc<AtomicU32>,
    /// What went wrong after the start (a device that failed or went away), while it lasts.
    trouble: Arc<Mutex<Option<String>>>,
    ready: mpsc::SyncSender<Result<(), String>>,
}

impl Shared {
    /// Says the devices are open. False when nobody waits any more (the start gave up): the thread then ends.
    fn ready(&self) -> bool {
        self.ready.send(Ok(())).is_ok() && !self.stop.load(Ordering::Relaxed)
    }

    fn fail(&self, message: String) {
        let _ = self.ready.try_send(Err(message));
    }

    fn trouble(&self, message: Option<String>) {
        *self.trouble.lock().unwrap() = message;
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

/// Sets the stop flag when dropped: a start that failed, or gave up waiting, leaves no thread running.
struct StopOnDrop(Option<Arc<AtomicBool>>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        if let Some(stop) = &self.0 {
            stop.store(true, Ordering::Relaxed);
        }
    }
}

/// A sound thread while it lives.
struct Running {
    stop: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    trouble: Arc<Mutex<Option<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Running {
    /// Starts `run` on a thread and waits up to [`START_WAIT`] for it to say its devices are open.
    fn start(name: &str, run: impl FnOnce(&Shared) + Send + 'static) -> anyhow::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let trouble = Arc::new(Mutex::new(None));
        let (ready, ready_rx) = mpsc::sync_channel(1);
        let mut guard = StopOnDrop(Some(stop.clone()));
        let shared = Shared { stop: stop.clone(), level: level.clone(), trouble: trouble.clone(), ready };
        let thread = std::thread::Builder::new().name(name.into()).spawn(move || run(&shared))?;
        match ready_rx.recv_timeout(START_WAIT) {
            Ok(Ok(())) => {
                guard.0 = None;
                Ok(Self { stop, level, trouble, thread: Some(thread) })
            }
            Ok(Err(e)) => anyhow::bail!("{e}"),
            Err(_) => anyhow::bail!("{}", orbiters_i18n::tr("the sound devices did not open in time")),
        }
    }

    fn take_level(&self) -> f32 {
        f32::from_bits(self.level.swap(0, Ordering::Relaxed))
    }

    fn trouble(&self) -> Option<String> {
        self.trouble.lock().unwrap().clone()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Plays a microphone on the default output while it lives, and measures its level.
pub struct MicMonitor {
    running: Running,
}

impl MicMonitor {
    /// `device`: a microphone id from the capture worker's list, `None` for the system's default. Waits up to 3 s for
    /// the devices; fails when the microphone or the output cannot open.
    pub fn start(device: Option<String>) -> anyhow::Result<Self> {
        let running = Running::start("mic-monitor", move |shared| {
            let queue = Mutex::new(VecDeque::new());
            // Bounded here: with the output gone nothing plays it out (and trims it), and the microphone goes on.
            let most = MONITOR_BUFFERING.keep * 2;
            imp::run(device.as_deref(), shared, &mut |samples| push_newest(&mut queue.lock().unwrap(), samples, most), &queue, MONITOR_BUFFERING, false);
        })?;
        Ok(Self { running })
    }

    /// The loudest sample since the last call (0..1).
    pub fn take_level(&self) -> f32 {
        self.running.take_level()
    }

    /// What went wrong since it started (a device that went away and is being opened again), while it lasts.
    pub fn error(&self) -> Option<String> {
        self.running.trouble()
    }
}

/// A call's audio while it lives: the microphone handed to `mic` (mono, [`RATE`], as it comes) and `speaker` (mono,
/// [`RATE`], filled by the caller) played on the default output, through an adaptive jitter buffer (60 ms to start,
/// more after the queue ran dry, a backlog beyond 300 ms dropped).
pub struct Duplex {
    running: Running,
}

impl Duplex {
    /// Waits up to 3 s for the devices. A chosen microphone that cannot open gives way to the default one, and a call
    /// with no microphone or no output at all goes on: [`Duplex::error`] says what is missing meanwhile.
    pub fn start(device: Option<String>, mut mic: impl FnMut(&[f32]) + Send + 'static, speaker: Arc<Mutex<VecDeque<f32>>>) -> anyhow::Result<Self> {
        let running = Running::start("call-audio", move |shared| imp::run(device.as_deref(), shared, &mut mic, &speaker, CALL_BUFFERING, true))?;
        Ok(Self { running })
    }

    /// The microphone's loudest sample since the last call (0..1).
    pub fn take_level(&self) -> f32 {
        self.running.take_level()
    }

    /// What is wrong with the devices now (the chosen microphone replaced by the default one, a device that went
    /// away and is being opened again), if anything.
    pub fn error(&self) -> Option<String> {
        self.running.trouble()
    }
}

/// How a speaker plays a queue filled from elsewhere (the network, a microphone), in samples: it waits for `start`
/// before it plays and again after the queue ran dry (a little longer each time, up to `most`); a backlog beyond
/// `keep` goes at once.
#[derive(Clone, Copy, Debug)]
struct Buffering {
    start: usize,
    most: usize,
    keep: usize,
}

const CALL_BUFFERING: Buffering = Buffering { start: RATE as usize * 6 / 100, most: RATE as usize / 5, keep: RATE as usize * 3 / 10 };
/// Adds `samples` to `queue`, keeping at most `most` of the newest (a slice longer than that adds only its end).
fn push_newest(queue: &mut VecDeque<f32>, samples: &[f32], most: usize) {
    let samples = &samples[samples.len().saturating_sub(most)..];
    let over = (queue.len() + samples.len()).saturating_sub(most);
    queue.drain(..over.min(queue.len()));
    queue.extend(samples.iter().copied());
}

const MONITOR_BUFFERING: Buffering = Buffering { start: RATE as usize / 50, most: RATE as usize * 6 / 100, keep: RATE as usize * 8 / 100 };
/// What each run dry adds to the target, and what a calm while takes back.
const GROW: usize = RATE as usize / 50;
const SHRINK: usize = RATE as usize / 100;
/// Samples played without running dry before the target comes down a step.
const CALM: usize = RATE as usize * 20;
/// At most one sample in this many is left out or repeated (0.5 %: not heard).
const ADJUST_EVERY: usize = 200;

/// The speaker's side of a queue (an adaptive jitter buffer). What waits is kept near a target that grows each time
/// the queue ran dry and comes back down after a calm while. Above the target a sample now and then is left out,
/// below half of it one is played twice, each where the sound is quietest: a clock a little faster or slower than
/// the other side's is absorbed without a click, and so is a burst after a late packet.
struct Playout {
    buffering: Buffering,
    target: usize,
    primed: bool,
    calm: usize,
    scratch: Vec<f32>,
}

impl Playout {
    fn new(buffering: Buffering) -> Self {
        Self { buffering, target: buffering.start, primed: false, calm: 0, scratch: Vec::new() }
    }

    /// Fills `out` (interleaved stereo) from `queue` (mono).
    fn fill(&mut self, queue: &mut VecDeque<f32>, out: &mut [f32]) {
        let frames = out.len() / 2;
        if queue.len() > self.buffering.keep {
            // A backlog: all but the target goes, the oldest first.
            let excess = queue.len() - self.target;
            queue.drain(..excess);
        }
        if !self.primed {
            if queue.len() < self.target.max(frames) {
                out.fill(0.0);
                return;
            }
            self.primed = true;
        }
        if queue.len() < frames {
            // Ran dry: what is left plays, then silence until the target is back (a little more of it from now on).
            for frame in out.as_chunks_mut::<2>().0 {
                let v = queue.pop_front().unwrap_or(0.0);
                frame[0] = v;
                frame[1] = v;
            }
            self.primed = false;
            self.target = (self.target + GROW).min(self.buffering.most);
            self.calm = 0;
            return;
        }
        self.calm += frames;
        if self.calm >= CALM {
            self.calm = 0;
            self.target = self.target.saturating_sub(SHRINK).max(self.buffering.start);
        }
        // What would wait after this: above the target by more than a block, a little faster; below half, slower.
        let after = queue.len() - frames;
        let step = (frames / ADJUST_EVERY).max(1);
        let take = if after > self.target + frames {
            frames + step.min(after - self.target)
        } else if after < self.target / 2 {
            frames - step.min(frames)
        } else {
            frames
        };
        self.scratch.clear();
        self.scratch.extend(queue.drain(..take.min(queue.len())));
        stretch(&mut self.scratch, frames);
        for (frame, &v) in out.as_chunks_mut::<2>().0.iter_mut().zip(&self.scratch) {
            frame[0] = v;
            frame[1] = v;
        }
    }
}

/// Makes `samples` exactly `len` long by leaving out (or playing twice) single samples where the sound is quietest,
/// one in each of as many equal parts.
fn stretch(samples: &mut Vec<f32>, len: usize) {
    let have = samples.len();
    if have == 0 || have == len {
        samples.resize(len, 0.0);
        return;
    }
    let changes = have.abs_diff(len);
    let part = (have / changes).max(1);
    // From the end, so earlier positions stay where they were.
    for k in (0..changes).rev() {
        let range = (k * part).min(have - 1)..((k + 1) * part).min(samples.len());
        if range.is_empty() {
            continue;
        }
        let quietest = range.clone().min_by(|&a, &b| samples[a].abs().total_cmp(&samples[b].abs())).unwrap_or(range.start);
        if have > len {
            samples.remove(quietest);
        } else {
            samples.insert(quietest, samples[quietest]);
        }
    }
    samples.resize(len, 0.0);
}

/// Mixes the playing sounds into `out` (interleaved stereo) and drops the finished ones. Peaks beyond the knee are
/// rounded off rather than cut flat.
fn mix(voices: &mut Vec<(Arc<Sound>, usize)>, out: &mut [f32]) {
    out.fill(0.0);
    for (sound, pos) in voices.iter_mut() {
        for (frame, src) in out.as_chunks_mut::<2>().0.iter_mut().zip(sound.frames[(*pos).min(sound.frames.len())..].iter()) {
            frame[0] += src[0];
            frame[1] += src[1];
        }
        *pos += out.len() / 2;
    }
    voices.retain(|(s, pos)| *pos < s.frames.len());
    out.iter_mut().for_each(|v| *v = soft_clip(*v));
}

/// Unchanged up to 0.9, then bent smoothly towards 1.
fn soft_clip(v: f32) -> f32 {
    const KNEE: f32 = 0.9;
    let a = v.abs();
    if a <= KNEE {
        return v;
    }
    let bent = KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh();
    bent.copysign(v)
}

#[cfg(windows)]
mod imp {
    use super::{mix, Buffering, Command, Playout, Shared, RATE};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::{Duration, Instant};
    use windows::core::{w, HSTRING};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};
    use windows::Win32::System::Threading::{AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW, WaitForMultipleObjects};

    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    const AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;
    /// How often the default devices are looked at, and a failed device tried again.
    const CHECK_EVERY: Duration = Duration::from_secs(1);
    /// The output player lets its device go after this much quiet (opening it again costs the next click 20-100 ms).
    const OUTPUT_IDLE: Duration = Duration::from_secs(15);

    fn format(channels: u16) -> WAVEFORMATEX {
        let block = channels * 4;
        WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: channels,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * block as u32,
            nBlockAlign: block,
            wBitsPerSample: 32,
            cbSize: 0,
        }
    }

    /// The thread in MMCSS's "Audio" class while it lives: Windows runs it ahead of busy threads (a VR game), so
    /// the sound does not break up under load.
    struct Mmcss(HANDLE);

    impl Mmcss {
        fn audio() -> Option<Self> {
            let mut index = 0u32;
            unsafe { AvSetMmThreadCharacteristicsW(w!("Audio"), &mut index) }.ok().map(Self) // i18n: not shown (Windows names this task class "Audio")
        }
    }

    impl Drop for Mmcss {
        fn drop(&mut self) {
            unsafe {
                let _ = AvRevertMmThreadCharacteristics(self.0);
            }
        }
    }

    /// An event-driven shared-mode client of 32-bit float at 48 kHz (`buffer` in 100 ns units).
    struct Stream {
        client: IAudioClient,
        event: HANDLE,
    }

    impl Stream {
        unsafe fn open(device: &IMMDevice, channels: u16, buffer: i64) -> anyhow::Result<Self> {
            unsafe {
                let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
                let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
                client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, buffer, 0, &format(channels), None)?;
                let event = CreateEventW(None, false, false, None)?;
                let stream = Self { client, event };
                stream.client.SetEventHandle(event)?;
                Ok(stream)
            }
        }
    }

    impl Drop for Stream {
        fn drop(&mut self) {
            unsafe {
                let _ = self.client.Stop();
                let _ = CloseHandle(self.event);
            }
        }
    }

    unsafe fn enumerator() -> anyhow::Result<IMMDeviceEnumerator> {
        unsafe { Ok(CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?) }
    }

    unsafe fn device_id(device: &IMMDevice) -> anyhow::Result<String> {
        unsafe {
            let id = device.GetId()?;
            let text = id.to_string();
            CoTaskMemFree(Some(id.0 as *const _));
            Ok(text?)
        }
    }

    /// The default device's id for `flow` (none when there is none).
    unsafe fn default_id(devices: &IMMDeviceEnumerator, flow: EDataFlow) -> Option<String> {
        unsafe { devices.GetDefaultAudioEndpoint(flow, eConsole).ok().and_then(|d| device_id(&d).ok()) }
    }

    /// A microphone, open and started.
    struct Capture {
        stream: Stream,
        client: IAudioCaptureClient,
        id: String,
    }

    unsafe fn open_capture(devices: &IMMDeviceEnumerator, device: Option<&str>) -> anyhow::Result<Capture> {
        unsafe {
            let mic = match device {
                Some(id) => devices.GetDevice(&HSTRING::from(id))?,
                None => devices.GetDefaultAudioEndpoint(eCapture, eConsole)?,
            };
            let id = device_id(&mic)?;
            let stream = Stream::open(&mic, 1, 200_000)?;
            let client: IAudioCaptureClient = stream.client.GetService()?;
            stream.client.Start()?;
            Ok(Capture { stream, client, id })
        }
    }

    /// The default output, open (started at its first feed).
    struct Render {
        stream: Stream,
        client: IAudioRenderClient,
        size: u32,
        /// What the device is kept filled to: 20 ms (the jitter buffer is where waiting belongs, not the device).
        fill: u32,
        started: bool,
        id: String,
    }

    unsafe fn open_render(devices: &IMMDeviceEnumerator) -> anyhow::Result<Render> {
        unsafe {
            let speaker = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let id = device_id(&speaker)?;
            // 30 ms of buffer: clicks follow the press closely.
            let stream = Stream::open(&speaker, 2, 300_000)?;
            let client: IAudioRenderClient = stream.client.GetService()?;
            let size = stream.client.GetBufferSize()?;
            Ok(Render { stream, client, size, fill: (RATE / 50).min(size), started: false, id })
        }
    }

    /// Tops the output up to its fill level: `fill` gets interleaved stereo to fill. Starts it the first time.
    unsafe fn feed(render: &mut Render, fill: &mut dyn FnMut(&mut [f32])) -> windows::core::Result<()> {
        unsafe {
            let padding = render.stream.client.GetCurrentPadding()?;
            let want = render.fill.saturating_sub(padding).min(render.size.saturating_sub(padding));
            if want > 0 {
                let data = render.client.GetBuffer(want)?;
                fill(std::slice::from_raw_parts_mut(data.cast::<f32>(), want as usize * 2));
                render.client.ReleaseBuffer(want, 0)?;
            }
            if !render.started {
                render.stream.client.Start()?;
                render.started = true;
            }
            Ok(())
        }
    }

    /// Hands what the microphone has to `on_mic`, noting its peak in `level`.
    unsafe fn drain(capture: &Capture, on_mic: &mut dyn FnMut(&[f32]), level: &AtomicU32) -> windows::core::Result<()> {
        const SILENCE: [f32; 480] = [0.0; 480];
        unsafe {
            while capture.client.GetNextPacketSize()? > 0 {
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                capture.client.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null();
                if silent {
                    let mut left = frames as usize;
                    while left > 0 {
                        let n = left.min(SILENCE.len());
                        on_mic(&SILENCE[..n]);
                        left -= n;
                    }
                } else {
                    let samples = std::slice::from_raw_parts(data.cast::<f32>(), frames as usize);
                    let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                    // Non-negative floats order like their bits.
                    level.fetch_max(peak.min(1.0).to_bits(), Ordering::Relaxed);
                    on_mic(samples);
                }
                capture.client.ReleaseBuffer(frames)?;
            }
            Ok(())
        }
    }

    pub fn output(rx: mpsc::Receiver<Command>) {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let _mmcss = Mmcss::audio();
        let mut voices: Vec<(Arc<super::Sound>, usize)> = Vec::new();
        // Idle: wait for a sound, then play until a while after the last one ended.
        while let Ok(Command::Play(sound)) = rx.recv() {
            voices.push((sound, 0));
            let mut failures = 0;
            loop {
                match unsafe { play(&rx, &mut voices) } {
                    // Quiet a while: the device goes.
                    Ok(true) => break,
                    // Another default output (or the device went away): the sounds go on there, where they were.
                    Ok(false) => failures = 0,
                    Err(e) => {
                        failures += 1;
                        if failures >= 2 {
                            eprintln!("sound output: {e:#}");
                            voices.clear();
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Plays `voices` and what comes in on the default output: true once quiet for [`OUTPUT_IDLE`], false when the
    /// default output changed.
    unsafe fn play(rx: &mpsc::Receiver<Command>, voices: &mut Vec<(Arc<super::Sound>, usize)>) -> anyhow::Result<bool> {
        unsafe {
            let devices = enumerator()?;
            let mut render = open_render(&devices)?;
            feed(&mut render, &mut |out| mix(voices, out))?;
            let mut quiet_since = None::<Instant>;
            let mut checked = Instant::now();
            loop {
                WaitForMultipleObjects(&[render.stream.event], false, 100);
                while let Ok(Command::Play(s)) = rx.try_recv() {
                    voices.push((s, 0));
                }
                feed(&mut render, &mut |out| mix(voices, out))?;
                if voices.is_empty() {
                    let since = *quiet_since.get_or_insert_with(Instant::now);
                    if since.elapsed() > OUTPUT_IDLE {
                        return Ok(true);
                    }
                } else {
                    quiet_since = None;
                }
                if checked.elapsed() >= CHECK_EVERY {
                    checked = Instant::now();
                    if default_id(&devices, eRender).is_some_and(|id| id != render.id) {
                        return Ok(false);
                    }
                }
            }
        }
    }

    /// The microphone (`device`, else the default one) to `on_mic`, `playback` to the default output, until stopped.
    /// The output follows the default device; a device that fails or goes away is opened again every second. When
    /// `lenient` (a call), a chosen microphone that cannot open gives way to the default one and a missing device
    /// does not stop the start; otherwise (the mic test) both must open.
    pub fn run(device: Option<&str>, shared: &Shared, on_mic: &mut dyn FnMut(&[f32]), playback: &Mutex<VecDeque<f32>>, buffering: Buffering, lenient: bool) {
        if let Err(e) = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
            shared.fail(format!("{e}"));
            return;
        }
        let _mmcss = Mmcss::audio();
        let devices = match unsafe { enumerator() } {
            Ok(d) => d,
            Err(e) => {
                shared.fail(format!("{e:#}"));
                return;
            }
        };
        let mut playout = Playout::new(buffering);
        // The microphone asked for (`None` once the default one stands in for it).
        let mut mic_choice = device.map(str::to_string);
        let mut mic_trouble = None::<String>;
        let mut out_trouble = None::<String>;
        let open_mic = |choice: &mut Option<String>, trouble: &mut Option<String>| -> Option<Capture> {
            match unsafe { open_capture(&devices, choice.as_deref()) } {
                Ok(c) => {
                    *trouble = None;
                    Some(c)
                }
                Err(e) if lenient && choice.is_some() => {
                    // The chosen microphone is not there: the default one, for the rest of the call.
                    *choice = None;
                    let fallback = unsafe { open_capture(&devices, None) };
                    *trouble = Some(match &fallback {
                        Ok(_) => orbiters_i18n::tr!("the chosen microphone could not be opened ({error}); the default one is used", error = format!("{e:#}")),
                        Err(e2) => orbiters_i18n::tr!("no microphone could be opened ({error}): the other side hears nothing", error = format!("{e2:#}")),
                    });
                    fallback.ok()
                }
                Err(e) => {
                    *trouble = Some(orbiters_i18n::tr!("the microphone could not be opened ({error})", error = format!("{e:#}")));
                    None
                }
            }
        };
        let mut capture = open_mic(&mut mic_choice, &mut mic_trouble);
        let mut render = match unsafe { open_render(&devices) } {
            Ok(r) => Some(r),
            Err(e) => {
                out_trouble = Some(orbiters_i18n::tr!("the sound output could not be opened ({error})", error = format!("{e:#}")));
                None
            }
        };
        if !lenient && (capture.is_none() || render.is_none()) {
            shared.fail(mic_trouble.or(out_trouble).unwrap_or_default());
            return;
        }
        let mut speak = |render: &mut Option<Render>, trouble: &mut Option<String>| {
            let failed = render.as_mut().and_then(|r| unsafe { feed(r, &mut |out| playout.fill(&mut playback.lock().unwrap(), out)) }.err());
            if let Some(e) = failed {
                *render = None;
                *trouble = Some(orbiters_i18n::tr!("the sound output stopped ({error}); opening it again", error = e));
            }
        };
        // Silence (or what already waits) goes out at once: the output starts with the call.
        speak(&mut render, &mut out_trouble);
        shared.trouble(mic_trouble.clone().or(out_trouble.clone()));
        if !shared.ready() {
            return;
        }
        let mut checked = Instant::now();
        while !shared.stopped() {
            let events: Vec<HANDLE> = capture.iter().map(|c| c.stream.event).chain(render.iter().map(|r| r.stream.event)).collect();
            if events.is_empty() {
                std::thread::sleep(Duration::from_millis(50));
            } else {
                // Either device's turn, whichever comes first: a microphone that stalls never silences the speaker.
                unsafe { WaitForMultipleObjects(&events, false, 50) };
            }
            let heard = capture.as_ref().and_then(|c| unsafe { drain(c, on_mic, &shared.level) }.err());
            if let Some(e) = heard {
                capture = None;
                mic_trouble = Some(orbiters_i18n::tr!("the microphone stopped ({error}); opening it again", error = e));
            }
            speak(&mut render, &mut out_trouble);
            if checked.elapsed() < CHECK_EVERY {
                continue;
            }
            checked = Instant::now();
            // A default device that changed is followed; a device that failed is tried again.
            let default_mic = mic_choice.is_none().then(|| unsafe { default_id(&devices, eCapture) }).flatten();
            if capture.is_none() || capture.as_ref().is_some_and(|c| default_mic.as_ref().is_some_and(|d| *d != c.id)) {
                drop(capture.take());
                capture = open_mic(&mut mic_choice, &mut mic_trouble);
            }
            let default_out = unsafe { default_id(&devices, eRender) };
            if render.is_none() || render.as_ref().is_some_and(|r| default_out.as_ref().is_some_and(|d| *d != r.id)) {
                drop(render.take());
                match unsafe { open_render(&devices) } {
                    Ok(r) => {
                        render = Some(r);
                        out_trouble = None;
                        speak(&mut render, &mut out_trouble);
                    }
                    Err(e) => out_trouble = Some(orbiters_i18n::tr!("the sound output could not be opened ({error})", error = format!("{e:#}"))),
                }
            }
            shared.trouble(mic_trouble.clone().or(out_trouble.clone()));
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{mix, Buffering, Command, Playout, Shared, RATE};
    use crate::coreaudio::{self as ca, Capture, Flow, Render, Turn};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::{Duration, Instant};

    /// How often the default devices are looked at, and a failed device tried again.
    const CHECK_EVERY: Duration = Duration::from_secs(1);
    /// The output player lets its device go after this much quiet (opening it again costs the next click some time).
    const OUTPUT_IDLE: Duration = Duration::from_secs(15);
    /// How much sound the microphone holds for this thread (a run of 10 ms buffers).
    const MIC_HOLD: Duration = Duration::from_millis(40);

    /// Outputs opened since the start, and an output that stands in for the system's default: a test moves the player
    /// between real devices without changing anything of the person's.
    #[cfg(test)]
    pub static RENDERS_OPENED: AtomicU32 = AtomicU32::new(0);
    #[cfg(test)]
    pub static DEFAULT_OUTPUT: Mutex<Option<String>> = Mutex::new(None);

    fn default_output() -> Option<String> {
        #[cfg(test)]
        if let Some(uid) = DEFAULT_OUTPUT.lock().unwrap().clone() {
            return Some(uid);
        }
        ca::default_uid(Flow::Output)
    }

    fn open_render(turn: &Arc<Turn>) -> Result<Render, String> {
        let uid = default_output().ok_or(orbiters_i18n::tr("this computer has no sound output"))?;
        #[cfg(test)]
        RENDERS_OPENED.fetch_add(1, Ordering::Relaxed);
        Render::open(&uid, turn)
    }

    /// The microphone `device`, else the default one, started. A person's refusal comes first: see
    /// [`ca::microphone_access`].
    fn open_capture(device: Option<&str>, turn: &Arc<Turn>) -> Result<Capture, String> {
        let uid = match device {
            Some(uid) => uid.to_string(),
            None => ca::default_uid(Flow::Input).ok_or(orbiters_i18n::tr("this computer has no microphone"))?,
        };
        let mut capture = Capture::open(&uid, RATE, MIC_HOLD, turn)?;
        capture.start()?;
        Ok(capture)
    }

    /// Hands what the microphone has to `on_mic`, noting its peak in `level`.
    fn drain(capture: &mut Capture, on_mic: &mut dyn FnMut(&[f32]), level: &AtomicU32) -> Result<(), String> {
        let handed: Result<(), std::convert::Infallible> = capture.drain(|samples, _| {
            let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            // Non-negative floats order like their bits.
            level.fetch_max(peak.min(1.0).to_bits(), Ordering::Relaxed);
            on_mic(samples);
            Ok(())
        })?;
        match handed {
            Ok(()) => Ok(()),
            Err(never) => match never {},
        }
    }

    pub fn output(rx: mpsc::Receiver<Command>) {
        ca::audio_priority();
        let mut voices: Vec<(Arc<super::Sound>, usize)> = Vec::new();
        // Idle: wait for a sound, then play until a while after the last one ended.
        while let Ok(Command::Play(sound)) = rx.recv() {
            voices.push((sound, 0));
            let mut failures = 0;
            loop {
                match play(&rx, &mut voices) {
                    // Quiet a while: the device goes.
                    Ok(true) => break,
                    // Another default output (or the device went away): the sounds go on there, where they were.
                    Ok(false) => failures = 0,
                    Err(e) => {
                        failures += 1;
                        if failures >= 2 {
                            eprintln!("sound output: {e}");
                            voices.clear();
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Plays `voices` and what comes in on the default output: true once quiet for [`OUTPUT_IDLE`], false when the
    /// default output changed.
    fn play(rx: &mpsc::Receiver<Command>, voices: &mut Vec<(Arc<super::Sound>, usize)>) -> Result<bool, String> {
        let turn = Arc::new(Turn::default());
        let mut render = open_render(&turn)?;
        render.feed(&mut |out| mix(voices, out))?;
        let mut quiet_since = None::<Instant>;
        let mut checked = Instant::now();
        loop {
            turn.wait(Duration::from_millis(100));
            while let Ok(Command::Play(s)) = rx.try_recv() {
                voices.push((s, 0));
            }
            render.feed(&mut |out| mix(voices, out))?;
            if voices.is_empty() {
                let since = *quiet_since.get_or_insert_with(Instant::now);
                if since.elapsed() > OUTPUT_IDLE {
                    return Ok(true);
                }
            } else {
                quiet_since = None;
            }
            if checked.elapsed() >= CHECK_EVERY {
                checked = Instant::now();
                if !ca::alive(&render.uid) || default_output().is_some_and(|id| id != render.uid) {
                    return Ok(false);
                }
            }
        }
    }

    /// The microphone (`device`, else the default one) to `on_mic`, `playback` to the default output, until stopped.
    /// The output follows the default device; a device that fails or goes away is opened again every second. When
    /// `lenient` (a call), a chosen microphone that cannot open gives way to the default one and a missing device
    /// does not stop the start; otherwise (the mic test) both must open.
    pub fn run(device: Option<&str>, shared: &Shared, on_mic: &mut dyn FnMut(&[f32]), playback: &Mutex<VecDeque<f32>>, buffering: Buffering, lenient: bool) {
        ca::audio_priority();
        let turn = Arc::new(Turn::default());
        let mut playout = Playout::new(buffering);
        // The microphone asked for (`None` once the default one stands in for it).
        let mut mic_choice = device.map(str::to_string);
        let mut mic_trouble = None::<String>;
        let mut out_trouble = None::<String>;
        let open_mic = |choice: &mut Option<String>, trouble: &mut Option<String>| -> Option<Capture> {
            // macOS's leave is not about one microphone: the chosen one stays chosen for when it comes.
            if let Err(e) = ca::microphone_access() {
                *trouble = Some(e);
                return None;
            }
            match open_capture(choice.as_deref(), &turn) {
                Ok(c) => {
                    *trouble = None;
                    Some(c)
                }
                Err(e) if lenient && choice.is_some() => {
                    // The chosen microphone is not there: the default one, for the rest of the call.
                    *choice = None;
                    let fallback = open_capture(None, &turn);
                    *trouble = Some(match &fallback {
                        Ok(_) => orbiters_i18n::tr!("the chosen microphone could not be opened ({error}); the default one is used", error = e),
                        Err(e2) => orbiters_i18n::tr!("no microphone could be opened ({error}): the other side hears nothing", error = e2),
                    });
                    fallback.ok()
                }
                Err(e) => {
                    *trouble = Some(orbiters_i18n::tr!("the microphone could not be opened ({error})", error = e));
                    None
                }
            }
        };
        let mut capture = open_mic(&mut mic_choice, &mut mic_trouble);
        let mut render = match open_render(&turn) {
            Ok(r) => Some(r),
            Err(e) => {
                out_trouble = Some(orbiters_i18n::tr!("the sound output could not be opened ({error})", error = e));
                None
            }
        };
        if !lenient && (capture.is_none() || render.is_none()) {
            shared.fail(mic_trouble.or(out_trouble).unwrap_or_default());
            return;
        }
        let mut speak = |render: &mut Option<Render>, trouble: &mut Option<String>| {
            let failed = render.as_mut().and_then(|r| r.feed(&mut |out| playout.fill(&mut playback.lock().unwrap(), out)).err());
            if let Some(e) = failed {
                *render = None;
                *trouble = Some(orbiters_i18n::tr!("the sound output stopped ({error}); opening it again", error = e));
            }
        };
        // Silence (or what already waits) goes out at once: the output starts with the call.
        speak(&mut render, &mut out_trouble);
        shared.trouble(mic_trouble.clone().or(out_trouble.clone()));
        if !shared.ready() {
            return;
        }
        let mut checked = Instant::now();
        while !shared.stopped() {
            // Either device's turn, whichever comes first: a microphone that stalls never silences the speaker.
            if capture.is_none() && render.is_none() {
                std::thread::sleep(Duration::from_millis(50));
            } else {
                turn.wait(Duration::from_millis(50));
            }
            let heard = capture.as_mut().and_then(|c| drain(c, on_mic, &shared.level).err());
            if let Some(e) = heard {
                capture = None;
                mic_trouble = Some(orbiters_i18n::tr!("the microphone stopped ({error}); opening it again", error = e));
            }
            speak(&mut render, &mut out_trouble);
            if checked.elapsed() < CHECK_EVERY {
                continue;
            }
            checked = Instant::now();
            // A default device that changed is followed; a device that failed or went away is tried again.
            let default_mic = mic_choice.is_none().then(|| ca::default_uid(Flow::Input)).flatten();
            if capture.is_none() || capture.as_ref().is_some_and(|c| !ca::alive(&c.uid) || default_mic.as_ref().is_some_and(|d| *d != c.uid)) {
                drop(capture.take());
                capture = open_mic(&mut mic_choice, &mut mic_trouble);
            }
            let default_out = default_output();
            if render.is_none() || render.as_ref().is_some_and(|r| !ca::alive(&r.uid) || default_out.as_ref().is_some_and(|d| *d != r.uid)) {
                drop(render.take());
                match open_render(&turn) {
                    Ok(r) => {
                        render = Some(r);
                        out_trouble = None;
                        speak(&mut render, &mut out_trouble);
                    }
                    Err(e) => out_trouble = Some(orbiters_i18n::tr!("the sound output could not be opened ({error})", error = e)),
                }
            }
            shared.trouble(mic_trouble.clone().or(out_trouble.clone()));
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use super::{Buffering, Command, Shared};
    use std::collections::VecDeque;
    use std::sync::{mpsc, Mutex};

    pub fn output(rx: mpsc::Receiver<Command>) {
        while rx.recv().is_ok() {}
    }

    pub fn run(
        _device: Option<&str>,
        shared: &Shared,
        _on_mic: &mut dyn FnMut(&[f32]),
        _playback: &Mutex<VecDeque<f32>>,
        _buffering: Buffering,
        _lenient: bool,
    ) {
        shared.fail(orbiters_i18n::tr("sound devices need Windows for now").into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_monitor_s_queue_stays_bounded_without_an_output_and_keeps_the_newest_sound() {
        // N4N0-12: the output lost, the microphone went on filling the queue (about 11 MB a minute).
        let most = MONITOR_BUFFERING.keep * 2;
        let mut queue = VecDeque::new();
        for i in 0..60_000 {
            push_newest(&mut queue, &[i as f32; 480], most);
            assert!(queue.len() <= most);
        }
        assert_eq!(queue.len(), most);
        assert_eq!(queue.back(), Some(&59_999.0), "the newest kept");
        // A slice larger than the bound adds only its end.
        let big: Vec<f32> = (0..most * 3).map(|i| i as f32).collect();
        push_newest(&mut queue, &big, most);
        assert_eq!((queue.len(), queue.front().copied(), queue.back().copied()), (most, Some((most * 2) as f32), Some((most * 3 - 1) as f32)));
    }

    #[test]
    fn voices_mix_and_finish() {
        let a = Arc::new(Sound { rate: RATE, frames: vec![[0.25, -0.25]; 3] });
        let b = Arc::new(Sound { rate: RATE, frames: vec![[0.5, 0.5]; 5] });
        let mut voices = vec![(a, 0), (b, 0)];
        let mut out = [0f32; 8];
        mix(&mut voices, &mut out);
        assert_eq!(out, [0.75, 0.25, 0.75, 0.25, 0.75, 0.25, 0.5, 0.5]);
        assert_eq!(voices.len(), 1);
        mix(&mut voices, &mut out);
        assert_eq!(out[..2], [0.5, 0.5]);
        assert_eq!(out[2..], [0.0; 6]);
        assert!(voices.is_empty());
    }

    #[test]
    fn loud_mixes_are_rounded_off_not_cut() {
        assert_eq!(soft_clip(0.5), 0.5);
        assert_eq!(soft_clip(-0.9), -0.9);
        let (a, b) = (soft_clip(1.0), soft_clip(3.0));
        assert!(0.9 < a && a < b && b <= 1.0, "{a} {b}");
        assert_eq!(soft_clip(-3.0), -b);
    }

    /// Plays `blocks` blocks of 10 ms from `queue`, mono.
    fn play(p: &mut Playout, queue: &mut VecDeque<f32>, blocks: usize) -> Vec<f32> {
        let mut heard = Vec::new();
        let mut out = vec![0.0; 960];
        for _ in 0..blocks {
            p.fill(queue, &mut out);
            heard.extend(out.as_chunks::<2>().0.iter().map(|f| f[0]));
        }
        heard
    }

    #[test]
    fn the_speaker_waits_for_its_target_then_plays_in_order() {
        let mut p = Playout::new(CALL_BUFFERING);
        let mut queue: VecDeque<f32> = (1..=3000).map(|i| i as f32).collect();
        // 3000 samples are more than 60 ms: it plays at once, in order, nothing left out.
        let heard = play(&mut p, &mut queue, 1);
        assert_eq!(heard[..3], [1.0, 2.0, 3.0]);
        assert_eq!(heard[479], 480.0);
        let mut short: VecDeque<f32> = (0..1000).map(|_| 0.5).collect();
        let mut fresh = Playout::new(CALL_BUFFERING);
        assert!(play(&mut fresh, &mut short, 1).iter().all(|&v| v == 0.0), "less than 60 ms waits");
        assert_eq!(short.len(), 1000);
    }

    #[test]
    fn running_dry_waits_longer_next_time() {
        let mut p = Playout::new(CALL_BUFFERING);
        let mut queue: VecDeque<f32> = std::iter::repeat_n(0.5, 3000).collect();
        play(&mut p, &mut queue, 3);
        assert_eq!(p.target, CALL_BUFFERING.start);
        // 120 samples left: they play, then silence.
        let heard = play(&mut p, &mut queue, 4);
        assert!(heard.contains(&0.0));
        assert!(!p.primed);
        assert_eq!(p.target, CALL_BUFFERING.start + GROW, "the next start waits 20 ms more");
        // It waits for the larger target before playing again.
        queue.extend(std::iter::repeat_n(0.5, CALL_BUFFERING.start + 10));
        assert!(play(&mut p, &mut queue, 1).iter().all(|&v| v == 0.0));
        queue.extend(std::iter::repeat_n(0.5, GROW));
        assert!(play(&mut p, &mut queue, 1).iter().all(|&v| v == 0.5));
    }

    #[test]
    fn a_backlog_goes_at_once() {
        let mut p = Playout::new(CALL_BUFFERING);
        let mut queue: VecDeque<f32> = std::iter::repeat_n(0.5, RATE as usize).collect();
        play(&mut p, &mut queue, 1);
        assert!(queue.len() <= CALL_BUFFERING.start, "{}", queue.len());
    }

    #[test]
    fn a_faster_or_slower_clock_is_absorbed() {
        for rate in [1.004f64, 0.996] {
            let mut p = Playout::new(CALL_BUFFERING);
            let mut queue = VecDeque::new();
            let mut produced = 0.0f64;
            let mut dry = 0;
            // A minute of 10 ms blocks, the other side's clock 0.4 % off.
            for block in 0..6000 {
                let due = ((block + 1) as f64 * 480.0 * rate) as usize;
                while (produced as usize) < due {
                    queue.push_back(0.5);
                    produced += 1.0;
                }
                let before = p.primed;
                play(&mut p, &mut queue, 1);
                if before && !p.primed {
                    dry += 1;
                }
                if block > 500 {
                    assert!(queue.len() < CALL_BUFFERING.keep, "rate {rate}: the queue grew to {}", queue.len());
                }
            }
            assert!(dry <= 1, "rate {rate}: ran dry {dry} times");
        }
    }

    #[test]
    fn stretching_leaves_out_or_repeats_the_quietest_samples() {
        let mut s = vec![0.9, 0.8, 0.0, 0.7, 0.9, 0.6, 0.01, 0.9];
        stretch(&mut s, 6);
        assert_eq!(s, vec![0.9, 0.8, 0.7, 0.9, 0.6, 0.9]);
        let mut s = vec![0.9, 0.1, 0.9, 0.9];
        stretch(&mut s, 5);
        assert_eq!(s, vec![0.9, 0.1, 0.1, 0.9, 0.9]);
        let mut s = vec![];
        stretch(&mut s, 3);
        assert_eq!(s, vec![0.0; 3]);
    }

    #[test]
    fn a_start_that_fails_leaves_no_thread() {
        let stop_seen = Arc::new(AtomicBool::new(false));
        let seen = stop_seen.clone();
        // The devices "open" after the start gave up: the thread hears that nobody waits and ends.
        let started = std::time::Instant::now();
        let result = Running::start("test-late", move |shared| {
            std::thread::sleep(START_WAIT + Duration::from_millis(200));
            if !shared.ready() {
                seen.store(true, Ordering::Relaxed);
            }
        });
        assert!(result.is_err());
        assert!(started.elapsed() >= START_WAIT);
        std::thread::sleep(Duration::from_millis(600));
        assert!(stop_seen.load(Ordering::Relaxed), "the late thread was told to stop");
        // A start that fails says why.
        let failed = Running::start("test-fail", |shared| shared.fail("no device".into()));
        assert_eq!(failed.err().map(|e| e.to_string()).as_deref(), Some("no device"));
    }

    /// What a device asked for while it played quiet for `seconds`.
    #[cfg(target_os = "macos")]
    struct Pulled {
        /// From the open to the first buffer it played and gave back: what a first sound waits.
        first_back: Duration,
        /// Frames a second it asked for after that.
        rate: f32,
        /// The waits between the turns it gave after that, the longest and the median.
        longest: Duration,
        median: Duration,
    }

    #[cfg(target_os = "macos")]
    fn pulls(uid: &str, seconds: f32) -> Pulled {
        use crate::coreaudio::{Render, Turn};
        use std::time::Instant;
        let turn = Arc::new(Turn::default());
        let opened = Instant::now();
        let mut render = Render::open(uid, &turn).unwrap();
        let mut first_back = None::<Instant>;
        let mut frames = 0usize;
        let mut waits = Vec::new();
        let mut last = Instant::now();
        let mut filled = 0;
        while opened.elapsed().as_secs_f32() < seconds {
            let mut asked = 0usize;
            render
                .feed(&mut |out| {
                    asked += out.len() / 2;
                    out.fill(0.0);
                })
                .unwrap();
            filled += 1;
            // The first feed fills the device whole: what comes after it is what it played.
            if filled > 1 && asked > 0 {
                let now = Instant::now();
                if first_back.is_none() {
                    first_back = Some(now);
                } else {
                    frames += asked;
                    waits.push(now - last);
                }
                last = now;
            }
            turn.wait(Duration::from_millis(100));
        }
        waits.sort();
        let first = first_back.expect("the device played");
        Pulled {
            first_back: first - opened,
            rate: frames as f32 / first.elapsed().as_secs_f32(),
            longest: waits.last().copied().unwrap_or_default(),
            median: waits.get(waits.len() / 2).copied().unwrap_or_default(),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_device_that_is_not_there_is_said_and_nothing_starts() {
        use crate::coreaudio::{Capture, Render, Turn};
        let turn = Arc::new(Turn::default());
        assert!(Render::open("orbiters-no-such-device", &turn).err().is_some_and(|e| e.contains("is not there")));
        assert!(Capture::open("orbiters-no-such-device", 16_000, Duration::from_millis(100), &turn).err().is_some_and(|e| e.contains("is not there")));
        assert!(crate::coreaudio::device("orbiters-no-such-device").is_none());
        assert!(!crate::coreaudio::alive("orbiters-no-such-device"));
    }

    /// This computer's devices, read only:
    /// `cargo test -p orbiters-sound lists_this_computers -- --ignored --nocapture`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn lists_this_computers_devices() {
        use crate::coreaudio::{alive, default_uid, devices, mic_access, output_volume, rate_of, Flow};
        for flow in [Flow::Output, Flow::Input] {
            let default = default_uid(flow);
            for d in devices(flow) {
                let mark = if default.as_deref() == Some(d.uid.as_str()) { "default " } else { "" };
                println!("{flow:?}: {mark}{} [{}] at {:?} Hz, alive {}", d.name, d.uid, rate_of(&d.uid), alive(&d.uid));
            }
        }
        println!("volume: {:?}", output_volume());
        println!("microphone access: {:?}", mic_access());
    }

    /// Plays silence on the default output and measures that it asks for 48 kHz in real time, whatever the device's own
    /// rate: `cargo test -p orbiters-sound the_default_output_runs -- --ignored --nocapture`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn the_default_output_runs() {
        use crate::coreaudio::{default_uid, rate_of, running, Flow};
        let uid = default_uid(Flow::Output).expect("a default output");
        println!("{uid} at {:?} Hz, running before: {:?}", rate_of(&uid), running(&uid));
        let probe = {
            let uid = uid.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(1));
                running(&uid)
            })
        };
        let pulled = pulls(&uid, 3.0);
        println!(
            "first buffer back after {:?}; then {:.0} frames a second at {RATE}; turns {:?} apart, {:?} at most",
            pulled.first_back, pulled.rate, pulled.median, pulled.longest
        );
        println!("running while playing: {:?}", probe.join().unwrap());
        assert!((pulled.rate - RATE as f32).abs() < RATE as f32 * 0.02, "{}", pulled.rate);
    }

    /// The interface sounds' player on the default output with a sound you hear, then its stand-in default moved to
    /// another real device and back with a long quiet sound under way, as when the person changes output while something
    /// plays. Nothing of the person's default changes (eqMac and the like would take it back at once):
    /// `cargo test -p orbiters-sound the_player_follows -- --ignored --nocapture`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn the_player_follows_the_default_output() {
        use crate::coreaudio::{default_uid, devices, Flow};
        let output = Output::new();
        let opened = || imp::RENDERS_OPENED.load(Ordering::Relaxed);
        let before = opened();
        let click = Arc::new(crate::synth::render(&crate::sounds::ROCKER, RATE));
        output.play(&click);
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(opened(), before + 1, "the device opened for the sound");
        output.play(&click);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(opened(), before + 1, "and stays open for the next one");
        let was = default_uid(Flow::Output).expect("a default output");
        let Some(other) = devices(Flow::Output).into_iter().find(|d| d.uid != was) else {
            println!("no other output to move to");
            return;
        };
        println!("from {was} to {}", other.uid);
        output.play(&Arc::new(Sound { rate: RATE, frames: vec![[0.0; 2]; RATE as usize * 5] }));
        *imp::DEFAULT_OUTPUT.lock().unwrap() = Some(other.uid.clone());
        std::thread::sleep(Duration::from_millis(1400));
        assert_eq!(opened(), before + 2, "the player moved to the other device within a second or so");
        *imp::DEFAULT_OUTPUT.lock().unwrap() = None;
        std::thread::sleep(Duration::from_millis(1400));
        assert_eq!(opened(), before + 3, "and back");
        println!("outputs opened: {}", opened() - before);
    }

    /// Sets the default output's volume and mute to what they are now, which changes nothing you hear, and reads them
    /// back: `cargo test -p orbiters-sound the_volume_takes -- --ignored --nocapture`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn the_volume_takes_what_it_already_has() {
        use crate::coreaudio::{output_volume, set_output_mute, set_output_volume};
        let Some((volume, muted)) = output_volume() else {
            println!("this output has no volume of its own");
            return;
        };
        println!("set volume: {:?}", set_output_volume(volume));
        println!("set mute: {:?}", set_output_mute(muted));
        println!("was {:?}, is {:?}", (volume, muted), output_volume());
        assert_eq!(output_volume().map(|(_, m)| m), Some(muted));
        assert!((output_volume().unwrap().0 - volume).abs() < 0.02);
    }

    /// Opens the default microphone's queue at each rate the app asks for and measures that it keeps time, once macOS
    /// lets this process's app use the microphone (it says so otherwise):
    /// `cargo test -p orbiters-sound the_microphone_queue -- --ignored --nocapture`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn the_microphone_queue_keeps_time() {
        use crate::coreaudio::{default_uid, mic_access, rate_of, running, Capture, Flow, Turn};
        use std::time::Instant;
        let uid = default_uid(Flow::Input).expect("a default microphone");
        println!("{uid} at {:?} Hz, access {:?}", rate_of(&uid), mic_access());
        for rate in [16_000u32, 48_000] {
            let turn = Arc::new(Turn::default());
            let opened = Instant::now();
            let mut capture = match Capture::open(&uid, rate, Duration::from_millis(100), &turn) {
                Ok(capture) => capture,
                Err(e) => {
                    println!("not opened: {e}");
                    return;
                }
            };
            capture.start().unwrap();
            let mut first = None::<Instant>;
            let (mut frames, mut peak, mut packets) = (0usize, 0f32, 0usize);
            let mut times = Vec::new();
            while opened.elapsed() < Duration::from_secs(2) {
                turn.wait(Duration::from_millis(100));
                let handed: Result<(), ()> = capture
                    .drain(|samples, stamp| {
                        if first.is_some() {
                            frames += samples.len();
                        }
                        first.get_or_insert_with(Instant::now);
                        packets += 1;
                        peak = samples.iter().fold(peak, |m, v| m.max(v.abs()));
                        if times.len() < 5 {
                            times.push((samples.len(), stamp.age_ns.map(|a| a / 1000)));
                        }
                        Ok(())
                    })
                    .unwrap();
                handed.unwrap();
            }
            let first = first.expect("the queue gave something");
            let measured = frames as f32 / first.elapsed().as_secs_f32();
            println!("at {rate} Hz: first packet after {:?}, {packets} packets, {measured:.0} frames a second, loudest {peak}", first - opened);
            println!("first packets (frames, age in us): {times:?}");
            println!("device running: {:?}", running(&uid));
            assert!((measured - rate as f32).abs() < rate as f32 * 0.05, "{measured}");
        }
    }

    /// A call's sound as the call worker starts it, with silence for the other side: it must play the queue in real time
    /// with or without a microphone, and say what is missing:
    /// `cargo test -p orbiters-sound the_call_sound -- --ignored --nocapture`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn the_call_sound_plays_its_queue_in_real_time() {
        use std::time::Instant;
        let speaker = Arc::new(Mutex::new(VecDeque::new()));
        let heard = Arc::new(Mutex::new(0usize));
        let (counted, kept) = (heard.clone(), speaker.clone());
        let started = Instant::now();
        let duplex = Duplex::start(None, move |samples: &[f32]| *counted.lock().unwrap() += samples.len(), kept).unwrap();
        println!("started in {:?}; trouble: {:?}", started.elapsed(), duplex.error());
        // The network: quiet as time passes, in packets of about 20 ms.
        let run = Instant::now();
        let mut sent = 0usize;
        while run.elapsed() < Duration::from_secs(3) {
            let due = (run.elapsed().as_secs_f64() * RATE as f64) as usize;
            speaker.lock().unwrap().extend(std::iter::repeat_n(0.0f32, due - sent));
            sent = due;
            std::thread::sleep(Duration::from_millis(20));
        }
        let left = speaker.lock().unwrap().len();
        let played = sent.saturating_sub(left);
        let rate = played as f32 / run.elapsed().as_secs_f32();
        println!(
            "sent {sent} samples, {left} wait: {rate:.0} played a second; the microphone gave {} samples; trouble: {:?}",
            heard.lock().unwrap(),
            duplex.error()
        );
        assert!(left < RATE as usize * 3 / 10, "the queue stays short: {left}");
        assert!((rate - RATE as f32).abs() < RATE as f32 * 0.1, "{rate}");
    }

    /// The mic test as Settings starts it, with its level: the microphone you hear on the output, so it stays short:
    /// `cargo test -p orbiters-sound the_mic_monitor -- --ignored --nocapture`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn the_mic_monitor_runs_or_says_why_not() {
        match MicMonitor::start(None) {
            Ok(monitor) => {
                let mut peak = 0.0f32;
                for _ in 0..30 {
                    std::thread::sleep(Duration::from_millis(100));
                    peak = peak.max(monitor.take_level());
                }
                println!("the monitor ran; loudest sample {peak}, trouble {:?}", monitor.error());
            }
            Err(e) => println!("the monitor did not start: {e:#}"),
        }
    }
}
