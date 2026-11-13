//! VRChat's Quick Menu going away under a press of its own buttons (Enzo, 2026-11-13: "When I click on a menu button from the quick menu (opening the Avatar page
//! for instance on the big menu and making the quick menu go away), the overlay stays [...] could be with invisible buttons hitbox at the same place as the vrchat
//! quick menu buttons").
//!
//! VRChat says nothing of its menus; [`super::vr::MenuButtons`] follows the button that opens its Quick Menu, and nothing saw a button *inside* it take it away.
//!
//! **Why not hitboxes placed on VRChat's buttons** (measured in the SteamVR sandbox with real VRChat, `done4-shell.md`): VRChat draws its laser from the
//! controller's raw pose with an offset of its own, not from the controller's tip that Orbiters' laser uses: pointed at one of Orbiters' tiles, VRChat's beam went
//! a long way past it. And the plane Orbiters lays its parts on was matched to VRChat's menu by eye, which leaves its depth free: VRChat's laser, coming from below,
//! landed one to two buttons away from where the same plane put it (aimed at Avatars, VRChat pressed Shop; aimed at the tabs, Groups). A button-by-button
//! map would close the parts on the wrong buttons and miss the right ones.
//!
//! **What is done instead, as cheaply:** a click VRChat itself received (the pointing hand's trigger down and up off Orbiters' parts, read with no trigger taken:
//! nothing here is an overlay and no click is swallowed, [`ClickWatch`]) is the cue to look whether VRChat's Quick Menu is still there, in the headset's own
//! picture: its row of tabs (rocket, bell, pin, camera, sound, chat, gear, on every page of it and on nothing else) where the plane puts it **as seen from the eye**,
//! which is what was matched by eye. [`Sight`] reads only that small part of one eye, on its own thread, a few times after a click; the tabs' shapes are the
//! desktop detector's ([`super::detect`]), sampled through the eye's projection. [`QmCheck`] decides: seen there in this opening, then not seen twice after a click:
//! it went (its Main Menu, Home, a portal...), and Orbiters' parts go. The same looks after a press of the menu button put right any other way the two got out of
//! step (VRChat's Main Menu closes on that press without opening the Quick Menu). Out of view, unreadable, or never seen in this opening: nothing is decided, and
//! the parts behave as before.

use super::super::floating::space::{self, V3};
use super::detect::{ICONS_NEEDED, ICON_HALF, ICON_MATCH, ICON_PATCH};
use super::vr::Plane;
use super::zones::{QM_ICONS_X, QM_ICONS_Y};
use orbiters_openvr::Matrix34;
use std::time::{Duration, Instant};

/// Watches the trigger VRChat gets, per hand: a click is down and up, begun off Orbiters' parts (a press begun on them was blocked from VRChat).
#[derive(Clone, Copy, Debug)]
pub struct ClickWatch {
    was: [bool; 2],
    ours: [bool; 2],
    /// A press seen beginning, per hand: one held from before clicks nothing when let go.
    began: [bool; 2],
}

impl Default for ClickWatch {
    fn default() -> Self {
        // A trigger already held when this starts clicks nothing.
        Self { was: [true; 2], ours: [false; 2], began: [false; 2] }
    }
}

impl ClickWatch {
    /// One turn: whether each hand's press began on Orbiters' parts (blocked), each trigger, and which hand may point (not the one holding the menu). True when
    /// VRChat got a click.
    pub fn step(&mut self, on_ours: [bool; 2], trigger: [bool; 2], pointing: [bool; 2]) -> bool {
        let mut clicked = false;
        for hand in 0..2 {
            let (down, was) = (trigger[hand], self.was[hand]);
            if down && !was {
                self.ours[hand] = on_ours[hand];
                self.began[hand] = true;
            } else if !down && was {
                clicked |= pointing[hand] && !self.ours[hand] && self.began[hand];
                self.ours[hand] = false;
                self.began[hand] = false;
            }
            self.was[hand] = down;
        }
        clicked
    }

    /// A menu closed or another opened: a trigger held through it clicks nothing.
    pub fn reset(&mut self) {
        self.was = [true; 2];
        self.began = [false; 2];
    }
}

