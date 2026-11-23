//! Settings in a page of the window, frame by frame, with made-up state (REQUEST21 2): every row it draws is in the
//! registry and every entry of the registry is drawn, the search lists and opens a setting, links land on theirs, the
//! settings moved from the N4N0 page save the same values as there, and each view has one scrolling region.

use super::*;
use crate::backend::{Backend, Ctl};
use crate::ipc::N4n0Command;
use crate::pages::settings_registry::{self as registry, Group, Open, ENTRIES};
use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Vec2};
use orbiters_i18n::{in_language, Language};
use orbiters_ui::toast::Toasts;
use std::sync::{Arc, Mutex};

/// What Settings asked of the app: the settings changed (as patches) and N4N0's commands.
#[derive(Default)]
struct Recorder {
    patches: Mutex<Vec<serde_json::Value>>,
    n4n0: Mutex<Vec<N4n0Command>>,
    /// The worker's log as the service keeps it.
    worker: Mutex<Option<Vec<String>>>,
}

impl Backend for Recorder {
    fn n4n0(&self, command: N4n0Command) {
        self.n4n0.lock().unwrap().push(command);
    }
    fn patch_settings(&self, patch: serde_json::Value) {
        self.patches.lock().unwrap().push(patch);
    }
    fn view(&self) -> Arc<Snapshot> {
        Arc::new(Snapshot::new(Settings::default()))
    }
    fn take_notices(&self) -> Vec<crate::controller::Notice> {
        Vec::new()
    }
    fn library_version(&self) -> u64 {
        0
    }
    fn attach(&self, _ctx: &egui::Context) {}
    fn start_capture(&self) {}
    fn stop_capture(&self) {}
    fn pause_capture(&self, _paused: bool) {}
    fn start_test_capture(&self) {}
    fn import_raw_capture(&self, _dir: PathBuf) {}
    fn trash_session(&self, _dir: PathBuf) {}
    fn clean_up(&self, _trim: Vec<PathBuf>, _delete: Vec<PathBuf>) {}
    fn reconstruct(&self, _dir: PathBuf) {}
    fn stop_reconstruction(&self) {}
    fn unqueue_reconstruction(&self, _dir: PathBuf) {}
    fn send_to_mixer(&self, _dir: PathBuf, _title: String) {}
    fn sign_in(&self) {}
    fn reopen_sign_in(&self) {}
    fn cancel_sign_in(&self) {}
    fn sign_out(&self) {}
    fn request_worker_log(&self) {}
    fn worker_log(&self) -> Option<Vec<String>> {
        self.worker.lock().unwrap().clone()
    }
    fn upload_pictures(&self, _world_id: String, _world_name: String, _paths: Vec<PathBuf>) {}
    fn upload_preview(&self, _world_id: String, _world_name: String, _session: PathBuf) {}
    fn request_maps(&self) {}
    fn maps(&self) -> Option<crate::state::MapsMine> {
        None
    }
    fn calibrated(&self, _calibration: orbiters_contracts::performance::calibration::Calibration) {}
    fn frames(&self, _chunk: crate::state::FrameChunk) {}
    fn headset(&self, _in_use: bool) {}
    fn walk_boost(&self, _strength: f32) {}
    fn osc_reconnect(&self) {}
    fn updates(&self, _action: crate::ipc::UpdateAction) {}
    fn quit(&self) {}
}

/// Settings alone in a window of `size`, its state and what it asked.
struct Harness {
    ctx: egui::Context,
    recorder: Arc<Recorder>,
    ctl: Ctl,
    snap: Snapshot,
    toasts: Toasts,
    state: State,
    time: f64,
    size: Vec2,
    go: Option<super::super::Go>,
}

impl Harness {
    /// Signed in, with every card's switches on that show more rows: the most Settings can draw here.
    fn new() -> Self {
        crate::audio_devices::made_up_mics();
        crate::audio_devices::stand_in(Vec::new(), Vec::new());
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let recorder = Arc::new(Recorder::default());
        let ctl: Ctl = recorder.clone();
        let mut s = Settings::default();
        s.n4n0.afk_message_enabled = true;
        s.n4n0.calls = true;
        s.floating.watch = true;
        s.walk_boost.enabled = true;
        s.orbit.camera_tiles = true;
        s.performance.record_mic = true;
        s.launch.performance = true;
        s.translate.provider = orbiters_core::settings::TranslateProvider::Address;
        let mut snap = Snapshot::new(s);
        // An AI server that offers models (the translator's Model row).
        snap.translate.set(crate::state::TranslateStatus { models: vec!["made-up-model".into()], ..Default::default() });
        snap.account.state = AccountState::SignedIn { username: "TestMember".into() };
        snap.vrchat_login.allowed = true;
        snap.bridge.allowed = true;
        // Calls open to this account (they start as "coming soon", their switch greyed).
        snap.n4n0.calls.link = crate::state::CallLink::Off;
        // The bridge listening: its address, token, controls and recent actions show.
        snap.bridge.listening = true;
        Self { ctx, recorder, ctl, snap, toasts: Toasts::default(), state: State::default(), time: 0.0, size: vec2(1280.0, 820.0), go: None }
    }

    fn frame(&mut self, events: Vec<Event>) -> egui::FullOutput {
        self.time += 1.0 / 60.0;
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)), time: Some(self.time), events, ..Default::default() };
        let Self { ctx, ctl, snap, toasts, state, go, .. } = self;
        let mut output = ctx.run_ui(input, |ui| {
            let mut cx = Cx { ctl, snap, toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 40.0, go: None };
            show(ui, &mut cx, state);
            *go = cx.go;
        });
        // Nothing paints these frames.
        output.textures_delta.clear();
        output
    }

    /// Frames enough for every card to have entered and settled.
    fn settle(&mut self) -> egui::FullOutput {
        let mut out = self.frame(Vec::new());
        for _ in 0..40 {
            out = self.frame(Vec::new());
        }
        out
    }

    fn open(&mut self, target: &str) -> egui::FullOutput {
        assert!(self.state.open(target, self.time), "{target} opens");
        self.settle()
    }

    fn press(&mut self, at: Pos2) {
        self.frame(vec![Event::PointerMoved(at), Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
    }

    fn key(&mut self, key: egui::Key) {
        self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        self.frame(vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE }]);
    }

    /// Where the row labelled `label` of the sub-tab shown was drawn.
    fn row(&self, label: &str) -> Rect {
        self.state.seen.iter().find(|(_, l, _)| l == label).map(|(_, _, r)| *r).unwrap_or_else(|| panic!("no row {label:?} in {:?}", self.state.seen.iter().map(|s| &s.1).collect::<Vec<_>>()))
    }

    fn patches(&self) -> Vec<serde_json::Value> {
        self.recorder.patches.lock().unwrap().clone()
    }
}

