//! `orbiters-desktop --screenshot <out folder> [--sessions <folder>] [--raw <folder>] [--scale <factor>] [--only <name>]
//! [--height <points>] [--width <points>] [--extend <frames>] [--sequence <frames>]`
//! renders the interface offscreen in several states and writes one PNG per state, for visual review without a
//! window. The controller is not started: the snapshots are made up, the session library is real. `--only e2e`
//! instead records a real test pattern through the worker, driven by synthetic clicks, with avatar parameters
//! from a stand-in VRChat (OSC on the loopback interface, current values from its OSCQuery tree). The end-to-end
//! runs click controls found by their labels where the last frame drew them, and wait on what the app shows or the
//! controller says (each wait with a time limit), never on fixed coordinates or delays. `--extend` runs each scenario that
//! many frames past its shot and `--sequence` writes its last frames too (`<name>-<frame>.png`), to check a motion frame
//! by frame (a window closing back into its card).
//!
//! Startup data that may be cleaned or rewritten lives in private settings/cache/local-data folders, seeded with
//! small copies. Capture libraries and VRChat pictures are read in place; thumbnail-cache writes are disabled by
//! the harness bootstrap. Explicit end-to-end capture operations are separate from the default scenario set.
//! The harness once refreshed the VRChat pictures'
//! index in the person's cache with the harness's settings, which name no picture folder, so every picture of the
//! folders the person added (2 TB on Enzo's archive disk) left the index, and the app read them all again.

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
use orbiters_n4n0_core::menu::Page as MenuPage;
use orbiters_ui::offscreen::{save_png, Offscreen};
use orbiters_ui::{toast, widgets};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

mod floating;
mod orbit;
mod gallery_layouts;
mod gallery_folders;
mod gallery_assets;
mod gallery_talk;
mod projects_panel;
mod editor;
mod splat_export;
mod splats_select;
mod capture_actions;
mod pause_resume;
mod language_live;
mod vrchat_login;
mod launcher;
mod launcher_real;
mod vr_audio;
mod vrchat_bridge;
mod worlds_overlay;

/// What the interface shows of a VRChat on the network, as a Mac sees it.
fn network_snapshot(base: &crate::state::Snapshot, link: crate::state::LanLink) -> crate::state::Snapshot {
    use orbiters_core::lan::FoundVrchat;
    use std::net::Ipv4Addr;
    let quest = FoundVrchat { name: "VRChat-Client-A1B2C3".into(), device: Some("Quest-3".into()), ip: Ipv4Addr::new(192, 168, 1, 23), query_port: 49152, osc_port: Some(9000) };
    let pc = FoundVrchat { name: "VRChat-Client-D4E5F6".into(), device: Some("DESKTOP-4F2K".into()), ip: Ipv4Addr::new(192, 168, 1, 40), query_port: 51120, osc_port: None };
    let mut snapshot = base.clone();
    let connected = link == crate::state::LanLink::Connected;
    *snapshot.lan = crate::state::LanStatus {
        link,
        found: match link {
            crate::state::LanLink::Looking | crate::state::LanLink::NotAllowed => Vec::new(),
            crate::state::LanLink::Connected => vec![quest.clone()],
            _ => vec![quest.clone(), pc],
        },
        peer: connected.then_some(quest.ip),
        peer_label: connected.then(|| quest.label()),
        this_computer: Some(Ipv4Addr::new(192, 168, 1, 134)),
    };
    snapshot.vrchat.running = connected;
    snapshot.vrchat.remote = connected;
    snapshot.vrchat.world = Default::default();
    snapshot
}

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
    /// The Sessions page's storage popover, or a session's detail picked by what it is.
    library: Option<LibraryShot>,
}

