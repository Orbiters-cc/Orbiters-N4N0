use super::layout::{self, Bar, Hit, Strip};
use super::morph::{Morph, FADE};
use super::*;
use crate::app::App;
use crate::backend::Backend;
use crate::pages::Page;
use crate::state::{N4n0Confirmation, N4n0Line, Snapshot};
use egui::{vec2, Event, Pos2, RawInput, ViewportId, ViewportInfo};
use orbiters_core::settings::Settings;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// --- The morph ---

/// Steps a morph at 60 frames a second for `seconds`; how many frames it moved in.
fn run(m: &mut Morph, from: f64, seconds: f64, reduced: bool) -> (f64, u32) {
    let mut moving = 0;
    let frames = (seconds * 60.0).round() as u32;
    for f in 1..=frames {
        if m.step(from + f as f64 / 60.0, reduced) {
            moving += 1;
        }
    }
    (from + seconds, moving)
}

#[test]
fn the_morph_opens_in_about_a_quarter_second_and_then_rests() {
    let mut m = Morph::default();
    assert!(!m.step(0.0, false) && m.closed(), "at rest it asks for nothing");
    m.set_open(true);
    let mut t = 0.0;
    let mut reached = None;
    for f in 1..=60 {
        t = f as f64 / 60.0;
        m.step(t, false);
        if reached.is_none() && m.shape() >= 0.95 {
            reached = Some(t);
        }
    }
    let reached = reached.expect("it opens");
    assert!((0.12..=0.35).contains(&reached), "95 % open after {reached:.3} s");
    assert!(m.at_rest() && m.shape() == 1.0 && m.openness() == 1.0, "settled open within a second");
    assert!(!m.step(t + 1.0 / 60.0, false), "and asks for no more frames");
}

#[test]
fn the_morph_overshoots_a_little_and_never_goes_under_the_bar() {
    let mut m = Morph::default();
    m.set_open(true);
    let mut peak = 0.0f32;
    for f in 1..=60 {
        m.step(f as f64 / 60.0, false);
        peak = peak.max(m.shape());
    }
    assert!(peak > 1.0 && peak < 1.06, "a spring's settle, not a wobble: peak {peak}");
    m.set_open(false);
    for f in 61..=120 {
        m.step(f as f64 / 60.0, false);
        assert!(m.shape() >= 0.0, "closing lands on the bar");
    }
    assert!(m.closed());
}

#[test]
fn an_opening_turned_back_half_way_shrinks_from_where_it_is() {
    let mut m = Morph::default();
    m.set_open(true);
    let (t, _) = run(&mut m, 0.0, 0.06, false);
    let mid = m.shape();
    assert!(mid > 0.1 && mid < 0.9, "half-way: {mid}");
    // Esc (or a press beside it): back from there, no jump.
    m.set_open(false);
    m.step(t + 1.0 / 60.0, false);
    assert!((m.shape() - mid).abs() < 0.12, "continuous: {mid} then {}", m.shape());
    let (t, moving) = run(&mut m, t + 1.0 / 60.0, 0.6, false);
    assert!(moving > 5 && m.closed(), "back in the bar, in one motion");
    // And reversed again while closing.
    m.set_open(true);
    run(&mut m, t, 0.05, false);
    m.set_open(false);
    run(&mut m, t + 0.05, 0.5, false);
    assert!(m.closed() && m.shape() == 0.0);
}

#[test]
fn reduced_motion_fades_the_chat_without_a_morph() {
    let mut m = Morph::default();
    m.step(0.0, true);
    m.set_open(true);
    m.step(1.0 / 60.0, true);
    assert_eq!(m.shape(), 1.0, "the chat's own shape at once, no growth");
    assert!(m.alpha() > 0.0 && m.alpha() < 1.0, "fading in: {}", m.alpha());
    let (t, moving) = run(&mut m, 1.0 / 60.0, 0.5, true);
    assert!(moving <= (FADE * 60.0).ceil() as u32 + 1, "a quick fade: {moving} frames");
    assert_eq!((m.alpha(), m.openness()), (1.0, 1.0));
    m.set_open(false);
    run(&mut m, t, 0.5, true);
    assert!(m.closed() && m.shape() == 0.0 && m.alpha() == 0.0);
}

