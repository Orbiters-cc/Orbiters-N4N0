//! `orbiters-desktop --screenshot <out folder> [--sessions <folder>] [--raw <folder>] [--scale <factor>] [--only <name>]
//! [--height <points>] [--width <points>]`
//! renders the interface offscreen in several states and writes one PNG per state, for visual review without a
//! window. The controller is not started: the snapshots are made up, the session library is real. `--only e2e`
//! instead records a real test pattern through the worker, driven by synthetic clicks, with avatar parameters
//! from a stand-in VRChat (OSC on the loopback interface, current values from its OSCQuery tree). The end-to-end
//! runs click controls found by their labels where the last frame drew them, and wait on what the app shows or the
//! controller says (each wait with a time limit), never on fixed coordinates or delays.

use crate::app::App;
use crate::backend::{Ctl, Local};
use crate::controller::{self, Controller};
use crate::pages::Page;
use crate::state::{CaptureState, Snapshot};
use anyhow::Context as _;
use egui::{pos2, vec2, Event, Pos2, RawInput, Rect, Vec2, ViewportId, ViewportInfo};
use orbiters_core::protocol::{coverage, CaptureStats, CoverageMap, SpoutSourceInfo};
use orbiters_core::settings::Settings;
use orbiters_core::vrchat_log::WorldState;
use orbiters_orb::menu::Page as MenuPage;
use orbiters_ui::offscreen::{save_png, Offscreen};
use orbiters_ui::{toast, widgets};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
    /// Home with one of its features' pages open ([`stage_home`]).
    home: Option<HomeShot>,
}