/// On the Sessions page: the storage popover (its overview, or its confirmation with the captures without a 3D view
/// chosen too), or the detail of a reconstructed session that still has its capture files (with "Keep only the 3D view"
/// asked), or of one reduced to its 3D view.
#[derive(Clone, Copy)]
enum LibraryShot {
    Storage { confirming: bool },
    Trim { asking: bool },
    ViewOnly,
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

/// Main copied mutable startup inputs into private roots before panic logging or any Controller/App. Large
/// capture libraries are read in place, optionally from ORBITERS_HARNESS_FIXTURES, through screenshot Settings.
fn sandbox_user_data() -> anyhow::Result<(PathBuf, Vec<PathBuf>)> {
    let roots = orbiters_core::paths::roots().context("screenshots require the harness bootstrap")?;
    let root = roots.config.parent().context("private root")?.to_path_buf();
    let folders: Vec<PathBuf> = std::env::var("ORBITERS_SCREENSHOT_PICTURE_FOLDERS")
        .unwrap_or_default().split(';').filter(|s| !s.trim().is_empty()).map(|s| PathBuf::from(s.trim())).collect();
    if !folders.is_empty() {
        Settings { picture_folders: folders.clone(), ..Settings::default() }.save_to(&roots.config.join("settings.json"))?;
    }
    Ok((root, folders))
}

/// Writes where text no longer fits when the run ends, however it ends.
struct TextReport(PathBuf);

impl Drop for TextReport {
    fn drop(&mut self) {
        if let Err(e) = std::fs::write(&self.0, orbiters_ui::text_audit::report()) {
            eprintln!("{}: {e}", self.0.display());
        }
    }
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let (sandbox, picture_folders) = sandbox_user_data()?;
    eprintln!("screenshots: settings and caches in {}", sandbox.display());
    shots(args, picture_folders)
}

fn shots(args: &[String], picture_folders: Vec<PathBuf>) -> anyhow::Result<()> {
    let out =
        PathBuf::from(args.first().context("usage: --screenshot <out folder> [--sessions <folder>] [--raw <folder>] [--scale <factor>] [--only <name>]")?);
    let (sessions, raw) = crate::harness::screenshot_library().context("screenshots require library inputs from the harness bootstrap")?;
    let mut settings = Settings { picture_folders, sessions_dir: sessions.clone(), raw_captures_dir: raw.clone(), ..Settings::default() };
    // The usage statistics' notice was closed: it shows in its own scene ("usage-notice") alone.
    settings.usage.told = true;
    settings.usage.told_perf = true;
    let mut scale = 1.0f32;
    let mut height = 820.0f32;
    let mut width = 1280.0f32;
    let mut only: Option<String> = None;
    let mut frames = Frames::default();
    let mut text_report = None;
    crate::audio_devices::made_up_mics();
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sessions" => settings.sessions_dir = PathBuf::from(it.next().context("--sessions <folder>")?),
            "--raw" => settings.raw_captures_dir = PathBuf::from(it.next().context("--raw <folder>")?),
            "--scale" => scale = it.next().context("--scale <factor>")?.parse()?,
            "--only" => only = Some(it.next().context("--only <name>")?.clone()),
            "--height" => height = it.next().context("--height <points>")?.parse()?,
            "--width" => width = it.next().context("--width <points>")?.parse()?,
            "--extend" => frames.extend = it.next().context("--extend <frames>")?.parse()?,
            "--sequence" => frames.sequence = it.next().context("--sequence <frames>")?.parse()?,
            // In any language, whatever is chosen; and where its text no longer fits.
            "--language" => {
                let tag = it.next().context("--language <en, ja, fr, es or pt-BR>")?;
                let language = orbiters_i18n::normalize(tag).with_context(|| format!("{tag} is not a language of the website"))?;
                orbiters_i18n::pin_language(language);
                orbiters_ui::fonts::prefer_language(language.tag());
            }
            "--text-report" => {
                text_report = Some(TextReport(PathBuf::from(it.next().context("--text-report <file>")?)));
                orbiters_ui::text_audit::turn_on();
            }
            other => anyhow::bail!("unknown option {other}"),
        }
    }
    let _text_report = text_report;
    settings.consent_acknowledged = true;
    std::fs::create_dir_all(&out)?;
    if only.as_deref() == Some("e2e") {
        return e2e(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("splats-3d") {
        return splats_3d(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("splats-select") {
        return splats_select::shots(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("capture-actions-select") {
        return capture_actions::select(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("capture-actions-tint") {
        return capture_actions::tint(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("capture-wearer") {
        return capture_actions::wearer(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("language-live") {
        return language_live::walk(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("pause-resume") {
        return pause_resume::walk(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("capture-actions") {
        return capture_actions::walk(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("splat-export") {
        return splat_export::shots(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("splat-export-cpu") {
        return splat_export::shots_cpu(&out, &settings, scale);
    }
    if only.as_deref() == Some("editor-3d") {
        return editor::three_d(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("editor-3d-live") {
        return editor::three_d_live(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("edit3d-depth") {
        return editor::three_d_depth(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("e2e-cleanup") {
        return e2e_cleanup(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("e2e-reconstruct") {
        return e2e_reconstruct(&mut Offscreen::new()?, &out, &settings, scale);
    }
    if only.as_deref() == Some("overlay") {
        return overlay_shots(&out, &settings);
    }
    if only.as_deref() == Some("orbit-behaviour") {
        return orbit::behaviour_shots(&out);
    }
    if only.as_deref() == Some("orbit") {
        return orbit::shots(&out);
    }
    if only.as_deref() == Some("orbit-camera") {
        return orbit::camera_shots(&out);
    }
    if let Some(refs) = only.as_deref().and_then(|o| o.strip_prefix("orbit-vr=")) {
        return orbit::vr_shots(&out, std::path::Path::new(refs));
    }
    if only.as_deref() == Some("floating") {
        return floating::shots(&out, &settings);
    }
    #[cfg(windows)]
    if only.as_deref() == Some("mismatch") {
        return orbit::mismatch_shots(&out);
    }
    if only.as_deref() == Some("asktoggle") {
        orbit::asktoggle_shots(&out)?;
        return crate::overlay::menu_pages::asktoggle_pictures(&out);
    }
    if only.as_deref() == Some("worlds-overlay") {
        return worlds_overlay::shots(&out);
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
    // Frames coming, none with the strip, as with Local User off on an upload that could not hide itself: the tile says why.
    // The two 3D pictures under way, the avatar hidden or not, and what stays on the page after the second.
    let mut picture_world = recording.clone();
    picture_world.capture.photo_3d = true;
    picture_world.capture.picture = Some(orbiters_core::settings::PictureMode::World);
    picture_world.capture.coverage = None;
    picture_world.capture.left_out = crate::state::LeftOut { wearer: orbiters_core::picture::Left::Out, others: orbiters_core::picture::Left::Out, why: None };
    let mut picture_people = picture_world.clone();
    picture_people.capture.picture = Some(orbiters_core::settings::PictureMode::People);
    picture_people.capture.left_out = Default::default();
    let cannot: &'static str = Box::leak(crate::controller::hide_self::cannot_hide_text(Some("The 1MB Ultirex"), Some("Orbiters Toolkit 0.4.11")).into_boxed_str());
    let mut picture_world_shows = picture_world.clone();
    picture_world_shows.capture.left_out = crate::state::LeftOut { wearer: orbiters_core::picture::Left::InPicture, others: orbiters_core::picture::Left::Out, why: Some(cannot.to_string()) };
    let mut idle_after = idle.clone();
    idle_after.capture.left_out = picture_world_shows.capture.left_out.clone();
    // The worn avatar in a project the fixture has, and in none: Update the Toolkit, and its polite no.
    let mut toolkit_project = idle_after.clone();
    toolkit_project.n4n0.avatar_id = Some("avtr_00000000-0000-4000-8000-000000000001".into());
    let mut toolkit_elsewhere = idle_after.clone();
    toolkit_elsewhere.n4n0.avatar_id = Some("avtr_4f3b5bc4-0000-4000-8000-000000000000".into());
    // Several VRChats on this computer (controller/vrchats.rs): one; his own beside the world tour's; the tour's chosen; his
    // chosen with its camera closed.
    let vrchats = |target: u32, chosen: bool, own_camera: bool, tour: crate::state::VrchatDriverShown| {
        use crate::state::{VrchatDriverShown, VrchatRunShown};
        let mut s = idle.clone();
        let own = VrchatRunShown { pid: 37076, profile: 0, account: Some("blackorbit".into()), world: Some("FRONTLINE KIZUNA".into()), desktop: Some(true), camera: Some(own_camera), driver: VrchatDriverShown::Person, target: target == 37076, chosen: false };
        let tour = VrchatRunShown { pid: 8244, profile: 1, account: Some("Orbiters".into()), world: Some("Audience Anarchy".into()), desktop: Some(true), camera: Some(!own_camera || target == 8244), driver: tour, target: target == 8244, chosen };
        s.vrchat.runs = vec![tour, own];
        let world = if target == 8244 { ("wrld_81800175-2fdb-4e77-b602-c7fa35f33c0c", "Audience Anarchy") } else { ("wrld_a51861e9-85a8-43da-85a3-854a184b2fc1", "FRONTLINE KIZUNA") };
        s.vrchat.world.world_id = Some(world.0.into());
        s.vrchat.world.world_name = Some(world.1.into());
        s
    };
    let mut vrchats_one = idle.clone();
    vrchats_one.vrchat.runs = vrchats(37076, false, true, crate::state::VrchatDriverShown::Left).vrchat.runs.into_iter().filter(|r| r.pid == 37076).collect();
    let vrchats_own = vrchats(37076, false, true, crate::state::VrchatDriverShown::Left);
    let vrchats_agent = vrchats(8244, true, false, crate::state::VrchatDriverShown::Agent { session: "p1-3f2a…".into() });
    let mut vrchats_closed = vrchats(37076, false, false, crate::state::VrchatDriverShown::Agent { session: "p1-3f2a…".into() });
    *vrchats_closed.sources = Vec::new();
    let mut no_strip = recording.clone();
    no_strip.capture.trail.clear();
    no_strip.capture.coverage = None;
    no_strip.capture.no_strip = Some(crate::state::NoStrip::MaskOff);
    no_strip.capture.stats = Some(CaptureStats {
        frames: 419,
        strip_ok: 0,
        strip_failed: 419,
        fps: 44.0,
        elapsed_s: 9.5,
        width: 1920,
        height: 1080,
        strip_missing_s: 9.5,
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
        integrated: true,
        ..crate::state::ReconstructionJob::new(s.dir.clone(), true)
    });
    let job = reconstructing.reconstruction.clone();
    // Paused by a capture, with what it did kept: what the worker's checkpoint leaves on show. What the real worker leaves
    // is `--only pause-resume`.
    let mut paused = reconstructing.clone();
    paused.capture.state = crate::state::CaptureState::Recording;
    if let Some(j) = paused.reconstruction.as_mut() {
        j.integrated = false;
        j.floor = crate::pages::sessions::overall_progress(j);
        j.paused = true;
    }
    let waiting = |base: &Snapshot| {
        let mut s = base.clone();
        s.reconstruction = sessions.get(1).zip(job.clone()).map(|(r, j)| crate::state::ReconstructionJob { session_dir: r.dir.clone(), ..j });
        s.reconstruct_queue = sessions.iter().enumerate().filter(|(i, _)| *i != 1).map(|(_, r)| r.dir.clone()).collect();
        s
    };

    // Visited worlds: the world with the most captures has a 3D preview shared; in the one with the most pictures, the
    // first two count as shared and the next two as being sent.
    // The pictures' index is the run's own ([`sandbox_user_data`]: VRChat's own picture folder only, unless
    // `ORBITERS_SCREENSHOT_PICTURE_FOLDERS` names more); `ORBITERS_SCREENSHOT_NO_PICTURES` leaves it out altogether,
    // for scenarios that do not show pictures when VRChat's folder is a large one.
    let visited = if std::env::var_os("ORBITERS_SCREENSHOT_NO_PICTURES").is_some() { Vec::new() } else { crate::worlds::load(&settings.sessions_dir) };
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

    // Freeing space: three reconstructed captures keeping only their 3D views, the first under way.
    let freeing = {
        let mut s = idle.clone();
        let viewed: Vec<PathBuf> = sessions.iter().filter(|x| x.reconstruction.is_some() && !x.trimmed).map(|x| x.dir.clone()).take(3).collect();
        let total: u64 = sessions.iter().filter(|x| viewed.contains(&x.dir)).map(|x| x.capture_bytes).sum();
        s.cleanup = Some(crate::state::CleanupJob {
            current: viewed.first().map(|d| (d.clone(), false)),
            trim: viewed.iter().skip(1).cloned().collect(),
            delete: Vec::new(),
            freed: total * 2 / 5,
            total,
        });
        s
    };
    let n4n0_busy = n4n0_snapshot(&signed_in);
    let n4n0_new = {
        let mut s = with_account(&idle, crate::state::AccountState::SignedOut, None);
        s.vrchat.world.avatar_name = Some("Rexouium".into());
        s.n4n0.avatar_id = Some("avtr_7f3c2b1e-0d4a-4a7b-9c55-1f0e9a2b6c11".into());
        s
    };

    // Home: N4N0's menu with what the service knows (friends to call, the light on, fast walk on, the avatar's saved
    // states).
    let home = {
        let mut s = with_calls(&n4n0_busy, false);
        s.n4n0.light = Some(crate::state::N4n0Light { on: true, color: 1, level: 0.6 });
        s.settings.walk_boost.enabled = true;
        s.n4n0.states = sample_states(s.n4n0.avatar_id.clone());
        s.n4n0.tracking = Some(orbiters_n4n0::status::TrackingView { on: true, ms: [0, 35, 78], note: "Smoothing your devices now".into() });
        s.n4n0.versions = "N4N0 1.0.9 · App 0.4.0".into();
        s.n4n0.world_video = Some(("https://www.youtube.com/watch?v=36YnV9STBqc".into(), Some("Lo-fi beats to relax to".into()), None));
        s
    };
    // The same, its N4N0's screen of version 5: a copied link can be dropped as a card.
    let home_drop = {
        let mut s = home.clone();
        s.n4n0.can_drop = true;
        s
    };
    // The same with a link card out in the world (Take back).
    let home_card = {
        let mut s = home_drop.clone();
        s.n4n0.card = Some(orbiters_n4n0::status::CardView {
            title: "1 A.M Study Session 📚 [lofi hip hop]".into(),
            link: "https://www.youtube.com/watch?v=sample00003".into(),
            left: 452,
        });
        s
    };
    // Home as many first see it: VRChat in desktop mode (no fast walk), calls off, no light on this N4N0.
    let home_plain = {
        let mut s = idle.clone();
        s.n4n0.desktop_mode = true;
        s.settings.n4n0.wake_word = true;
        s
    };
    let home_listening = {
        let mut s = home.clone();
        s.n4n0.listening = true;
        s.n4n0.panel = Some("Fast walk is on. Say what you want, or tap a tile.".into());
        s
    };

    let mut scenarios = vec![
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
            library: None,
        },
        Scenario {
            name: "network-looking",
            page: Page::Home,
            snapshot: network_snapshot(&home, crate::state::LanLink::Looking),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "network-not-allowed",
            page: Page::Home,
            snapshot: network_snapshot(&home, crate::state::LanLink::NotAllowed),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "network-several",
            page: Page::Home,
            snapshot: network_snapshot(&home, crate::state::LanLink::Choose),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "network-connected",
            page: Page::Home,
            snapshot: network_snapshot(&home, crate::state::LanLink::Connected),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "network-settings",
            page: Page::Settings,
            snapshot: network_snapshot(&home, crate::state::LanLink::Connected),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
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
            library: None,
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
            library: None,
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
            library: None,
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
            library: None,
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
            library: None,
        },
        Scenario {
            // N4N0-11: the wake word heard during a call puts "Talk with N4N0?" up, half its time gone.
            name: "wake-offer",
            page: Page::Home,
            snapshot: with_wake_offer(&with_calls(&home, false)),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
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
            library: None,
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
            library: None,
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
            library: None,
        },
        // A browser plays a YouTube video (found there): Home's music card copies its link, the music page offers Drop.
        Scenario {
            name: "home-playing-youtube",
            page: Page::Home,
            snapshot: home_drop.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Home, samples: true }),
            library: None,
        },
        Scenario {
            name: "home-music-youtube",
            page: Page::Home,
            snapshot: home_drop.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Music, samples: true }),
            library: None,
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
            library: None,
        },
        Scenario {
            name: "update-ready",
            page: Page::Home,
            snapshot: {
                let mut s = home.clone();
                s.settings.development = true;
                s.update.enabled = true;
                s.update.state = crate::state::UpdateState::Ready { version: "0.3.2-dev.2".into(), notes: "Development build 0.3.2-dev.2".into() };
                s
            },
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 200.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "home-qr",
            page: Page::Home,
            snapshot: {
                let mut s = home.clone();
                s.n4n0.qr = crate::overlay::menu_pages::sample_qr();
                s
            },
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Qr, samples: true }),
            library: None,
        },
        Scenario {
            name: "home-instances",
            page: Page::Home,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::AddFriend, samples: true }),
            library: None,
        },
        Scenario {
            name: "home-qr-no-camera",
            page: Page::Home,
            snapshot: {
                let mut s = home.clone();
                s.n4n0.qr = orbiters_n4n0_core::qr::QrView {
                    phase: orbiters_n4n0_core::qr::QrPhase::NoCamera,
                    note: orbiters_n4n0_core::qr::no_camera_note(Some(false)),
                    ..Default::default()
                };
                s
            },
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Qr, samples: true }),
            library: None,
        },
        Scenario {
            name: "home-tracking",
            page: Page::Home,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Tracking, samples: true }),
            library: None,
        },
        Scenario {
            name: "home-suggest",
            page: Page::Home,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Suggest, samples: true }),
            library: None,
        },
        Scenario {
            name: "home-results-drop",
            page: Page::Home,
            snapshot: home_drop.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Results, samples: true }),
            library: None,
        },
        Scenario {
            name: "home-states",
            page: Page::Home,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::States, samples: true }),
            library: None,
        },
        Scenario {
            name: "home-states-avatar",
            page: Page::Home,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::States, samples: true }),
            library: None,
        },
        Scenario {
            name: "home-states-other",
            page: Page::Home,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::States, samples: true }),
            library: None,
        },
        Scenario {
            name: "home-photo-camera",
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
            library: None,
        },
        Scenario {
            name: "home-music-card",
            page: Page::Home,
            snapshot: home_card.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Music, samples: true }),
            library: None,
        },
        Scenario {
            name: "home-results-card",
            page: Page::Home,
            snapshot: home_card.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::Results, samples: true }),
            library: None,
        },
        Scenario {
            name: "home-plain-states",
            page: Page::Home,
            snapshot: home_plain.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: Some(HomeShot { page: MenuPage::States, samples: false }),
            library: None,
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
            library: None,
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
            library: None,
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
            library: None,
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
            library: None,
        },
        Scenario {
            name: "n4n0-calls",
            page: Page::N4n0,
            snapshot: with_calls(&n4n0_busy, true),
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 1060.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "n4n0",
            page: Page::N4n0,
            snapshot: n4n0_busy.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        },
        // On your avatar: the panel, the menu, the AFK message, Keep my pose and Controller input (an avatar using four).
        Scenario {
            name: "n4n0-avatar",
            page: Page::N4n0,
            snapshot: {
                let mut s = n4n0_busy.clone();
                s.n4n0.input_params = 4;
                s
            },
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 560.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "n4n0-testing",
            page: Page::N4n0,
            snapshot: {
                let mut s = n4n0_busy.clone();
                s.n4n0.test_face = Some(crate::state::N4n0Face::Happy);
                s.n4n0.test_afk = true;
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
            library: None,
        },
        Scenario {
            name: "n4n0-scrolled",
            page: Page::N4n0,
            snapshot: n4n0_busy.clone(),
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 760.0,
            world: None,
            home: None,
            library: None,
        },
        // N4N0's menu settings: its keyboard and look.
        Scenario {
            name: "n4n0-menu-settings",
            page: Page::N4n0,
            snapshot: n4n0_busy,
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 1180.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "n4n0-new",
            page: Page::N4n0,
            snapshot: n4n0_new,
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
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
            library: None,
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
            library: None,
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
            library: None,
        },
        Scenario {
            name: "capture-picture-people",
            page: Page::Capture,
            snapshot: picture_people,
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "capture-picture-world",
            page: Page::Capture,
            snapshot: picture_world,
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "capture-picture-world-shows",
            page: Page::Capture,
            snapshot: picture_world_shows,
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![(toast::Kind::Info, cannot)],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "capture-toolkit-project",
            page: Page::Capture,
            snapshot: toolkit_project,
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "capture-toolkit-elsewhere",
            page: Page::Capture,
            snapshot: toolkit_elsewhere,
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "capture-idle-after",
            page: Page::Capture,
            snapshot: idle_after,
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "capture-no-strip",
            page: Page::Capture,
            snapshot: no_strip,
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![(toast::Kind::Warning, crate::state::NoStrip::MaskOff.message())],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
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
            library: None,
        },
        Scenario {
            name: "capture-vrchats-one",
            page: Page::Capture,
            snapshot: vrchats_one,
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "capture-vrchats-own",
            page: Page::Capture,
            snapshot: vrchats_own,
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "capture-vrchats-agent",
            page: Page::Capture,
            snapshot: vrchats_agent,
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "capture-vrchats-closed",
            page: Page::Capture,
            snapshot: vrchats_closed,
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![(toast::Kind::Error, Box::leak(orbiters_i18n::tr!("Open the camera in {who}'s VRChat first", who = "blackorbit").into_boxed_str()))],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
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
            library: None,
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
            library: None,
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
            library: None,
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
            library: None,
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
            library: None,
        },
        Scenario {
            name: "session-reconstruction-paused",
            page: Page::Sessions,
            snapshot: paused,
            frames: 150,
            splash: false,
            open_detail: true,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
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
            library: None,
        },
        Scenario {
            name: "settings-booster",
            page: Page::Settings,
            // The figures are sample data for the picture, not measured.
            snapshot: {
                use orbiters_booster::{engine, trial};
                let mut s = idle.clone();
                s.steamvr.running = true;
                s.settings.booster.level = orbiters_booster::Level::Background;
                s.booster.watching = true;
                s.booster.overlay.streamer = true;
                let report = &mut s.booster.overlay.report;
                report.line = engine::Line::Kept;
                report.limit = orbiters_booster::classify::Limit::Processor;
                report.comparisons = vec![trial::Comparison {
                    kind: trial::Kind::Programs,
                    verdict: trial::Verdict::Kept,
                    before_fps: 36.0,
                    after_fps: 41.0,
                    before_filled: 0.49,
                    after_filled: 0.38,
                    before_over_2x: 0.0,
                    after_over_2x: 0.0,
                    before_late_cpu: 0.4,
                    after_late_cpu: 0.3,
                    before_late_gpu: 0.0,
                    after_late_gpu: 0.0,
                    stalls_at_switch: 0,
                    kept: true,
                }];
                s
            },
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "settings-translate",
            page: Page::Settings,
            // Live translation on, with an AI app found on this computer. Sample figures, not measured.
            snapshot: {
                let mut s = idle.clone();
                s.steamvr.running = true;
                s.settings.translate.enabled = true;
                s.translate.set(crate::state::TranslateStatus {
                    state: crate::state::TranslateState::Looking,
                    shown: 3,
                    lines: 5,
                    look_ms: Some(214),
                    server: Some("http://127.0.0.1:1234/v1".into()),
                    models: vec!["qwen3.5-2b".into(), "qwen3-8b".into(), "translategemma-4b-it".into()],
                    waited_s: None,
                    // The advice a too small model gets.
                    small_model: Some("qwen3.5-2b".into()),
                });
                s
            },
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 760.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "settings-perf-report",
            page: Page::Settings,
            // N-friend-lag: a Performance report made and shown before sending, and the self-check's hint. Sample lines.
            snapshot: {
                let mut s = idle.clone();
                let sample = serde_json::json!({
                    "machine": { "cpu": "Intel(R) Core(TM) i5-7400 CPU @ 3.00GHz", "physical_cores": 4, "logical_processors": 4, "core_kinds": [], "gpus": [{"name": "NVIDIA GeForce GTX 1060 6GB", "dedicated_mb": 6144.0, "driver": "32.0.15.6094"}], "ram_total_mb": 16384.0, "ram_free_mb": 3900.0, "displays": [{"width": 1920, "height": 1080, "hz": 60}] },
                    "windows": { "os": "Windows 10 Home 10.0.19045", "antivirus": ["Windows Defender"] },
                    "steamvr": { "drivers": ["orbiters_smooth"] }, "programs_running": ["OVR Toolkit", "Discord"], "features": ["steamvr_overlay", "panels_wheel", "panels_quick_menu", "watch"],
                    "hint": "reconstruct_gpu", "lines": [],
                });
                s.perf.set(crate::perf::PerfView {
                    hint: Some(crate::perf::Hint { key: "reconstruct_gpu".into(), part: None }),
                    report: Some(crate::perf::ReportView { path: Some("perf-report-20261116044800.json".into()), summary: crate::perf::report::summary(&sample), ..Default::default() }),
                });
                s
            },
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 1250.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "settings-translate-orbiters",
            page: Page::Settings,
            // Orbiters chosen, its notice not yet accepted.
            snapshot: {
                let mut s = idle.clone();
                s.steamvr.running = true;
                s.settings.translate.enabled = true;
                s.settings.translate.provider = orbiters_core::settings::TranslateProvider::Orbiters;
                s.translate.set(crate::state::TranslateStatus { state: crate::state::TranslateState::NeedsAgreement, ..Default::default() });
                s
            },
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 760.0,
            world: None,
            home: None,
            library: None,
        },
        // The beta's one notice about its usage statistics, over Home, until it is closed.
        Scenario {
            name: "usage-notice",
            page: Page::Home,
            snapshot: { let mut s = idle.clone(); s.settings.usage.told = false; s },
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
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
            library: None,
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
            library: None,
        },
        Scenario {
            name: "settings-signing-in",
            page: Page::Settings,
            snapshot: with_account(&idle, crate::state::AccountState::SigningIn, None),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
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
            library: None,
        },
        Scenario {
            name: "settings-walk",
            page: Page::Settings,
            snapshot: {
                let mut s = idle.clone();
                s.settings.walk_boost.enabled = true;
                s.steamvr.walk_path = Some(crate::state::WalkPath::PlaySpace(crate::state::WhyPlaySpace::OldDriver));
                s.settings.world_performance = true;
                s
            },
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 3450.0,
            world: None,
            home: None,
            library: None,
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
            library: None,
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
            library: None,
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
            library: None,
        },
        // The world's page flying back into its card (closed on frame 120; `--extend`/`--sequence` show it land).
        Scenario {
            name: "world-close",
            page: Page::Worlds,
            snapshot: signed_in.clone(),
            frames: 140,
            splash: false,
            open_detail: true,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: Some(WorldShot { maps: Some(maps.clone()), ..Default::default() }),
            home: None,
            library: None,
        },
        // A picture shown large shrinking back into its tile (closed on frame 170).
        Scenario {
            name: "world-viewer-close",
            page: Page::Worlds,
            snapshot: signed_in.clone(),
            frames: 200,
            splash: false,
            open_detail: true,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: Some(WorldShot { pictures: true, view: Some(1), maps: Some(maps.clone()), ..Default::default() }),
            home: None,
            library: None,
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
            library: None,
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
            library: None,
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
            library: None,
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
            library: None,
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
            library: None,
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
            library: None,
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
            library: None,
        },
        Scenario {
            name: "sessions-storage",
            page: Page::Sessions,
            snapshot: idle.clone(),
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: Some(LibraryShot::Storage { confirming: false }),
        },
        Scenario {
            name: "sessions-storage-confirm",
            page: Page::Sessions,
            snapshot: idle.clone(),
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: Some(LibraryShot::Storage { confirming: true }),
        },
        // The service deleting: how far it got, in the popover and on the cards.
        Scenario {
            name: "sessions-freeing",
            page: Page::Sessions,
            snapshot: freeing,
            frames: 120,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: Some(LibraryShot::Storage { confirming: false }),
        },
        Scenario {
            name: "session-trim",
            page: Page::Sessions,
            snapshot: idle.clone(),
            frames: 150,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: Some(LibraryShot::Trim { asking: false }),
        },
        Scenario {
            name: "session-trim-asking",
            page: Page::Sessions,
            snapshot: idle.clone(),
            frames: 180,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: Some(LibraryShot::Trim { asking: true }),
        },
        Scenario {
            name: "session-view-only",
            page: Page::Sessions,
            snapshot: idle.clone(),
            frames: 150,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: Some(LibraryShot::ViewOnly),
        },
        // The videos the worlds played ([`stage_videos`]): the history by day, a bookmark list, the bookmark picker.
        Scenario {
            name: "videos",
            page: Page::Videos,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "videos-scrolled",
            page: Page::Videos,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 260.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "videos-list",
            page: Page::Videos,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        },
        Scenario {
            name: "videos-picker",
            page: Page::Videos,
            snapshot: home.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
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
            library: None,
        },
    ];

    // Unity projects and Capture's N4N0 card: every state, the dialogs, and frames in the middle of their motion
    // (`-mid`: a few frames after the press that starts it). The panel docks on the default window; `--width 1040`
    // shows the narrow layout (`projects-narrow-detail`, `projects-sheet-*`).
    for name in [
        "projects-gallery",
        "projects-detail",
        "projects-packages",
        "projects-narrow-detail",
        "projects-loading",
        "projects-scanning",
        "projects-empty",
        "projects-panel-loading",
        "projects-error",
        "projects-busy",
        "projects-search",
        "projects-proposals",
        "projects-repos",
        "projects-repos-loading",
        "projects-repos-mid",
        "projects-repos-cards",
        "projects-repos-scrolled",
        "projects-repos-search",
        "projects-repos-search-japanese",
        "projects-repos-search-owner",
        "projects-repos-nothing",
        "projects-repos-many",
        "projects-repos-added",
        "projects-repos-unreachable",
        "projects-repos-partly",
        "projects-repos-bare",
        "projects-panel-cut",
        "projects-panel-cut-top",
        "projects-new",
        "projects-new-custom",
        "projects-new-duplicate",
        "projects-new-copying",
        "projects-new-mid",
        "projects-new-open",
        "projects-installing",
        "projects-setup",
        "projects-setup-closed",
        "projects-setup-done",
        "projects-setup-declined",
        "projects-setup-failed",
        "projects-installed",
        "projects-setup-from-panel",
        "projects-sheet-mid",
        "projects-sheet-open",
        "projects-switch-mid",
        "projects-press",
        "projects-hover",
        "projects-dropdown",
        "projects-menu",
        "projects-avatars",
        "projects-avatars-loading",
        "projects-avatars-empty",
        "projects-avatars-mid",
        "projects-avatars-open",
        "projects-choice",
        "projects-ready",
        "projects-capture",
        "projects-crossfade",
        "projects-resize",
        "projects-resize-mid",
        "projects-mixer",
        "projects-mixer-installed",
        "projects-mixer-adding",
        "projects-mixer-open-mid",
        "projects-mixer-open",
        "projects-mixer-fold-mid",
        "projects-new-covers",
        "projects-new-novcc",
        "projects-new-loading",
        "projects-repos-novcc",
        "projects-icons-live",
        "projects-repos-crossfade",
        "projects-repos-live",
        "projects-commits",
        "projects-commits-mid",
        "projects-commits-open",
        "projects-commits-narrow",
        "projects-world",
    ] {
        let capture = name.starts_with("projects-avatars") || matches!(name, "projects-choice" | "projects-ready" | "projects-capture");
        let mut snapshot = scenarios[0].snapshot.clone();
        snapshot.n4n0.avatar_id = Some("avtr_00000000-0000-4000-8000-000000000000".into());
        snapshot.n4n0.manifest = name == "projects-ready";
        snapshot.n4n0.prop_sync = (name == "projects-ready").then_some(8);
        // The development gallery, which has N4N0.
        snapshot.account.development = true;
        scenarios.push(Scenario {
            name,
            page: if capture { Page::Capture } else { Page::Projects },
            snapshot,
            frames: if matches!(name, "projects-icons-live" | "projects-repos-live") {
                // Real lookups: time for them to land.
                600
            } else if name == "projects-resize-mid" {
                60
            } else if name == "projects-commits-mid" {
                // The rows coming in one after the other.
                12
            } else if matches!(name, "projects-crossfade" | "projects-new-loading" | "projects-repos-crossfade") {
                66
            } else if name.ends_with("-mid") {
                67
            } else if name.ends_with("-open") || matches!(name, "projects-dropdown" | "projects-menu" | "projects-setup-from-panel") {
                100
            } else {
                90
            },
            splash: false,
            open_detail: false,
            toasts: vec![],
            // The wheel over the panel, not the cards beside it.
            pointer: (name == "projects-panel-cut-top").then_some(pos2(1050.0, 500.0)),
            scroll: match name {
                "projects-proposals" => 560.0,
                "projects-repos-scrolled" => 5000.0,
                "projects-repos-added" => 520.0,
                "projects-panel-cut-top" => 110.0,
                _ => 0.0,
            },
            world: None,
            home: None,
            library: None,
        });
    }
    // The selected project's 3D preview (a stand-in scene, or `ORBITERS_PREVIEW_FILE`, one Toolkit wrote): settled,
    // rising out of the ground, the camera swinging round, turned by a drag, its window growing out of the card and
    // open (with two scenes), close up and from behind in the open window (materials' look), in the scene's light and
    // switched to the studio's, a sheet's on narrow windows (`--width 1040`), and the notes of projects without one.
    for (name, frames) in [
        ("projects-preview", 200),
        ("projects-preview-reveal", 30),
        ("projects-preview-swing", 60),
        ("projects-preview-turned", 120),
        ("projects-preview-open-mid", 96),
        ("projects-preview-open", 230),
        ("projects-preview-close", 230),
        ("projects-preview-back", 230),
        ("projects-preview-scene-light", 230),
        ("projects-preview-studio-light", 230),
        ("projects-preview-scenes", 230),
        ("projects-preview-sheet", 200),
        ("projects-preview-none", 90),
        ("projects-preview-no-toolkit", 90),
        ("projects-preview-old-toolkit", 90),
        ("projects-preview-building", 90),
        ("projects-preview-no-scene", 90),
        ("projects-preview-native", 200),
        ("projects-preview-native-building", 90),
        ("projects-preview-native-open", 230),
        ("projects-preview-native-switch", 200),
        ("projects-preview-native-switched", 300),
        ("projects-preview-native-all", 200),
        ("projects-stills", 120),
    ] {
        let mut snapshot = scenarios[0].snapshot.clone();
        snapshot.account.development = true;
        scenarios.push(Scenario {
            name,
            page: Page::Projects,
            snapshot,
            frames,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        });
    }

    for name in [
        "gallery-mixed",
        "gallery-server-only",
        "gallery-mine",
        "gallery-shared",
        "gallery-search",
        "gallery-owner",
        "gallery-public",
        "gallery-grantee",
        "gallery-friends",
        "gallery-friends-error",
        "gallery-friends-search",
        "gallery-explicit",
        "gallery-error",
        "gallery-picture",
        "gallery-progress",
        "gallery-request",
        "gallery-instance",
        "gallery-instance-live",
        "gallery-instance-closed",
        "gallery-instance-error",
        "gallery-empty",
        "gallery-requests",
        "gallery-requests-explicit",
        "gallery-requests-approved",
        "gallery-requests-error",
        "gallery-requests-outgoing",
        "gallery-requests-denied",
        "gallery-requests-empty",
        "gallery-requests-opening",
        "gallery-requests-close",
        "gallery-instance-close",
        "gallery-done",
        "gallery-open",
        "gallery-open-mid",
        "gallery-close-mid",
        "gallery-next",
        "gallery-next-mid",
        "gallery-select",
        "gallery-select-mid",
        "gallery-drag-lift",
        "gallery-copied",
        "gallery-copy-link",
        "gallery-copy-link-offer",
        "gallery-edited",
        "gallery-updating",
        "gallery-update-replacing",
        "gallery-updated",
        "gallery-update-failed",
        "gallery-reconstructed",
        "gallery-again-shared",
        "world-share-window",
        "world-share-public",
        "world-share-sending",
        "world-share-mid",
        "world-share-open",
        "world-share-detail",
        "world-card-menu",
    ] {
        scenarios.push(Scenario {
            name,
            page: if name.starts_with("world-") { Page::Worlds } else { Page::Sessions },
            snapshot: scenarios[0].snapshot.clone(),
            frames: 140,
            splash: false,
            open_detail: name == "world-share-detail",
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: if name == "world-share-detail" { Some(WorldShot::default()) } else { None },
            home: None,
            library: None,
        });
    }
    // Events (the hero, the cards, the event's window and Invite me) and the communities' galleries in Photos & captures.
    for (name, frames) in [
        ("events", 140),
        ("events-enter-mid", 10),
        ("events-empty", 120),
        ("events-error", 120),
        ("events-search", 120),
        ("events-open", 150),
        ("events-open-mid", 66),
        ("events-open-card", 150),
        ("events-close-mid", 156),
        ("events-loading", 150),
        ("events-invite-busy", 150),
        ("events-invite-listed", 150),
        ("events-invite-friend", 150),
        ("events-invite-group", 150),
        ("events-invite-link", 150),
        ("events-invite-sent", 150),
        ("events-invite-n4n0", 150),
        ("events-pictures", 150),
        ("gallery-community", 140),
        ("gallery-community-all", 140),
        ("gallery-community-one", 140),
        ("gallery-community-open", 150),
        ("gallery-community-open-mid", 86),
        ("gallery-community-event", 150),
        ("gallery-community-event-mid", 86),
    ] {
        scenarios.push(Scenario {
            name,
            page: if name.starts_with("events") { Page::Events } else { Page::Sessions },
            snapshot: scenarios[0].snapshot.clone(),
            frames,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        });
    }
    vrchat_login::append(&mut scenarios);
    editor::append(&mut scenarios);
    projects_panel::append(&mut scenarios);
    gallery_layouts::append(&mut scenarios);
    gallery_assets::append(&mut scenarios);
    gallery_talk::append(&mut scenarios);
    floating::append(&mut scenarios);
    vr_audio::append(&mut scenarios);
    vrchat_bridge::append(&mut scenarios);
    launcher::append(&mut scenarios);
    launcher_real::append(&mut scenarios);
    // Last: they read a picture folder on another drive, and what comes after them can wait on a slow or failing one.
    gallery_folders::append(&mut scenarios);

    // Home's music is the overlay's sample song, never what this computer plays.
    crate::media::watcher().stand_in(crate::media::Reading {
        // Its own volume as Windows' mixer has it (Enzo, 2026-11-16: the card sets the app playing only).
        media: Some(orbiters_n4n0_core::menu::Media {
            sound: orbiters_n4n0_core::menu::AppSound::Level(orbiters_n4n0_core::menu::Volume { percent: 35, muted: false }),
            ..crate::overlay::menu_pages::sample_media()
        }),
        volume: Some(orbiters_n4n0_core::menu::Volume { percent: 40, muted: false }),
        art: Some(std::sync::Arc::new(crate::media::Art { key: crate::overlay::menu_pages::SAMPLE_ART.into(), rgb: crate::overlay::menu_pages::sample_art() })),
    });

    let mut gpu = Offscreen::new()?;
    for sc in scenarios {
        // `--only a,b,c`: several scenarios in one run (the scenarios are made once).
        if only.as_deref().is_some_and(|list| list.split(',').all(|o| {
            o != sc.name
                && !(o == "vrchat-login" && sc.name.starts_with("vrchat-login-"))
                && !(o == "editor" && sc.name.starts_with("editor-"))
                && !(o == "launcher" && sc.name.starts_with("launcher-") && !sc.name.starts_with("launcher-small"))
                && !(o == "sharing" && (sc.name.starts_with("gallery-") || sc.name.starts_with("world-share")))
                && !(o == "events" && (sc.name.starts_with("events") || sc.name.starts_with("gallery-community")))
        })) {
            continue;
        }
        // These read a real Unity project: a run with no scene named goes on without them.
        if only.is_none() && sc.name.starts_with("projects-preview-native") && std::env::var_os("ORBITERS_PREVIEW_PROJECT").is_none() {
            continue;
        }
        let path = out.join(format!("{}.png", sc.name));
        render(&mut gpu, &sc, scale, vec2(width, height), &path, frames)?;
        println!("{}", path.display());
    }
    Ok(())
}

/// Events and the communities' galleries on this frame of their scenario: the fixture staged, a window opened (out of
/// the hero, a card, a gallery picture or an event gallery's card) on the member's invites the name says.
fn stage_events(ctx: &egui::Context, app: &mut App, sc: &Scenario, f: u32) {
    use crate::pages::events::Ask;
    let dir = sc.snapshot.settings.sessions_dir.clone();
    if sc.page == Page::Events {
        if f == 0 {
            app.events_mut().stage(sc.name, &dir);
        }
        let open_at = match sc.name {
            "events-open-mid" => Some(sc.frames - 6),
            n if n.starts_with("events-open")
                || n.starts_with("events-invite")
                || n == "events-loading"
                || n == "events-close-mid"
                || n == "events-pictures" =>
            {
                Some(60)
            }
            _ => None,
        };
        if open_at == Some(f) {
            let friend = |is: bool| orbiters_core::community::Friend {
                is_friend: Some(is),
                accepting: false,
                account: Some(orbiters_core::community::InviteAccount {
                    name: Some("OrbitPhotoClub".into()),
                    vrchat_id: Some("usr_c0ffee00-0000-4000-8000-000000000001".into()),
                }),
            };
            let ask = match sc.name {
                "events-invite-busy" => Ask::Busy("Signing you up…"),
                "events-invite-listed" => Ask::Listed { friend: Some(friend(true)) },
                "events-invite-friend" => Ask::Listed { friend: Some(friend(false)) },
                "events-invite-group" => Ask::Group,
                "events-invite-link" => Ask::Link,
                "events-invite-sent" => Ask::Sent("Invite sent"),
                "events-invite-n4n0" => Ask::Befriend { name: "Orbiters".into(), link: "https://vrchat.com/home/user/usr_orbiters".into() },
                _ => Ask::Idle,
            };
            let n = usize::from(sc.name == "events-open-card");
            app.events_mut().open_card(ctx, n, &[("main", ask)], sc.name == "events-loading", sc.name == "events-pictures");
        }
        if sc.name == "events-close-mid" && f + 6 == sc.frames {
            app.events_mut().close_window();
        }
        return;
    }
    let gallery = app.sessions_mut().gallery_mut();
    if f == 1 || f == 46 {
        gallery.stage_community(sc.name, &dir);
    }
    let open_at = if sc.name.ends_with("-mid") { sc.frames - 6 } else { 80 };
    if f == open_at {
        if sc.name.starts_with("gallery-community-open") {
            let shown = gallery.shown().to_vec();
            gallery.community_mut().open_nth(ctx, 0, &shown);
        }
        if sc.name.starts_with("gallery-community-event") {
            gallery.community_mut().open_event(ctx);
        }
    }
}

/// Frames past a scenario's shot, and how many of the last frames are written each.
#[derive(Clone, Copy, Default)]
struct Frames {
    extend: u32,
    sequence: u32,
}

fn render(gpu: &mut Offscreen, sc: &Scenario, scale: f32, size: Vec2, path: &std::path::Path, run: Frames) -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    orbiters_ui::install(&ctx);
    let ctl = Controller::new(sc.snapshot.settings.clone());
    ctl.set_snapshot(sc.snapshot.clone());
    let backend: Ctl = std::sync::Arc::new(Local::new(ctl.clone()));
    let mut world_opened = false;
    let mut share_staged = false;
    let mut library_staged = false;
    // A press staged on the last frame, released on this one (with its button).
    let mut release: Option<Pos2> = None;
    let mut release_button = egui::PointerButton::Primary;
    backend.attach(&ctx);
    let mut app = App::offline(backend);
    if sc.name == "vrchat-login-entering-password" {
        app.stage_vrchat_login();
    }
    app.set_gpu(crate::gpu::Handles::of_offscreen(gpu));
    if !sc.splash {
        app.skip_splash();
    }
    let total = sc.frames + run.extend;
    for f in 0..total {
        let t = f as f64 / 60.0;
        let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), time: Some(t), ..Default::default() };
        input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(scale), focused: Some(true), ..Default::default() });
        if let Some(p) = sc.pointer {
            input.events.push(Event::PointerMoved(p));
        }
        // `ORBITERS_SCREENSHOT_SCROLL`: any scene, further down.
        let wanted = std::env::var("ORBITERS_SCREENSHOT_SCROLL").ok().and_then(|s| s.parse::<f32>().ok());
        if let (Some(scroll), true) = (wanted, f + 30 == sc.frames) {
            input.events.push(Event::PointerMoved(pos2(760.0, 500.0)));
            input.events.push(Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -scroll), phase: egui::TouchPhase::Move, modifiers: egui::Modifiers::NONE, source: egui::MouseWheelSource::Wheel });
        }
        if sc.scroll != 0.0 && f + 30 == sc.frames {
            input.events.push(Event::PointerMoved(sc.pointer.unwrap_or(pos2(760.0, 500.0))));
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
            if sc.name.starts_with("projects-") || sc.name.starts_with("capture-toolkit") {
                app.projects_mut().stage(&ctx, sc.name);
            }
            if sc.name.starts_with("projects-preview") || sc.name == "projects-stills" {
                app.projects_mut().stage_preview(sc.name);
            }
            if sc.page == Page::Videos {
                stage_videos(&ctx, &mut app, sc.name);
            }
        }
        launcher::stage(&ctx, &mut app, sc.name, f, sc.frames, t);
        launcher_real::stage(&ctx, &mut app, sc.name, f);
        // What the person does on this frame of a Unity projects scenario: a press where a control was drawn.
        if let Some(p) = release.take() {
            input.events.push(Event::PointerButton { pos: p, button: release_button, pressed: false, modifiers: egui::Modifiers::NONE });
            release_button = egui::PointerButton::Primary;
        }
        if sc.name.starts_with("projects-") && f > 0 {
            use crate::pages::projects::Poke;
            let poke = app.projects_mut().stage_frame(sc.name, f);
            match poke.or_else(|| app.projects_mut().stage_preview_frame(sc.name, f)) {
                Some(Poke::Press(p)) => {
                    input.events.push(Event::PointerMoved(p));
                    input.events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
                    release = Some(p);
                }
                Some(Poke::Hold(p)) => {
                    input.events.push(Event::PointerMoved(p));
                    input.events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
                }
                Some(Poke::Hover(p)) => input.events.push(Event::PointerMoved(p)),
                Some(Poke::Context(p)) => {
                    input.events.push(Event::PointerMoved(p));
                    input.events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: true, modifiers: egui::Modifiers::NONE });
                    release = Some(p);
                    release_button = egui::PointerButton::Secondary;
                }
                None => {}
            }
        }
        // A Visited worlds card's right-click menu.
        if sc.name == "world-card-menu" && f == 70 {
            if let Some(r) = app.worlds_mut().first_card() {
                let p = r.center();
                input.events.push(Event::PointerMoved(p));
                input.events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: true, modifiers: egui::Modifiers::NONE });
                release = Some(p);
                release_button = egui::PointerButton::Secondary;
            }
        }
        // Unity projects left for Settings and entered again (its pictures must stay drawn).
        if sc.name == "projects-revisit" && (f == 50 || f == 62) {
            app.set_page(if f == 50 { Page::Settings } else { Page::Projects }, t);
        }
        if sc.page == Page::Sessions && (f == 0 || f == 45) {
            app.sessions_mut().stage_gallery(&ctx, sc.name);
        }
        // A card growing into its window (a twelfth of a second in, or settled), the window landing back on its card,
        // the requests window growing out of its button.
        if sc.page == Page::Sessions {
            let gallery = app.sessions_mut().gallery_mut();
            match sc.name {
                "gallery-open" | "gallery-close-mid" | "gallery-next" | "gallery-next-mid" if f == 70 => gallery.open_card(&ctx, 0),
                "gallery-next" if f == 100 => gallery.step_window(1),
                "gallery-next-mid" if f + 6 == sc.frames => gallery.step_window(1),
                "gallery-open-mid" if f + 6 == sc.frames => gallery.open_card(&ctx, 0),
                "gallery-close-mid" if f + 6 == sc.frames => gallery.close_window(),
                "gallery-requests-opening" if f + 6 == sc.frames => gallery.open_requests(),
                "gallery-requests-close" if f == 60 => gallery.open_requests(),
                "gallery-requests-close" if f == 120 => gallery.close_requests(),
                "gallery-instance-close" if f == 120 => gallery.close_instance(),
                // Cards chosen (the bar rising out of the bottom edge, or settled), lifted as a drag out starts (its drag
                // image written beside the shot), the bar saying what a drag copied; Copy public link just pressed.
                "gallery-select" | "gallery-drag-lift" if f == 70 => gallery.stage_select(3),
                "gallery-select-mid" if f + 5 == sc.frames => gallery.stage_select(2),
                "gallery-drag-lift" if f == 120 => gallery.stage_lift(Some(&path.with_file_name("gallery-drag-image.png"))),
                "gallery-copied" if f == 70 => gallery.stage_select(2),
                "gallery-copied" if f == 100 => gallery.stage_copied(&ctx, "Copied 2 pictures"),
                "gallery-copy-link" if f + 8 == sc.frames => gallery.stage_copied_link(&ctx),
                _ => {}
            }
            // The layouts and a multi-layer picture, on made-up pictures.
            gallery_layouts::stage(&ctx, gallery, sc.name, f);
            if let Some(p) = gallery_folders::stage(gallery, sc.name, f, total) {
                input.events.push(Event::PointerMoved(p));
                input.events.push(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
                release = Some(p);
            }
            gallery_assets::stage(&ctx, gallery, sc.name, f, &sc.snapshot.settings.sessions_dir);
            gallery_talk::stage(&ctx, gallery, sc.name, f);
        }
        // The photo editor over a picture window, its preview made before each frame.
        if sc.name.starts_with("editor-") {
            editor::stage(&ctx, &mut app, sc.name, f);
        }
        if sc.page == Page::Events || sc.name.starts_with("gallery-community") {
            stage_events(&ctx, &mut app, sc, f);
        }
        // A world card's Share to … or Share publicly, the window growing out of the card (on made-up friends).
        if matches!(sc.name, "world-share-window" | "world-share-public") && !share_staged && f >= 60 && app.worlds_mut().loaded() {
            share_staged = true;
            app.worlds_mut().stage_share_window(&ctx, sc.snapshot.settings.sessions_dir.clone(), sc.name == "world-share-public");
        }
        // Sent to a friend: shown at once among who sees it, the files on their way.
        if sc.name == "world-share-sending" && !share_staged && f >= 60 && app.worlds_mut().loaded() {
            share_staged = true;
            app.worlds_mut().stage_share_as(&ctx, sc.snapshot.settings.sessions_dir.clone(), false, "sending");
        }
        if sc.name == "world-share-mid" && f == sc.frames - 4 {
            app.worlds_mut().stage_share_morph(&ctx, true);
        }
        if sc.name == "world-share-detail" && f == 90 {
            app.worlds_mut().stage_detail_share(&ctx);
        }
        if sc.name == "world-share-open" && f == 45 {
            app.worlds_mut().stage_share_morph(&ctx, false);
        }
        if sc.name == "world-close" && f == 120 {
            app.worlds_mut().close_world();
        }
        if sc.name == "world-viewer-close" && f == 170 {
            app.worlds_mut().close_picture();
        }
        // Home made its menu on the first frame.
        if let (Some(shot), 2) = (sc.home, f) {
            if let Some(host) = app.home_mut().menu_host() {
                stage_home(host, shot);
                // VRChat's camera was closed when the picture was asked for.
                if sc.name == "home-photo-camera" {
                    host.menu.notice = Some(orbiters_n4n0::platform::CAMERA_CLOSED.into());
                }
            }
        }
        // An avatar's card opened on the States page (once the page opened on the cards).
        if f == 4 {
            let opened = match sc.name {
                "home-states-avatar" => sc.snapshot.n4n0.states.avatar_id.clone(),
                "home-states-other" => sc.snapshot.n4n0.states.avatars.iter().find(|a| !a.worn).map(|a| a.id.clone()),
                _ => None,
            };
            if let Some(id) = opened {
                app.home_mut().states_mut().open_avatar(&id);
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
        if let Some(shot) = sc.library {
            if !library_staged && f >= 45 && app.sessions_mut().loaded() {
                library_staged = true;
                let origin = Rect::from_min_size(pos2(264.0, 160.0), vec2(300.0, 265.0));
                match shot {
                    LibraryShot::Storage { confirming } => app.sessions_mut().open_storage(confirming),
                    LibraryShot::Trim { .. } => {
                        app.sessions_mut().open_where(&ctx, origin, |s| s.reconstruction.is_some() && !s.trimmed && s.capture_bytes > 0)
                    }
                    LibraryShot::ViewOnly => app.sessions_mut().open_where(&ctx, origin, |s| s.reconstruction.is_some() && s.trimmed),
                }
            }
            if matches!(shot, LibraryShot::Trim { asking: true }) && f == 120 {
                app.sessions_mut().open_trim(&ctx);
            }
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
                let pick = if sc.name == "world-close" {
                    // The first card: it is on screen to land back on.
                    visited.first()
                } else if ws.pictures {
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
        let shot = f + 1 == sc.frames;
        let capture = shot || f + run.sequence >= total;
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
            if f + run.sequence >= total {
                save_png(&path.with_file_name(format!("{}-{f:03}.png", sc.name)), &image)?;
            }
            if shot {
                save_png(path, &image)?;
            }
        }
        // Background jobs (session list, thumbnails) land between frames.
        std::thread::sleep(Duration::from_millis(3));
    }
    Ok(())
}

/// The Videos page over made-up videos and lists, never this computer's (`videos`: the history; `videos-list`: the
/// second list; `videos-picker`: the bookmark picker on the first video), YouTube's thumbnails the overlay's samples.
fn stage_videos(ctx: &egui::Context, app: &mut App, name: &str) {
    use crate::overlay::menu_pages::{sample_rgba, sample_videos};
    use orbiters_core::video_history::{Bookmark, Bookmarks, History};
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let samples = sample_videos();
    let mut history = History::default();
    let yt = |i: usize| format!("https://www.youtube.com/watch?v={}", samples[i].id);
    let plays: [(String, u64, &str); 9] = [
        (yt(1), 3 * 86_400, "Japan Shrine"),
        (yt(4), 3 * 86_400 - 1800, "Japan Shrine"),
        (yt(2), 86_400 + 3600, "The Black Cat"),
        ("https://stream.vrcdn.live/live/clubnight.live.ts".into(), 2 * 3600, "The Black Cat"),
        ("https://vimeo.com/76979871".into(), 9 * 86_400, "Movie Night"),
        (yt(3), 50 * 60, "Movie Night"),
        (yt(0), 4 * 3600, "Movie Night"),
        (yt(0), 2 * 3600 + 600, "Movie Night"),
        (yt(0), 12 * 60, "Movie Night"),
    ];
    for (url, ago, world) in &plays {
        history.record(url, now - ago, Some("wrld_99123966-6b37-4b5b-9f3c-0a1b2c3d4e5f"), Some(world));
    }
    for (i, v) in samples.iter().enumerate() {
        history.set_title(&yt(i), Some((v.title.clone(), v.channel.clone())));
    }
    let mut bookmarks = Bookmarks::default();
    let mark = |url: String, ago: u64| Bookmark { url, added: now - ago, ..Default::default() };
    let favourites = bookmarks.create("Favourites");
    bookmarks.add(&favourites, mark(yt(3), 2 * 86_400));
    bookmarks.add(&favourites, mark(yt(1), 3600));
    let movies = bookmarks.create("Movie night");
    bookmarks.add(&movies, mark("https://vimeo.com/76979871".into(), 8 * 86_400));
    bookmarks.add(&movies, mark(yt(0), 600));
    bookmarks.create("Study");
    let pictures = (0..samples.len())
        .map(|i| {
            let (w, h, rgba) = sample_rgba(i);
            let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
            (samples[i].id.clone(), ctx.load_texture(format!("video-{i}"), image, egui::TextureOptions::LINEAR))
        })
        .collect();
    let list = (name == "videos-list").then_some(1);
    app.videos_mut().stand_in(history, bookmarks, list, name == "videos-picker", pictures);
}

/// Home with one of its features' pages open: YouTube with the overlay's sample suggestions or results (their
/// thumbnails too, nothing fetched) or as it opens; calls, the music and the light as the snapshot and the stand-in
/// music say.
fn stage_home(host: &mut crate::menu_host::MenuHost, HomeShot { page, samples }: HomeShot) {
    use crate::overlay::menu_pages::{sample_rgba, sample_suggestions, sample_videos};
    use orbiters_n4n0_core::menu::{Effect, Target};
    // Where a card can be dropped: a browser plays one of the sample results, found on YouTube (Home's music card copies
    // its link; the music page shows it copied, offering Drop).
    if samples && host.menu.can_drop && matches!(page, MenuPage::Home | MenuPage::Music) {
        let video = &sample_videos()[3];
        let (w, h, rgba) = sample_rgba(3);
        host.put_picture(&video.id, egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba));
        host.menu.media = Some(orbiters_n4n0_core::menu::Media {
            title: video.title.clone(),
            artist: video.channel.clone(),
            app: "Chrome".into(),
            playing: true,
            can_play_pause: true,
            can_next: true,
            // YouTube's page takes a position (its "seekto"), as Chrome tells Windows.
            can_seek: true,
            position: Some(1274),
            duration: Some(3674),
            art: Some(video.id.clone()),
            video: Some(video.id.clone()),
            ..Default::default()
        });
        if page == MenuPage::Music {
            host.press(Target::Music);
            host.press(Target::CopyPlaying);
            return;
        }
    }
    let open = match page {
        MenuPage::Home => return,
        MenuPage::Search | MenuPage::Results => Target::YouTube,
        MenuPage::Call => Target::Call,
        // The window shares captures in Photos & captures: its 3D picture page stands for the menu's sharing pages.
        MenuPage::Photo | MenuPage::History | MenuPage::ShareTo | MenuPage::Explicit => Target::Photo,
        MenuPage::Music => Target::Music,
        MenuPage::Light => Target::Light,
        MenuPage::States | MenuPage::NewState => Target::States,
        MenuPage::Tracking => Target::Tracking,
        MenuPage::Suggest | MenuPage::Type => Target::Suggest,
        MenuPage::Qr => Target::Qr,
        MenuPage::Instances | MenuPage::AddFriend => Target::Instances,
        MenuPage::Feedback | MenuPage::FeedbackLabels | MenuPage::Comments => Target::WorldFeedback,
        MenuPage::Card | MenuPage::More | MenuPage::Mirror => return,
    };
    host.press(open);
    if !samples {
        return;
    }
    match page {
        // The sample player's instances (never this computer's), and Orbiters asking to be added as a friend first.
        MenuPage::Instances | MenuPage::AddFriend => {
            use orbiters_n4n0_core::menu::{InviteAccount, InviteAnswer};
            host.menu.set_instances(crate::overlay::menu_pages::sample_instances(), true, "");
            if page == MenuPage::AddFriend {
                host.press(Target::InviteMe(0));
                let account = InviteAccount { name: "Orbiters".into(), link: "https://vrchat.com/home/user/usr_0c5b8f2e-3a41-4d7c-9e2b-5f6a7b8c9d0e".into() };
                host.menu.invite_answered(InviteAnswer::NeedsFriend {
                    account,
                    message: "Add Orbiters' VRChat account as a friend on VRChat first, then try again.".into(),
                });
            }
        }
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
                host.menu.results_arrived(id, orbiters_n4n0_core::youtube::Results { videos, continuation: Some("next".into()) }, false);
                // Where a card can be dropped: the second result's link copied, its row offering Drop.
                if host.menu.can_drop {
                    host.press(Target::Copy(1));
                }
            }
        }
        _ => {}
    }
}

