//! What a person does with the menus, turn by turn, as `vr.rs` drives them, with nothing of SteamVR. Each test here is a workflow that broke in a headset.

use super::panels::{Action, Item, Sheet};
use super::zones::{self, Around, Placed, Zone};
use super::Panels;
use crate::state::Snapshot;
use orbiters_core::settings::Settings;
use orbiters_n4n0_core::menu::Target;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Person {
    panels: Panels,
    snap: Arc<Snapshot>,
    around: Option<Around>,
    /// The library's version, as the service says it: another one has the gallery's list read again.
    library: u64,
    /// One person at a time: the look chosen (modern or retro) was the whole process's, as the app has one, and a retro test on another thread laid All tools out
    /// under a modern one's press, which then missed its tile (two of these failed so beside the hole's retro runs). In tests the look is now each thread's
    /// (`panels::look::chosen`), which also keeps it from the tests that never took this lock (VR1115 flaky); the lock stays for what else these share.
    _alone: std::sync::MutexGuard<'static, ()>,
}

/// Turns of 11 ms the springs get to rest once everything they show is in: 22 s of their time, many times what any part takes.
const AT_REST_TURNS: usize = 2000;
/// How long the other threads' pictures and lists may take before a test calls it a hang.
const AT_REST_HANG: Duration = Duration::from_secs(120);

pub(super) static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// MENU-8: the QR code sheet closed (its head, VRChat's menu closing), or left for another of N4N0's pages, lets the
/// panels' scan go at once; nothing waits for some later page to say so.
#[test]
fn the_qr_code_sheet_closed_or_left_stops_its_scan() {
    use crate::ipc::N4n0Command;
    use orbiters_n4n0::MenuHolder;
    use orbiters_n4n0_core::menu::Target;
    let (mut person, heard) = Person::heard();
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    heard.n4n0.lock().unwrap().clear();
    let snap = person.snap.clone();
    let p = &mut person.panels;
    let qr = |heard: &Heard| heard.n4n0.lock().unwrap().iter().filter_map(|c| match c { N4n0Command::Qr { on, holder } => Some((*on, *holder)), _ => None }).collect::<Vec<_>>();
    p.open_page(Target::Qr, &snap);
    assert_eq!(qr(&heard), [(true, MenuHolder::Panels)]);
    assert_eq!(p.facts.sheet, Some(Sheet::Menu));
    p.close_sheet();
    assert_eq!(qr(&heard), [(true, MenuHolder::Panels), (false, MenuHolder::Panels)], "closed, the scan is let go");
    heard.n4n0.lock().unwrap().clear();
    p.open_page(Target::Qr, &snap);
    p.open_page(Target::Music, &snap);
    // (The panels' menu still stood on the scanner's page after the close: opening it again says stop once more, for
    // nothing it holds.)
    assert_eq!(qr(&heard).last(), Some(&(false, MenuHolder::Panels)), "left for another page");
    assert!(qr(&heard).ends_with(&[(true, MenuHolder::Panels), (false, MenuHolder::Panels)]), "{:?}", qr(&heard));
}

/// verify-r15 B2: on the desktop, the Worlds page left for another tab, or the menu closed, its capture stops turning and is not started again by the turns
/// that follow (the Worlds' own page stays open behind the other tab); back on Worlds it turns again.
#[test]
fn a_world_s_capture_stops_behind_another_tab_and_with_the_menu() {
    let mut person = Person::new();
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    person.press(Action::Worlds);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Worlds));
    let gone = std::env::temp_dir().join(format!("orbiters-no-capture-flow-{}", std::process::id()));
    let mut list = crate::overlay::worlds::catalog::parse(r#"{"previews":[{"slot":1,"key":1,"id":1,"worldId":"wrld_00000000-0000-4000-8000-000000000001","name":"W","author":"a","uploader":"b","view":{"position":[0,1,0],"target":[0,1,1]}}],"feedback":{}}"#).unwrap();
    list[0].local = Some(gone);
    person.panels.stage.worlds.as_mut().expect("Worlds' page").chosen_for_test(list, 1);
    person.turns(2);
    let turning = |p: &Person| p.panels.stage.worlds.as_ref().unwrap().turning_for_test();
    assert_eq!(turning(&person), (Some(1), true), "its capture turns on its page");
    person.press(Action::Page(Target::Instances));
    for _ in 0..5 {
        person.turn();
        assert_eq!(turning(&person), (None, false), "nothing turns or reads behind Instances");
    }
    person.press(Action::Worlds);
    person.turns(2);
    let mut again = crate::overlay::worlds::catalog::parse(r#"{"previews":[{"slot":1,"key":1,"id":1,"worldId":"wrld_00000000-0000-4000-8000-000000000001","name":"W","author":"a","uploader":"b","view":{"position":[0,1,0],"target":[0,1,1]}}],"feedback":{}}"#).unwrap();
    again[0].local = Some(std::env::temp_dir().join("orbiters-no-capture-flow-again"));
    person.panels.stage.worlds.as_mut().unwrap().chosen_for_test(again, 1);
    person.turns(2);
    assert_eq!(turning(&person), (Some(1), true), "back on Worlds: it turns again");
    person.around = None;
    for _ in 0..3 {
        person.turn();
        assert_eq!(turning(&person), (None, false), "the menu closed: nothing turns behind it");
        assert!(person.panels.stage.worlds.is_some());
    }
}

/// REQUEST15 item 3 fix B1: in VRChat's desktop mode with no SteamVR, an avatar whose older N4N0 hides its screen with the bot has its menu
/// asked for (the Action Menu's Menu toggle, VRChat's wheel open): All tools opens here and the service hears it shows, so the toggle stays on;
/// a toggling request closes it, VRChat's menu going closes it, and an avatar whose screen takes the menu has nothing here.
#[test]
fn the_service_s_menu_request_opens_all_tools_on_the_desktop() {
    use crate::ipc::N4n0Command;
    let (mut person, heard) = Person::heard();
    let opened = |heard: &Heard| heard.n4n0.lock().unwrap().iter().filter_map(|c| match c { N4n0Command::MenuState { open, .. } => Some(*open), _ => None }).collect::<Vec<_>>();
    {
        let snap = Arc::make_mut(&mut person.snap);
        snap.n4n0.desktop_mode = true;
        snap.steamvr.running = false;
        snap.n4n0.screen = false;
    }
    person.around = Some(Around::Wheel);
    person.turns(3);
    assert_eq!(person.panels.facts.sheet, None, "nothing asked yet");
    let ask = |person: &mut Person, toggle: bool| {
        let snap = Arc::make_mut(&mut person.snap);
        snap.n4n0.menu_request = snap.n4n0.menu_request.wrapping_add(1);
        snap.n4n0.menu_toggle = toggle;
        person.turns(2);
    };
    ask(&mut person, false);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::All), "the Menu toggle opens All tools beside VRChat's wheel");
    assert_eq!(opened(&heard).last(), Some(&true), "and the service hears N4N0's menu shows: {:?}", opened(&heard));
    ask(&mut person, true);
    assert_eq!(person.panels.facts.sheet, None, "a toggling request closes it");
    assert_eq!(opened(&heard).last(), Some(&false));
    ask(&mut person, false);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::All));
    person.around = None;
    person.turns(2);
    assert_eq!(opened(&heard).last(), Some(&false), "VRChat's menu closing closes N4N0's menu here");
    // The avatar's screen takes the menu: nothing opens here.
    person.around = Some(Around::Wheel);
    Arc::make_mut(&mut person.snap).n4n0.screen = true;
    person.turns(2);
    ask(&mut person, false);
    assert_eq!(person.panels.facts.sheet, None, "the avatar's screen shows it");
    // In VR (SteamVR runs, VRChat in VR) the SteamVR overlay serves it, not this.
    {
        let snap = Arc::make_mut(&mut person.snap);
        snap.n4n0.screen = false;
        snap.n4n0.desktop_mode = false;
        snap.steamvr.running = true;
    }
    ask(&mut person, false);
    assert_eq!(person.panels.facts.sheet, None, "the SteamVR overlay's");
}

/// REQUEST15 fix B2: All tools serving N4N0's menu on the desktop, and the avatar's screen taking the menu back (its older N4N0's bot shown
/// again, the service opening the menu there): the sheet goes and the service hears it closed here, so only the screen has the menu.
#[test]
fn all_tools_gives_n4n0_s_menu_back_to_the_avatar_s_screen() {
    use crate::ipc::N4n0Command;
    let (mut person, heard) = Person::heard();
    let opened = |heard: &Heard| heard.n4n0.lock().unwrap().iter().filter_map(|c| match c { N4n0Command::MenuState { open, .. } => Some(*open), _ => None }).collect::<Vec<_>>();
    {
        let snap = Arc::make_mut(&mut person.snap);
        snap.n4n0.desktop_mode = true;
        snap.n4n0.screen = false;
    }
    person.around = Some(Around::Wheel);
    person.turns(2);
    let snap = Arc::make_mut(&mut person.snap);
    snap.n4n0.menu_request = snap.n4n0.menu_request.wrapping_add(1);
    person.turns(2);
    assert_eq!((person.panels.facts.sheet, opened(&heard).last().copied()), (Some(Sheet::All), Some(true)));
    Arc::make_mut(&mut person.snap).n4n0.screen = true;
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, None, "the screen has the menu now");
    assert_eq!(opened(&heard).last(), Some(&false), "and the service hears this one closed");
}

/// verify2-r15 S3: on the desktop, Translation's On turns on the reading of VRChat's window too, and Off turns it off; in a headset, On is translation's alone.
#[test]
fn the_desktop_s_translation_on_is_the_desktop_s_own() {
    use super::panels::{Pick, TranslatePick};
    let (mut person, heard) = Person::heard();
    person.around = Some(Around::QuickMenu);
    person.turns(2);
    // The desktop's menus (as `facts_of` says with VRChat in desktop mode).
    person.panels.facts.desktop = true;
    let snap = person.snap.clone();
    person.panels.set(Pick::Translate(TranslatePick::On(true)), &snap);
    person.panels.set(Pick::Translate(TranslatePick::On(false)), &snap);
    let patches = heard.patches.lock().unwrap().clone();
    let on: Vec<_> = patches.iter().filter_map(|p| p.get("translate")).collect();
    assert_eq!(on.len(), 2, "{patches:?}");
    assert_eq!(on[0]["enabled"], true);
    assert_eq!(on[0]["window_on"], true, "on on the desktop: VRChat's window is read");
    assert_eq!(on[1]["window_on"], false, "off: no longer");
    person.panels.facts.desktop = false;
    person.panels.set(Pick::Translate(TranslatePick::On(true)), &snap);
    let last = heard.patches.lock().unwrap().last().cloned().unwrap();
    assert!(last["translate"].get("window_on").is_none(), "in a headset: the desktop's own is not turned on");
}

/// OVL2-17: a menu going away under a held press does nothing: the list press is not run later, the held booster level and
/// smoothing are not saved.
#[test]
fn a_menu_going_away_under_a_held_press_does_nothing() {
    let (mut person, heard) = Person::heard();
    let snap = person.snap.clone();
    let p = &mut person.panels;
    p.open = Some(Around::QuickMenu);
    p.drag = Some(super::Drag { from: Some(0.0), scroll: 0.0, action: Some(Action::DemoAvatar), moved: false, bar: false, speed: 0.0, last: None });
    p.booster_held = Some(orbiters_booster::Level::default());
    p.smoothing_held = Some((super::panels::Smoothed::QuickMenu, 40));
    p.set_open(None, Vec::new(), &snap);
    assert!(p.pressed_late.is_none(), "the press is not kept to be run later");
    assert!(p.booster_held.is_none() && p.smoothing_held.is_none() && p.drag.is_none());
    assert!(heard.boosters.lock().unwrap().is_empty(), "no booster level saved: {:?}", heard.boosters.lock().unwrap());
    assert!(heard.patches.lock().unwrap().is_empty(), "nothing saved: {:?}", heard.patches.lock().unwrap());
    // A real let-go still does what it did.
    p.drag = Some(super::Drag { from: Some(0.0), scroll: 0.0, action: Some(Action::DemoAvatar), moved: false, bar: false, speed: 0.0, last: None });
    p.release();
    assert!(p.pressed_late.is_some(), "a let-go runs the press, as before");
}

/// verify-vr on OVL2-17: a tap let go on a list row earns its action; the menu closing before the next turn does not erase it, and
/// it runs exactly once.
#[test]
fn an_action_earned_by_a_let_go_runs_once_though_the_menu_closed() {
    let (mut person, heard) = Person::heard();
    let snap = person.snap.clone();
    let p = &mut person.panels;
    p.open = Some(Around::QuickMenu);
    p.drag = Some(super::Drag { from: Some(0.0), scroll: 0.0, action: Some(Action::NewConversation), moved: false, bar: false, speed: 0.0, last: None });
    p.release();
    p.set_open(None, Vec::new(), &snap);
    p.run_pending(&snap);
    p.run_pending(&snap);
    let runs = heard.said().iter().filter(|c| c.contains("NewConversation")).count();
    assert_eq!(runs, 1, "{:?}", heard.said());
}

/// verify2: two rows let go before the turn that runs them (two clicks between two reads on the desktop, or the other hand's) are
/// two actions, each once.
#[test]
fn two_let_goes_before_a_turn_are_two_actions() {
    let (mut person, heard) = Person::heard();
    let snap = person.snap.clone();
    let p = &mut person.panels;
    p.open = Some(Around::QuickMenu);
    for _ in 0..2 {
        p.drag = Some(super::Drag { from: Some(0.0), scroll: 0.0, action: Some(Action::NewConversation), moved: false, bar: false, speed: 0.0, last: None });
        p.release();
    }
    p.run_pending(&snap);
    p.run_pending(&snap);
    let runs = heard.said().iter().filter(|c| c.contains("NewConversation")).count();
    assert_eq!(runs, 2, "{:?}", heard.said());
}

impl Person {
    fn new() -> Self {
        let alone = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let panels = Panels::new(Arc::new(crate::screenshot::Still), 2.0, true);
        let snap = Arc::new(crate::screenshot::idle_snapshot(&Settings::default(), &[]));
        Self { panels, snap, around: None, library: 0, _alone: alone }
    }

    /// A person whose presses reach a backend that keeps what it was asked: N4N0's commands and sign-ins.
    fn heard() -> (Self, Arc<Heard>) {
        let mut person = Self::new();
        let heard = Arc::new(Heard::default());
        person.panels = Panels::new(heard.clone(), 2.0, true);
        (person, heard)
    }

    /// One turn of the overlay's loop, as `vr.rs` takes it.
    fn turn(&mut self) {
        turn_of(&mut self.panels, &self.snap, self.around, self.library);
    }

    /// A person whose computer plays the menus' sample song, with its picture, whatever this computer plays: for what compares pictures.
    fn with_a_song() -> Self {
        let mut person = Self::new();
        hears_a_song(&mut person.panels);
        person
    }

    /// Turns until `done`, for what another thread brings (the gallery's list): it comes in its own time.
    fn turns_until(&mut self, what: &str, done: impl Fn(&Person) -> bool) {
        let began = Instant::now();
        while !done(self) {
            assert!(began.elapsed() < Duration::from_secs(10), "{what}: it never came");
            std::thread::sleep(Duration::from_millis(2));
            self.turn();
        }
    }

    /// Turns until every part is painted and at rest and the gallery's list is read. The two halves wait differently (BUILD-9): the springs move a fixed 11 ms a turn,
    /// so once the pictures and the list are in they must rest within [`AT_REST_TURNS`] turns whatever the machine does; the pictures and the list come from other
    /// threads, in their own time, so they have a clock, only against a hang: a debug build sharing the processor with other builds took more than 10 s for what
    /// takes about one alone. A failure says how many turns it took and what was still moving, unpainted or being read.
    fn turns_until_at_rest(&mut self, what: &str) {
        let began = Instant::now();
        let (mut turns, mut since_in) = (0usize, 0usize);
        loop {
            let moving: Vec<Zone> = self.panels.all().filter(|p| !p.appear.settled()).map(|p| p.zone).collect();
            let unpainted: Vec<Zone> = self.panels.all().filter(|p| !p.painted).map(|p| p.zone).collect();
            // The list is taken only on a sheet that shows it (Photos & captures, All tools): one still being read when another sheet was pressed waits there for
            // the gallery's next showing, and is not what a part at rest waits for. On a loaded machine the list thread answered after the press, and the test
            // waited its whole clock for it.
            let reading = self.panels.recents.rx.is_some() && matches!(self.panels.facts.sheet, Some(Sheet::Pictures | Sheet::All));
            if moving.is_empty() && unpainted.is_empty() && !reading {
                return;
            }
            since_in = if unpainted.is_empty() && !reading { since_in + 1 } else { 0 };
            let state = || format!("after {turns} turns ({:?}): moving {moving:?}, unpainted {unpainted:?}, the list being read {reading}, the painter busy {}", began.elapsed(), self.panels.stage.painter.busy());
            assert!(since_in <= AT_REST_TURNS, "{what}: the springs never rested, {}", state());
            assert!(began.elapsed() < AT_REST_HANG, "{what}: the pictures or the list never came, {}", state());
            std::thread::sleep(Duration::from_millis(2));
            self.turn();
            turns += 1;
        }
    }

    fn turns(&mut self, n: usize) {
        for _ in 0..n {
            self.turn();
        }
    }

    /// Where something that does `action` is: its part's place in the list, and a point on it.
    fn find(&self, action: Action) -> Option<(usize, egui::Pos2)> {
        find_on(&self.panels, action)
    }

    /// Points at `target` on the 3D worlds sheet's page, presses and lets go, a turn each, the page told each turn that it is on a sheet in VR (`relay`), as
    /// `vr.rs` tells it; the hand then goes.
    fn press_world(&mut self, target: crate::overlay::worlds::panel::Target, relay: &crate::overlay::worlds::panel::Status) {
        // A docked view animates continuously, so the painter need never be idle. Wait for the
        // requested target in the published picture, just as a person presses what is visible.
        if self.panels.stage.async_pages { self.turns_until("the displayed worlds page", |p| p.panels.takeover.as_ref().is_some_and(|p| p.painted) && p.panels.stage.worlds.as_ref().is_some_and(|w| w.target_for_test(target).is_some())); }
        let at = self.panels.stage.worlds.as_ref().and_then(|w| w.target_for_test(target)).unwrap_or_else(|| panic!("nothing is {target:?} on the page"));
        let sheet = self.panels.all().count() - 1;
        let snap = self.snap.clone();
        let step = |person: &mut Person| {
            let _ = person.panels.world_relay(relay.clone());
            person.turn();
        };
        self.panels.hover(Some((sheet, at)));
        step(self);
        self.panels.press(&snap);
        step(self);
        self.panels.release();
        step(self);
        self.panels.hover(None);
        step(self);
    }

    /// Points at what does `action`, presses and lets go, with turns between as a hand takes them.
    fn press(&mut self, action: Action) -> super::Pressed {
        if self.panels.stage.async_pages { self.turns_until("the displayed control", |p| p.find(action).is_some()); }
        let (part, at) = self.find(action).unwrap_or_else(|| panic!("nothing does {action:?}"));
        self.panels.hover(Some((part, at)));
        self.turn();
        let pressed = self.panels.press(&self.snap);
        self.turn();
        self.panels.release();
        self.turn();
        pressed
    }
}

#[test]
fn tracking_thumbs_use_configured_values_while_the_driver_view_lags() {
    let mut person = Person::new();
    let mut snap = (*person.snap).clone();
    snap.settings.tracking.enabled = true;
    snap.settings.tracking.ms = [90, 70, 40];
    snap.n4n0.tracking = Some(orbiters_n4n0::status::TrackingView { on: false, ms: [0, 0, 0], note: String::new() });
    person.snap = Arc::new(snap);
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    assert_eq!(person.panels.facts.tracking.as_ref().map(|t| (t.0, t.1)), Some((true, [90, 70, 40])));
}