/// One of Home's pages open: with the overlay's sample content (a search typed, its results), or as it opens.
#[derive(Clone, Copy)]
struct HomeShot {
    page: MenuPage,
    samples: bool,
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
    let mut width = 1280.0f32;
    let mut only: Option<String> = None;
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sessions" => settings.sessions_dir = PathBuf::from(it.next().context("--sessions <folder>")?),
            "--raw" => settings.raw_captures_dir = PathBuf::from(it.next().context("--raw <folder>")?),
            "--scale" => scale = it.next().context("--scale <factor>")?.parse()?,
            "--only" => only = Some(it.next().context("--only <name>")?.clone()),
            "--height" => height = it.next().context("--height <points>")?.parse()?,
            "--width" => width = it.next().context("--width <points>")?.parse()?,
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
    recording.capture.coverage = Some(sample_coverage(&trail));
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
        integrated: true,
    });
    let job = reconstructing.reconstruction.clone();
    let waiting = |base: &Snapshot| {
        let mut s = base.clone();
        s.reconstruction = sessions.get(1).zip(job.clone()).map(|(r, j)| crate::state::ReconstructionJob { session_dir: r.dir.clone(), ..j });
        s.reconstruct_queue = sessions.iter().enumerate().filter(|(i, _)| *i != 1).map(|(_, r)| r.dir.clone()).collect();
        s
    };

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
            pictures: first
                .iter()
                .filter_map(|p| hashes.get(p))
                .enumerate()
                .map(|(i, h)| orbiters_core::maps::MinePicture { id: i as u64 + 1, source_hash: h.clone() })
                .collect(),
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

    let orb_busy = orb_snapshot(&signed_in);
    let orb_new = {
        let mut s = with_account(&idle, crate::state::AccountState::SignedOut, None);
        s.vrchat.world.avatar_name = Some("Rexouium".into());
        s.orb.avatar_id = Some("avtr_7f3c2b1e-0d4a-4a7b-9c55-1f0e9a2b6c11".into());
        s
    };

    // Home: N4N0's menu with what the service knows (friends to call, the light on, fast walk on).
    let home = {
        let mut s = with_calls(&orb_busy, false);
        s.orb.light = Some(crate::state::OrbLight { on: true, color: 1, level: 0.6 });
        s.settings.walk_boost.enabled = true;
        s
    };
    // Home as many first see it: VRChat in desktop mode (no fast walk), calls off, no light on this N4N0.
    let home_plain = {
        let mut s = idle.clone();
        s.orb.desktop_mode = true;
        s.settings.orb.wake_word = true;
        s
    };
    let home_listening = {
        let mut s = home.clone();
        s.orb.listening = true;
        s.orb.panel = Some("Fast walk is on. Say what you want, or tap a tile.".into());
        s
    };

    let scenarios = vec![
        Scenario {
            name: "home",
            page: Page::Home,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
        },
        Scenario {
            name: "home-listening",
            page: Page::Home,
            snapshot: home_listening,
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: Some(pos2(560.0, 300.0)),
            scroll: 0.0,
            world: None,
            home: None,
        },
        Scenario {
            name: "home-youtube",
            page: Page::Home,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Search, samples: true }),
        },
        Scenario {
            name: "home-results",
            page: Page::Home,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Results, samples: true }),
        },
        Scenario {
            name: "home-results-scrolled",
            page: Page::Home,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 560.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Results, samples: true }),
        },
        Scenario {
            name: "home-call",
            page: Page::Home,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Call, samples: true }),
        },
        Scenario {
            name: "home-ringing",
            page: Page::Home,
            snapshot: with_calls(&home, true),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
        },
        Scenario {
            name: "home-photo",
            page: Page::Home,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Photo, samples: true }),
        },
        Scenario {
            name: "home-music",
            page: Page::Home,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Music, samples: true }),
        },
        Scenario {
            name: "home-light",
            page: Page::Home,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Light, samples: true }),
        },
        Scenario {
            name: "home-plain",
            page: Page::Home,
            snapshot: home_plain.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
        },
        Scenario {
            name: "home-plain-youtube",
            page: Page::Home,
            snapshot: home_plain.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Search, samples: false }),
        },
        Scenario {
            name: "home-plain-call",
            page: Page::Home,
            snapshot: home_plain.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Call, samples: false }),
        },
        Scenario {
            name: "home-plain-light",
            page: Page::Home,
            snapshot: home_plain.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Light, samples: false }),
        },
        Scenario {
            name: "orb-calls",
            page: Page::Orb,
            snapshot: with_calls(&orb_busy, true),
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 1060.0,
            world: None,
            home: None,
        },
        Scenario {
            name: "orb",
            page: Page::Orb,
            snapshot: orb_busy.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
        },
        Scenario {
            name: "orb-testing",
            page: Page::Orb,
            snapshot: {
                let mut s = orb_busy.clone();
                s.orb.test_face = Some(crate::state::OrbFace::Happy);
                s.orb.test_afk = true;
                s
            },
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 100_000.0,
            world: None,
            home: None,
        },
        Scenario {
            name: "orb-scrolled",
            page: Page::Orb,
            snapshot: orb_busy.clone(),
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 760.0,
            world: None,
            home: None,
        },
        // N4N0's menu settings: its keyboard and look.
        Scenario {
            name: "orb-menu-settings",
            page: Page::Orb,
            snapshot: orb_busy,
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 1180.0,
            world: None,
            home: None,
        },
        Scenario {
            name: "orb-new",
            page: Page::Orb,
            snapshot: orb_new,
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
        },
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
            home: None,
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
            home: None,
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
            home: None,
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
            home: None,
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
            home: None,
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
            home: None,
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
            home: None,
        },
        Scenario {
            name: "sessions-waiting",
            page: Page::Sessions,
            snapshot: waiting(&idle),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
        },
        Scenario {
            name: "session-waiting",
            page: Page::Sessions,
            snapshot: waiting(&idle),
            frames: 150,
            splash: false,
            open_detail: true,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
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
            home: None,
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
            home: None,
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
            home: None,
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
            home: None,
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
            home: None,
        },
        Scenario {
            name: "settings-walk",
            page: Page::Settings,
            snapshot: {
                let mut s = idle.clone();
                s.settings.walk_boost.enabled = true;
                s.settings.world_performance = true;
                s
            },
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 1750.0,
            world: None,
            home: None,
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
            home: None,
        },
        // The end of Settings: starting with Windows and SteamVR (SteamVR to be told at its next start), interface, about.
        Scenario {
            name: "settings-start",
            page: Page::Settings,
            snapshot: {
                let mut s = idle.clone();
                s.settings.startup.steamvr = true;
                s.settings.startup.steamvr_pending = true;
                s
            },
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 100_000.0,
            world: None,
            home: None,
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
            home: None,
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
            home: None,
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
            home: None,
        },
        Scenario {
            name: "world-waiting",
            page: Page::Worlds,
            snapshot: waiting(&signed_in),
            frames: 170,
            splash: false,
            open_detail: true,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: Some(WorldShot { maps: Some(maps.clone()), ..Default::default() }),
            home: None,
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
            home: None,
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
            home: None,
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
            home: None,
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
            home: None,
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
            home: None,
        },
    ];

    // Home's music is the overlay's sample song, never what this computer plays.
    crate::media::watcher().stand_in(crate::media::Reading {
        media: Some(crate::overlay::menu_pages::sample_media()),
        volume: Some(orbiters_orb::menu::Volume { percent: 40, muted: false }),
        art: Some(std::sync::Arc::new(crate::media::Art { key: crate::overlay::menu_pages::SAMPLE_ART.into(), rgb: crate::overlay::menu_pages::sample_art() })),
    });

    let mut gpu = Offscreen::new()?;
    for sc in scenarios {
        if only.as_deref().is_some_and(|o| o != sc.name) {
            continue;
        }
        let path = out.join(format!("{}.png", sc.name));
        render(&mut gpu, &sc, scale, vec2(width, height), &path)?;
        println!("{}", path.display());
    }
    Ok(())
}