/// An offscreen run in real time for the end-to-end flows: a frame about every 16 ms, presses on controls found by
/// their labels, and waits on what the app or the controller shows, each with a time limit.
struct Run<'a> {
    /// What paints the frames shot: the GPU offscreen, or without one ([`Run::cpu`]) the VR overlay's CPU painter
    /// (orbiters_ui::software), which skips paint callbacks (the 3D view's splats) and needs no GPU at all.
    gpu: Option<&'a mut Offscreen>,
    cpu: Option<orbiters_ui::software::Software>,
    ctx: egui::Context,
    app: App,
    scale: f32,
    start: Instant,
    /// A press of the last frame, let go of in this one.
    release: Option<Pos2>,
    /// Pause between frames (longer while a reconstruction trains).
    pace: Duration,
    /// The window's size in points.
    size: Vec2,
}

impl<'a> Run<'a> {
    fn new(gpu: &'a mut Offscreen, backend: Ctl, scale: f32) -> Self {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        widgets::watch_controls();
        backend.attach(&ctx);
        let mut app = App::offline(backend);
        app.skip_splash();
        Self { gpu: Some(gpu), cpu: None, ctx, app, scale, start: Instant::now(), release: None, pace: Duration::from_millis(16), size: vec2(1280.0, 820.0) }
    }