/// Enzo: "When I click the saved looks tile, it makes the whole menu bug and become unresponsive": with real saved looks on the list, in both looks, everything on the page pressed.
#[test]
fn saved_looks_with_real_looks_answers() {
    use orbiters_core::settings::MenuLook;
    use orbiters_n4n0::status::{StateView, StatesView};
    for look in [MenuLook::Modern, MenuLook::Retro] {
        let mut person = Person::new();
        let mut snap = (*person.snap).clone();
        snap.settings.orbit.theme = look;
        snap.n4n0.states = StatesView {
            avatar_id: Some("avtr_x".into()),
            avatar_name: Some("Ghost".into()),
            states: (0..9).map(|i| StateView { id: format!("s{i}"), name: format!("Look {i} ちま é 🌸 a very long name that goes on and on and on and on"), default: i == 0, saved_utc: "2026-11-07T10:00:00Z".into(), pose: false, size: false, parameters: 12, pose_parameters: 0, eye_height: Some(1.4) }).collect(),
            note: None,
            saving: false,
            avatars: vec![],
        };
        person.snap = Arc::new(snap);
        person.around = Some(Around::QuickMenu);
        person.turns(3);
        person.press(Action::All);
        person.turns(3);
        let began = Instant::now();
        person.press(Action::Page(Target::States));
        person.turns(3);
        println!("saved looks {look:?}: open in {:?}", began.elapsed());
        assert_eq!(person.panels.facts.sheet, Some(Sheet::Menu));
        assert_eq!(person.panels.stage.host.menu.page, orbiters_n4n0_core::menu::Page::States);
        // At rest it paints nothing, or nothing else would get a turn. At rest is once the press's pop is over: it runs on the clock for `POP` (700 ms), and 43 turns
        // took less than that on a fast run, which then still painted it (dev.21's test run failed here once).
        let waited = Instant::now();
        while person.panels.stage.host.menu.popping().is_some() {
            assert!(waited.elapsed() < orbiters_n4n0_core::menu::POP * 3, "{look:?}: the pop never ends");
            std::thread::sleep(Duration::from_millis(5));
        }
        // What runs on the clock (a part coming in, a fade) ends in its own time, not in a number of turns: under load 40 turns ran before it had, and painted
        // (dev.29's test run failed here). Turned until three turns in a row paint nothing, with a deadline far past any of them; a part that never comes to rest
        // still fails here.
        let quiet_turn = |person: &mut Person| {
            let mut painted = 0;
            person.panels.follow(&person.snap, 0);
            person.panels.menu_turn(&person.snap);
            person.panels.step_springs(0.011);
            person.panels.paint(&person.snap, |_, _| painted += 1);
            painted == 0
        };
        let settling = Instant::now();
        let mut quiet = 0;
        while quiet < 3 {
            quiet = if quiet_turn(&mut person) { quiet + 1 } else { 0 };
            assert!(settling.elapsed() < Duration::from_secs(10), "{look:?}: it never comes to rest");
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut painted = 0;
        for _ in 0..10 {
            person.panels.follow(&person.snap, 0);
            person.panels.menu_turn(&person.snap);
            person.panels.step_springs(0.011);
            person.panels.paint(&person.snap, |_, _| painted += 1);
        }
        assert_eq!(painted, 0, "{look:?}: at rest");
        // Every point answers.
        let part = person.panels.list.len();
        let mut found = 0;
        let began = Instant::now();
        for y in (40..560).step_by(20) {
            person.panels.hover(Some((part, egui::pos2(500.0, y as f32))));
            person.turn();
            found += person.panels.takeover.as_ref().is_some_and(|p| p.hover.is_some()) as usize;
        }
        println!("saved looks {look:?}: {found} of 26 points on a row, 26 turns in {:?}", began.elapsed());
        assert!(found > 5);
        // Its rows, its pages, and back to All tools; the wing around it answers all the while.
        let at = |x: f32, y: f32| (part, egui::pos2(x, y));
        for (what, point) in [("a row", at(300.0, 140.0)), ("next", at(964.0, 52.0)), ("previous", at(836.0, 52.0)), ("apply", at(898.0, 140.0)), ("default", at(730.0, 140.0))] {
            person.panels.hover(Some(point));
            person.turn();
            let clicked = person.panels.press(&person.snap).clicked;
            person.panels.release();
            person.turns(3);
            assert!(clicked, "{what}");
            assert_eq!(person.panels.facts.sheet, Some(Sheet::Menu), "{what}: it stays");
        }
        assert!(person.find(Action::Pictures).is_some(), "the head's tabs are there");
        assert!(person.press(Action::Back).clicked, "back");
        person.turns(3);
        assert_eq!(person.panels.facts.sheet, Some(Sheet::All), "back to All tools");
        person.press(Action::Page(Target::States));
        person.turns(3);
        assert_eq!(person.panels.stage.host.menu.page, orbiters_n4n0_core::menu::Page::States, "and in again");
    }
}

/// One turn of the overlay's loop on `panels`, as `vr.rs` takes it. Gives what was handed to the platform: each part and how much of its picture.
fn turn_of(panels: &mut Panels, snap: &Snapshot, around: Option<Around>, library: u64) -> Vec<(Zone, [u32; 2], [u32; 4])> {
    let mut places: Vec<Placed> = around.map_or_else(Vec::new, |m| Zone::around(m).iter().map(|z| zones::in_units(*z)).collect());
    panels.way_back(around, &mut places);
    panels.takeover_place = Some(zones::in_units(Zone::Takeover));
    panels.back_place = Some(zones::in_units(Zone::SheetBack));
    panels.set_open(around, places, snap);
    panels.serve_menu(snap);
    panels.follow(snap, library);
    panels.serve_assistant(snap);
    panels.menu_turn(snap);
    panels.step_springs(0.011);
    let mut handed = Vec::new();
    panels.paint(snap, |panel, changed| handed.push((panel.zone, panel.px, changed)));
    handed
}

/// Has `panels` hear the menus' sample song with its picture, from a watcher of its own that reads nothing of this computer.
fn hears_a_song(panels: &mut Panels) {
    use crate::overlay::menu_pages::{sample_art, sample_media, SAMPLE_ART};
    let media = Arc::new(crate::media::Watcher::new(|| Box::new(orbiters_n4n0::media::NoMedia)));
    media.stand_in(crate::media::Reading {
        media: Some(sample_media()),
        volume: Some(orbiters_n4n0_core::menu::Volume { percent: 40, muted: false }),
        art: Some(Arc::new(crate::media::Art { key: SAMPLE_ART.into(), rgb: sample_art() })),
    });
    panels.stage = super::Stage::new([1, 1], false, &media);
    panels.media = media;
}

/// All of every part that a person can see and a pointer can reach, a line a part: where it is, whether it shows and how far in, what is pointed at and pressed on it, what
/// can be pressed on it, and its picture once it has one.
fn seen(panels: &Panels) -> Vec<String> {
    use std::hash::{Hash, Hasher};
    panels
        .all()
        .map(|p| {
            let mut picture = std::collections::hash_map::DefaultHasher::new();
            if p.painted {
                p.pixels().hash(&mut picture);
            }
            format!(
                "{:?} at {:?}, shown {}, painted {}, alpha {}, out {}, hover {:?}, pressed {:?}, items {:?}, picture {:x}", // i18n: not shown (test state comparison diagnostic)
                p.zone,
                p.placed,
                p.shown,
                p.painted,
                p.appear.alpha(),
                p.outward(),
                p.hover,
                p.pressed,
                p.items,
                picture.finish()
            )
        })
        .collect()
}

/// The architecture review's DRT-23: the parts of a menu that closed are kept for its next opening, each with its interface, its fonts and what it painted last. Whatever the
/// first opening was left in, the next one must be a first opening to the eye and to the pointer: here a menu is left with a sheet open and scrolled, a tile pointed at and
/// pressed and never let go, and closed under the hand; then opened again beside menus made that moment, and compared with them turn by turn: every part's place, whether it
/// shows, how far in it is, what is pointed at, pressed and pressable on it, and every pixel of its picture. In both looks, around both menus, and with the look changed
/// while it was closed.
#[test]
fn a_menu_opened_again_is_one_opened_for_the_first_time() {
    use orbiters_core::settings::MenuLook;
    for (look, look_after, around) in [
        (MenuLook::Modern, MenuLook::Modern, Around::QuickMenu),
        (MenuLook::Modern, MenuLook::Modern, Around::Wheel),
        (MenuLook::Retro, MenuLook::Retro, Around::QuickMenu),
        (MenuLook::Retro, MenuLook::Retro, Around::Wheel),
        (MenuLook::Modern, MenuLook::Retro, Around::QuickMenu),
        (MenuLook::Retro, MenuLook::Modern, Around::Wheel),
    ] {
        let case = format!("{look:?} then {look_after:?}, {around:?}");
        let mut person = Person::with_a_song();
        // No list of this computer's pictures on the sheet.
        fn none(_: &std::path::Path, _: &[std::path::PathBuf]) -> Vec<super::LoadedRecent> {
            Vec::new()
        }
        person.panels.recents.lister = Some(none);
        let mut snap = (*person.snap).clone();
        snap.settings.orbit.theme = look;
        person.snap = Arc::new(snap);
        person.around = Some(around);
        person.turns(40);
        assert!(person.panels.all().all(|p| p.painted), "{case}: the first opening");
        // A sheet, scrolled; then something pointed at, pressed, and held.
        person.press(Action::All);
        person.turns(4);
        assert_eq!(person.panels.facts.sheet, Some(Sheet::All), "{case}");
        person.panels.wheel(-2.0);
        person.turns(4);
        let held = person.find(Action::Pictures).or_else(|| person.find(Action::Back)).expect("something to hold");
        person.panels.hover(Some(held));
        person.turn();
        person.panels.press(&person.snap.clone());
        person.turn();
        assert!(person.panels.all().any(|p| p.pressed.is_some() || p.hover.is_some()), "{case}: held");
        // Closed under the hand.
        person.around = None;
        person.turns(3);
        assert!(person.panels.is_empty(), "{case}");
        let (kept, bytes) = person.panels.resting();
        assert!(kept >= Zone::around(around).len(), "{case}: {kept} parts kept");
        assert!(bytes <= super::RESTS_HOLD, "{case}: {bytes} bytes kept");
        // The look may change while it is closed (from the app's window, say).
        let mut snap = (*person.snap).clone();
        snap.settings.orbit.theme = look_after;
        person.snap = Arc::new(snap);
        // Opened again, beside one made now.
        let mut fresh = Panels::new(Arc::new(crate::screenshot::Still), 2.0, true);
        hears_a_song(&mut fresh);
        fresh.recents.lister = Some(none);
        for n in 0..120 {
            let time = 500.0 + n as f64 * 0.011;
            (person.panels.clock, fresh.clock) = (Some(time), Some(time));
            let again = turn_of(&mut person.panels, &person.snap, Some(around), 0);
            let first = turn_of(&mut fresh, &person.snap, Some(around), 0);
            assert_eq!(person.panels.facts.sheet, None, "{case}, turn {n}: the sheet does not come back");
            assert!(person.panels.takeover.is_none(), "{case}, turn {n}");
            if n == 0 {
                // What the platform holds of a part is replaced whole at every opening, as it was when each opening made its parts.
                assert_eq!(again.len(), person.panels.list.len(), "{case}: every part is handed over at once");
                assert!(again.iter().all(|(_, px, changed)| *changed == [0, 0, px[0], px[1]]), "{case}: and whole: {again:?}");
                assert_eq!(again.iter().map(|h| h.0).collect::<Vec<_>>(), first.iter().map(|h| h.0).collect::<Vec<_>>(), "{case}");
            }
            // From the first turn: a part made that moment has its whole picture at its first paint (the paint bench's log shows no part whose second differs).
            assert_eq!(seen(&person.panels), seen(&fresh), "{case}, turn {n}");
        }
        assert!(person.panels.all().all(|p| p.painted && p.appear.settled()), "{case}: and it came to rest");
    }
}

/// Where something that does `action` is on `panels`: its part's place in the list, and a point on it.
fn find_on(panels: &Panels, action: Action) -> Option<(usize, egui::Pos2)> {
    panels.all().enumerate().find_map(|(i, p)| p.items.iter().find(|item: &&Item| item.action == action).map(|item| (i, item.rect.center())))
}

/// Points at what does `action` on `panels`, presses and lets go, a turn each; the hand then goes.
fn press_on(panels: &mut Panels, snap: &Snapshot, around: Option<Around>, action: Action) {
    let at = find_on(panels, action).unwrap_or_else(|| panic!("nothing does {action:?}"));
    panels.hover(Some(at));
    turn_of(panels, snap, around, 0);
    panels.press(snap);
    turn_of(panels, snap, around, 0);
    panels.release();
    turn_of(panels, snap, around, 0);
    panels.hover(None);
    turn_of(panels, snap, around, 0);
}

/// What keeping a part's interface costs in exactness, and what [`Panels::fonts_anew`] buys. A part's letters are drawn from an atlas that fills as the part draws words,
/// so where a letter lies in it depends on what the part said before; the edge of a letter drawn from another place can come out a level or two of 255 apart. Here the
/// sheet's head and the caption say many things around the Quick Menu, and are then used again from the wheel, beside parts made that moment: with the interface kept, no
/// pixel of any part is more than a few levels from the new part's, and few are off at all; with it made anew, none is.
#[test]
fn a_kept_part_is_a_level_or_two_off_a_new_one_at_most_and_not_at_all_with_its_interface_anew() {
    // This sample corpus has an English atlas-history bound. Do not let an earlier
    // controller test or the computer's locale select a different corpus halfway
    // through its retained/fresh comparison. Pixel assertions remain unchanged.
    orbiters_i18n::in_language(orbiters_i18n::Language::En, || {
    fn none(_: &std::path::Path, _: &[std::path::PathBuf]) -> Vec<super::LoadedRecent> {
        Vec::new()
    }
    for anew in [false, true] {
        let mut person = Person::with_a_song();
        Arc::make_mut(&mut person.snap).settings.language = Some("en".into());
        person.panels.recents.lister = Some(none);
        person.panels.fonts_anew = anew;
        person.panels.clock = Some(1.0);
        let snap = person.snap.clone();
        // Around the Quick Menu: sheets, and everything beside it pointed at, so that the head and the caption say what each does.
        let quick_menu = Some(Around::QuickMenu);
        for _ in 0..40 {
            turn_of(&mut person.panels, &snap, quick_menu, 0);
        }
        for sheet in [Action::All, Action::Pictures, Action::Page(Target::Instances), Action::All, Action::Settings] {
            press_on(&mut person.panels, &snap, quick_menu, sheet);
            for action in [Action::Worlds, Action::Pictures, Action::Talk, Action::All, Action::PlayPause] {
                if let Some(at) = find_on(&person.panels, action) {
                    person.panels.hover(Some(at));
                    turn_of(&mut person.panels, &snap, quick_menu, 0);
                    turn_of(&mut person.panels, &snap, quick_menu, 0);
                }
            }
            person.panels.hover(None);
            for _ in 0..4 {
                turn_of(&mut person.panels, &snap, quick_menu, 0);
            }
        }
        for _ in 0..3 {
            turn_of(&mut person.panels, &snap, None, 0);
        }
        // From the wheel, beside menus made now.
        let mut fresh = Panels::new(Arc::new(crate::screenshot::Still), 2.0, true);
        hears_a_song(&mut fresh);
        fresh.recents.lister = Some(none);
        fresh.clock = Some(1.0);
        let wheel = Some(Around::Wheel);
        for panels in [&mut person.panels, &mut fresh] {
            for _ in 0..100 {
                turn_of(panels, &snap, wheel, 0);
            }
            press_on(panels, &snap, wheel, Action::All);
            for _ in 0..6 {
                turn_of(panels, &snap, wheel, 0);
            }
            press_on(panels, &snap, wheel, Action::Pictures);
            let began = Instant::now();
            while panels.recents.rx.is_some() || began.elapsed() < Duration::from_millis(50) {
                assert!(began.elapsed() < Duration::from_secs(10), "the list never came");
                std::thread::sleep(Duration::from_millis(2));
                turn_of(panels, &snap, wheel, 0);
            }
            for _ in 0..6 {
                turn_of(panels, &snap, wheel, 0);
            }
        }
        assert_eq!(person.panels.facts.sheet, Some(Sheet::Pictures));
        let (mut off, mut most, mut pixels) = (0usize, 0u8, 0usize);
        for (kept, new) in person.panels.all().zip(fresh.all()) {
            assert_eq!((kept.zone, kept.px, kept.painted, kept.shown), (new.zone, new.px, new.painted, new.shown));
            for (a, b) in kept.pixels().iter().zip(new.pixels()) {
                let apart = a.abs_diff(*b);
                off += (apart > 0) as usize;
                most = most.max(apart);
            }
            pixels += kept.pixels().len();
        }
        println!("interface anew {anew}: {off} of {pixels} bytes of the pictures differ, by {most} of 255 at most");
        if anew {
            assert_eq!((off, most), (0, 0), "made anew, every pixel is a new part's");
        } else {
            assert!(most <= 3, "kept, a letter's edge is {most} of 255 off a new part's");
            assert!(off * 500 <= pixels, "and {off} of {pixels} bytes are off at all");
        }
    }
    });
}

/// With a menu up and nothing changing, no part is due a picture. Every part was, every turn: the facts held a new empty gallery each turn, and galleries are compared by
/// being the same list, so the facts "changed" ninety times a second and every part was laid out and compared again to change no pixel (the paint bench counted them; the
/// architecture review's DRT-26). Around both menus, and on sheets.
#[test]
fn with_nothing_changing_no_part_is_painted() {
    fn none(_: &std::path::Path, _: &[std::path::PathBuf]) -> Vec<super::LoadedRecent> {
        Vec::new()
    }
    // One turn but for the painting: which parts are due a picture; then they are painted.
    fn due(person: &mut Person) -> Vec<Zone> {
        let mut places: Vec<Placed> = person.around.map_or_else(Vec::new, |m| Zone::around(m).iter().map(|z| zones::in_units(*z)).collect());
        person.panels.way_back(person.around, &mut places);
        person.panels.set_open(person.around, places, &person.snap);
        person.panels.follow(&person.snap, 0);
        person.panels.menu_turn(&person.snap);
        person.panels.step_springs(0.011);
        let due = person.panels.all().filter(|p| p.dirty).map(|p| p.zone).collect();
        person.panels.paint(&person.snap, |_, _| {});
        due
    }
    for (around, sheets) in [(Around::QuickMenu, &[][..]), (Around::QuickMenu, &[Action::All][..]), (Around::QuickMenu, &[Action::All, Action::Settings][..]), (Around::Wheel, &[][..]), (Around::Wheel, &[Action::All][..])] {
        let mut person = Person::with_a_song();
        person.panels.recents.lister = Some(none);
        person.around = Some(around);
        person.turns(3);
        for sheet in sheets {
            person.press(*sheet);
            person.turns(3);
        }
        // Everything in: the parts on their springs, the list read, the song's picture.
        person.turns_until_at_rest("its parts at rest");
        person.turns(10);
        for turn in 0..30 {
            assert_eq!(due(&mut person), [], "{around:?} {sheets:?}, turn {turn}: painted for nothing");
        }
        // And something that does change is still painted: the seconds to a 3D picture.
        let mut snap = (*person.snap).clone();
        snap.n4n0.photo_countdown = Some(3);
        person.snap = Arc::new(snap);
        assert!(!due(&mut person).is_empty(), "{around:?} {sheets:?}: a countdown");
    }
}

/// The flaky one above under load: All tools asks for the gallery's list, and Settings is pressed before the list thread answers (a
/// slow machine, here a list that waits). The list is taken only on a sheet that shows it, so the Settings sheet is at rest with it
/// pending; it was waited for until the clock ran out. The list is taken on the next showing of All tools.
#[test]
fn a_list_still_read_when_another_sheet_is_pressed_does_not_keep_the_menu_from_rest() {
    static LET_GO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    fn slow(_: &std::path::Path, _: &[std::path::PathBuf]) -> Vec<super::LoadedRecent> {
        let began = Instant::now();
        while !LET_GO.load(std::sync::atomic::Ordering::SeqCst) && began.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(2));
        }
        Vec::new()
    }
    LET_GO.store(false, std::sync::atomic::Ordering::SeqCst);
    let mut person = Person::with_a_song();
    person.panels.recents.lister = Some(slow);
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    person.press(Action::All);
    assert!(person.panels.recents.rx.is_some(), "All tools asked for the list");
    person.press(Action::Settings);
    person.turns(3);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Settings));
    let began = Instant::now();
    person.turns_until_at_rest("Settings, the list still being read");
    assert!(began.elapsed() < Duration::from_secs(60), "at rest without the list: {:?}", began.elapsed());
    assert!(person.panels.recents.rx.is_some(), "the list is still pending: nothing took it on Settings");
    LET_GO.store(true, std::sync::atomic::Ordering::SeqCst);
    person.panels.close_sheet();
    person.turns(3);
    person.press(Action::All);
    person.turns_until("the list taken on All tools", |p| p.panels.recents.rx.is_none());
}

