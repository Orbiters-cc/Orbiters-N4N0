//! Orbiters desktop capture worker.
//!
//! * `orbiters-capture worker` — driven by the app: JSON-lines commands on stdin, events on stdout.
//! * `orbiters-capture record --source <spout[:name] | raw:<folder> | test> [--out <folder>] [--quality …] [--frames N]`
//! * `orbiters-capture send-test [--name <sender>] [--width W] [--height H] [--fps F] [--depth-every N] [--seconds S]`
//! * `orbiters-capture verify <raw capture folder> [--frames N]`
//! * `orbiters-capture list-sources`

mod audio;
mod performance;
mod session;
mod sources;
mod tracking;
mod verify;

use crossbeam_channel::RecvTimeoutError;
use orbiters_contracts::manifest::{CaptureMode, ConsentInfo, StorageQuality};
use orbiters_core::protocol::{AudioDeviceInfo, PerformanceSpec, SourceSpec, SpoutSourceInfo, StartCapture, WorkerCommand, WorkerEvent};
use orbiters_core::settings::KeyframeSettings;
use session::CaptureSession;
use sources::{FrameSource, Next, RawSequenceSource, TestPatternSource};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("worker") => run_worker(),
        Some("record") => cli_record(&args[1..]),
        Some("send-test") => cli_send_test(&args[1..]),
        Some("verify") => cli_verify(&args[1..]),
        Some("list-sources") => {
            for s in list_sources() {
                println!("{}  {}x{}  DXGI {}  {}", s.name, s.width, s.height, s.format, s.description);
            }
            for m in list_mics() {
                println!("mic: {}{}  {}", m.name, if m.default { " (default)" } else { "" }, m.id);
            }
            Ok(())
        }
        _ => {
            eprintln!("{}", include_str!("usage.txt"));
            Ok(())
        }
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn list_sources() -> Vec<SpoutSourceInfo> {
    #[cfg(windows)]
    {
        orbiters_spout::list_senders()
            .into_iter()
            .filter_map(|n| orbiters_spout::sender_info(&n))
            .map(|i| SpoutSourceInfo { name: i.name, width: i.width, height: i.height, format: i.format, description: i.description })
            .collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

fn list_mics() -> Vec<AudioDeviceInfo> {
    audio::list_mics().into_iter().map(|d| AudioDeviceInfo { id: d.id, name: d.name, default: d.default }).collect()
}

fn open_source(spec: &SourceSpec) -> anyhow::Result<Box<dyn FrameSource>> {
    Ok(match spec {
        #[cfg(windows)]
        SourceSpec::Spout { name } => Box::new(sources::SpoutSource::new(name)?),
        #[cfg(not(windows))]
        SourceSpec::Spout { .. } => anyhow::bail!("Spout is only available on Windows; use the PipeWire source on Linux"),
        SourceSpec::RawSequence { path, realtime } => Box::new(RawSequenceSource::open(path, *realtime)?),
        SourceSpec::TestPattern { width, height, fps, depth_every, frames } => {
            Box::new(TestPatternSource::new(*width, *height, *fps, *depth_every, *frames, true))
        }
    })
}

fn parse_source(text: &str) -> anyhow::Result<SourceSpec> {
    Ok(if text == "test" {
        SourceSpec::TestPattern { width: 1920, height: 1080, fps: 30.0, depth_every: 4, frames: 300 }
    } else if let Some(name) = text.strip_prefix("spout") {
        SourceSpec::Spout { name: name.trim_start_matches(':').to_string() }
    } else if let Some(path) = text.strip_prefix("raw:") {
        SourceSpec::RawSequence { path: PathBuf::from(path), realtime: false }
    } else {
        anyhow::bail!("unknown source {text:?} (spout[:name], raw:<folder>, test)")
    })
}

fn parse_quality(text: Option<&str>) -> StorageQuality {
    match text {
        Some("lossless") => StorageQuality::Lossless,
        Some("compact") => StorageQuality::Compact,
        _ => StorageQuality::High,
    }
}

/// Drives a session until the source ends, `max_frames` frames or `max_seconds` pass.
fn run_session(source: &mut dyn FrameSource, session: &mut CaptureSession, max_frames: Option<u64>, max_seconds: Option<f64>) -> anyhow::Result<String> {
    let started = Instant::now();
    let mut last_print = Instant::now();
    loop {
        let mut err = None;
        match source.next(Duration::from_millis(100), &mut |v| {
            if let Err(e) = session.process(v) {
                err = Some(e);
            }
        })? {
            Next::End => return Ok("end of input".into()),
            Next::Frame | Next::Timeout => {}
        }
        if let Some(e) = err {
            return Err(e);
        }
        if max_frames.is_some_and(|m| session.stats.frames >= m) {
            return Ok("frame limit".into());
        }
        if max_seconds.is_some_and(|m| started.elapsed().as_secs_f64() >= m) {
            return Ok("time limit".into());
        }
        if last_print.elapsed() >= Duration::from_secs(1) {
            let s = session.refresh_stats();
            eprintln!(
                "{:>6} frames  {:>5.1} fps  strip ok {:>6}  keyframes {:>5}  depth {:>5}  decode {:.2} ms  {:.1} MB",
                s.frames,
                s.fps,
                s.strip_ok,
                s.keyframes,
                s.depth_frames,
                s.decode_ms,
                s.bytes as f64 / 1e6
            );
            last_print = Instant::now();
        }
    }
}

fn cli_record(args: &[String]) -> anyhow::Result<()> {
    let source_spec = parse_source(flag(args, "--source").unwrap_or("spout"))?;
    let out = flag(args, "--out").map(PathBuf::from).unwrap_or_else(orbiters_core::paths::default_sessions_dir);
    let cmd = StartCapture {
        source: source_spec.clone(),
        sessions_dir: out,
        mode: CaptureMode::RecordOnly,
        quality: parse_quality(flag(args, "--quality")),
        keyframes: KeyframeSettings::default(),
        world: None,
        consent: ConsentInfo::default(),
        folder_name: flag(args, "--name").map(String::from),
        performance: PerformanceSpec {
            mic: flag(args, "--mic").map(String::from),
            game_audio_pid: flag(args, "--game-audio-pid").and_then(|p| p.parse().ok()),
            tracking_hz: flag(args, "--tracking-hz").and_then(|p| p.parse().ok()),
            calibration: None,
        },
    };
    let mut source = match &source_spec {
        SourceSpec::TestPattern { width, height, fps, depth_every, frames } => {
            Box::new(TestPatternSource::new(*width, *height, *fps, *depth_every, *frames, false)) as Box<dyn FrameSource>
        }
        other => open_source(other)?,
    };
    let mut session = CaptureSession::start(&cmd, source.describe(), &source.folder_hint())?;
    let reason =
        run_session(source.as_mut(), &mut session, flag(args, "--frames").and_then(|f| f.parse().ok()), flag(args, "--seconds").and_then(|f| f.parse().ok()))?;
    let stats = session.refresh_stats().clone();
    let (dir, manifest) = session.finish()?;
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "session": dir, "reason": reason, "stats": stats, "manifest_stats": manifest.stats }))?);
    Ok(())
}

