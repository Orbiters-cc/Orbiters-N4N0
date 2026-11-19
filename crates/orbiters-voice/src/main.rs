//! N4N0's ears: the voice worker of Orbiters - N4N0 (protocol: `orbiters_core::voice`). The microphone is open only
//! while N4N0 listens or the wake word is on. A held sentence (the Talk button) is transcribed when it is let go; any
//! other sentence ends where Silero VAD hears it end. Transcription runs here, on the CPU, with NVIDIA Parakeet
//! through sherpa-onnx. Audio is kept in memory only until its sentence is transcribed.
//!
//! Only the VAD (2 MB) stays loaded: the recogniser (about 1 GB) loads on a thread of its own when a sentence needs
//! it (a press of Talk starts it, so a held sentence rarely waits) and is let go after ten minutes without one. With
//! the wake word on, a long sentence is first transcribed for its start only, and whole only when that start holds a
//! wake word: talk with friends costs a fraction of what it did.
//!
//! `orbiters-voice transcribe <file.wav>…` transcribes files (a check of the models and their speed).

use orbiters_audio::AudioSource;
use orbiters_core::voice::{self, VoiceCommand, VoiceEvent};
use sha2::{Digest, Sha256};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig, SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
use std::collections::VecDeque;
use std::io::{BufRead, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, Condvar};
use std::time::{Duration, Instant};

const RATE: usize = voice::SAMPLE_RATE as usize;
/// Silero VAD's window at 16 kHz.
const WINDOW: usize = 512;
/// A sentence for N4N0 that never starts is given up after this.
const NO_SPEECH: Duration = Duration::from_secs(8);
/// The longest sentence (held or not).
const LONGEST: Duration = Duration::from_secs(30);
/// Sound kept from before N4N0 starts listening, while the microphone is already open (the wake word).
const PREROLL: usize = RATE * 3 / 10;
/// Threads of the recogniser: VRChat needs the rest.
const THREADS: i32 = 2;
/// A microphone that went away or failed is opened again after this, doubling up to [`REOPEN_MAX`].
const REOPEN_FIRST: Duration = Duration::from_secs(1);
const REOPEN_MAX: Duration = Duration::from_secs(30);
/// The recogniser is let go after this long without a sentence.
const IDLE_UNLOAD: Duration = Duration::from_secs(10 * 60);
/// The start of a long wake-word sentence transcribed first: room for a greeting and the wake word.
const PREFIX: usize = RATE * 5 / 2;
/// An install whose download or unpacking does not move for this long is given up (what came is kept).
const STALL: Duration = Duration::from_secs(60);
/// The share of the progress bar the downloads take; unpacking (15 to 30 s) the rest.
const DOWNLOAD_SHARE: f32 = 0.9;
/// Microphone packets the capture thread may have waiting for the loop (one every 10 ms or so): more are dropped.
const AUDIO_QUEUE: usize = 8;
/// Packets one pass of the loop takes at most before it looks at the commands and its deadlines again: far more than
/// come between two passes (a pass waits at most 10 ms for a command, ~16 ms on Windows' default timer), so the queue
/// is emptied each time and a burst after a slow pass catches up at once.
const AUDIO_PER_PASS: usize = 64;
/// Dropped packets are said in the log at most this often.
const DROPS_SAID_EVERY: Duration = Duration::from_secs(60);

/// Microphone packets the capture thread dropped because the loop's queue was full, and their frames.
#[derive(Default)]
struct Drops {
    packets: AtomicU64,
    frames: AtomicU64,
}

/// Hands one microphone packet of capture `generation` to the loop: dropped (and counted) when its queue is full; an
/// error when the worker is gone.
fn offer(audio: &mpsc::SyncSender<(u64, Vec<f32>)>, drops: &Drops, generation: u64, chunk: Vec<f32>) -> anyhow::Result<()> {
    let frames = chunk.len() as u64;
    match audio.try_send((generation, chunk)) {
        Ok(()) => Ok(()),
        Err(mpsc::TrySendError::Full(_)) => {
            drops.packets.fetch_add(1, Ordering::Relaxed);
            drops.frames.fetch_add(frames, Ordering::Relaxed);
            Ok(())
        }
        Err(mpsc::TrySendError::Disconnected(_)) => Err(anyhow::anyhow!("{}", orbiters_i18n::tr("the worker stopped"))),
    }
}

/// What reaches the worker's loop. Microphone messages carry the generation of the capture thread that sent them:
/// a thread being replaced may still speak once or twice after its successor started.
enum Input {
    Command(VoiceCommand),
    MicOpened(u64, String),
    MicClosed(u64, Option<String>),
    /// The recogniser of load `n` loaded (or not), in this long.
    Loaded(u64, Result<OfflineRecognizer, String>, Duration),
    Installed(Result<(), String>),
    StdinClosed,
}

fn emit(event: &VoiceEvent) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", event.to_line());
    let _ = out.flush();
}

fn path_string(p: PathBuf) -> Option<String> {
    Some(p.to_string_lossy().into_owned())
}

fn load_vad(models: &Path) -> anyhow::Result<VoiceActivityDetector> {
    anyhow::ensure!(models.join(voice::VAD_FILE).is_file(), "{}", orbiters_i18n::tr!("N4N0's speech models are not installed in {folder}", folder = models.display()));
    VoiceActivityDetector::create(
        &VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: path_string(models.join(voice::VAD_FILE)),
                threshold: 0.5,
                // A sentence ends after this much silence.
                min_silence_duration: 0.6,
                min_speech_duration: 0.25,
                window_size: WINDOW as i32,
                max_speech_duration: LONGEST.as_secs_f32(),
            },
            sample_rate: RATE as i32,
            num_threads: 1,
            provider: Some("cpu".into()),
            ..Default::default()
        },
        LONGEST.as_secs_f32() + 5.0,
    )
    .ok_or_else(|| anyhow::anyhow!("{}", orbiters_i18n::tr("voice activity detection did not load")))
}

fn load_recognizer(models: &Path) -> anyhow::Result<OfflineRecognizer> {
    anyhow::ensure!(voice::installed(models), "{}", orbiters_i18n::tr!("N4N0's speech models are not installed in {folder} (or are damaged)", folder = models.display()));
    let asr = models.join(voice::ASR_DIR);
    let mut config = OfflineRecognizerConfig::default();
    config.model_config.transducer = OfflineTransducerModelConfig {
        encoder: path_string(asr.join("encoder.int8.onnx")),
        decoder: path_string(asr.join("decoder.int8.onnx")),
        joiner: path_string(asr.join("joiner.int8.onnx")),
    };
    config.model_config.tokens = path_string(asr.join("tokens.txt"));
    config.model_config.model_type = Some("nemo_transducer".into());
    config.model_config.num_threads = THREADS;
    config.model_config.provider = Some("cpu".into());
    config.decoding_method = Some("greedy_search".into());
    OfflineRecognizer::create(&config).ok_or_else(|| anyhow::anyhow!("{}", orbiters_i18n::tr("the speech recogniser did not load")))
}

fn transcribe(recognizer: &OfflineRecognizer, samples: &[f32]) -> (String, u32) {
    let started = Instant::now();
    let stream = recognizer.create_stream();
    stream.accept_waveform(RATE as i32, samples);
    recognizer.decode(&stream);
    let text = stream.get_result().map(|r| r.text.trim().to_string()).unwrap_or_default();
    (text, started.elapsed().as_millis() as u32)
}

/// The speech recogniser: loaded when a sentence needs it, let go after [`IDLE_UNLOAD`] without one.
enum Recognizer {
    Unloaded,
    /// Loading on a thread of its own (load number), the sentences meanwhile waiting.
    Loading(u64),
    Ready(Arc<Mutex<OfflineRecognizer>>),
}