/// What plays's picture arriving after its name (it is read apart, and can be large) is painted on All tools too. Only the parts around the menu were marked for it, and the
/// sheet's page had it because every part was painted every turn.
#[test]
fn the_song_s_picture_arriving_paints_all_tools_again() {
    use crate::overlay::menu_pages::{sample_art, sample_media};
    fn none(_: &std::path::Path, _: &[std::path::PathBuf]) -> Vec<super::LoadedRecent> {
        Vec::new()
    }
    let mut person = Person::with_a_song();
    person.panels.recents.lister = Some(none);
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    person.press(Action::All);
    person.turns_until_at_rest("its parts at rest");
    person.turns(10);
    assert_eq!(turn_of(&mut person.panels, &person.snap, person.around, 0), [], "at rest");
    // The same song, with another picture for it.
    let mut media = sample_media();
    media.art = Some("art:another".into());
    let mut rgb = sample_art();
    rgb.pixels.iter_mut().for_each(|v| *v = 255 - *v);
    person.panels.media.stand_in(crate::media::Reading { media: Some(media), volume: Some(orbiters_n4n0_core::menu::Volume { percent: 40, muted: false }), art: Some(Arc::new(crate::media::Art { key: "art:another".into(), rgb })) });
    let mut handed = Vec::new();
    for _ in 0..20 {
        handed.extend(turn_of(&mut person.panels, &person.snap, person.around, 0).into_iter().map(|h| h.0));
    }
    // The wing is away while a sheet is up (Enzo, 2026-11-11): All tools has what plays.
    assert!(handed.contains(&Zone::Takeover), "All tools: {handed:?}");
}

/// The parts kept are bounded: one for a zone, those in use not among them, a part of another size made anew in its zone's place, all of them let go after
/// [`super::RESTS_FOR`], and the ones kept longest first when they hold too much.
#[test]
fn the_parts_kept_are_one_a_zone_and_are_let_go() {
    let mut person = Person::with_a_song();
    let now = Instant::now();
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    assert_eq!(person.panels.resting().0, 0, "nothing closed yet");
    person.around = None;
    person.turns(2);
    // Its Stop is there only while a recording runs.
    let quick_menu = Zone::around(Around::QuickMenu).iter().filter(|z| !z.only_while_needed()).count();
    assert_eq!(person.panels.resting().0, quick_menu);
    // Opened and closed again: the same ones, not more.
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    assert_eq!(person.panels.resting().0, 0, "they are the menu's again");
    assert!(person.panels.list.iter().all(|p| !p.again && p.painted), "and painted for this opening");
    person.around = None;
    person.turns(2);
    assert_eq!(person.panels.resting().0, quick_menu);
    // The wheel's are other zones.
    person.around = Some(Around::Wheel);
    person.turns(3);
    person.around = None;
    person.turns(2);
    let both = quick_menu + Zone::around(Around::Wheel).iter().filter(|z| !z.only_while_needed()).count();
    assert_eq!(person.panels.resting().0, both);
    // A part asked for at another size (the game's window resized on a desktop) is made anew, and the one kept for its zone goes.
    let mut larger = zones::in_units(Zone::WingWorlds);
    larger.rect = egui::Rect::from_min_size(larger.rect.min, larger.rect.size() * 1.5);
    let part = person.panels.part(larger, 0, false);
    assert!(!part.again, "a new part");
    assert_eq!(person.panels.resting().0, both - 1);
    person.panels.rest(part);
    assert_eq!(person.panels.resting().0, both, "one for its zone");
    // The same size again: that one.
    let part = person.panels.part(larger, 0, false);
    assert!(part.again);
    person.panels.rest(part);
    // Too much held: the ones kept longest go first.
    let (_, bytes) = person.panels.resting();
    let newest = person.panels.rested.last().map(|r| r.panel.zone);
    person.panels.hold_at_most(bytes / 2);
    let (kept, held) = person.panels.resting();
    assert!(kept < both && held <= bytes / 2, "{kept} parts, {held} of {bytes} bytes");
    assert_eq!(person.panels.rested.last().map(|r| r.panel.zone), newest, "the newest stays");
    // And after a while with no opening, all of them.
    person.panels.let_go(now + super::RESTS_FOR - Duration::from_secs(60));
    assert_eq!(person.panels.resting().0, kept, "not yet");
    person.panels.let_go(Instant::now() + super::RESTS_FOR + Duration::from_secs(1));
    assert_eq!(person.panels.resting(), (0, 0));
    // An opening after that is a first one.
    person.around = Some(Around::QuickMenu);
    person.turn();
    assert!(person.panels.list.iter().all(|p| !p.again));
}

/// Two pictures of one world, taken together, as the gallery's list has them: under the name `world`.
fn two_of(world: &str) -> Vec<super::LoadedRecent> {
    ["one.png", "two.png"]
        .into_iter()
        .map(|file| {
            let path = std::path::PathBuf::from(file);
            super::LoadedRecent {
                source: crate::worlds::Picture::Shot { path: path.clone(), size: 1, width: 560 },
                key: path,
                world_id: "wrld_renamed".into(),
                title: world.into(),
                image: None,
                asked: false,
                has_view: false,
                session: false,
                picture: false,
                taken: 1_700_000_000,
                modified: 1_700_000_000,
                aspect: 16.0 / 9.0,
                at: None,
            }
        })
        .collect()
}

/// A set of one world's pictures open on the sheet, and the list read again with another name for that world (the visit history learns a name when the person joins; VRChat
/// renames worlds): the set is still open with both its pictures, under the new name. It showed empty until Back and opening it again (the second review of `window-quick`, F2).
#[test]
fn a_set_open_in_the_gallery_stays_when_its_world_is_renamed() {
    fn before(_: &std::path::Path, _: &[std::path::PathBuf]) -> Vec<super::LoadedRecent> {
        two_of("Sky Dream")
    }
    fn after(_: &std::path::Path, _: &[std::path::PathBuf]) -> Vec<super::LoadedRecent> {
        two_of("Sky Dream (Night)")
    }
    let mut person = Person::new();
    person.panels.recents.lister = Some(before);
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    person.press(Action::Pictures);
    person.turns_until("the gallery's list", |p| p.panels.facts.recent.iter().count() == 1);
    assert_eq!(person.panels.facts.recent.get(0).map(|tile| tile.count), Some(2), "one tile for the two");
    person.press(Action::Capture(0));
    person.turns_until("the set, opened", |p| p.panels.facts.gallery_set.is_some());
    person.turns(2);
    assert_eq!(person.panels.facts.gallery_set.as_deref(), Some("Sky Dream"));
    assert_eq!(person.panels.facts.recent.iter().count(), 2, "its two pictures");
    // The library changed, and the world has another name in the list read for it.
    person.panels.recents.lister = Some(after);
    person.library = 1;
    person.turns_until("the list read again", |p| p.panels.recents.loaded.iter().all(|r| r.title == "Sky Dream (Night)") && p.panels.recents.rx.is_none());
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Pictures));
    assert_eq!(person.panels.facts.recent.iter().count(), 2, "both are still in the set");
    assert_eq!(person.panels.facts.gallery_set.as_deref(), Some("Sky Dream (Night)"), "and its heading says the new name");
    // Back shows the whole gallery again, as it did.
    person.press(Action::Back);
    person.turns(2);
    assert_eq!(person.panels.facts.gallery_set, None);
    assert_eq!(person.panels.facts.recent.iter().count(), 1);
}

/// A panel on a sheet in VR is a headset's: a press on a capture goes to the 3D view, with Around me and Small here, and is never a flat picture on the page.
#[test]
fn the_panel_on_a_sheet_in_vr_is_a_headsets() {
    use crate::overlay::worlds::panel::Status;
    let mut worlds = crate::overlay::worlds::Worlds::alone(egui::Context::default());
    assert!(!worlds.status_for_test().headset, "alone, on a screen");
    let view = Status { headset: true, small: true, small_docked: true, shown: true, current: Some(7), ..Default::default() };
    worlds.relay_to(view);
    let status = worlds.status_for_test();
    assert!(status.headset && status.small && status.small_docked && status.shown);
    assert_eq!(status.current, Some(7));
}

/// Enzo: "if I made a research on the Youtube one and then went in the world page, when going back to the youtube one, I will still have the video results and scroll position".
#[test]
fn a_screen_is_as_it_was_left() {
    use orbiters_n4n0_core::menu::Page;
    let mut person = Person::new();
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    person.press(Action::Pictures);
    person.turns(2);
    // With no turn between: this empty list would bring it back within what it allows.
    person.panels.gallery_to = 60.0;
    person.panels.facts.gallery_scroll = 60.0;
    let snap = person.snap.clone();
    person.panels.open_sheet(Sheet::All, "All tools", &snap);
    assert_eq!(person.panels.facts.gallery_scroll, 0.0, "a screen of its own");
    person.panels.open_sheet(Sheet::Pictures, "Pictures", &snap);
    assert_eq!(person.panels.gallery_to, 60.0, "and that one where it was");
    person.panels.stage.host.menu.states_ready = true;
    person.panels.open_sheet(Sheet::Pictures, "Pictures", &snap);
    person.turns(2);
    // N4N0's page, on an inner page of it.
    person.press(Action::All);
    person.turns(2);
    person.press(Action::Page(Target::States));
    person.turns(2);
    person.panels.stage.host.menu.page = Page::NewState;
    person.press(Action::All);
    person.turns(2);
    person.press(Action::Page(Target::States));
    person.turns(2);
    assert_eq!(person.panels.stage.host.menu.page, Page::NewState);
}

/// Enzo: "a consistent back button on every page that will take us to the last screen". In one place, within a screen first, through the tabs, and off the sheet at the end.
#[test]
fn back_goes_to_the_last_screen_from_one_place() {
    use orbiters_n4n0_core::menu::Page;
    let mut person = Person::new();
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    person.press(Action::All);
    person.turns(2);
    let place = |person: &Person| person.panels.all().find_map(|p| p.items.iter().find(|i| i.action == Action::Back).map(|i| i.rect));
    person.panels.stage.host.menu.states_ready = true;
    person.turns(2);
    let at = place(&person).expect("a Back on All tools");
    person.press(Action::Page(Target::States));
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Menu));
    assert_eq!(place(&person), Some(at), "in the same place");
    // Further in it, it comes back within.
    person.panels.stage.host.menu.page = Page::NewState;
    person.turns(1);
    person.press(Action::Back);
    person.turns(2);
    assert_eq!(person.panels.stage.host.menu.page, Page::States);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Menu));
    person.press(Action::Back);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::All), "and on to the screen before");
    // Through the tiles and the tabs alike.
    person.press(Action::Pictures);
    person.turns(2);
    assert_eq!(place(&person), Some(at));
    person.press(Action::Back);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::All));
    person.press(Action::Back);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, None, "and off the sheet");
}

/// Enzo: "up down joystick ... scroll in scrollable pages". A stick at rest does nothing.
#[test]
fn the_stick_scrolls_what_is_on_the_sheet() {
    let mut person = Person::new();
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    person.press(Action::Page(Target::Instances));
    person.turns(2);
    person.panels.facts.worlds_visited = vec![Default::default(); 20];
    let max = super::panels::max_scroll(Sheet::Instances, &person.panels.facts);
    assert!(max > 100.0, "{max}");
    let snap = person.snap.clone();
    person.panels.stick(0.1, 0.1, &snap);
    assert_eq!(person.panels.gallery_to, 0.0, "at rest");
    person.panels.stick(-1.0, 0.1, &snap);
    assert_eq!(person.panels.gallery_to, 90.0, "down the page, pulled");
    person.panels.stick(1.0, 10.0, &snap);
    assert_eq!(person.panels.gallery_to, 0.0, "and no further than its start");
}

/// Enzo: "the hole is glitchy and I don't see the 3D view behind it". What is submitted for the sheet's page and its back, by the same painting as in a headset, read back at the hole. This says
/// the pictures are clear there: what becomes of them in the compositor only `vr-test.log` can say.
///
/// The page's hole holds one thing by design: in the modern look, a shade along its top edge ([`crate::overlay::worlds::panel::hole_shade`], the panel's thickness).
/// It is drawn with the world's details, so a hole whose world is not listed has none. This test once picked a world by an id only (7) and so met the shade
/// only when this computer's own world lists, loading in the background, happened to hold a world of that id by the time the hole opened: under load they
/// did, and its top row read 42 (the shade's fourth band), failing release runs from dev.45 on (done4-hole-test.md). The world is now given, so the page is
/// the one a person sees whatever loads meanwhile, and every pixel of the hole is checked against exactly what is meant there.
#[test]
fn the_hole_is_clear_in_what_is_submitted() {
    use crate::overlay::worlds::panel::{hole_shade, Status, HOLE_SHADE_BAND, HOLE_SHADE_BANDS};
    // A page's alpha at a point of it, in its units.
    fn alpha(panel: &super::Panel, x: f32, y: f32) -> u8 {
        let zoom = panel.px[0] as f32 / panel.logical.x;
        panel.pixels()[((y * zoom) as usize * panel.px[0] as usize + (x * zoom) as usize) * 4 + 3]
    }
    // What is wrong with the page's hole: below its shade (and clear of the frame's corner marks, which lie on its corners) nothing at all; in the shade, each
    // band exactly as dark as it is meant to be, and nothing of the page; in the retro look, no shade.
    fn page_hole(person: &Person, retro: bool) -> Vec<String> {
        let page = person.panels.takeover.as_ref().unwrap();
        let h = person.panels.stage.worlds.as_ref().and_then(|w| w.small_hole()).expect("the hole open");
        let shade = if retro { 0.0 } else { HOLE_SHADE_BAND * HOLE_SHADE_BANDS as f32 };
        let xs: Vec<f32> = (0..9).map(|i| h.left() + 40.0 + (h.width() - 80.0) * i as f32 / 8.0).collect();
        let mut wrong = Vec::new();
        // From 2 units in: the frame's edge is soft by a pixel (the retro look's restyle leaves 6 of 255 on the first unit in).
        let mut y = h.top() + shade + 2.0;
        while y < h.bottom() - 2.0 {
            for &x in &xs {
                let a = alpha(page, x, y);
                if a != 0 {
                    wrong.push(format!("({x}, {y}) {a} below the shade"));
                }
            }
            y += 4.0;
        }
        for band in 0..HOLE_SHADE_BANDS {
            let y = h.top() + (band as f32 + 0.5) * HOLE_SHADE_BAND;
            let meant = if retro { 0.0 } else { hole_shade(band) * 255.0 };
            for &x in &xs {
                let a = alpha(page, x, y);
                if (a as f32 - meant).abs() > 1.5 {
                    wrong.push(format!("({x}, {y}) {a} in shade band {band}, meant {meant:.0}"));
                }
            }
        }
        wrong
    }
    // At rest, and with its side panel still on its way in (a capture shows docked at once, as a world is pressed): in the same turn the hole opens, all of it
    // is as it should be on the page and clear on its back. dev.21's run once found the page still solid there: the page waited for its own 22 ms while the back did not.
    for (retro, sliding) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut person = Person::new();
        let mut snap = (*person.snap).clone();
        if retro {
            snap.settings.orbit.theme = orbiters_core::settings::MenuLook::Retro;
        }
        person.snap = Arc::new(snap);
        person.around = Some(Around::QuickMenu);
        person.turns(3);
        person.press(Action::Worlds);
        person.turns(2);
        let found = orbiters_core::maps::FoundWorld { world: orbiters_core::maps::World { world_id: "wrld_hole".into(), name: "Hole".into(), ..Default::default() }, ..Default::default() };
        let entry = crate::overlay::worlds::catalog::Entry::world(&found, None, Default::default());
        let id = entry.id;
        let worlds = person.panels.stage.worlds.as_mut().unwrap();
        worlds.pick_for_test(entry);
        if sliding {
            worlds.slide_world_in_for_test(id);
        }
        // Painted just now, without its hole: the next paint is within the page's own 22 ms, whatever the machine's speed.
        person.turn();
        let docked = Status { headset: true, small: true, small_docked: true, shown: true, current: Some(id), ..Default::default() };
        let _ = person.panels.world_relay(docked.clone());
        person.turn();
        let [page_middle, page_outside, back_middle, back_outside] = person.panels.hole_alphas();
        let (_, back) = person.panels.hole_alphas_across();
        println!("hole, retro {retro} sliding {sliding}: page {page_middle:?} outside {page_outside:?}, back {back_middle:?} outside {back_outside:?}; back across {back:?}");
        let wrong = page_hole(&person, retro);
        assert!(wrong.is_empty(), "the page, retro {retro} sliding {sliding}: {wrong:?}");
        assert!(!back.is_empty() && back.iter().all(|a| *a == Some(0)), "its back, retro {retro} sliding {sliding}: {back:?}");
        assert!(page_outside.is_some_and(|a| a > 200), "and solid around it: {page_outside:?}");
        assert!(back_outside.is_some_and(|a| a > 200), "{back_outside:?}");
        // And it stays so, turn after turn.
        for _ in 0..6 {
            let _ = person.panels.world_relay(docked.clone());
            person.turn();
        }
        let wrong = page_hole(&person, retro);
        let (_, back) = person.panels.hole_alphas_across();
        assert!(wrong.is_empty() && back.iter().all(|a| *a == Some(0)), "later, retro {retro} sliding {sliding}: {wrong:?} {back:?}");
    }
}

/// A cache budget smaller than its newest part must evict that part too.
#[test]
fn a_resting_part_larger_than_the_budget_is_dropped() {
    let mut person = Person::new();
    let panel = super::Panel::new(zones::in_units(Zone::WingWorlds), 2.0, 0, false);
    let bytes = panel.bytes();
    assert!(bytes > 0);
    person.panels.rest(panel);
    person.panels.hold_at_most(bytes - 1);
    assert_eq!(person.panels.resting(), (0, 0));
}

/// Removing artwork must repaint even when the song and its position stay unchanged.
#[test]
fn the_song_s_picture_disappearing_paints_all_tools_again() {
    use crate::overlay::menu_pages::sample_media;
    fn none(_: &std::path::Path, _: &[std::path::PathBuf]) -> Vec<super::LoadedRecent> {
        Vec::new()
    }
    let mut person = Person::with_a_song();
    person.panels.recents.lister = Some(none);
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    person.press(Action::All);
    person.turns_until_at_rest("its parts at rest");
    person.turns(10);
    assert_eq!(turn_of(&mut person.panels, &person.snap, person.around, 0), [], "at rest");
    // Only the artwork disappears; all other facts stay the same.
    let mut media = sample_media();
    media.art = None;
    person.panels.media.stand_in(crate::media::Reading { media: Some(media), volume: Some(orbiters_n4n0_core::menu::Volume { percent: 40, muted: false }), art: None });
    let mut handed = Vec::new();
    for _ in 0..20 {
        handed.extend(turn_of(&mut person.panels, &person.snap, person.around, 0).into_iter().map(|h| h.0));
    }
    // The wing is away while a sheet is up (Enzo, 2026-11-11): All tools has what plays.
    assert!(handed.contains(&Zone::Takeover), "All tools: {handed:?}");
    // An announced image which has not arrived must not bring back painting every turn.
    let mut media = sample_media();
    media.art = Some("art:unavailable".into());
    person.panels.media.stand_in(crate::media::Reading { media: Some(media), volume: Some(orbiters_n4n0_core::menu::Volume { percent: 40, muted: false }), art: None });
    person.turns(10);
    for _ in 0..10 {
        person.panels.follow(&person.snap, 0);
        assert!(person.panels.all().all(|p| !p.dirty), "unavailable art must stay at rest");
        person.turn();
    }

}


