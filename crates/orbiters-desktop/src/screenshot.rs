//! `orbiters-desktop --screenshot <out folder> [--sessions <folder>] [--raw <folder>] [--scale <factor>] [--only <name>]
//! [--height <points>]`
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
    /// Points the page is scrolled down by (a mouse wheel over it, half a second before the capture).
    scroll: f32,
    /// The Visited worlds page: a world opened, and what happens in it.
    world: Option<WorldShot>,
}

/// In a world's page (the one with the most captures, or with `pictures` the most pictures): pictures picked or shown
/// large, and what this member already shared.
#[derive(Clone, Default)]
struct WorldShot {
    pictures: bool,
    select: usize,
    view: Option<usize>,
    maps: Option<crate::state::MapsMine>,
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let out =
        PathBuf::from(args.first().context("usage: --screenshot <out folder> [--sessions <folder>] [--raw <folder>] [--scale <factor>] [--only <name>]")?);
    let mut settings = Settings::default();
    let mut scale = 1.0f32;
    let mut height = 820.0f32;
    let mut only: Option<String> = None;
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sessions" => settings.sessions_dir = PathBuf::from(it.next().context("--sessions <folder>")?),
            "--raw" => settings.raw_captures_dir = PathBuf::from(it.next().context("--raw <folder>")?),
            "--scale" => scale = it.next().context("--scale <factor>")?.parse()?,
            "--only" => only = Some(it.next().context("--only <name>")?.clone()),
            "--height" => height = it.next().context("--height <points>")?.parse()?,
            other => anyhow::bail!("unknown option {other}"),
        }
    }
    settings.consent_acknowledged = true;
    std::fs::create_dir_all(&out)?;
    if only.as_deref() == Some("e2e") {
        return e2e(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("splats-3d") {
        return splats_3d(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("e2e-reconstruct") {
        return e2e_reconstruct(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("overlay") {
        return overlay_shots(&out, &settings);
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
        parameter_changes: 41_870,
        mic_level: Some(0.38),
        tracking_connected: Some(true),
        tracking_devices: 6,
        tracking_samples: 44_280,
        skeleton_frames: 3_610,
        ..Default::default()
    });
    let mut consent = idle.clone();
    consent.settings.consent_acknowledged = false;
    let mut reconstructing = idle.clone();
    reconstructing.reconstruction = sessions.first().map(|s| crate::state::ReconstructionJob {
        session_dir: s.dir.clone(),
        stage: orbiters_core::reconstruction::Stage::Training,
        progress: 0.42,
        iteration: 3780,
        iterations: 9000,
        gaussians: 512_400,
        psnr: Some(33.2),
        seconds: 41.0,
    });

    // Visited worlds: the world with the most captures has a 3D preview shared; in the one with the most pictures, the
    // first two count as shared and the next two as being sent.
    let visited = crate::worlds::load(&settings.sessions_dir);
    let captured = visited.iter().max_by_key(|w| (w.reconstructed(), w.sessions.len())).cloned();
    let pictured = visited.iter().max_by_key(|w| w.shots.len()).cloned();
    let signed_in = with_account(&idle, crate::state::AccountState::SignedIn { username: "Desktop Tester".into() }, None);
    let mut maps = crate::state::MapsMine::default();
    let mut sharing = signed_in.clone();
    if let Some(w) = &captured {
        maps.worlds.push(orbiters_core::maps::MineWorld {
            world_id: w.id.clone(),
            preview: Some(orbiters_core::maps::MinePreview { id: 1, splats: 600_000, created_at: None }),
            pictures: Vec::new(),
        });
    }
    if let Some(w) = &pictured {
        let first: Vec<PathBuf> = w.shots.iter().take(2).map(|s| s.path.clone()).collect();
        let hashes = crate::worlds::hashes(first.clone());
        maps.worlds.push(orbiters_core::maps::MineWorld {
            world_id: w.id.clone(),
            preview: None,
            pictures: first.iter().filter_map(|p| hashes.get(p)).enumerate().map(|(i, h)| orbiters_core::maps::MinePicture { id: i as u64 + 1, source_hash: h.clone() }).collect(),
        });
        sharing.uploads.push(crate::state::UploadStatus {
            id: 1,
            world_id: w.id.clone(),
            world_name: w.name.clone(),
            kind: crate::state::UploadKind::Pictures,
            paths: w.shots.iter().skip(2).take(2).map(|s| s.path.clone()).collect(),
            sent: 9_400_000,
            total: 21_000_000,
            state: crate::state::UploadState::Sending,
        });
    }

    let scenarios = vec![
        Scenario {
            name: "splash",
            page: Page::Capture,
            snapshot: idle.clone(),
            frames: 66,
            splash: true,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
        },
        Scenario {
            name: "capture-idle",
            page: Page::Capture,
            snapshot: idle.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
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
            scroll: 0.0,
            world: None,
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
            scroll: 0.0,
            world: None,
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
            scroll: 0.0,
            world: None,
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
            scroll: 0.0,
            world: None,
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
            scroll: 0.0,
            world: None,
        },
        Scenario {
            name: "session-reconstructing",
            page: Page::Sessions,
            snapshot: reconstructing,
            frames: 150,
            splash: false,
            open_detail: true,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
        },
        Scenario {
            name: "settings",
            page: Page::Settings,
            snapshot: idle.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
        },
        // The Orbiters account: signed out, then finishing in the browser.
        Scenario {
            name: "settings-signed-out",
            page: Page::Settings,
            snapshot: with_account(&idle, crate::state::AccountState::SignedOut, None),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
        },
        Scenario {
            name: "settings-signing-in",
            page: Page::Settings,
            snapshot: with_account(&idle, crate::state::AccountState::SigningIn { code: "K7QX".into(), provider: "discord".into() }, None),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
        },
        // Content under the title bar (and the sessions under their heading): blurred, not cut.
        Scenario {
            name: "capture-scrolled",
            page: Page::Capture,
            snapshot: idle.clone(),
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 112.0,
            world: None,
        },
        Scenario {
            name: "settings-scrolled",
            page: Page::Settings,
            snapshot: idle.clone(),
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 250.0,
            world: None,
        },
        Scenario {
            name: "worlds",
            page: Page::Worlds,
            snapshot: signed_in.clone(),
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: Some(pos2(420.0, 260.0)),
            scroll: 0.0,
            world: Some(WorldShot { maps: Some(maps.clone()), ..Default::default() }),
        },
        // The card flying into the world's page, a sixth of a second in.
        Scenario {
            name: "world-flight",
            page: Page::Worlds,
            snapshot: signed_in.clone(),
            frames: 56,
            splash: false,
            open_detail: true,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: Some(WorldShot { maps: Some(maps.clone()), ..Default::default() }),
        },
        Scenario {
            name: "world-detail",
            page: Page::Worlds,
            snapshot: signed_in.clone(),
            frames: 170,
            splash: false,
            open_detail: true,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: Some(WorldShot { maps: Some(maps.clone()), ..Default::default() }),
        },
        Scenario {
            name: "world-pictures",
            page: Page::Worlds,
            snapshot: signed_in.clone(),
            frames: 200,
            splash: false,
            open_detail: true,
            toasts: vec![],
            pointer: None,
            scroll: 560.0,
            world: Some(WorldShot { pictures: true, select: 3, maps: Some(maps.clone()), ..Default::default() }),
        },
        Scenario {
            name: "world-sharing",
            page: Page::Worlds,
            snapshot: sharing,
            frames: 200,
            splash: false,
            open_detail: true,
            toasts: vec![],
            pointer: None,
            scroll: 560.0,
            world: Some(WorldShot { pictures: true, maps: Some(maps.clone()), ..Default::default() }),
        },
        Scenario {
            name: "world-signed-out",
            page: Page::Worlds,
            snapshot: with_account(&idle, crate::state::AccountState::SignedOut, None),
            frames: 200,
            splash: false,
            open_detail: true,
            toasts: vec![],
            pointer: None,
            scroll: 560.0,
            world: Some(WorldShot { pictures: true, select: 2, ..Default::default() }),
        },
        Scenario {
            name: "world-viewer",
            page: Page::Worlds,
            snapshot: signed_in.clone(),
            frames: 200,
            splash: false,
            open_detail: true,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: Some(WorldShot { pictures: true, view: Some(1), maps: Some(maps), ..Default::default() }),
        },
        Scenario {
            name: "sessions-scrolled",
            page: Page::Sessions,
            snapshot: idle,
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 140.0,
            world: None,
        },
    ];

    let mut gpu = Offscreen::new()?;
    for sc in scenarios {
        if only.as_deref().is_some_and(|o| o != sc.name) {
            continue;
        }
        let path = out.join(format!("{}.png", sc.name));
        render(&mut gpu, &sc, scale, height, &path)?;
        println!("{}", path.display());
    }
    Ok(())
}

fn render(gpu: &mut Offscreen, sc: &Scenario, scale: f32, height: f32, path: &std::path::Path) -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    orbiters_ui::install(&ctx);
    let ctl = Controller::new(sc.snapshot.settings.clone());
    ctl.set_snapshot(sc.snapshot.clone());
    let backend: Ctl = std::sync::Arc::new(Local::new(ctl.clone()));
    let mut world_opened = false;
    backend.attach(&ctx);
    let mut app = App::offline(backend);
    app.set_gpu(crate::gpu::Handles { device: gpu.device().clone(), queue: gpu.queue().clone(), textures: gpu.textures().clone() });
    if !sc.splash {
        app.skip_splash();
    }
    let size = vec2(1280.0, height);
    for f in 0..sc.frames {
        let t = f as f64 / 60.0;
        let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), time: Some(t), ..Default::default() };
        input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(scale), focused: Some(true), ..Default::default() });
        if let Some(p) = sc.pointer {
            input.events.push(Event::PointerMoved(p));
        }
        if sc.scroll != 0.0 && f + 30 == sc.frames {
            input.events.push(Event::PointerMoved(pos2(760.0, 500.0)));
            input.events.push(Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, -sc.scroll),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
                source: egui::MouseWheelSource::Wheel,
            });
        }
        if f == 0 {
            app.set_page(sc.page, t);
        }
        if f == 1 {
            for (kind, message) in &sc.toasts {
                app.toasts_mut().push(&ctx, *kind, *message);
            }
        }
        if sc.open_detail && sc.world.is_none() && f == 45 && app.sessions_mut().loaded() {
            app.sessions_mut().open_first(&ctx, Rect::from_min_size(pos2(264.0, 160.0), vec2(300.0, 265.0)));
        }
        if let Some(ws) = &sc.world {
            // After the page asked for it (the stand-in controller has no account and answers nothing).
            if f == 20 {
                if let Some(m) = &ws.maps {
                    ctl.set_maps(m.clone());
                }
            }
            if sc.open_detail && !world_opened && f >= 45 && app.worlds_mut().loaded() {
                world_opened = true;
                let visited = crate::worlds::load(&sc.snapshot.settings.sessions_dir);
                let pick = if ws.pictures { visited.iter().max_by_key(|w| w.shots.len()) } else { visited.iter().max_by_key(|w| (w.reconstructed(), w.sessions.len())) };
                if let Some(w) = pick {
                    app.worlds_mut().open_world(&ctx, &w.id, Rect::from_min_size(pos2(264.0, 152.0), vec2(300.0, 169.0)));
                }
            }
            if f == 110 {
                if ws.select > 0 {
                    // After the two counted as shared.
                    app.worlds_mut().select_some(2, ws.select);
                }
                if let Some(i) = ws.view {
                    app.worlds_mut().view_picture(i);
                }
            }
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
            "newest: {} ({} frames, {} strips decoded, {} keyframes, {} depth frames, {} skeleton frames)",
            s.dir.display(),
            st.frames,
            st.strip_ok,
            st.keyframes,
            st.depth_frames,
            s.manifest.performance.skeleton_frames
        );
        vrchat.check(&s.dir, s.manifest.performance.parameter_changes)?;
    }
    ctl.shutdown();
    Ok(())
}