// --- The strip ---

const WINDOWS_BUTTONS: f32 = 4.0 + 44.0 * 3.0;
/// "Orbiters - N4N0" and its BETA chip end about here.
const NAME_END: f32 = 172.0;

fn windows_strip(width: f32, right_reserved: f32) -> Strip {
    let window = Rect::from_min_size(Pos2::ZERO, vec2(width, 820.0));
    Strip::new(window, 40.0, NAME_END, width - right_reserved)
}

#[test]
fn the_bar_is_centred_and_shrinks_with_the_window() {
    for (width, wanted) in [(1280.0, 512.0), (1040.0, 416.0), (900.0, 360.0)] {
        let s = windows_strip(width, WINDOWS_BUTTONS);
        let Bar::Full(bar) = s.bar else { panic!("{width}: a bar") };
        assert_eq!(bar.center().x, width / 2.0, "{width}: centred on the window");
        assert_eq!(bar.width(), wanted, "{width}");
        assert_eq!((bar.top(), bar.height()), (layout::BAR_TOP, layout::BAR_H));
        assert!(bar.left() >= NAME_END + layout::GAP && bar.right() <= width - WINDOWS_BUTTONS - layout::GAP, "{width}: clear of the name and the buttons");
    }
    // The narrowest window still has a bar, narrower.
    let Bar::Full(bar) = windows_strip(640.0, WINDOWS_BUTTONS).bar else { panic!("a bar at 640") };
    assert!(bar.width() >= layout::BAR_MIN && bar.width() < 360.0);
}

#[test]
fn a_crowded_strip_slides_the_bar_then_gives_a_button() {
    // A recording's Stop beside the window buttons: less room on the right.
    let stop = WINDOWS_BUTTONS + 12.0 + 150.0;
    let Bar::Full(bar) = windows_strip(760.0, stop).bar else { panic!("still a bar") };
    assert!(bar.right() <= 760.0 - stop - layout::GAP, "off-centre, in the room there is");
    let s = windows_strip(640.0, stop);
    let Bar::Icon(icon) = s.bar else { panic!("a button at 640 with Stop: {:?}", s.bar) };
    assert!(icon.left() >= NAME_END + layout::GAP && icon.right() <= 640.0 - stop - layout::GAP);
    assert_eq!(s.hit(icon.center()), Hit::Bar);
}

#[test]
fn every_part_of_the_strip_keeps_its_job_at_every_width() {
    for width in [1280.0, 1040.0, 900.0, 640.0] {
        let s = windows_strip(width, WINDOWS_BUTTONS);
        let bar = s.bar.rect();
        let y = 20.0;
        assert_eq!(s.hit(pos2(60.0, y)), Hit::Kept, "{width}: the name");
        assert_eq!(s.hit(pos2(bar.center().x, y)), Hit::Bar, "{width}: the bar");
        assert_eq!(s.hit(pos2(bar.left() - 4.0, y)), Hit::Drag, "{width}: left of the bar drags");
        assert_eq!(s.hit(pos2(bar.right() + 4.0, y)), Hit::Drag, "{width}: right of the bar drags");
        assert_eq!(s.hit(pos2(bar.center().x, 2.0)), Hit::Drag, "{width}: above the bar drags");
        assert_eq!(s.hit(pos2(bar.center().x, 37.0)), Hit::Drag, "{width}: under the bar drags");
        for i in 0..3 {
            assert_eq!(s.hit(pos2(width - 4.0 - 44.0 * (3 - i) as f32 + 22.0, y)), Hit::Kept, "{width}: window button {i}");
        }
        assert_eq!(s.hit(pos2(bar.center().x, 60.0)), Hit::Outside);
        assert!(s.free_until() < bar.left(), "the world's name stops before the bar");
    }
}

