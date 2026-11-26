//! REQUEST21 5: Photos & captures with its capture element, over made-up pictures (the layouts' own) and made-up
//! states, nothing of this computer's: the gallery with the bar alone (`photos-capture`), every option open
//! (`-options`, and `-options-scrolled` further down the one list), a recording and a 3D picture under way, the last
//! capture's error, what it could not leave out, N4N0 missing from the avatar, a capture just saved, a reconstruction,
//! and the bar hidden by the person during a recording (`-recording-hidden`).
use super::{Page, Scenario};
use crate::pages::capture::Part;
use orbiters_core::settings::PictureMode;

pub(super) const NAMES: [&str; 11] = [
    "photos-capture",
    "photos-capture-options",
    "photos-capture-options-scrolled",
    "photos-capture-recording",
    "photos-capture-picture",
    "photos-capture-error",
    "photos-capture-left-out",
    "photos-capture-kit",
    "photos-capture-finished",
    "photos-capture-reconstructing",
    "photos-capture-recording-hidden",
];

fn snapshot_of(scenarios: &[Scenario], name: &str) -> crate::state::Snapshot {
    scenarios.iter().find(|s| s.name == name).map(|s| s.snapshot.clone()).unwrap_or_else(|| scenarios[0].snapshot.clone())
}

/// The made-up pictures' folder (the layouts' own, written once).
fn fixture() -> std::path::PathBuf {
    orbiters_core::paths::stand_ins().join("gallery-fixture")
}

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let idle = snapshot_of(scenarios, "capture-idle");
    // REQUEST21: Settings › VR with the menus' smoothing moved off the values measured with VRChat: Reset under them.
    let mut moved = idle.clone();
    moved.settings.orbit.vr_smoothing_quick_menu_ms = 62.0;
    moved.settings.orbit.vr_smoothing_wheel_ms = 8.0;
    scenarios.push(Scenario {
        name: "settings-smoothing-reset",
        page: Page::Settings,
        snapshot: moved,
        frames: 120,
        splash: false,
        open_detail: false,
        toasts: vec![],
        pointer: None,
        scroll: 0.0,
        world: None,
        home: None,
        library: None,
    });
    for name in NAMES {
        let mut snapshot = match name {
            "photos-capture-recording" | "photos-capture-recording-hidden" => snapshot_of(scenarios, "capture-recording"),
            "photos-capture-picture" => snapshot_of(scenarios, "capture-picture-people"),
            "photos-capture-left-out" => snapshot_of(scenarios, "capture-idle-after"),
            _ => idle.clone(),
        };
        // The badge of the sidebar's Photos & captures: how many captures this made-up library has.
        snapshot.storage.sessions = 97;
        match name {
            "photos-capture-error" => snapshot.capture.error = Some("VRChat's camera stopped sending frames: the capture was saved up to there.".into()), // i18n: not shown (made-up error of the pictures)
            "photos-capture-kit" => {
                snapshot.n4n0.avatar_id = Some("avtr_00000000-0000-4000-8000-000000000001".into());
                snapshot.n4n0.manifest = false;
            }
            "photos-capture-reconstructing" => {
                snapshot.reconstruction = Some(crate::state::ReconstructionJob {
                    stage: orbiters_core::reconstruction::Stage::Training,
                    progress: 0.46,
                    iteration: 4140,
                    iterations: 9000,
                    gaussians: 812_000,
                    psnr: Some(27.4),
                    seconds: 214.0,
                    ..crate::state::ReconstructionJob::new(fixture().join("shape-0-960x540.png"), false)
                });
            }
            _ => {}
        }
        scenarios.push(Scenario {
            name,
            page: Page::Sessions,
            snapshot,
            frames: 150,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: if name == "photos-capture-options-scrolled" { 900.0 } else { 0.0 },
            world: None,
            home: None,
            library: None,
        });
    }
}

/// What each scenario does on frame `f` (after the gallery's own staging): the made-up pictures, and the element.
pub(super) fn stage(ctx: &egui::Context, app: &mut crate::app::App, name: &str, f: u32, t: f64) {
    if !NAMES.contains(&name) {
        return;
    }
    if f == 0 || f == 45 {
        let gallery = app.sessions_mut().gallery_mut();
        gallery.stage_extra(super::gallery_layouts::entries(&fixture()));
        gallery.stage_made_up_only();
    }
    if f == 0 {
        match name {
            "photos-capture-options" | "photos-capture-options-scrolled" => app.open_capture(Part::Actions, t),
            "photos-capture-finished" => app.capture_mut().stage_finished(fixture().join("shape-3-800x600.png"), Some(PictureMode::World)),
            _ => {}
        }
    }
    // The person's Hide pressed while the recording runs: the bar alone, its Stop on it.
    if name == "photos-capture-recording-hidden" && f == 60 {
        app.capture_mut().stage_hidden();
    }
    let _ = ctx;
}
