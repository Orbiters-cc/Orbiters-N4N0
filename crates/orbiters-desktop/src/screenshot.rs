//! `orbiters-desktop --screenshot <out folder> [--sessions <folder>] [--raw <folder>] [--scale <factor>] [--only <name>]`
//! renders the interface offscreen in several states and writes one PNG per state, for visual review without a
//! window. The controller is not started: the snapshots are made up, the session library is real.

use crate::app::App;
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
    ctl.attach(&ctx);
    let mut app = App::offline(ctl);
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
