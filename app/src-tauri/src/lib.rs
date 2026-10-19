//! Orbiters desktop shell: owns the capture worker, watches VRChat (log, process, Spout senders), listens to
//! the Action Menu over OSC, and serves the web interface through commands and `snapshot` events.

mod state;
mod worker;

use orbiters_contracts::manifest::{ConsentInfo, WorldInfo};
use orbiters_core::bundle::{self, SessionSummary};
use orbiters_core::osc::{OscEvent, OscService};
use orbiters_core::protocol::{SourceSpec, StartCapture, WorkerCommand, WorkerEvent};
use orbiters_core::settings::Settings;
use orbiters_core::vrchat_log::{LogEvent, LogWatcher};
use serde::Serialize;
use state::{CaptureState, Snapshot};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, Wry};
use worker::WorkerHandle;

/// What the interface needs to know about a finished session.
#[derive(Clone, Serialize)]
struct SessionSaved {
    dir: PathBuf,
    reason: String,
    frames: u64,
    keyframes: u64,
    depth_frames: u64,
}

#[derive(Clone, Serialize)]
struct Toast {
    kind: String,
    message: String,
}

#[derive(Clone, Serialize)]
struct RawCaptureSummary {
    dir: PathBuf,
    name: String,
    scene: Option<String>,
    frames: u32,
    width: u32,
    height: u32,
    depth_every: u32,
    has_ground_truth: bool,
    created_utc: String,
}

pub struct Controller {
    app: AppHandle,
    snapshot: Mutex<Snapshot>,
    worker: Mutex<Option<WorkerHandle>>,
    osc: Mutex<Option<OscService>>,
    worker_log: Mutex<VecDeque<String>>,
    tray_toggle: Mutex<Option<MenuItem<Wry>>>,
    /// The source of the running capture, reused when a world change starts a new segment.
    last_source: Mutex<Option<SourceSpec>>,
    last_emit: Mutex<Instant>,
}

type Ctl = Arc<Controller>;

impl Controller {
    fn new(app: AppHandle, settings: Settings) -> Ctl {
        Arc::new(Self {
            app,
            snapshot: Mutex::new(Snapshot::new(settings)),
            worker: Mutex::new(None),
            osc: Mutex::new(None),
            worker_log: Mutex::new(VecDeque::new()),
            tray_toggle: Mutex::new(None),
            last_source: Mutex::new(None),
            last_emit: Mutex::new(Instant::now()),
        })
    }

    fn snapshot(&self) -> Snapshot {
        let mut s = self.snapshot.lock().unwrap().clone();
        // The interface only needs a light trail.
        if s.capture.trail.len() > 1500 {
            let step = s.capture.trail.len().div_ceil(1500);
            s.capture.trail = s.capture.trail.iter().step_by(step).copied().collect();
        }
        s
    }

    fn emit(&self) {
        let snap = self.snapshot();
        let capturing = snap.is_capturing();
        let _ = self.app.emit("snapshot", &snap);
        if let Some(item) = self.tray_toggle.lock().unwrap().as_ref() {
            let _ = item.set_text(if capturing { "Stop recording" } else { "Start recording" });
        }
        *self.last_emit.lock().unwrap() = Instant::now();
    }

    fn update<R>(&self, f: impl FnOnce(&mut Snapshot) -> R) -> R {
        let r = f(&mut self.snapshot.lock().unwrap());
        self.emit();
        r
    }

    fn toast(&self, kind: &str, message: impl Into<String>) {
        let _ = self.app.emit("toast", Toast { kind: kind.into(), message: message.into() });
    }

    fn settings(&self) -> Settings {
        self.snapshot.lock().unwrap().settings.clone()
    }

