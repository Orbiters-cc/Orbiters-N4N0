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
}

impl Person {
    fn new() -> Self {
        let panels = Panels::new(Arc::new(crate::screenshot::Still), 2.0, true);
        let snap = Arc::new(crate::screenshot::idle_snapshot(&Settings::default(), &[]));
        Self { panels, snap, around: None }
    }

    /// One turn of the overlay's loop, as `vr.rs` takes it.
    fn turn(&mut self) {
        let mut places: Vec<Placed> = self.around.map_or_else(Vec::new, |m| Zone::around(m).iter().map(|z| zones::in_units(*z)).collect());
        self.panels.way_back(self.around, &mut places);
        self.panels.takeover_place = Some(zones::in_units(Zone::Takeover));
        self.panels.back_place = Some(zones::in_units(Zone::SheetBack));
        self.panels.peek_place = Some(zones::in_units(Zone::SheetPeek));
        self.panels.set_open(self.around, places, &self.snap);
        self.panels.follow(&self.snap, 0);
        self.panels.menu_turn(&self.snap);
        self.panels.step_springs(0.011);
        self.panels.paint(&self.snap, |_, _| {});
    }

    fn turns(&mut self, n: usize) {
        for _ in 0..n {
            self.turn();
        }
    }

    /// Where something that does `action` is: its part's place in the list, and a point on it.
    fn find(&self, action: Action) -> Option<(usize, egui::Pos2)> {
        self.panels.all().enumerate().find_map(|(i, p)| p.items.iter().find(|item: &&Item| item.action == action).map(|item| (i, item.rect.center())))
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
        // At rest it paints nothing, or nothing else would get a turn.
        person.turns(40);
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