#[test]
fn a_mac_strip_keeps_its_three_buttons_at_the_left() {
    // A Mac's buttons take 82 points before the name; nothing is on the right but a margin.
    let window = Rect::from_min_size(Pos2::ZERO, vec2(900.0, 700.0));
    let s = Strip::new(window, 40.0, 82.0 + NAME_END - 16.0, 900.0 - 12.0);
    let Bar::Full(bar) = s.bar else { panic!() };
    assert_eq!(bar.center().x, 450.0);
    assert_eq!(s.hit(pos2(30.0, 20.0)), Hit::Kept, "the red, yellow and green buttons");
}

#[test]
fn the_shape_is_the_bar_at_rest_and_the_chat_open_and_one_rect_between() {
    let window = Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 820.0));
    let bar = windows_strip(1280.0, WINDOWS_BUTTONS).bar.rect();
    let panel = layout::panel(window);
    assert_eq!(panel.center().x, 640.0);
    assert_eq!(panel.top(), bar.top(), "the chat hangs where the bar is");
    assert!(panel.bottom() <= 820.0 - layout::PANEL_MARGIN);
    assert_eq!(layout::shape(bar, panel, 0.0), (bar, layout::BAR_RADIUS));
    assert_eq!(layout::shape(bar, panel, 1.0), (panel, layout::PANEL_RADIUS));
    let mut last = bar;
    for k in 1..=20 {
        let (r, radius) = layout::shape(bar, panel, k as f32 / 20.0);
        assert!(r.contains_rect(last) || r.width() >= last.width(), "it only grows");
        assert!(r.top() == bar.top() && (r.center().x - 640.0).abs() < 0.01, "anchored at the top, centred");
        assert!(radius >= layout::BAR_RADIUS.min(layout::PANEL_RADIUS) - 0.01);
        last = r;
    }
    // The field rides inside the shape all the way.
    let composer = Rect::from_min_size(pos2(panel.left() + 16.0, panel.bottom() - 90.0), vec2(panel.width() - 32.0, 52.0));
    for k in 0..=20 {
        let t = k as f32 / 20.0;
        let (shape, _) = layout::shape(bar, panel, t);
        let composer_now = composer.translate(vec2(0.0, shape.bottom() - panel.bottom()));
        let f = layout::field(Rect::from_min_size(bar.min + vec2(30.0, 5.0), vec2(300.0, 18.0)), composer_now, t);
        assert!(shape.expand(1.0).contains_rect(f), "t {t}: {f:?} in {shape:?}");
    }
}

// --- The thread ---

fn line(who: N4n0Who, text: &str, at_ms: u64) -> N4n0Line {
    N4n0Line { who, text: text.into(), at_ms, streaming: false }
}

#[test]
fn the_thread_names_n4n0_once_a_run_and_lets_the_card_say_its_question() {
    let mut snap = Snapshot::new(Settings::default());
    let question = "N4N0 will do this if you say yes: Boop \"Sam\".";
    snap.n4n0.lines = vec![
        line(N4n0Who::You, "Boop Sam", 1),
        line(N4n0Who::N4n0, "Sure.", 2),
        line(N4n0Who::N4n0, "One moment.", 3),
        line(N4n0Who::Note, question, 4),
        line(N4n0Who::Error, "Can't reach Orbiters.", 5),
    ]
    .into();
    snap.n4n0.confirmations = vec![N4n0Confirmation { id: "1".into(), title: String::new(), summary: question.into(), approve_url: format!("{}1", orbiters_n4n0::status::LOCAL_APPROVAL) }];
    snap.n4n0.thinking = true;
    let items = panel::items(&snap.n4n0);
    assert!(matches!(items[1], panel::Item::N4n0 { first: true, .. }) && matches!(items[2], panel::Item::N4n0 { first: false, .. }));
    assert!(!items.iter().any(|i| matches!(i, panel::Item::Note(_))), "the question is the card's");
    assert!(matches!(items[3], panel::Item::Error(_)));
    assert!(matches!(items[4], panel::Item::Thinking), "dots before an answer's first words");
    assert!(matches!(items.last(), Some(panel::Item::Confirm(_))), "the question waits at the end");
    snap.n4n0.lines.push(N4n0Line { streaming: true, ..line(N4n0Who::N4n0, "Booping", 6) });
    assert!(!panel::items(&snap.n4n0).iter().any(|i| matches!(i, panel::Item::Thinking)), "the answer arriving says it all");
}

