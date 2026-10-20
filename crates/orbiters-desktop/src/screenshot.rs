//! `orbiters-desktop --screenshot <out folder> [--sessions <folder>] [--raw <folder>] [--scale <factor>] [--only <name>]`
//! renders the interface offscreen in several states and writes one PNG per state, for visual review without a
//! window. The controller is not started: the snapshots are made up, the session library is real. `--only e2e`
//! instead records a real test pattern through the worker, driven by synthetic clicks, with avatar parameters
//! from a stand-in VRChat (OSC on the loopback interface, current values from its OSCQuery tree).

use crate::app::App;
use crate::backend::{Ctl, Local};
use crate::controller::{self, Controller};
use crate::pages::Page;
use crate::state::{CaptureState, Snapshot};
use anyhow::Context as _;
use egui::{pos2, vec2, Event, Pos2, RawInput, Rect, ViewportId, ViewportInfo};
use orbiters_core::protocol::{CaptureStats, SpoutSourceInfo};
use orbiters_core::settings::Settings;
use orbiters_core::vrchat_log::WorldState;
use orbiters_ui::offscreen::{save_png, Offscreen};
use orbiters_ui::toast;
use std::path::PathBuf;
use std::time::Duration;

struct Scenario {
    name: &'static str,
    page: Page,
    snapshot: Snapshot,
    /// Frames rendered at 60 fps before the capture.
    frames: u32,
    splash: bool,
    open_detail: bool,
    toasts: Vec<(toast::Kind, &'static str)>,
    pointer: Option<Pos2>,
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let out =
        PathBuf::from(args.first().context("usage: --screenshot <out folder> [--sessions <folder>] [--raw <folder>] [--scale <factor>] [--only <name>]")?);
    let mut settings = Settings::default();
    let mut scale = 1.0f32;
    let mut only: Option<String> = None;
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sessions" => settings.sessions_dir = PathBuf::from(it.next().context("--sessions <folder>")?),
            "--raw" => settings.raw_captures_dir = PathBuf::from(it.next().context("--raw <folder>")?),
            "--scale" => scale = it.next().context("--scale <factor>")?.parse()?,
            "--only" => only = Some(it.next().context("--only <name>")?.clone()),
            other => anyhow::bail!("unknown option {other}"),
        }
    }
    settings.consent_acknowledged = true;
    std::fs::create_dir_all(&out)?;
    if only.as_deref() == Some("e2e") {
        return e2e(&mut Offscreen::new()?, &out, &settings, scale);
    }

    let sessions = controller::list_sessions(&settings.sessions_dir);
    let trail = sessions.first().and_then(|s| controller::session_trail(&s.dir).ok()).unwrap_or_else(spiral);

    let idle = idle_snapshot(&settings, &sessions);
    let mut recording = idle.clone();
    recording.capture.state = CaptureState::Recording;
    recording.capture.trail = trail.clone();
    recording.capture.source = Some("Spout: VRChat 1920×1080".into());
    recording.capture.session_dir = Some(settings.sessions_dir.join("Canine Test Lab 2026-10-20 02-14"));
    recording.capture.stats = Some(CaptureStats {
        frames: 4821,
        strip_ok: 4815,
        strip_failed: 6,
        keyframes: 312,
        depth_frames: 1205,
        fps: 59.8,
        decode_ms: 1.9,
        bytes: 1_840_000_000,
        elapsed_s: 82.0,
        position: trail.last().copied(),
        heading_deg: Some(135.0),
        width: 1920,
        height: 1080,
        ..Default::default()
    });
    let mut consent = idle.clone();
    consent.settings.consent_acknowledged = false;

    let scenarios = vec![
        Scenario { name: "splash", page: Page::Capture, snapshot: idle.clone(), frames: 66, splash: true, open_detail: false, toasts: vec![], pointer: None },
        Scenario {
            name: "capture-idle",
            page: Page::Capture,
            snapshot: idle.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
        },
        Scenario {
            name: "capture-recording",
            page: Page::Capture,
            snapshot: recording,
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
        },
        Scenario {
            name: "capture-consent",
            page: Page::Capture,
            snapshot: consent,
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
        },
        Scenario {
            name: "toasts",
            page: Page::Capture,
            snapshot: idle.clone(),
            frames: 60,
            splash: false,
            open_detail: false,
            toasts: vec![
                (toast::Kind::Success, "Session saved: 4,821 frames, 312 keyframes, 1,205 depth frames."),
                (toast::Kind::Info, "New world: started a new session."),
            ],
            pointer: None,
        },
        Scenario {
            name: "sessions",
            page: Page::Sessions,
            snapshot: idle.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: Some(pos2(400.0, 300.0)),
        },
        Scenario {
            name: "session-detail",
            page: Page::Sessions,
            snapshot: idle.clone(),
            frames: 150,
            splash: false,
            open_detail: true,
            toasts: vec![],
            pointer: None,
        },
        Scenario { name: "settings", page: Page::Settings, snapshot: idle, frames: 90, splash: false, open_detail: false, toasts: vec![], pointer: None },
    ];

    let mut gpu = Offscreen::new()?;
    for sc in scenarios {
        if only.as_deref().is_some_and(|o| o != sc.name) {
            continue;
        }
        let path = out.join(format!("{}.png", sc.name));
        render(&mut gpu, &sc, scale, &path)?;
        println!("{}", path.display());
    }
    Ok(())
}

