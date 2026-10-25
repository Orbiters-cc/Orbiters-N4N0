//! Orbiters desktop capture worker.
//!
//! * `orbiters-capture worker` — driven by the app: JSON-lines commands on stdin, events on stdout.
//! * `orbiters-capture record --source <spout[:name] | raw:<folder> | test> [--out <folder>] [--quality …] [--frames N] [--rig]`
//! * `orbiters-capture send-test [--name <sender>] [--width W] [--height H] [--fps F] [--depth-every N] [--seconds S] [--rig]`
//!
//! `--rig`: the test pattern's odd colour frames are capture-rig frames (contracts' Docs/rig-frames.md).
//! * `orbiters-capture verify <raw capture folder> [--frames N]`
//! * `orbiters-capture list-sources`

mod audio;
mod coverage;
mod performance;
mod session;
mod sources;
mod tracking;
mod verify;

use crossbeam_channel::RecvTimeoutError;
use orbiters_contracts::manifest::{CaptureMode, ConsentInfo, StorageQuality};
use orbiters_core::protocol::{AudioDeviceInfo, PerformanceSpec, SourceSpec, SpoutSourceInfo, StartCapture, WorkerCommand, WorkerEvent};
use orbiters_core::settings::KeyframeSettings;
use session::{CaptureSession, Processed, Warning};
use sources::{FrameSource, Next, RawSequenceSource, TestPatternSource};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() {
    orbiters_core::panic_log::install();
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

/// A test pattern's size and rate, when they make sense (a size of 0 would panic, 100k × 100k would not fit in memory).
fn check_pattern(width: u32, height: u32, fps: f32) -> anyhow::Result<()> {
    anyhow::ensure!((64..=8192).contains(&width) && (64..=8192).contains(&height), "a test pattern of {width}×{height}: from 64 to 8192 pixels a side");
    anyhow::ensure!(fps.is_finite() && fps > 0.0 && fps <= 1000.0, "a test pattern at {fps} fps: more than 0 and at most 1000");
    Ok(())
}

fn open_source(spec: &SourceSpec) -> anyhow::Result<Box<dyn FrameSource>> {
    Ok(match spec {
        #[cfg(windows)]
        SourceSpec::Spout { name } => Box::new(sources::SpoutSource::new(name)?),
        #[cfg(not(windows))]
        SourceSpec::Spout { .. } => anyhow::bail!("Spout is only available on Windows; use the PipeWire source on Linux"),
        SourceSpec::RawSequence { path, realtime } => Box::new(RawSequenceSource::open(path, *realtime)?),
        SourceSpec::TestPattern { width, height, fps, depth_every, frames } => {
            check_pattern(*width, *height, *fps)?;
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

/// Drives a session until the source ends, `max_frames` frames or `max_seconds` pass (or the frame size changes,
/// or the disk fills up).
fn run_session(source: &mut dyn FrameSource, session: &mut CaptureSession, max_frames: Option<u64>, max_seconds: Option<f64>) -> anyhow::Result<String> {
    let started = Instant::now();
    let mut last_print = Instant::now();
    loop {
        let mut outcome = None;
        let next = source.next(Duration::from_millis(100), &mut |frame| outcome = Some(session.process(frame)))?;
        match outcome {
            // A frame that could not be read (a Spout readback that failed): the source opens its sender again.
            Some(Err(e)) => eprintln!("warning: a frame could not be read: {e:#}"),
            Some(Ok(Processed::SizeChanged { width, height })) => return Ok(format!("the frame size changed to {width}x{height}")),
            _ => {}
        }
        if let Next::End = next {
            return Ok("end of input".into());
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
            if let Some(w) = source.warning() {
                eprintln!("warning: {w}");
            }
            for (_, w) in session.take_warnings() {
                eprintln!("warning: {w}");
            }
            if let Some(why) = session.must_stop() {
                return Ok(why.to_string());
            }
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
            let pattern = TestPatternSource::new(*width, *height, *fps, *depth_every, *frames, false);
            Box::new(if args.iter().any(|a| a == "--rig") { pattern.with_rig(None) } else { pattern }) as Box<dyn FrameSource>
        }
        other => open_source(other)?,
    };
    let mut session = CaptureSession::start(&cmd, source.describe(), &source.folder_hint())?;
    let outcome =
        run_session(source.as_mut(), &mut session, flag(args, "--frames").and_then(|f| f.parse().ok()), flag(args, "--seconds").and_then(|f| f.parse().ok()));
    // The session is finished whatever ended it: what was recorded is kept.
    let reason = match &outcome {
        Ok(reason) => reason.clone(),
        Err(e) => format!("error: {e:#}"),
    };
    let stats = session.refresh_stats().clone();
    let finished = session.finish();
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({ "session": finished.dir, "reason": reason, "stats": stats, "manifest_stats": finished.manifest.stats, "warnings": finished.warnings })
        )?
    );
    outcome.map(|_| ())
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
        check_pattern(width, height, fps)?;
        let mut sender = orbiters_spout::Sender::new(name, width, height)?;
        let pattern = TestPatternSource::new(width, height, fps, depth_every, 0, false);
        let pattern = if args.iter().any(|a| a == "--rig") { pattern.with_rig(None) } else { pattern };
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

/// One running session: what it records, what it reads from, the command that started it (a session that has to
/// start over, at a new frame size, starts the same way), the warnings told about it, and how its manifest names its
/// source.
struct Running {
    session: CaptureSession,
    source: Box<dyn FrameSource>,
    start: Box<StartCapture>,
    told: Told,
    described: String,
}

/// Warnings told to the service as `Error` events: each kind once, and again when its message changes, at most every
/// half minute (a disk that fails every frame was 85 toasts a second).
#[derive(Default)]
struct Told(HashMap<Warning, (String, Instant)>);

impl Told {
    const AGAIN_AFTER: Duration = Duration::from_secs(30);

    fn should_tell(&mut self, kind: Warning, message: &str) -> bool {
        match self.0.get(&kind) {
            Some((told, _)) if told == message => false,
            Some((_, at)) if at.elapsed() < Self::AGAIN_AFTER => false,
            _ => {
                self.0.insert(kind, (message.to_string(), Instant::now()));
                true
            }
        }
    }
}

/// What changes during a session changes the command a session starting over (at a new frame size) starts with too:
/// the world now, the VRChat that runs now, the calibration made last.
fn keep_current(start: &mut StartCapture, command: &WorkerCommand) {
    match command {
        WorkerCommand::SetWorld { world } => start.world = world.clone(),
        WorkerCommand::GameAudio { pid } => start.performance.game_audio_pid = Some(*pid),
        WorkerCommand::Calibration { calibration } if start.performance.tracking_hz.is_some() => {
            start.performance.calibration = Some((**calibration).clone());
        }
        _ => {}
    }
}

/// Tells the service what the source and the session have to say.
fn tell(out: &mut Output, running: &mut Running) {
    if let Some(w) = running.source.warning() {
        running.session.warn(Warning::Source, w);
    }
    for (kind, message) in running.session.take_warnings() {
        if running.told.should_tell(kind, &message) {
            out.emit(&WorkerEvent::Error { message });
        }
    }
}

/// Starts a session (on `source` when it is open already): `Started`, or `StartFailed`.
fn start_session(start: Box<StartCapture>, source: Option<Box<dyn FrameSource>>, out: &mut Output) -> Option<Running> {
    let opened = match source {
        Some(source) => Ok(source),
        None => open_source(&start.source),
    };
    match opened.and_then(|source| Ok((CaptureSession::start(&start, source.describe(), &source.folder_hint())?, source))) {
        Ok((session, source)) => {
            let described = source.describe();
            out.emit(&WorkerEvent::Started { session_dir: session.dir(), source: described.clone() });
            let mut running = Running { session, source, start, told: Told::default(), described };
            // After `Started`: an error before it would end the start in the service.
            tell(out, &mut running);
            Some(running)
        }
        Err(e) => {
            out.emit(&WorkerEvent::StartFailed { message: format!("{e:#}") });
            None
        }
    }
}

/// Ends the session: `Stopped` always comes, after what could not be written (the service waits for it). The source
/// comes back, for a session that starts over on it.
fn finish(current: &mut Option<Running>, out: &mut Output, reason: &str) -> Option<(Box<dyn FrameSource>, Box<StartCapture>)> {
    let Running { session, source, start, .. } = current.take()?;
    let finished = session.finish();
    if !finished.warnings.is_empty() {
        out.emit(&WorkerEvent::Error { message: format!("Finishing the session: {}", finished.warnings.join("; ")) });
    }
    out.emit(&WorkerEvent::Stopped { session_dir: finished.dir, manifest: Box::new(finished.manifest), reason: reason.into() });
    Some((source, start))
}

fn run_worker() -> anyhow::Result<()> {
    let mut out = Output(std::io::stdout());
    out.emit(&WorkerEvent::Ready { version: orbiters_core::APP_VERSION.into() });
    let (tx, rx) = crossbeam_channel::unbounded::<WorkerCommand>();
    let errors = crossbeam_channel::unbounded::<String>();
    {
        let err_tx = errors.0.clone();
        std::thread::Builder::new().name("stdin".into()).spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut line = Vec::new();
            loop {
                line.clear();
                match stdin.read_until(b'\n', &mut line) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
                // A byte that is not UTF-8 spoils its line, not the worker.
                let text = String::from_utf8_lossy(&line);
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                match serde_json::from_str::<WorkerCommand>(text) {
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

    let mut current: Option<Running> = None;
    let mut paused = false;
    let mut last_stats = Instant::now();
    let mut last_coverage = Instant::now();
    let listing = Arc::new(AtomicBool::new(false));

    loop {
        while let Ok(msg) = errors.1.try_recv() {
            out.emit(&WorkerEvent::Error { message: msg });
        }
        // Every command that came, not one a turn: OSC batches arrive many times a second.
        let mut commands: Vec<WorkerCommand> = Vec::new();
        if current.is_some() && !paused {
            commands.extend(rx.try_iter());
        } else {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(c) => {
                    commands.push(c);
                    commands.extend(rx.try_iter());
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => commands.push(WorkerCommand::Quit),
            }
        }
        for command in commands {
            if let Some(r) = current.as_mut() {
                keep_current(&mut r.start, &command);
            }
            match command {
                WorkerCommand::ListSources => {
                    // Listing microphones can take a while (Bluetooth devices): not on the thread that reads frames,
                    // and one listing at a time.
                    if !listing.swap(true, Ordering::SeqCst) {
                        let flag = listing.clone();
                        let spawned = std::thread::Builder::new().name("list-sources".into()).spawn(move || {
                            Output(std::io::stdout()).emit(&WorkerEvent::Sources { spout: list_sources(), mics: list_mics() });
                            flag.store(false, Ordering::SeqCst);
                        });
                        if spawned.is_err() {
                            listing.store(false, Ordering::SeqCst);
                        }
                    }
                }
                WorkerCommand::Start(start) => {
                    finish(&mut current, &mut out, "restarted");
                    current = start_session(start, None, &mut out);
                    paused = false;
                }
                WorkerCommand::Stop => {
                    finish(&mut current, &mut out, "stopped");
                }
                WorkerCommand::Pause => {
                    paused = true;
                    if let Some(r) = current.as_mut() {
                        r.session.set_performance_paused(true);
                    }
                }
                WorkerCommand::Resume => {
                    if let Some(r) = current.as_mut() {
                        if paused {
                            r.session.resume();
                        }
                        r.session.set_performance_paused(false);
                    }
                    paused = false;
                }
                WorkerCommand::Osc { samples } => {
                    if let Some(r) = current.as_mut() {
                        r.session.osc(&samples, paused);
                    }
                }
                WorkerCommand::Calibration { calibration } => {
                    if let Some(r) = current.as_mut() {
                        r.session.add_calibration(*calibration);
                    }
                }
                WorkerCommand::SetWorld { world } => {
                    if let Some(r) = current.as_mut() {
                        r.session.set_world(world);
                    }
                }
                WorkerCommand::GameAudio { pid } => {
                    if let Some(r) = current.as_mut() {
                        r.session.game_audio(pid);
                    }
                }
                WorkerCommand::Quit => {
                    finish(&mut current, &mut out, "quit");
                    return Ok(());
                }
            }
        }

        let (mut ended, mut resized, mut stop) = (false, None, None);
        if let Some(r) = current.as_mut() {
            if !paused {
                let mut outcome = None;
                let session = &mut r.session;
                match r.source.next(Duration::from_millis(50), &mut |frame| outcome = Some(session.process(frame))) {
                    Ok(Next::End) => ended = true,
                    Ok(_) => {}
                    Err(e) => {
                        r.session.warn(Warning::Source, format!("{e:#}"));
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                match outcome {
                    Some(Ok(Processed::SizeChanged { width, height })) => resized = Some((width, height)),
                    Some(Err(e)) => r.session.warn(Warning::Source, format!("A frame could not be read: {e:#}")),
                    _ => {}
                }
            }
            if last_stats.elapsed() >= Duration::from_millis(250) {
                let described = r.source.describe();
                if described != r.described {
                    r.session.set_source_name(described.clone());
                    r.described = described;
                }
                let mut stats = r.session.refresh_stats().clone();
                stats.paused = paused;
                out.emit(&WorkerEvent::Stats(stats));
                tell(&mut out, r);
                stop = r.session.must_stop().map(String::from);
                last_stats = Instant::now();
            }
            if last_coverage.elapsed() >= Duration::from_secs(1) {
                if let Some(map) = r.session.coverage_map() {
                    out.emit(&WorkerEvent::Coverage(map));
                }
                last_coverage = Instant::now();
            }
        }
        if let Some(why) = stop {
            eprintln!("{why}");
            out.emit(&WorkerEvent::Error { message: why });
            finish(&mut current, &mut out, "disk");
        } else if let Some((width, height)) = resized {
            // A bundle holds frames of one size: a new resolution (the Stream camera's changed) makes a new session,
            // started as this one was, on the same source. "resized" tells the service the recording goes on (a 3D
            // picture's or a scan's part is reconstructed on its own).
            eprintln!("the frame size changed to {width}x{height}: a new session");
            if let Some((source, start)) = finish(&mut current, &mut out, "resized") {
                current = start_session(start, Some(source), &mut out);
            }
        } else if ended {
            finish(&mut current, &mut out, "end of input");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_contracts::manifest::WorldInfo;
    use orbiters_contracts::performance::calibration::Calibration;

    /// A session starting over at a new frame size starts as things are now, not as they were when the first one did.
    #[test]
    fn a_session_starting_over_starts_as_things_are_now() {
        let mut start = StartCapture {
            source: SourceSpec::Spout { name: String::new() },
            sessions_dir: PathBuf::from("unused"),
            mode: CaptureMode::RecordOnly,
            quality: StorageQuality::High,
            keyframes: KeyframeSettings::default(),
            world: None,
            consent: ConsentInfo::default(),
            folder_name: None,
            performance: PerformanceSpec { tracking_hz: Some(90.0), ..Default::default() },
        };
        let lobby = WorldInfo { name: Some("Lobby".into()), ..Default::default() };
        keep_current(&mut start, &WorkerCommand::SetWorld { world: Some(lobby.clone()) });
        keep_current(&mut start, &WorkerCommand::GameAudio { pid: 4242 });
        let t_pose = Calibration { pose: "t_pose".into(), time_ns: 7, ..Default::default() };
        keep_current(&mut start, &WorkerCommand::Calibration { calibration: Box::new(t_pose.clone()) });
        keep_current(&mut start, &WorkerCommand::Pause);
        assert_eq!(start.world, Some(lobby));
        assert_eq!(start.performance.game_audio_pid, Some(4242));
        assert_eq!(start.performance.calibration, Some(t_pose.clone()));
        // Without tracking, a calibration is no part of the next session's start.
        start.performance = PerformanceSpec::default();
        keep_current(&mut start, &WorkerCommand::Calibration { calibration: Box::new(t_pose) });
        assert_eq!(start.performance.calibration, None);
    }

    #[test]
    fn warnings_are_told_once_and_again_when_they_change() {
        let mut told = Told::default();
        assert!(told.should_tell(Warning::Disk, "full"));
        assert!(!told.should_tell(Warning::Disk, "full"), "the same again");
        assert!(told.should_tell(Warning::Microphone, "unplugged"), "another kind");
        assert!(!told.should_tell(Warning::Disk, "fuller"), "changed, but just told");
        told.0.get_mut(&Warning::Disk).unwrap().1 = Instant::now().checked_sub(Told::AGAIN_AFTER).unwrap();
        assert!(told.should_tell(Warning::Disk, "fuller"), "changed, half a minute on");
    }
}
