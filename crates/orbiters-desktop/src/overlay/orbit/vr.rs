//! The panels in VR: a SteamVR overlay each, in a plane around the wheel on the hand that opened it, or around the Quick Menu where it opened, pointed at with the
//! controllers' own lasers ([`super::super::floating::laser`]) with the pointing hand's trigger taken from VRChat ([`super::block`]).
//!
//! **What cannot be known in VR, said plainly.** VRChat tells nothing about its menus (see [`super::detect`]), and no eye picture is read here: finding a menu in one
//! could not be built without a headset to measure it on. What is known is the button: VRChat's own bindings (`VRChat_Data/StreamingAssets/SteamVR`) put its `Menu` action on
//! B (Index), Y and B (Touch and most others) and the application menu button (Vive, WMR), a short press being the Quick Menu and a held one the wheel. [`MenuButtons`] follows that and
//! nothing more, so:
//! - it can be wrong when VRChat's menu closes by another way than the button (its own close, a world change). That is why panels go by themselves after [`IDLE_OPEN`] with no laser on
//!   them, and why after that, or after their close button, the next short press counts as closing: **a menu without panels is late, panels without a menu is wrong.**
//! - where the menus are, and how large, is measured on the desktop and estimated here ([`WHEEL_AHEAD_M`], [`QM_DISTANCE_M`], [`QM_UNIT_M`], [`WHEEL_RADIUS_M`]). Enzo's first test says
//!   what to change; Settings' VR size scales them meanwhile.

use super::super::actions::Actions;
use super::super::floating::laser::{self, Lasers};
use super::super::floating::space::{self, Quad, V3};
use super::super::Presenter;
use super::block::{Block, HandNow, Stuck};
use super::zones::{self, Around, Placed, Zone};
use super::Panels;
use crate::backend::Ctl;
use crate::state::{OrbitStatus, Snapshot};
use egui::Pos2;
use orbiters_openvr::{self as vr, ControllerRole, Matrix34, Pose, Universe, MAX_DEVICES};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A press this long is the wheel's, shorter the Quick Menu's. VRChat does not publish it: to be confirmed in the headset.
pub const HOLD: Duration = Duration::from_millis(500);
/// With no laser on them for this long the panels go: a menu closed some other way must not leave them hanging.
pub const IDLE_OPEN: Duration = Duration::from_secs(90);

/// What the buttons say is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Open {
    Closed,
    /// Nothing shows, and it is not known whether VRChat's menu is open: the next short press closes it rather than shows panels.
    Unsure,
    QuickMenu,
    /// On this hand (0 left, 1 right).
    Wheel(usize),
}

/// From the menu button of each hand to what VRChat should be showing.
#[derive(Clone, Copy, Debug)]
pub struct MenuButtons {
    open: Open,
    down: [bool; 2],
    /// The press under way, on which hand and since when; it closed something, so it opens nothing.
    press: Option<(usize, Instant, bool)>,
}

impl Default for MenuButtons {
    fn default() -> Self {
        // Buttons taken as held: one already down when this starts presses nothing.
        Self { open: Open::Closed, down: [true; 2], press: None }
    }
}

impl MenuButtons {
    pub fn open(&self) -> Open {
        self.open
    }

    /// The panels go without VRChat having been told anything: the person's close, nobody pointing at them for long.
    pub fn unsure(&mut self) {
        if matches!(self.open, Open::QuickMenu | Open::Wheel(_)) {
            self.open = Open::Unsure;
        }
    }

    /// VRChat is not where a menu can be (a world change, the dashboard, it closed): nothing is open.
    pub fn reset(&mut self) {
        self.open = Open::Closed;
        self.press = None;
    }