fn render(gpu: &mut Offscreen, sc: &Scenario, scale: f32, path: &std::path::Path) -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    orbiters_ui::install(&ctx);
    let ctl = Controller::new(sc.snapshot.settings.clone());
    ctl.set_snapshot(sc.snapshot.clone());
    let backend: Ctl = std::sync::Arc::new(Local::new(ctl));
    backend.attach(&ctx);
    let mut app = App::offline(backend);
    if !sc.splash {
        app.skip_splash();
    }
    let size = vec2(1280.0, 820.0);
    for f in 0..sc.frames {
        let t = f as f64 / 60.0;
        let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), time: Some(t), ..Default::default() };
        input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(scale), focused: Some(true), ..Default::default() });
        if let Some(p) = sc.pointer {
            input.events.push(Event::PointerMoved(p));
        }
        if f == 0 {
            app.set_page(sc.page, t);
        }
        if f == 1 {
            for (kind, message) in &sc.toasts {
                app.toasts_mut().push(&ctx, *kind, *message);
            }
        }
        if sc.open_detail && f == 45 && app.sessions_mut().loaded() {
            app.sessions_mut().open_first(&ctx, Rect::from_min_size(pos2(264.0, 160.0), vec2(300.0, 265.0)));
        }
        let capture = f + 1 == sc.frames;
        let image = gpu.frame(
            &ctx,
            input,
            |ui| {
                app.logic_offline(ui.ctx());
                app.draw(ui);
            },
            capture,
        )?;
        if let Some(image) = image {
            save_png(path, &image)?;
        }
        // Background jobs (session list, thumbnails) land between frames.
        std::thread::sleep(Duration::from_millis(3));
    }
    Ok(())
}

