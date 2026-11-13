//! VRChat's own Quick Menu buttons that take its Quick Menu away (Enzo, 2026-11-13: "When I click on a menu button from the quick menu (opening the Avatar page for
//! instance on the big menu and making the quick menu go away), the overlay stays [...] could be with invisible buttons hitbox at the same place as the vrchat quick
//! menu buttons").
//!
//! VRChat says nothing of its menus; what is known here is the button that opened its Quick Menu ([`super::vr::MenuButtons`]). So the press of a laser on one of
//! its buttons is watched: where the pointing hand's laser meets the Quick Menu's own plane (the plane the wing and the sheet are laid on), and its trigger going down
//! and up there, as a click is in VRChat's menus. **Nothing here is an overlay and nothing takes a trigger**: these "hitboxes" are rectangles in the menu's units,
//! looked at with the trigger VRChat itself receives ([`super::super::actions::Actions::orbit_triggers`] with no hand blocked), so they cannot swallow a click VRChat
//! needs. A press that began on one of Orbiters' parts was Orbiters' (blocked from VRChat) and is not looked at.
//!
//! What is under the laser depends on the page VRChat's Quick Menu shows: its row of tabs (rocket, bell, pin, camera, sound, chat, gear) is on every page at the same
//! place, and My Profile beside it is the same on every page; the buttons of the Launch Pad (the rocket's page) only there. A button that opens another page of the
//! Quick Menu (not the Main Menu) leaves what lies where unknown until a tab is pressed: nothing on the main panel counts then. Units: a 1280 x 720 window's pixels
//! from the menu's middle, x right and y down ([`super::zones`]), measured on VRChat's own menu (`sota/reports/orbit-overlays/qm-live-1280.png`) and checked in the
//! SteamVR sandbox with real VRChat (`done4-shell.md`).

use super::zones::{QM_ICONS_X, QM_ICONS_Y};
use egui::{pos2, Pos2, Rect};

/// What pressing a button of VRChat's Quick Menu does to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leads {
    /// VRChat's Quick Menu goes (its Main Menu opens in its place): Orbiters' parts go with it.
    Away,
    /// Another page of the Quick Menu: what is where is not known until a tab is pressed.
    Elsewhere,
    /// The page the rocket's tab shows, or another tab's.
    Tab(usize),
}

/// A button of VRChat's Quick Menu, by what it says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    LiveNow,
    Worlds,
    Avatars,
    Social,
    Groups,
    Shop,
    ProfileCard,
    ProfileAvatar,
    ProfileWorld,
    ProfileFriends,
    Home,
    Respawn,
    Select,
    Safety,
    Tab(usize),
}

/// Where each is, and what it leads to. A margin is left inside each edge: a laser on a button's very edge may be on its neighbour for VRChat, whose own laser
/// starts where this one does (the controller's tip) but is read a frame apart.
const INSIDE: f32 = 5.0;

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect::from_min_max(pos2(x0 + INSIDE, y0 + INSIDE), pos2(x1 - INSIDE, y1 - INSIDE))
}

/// The Launch Pad's buttons: its grid of six (to the Main Menu's pages) and the row under it.
fn launch_pad() -> [(Button, Rect, Leads); 10] {
    [
        (Button::LiveNow, rect(-143.0, 1.0, -57.0, 60.0), Leads::Away),
        (Button::Worlds, rect(-43.0, 1.0, 43.0, 60.0), Leads::Away),
        (Button::Avatars, rect(57.0, 1.0, 143.0, 60.0), Leads::Away),
        (Button::Social, rect(-143.0, 73.0, -57.0, 132.0), Leads::Away),
        (Button::Groups, rect(-43.0, 73.0, 43.0, 132.0), Leads::Away),
        (Button::Shop, rect(57.0, 73.0, 143.0, 132.0), Leads::Away),
        (Button::Home, rect(-164.0, 169.0, -80.0, 196.0), Leads::Elsewhere),
        (Button::Respawn, rect(-78.0, 169.0, 0.0, 196.0), Leads::Elsewhere),
        (Button::Select, rect(5.0, 169.0, 85.0, 196.0), Leads::Elsewhere),
        (Button::Safety, rect(90.0, 169.0, 165.0, 196.0), Leads::Elsewhere),
    ]
}

