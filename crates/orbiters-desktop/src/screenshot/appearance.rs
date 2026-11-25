//! `--only appearance`: Settings' Appearance (the default look, palettes, fonts, each background's still frame, the fonts of this
//! computer as a made-up list, a small window), and pages of the app under another look. `--appearance
//! <palette>[:<font>[:<background>]]` draws any scene in a look (`aurora:nunito:aero-shards`).

use super::{Page, Scenario};
use crate::app::App;
use crate::appearance::PALETTES;
use orbiters_core::appearance::{AppearanceSettings, Background};

/// What `--appearance` says.
pub(super) fn parse(text: &str) -> Option<AppearanceSettings> {
    let mut parts = text.split(':');
    let mut a = AppearanceSettings::default();
    let palette = parts.next().unwrap_or_default();
    if !palette.is_empty() {
        PALETTES.iter().find(|p| p.name.eq_ignore_ascii_case(palette))?.apply(&mut a);
    }
    a.font = parts.next().unwrap_or_default().to_string();
    if let Some(id) = parts.next().filter(|b| !b.is_empty()) {
        a.background = background(id)?;
    }
    Some(a)
}

fn background(id: &str) -> Option<Background> {
    crate::appearance::BACKGROUNDS.into_iter().find(|b| crate::appearance::scene(*b).id() == id)
}

/// A scene's look, after its name: `appearance-<page>-<palette>`, `-font-<font>`, `-bg-<background>`.
fn look_of(name: &str) -> Option<AppearanceSettings> {
    let rest = name.strip_prefix("appearance-")?;
    let mut a = AppearanceSettings::default();
    for p in &PALETTES {
        if rest.ends_with(&format!("-{}", p.name.to_lowercase())) {
            p.apply(&mut a);
        }
    }
    if let Some(font) = rest.strip_prefix("font-") {
        a.font = font.to_string();
    }
    if let Some((_, id)) = rest.split_once("bg-") {
        a.background = background(id)?;
    }
    if rest == "dark-accent" {
        a.accent = [0x23, 0x2a, 0x78];
    }
    Some(a)
}

/// What a scene does on its first frame; true when it shows a setting of its own.
pub(super) fn stage(app: &mut App, name: &str, now: f64) -> bool {
    let Some(rest) = name.strip_prefix("appearance-") else { return false };
    let at = match rest {
        r if r.starts_with("bg-") => "appearance.background",
        r if r.starts_with("font-") || r == "fonts" => "appearance.font",
        "dark-accent" => "appearance.own-accent",
        r if r.starts_with("page") => "appearance",
        _ => return false,
    };
    app.open_settings(at, now);
    app.settings_mut().quietly();
    if rest == "fonts" {
        app.settings_mut().stage_fonts(&["Arial", "Bahnschrift", "Candara", "Comic Sans MS", "Georgia", "Trebuchet MS", "Verdana"]);
    }
    true
}

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let snapshot = scenarios[0].snapshot.clone();
    let mut add = |name: &'static str, page: Page| {
        let mut s = snapshot.clone();
        if let Some(a) = look_of(name) {
            s.settings.appearance = a;
        }
        scenarios.push(Scenario { name, page, snapshot: s, frames: 90, splash: false, open_detail: false, toasts: vec![], pointer: None, scroll: 0.0, world: None, home: None, library: None });
    };
    for name in [
        "appearance-page-orbiters",
        "appearance-page-aurora",
        "appearance-page-ember",
        "appearance-page-midnight",
        "appearance-page-moonlight",
        "appearance-dark-accent",
        "appearance-font-nunito",
        "appearance-font-roboto-slab",
        "appearance-fonts",
        "appearance-bg-aero-shards",
        "appearance-bg-grainient",
        "appearance-bg-gradient-waves",
        "appearance-bg-pattern-waves",
        "appearance-bg-gradient-ascii",
        "appearance-bg-leaf-shadow",
    ] {
        add(name, Page::Settings);
    }
    for name in [
        "appearance-home-aurora",
        "appearance-home-ember",
        "appearance-home-bg-aero-shards",
        "appearance-home-bg-grainient",
        "appearance-home-bg-gradient-waves",
        "appearance-home-bg-pattern-waves",
        "appearance-home-bg-gradient-ascii",
        "appearance-home-bg-leaf-shadow",
    ] {
        add(name, Page::Home);
    }
}