/// Where a point lies in an eye's picture: `eye` the eye's pose in the play space, `raw` its sides' tangents ([`orbiters_openvr::Client::projection_raw`]), `size`
/// the picture's pixels. `None` behind the eye.
pub fn in_eye(eye: &Matrix34, raw: [f32; 4], size: [u32; 2], p: V3) -> Option<[f32; 2]> {
    let c = space::apply(&space::inverse(eye), p);
    if c[2] > -0.01 {
        return None;
    }
    let [left, right, top, bottom] = super::camera_track::eye_sides(raw);
    let (x, y) = (c[0] / -c[2], -c[1] / -c[2]);
    Some([(x - left) / (right - left) * size[0] as f32, (y - top) / (bottom - top) * size[1] as f32])
}

/// A point of the Quick Menu, in its units (x right, y down from its middle), in the play space.
pub fn on_plane(plane: &Plane, x: f32, y: f32) -> V3 {
    space::apply(&plane.frame, [x * plane.unit, -y * plane.unit, 0.0])
}

/// Shifts of the whole row tried, in the menu's units: the plane was matched by eye within a few, and the hand moves between the pose and the picture.
const SHIFTS: [f32; 5] = [-8.0, -4.0, 0.0, 4.0, 8.0];
/// At most this many found: no Quick Menu.
pub const GONE_AT_MOST: usize = 1;

/// Where each sample of each tab's patch lies in the eye's picture, at each shift: `None` when any of the row is behind the eye or off the picture.
fn samples(eye: &Matrix34, raw: [f32; 4], size: [u32; 2], plane: &Plane) -> Option<Vec<[f32; 2]>> {
    let mut out = Vec::with_capacity(SHIFTS.len() * SHIFTS.len() * QM_ICONS_X.len() * ICON_PATCH * ICON_PATCH);
    for dy in SHIFTS {
        for dx in SHIFTS {
            for x in QM_ICONS_X {
                for i in 0..ICON_PATCH * ICON_PATCH {
                    let g = |v: usize| ((v as f32 + 0.5) / ICON_PATCH as f32 * 2.0 - 1.0) * ICON_HALF;
                    let p = in_eye(eye, raw, size, on_plane(plane, x + dx + g(i % ICON_PATCH), QM_ICONS_Y + dy + g(i / ICON_PATCH)))?;
                    if !(0.0..size[0] as f32 - 1.0).contains(&p[0]) || !(0.0..size[1] as f32 - 1.0).contains(&p[1]) {
                        return None;
                    }
                    out.push(p);
                }
            }
        }
    }
    Some(out)
}

/// How many of the tabs' icons a picture shows at the shift that finds most: `grey` reads the picture at a pixel of the whole eye.
fn found_in(points: &[[f32; 2]], grey: impl Fn(f32, f32) -> f32) -> usize {
    let patch = ICON_PATCH * ICON_PATCH;
    let mut best = 0;
    for shift in points.chunks_exact(QM_ICONS_X.len() * patch) {
        let found = shift
            .chunks_exact(patch)
            .enumerate()
            .filter(|(i, pts)| {
                let a: [f32; ICON_PATCH * ICON_PATCH] = std::array::from_fn(|k| grey(pts[k][0], pts[k][1]));
                super::detect::icon_correlation(&a, *i) >= ICON_MATCH
            })
            .count();
        best = best.max(found);
    }
    best
}

/// What [`Sight`] is asked to look at.
#[derive(Clone, Copy)]
pub struct Ask {
    /// The left eye's pose in the play space, now, and its sides' tangents.
    pub eye: Matrix34,
    pub raw: [f32; 4],
    pub plane: Plane,
}

/// Looks at VRChat's Quick Menu's tabs in the left eye's picture, one look at a time, on its own thread with its own small reader of the headset's picture.
pub struct Sight {
    asks: std::sync::mpsc::Sender<Ask>,
    told: std::sync::mpsc::Receiver<Option<usize>>,
    pub busy: bool,
}