/// A sentence to transcribe: for N4N0 (answering listen `listen`), or heard for the wake word.
struct Sentence {
    samples: Vec<f32>,
    for_n4n0: bool,
    listen: u64,
}

struct Mic {
    stop: Arc<AtomicBool>,
    generation: u64,
}

impl Drop for Mic {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

struct Listen {
    hold: bool,
    since: Instant,
    /// The service's number for it, given back with its sentence.
    id: u64,
}

// i18n: not shown (inference scheduler and transcript internals)
type DecodeJob = Box<dyn FnOnce() + Send>;
#[derive(Default)]
struct Jobs { pending: Option<(bool, DecodeJob)>, stopped: bool }
struct Decoder(Arc<(Mutex<Jobs>, Condvar)>);
impl Decoder {
    fn new() -> Self {
        let state = Arc::new((Mutex::new(Jobs::default()), Condvar::new()));
        let consumer = state.clone();
        std::thread::spawn(move || loop {
            let job = {
                let (lock, wake) = &*consumer;
                let mut q = lock.lock().unwrap();
                while q.pending.is_none() && !q.stopped { q = wake.wait(q).unwrap(); }
                if q.stopped { return; }
                q.pending.take().unwrap().1
            };
            job();
        });
        Self(state)
    }
    fn submit(&self, final_job: bool, job: DecodeJob) {
        let (lock, wake) = &*self.0;
        let mut q = lock.lock().unwrap();
        if final_job || !q.pending.as_ref().is_some_and(|p| p.0) {
            q.pending = Some((final_job, job));
            wake.notify_one();
        }
    }
    fn clear(&self) { self.0.0.lock().unwrap().pending = None; }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        let mut q = self.0.0.lock().unwrap(); q.stopped = true; q.pending = None; self.0.1.notify_one();
    }
}

#[derive(Default)]
struct LiveText { tokens: Vec<(f32, String)> }
impl LiveText {
    fn update(&mut self, start: f32, result: sherpa_onnx::OfflineRecognizerResult) -> String {
        // The first second of an overlapping window supplies context only. Preserve the preceding
        // hypothesis before that boundary; the original full audio gets one final correction.
        let cutoff = if start > 0.0 { start + 1.0 } else { 0.0 };
        if let Some(times) = result.timestamps.filter(|t| !t.is_empty() && t.len() == result.tokens.len()) {
            self.tokens.retain(|(at, _)| *at < cutoff);
            self.tokens.extend(times.into_iter().zip(result.tokens).filter_map(|(at, word)| {
                let at = start + at; (at >= cutoff).then_some((at, word))
            }));
        }
        self.tokens.iter().map(|(_, t)| t.as_str()).collect::<String>().replace('▁', " ").trim().to_string()
    }
}

struct Worker {
    tx: mpsc::Sender<Input>,
    audio_tx: mpsc::SyncSender<(u64, Vec<f32>)>,
    audio_rx: mpsc::Receiver<(u64, Vec<f32>)>,
    /// What the capture threads dropped, and what of it the log has said (and when).
    drops: Arc<Drops>,
    drops_said: (u64, Instant),
    /// Packets of the current microphone the loop took (for the tests).
    taken: u64,
    decoder: Decoder,
    decode_generation: Arc<AtomicU64>,
    live: Arc<Mutex<LiveText>>,
    models: Option<PathBuf>,
    vad: Option<VoiceActivityDetector>,
    recognizer: Recognizer,
    loads: u64,
    /// When the recogniser was last needed: it goes after [`IDLE_UNLOAD`] without.
    used_at: Instant,
    waiting: VecDeque<Sentence>,
    mic_id: Option<String>,
    mic: Option<Mic>,
    /// The last capture thread started.
    mic_generation: u64,
    /// After a microphone went away or failed: not before this, and how many times in a row.
    reopen_at: Option<Instant>,
    mic_failures: u32,
    wake_word: bool,
    wake_words: Vec<String>,
    listen: Option<Listen>,
    /// The held sentence so far.
    held: Vec<f32>,
    partial_at: Instant,
    partial_id: Arc<AtomicU64>,
    /// Samples waiting for a whole VAD window.
    pending: Vec<f32>,
    preroll: VecDeque<f32>,
    speaking: bool,
    level: (f32, usize, Option<Instant>),
    /// The models' install under way (this process does nothing else then).
    installing: Option<Arc<Progress>>,
}

impl Worker {
    fn new(tx: mpsc::Sender<Input>) -> Self {
        let (audio_tx, audio_rx) = mpsc::sync_channel(AUDIO_QUEUE);
        Self {
            tx, audio_tx, audio_rx, decoder: Decoder::new(),
            drops: Arc::new(Drops::default()),
            drops_said: (0, Instant::now()),
            taken: 0,
            decode_generation: Arc::new(AtomicU64::new(0)),
            live: Arc::new(Mutex::new(LiveText::default())),
            models: None,
            vad: None,
            recognizer: Recognizer::Unloaded,
            loads: 0,
            used_at: Instant::now(),
            waiting: VecDeque::new(),
            mic_id: None,
            mic: None,
            mic_generation: 0,
            reopen_at: None,
            mic_failures: 0,
            wake_word: false,
            wake_words: Vec::new(),
            listen: None,
            held: Vec::new(),
            partial_at: Instant::now(),
            partial_id: Arc::new(AtomicU64::new(0)),
            pending: Vec::new(),
            preroll: VecDeque::new(),
            speaking: false,
            level: (0.0, 0, None),
            installing: None,
        }
    }

    fn wants_mic(&self) -> bool {
        self.vad.is_some() && (self.wake_word || self.listen.is_some())
    }

    /// Opens the microphone when something listens (unless it failed a moment ago), closes it when nothing does.
    fn follow_mic(&mut self) {
        if self.wants_mic() && self.mic.is_none() {
            if self.reopen_at.is_some_and(|t| Instant::now() < t) {
                return;
            }
            self.mic_generation += 1;
            let generation = self.mic_generation;
            let stop = Arc::new(AtomicBool::new(false));
            let (tx, id, s) = (self.tx.clone(), self.mic_id.clone(), stop.clone());
            let (audio_tx, drops) = (self.audio_tx.clone(), self.drops.clone());
            let _ = std::thread::Builder::new().name("mic".into()).spawn(move || {
                let (ready, audio) = (tx.clone(), audio_tx);
                let result = orbiters_audio::capture(
                    &AudioSource::Mic(id),
                    Some(voice::SAMPLE_RATE),
                    &s,
                    |name, _| {
                        let _ = ready.send(Input::MicOpened(generation, name.to_string()));
                        Ok(())
                    },
                    |_, frames, samples| {
                        let chunk = samples.map(<[f32]>::to_vec).unwrap_or_else(|| vec![0.0; frames as usize]);
                        offer(&audio, &drops, generation, chunk)
                    },
                );
                let _ = tx.send(Input::MicClosed(generation, result.err().map(|e| format!("{e:#}"))));
            });
            self.mic = Some(Mic { stop, generation });
        } else if !self.wants_mic() && self.mic.is_some() {
            self.mic = None;
            self.forget_sound();
        }
    }

    /// Forgets the sound between sentences (the VAD's state, what waited for it): the microphone closed.
    fn forget_sound(&mut self) {
        self.pending.clear();
        self.preroll.clear();
        if let Some(vad) = &self.vad {
            vad.reset();
        }
        if std::mem::take(&mut self.speaking) {
            emit(&VoiceEvent::Speech { active: false });
        }
    }

    /// The current capture thread opened its microphone.
    fn mic_opened(&mut self, generation: u64, name: String) {
        if self.mic.as_ref().is_some_and(|m| m.generation == generation) {
            self.mic_failures = 0;
            self.reopen_at = None;
            emit(&VoiceEvent::Mic { name: Some(name) });
        }
    }