/// `--only e2e`: a real test-pattern recording through the capture worker, driven by synthetic clicks in real time
/// (Test pattern, then the record button to stop). Writes `e2e-recording.png` and `e2e-saved.png`; the session
/// lands in the sessions folder.
fn e2e(gpu: &mut Offscreen, out: &std::path::Path, settings: &Settings, scale: f32) -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    orbiters_ui::install(&ctx);
    let ctl = Controller::new(settings.clone());
    let backend: Ctl = std::sync::Arc::new(Local::new(ctl.clone()));
    backend.attach(&ctx);
    let mut app = App::offline(backend);
    app.skip_splash();
    let vrchat = StandInVrchat::start(&ctl)?;
    let before = controller::list_sessions(&settings.sessions_dir).len();
    let start = std::time::Instant::now();
    // Where the capture page puts its Test pattern button and the record button at 1280×820.
    let mut clicks = vec![(1.0, pos2(1188.0, 102.0)), (6.0, pos2(511.0, 415.0))];
    let mut shots = vec![(5.0, "e2e-recording"), (8.5, "e2e-saved")];
    let mut release: Option<Pos2> = None;
    while start.elapsed().as_secs_f64() < 9.0 {
        let t = start.elapsed().as_secs_f64();
        let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 820.0))), time: Some(t), ..Default::default() };
        input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(scale), focused: Some(true), ..Default::default() });
        if let Some(p) = release.take() {
            input.events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
        } else if clicks.first().is_some_and(|(at, _)| t >= *at) {
            let (_, p) = clicks.remove(0);
            input.events.push(Event::PointerMoved(p));
            input.events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
            release = Some(p);
        }
        vrchat.tick(t);
        let shot = if shots.first().is_some_and(|(at, _)| t >= *at) { Some(shots.remove(0).1) } else { None };
        let image = gpu.frame(
            &ctx,
            input,
            |ui| {
                app.logic_offline(ui.ctx());
                app.draw(ui);
            },
            shot.is_some(),
        )?;
        if let (Some(name), Some(image)) = (shot, image) {
            let path = out.join(format!("{name}.png"));
            save_png(&path, &image)?;
            println!("{}", path.display());
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    let after = controller::list_sessions(&settings.sessions_dir);
    println!("sessions: {before} before, {} after", after.len());
    if let Some(s) = after.first() {
        let st = &s.manifest.stats;
        println!(
            "newest: {} ({} frames, {} strips decoded, {} keyframes, {} depth frames)",
            s.dir.display(),
            st.frames,
            st.strip_ok,
            st.keyframes,
            st.depth_frames
        );
        vrchat.check(&s.dir, s.manifest.performance.parameter_changes)?;
    }
    ctl.shutdown();
    Ok(())
}

/// What VRChat would do for the e2e run: an OSCQuery tree with current values (read once, when recording starts)
/// and parameter changes sent to the app's OSC port while it records.
struct StandInVrchat {
    socket: std::net::UdpSocket,
    port: u16,
    sent: std::cell::Cell<u32>,
}

impl StandInVrchat {
    fn start(ctl: &controller::Shared) -> anyhow::Result<Self> {
        use std::io::{Read, Write};
        ctl.start_osc_with(false);
        let port = ctl.view().osc.port.context("OSC did not start")?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        ctl.with_osc(|o| o.set_vrchat_query(Some(addr)));
        std::thread::spawn(move || {
            let tree = serde_json::json!({ "FULL_PATH": "/avatar", "ACCESS": 0, "CONTENTS": {
                "change": { "FULL_PATH": "/avatar/change", "TYPE": "s", "ACCESS": 3, "VALUE": ["avtr_e2e"] },
                "parameters": { "FULL_PATH": "/avatar/parameters", "ACCESS": 0, "CONTENTS": {
                    "Viseme": { "FULL_PATH": "/avatar/parameters/Viseme", "TYPE": "i", "ACCESS": 1, "VALUE": [7] },
                    "Toggle": { "FULL_PATH": "/avatar/parameters/Toggle", "TYPE": "T", "ACCESS": 3, "VALUE": [true] }
                } }
            } })
            .to_string();
            for stream in listener.incoming().flatten() {
                let mut s = stream;
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{tree}", tree.len()).as_bytes());
            }
        });
        Ok(Self { socket: std::net::UdpSocket::bind("127.0.0.1:0")?, port, sent: std::cell::Cell::new(0) })
    }

    /// Between 2 and 5 s: a jaw opening and closing every frame, and a head pose every 4th.
    fn tick(&self, t: f64) {
        use rosc::{OscMessage, OscPacket, OscType};
        if !(2.0..5.0).contains(&t) {
            return;
        }
        let n = self.sent.get();
        self.sent.set(n + 1);
        let send = |addr: &str, args: Vec<OscType>| {
            let packet = OscPacket::Message(OscMessage { addr: addr.into(), args });
            let _ = self.socket.send_to(&rosc::encoder::encode(&packet).unwrap(), ("127.0.0.1", self.port));
        };
        send("/avatar/parameters/v2/JawOpen", vec![OscType::Float((t * 3.0).sin().abs() as f32)]);
        if n % 4 == 0 {
            let pose = [0.1, 1.6, 0.2, 0.0, (t * 10.0) as f32, 0.0];
            send("/tracking/vrsystem/head/pose", pose.iter().map(|v| OscType::Float(*v as f32)).collect());
        }
    }