impl Sight {
    #[cfg(windows)]
    pub fn start(source: orbiters_openvr::compositor::MirrorSource, adapter: Option<u32>) -> Self {
        let (asks, asked) = std::sync::mpsc::channel::<Ask>();
        let (tell, told) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new().name("orbit-qm-sight".into()).spawn(move || {
            let reader = orbiters_openvr::mirror::EyeReader::new(adapter).ok();
            let mut whole: Option<[u32; 2]> = None;
            while let Ok(ask) = asked.recv() {
                let found = reader.as_ref().and_then(|reader| {
                    let size = match whole {
                        Some(s) => s,
                        None => {
                            let all = reader.read_part(source, orbiters_openvr::compositor::Eye::Left, None).ok()?;
                            *whole.insert(all.whole)
                        }
                    };
                    let points = samples(&ask.eye, ask.raw, size, &ask.plane)?;
                    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, 0.0f32, 0.0f32);
                    for p in &points {
                        (x0, y0, x1, y1) = (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1]));
                    }
                    let area = [x0 as u32, y0 as u32, (x1 as u32 + 2 - x0 as u32).min(size[0] - x0 as u32), (y1 as u32 + 2 - y0 as u32).min(size[1] - y0 as u32)];
                    let part = reader.read_part(source, orbiters_openvr::compositor::Eye::Left, Some(area)).ok()?;
                    if part.whole != size {
                        whole = Some(part.whole);
                        return None;
                    }
                    let grey = |x: f32, y: f32| {
                        let (fx, fy) = ((x - part.from[0] as f32).clamp(0.0, part.width as f32 - 1.001), (y - part.from[1] as f32).clamp(0.0, part.height as f32 - 1.001));
                        let (ix, iy) = (fx as usize, fy as usize);
                        let (ax, ay) = (fx - ix as f32, fy - iy as f32);
                        let g = |x: usize, y: usize| {
                            let i = (y * part.width as usize + x) * 4;
                            part.rgba.get(i..i + 3).map_or(0.0, |p| (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.0)
                        };
                        (g(ix, iy) * (1.0 - ax) + g(ix + 1, iy) * ax) * (1.0 - ay) + (g(ix, iy + 1) * (1.0 - ax) + g(ix + 1, iy + 1) * ax) * ay
                    };
                    Some(found_in(&points, grey))
                });
                if tell.send(found).is_err() {
                    return;
                }
            }
        });
        Self { asks, told, busy: false }
    }

    /// Looks once, unless a look is under way.
    pub fn ask(&mut self, ask: Ask) {
        if !self.busy && self.asks.send(ask).is_ok() {
            self.busy = true;
        }
    }

    /// What the last look found, once: `Some(None)` when it could not see the row (out of view, unreadable).
    pub fn take(&mut self) -> Option<Option<usize>> {
        let found = self.told.try_recv().ok()?;
        self.busy = false;
        Some(found)
    }
}

/// Why looks are due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// The Quick Menu is taken as open: is it seen there, to trust a later "not seen".
    Opened,
    /// VRChat got a click while its Quick Menu was taken as open.
    Clicked,
    /// The menu button was pressed: is what the buttons say what shows.
    Pressed,
}

/// What the looks say to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// VRChat's Quick Menu is not there: Orbiters' parts go.
    Gone,
    /// It is there though taken as closed: Orbiters' parts come.
    There,
}

/// After a click or a press, looks at these moments: VRChat's menus take about 0.2 to 0.3 s to come or go.
const LOOKS_AFTER: [u64; 4] = [300, 550, 850, 1300];
const LOOKS_AFTER_OPENING: [u64; 2] = [500, 900];

/// Decides from looks whether VRChat's Quick Menu is there, against what the buttons say.
#[derive(Debug, Default)]
pub struct QmCheck {
    due: Vec<Instant>,
    why: Option<Why>,
    /// Seen there since it was taken as open: only then is "not seen" believed.
    seen: bool,
    /// Looks in a row that found it gone, or there.
    gone: u8,
    there: u8,
}

impl QmCheck {
    fn plan(&mut self, why: Why, now: Instant, after: &[u64]) {
        self.why = Some(why);
        self.due = after.iter().map(|ms| now + Duration::from_millis(*ms)).collect();
        (self.gone, self.there) = (0, 0);
    }

    /// Why the looks under way were asked for (for the log).
    pub fn why(&self) -> Option<Why> {
        self.why
    }

    /// The buttons now take the Quick Menu as open (`true`) or not.
    pub fn taken(&mut self, open: bool, now: Instant) {
        self.seen = false;
        self.plan(if open { Why::Opened } else { Why::Pressed }, now, if open { &LOOKS_AFTER_OPENING } else { &LOOKS_AFTER });
    }