    /// A capture thread ended: when it is the current one, the microphone went away (unplugged, the endpoint reset)
    /// or failed. It opens again after a pause while something still listens; a sentence waiting for a microphone
    /// that cannot open is answered with nothing.
    fn mic_closed(&mut self, generation: u64, error: Option<String>) {
        if !self.mic.as_ref().is_some_and(|m| m.generation == generation) {
            return;
        }
        self.mic = None;
        self.forget_sound();
        emit(&VoiceEvent::Mic { name: None });
        self.mic_failures += 1;
        let pause = REOPEN_FIRST.saturating_mul(1 << (self.mic_failures - 1).min(5)).min(REOPEN_MAX);
        self.reopen_at = Some(Instant::now() + pause);
        if let Some(message) = error {
            let message = orbiters_i18n::tr!("The microphone could not be opened: {error}", error = message);
            emit(&VoiceEvent::Error { message });
            // Whoever listened hears nothing: say so instead of waiting.
            if let Some(listen) = self.listen.take() {
                self.held.clear();
                emit(&VoiceEvent::Heard { text: String::new(), for_n4n0: true, seconds: 0.0, decode_ms: 0, listen: listen.id });
            }
        }
    }

    /// Runs `command`; false when the worker should quit.
    fn command(&mut self, command: VoiceCommand) -> bool {
        match command {
            VoiceCommand::Start { models, mic, wake_word, wake_words } => {
                let started = Instant::now();
                match load_vad(&models) {
                    Ok(vad) => {
                        self.vad = Some(vad);
                        emit(&VoiceEvent::Ready { engine: "Parakeet TDT 0.6B v3 (int8, sherpa-onnx)".into() /* i18n: not shown */, load_ms: started.elapsed().as_millis() as u32 });
                    }
                    Err(e) => emit(&VoiceEvent::Error { message: format!("{e:#}") }),
                }
                self.models = Some(models);
                self.mic_id = mic;
                self.wake_word = wake_word;
                self.wake_words = wake_words;
                if wake_word {
                    // Sentences will come: the recogniser loads now rather than at the first one.
                    self.need_recognizer();
                }
            }
            VoiceCommand::Mic { mic } => {
                if self.mic_id != mic {
                    self.mic_id = mic;
                    // The old microphone's thread stops; the new one opens below, the models untouched.
                    if self.mic.take().is_some() {
                        self.forget_sound();
                    }
                    self.mic_failures = 0;
                    self.reopen_at = None;
                }
            }
            VoiceCommand::Listen { hold, id } => {
                self.decoder.clear(); self.waiting.clear();
                self.decode_generation.fetch_add(1, Ordering::SeqCst);
                self.live = Arc::new(Mutex::new(LiveText::default()));
                if let Some(vad) = &self.vad {
                    // A fresh sentence: what the VAD was following belongs to before.
                    vad.reset();
                }
                self.pending.clear();
                self.speaking = false;
                self.held = if hold { self.preroll.iter().copied().collect() } else { Vec::new() };
                self.partial_id.store(id, Ordering::Relaxed);
                self.partial_at = Instant::now();
                self.listen = Some(Listen { hold, since: Instant::now(), id });
                // Someone speaks now: a microphone that failed a moment ago is tried again at once, and the
                // recogniser loads while they speak.
                self.reopen_at = None;
                self.need_recognizer();
            }
            VoiceCommand::Finish => {
                self.partial_id.store(0, Ordering::Relaxed);
                if let Some(listen) = self.listen.take_if(|l| l.hold) {
                    let held = std::mem::take(&mut self.held);
                    self.sentence(held, true, listen.id);
                }
            }
            VoiceCommand::Cancel => {
                self.decoder.clear(); self.waiting.clear();
                self.decode_generation.fetch_add(1, Ordering::SeqCst);
                self.partial_id.store(0, Ordering::Relaxed);
                if self.installing.is_some() {
                    // Stops the install: what was downloaded stays for the next one.
                    return false;
                }
                self.listen = None;
                self.held.clear();
            }
            VoiceCommand::WakeWord { on, words } => {
                self.wake_word = on;
                self.wake_words = words;
            }
            VoiceCommand::Install { models, repair } => {
                if self.installing.is_none() {
                    let progress = Arc::new(Progress::new());
                    let (tx, p) = (self.tx.clone(), progress.clone());
                    let _ = std::thread::Builder::new().name("install".into()).spawn(move || {
                        let result = install(&models, repair, &p).map_err(|e| format!("{e:#}"));
                        let _ = tx.send(Input::Installed(result));
                    });
                    self.installing = Some(progress);
                }
                return true;
            }
            VoiceCommand::Quit => return false,
        }
        self.follow_mic();
        true
    }