/// My Profile, the side panel at the menu's right on every page: its card and its rows to the Main Menu.
fn my_profile() -> [(Button, Rect, Leads); 4] {
    [
        (Button::ProfileCard, rect(196.0, -111.0, 318.0, -26.0), Leads::Away),
        (Button::ProfileAvatar, rect(196.0, 36.0, 318.0, 72.0), Leads::Away),
        (Button::ProfileWorld, rect(196.0, 84.0, 318.0, 120.0), Leads::Away),
        (Button::ProfileFriends, rect(196.0, 132.0, 318.0, 168.0), Leads::Away),
    ]
}

/// The row of tabs under the main panel, on every page.
fn tabs() -> impl Iterator<Item = (Button, Rect, Leads)> {
    QM_ICONS_X.iter().enumerate().map(|(i, x)| (Button::Tab(i), Rect::from_center_size(pos2(*x, QM_ICONS_Y), egui::vec2(40.0, 36.0)), Leads::Tab(i)))
}

/// The Quick Menu's page as far as is known: the Launch Pad (its rocket tab, where VRChat starts), another tab's, or not known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    LaunchPad,
    Tab(usize),
    Unknown,
}

/// The button at `at` on `page`, and what it leads to.
pub fn button_at(at: Pos2, page: Page) -> Option<(Button, Leads)> {
    let all = launch_pad();
    let pad: &[(Button, Rect, Leads)] = if page == Page::LaunchPad { &all } else { &[] };
    tabs().chain(my_profile()).chain(pad.iter().copied()).find(|(_, r, _)| r.contains(at)).map(|(b, _, l)| (b, l))
}

/// Watches the presses of the laser of the hand that does not hold the Quick Menu on its buttons.
#[derive(Clone, Copy, Debug)]
pub struct QmWatch {
    page: Page,
    /// The button a trigger went down on, per hand: a click is down and up on the same.
    down: [Option<Button>; 2],
    /// Each trigger as last seen, and whether its press began on Orbiters' parts (blocked from VRChat).
    was: [bool; 2],
    ours: [bool; 2],
    /// For the log: where the last press on VRChat's menu went down (its units, the button there), and the last click with what it led to.
    pub pressed: Option<(Pos2, Option<Button>)>,
    pub clicked: Option<(Button, Leads)>,
}

impl Default for QmWatch {
    fn default() -> Self {
        // VRChat starts on its Launch Pad; this may start with it elsewhere: not known until a tab is pressed, or VRChat starts again.
        Self { page: Page::Unknown, down: [None; 2], was: [true; 2], ours: [false; 2], pressed: None, clicked: None }
    }
}

impl QmWatch {
    pub fn page(&self) -> Page {
        self.page
    }

    /// VRChat (re)started: its Quick Menu shows its Launch Pad.
    pub fn vrchat_started(&mut self) {
        self.page = Page::LaunchPad;
    }

    /// Where a laser from `tip` meets VRChat's Quick Menu laid on `plane` (its frame on the menu's middle, +Z towards who holds it), in the menu's units: x right, y
    /// down. `None` when it points away from it or along it.
    pub fn on_menu(plane: &super::vr::Plane, tip: &orbiters_openvr::Matrix34) -> Option<Pos2> {
        use super::super::floating::space;
        let (origin, direction) = space::ray(tip);
        let local = space::inverse(&plane.frame);
        let (o, d) = (space::apply(&local, origin), space::rotate(&local, direction));
        if d[2].abs() < 1e-6 {
            return None;
        }
        let t = -o[2] / d[2];
        if !(t > 0.0) || plane.unit <= 0.0 {
            return None;
        }
        Some(pos2((o[0] + d[0] * t) / plane.unit, -(o[1] + d[1] * t) / plane.unit))
    }