    fn ensure_worker(self: &Ctl) -> anyhow::Result<()> {
        let mut guard = self.worker.lock().unwrap();
        if let Some(w) = guard.as_mut() {
            if w.alive() {
                return Ok(());
            }
        }
        let on_event = {
            let ctl = self.clone();
            move |e| ctl.on_worker_event(e)
        };
        let on_log = {
            let ctl = self.clone();
            move |line: String| {
                let mut log = ctl.worker_log.lock().unwrap();
                log.push_back(line);
                while log.len() > 400 {
                    log.pop_front();
                }
            }
        };
        let on_exit = {
            let ctl = self.clone();
            move || {
                ctl.update(|s| {
                    s.worker.ready = false;
                    if s.is_capturing() {
                        s.capture.state = CaptureState::Idle;
                        s.capture.error = Some("The capture worker stopped unexpectedly.".into());
                    }
                });
            }
        };
        let handle = WorkerHandle::spawn(on_event, on_log, on_exit).map_err(|e| {
            self.update(|s| s.worker.error = Some(e.to_string()));
            e
        })?;
        *guard = Some(handle);
        Ok(())
    }

    fn send(self: &Ctl, cmd: &WorkerCommand) -> anyhow::Result<()> {
        self.ensure_worker()?;
        let mut guard = self.worker.lock().unwrap();
        guard.as_mut().expect("worker").send(cmd)
    }

    fn on_worker_event(self: &Ctl, e: WorkerEvent) {
        match e {
            WorkerEvent::Ready { version } => self.update(|s| {
                s.worker.ready = true;
                s.worker.error = (version != orbiters_core::APP_VERSION).then(|| format!("worker version {version} differs from the app"));
            }),
            WorkerEvent::Sources { spout } => self.update(|s| s.sources = spout),
            WorkerEvent::Started { session_dir, source } => self.update(|s| {
                if s.capture.state != CaptureState::Importing {
                    s.capture.state = CaptureState::Recording;
                }
                s.capture.session_dir = Some(session_dir);
                s.capture.source = Some(source);
                s.capture.error = None;
                s.capture.trail.clear();
            }),
            WorkerEvent::Stats(stats) => {
                let over_limit = {
                    let mut s = self.snapshot.lock().unwrap();
                    if let Some(p) = stats.position {
                        s.push_trail(p);
                    }
                    if s.capture.state == CaptureState::Recording && stats.paused {
                        s.capture.state = CaptureState::Paused;
                    } else if s.capture.state == CaptureState::Paused && !stats.paused {
                        s.capture.state = CaptureState::Recording;
                    }
                    let limit = s.settings.storage_limit_gb;
                    let over = limit > 0.0 && (s.storage.used_bytes + stats.bytes) as f64 > limit * 1e9 && s.capture.state == CaptureState::Recording;
                    s.capture.stats = Some(stats);
                    over
                };
                // Stats arrive four times a second; that is also the interface's refresh rate.
                self.emit();
                if over_limit {
                    self.toast("warning", "Storage limit reached: recording stopped. Raise the limit in Settings or free space.");
                    let _ = self.stop_capture();
                }
            }
            WorkerEvent::Stopped { session_dir, manifest, reason } => {
                let restarting = reason == "restarted";
                self.update(|s| {
                    if !restarting {
                        s.capture.state = CaptureState::Idle;
                        s.capture.from_osc = false;
                    }
                    s.storage.used_bytes += manifest.stats.bytes;
                    s.storage.sessions += 1;
                });
                let _ = self.app.emit(
                    "session-saved",
                    SessionSaved {
                        dir: session_dir,
                        reason,
                        frames: manifest.stats.frames,
                        keyframes: manifest.stats.keyframes,
                        depth_frames: manifest.stats.depth_frames,
                    },
                );
            }
            WorkerEvent::Error { message } => {
                self.update(|s| {
                    s.capture.error = Some(message.clone());
                    if s.capture.state == CaptureState::Starting {
                        s.capture.state = CaptureState::Idle;
                    }
                });
                self.toast("error", message);
            }
            WorkerEvent::Log { .. } => {}
        }
    }

    fn current_world(&self) -> Option<WorldInfo> {
        let s = self.snapshot.lock().unwrap();
        let w = &s.vrchat.world;
        (w.in_world && (w.world_id.is_some() || w.world_name.is_some())).then(|| WorldInfo {
            id: w.world_id.clone(),
            name: w.world_name.clone(),
            author_name: None,
            instance: w.instance.clone(),
        })
    }