/// The gates the harness opens: no window GPU to choose here.
const HERE: Open = Open { bridge: true, vrchat_login: true, gpu_choice: false, gpu_split: false };

impl Harness {
    /// Every switch and choice that hides another off, signed out of nothing else: the other conditional state.
    fn dependencies_off(&mut self) {
        let s = &mut self.snap.settings;
        s.n4n0.afk_message_enabled = false;
        s.n4n0.calls = false;
        s.floating.watch = false;
        s.walk_boost.enabled = false;
        s.orbit.camera_tiles = false;
        s.performance.record_mic = false;
        s.launch.performance = false;
        s.translate.provider = orbiters_core::settings::TranslateProvider::ThisComputer;
        self.snap.account.state = AccountState::SignedOut;
        self.snap.translate.set(crate::state::TranslateStatus::default());
    }

    /// Every row and every control the sub-tab shown drew that no setting claims, and is neither a status row nor an action.
    fn unclaimed(&self, group: Group) -> Vec<String> {
        let mut out = Vec::new();
        for (card, _) in &self.state.seen_cards {
            if !ENTRIES.iter().any(|e| e.group == group && e.card == *card) {
                out.push(format!("the card {card:?} has no entry"));
            }
        }
        for (card, label, _) in &self.state.seen {
            let known = ENTRIES.iter().any(|e| e.group == group && e.card == *card && e.has_row(label)) || registry::is_status_row(card, label);
            if !known {
                out.push(format!("{card} › row {label:?}"));
            }
        }
        for (card, key, _) in &self.state.seen_controls {
            if registry::claim(group, card, key) == registry::Claim::Nobody {
                out.push(format!("{card} › control {key:?}"));
            }
        }
        // A key drawn more often than settings name it is another control under a name already taken (chips and tiles are
        // the several choices of one setting).
        let mut counted = std::collections::BTreeMap::<(&str, &str), usize>::new();
        for ((card, key, _), kind) in self.state.seen_controls.iter().zip(&self.state.seen_kinds) {
            if !matches!(*kind, "chip" | "tile") && matches!(registry::claim(group, card, key), registry::Claim::Setting(_)) {
                *counted.entry((card, key)).or_default() += 1;
            }
        }
        for ((card, key), drawn) in counted {
            let named = ENTRIES.iter().filter(|e| e.group == group && e.card == card && e.controls.contains(&key)).count();
            if drawn > named {
                out.push(format!("{card} › control {key:?} is drawn {drawn} times, named by {named}"));
            }
        }
        out
    }
}

/// The test the registry is held to, in both states of every dependency: each row Settings draws is a setting's or a
/// status row, and each control (toggle, slider, segments, step slider, text field, dropdown, button, chip, tile) is a
/// setting's (in its row, or named by it) or a listed action. A control nobody claims is a setting nobody can search.
#[test]
fn every_row_and_control_drawn_in_settings_belongs_to_a_setting() {
    for off in [false, true] {
        let mut h = Harness::new();
        if off {
            h.dependencies_off();
        }
        for group in registry::groups(&HERE) {
            h.open(group.id());
            assert_eq!(h.state.group(), group);
            assert!(!h.state.seen_controls.is_empty(), "{group:?} draws no control");
            let unclaimed = h.unclaimed(group);
            assert!(unclaimed.is_empty(), "dependencies {}: {group:?}: drawn without a registry entry (pages/settings_registry.rs):\n{}", if off { "off" } else { "on" }, unclaimed.join("\n"));
        }
    }
}

/// A broad card's own control left out of the registry fails the test above: the FPS booster's level slider, claimed by
/// its setting, and the same slider under a name no entry has.
#[test]
fn a_control_missing_from_the_registry_is_caught_on_a_broad_card() {
    let mut h = Harness::new();
    h.open("vrchat");
    let (card, key, rect) = h.state.seen_controls.iter().find(|(_, k, _)| k == "booster-level").cloned().expect("the booster's level slider is drawn and recorded");
    assert_eq!(card, "FPS booster");
    assert_eq!(registry::claim(Group::Vrchat, card, &key), registry::Claim::Setting("vrchat.booster"));
    // The same control under another name: nobody claims it, and the card being "the FPS booster" does not excuse it.
    assert_eq!(registry::claim(Group::Vrchat, card, "booster-another-slider"), registry::Claim::Nobody);
    add_control(&mut h, card, "booster-another-slider", "slider", rect);
    assert_eq!(h.unclaimed(Group::Vrchat), vec!["FPS booster › control \"booster-another-slider\"".to_string()]);
}

