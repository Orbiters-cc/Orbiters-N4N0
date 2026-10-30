//! The gallery folder manager with an unplugged archive, without touching real folders.
use super::{Page, Scenario};
pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let mut snapshot = scenarios[0].snapshot.clone();
    snapshot.settings.picture_folders = vec!["I:/VRChat pics".into()];
    scenarios.push(Scenario { name: "gallery-folders", page: Page::Sessions, snapshot, frames: 140,
        splash: false, open_detail: false, toasts: vec![], pointer: None, scroll: 0.0, world: None, home: None, library: None });
}
pub(super) fn stage(gallery: &mut crate::pages::gallery::State, name: &str, f: u32) {
    if name == "gallery-folders" && f == 80 { gallery.stage_folders(); }
}