fn render(gpu: &mut Offscreen, sc: &Scenario, scale: f32, size: Vec2, path: &std::path::Path) -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    orbiters_ui::install(&ctx);
    let ctl = Controller::new(sc.snapshot.settings.clone());
    ctl.set_snapshot(sc.snapshot.clone());
    let backend: Ctl = std::sync::Arc::new(Local::new(ctl.clone()));
    let mut world_opened = false;
    backend.attach(&ctx);
    let mut app = App::offline(backend);
    app.set_gpu(crate::gpu::Handles::of_offscreen(gpu));
    if !sc.splash {
        app.skip_splash();
    }
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
        // Home made its menu on the first frame.
        if let (Some(shot), 2) = (sc.home, f) {
            if let Some(host) = app.home_mut().menu_host() {
                stage_home(host, shot);
            }
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
                let pick = if ws.pictures {
                    visited.iter().max_by_key(|w| w.shots.len())
                } else {
                    visited.iter().max_by_key(|w| (w.reconstructed(), w.sessions.len()))
                };
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
        if capture {
            // What the page still reads (the list, pictures) lands before the shot: a slow disk makes the run
            // longer, not the picture emptier. The frames meanwhile stand at the same time.
            let limit = Instant::now() + Duration::from_secs(20);
            while app.page_busy() && Instant::now() < limit {
                std::thread::sleep(Duration::from_millis(10));
                gpu.frame(
                    &ctx,
                    input.clone(),
                    |ui| {
                        app.logic_offline(ui.ctx());
                        app.draw(ui);
                    },
                    false,
                )?;
            }
        }
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

/// Home with one of its features' pages open: YouTube with the overlay's sample suggestions or results (their
/// thumbnails too, nothing fetched) or as it opens; calls, the music and the light as the snapshot and the stand-in
/// music say.
fn stage_home(host: &mut crate::menu_host::MenuHost, HomeShot { page, samples }: HomeShot) {
    use crate::overlay::menu_pages::{sample_rgba, sample_suggestions, sample_videos};
    use orbiters_orb::menu::{Effect, Target};
    let open = match page {
        MenuPage::Home => return,
        MenuPage::Search | MenuPage::Results => Target::YouTube,
        MenuPage::Call => Target::Call,
        MenuPage::Photo => Target::Photo,
        MenuPage::Music => Target::Music,
        MenuPage::Light => Target::Light,
    };
    host.press(open);
    if !samples {
        return;
    }
    match page {
        MenuPage::Search => {
            if let Some(Effect::Suggest { id, text }) = host.menu.set_query("lofi hi").into_iter().next() {
                host.menu.suggestions_arrived(id, &text, sample_suggestions());
            }
        }
        MenuPage::Results => {
            if let Some(Effect::Search { id, .. }) = host.menu.search_for("lofi hip hop").into_iter().next() {
                let videos = sample_videos();
                for (i, video) in videos.iter().enumerate() {
                    let (w, h, rgba) = sample_rgba(i);
                    host.put_picture(&video.id, egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba));
                }
                host.menu.results_arrived(id, orbiters_orb::youtube::Results { videos, continuation: Some("next".into()) }, false);
            }
        }
        _ => {}
    }
}

