//! A community picture's replies and reactions in its window, and pictures dragged out of their windows, on the
//! gallery's community fixture (this computer's pictures standing in for the Discord rooms' pictures, the replies and
//! reactions from `orbiters-core/tests/fixtures/gallery-replies.json` and `gallery-reactions.json`).
//!
//! `gallery-community-talk` (the reactions under the words, the replies after the event), `gallery-community-talk-hover`
//! (who reacted with the first emoji, over its pill), `gallery-community-talk-loading` (the counts and placeholder rows),
//! `gallery-community-talk-error` (Try again), `gallery-community-drag` (the window's picture lifted by a drag out),
//! `gallery-community-drag-wait` (lifted, the bar over the window waiting for it from Orbiters), `gallery-open-drag`
//! (a local picture's item window, its picture lifted).
use super::{Page, Scenario};
use crate::pages::community_talk::{Phase, Said};

const NAMES: [&str; 7] = [
    "gallery-community-talk",
    "gallery-community-talk-hover",
    "gallery-community-talk-loading",
    "gallery-community-talk-error",
    "gallery-community-drag",
    "gallery-community-drag-wait",
    "gallery-open-drag",
];

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let base = scenarios[0].snapshot.clone();
    for name in NAMES {
        scenarios.push(Scenario {
            name,
            page: Page::Sessions,
            snapshot: base.clone(),
            frames: 160,
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

fn said(name: &str) -> Said {
    let replies = serde_json::from_str(include_str!("../../../orbiters-core/tests/fixtures/gallery-replies.json")).expect("replies fixture");
    let reactions = serde_json::from_str(include_str!("../../../orbiters-core/tests/fixtures/gallery-reactions.json")).expect("reactions fixture");
    match name {
        "gallery-community-talk-loading" => Said { replies: Phase::Waiting, reactions: Phase::Waiting },
        "gallery-community-talk-error" => Said {
            replies: Phase::Failed("Orbiters answered 503.".into()),
            reactions: Phase::Failed("Orbiters answered 503.".into()),
        },
        _ => Said { replies: Phase::Ready(replies), reactions: Phase::Ready(reactions) },
    }
}

/// What each scenario does on frame `f` (after the gallery's own staging; the community fixture is staged by
/// `stage_events` for the `gallery-community-…` names).
pub(super) fn stage(ctx: &egui::Context, gallery: &mut crate::pages::gallery::State, name: &str, f: u32) {
    if !NAMES.contains(&name) {
        return;
    }
    if name == "gallery-open-drag" {
        match f {
            70 => gallery.open_card(ctx, 0),
            120 => gallery.stage_window_lift(false),
            _ => {}
        }
        return;
    }
    match f {
        50 => {
            let hover = (name == "gallery-community-talk-hover").then_some(0);
            gallery.community_mut().stage_talk(said(name), hover);
        }
        80 => {
            let shown = gallery.shown().to_vec();
            gallery.community_mut().open_nth(ctx, 0, &shown);
        }
        130 if name == "gallery-community-drag" => gallery.stage_window_lift(false),
        130 if name == "gallery-community-drag-wait" => gallery.stage_window_lift(true),
        _ => {}
    }
}