    pub fn step(&mut self, down: [bool; 2], now: Instant) -> Open {
        for hand in 0..2 {
            if down[hand] && !self.down[hand] && self.press.is_none() {
                // Open: this press closes it, at once, and opens nothing.
                let closing = self.open != Open::Closed;
                if matches!(self.open, Open::QuickMenu | Open::Wheel(_)) {
                    self.open = Open::Closed;
                }
                self.press = Some((hand, now, closing));
            }
        }
        if let Some((hand, since, closing)) = self.press {
            let held = now.duration_since(since);
            if down[hand] {
                if held >= HOLD && !closing && self.open == Open::Closed {
                    self.open = Open::Wheel(hand);
                }
            } else {
                if closing {
                    self.open = Open::Closed;
                } else if held < HOLD {
                    self.open = Open::QuickMenu;
                }
                self.press = None;
            }
        }
        self.down = down;
        self.open
    }
}

/// The wheel's radius in the headset. Measured nowhere yet: to be set from Enzo's first test.
pub const WHEEL_RADIUS_M: f32 = 0.09;
/// How far ahead of the controller's grip the wheel's centre is, along where it points.
pub const WHEEL_AHEAD_M: f32 = 0.06;
/// The Quick Menu: how far before the eyes it opens, how far below them, and what one of its units is (its main panel is 376 wide).
pub const QM_DISTANCE_M: f32 = 0.75;
pub const QM_BELOW_M: f32 = 0.1;
pub const QM_UNIT_M: f32 = 0.0015;
/// Panels hang this little in front of the plane of what they are around, the takeover in front of them.
const NEAR_M: f32 = 0.004;
const TAKEOVER_NEAR_M: f32 = 0.02;
/// Overlays' order: over N4N0's menu, under the lasers.
const SORT_ORDER: u32 = 15;

/// A plane the panels sit on, its +Z facing the eyes, and what one unit is there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub frame: Matrix34,
    pub unit: f32,
}

impl Plane {
    /// Where a panel's quad is, at `back` units toward the plane's centre (its spring).
    pub fn quad(&self, placed: &Placed, back: f32, near: f32) -> Quad {
        let c = placed.rect.center();
        let way = placed.zone.ring().map_or_else(
            || {
                let length = (c.x * c.x + c.y * c.y).sqrt().max(1.0);
                egui::vec2(c.x / length, c.y / length)
            },
            |ring| zones::polar(1.0, ring.middle()),
        );
        let (x, y) = (c.x - way.x * back, c.y - way.y * back);
        Quad { transform: space::offset(&self.frame, [x * self.unit, -y * self.unit, near]), width: placed.rect.width() * self.unit, height: placed.rect.height() * self.unit }
    }
}

/// The wheel on a hand, facing the eyes, upright.
pub fn wheel_plane(grip: &Matrix34, head: &Matrix34, size: f32) -> Option<Plane> {
    let ahead = space::scale(space::axis(grip, 2), -WHEEL_AHEAD_M);
    let at = space::add(space::position(grip), ahead);
    Some(Plane { frame: space::facing(at, space::position(head), [0.0, 1.0, 0.0])?, unit: WHEEL_RADIUS_M * size / 100.0 })
}

/// The Quick Menu as it opens before the eyes.
pub fn quick_menu_plane(head: &Matrix34, size: f32) -> Plane {
    Plane { frame: space::before_eyes(head, QM_DISTANCE_M, QM_BELOW_M), unit: QM_UNIT_M * size }
}

/// One hand's laser on the panels: which, where in its units and its hit. The nearest wins, the wheel's bite is not on them.
fn laser_on(quads: &[(Quad, Placed)], origin: V3, direction: V3) -> Option<(usize, [f32; 2], space::Hit)> {
    let mut best: Option<(usize, [f32; 2], space::Hit)> = None;
    for (i, (quad, placed)) in quads.iter().enumerate() {
        let Some(hit) = quad.hit(origin, direction, 0.0) else { continue };
        let at = placed.rect.min + egui::vec2(hit.uv[0], hit.uv[1]) * placed.rect.size();
        if placed.contains(at) && best.is_none_or(|(_, _, b)| hit.distance < b.distance) {
            best = Some((i, hit.uv, hit));
        }
    }
    best
}

pub struct Cx<'a, 'o> {
    pub overlay: &'a vr::Overlay<'o>,
    pub steamvr: &'a vr::Client,
    pub(in crate::overlay) presenter: &'a mut Presenter,
    pub snap: &'a Arc<Snapshot>,
    pub backend: &'a Ctl,
    pub actions: &'a mut Actions,
    pub input_read: bool,
    pub dashboard: bool,
    /// VRChat has the scene.
    pub in_game: bool,
    pub prediction: f32,
}