/// REQUEST19 2: YouTube's page has History in its own head row (on its search in the place of the menu's own Back, on its
/// results before the search; nowhere else); it opens the page of the videos played in worlds with YouTube's tab still lit; a
/// video there is copied and dropped through N4N0's menu's own commands, as a result is; and the head's Back or the page's own
/// way back return to YouTube as it was.
#[test]
fn youtube_s_history_opens_plays_again_and_goes_back() {
    use super::panels::{history_sheet, Look};
    use crate::ipc::N4n0Command;
    use orbiters_n4n0_core::menu::Page;
    // Points at the takeover's page at `at` (its units), presses and lets go, a turn each.
    fn press_page_at(person: &mut Person, at: egui::Pos2) {
        let page = person.panels.all().count() - 1;
        person.panels.hover(Some((page, at)));
        person.turn();
        let snap = person.snap.clone();
        person.panels.press(&snap);
        person.turn();
        person.panels.release();
        person.panels.hover(None);
        person.turns(3);
    }
    let (mut person, heard) = Person::heard();
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    person.panels.history.stand_in(history_sheet::sample(40, 1_800_000_000));
    assert!(person.find(Action::VideoHistory).is_none(), "no History with no page up");
    person.press(Action::Page(Target::YouTube));
    person.turns(2);
    assert_eq!((person.panels.facts.sheet, person.panels.facts.sheet_title.as_str(), person.panels.stage.host.menu.page), (Some(Sheet::Menu), "YouTube", Page::Search));
    assert!(history_sheet::on_search(&person.panels.stage.host.menu), "History where the menu's own Back would be");
    // Pointed at, the caption names it; pressed, its page.
    let page = person.panels.all().count() - 1;
    person.panels.hover(Some((page, history_sheet::SEARCH_BUTTON.center())));
    person.turn();
    assert_eq!(person.panels.facts.pointed, Some(Action::VideoHistory), "the caption says what the icon is");
    person.panels.hover(None);
    person.turn();
    press_page_at(&mut person, history_sheet::SEARCH_BUTTON.center());
    assert_eq!(person.panels.facts.sheet, Some(Sheet::VideoHistory));
    assert!(super::panels::tab_lit(&person.panels.facts, Action::Page(Target::YouTube)), "YouTube's tab stays lit on its History");
    let h = &person.panels.facts.history;
    assert_eq!(h.count, 40);
    assert_eq!(h.rows.first().map(|r| (r.title.as_str(), r.link_only)), Some(("stream.example.invalid/live/sample-39.m3u8", true)), "the latest first, a stream by its link");
    // Played again: its link to the clipboard, for the world's player.
    heard.n4n0.lock().unwrap().clear();
    person.press(Action::HistoryCopy(0));
    person.turns(1);
    assert!(heard.n4n0.lock().unwrap().iter().any(|c| matches!(c, N4n0Command::CopyText { text } if text == "https://stream.example.invalid/live/sample-39.m3u8")), "{:?}", heard.said());
    assert_eq!(person.panels.facts.history.copied, Some(0), "the row says Copied");
    // Drop in world: nothing where the avatar cannot drop a card, a card where it can.
    let can_drop = |person: &mut Person, can: bool| {
        let mut snap = (*person.snap).clone();
        snap.n4n0.can_drop = can;
        person.snap = Arc::new(snap);
        person.turns(2);
    };
    can_drop(&mut person, false);
    person.press(Action::HistoryDrop(1));
    assert!(!heard.n4n0.lock().unwrap().iter().any(|c| matches!(c, N4n0Command::Drop { .. })));
    can_drop(&mut person, true);
    person.press(Action::HistoryDrop(1));
    assert!(heard.n4n0.lock().unwrap().iter().any(|c| matches!(c, N4n0Command::Drop { link, video, .. } if link == "https://youtu.be/sample00038" && video.as_deref() == Some("sample00038"))), "{:?}", heard.said());
    // Back to YouTube by the head's Back.
    person.press(Action::Back);
    person.turns(2);
    assert_eq!((person.panels.facts.sheet, person.panels.facts.sheet_title.as_str()), (Some(Sheet::Menu), "YouTube"), "back on YouTube's search");
    // From YouTube's results: History before the search, there and back by the page's own way back, the results kept.
    person.panels.stage.host.menu.results = crate::overlay::menu_pages::sample_videos();
    person.panels.stage.host.menu.page = Page::Results;
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Results));
    assert!(!history_sheet::on_search(&person.panels.stage.host.menu));
    person.press(Action::VideoHistory);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::VideoHistory));
    let page_back = person.panels.takeover.as_ref().and_then(|p| p.items.iter().find(|i| i.look == Look::HistoryBack).map(|i| i.rect)).expect("the page's own way back to YouTube");
    press_page_at(&mut person, page_back.center());
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Results), "the page's own way back: YouTube's results, as they were");
    assert_eq!(person.panels.stage.host.menu.results.len(), crate::overlay::menu_pages::sample_videos().len());
    // Other pages have no History.
    person.press(Action::Pictures);
    person.turns(2);
    assert!(person.find(Action::VideoHistory).is_none(), "History is YouTube's alone");
}

#[test]
fn unchanged_results_stop_painting_after_the_host_update_is_captured() {
    let mut person = Person::with_a_song();
    let mut song = crate::overlay::menu_pages::sample_media();
    song.playing = false;
    person.panels.media.stand_in(crate::media::Reading { media: Some(song), volume: None, art: None });
    person.panels.stage.async_pages = true;
    person.around = Some(Around::QuickMenu);
    person.turns(120);
    person.panels.open_sheet(Sheet::Results, "YouTube", &person.snap);
    person.panels.stage.host.open(&person.snap);
    person.panels.stage.host.menu.page = orbiters_n4n0_core::menu::Page::Results;
    person.turns_until("the unchanged Results page rests", |p| {
        assert_eq!(p.panels.facts.sheet, Some(Sheet::Results), "the fixture must remain on Results");
        !p.panels.stage.host.dirty && !p.panels.stage.painter.busy() && p.panels.takeover.as_ref().is_some_and(|panel| panel.painted && !panel.dirty)
    });
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(11));
        let handed = turn_of(&mut person.panels, &person.snap, person.around, person.library);
        assert!(!handed.iter().any(|(zone, _, _)| *zone == Zone::Takeover), "unchanged result data produced another page frame");
    }
}


#[test]
fn unchanged_world_video_reuses_its_page_texture() {
    let mut person = Person::with_a_song();
    person.panels.stage.async_pages = true;
    person.around = Some(Around::QuickMenu);
    person.turns(120);
    person.panels.open_sheet(Sheet::All, "", &person.snap);
    person.panels.stage.host.open(&person.snap);
    person.panels.stage.host.put_picture("abcdefghijk", egui::ColorImage::filled([64, 36], egui::Color32::RED));
    let mut snap = (*person.snap).clone();
    snap.n4n0.world_video = Some(("https://www.youtube.com/watch?v=abcdefghijk".into(), Some("test video".into()), None));
    person.snap = Arc::new(snap);
    person.turn();
    let key = "world-video abcdefghijk";
    let first = person.panels.takeover.as_ref().unwrap().pictures.get(key).unwrap().id();
    person.panels.takeover.as_mut().unwrap().dirty = true;
    person.turn();
    assert_eq!(person.panels.takeover.as_ref().unwrap().pictures.get(key).unwrap().id(), first, "unchanged video recreated its texture");
}

/// Enzo (F13, F15): nothing on the wheel, the Quick Menu or a sheet starts a recording; beside VRChat's camera, Record does. While one runs, Stop is in reach on the
/// wheel, beside the Quick Menu, on a sheet's head and at the camera; pressed, it says Stopping at once and a second press does nothing.
#[test]
fn a_recording_starts_beside_the_camera_alone_and_stops_from_anywhere() {
    use crate::state::CaptureState;
    let mut person = Person::new();
    for around in [Around::Wheel, Around::QuickMenu] {
        person.around = Some(around);
        person.turns(3);
        assert!(person.find(Action::Record).is_none(), "{around:?} offers to record");
        assert!(person.find(Action::StopRecording).is_none(), "{around:?} has a Stop with nothing to stop");
    }
    person.press(Action::Pictures);
    person.turns(3);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Pictures));
    assert!(person.find(Action::Record).is_none(), "the Pictures sheet offers to record");
    person.around = None;
    person.turns(2);
    person.around = Some(Around::Camera);
    person.turns(3);
    assert!(person.find(Action::Record).is_some(), "no Record beside the camera");

    let mut snap = (*person.snap).clone();
    snap.capture.state = CaptureState::Recording;
    person.snap = Arc::new(snap);
    for around in [Around::Wheel, Around::QuickMenu, Around::Camera] {
        person.around = Some(around);
        person.turns(3);
        assert!(person.find(Action::StopRecording).is_some(), "no Stop {around:?}");
        assert!(person.find(Action::Record).is_none(), "a start in disguise {around:?}");
    }
    person.around = Some(Around::QuickMenu);
    person.turns(2);
    person.press(Action::All);
    person.turns(3);
    let head = person.panels.all().find(|p| p.zone == Zone::SheetBack).expect("a sheet's head");
    assert!(head.items.iter().any(|i| i.action == Action::StopRecording), "no Stop on the sheet's head");
    let pressed = person.press(Action::StopRecording);
    assert!(pressed.clicked);
    person.turns(2);
    assert!(person.panels.facts.rec_stopping, "it says Stopping at once");
    assert!(person.find(Action::StopRecording).is_none(), "pressed, nothing more to press");
    assert!(person.find(Action::Record).is_none());
}

/// Enzo (F16): beside the camera, an avatar with no capture kit has its two 3D pictures greyed, saying why, and a press takes none; one not known yet is not said
/// to have none.
#[test]
fn the_camera_s_3d_pictures_say_the_kit_is_missing_only_when_it_is() {
    use super::panels::camera_words;
    let mut person = Person::new();
    person.around = Some(Around::Camera);
    for (kit, missing) in [(None, false), (Some(true), false), (Some(false), true)] {
        let mut snap = (*person.snap).clone();
        snap.worn_kit = kit;
        person.snap = Arc::new(snap);
        person.turns(3);
        assert_eq!(person.panels.facts.no_kit, missing, "{kit:?}");
        assert_eq!(person.panels.facts.camera.kit, kit);
        let said = camera_words(&person.panels.facts).map(|w| w.0);
        assert_eq!(said.as_deref() == Some(orbiters_i18n::tr("This avatar has no capture kit")), missing, "{kit:?}: {said:?}");
    }
    person.press(Action::Picture(orbiters_n4n0_core::menu::PhotoMode::People));
    assert_eq!(person.panels.facts.picture_mode, None, "a greyed 3D picture took one");
}

/// Enzo (F17): the camera's QR code opens no page: it looks around the camera, with corners round its picture, and a second press stops it.
#[test]
fn the_camera_s_qr_code_looks_around_the_camera_and_opens_no_page() {
    use super::panels::CameraQr;
    let mut person = Person::new();
    person.around = Some(Around::Camera);
    person.turns(3);
    assert!(!person.panels.all().any(|p| p.zone == Zone::CameraScan), "corners with nothing looked for");
    person.press(Action::CameraQr);
    // What the camera's own scan says once it runs (camera-core).
    let mut snap = (*person.snap).clone();
    snap.camera_tools.qr = crate::controller::camera_tools::Qr::Looking;
    person.snap = Arc::new(snap);
    person.turns(3);
    assert_eq!(person.panels.facts.sheet, None, "the QR code page opened");
    assert!(person.panels.takeover.is_none());
    assert_eq!(person.panels.facts.camera.qr, CameraQr::Looking);
    assert!(person.panels.all().any(|p| p.zone == Zone::CameraScan && p.shown), "no corners round the camera");
    person.press(Action::CameraQr);
    let mut snap = (*person.snap).clone();
    snap.camera_tools.qr = crate::controller::camera_tools::Qr::Off;
    person.snap = Arc::new(snap);
    person.turns(3);
    assert_eq!(person.panels.facts.camera.qr, CameraQr::Off);
    assert!(!person.panels.all().any(|p| p.zone == Zone::CameraScan), "the corners stayed");
}

/// Request 18 (Enzo: "the page linked by the QR code often takes a while to load"): while the camera looks for a code in a headset an engine is started
/// ahead and the code's page opens in it; the same code read again keeps its page as it is; the camera no longer looking keeps no engine; on the desktop
/// (pages open in the browser) none is started.
#[test]
fn an_engine_waits_while_the_camera_looks_and_the_same_code_keeps_its_page() {
    use crate::controller::camera_tools::Qr;
    let mut person = Person::new();
    person.around = Some(Around::Camera);
    let set = |person: &mut Person, change: &dyn Fn(&mut Snapshot)| {
        let mut snap = (*person.snap).clone();
        change(&mut snap);
        person.snap = Arc::new(snap);
    };
    person.turns(2);
    assert!(person.panels.web_spare.is_none(), "nothing looks: no engine");
    set(&mut person, &|s| {
        s.camera_open_said = Some(true);
        s.camera_tools.qr = Qr::Looking;
        s.camera_tools.qr_generation = 1;
    });
    person.turns(2);
    assert!(person.panels.web_spare.is_some(), "an engine waits while the camera looks");
    let link = "https://linktr.ee/someone";
    set(&mut person, &|s| {
        s.camera_tools.qr = Qr::Found { url: link.into() };
        s.camera_tools.qr_generation = 2;
    });
    person.turns(2);
    assert!(person.panels.web_spare.is_none(), "the page took it");
    assert_eq!(person.panels.web.as_ref().map(|w| w.asked.as_str()), Some(link));
    // The page as the person left it (here: named), then the same code read by a new scan: kept.
    person.panels.web.as_mut().unwrap().title = "Someone's links".into();
    set(&mut person, &|s| s.camera_tools.qr = Qr::Looking);
    person.turns(2);
    assert!(person.panels.web_spare.is_some(), "looking again: an engine waits again");
    set(&mut person, &|s| {
        s.camera_tools.qr = Qr::Found { url: link.into() };
        s.camera_tools.qr_generation = 3;
    });
    person.turns(2);
    assert_eq!(person.panels.web.as_ref().map(|w| w.title.as_str()), Some("Someone's links"), "the same link keeps its page");
    // Another link opens anew.
    set(&mut person, &|s| {
        s.camera_tools.qr = Qr::Found { url: "https://orbiters.cc/w/another".into() };
        s.camera_tools.qr_generation = 4;
    });
    person.turns(2);
    assert_eq!(person.panels.web.as_ref().map(|w| (w.asked.as_str(), w.title.as_str())), Some(("https://orbiters.cc/w/another", "")));
    // Off: no engine kept.
    set(&mut person, &|s| s.camera_tools.qr = Qr::Off);
    person.turns(2);
    assert!(person.panels.web_spare.is_none());
    // On the desktop a code's page opens in the browser: no engine is started for it.
    person.panels.facts.desktop = true;
    set(&mut person, &|s| s.camera_tools.qr = Qr::Looking);
    person.turns(2);
    assert!(person.panels.web_spare.is_none(), "no engine on the desktop");
}

/// Enzo (F17): a code found beside the camera opens its page over the camera, once for that scan, with its bar; closed, it does not come back for the same code;
/// VRChat's camera closing takes it away.
#[test]
fn a_code_found_beside_the_camera_opens_its_page_over_it_once() {
    use crate::controller::camera_tools::Qr;
    let mut person = Person::new();
    person.around = Some(Around::Camera);
    let mut snap = (*person.snap).clone();
    snap.camera_open_said = Some(true);
    person.snap = Arc::new(snap);
    person.turns(3);
    assert!(!person.panels.all().any(|p| p.zone == Zone::CameraWeb), "a page with no code");
    let mut snap = (*person.snap).clone();
    snap.camera_tools.qr = Qr::Found { url: "https://orbiters.cc/w/silent-dawn".into() };
    snap.camera_tools.qr_generation = 3;
    person.snap = Arc::new(snap);
    person.turns(3);
    for zone in [Zone::CameraWeb, Zone::CameraWebBar] {
        assert!(person.panels.all().any(|p| p.zone == zone && p.shown), "no {zone:?}");
    }
    assert_eq!(person.panels.facts.camera.web.as_ref().map(|w| w.url.as_str()), Some("https://orbiters.cc/w/silent-dawn"));
    assert_eq!(person.panels.facts.sheet, None, "the QR code page opened");
    assert!(person.find(Action::WebOpen).is_some() && person.find(Action::WebClose).is_some());
    person.press(Action::WebClose);
    person.turns(3);
    assert!(person.panels.facts.camera.web.is_none(), "it did not close");
    assert!(!person.panels.all().any(|p| p.zone == Zone::CameraWeb), "it came back for the same code");
    // Another scan finds one: it opens; the camera closes: it goes.
    let mut snap = (*person.snap).clone();
    snap.camera_tools.qr_generation = 4;
    person.snap = Arc::new(snap);
    person.turns(3);
    assert!(person.panels.facts.camera.web.is_some(), "a new scan's code did not open");
    let mut snap = (*person.snap).clone();
    snap.camera_open_said = Some(false);
    person.snap = Arc::new(snap);
    person.turns(3);
    assert!(person.panels.facts.camera.web.is_none(), "the page outlived the camera");
}

/// Enzo (F16): the demo avatar is offered beside the camera only to someone signed in to VRChat through Orbiters as the one playing, with an avatar to put on and
/// no kit on theirs; while it switches it says so and takes no second press.
#[test]
fn the_demo_avatar_is_offered_only_where_it_can_be_put_on() {
    use crate::controller::camera_tools::Demo;
    use super::panels::DemoSwitch;
    let mut person = Person::new();
    person.around = Some(Around::Camera);
    let mut snap = (*person.snap).clone();
    snap.worn_kit = Some(false);
    snap.vrchat.running = true;
    snap.camera_tools.demo_avatar = Some("avtr_demo".into());
    person.snap = Arc::new(snap);
    person.turns(3);
    assert_eq!(person.panels.facts.camera.demo, None, "offered while signed out");
    assert!(person.find(Action::DemoAvatar).is_none());
    let mut snap = (*person.snap).clone();
    snap.vrchat_login.allowed = true;
    snap.vrchat_login.user = Some(orbiters_core::vrchat_api::User { id: "usr_me".into(), display_name: "Me".into(), ..Default::default() });
    snap.vrchat.world.local_player = Some("Me".into());
    snap.vrchat.world.players.insert("usr_me".into(), "Me".into());
    person.snap = Arc::new(snap);
    person.turns(3);
    assert_eq!(person.panels.facts.camera.demo, Some(DemoSwitch::Offered));
    assert!(person.find(Action::DemoAvatar).is_some(), "no button where it can be done");
    // Another account than the one playing: not offered.
    let mut other = (*person.snap).clone();
    other.vrchat_login.user = Some(orbiters_core::vrchat_api::User { id: "usr_other".into(), ..Default::default() });
    let mine = std::mem::replace(&mut person.snap, Arc::new(other));
    person.turns(3);
    assert_eq!(person.panels.facts.camera.demo, None, "offered to another account than the one playing");
    person.snap = mine;
    person.turns(3);
    let mut snap = (*person.snap).clone();
    snap.camera_tools.demo = Demo::Busy;
    person.snap = Arc::new(snap);
    person.turns(3);
    assert_eq!(person.panels.facts.camera.demo, Some(DemoSwitch::Busy));
    assert!(person.find(Action::DemoAvatar).is_none(), "a second press while it switches");
    // No avatar to put on (none configured): nothing offered, whatever else.
    let mut snap = (*person.snap).clone();
    snap.camera_tools.demo = Demo::Idle;
    snap.camera_tools.demo_avatar = None;
    person.snap = Arc::new(snap);
    person.turns(3);
    assert_eq!(person.panels.facts.camera.demo, None);
}

