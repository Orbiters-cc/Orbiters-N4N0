//! Photos & captures' My assets, the cards' menus and remote pictures selected and dragged, on the gallery's fixture
//! (the member's assets and their showcase galleries made up in `pages::gallery::assets`, this computer's pictures
//! standing in for the Discord rooms' pictures).
//!
//! `gallery-assets` (My assets: one group for each asset's showcase), `gallery-assets-all` (All, the assets among the rest),
//! `gallery-assets-open` (an asset's picture in the picture window), `gallery-menu` (a local picture's menu),
//! `gallery-menu-asset` (an asset picture's menu), `gallery-select-remote` (three asset pictures selected and lifted, the
//! drag waiting for two of them from Orbiters).
use super::{Page, Scenario};

const NAMES: [&str; 6] = ["gallery-assets", "gallery-assets-all", "gallery-assets-open", "gallery-menu", "gallery-menu-asset", "gallery-select-remote"];

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let base = scenarios[0].snapshot.clone();
    for name in NAMES {
        scenarios.push(Scenario {
            name,
            page: Page::Sessions,
            snapshot: base.clone(),
            frames: 150,
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

/// What each scenario does on frame `f` (after the gallery's own staging).
pub(super) fn stage(ctx: &egui::Context, gallery: &mut crate::pages::gallery::State, name: &str, f: u32, sessions_dir: &std::path::Path) {
    if !NAMES.contains(&name) {
        return;
    }
    let assets = name != "gallery-menu";
    if assets && (f == 1 || f == 46) {
        gallery.stage_assets(name, sessions_dir);
    }
    match (name, f) {
        ("gallery-assets-open", 80) => gallery.open_asset(ctx, 1),
        ("gallery-menu", 100) => gallery.stage_menu(ctx, 0),
        ("gallery-menu-asset", 100) => gallery.stage_menu(ctx, 0),
        ("gallery-select-remote", 90) => gallery.stage_select(3),
        ("gallery-select-remote", 100) => gallery.stage_wait(3, 2),
        _ => {}
    }
}
