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

    /// The rows drawn in `card` last frame, by label, for [`registry::claim`].
    fn rows_of(&self, card: &str) -> Vec<(String, Rect)> {
        self.state.seen.iter().filter(|(c, _, _)| *c == card).map(|(_, l, r)| (l.clone(), *r)).collect()
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
        for (card, key, rect) in &self.state.seen_controls {
            if registry::claim(group, card, key, rect.center(), &self.rows_of(card)) == registry::Claim::Nobody {
                out.push(format!("{card} › control {key:?}"));
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
    let rows = h.rows_of(card);
    assert_eq!(registry::claim(Group::Vrchat, card, &key, rect.center(), &rows), registry::Claim::Setting("vrchat.booster"));
    // The same control under another name: nobody claims it, and the card being "the FPS booster" does not excuse it.
    assert_eq!(registry::claim(Group::Vrchat, card, "booster-another-slider", rect.center(), &rows), registry::Claim::Nobody);
    h.state.seen_controls.push((card, "booster-another-slider".into(), rect));
    assert_eq!(h.unclaimed(Group::Vrchat), vec!["FPS booster › control \"booster-another-slider\"".to_string()]);
}

/// And the other way, in both states: with every dependency on, every setting that can show here is drawn where the
/// registry says (a row of it, a control of it, or its whole card); with them off, each setting that needs another is
/// not drawn, the one it needs is, and a search result for it leads there.
#[test]
fn every_setting_of_the_registry_is_drawn_where_it_says() {
    let drawn = |h: &Harness, e: &registry::Entry| {
        let card = h.state.seen_cards.iter().find(|(c, r)| *c == e.card && r.height() > 1.0).map(|(_, r)| *r);
        let row = h.state.seen.iter().any(|(c, label, _)| *c == e.card && e.has_row(label));
        let control = h.state.seen_controls.iter().any(|(c, k, _)| *c == e.card && e.controls.contains(&k.as_str()));
        card.is_some() && (row || control || e.whole)
    };
    let mut h = Harness::new();
    for group in registry::groups(&HERE) {
        h.open(group.id());
        for e in registry::of_group(group, &HERE) {
            assert!(drawn(&h, e), "dependencies on: {} is not drawn in {group:?} › {}", e.id, e.card);
        }
    }
    let mut h = Harness::new();
    h.dependencies_off();
    let mut hidden = 0;
    for group in registry::groups(&HERE) {
        h.open(group.id());
        for e in registry::of_group(group, &HERE) {
            let Some(needs) = e.needs else { continue };
            assert!(!(needs.met)(&h.snap), "{}: its dependency is still met", e.id);
            let why = (
                h.state.seen.iter().filter(|(c, label, _)| *c == e.card && e.has_row(label)).map(|(_, l, _)| l.clone()).collect::<Vec<_>>(),
                h.state.seen_controls.iter().filter(|(c, k, _)| *c == e.card && e.controls.contains(&k.as_str())).map(|(_, k, _)| k.clone()).collect::<Vec<_>>(),
            );
            assert!(!drawn(&h, e), "dependencies off: {} is drawn while {:?} is not so: {why:?}", e.id, needs.when);
            let parent = registry::entry(needs.parent).expect("its parent is an entry");
            assert_eq!(parent.group, e.group, "{}: what shows it is in another sub-tab", e.id);
            assert!(drawn(&h, parent), "dependencies off: {} is hidden, and {} that shows it is not drawn", e.id, parent.id);
            assert_eq!(target_of(e, &h.snap).id, parent.id, "{}: a result for it leads to {}", e.id, parent.id);
            hidden += 1;
        }
    }
    assert!(hidden >= 12, "only {hidden} settings were tested hidden");
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