/// What a turn asks of the overlay's loop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Turn {
    /// A press happened (for the click's sound).
    pub clicked: bool,
    /// Worlds in 3D was pressed: the panel with its 3D view lives with N4N0's menu here.
    pub worlds: bool,
}

pub struct OrbitVr {
    key: String,
    panels: Panels,
    buttons: MenuButtons,
    block: Block,
    stuck: Stuck,
    lasers: Lasers,
    handles: HashMap<Zone, vr::OverlayHandle>,
    /// Where the Quick Menu opened; where a page opened from the wheel goes.
    plane: Option<Plane>,
    takeover_plane: Option<Plane>,
    trigger_was: [bool; 2],
    pointing_since: Instant,
    last_turn: Instant,
    instance: Option<String>,
    told: Option<OrbitStatus>,
    overrides_asked: bool,
    /// The hand whose laser moves the pointer.
    active: Option<usize>,
    last_shown: bool,
}

impl OrbitVr {
    pub fn new(overlay: &vr::Overlay, key: &str, backend: Ctl) -> anyhow::Result<Self> {
        let now = Instant::now();
        Ok(Self {
            key: format!("{key}.orbit"),
            panels: Panels::new(backend, 2.0, false),
            buttons: MenuButtons::default(),
            block: Block::default(),
            stuck: Stuck::start(),
            lasers: Lasers::new(overlay, &format!("{key}.orbit"))?,
            handles: HashMap::new(),
            plane: None,
            takeover_plane: None,
            trigger_was: [true; 2],
            pointing_since: now,
            last_turn: now,
            instance: None,
            told: None,
            overrides_asked: false,
            active: None,
            last_shown: false,
        })
    }

    pub fn destroy(&self, overlay: &vr::Overlay) {
        for handle in self.handles.values() {
            let _ = overlay.destroy(*handle);
        }
        self.lasers.destroy(overlay);
    }

    /// Says to Settings whether the block can work. SteamVR's own setting is turned on here once, as the panels are wanted; that it can be turned off again in SteamVR is the person's.
    fn overrides(&mut self, cx: &Cx<'_, '_>) {
        let mut on = cx.steamvr.setting_bool(vr::STEAMVR_SECTION, vr::GLOBAL_PRIORITY_KEY);
        if on != Some(true) && !self.overrides_asked {
            self.overrides_asked = true;
            if cx.steamvr.set_setting_bool(vr::STEAMVR_SECTION, vr::GLOBAL_PRIORITY_KEY, true) {
                on = cx.steamvr.setting_bool(vr::STEAMVR_SECTION, vr::GLOBAL_PRIORITY_KEY);
            }
        }
        let status = OrbitStatus {
            note: (on != Some(true)).then(|| "SteamVR's \"Experimental overlay input overrides\" (Settings, Developer) is off: pointing at a panel still presses in VRChat too.".to_string()),
            steamvr_overrides: on,
        };
        if self.told.as_ref() != Some(&status) {
            cx.backend.orbit_status(status.clone());
            self.told = Some(status);
        }
    }