    fn start_with(self: &Ctl, source: SourceSpec, importing: bool, from_osc: bool) -> anyhow::Result<()> {
        let settings = self.settings();
        if !importing && !settings.consent_acknowledged {
            anyhow::bail!("Please read and accept the capture notice first (Capture page).");
        }
        let cmd = StartCapture {
            source: source.clone(),
            sessions_dir: settings.sessions_dir.clone(),
            mode: settings.capture_mode,
            quality: settings.storage_quality,
            keyframes: settings.keyframes.clone(),
            world: if importing { None } else { self.current_world() },
            consent: ConsentInfo::default(),
            folder_name: None,
        };
        self.update(|s| {
            s.capture.state = if importing { CaptureState::Importing } else { CaptureState::Starting };
            s.capture.error = None;
            s.capture.stats = None;
            s.capture.trail.clear();
            s.capture.from_osc = from_osc;
        });
        *self.last_source.lock().unwrap() = (!importing).then_some(source);
        if let Err(e) = self.send(&WorkerCommand::Start(cmd)) {
            self.update(|s| {
                s.capture.state = CaptureState::Idle;
                s.capture.error = Some(e.to_string());
            });
            return Err(e);
        }
        Ok(())
    }

    fn start_capture(self: &Ctl, from_osc: bool) -> anyhow::Result<()> {
        if self.snapshot.lock().unwrap().is_capturing() {
            return Ok(());
        }
        let name = self.settings().spout_sender;
        self.start_with(SourceSpec::Spout { name }, false, from_osc)
    }

    fn stop_capture(self: &Ctl) -> anyhow::Result<()> {
        if !self.snapshot.lock().unwrap().is_capturing() {
            return Ok(());
        }
        self.update(|s| s.capture.state = CaptureState::Stopping);
        self.send(&WorkerCommand::Stop)
    }

    fn start_osc(self: &Ctl) {
        *self.osc.lock().unwrap() = None;
        let settings = self.settings();
        if !settings.osc.enabled {
            self.update(|s| s.osc = state::OscStatus { enabled: false, ..Default::default() });
            return;
        }
        let ctl = self.clone();
        let parameter = settings.osc.record_parameter.clone();
        let wanted = vec![parameter.clone()];
        let result = OscService::start("Orbiters desktop", &wanted, settings.osc.vrchat_port, true, move |e| {
            if let OscEvent::Parameter { name, value } = &e {
                if *name == parameter {
                    let on = value.truthy();
                    ctl.update(|s| s.osc.last_event = Some(format!("{name} = {}", if on { "on" } else { "off" })));
                    let r = if on { ctl.start_capture(true) } else { ctl.stop_capture() };
                    if let Err(err) = r {
                        ctl.toast("error", err.to_string());
                    }
                }
            }
        });
        match result {
            Ok(service) => {
                let (port, adv) = (service.osc_port, service.advertising());
                *self.osc.lock().unwrap() = Some(service);
                self.update(|s| s.osc = state::OscStatus { enabled: true, listening: true, advertising: adv, port: Some(port), last_event: None, error: None });
            }
            Err(e) => self.update(|s| s.osc = state::OscStatus { enabled: true, error: Some(e.to_string()), ..Default::default() }),
        }
    }

    fn shutdown(&self) {
        *self.osc.lock().unwrap() = None;
        // Dropping the handle asks the worker to quit, which finishes a running session.
        *self.worker.lock().unwrap() = None;
    }
}

