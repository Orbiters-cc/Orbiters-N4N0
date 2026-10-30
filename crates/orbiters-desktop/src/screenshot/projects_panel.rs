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
    ] {
        let mut snapshot = base.clone();
        snapshot.account.development = true;
        snapshot.account.state =
            if name == "projects-worlds-signed-out" { AccountState::SignedOut } else { AccountState::SignedIn { username: "Desktop Tester".into() } };
        scenarios.push(Scenario {
            name,
            page: Page::Projects,
            snapshot,
            frames: if name.ends_with("-menu") || name.ends_with("-press") {
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