    /// The recogniser is needed soon: it starts loading (on a thread of its own) unless it is there or on its way.
    fn need_recognizer(&mut self) {
        self.used_at = Instant::now();
        if !matches!(self.recognizer, Recognizer::Unloaded) {
            return;
        }
        let Some(models) = self.models.clone() else { return };
        self.loads += 1;
        let (load, tx) = (self.loads, self.tx.clone());
        self.recognizer = Recognizer::Loading(load);
        // Said before it starts: a crash while loading (a damaged model) shows in the service's log as such.
        eprintln!("voice: loading the speech recogniser");
        let spawned = std::thread::Builder::new().name("recogniser".into()).spawn(move || {
            let started = Instant::now();
            let result = load_recognizer(&models).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Input::Loaded(load, result, started.elapsed()));
        });
        if spawned.is_err() {
            self.recognizer = Recognizer::Unloaded;
        }
    }

    /// Recogniser load `load` ended: the sentences that waited are transcribed (or answered with nothing).
    fn loaded(&mut self, load: u64, result: Result<OfflineRecognizer, String>, took: Duration) {
        if !matches!(self.recognizer, Recognizer::Loading(n) if n == load) {
            return;
        }
        match result {
            Ok(recognizer) => {
                eprintln!("voice: speech recogniser loaded in {} ms", took.as_millis());
                self.recognizer = Recognizer::Ready(Arc::new(Mutex::new(recognizer)));
                self.used_at = Instant::now();
                while let Some(s) = self.waiting.pop_front() {
                    self.decode(s);
                }
            }
            Err(e) => {
                // Tried again at the next sentence.
                self.recognizer = Recognizer::Unloaded;
                let message = orbiters_i18n::tr!("N4N0's speech recogniser did not load: {error}", error = e);
                emit(&VoiceEvent::Error { message });
                while let Some(s) = self.waiting.pop_front() {
                    if s.for_n4n0 {
                        emit(&VoiceEvent::Heard { text: String::new(), for_n4n0: true, seconds: 0.0, decode_ms: 0, listen: s.listen });
                    }
                }
            }
        }
    }

    fn audio(&mut self, samples: Vec<f32>) {
        if self.vad.is_none() {
            return;
        }
        if self.listen.is_some() {
            let (sum, n, at) = &mut self.level;
            *sum += samples.iter().map(|s| s * s).sum::<f32>();
            *n += samples.len();
            if at.is_none_or(|t| t.elapsed() >= Duration::from_millis(100)) && *n > 0 {
                // RMS on a speech-friendly scale: a normal voice around 0.5.
                let rms = (*sum / *n as f32).sqrt();
                emit(&VoiceEvent::Level { level: (rms * 6.0).min(1.0) });
                *at = Some(Instant::now());
                (*sum, *n) = (0.0, 0);
            }
        }
        if self.listen.as_ref().is_some_and(|l| l.hold || self.speaking) && self.held.len() < LONGEST.as_secs() as usize * RATE {
            self.held.extend_from_slice(&samples);
            self.partial();
        }
        if self.listen.as_ref().is_some_and(|l| l.hold) {
            return;
        }
        self.preroll.extend(samples.iter().copied());
        let excess = self.preroll.len().saturating_sub(PREROLL);
        self.preroll.drain(..excess);
        self.pending.extend_from_slice(&samples);
        let mut segments = Vec::new();
        let speaking = {
            let vad = self.vad.as_ref().expect("checked above");
            while self.pending.len() >= WINDOW {
                vad.accept_waveform(&self.pending[..WINDOW]);
                self.pending.drain(..WINDOW);
                while !vad.is_empty() {
                    if let Some(segment) = vad.front() {
                        segments.push(segment.samples().to_vec());
                    }
                    vad.pop();
                }
            }
            vad.detected()
        };
        if speaking != self.speaking {
            self.speaking = speaking;
            emit(&VoiceEvent::Speech { active: speaking });
            if speaking && self.listen.is_some() {
                self.held = self.preroll.iter().copied().collect();
            }
            if speaking && self.wake_word {
                // A sentence starts: the recogniser loads while it goes on.
                self.need_recognizer();
            }
        }
        for segment in segments {
            if let Some(listen) = self.listen.take_if(|l| !l.hold) {
                self.sentence(segment, true, listen.id);
                self.follow_mic();
            } else if self.wake_word {
                self.sentence(segment, false, 0);
            }
        }
    }

    /// A whole sentence: transcribed now, or once the recogniser has loaded.
    fn sentence(&mut self, samples: Vec<f32>, for_n4n0: bool, listen: u64) {
        if for_n4n0 {
            self.partial_id.store(0, Ordering::Relaxed);
            emit(&VoiceEvent::Finalizing { listen });
        }
        // Less than a quarter of a second holds no word.
        if samples.len() < RATE / 4 {
            if for_n4n0 {
                emit(&VoiceEvent::Heard { text: String::new(), for_n4n0, seconds: samples.len() as f32 / RATE as f32, decode_ms: 0, listen });
            }
            return;
        }
        let sentence = Sentence { samples, for_n4n0, listen };
        if matches!(self.recognizer, Recognizer::Ready(_)) {
            self.decode(sentence);
            return;
        }
        // Latest sentence only, including while models load.
        if !for_n4n0 && self.waiting.iter().any(|s| s.for_n4n0) { return; }
        self.waiting.clear();
        self.waiting.push_back(sentence);
        self.need_recognizer();
    }

    /// Transcribes a sentence and says it. One heard for the wake word that is long is transcribed for its start
    /// first, and whole only when that start holds a wake word (the others are nobody's business).
    fn decode(&mut self, s: Sentence) {
        let Recognizer::Ready(recognizer) = &self.recognizer else { return };
        self.used_at = Instant::now();
        let recognizer = recognizer.clone();
        let wake_words = self.wake_words.clone();
        // Final decode also runs away from the audio/VAD loop. The single recognizer serializes inference.
        let generation = self.decode_generation.clone();
        let expected = generation.load(Ordering::SeqCst);
        self.decoder.submit(s.for_n4n0, Box::new(move || {
            if generation.load(Ordering::SeqCst) != expected { return; }
            let recognizer = recognizer.lock().unwrap();
            let seconds = s.samples.len() as f32 / RATE as f32;
            if !s.for_n4n0 && !wake_words.is_empty() && s.samples.len() > PREFIX {
                let (start, _) = transcribe(&recognizer, &s.samples[..PREFIX]);
                if orbiters_n4n0_core::wake::command(&start, &wake_words).is_none() { return; }
            }
            let (text, decode_ms) = transcribe(&recognizer, &s.samples);
            if generation.load(Ordering::SeqCst) == expected {
                emit(&VoiceEvent::Heard { text, for_n4n0: s.for_n4n0, seconds, decode_ms, listen: s.listen });
            }
        }));
    }

    /// One running job and one replaceable pending snapshot; an active final has priority.
    fn partial(&mut self) {
        if self.held.len() < RATE || self.partial_at.elapsed() < Duration::from_millis(1000) { return; }
        let Recognizer::Ready(recognizer) = &self.recognizer else { return };

        self.partial_at = Instant::now();
        let start = self.held.len().saturating_sub(RATE * 6);
        let (recognizer, samples) = (recognizer.clone(), self.held[start..].to_vec());
        let live = self.live.clone();
        let listen = self.listen.as_ref().unwrap().id;
        let current = self.partial_id.clone();
        self.decoder.submit(false, Box::new(move || {
            if current.load(Ordering::Relaxed) == listen {
                let recognizer = recognizer.lock().unwrap();
                let stream = recognizer.create_stream();
                stream.accept_waveform(RATE as i32, &samples);
                recognizer.decode(&stream);
                if current.load(Ordering::Relaxed) == listen {
                    if let Some(result) = stream.get_result() {
                        let text = live.lock().unwrap().update(start as f32 / RATE as f32, result);
                        if !text.is_empty() { emit(&VoiceEvent::Provisional { listen, text }); }
                    }
                }
            }
        }));
    }

    /// Takes the microphone's packets waiting for the loop, at most `most`: those of the current capture are heard,
    /// those of one being replaced dropped. How many were taken.
    fn take_audio(&mut self, most: usize) -> usize {
        let mut taken = 0;
        while taken < most {
            let Ok((generation, samples)) = self.audio_rx.try_recv() else { break };
            taken += 1;
            if self.mic.as_ref().is_some_and(|m| m.generation == generation) {
                self.taken += 1;
                self.audio(samples);
            }
        }
        taken
    }

    /// One pass of the worker's loop: a command or a message (waiting up to 10 ms for one), the microphone's packets
    /// (up to `most`), the deadlines. False when the worker should quit.
    fn pass(&mut self, rx: &mpsc::Receiver<Input>, most: usize) -> bool {
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(Input::Command(c)) => {
                if !self.command(c) {
                    return false;
                }
            }
            Ok(Input::MicOpened(generation, name)) => self.mic_opened(generation, name),
            Ok(Input::MicClosed(generation, error)) => self.mic_closed(generation, error),
            Ok(Input::Loaded(load, result, took)) => self.loaded(load, result, took),
            Ok(Input::Installed(result)) => {
                match result {
                    Ok(()) => emit(&VoiceEvent::Installed),
                    Err(e) => {
                        let message = orbiters_i18n::tr!("Could not install N4N0's speech models: {error}", error = e);
                        emit(&VoiceEvent::Error { message })
                    }
                }
                return false;
            }
            Ok(Input::StdinClosed) => return false,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return false,
        }
        self.take_audio(most);
        self.tick()
    }

    /// Says in the log, at most once a minute, how much sound the loop's queue dropped since it last said.
    fn say_drops(&mut self) {
        let packets = self.drops.packets.load(Ordering::Relaxed);
        if packets > self.drops_said.0 && self.drops_said.1.elapsed() >= DROPS_SAID_EVERY {
            let frames = self.drops.frames.load(Ordering::Relaxed);
            eprintln!("voice: the microphone's queue was full: {} packets dropped ({frames} frames in all)", packets - self.drops_said.0);
            self.drops_said = (packets, Instant::now());
        }
    }

    /// A sentence for N4N0 that never started, or went on too long; a microphone due to open again; a recogniser
    /// idle long enough to go. False when the install stalled (the worker then quits).
    fn tick(&mut self) -> bool {
        self.say_drops();
        if let Some(progress) = &self.installing {
            if progress.idle_for() > STALL {
                emit(&VoiceEvent::Error {
                    message: orbiters_i18n::tr("The download of N4N0's speech models stalled (nothing came for a minute). Try again: it goes on from where it stopped.").into(),
                });
                return false;
            }
            return true;
        }
        if self.wants_mic() && self.mic.is_none() {
            self.follow_mic();
        }
        if matches!(self.recognizer, Recognizer::Ready(_))
            && self.listen.is_none()
            && self.waiting.is_empty()
            && !self.speaking
            && self.used_at.elapsed() > IDLE_UNLOAD
        {
            // About 1 GB back to VRChat until the next sentence.
            self.recognizer = Recognizer::Unloaded;
            eprintln!("voice: speech recogniser let go after {} idle minutes", IDLE_UNLOAD.as_secs() / 60);
        }
        let Some(listen) = &self.listen else { return true };
        let waited = listen.since.elapsed();
        if listen.hold {
            if waited > LONGEST {
                let id = listen.id;
                self.listen = None;
                let held = std::mem::take(&mut self.held);
                self.sentence(held, true, id);
                self.follow_mic();
            }
        } else if (waited > NO_SPEECH && !self.speaking) || waited > LONGEST {
            let id = listen.id;
            self.listen = None;
            if waited > LONGEST && !self.held.is_empty() {
                let samples = std::mem::take(&mut self.held);
                self.sentence(samples, true, id);
            } else {
                self.partial_id.store(0, Ordering::Relaxed);
                emit(&VoiceEvent::Heard { text: String::new(), for_n4n0: true, seconds: 0.0, decode_ms: 0, listen: id });
            }
            self.follow_mic();
        }
        true
    }
}

