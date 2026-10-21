//! Orb's ears: the voice worker of Orbiters desktop (protocol: `orbiters_core::voice`). The microphone is open only
//! while Orb listens or the wake word is on. A held sentence (the Talk button) is transcribed when it is let go; any
//! other sentence ends where Silero VAD hears it end. Transcription runs here, on the CPU, with NVIDIA Parakeet
//! through sherpa-onnx. Audio is kept in memory only until its sentence is transcribed.
//!
//! `orbiters-voice transcribe <file.wav>…` transcribes files (a check of the models and their speed).

use orbiters_audio::AudioSource;
use orbiters_core::voice::{self, VoiceCommand, VoiceEvent};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig, SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
use std::collections::VecDeque;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const RATE: usize = voice::SAMPLE_RATE as usize;
/// Silero VAD's window at 16 kHz.
const WINDOW: usize = 512;
/// A sentence for Orb that never starts is given up after this.
const NO_SPEECH: Duration = Duration::from_secs(8);
/// The longest sentence (held or not).
const LONGEST: Duration = Duration::from_secs(30);
/// Sound kept from before Orb starts listening, while the microphone is already open (the wake word).
const PREROLL: usize = RATE * 3 / 10;
/// Threads of the recogniser: VRChat needs the rest.
const THREADS: i32 = 2;

enum Input {
    Command(VoiceCommand),
    Audio(Vec<f32>),
    MicOpened(String),
    MicClosed(Option<String>),
    StdinClosed,
}

fn emit(event: &VoiceEvent) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", event.to_line());
    let _ = out.flush();
}

struct Ears {
    recognizer: OfflineRecognizer,
    vad: VoiceActivityDetector,
}