/// An offscreen run in real time for the end-to-end flows: a frame about every 16 ms, presses on controls found by
/// their labels, and waits on what the app or the controller shows, each with a time limit.
struct Run<'a> {
    gpu: &'a mut Offscreen,
    ctx: egui::Context,
    app: App,
    scale: f32,
    start: Instant,
    /// A press of the last frame, let go of in this one.
    release: Option<Pos2>,
    /// Pause between frames (longer while a reconstruction trains).
    pace: Duration,
}

impl<'a> Run<'a> {
    fn new(gpu: &'a mut Offscreen, backend: Ctl, scale: f32) -> Self {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        widgets::watch_controls();
        backend.attach(&ctx);
        let mut app = App::offline(backend);
        app.skip_splash();
        Self { gpu, ctx, app, scale, start: Instant::now(), release: None, pace: Duration::from_millis(16) }
    }

    fn time(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// One frame with `events` (and the release of last frame's press); its picture when `capture`.
    fn frame(&mut self, mut events: Vec<Event>, capture: bool) -> anyhow::Result<Option<egui::ColorImage>> {
        let t = self.time();
        let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 820.0))), time: Some(t), ..Default::default() };
        input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(self.scale), focused: Some(true), ..Default::default() });
        if let Some(p) = self.release.take() {
            events.insert(0, Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
        }
        input.events = events;
        let app = &mut self.app;
        let image = self.gpu.frame(
            &self.ctx,
            input,
            |ui| {
                app.logic_offline(ui.ctx());
                app.draw(ui);
            },
            capture,
        )?;
        std::thread::sleep(self.pace);
        Ok(image)
    }

    /// Frames until `done` says so, checked after each; fails after `timeout` seconds, naming what was awaited.
    fn until(&mut self, what: &str, timeout: f64, mut done: impl FnMut(&mut Self) -> bool) -> anyhow::Result<()> {
        let limit = self.time() + timeout;
        loop {
            self.frame(Vec::new(), false)?;
            if done(self) {
                return Ok(());
            }
            anyhow::ensure!(self.time() < limit, "waited {timeout} s for {what}");
        }
    }

    /// Frames for `seconds` (an animation settling before a shot).
    fn wait(&mut self, seconds: f64) -> anyhow::Result<()> {
        let end = self.time() + seconds;
        self.until("a pause", seconds + 10.0, |run| run.time() >= end)
    }

    /// Waits for the control labelled `label` to be drawn, to take clicks and to stand still for a few frames (its
    /// entrance or a morph over), then presses it at its centre and lets go on the next frame.
    fn click(&mut self, label: &str, timeout: f64) -> anyhow::Result<()> {
        let (mut last, mut still) = (None::<Rect>, 0);
        self.until(&format!("the {label:?} control"), timeout, |run| match widgets::control(&run.ctx, label) {
            Some((rect, true)) => {
                still = if last == Some(rect) { still + 1 } else { 0 };
                last = Some(rect);
                still >= 5
            }
            _ => {
                (last, still) = (None, 0);
                false
            }
        })?;
        let p = last.map(|r| r.center()).unwrap_or_default();
        let press = Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() };
        self.frame(vec![Event::PointerMoved(p), press], false)?;
        self.release = Some(p);
        self.frame(Vec::new(), false)?;
        Ok(())
    }

    fn shoot(&mut self, out: &Path, name: &str) -> anyhow::Result<()> {
        let image = self.frame(Vec::new(), true)?.context("the frame gave no picture")?;
        let path = out.join(format!("{name}.png"));
        save_png(&path, &image)?;
        println!("{}", path.display());
        Ok(())
    }
}