/// Enzo (VR feedback F6): "Clicking Write a comment does nothing". A capture's page on the sheet in VR, small in its frame as he had it, its comments opened and
/// Write a comment pressed, all through the hand's pointer and the host as `vr.rs` drives them, with the world's feedback never read: the development server
/// answered 500 to every read, and Write a comment waited for that read, saying so for a few seconds in a corner of the page. A comment needs no read: signed in,
/// the keyboard opens; signed out, the button says Sign in to comment, says why beside itself when pressed, and starts signing in.
#[test]
fn write_a_comment_opens_its_keyboard_on_the_sheet_in_vr() {
    use crate::overlay::worlds::panel::{Compose, Page, Status, Target as WorldTarget};
    use orbiters_n4n0_core::feedback::FeedbackView;
    const LISTING: &str = r#"{"previews":[{"slot":0,"key":1,"id":7,"worldId":"wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd","name":"Silent Dawn","author":"Mei","uploader":"a","view":{"position":[0,1,0],"target":[0,1,1]}}]}"#;
    for (signed_in, worker) in [(true, false), (false, false), (true, true), (false, true)] {
        let (mut person, heard) = Person::heard();
        person.panels.stage.async_pages = worker;
        let mut snap = (*person.snap).clone();
        snap.n4n0.feedback = FeedbackView { signed_in, ..Default::default() };
        person.snap = Arc::new(snap);
        person.around = Some(Around::QuickMenu);
        person.turns(3);
        person.press(Action::Worlds);
        person.turns(2);
        assert_eq!(person.panels.facts.sheet, Some(Sheet::Worlds));
        let entry = crate::overlay::worlds::catalog::parse(LISTING).unwrap().remove(0);
        // Listed, as a capture shown around the person is (verify2-r15 blocker 1: one the listing does not have shows nowhere).
        person.panels.stage.worlds.as_ref().unwrap().listing_for_test(vec![entry.clone()]);
        person.panels.stage.worlds.as_mut().unwrap().pick_for_test(entry);
        let docked = Status { headset: true, small: true, small_docked: true, shown: true, current: Some(7), ..Default::default() };
        let _ = person.panels.world_relay(docked.clone());
        person.turns(2);
        let page = |person: &Person| person.panels.stage.worlds.as_ref().unwrap().page_for_test();
        person.press_world(WorldTarget::Comments, &docked);
        assert_eq!(page(&person), Page::Comments, "signed in {signed_in}: its comments");
        person.press_world(WorldTarget::WriteComment, &docked);
        if signed_in {
            assert_eq!(page(&person), Page::Compose(Compose::Comment), "the keyboard, though the world's feedback was never read");
            assert!(person.panels.stage.worlds.as_ref().unwrap().typing(), "the computer's keys type there too");
            // Physical keys and a laser key arrive while older pictures may still be pending.
            let worlds = person.panels.stage.worlds.as_mut().unwrap();
            worlds.keys(&[]);
            worlds.keys(&[(1, orbiters_n4n0_core::menu::KeyInput::Text("hello".into()))]);
            person.press_world(WorldTarget::Space, &docked);
            person.press_world(WorldTarget::Key('a'), &docked);
            assert_eq!(person.panels.stage.worlds.as_ref().unwrap().draft_for_test(), "hello a");

        } else {
            assert_eq!(page(&person), Page::Comments, "nothing to write as, yet");
            assert!(person.panels.stage.worlds.as_ref().unwrap().target_for_test(WorldTarget::WriteComment).is_some(), "the way to sign in stays there");
            assert_eq!(heard.sign_ins.load(std::sync::atomic::Ordering::Relaxed), 1, "and the press started signing in");
        }
    }
}

/// Enzo (VR feedback F10): "world info from the previous-instance rows". A row of Instances opens its world's page on the 3D worlds sheet, a world with a 3D
/// capture as its capture's page and one without as its world's; opening it invites nobody; Back comes to Instances where it was scrolled to; its invite chips
/// still invite.
#[test]
fn a_previous_instance_opens_its_world_s_page_and_back_comes_to_where_it_was() {
    use crate::overlay::worlds::panel::Page;
    use orbiters_n4n0_core::menu::InstanceRow;
    // The second world has a 3D capture on Orbiters.
    let world = |i: usize| format!("wrld_00000000-0000-4000-8000-{i:012}");
    // The world the scrolled list shows first has a 3D capture on Orbiters.
    let listing = |n: usize| {
        format!(r#"{{"previews":[{{"slot":0,"key":1,"id":7,"worldId":"{}","name":"World","author":"Mei","uploader":"a","view":{{"position":[0,1,0],"target":[0,1,1]}}}}]}}"#, world(n))
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let mut person = Person::new();
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    person.press(Action::Page(Target::Instances));
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Instances));
    let rows: Vec<InstanceRow> = (0..12).map(|i| InstanceRow { location: format!("{}:1234{i}~region(eu)", world(i)), world: format!("World {i}"), at: now - i as u64 * 3600 }).collect();
    for capture in [false, true] {
        // These instances, whatever this computer's history has (opening Instances reads it).
        person.panels.stage.host.menu.set_instances(rows.clone(), true, "");
        person.turns(2);
        assert_eq!(person.panels.facts.worlds_visited.len(), 12);
        // Scrolled down by the wheel, as a person reading the list is, and come to rest (a list still gliding takes a press for a drag).
        if capture {
            person.panels.wheel(-2.0);
            person.turns_until("the list at rest", |p| (p.panels.facts.gallery_scroll - p.panels.gallery_to).abs() < 0.01 && p.panels.throw == 0.0);
            person.turns(2);
        }
        let scroll = person.panels.gallery_to;
        assert!(!capture || scroll > 0.0, "{scroll}");
        // The first row wholly in sight (past the list's head).
        let n = (0..12).find(|&i| person.find(Action::WorldInfo(i)).is_some_and(|(_, at)| at.y > 120.0)).expect("a row in sight");
        let listed = if capture { vec![crate::overlay::worlds::catalog::parse(&listing(n)).unwrap().remove(0)] } else { Vec::new() };
        person.panels.stage.worlds.as_ref().expect("the worlds' panel").listing_for_test(listed);
        person.press(Action::WorldInfo(n));
        person.turns(2);
        assert_eq!(person.panels.facts.sheet, Some(Sheet::Worlds), "world {n}: its page");
        let shown = person.panels.stage.worlds.as_ref().unwrap().shown_world_for_test();
        assert_eq!(shown, Some((Page::Detail, world(n), capture)), "world {n}");
        assert!(person.panels.invited.is_none() && !person.panels.stage.host.menu.invite_waiting, "world {n}: nobody invited");
        // Back: Instances, as far down as it was, at once (before this computer's history replaces the rows).
        let back = person.find(Action::Back).expect("the sheet's Back");
        person.panels.hover(Some(back));
        person.turn();
        let snap = person.snap.clone();
        person.panels.press(&snap);
        assert_eq!(person.panels.facts.sheet, Some(Sheet::Instances), "world {n}: back where it was opened from");
        assert_eq!(person.panels.gallery_to, scroll, "world {n}: as far down as it was");
        person.panels.release();
        person.panels.hover(None);
        person.turn();
    }
    person.panels.stage.host.menu.set_instances(rows.clone(), true, "");
    person.turns(2);
    // The chips still invite.
    let shown = (0..12).find(|&i| person.find(Action::Invite(i, 0)).is_some_and(|(_, at)| at.y > 120.0)).expect("an invite in sight");
    person.press(Action::Invite(shown, 0));
    assert!(person.panels.invited.is_some(), "an invite asked for");
}

/// Enzo (VR feedback F3): with no N4N0 link card on the avatar, Drop link is greyed, says why when pointed at, and a press does nothing; above all it no longer
/// marks the link copied when the clipboard never had it.
#[test]
fn drop_link_without_the_avatar_s_card_says_why_and_does_nothing() {
    use crate::overlay::menu_pages::sample_media;
    let mut person = Person::new();
    let media = Arc::new(crate::media::Watcher::new(|| Box::new(orbiters_n4n0::media::NoMedia)));
    let mut playing = sample_media();
    playing.video = Some("dQw4w9WgXcQ".into());
    media.stand_in(crate::media::Reading { media: Some(playing), volume: None, art: None });
    person.panels.stage = super::Stage::new([1, 1], false, &media);
    person.panels.media = media;
    for (avatar, prop, says) in [(None, None, "Waiting for VRChat"), (Some("avtr_x"), None, "no N4N0"), (Some("avtr_x"), Some(8), "no link card")] {
        let mut snap = (*person.snap).clone();
        snap.n4n0.avatar_id = avatar.map(String::from);
        snap.n4n0.prop_sync = prop;
        snap.n4n0.can_drop = false;
        person.snap = Arc::new(snap);
        person.around = Some(Around::QuickMenu);
        person.turns(3);
        if person.panels.facts.sheet != Some(Sheet::All) {
            person.press(Action::All);
            person.turns(3);
        }
        let drop = Action::PlayingLink(Target::DropPlaying);
        let at = person.find(drop).expect("Drop link is there, greyed");
        person.panels.hover(Some(at));
        person.turn();
        assert_eq!(person.panels.facts.pointed, Some(drop));
        let (_, why) = super::panels::caption(drop, &person.panels.facts).unwrap();
        assert!(why.contains(says), "{why}");
        person.press(drop);
        person.turns(2);
        assert_eq!(person.panels.stage.host.menu.copied, None, "{says}: nothing marked copied");
        assert!(person.find(Action::PlayingLink(Target::CopyPlaying)).is_some(), "Copy link stays");
    }
}

/// REQUEST20 6: the assistant asked for a page: it opens here once, as its own press opens it; one the panels were not up for
/// waits for them; a page of the window is the window's; the request standing when the panels started is an old one.
#[test]
fn the_assistant_opens_the_page_it_asked_for_once() {
    use orbiters_n4n0::{AssistantView, OpenAsk};
    use orbiters_n4n0_core::intents::Page;
    let ask = |person: &mut Person, id: u32, page: Page| {
        let snap = Arc::make_mut(&mut person.snap);
        snap.n4n0.assistant = AssistantView { open: Some(OpenAsk { id, page, world: None }), ..Default::default() }.into();
    };
    let mut person = Person::new();
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    ask(&mut person, 1, Page::Translation);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Translate), "by Translation's own press");
    // Once: closed by the person, it stays closed.
    person.panels.close_sheet();
    person.turns(3);
    assert_eq!(person.panels.facts.sheet, None);
    ask(&mut person, 2, Page::MenuSettings);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Settings));
    // The window's page: not these panels'.
    ask(&mut person, 3, Page::AppSettings);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Settings));
    // Asked while VRChat's menu is closed: it opens when the menu comes.
    person.panels.close_sheet();
    person.around = None;
    person.turns(2);
    ask(&mut person, 4, Page::Instances);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, None);
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Instances));
    drop(person);
    // Panels started with a request already standing: it was for whoever was there then.
    let mut late = Person::new();
    ask(&mut late, 9, Page::Translation);
    late.around = Some(Around::QuickMenu);
    late.turns(3);
    assert_eq!(late.panels.facts.sheet, None);
}

#[test]
fn ask_streamed_snapshot_marks_the_page_for_repaint() {
    use orbiters_n4n0::status::{N4n0Line, N4n0Who};
    let mut person = Person::new();
    person.around = Some(Around::QuickMenu);
    person.turns(40);
    person.panels.facts.sheet = Some(Sheet::Talk);
    for words in ["It is", "It is sunny", "It is sunny and warm."] {
        let snap = Arc::make_mut(&mut person.snap);
        snap.n4n0.lines = vec![N4n0Line { who: N4n0Who::N4n0, text: words.into(), at_ms: 0, streaming: true }].into();
        for panel in &mut person.panels.list { panel.dirty = false; }
        person.panels.follow(&person.snap, 0);
        assert!(person.panels.list.iter().any(|p| p.dirty), "each intermediate snapshot repaints");
        assert_eq!(person.panels.facts.talk_lines.last().unwrap().1, words);
    }
}

/// REQUEST18 3: "Share 3D capture" under the wing is there while the service offers a capture of this world (none of its
/// captures public), and never while a recording runs. Its first press only asks (publishing is the person's decision, as
/// "Share publicly" beside the camera asks the first time); Cancel puts the question away with nothing sent; Yes asks the
/// explorer to share that capture, the question answered. Asked before, one press shares. No capture offered: no tile.
#[test]
fn share_3d_capture_asks_first_then_shares_through_the_explorer() {
    use super::panels::share_here::TileState;
    use crate::controller::explorer::{offer_id, Command, ShareHere};
    let (mut person, heard) = Person::heard();
    let key = "C:/Sessions/2026-11-20_10-00-00".to_string();
    let id = offer_id(&key);
    let set = |person: &mut Person, heard: &Heard, change: &dyn Fn(&mut Snapshot)| {
        change(Arc::make_mut(&mut person.snap));
        *heard.shown.lock().unwrap() = Some(person.snap.clone());
        person.turns(2);
    };
    set(&mut person, &heard, &|snap| {
        snap.explorer.share_here = Some(ShareHere { key: key.clone(), world_id: "wrld_sample".into(), world_name: "Sample world".into(), last: None });
        snap.settings.explorer.public_confirmed = false;
    });
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    let tile = |person: &Person| person.panels.list.iter().any(|p| p.zone == Zone::QmShare);
    let state = |person: &Person| person.panels.facts.share_here.as_ref().map(|t| t.state.clone());
    let sent = |heard: &Heard| heard.explorer.lock().unwrap().clone();
    assert!(tile(&person), "offered: the tile is under the wing");
    person.press(Action::ShareHere(id));
    assert_eq!(state(&person), Some(TileState::Asking));
    assert!(sent(&heard).is_empty(), "the first press asks: nothing is shared");
    person.press(Action::ShareHereNo);
    assert_eq!(state(&person), Some(TileState::Ready));
    assert!(sent(&heard).is_empty(), "put away: nothing is shared");
    person.press(Action::ShareHere(id));
    person.press(Action::ShareHereYes(id));
    assert_eq!(sent(&heard), [Command::ShareCapture { key: key.clone(), confirmed: true }], "yes: shared, the question answered");
    // Asked once already: one press shares.
    set(&mut person, &heard, &|snap| snap.settings.explorer.public_confirmed = true);
    person.press(Action::ShareHere(id));
    assert_eq!(sent(&heard).last(), Some(&Command::ShareCapture { key: key.clone(), confirmed: false }));
    // While a recording runs: not there (Stop is in its place).
    set(&mut person, &heard, &|snap| snap.capture.state = crate::state::CaptureState::Recording);
    assert!(!tile(&person), "not while recording");
    set(&mut person, &heard, &|snap| snap.capture.state = crate::state::CaptureState::Idle);
    assert!(tile(&person));
    // Nothing offered (no capture here, or one public already): gone.
    set(&mut person, &heard, &|snap| snap.explorer.share_here = None);
    assert!(!tile(&person), "no capture to offer: no tile");
}

/// A backend that keeps what the menus asked of it (N4N0's commands, sign-ins), and is otherwise the screenshots' still one.
#[derive(Default)]
struct Heard {
    n4n0: std::sync::Mutex<Vec<crate::ipc::N4n0Command>>,
    sign_ins: std::sync::atomic::AtomicUsize,
    boosters: std::sync::Mutex<Vec<String>>,
    /// What the camera's tools asked of the service: `record`, `qr on`, `qr off`.
    camera: std::sync::Mutex<Vec<String>>,
    /// The settings' changes asked for, in order.
    patches: std::sync::Mutex<Vec<serde_json::Value>>,
    /// What the explorer was asked (shares), in order; the service's snapshot as it says it, when a test sets one.
    explorer: std::sync::Mutex<Vec<crate::controller::explorer::Command>>,
    shown: std::sync::Mutex<Option<Arc<Snapshot>>>,
}

impl Heard {
    fn said(&self) -> Vec<String> {
        self.n4n0.lock().unwrap().iter().map(|c| format!("{c:?}")).collect()
    }
}

impl crate::backend::Backend for Heard {
    fn n4n0(&self, command: crate::ipc::N4n0Command) {
        self.n4n0.lock().unwrap().push(command);
    }
    fn sign_in(&self) {
        self.sign_ins.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    fn booster(&self, command: crate::booster::Command) {
        self.boosters.lock().unwrap().push(format!("{command:?}"));
    }
    fn explorer(&self, command: crate::controller::explorer::Command) {
        self.explorer.lock().unwrap().push(command);
    }
    fn view(&self) -> Arc<Snapshot> {
        self.shown.lock().unwrap().clone().unwrap_or_else(|| Arc::new(crate::state::Snapshot::new(Settings::default())))
    }
    fn take_notices(&self) -> Vec<crate::controller::Notice> {
        Vec::new()
    }
    fn library_version(&self) -> u64 {
        0
    }
    fn attach(&self, _ctx: &egui::Context) {}
    fn start_camera_recording(&self) {
        self.camera.lock().unwrap().push("record".into());
    }
    fn camera_qr(&self, active: bool) {
        self.camera.lock().unwrap().push(if active { "qr on" } else { "qr off" }.into());
    }
    fn start_capture(&self) {}
    fn stop_capture(&self) {}
    fn pause_capture(&self, _paused: bool) {}
    fn start_test_capture(&self) {}
    fn import_raw_capture(&self, _dir: std::path::PathBuf) {}
    fn patch_settings(&self, patch: serde_json::Value) {
        self.patches.lock().unwrap().push(patch);
    }
    fn trash_session(&self, _dir: std::path::PathBuf) {}
    fn clean_up(&self, _trim: Vec<std::path::PathBuf>, _delete: Vec<std::path::PathBuf>) {}
    fn reconstruct(&self, _dir: std::path::PathBuf) {}
    fn stop_reconstruction(&self) {}
    fn unqueue_reconstruction(&self, _dir: std::path::PathBuf) {}
    fn send_to_mixer(&self, _dir: std::path::PathBuf, _title: String) {}
    fn reopen_sign_in(&self) {}
    fn cancel_sign_in(&self) {}
    fn sign_out(&self) {}
    fn request_worker_log(&self) {}
    fn worker_log(&self) -> Option<Vec<String>> {
        None
    }
    fn upload_pictures(&self, _world_id: String, _world_name: String, _paths: Vec<std::path::PathBuf>) {}
    fn upload_preview(&self, _world_id: String, _world_name: String, _session: std::path::PathBuf) {}
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

/// Enzo (VR feedback F23, F24): Ask "does not update live while I talk" and "should say I must sign in before I can speak"; listening "automatic by default on
/// entering the page, until I press the talk button myself". Through the hand and the host: entering asks for the page's hands-free listening and leaving lets it
/// go; signed out, the button says so before a word and a press signs in, never listens; hands-free waits for the person, shows what it hears as it hears it,
/// and the person's own press takes over by the button; a stopped microphone is tried again with a press.
#[test]
fn ask_listens_by_itself_says_what_it_hears_and_asks_for_a_sign_in_first() {
    use orbiters_n4n0::status::{ProvisionalSpeech, VoiceReadiness};
    use super::panels::TalkNow;
    let (mut person, heard) = Person::heard();
    let set = |person: &mut Person, change: &dyn Fn(&mut crate::state::Snapshot)| {
        let mut snap = (*person.snap).clone();
        snap.settings.n4n0.enabled = true;
        change(&mut snap);
        person.snap = Arc::new(snap);
        person.turns(2);
    };
    set(&mut person, &|s| s.n4n0.voice.readiness = VoiceReadiness::SignIn);
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    person.press(Action::Talk);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Talk));
    assert!(heard.said().iter().any(|c| c.contains("AskPage") && c.contains("true")), "entering asks for the page's listening: {:?}", heard.said());
    // Signed out: said before a word, and the press is the way to sign in.
    assert_eq!(person.panels.facts.talk, TalkNow::SignIn);
    let before = heard.said().len();
    person.press(Action::TalkToggle);
    assert_eq!(heard.sign_ins.load(std::sync::atomic::Ordering::Relaxed), 1, "the browser opens to sign in");
    assert!(!heard.said()[before..].iter().any(|c| c.contains("AskTalk")), "nothing listened to");
    // Signed in, hands-free: waiting for the person, then what it hears as it hears it.
    set(&mut person, &|s| {
        s.n4n0.voice.readiness = VoiceReadiness::Ready;
        s.n4n0.voice.ask_open = true;
        s.n4n0.voice.automatic = true;
        s.n4n0.listening = true;
    });
    assert_eq!(person.panels.facts.talk, TalkNow::Waiting);
    assert!(person.panels.facts.talk_auto, "the mode shows");
    for words in ["what's", "what's the weather", "what's the weather in Paris"] {
        set(&mut person, &|s| {
            s.n4n0.voice.speech = true;
            s.n4n0.voice.level = 0.4;
            s.n4n0.voice.provisional = Some(ProvisionalSpeech { listen: 3, text: words.into() });
        });
        assert!(matches!(person.panels.facts.talk, TalkNow::Listening(_)));
        assert_eq!(person.panels.facts.talk_heard.as_deref(), Some(words), "the words so far, replaced");
        assert!(person.panels.facts.talk_lines.is_empty(), "and not in the conversation until said");
    }
    // The person's own press: by the button.
    let before = heard.said().len();
    person.press(Action::TalkToggle);
    assert!(heard.said()[before..].iter().any(|c| c.contains("AskTalk") && c.contains("true")), "{:?}", &heard.said()[before..]);
    // A stopped microphone: a press tries again.
    set(&mut person, &|s| s.n4n0.voice.readiness = VoiceReadiness::MicrophoneProblem);
    assert_eq!(person.panels.facts.talk, TalkNow::Failed);
    let before = heard.said().len();
    person.press(Action::TalkToggle);
    assert!(heard.said()[before..].iter().any(|c| c.contains("AskRetry")), "{:?}", &heard.said()[before..]);
    // Leaving lets the microphone go.
    let before = heard.said().len();
    person.press(Action::All);
    person.turns(2);
    assert!(heard.said()[before..].iter().any(|c| c.contains("AskPage") && c.contains("false")), "{:?}", &heard.said()[before..]);
}

