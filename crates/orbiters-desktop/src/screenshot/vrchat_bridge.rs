//! `--only settings-experimental…`: Settings' Experimental section as the owner sees it (off, running with a client and a log,
//! paused) and as everybody else sees it (not at all). Synthetic: nothing listens, and the token is a stand-in.

use super::{Page, Scenario};
use crate::bridge::person::Paused;
use crate::bridge::{ClientSeen, LogLine, Status};
use crate::state::{AccountState, Snapshot};

fn log() -> Vec<LogLine> {
    let now = orbiters_core::visited::now();
    let line = |back: u64, ok: bool, text: &str| LogLine { at: now - back, text: text.to_string(), ok };
    vec![
        line(640, true, "Listening on 127.0.0.1:47615."),
        line(420, true, "go to wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b"),
        line(380, true, "screenshot"),
        line(372, false, "press esc: not_foreground"),
        line(360, true, "bring VRChat forward"),
        line(355, true, "press esc"),
        line(350, true, "mouse click at 148, 212"),
        line(330, true, "turn about 45°"),
        line(300, true, "walk forward 4.0 s"),
        line(260, true, "3D picture of 8 s"),
        line(120, true, "walk forward 2.5 s"),
    ]
}

fn scenario(name: &'static str, snapshot: Snapshot) -> Scenario {
    Scenario {
        name,
        page: Page::Settings,
        snapshot,
        frames: 90,
        splash: false,
        open_detail: false,
        toasts: vec![],
        pointer: None,
        scroll: 0.0,
        world: None,
        home: None,
        library: None,
    }
}

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let mut base = scenarios[0].snapshot.clone();
    base.account.state = AccountState::SignedIn { username: "Enzo".into() };
    let running = Status {
        allowed: true,
        listening: true,
        address: Some("http://127.0.0.1:47615/mcp".into()),
        token: Some("0123456789abcdef0123456789abcdef".into()),
        client: Some(ClientSeen { name: "claude-code".into(), connected: true }),
        acting: true,
        log: log(),
        ..Default::default()
    };

    // The owner, the switch off: the section and what it means, nothing listening.
    let mut off = base.clone();
    off.bridge = Status { allowed: true, ..Default::default() };
    scenarios.push(scenario("settings-experimental-owner-off", off));

    let mut on = base.clone();
    on.settings.experimental.vrchat_bridge = true;
    on.bridge = running.clone();
    scenarios.push(scenario("settings-experimental-owner", on.clone()));

    let mut paused = on.clone();
    paused.bridge.paused = Some(Paused::Person { resumes_in_s: 0 });
    paused.bridge.acting = false;
    scenarios.push(scenario("settings-experimental-paused", paused));

    let mut held = on;
    held.bridge.paused = Some(Paused::Held);
    held.bridge.acting = false;
    held.bridge.client = None;
    held.bridge.log = vec![];
    held.bridge.hotkey_taken = true;
    scenarios.push(scenario("settings-experimental-held", held));

    // Anybody else: even with the setting on in their file, there is no section, as the server did not say yes.
    let mut member = base;
    member.settings.experimental.vrchat_bridge = true;
    member.bridge = Status::default();
    scenarios.push(scenario("settings-experimental-member", member));
}