    /// A run painted on the CPU: no GPU, no 3D view.
    fn cpu(backend: Ctl, scale: f32) -> Run<'static> {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        widgets::watch_controls();
        backend.attach(&ctx);
        let mut app = App::offline(backend);
        app.skip_splash();
        let painter = orbiters_ui::software::Software::new((1280.0 * scale).round() as u32, (820.0 * scale).round() as u32);
        Run { gpu: None, cpu: Some(painter), ctx, app, scale, start: Instant::now(), release: None, pace: Duration::from_millis(16), size: vec2(1280.0, 820.0) }
    }

    fn time(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// One frame with `events` (and the release of last frame's press); its picture when `capture`.
    fn frame(&mut self, mut events: Vec<Event>, capture: bool) -> anyhow::Result<Option<egui::ColorImage>> {
        let t = self.time();
        let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)), time: Some(t), ..Default::default() };
        input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(self.scale), focused: Some(true), ..Default::default() });
        if let Some(p) = self.release.take() {
            events.insert(0, Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
        }
        input.events = events;
        let app = &mut self.app;
        let run_ui = |ui: &mut egui::Ui| {
            app.logic_offline(ui.ctx());
            app.draw(ui);
        };
        let image = match (self.gpu.as_mut(), self.cpu.as_mut()) {
            (Some(gpu), _) => gpu.frame(&self.ctx, input, run_ui, capture)?,
            (None, Some(sw)) => {
                let mut full = self.ctx.run_ui(input, run_ui);
                sw.set_textures(&mut full.textures_delta);
                let ppp = full.pixels_per_point;
                let image = capture.then(|| {
                    let (w, h) = sw.size();
                    let pixels = sw.paint(&self.ctx.tessellate(full.shapes, ppp), ppp, egui::Color32::BLACK);
                    egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], pixels)
                });
                sw.free_textures(&mut full.textures_delta);
                image
            }
            (None, None) => unreachable!("a run paints on the GPU or the CPU"),
        };
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
    run.app.sessions_mut().open_viewer(&ctx, &handles, false);
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