// --- The window ---

/// What the window asked of N4N0, and the snapshot it shows.
struct Recorder {
    snap: Mutex<Arc<Snapshot>>,
    n4n0: Mutex<Vec<N4n0Command>>,
    patches: Mutex<Vec<serde_json::Value>>,
    signed_in: Mutex<u32>,
}

impl Recorder {
    fn new(snap: Snapshot) -> Arc<Self> {
        Arc::new(Self { snap: Mutex::new(Arc::new(snap)), n4n0: Mutex::default(), patches: Mutex::default(), signed_in: Mutex::new(0) })
    }

    fn set(&self, f: impl FnOnce(&mut Snapshot)) {
        let mut s = (**self.snap.lock().unwrap()).clone();
        f(&mut s);
        *self.snap.lock().unwrap() = Arc::new(s);
    }

    fn sent(&self) -> Vec<N4n0Command> {
        self.n4n0.lock().unwrap().clone()
    }
}

impl Backend for Recorder {
    fn n4n0(&self, command: N4n0Command) {
        self.n4n0.lock().unwrap().push(command);
    }
    fn patch_settings(&self, patch: serde_json::Value) {
        self.patches.lock().unwrap().push(patch);
    }
    fn view(&self) -> Arc<Snapshot> {
        self.snap.lock().unwrap().clone()
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
    fn sign_in(&self) {
        *self.signed_in.lock().unwrap() += 1;
    }
    fn reopen_sign_in(&self) {}
    fn cancel_sign_in(&self) {}
    fn sign_out(&self) {}
    fn request_worker_log(&self) {}
    fn worker_log(&self) -> Option<Vec<String>> {
        None
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

/// A signed-in member, the assistant on, the usage notice already closed.
fn ready() -> Snapshot {
    let mut s = Snapshot::new(Settings::default());
    s.settings.usage.told = true;
    s.settings.usage.told_perf = true;
    s.settings.usage.told_account = true;
    s.settings.consent_acknowledged = true;
    s.settings.n4n0.enabled = true;
    s.settings.n4n0.assistant = true;
    s.account.state = AccountState::SignedIn { username: "Sample".into() };
    s
}

/// The window, frame by frame (on N4N0's page, which reads nothing from this computer while nobody presses).
struct Window {
    ctx: egui::Context,
    app: App,
    backend: Arc<Recorder>,
    time: f64,
    size: egui::Vec2,
    /// The last frame asked for another.
    repaint: bool,
}

impl Window {
    fn new(snap: Snapshot, width: f32) -> Self {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        orbiters_ui::widgets::watch_controls();
        let backend = Recorder::new(snap);
        let ctl: Ctl = backend.clone();
        let mut app = App::offline(ctl);
        app.skip_splash();
        app.set_page(Page::N4n0, 0.0);
        let mut w = Self { ctx, app, backend, time: 0.0, size: vec2(width, 820.0), repaint: false };
        w.settle();
        w
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.frame_after(1.0 / 60.0, events);
    }

    fn frame_after(&mut self, seconds: f64, events: Vec<Event>) {
        self.time += seconds;
        let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)), time: Some(self.time), ..Default::default() };
        input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(1.0), focused: Some(true), ..Default::default() });
        input.events = events;
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(input, |ui| {
            app.logic_offline(ui.ctx());
            app.draw(ui);
        });
        // Nothing paints these frames.
        out.textures_delta.clear();
        self.repaint = out.viewport_output.get(&ViewportId::ROOT).is_some_and(|v| v.repaint_delay < std::time::Duration::from_secs(1));
    }

    fn settle(&mut self) {
        for _ in 0..90 {
            self.frame(Vec::new());
        }
    }

    fn press(&mut self, p: Pos2) {
        self.frame(vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        self.frame(vec![Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]);
    }

    fn type_text(&mut self, text: &str) {
        self.frame(vec![Event::Text(text.into())]);
    }

    fn focused(&self) -> bool {
        self.ctx.memory(|m| m.has_focus(field_id()))
    }

    fn chat(&self) -> &ChatBar {
        self.app.chat()
    }

    fn strip(&self) -> Strip {
        let w = self.size.x;
        Strip::new(Rect::from_min_size(Pos2::ZERO, self.size), crate::app::TITLE_H, NAME_END, w - WINDOWS_BUTTONS)
    }

    fn bar(&self) -> Rect {
        self.app.chat().strip().expect("the strip was drawn").bar.rect()
    }
}