/// Enzo (VR feedback F4): a test the booster runs can be stopped from its page, through the hand and the host: the engine is asked to stop it.
#[test]
fn the_booster_s_test_stops_from_its_page() {
    use orbiters_booster::engine::Testing;
    use orbiters_booster::trial::{Kind, Side};
    let (mut person, heard) = Person::heard();
    let mut snap = (*person.snap).clone();
    snap.booster.overlay.report.testing = Some(Testing { kind: Kind::Programs, seconds_left: 20.0, side: Side::With, manual: true });
    person.snap = Arc::new(snap);
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    person.press(Action::All);
    person.turns(2);
    person.press(Action::Booster);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Booster));
    person.press(Action::Set(super::panels::Pick::Booster(super::panels::BoosterPick::StopTest)));
    let asked = heard.boosters.lock().unwrap().clone();
    assert!(asked.iter().any(|c| c.contains("StopTest")), "{asked:?}");
}

#[test]
fn review_slider_releases_round_trip_through_service_with_delayed_state() {
    use super::panels::Smoothed;
    use crate::backend::Backend;
    use crate::service::tests::{service, linked_through, until};
    let service = service();
    let (client, wire) = linked_through(&service, crate::ipc::Role::Overlay);
    let backend = Arc::new(crate::backend::Remote(client.clone()));
    let mut panels = Panels::new(backend.clone(), 2.0, true);
    let mut clock = 0.0;
    let mut turn = |panels: &mut Panels| {
        clock += 0.011;
        panels.clock = Some(clock);
        let mut snap = (*backend.view()).clone();
        // Driver status deliberately lags configured settings throughout the real UI/IPC path.
        snap.n4n0.tracking = Some(orbiters_n4n0::status::TrackingView { on: false, ms: [0; 3], note: String::new() });
        turn_of(panels, &snap, Some(Around::QuickMenu), 0);
        snap
    };
    let snap = turn(&mut panels);
    panels.open_sheet(Sheet::Tracking, "", &snap);
    turn(&mut panels);
    let unchanged_tracker = backend.view().settings.tracking.ms[2];
    wire.hold(true);
    for (which, value) in [(Smoothed::Device(0), 90), (Smoothed::Device(1), 70), (Smoothed::QuickMenu, 60), (Smoothed::Wheel, 75)] {
        let snap = turn(&mut panels);
        let mut at = find_on(&panels, Action::Set(super::panels::Pick::SmoothingBar(which))).unwrap();
        at.1.x = (0..=1024).find(|x| super::panels::smoothing_at(*x as f32) == value).unwrap() as f32;
        panels.hover(Some(at));
        turn(&mut panels);
        panels.press(&snap);
        assert_eq!(panels.smoothing_held, Some((which, value)));
        turn(&mut panels);
        panels.release();
        panels.release();
        turn(&mut panels);
    }
    assert_eq!(backend.view().settings.tracking.ms, [90,70,unchanged_tracker]);
    assert_eq!(backend.view().settings.orbit.vr_smoothing_quick_menu_ms, 60.0);
    assert_eq!(backend.view().settings.orbit.vr_smoothing_wheel_ms, 75.0);
    wire.hold(false);
    assert!(until(|| client.snapshot().smoothing_revision == 4));
    assert_eq!(client.snapshot().settings.tracking.ms, [90,70,unchanged_tracker]);
    client.patch_smoothing(serde_json::json!({"tracking":{"ms":[65535,70,0]}}));
    assert!(until(|| client.snapshot().smoothing_revision == 5));
    assert_eq!(client.snapshot().settings.tracking.ms[0], orbiters_core::settings::TrackingSettings::MAX_MS);
    let before = client.snapshot().settings.tracking.ms;
    client.patch_smoothing(serde_json::json!({"tracking":{"ms":"invalid"}}));
    assert!(until(|| client.snapshot().smoothing_revision == 6));
    assert_eq!(client.snapshot().settings.tracking.ms, before);
}

/// OWNER DECISION (Enzo, repeatedly; latest 2026-11-13: "I asked multiple time to make the side tiles on the quick menu stay even when a page is open"): a page opened
/// from the Quick Menu leaves its side tiles where they are, every one, to see and to point at, whichever page, and pressing one goes on from there. 2026-11-11's F8
/// had them go; that is overruled. Do not remove this test or weaken it: see AGENTS.md, "Owner decisions".
#[test]
fn the_side_tiles_stay_while_a_page_is_up() {
    let mut person = Person::new();
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    let wing = |person: &Person| person.panels.all().filter(|p| (Zone::WING.contains(&p.zone) || p.zone == Zone::QmCaption) && p.shown).count();
    let at_first = wing(&person);
    assert!(at_first >= 7, "the wing at first: {at_first}");
    for tile in [Action::Worlds, Action::Page(Target::YouTube), Action::Page(Target::Instances), Action::Pictures, Action::Talk, Action::All] {
        person.press(tile);
        person.turns(2);
        assert!(person.panels.sheet_open(), "{tile:?} opens its sheet");
        assert_eq!(wing(&person), at_first, "{tile:?}: every tile of the wing beside it");
        assert!(person.panels.facts.beside_wing, "{tile:?}");
        // Each still answers a laser: another tile goes to its page.
        let other = if tile == Action::Pictures { Action::Worlds } else { Action::Pictures };
        let on_wing = person.panels.all().enumerate().find_map(|(i, p)| (Zone::WING.contains(&p.zone)).then(|| p.items.iter().find(|it| it.action == other).map(|it| (i, it.rect.center()))).flatten());
        let (part, at) = on_wing.unwrap_or_else(|| panic!("{tile:?}: {other:?} on the wing"));
        person.panels.hover(Some((part, at)));
        person.turn();
        person.panels.press(&person.snap.clone());
        person.turn();
        person.panels.release();
        person.turns(2);
        assert!(person.panels.sheet_open() && panels_tool_open(&person, other), "{tile:?}: the wing's {other:?} went there");
        assert_eq!(wing(&person), at_first, "{tile:?} then {other:?}: the wing still there");
        person.press(Action::CloseSheet);
        person.turns(2);
        assert!(!person.panels.sheet_open());
        assert_eq!(wing(&person), at_first, "{tile:?}: the wing as it was");
    }
    // A page reached from All tools (Settings) as well.
    person.press(Action::All);
    person.turns(2);
    person.press(Action::Settings);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Settings));
    assert_eq!(wing(&person), at_first, "Settings: the wing beside it");
}

/// Whether `action`'s tool has its page up.
fn panels_tool_open(person: &Person, action: Action) -> bool {
    super::panels::tool_is_open(&person.panels.facts, action)
}

/// The same in a headset's own layouts: the wing on either hand's side and the strip under the menu, as `vr.rs` lays them, stay with a page up (owner decision,
/// 2026-11-13; see [`the_side_tiles_stay_while_a_page_is_up`]). From the wheel the page alone shows, as before: the ring's cells are not the Quick Menu's side tiles.
#[test]
fn the_side_tiles_stay_in_every_headset_layout() {
    use super::vr;
    for (layout, places) in [("wing, right hand", vr::quick_menu_places(0)), ("wing, left hand", vr::quick_menu_places(1)), ("strip", vr::strip_places())] {
        let mut person = Person::new();
        let snap = person.snap.clone();
        let turn = |panels: &mut Panels| {
            let mut places = places.clone();
            panels.way_back(Some(Around::QuickMenu), &mut places);
            panels.takeover_place = Some(vr::sheet_place(Zone::Takeover));
            panels.back_place = Some(vr::sheet_place(Zone::SheetBack));
            panels.set_open(Some(Around::QuickMenu), places, &snap);
            panels.follow(&snap, 0);
            panels.menu_turn(&snap);
            panels.step_springs(0.011);
            panels.paint(&snap, |_, _| {});
        };
        for _ in 0..3 { turn(&mut person.panels); }
        let tiles = |panels: &Panels| panels.all().filter(|p| Zone::WING.contains(&p.zone) || matches!(p.zone, Zone::QmStrip | Zone::QmCaption)).map(|p| p.zone).collect::<Vec<_>>();
        let before = tiles(&person.panels);
        assert!(!before.is_empty(), "{layout}");
        person.panels.open_sheet(Sheet::Pictures, "Pictures", &snap);
        for _ in 0..3 { turn(&mut person.panels); }
        assert!(person.panels.sheet_open(), "{layout}");
        assert_eq!(tiles(&person.panels), before, "{layout}: the side tiles stay with a page up");
        // Laid where vr.rs lays them with a page up, none lies over the page.
        let page = [Zone::SheetBack, Zone::Takeover].map(|z| vr::clear_of_sheet(vr::sheet_place(z), true).rect);
        for part in person.panels.all().filter(|p| before.contains(&p.zone)) {
            let at = vr::clear_of_sheet(part.placed, true).rect;
            assert!(page.iter().all(|r| !r.intersects(at)), "{layout}: {:?} at {at:?} over the page {page:?}", part.zone);
        }
    }
}

/// Enzo, 2026-11-11 (F22): the tile or tab of the tool whose page is open, pressed again, closes the page and VRChat's menu is there under it. Pressed on another tool it
/// goes there; pressed within the page it does what it does there.
#[test]
fn the_open_tool_pressed_again_closes_its_page() {
    let mut person = Person::new();
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    // From the wing, then its own tab in the head.
    person.press(Action::Pictures);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Pictures));
    let head = person.panels.all().position(|p| p.zone == Zone::SheetBack).expect("the head");
    let tab = person.panels.all().nth(head).and_then(|p| p.items.iter().find(|i| i.action == Action::Pictures)).map(|i| i.rect.center()).expect("its tab in the head");
    // Its tab in the head, as the wing's tile beside it would.
    person.panels.hover(Some((head, tab)));
    person.turn();
    let pressed = person.panels.press(&person.snap.clone());
    person.turn();
    person.panels.release();
    person.turn();
    person.turns(2);
    assert!(pressed.clicked, "it clicks");
    assert_eq!(person.panels.facts.sheet, None, "Pictures pressed again: closed");
    assert!(person.panels.all().any(|p| p.zone == Zone::WingPictures), "the menu as it was");
    // Another tool's tab goes there, and that one again closes it.
    person.press(Action::Page(Target::Instances));
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Instances));
    person.press(Action::Worlds);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Worlds), "another tab: another page");
    person.press(Action::Worlds);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, None, "Worlds pressed again: closed");
    // YouTube is the menu's page of that name, or its results.
    person.press(Action::Page(Target::YouTube));
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Menu));
    person.press(Action::Page(Target::YouTube));
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, None, "YouTube pressed again: closed");
    // All tools has no tabs: what is on its page opens what it says.
    person.press(Action::All);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::All));
    assert!(!person.panels.all().filter(|p| p.zone == Zone::SheetBack).any(|p| p.items.iter().any(|i| i.action == Action::All)), "All tools' head has no tabs");
    person.press(Action::Pictures);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Pictures), "a card of All tools opens its tool");
    // And the next opening of the menu is a menu with nothing open.
    person.press(Action::Pictures);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, None);
    person.around = None;
    person.turns(2);
    person.around = Some(Around::QuickMenu);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, None);
}

/// The way to VRChat's own menus has one place: a button on the sheet's head, beside its close, inside the frame (IMG1 had its words float off the frame's right). Pressed,
/// everything goes but a pill in the same place, its eye where the button was, which brings it all back.
#[test]
fn the_way_to_vrchat_s_menus_keeps_its_place_on_the_head() {
    let mut person = Person::new();
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    person.press(Action::Pictures);
    person.turns(2);
    let back = person.panels.all().find(|p| p.zone == Zone::SheetBack).map(|p| p.placed).expect("the back");
    let button = person.panels.all().find(|p| p.zone == Zone::SheetBack).and_then(|p| p.items.iter().find(|i| i.action == Action::Peek)).map(|i| i.rect).expect("on the head");
    assert!(button.top() >= 0.0 && button.bottom() <= super::panels::SHEET_HEAD, "in the head: {button:?}");
    let pill = person.panels.all().find(|p| p.zone == Zone::SheetPeek).map(|p| (p.placed, p.shown)).expect("the pill");
    assert!(!pill.1, "the pill shows only while VRChat's menus are reached");
    assert!(back.rect.contains_rect(pill.0.rect), "inside the frame: {:?} in {:?}", pill.0.rect, back.rect);
    let button_right = back.rect.left() + button.right() * back.unit;
    assert!((pill.0.rect.right() - button_right).abs() < 0.01, "it ends where the button does");
    person.press(Action::Peek);
    person.turns(2);
    let shown: Vec<Zone> = person.panels.all().filter(|p| p.shown).map(|p| p.zone).collect();
    assert_eq!(shown, vec![Zone::SheetPeek], "only the way back");
    person.press(Action::Peek);
    person.turns(2);
    assert!(person.panels.all().all(|p| p.shown != (p.zone == Zone::SheetPeek)), "all back, the pill away");
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Pictures));
}

/// Enzo's IMG6 (F25): one silhouette whatever the page, in either look. The page as submitted lies inside its sheet's shape: nothing of it outside the back's corners,
/// as the headset is given them in one picture ([`super::vr::compose_sheet`]).
#[test]
fn every_page_has_its_sheet_s_corners() {
    use orbiters_core::settings::MenuLook;
    for (look, worker) in [(MenuLook::Modern, false), (MenuLook::Retro, false), (MenuLook::Modern, true), (MenuLook::Retro, true)] {
        for opens in [Action::All, Action::Worlds, Action::Pictures, Action::Page(Target::YouTube), Action::Page(Target::Instances)] {
            let mut person = Person::new();
            person.panels.stage.async_pages = worker;
            let mut snap = (*person.snap).clone();
            snap.settings.orbit.theme = look;
            person.snap = Arc::new(snap);
            person.around = Some(Around::QuickMenu);
            person.turns(3);
            person.press(opens);
            person.turns_until("its parts painted", |p| p.panels.all().all(|part| part.painted || !part.shown));
            person.turns(3);
            let back = person.panels.all().find(|p| p.zone == Zone::SheetBack).expect("a back");
            let page = person.panels.takeover.as_ref().expect("a page");
            let top = super::vr::page_row(back, page).expect("one picture");
            let whole = super::vr::compose_sheet(back.pixels(), back.px, page.pixels(), page.px, top);
            let [w, h] = back.px;
            let alpha = |pixels: &[u8], x: u32, y: u32| pixels[((y * w + x) * 4 + 3) as usize];
            // At the foot's corners, where the page reaches: nothing of it where its back is not. (Off a pixel and a half of the straight sides, whose outer pixel the
            // back's edge feathers; along the corners' arcs two edges smoothed two ways differ by a part of a pixel.)
            let corner = 40;
            let mut worst = (0i32, 0, 0);
            for y in top..top + page.px[1] {
                for x in (2..corner).chain(w - corner..w - 2) {
                    let past = alpha(&whole, x, y) as i32 - alpha(back.pixels(), x, y) as i32;
                    if past > worst.0 {
                        worst = (past, x, y);
                    }
                }
            }
            assert!(worst.0 <= 64, "{look:?} {opens:?}: the page reaches {} past its sheet's corners at ({}, {})", worst.0, worst.1, worst.2);
            for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
                assert_eq!(alpha(&whole, x, y), 0, "{look:?} {opens:?}: its corner ({x}, {y}) is round");
            }
            assert!(alpha(&whole, w / 2, h / 2) > 250, "{look:?} {opens:?}: solid in its middle");
        }
    }
}


#[test]
fn worker_switches_publish_matching_page_header_and_hit_geometry() {
    let mut person = Person::new();
    person.panels.stage.async_pages = true;
    person.around = Some(Around::QuickMenu);
    person.turns(4);
    person.panels.open_sheet(Sheet::Settings, "Settings", &person.snap);
    person.turns_until("settings worker frame", |p| p.panels.takeover.as_ref().is_some_and(|p| p.painted) && !p.panels.stage.painter.busy());
    let settings = person.panels.takeover.as_ref().unwrap().pixels().to_vec();
    let epoch = person.panels.stage.epoch;
    person.panels.open_sheet(Sheet::Tracking, "Tracking", &person.snap);
    person.turn();
    if !person.panels.takeover.as_ref().unwrap().painted {
        assert!(person.panels.takeover.as_ref().unwrap().items.is_empty(), "a cold blank page has no targets");
    }
    person.turns_until("tracking worker frame", |p| p.panels.takeover.as_ref().is_some_and(|p| p.painted) && !p.panels.stage.painter.busy());
    person.panels.open_sheet(Sheet::Settings, "Settings", &person.snap);
    person.turn();
    assert!(person.panels.stage.epoch > epoch);
    let page = person.panels.takeover.as_ref().unwrap();
    assert_eq!(page.pixels(), settings, "A-B-A restores A on the next turn");
    let (_, shown, _) = person.panels.stage.presented.as_ref().unwrap();
    assert_eq!(shown.sheet, Some(Sheet::Settings));
    assert_eq!(page.items, super::panels::sheet_items(Sheet::Settings, shown));
    let back = person.panels.list.iter().find(|p| p.zone == Zone::SheetBack).unwrap();
    assert_eq!(back.items, super::panels::items(back.zone, back.logical, shown));
    assert!(super::vr::page_row(back, page).is_some());
    // Invalidation also invalidates the page-cache identity, so old language/look results cannot return.
    let before = person.panels.stage.epoch;
    person.panels.paint_all_again();
    assert!(person.panels.stage.epoch > before);
    assert!(person.panels.stage.cache.is_empty());
}


#[test]
fn special_pages_consume_the_outer_paint_request() {
    for sheet in [Sheet::Menu, Sheet::Worlds] {
        let mut person = Person::new();
        person.around = Some(Around::QuickMenu);
        person.turns(4);
        if sheet == Sheet::Worlds {
            person.panels.act(Action::Worlds, &person.snap, &mut super::Pressed::default());
        } else {
            person.panels.stage.host.menu.page = orbiters_n4n0_core::menu::Page::States;
            person.panels.open_sheet(sheet, "", &person.snap);
        }
        let panel = person.panels.takeover.as_mut().unwrap();
        panel.dirty = true;
        panel.painted = false;
        person.panels.paint(&person.snap, |_, _| {});
        assert!(!person.panels.takeover.as_ref().unwrap().dirty, "{sheet:?} left a consumed paint request pending");
    }
}


#[test]
fn review_every_page_paints_off_the_overlay_thread() {
    for sheet in [Sheet::Menu, Sheet::Worlds] {
        let mut person = Person::new();
        person.panels.stage.async_pages = true;
        person.around = Some(Around::QuickMenu);
        person.turns(4);
        if sheet == Sheet::Worlds { person.panels.act(Action::Worlds, &person.snap, &mut super::Pressed::default()); }
        else { person.panels.stage.host.menu.page = orbiters_n4n0_core::menu::Page::States; person.panels.open_sheet(sheet, "", &person.snap); }
        let before = person.panels.stage.canvas.direct_paints;
        person.panels.takeover.as_mut().unwrap().painted = false;
        person.panels.paint(&person.snap, |_, _| {});
        assert_eq!(person.panels.stage.canvas.direct_paints, before, "{sheet:?} painted on the overlay thread");
        person.turns_until("the worker paints its page", |p| p.panels.takeover.as_ref().is_some_and(|p| p.painted));
    }
}