/// `--only e2e-cleanup`: freeing space through the interface, on a throwaway library (`--sessions` under the temporary
/// folder: this deletes for good). Keeps only the 3D view of one reconstructed session from its detail, then frees the
/// rest at once from the storage popover, the captures without a 3D view included; shoots each step and checks the
/// folders after.
fn e2e_cleanup(gpu: &mut Offscreen, out: &std::path::Path, settings: &Settings, scale: f32) -> anyhow::Result<()> {
    let library = settings.sessions_dir.canonicalize().context("--sessions <throwaway library>")?;
    anyhow::ensure!(
        library.starts_with(std::env::temp_dir().canonicalize()?),
        "e2e-cleanup deletes for good: give it a throwaway library under {}",
        std::env::temp_dir().display()
    );
    let before = controller::list_sessions(&settings.sessions_dir);
    let ctl = Controller::new(settings.clone());
    ctl.set_snapshot(idle_snapshot(settings, &before));
    let backend: Ctl = std::sync::Arc::new(Local::new(ctl.clone()));
    let mut run = Run::new(gpu, backend, scale);
    let t = run.time();
    run.app.set_page(Page::Sessions, t);
    run.until("the session list", 30.0, |run| run.app.sessions_mut().loaded())?;
    let ctx = run.ctx.clone();
    // One session from its detail.
    let one =
        before.iter().find(|s| s.reconstruction.is_some() && !s.trimmed && s.capture_bytes > 0).context("no reconstructed session with capture files")?.clone();
    run.app.sessions_mut().open_where(&ctx, Rect::from_min_size(pos2(264.0, 160.0), vec2(300.0, 265.0)), |s| s.dir == one.dir);
    run.click("Keep only the 3D view", 20.0)?;
    // The dialog once it knows what would go: the gallery's own jobs need not have ended.
    let delete = format!("Delete {}", orbiters_ui::format::bytes(one.capture_bytes as f64));
    run.until("what would go", 20.0, |run| orbiters_ui::widgets::control(&run.ctx, &delete).is_some())?;
    run.wait(0.6)?;
    run.shoot(out, "e2e-trim-asking")?;
    run.click(&format!("Delete {}", orbiters_ui::format::bytes(one.capture_bytes as f64)), 10.0)?;
    run.until("the capture files to go", 60.0, |_| ctl.view().cleanup.is_none() && orbiters_core::cleanup::trimmed(&one.dir))?;
    run.wait(1.2)?;
    run.shoot(out, "e2e-trim-done")?;
    // The rest at once, the captures without a 3D view too.
    run.frame(vec![egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }], false)?;
    run.wait(0.8)?;
    let now = controller::list_sessions(&settings.sessions_dir);
    let overview = crate::pages::storage::overview(&now, &ctl.view());
    run.app.sessions_mut().open_storage(true);
    run.wait(0.6)?;
    run.shoot(out, "e2e-cleanup-confirm")?;
    run.click(&format!("Delete {}", orbiters_ui::format::bytes((overview.trim_bytes + overview.unviewed_bytes) as f64)), 10.0)?;
    run.shoot(out, "e2e-cleanup-running")?;
    run.until("the clean-up to end", 120.0, |_| ctl.view().cleanup.is_none())?;
    run.wait(1.5)?;
    run.shoot(out, "e2e-cleanup-done")?;
    let after = controller::list_sessions(&settings.sessions_dir);
    for n in ctl.take_notices() {
        println!("notice: {n:?}");
    }
    let freed: u64 = before.iter().map(|s| s.size_bytes).sum::<u64>() - after.iter().map(|s| s.size_bytes).sum::<u64>();
    println!("sessions: {} before, {} after; {} freed", before.len(), after.len(), orbiters_ui::format::bytes(freed as f64));
    for s in &after {
        anyhow::ensure!(s.reconstruction.is_some() && s.trimmed && s.capture_bytes == 0, "{} still has capture files", s.dir.display());
        anyhow::ensure!(orbiters_core::cleanup::has_view(&s.dir), "{} lost its 3D view", s.dir.display());
    }
    anyhow::ensure!(after.len() == before.iter().filter(|s| s.reconstruction.is_some()).count(), "the captures without a 3D view are gone");
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
fn n4n0_snapshot(base: &Snapshot) -> Snapshot {
    use crate::state::{N4n0Confirmation, N4n0Face, N4n0Line, N4n0Who, ProcessUsage};
    let mut s = base.clone();
    s.vrchat.world.avatar_name = Some("Rexouium".into());
    s.vrchat.world.last_video = Some(orbiters_core::vrchat_log::VideoSeen { url: "https://www.youtube.com/watch?v=36YnV9STBqc".into(), at: None });
    s.settings.n4n0.afk_message_enabled = true;
    s.settings.n4n0.afk_message = "Getting food, back in 10!".into();
    s.settings.n4n0.pins = vec![orbiters_core::settings::Pin { parameter: "Hoodie".into(), value: orbiters_core::osc::OscValue::Bool(true) }];
    let line = |who, text: &str| N4n0Line { who, text: text.into(), at_ms: 0, streaming: false };
    let o = &mut s.n4n0;
    o.avatar_id = Some("avtr_7f3c2b1e-0d4a-4a7b-9c55-1f0e9a2b6c11".into());
    o.prop_sync = Some(8);
    o.manifest = true;
    o.controls = 42;
    o.face = N4n0Face::Talking;
    o.panel = Some("Done, hoodie on. Now go find\nyour friends instead of\ntalking to a floating ball.".into());
    *o.lines = vec![
        line(N4n0Who::You, "Nano, mets mon sweat à capuche"),
        line(N4n0Who::Note, "N4N0 set Clothes / Hoodie"),
        line(N4n0Who::N4n0, "Done, hoodie on. Now go find your friends instead of talking to a floating ball."),
        line(N4n0Who::You, "Plan a movie night for my community on Saturday at 9 pm"),
        line(N4n0Who::Note, "N4N0: create event draft…"),
        line(N4n0Who::N4n0, "Drafted “Movie night” for Saturday 21:00 in Cozy Cinema. Approve it on Orbiters and I'll announce it."),
    ];
    o.confirmations = vec![N4n0Confirmation {
        id: "c1".into(),
        title: "Publish “Movie night”".into(),
        summary: "Saturday 21:00 · Cozy Cinema · announced in #events".into(),
        approve_url: "https://orbiters.cc/confirm/c1".into(),
    }];
    o.quota = Some(orbiters_n4n0_core::assistant::Quota { used: 13, limit: 80, resets_at: None, own_key: false });
    o.voice = crate::state::VoiceStatus { installed: true, running: true, ..Default::default() };
    *o.videos = vec![
        orbiters_n4n0_core::videos::SavedVideo {
            url: "https://www.youtube.com/watch?v=36YnV9STBqc".into(),
            title: Some("Lo-fi beats to sit in a VRChat club to".into()),
            world: Some("The Black Cat".into()),
            saved_utc: "2026-10-20T22:14:03Z".into(),
        },
        orbiters_n4n0_core::videos::SavedVideo {
            url: "https://youtu.be/pUaeOcyzDrA".into(),
            title: None,
            world: Some("Palm Point - Night".into()),
            saved_utc: "2026-10-18T21:33:48Z".into(),
        },
    ];
    *o.resources = vec![
        ProcessUsage { name: "Orbiters - N4N0".into(), cpu_percent: 1.2, memory_mb: 148.0 },
        ProcessUsage { name: "N4N0's ears".into(), cpu_percent: 0.4, memory_mb: 912.0 },
        ProcessUsage { name: "VRChat".into(), cpu_percent: 187.0, memory_mb: 6840.0 },
    ];
    s
}