    /// One turn with VRChat's Quick Menu open: for each hand, where its laser meets the menu's plane (in its units; `None` off it or for the hand that holds it),
    /// whether it is on one of Orbiters' parts, and its trigger as VRChat gets it. Returns the button clicked that takes the Quick Menu away, if one was.
    pub fn step(&mut self, at: [Option<Pos2>; 2], on_ours: [bool; 2], trigger: [bool; 2]) -> Option<Button> {
        let mut away = None;
        for hand in 0..2 {
            let (down, was) = (trigger[hand], self.was[hand]);
            if down && !was {
                self.ours[hand] = on_ours[hand];
                self.down[hand] = if on_ours[hand] { None } else { at[hand].and_then(|p| button_at(p, self.page)).map(|(b, _)| b) };
                if let Some(p) = at[hand].filter(|_| !on_ours[hand]) {
                    self.pressed = Some((p, self.down[hand]));
                }
            } else if !down && was {
                let pressed = self.down[hand].take();
                let released = if self.ours[hand] { None } else { at[hand].and_then(|p| button_at(p, self.page)) };
                if let (Some(pressed), Some((button, leads))) = (pressed, released) {
                    if pressed == button {
                        self.clicked = Some((button, leads));
                        match leads {
                            Leads::Away => away = Some(button),
                            Leads::Elsewhere => self.page = Page::Unknown,
                            Leads::Tab(0) => self.page = Page::LaunchPad,
                            Leads::Tab(i) => self.page = Page::Tab(i),
                        }
                    }
                }
                self.ours[hand] = false;
            }
            self.was[hand] = down;
        }
        away
    }