/// An install's progress for the service, and when it last moved (the stall watchdog's clock).
struct Progress {
    started: Instant,
    /// Bytes downloaded (or found already downloaded) of [`voice::DOWNLOAD_BYTES`].
    done: AtomicU64,
    /// When something last moved, in ms after `started`.
    moved: AtomicU64,
    shown: AtomicI32,
}

impl Progress {
    fn new() -> Self {
        Self { started: Instant::now(), done: AtomicU64::new(0), moved: AtomicU64::new(0), shown: AtomicI32::new(-1) }
    }

    fn moved(&self) {
        self.moved.store(self.started.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    fn idle_for(&self) -> Duration {
        self.started.elapsed().saturating_sub(Duration::from_millis(self.moved.load(Ordering::Relaxed)))
    }

    fn show(&self, fraction: f32) {
        let percent = (fraction.clamp(0.0, 1.0) * 100.0).round() as i32;
        if self.shown.swap(percent, Ordering::Relaxed) != percent {
            emit(&VoiceEvent::Installing { fraction: fraction.clamp(0.0, 1.0) });
        }
    }

    /// `n` more bytes downloaded (negative: counted again from the start). Only the install's thread counts.
    fn add(&self, n: i64) {
        let done = self.done.load(Ordering::Relaxed).saturating_add_signed(n);
        self.done.store(done, Ordering::Relaxed);
        self.moved();
        self.show(DOWNLOAD_SHARE * (done as f64 / voice::DOWNLOAD_BYTES as f64).min(1.0) as f32);
    }

    /// Unpacking, `fraction` of it done.
    fn unpacking(&self, fraction: f32) {
        self.moved();
        self.show(DOWNLOAD_SHARE + (1.0 - DOWNLOAD_SHARE) * fraction.min(1.0));
    }
}

fn file_size(p: &Path) -> Option<u64> {
    std::fs::metadata(p).ok().filter(|m| m.is_file()).map(|m| m.len())
}

/// An HTTPS-only client for the models (GitHub sends them on to its own storage, over HTTPS too). Stalls are the
/// watchdog's business: reads have no overall time limit, a slow line may take an hour.
fn download_agent() -> ureq::Agent {
    ureq::Agent::config_builder().tls_config(orbiters_core::http_tls()).https_only(true).timeout_connect(Some(Duration::from_secs(20))).build().new_agent()
}

/// Downloads what is missing or damaged in `models` (all of it with `repair`): Silero VAD, then Parakeet's archive,
/// unpacked in place. Each download is checked against its size (and SHA-256 once pinned) and goes on from where an
/// earlier one stopped; files land under a temporary name first and are flushed to the disk before they take their
/// place, so a cut install is never taken for models.
fn install(models: &Path, repair: bool, progress: &Progress) -> anyhow::Result<()> {
    std::fs::create_dir_all(models)?;
    let agent = download_agent();
    progress.moved();
    let damaged = voice::damaged(models);
    let vad = models.join(voice::VAD_FILE);
    if repair || damaged.iter().any(|f| f == voice::VAD_FILE) {
        let part = models.join(format!("{}.part", voice::VAD_FILE));
        download(&agent, voice::VAD_URL, &part, voice::VAD, progress)?;
        let _ = std::fs::remove_file(&vad);
        orbiters_core::rename_retrying(&part, &vad)?;
    } else {
        progress.add(voice::VAD.size as i64);
    }
    if repair || damaged.iter().any(|f| f.starts_with(voice::ASR_DIR)) {
        let archive = models.join("parakeet.tar.bz2.part");
        download(&agent, voice::ASR_URL, &archive, voice::ASR_ARCHIVE, progress)?;
        let unpack = models.join(format!("{}.part", voice::ASR_DIR));
        if let Err(e) = unpack_archive(&archive, &unpack, progress) {
            let _ = std::fs::remove_dir_all(&unpack);
            // A whole archive that does not unpack as it should is not the expected one: it goes.
            let _ = std::fs::remove_file(&archive);
            return Err(e);
        }
        let target = models.join(voice::ASR_DIR);
        let _ = std::fs::remove_dir_all(&target);
        orbiters_core::rename_retrying(&unpack, &target)?;
        let _ = std::fs::remove_file(&archive);
    }
    let left = voice::damaged(models);
    anyhow::ensure!(left.is_empty(), "{}", orbiters_i18n::tr!("the models are still incomplete ({files})", files = left.join(", ")));
    progress.unpacking(1.0);
    Ok(())
}

/// Downloads `url` into `to` (a temporary file, kept between tries): it goes on from what `to` holds when the server
/// allows it, and must end at `expected`. A file that turns out not to be the expected one is deleted.
fn download(agent: &ureq::Agent, url: &str, to: &Path, expected: voice::Expected, progress: &Progress) -> anyhow::Result<()> {
    let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut have = file_size(to).unwrap_or(0);
    if have > expected.size {
        std::fs::remove_file(to)?;
        have = 0;
    }
    let mut hasher = Sha256::new();
    if have > 0 {
        // What came before counts, hashed again: the hash covers the whole file.
        let mut file = std::fs::File::open(to)?.take(have);
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            progress.add(n as i64);
        }
    }
    if have < expected.size {
        let mut request = agent.get(url);
        if have > 0 {
            request = request.header("Range", &format!("bytes={have}-"));
        }
        let response = request.call()?;
        let status = response.status().as_u16();
        anyhow::ensure!(status == 200 || status == 206, "{url}: HTTP {status}");
        let resumed = have > 0 && status == 206;
        if have > 0 && !resumed {
            // The server sends it whole: from the start.
            progress.add(-(have as i64));
            hasher = Sha256::new();
            have = 0;
        }
        let file = std::fs::OpenOptions::new().create(true).write(true).append(resumed).truncate(!resumed).open(to)?;
        let mut file = std::io::BufWriter::with_capacity(1 << 20, file);
        // At most what is left, and a byte more to see a larger file.
        let mut reader = response.into_body().into_with_config().limit(expected.size - have + 1).reader();
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = match reader.read(&mut buf) {
                Ok(n) => n,
                // The body went past what is left: it is longer than the file Orbiters knows.
                Err(e) if have + 1 > expected.size => {
                    drop(file);
                    let _ = std::fs::remove_file(to);
                    anyhow::bail!("{}", orbiters_i18n::tr!("{url}: larger than the {size} bytes expected ({error})", url = url, size = expected.size, error = e));
                }
                Err(e) => return Err(e.into()),
            };
            if n == 0 {
                break;
            }
            have += n as u64;
            if have > expected.size {
                drop(file);
                let _ = std::fs::remove_file(to);
                anyhow::bail!("{}", orbiters_i18n::tr!("{url}: larger than the {size} bytes expected: not the file Orbiters knows", url = url, size = expected.size));
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])?;
            progress.add(n as i64);
        }
        file.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    }
    anyhow::ensure!(have == expected.size, "{}", orbiters_i18n::tr!("{url}: the download stopped at {have} of {size} bytes (the next try goes on from there)", url = url, have = have, size = expected.size));
    let digest = format!("{:x}", hasher.finalize());
    eprintln!("voice: {name}: {have} bytes, SHA-256 {digest}");
    if let Some(pinned) = expected.sha256 {
        if digest != pinned {
            let _ = std::fs::remove_file(to);
            anyhow::bail!("{}", orbiters_i18n::tr!("{url}: not the file Orbiters knows (SHA-256 {digest})", url = url, digest = digest));
        }
    }
    Ok(())
}