/// The sample avatar's saved states (`avatar`: the avatar worn): the second the default, the third with the pose and the
/// size.
fn sample_states(avatar: Option<String>) -> crate::state::StatesView {
    use crate::state::{StateView, StatesView};
    let state = |id: &str, name: &str, saved: &str, default: bool, pose: bool, size: bool, parameters: usize, pose_parameters: usize| StateView {
        id: id.into(),
        name: name.into(),
        saved_utc: saved.into(),
        default,
        pose,
        size,
        parameters,
        pose_parameters,
        eye_height: Some(1.62),
    };
    let states = vec![
        StateView { eye_height: None, ..state("s1", "Hoodie on", "2026-10-21T19:40:00Z", false, false, false, 46, 12) },
        state("s2", "Cosy evening", "2026-10-23T21:05:00Z", true, true, false, 46, 12),
        state("s3", "Tall and sitting", "2026-10-24T18:12:00Z", false, true, true, 46, 12),
    ];
    // The avatars' cards: the one worn, then others saved in before (one from before names were kept).
    let mut avatars = vec![
        crate::state::AvatarView {
            id: "avtr_2b8e41c7-5f1a-4d90-8e3b-77a1c0d4e9f2".into(),
            name: Some("Ultirex ✨ Noir".into()),
            worn: false,
            states: vec![
                state("u1", "Club night", "2026-10-22T23:10:00Z", true, false, false, 88, 0),
                state("u2", "Daytime", "2026-10-20T14:00:00Z", false, false, true, 88, 0),
            ],
        },
        crate::state::AvatarView {
            id: "avtr_9d04a3e2-1c55-4b7e-a0f1-2e6c8b5d7a13".into(),
            name: None,
            worn: false,
            states: vec![state("o1", "State 1", "2026-09-30T10:00:00Z", false, false, false, 12, 0)],
        },
    ];
    if let Some(id) = &avatar {
        avatars.insert(0, crate::state::AvatarView { id: id.clone(), name: Some("Rexouium".into()), worn: true, states: states.clone() });
    }
    StatesView { avatar_id: avatar, avatar_name: Some("Rexouium".into()), states, note: None, saving: false, avatars }
}