    /// The Quick Menu closed or another opened: no press under way is VRChat's Quick Menu's.
    pub fn closed(&mut self) {
        self.down = [None; 2];
        // A trigger held through the change presses nothing on the next menu.
        self.was = [true; 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click(watch: &mut QmWatch, at: Pos2) -> Option<Button> {
        let mut away = watch.step([None, Some(at)], [false; 2], [false; 2]);
        away = away.or(watch.step([None, Some(at)], [false; 2], [false, true]));
        away.or(watch.step([None, Some(at)], [false; 2], [false; 2]))
    }

    fn centre(b: Button, page: Page) -> Pos2 {
        let all: Vec<(Button, Rect, Leads)> = tabs().chain(my_profile()).chain(launch_pad()).collect();
        let r = all.iter().find(|(x, _, _)| *x == b).unwrap().1;
        assert_eq!(button_at(r.center(), page).map(|(x, _)| x), Some(b));
        r.center()
    }

    /// Enzo: the Avatars button of VRChat's Quick Menu opens its Main Menu and the Quick Menu goes: Orbiters' parts must go with it. Its tabs, and its buttons
    /// that keep it up, must not.
    #[test]
    fn the_main_menu_s_buttons_take_the_parts_away_and_the_others_do_not() {
        let mut watch = QmWatch::default();
        watch.vrchat_started();
        for b in [Button::LiveNow, Button::Worlds, Button::Avatars, Button::Social, Button::Groups, Button::Shop] {
            assert_eq!(click(&mut watch, centre(b, Page::LaunchPad)), Some(b), "{b:?} on the Launch Pad");
        }
        // The tabs keep it up, and another tab's page has none of the Launch Pad's buttons.
        let avatars = centre(Button::Avatars, Page::LaunchPad);
        assert_eq!(click(&mut watch, centre(Button::Tab(3), Page::LaunchPad)), None, "the camera's tab");
        assert_eq!(watch.page(), Page::Tab(3));
        assert_eq!(click(&mut watch, avatars), None, "where Avatars is on the Launch Pad, on the camera's page");
        // My Profile is beside every page.
        for b in [Button::ProfileCard, Button::ProfileAvatar, Button::ProfileWorld, Button::ProfileFriends] {
            assert_eq!(click(&mut watch, centre(b, Page::Tab(3))), Some(b), "{b:?} on another tab's page");
        }
        assert_eq!(click(&mut watch, centre(Button::Tab(0), Page::Tab(3))), None);
        assert_eq!(click(&mut watch, avatars), Some(Button::Avatars), "back on the Launch Pad");
        // A page of its own (Safety, Select...) leaves the main panel unknown until a tab.
        assert_eq!(click(&mut watch, centre(Button::Safety, Page::LaunchPad)), None);
        assert_eq!(watch.page(), Page::Unknown);
        assert_eq!(click(&mut watch, avatars), None, "on an unknown page");
    }

    /// A press is a click only down and up on the same button, as in VRChat's menus; one begun on Orbiters' parts was Orbiters' and VRChat never had it.
    #[test]
    fn only_a_click_of_vrchat_s_counts() {
        let mut watch = QmWatch::default();
        watch.vrchat_started();
        let (avatars, worlds) = (centre(Button::Avatars, Page::LaunchPad), centre(Button::Worlds, Page::LaunchPad));
        watch.step([None, Some(avatars)], [false; 2], [false; 2]);
        watch.step([None, Some(avatars)], [false; 2], [false, true]);
        assert_eq!(watch.step([None, Some(worlds)], [false; 2], [false; 2]), None, "down on one, up on another");
        watch.step([None, Some(avatars)], [false, true], [false, true]);
        assert_eq!(watch.step([None, Some(avatars)], [false; 2], [false; 2]), None, "begun on Orbiters' parts");
        // Between buttons, and off the menu.
        assert_eq!(click(&mut watch, pos2(50.0, 66.0)), None);
        watch.step([None, None], [false; 2], [false, true]);
        assert_eq!(watch.step([None, None], [false; 2], [false; 2]), None);
        // A trigger held when the menu opens presses nothing.
        let mut watch = QmWatch::default();
        watch.vrchat_started();
        assert_eq!(watch.step([None, Some(avatars)], [false; 2], [false, true]), None);
        assert_eq!(watch.step([None, Some(avatars)], [false; 2], [false; 2]), None, "held from before");
    }

    /// A laser from the other hand pointed at a button of VRChat's Quick Menu, held on either hand, is found on that button, as VRChat's own laser from the same tip.
    #[test]
    fn a_laser_on_the_menu_is_found_on_its_button() {
        use super::super::super::floating::space;
        let tune = super::super::vr::Tune::default();
        for hand in 0..2 {
            let grip = space::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [if hand == 0 { -0.2 } else { 0.2 }, 1.1, -0.3]);
            let plane = super::super::vr::quick_menu_plane(&grip, hand, &tune, 1.0);
            for b in [Button::Avatars, Button::ProfileWorld, Button::Tab(0)] {
                let at = centre(b, Page::LaunchPad);
                let target = space::apply(&plane.frame, [at.x * plane.unit, -at.y * plane.unit, 0.0]);
                // From in front of the menu, a little aside, pointing at it: the tip's -Z along the way.
                let from = space::add(target, space::add(space::scale(space::axis(&plane.frame, 2), 0.35), [0.1, 0.05, 0.0]));
                let back = space::unit(space::sub(from, target)).unwrap();
                let side = space::unit(space::cross([0.0, 1.0, 0.0], back)).unwrap();
                let tip = space::from_axes(side, space::cross(back, side), back, from);
                let found = QmWatch::on_menu(&plane, &tip).expect("on the plane");
                assert!((found - at).length() < 0.5, "{b:?} on hand {hand}: {found:?} for {at:?}");
                assert_eq!(button_at(found, Page::LaunchPad).map(|(x, _)| x), Some(b));
            }
            // Pointing away from it.
            let z = space::scale(space::axis(&plane.frame, 2), -1.0);
            let x = space::unit(space::cross([0.0, 1.0, 0.0], z)).unwrap();
            let away = space::from_axes(x, space::cross(z, x), z, space::add(space::position(&plane.frame), space::scale(space::axis(&plane.frame, 2), 0.3)));
            assert!(QmWatch::on_menu(&plane, &away).is_none(), "pointing away from it, on hand {hand}");
        }
    }

    /// No two buttons overlap, and none lies on Orbiters' wing beside the menu (on either hand) or the strip under its tabs.
    #[test]
    fn the_buttons_are_apart_and_clear_of_orbiters_parts() {
        let all: Vec<(Button, Rect, Leads)> = tabs().chain(my_profile()).chain(launch_pad()).collect();
        for (i, (a, ra, _)) in all.iter().enumerate() {
            for (b, rb, _) in &all[i + 1..] {
                assert!(!ra.intersects(*rb), "{a:?} and {b:?}");
            }
        }
        for places in [super::super::vr::quick_menu_places(0), super::super::vr::quick_menu_places(1), super::super::vr::strip_places()] {
            for part in places {
                for (b, r, _) in &all {
                    assert!(!part.rect.intersects(*r), "{b:?} under {:?}", part.zone);
                }
            }
        }
    }
}