    /// The menu button was pressed and the buttons' answer is the same as before (a long press...): look as well.
    pub fn pressed(&mut self, now: Instant) {
        self.plan(Why::Pressed, now, &LOOKS_AFTER);
    }

    pub fn clicked(&mut self, now: Instant) {
        self.plan(Why::Clicked, now, &LOOKS_AFTER);
    }

    /// Whether a look is due now.
    pub fn look_now(&mut self, now: Instant) -> bool {
        let due = self.due.first().is_some_and(|at| *at <= now);
        if due {
            self.due.remove(0);
        }
        due
    }

    /// A look found `found` icons of its tabs (`None`: could not see the row). `open`: what the buttons take it as.
    pub fn found(&mut self, found: Option<usize>, open: bool) -> Option<Verdict> {
        let found = found?;
        if found >= ICONS_NEEDED {
            self.seen = true;
            self.gone = 0;
            self.there = self.there.saturating_add(1);
            if !open && self.there >= 2 {
                self.due.clear();
                return Some(Verdict::There);
            }
        } else if found <= GONE_AT_MOST {
            self.there = 0;
            self.gone = self.gone.saturating_add(1);
            if open && self.seen && self.gone >= 2 {
                self.due.clear();
                self.seen = false;
                return Some(Verdict::Gone);
            }
            // Never seen in this opening: nothing is believed. Were the row never found in some light or headset, the parts would otherwise go each time
            // they open. (VRChat's Main Menu closing on the menu button is [`super::vr::MenuButtons::main_menu_opened`]'s.)
        } else {
            // Half there (mid-animation, partly hidden): neither, and the looks in a row start again.
            (self.gone, self.there) = (0, 0);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Enzo: VRChat's Avatars button takes its Quick Menu away and Orbiters' parts stayed. Seen there, clicked, then not seen twice: gone.
    #[test]
    fn a_click_that_takes_the_quick_menu_away_takes_the_parts() {
        let t0 = Instant::now();
        let mut check = QmCheck::default();
        check.taken(true, t0);
        assert!(!check.look_now(t0));
        assert!(check.look_now(t0 + Duration::from_millis(500)));
        assert_eq!(check.found(Some(6), true), None, "seen there");
        check.clicked(t0 + Duration::from_secs(3));
        assert_eq!(check.found(Some(0), true), None, "one look is not enough");
        assert_eq!(check.found(Some(1), true), Some(Verdict::Gone));
    }

    /// A click on a tab, or anything that keeps it up, finds it still there: nothing goes. Neither does what could not be seen, nor a menu never seen there
    /// (another world's light, the row out of view): the parts then behave as before.
    #[test]
    fn what_keeps_it_up_or_cannot_be_seen_changes_nothing() {
        let t0 = Instant::now();
        let mut check = QmCheck::default();
        check.taken(true, t0);
        assert_eq!(check.found(Some(7), true), None);
        check.clicked(t0);
        for found in [Some(5), Some(4), None, None, Some(6)] {
            assert_eq!(check.found(found, true), None, "{found:?}");
        }
        // Half there (2 or 3 icons, mid-animation) is neither.
        assert_eq!(check.found(Some(2), true), None);
        assert_eq!(check.found(Some(0), true), None);
        assert_eq!(check.found(Some(3), true), None);
        assert_eq!(check.found(Some(0), true), None, "not two in a row");
        let mut never = QmCheck::default();
        never.taken(true, t0);
        never.clicked(t0);
        for _ in 0..4 {
            assert_eq!(never.found(Some(0), true), None, "never seen in this opening");
        }
    }

    /// A Quick Menu seen while taken as closed (the buttons out of step some other way) brings the parts back; one not seen while taken as open is believed
    /// gone only once it was seen in that opening.
    #[test]
    fn looks_after_the_menu_button_put_the_buttons_right() {
        let t0 = Instant::now();
        let mut check = QmCheck::default();
        check.taken(true, t0);
        for _ in 0..4 {
            assert_eq!(check.found(Some(0), true), None, "never seen in this opening: not believed");
        }
        let mut check = QmCheck::default();
        check.taken(false, t0);
        assert_eq!(check.found(Some(5), false), None);
        assert_eq!(check.found(Some(6), false), Some(Verdict::There));
    }

    /// Seen in the sandbox: once a Quick Menu button opened VRChat's Main Menu, the next short press of the menu button closes that Main Menu and opens nothing;
    /// the press after opens the Quick Menu, and its parts with it.
    #[test]
    fn the_press_after_the_main_menu_opens_nothing() {
        use super::super::vr::{MenuButtons, Open};
        let t0 = Instant::now();
        let mut buttons = MenuButtons::default();
        let mut at = t0;
        let mut press = |b: &mut MenuButtons| {
            b.step([false; 2], at);
            at += Duration::from_millis(50);
            b.step([true, false], at);
            at += Duration::from_millis(100);
            let open = b.step([false; 2], at);
            at += Duration::from_millis(50);
            open
        };
        assert_eq!(press(&mut buttons), Open::QuickMenu(0));
        buttons.main_menu_opened();
        assert_eq!(buttons.open(), Open::Closed);
        assert_eq!(press(&mut buttons), Open::Closed, "closes VRChat's Main Menu only");
        assert_eq!(press(&mut buttons), Open::QuickMenu(0));
        assert_eq!(buttons.last_hand(), Some(0));
        buttons.reset();
        buttons.seen_open(1);
        assert_eq!(buttons.open(), Open::QuickMenu(1));
    }

    /// Only VRChat's clicks: a press begun on Orbiters' parts was blocked from VRChat; the hand holding the menu points at nothing; a trigger held from before
    /// clicks nothing.
    #[test]
    fn only_a_click_vrchat_got_counts() {
        let mut watch = ClickWatch::default();
        assert!(!watch.step([false; 2], [false, true], [true; 2]), "held from before");
        assert!(!watch.step([false; 2], [false, false], [true; 2]), "held from before, let go");
        assert!(!watch.step([false; 2], [false, true], [true; 2]));
        assert!(watch.step([false; 2], [false, false], [true; 2]), "a click");
        assert!(!watch.step([false, true], [false, true], [true; 2]));
        assert!(!watch.step([false; 2], [false, false], [true; 2]), "begun on Orbiters' parts");
        assert!(!watch.step([false; 2], [true, false], [false, true]));
        assert!(!watch.step([false; 2], [false, false], [false, true]), "the menu's own hand");
    }

    /// The row sampled through the eye's projection lands where the eye sees the plane: a point straight ahead is the picture's middle sideways, and the tabs
    /// of a menu held before the eyes are all in view, those behind it are not.
    #[test]
    fn the_tabs_are_found_where_the_eye_sees_them() {
        let raw = [-1.376, 0.839, -1.376, 0.9];
        let eye = space::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.65, 0.0]);
        let p = in_eye(&eye, raw, [1620, 1800], [0.0, 1.65, -1.0]).unwrap();
        assert!((p[0] - 1620.0 * 1.376 / (1.376 + 0.839)).abs() < 0.5 && (p[1] - 1800.0 * 0.9 / (0.9 + 1.376)).abs() < 0.5, "{p:?}");
        assert!(in_eye(&eye, raw, [1620, 1800], [0.0, 1.65, 1.0]).is_none());
        let plane = Plane { frame: space::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.55, -0.4]), unit: 0.0009 };
        let points = samples(&eye, raw, [1620, 1800], &plane).expect("in view");
        assert_eq!(points.len(), SHIFTS.len() * SHIFTS.len() * 7 * ICON_PATCH * ICON_PATCH);
        let behind = Plane { frame: space::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.55, 0.4]), unit: 0.0009 };
        assert!(samples(&eye, raw, [1620, 1800], &behind).is_none());
        // A picture of the icons themselves, drawn where the plane puts them, is found; a blank one is not.
        let exact: Vec<[f32; 2]> = points.chunks_exact(7 * ICON_PATCH * ICON_PATCH).nth(SHIFTS.len() * SHIFTS.len() / 2).unwrap().to_vec();
        let painted: Vec<([f32; 2], f32)> = exact.iter().enumerate().map(|(n, p)| (*p, super::super::detect::icon_template(n / (ICON_PATCH * ICON_PATCH))[n % (ICON_PATCH * ICON_PATCH)] as f32)).collect();
        let icons = |x: f32, y: f32| painted.iter().find(|(p, _)| (p[0] - x).abs() < 0.01 && (p[1] - y).abs() < 0.01).map_or(0.0, |(_, v)| *v);
        assert_eq!(found_in(&exact, icons), 7);
        assert_eq!(found_in(&points, |_, _| 128.0), 0);
    }
}