/// The 3D view of the newest session's splats (`--only splats-3d`): as it opens, then after an orbit drag.
fn splats_3d(gpu: &mut Offscreen, out: &std::path::Path, settings: &Settings, scale: f32) -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    orbiters_ui::install(&ctx);
    let ctl = Controller::new(settings.clone());
    let sessions = controller::list_sessions(&settings.sessions_dir);
    ctl.set_snapshot(idle_snapshot(settings, &sessions));
    let backend: Ctl = std::sync::Arc::new(Local::new(ctl));
    backend.attach(&ctx);
    let mut app = App::offline(backend);
    app.skip_splash();
    let handles = crate::gpu::Handles { device: gpu.device().clone(), queue: gpu.queue().clone(), textures: gpu.textures().clone() };
    app.set_gpu(handles.clone());
    let start = std::time::Instant::now();
    let (mut opened, mut viewing, mut loaded_at) = (false, false, None::<f64>);
    let mut shots = 0;
    let centre = pos2(640.0, 450.0);
    loop {
        let t = start.elapsed().as_secs_f64();
        anyhow::ensure!(t < 120.0, "the 3D view did not load in two minutes");
        let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 820.0))), time: Some(t), ..Default::default() };
        input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(scale), focused: Some(true), ..Default::default() });
        if t < 0.05 {
            app.set_page(Page::Sessions, t);
        }
        if !opened && t > 0.5 && app.sessions_mut().loaded() {
            app.sessions_mut().open_first(&ctx, Rect::from_min_size(pos2(264.0, 160.0), vec2(300.0, 265.0)));
            opened = true;
        }
        if opened && !viewing && t > 1.5 {
            app.sessions_mut().open_viewer(&ctx, &handles);
            viewing = true;
        }
        if loaded_at.is_none() && app.sessions_mut().viewer_loaded() {
            loaded_at = Some(t);
        }
        // After the first shot: drag right and a little up across half a second, then shoot again.
        let drag = loaded_at.map(|l| t - l - 1.0).filter(|d| shots == 1 && (0.0..=0.6).contains(d));
        if let Some(d) = drag {
            let p = centre + vec2(260.0, -80.0) * (d / 0.5).min(1.0) as f32;
            input.events.push(Event::PointerMoved(p));
            if d < 0.05 {
                input.events.push(Event::PointerButton { pos: centre, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
            }
            if d > 0.55 {
                input.events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
            }
        }
        let shot = match loaded_at {
            Some(l) if shots == 0 && t > l + 0.6 => Some("splats-3d"),
            Some(l) if shots == 1 && t > l + 2.0 => Some("splats-3d-orbit"),
            _ => None,
        };
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
            shots += 1;
            if shots == 2 {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
}

/// End to end on the newest session of `--sessions`: opens its detail, presses Reconstruct (the real worker
/// replaces its `reconstruction/` folder) and shoots the progress and the result.
fn e2e_reconstruct(gpu: &mut Offscreen, out: &std::path::Path, settings: &Settings, scale: f32) -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    orbiters_ui::install(&ctx);
    let ctl = Controller::new(settings.clone());
    let backend: Ctl = std::sync::Arc::new(Local::new(ctl.clone()));
    backend.attach(&ctx);
    let mut app = App::offline(backend);
    app.skip_splash();
    let start = std::time::Instant::now();
    let (mut opened, mut clicked_at, mut ran, mut done_at) = (false, None::<f64>, false, None::<f64>);
    let mut release: Option<Pos2> = None;
    let (mut shot_progress, mut paged) = (false, false);
    // Where the detail puts its Reconstruct button at 1280×820.
    let button = pos2(1072.0, 591.0);
    loop {
        let t = start.elapsed().as_secs_f64();
        anyhow::ensure!(t < 900.0, "the reconstruction did not finish in 15 minutes");
        let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 820.0))), time: Some(t), ..Default::default() };
        input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(scale), focused: Some(true), ..Default::default() });
        if !paged {
            app.set_page(Page::Sessions, t);
            paged = true;
        }
        if !opened && t > 0.8 && app.sessions_mut().loaded() {
            app.sessions_mut().open_first(&ctx, Rect::from_min_size(pos2(264.0, 160.0), vec2(300.0, 265.0)));
            opened = true;
        }
        if let Some(p) = release.take() {
            input.events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
        } else if opened && clicked_at.is_none() && t > 4.0 {
            input.events.push(Event::PointerMoved(button));
            input.events.push(Event::PointerButton { pos: button, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
            release = Some(button);
            clicked_at = Some(t);
        }
        let job = ctl.view().reconstruction.clone();
        ran |= job.is_some();
        if ran && job.is_none() && done_at.is_none() {
            done_at = Some(t);
        }
        let shot = if !shot_progress && job.as_ref().is_some_and(|j| j.iteration >= 1000) {
            shot_progress = true;
            Some("e2e-reconstructing")
        } else if done_at.is_some_and(|d| t > d + 3.0) {
            Some("e2e-reconstructed")
        } else {
            None
        };
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
            if name == "e2e-reconstructed" {
                break;
            }
        }
        if clicked_at.is_some_and(|c| t > c + 5.0) && !ran {
            anyhow::bail!("pressing Reconstruct started nothing: {:?}", ctl.take_notices());
        }
        // Light while the worker trains.
        std::thread::sleep(Duration::from_millis(if ran { 100 } else { 16 }));
    }
    for n in ctl.take_notices() {
        println!("notice: {n:?}");
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
        if n.is_multiple_of(4) {
            let pose: [f32; 6] = [0.1, 1.6, 0.2, 0.0, (t * 10.0) as f32, 0.0];
            send("/tracking/vrsystem/head/pose", pose.iter().map(|v| OscType::Float(*v)).collect());
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

fn with_account(base: &Snapshot, state: crate::state::AccountState, error: Option<&str>) -> Snapshot {
    let mut s = base.clone();
    s.account.state = state;
    s.account.error = error.map(str::to_string);
    s
}

fn idle_snapshot(settings: &Settings, sessions: &[orbiters_core::bundle::SessionSummary]) -> Snapshot {
    let mut s = Snapshot::new(settings.clone());
    s.account.state = crate::state::AccountState::SignedIn { username: "Orbit".into() };
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
    s.osc.vrchat_found = true;
    s.osc.values = 214;
    s.steamvr.running = true;
    s.steamvr.overlay = true;
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    s.steamvr.calibration = Some(crate::state::CalibrationInfo { head_height_m: 1.63, arm_span_m: 1.41, devices: 6, utc_ms: now_ms - 7 * 60_000 });
    s.settings.performance.record_mic = true;
    s.mics = vec![
        orbiters_core::protocol::AudioDeviceInfo { id: "index".into(), name: "Microphone (Valve Index)".into(), default: true },
        orbiters_core::protocol::AudioDeviceInfo { id: "desk".into(), name: "Desk microphone (USB)".into(), default: false },
    ];
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

/// A backend that does nothing: the overlay shots press buttons (Calibrate) without touching the app's files.
struct Still;

impl crate::backend::Backend for Still {
    fn view(&self) -> std::sync::Arc<Snapshot> {
        std::sync::Arc::new(Snapshot::new(Settings::default()))
    }
    fn take_notices(&self) -> Vec<controller::Notice> {
        Vec::new()
    }
    fn library_version(&self) -> u64 {
        0
    }
    fn attach(&self, _ctx: &egui::Context) {}
    fn start_capture(&self) {}
    fn stop_capture(&self) {}
    fn pause_capture(&self, _paused: bool) {}
    fn start_test_capture(&self) {}
    fn import_raw_capture(&self, _dir: PathBuf) {}
    fn save_settings(&self, _settings: Settings) {}
    fn trash_session(&self, _dir: PathBuf) {}
    fn reconstruct(&self, _dir: PathBuf) {}
    fn stop_reconstruction(&self) {}
    fn send_to_mixer(&self, _dir: PathBuf, _title: String) {}
    fn sign_in(&self, _provider: &str) {}
    fn reopen_sign_in(&self) {}
    fn cancel_sign_in(&self) {}
    fn sign_out(&self) {}
    fn request_worker_log(&self) {}
    fn worker_log(&self) -> Option<Vec<String>> {
        None
    }
    fn upload_pictures(&self, _world_id: String, _world_name: String, _paths: Vec<PathBuf>) {}
    fn upload_preview(&self, _world_id: String, _world_name: String, _session: PathBuf) {}
    fn request_maps(&self) {}
    fn maps(&self) -> Option<crate::state::MapsMine> {
        None
    }
    fn calibrated(&self, _calibration: orbiters_contracts::performance::calibration::Calibration) {}
    fn quit(&self) {}
}

/// A body in a T-pose (or with the arms down) for the overlay's calibration shots.
struct StandInBody {
    t_pose: bool,
}

impl crate::overlay::ui::Poses for StandInBody {
    fn read(&mut self) -> (Vec<orbiters_contracts::performance::tracking::Device>, Vec<orbiters_contracts::performance::tracking::Sample>) {
        use orbiters_contracts::performance::tracking::{flags, Device, DeviceClass, Sample};
        // A T-pose (the right hand a little higher), or the arms down with one hand raised.
        let (hy, hx, dy) = if self.t_pose { (1.42, 0.68, 0.03) } else { (0.9, 0.3, 0.2) };
        let at = |device: u8, position: [f32; 3]| Sample { device, flags: flags::POSE_VALID, position, rotation: [0.0, 0.0, 0.0, 1.0], ..Default::default() };
        let device = |class, role: &str, serial: &str| Device { class, role: role.into(), serial: serial.into(), ..Default::default() };
        (
            vec![
                device(DeviceClass::Hmd, "head", "HMD"),
                device(DeviceClass::Controller, "left_hand", "L"),
                device(DeviceClass::Controller, "right_hand", "R"),
                device(DeviceClass::Tracker, "waist", "T1"),
                device(DeviceClass::Tracker, "left_foot", "T2"),
                device(DeviceClass::Tracker, "right_foot", "T3"),
            ],
            vec![
                at(0, [0.0, 1.63, 0.0]),
                at(1, [-hx, hy, 0.0]),
                at(2, [hx, hy + dy, 0.0]),
                at(3, [0.02, 0.98, 0.02]),
                at(4, [-0.13, 0.09, 0.03]),
                at(5, [0.13, 0.09, 0.03]),
            ],
        )
    }
}

/// `--only overlay`: the SteamVR dashboard tab in several states, painted on the CPU as SteamVR gets it.
fn overlay_shots(out: &std::path::Path, settings: &Settings) -> anyhow::Result<()> {
    use crate::overlay::ui::{OverlayUi, SIZE};
    use crate::state::CalibrationInfo;
    let sessions = controller::list_sessions(&settings.sessions_dir);
    let mut idle = idle_snapshot(settings, &sessions);
    idle.steamvr.running = true;
    idle.steamvr.overlay = true;
    idle.osc.vrchat_found = true;
    idle.osc.values = 214;
    idle.settings.performance.record_mic = true;
    idle.mics = vec![orbiters_core::protocol::AudioDeviceInfo { id: "1".into(), name: "Microphone (Valve Index)".into(), default: true }];
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    idle.steamvr.calibration = Some(CalibrationInfo { head_height_m: 1.63, arm_span_m: 1.41, devices: 6, utc_ms: now_ms - 7 * 60_000 });
    let mut recording = idle.clone();
    recording.capture.state = CaptureState::Recording;
    recording.capture.source = Some("Spout: VRChat".into());
    recording.settings.performance.record_game_audio = true;
    recording.vrchat.running = true;
    recording.capture.stats = Some(CaptureStats {
        frames: 5_412,
        strip_ok: 5_410,
        elapsed_s: 94.0,
        parameter_changes: 48_210,
        mic_level: Some(0.42),
        game_level: Some(0.18),
        tracking_connected: Some(true),
        tracking_devices: 6,
        tracking_samples: 50_760,
        skeleton_frames: 4_050,
        ..Default::default()
    });
    // (name, snapshot, calibration: (T-pose held, started at this time; the last frame is at 1.48 s)).
    type Shot<'a> = (&'a str, &'a Snapshot, Option<(bool, f64)>);
    let shots: [Shot; 5] = [
        ("overlay-idle", &idle, None),
        ("overlay-recording", &recording, None),
        ("overlay-countdown", &idle, Some((true, 0.0))),
        ("overlay-holding", &idle, Some((false, -2.0))),
        ("overlay-calibrated", &idle, Some((true, -3.0))),
    ];
    for (name, snap, calibrate) in shots {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let backend: Ctl = std::sync::Arc::new(Still);
        let mut ui_state = OverlayUi::default();
        let mut body = StandInBody { t_pose: calibrate.is_some_and(|(t_pose, _)| t_pose) };
        let mut sw = crate::overlay::painter();
        let frames = 90;
        let mut image = Vec::new();
        for f in 0..frames {
            let t = f as f64 / 60.0;
            if let (Some((_, start)), 0) = (calibrate, f) {
                ui_state.start_calibration(start, &mut body);
                // The dashboard closes: the countdown starts at once.
                ui_state.tick(start, false, &mut body, &backend);
            }
            ui_state.tick(t, false, &mut body, &backend);
            let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)), time: Some(t), ..Default::default() };
            input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(1.0), focused: Some(true), ..Default::default() });
            let mut full = ctx.run_ui(input, |ui| ui_state.show(ui, snap, &backend, &mut body));
            sw.set_textures(&mut full.textures_delta);
            if f == frames - 1 {
                let prims = ctx.tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
                let started = std::time::Instant::now();
                image = sw.paint(&prims, full.pixels_per_point, orbiters_ui::theme::INK_950).to_vec();
                let vertices: usize = prims
                    .iter()
                    .map(|p| match &p.primitive {
                        egui::epaint::Primitive::Mesh(m) => m.vertices.len(),
                        _ => 0,
                    })
                    .sum();
                println!("{name}: painted in {:.1} ms ({} meshes, {vertices} vertices)", started.elapsed().as_secs_f64() * 1000.0, prims.len());
            }
            sw.free_textures(&mut full.textures_delta);
        }
        let img = egui::ColorImage::from_rgba_premultiplied([SIZE.x as usize, SIZE.y as usize], &image);
        let path = out.join(format!("{name}.png"));
        save_png(&path, &img)?;
        println!("{}", path.display());

        // The mirror and the hand targets as they stand in the play space at the last frame (on a dark ground,
        // as the play space usually is behind them).
        if calibrate.is_some() {
            use crate::overlay::calibration::{self, MIRROR_PX, TARGET_PX};
            let t = (frames - 1) as f64 / 60.0;
            let cal = &ui_state.calibrator;
            let mut canvas = crate::overlay::Canvas::new(MIRROR_PX[0], MIRROR_PX[1]);
            let mut rgba = Vec::new();
            for _ in 0..2 {
                rgba = canvas.paint(t, |p| calibration::paint_mirror(p, cal, t)).to_vec();
            }
            let mirror = name.replace("overlay-", "mirror-");
            save_png(&out.join(format!("{mirror}.png")), &over_ground(&rgba, MIRROR_PX[0], MIRROR_PX[1]))?;
            let mut canvas = crate::overlay::Canvas::new(TARGET_PX, TARGET_PX);
            for (i, label) in ["L", "R"].into_iter().enumerate() {
                let on = cal.hand_on_target(i);
                for _ in 0..2 {
                    rgba = canvas.paint(t, |p| calibration::paint_target(p, label, on, t)).to_vec();
                }
                save_png(&out.join(format!("{}-target-{label}.png", mirror)), &over_ground(&rgba, TARGET_PX, TARGET_PX))?;
            }
            println!("{}", out.join(format!("{mirror}.png")).display());
        }
    }
    Ok(())
}

/// A premultiplied RGBA picture over a dark grey ground.
fn over_ground(rgba: &[u8], width: u32, height: u32) -> egui::ColorImage {
    let ground = [38u8, 40, 46];
    let pixels: Vec<u8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = p[3] as u32;
            let mix = |c: u8, g: u8| (c as u32 + g as u32 * (255 - a) / 255).min(255) as u8;
            [mix(p[0], ground[0]), mix(p[1], ground[1]), mix(p[2], ground[2]), 255]
        })
        .collect();
    egui::ColorImage::from_rgba_premultiplied([width as usize, height as usize], &pixels)
}