/// Unpacks the recogniser's own files from `archive` into `into`: regular files of their size, flattened, each flushed
/// to the disk. An entry that would land outside the archive's folder, or any link, refuses the whole archive.
fn unpack_archive(archive: &Path, into: &Path, progress: &Progress) -> anyhow::Result<()> {
    let _ = std::fs::remove_dir_all(into);
    std::fs::create_dir_all(into)?;
    let total: u64 = voice::ASR_SIZES.iter().sum();
    let mut written = 0u64;
    let mut found = [false; voice::ASR_FILES.len()];
    let mut tar = tar::Archive::new(bzip2::read::BzDecoder::new(std::io::BufReader::new(std::fs::File::open(archive)?)));
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        anyhow::ensure!(
            path.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
            "{}",
            orbiters_i18n::tr!("the archive holds a path outside its folder ({path}): refused", path = path.display())
        );
        let kind = entry.header().entry_type();
        anyhow::ensure!(!kind.is_symlink() && !kind.is_hard_link(), "{}", orbiters_i18n::tr!("the archive holds a link ({path}): refused", path = path.display()));
        let parts: Vec<String> =
            path.components().filter_map(|c| if let Component::Normal(p) = c { Some(p.to_string_lossy().into_owned()) } else { None }).collect();
        let name = match parts.as_slice() {
            [dir, name] if dir == voice::ASR_DIR => name.clone(),
            [name] => name.clone(),
            _ => continue,
        };
        let Some(index) = voice::ASR_FILES.iter().position(|f| *f == name) else { continue };
        anyhow::ensure!(kind.is_file(), "{}", orbiters_i18n::tr!("{name} is not a file in the archive", name = name));
        let size = voice::ASR_SIZES[index];
        anyhow::ensure!(entry.size() == size, "{}", orbiters_i18n::tr!("{name} is {found} bytes in the archive, not the {size} Orbiters knows", name = name, found = entry.size(), size = size));
        anyhow::ensure!(!found[index], "{}", orbiters_i18n::tr!("{name} is twice in the archive", name = name));
        let mut file = std::fs::File::create(into.join(&name))?;
        let mut buf = vec![0u8; 256 * 1024];
        let mut left = size;
        let mut limited = (&mut entry).take(size);
        while left > 0 {
            let n = limited.read(&mut buf)?;
            anyhow::ensure!(n > 0, "{}", orbiters_i18n::tr!("{name} is cut short in the archive", name = name));
            file.write_all(&buf[..n])?;
            left -= n as u64;
            written += n as u64;
            progress.unpacking(written as f32 / total as f32);
        }
        file.sync_all()?;
        found[index] = true;
    }
    anyhow::ensure!(found.iter().all(|f| *f), "{}", orbiters_i18n::tr("the archive lacks some of the recogniser's files"));
    Ok(())
}

fn transcribe_files(files: &[String]) -> anyhow::Result<()> {
    let started = Instant::now();
    let recognizer = load_recognizer(&orbiters_core::paths::models_dir())?;
    println!("models loaded in {} ms", started.elapsed().as_millis());
    for file in files {
        let wave = sherpa_onnx::Wave::read(file).ok_or_else(|| anyhow::anyhow!("cannot read {file}"))?; // i18n: not shown (the `--transcribe` command line, for developers)
        anyhow::ensure!(wave.sample_rate() == RATE as i32, "{file}: {} Hz (16000 needed)", wave.sample_rate()); // i18n: not shown (the `--transcribe` command line, for developers)
        for seconds in [1usize, 2, 4, 6, 8, 12, 20, 30] {
            let count = (seconds * RATE).min(wave.samples().len());
            let (_, ms) = transcribe(&recognizer, &wave.samples()[..count]);
            println!("prefix {count} samples: {ms} ms");
            if count == wave.samples().len() { break; }
        }
        let mut live = LiveText::default();
        for end in (RATE..=wave.samples().len()).step_by(RATE) {
            let start = end.saturating_sub(RATE * 6);
            let now = Instant::now();
            let stream = recognizer.create_stream();
            stream.accept_waveform(RATE as i32, &wave.samples()[start..end]);
            recognizer.decode(&stream);
            if let Some(result) = stream.get_result() {
                let text = live.update(start as f32 / RATE as f32, result);
                println!("live end={} start={} ms={} text={text}", end / RATE, start / RATE, now.elapsed().as_millis());
            }
        }
        let (text, ms) = transcribe(&recognizer, wave.samples());
        println!("{file}: {:.1} s of audio in {ms} ms: {text}", wave.samples().len() as f32 / RATE as f32);
    }
    Ok(())
}