/// `--only e2e`: a real test-pattern recording through the capture worker, driven by synthetic clicks in real time
/// (Test pattern, then the record button to stop). Writes `e2e-recording.png` and `e2e-saved.png`; the session
/// lands in the sessions folder.
fn e2e(gpu: &mut Offscreen, out: &std::path::Path, settings: &Settings, scale: f32) -> anyhow::Result<()> {
    let ctl = Controller::new(settings.clone());
    let backend: Ctl = std::sync::Arc::new(Local::new(ctl.clone()));
    let vrchat = StandInVrchat::start(&ctl)?;
    let before = controller::list_sessions(&settings.sessions_dir).len();
    let mut run = Run::new(gpu, backend, scale);
    // The window opens on Home: recording is on Capture.
    let t = run.time();
    run.app.set_page(Page::Capture, t);
    run.click("Test pattern", 20.0)?;
    run.until("the test pattern to record", 30.0, |_| {
        let v = ctl.view();
        v.capture.state == CaptureState::Recording && v.capture.stats.as_ref().is_some_and(|s| s.frames > 0)
    })?;
    // A second into the recording the stand-in VRChat sends for three seconds.
    vrchat.send_from(run.time() + 1.0);
    run.until("the stand-in VRChat's parameters", 20.0, |run| {
        let t = run.time();
        vrchat.tick(t);
        vrchat.finished(t)
    })?;
    run.wait(0.5)?;
    run.shoot(out, "e2e-recording")?;
    run.click(orbiters_ui::record::LABEL, 10.0)?;
    let mut listed = 0.0;
    run.until("the saved session", 60.0, |run| {
        // The library is walked twice a second at most, once the capture is over.
        if ctl.view().capture.state != CaptureState::Idle || run.time() - listed < 0.5 {
            return false;
        }
        listed = run.time();
        controller::list_sessions(&settings.sessions_dir).len() > before
    })?;
    // The "Session saved" toast comes in.
    run.wait(0.8)?;
    run.shoot(out, "e2e-saved")?;
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
    let ctl = Controller::new(settings.clone());
    let sessions = controller::list_sessions(&settings.sessions_dir);
    ctl.set_snapshot(idle_snapshot(settings, &sessions));
    let backend: Ctl = std::sync::Arc::new(Local::new(ctl));
    let handles = crate::gpu::Handles::of_offscreen(gpu);
    let mut run = Run::new(gpu, backend, scale);
    run.app.set_gpu(handles.clone());
    let t = run.time();
    run.app.set_page(Page::Sessions, t);
    run.until("the session list", 30.0, |run| run.app.sessions_mut().loaded())?;
    let ctx = run.ctx.clone();
    run.app.sessions_mut().open_first(&ctx, Rect::from_min_size(pos2(264.0, 160.0), vec2(300.0, 265.0)));
    anyhow::ensure!(run.app.sessions_mut().open_dir().is_some(), "no session in {}", settings.sessions_dir.display());
    // The detail opens, then the 3D view over it.
    run.wait(1.0)?;
    run.app.sessions_mut().open_viewer(&ctx, &handles);
    run.until("the 3D view's splats", 120.0, |run| run.app.sessions_mut().viewer_loaded())?;
    run.wait(0.6)?;
    run.shoot(out, "splats-3d")?;
    // Drag right and a little up across half a second, then shoot again.
    let centre = pos2(640.0, 450.0);
    let press = Event::PointerButton { pos: centre, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() };
    run.frame(vec![Event::PointerMoved(centre), press], false)?;
    let from = run.time();
    loop {
        let d = run.time() - from;
        let p = centre + vec2(260.0, -80.0) * (d / 0.5).min(1.0) as f32;
        run.frame(vec![Event::PointerMoved(p)], false)?;
        if d > 0.55 {
            run.frame(vec![Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }], false)?;
            break;
        }
    }
    run.wait(1.4)?;
    run.shoot(out, "splats-3d-orbit")?;
    // What the keyframes saw poorly, faded, then hazed, from the recorded view again: the control shows where the PLY
    // says what they saw.
    run.click("Recorded view", 3.0)?;
    run.wait(0.8)?;
    for (label, name) in [("Fade", "splats-3d-fade"), ("Haze", "splats-3d-haze"), ("Fog", "splats-3d-fog"), ("Crop", "splats-3d-crop")] {
        if run.click(label, 3.0).is_err() {
            break;
        }
        run.wait(0.8)?;
        run.shoot(out, name)?;
    }
    Ok(())
}