    /// The session holds the initial values, every jaw change and the head poses.
    fn check(&self, dir: &std::path::Path, counted: u64) -> anyhow::Result<()> {
        use orbiters_contracts::performance::{self as perf, parameters};
        let bin = std::fs::read(dir.join(perf::PARAMETERS_FILE)).context("parameters.bin")?;
        let table: parameters::Table = serde_json::from_slice(&std::fs::read(dir.join(perf::PARAMETERS_TABLE)).context("parameters.json")?)?;
        let changes = parameters::read(&bin)?;
        let name = |c: &parameters::Change| table.parameters[c.id as usize].name.clone();
        let initial: Vec<String> =
            changes.iter().filter(|c| c.flags & parameters::flags::INITIAL != 0).map(|c| format!("{}={}", name(c), c.value.as_f32())).collect();
        let jaw = changes.iter().filter(|c| name(c) == "v2/JawOpen").count();
        let pose = changes.iter().filter(|c| name(c).starts_with("/tracking/vrsystem/head/pose[")).count();
        println!(
            "parameters: {} changes ({counted} in the manifest), initial {initial:?}, {jaw} jaw, {pose} head pose values, avatars {:?}",
            changes.len(),
            table.avatars
        );
        let sent = self.sent.get() as usize;
        anyhow::ensure!(counted as usize == changes.len(), "manifest counts {counted}, file has {}", changes.len());
        anyhow::ensure!(initial.contains(&"Viseme=7".to_string()) && initial.contains(&"Toggle=1".to_string()), "initial values {initial:?}");
        anyhow::ensure!(jaw == sent, "{jaw} jaw changes recorded, {sent} sent");
        anyhow::ensure!(pose == sent.div_ceil(4) * 6, "{pose} head pose values for {sent} frames");
        anyhow::ensure!(table.avatars.first().is_some_and(|a| a.id == "avtr_e2e" && a.time_ns == 0), "avatar {:?}", table.avatars);
        let times: Vec<u64> = changes.iter().filter(|c| c.flags == 0).map(|c| c.time_ns).collect();
        anyhow::ensure!(times.windows(2).all(|w| w[0] <= w[1]) && times.first().is_some_and(|t| *t > 500_000_000), "times {:?}", &times[..times.len().min(5)]);
        Ok(())
    }
}

fn idle_snapshot(settings: &Settings, sessions: &[orbiters_core::bundle::SessionSummary]) -> Snapshot {
    let mut s = Snapshot::new(settings.clone());
    s.vrchat.running = true;
    s.vrchat.log_found = true;
    s.vrchat.world = WorldState {
        world_id: Some("wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b".into()),
        world_name: Some("Canine Test Lab".into()),
        instance: Some("12345~private".into()),
        in_world: true,
        log_file: Some(PathBuf::from(r"C:\Users\Enzo\AppData\LocalLow\VRChat\VRChat\output_log_2026-10-20_01-12-44.txt")),
    };
    s.sources = vec![SpoutSourceInfo { name: "VRChat".into(), width: 1920, height: 1080, format: 87, description: String::new() }];
    s.osc.enabled = true;
    s.osc.listening = true;
    s.osc.advertising = true;
    s.osc.port = Some(9001);
    s.storage.used_bytes = sessions.iter().map(|x| x.size_bytes).sum();
    s.storage.sessions = sessions.len();
    s.storage.free_bytes = Some(512_000_000_000);
    s.worker.ready = true;
    s
}

/// A made-up camera path when no session is at hand.
fn spiral() -> Vec<[f32; 3]> {
    (0..400)
        .map(|i| {
            let t = i as f32 * 0.05;
            [t.cos() * (3.0 + t * 0.4), 1.6, t.sin() * (3.0 + t * 0.4)]
        })
        .collect()
}