    /// A turn of the overlay's loop.
    pub fn update(&mut self, mut cx: Cx<'_, '_>) -> Turn {
        let now = Instant::now();
        let dt = now.duration_since(self.last_turn).as_secs_f32();
        self.last_turn = now;
        let settings = cx.snap.settings.orbit;
        let wanted = settings.any() && cx.snap.vrchat.running;
        if !wanted || !cx.in_game || cx.dashboard {
            // Free at once, whatever is held.
            if self.last_shown || self.block.blocked() != [false; 2] {
                self.close(&mut cx);
            }
            self.buttons.reset();
            self.stuck.beat(false);
            return Turn::default();
        }
        if self.told.is_none() || !self.overrides_asked {
            self.overrides(&cx);
        }
        // A new instance: whatever was open went with the last.
        let instance = cx.snap.vrchat.world.instance.clone();
        if instance != self.instance {
            self.instance = instance;
            self.buttons.reset();
        }
        let down = if cx.input_read { cx.actions.orbit_menu_buttons(cx.steamvr) } else { [false; 2] };
        let open = if cx.input_read { self.buttons.step(down, now) } else { self.buttons.open() };
        let menu = match open {
            Open::QuickMenu if settings.around_quick_menu() => Some(Around::QuickMenu),
            Open::Wheel(_) if settings.around_wheel() => Some(Around::Wheel),
            _ => None,
        };
        if menu.is_none() && self.panels.is_empty() {
            if self.last_shown {
                self.close(&mut cx);
            }
            self.stuck.beat(false);
            return Turn::default();
        }
        let mut poses = [Pose::default(); MAX_DEVICES];
        cx.steamvr.predicted_poses(Universe::Standing, cx.prediction, &mut poses);
        let head = poses[0].pose_is_valid.then_some(poses[0].device_to_absolute);
        let device = |role| cx.steamvr.controller_index(role).filter(|&d| (d as usize) < MAX_DEVICES && poses[d as usize].pose_is_valid);
        let devices = [device(ControllerRole::LeftHand), device(ControllerRole::RightHand)];
        let Some(head) = head else {
            // Lost the headset's pose: nothing to place by, nothing shown.
            self.close(&mut cx);
            self.stuck.beat(false);
            return Turn::default();
        };
        // The planes: the wheel rides its hand, the Quick Menu stays where it opened.
        if self.panels.open_menu() != menu {
            self.plane = None;
            self.takeover_plane = None;
            self.pointing_since = now;
        }
        let plane = match (menu, open) {
            (Some(Around::Wheel), Open::Wheel(hand)) => devices[hand].and_then(|d| wheel_plane(&poses[d as usize].device_to_absolute, &head, settings.vr_size)),
            (Some(Around::QuickMenu), _) => Some(*self.plane.get_or_insert_with(|| quick_menu_plane(&head, settings.vr_size))),
            _ => None,
        };
        self.plane = plane;
        let mut places: Vec<Placed> = menu.map_or_else(Vec::new, |m| Zone::around(m).iter().map(|z| zones::in_units(*z)).collect());
        // Here VRChat's wheel can close unseen: the ring has its own close.
        if menu == Some(Around::Wheel) && !self.panels.sheet_open() {
            places.push(zones::in_units(Zone::WheelHide));
        }
        self.panels.takeover_place = Some(zones::in_units(Zone::Takeover));
        self.panels.back_place = Some(zones::in_units(Zone::SheetBack));
        self.panels.set_open(menu, places, cx.snap);
        if self.panels.is_empty() {
            self.close(&mut cx);
            self.stuck.beat(false);
            return Turn::default();
        }
        self.panels.follow(cx.snap, cx.backend.library_version());
        self.panels.menu_turn(cx.snap);
        self.panels.step_springs(dt);

        // Where everything is this turn.
        let Some(plane) = plane.or(self.plane) else {
            self.stuck.beat(false);
            return Turn::default();
        };
        if self.panels.takeover.is_some() && self.takeover_plane.is_none() {
            self.takeover_plane = Some(if menu == Some(Around::QuickMenu) { plane } else { quick_menu_plane(&head, settings.vr_size) });
        }
        if self.panels.takeover.is_none() {
            self.takeover_plane = None;
        }
        let quads: Vec<(Quad, Placed)> = self
            .panels
            .all()
            .map(|p| {
                let sheet = matches!(p.zone, Zone::Takeover | Zone::SheetBack);
                let on = if sheet { self.takeover_plane.unwrap_or(plane) } else { plane };
                let near = match p.zone {
                    Zone::Takeover => TAKEOVER_NEAR_M,
                    Zone::SheetBack => TAKEOVER_NEAR_M - 0.004,
                    _ => NEAR_M,
                };
                (on.quad(&p.placed, -p.outward(), near), p.placed)
            })
            .collect();

        // Who points at what.
        let lasers = if cx.input_read { cx.actions.lasers(cx.steamvr, cx.prediction) } else { [None; 2] };
        let triggers = cx.actions.orbit_triggers(cx.steamvr, self.block.blocked());
        let hits: [Option<(usize, [f32; 2], space::Hit)>; 2] = std::array::from_fn(|i| {
            // The wheel's own hand points at nothing: it holds the wheel.
            if matches!(open, Open::Wheel(h) if h == i) {
                return None;
            }
            lasers[i].and_then(|tip| {
                let (o, d) = space::ray(&tip);
                let shown: Vec<(Quad, Placed)> = quads.iter().zip(self.panels.all()).map(|(q, p)| if p.shown { *q } else { (Quad { width: 0.0, height: 0.0, ..q.0 }, q.1) }).collect();
                laser_on(&shown, o, d)
            })
        });
        let hands: [HandNow; 2] = std::array::from_fn(|i| HandNow { on_panel: hits[i].is_some(), trigger: triggers[i] });
        let blocked_before = self.block.blocked();
        let blocked = self.block.step(hands, true);
        cx.actions.set_block(blocked);
        self.stuck.beat(blocked != [false; 2]);
        if hits.iter().any(Option::is_some) {
            self.pointing_since = now;
        }
        if now.duration_since(self.pointing_since) >= IDLE_OPEN {
            self.buttons.unsure();
        }
        // The pointer: the hand that pressed stays; else the right, else the left.
        if self.active.is_none_or(|h| hits[h].is_none() && !triggers[h]) {
            self.active = [1, 0].into_iter().find(|&i| hits[i].is_some());
        }
        let pointer = self.active.and_then(|h| hits[h]).map(|(panel, uv, _)| {
            let logical: Pos2 = self.panels.all().nth(panel).map_or(Pos2::ZERO, |p| p.to_logical(uv));
            (panel, logical)
        });
        self.panels.hover(pointer);
        let mut turn = Turn::default();
        if let Some(h) = self.active {
            let (down, was) = (triggers[h], self.trigger_was[h]);
            // Only a press that began blocked is the panels'.
            if down && !was && blocked_before[h] {
                let pressed = self.panels.press(cx.snap);
                turn.clicked |= pressed.clicked;
                turn.worlds |= pressed.worlds;
                if pressed.hide {
                    self.buttons.unsure();
                }
            } else if !down && was {
                self.panels.release();
            }
        }
        self.trigger_was = triggers;

        // Beams, from the tip to where they end.
        for hand in 0..2 {
            let beam = hits[hand].zip(lasers[hand]).zip(devices[hand]).map(|(((panel, _, hit), tip), device)| laser::Laser {
                device,
                grip: poses[device as usize].device_to_absolute,
                tip,
                eyes: space::position(&head),
                end: hit.point,
                surface: quads[panel].0.transform,
                pressed: triggers[hand],
            });
            self.lasers.update(cx.overlay, cx.presenter, hand, beam);
        }

        // Pictures, then each overlay where it goes.
        self.show(&mut cx, &quads);
        self.last_shown = true;
        turn
    }