/// End to end on the newest session of `--sessions`: opens its detail, presses Reconstruct (the real worker
/// replaces its `reconstruction/` folder) and shoots the progress and the result.
fn e2e_reconstruct(gpu: &mut Offscreen, out: &std::path::Path, settings: &Settings, scale: f32) -> anyhow::Result<()> {
    use orbiters_core::reconstruction::Stage;
    let ctl = Controller::new(settings.clone());
    let backend: Ctl = std::sync::Arc::new(Local::new(ctl.clone()));
    let mut run = Run::new(gpu, backend, scale);
    let t = run.time();
    run.app.set_page(Page::Sessions, t);
    run.until("the session list", 30.0, |run| run.app.sessions_mut().loaded())?;
    let ctx = run.ctx.clone();
    run.app.sessions_mut().open_first(&ctx, Rect::from_min_size(pos2(264.0, 160.0), vec2(300.0, 265.0)));
    let dir = run.app.sessions_mut().open_dir().with_context(|| format!("no session in {}", settings.sessions_dir.display()))?;
    let before = run.app.sessions_mut().reconstruction_of(&dir);
    // Its button, once the detail has opened and stands still.
    let label = if before.is_some() { "Reconstruct again" } else { "Reconstruct" };
    run.click(label, 20.0)?;
    if run.until("the reconstruction to start", 15.0, |_| ctl.view().reconstruction.is_some()).is_err() {
        anyhow::bail!("pressing {label} started nothing: {:?}", ctl.take_notices());
    }
    // Light while the worker trains.
    run.pace = Duration::from_millis(100);
    run.until("training under way", 600.0, |_| ctl.view().reconstruction.as_ref().is_some_and(|j| j.stage == Stage::Training && j.progress >= 0.1))?;
    run.shoot(out, "e2e-reconstructing")?;
    run.until("the reconstruction to finish", 900.0, |_| ctl.view().reconstruction.is_none())?;
    run.pace = Duration::from_millis(16);
    // The library reloads with the new report, and the detail shows its preview.
    run.until("the new splats in the session's detail", 60.0, |run| {
        let now = run.app.sessions_mut().reconstruction_of(&dir);
        now.is_some() && now != before
    })?;
    run.wait(1.0)?;
    run.shoot(out, "e2e-reconstructed")?;
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
    /// When it starts sending (once the recording runs).
    from: std::cell::Cell<Option<f64>>,
}