fn with_account(base: &Snapshot, state: crate::state::AccountState, error: Option<&str>) -> Snapshot {
    let mut s = base.clone();
    s.account.state = state;
    s.account.error = error.map(str::to_string);
    s
}

pub(crate) fn idle_snapshot(settings: &Settings, sessions: &[orbiters_core::bundle::SessionSummary]) -> Snapshot {
    let mut s = Snapshot::new(settings.clone());
    s.account.state = crate::state::AccountState::SignedIn { username: "Orbit".into() };
    s.vrchat.running = true;
    s.vrchat.log_found = true;
    *s.vrchat.world = WorldState {
        world_id: Some("wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b".into()),
        world_name: Some("Canine Test Lab".into()),
        instance: Some("12345~private".into()),
        in_world: true,
        log_file: Some(PathBuf::from(r"C:\Users\Enzo\AppData\LocalLow\VRChat\VRChat\output_log_2026-10-20_01-12-44.txt")),
        ..Default::default()
    };
    *s.sources = vec![SpoutSourceInfo { name: "VRChat".into(), width: 1920, height: 1080, format: 87, description: String::new() }];
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
    *s.mics = vec![
        orbiters_core::protocol::AudioDeviceInfo { id: "index".into(), name: "Microphone (Valve Index)".into(), default: true },
        orbiters_core::protocol::AudioDeviceInfo { id: "desk".into(), name: "Desk microphone (USB)".into(), default: false },
    ];
    s.storage.used_bytes = sessions.iter().map(|x| x.size_bytes).sum();
    s.storage.sessions = sessions.len();
    let trimmable: Vec<u64> = sessions.iter().filter(|x| x.reconstruction.is_some() && x.capture_bytes > 0).map(|x| x.capture_bytes).collect();
    s.storage.capture_bytes = trimmable.iter().sum();
    s.storage.capture_sessions = trimmable.len();
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
pub(crate) struct Still;

impl crate::backend::Backend for Still {
    fn n4n0(&self, _command: crate::ipc::N4n0Command) {}

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
    fn clean_up(&self, _trim: Vec<PathBuf>, _delete: Vec<PathBuf>) {}
    fn reconstruct(&self, _dir: PathBuf) {}
    fn stop_reconstruction(&self) {}
    fn unqueue_reconstruction(&self, _dir: PathBuf) {}
    fn send_to_mixer(&self, _dir: PathBuf, _title: String) {}
    fn sign_in(&self) {}
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
    *idle.mics = vec![orbiters_core::protocol::AudioDeviceInfo { id: "1".into(), name: "Microphone (Valve Index)".into(), default: true }];
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
    let mut talking = n4n0_snapshot(&with_account(&idle, crate::state::AccountState::SignedIn { username: "Desktop Tester".into() }, None));
    talking.vrchat.running = true;
    let mut listening = talking.clone();
    listening.n4n0.listening = true;
    listening.n4n0.voice.level = 0.55;
    let calling = with_calls(&talking, true);
    let mut walking = talking.clone();
    walking.vrchat.world.in_world = true;
    walking.settings.walk_boost.enabled = true;
    walking.settings.walk_boost.strength = 0.35;
    // (name, snapshot, calibration: (T-pose held, started at this time; the last frame is at 1.48 s)).
    type Shot<'a> = (&'a str, &'a Snapshot, Option<(bool, f64)>);
    let shots: [Shot; 9] = [
        ("overlay-n4n0-call", &calling, None),
        ("overlay-walk", &walking, None),
        ("overlay-n4n0", &talking, None),
        ("overlay-n4n0-listening", &listening, None),
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
        if name.starts_with("overlay-n4n0") {
            ui_state.page = crate::overlay::ui::Page::N4n0;
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
    overlay_menu_shots(out, &talking)
}

/// N4N0's menu in the play space sharing 3D captures (screen version 12's pages), driven as the overlay drives it: the
/// service's snapshot followed (the sample captures, then the sample friends), presses as a player makes them, drawn in
/// the overlay's look. The pictures are the sample thumbnails, never this computer's sessions.
fn overlay_menu_shots(out: &std::path::Path, snap: &Snapshot) -> anyhow::Result<()> {
    use crate::menu_host::MenuHost;
    use crate::overlay::menu_pages::{sample_captures, sample_friends, sample_rgba};
    use orbiters_n4n0_core::captures::Friends;
    use orbiters_n4n0_core::menu::Target;
    let backend: Ctl = std::sync::Arc::new(Still);
    let mut snap = snap.clone();
    *snap.n4n0.captures = sample_captures();
    let mut canvas = crate::overlay::Canvas::new(1024, 576);
    let mut host = MenuHost::new(canvas.ctx().clone());
    for row in &snap.n4n0.captures.history {
        let (w, h, rgba) = sample_rgba(row.key.trim_start_matches("sample").parse().unwrap_or(0));
        host.put_picture(&row.picture_key(), egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba));
    }
    host.open(&snap);
    host.follow(&snap);
    let press = |host: &mut MenuHost, snap: &Snapshot, target: Target| {
        let effects = host.press(target);
        host.effects(effects, snap, &backend);
    };
    let mut shoot = |host: &MenuHost, name: &str, hover: Target| -> anyhow::Result<()> {
        use crate::overlay::menu_ui::{self, Skin, View};
        use orbiters_n4n0_core::menu::layout::{self, ScreenPage};
        let hover = layout::cached(ScreenPage::of(&host.menu)).elements.iter().position(|e| e.target == hover);
        let picture = |id: &str| host.picture(id, false);
        let view = View { menu: &host.menu, hover, pressed: None, picture: &picture, time: 0.4, skin: Skin::english() };
        let rgba = canvas.paint(0.4, |p| menu_ui::draw(p, pos2(0.0, 0.0), &view)).to_vec();
        let path = out.join(format!("{name}.png"));
        save_png(&path, &over_ground(&rgba, 1024, 576))?;
        println!("{}", path.display());
        Ok(())
    };
    press(&mut host, &snap, Target::Photo);
    shoot(&host, "overlay-menu-photo-share", Target::ShareLatestTo)?;
    // The two pictures as buttons: one hovered; greyed out while a timer counts; what stopped or warns about a picture.
    use orbiters_n4n0_core::menu::PhotoMode;
    shoot(&host, "overlay-menu-photo-two-hover", Target::TakeOf(PhotoMode::World))?;
    host.menu.countdown = Some(3);
    shoot(&host, "overlay-menu-photo-two-countdown", Target::Back)?;
    host.menu.countdown = None;
    host.menu.notice = Some("Your avatar is in this picture: see Capture".into());
    shoot(&host, "overlay-menu-photo-two-notice", Target::Back)?;
    host.menu.notice = None;
    press(&mut host, &snap, Target::History);
    shoot(&host, "overlay-menu-history", Target::SharePublic(3))?;
    // Share to …: the friends come with the next snapshot, two are chosen.
    press(&mut host, &snap, Target::ShareTo(0));
    shoot(&host, "overlay-menu-share-to-loading", Target::FindFriend)?;
    *snap.n4n0.friends = Friends::ready(sample_friends());
    host.follow(&snap);
    press(&mut host, &snap, Target::Friend(0));
    press(&mut host, &snap, Target::Friend(4));
    shoot(&host, "overlay-menu-share-to", Target::SendShare)?;
    press(&mut host, &snap, Target::SendShare);
    *snap.n4n0.friends = Friends::None;
    // The flagged capture's Share publicly: the warning first.
    press(&mut host, &snap, Target::SharePublic(2));
    shoot(&host, "overlay-menu-explicit", Target::ShareAnyway)?;
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
/// `snap` with a call connected and "Talk with N4N0?" up (half its time left).
fn with_wake_offer(snap: &Snapshot) -> Snapshot {
    use crate::state::{CallPhase, CallView};
    let mut s = snap.clone();
    s.n4n0.calls.call = Some(CallView { name: "Nova Fox".into(), phase: CallPhase::Connected, incoming: false, connected_at_ms: Some(crate::wake_offer::now_ms() - 192_000), muted: false, mic_level: 0.2, remote_level: 0.4 });
    // Long enough for the frames the harness paints.
    s.n4n0.voice.wake_offer = Some(orbiters_n4n0::status::WakeOffer { id: 1, until_ms: crate::wake_offer::now_ms() + 600_000, for_ms: 1_200_000 });
    s
}

fn with_calls(snap: &Snapshot, ringing: bool) -> Snapshot {
    use crate::state::{CallContact, CallLink, CallPhase, CallView};
    let mut s = snap.clone();
    s.settings.n4n0.calls = true;
    s.n4n0.calls.link = CallLink::Linked { account: "usr_1339e525-d75d-4832-9104-98dd89287681".into() };
    s.n4n0.calls.contacts =
        ["Kiri", "Nova Fox", "Rexouium Fan"].iter().enumerate().map(|(i, n)| CallContact { id: format!("c_{i}"), name: n.to_string() }).collect();
    if ringing {
        s.n4n0.calls.call = Some(CallView {
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

#[cfg(test)]
mod dashboard_tests {
    use super::*;

    /// The SteamVR dashboard as it is painted, settled: its pixels.
    fn dashboard(snap: &Snapshot) -> Vec<u8> {
        use crate::overlay::ui::{OverlayUi, SIZE};
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let backend: Ctl = std::sync::Arc::new(Still);
        let mut ui_state = OverlayUi::default();
        let mut body = StandInBody { t_pose: false };
        let mut sw = crate::overlay::painter();
        let mut image = Vec::new();
        for f in 0..90 {
            let t = f as f64 / 60.0;
            ui_state.tick(t, false, &mut body, &backend);
            let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)), time: Some(t), ..Default::default() };
            input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(1.0), focused: Some(true), ..Default::default() });
            let mut full = ctx.run_ui(input, |ui| ui_state.show(ui, snap, &backend, &mut body));
            sw.set_textures(&mut full.textures_delta);
            if f == 89 {
                let prims = ctx.tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
                image = sw.paint(&prims, full.pixels_per_point, orbiters_ui::theme::INK_950).to_vec();
            }
            sw.free_textures(&mut full.textures_delta);
        }
        image
    }

    /// Enzo (F15): with nothing to stop, the dashboard's recorder starts nothing, and its button looks so: no bright red dot that reads as Record.
    #[test]
    fn the_dashboard_s_record_button_looks_disabled_with_nothing_to_stop() {
        let mut idle = idle_snapshot(&Settings::default(), &[]);
        idle.steamvr.running = true;
        idle.steamvr.overlay = true;
        let rec = orbiters_ui::theme::REC;
        let reddish = |image: &[u8]| image.chunks_exact(4).filter(|p| (p[0] as i32 - rec.r() as i32).abs() < 40 && (p[1] as i32 - rec.g() as i32).abs() < 40 && (p[2] as i32 - rec.b() as i32).abs() < 40).count();
        assert!(reddish(&dashboard(&idle)) < 40, "a record-red dot on a button that does nothing");
        // Recording, its Stop is live and red as ever.
        let mut recording = idle.clone();
        recording.capture.state = CaptureState::Recording;
        assert!(reddish(&dashboard(&recording)) > 40, "the live recorder lost its colour");
    }
}