impl Ears {
    fn load(models: &Path) -> anyhow::Result<Self> {
        anyhow::ensure!(voice::installed(models), "Orb's speech models are not installed in {}", models.display());
        let asr = models.join(voice::ASR_DIR);
        let path = |p: PathBuf| Some(p.to_string_lossy().into_owned());
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.transducer = OfflineTransducerModelConfig {
            encoder: path(asr.join("encoder.int8.onnx")),
            decoder: path(asr.join("decoder.int8.onnx")),
            joiner: path(asr.join("joiner.int8.onnx")),
        };
        config.model_config.tokens = path(asr.join("tokens.txt"));
        config.model_config.model_type = Some("nemo_transducer".into());
        config.model_config.num_threads = THREADS;
        config.model_config.provider = Some("cpu".into());
        config.decoding_method = Some("greedy_search".into());
        let recognizer = OfflineRecognizer::create(&config).ok_or_else(|| anyhow::anyhow!("the speech recogniser did not load"))?;
        let vad = VoiceActivityDetector::create(
            &VadModelConfig {
                silero_vad: SileroVadModelConfig {
                    model: path(models.join(voice::VAD_FILE)),
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
        .ok_or_else(|| anyhow::anyhow!("voice activity detection did not load"))?;
        Ok(Self { recognizer, vad })
    }

    fn transcribe(&self, samples: &[f32]) -> (String, u32) {
        let started = Instant::now();
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(RATE as i32, samples);
        self.recognizer.decode(&stream);
        let text = stream.get_result().map(|r| r.text.trim().to_string()).unwrap_or_default();
        (text, started.elapsed().as_millis() as u32)
    }
}

struct Mic {
    stop: Arc<AtomicBool>,
}

impl Drop for Mic {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

struct Listen {
    hold: bool,
    since: Instant,
}

struct Worker {
    tx: mpsc::Sender<Input>,
    ears: Option<Ears>,
    mic_id: Option<String>,
    mic: Option<Mic>,
    wake_word: bool,
    listen: Option<Listen>,
    /// The held sentence so far.
    held: Vec<f32>,
    /// Samples waiting for a whole VAD window.
    pending: Vec<f32>,
    preroll: VecDeque<f32>,
    speaking: bool,
    level: (f32, usize, Option<Instant>),
}

impl Worker {
    fn wants_mic(&self) -> bool {
        self.ears.is_some() && (self.wake_word || self.listen.is_some())
    }

    /// Opens the microphone when something listens, closes it when nothing does.
    fn follow_mic(&mut self) {
        if self.wants_mic() && self.mic.is_none() {
            let stop = Arc::new(AtomicBool::new(false));
            let (tx, id, s) = (self.tx.clone(), self.mic_id.clone(), stop.clone());
            let _ = std::thread::Builder::new().name("mic".into()).spawn(move || {
                let (ready, audio) = (tx.clone(), tx.clone());
                let result = orbiters_audio::capture(
                    &AudioSource::Mic(id),
                    Some(voice::SAMPLE_RATE),
                    &s,
                    |name, _| {
                        let _ = ready.send(Input::MicOpened(name.to_string()));
                        Ok(())
                    },
                    |_, frames, samples| {
                        let chunk = samples.map(<[f32]>::to_vec).unwrap_or_else(|| vec![0.0; frames as usize]);
                        audio.send(Input::Audio(chunk)).map_err(|_| anyhow::anyhow!("the worker stopped"))
                    },
                );
                let _ = tx.send(Input::MicClosed(result.err().map(|e| format!("{e:#}"))));
            });
            self.mic = Some(Mic { stop });
        } else if !self.wants_mic() && self.mic.is_some() {
            self.mic = None;
            self.pending.clear();
            self.preroll.clear();
            if let Some(ears) = &self.ears {
                ears.vad.reset();
            }
            self.speaking = false;
        }
    }

    fn command(&mut self, command: VoiceCommand) -> bool {
        match command {
            VoiceCommand::Start { models, mic, wake_word } => {
                let started = Instant::now();
                match Ears::load(&models) {
                    Ok(ears) => {
                        self.ears = Some(ears);
                        emit(&VoiceEvent::Ready { engine: "Parakeet TDT 0.6B v3 (int8, sherpa-onnx)".into(), load_ms: started.elapsed().as_millis() as u32 });
                    }
                    Err(e) => emit(&VoiceEvent::Error { message: format!("{e:#}") }),
                }
                self.mic_id = mic;
                self.wake_word = wake_word;
            }
            VoiceCommand::Listen { hold } => {
                if let Some(ears) = &self.ears {
                    // A fresh sentence: what the VAD was following belongs to before.
                    ears.vad.reset();
                }
                self.pending.clear();
                self.speaking = false;
                self.held = if hold { self.preroll.iter().copied().collect() } else { Vec::new() };
                self.listen = Some(Listen { hold, since: Instant::now() });
            }
            VoiceCommand::Finish => {
                if self.listen.as_ref().is_some_and(|l| l.hold) {
                    self.listen = None;
                    let held = std::mem::take(&mut self.held);
                    self.heard(&held, true);
                }
            }
            VoiceCommand::Cancel => {
                self.listen = None;
                self.held.clear();
            }
            VoiceCommand::WakeWord { on } => self.wake_word = on,
            VoiceCommand::Install { models } => {
                match install(&models) {
                    Ok(()) => emit(&VoiceEvent::Installed),
                    Err(e) => emit(&VoiceEvent::Error { message: format!("Could not install Orb's speech models: {e:#}") }),
                }
                return false;
            }
            VoiceCommand::Quit => return false,
        }
        self.follow_mic();
        true
    }

    fn audio(&mut self, samples: Vec<f32>) {
        if self.ears.is_none() {
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
        if self.listen.as_ref().is_some_and(|l| l.hold) {
            if self.held.len() < LONGEST.as_secs() as usize * RATE {
                self.held.extend_from_slice(&samples);
            }
            return;
        }
        self.preroll.extend(samples.iter().copied());
        let excess = self.preroll.len().saturating_sub(PREROLL);
        self.preroll.drain(..excess);
        self.pending.extend_from_slice(&samples);
        let mut segments = Vec::new();
        let speaking = {
            let ears = self.ears.as_ref().expect("checked above");
            while self.pending.len() >= WINDOW {
                ears.vad.accept_waveform(&self.pending[..WINDOW]);
                self.pending.drain(..WINDOW);
                while !ears.vad.is_empty() {
                    if let Some(segment) = ears.vad.front() {
                        segments.push(segment.samples().to_vec());
                    }
                    ears.vad.pop();
                }
            }
            ears.vad.detected()
        };
        if speaking != self.speaking {
            self.speaking = speaking;
            emit(&VoiceEvent::Speech { active: speaking });
        }
        for segment in segments {
            let for_orb = self.listen.as_ref().is_some_and(|l| !l.hold);
            if for_orb {
                self.listen = None;
                self.heard(&segment, true);
                self.follow_mic();
            } else if self.wake_word {
                self.heard(&segment, false);
            }
        }
    }

    fn heard(&self, samples: &[f32], for_orb: bool) {
        let Some(ears) = &self.ears else { return };
        // Less than a quarter of a second holds no word.
        let (text, decode_ms) = if samples.len() < RATE / 4 { (String::new(), 0) } else { ears.transcribe(samples) };
        emit(&VoiceEvent::Heard { text, for_orb, seconds: samples.len() as f32 / RATE as f32, decode_ms });
    }

    /// A sentence for Orb that never started, or went on too long.
    fn tick(&mut self) {
        let Some(listen) = &self.listen else { return };
        let waited = listen.since.elapsed();
        if listen.hold {
            if waited > LONGEST {
                self.listen = None;
                let held = std::mem::take(&mut self.held);
                self.heard(&held, true);
                self.follow_mic();
            }
        } else if (waited > NO_SPEECH && !self.speaking) || waited > LONGEST {
            self.listen = None;
            emit(&VoiceEvent::Heard { text: String::new(), for_orb: true, seconds: 0.0, decode_ms: 0 });
            self.follow_mic();
        }
    }
}

/// Downloads the models into `models`: Silero VAD, then Parakeet's archive, unpacked in place. Files land under a
/// temporary name first, so a cut download is never taken for a model.
fn install(models: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(models)?;
    let agent = orbiters_core::http_agent(Some(Duration::from_secs(20)), None);
    let mut done = 0u64;
    let mut last = -1i32;
    let mut progress = |n: u64| {
        done += n;
        let fraction = (done as f64 / voice::DOWNLOAD_BYTES as f64).min(1.0) as f32;
        let percent = (fraction * 100.0) as i32;
        if percent != last {
            last = percent;
            emit(&VoiceEvent::Installing { fraction });
        }
    };
    let vad = models.join(voice::VAD_FILE);
    if !vad.is_file() {
        let part = models.join(format!("{}.part", voice::VAD_FILE));
        download(&agent, voice::VAD_URL, &part, &mut progress)?;
        std::fs::rename(&part, &vad)?;
    } else {
        progress(2_313_101);
    }
    if !voice::installed(models) {
        let archive = models.join("parakeet.tar.bz2.part");
        download(&agent, voice::ASR_URL, &archive, &mut progress)?;
        let unpack = models.join(format!("{}.part", voice::ASR_DIR));
        let _ = std::fs::remove_dir_all(&unpack);
        std::fs::create_dir_all(&unpack)?;
        let mut tar = tar::Archive::new(bzip2::read::BzDecoder::new(std::io::BufReader::new(std::fs::File::open(&archive)?)));
        // Only the recogniser's own files, flattened: nothing in the archive chooses where it is written.
        for entry in tar.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.into_owned();
            let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else { continue };
            if voice::ASR_FILES.contains(&name.as_str()) {
                entry.unpack(unpack.join(&name))?;
            }
        }
        let target = models.join(voice::ASR_DIR);
        let _ = std::fs::remove_dir_all(&target);
        std::fs::rename(&unpack, &target)?;
        let _ = std::fs::remove_file(&archive);
    }
    anyhow::ensure!(voice::installed(models), "the downloaded models are incomplete");
    Ok(())
}

fn download(agent: &ureq::Agent, url: &str, to: &Path, progress: &mut impl FnMut(u64)) -> anyhow::Result<()> {
    let response = agent.get(url).call()?;
    anyhow::ensure!(response.status().is_success(), "{url}: HTTP {}", response.status());
    let mut reader = response.into_body().into_with_config().limit(u64::MAX).reader();
    let mut file = std::io::BufWriter::new(std::fs::File::create(to)?);
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        progress(n as u64);
    }
    file.flush()?;
    Ok(())
}

fn transcribe_files(files: &[String]) -> anyhow::Result<()> {
    let started = Instant::now();
    let ears = Ears::load(&orbiters_core::paths::models_dir())?;
    println!("models loaded in {} ms", started.elapsed().as_millis());
    for file in files {
        let wave = sherpa_onnx::Wave::read(file).ok_or_else(|| anyhow::anyhow!("cannot read {file}"))?;
        anyhow::ensure!(wave.sample_rate() == RATE as i32, "{file}: {} Hz (16000 needed)", wave.sample_rate());
        let (text, ms) = ears.transcribe(wave.samples());
        println!("{file}: {:.1} s of audio in {ms} ms: {text}", wave.samples().len() as f32 / RATE as f32);
    }
    Ok(())
}

fn main() {
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
                Err(e) => eprintln!("voice: unreadable command ({e}): {line}"),
            }
        }
        let _ = commands.send(Input::StdinClosed);
    });
    let mut worker = Worker {
        tx,
        ears: None,
        mic_id: None,
        mic: None,
        wake_word: false,
        listen: None,
        held: Vec::new(),
        pending: Vec::new(),
        preroll: VecDeque::new(),
        speaking: false,
        level: (0.0, 0, None),
    };
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Input::Command(c)) => {
                if !worker.command(c) {
                    break;
                }
            }
            Ok(Input::Audio(samples)) => worker.audio(samples),
            Ok(Input::MicOpened(name)) => emit(&VoiceEvent::Mic { name: Some(name) }),
            Ok(Input::MicClosed(error)) => {
                worker.mic = None;
                emit(&VoiceEvent::Mic { name: None });
                if let Some(message) = error {
                    emit(&VoiceEvent::Error { message: format!("The microphone could not be opened: {message}") });
                    // Whoever listened hears nothing: say so instead of waiting.
                    if worker.listen.take().is_some() {
                        emit(&VoiceEvent::Heard { text: String::new(), for_orb: true, seconds: 0.0, decode_ms: 0 });
                    }
                }
            }
            Ok(Input::StdinClosed) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        worker.tick();
    }
}
