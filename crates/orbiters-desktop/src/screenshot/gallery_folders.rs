//! The gallery folder manager with an unplugged archive, without touching real folders.
//!
//! `gallery-folders` (staged open), `gallery-folders-click` (opened by a press and its release on Manage folders, as a
//! person does: it must stay open).
use super::{Page, Scenario};
use egui::Pos2;

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let mut snapshot = scenarios[0].snapshot.clone();
    snapshot.settings.picture_folders = vec!["I:/VRChat pics".into()];
    for name in ["gallery-folders", "gallery-folders-click"] {
        scenarios.push(Scenario { name, page: Page::Sessions, snapshot: snapshot.clone(), frames: 140,
            splash: false, open_detail: false, toasts: vec![], pointer: None, scroll: 0.0, world: None, home: None, library: None });
    }
}

/// What the scenario does on frame `f`; `Some` presses there (released on the next frame).
pub(super) fn stage(gallery: &mut crate::pages::gallery::State, name: &str, f: u32, frames: u32) -> Option<Pos2> {
    match (name, f) {
        ("gallery-folders", 80) => gallery.stage_folders(),
        ("gallery-folders-click", 80) => gallery.stage_folders_closed(),
        ("gallery-folders-click", 90) => return gallery.folders_button().map(|r| r.center()),
        ("gallery-folders-click", _) if f + 1 == frames => {
            assert!(gallery.folders_open(), "gallery-folders-click: the popover closed after the click that opened it");
        }
        _ => {}
    }
    None
}
