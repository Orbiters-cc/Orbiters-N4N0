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
    /// One person at a time: the look chosen (modern or retro) is the whole process's, as the app has one, and a retro test on another thread laid All tools out
    /// under a modern one's press, which then missed its tile (two of these failed so beside the hole's retro runs).
    _alone: std::sync::MutexGuard<'static, ()>,
}

static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl Person {
    fn new() -> Self {
        let alone = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let panels = Panels::new(Arc::new(crate::screenshot::Still), 2.0, true);
        let snap = Arc::new(crate::screenshot::idle_snapshot(&Settings::default(), &[]));
        Self { panels, snap, around: None, library: 0, _alone: alone }
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
        assert!(person.find(Action::Pictures).is_some(), "the wing is there");
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
    panels.peek_place = Some(zones::in_units(Zone::SheetPeek));
    panels.set_open(around, places, snap);
    panels.follow(snap, library);
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
    fn none(_: &std::path::Path, _: &[std::path::PathBuf]) -> Vec<super::LoadedRecent> {
        Vec::new()
    }
    for anew in [false, true] {
        let mut person = Person::with_a_song();
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
        person.turns_until("its parts at rest", |p| p.panels.all().all(|part| part.appear.settled() && part.painted) && p.panels.recents.rx.is_none());
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
    person.turns_until("its parts at rest", |p| p.panels.all().all(|part| part.appear.settled() && part.painted) && p.panels.recents.rx.is_none());
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
    assert!(handed.contains(&Zone::WingPlaying), "what plays beside the menu: {handed:?}");
    assert!(handed.contains(&Zone::Takeover), "and All tools: {handed:?}");
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
    let quick_menu = Zone::around(Around::QuickMenu).len();
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
    let both = quick_menu + Zone::around(Around::Wheel).len();
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
#[test]
fn the_hole_is_clear_in_what_is_submitted() {
    use crate::overlay::worlds::panel::Status;
    // At rest, and with its side panel still on its way in (a capture shows docked at once, as a world is pressed): in the same turn the hole opens, all of it
    // is clear on the page and on its back. dev.21's run once found the page still solid there: the page waited for its own 22 ms while the back did not.
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
        let worlds = person.panels.stage.worlds.as_mut().unwrap();
        if sliding {
            worlds.slide_world_in_for_test(7);
        } else {
            worlds.show_world_for_test(7);
        }
        // Painted just now, without its hole: the next paint is within the page's own 22 ms, whatever the machine's speed.
        person.turn();
        let docked = Status { headset: true, small: true, small_docked: true, shown: true, current: Some(7), ..Default::default() };
        let _ = person.panels.world_relay(docked.clone());
        person.turn();
        let [page_middle, page_outside, back_middle, back_outside] = person.panels.hole_alphas();
        let (page, back) = person.panels.hole_alphas_across();
        println!("hole, retro {retro} sliding {sliding}: page {page_middle:?} outside {page_outside:?}, back {back_middle:?} outside {back_outside:?}; across {page:?} {back:?}");
        assert!(!page.is_empty() && page.iter().all(|a| *a == Some(0)), "the page, retro {retro} sliding {sliding}: {page:?}");
        assert!(!back.is_empty() && back.iter().all(|a| *a == Some(0)), "its back, retro {retro} sliding {sliding}: {back:?}");
        assert!(page_outside.is_some_and(|a| a > 200), "and solid around it: {page_outside:?}");
        assert!(back_outside.is_some_and(|a| a > 200), "{back_outside:?}");
        // And it stays so, turn after turn.
        for _ in 0..6 {
            let _ = person.panels.world_relay(docked.clone());
            person.turn();
        }
        let (page, back) = person.panels.hole_alphas_across();
        assert!(page.iter().chain(&back).all(|a| *a == Some(0)), "later, retro {retro} sliding {sliding}: {page:?} {back:?}");
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
    person.turns_until("its parts at rest", |p| p.panels.all().all(|part| part.appear.settled() && part.painted) && p.panels.recents.rx.is_none());
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
    assert!(handed.contains(&Zone::WingPlaying), "what plays beside the menu: {handed:?}");
    assert!(handed.contains(&Zone::Takeover), "and All tools: {handed:?}");
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

/// Enzo (VR feedback F6): "Clicking Write a comment does nothing". A capture's page on the sheet in VR, small in its frame as he had it, its comments opened and
/// Write a comment pressed, all through the hand's pointer and the host as `vr.rs` drives them, with the world's feedback never read: the development server
/// answered 500 to every read, and Write a comment waited for that read, saying so for a few seconds in a corner of the page. A comment needs no read: signed in,
/// the keyboard opens; signed out, the button says Sign in to comment, says why beside itself when pressed, and starts signing in.
#[test]
fn write_a_comment_opens_its_keyboard_on_the_sheet_in_vr() {
    use crate::overlay::worlds::panel::{Compose, Page, Status, Target as WorldTarget};
    use orbiters_n4n0_core::feedback::FeedbackView;
    const LISTING: &str = r#"{"previews":[{"slot":0,"key":1,"id":7,"worldId":"wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd","name":"Silent Dawn","author":"Mei","uploader":"a","view":{"position":[0,1,0],"target":[0,1,1]}}]}"#;
    for signed_in in [true, false] {
        let mut person = Person::new();
        let mut snap = (*person.snap).clone();
        snap.n4n0.feedback = FeedbackView { signed_in, ..Default::default() };
        person.snap = Arc::new(snap);
        person.around = Some(Around::QuickMenu);
        person.turns(3);
        person.press(Action::Worlds);
        person.turns(2);
        assert_eq!(person.panels.facts.sheet, Some(Sheet::Worlds));
        let entry = crate::overlay::worlds::catalog::parse(LISTING).unwrap().remove(0);
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
        } else {
            assert_eq!(page(&person), Page::Comments, "nothing to write as, yet");
            assert!(person.panels.stage.worlds.as_ref().unwrap().target_for_test(WorldTarget::WriteComment).is_some(), "the way to sign in stays there");
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
    let listing = format!(
        r#"{{"previews":[{{"slot":0,"key":1,"id":7,"worldId":"{}","name":"World 1","author":"Mei","uploader":"a","view":{{"position":[0,1,0],"target":[0,1,1]}}}}]}}"#,
        world(1)
    );
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let mut person = Person::new();
    person.around = Some(Around::QuickMenu);
    person.turns(3);
    person.press(Action::Page(Target::Instances));
    person.turns(2);
    assert_eq!(person.panels.facts.sheet, Some(Sheet::Instances));
    let rows: Vec<InstanceRow> = (0..12).map(|i| InstanceRow { location: format!("{}:1234{i}~region(eu)", world(i)), world: format!("World {i}"), at: now - i as u64 * 3600 }).collect();
    person.panels.stage.worlds.as_ref().expect("the worlds' panel").listing_for_test(crate::overlay::worlds::catalog::parse(&listing).unwrap());
    for (n, capture) in [(0, false), (1, true)] {
        // These instances, whatever this computer's history has (opening Instances reads it).
        person.panels.stage.host.menu.set_instances(rows.clone(), true, "");
        person.turns(2);
        assert_eq!(person.panels.facts.worlds_visited.len(), 12);
        // Scrolled down, as a person reading the list is, to where a list comes to rest (a list still gliding takes a press for a drag).
        let scroll = if n == 0 { 0.0 } else { super::panels::snap_scroll(Sheet::Instances, &person.panels.facts, 150.0) };
        assert!(n == 0 || scroll > 0.0, "{scroll}");
        (person.panels.facts.gallery_scroll, person.panels.gallery_to) = (scroll, scroll);
        person.turns(2);
        assert_eq!(person.panels.facts.gallery_scroll, scroll, "at rest there");
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
    person.press(Action::Invite(0, 0));
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