#[test]
fn the_shortcut_takes_the_keyboard_to_the_bar_and_typing_grows_the_chat() {
    let mut w = Window::new(ready(), 1280.0);
    assert!(!w.focused() && !w.chat().shows());
    w.key(Key::J, Modifiers::COMMAND);
    w.frame(Vec::new());
    assert!(w.focused(), "Ctrl+J: the bar has the keyboard");
    assert!(!w.chat().is_open(), "nothing typed, no conversation: still the bar");
    w.type_text("hello");
    assert_eq!(w.chat().draft(), "hello");
    assert!(w.chat().is_open(), "typing grows the chat");
    w.settle();
    assert!(w.focused(), "the field kept the keyboard on its way down");
    w.key(Key::Enter, Modifiers::NONE);
    assert!(matches!(w.backend.sent().last(), Some(N4n0Command::Ask { text }) if text == "hello"), "{:?}", w.backend.sent());
    assert_eq!(w.chat().draft(), "", "sent and cleared");
}

#[test]
fn ctrl_k_stays_the_settings_search_s_and_ctrl_j_is_the_chat_s_on_every_page() {
    let mut w = Window::new(ready(), 1280.0);
    w.app.set_page(Page::Settings, w.time);
    w.settle();
    w.key(Key::K, Modifiers::COMMAND);
    w.frame(Vec::new());
    assert!(!w.focused() && !w.chat().shows(), "Ctrl+K on Settings is its search's");
    w.key(Key::J, Modifiers::COMMAND);
    w.frame(Vec::new());
    assert!(w.focused(), "Ctrl+J: the bar, on Settings too");
}