    fn show(&mut self, cx: &mut Cx<'_, '_>, quads: &[(Quad, Placed)]) {
        let (overlay, presenter) = (cx.overlay, &mut *cx.presenter);
        let (handles, key) = (&mut self.handles, &self.key);
        self.panels.paint(cx.snap, |panel, changed| {
            let handle = *handles.entry(panel.zone).or_insert_with(|| {
                let made = overlay.create(&format!("{key}.{:?}", panel.zone), &format!("Orbiters {:?}", panel.zone)).unwrap_or(0);
                let _ = overlay.set_flag(made, vr::overlay_flag::IS_PREMULTIPLIED, true);
                let _ = overlay.set_sort_order(made, SORT_ORDER + u32::from(panel.zone == Zone::Takeover));
                made
            });
            if handle != 0 {
                presenter.show_part(overlay, handle, panel.pixels(), panel.px[0], panel.px[1], changed);
            }
        });
        let shown: Vec<Zone> = self.panels.all().map(|p| p.zone).collect();
        for ((quad, _), panel) in quads.iter().zip(self.panels.all()) {
            if let Some(handle) = self.handles.get(&panel.zone) {
                if !panel.painted || !panel.shown {
                    let _ = cx.overlay.hide(*handle);
                    continue;
                }
                let _ = cx.overlay.set_width_in_meters(*handle, quad.width);
                let _ = cx.overlay.set_transform_absolute(*handle, &quad.transform);
                let _ = cx.overlay.set_alpha(*handle, panel.appear.alpha());
                if !cx.overlay.is_visible(*handle) {
                    let _ = cx.overlay.show(*handle);
                }
            }
        }
        for (zone, handle) in &self.handles {
            if !shown.contains(zone) && cx.overlay.is_visible(*handle) {
                let _ = cx.overlay.hide(*handle);
            }
        }
    }