/// How long the stand-in VRChat sends parameters.
const SEND_FOR: f64 = 3.0;

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
        Ok(Self { socket: std::net::UdpSocket::bind("127.0.0.1:0")?, port, sent: std::cell::Cell::new(0), from: std::cell::Cell::new(None) })
    }

    /// Sends for [`SEND_FOR`] seconds from `t` on.
    fn send_from(&self, t: f64) {
        self.from.set(Some(t));
    }

    fn finished(&self, t: f64) -> bool {
        self.from.get().is_some_and(|from| t >= from + SEND_FOR)
    }

    /// While it sends: a jaw opening and closing every frame, and a head pose every 4th.
    fn tick(&self, t: f64) {
        use rosc::{OscMessage, OscPacket, OscType};
        if !self.from.get().is_some_and(|from| (from..from + SEND_FOR).contains(&t)) {
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

/// N4N0 on an avatar with its prop and menu, a conversation under way, saved videos and the speech models installed.
fn orb_snapshot(base: &Snapshot) -> Snapshot {
    use crate::state::{OrbConfirmation, OrbFace, OrbLine, OrbWho, ProcessUsage};
    let mut s = base.clone();
    s.vrchat.world.avatar_name = Some("Rexouium".into());
    s.vrchat.world.last_video = Some(orbiters_core::vrchat_log::VideoSeen { url: "https://www.youtube.com/watch?v=36YnV9STBqc".into(), at: None });
    s.settings.orb.afk_message_enabled = true;
    s.settings.orb.afk_message = "Getting food, back in 10!".into();
    s.settings.orb.pins = vec![orbiters_core::settings::Pin { parameter: "Hoodie".into(), value: orbiters_core::osc::OscValue::Bool(true) }];
    let line = |who, text: &str| OrbLine { who, text: text.into(), at_ms: 0 };
    let o = &mut s.orb;
    o.avatar_id = Some("avtr_7f3c2b1e-0d4a-4a7b-9c55-1f0e9a2b6c11".into());
    o.prop_sync = Some(8);
    o.manifest = true;
    o.controls = 42;
    o.face = OrbFace::Talking;
    o.panel = Some("Done, hoodie on. Now go find\nyour friends instead of\ntalking to a floating ball.".into());
    o.lines = vec![
        line(OrbWho::You, "Nano, mets mon sweat à capuche"),
        line(OrbWho::Note, "N4N0 set Clothes / Hoodie"),
        line(OrbWho::Orb, "Done, hoodie on. Now go find your friends instead of talking to a floating ball."),
        line(OrbWho::You, "Plan a movie night for my community on Saturday at 9 pm"),
        line(OrbWho::Note, "N4N0: create event draft…"),
        line(OrbWho::Orb, "Drafted “Movie night” for Saturday 21:00 in Cozy Cinema. Approve it on Orbiters and I'll announce it."),
    ];
    o.confirmations = vec![OrbConfirmation {
        id: "c1".into(),
        title: "Publish “Movie night”".into(),
        summary: "Saturday 21:00 · Cozy Cinema · announced in #events".into(),
        approve_url: "https://orbiters.cc/confirm/c1".into(),
    }];
    o.quota = Some(orbiters_orb::assistant::Quota { used: 13, limit: 80, resets_at: None, own_key: false });
    o.voice = crate::state::VoiceStatus { installed: true, running: true, ..Default::default() };
    o.videos = vec![
        orbiters_orb::videos::SavedVideo {
            url: "https://www.youtube.com/watch?v=36YnV9STBqc".into(),
            title: Some("Lo-fi beats to sit in a VRChat club to".into()),
            world: Some("The Black Cat".into()),
            saved_utc: "2026-10-20T22:14:03Z".into(),
        },
        orbiters_orb::videos::SavedVideo {
            url: "https://youtu.be/pUaeOcyzDrA".into(),
            title: None,
            world: Some("Palm Point - Night".into()),
            saved_utc: "2026-10-18T21:33:48Z".into(),
        },
    ];
    o.resources = vec![
        ProcessUsage { name: "Orbiters - N4N0".into(), cpu_percent: 1.2, memory_mb: 148.0 },
        ProcessUsage { name: "N4N0's ears".into(), cpu_percent: 0.4, memory_mb: 912.0 },
        ProcessUsage { name: "VRChat".into(), cpu_percent: 187.0, memory_mb: 6840.0 },
    ];
    s
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
        ..Default::default()
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
/// A coverage map for the recording shot: seen from many directions along the path, fewer farther out, from one or
/// two at the edge, and a wall around it all; with two hints.
fn sample_coverage(trail: &[[f32; 3]]) -> CoverageMap {
    let cell = 0.5f32;
    let (lo, hi) = trail.iter().fold(([f32::MAX; 2], [f32::MIN; 2]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[2])], [hi[0].max(p[0]), hi[1].max(p[2])]));
    let (x0, z0) = (((lo[0] - 7.0) / cell).floor() * cell, ((lo[1] - 7.0) / cell).floor() * cell);
    let (width, depth) = ((((hi[0] - lo[0]) + 14.0) / cell) as u32, (((hi[1] - lo[1]) + 14.0) / cell) as u32);
    let mut cells = vec![0u8; (2 * width * depth) as usize];
    for iz in 0..depth {
        for ix in 0..width {
            let (x, z) = (x0 + (ix as f32 + 0.5) * cell, z0 + (iz as f32 + 0.5) * cell);
            let near = trail.iter().step_by(4).map(|p| ((p[0] - x).powi(2) + (p[2] - z).powi(2)).sqrt()).fold(f32::MAX, f32::min);
            let i = (2 * (iz * width + ix)) as usize;
            let (directions, bits) = match near {
                d if d < 2.0 => (9, coverage::FLOOR | coverage::HIGH | coverage::LOW),
                d if d < 3.5 => (4, coverage::FLOOR | coverage::LOW),
                d if d < 5.0 => (2, coverage::FLOOR | coverage::GRAZING),
                d if d < 5.5 => (2, coverage::OBSTACLE | coverage::GRAZING),
                _ => (0, 0),
            };
            cells[i] = directions;
            cells[i + 1] = bits;
        }
    }
    CoverageMap {
        cell_m: cell,
        x0,
        z0,
        width,
        depth,
        cells,
        floor_y: Some(0.0),
        hints: vec![
            "What is behind you to the left (about 5 m away) was only seen from one direction: look at it from another angle too.".into(),
            "Crouch or raise the camera for a moment: everything so far was seen from one height.".into(),
        ],
    }
}

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
    fn orb(&self, _command: crate::ipc::OrbCommand) {}

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
    fn patch_settings(&self, _patch: serde_json::Value) {}
    fn trash_session(&self, _dir: PathBuf) {}
    fn reconstruct(&self, _dir: PathBuf) {}
    fn stop_reconstruction(&self) {}
    fn unqueue_reconstruction(&self, _dir: PathBuf) {}
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
    fn frames(&self, _chunk: crate::state::FrameChunk) {}
    fn headset(&self, _in_use: bool) {}
    fn walk_boost(&self, _strength: f32) {}
    fn osc_reconnect(&self) {}
    fn updates(&self, _action: crate::ipc::UpdateAction) {}
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
    let mut talking = orb_snapshot(&with_account(&idle, crate::state::AccountState::SignedIn { username: "Desktop Tester".into() }, None));
    talking.vrchat.running = true;
    let mut listening = talking.clone();
    listening.orb.listening = true;
    listening.orb.voice.level = 0.55;
    let calling = with_calls(&talking, true);
    let mut walking = talking.clone();
    walking.vrchat.world.in_world = true;
    walking.settings.walk_boost.enabled = true;
    walking.settings.walk_boost.strength = 0.35;
    // (name, snapshot, calibration: (T-pose held, started at this time; the last frame is at 1.48 s)).
    type Shot<'a> = (&'a str, &'a Snapshot, Option<(bool, f64)>);
    let shots: [Shot; 9] = [
        ("overlay-orb-call", &calling, None),
        ("overlay-walk", &walking, None),
        ("overlay-orb", &talking, None),
        ("overlay-orb-listening", &listening, None),
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
        if name.starts_with("overlay-orb") {
            ui_state.page = crate::overlay::ui::Page::Orb;
        }
        if name == "overlay-walk" {
            ui_state.page = crate::overlay::ui::Page::Walk;
        }
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

/// A member with calls on, three friends who can be called and one calling.
fn with_calls(snap: &Snapshot, ringing: bool) -> Snapshot {
    use crate::state::{CallContact, CallLink, CallPhase, CallView};
    let mut s = snap.clone();
    s.settings.orb.calls = true;
    s.orb.calls.link = CallLink::Linked { account: "usr_1339e525-d75d-4832-9104-98dd89287681".into() };
    s.orb.calls.contacts =
        ["Kiri", "Nova Fox", "Rexouium Fan"].iter().enumerate().map(|(i, n)| CallContact { id: format!("c_{i}"), name: n.to_string() }).collect();
    if ringing {
        s.orb.calls.call = Some(CallView {
            name: "Nova Fox".into(),
            phase: CallPhase::Ringing,
            incoming: true,
            connected_at_ms: None,
            muted: false,
            mic_level: 0.0,
            remote_level: 0.0,
        });
    }
    s
}