/// And the other way, in both states (every dependency on, every dependency off): every setting that can show here is
/// drawn where the registry says (a row of it, a control of it, or its whole card) exactly when what it needs is so; when
/// it is not, the setting it needs is drawn and a search result leads there. No setting is skipped. (Each dependency's
/// other states, mixed ones included, are in `every_dependency_in_each_of_its_states`.)
#[test]
fn every_setting_of_the_registry_is_drawn_where_it_says() {
    let drawn = |h: &Harness, e: &registry::Entry| {
        let card = h.state.seen_cards.iter().find(|(c, r)| *c == e.card && r.height() > 1.0).map(|(_, r)| *r);
        let row = h.state.seen.iter().any(|(c, label, _)| *c == e.card && e.has_row(label));
        let control = h.state.seen_controls.iter().any(|(c, k, _)| *c == e.card && e.controls.contains(&k.as_str()));
        card.is_some() && (row || control || e.whole)
    };
    let mut wrong = Vec::new();
    for off in [false, true] {
        let mut h = Harness::new();
        if off {
            h.dependencies_off();
        }
        let state = if off { "off" } else { "on" };
        // The gates as the page itself opens them in this state (signed out, there is no VRChat account card).
        let open = super::open_of(&h.snap, None);
        for group in registry::groups(&open) {
            h.open(group.id());
            for e in registry::of_group(group, &open) {
                match super::hidden_by(e, &h.snap) {
                    None => {
                        if !drawn(&h, e) {
                            wrong.push(format!("dependencies {state}: {} shows here, and is not drawn in {group:?} › {}", e.id, e.card));
                        }
                    }
                    Some((needs, when)) => {
                        if drawn(&h, e) {
                            wrong.push(format!("dependencies {state}: {} is drawn while {when:?}", e.id));
                        }
                        let parent = registry::entry(needs.parent).expect("its parent is an entry");
                        if parent.group != e.group || !drawn(&h, parent) {
                            wrong.push(format!("dependencies {state}: {} is hidden, and {} that shows it is not drawn beside it", e.id, parent.id));
                        }
                        if target_of(e, &h.snap).id != parent.id {
                            wrong.push(format!("dependencies {state}: a result for {} does not lead to {}", e.id, parent.id));
                        }
                    }
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Major 10's case: Direct calls searched with Calls off. The result says what it needs; Enter opens Calls' sub-tab and
/// lights the Calls switch (not the card); the banner names Direct calls and why it is not there.
#[test]
fn a_hidden_setting_says_what_it_needs_and_lights_its_switch() {
    let mut h = Harness::new();
    h.snap.settings.n4n0.calls = false;
    h.open("account");
    h.state.stage_search("direct calls");
    let texts = super::super::rest::texts(&h.settle());
    assert!(texts.contains("Shown when Calls is on"), "{texts}");
    let id = h.state.search_id.expect("the search field");
    h.ctx.memory_mut(|m| m.request_focus(id));
    h.frame(Vec::new());
    assert_eq!(registry::search("direct calls", &HERE)[0].entry.id, "n4n0.direct-calls");
    h.key(egui::Key::Enter);
    let texts = super::super::rest::texts(&h.settle_a_little_more());
    assert_eq!(h.state.group(), Group::N4n0);
    assert_eq!(h.state.found, Some("n4n0.direct-calls"));
    let jump = h.state.jump.expect("something is lit");
    assert_eq!(jump.id, "n4n0.calls", "the switch that shows it is lit, not its card");
    assert!(jump.lit.is_some(), "and it was found and brought into view");
    assert!(texts.contains("Shown when Calls is on"), "the banner says why: {texts}");
    // A link to it does the same.
    let mut h = Harness::new();
    h.snap.settings.n4n0.calls = false;
    h.open("n4n0.direct-calls");
    assert_eq!(h.state.jump.map(|j| j.id), Some("n4n0.calls"));
    // With Calls on, Direct calls itself.
    let mut h = Harness::new();
    h.open("n4n0.direct-calls");
    assert_eq!(h.state.jump.map(|j| j.id), Some("n4n0.direct-calls"));
}

/// N4N0's settings are no longer drawn on the N4N0 page: each is in Settings once.
#[test]
fn n4n0_settings_left_the_n4n0_page() {
    let source = include_str!("n4n0.rs");
    for gone in ["\"n4n0-afk\"", "\"n4n0-carry\"", "\"n4n0-controller-input\"", "\"n4n0-menu-button\"", "\"n4n0-wake\"", "\"n4n0-mute\"", "\"n4n0-ai\"", "\"n4n0-enabled\"", "\"n4n0-calls\"", "\"n4n0-direct-calls\"", "\"n4n0-test-step\"", "\"n4n0-bot\"", "\"n4n0-screen-size\""] {
        assert!(!source.contains(gone), "{gone} is still on the N4N0 page");
        assert!(include_str!("settings_n4n0.rs").contains(gone), "{gone} is not in Settings");
    }
}

/// The search lists what matches with its sub-tab; the arrows and Enter open one: its sub-tab shows, the search
/// clears, the setting comes into view and is lit. Escape clears the search.
#[test]
fn the_search_opens_a_setting_and_lights_it() {
    let mut h = Harness::new();
    h.open("account");
    h.state.stage_search("wake word");
    let out = h.settle();
    let texts = super::super::rest::texts(&out);
    assert!(texts.contains("matching settings") || texts.contains("matching setting"), "{texts}");
    assert!(texts.contains(Group::Voice.title()), "the result says its sub-tab: {texts}");
    // The search field has the keys: down once (to "Say Nano"'s or the next), back up, Enter.
    let id = h.state.search_id.expect("the search field");
    h.ctx.memory_mut(|m| m.request_focus(id));
    h.frame(Vec::new());
    h.key(egui::Key::ArrowDown);
    assert_eq!(h.state.active, 1);
    h.key(egui::Key::ArrowUp);
    assert_eq!(h.state.active, 0);
    let first = registry::search("wake word", &HERE)[0].entry;
    h.key(egui::Key::Enter);
    h.settle_a_little();
    assert_eq!(h.state.group(), first.group, "its sub-tab shows");
    assert!(h.state.query.is_empty(), "the search gave way");
    assert_eq!(h.state.found, Some(first.id), "said above the sub-tab");
    let lit = h.state.jump.and_then(|j| j.lit).expect("brought into view and lit");
    assert!(glow(h.time - lit) > 0.5);
    // Lit briefly: gone after three seconds.
    for _ in 0..200 {
        h.frame(Vec::new());
    }
    assert!(h.state.jump.is_none(), "the light went out");
    // Escape clears what is typed.
    h.state.stage_search("micro");
    h.frame(Vec::new());
    h.ctx.memory_mut(|m| m.request_focus(id));
    h.frame(Vec::new());
    h.key(egui::Key::Escape);
    assert!(h.state.query.is_empty());
}

impl Harness {
    fn settle_a_little(&mut self) {
        for _ in 0..12 {
            self.frame(Vec::new());
        }
    }
}

/// No result: what to try, and a way back.
#[test]
fn no_result_says_what_to_try() {
    let mut h = Harness::new();
    h.state.stage_search("qqqq zzzz");
    let texts = super::super::rest::texts(&h.settle());
    assert!(texts.contains("No matching settings"), "{texts}");
    assert!(texts.contains("Clear the search"));
}

/// Every link into Settings lands on its setting, in its sub-tab, and lights it.
#[test]
fn links_land_on_their_setting() {
    use super::super::Go;
    let mut h = Harness::new();
    for (go, id) in [(Go::VrchatAccount, "account.vrchat"), (Go::ExplorerPoints, "account.explorer-points"), (Go::Settings("n4n0.calls"), "n4n0.calls"), (Go::Settings("privacy.usage"), "privacy.usage")] {
        assert_eq!(go.setting(), Some(id));
        h.open(id);
        let entry = registry::entry(id).unwrap();
        assert_eq!(h.state.group(), entry.group, "{id}");
        assert!(h.state.jump.and_then(|j| j.lit).is_some(), "{id} was found and lit");
    }
    assert_eq!(Go::FreeSpace.setting(), None);
    // A sub-tab by its name, and nothing for what is none.
    assert!(h.state.open("vr", 0.0));
    assert_eq!(h.state.group(), Group::Vr);
    assert!(!h.state.open("no.such.setting", 0.0));
    // The N4N0 page's Settings button and its Calls card.
    assert_eq!(Go::Settings("n4n0").setting().and_then(Group::of_id), Some(Group::N4n0));
    // N4N0's assistant opens the window's Settings and N4N0 pages as before.
    assert_eq!(crate::controller::assistant::window_page("page:Settings"), Some("Settings"));
    assert_eq!(crate::controller::assistant::window_page("page:N4n0"), Some("N4n0"));
    // Every entry can be linked to.
    for e in ENTRIES {
        assert!(State::default().open(e.id, 0.0), "{}", e.id);
    }
}

/// The settings that moved from the N4N0 page save the same values as there: the same switch writes the same field.
#[test]
fn moved_settings_save_the_same_values() {
    let mut h = Harness::new();
    // Tall enough for every row of a sub-tab to be on screen.
    h.size = vec2(1280.0, 2600.0);
    let defaults = Settings::default();
    for (group, label, path, was) in [
        ("voice", "Wake word", "/n4n0/wake_word", defaults.n4n0.wake_word),
        ("voice", "Mute VRChat while I hold Talk", "/n4n0/mute_while_talking", defaults.n4n0.mute_while_talking),
        ("voice", "AI assistant", "/n4n0/assistant", defaults.n4n0.assistant),
        ("n4n0", "N4N0", "/n4n0/enabled", defaults.n4n0.enabled),
        ("n4n0", "Controller input", "/n4n0/controller_input", defaults.n4n0.controller_input),
        ("n4n0", "Controller button", "/n4n0/menu/button", defaults.n4n0.menu.button),
        ("n4n0", "Touch N4N0", "/n4n0/menu/touch", defaults.n4n0.menu.touch),
        ("n4n0", "Calls", "/n4n0/calls", true),
        ("app", "Reduce motion", "/reduce_motion", defaults.reduce_motion),
    ] {
        h.open(group);
        h.recorder.patches.lock().unwrap().clear();
        let row = h.row(label);
        // The switch is at the row's right end.
        h.press(pos2(row.right() - 22.0, row.center().y));
        let patches = h.patches();
        let value = patches.iter().find_map(|p| p.pointer(path).cloned());
        assert_eq!(value, Some(serde_json::Value::Bool(!was)), "{label}: {patches:?}");
    }
}

/// One region scrolls on each view (the owner's rule: never a scroll area inside another): Settings' page, and none
/// in any card it draws.
#[test]
fn each_view_has_one_scrolling_region() {
    let count = |source: &str| source.matches("ScrollArea::").count();
    assert_eq!(count(include_str!("settings.rs")), 1, "Settings scrolls once");
    for (name, source) in [
        ("settings_n4n0", include_str!("settings_n4n0.rs")),
        ("experimental", include_str!("experimental.rs")),
        ("beta_feedback", include_str!("beta_feedback.rs")),
        ("booster_card", include_str!("booster_card.rs")),
        ("translate_card", include_str!("translate_card.rs")),
        ("launch_card", include_str!("launch_card.rs")),
        ("vr_audio", include_str!("vr_audio.rs")),
        ("network", include_str!("network.rs")),
        ("mic_grid", include_str!("mic_grid.rs")),
        ("language_card", include_str!("language_card.rs")),
        ("perf_card", include_str!("perf_card.rs")),
        ("mic", include_str!("mic.rs")),
        ("form", include_str!("form.rs")),
    ] {
        assert_eq!(count(source), 0, "{name} has a scroll area inside Settings' own");
    }
    assert_eq!(count(include_str!("n4n0.rs")), 1, "the N4N0 page scrolls once");
}

/// In a window 900 wide the sub-tabs are a row and every row of every card stays inside the page.
#[test]
fn a_narrow_window_keeps_every_row_inside() {
    let mut h = Harness::new();
    h.size = vec2(900.0, 700.0);
    for group in registry::groups(&HERE) {
        h.open(group.id());
        for (card, label, r) in &h.state.seen {
            assert!(r.left() >= 0.0 && r.right() <= 900.0 - 24.0, "{group:?} › {card} › {label}: {r:?}");
        }
    }
}

/// In French and Japanese the sub-tabs, the cards and the results are in the language, and the search finds a setting
/// by its own name there, accents or not.
#[test]
fn settings_speak_the_language() {
    for language in [Language::Fr, Language::Ja] {
        in_language(language, || {
            let mut h = Harness::new();
            let texts = super::super::rest::texts(&h.open("voice"));
            assert!(texts.contains(Group::Voice.title()), "{language:?}: {texts}");
            assert!(!texts.contains("Voice & assistant"), "{language:?}");
            let wake = orbiters_i18n::tr("Wake word");
            h.state.stage_search(wake);
            let texts = super::super::rest::texts(&h.settle());
            assert!(texts.contains(wake), "{language:?}: {texts}");
        });
    }
}

/// REQUEST21 (Enzo: "a reset button for the smoothing values to put them back to the default ones that have been measured
/// to be matching vrchat the most"): Reset under the two sliders writes both back to the measured values (the defaults),
/// as the sliders write them, says so briefly, and is greyed once both are there.
#[cfg(not(target_os = "macos"))]
#[test]
fn reset_puts_the_menus_smoothing_back_to_the_values_measured_with_vrchat() {
    use orbiters_core::settings::OrbitSettings;
    orbiters_ui::widgets::watch_controls();
    let mut h = Harness::new();
    h.snap.settings.orbit.vr_smoothing_quick_menu_ms = 60.0;
    h.snap.settings.orbit.vr_smoothing_wheel_ms = 5.0;
    // Brought into view as the search would (it is under the card's other rows).
    h.open("vr.smoothing-reset");
    h.row("Reset the smoothing");
    let (r, on) = orbiters_ui::widgets::control(&h.ctx, "Reset").expect("Reset is drawn");
    assert!(on, "off the measured values: it takes a press");
    h.press(r.center());
    h.settle();
    let (quick, wheel) = OrbitSettings::measured_smoothing();
    let patch = h.patches().last().cloned().expect("a change written");
    assert_eq!(patch["orbit"]["vr_smoothing_quick_menu_ms"].as_f64(), Some(quick as f64), "{patch}");
    assert_eq!(patch["orbit"]["vr_smoothing_wheel_ms"].as_f64(), Some(wheel as f64), "{patch}");
    let (patched, left_out) = (*h.snap.settings).clone().patched(&patch);
    assert!(left_out.is_empty() && patched.orbit.smoothing_as_measured(), "{left_out:?}");
    // The service writes it: at the measured values, Reset is greyed.
    h.snap.settings.orbit.vr_smoothing_quick_menu_ms = quick;
    h.snap.settings.orbit.vr_smoothing_wheel_ms = wheel;
    h.settle();
    let (_, on) = orbiters_ui::widgets::control(&h.ctx, "Reset").expect("Reset is drawn");
    assert!(!on, "both at the measured values: nothing to reset");
}

impl Harness {
    fn settle_a_little_more(&mut self) -> egui::FullOutput {
        let mut out = self.frame(Vec::new());
        for _ in 0..20 {
            out = self.frame(Vec::new());
        }
        out
    }

    /// Moves the pointer to `at` (from just beside it, a real move), then leaves it there.
    fn park_pointer(&mut self, at: Pos2) {
        self.frame(vec![Event::PointerMoved(at - vec2(6.0, 0.0))]);
        self.frame(vec![Event::PointerMoved(at)]);
    }
}

/// Minor 1: a pointer left over a result never takes the lit result back from the arrows, and Enter opens the one the
/// arrows chose.
#[test]
fn a_parked_pointer_does_not_take_the_arrows_selection_back() {
    let mut h = Harness::new();
    h.open("account");
    h.state.stage_search("micro");
    h.settle();
    let found = registry::search("micro", &HERE);
    assert!(found.len() >= 3, "{}", found.len());
    // The pointer comes to rest over the third result: it lights it, as a move does.
    let third = h.state.result_rects[2];
    h.park_pointer(third.center());
    h.settle_a_little();
    assert_eq!(h.state.active, 2, "a pointer moving onto a result lights it");
    // The arrows take it from there, and keep it while the pointer stays where it is.
    let id = h.state.search_id.expect("the search field");
    h.ctx.memory_mut(|m| m.request_focus(id));
    h.frame(Vec::new());
    h.key(egui::Key::ArrowUp);
    assert_eq!(h.state.active, 1);
    h.settle_a_little();
    assert_eq!(h.state.active, 1, "the parked pointer took the selection back");
    h.key(egui::Key::Enter);
    h.settle_a_little();
    assert_eq!(h.state.found, Some(found[1].entry.id), "Enter opened the result the arrows chose");
}

/// Minor 1: the arrows bring the lit result into view in a short window.
#[test]
fn the_arrows_bring_the_lit_result_into_view() {
    let mut h = Harness::new();
    h.size = vec2(1280.0, 520.0);
    h.open("account");
    h.state.stage_search("vrchat");
    h.settle();
    let shown = h.state.result_rects.len();
    assert!(shown >= 8, "{shown} results");
    let id = h.state.search_id.expect("the search field");
    h.ctx.memory_mut(|m| m.request_focus(id));
    h.frame(Vec::new());
    for _ in 0..shown - 1 {
        h.key(egui::Key::ArrowDown);
    }
    h.settle_a_little();
    assert_eq!(h.state.active, shown - 1);
    let r = h.state.result_rects[shown - 1];
    assert!(r.bottom() <= 520.0 + 0.5 && r.top() >= 0.0, "the last result, lit by the arrows, is out of view: {r:?}");
}

/// The press of the button whose key starts with `start` among the controls drawn last frame.
fn press_control(h: &mut Harness, start: &str) -> bool {
    let Some((_, _, r)) = h.state.seen_controls.iter().find(|(_, k, _)| k.starts_with(start)).cloned() else { return false };
    h.press(r.center());
    h.settle_a_little();
    true
}

/// Minor 2: every line the service keeps of the worker's log can be reached: the last ones at once, older ones a press
/// of Show more at a time, in the page's one scroll.
#[test]
fn every_worker_log_line_is_reachable() {
    let mut h = Harness::new();
    h.size = vec2(1280.0, 6000.0);
    *h.recorder.worker.lock().unwrap() = Some((1..=400).map(|i| format!("worker line {i}")).collect());
    h.state.log_open = true;
    let texts = super::super::rest::texts(&h.open("app.about"));
    assert!(texts.contains("worker line 400") && texts.contains("worker line 361"), "the last lines");
    assert!(!texts.contains("worker line 360\n"));
    let mut presses = 0;
    while press_control(&mut h, "Show more (") {
        presses += 1;
        assert!(presses < 10);
    }
    let texts = super::super::rest::texts(&h.settle());
    assert!(texts.contains("worker line 1\n"), "the oldest line is reachable");
    assert!(presses >= 4, "{presses} presses");
}

/// Minor 2: every action the bridge keeps can be reached the same way.
#[test]
fn every_bridge_action_is_reachable() {
    let mut h = Harness::new();
    h.size = vec2(1280.0, 8000.0);
    h.snap.bridge.log = (1..=80).map(|i| crate::bridge::LogLine { at: 1_795_000_000 + i, text: format!("made-up action {i}"), ok: true }).collect();
    let texts = super::super::rest::texts(&h.open("vrchat.bridge"));
    assert!(texts.contains("made-up action 80"), "the last action: {texts}");
    assert!(!texts.contains("made-up action 1\n"), "the last actions only");
    let mut presses = 0;
    while press_control(&mut h, "Show more (") {
        presses += 1;
        assert!(presses < 10);
    }
    let texts = super::super::rest::texts(&h.settle());
    assert!(texts.contains("made-up action 1\n"), "the oldest action is reachable");
}

/// At the smallest window (1040 wide, the sidebar's 232 points off: 808 for the page), in English and French, every
/// row leaves its text the room a row needs beside its control, every control stays in the room its row gave it, and
/// nothing goes past the page.
#[test]
fn every_control_fits_at_the_smallest_window() {
    let mut wrong = Vec::new();
    for language in [Language::En, Language::Fr] {
        in_language(language, || {
            let mut h = Harness::new();
            h.size = vec2(808.0, 700.0);
            for group in registry::groups(&HERE) {
                h.open(group.id());
                for (label, control, text_w) in &h.state.seen_fields {
                    if *text_w < 120.0 {
                        wrong.push(format!("{language:?} {group:?}: the row {label:?} leaves its text {text_w:.0} pt beside its control"));
                    }
                    for (_, key, r) in h.state.seen_controls.iter().filter(|(_, _, r)| control.contains(r.center())) {
                        if r.left() < control.left() - 0.5 {
                            wrong.push(format!("{language:?} {group:?}: {key:?} in {label:?} runs {:.0} pt over its row's text", control.left() - r.left()));
                        }
                    }
                }
                for (card, key, r) in &h.state.seen_controls {
                    if r.left() < 0.0 || r.right() > 808.0 - 24.0 {
                        wrong.push(format!("{language:?} {group:?} › {card}: {key:?} goes past the page: {r:?}"));
                    }
                }
            }
        });
    }
    assert!(wrong.is_empty(), "{} controls do not fit:\n{}", wrong.len(), wrong.join("\n"));
}

// ---- verify2-r21 m1 / Major 10: every dependency in each of its states; persisting controls; rows with two settings ----

/// What hides `e` here, said in English, or nothing when it shows.
fn hint_of(e: &registry::Entry, snap: &Snapshot) -> Option<&'static str> {
    super::hidden_by(e, snap).map(|(_, when)| when)
}

/// The settings that need another to show, every one of them, by id: the count is exact, so a dependency added or lost
/// without its states below fails here.
const DEPENDENT: [&str; 17] = [
    "account.explorer-points",
    "n4n0.afk-message",
    "n4n0.direct-calls",
    "vr.camera-find",
    "vr.watch-hand",
    "vr.walk-strength",
    "vr.walk-driver",
    "capture.voice-mic",
    "vrchat.priority",
    "vrchat.main-thread",
    "vrchat.frame-cap",
    "vrchat.video-decoding",
    "vrchat.tiny-window",
    "vrchat.perf-hint-mute",
    "translate.server",
    "translate.model",
    "translate.orbiters-agreement",
];

#[test]
fn the_settings_that_need_another_are_exactly_these() {
    let mut declared: Vec<&str> = ENTRIES.iter().filter(|e| e.needs.is_some()).map(|e| e.id).collect();
    declared.sort_unstable();
    let mut expected = DEPENDENT.to_vec();
    expected.sort_unstable();
    assert_eq!(declared, expected, "{} declared", declared.len());
}

/// One state of a dependency: its name, how to put the made-up state in it, and whether the setting shows then (with
/// the English of what the search says while it does not).
type DepState = (&'static str, fn(&mut Harness), Option<&'static str>);

fn signed_in(h: &mut Harness) {
    h.snap.account.state = AccountState::SignedIn { username: "TestMember".into() };
}

/// Every dependency, in each of its states: the account's (signed in, out, offline, expired), calls not open to this
/// account with Calls on or off, each translator with and without models, an agreement given or not, a hint or none.
fn dependency_states() -> Vec<(&'static str, Vec<DepState>)> {
    use orbiters_core::settings::TranslateProvider as P;
    vec![
        (
            "account.explorer-points",
            vec![
                ("signed in", signed_in as fn(&mut Harness), None),
                ("offline, known", |h| h.snap.account.state = AccountState::Offline { username: Some("TestMember".into()) }, None),
                ("signed out", |h| h.snap.account.state = AccountState::SignedOut, Some("Shown once you are signed in to Orbiters")),
                ("sign-in expired", |h| h.snap.account.state = AccountState::Expired { username: Some("TestMember".into()) }, Some("Shown once you are signed in to Orbiters")),
            ],
        ),
        ("n4n0.afk-message", vec![("AFK on", |h| h.snap.settings.n4n0.afk_message_enabled = true, None), ("AFK off", |h| h.snap.settings.n4n0.afk_message_enabled = false, Some("Shown when AFK message is on"))]),
        (
            "n4n0.direct-calls",
            vec![
                ("calls open, on", |h| { h.snap.n4n0.calls.link = crate::state::CallLink::Off; h.snap.settings.n4n0.calls = true; }, None),
                ("calls open, off", |h| { h.snap.n4n0.calls.link = crate::state::CallLink::Off; h.snap.settings.n4n0.calls = false; }, Some("Shown when Calls is on")),
                // Not open to this account: Calls is on and greyed, nothing to turn on.
                ("not open to the account, Calls on", |h| { h.snap.n4n0.calls.link = crate::state::CallLink::ComingSoon; h.snap.settings.n4n0.calls = true; }, Some("Coming soon: calls aren't open for your account yet")),
                ("not open to the account, Calls off", |h| { h.snap.n4n0.calls.link = crate::state::CallLink::ComingSoon; h.snap.settings.n4n0.calls = false; }, Some("Coming soon: calls aren't open for your account yet")),
            ],
        ),
        ("vr.camera-find", vec![("tiles on", |h| h.snap.settings.orbit.camera_tiles = true, None), ("tiles off", |h| h.snap.settings.orbit.camera_tiles = false, Some("Shown when Around the camera is on"))]),
        ("vr.watch-hand", vec![("watch on", |h| h.snap.settings.floating.watch = true, None), ("watch off", |h| h.snap.settings.floating.watch = false, Some("Shown when Orbiters watch is on"))]),
        ("vr.walk-strength", vec![("walk on", |h| h.snap.settings.walk_boost.enabled = true, None), ("walk off", |h| h.snap.settings.walk_boost.enabled = false, Some("Shown when Walk boost is on"))]),
        ("vr.walk-driver", vec![("walk on", |h| h.snap.settings.walk_boost.enabled = true, None), ("walk off", |h| h.snap.settings.walk_boost.enabled = false, Some("Shown when Walk boost is on"))]),
        ("capture.voice-mic", vec![("voice on", |h| h.snap.settings.performance.record_mic = true, None), ("voice off", |h| h.snap.settings.performance.record_mic = false, Some("Shown when Your voice is on"))]),
        ("vrchat.priority", vec![("options on", |h| h.snap.settings.launch.performance = true, None), ("options off", |h| h.snap.settings.launch.performance = false, Some("Shown when Performance launch options is on"))]),
        ("vrchat.main-thread", vec![("options on", |h| h.snap.settings.launch.performance = true, None), ("options off", |h| h.snap.settings.launch.performance = false, Some("Shown when Performance launch options is on"))]),
        ("vrchat.frame-cap", vec![("options on", |h| h.snap.settings.launch.performance = true, None), ("options off", |h| h.snap.settings.launch.performance = false, Some("Shown when Performance launch options is on"))]),
        ("vrchat.video-decoding", vec![("options on", |h| h.snap.settings.launch.performance = true, None), ("options off", |h| h.snap.settings.launch.performance = false, Some("Shown when Performance launch options is on"))]),
        ("vrchat.tiny-window", vec![("options on", |h| h.snap.settings.launch.performance = true, None), ("options off", |h| h.snap.settings.launch.performance = false, Some("Shown when Performance launch options is on"))]),
        (
            "vrchat.perf-hint-mute",
            vec![
                ("a hint", |h| { h.snap.perf.set(crate::perf::PerfView { hint: Some(crate::perf::Hint { key: "reconstruct_gpu".into(), part: None }), report: None }); }, None),
                ("no hint", |h| { h.snap.perf.set(crate::perf::PerfView::default()); }, Some("Shown while N4N0 has a performance hint for you")),
            ],
        ),
        (
            "translate.server",
            vec![
                ("AI server", |h| h.snap.settings.translate.provider = P::Address, None),
                ("this computer", |h| h.snap.settings.translate.provider = P::ThisComputer, Some("Shown when Translator is “AI server”")),
                ("Orbiters", |h| h.snap.settings.translate.provider = P::Orbiters, Some("Shown when Translator is “AI server”")),
            ],
        ),
        (
            "translate.model",
            vec![
                ("AI server with models", |h| { h.snap.settings.translate.provider = P::Address; models(h, true); }, None),
                ("this computer with models", |h| { h.snap.settings.translate.provider = P::ThisComputer; models(h, true); }, None),
                ("AI server, no model yet", |h| { h.snap.settings.translate.provider = P::Address; models(h, false); }, Some("Shown once the AI app or server offers models")),
                ("this computer, no model yet", |h| { h.snap.settings.translate.provider = P::ThisComputer; models(h, false); }, Some("Shown once the AI app or server offers models")),
                ("Orbiters, models listed", |h| { h.snap.settings.translate.provider = P::Orbiters; models(h, true); }, Some("Not with Orbiters as the translator")),
            ],
        ),
        (
            "translate.orbiters-agreement",
            vec![
                ("Orbiters, not agreed", |h| { h.snap.settings.translate.provider = P::Orbiters; agreed(h, false); }, None),
                ("Orbiters, agreed", |h| { h.snap.settings.translate.provider = P::Orbiters; agreed(h, true); }, Some("Already agreed to")),
                ("AI server", |h| { h.snap.settings.translate.provider = P::Address; agreed(h, false); }, Some("Shown when Translator is “Orbiters”")),
                ("this computer", |h| { h.snap.settings.translate.provider = P::ThisComputer; agreed(h, false); }, Some("Shown when Translator is “Orbiters”")),
            ],
        ),
    ]
}

fn models(h: &mut Harness, some: bool) {
    let models = if some { vec!["made-up-model".to_string()] } else { Vec::new() };
    h.snap.translate.set(crate::state::TranslateStatus { models, ..Default::default() });
}

fn agreed(h: &mut Harness, yes: bool) {
    h.snap.settings.translate.orbiters_agreed = yes;
    h.snap.settings.translate.orbiters_agreed_window = yes;
}

/// Each dependency in each of its states: shown exactly when its condition holds; while it does not, not drawn, the
/// search says the condition that is actually unmet, and a result leads to (and lights) the setting that shows it.
#[test]
fn every_dependency_in_each_of_its_states() {
    let states = dependency_states();
    let mut covered: Vec<&str> = states.iter().map(|(id, _)| *id).collect();
    covered.sort_unstable();
    let mut all = DEPENDENT.to_vec();
    all.sort_unstable();
    assert_eq!(covered, all, "every dependency has its states here");
    let mut wrong = Vec::new();
    for (id, cases) in states {
        let Some(e) = registry::entry(id) else {
            wrong.push(format!("{id} is no entry"));
            continue;
        };
        for (name, set, said) in cases {
            let mut h = Harness::new();
            set(&mut h);
            h.open(e.group.id());
            let drawn = h.state.seen.iter().any(|(c, label, _)| *c == e.card && e.has_row(label))
                || h.state.seen_controls.iter().any(|(c, k, _)| *c == e.card && e.controls.contains(&k.as_str()));
            let hint = hint_of(e, &h.snap);
            if hint != said {
                wrong.push(format!("{id} ({name}): the search says {hint:?}, expected {said:?}"));
            }
            if drawn != said.is_none() {
                wrong.push(format!("{id} ({name}): drawn {drawn}, expected {}", said.is_none()));
            }
            if said.is_some() {
                let parent = e.needs.map(|n| n.parent).unwrap_or_default();
                let target = target_of(e, &h.snap).id;
                if target != parent {
                    wrong.push(format!("{id} ({name}): a result leads to {target}, not {parent}"));
                }
                // The setting that shows it is drawn, to be lit.
                let p = registry::entry(parent).expect("parent");
                let shown = h.state.seen.iter().any(|(c, label, _)| *c == p.card && p.has_row(label))
                    || h.state.seen_controls.iter().any(|(c, k, _)| *c == p.card && p.controls.contains(&k.as_str()))
                    || (p.whole && h.state.seen_cards.iter().any(|(c, _)| *c == p.card));
                if !shown {
                    wrong.push(format!("{id} ({name}): {parent}, which shows it, is not drawn"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{} wrong:\n{}", wrong.len(), wrong.join("\n"));
}

/// Controls that keep a choice are settings, found by the search, never "actions that change no setting": Don't tell
/// me again (a hint muted for good) and I agree (the agreement for translation by Orbiters).
#[test]
fn controls_that_keep_a_choice_are_settings() {
    for (card, key, id) in [
        ("Performance report", "Don't tell me again", "vrchat.perf-hint-mute"),
        ("Translation in VR", "I agree", "translate.orbiters-agreement"),
        // The booster's question about its last level: both answers write the level.
        ("FPS booster", "Show me how", "vrchat.booster"),
        ("FPS booster", "Stay on Picture", "vrchat.booster"),
    ] {
        assert!(!registry::is_action(card, key), "{key:?} keeps a choice: it is no action");
        let e = registry::entry(id).unwrap_or_else(|| panic!("{id} is no entry"));
        assert_eq!(registry::claim(e.group, card, key), registry::Claim::Setting(id));
        assert!(registry::search(&e.shown(e.title).to_lowercase(), &HERE).iter().any(|f| f.entry.id == id), "{id} is found by its name");
    }
    assert!(registry::search("don't tell me again", &HERE).iter().any(|f| f.entry.id == "vrchat.perf-hint-mute"));
    assert!(registry::search("agree orbiters", &HERE).iter().any(|f| f.entry.id == "translate.orbiters-agreement"));
}

/// Any control put inside a registered row needs its own entry: beside the row's own switch, in a row with only a button,
/// in a status row, a button that is no known action, or the row's own key a second time.
#[test]
fn another_control_inside_a_registered_row_needs_its_own_entry() {
    let mut missed = Vec::new();
    for (group, card, label, key, kind) in [
        ("voice", "N4N0's ears", "Wake word", "n4n0-wake-sensitivity", "slider"),
        ("voice", "N4N0's ears", "Wake word", "Keep listening", "button"),
        ("n4n0", "N4N0's menu", "Try it", "n4n0-menu-try-often", "toggle"),
        ("n4n0", "N4N0's menu", "Keyboard", "n4n0-menu-keyboard", "segments"),
        ("capture", "VRChat and OSC", "VRChat log", "log-keep", "toggle"),
        ("vr", "Around VRChat's menus", "Right now", "right-now-often", "toggle"),
        ("app", "About", "Updates", "update-channel", "segments"),
    ] {
        let mut h = Harness::new();
        h.open(group);
        let shown = h.state.group();
        assert!(h.unclaimed(shown).is_empty(), "{group}: {:?}", h.unclaimed(shown));
        let row = h.row(label);
        add_control(&mut h, card, key, kind, Rect::from_center_size(row.center(), vec2(48.0, 28.0)));
        if !h.unclaimed(shown).iter().any(|u| u.contains(key)) {
            missed.push(format!("{card} › {label}: {kind} {key:?}"));
        }
    }
    assert!(missed.is_empty(), "not caught:\n{}", missed.join("\n"));
}

/// Every key the registry names is drawn somewhere: an id salt or a button's label in the cards' own source (a key with a
/// slip of the pen would claim nothing, in a state these tests do not draw).
#[test]
fn every_key_the_registry_names_is_in_the_cards_source() {
    let sources = [
        include_str!("settings.rs"),
        include_str!("settings_n4n0.rs"),
        include_str!("vr_audio.rs"),
        include_str!("mic.rs"),
        include_str!("mic_grid.rs"),
        include_str!("form.rs"),
        include_str!("booster_card.rs"),
        include_str!("launch_card.rs"),
        include_str!("translate_card.rs"),
        include_str!("language_card.rs"),
        include_str!("beta_feedback.rs"),
        include_str!("experimental.rs"),
        include_str!("network.rs"),
        include_str!("perf_card.rs"),
    ]
    .concat();
    let named = ENTRIES.iter().flat_map(|e| e.controls.iter().copied()).chain(registry::ACTIONS.iter().map(|(_, k)| *k));
    let lost: Vec<&str> = named
        .filter(|key| match key.strip_suffix('*') {
            Some(start) => !sources.contains(&format!("\"{start}")),
            None => !sources.contains(&format!("\"{key}\"")),
        })
        .collect();
    assert!(lost.is_empty(), "{lost:?}");
}

/// Tests: a control as if drawn at `rect` in `card` (a second setting slipped into a row).
fn add_control(h: &mut Harness, card: &'static str, key: &str, kind: &'static str, rect: Rect) {
    h.state.seen_controls.push((card, key.to_owned(), rect));
    h.state.seen_kinds.push(kind);
}