#[test]
fn review_menu_navigation_acts_on_the_visible_row_after_unrelated_arrivals() {
    for change in [0, 1, 2] {
        let mut person = Person::new();
        person.panels.stage.async_pages = true;
        person.around = Some(Around::QuickMenu);
        person.turns(4);
        person.panels.stage.host.menu.page = orbiters_n4n0_core::menu::Page::Photo;
        person.panels.stage.host.menu.open = true;
        person.panels.open_sheet(Sheet::Menu, "", &person.snap);
        person.turns_until("the displayed menu", |p| p.panels.stage.presented_scene.as_ref().and_then(|s| s.menu()).is_some()
            && p.panels.takeover.as_ref().is_some_and(|p| p.painted) && !p.panels.stage.painter.busy());
        let shown = person.panels.stage.presented_scene.as_ref().unwrap().menu().unwrap();
        let page = orbiters_n4n0_core::menu::layout::cached(orbiters_n4n0_core::menu::layout::ScreenPage::of(shown));
        let element = page.elements.iter().position(|e| e.target == Target::History).expect("the displayed History row");
        assert!(orbiters_n4n0_core::menu::layout::state(shown, &page.elements[element]).pointable());
        // Freeze the published picture, then let unrelated backend data arrive before the press.
        person.panels.stage.host.menu.heard = Some("arrived after painting".into());
        if change == 1 { person.panels.stage.host.menu.page = orbiters_n4n0_core::menu::Page::Results; }
        if change == 2 { person.panels.stage.host.menu.pages = orbiters_n4n0_core::menu::Pages::Four; }
        person.panels.takeover.as_mut().unwrap().hover = Some(element);
        person.panels.press(&person.snap);
        assert_eq!(person.panels.stage.host.menu.page,
            match change { 0 => orbiters_n4n0_core::menu::Page::History, 1 => orbiters_n4n0_core::menu::Page::Results, _ => orbiters_n4n0_core::menu::Page::Photo },
            "the same displayed navigation survives unrelated arrivals; a different page rejects it");
    }
}

#[test]
fn review_menu_navigation_keeps_context_and_indexed_actions_guarded() {
    use orbiters_n4n0_core::menu::{Menu, Page, Pages, Typing};
    use orbiters_n4n0_core::menu::layout::{self, ScreenPage};
    let accepts = |shown: &Menu, live: &Menu, target| {
        let page = layout::cached(ScreenPage::of(shown));
        let index = page.elements.iter().position(|e| e.target == target).expect("the shown control");
        super::menu_press_compatible(shown, live, index)
    };
    let mut shown = Menu::default();
    shown.open = true;
    shown.pages = Pages::ALL;
    shown.page = Page::Type;
    shown.typing = Typing::StateName;
    let mut live = shown.clone();
    live.heard = Some("an unrelated arrival".into());
    assert!(accepts(&shown, &live, Target::Back));
    live.typing = Typing::Suggestion;
    assert!(!accepts(&shown, &live, Target::Back), "Back must keep its destination");
    shown.page = Page::Results;
    let mut live = shown.clone();
    live.heard = Some("an unrelated arrival".into());
    assert!(!accepts(&shown, &live, Target::Copy(0)), "indexed actions keep strict state validation");
}

#[test]
fn review_a_stale_invite_press_never_invites_a_new_row() {
    for cached in [false, true] {
        let (mut person, heard) = Person::heard();
        person.panels.stage.async_pages = true;
        person.around = Some(Around::QuickMenu);
        person.turns(4);
        person.panels.open_sheet(Sheet::Instances, "Instances", &person.snap);
        person.panels.stage.host.menu.page = orbiters_n4n0_core::menu::Page::Instances;
        person.turn();
        person.panels.takeover.as_mut().unwrap().dirty = true;
        let a = super::panels::WorldVisit { world_id: "wrld_a".into(), name: "Shown A".into(), location: "wrld_a:111".into(), ..Default::default() };
        person.panels.facts.worlds_visited = vec![a];
        let began = Instant::now();
        while !person.panels.takeover.as_ref().unwrap().painted || person.panels.stage.painter.busy() {
            assert!(began.elapsed() < Duration::from_secs(10));
            person.panels.paint(&person.snap, |_, _| {});
            std::thread::sleep(Duration::from_millis(2));
        }
        person.panels.facts.worlds_visited = vec![super::panels::WorldVisit { world_id: "wrld_b".into(), name: "New B".into(), location: "wrld_b:222".into(), ..Default::default() }];
        if cached {
            person.panels.open_sheet(Sheet::Settings, "", &person.snap);
            person.panels.open_sheet(Sheet::Instances, "Instances", &person.snap);
            person.panels.paint(&person.snap, |_, _| {});
        }
        let i = person.panels.list.len();
        let at = person.panels.takeover.as_ref().unwrap().items.iter().find(|x| x.action == Action::Invite(0, 0)).unwrap_or_else(|| panic!("cached={cached}, items={:?}, cache={:?}", person.panels.takeover.as_ref().unwrap().items, person.panels.stage.cache.iter().map(|f| (f.key(), f.items.len())).collect::<Vec<_>>())).rect.center();
        person.panels.hover(Some((i, at)));
        person.panels.press(&person.snap);
        person.panels.release();
        if let Some(action) = person.panels.pressed_late.take() { person.panels.act(action, &person.snap, &mut super::Pressed::default()); }
        assert_eq!(person.panels.invited.as_ref().map(|x| x.0.as_str()), Some("wrld_a:111"), "cached {cached}: the shown row must retain its source");
        assert!(heard.said().iter().any(|s| s.contains("wrld_a:111")), "the backend must receive A");
    }
}

#[test]
fn review_shell_pictures_are_rendered_off_the_overlay_thread() {
    let mut person = Person::with_a_song(); person.panels.stage.async_pages = true; person.around = Some(Around::QuickMenu);
    person.turns_until("shell pictures", |p| !p.panels.is_empty() && p.panels.all().all(|p| p.painted));
    assert!(person.panels.all().all(|p| p.canvas.direct_paints == 0));
}
#[test]
fn review_streamed_ask_words_and_answers_reach_the_published_picture() {
    use orbiters_n4n0::status::{N4n0Line, N4n0Who, ProvisionalSpeech, VoiceReadiness};
    let mut person = Person::new(); person.panels.stage.async_pages = true; person.around = Some(Around::QuickMenu); person.turns(4);
    person.panels.open_sheet(Sheet::Talk, "Ask", &person.snap);
    for words in ["what", "what is", "what is new"] {
        let s = Arc::make_mut(&mut person.snap); s.settings.n4n0.enabled = true; s.n4n0.voice.readiness = VoiceReadiness::Ready; s.n4n0.voice.speech = true; s.n4n0.voice.automatic = true; s.n4n0.voice.provisional = Some(ProvisionalSpeech { listen: 3, text: words.into() });
        person.turn();
        person.turns_until("live words on the picture", |p| p.panels.stage.presented.as_ref().is_some_and(|(_,f,_)| f.talk_heard.as_deref() == Some(words)));
    }
    for words in ["Here", "Here is", "Here is the answer."] {
        Arc::make_mut(&mut person.snap).n4n0.lines = vec![N4n0Line { who: N4n0Who::N4n0, text: words.into(), at_ms: 0, streaming: true }].into();
        person.turn();
        person.turns_until("streamed answer on the picture", |p| p.panels.stage.presented.as_ref().is_some_and(|(_,f,_)| f.talk_lines.last().is_some_and(|(_,s)| s == words)));
    }
}

#[test]
fn review_camera_web_frames_and_header_stop_repaint_on_the_worker() {
    use super::panels::WebState;
    let mut person = Person::new(); person.panels.stage.async_pages = true;
    person.around = Some(Around::Camera);
    Arc::make_mut(&mut person.snap).camera_open_said = Some(true);
    person.panels.web = Some(super::camera_web::CameraWeb::open("https://orbiters.cc"));
    for color in [egui::Color32::RED, egui::Color32::BLUE] {
        let web = person.panels.web.as_mut().unwrap(); web.frame = Some(egui::ColorImage::filled([32,32], color)); web.state = WebState::Shown; web.fresh = true;
        for panel in &mut person.panels.list { panel.dirty = true; }
        person.turn();
        person.turns_until("camera web frame", |p| p.panels.list.iter().find(|p| p.zone == Zone::CameraWeb).is_some_and(|p| p.painted && !p.dirty) && !p.panels.stage.painter.busy());
        let panel = person.panels.list.iter().find(|p| p.zone == Zone::CameraWeb).unwrap();
        assert_eq!(panel.canvas.direct_paints, 0);
        assert!(panel.displayed.as_ref().unwrap().camera.web.as_ref().unwrap().picture.is_some());
        assert!(panel.pixels().chunks_exact(4).any(|p| p == color.to_array()), "camera web texture did not reach the shown picture");
    }
    person.around = Some(Around::QuickMenu); person.turns(2);
    person.panels.open_sheet(Sheet::Settings, "", &person.snap);
    Arc::make_mut(&mut person.snap).capture.state = crate::state::CaptureState::Recording;
    person.turns_until("header Stop", |p| p.find(Action::StopRecording).is_some());
    let head = person.panels.list.iter().find(|p| p.zone == Zone::SheetBack).unwrap();
    assert_eq!(head.canvas.direct_paints, 0); assert!(head.displayed.as_ref().unwrap().recording);
    person.press(Action::StopRecording);
    person.turns_until("Stopping header", |p| p.panels.list.iter().find(|p| p.zone == Zone::SheetBack).is_some_and(|p| p.displayed.as_ref().is_some_and(|f| f.rec_stopping)));
    assert!(person.find(Action::StopRecording).is_none());
}

#[test]
fn review_a_held_or_cached_video_press_keeps_the_shown_source() {
    use orbiters_n4n0_core::youtube::Video;
    let video = |id: &str| Video { id: id.into(), title: id.into(), channel: String::new(), duration: None, live: false };
    for cached in [false, true] { for reorder in [false, true] {
        let (mut person, heard) = Person::heard(); person.panels.stage.async_pages = true; person.around = Some(Around::QuickMenu); person.turns(4);
        person.panels.open_sheet(Sheet::Results, "", &person.snap);
        person.panels.stage.host.menu.page = orbiters_n4n0_core::menu::Page::Results; person.turn();
        person.panels.stage.host.menu.open = true;
        person.panels.stage.host.menu.can_drop = true; person.panels.stage.host.menu.copied = Some("aaaaaaaaaaa".into());
        person.panels.stage.host.menu.results = vec![video("aaaaaaaaaaa")];
        person.panels.facts.videos = vec![super::panels::VideoTile { id: "aaaaaaaaaaa".into(), title: "Shown A".into(), channel: String::new(), duration: None, picture: None }];
        person.panels.facts.video_copied = Some(0); person.panels.takeover.as_mut().unwrap().dirty = true;
        let start = Instant::now();
        loop { person.panels.paint(&person.snap, |_,_| {}); if person.panels.stage.presented.as_ref().is_some_and(|(_,f,_)| f.videos.first().is_some_and(|v| v.id == "aaaaaaaaaaa")) && !person.panels.stage.painter.busy() { break; } assert!(start.elapsed() < Duration::from_secs(10)); std::thread::sleep(Duration::from_millis(2)); }
        if cached { person.panels.open_sheet(Sheet::Settings, "", &person.snap); person.panels.open_sheet(Sheet::Results, "", &person.snap); person.panels.paint(&person.snap, |_,_| {}); }
        let i = person.panels.list.len();
        let at = person.panels.takeover.as_ref().unwrap().items.iter().find(|i| i.action == Action::VideoDrop(0)).unwrap_or_else(|| panic!("cached={cached}, items={:?}, cache={:?}", person.panels.takeover.as_ref().unwrap().items, person.panels.stage.cache.iter().map(|f| (f.key(), f.items.len())).collect::<Vec<_>>())).rect.center();
        person.panels.hover(Some((i, at))); person.panels.press(&person.snap);
        assert!(person.panels.drag.is_some(), "the press waits for release");
        person.panels.stage.host.menu.results = if reorder { vec![video("bbbbbbbbbbb"), video("aaaaaaaaaaa")] } else { vec![video("bbbbbbbbbbb")] };
        if !reorder { person.panels.stage.host.menu.copied = Some("bbbbbbbbbbb".into()); }
        person.panels.release();
        let action = person.panels.pressed_late.take().unwrap(); person.panels.act(action, &person.snap, &mut super::Pressed::default());
        let said = heard.said();
        assert!(!said.iter().any(|s| s.contains("bbbbbbbbbbb")), "cached={cached}, reorder={reorder}: shown A dispatched B: {said:?}");
        assert_eq!(said.iter().filter(|s| s.contains("aaaaaaaaaaa")).count(), usize::from(reorder), "a surviving shown source must still work");
    } }
}

#[test]
fn review_the_instance_chooser_paints_and_returns_on_the_page_worker() {
    use crate::overlay::worlds::panel::{Page, Status, Target as WTarget};
    let (mut person, heard) = Person::heard(); person.panels.stage.async_pages = true;
    let snap = Arc::make_mut(&mut person.snap); snap.vrchat_login.allowed = true;
    snap.vrchat_login.user = Some(orbiters_core::vrchat_api::User { id: "usr_replay".into(), display_name: "Replay".into(), ..Default::default() });
    person.around = Some(Around::QuickMenu); person.turns(3); person.press(Action::Worlds);
    let found = orbiters_core::maps::FoundWorld { world: orbiters_core::maps::World { world_id: "wrld_replay".into(), name: "Replay".into(), ..Default::default() }, ..Default::default() };
    let entry = crate::overlay::worlds::catalog::Entry::world(&found, None, Default::default());
    person.panels.stage.worlds.as_mut().unwrap().pick_for_test(entry);
    let status = Status { headset: true, ..Default::default() }; let _ = person.panels.world_relay(status.clone()); person.turn();
    person.press_world(WTarget::Join, &status);
    person.turns_until("instance chooser picture", |p| matches!(&p.panels.stage.presented_scene, Some(super::page_painter::Scene::Worlds(s)) if s.panel.page == Page::Go));
    assert_eq!(person.panels.stage.canvas.direct_paints, 0);
    assert!(!heard.said().iter().any(|s| s.contains("InviteMe")), "opening a chooser must not invite");
    person.press(Action::Back);
    person.turns_until("detail picture after chooser", |p| matches!(&p.panels.stage.presented_scene, Some(super::page_painter::Scene::Worlds(s)) if s.panel.page == Page::Detail));
}

#[test]
fn review_new_shell_parts_build_masks_on_the_raster_owner() {
    let mut person = Person::new(); person.panels.stage.async_pages = true; person.around = Some(Around::Wheel);
    let places = Zone::around(Around::Wheel).iter().map(|z| zones::in_units(*z)).collect();
    person.panels.set_open(person.around, places, &person.snap);
    assert!(person.panels.list.iter().filter(|p| p.zone.ring().is_some()).all(|p| p.mask.is_none()));
    person.turns_until("ring masks with their pictures", |p| p.panels.list.iter().filter(|p| p.zone.ring().is_some()).all(|p| p.painted && p.mask.is_some()));
    assert!(person.panels.list.iter().all(|p| p.canvas.direct_paints == 0));
}
#[test]
fn review_invalidated_pictures_are_retired_one_pair_per_input_turn() {
    let mut person = Person::new(); person.panels.stage.async_pages = true; person.around = Some(Around::QuickMenu); person.turns(3);
    for sheet in [Sheet::Settings, Sheet::Tracking, Sheet::Mics] {
        person.panels.open_sheet(sheet, "", &person.snap);
        person.turns_until("cached picture", |p| p.panels.stage.cache.iter().any(|f| f.sheet == sheet) && !p.panels.stage.painter.busy());
    }
    assert_eq!(person.panels.stage.cache.len(), 3);
    person.panels.paint_all_again();
    assert!(person.panels.stage.cache.is_empty(), "old look/language pictures cannot be restored");
    assert_eq!(person.panels.stage.retired.len(), 3);
    person.panels.follow(&person.snap, 0);
    assert_eq!(person.panels.stage.retired.len(), 2, "free one pair, not the whole cache on the input thread");
}

#[test]
fn review_the_worker_publishes_the_complete_sheet_and_matching_geometry() {
    let mut person = Person::new(); person.panels.stage.async_pages = true; person.around = Some(Around::QuickMenu); person.turns(3);
    person.panels.open_sheet(Sheet::Settings, "", &person.snap);
    person.turns_until("complete worker sheet", |p| p.panels.stage.sheet_picture.is_some());
    let picture = person.panels.take_sheet_picture().unwrap();
    let back = person.panels.list.iter().find(|p| p.zone == Zone::SheetBack).unwrap();
    let page = person.panels.takeover.as_ref().unwrap();
    assert_eq!(picture.0.as_slice(), super::vr::compose_sheet(back.pixels(), back.px, page.pixels(), page.px, picture.2));
    let mut sheet = super::vr::SheetPicture::default(); let original = picture.0.clone(); let size = picture.1;
    sheet.adopt(picture);
    assert_eq!(sheet.pixels().as_ptr(), original.as_ptr(), "input adopts the worker allocation without pixel work");
    assert_eq!(sheet.changed, Some([0,0,size[0],size[1]]), "damage covers the entire composed picture");
    let rendered = person.panels.stage.cache.iter().find(|f| f.sheet == Sheet::Settings).unwrap();
    assert_eq!(page.pixels().as_ptr(), rendered.pixels.as_ptr(), "input adopts the raw page allocation without a pixel copy");
    assert_eq!(back.pixels().as_ptr(), rendered.header.as_ref().unwrap().pixels.as_ptr(), "input adopts the matching raw header allocation");
    assert_eq!(page.items, super::panels::sheet_items(Sheet::Settings, &page.displayed.as_ref().unwrap()));
}
#[test]
fn review_a_camera_bar_for_a_previous_url_cannot_act_on_the_new_page() {
    let mut person = Person::new(); person.panels.stage.async_pages = true; person.around = Some(Around::Camera);
    Arc::make_mut(&mut person.snap).camera_open_said = Some(true);
    person.panels.web = Some(super::camera_web::CameraWeb::open("https://orbiters.cc/a"));
    person.turns_until("camera bar A", |p| p.find(Action::WebClose).is_some());
    person.panels.web.as_mut().unwrap().url = "https://orbiters.cc/b".into();
    let at = person.find(Action::WebClose).unwrap(); person.panels.hover(Some(at)); person.panels.press(&person.snap); person.panels.release();
    assert_eq!(person.panels.web.as_ref().map(|w| w.url.as_str()), Some("https://orbiters.cc/b"), "the old bar must not close a different page");
    person.turns_until("camera bar B", |p| p.panels.list.iter().find(|p| p.zone == Zone::CameraWebBar).is_some_and(|p| p.displayed.as_ref().and_then(|f| f.camera.web.as_ref()).is_some_and(|w| w.url == "https://orbiters.cc/b")));
    let at = person.find(Action::WebClose).unwrap(); person.panels.hover(Some(at)); person.panels.press(&person.snap);
    assert!(person.panels.web.is_none(), "the current bar still closes its own page");
}

#[test]
fn review_accepted_worker_pictures_consume_the_opening_flag_and_keep_the_atlas() {
    let mut person = Person::new(); person.panels.stage.async_pages = true; person.around = Some(Around::QuickMenu); person.turns(3);
    person.panels.open_sheet(Sheet::Settings, "", &person.snap);
    person.turns_until("first picture", |p| p.panels.takeover.as_ref().is_some_and(|p| p.painted) && !p.panels.stage.painter.busy());
    person.panels.close_sheet(); person.turns(2);
    person.panels.open_sheet(Sheet::Settings, "", &person.snap);
    person.turns_until("reopened picture", |p| p.panels.list.iter().find(|p| p.zone == Zone::SheetBack).is_some_and(|p| p.painted) && !p.panels.stage.painter.busy());
    let header = person.panels.list.iter().find(|p| p.zone == Zone::SheetBack).unwrap();
    let atlas = header.canvas.id;
    assert!(!header.again && !person.panels.takeover.as_ref().unwrap().again, "a published picture has completed the new opening");
    person.panels.paint_all_again();
    assert_eq!(person.panels.list.iter().find(|p| p.zone == Zone::SheetBack).unwrap().canvas.id, atlas, "an already painted header must keep its atlas through a color repaint");
}

