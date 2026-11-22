//! `--only r21`: Settings in sub-tabs with its search (REQUEST21 2), on made-up data: each sub-tab, a search and its
//! results, a setting the search opened (brought into view and lit), no result, and the N4N0 page with what is left of it.
//! Run with `--width 900 --height 700` for the narrow window, `--language fr` / `ja` for those languages. Also: which
//! setting each older Settings scene shows now that Settings has sub-tabs.

use super::{Page, Scenario};
use crate::app::App;
use crate::pages::settings_registry::Group;

/// The older Settings scenes, each on the setting it was made for (brought into view without being lit).
fn older(name: &str) -> Option<&'static str> {
    Some(match name {
        "settings" | "settings-signed-out" | "settings-signing-in" => "account",
        "settings-explorer" | "settings-explorer-negative" => "account.explorer-points",
        "settings-booster" => "vrchat.booster",
        "settings-translate" | "settings-translate-orbiters" => "translate.on",
        "settings-perf-report" => "vrchat.perf-report",
        "settings-walk" | "settings-walk-trackers" => "vr.walk",
        "settings-start" => "app.start-windows",
        "settings-watch" => "vr.watch",
        "settings-vr-audio" => "vr.audio-output",
        // REQUEST21: the smoothing's Reset, the values moved off the measured ones (photos_capture.rs makes the scene).
        "settings-smoothing-reset" => "vr.smoothing-reset",
        "network-settings" => "vrchat.network",
        "vrchat-bridge" => "vrchat.bridge",
        n if n.starts_with("vrchat-login") || n.starts_with("launcher-") => "account.vrchat",
        _ => return None,
    })
}

/// What a Settings (or N4N0) scene does on its first frame; true when it shows a setting of its own (so its old wheel
/// scroll is not needed).
pub(super) fn stage(app: &mut App, name: &str, now: f64) -> bool {
    if let Some(group) = name.strip_prefix("r21-").and_then(Group::of_id) {
        app.open_settings(group.id(), now);
        return true;
    }
    match name {
        "r21-search" => {
            app.open_settings("account", now);
            app.settings_mut().stage_search(search_words());
            true
        }
        "r21-no-results" => {
            app.open_settings("account", now);
            app.settings_mut().stage_search("zzqx");
            true
        }
        "r21-jump" => {
            // As if the first result of the search had been chosen.
            let first = crate::pages::settings_registry::search(search_words(), &crate::pages::settings_registry::Open::default());
            let id = first.first().map_or("voice.mic", |f| f.entry.id);
            app.open_settings(id, now);
            app.settings_mut().stage_found(id);
            true
        }
        _ => match older(name) {
            Some(target) => {
                app.open_settings(target, now);
                app.settings_mut().quietly();
                true
            }
            None => false,
        },
    }
}

/// What the search scenes type: a word of the language, an accent left out in French.
fn search_words() -> &'static str {
    match orbiters_i18n::language() {
        orbiters_i18n::Language::Fr => "demarrage",
        orbiters_i18n::Language::Ja => "マイク",
        orbiters_i18n::Language::Es => "micro",
        orbiters_i18n::Language::PtBr => "micro",
        orbiters_i18n::Language::En => "micro",
    }
}

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let mut snapshot = scenarios[0].snapshot.clone();
    snapshot.account.state = crate::state::AccountState::SignedIn { username: "SampleMember".into() };
    snapshot.vrchat_login.allowed = true;
    snapshot.settings.n4n0.calls = true;
    snapshot.settings.walk_boost.enabled = true;
    let scene = |name: &'static str, page: Page, frames: u32| Scenario {
        name,
        page,
        snapshot: snapshot.clone(),
        frames,
        splash: false,
        open_detail: false,
        toasts: vec![],
        pointer: None,
        scroll: 0.0,
        world: None,
        home: None,
        library: None,
    };
    for name in ["r21-account", "r21-n4n0", "r21-voice", "r21-vr", "r21-capture", "r21-vrchat", "r21-language", "r21-privacy", "r21-app"] {
        scenarios.push(scene(name, Page::Settings, 90));
    }
    scenarios.push(scene("r21-search", Page::Settings, 90));
    scenarios.push(scene("r21-no-results", Page::Settings, 90));
    // Lit at its brightest: half a second after it was chosen.
    scenarios.push(scene("r21-jump", Page::Settings, 60));
    scenarios.push(scene("r21-n4n0-page", Page::N4n0, 90));
}