#[test]
fn shift_enter_starts_a_line_and_a_composing_input_method_keeps_its_enter() {
    let mut w = Window::new(ready(), 1280.0);
    w.key(Key::J, Modifiers::COMMAND);
    w.type_text("first");
    w.key(Key::Enter, Modifiers::SHIFT);
    w.type_text("second");
    assert_eq!(w.chat().draft(), "first\nsecond");
    assert!(w.backend.sent().is_empty());
    // Japanese: the Enter that commits the composition is the input method's.
    w.frame(vec![Event::Ime(egui::ImeEvent::Preedit { text: "にほん".into(), active_range_chars: None })]);
    w.frame(vec![Event::Ime(egui::ImeEvent::Commit("日本".into())), Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    assert!(w.backend.sent().is_empty(), "the committing Enter sends nothing");
    assert!(w.chat().draft().ends_with("日本"), "{:?}", w.chat().draft());
    w.key(Key::Enter, Modifiers::NONE);
    assert!(matches!(w.backend.sent().last(), Some(N4n0Command::Ask { text }) if text == "first\nsecond日本"));
}

#[test]
fn esc_and_a_press_beside_the_chat_shrink_it_back_and_keep_what_was_typed() {
    let mut w = Window::new(ready(), 1280.0);
    w.key(Key::J, Modifiers::COMMAND);
    w.type_text("half a thought");
    for _ in 0..4 {
        w.frame(Vec::new());
    }
    assert!(w.chat().is_open());
    // Esc half-way: it turns back.
    w.key(Key::Escape, Modifiers::NONE);
    assert!(!w.chat().is_open());
    w.settle();
    assert!(!w.chat().shows() && !w.focused());
    assert_eq!(w.chat().draft(), "half a thought", "kept for later");
    assert_eq!(w.app.shown_page(), Page::N4n0, "Esc was the chat's, not the page's");
    // Back, then a press on the page beside it.
    w.press(w.bar().center());
    w.type_text("!");
    w.settle();
    assert!(w.chat().is_open());
    w.press(pos2(20.0, 780.0));
    w.settle();
    assert!(!w.chat().shows(), "a press beside the chat closes it");
}

#[test]
fn the_page_gets_no_keys_presses_or_wheel_while_the_chat_has_them() {
    let mut w = Window::new(ready(), 1280.0);
    w.key(Key::J, Modifiers::COMMAND);
    w.type_text("x");
    w.settle();
    let panel = layout::panel(Rect::from_min_size(Pos2::ZERO, w.size));
    // The chat's area takes what is over it, the backdrop what is beside it: the page under it is never reached.
    assert_eq!(w.ctx.layer_id_at(panel.center()).map(|l| l.order), Some(Order::Middle));
    let beside = w.ctx.layer_id_at(pos2(30.0, 700.0)).expect("a layer");
    assert_ne!(beside, LayerId::background(), "the page does not get the wheel or a press beside the chat");
    // The title strip stays the window's: it drags beside the chat, its buttons work.
    assert_eq!(w.ctx.layer_id_at(pos2(1280.0 - 30.0, 20.0)), Some(LayerId::background()));
    assert_eq!(w.ctx.layer_id_at(pos2(80.0, 20.0)), Some(LayerId::background()));
}

#[test]
fn a_press_in_the_strip_reaches_what_is_there_at_every_width() {
    for width in [1280.0, 1040.0, 900.0] {
        let mut w = Window::new(ready(), width);
        let strip = w.strip();
        let bar = w.bar();
        assert_eq!(bar, strip.bar.rect(), "{width}: the bar where the layout says");
        // Beside the bar the strip is the window's (it drags it): the field does not take the keyboard.
        w.press(pos2(bar.left() - 10.0, 20.0));
        assert!(!w.focused(), "{width}: a press beside the bar");
        assert_eq!(w.ctx.layer_id_at(pos2(bar.left() - 10.0, 20.0)), Some(LayerId::background()), "{width}: the strip under the pointer");
        w.press(pos2(width - 4.0 - 44.0 * 2.5, 20.0));
        assert!(!w.focused(), "{width}: the minimise button");
        // The bar takes it.
        w.press(bar.center());
        w.frame(Vec::new());
        assert!(w.focused(), "{width}: a press on the bar gives it the keyboard");
        assert_eq!(w.ctx.layer_id_at(bar.center()).map(|l| l.order), Some(Order::Middle));
    }
}

#[test]
fn a_conversation_under_way_opens_from_the_bar_and_stays_across_pages() {
    let mut snap = ready();
    snap.n4n0.lines = vec![line(N4n0Who::You, "Turn on fast walk", 1), line(N4n0Who::N4n0, "Fast walk is on.", 2)].into();
    let mut w = Window::new(snap, 1280.0);
    w.press(w.bar().center());
    w.settle();
    assert!(w.chat().is_open(), "a press on the bar goes back to the conversation");
    w.type_text("thanks");
    w.app.set_page(Page::Settings, w.time);
    w.settle();
    assert!(w.chat().is_open() && w.chat().draft() == "thanks", "the chat and what is typed stay over another page");
    w.key(Key::Escape, Modifiers::NONE);
    w.app.set_page(Page::Home, w.time);
    w.settle();
    assert_eq!(w.chat().draft(), "thanks");
    w.key(Key::J, Modifiers::COMMAND);
    w.settle();
    assert!(w.chat().is_open(), "Ctrl+J with a conversation: the chat itself");
}

#[test]
fn a_streamed_answer_flows_in_and_the_window_rests_after() {
    let mut w = Window::new(ready(), 1280.0);
    w.key(Key::J, Modifiers::COMMAND);
    w.type_text("tell me");
    w.key(Key::Enter, Modifiers::NONE);
    w.backend.set(|s| {
        s.n4n0.thinking = true;
        s.n4n0.lines.push(line(N4n0Who::You, "tell me", 1));
    });
    w.settle();
    let long = "Here is a long answer that arrives a few words at a time, as the server streams it.";
    w.backend.set(|s| s.n4n0.lines.push(N4n0Line { streaming: true, ..line(N4n0Who::N4n0, &long[..20], 2) }));
    w.frame(Vec::new());
    assert!(w.repaint, "streaming asks for frames");
    w.backend.set(|s| {
        s.n4n0.thinking = false;
        let last = s.n4n0.lines.last_mut().unwrap();
        last.text = long.into();
        last.streaming = false;
    });
    w.settle();
    // Rest: the chat open, nothing moving, the field without the keyboard (a blinking caret asks for frames).
    w.ctx.memory_mut(|m| m.surrender_focus(field_id()));
    // The window's own decorations (the glow's drift) rest half a minute after the last input: past that, only what
    // moves asks for frames.
    for _ in 0..40 {
        w.frame_after(1.0, Vec::new());
    }
    assert!(w.chat().is_open(), "still open");
    assert!(!w.repaint, "an open chat at rest asks for no frames");
}

#[test]
fn a_question_is_answered_with_its_buttons() {
    let mut snap = ready();
    let question = "N4N0 will do this if you say yes: Boop \"Sam\".";
    snap.n4n0.lines = vec![line(N4n0Who::You, "Boop Sam", 1), line(N4n0Who::Note, question, 2)].into();
    snap.n4n0.confirmations = vec![N4n0Confirmation { id: "7".into(), title: String::new(), summary: question.into(), approve_url: format!("{}7", orbiters_n4n0::status::LOCAL_APPROVAL) }];
    let mut w = Window::new(snap, 1280.0);
    w.key(Key::J, Modifiers::COMMAND);
    w.settle();
    let (yes, on) = orbiters_ui::widgets::control(&w.ctx, orbiters_i18n::tr("Yes, do it")).expect("Yes is drawn");
    assert!(on);
    w.press(yes.center());
    assert!(matches!(w.backend.sent().last(), Some(N4n0Command::AskAnswer { yes: true })), "{:?}", w.backend.sent());
    let (no, _) = orbiters_ui::widgets::control(&w.ctx, orbiters_i18n::tr("No, leave it")).expect("No is drawn");
    w.press(no.center());
    assert!(matches!(w.backend.sent().last(), Some(N4n0Command::AskAnswer { yes: false })));
}

#[test]
fn signed_out_and_assistant_off_say_so_and_lead_to_the_fix() {
    let mut snap = ready();
    snap.account.state = AccountState::SignedOut;
    let mut w = Window::new(snap, 1280.0);
    w.press(w.bar().center());
    w.settle();
    assert!(w.chat().is_open(), "the bar opens on why N4N0 cannot answer");
    w.type_text("ignored");
    assert_eq!(w.chat().draft(), "", "nothing is typed for nobody");
    let (sign_in, _) = orbiters_ui::widgets::control(&w.ctx, orbiters_i18n::tr("Sign in")).expect("Sign in is offered");
    w.press(sign_in.center());
    assert_eq!(*w.backend.signed_in.lock().unwrap(), 1);

    let mut off = ready();
    off.settings.n4n0.assistant = false;
    let mut w = Window::new(off, 1280.0);
    w.key(Key::J, Modifiers::COMMAND);
    w.settle();
    let (on, _) = orbiters_ui::widgets::control(&w.ctx, orbiters_i18n::tr("Turn on the assistant")).expect("Turn on is offered");
    w.press(on.center());
    let patch = w.backend.patches.lock().unwrap().last().cloned().expect("settings patched");
    assert_eq!(patch["n4n0"]["assistant"], serde_json::json!(true), "{patch}");
}