#[test]
fn review_cached_header_pixels_do_not_claim_a_new_context_has_fonts() {
    let mut person = Person::new(); person.panels.stage.async_pages = true; person.around = Some(Around::QuickMenu); person.turns(3);
    person.panels.open_sheet(Sheet::Settings, "", &person.snap);
    person.turns_until("original header", |p| p.panels.takeover.as_ref().is_some_and(|p| p.painted) && !p.panels.stage.painter.busy());
    let old_id = person.panels.stage.cache.iter().find(|f| f.sheet == Sheet::Settings).unwrap().header_spec.as_ref().unwrap().canvas_id;
    person.panels.close_sheet();
    person.turns_until("closed header", |p| !p.panels.list.iter().any(|p| p.zone == Zone::SheetBack) && !p.panels.stage.painter.busy());
    person.panels.rested.retain(|p| p.panel.zone != Zone::SheetBack);
    let (at, go) = person.panels.stage.painter.hold_next();
    person.panels.open_sheet(Sheet::Settings, "", &person.snap); person.turn();
    at.recv_timeout(Duration::from_secs(10)).unwrap();
    let header = person.panels.list.iter().find(|p| p.zone == Zone::SheetBack).unwrap();
    let id = header.canvas.id;
    assert_ne!(id, old_id);
    assert!(header.painted, "the old valid picture is immediately visible");
    assert!(!header.canvas.ran, "cached pixels do not initialize a different context's fonts");
    assert_eq!(header.canvas.atlas_bytes(), 0);
    go.send(()).unwrap();
    person.turns_until("new header fonts", |p| p.panels.list.iter().find(|p| p.zone == Zone::SheetBack).is_some_and(|p| p.canvas.ran) && !p.panels.stage.painter.busy());
    assert!(person.panels.list.iter().find(|p| p.zone == Zone::SheetBack).unwrap().canvas.atlas_bytes() > 0);
}

/// Enzo, 2026-11-13, in the headset: "When switching from page to page, there is 1 or a few frame where no page is visible, it breaks the flow". Every way of going from a
/// page to another (a head tab, a side tile, a card, Back) keeps the page left SEEN on the sheet until the next page's first picture is ready, and nothing on that old
/// picture answers a pointer meanwhile. The worker is held so that those frames are many, as on a busy computer; `vr::part_shows` is what `OrbitVr::show` hides by.
#[test]
fn switching_pages_never_shows_no_page() {
    let mut person = Person::new();
    person.panels.stage.async_pages = true;
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    fn sheet_seen(p: &Person) -> bool {
        p.panels.list.iter().find(|x| x.zone == Zone::SheetBack).is_some_and(|x| super::vr::part_shows(x) && !x.pixels().is_empty())
    }
    fn settled(p: &Person) -> bool {
        sheet_seen(p) && p.panels.takeover.as_ref().is_some_and(|t| t.painted && !t.dirty) && !p.panels.stage.painter.busy()
    }
    person.press(Action::Pictures);
    person.turns_until("Pictures shown", settled);
    // How each switch is pressed: on the sheet's head (its tabs, Back), on the wing beside it, or on the page (All tools' cards).
    let head = |z: Zone| z == Zone::SheetBack;
    let wing = |z: Zone| Zone::WING.contains(&z);
    let page = |z: Zone| z == Zone::Takeover;
    let ways: [(&str, &dyn Fn(Zone) -> bool, Action, Option<Sheet>); 5] = [
        ("a head tab", &head, Action::Page(Target::YouTube), None),
        ("a side tile", &wing, Action::Page(Target::Instances), Some(Sheet::Instances)),
        ("All tools' head tab", &head, Action::All, Some(Sheet::All)),
        ("a card on All tools", &page, Action::Settings, Some(Sheet::Settings)),
        ("Back", &head, Action::Back, Some(Sheet::All)),
    ];
    for (way, on, action, expected) in ways {
        // Each a first visit: no kept picture of it to show at once.
        person.panels.stage.cache.clear();
        let was = person.panels.facts.sheet;
        let (part, at) = person.panels.all().enumerate().find_map(|(i, p)| on(p.zone).then(|| p.items.iter().find(|it| it.action == action).map(|it| (i, it.rect.center()))).flatten()).unwrap_or_else(|| panic!("{way}: nothing does {action:?} from {was:?}"));
        let (asked, go) = person.panels.stage.painter.hold_next();
        person.panels.hover(Some((part, at)));
        person.turn();
        person.panels.press(&person.snap.clone());
        person.panels.release();
        if let Some(late) = person.panels.pressed_late.take() { person.panels.act(late, &person.snap.clone(), &mut super::Pressed::default()); }
        person.panels.hover(None);
        let to = person.panels.facts.sheet;
        assert!(to.is_some() && to != was && expected.is_none_or(|e| to == Some(e)), "{way}: {action:?} from {was:?} went to {to:?}");
        // Until the worker has painted the next page, the one left shows, pointed at or not, and nothing of it is pointable.
        let began = Instant::now();
        while asked.try_recv().is_err() {
            assert!(began.elapsed() < Duration::from_secs(10), "{way}: the next page was never asked for");
            person.turn();
            assert!(sheet_seen(&person), "{way}: no page shown while the next is asked for");
        }
        for _ in 0..6 {
            person.turn();
            assert!(sheet_seen(&person), "{way}: no page shown while the next one is painted");
            let back = person.panels.list.iter().position(|x| x.zone == Zone::SheetBack).unwrap();
            let middle = (person.panels.list[back].logical / 2.0).to_pos2();
            person.panels.hover(Some((back, middle)));
            person.panels.hover(Some((person.panels.list.len(), egui::pos2(512.0, 300.0))));
            assert!(person.panels.takeover.as_ref().unwrap().hover.is_none() && person.panels.list[back].hover.is_none(), "{way}: the page left answers a pointer");
            person.panels.hover(None);
        }
        go.send(()).unwrap();
        person.turns_until("the next page shown", |p| { assert!(sheet_seen(p), "no page shown as the next one comes"); settled(p) && p.panels.stage.presented.as_ref().is_some_and(|(_, f, _)| f.sheet == to) });
    }
}

/// The beta's usage statistics follow the person through the menus: which menu, sheet and N4N0 page they are on, and what they press there, by names that
/// are the same in every language and hold nothing typed.
#[test]
fn usage_follows_the_menus_and_their_presses() {
    crate::usage::reset();
    crate::usage::set_enabled(true);
    let mut person = Person::new();
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    person.press(Action::Worlds);
    person.turns(2);
    person.press(Action::All);
    person.turns(2);
    person.press(Action::Settings);
    person.turns(3);
    person.press(Action::CloseSheet);
    person.turns(2);
    person.around = None;
    person.turns(2);
    let steps = crate::usage::take();
    crate::usage::set_enabled(false);
    let entered: Vec<(&str, Option<&str>)> = steps.iter().filter(|s| s.kind == crate::usage::Kind::Enter).map(|s| (s.screen.as_str(), s.from.as_deref())).collect();
    assert_eq!(
        entered,
        vec![("qm", None), ("qm/Worlds", Some("qm")), ("qm/All", Some("qm/Worlds")), ("qm/Settings", Some("qm/All")), ("qm", Some("qm/Settings")), ("closed", Some("qm"))],
        "{steps:?}"
    );
    let pressed: Vec<(&str, &str)> = steps.iter().filter(|s| s.kind == crate::usage::Kind::Press).map(|s| (s.screen.as_str(), s.control.as_deref().unwrap())).collect();
    assert_eq!(pressed, vec![("qm", "Worlds"), ("qm/Worlds", "All"), ("qm/All", "Settings"), ("qm/Settings", "CloseSheet")], "{steps:?}");
}

/// Opt-in, against a server under test (`ORBITERS_USAGE_API=http://127.0.0.1:4200 cargo test -- --ignored usage_reaches`): a walk through the
/// menus as a person takes it, noted by the real recorder, kept by the real spool and sent over HTTP as the app sends it.
#[test]
#[ignore]
fn usage_reaches_a_server_under_test() {
    let Ok(base) = std::env::var("ORBITERS_USAGE_API") else { return };
    let spool = crate::usage::spool::Spool::default();
    for (link, around, walk) in [
        (1, Around::QuickMenu, vec![Action::Worlds, Action::Pictures, Action::All, Action::Settings, Action::CloseSheet]),
        (2, Around::Wheel, vec![Action::Pictures, Action::All, Action::Translate, Action::CloseSheet]),
        (3, Around::QuickMenu, vec![Action::Page(Target::Instances), Action::Worlds, Action::CloseSheet]),
    ] {
        crate::usage::reset();
        crate::usage::set_enabled(true);
        spool.linked(link, crate::ipc::Role::Overlay);
        let mut person = Person::new();
        person.around = Some(around);
        person.turns(3);
        for action in walk {
            if person.find(action).is_some() {
                person.press(action);
                person.turns(2);
            }
        }
        person.around = None;
        person.turns(2);
        spool.steps(link, crate::usage::take(), true);
        spool.unlinked(link, true);
    }
    crate::usage::set_enabled(false);
    let waiting = spool.waiting().len();
    assert!(waiting > 10, "{waiting}");
    assert_eq!(spool.send(&orbiters_core::account::Api::at(&base), None), crate::usage::spool::Sent::Taken(waiting));
}

/// A person at VRChat's desktop camera (L-desk-camera): the app sees no headset, VRChat says its camera is open, the column is up beside its panel.
fn at_the_desktop_camera() -> (Person, Arc<Heard>) {
    let (mut person, heard) = Person::heard();
    let mut snap = (*person.snap).clone();
    snap.steamvr.running = false;
    snap.camera_open_said = Some(true);
    snap.worn_kit = Some(true);
    person.snap = Arc::new(snap);
    person.around = Some(Around::Camera);
    person.turns(3);
    assert!(person.panels.facts.desktop, "the desktop");
    (person, heard)
}

/// Enzo, 2026-11-15 (L-desk-camera): the column beside the desktop's camera panel has the headset's four tools, and each does its work with what the desktop has:
/// a 3D picture is asked for and starts once VRChat says where its camera is (the service's wait, which a second press calls off), Record records VRChat's camera,
/// the QR code makes the camera look for a code. None is a dead button.
#[test]
fn beside_the_desktop_camera_each_tool_does_its_work() {
    use orbiters_n4n0_core::menu::PhotoMode;
    let (mut person, heard) = at_the_desktop_camera();
    for action in [Action::Picture(PhotoMode::People), Action::Picture(PhotoMode::World), Action::Record, Action::CameraQr] {
        assert!(person.find(action).is_some(), "no {action:?} beside the desktop's camera");
    }
    assert!(person.press(Action::Picture(PhotoMode::People)).clicked);
    assert!(heard.said().iter().any(|c| c.starts_with("Photo3DWhenCamera")), "no 3D picture asked for: {:?}", heard.said());
    assert_eq!(person.panels.facts.picture_mode, Some(PhotoMode::People));
    assert!(person.press(Action::Record).clicked);
    assert!(person.press(Action::CameraQr).clicked);
    assert_eq!(*heard.camera.lock().unwrap(), ["record", "qr on"]);
}

/// N-length-switch (Enzo, 2026-11-16: "switch between 1, 4 and 8 sec captures directly from the overlay camera UI"): in the headset and
/// on the desktop, both 3D tiles show the length and their chip goes round 1, 4 and 8 s, saved as the Capture page's one setting; a
/// picture pressed at once after takes the length just chosen, not the one the settings had a moment before.
#[test]
fn the_camera_s_3d_tiles_show_and_change_the_pictures_length() {
    use orbiters_core::settings::Photo3dLength;
    use orbiters_n4n0_core::menu::PhotoMode;
    for desktop in [true, false] {
        let (mut person, heard) = at_the_desktop_camera();
        if !desktop {
            let mut snap = (*person.snap).clone();
            snap.steamvr.running = true;
            person.snap = Arc::new(snap);
            person.turns(3);
            assert!(!person.panels.facts.desktop);
        }
        let length_said = |heard: &Heard| heard.patches.lock().unwrap().iter().rev().find_map(|p| p.get("photo_3d").cloned());
        assert_eq!(person.panels.facts.length, Photo3dLength::Standard, "the setting's default, 4 s");
        let tiles: Vec<Zone> = person.panels.all().filter(|p| p.items.iter().any(|i| i.action == Action::PictureLength)).map(|p| p.zone).collect();
        assert_eq!(tiles, [Zone::CameraPeople, Zone::CameraWorld], "desktop {desktop}: the length is on both 3D tiles, and nowhere else");
        for want in [Photo3dLength::Full, Photo3dLength::Quick, Photo3dLength::Standard] {
            assert!(person.press(Action::PictureLength).clicked);
            assert_eq!(person.panels.facts.length, want, "desktop {desktop}");
            assert_eq!(length_said(&heard), Some(serde_json::to_value(want).unwrap()), "desktop {desktop}: saved as the Capture page's setting");
            // The service says it back.
            let mut snap = (*person.snap).clone();
            snap.settings.photo_3d = want;
            person.snap = Arc::new(snap);
            person.turns(2);
            assert_eq!(person.panels.facts.length, want);
        }
        // 8 s chosen, and the settings have not said it back yet: the picture pressed at once still takes 8 s.
        assert!(person.press(Action::PictureLength).clicked);
        assert_eq!(person.panels.facts.length, Photo3dLength::Full);
        heard.patches.lock().unwrap().clear();
        assert!(person.press(Action::Picture(PhotoMode::World)).clicked);
        assert_eq!(length_said(&heard), Some(serde_json::to_value(Photo3dLength::Full).unwrap()), "desktop {desktop}: {:?}", heard.patches.lock().unwrap());
        // No capture kit: the tile is locked, and so is its length.
        let mut snap = (*person.snap).clone();
        snap.worn_kit = Some(false);
        person.snap = Arc::new(snap);
        person.turns(3);
        assert!(person.find(Action::PictureLength).is_none());
    }
}

/// Waiting for VRChat to say where its camera is, the desktop's column says so under the tiles, and how to call it off; the countdown is on the tile pressed.
#[test]
fn the_desktop_camera_s_column_says_what_a_3d_picture_waits_for() {
    use super::panels::camera_words;
    use orbiters_n4n0_core::menu::PhotoMode;
    let (mut person, _) = at_the_desktop_camera();
    person.press(Action::Picture(PhotoMode::World));
    let mut snap = (*person.snap).clone();
    snap.n4n0.photo_wait = Some(orbiters_n4n0::status::PhotoWait::Move);
    person.snap = Arc::new(snap);
    person.panels.hover(None);
    person.turns(3);
    let said = camera_words(&person.panels.facts).expect("words under the tiles");
    assert_eq!(said.0, orbiters_i18n::tr("Move VRChat's camera a little"));
    assert!(person.panels.all().any(|p| p.zone == Zone::CameraCaption && p.shown));
    let mut snap = (*person.snap).clone();
    snap.n4n0.photo_wait = None;
    snap.n4n0.photo_countdown = Some(3);
    person.snap = Arc::new(snap);
    person.turns(3);
    assert_eq!(person.panels.facts.countdown, Some(3), "the tile counts down");
}

/// On the desktop nothing may lie on the camera's picture: a code found opens its page in the person's browser, once for that scan, and no page shows over the
/// camera. In a headset the page still opens over it (`a_code_found_beside_the_camera_opens_its_page_over_it_once`).
#[test]
fn a_code_found_beside_the_desktop_camera_opens_in_the_browser() {
    use crate::controller::camera_tools::Qr;
    let (mut person, heard) = at_the_desktop_camera();
    assert_eq!(super::panels::caption(Action::CameraQr, &person.panels.facts).map(|c| c.1), Some(orbiters_i18n::tr("Point VRChat's camera at a code: its page opens in your browser.").to_string()));
    let mut snap = (*person.snap).clone();
    snap.camera_tools.qr = Qr::Found { url: "https://orbiters.cc/w/silent-dawn".into() };
    snap.camera_tools.qr_generation = 3;
    person.snap = Arc::new(snap);
    person.turns(5);
    assert_eq!(person.panels.opened_in_browser, ["https://orbiters.cc/w/silent-dawn"], "opened once");
    assert!(person.panels.facts.camera.web.is_none(), "a page over the camera");
    assert!(!person.panels.all().any(|p| matches!(p.zone, Zone::CameraWeb | Zone::CameraWebBar)));
    assert_eq!(heard.camera.lock().unwrap().last().map(String::as_str), Some("qr off"), "the scan stops once its code is found");
}

/// Enzo, 2026-11-16: "I would like to be able to grab the volume slider like for the timeline position slider." Now playing's level is grabbed where pressed, follows
/// the pointer while held (even off the bar, past its ends), sets the volume as it goes and where it is let go: the playing app's (the computer's where apps have
/// none), the same setting as its minus and plus.
#[test]
fn now_playing_s_volume_is_grabbed_and_dragged_as_its_timeline_is() {
    use orbiters_n4n0_core::menu::PlayingVolume;
    let mut person = Person::with_a_song();
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    person.press(Action::All);
    person.turns_until("Now playing's volume", |p| p.find(Action::VolumeBar).is_some());
    let (part, at) = person.find(Action::VolumeBar).unwrap();
    let grab = person.panels.all().nth(part).unwrap().items.iter().find(|i| i.action == Action::VolumeBar).unwrap().rect;
    let x = |f: f32| grab.left() + 13.0 + (grab.width() - 26.0) * f;
    let level = |p: &Person| match p.panels.stage.host.menu.playing_volume() {
        Some(PlayingVolume::App(v) | PlayingVolume::Computer(v)) => v.percent,
        other => panic!("{other:?}"),
    };
    assert_eq!(level(&person), 40);
    // Grabbed near its left: set there at once.
    person.panels.hover(Some((part, egui::pos2(x(0.1), at.y))));
    person.turn();
    let snap = person.snap.clone();
    person.panels.press(&snap);
    person.turn();
    assert_eq!(level(&person), 10);
    // Dragged right, the hand drifting off the bar and past its end: it follows, set as it goes.
    for (f, dy) in [(0.5, 10.0), (0.8, 30.0), (1.4, 30.0)] {
        std::thread::sleep(super::VOLUME_EVERY);
        person.panels.hover(Some((part, egui::pos2(x(f), at.y + dy))));
        person.turn();
    }
    assert_eq!(person.panels.facts.volume.map(|v| v.0), Some(100), "shown where it is held");
    assert_eq!(level(&person), 100);
    // Let go a little back: where it was let go, and no longer held.
    person.panels.hover(Some((part, egui::pos2(x(0.6), at.y))));
    person.turn();
    person.panels.release();
    person.turn();
    assert_eq!(level(&person), 60);
    assert!(person.panels.volume_held.is_none());
    // Pointing at it again without a press moves nothing.
    person.panels.hover(Some((part, egui::pos2(x(0.2), at.y))));
    person.turns(2);
    assert_eq!(level(&person), 60);
}

/// Enzo, 2026-11-16: "a very easy way to disable the conversation / AI assistant feature of N4N0 from the overlay ... with a little switch on
/// the corner of the feature tile". Through the hand: the switch on Ask N4N0's corner asks N4N0 to turn its assistant off and shows it off
/// at once, opening nothing; the rest of the tile still opens Ask N4N0, whose button then offers to turn it on again, and does.
#[test]
fn ask_n4n0_s_switch_turns_the_assistant_off_and_its_page_on_again() {
    use super::panels::TalkNow;
    let (mut person, heard) = Person::heard();
    let set = |person: &mut Person, on: bool| {
        let mut snap = (*person.snap).clone();
        snap.settings.n4n0.enabled = true;
        snap.settings.n4n0.assistant = on;
        person.snap = Arc::new(snap);
        person.turns(2);
    };
    set(&mut person, true);
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    assert!(!person.panels.facts.assistant_off);
    person.press(Action::AssistantSwitch);
    assert!(heard.said().iter().any(|c| c.contains("Assistant") && c.contains("false")), "asked of N4N0: {:?}", heard.said());
    assert!(person.panels.facts.assistant_off, "off at once, before the settings say it");
    assert_eq!(person.panels.facts.sheet, None, "the switch opens nothing");
    // N4N0 saved it: still off, and it stays so.
    set(&mut person, false);
    assert!(person.panels.facts.assistant_off);
    // The tile itself: Ask N4N0, which says the assistant is off and how to have it back.
    person.press(Action::Talk);
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Talk));
    assert_eq!(person.panels.facts.talk, TalkNow::AssistantOff);
    let before = heard.said().len();
    person.press(Action::TalkToggle);
    let said = heard.said()[before..].to_vec();
    assert!(said.iter().any(|c| c.contains("Assistant") && c.contains("true")), "turned on again: {said:?}");
    assert!(!said.iter().any(|c| c.contains("AskTalk")), "nothing listened to by that press");
    assert!(!person.panels.facts.assistant_off);
}