fn refresh_storage(ctl: &Ctl) {
    let dir = ctl.settings().sessions_dir;
    let sessions = bundle::list_sessions(&dir);
    let used: u64 = sessions.iter().map(|s| s.size_bytes).sum();
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let free = disks
        .list()
        .iter()
        .filter(|d| dir.starts_with(d.mount_point()) || dir.to_string_lossy().to_lowercase().starts_with(&d.mount_point().to_string_lossy().to_lowercase()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space());
    ctl.update(|s| {
        s.storage.sessions_dir = dir.clone();
        s.storage.used_bytes = used;
        s.storage.free_bytes = free;
        s.storage.limit_gb = s.settings.storage_limit_gb;
        s.storage.sessions = sessions.len();
    });
}

fn spawn_watchers(ctl: Ctl) {
    std::thread::Builder::new()
        .name("watchers".into())
        .spawn(move || {
            let mut log = LogWatcher::new(None);
            let mut system = sysinfo::System::new();
            let mut tick = 0u64;
            loop {
                // VRChat log: world changes (and new segments when recording).
                let events = log.poll();
                let world = log.state.clone();
                let joined = events.iter().any(|e| matches!(e, LogEvent::Joining { .. }));
                let changed = {
                    let mut s = ctl.snapshot.lock().unwrap();
                    let changed = s.vrchat.world != world;
                    s.vrchat.world = world;
                    s.vrchat.log_found = log.state.log_file.is_some() || orbiters_core::paths::vrchat_log_dir().is_some();
                    changed
                };
                if changed {
                    ctl.emit();
                }
                if joined {
                    let (recording, split) = {
                        let s = ctl.snapshot.lock().unwrap();
                        (s.capture.state == CaptureState::Recording || s.capture.state == CaptureState::Paused, s.settings.split_on_world_change)
                    };
                    if recording {
                        if split {
                            let source = ctl.last_source.lock().unwrap().clone();
                            if let Some(source) = source {
                                let from_osc = ctl.snapshot.lock().unwrap().capture.from_osc;
                                let _ = ctl.start_with(source, false, from_osc);
                                ctl.toast("info", "New world: started a new session.");
                            }
                        } else {
                            let _ = ctl.send(&WorkerCommand::SetWorld { world: ctl.current_world() });
                        }
                    }
                }

                // VRChat process, every 3 s.
                if tick % 3 == 0 {
                    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
                    let running = system.processes().values().any(|p| {
                        let n = p.name().to_string_lossy().to_lowercase();
                        n == "vrchat.exe" || n == "vrchat"
                    });
                    let changed = ctl.update_if(|s| {
                        let c = s.vrchat.running != running;
                        s.vrchat.running = running;
                        c
                    });
                    let _ = changed;
                }

                // Spout senders, every 2 s.
                #[cfg(windows)]
                if tick % 2 == 0 {
                    let sources: Vec<_> = orbiters_spout::list_senders()
                        .into_iter()
                        .filter_map(|n| orbiters_spout::sender_info(&n))
                        .map(|i| orbiters_core::protocol::SpoutSourceInfo { name: i.name, width: i.width, height: i.height, format: i.format, description: i.description })
                        .collect();
                    ctl.update_if(|s| {
                        let c = s.sources != sources;
                        s.sources = sources;
                        c
                    });
                }

                // Storage, every 30 s (it walks the session folders).
                if tick % 30 == 0 {
                    refresh_storage(&ctl);
                }
                tick += 1;
                std::thread::sleep(Duration::from_secs(1));
            }
        })
        .expect("watchers thread");
}

impl Controller {
    /// Applies a change and emits only when `f` reports one.
    fn update_if(&self, f: impl FnOnce(&mut Snapshot) -> bool) -> bool {
        let changed = f(&mut self.snapshot.lock().unwrap());
        if changed {
            self.emit();
        }
        changed
    }
}

// ---------------------------------------------------------------- commands

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
fn get_snapshot(ctl: State<'_, Ctl>) -> Snapshot {
    ctl.snapshot()
}

#[tauri::command]
fn start_capture(ctl: State<'_, Ctl>) -> Result<(), String> {
    ctl.start_capture(false).map_err(err)
}

#[tauri::command]
fn stop_capture(ctl: State<'_, Ctl>) -> Result<(), String> {
    ctl.stop_capture().map_err(err)
}

#[tauri::command]
fn pause_capture(ctl: State<'_, Ctl>, paused: bool) -> Result<(), String> {
    ctl.send(if paused { &WorkerCommand::Pause } else { &WorkerCommand::Resume }).map_err(err)?;
    ctl.update(|s| {
        if s.capture.state == CaptureState::Recording || s.capture.state == CaptureState::Paused {
            s.capture.state = if paused { CaptureState::Paused } else { CaptureState::Recording };
        }
    });
    Ok(())
}

#[tauri::command]
fn start_test_capture(ctl: State<'_, Ctl>) -> Result<(), String> {
    ctl.start_with(SourceSpec::TestPattern { width: 1920, height: 1080, fps: 30.0, depth_every: 4, frames: 0 }, false, false).map_err(err)
}

#[tauri::command]
async fn list_sessions(ctl: State<'_, Ctl>) -> Result<Vec<SessionSummary>, String> {
    Ok(bundle::list_sessions(&ctl.settings().sessions_dir))
}

#[tauri::command]
async fn session_thumbnail(dir: PathBuf) -> Result<Option<String>, String> {
    Ok(bundle::thumbnail_bmp_base64(&dir))
}

/// Camera positions of a session (Unity world, x y z), thinned to at most 2000 points.
#[tauri::command]
async fn session_trail(dir: PathBuf) -> Result<Vec<[f32; 3]>, String> {
    let records = bundle::read_records(&dir).map_err(err)?;
    let points: Vec<[f32; 3]> = records
        .iter()
        .filter(|r| r.has(orbiters_contracts::frame_record::record_flags::STRIP_OK) && !r.has(orbiters_contracts::frame_record::record_flags::GATED))
        .map(|r| [r.camera_to_world[3], r.camera_to_world[7], r.camera_to_world[11]])
        .collect();
    let step = points.len().div_ceil(2000).max(1);
    Ok(points.into_iter().step_by(step).collect())
}

fn inside(root: &Path, dir: &Path) -> bool {
    match (root.canonicalize(), dir.canonicalize()) {
        (Ok(r), Ok(d)) => d.starts_with(&r) && d != r,
        _ => false,
    }
}

#[tauri::command]
async fn trash_session(ctl: State<'_, Ctl>, dir: PathBuf) -> Result<(), String> {
    let root = ctl.settings().sessions_dir;
    if !inside(&root, &dir) || !dir.join("manifest.json").is_file() {
        return Err("Only session folders inside the sessions folder can be removed.".into());
    }
    trash::delete(&dir).map_err(err)?;
    refresh_storage(&ctl);
    Ok(())
}

#[tauri::command]
fn open_path(dir: PathBuf) -> Result<(), String> {
    #[cfg(windows)]
    let mut cmd = std::process::Command::new("explorer");
    #[cfg(target_os = "linux")]
    let mut cmd = std::process::Command::new("xdg-open");
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    cmd.arg(&dir).spawn().map(|_| ()).map_err(err)
}

#[tauri::command]
async fn list_raw_captures(ctl: State<'_, Ctl>) -> Result<Vec<RawCaptureSummary>, String> {
    let root = ctl.settings().raw_captures_dir;
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for e in entries.flatten() {
            let dir = e.path();
            let Ok(text) = std::fs::read_to_string(dir.join(orbiters_contracts::raw_capture::RAW_FILE)) else { continue };
            let Ok(meta) = serde_json::from_str::<orbiters_contracts::raw_capture::RawCapture>(&text) else { continue };
            out.push(RawCaptureSummary {
                name: dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                dir,
                scene: meta.scene,
                frames: meta.frame_count,
                width: meta.width,
                height: meta.height,
                depth_every: meta.depth_every,
                has_ground_truth: meta.has_ground_truth,
                created_utc: meta.created_utc,
            });
        }
    }
    out.sort_by(|a, b| b.created_utc.cmp(&a.created_utc));
    Ok(out)
}