    /// Everything down and the trigger free, at once.
    fn close(&mut self, cx: &mut Cx<'_, '_>) {
        self.block.release();
        cx.actions.set_block([false; 2]);
        self.panels.set_open(None, Vec::new(), cx.snap);
        self.panels.menu_turn(cx.snap);
        for handle in self.handles.values() {
            if cx.overlay.is_visible(*handle) {
                let _ = cx.overlay.hide(*handle);
            }
        }
        for hand in 0..2 {
            self.lasers.update(cx.overlay, cx.presenter, hand, None);
        }
        self.plane = None;
        self.takeover_plane = None;
        self.active = None;
        self.trigger_was = [true; 2];
        self.last_shown = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_core::settings::OrbitSettings;

    fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    /// Runs presses (hand, from, to) in ms and says what is open at the end.
    fn run(buttons: &mut MenuButtons, t0: Instant, presses: &[(usize, u64, u64)], until: u64) -> Open {
        let mut open = buttons.open();
        for ms in (0..=until).step_by(11) {
            let down = std::array::from_fn(|hand| presses.iter().any(|&(h, a, b)| h == hand && (a..b).contains(&ms)));
            open = buttons.step(down, at(t0, ms));
        }
        open
    }

    fn fresh(t0: Instant) -> MenuButtons {
        let mut b = MenuButtons::default();
        b.step([false; 2], t0);
        b
    }

    #[test]
    fn a_short_press_is_the_quick_menu_and_a_long_one_the_wheel_on_that_hand() {
        let t0 = Instant::now();
        assert_eq!(run(&mut fresh(t0), t0, &[(1, 100, 250)], 400), Open::QuickMenu);
        assert_eq!(run(&mut fresh(t0), t0, &[(0, 100, 900)], 700), Open::Wheel(0), "while still held");
        assert_eq!(run(&mut fresh(t0), t0, &[(1, 100, 900)], 1000), Open::Wheel(1), "and after it");
    }

    #[test]
    fn a_press_while_open_closes_at_once_and_opens_nothing() {
        let t0 = Instant::now();
        let mut b = fresh(t0);
        run(&mut b, t0, &[(1, 100, 200)], 300);
        assert_eq!(b.step([true, false], at(t0, 1000)), Open::Closed, "goes as the button goes down");
        assert_eq!(b.step([false, false], at(t0, 1100)), Open::Closed);
        // Held long, it still opens nothing: it was a closing.
        let mut b = fresh(t0);
        run(&mut b, t0, &[(0, 100, 900)], 1000);
        assert_eq!(b.step([true, false], at(t0, 2000)), Open::Closed);
        assert_eq!(b.step([true, false], at(t0, 3000)), Open::Closed);
        assert_eq!(b.step([false, false], at(t0, 3100)), Open::Closed);
    }

    #[test]
    fn a_button_already_held_when_this_starts_presses_nothing() {
        let t0 = Instant::now();
        let mut b = MenuButtons::default();
        assert_eq!(b.step([true, false], t0), Open::Closed);
        assert_eq!(b.step([true, false], at(t0, 2000)), Open::Closed);
        assert_eq!(b.step([false, false], at(t0, 2100)), Open::Closed);
    }

    #[test]
    fn after_panels_went_on_their_own_the_next_press_closes_instead_of_showing_them() {
        let t0 = Instant::now();
        let mut b = fresh(t0);
        run(&mut b, t0, &[(1, 100, 200)], 300);
        b.unsure();
        assert_eq!(b.open(), Open::Unsure);
        assert_eq!(b.step([false, true], at(t0, 1000)), Open::Unsure);
        assert_eq!(b.step([false, false], at(t0, 1100)), Open::Closed, "no panels for this press");
        assert_eq!(b.step([false, true], at(t0, 2000)), Open::Closed);
        assert_eq!(b.step([false, false], at(t0, 2100)), Open::QuickMenu, "from now on as ever");
    }

    #[test]
    fn a_world_change_closes_it() {
        let t0 = Instant::now();
        let mut b = fresh(t0);
        run(&mut b, t0, &[(1, 100, 200)], 300);
        b.reset();
        assert_eq!(b.open(), Open::Closed);
    }

    #[test]
    fn panels_lie_around_the_wheel_facing_the_eyes_and_follow_the_hand() {
        let head = space::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.6, 0.0]);
        let grip = space::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.3, 1.2, -0.4]);
        let plane = wheel_plane(&grip, &head, 1.0).unwrap();
        let eyes = space::position(&head);
        assert!(space::dot(space::axis(&plane.frame, 2), space::sub(eyes, space::position(&plane.frame))) > 0.0, "it faces the eyes");
        assert!(space::axis(&plane.frame, 1)[1] > 0.5, "upright");
        let top = plane.quad(&zones::in_units(Zone::WheelWorlds), 0.0, NEAR_M);
        let center = space::position(&plane.frame);
        assert!(space::position(&top.transform)[1] > center[1], "the top cell above the wheel's centre");
        // It keeps its share of the wheel.
        assert!((top.width / WHEEL_RADIUS_M - zones::Zone::WheelWorlds.units().width() / 100.0).abs() < 0.01);
        // Larger in Settings, larger here.
        assert!((wheel_plane(&grip, &head, 1.25).unwrap().quad(&zones::in_units(Zone::WheelWorlds), 0.0, 0.0).width / top.width - 1.25).abs() < 0.01);
        assert!(OrbitSettings::VR_SIZES.contains(&1.25));
    }

    #[test]
    fn a_laser_finds_the_nearest_panel_and_only_on_its_shape() {
        let head = space::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.6, 0.0]);
        let plane = quick_menu_plane(&head, 1.0);
        let placed = zones::in_units(Zone::WheelPlaying);
        let quad = plane.quad(&placed, 0.0, 0.0);
        let quads = [(quad, placed)];
        let eyes = space::position(&head);
        let shoot = |uv: [f32; 2]| {
            let target = space::apply(&quad.transform, [(uv[0] - 0.5) * quad.width, (0.5 - uv[1]) * quad.height, 0.0]);
            laser_on(&quads, eyes, space::unit(space::sub(target, eyes)).unwrap())
        };
        // Its middle is on it; of its rectangle's corners, one at least is off its arc.
        let anchor = placed.zone.ring().unwrap().anchor().to_pos2();
        let uv = (anchor - placed.rect.min) / placed.rect.size();
        assert!(shoot([uv.x, uv.y]).is_some());
        assert!([[0.02, 0.02], [0.98, 0.02], [0.02, 0.98], [0.98, 0.98]].iter().any(|c| shoot(*c).is_none()));
        assert!(shoot([1.2, 0.5]).is_none());
        // Whatever is nearer in front wins: the takeover over the panels.
        let front = zones::in_units(Zone::Takeover);
        let under = zones::in_units(Zone::QmWing);
        let back = zones::in_units(Zone::SheetBack);
        let quads = [(plane.quad(&under, 0.0, 0.0), under), (plane.quad(&back, 0.0, TAKEOVER_NEAR_M - 0.004), back), (plane.quad(&front, 0.0, TAKEOVER_NEAR_M), front)];
        let target = space::position(&quads[2].0.transform);
        assert_eq!(laser_on(&quads, eyes, space::unit(space::sub(target, eyes)).unwrap()).map(|h| h.0), Some(2));
    }
}
