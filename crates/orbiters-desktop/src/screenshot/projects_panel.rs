//! The Unity projects panel's sizes, Duplicate dialog, hidden projects, avatars' VRChat pages and Blender files, and
//! world projects' Worlds card (staged by `projects::State::stage_panel`): no disk walk, file dialog, upload or
//! request.
use super::{Page, Scenario};
use crate::state::AccountState;

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let base = scenarios[0].snapshot.clone();
    for name in [
        "projects-size",
        "projects-size-measuring",
        "projects-avatars-links",
        "projects-worlds",
        "projects-worlds-new",
        "projects-worlds-sharing",
        "projects-worlds-done",
        "projects-worlds-failed",
        "projects-worlds-signed-out",
        "projects-worlds-capture-menu",
        "projects-duplicate",
        "projects-duplicate-taken",
        "projects-duplicate-library",
        "projects-duplicate-copying",
        "projects-hidden",
        "projects-hidden-shown",
        "projects-hidden-press",
        "projects-header-blur",
        "projects-revisit",
        "projects-card-menu",
        "projects-avatar-menu",
        // Updates, missing packages' repositories, conflicts, a project open in Unity (`package_fixtures.rs`).
        "projects-updates",
        "projects-updates-plan",
        "projects-missing",
        "projects-missing-looking",
        "projects-missing-found",
        "projects-missing-adding",
        "projects-missing-done",
        "projects-conflict",
        "projects-conflict-blocked",
        "projects-conflict-embedded",
        "projects-unity-open",
        "projects-unity-kill",
        "projects-unity-closing",
        "projects-unity-killing",
        "projects-unity-bar",
        "projects-unity-menu",
        // What the panel says about a project's disk, and a Library move (`storage_card.rs`). The `-real` ones really move
        // a stand-in project in the run's sandbox (`ORBITERS_SCREENSHOT_REPAIR_MB`), pressed through the card's buttons.
        "projects-storage-low",
        "projects-storage-failed",
        "projects-storage-earlier",
        "projects-storage-unity-open",
        "projects-storage-measuring",
        "projects-storage-no-room",
        "projects-storage-redirected",
        "projects-repair-copying",
        "projects-repair-cancelling",
        "projects-repair-removing",
        "projects-repair-other",
        "projects-repair-real",
        "projects-repair-real-cancel",
        "projects-repair-real-done",
        // VRChat's move to Unity 6 (`pages/projects/unity6_fixtures.rs`): stand-in projects generated in the sandbox.
        "projects-u6-gallery",
        "projects-u6-jobs",
        "projects-u6-damaged",
        "projects-u6-panel-avatar",
        "projects-u6-packages",
        "projects-u6-panel-world",
        "projects-u6-editor-unverified",
        "projects-u6-editor-found",
        "projects-u6-mac",
        "projects-u6-try-ids",
        "projects-u6-try-baseline",
        "projects-u6-try-budget",
        "projects-u6-try-budget-short",
        "projects-u6-try-blocked",
        "projects-u6-try-copying",
        "projects-u6-try-done",
        "projects-u6-try-failed",
        "projects-u6-trial-waiting",
        "projects-u6-trial-vcc-copy",
        "projects-u6-trial-ready",
        "projects-u6-outcomes-a",
        "projects-u6-outcomes-b",
    ] {
        let mut snapshot = base.clone();
        snapshot.account.development = true;
        snapshot.account.state =
            if name == "projects-worlds-signed-out" { AccountState::SignedOut } else { AccountState::SignedIn { username: "Desktop Tester".into() } };
        scenarios.push(Scenario {
            name,
            page: Page::Projects,
            snapshot,
            frames: if name.starts_with("projects-repair-real") {
                // Long enough for a real move to start, run and end, as the disk allows.
                400
            } else if name.ends_with("-menu") || name.ends_with("-press") {
                100
            } else if name == "projects-revisit" {
                // Left, entered again, then every picture asked for again and Nova's replaced (`--sequence 60`).
                120
            } else {
                90
            },
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
}