#[tauri::command]
fn import_raw_capture(ctl: State<'_, Ctl>, dir: PathBuf) -> Result<(), String> {
    if ctl.snapshot.lock().unwrap().is_capturing() {
        return Err("Finish the current capture first.".into());
    }
    ctl.start_with(SourceSpec::RawSequence { path: dir, realtime: false }, true, false).map_err(err)
}

#[tauri::command]
fn save_settings(ctl: State<'_, Ctl>, settings: Settings) -> Result<Snapshot, String> {
    let previous = ctl.settings();
    settings.save().map_err(err)?;
    ctl.update(|s| {
        s.settings = settings.clone();
        s.storage.limit_gb = settings.storage_limit_gb;
    });
    if previous.osc != settings.osc {
        ctl.start_osc();
    }
    if previous.sessions_dir != settings.sessions_dir {
        let c = ctl.inner().clone();
        std::thread::spawn(move || refresh_storage(&c));
    }
    Ok(ctl.snapshot())
}

#[tauri::command]
fn worker_log(ctl: State<'_, Ctl>) -> Vec<String> {
    ctl.worker_log.lock().unwrap().iter().cloned().collect()
}

#[tauri::command]
fn quit_app(app: AppHandle, ctl: State<'_, Ctl>) {
    ctl.shutdown();
    app.exit(0);
}