fn cli_send_test(args: &[String]) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        let name = flag(args, "--name").unwrap_or("Orbiters Test Pattern");
        let width = flag(args, "--width").and_then(|v| v.parse().ok()).unwrap_or(1920);
        let height = flag(args, "--height").and_then(|v| v.parse().ok()).unwrap_or(1080);
        let fps: f32 = flag(args, "--fps").and_then(|v| v.parse().ok()).unwrap_or(30.0);
        let depth_every = flag(args, "--depth-every").and_then(|v| v.parse().ok()).unwrap_or(4);
        let seconds: f64 = flag(args, "--seconds").and_then(|v| v.parse().ok()).unwrap_or(0.0);
        let mut sender = orbiters_spout::Sender::new(name, width, height)?;
        let pattern = TestPatternSource::new(width, height, fps, depth_every, 0, false);
        eprintln!("sending \"{name}\" {width}x{height} at {fps} fps (Ctrl+C to stop)");
        let start = Instant::now();
        let mut i = 0u32;
        loop {
            let frame = pattern.render(i);
            sender.send_rgba(&frame.data)?;
            i += 1;
            if seconds > 0.0 && start.elapsed().as_secs_f64() >= seconds {
                break;
            }
            let due = start + Duration::from_secs_f64(i as f64 / fps as f64);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
        eprintln!("sent {i} frames");
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        anyhow::bail!("Spout is only available on Windows")
    }
}