fn main() {
    // The release's version, for the libraries too (orbiters-version says why it is given here).
    orbiters_version::set(env!("ORBITERS_VERSION"));
    orbiters_core::panic_log::install();
    orbiters_core::leave_with_parent();
    // What it reports is written in the language of the app that reads it.
    orbiters_core::settings::Settings::speak_saved_language();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("transcribe") {
        if let Err(e) = transcribe_files(&args[1..]) {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
        return;
    }
    let (tx, rx) = mpsc::channel::<Input>();
    let commands = tx.clone();
    let _ = std::thread::Builder::new().name("stdin".into()).spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            match serde_json::from_str::<VoiceCommand>(&line) {
                Ok(c) => {
                    if commands.send(Input::Command(c)).is_err() {
                        return;
                    }
                }
                Err(e) => eprintln!("voice: unreadable command ({} bytes): {e}", line.len()),
            }
        }
        let _ = commands.send(Input::StdinClosed);
    });
    let mut worker = Worker::new(tx);
    while worker.pass(&rx, AUDIO_PER_PASS) {}
    // An install or a recogniser still loading may hold a thread: the process ends here all the same.
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A worker whose microphone (capture `generation` 1) is "open", without a device: only the loop is exercised.
    fn listening_worker() -> (Worker, mpsc::Receiver<Input>) {
        let (tx, rx) = mpsc::channel();
        let mut worker = Worker::new(tx);
        worker.mic = Some(Mic { stop: Arc::new(AtomicBool::new(false)), generation: 1 });
        (worker, rx)
    }

    #[test]
    fn one_pass_takes_every_packet_waiting_and_none_of_an_old_microphone() {
        // N4N0-5: the loop took one packet a pass; a full queue is emptied by one pass now.
        let (mut worker, rx) = listening_worker();
        let drops = worker.drops.clone();
        for _ in 0..AUDIO_QUEUE {
            offer(&worker.audio_tx, &drops, 1, vec![0.0; 160]).unwrap();
        }
        assert!(worker.pass(&rx, AUDIO_PER_PASS));
        assert_eq!(worker.taken, AUDIO_QUEUE as u64, "the whole queue in one pass");
        // A packet of the capture being replaced is taken off the queue, not heard.
        offer(&worker.audio_tx, &drops, 0, vec![0.0; 160]).unwrap();
        offer(&worker.audio_tx, &drops, 1, vec![0.0; 160]).unwrap();
        assert!(worker.pass(&rx, AUDIO_PER_PASS));
        assert_eq!(worker.taken, AUDIO_QUEUE as u64 + 1);
        // A full queue drops, and counts what it dropped.
        for _ in 0..AUDIO_QUEUE + 3 {
            offer(&worker.audio_tx, &drops, 1, vec![0.0; 160]).unwrap();
        }
        assert_eq!((drops.packets.load(Ordering::Relaxed), drops.frames.load(Ordering::Relaxed)), (3, 480));
    }

    /// Runs the loop against a stand-in microphone sending `chunks` packets 10 ms apart, as WASAPI's shared mode does;
    /// the loop takes at most `most` packets a pass. What was dropped.
    fn loop_drops(most: usize, chunks: u64) -> (u64, u64) {
        let (mut worker, rx) = listening_worker();
        let (audio, drops) = (worker.audio_tx.clone(), worker.drops.clone());
        let producer = std::thread::spawn(move || {
            let start = Instant::now();
            for i in 0..chunks {
                let due = start + Duration::from_millis(10 * i);
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
                offer(&audio, &drops, 1, vec![0.0; 160]).unwrap();
            }
        });
        while !producer.is_finished() {
            assert!(worker.pass(&rx, most));
        }
        producer.join().unwrap();
        while worker.take_audio(most) > 0 {}
        (worker.taken, worker.drops.packets.load(Ordering::Relaxed))
    }

    #[test]
    fn the_loop_keeps_up_with_the_microphone() {
        // N4N0-5, measured through the worker's own loop: one packet a pass (as before) against all of them.
        let chunks = 300;
        let (old_taken, old_dropped) = loop_drops(1, chunks);
        let (taken, dropped) = loop_drops(AUDIO_PER_PASS, chunks);
        eprintln!("one packet a pass: {old_taken} taken, {old_dropped} dropped of {chunks}; all of them: {taken} taken, {dropped} dropped");
        assert_eq!(taken + dropped, chunks);
        // A busy test machine may hold the loop back 80 ms at worst once or twice: never a share of the sound.
        assert!(dropped * 100 <= chunks, "{dropped} of {chunks} dropped");
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-voice-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A bzip2 tar holding `entries` (path, content, kind).
    fn archive(dir: &Path, entries: &[(&str, &[u8], tar::EntryType)]) -> PathBuf {
        let path = dir.join("a.tar.bz2");
        let file = std::fs::File::create(&path).unwrap();
        let mut builder = tar::Builder::new(bzip2::write::BzEncoder::new(file, bzip2::Compression::fast()));
        for (name, data, kind) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(*kind);
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            // Written raw: the builder's own path checks would refuse what a hostile archive holds.
            if kind.is_symlink() || kind.is_hard_link() {
                header.as_old_mut().linkname[..9].copy_from_slice(b"elsewhere");
            }
            let bytes = name.as_bytes();
            header.as_old_mut().name[..bytes.len()].copy_from_slice(bytes);
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
        path
    }

    #[test]
    fn archives_with_links_or_outside_paths_are_refused() {
        let dir = temp("unpack");
        let progress = Progress::new();
        let into = dir.join("out");
        let link = archive(&dir, &[(&format!("{}/encoder.int8.onnx", voice::ASR_DIR), b"", tar::EntryType::Symlink)]);
        let e = unpack_archive(&link, &into, &progress).unwrap_err();
        assert!(e.to_string().contains("link"), "{e}");
        let hard = archive(&dir, &[("tokens.txt", b"", tar::EntryType::Link)]);
        assert!(unpack_archive(&hard, &into, &progress).unwrap_err().to_string().contains("link"));
        let outside = archive(&dir, &[("../tokens.txt", b"x", tar::EntryType::Regular)]);
        assert!(unpack_archive(&outside, &into, &progress).unwrap_err().to_string().contains("outside"));
        assert!(!dir.join("tokens.txt").exists(), "nothing was written outside");
        // A model file of another size than the one known is refused before anything of it is written.
        let small = archive(&dir, &[(&format!("{}/tokens.txt", voice::ASR_DIR), b"not the tokens", tar::EntryType::Regular)]);
        assert!(unpack_archive(&small, &into, &progress).unwrap_err().to_string().contains("bytes"));
        // Other files of the archive (its test sounds) are left out; missing model files fail.
        let other = archive(&dir, &[(&format!("{}/test_wavs/0.wav", voice::ASR_DIR), b"RIFF", tar::EntryType::Regular)]);
        assert!(unpack_archive(&other, &into, &progress).unwrap_err().to_string().contains("lacks"));
        assert!(!into.join("0.wav").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn progress_counts_downloads_then_unpacking() {
        let p = Progress::new();
        p.add(voice::DOWNLOAD_BYTES as i64 / 2);
        assert_eq!(p.shown.load(Ordering::Relaxed), 45);
        p.add(-(voice::DOWNLOAD_BYTES as i64 / 2));
        assert_eq!(p.done.load(Ordering::Relaxed), 0);
        p.unpacking(0.5);
        assert_eq!(p.shown.load(Ordering::Relaxed), 95);
        assert!(p.idle_for() < Duration::from_secs(1));
    }

    /// A one-request HTTP server on this computer sending `body`, from where a `Range` asks when `ranges`.
    fn serve(body: Vec<u8>, ranges: bool) -> (String, std::thread::JoinHandle<Option<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/model", listener.local_addr().unwrap().port());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") && socket.read(&mut byte).unwrap_or(0) == 1 {
                request.push(byte[0]);
            }
            let request = String::from_utf8_lossy(&request).to_lowercase();
            let range = request.lines().find_map(|l| l.strip_prefix("range: bytes=")).and_then(|r| r.trim_end_matches('-').parse::<usize>().ok());
            let (status, from) = match range {
                Some(from) if ranges => ("206 Partial Content", from),
                _ => ("200 OK", 0),
            };
            let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len() - from);
            socket.write_all(head.as_bytes()).unwrap();
            socket.write_all(&body[from..]).unwrap();
            range.map(|r| r.to_string())
        });
        (url, server)
    }

    fn sha256(data: &[u8]) -> &'static str {
        Box::leak(format!("{:x}", Sha256::digest(data)).into_boxed_str())
    }

    #[test]
    fn a_cut_download_goes_on_from_where_it_stopped() {
        let dir = temp("download");
        let body: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let part = dir.join("model.part");
        std::fs::write(&part, &body[..40_000]).unwrap();
        let (url, server) = serve(body.clone(), true);
        let expected = voice::Expected { size: body.len() as u64, sha256: Some(sha256(&body)) };
        download(&ureq::Agent::new_with_defaults(), &url, &part, expected, &Progress::new()).unwrap();
        assert_eq!(server.join().unwrap().as_deref(), Some("40000"), "it asked for the rest only");
        assert_eq!(std::fs::read(&part).unwrap(), body);
        // A server that sends it whole again: from the start, and still right.
        std::fs::write(&part, &body[..10]).unwrap();
        let (url, server) = serve(body.clone(), false);
        download(&ureq::Agent::new_with_defaults(), &url, &part, expected, &Progress::new()).unwrap();
        server.join().unwrap();
        assert_eq!(std::fs::read(&part).unwrap(), body);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_download_that_is_not_the_known_file_is_refused_and_deleted() {
        let dir = temp("refused");
        let body = vec![7u8; 5000];
        let part = dir.join("model.part");
        // Larger than the size Orbiters knows.
        let (url, server) = serve(body.clone(), true);
        let small = voice::Expected { size: 4000, sha256: None };
        let e = download(&ureq::Agent::new_with_defaults(), &url, &part, small, &Progress::new()).unwrap_err();
        server.join().unwrap();
        assert!(e.to_string().contains("larger"), "{e}");
        assert!(!part.exists());
        // Of the right size, but not the right bytes.
        let (url, server) = serve(body.clone(), true);
        let pinned = voice::Expected { size: 5000, sha256: Some(sha256(b"something else")) };
        let e = download(&ureq::Agent::new_with_defaults(), &url, &part, pinned, &Progress::new()).unwrap_err();
        server.join().unwrap();
        assert!(e.to_string().contains("SHA-256"), "{e}");
        assert!(!part.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::*;

    #[test]
    #[ignore = "requires installed CPU recognizer and ORBITERS_VOICE_FIXTURE, runs in real time without a microphone"]
    fn review_realtime_live_fixture() {
        let wave = sherpa_onnx::Wave::read(&std::env::var("ORBITERS_VOICE_FIXTURE").unwrap()).unwrap();
        assert_eq!(wave.sample_rate(), RATE as i32);
        let (tx, _) = mpsc::channel();
        let mut worker = Worker::new(tx);
        let models = orbiters_core::paths::models_dir();
        worker.vad = Some(load_vad(&models).unwrap());
        worker.recognizer = Recognizer::Ready(Arc::new(Mutex::new(load_recognizer(&models).unwrap())));
        worker.listen = Some(Listen { hold: true, since: Instant::now(), id: 41 });
        worker.partial_id.store(41, Ordering::Relaxed);
        let began = Instant::now();
        let mut previous = String::new();
        let mut updates = 0;
        for (i, chunk) in wave.samples().chunks(WINDOW).enumerate() {
            worker.audio(chunk.to_vec());
            let snapshot = format!("{:?}", worker.live.lock().unwrap().tokens);
            if snapshot != previous && snapshot != "[]" {
                eprintln!("live_probe at_ms={} tokens={snapshot}", began.elapsed().as_millis());
                updates += 1; previous = snapshot;
            }
            let due = Duration::from_secs_f64((i + 1) as f64 * WINDOW as f64 / RATE as f64);
            std::thread::sleep(due.saturating_sub(began.elapsed()));
        }
        let live = worker.live.lock().unwrap();
        assert!(live.tokens.first().unwrap().0 < 4.0, "prefix was retained");
        assert!(live.tokens.last().unwrap().0 > 24.0, "words kept arriving after eight seconds");
        assert!(updates >= 15, "sustained live updates: {updates}");
        drop(live);
        worker.partial_id.store(0, Ordering::Relaxed);
        worker.decoder.clear();
        worker.listen = None;
        let before = format!("{:?}", worker.live.lock().unwrap().tokens);
        std::thread::sleep(Duration::from_millis(750));
        assert_eq!(before, format!("{:?}", worker.live.lock().unwrap().tokens), "cancelled decode cannot revise live words");
        assert!(worker.mic.is_none());
    }
    #[test]
    fn review_long_sentence_never_clears_live_identity() {
        let (tx, _) = mpsc::channel(); let mut worker = Worker::new(tx);
        worker.listen = Some(Listen { hold: true, since: Instant::now(), id: 41 });
        worker.partial_id.store(41, Ordering::Relaxed);
        for seconds in [8, 9, 12, 20, 30] {
            worker.held = vec![0.0; RATE * seconds]; worker.partial();
            assert_eq!(worker.partial_id.load(Ordering::Relaxed), 41, "live words disappeared at {seconds}s");
        }
    }

    #[test]
    fn review_loading_queue_is_latest_only() {
        let (tx, _) = mpsc::channel();
        let mut worker = Worker::new(tx);
        for id in 1..=100 { worker.sentence(vec![0.0; RATE], true, id); }
        assert_eq!(worker.waiting.len(), 1);
        assert_eq!(worker.waiting[0].listen, 100);
        worker.sentence(vec![0.0; RATE], false, 0);
        assert_eq!(worker.waiting[0].listen, 100, "wake work cannot displace the active final");
        worker.command(VoiceCommand::Cancel);
        assert!(worker.waiting.is_empty());
    }


    #[test]
    fn review_long_live_text_keeps_prefix_in_all_five_languages() {
        for words in ["one two three four five six seven eight nine ten", "un deux trois quatre cinq six sept huit neuf dix", "uno dos tres cuatro cinco seis siete ocho nueve diez", "um dois tres quatro cinco seis sete oito nove dez", "一 二 三 四 五 六 七 八 九 十"] {
            let words: Vec<_> = words.split_whitespace().collect();
            let mut live = LiveText::default();
            for end in 1usize..=30 {
                let start = end.saturating_sub(6);
                let result = sherpa_onnx::OfflineRecognizerResult {
                    text: String::new(),
                    tokens: (start..end).map(|i| format!("▁{}", words[i % 10])).collect(),
                    timestamps: Some((0..end-start).map(|i| i as f32).collect()), durations: None,
                };
                let text = live.update(start as f32, result);
                assert_eq!(text, (0..end).map(|i| words[i % 10]).collect::<Vec<_>>().join(" "));
            }
            let kept = live.tokens.clone();
            live.update(24.0, sherpa_onnx::OfflineRecognizerResult {
                text: String::new(), tokens: Vec::new(), timestamps: Some(Vec::new()), durations: None,
            });
            assert_eq!(live.tokens, kept, "an empty decode retains the useful words");
        }
    }

    #[test]
    fn review_decoder_overload_keeps_only_latest_and_final_priority() {
        let decoder = Decoder::new();
        let (begun, started) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        decoder.submit(false, Box::new(move || { begun.send(()).unwrap(); blocked.recv().unwrap(); }));
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        let (tx, rx) = mpsc::channel();
        for i in 0..100 {
            let tx = tx.clone();
            decoder.submit(true, Box::new(move || { tx.send(i).unwrap(); }));
        }
        let spare = tx.clone();
        decoder.submit(false, Box::new(move || { spare.send(999).unwrap(); }));
        release.send(()).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), 99);
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        let (begun, started) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        decoder.submit(false, Box::new(move || { begun.send(()).unwrap(); blocked.recv().unwrap(); }));
        started.recv().unwrap();
        decoder.submit(true, Box::new(move || { tx.send(100).unwrap(); }));
        decoder.clear(); release.send(()).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
    }

    #[test]
    #[ignore = "requires installed Silero and ORBITERS_VOICE_FIXTURE (16 kHz PCM), never a microphone"]
    fn fixture_vad_ends_one_automatic_sentence() {
        let path = std::env::var("ORBITERS_VOICE_FIXTURE").unwrap();
        let wave = sherpa_onnx::Wave::read(&path).unwrap();
        assert_eq!(wave.sample_rate(), RATE as i32);
        let (tx, _) = mpsc::channel();
        let mut worker = Worker::new(tx);
        worker.vad = Some(load_vad(&orbiters_core::paths::models_dir()).unwrap());
        worker.listen = Some(Listen { hold: false, since: Instant::now(), id: 41 });
        for chunk in wave.samples().chunks(WINDOW) { worker.audio(chunk.to_vec()); }
        for _ in 0..40 { worker.audio(vec![0.0; WINDOW]); }
        assert!(worker.listen.is_none(), "Silero ended the sentence");
        assert_eq!(worker.waiting.len(), 1, "exactly one final decode request");
        assert_eq!(worker.waiting[0].listen, 41);
        assert!(worker.mic.is_none(), "fixture never opened a microphone");
    }
}