/// Shows the window, recreating it when it was closed to the tray. Closing destroys the web view, so the app
/// costs no GPU time and only the shell's memory while it waits in the tray (the web view alone kept the GPU's
/// 3D engine at 14-17 % and held about 450 MB).
fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }
    let Some(config) = app.config().app.windows.first().cloned() else { return };
    match tauri::WebviewWindowBuilder::from_config(app, &config).and_then(|b| b.build()) {
        Ok(w) => {
            let _ = w.set_focus();
        }
        Err(e) => eprintln!("could not open the window: {e}"),
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let settings = Settings::load();
            let _ = std::fs::create_dir_all(&settings.sessions_dir);
            let _ = std::fs::create_dir_all(&settings.raw_captures_dir);
            let ctl = Controller::new(app.handle().clone(), settings);
            app.manage(ctl.clone());

            // Tray: show, start/stop, quit. Closing the window destroys it; the app keeps running in the tray.
            let show = MenuItem::with_id(app, "show", "Show Orbiters desktop", true, None::<&str>)?;
            let toggle = MenuItem::with_id(app, "toggle", "Start recording", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Orbiters desktop", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &toggle, &PredefinedMenuItem::separator(app)?, &quit])?;
            *ctl.tray_toggle.lock().unwrap() = Some(toggle);
            let tray_ctl = ctl.clone();
            let mut tray = TrayIconBuilder::with_id("main").tooltip("Orbiters desktop").menu(&menu).show_menu_on_left_click(false);
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.on_menu_event(move |app, event| match event.id.as_ref() {
                "show" => show_main(app),
                "toggle" => {
                    let capturing = tray_ctl.snapshot.lock().unwrap().is_capturing();
                    let r = if capturing { tray_ctl.stop_capture() } else { tray_ctl.start_capture(false) };
                    if let Err(e) = r {
                        tray_ctl.toast("error", e.to_string());
                        show_main(app);
                    }
                }
                "quit" => {
                    tray_ctl.shutdown();
                    app.exit(0);
                }
                _ => {}
            })
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                    show_main(tray.app_handle());
                }
            })
            .build(app)?;

            ctl.start_osc();
            let c = ctl.clone();
            std::thread::spawn(move || {
                if let Err(e) = c.ensure_worker() {
                    eprintln!("capture worker: {e}");
                }
            });
            spawn_watchers(ctl);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            start_capture,
            stop_capture,
            pause_capture,
            start_test_capture,
            list_sessions,
            session_thumbnail,
            session_trail,
            trash_session,
            open_path,
            list_raw_captures,
            import_raw_capture,
            save_settings,
            worker_log,
            quit_app
        ])
        .build(tauri::generate_context!())
        .expect("error while building Orbiters desktop")
        .run(|_app, event| {
            // The last window closing is not a reason to quit: capture keeps running from the tray.
            if let tauri::RunEvent::ExitRequested { api, code: None, .. } = event {
                api.prevent_exit();
            }
        });
}