fn cli_verify(args: &[String]) -> anyhow::Result<()> {
    let dir = args.first().ok_or_else(|| anyhow::anyhow!("usage: verify <raw capture folder> [--frames N]"))?;
    let report = verify::verify(std::path::Path::new(dir), flag(args, "--frames").and_then(|f| f.parse().ok()))?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

// ---------------------------------------------------------------- worker protocol

struct Output(std::io::Stdout);

impl Output {
    fn emit(&mut self, e: &WorkerEvent) {
        let mut lock = self.0.lock();
        let _ = writeln!(lock, "{}", e.to_line());
        let _ = lock.flush();
    }
}

fn run_worker() -> anyhow::Result<()> {
    let mut out = Output(std::io::stdout());
    out.emit(&WorkerEvent::Ready { version: orbiters_core::APP_VERSION.into() });
    let (tx, rx) = crossbeam_channel::unbounded::<WorkerCommand>();
    let errors = crossbeam_channel::unbounded::<String>();
    {
        let err_tx = errors.0.clone();
        std::thread::Builder::new().name("stdin".into()).spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<WorkerCommand>(&line) {
                    Ok(c) => {
                        if tx.send(c).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = err_tx.send(format!("bad command: {e}"));
                    }
                }
            }
            let _ = tx.send(WorkerCommand::Quit);
        })?;
    }

    let mut current: Option<(CaptureSession, Box<dyn FrameSource>)> = None;
    let mut paused = false;
    let mut last_stats = Instant::now();
    let finish = |current: &mut Option<(CaptureSession, Box<dyn FrameSource>)>, out: &mut Output, reason: &str| {
        if let Some((session, _)) = current.take() {
            match session.finish() {
                Ok((dir, manifest)) => out.emit(&WorkerEvent::Stopped { session_dir: dir, manifest: Box::new(manifest), reason: reason.into() }),
                Err(e) => out.emit(&WorkerEvent::Error { message: format!("finishing the session: {e:#}") }),
            }
        }
    };

    loop {
        while let Ok(msg) = errors.1.try_recv() {
            out.emit(&WorkerEvent::Error { message: msg });
        }
        let command = if current.is_some() && !paused {
            rx.try_recv().ok()
        } else {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => Some(WorkerCommand::Quit),
            }
        };
        if let Some(command) = command {
            match command {
                WorkerCommand::ListSources => out.emit(&WorkerEvent::Sources { spout: list_sources(), mics: list_mics() }),
                WorkerCommand::Start(start) => {
                    finish(&mut current, &mut out, "restarted");
                    match open_source(&start.source).and_then(|src| {
                        let session = CaptureSession::start(&start, src.describe(), &src.folder_hint())?;
                        Ok((session, src))
                    }) {
                        Ok((session, src)) => {
                            out.emit(&WorkerEvent::Started { session_dir: session.dir(), source: src.describe() });
                            current = Some((session, src));
                            paused = false;
                        }
                        Err(e) => out.emit(&WorkerEvent::Error { message: format!("{e:#}") }),
                    }
                }
                WorkerCommand::Stop => finish(&mut current, &mut out, "stopped"),
                WorkerCommand::Pause => {
                    paused = true;
                    if let Some((session, _)) = current.as_mut() {
                        session.set_performance_paused(true);
                    }
                }
                WorkerCommand::Resume => {
                    if let Some((session, _)) = current.as_mut() {
                        if paused {
                            session.resume();
                        }
                        session.set_performance_paused(false);
                    }
                    paused = false;
                }
                WorkerCommand::Osc { samples } => {
                    if let Some((session, _)) = current.as_mut() {
                        session.osc(&samples, paused);
                    }
                }
                WorkerCommand::Calibration { calibration } => {
                    if let Some((session, _)) = current.as_mut() {
                        session.add_calibration(*calibration);
                    }
                }
                WorkerCommand::SetWorld { world } => {
                    if let Some((session, _)) = current.as_mut() {
                        session.set_world(world);
                    }
                }
                WorkerCommand::Quit => {
                    finish(&mut current, &mut out, "quit");
                    return Ok(());
                }
            }
        }

        let mut ended = false;
        if let Some((session, source)) = current.as_mut() {
            if !paused {
                let mut err = None;
                match source.next(Duration::from_millis(50), &mut |v| {
                    if let Err(e) = session.process(v) {
                        err = Some(e);
                    }
                }) {
                    Ok(Next::End) => ended = true,
                    Ok(_) => {}
                    Err(e) => {
                        session.stats.last_error = Some(format!("{e:#}"));
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                if let Some(e) = err {
                    out.emit(&WorkerEvent::Error { message: format!("{e:#}") });
                }
            }
            if last_stats.elapsed() >= Duration::from_millis(250) {
                let mut stats = session.refresh_stats().clone();
                stats.paused = paused;
                out.emit(&WorkerEvent::Stats(stats));
                last_stats = Instant::now();
            }
        }
        if ended {
            finish(&mut current, &mut out, "end of input");
        }
    }
}
