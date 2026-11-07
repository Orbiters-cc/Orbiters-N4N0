//! The panels in VR: a SteamVR overlay each, on a plane riding the hand that opened VRChat's menu (Enzo, on 2026-11-07: its wheel "follows the rotation of the hand", and its Quick Menu "is
//! attached to the hand that opened it, the same way"), pointed at with the other hand's laser ([`super::super::floating::laser`]) with the pointing hand's trigger taken from VRChat
//! ([`super::block`]).
//!
//! **What cannot be known in VR, said plainly.** VRChat tells nothing about its menus (see [`super::detect`]), and no eye picture is read here. What is known is the button: VRChat's own
//! bindings (`VRChat_Data/StreamingAssets/SteamVR`) put its `Menu` action on B (Index), Y and B (Touch and most others) and the application menu button (Vive, WMR), a short press being the
//! Quick Menu and a held one the wheel. [`MenuButtons`] follows that and nothing more, so:
//! - only one menu is open at a time, and each thing known to close one closes it here too (Enzo, 2026-11-07: "walking, opening another menu"): the Quick Menu goes with the left stick,
//!   the other menu opening takes the first's place. Anything else that closes one leaves these out of step, as nothing says so: **the parts' close button says nothing is open** and puts that
//!   right at once, and so does a world change, or a long time with no press and no laser on them.
//! - where the menus are on the hand, and how large, can only be measured in a headset: [`Tune`] holds it, set by grabbing the parts and laying them on VRChat's menu ([`Tune::align`]).

use super::super::actions::Actions;
use super::super::floating::laser::{self, Lasers};
use super::super::floating::space::{self, Quad, V3};
use super::super::Presenter;
use super::block::{Block, HandNow, Stuck};
use super::zones::{self, Around, Placed, Zone};
use orbiters_core::settings::VrMotion;
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
/// With no press and no laser on them for this long the panels go, and nothing is taken to be open.
pub const IDLE_OPEN: Duration = Duration::from_secs(300);

/// How far the left stick goes before the avatar walks, and for how long, as a thumb resting on it does not.
const WALKS_AT: f32 = 0.5;
const WALKS_AFTER: Duration = Duration::from_millis(200);

/// What the buttons say shows, on which hand (0 left, 1 right).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Open {
    Closed,
    QuickMenu(usize),
    Wheel(usize),
}

/// From the menu button of each hand to what VRChat should be showing: one menu at a time, as VRChat has them.
#[derive(Clone, Copy, Debug)]
pub struct MenuButtons {
    open: Open,
    down: [bool; 2],
    /// The press under way: on which hand, since when, and whether it began on that hand's open wheel.
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

    /// Nothing is open: the person said so with the parts' close, or VRChat is not where a menu can be (a world change, the dashboard, it closed), or nothing happened for a long time.
    pub fn reset(&mut self) {
        self.open = Open::Closed;
        self.press = None;
    }

    /// The avatar walks: VRChat closes its Quick Menu then, and not its wheel, whose hand's stick picks its slices.
    pub fn walking(&mut self) {
        if matches!(self.open, Open::QuickMenu(_)) {
            self.open = Open::Closed;
        }
    }

    pub fn step(&mut self, down: [bool; 2], now: Instant) -> Open {
        for hand in 0..2 {
            if down[hand] && !self.down[hand] && self.press.is_none() {
                self.press = Some((hand, now, self.open == Open::Wheel(hand)));
            }
        }
        if let Some((hand, since, on_wheel)) = self.press {
            let held = now.duration_since(since);
            if down[hand] {
                // The wheel takes the place of what was open.
                if held >= HOLD && !on_wheel {
                    self.open = Open::Wheel(hand);
                }
            } else {
                if on_wheel {
                    self.open = Open::Closed;
                } else if held < HOLD {
                    // As VRChat must, this waits for the release: only then is it known not to be the wheel's. A wheel on the other hand gives way to it, and so does a Quick Menu
                    // on the other hand (Enzo: "if the left quick menu is open, opening the right quick menu closes the left").
                    self.open = if self.open == Open::QuickMenu(hand) { Open::Closed } else { Open::QuickMenu(hand) };
                }
                self.press = None;
            }
        }
        self.down = down;
        self.open
    }
}

/// A pose on a hand, as saved: three rows of a matrix.
pub type Saved = [f32; 12];

fn to_saved(m: &Matrix34) -> Saved {
    std::array::from_fn(|i| m.m[i / 4][i % 4])
}

fn from_saved(saved: &Saved) -> Matrix34 {
    let mut m = Matrix34::IDENTITY;
    for (i, v) in saved.iter().enumerate() {
        m.m[i / 4][i % 4] = *v;
    }
    m
}

/// A pose on one hand as it lies on the other: the two hands mirror each other across the controller's own middle.
fn mirrored(on_hand: &Matrix34) -> Matrix34 {
    let flip = |v: [f32; 3]| [-v[0], v[1], v[2]];
    let (y, z) = (flip(space::axis(on_hand, 1)), flip(space::axis(on_hand, 2)));
    space::from_axes(space::cross(y, z), y, z, flip(space::position(on_hand)))
}

/// The wheel on the right hand and the Quick Menu on each, as Enzo laid the parts on VRChat's own in his headset on 2026-11-07. The wheel lies flat on the controller's top, its first slice
/// toward where it points, and the left hand's is its mirror image. Measured to the plane's middle, so the right hand's Quick Menu is taken back by the 886 units (0.797 m) he had moved it so that
/// the wing lies on its left, which [`quick_menu_places`] does: it then comes within 1 cm of the left hand's mirror image.
const WHEEL_RIGHT: Saved = [1.000_019, -0.001_379, -0.004_553, 0.000_158, 0.004_526, -0.011_102, 0.999_986, -0.017_823, -0.001_471, -0.999_988, -0.011_089, -0.062_023];
const QM_LEFT: Saved = [0.971_491, -0.236_440, -0.017_445, 0.158_526, -0.023_072, -0.167_546, 0.985_601, -0.027_001, -0.235_958, -0.957_100, -0.168_225, -0.126_699];
const QM_RIGHT: Saved = [0.969_537, 0.244_993, 0.003_284, -0.157_4, 0.034_260, -0.148_824, 0.988_287, -0.034_1, 0.242_606, -0.958_053, -0.152_679, -0.128_4];

/// Where VRChat's menus are on the hand, and how large: VRChat says nowhere, and only a headset can show it. `orbit-vr-tune.json` in the app's data folder can change any of it, read live. With
/// `align` on in it, the hand not holding the menu grabs the parts by its grip and lays them on VRChat's menu: letting go saves that, for that hand and, mirrored, for the other until it has its
/// own. Holding the other grip too, the distance between the hands scales them. It is how the values below were measured, and how to measure them again if VRChat moves its menus.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Tune {
    /// The parts can be grabbed and laid on VRChat's menu.
    pub align: bool,
    /// As laid there, on the left hand and on the right.
    pub wheel_on: [Option<Saved>; 2],
    pub qm_on: [Option<Saved>; 2],
    /// The estimates, until then. How far the plane leans back from the top of the controller toward the wrist, in degrees: 0 lies flat on it, 90 stands across it.
    pub wheel_tilt: f32,
    /// How far from the grip, along where the controller points and along its top, in metres.
    pub wheel_ahead: f32,
    pub wheel_up: f32,
    pub wheel_radius: f32,
    pub qm_tilt: f32,
    pub qm_ahead: f32,
    pub qm_up: f32,
    /// What one of the Quick Menu's units is, in metres (its main panel is 376 wide).
    pub qm_unit: f32,
}

impl Default for Tune {
    fn default() -> Self {
        Self { align: false, wheel_on: [None, Some(WHEEL_RIGHT)], qm_on: [Some(QM_LEFT), Some(QM_RIGHT)], wheel_tilt: 0.0, wheel_ahead: 0.062, wheel_up: -0.018, wheel_radius: 0.101_8, qm_tilt: 10.0, qm_ahead: 0.127, qm_up: -0.03, qm_unit: 0.0009 }
    }
}

impl Tune {
    fn file() -> std::path::PathBuf {
        orbiters_core::paths::local_data_root().join("orbit-vr-tune.json")
    }

    fn read() -> Option<Self> {
        match serde_json::from_str(&std::fs::read_to_string(Self::file()).ok()?) {
            Ok(tune) => Some(tune),
            Err(e) => {
                eprintln!("{}: {e}", Self::file().display());
                None
            }
        }
    }

    fn write(&self) {
        let written = serde_json::to_string_pretty(self).map_err(|e| e.to_string()).and_then(|text| std::fs::write(Self::file(), text).map_err(|e| e.to_string()));
        if let Err(e) = written {
            eprintln!("{}: {e}", Self::file().display());
        }
    }

    /// The wheel's plane on this hand, from its grip.
    pub fn wheel(&self, hand: usize) -> Matrix34 {
        on_this_hand(&self.wheel_on, hand).unwrap_or_else(|| on_hand(self.wheel_tilt, self.wheel_ahead, self.wheel_up))
    }

    /// The Quick Menu's plane on this hand, from its grip.
    pub fn quick_menu(&self, hand: usize) -> Matrix34 {
        on_this_hand(&self.qm_on, hand).unwrap_or_else(|| on_hand(self.qm_tilt, self.qm_ahead, self.qm_up))
    }
}

/// What was laid on this hand, else on the other, mirrored.
fn on_this_hand(saved: &[Option<Saved>; 2], hand: usize) -> Option<Matrix34> {
    saved[hand].as_ref().map(from_saved).or_else(|| saved[1 - hand].as_ref().map(|other| mirrored(&from_saved(other))))
}

/// A plane on a controller's own axes, leaning `tilt` degrees from its top toward its wrist, `ahead` along where it points and `up` along its top. Its +Z faces who holds it.
fn on_hand(tilt: f32, ahead: f32, up: f32) -> Matrix34 {
    let (sin, cos) = tilt.to_radians().sin_cos();
    space::from_axes([1.0, 0.0, 0.0], [0.0, sin, -cos], [0.0, cos, sin], [0.0, up, -ahead])
}

/// The grip that would hold a plane lying `on_hand` as it is, for the pictures of a hand held where a person looks at its menu.
pub fn grip_holding(frame: &Matrix34, on_hand: &Matrix34) -> Matrix34 {
    space::compose(frame, &space::inverse(on_hand))
}

/// One grip's hold on the parts while they are laid on VRChat's menu.
#[derive(Clone, Copy, Debug)]
struct Aligning {
    /// The plane as held by the grabbing hand.
    held: Matrix34,
    /// With the other grip too: how far apart the hands were, and the size then.
    scaling: Option<(f32, f32)>,
}

/// A tile on its spring: where it is and how fast it goes, and how it is turned. Enzo: "a bit of a spring effect as it moves".
#[derive(Clone, Copy, Debug)]
pub struct Jelly {
    at: V3,
    speed: V3,
    axes: [V3; 3],
}

/// How far a tile can trail, in metres: a hand flung about does not leave the menu behind.
const JELLY_REACH_M: f32 = 0.05;

impl Jelly {
    pub fn at(target: &Matrix34) -> Self {
        Self { at: space::position(target), speed: [0.0; 3], axes: [space::axis(target, 0), space::axis(target, 1), space::axis(target, 2)] }
    }

    /// `dt` further toward `target`, `soft` from 0 (near the hand, firm) to 1 (far out on it, looser), `own` from -1 to 1 being this tile's own character: no two alike, so that they seem
    /// independent (Enzo). Where it is now.
    pub fn step(&mut self, target: &Matrix34, soft: f32, own: f32, dt: f32) -> Matrix34 {
        // A little bounce, as the Quick Menu's own parts have when they appear: past the mark once, and settled in a fraction of a second.
        let stiff = (520.0 - 240.0 * soft.clamp(0.0, 1.0)) * (1.0 + 0.4 * own);
        let damp = 2.0 * (0.5 - 0.1 * own) * stiff.sqrt();
        let goal = space::position(target);
        let mut left = dt.min(0.05);
        while left > 0.0 {
            let h = left.min(0.004);
            let off = space::sub(goal, self.at);
            self.speed = space::add(self.speed, space::scale(space::sub(space::scale(off, stiff), space::scale(self.speed, damp)), h));
            self.at = space::add(self.at, space::scale(self.speed, h));
            left -= h;
        }
        let off = space::sub(self.at, goal);
        let far = space::length(off);
        if far > JELLY_REACH_M {
            self.at = space::add(goal, space::scale(off, JELLY_REACH_M / far));
        }
        // It turns toward the hand's turn the same way, without the bounce.
        let turn = 1.0 - (-(30.0 - 12.0 * soft) * dt).exp();
        for (i, axis) in self.axes.iter_mut().enumerate() {
            *axis = space::add(*axis, space::scale(space::sub(space::axis(target, i), *axis), turn));
        }
        let frame = space::from_axes(self.axes[0], self.axes[1], self.axes[2], self.at);
        space::orthonormal(&frame).unwrap_or(*target)
    }
}

/// A tile's own character, from -1 to 1: the same each time, different from its neighbours'.
fn character(zone: Zone) -> f32 {
    let mut order = Zone::WHEEL.iter().position(|z| *z == zone).or_else(|| Zone::QUICK_MENU.iter().position(|z| *z == zone)).unwrap_or(0) as f32;
    order = (order * 2.399_963).sin();
    order
}

/// Where a hand was at `at`, from what it was at the last moments, the oldest first: between the two nearest. `None` with nothing to go by.
fn hand_at(history: &std::collections::VecDeque<(Instant, Matrix34)>, at: Instant) -> Option<Matrix34> {
    let after = history.iter().position(|(t, _)| *t >= at);
    match after {
        None => history.back().map(|(_, m)| *m),
        Some(0) => history.front().map(|(_, m)| *m),
        Some(i) => {
            let ((t0, a), (t1, b)) = (history[i - 1], history[i]);
            let span = t1.duration_since(t0).as_secs_f32();
            let k = if span > 0.0 { at.duration_since(t0).as_secs_f32() / span } else { 1.0 };
            let lerp = |x: [f32; 3], y: [f32; 3]| space::add(x, space::scale(space::sub(y, x), k));
            let frame = space::from_axes(lerp(space::axis(&a, 0), space::axis(&b, 0)), lerp(space::axis(&a, 1), space::axis(&b, 1)), lerp(space::axis(&a, 2), space::axis(&b, 2)), lerp(space::position(&a), space::position(&b)));
            space::orthonormal(&frame)
        }
    }
}

/// A place on the hand, moved so that it shows where it was `behind` ago: from the controller still, as SteamVR draws, so that no delay costs what setting it in the room costs. With no
/// delay it is the place itself. The first way of delaying set it in the room, which trailed by a frame whatever was asked: 0.05 frames looked like 1 to Enzo, and 0 was too early.
fn behind_the_hand(on_hand: &Matrix34, now: &Matrix34, then: &Matrix34) -> Matrix34 {
    space::compose(&space::compose(&space::inverse(now), then), on_hand)
}

/// VRChat's own time between frames from the compositor's last ones: the middle one, as a hitch says nothing of the rate. `None`: nothing to go by.
fn game_frame_seconds(intervals_ms: &mut [f32]) -> Option<f32> {
    let mut known: Vec<f32> = intervals_ms.iter().copied().filter(|ms| *ms > 1.0 && *ms < 200.0).collect();
    if known.is_empty() {
        return None;
    }
    known.sort_by(f32::total_cmp);
    Some(known[known.len() / 2] / 1000.0)
}

/// How a part is told where it is.
#[derive(Clone, Copy)]
enum Placing {
    /// In the room, where the quad says.
    Room,
    /// In the room, somewhere else than the quad: on its spring.
    At(Matrix34),
    /// From the controller, by SteamVR as it draws.
    Hand(u32, Matrix34),
}

/// The Quick Menu's parts on this hand. The wing lies on the side of the menu away from the hand that holds it: on the right hand it is on the left (as Enzo laid it there). The sheet is not one of
/// these: it stays on the menu.
pub fn quick_menu_places(hand: usize) -> Vec<Placed> {
    Zone::around(Around::QuickMenu)
        .iter()
        .map(|zone| {
            let mut placed = zones::in_units(*zone);
            if hand == 1 {
                let rect = placed.rect;
                // The wing as a whole, its tiles in the same order; the caption over it.
                placed.rect = if *zone == Zone::QmCaption {
                    egui::Rect::from_min_max(egui::pos2(-rect.right(), rect.top()), egui::pos2(-rect.left(), rect.bottom()))
                } else {
                    rect.translate(egui::vec2(-(362.0 + 524.0), 0.0))
                };
            }
            placed
        })
        .collect()
}

/// The sheet opened from the wheel: it stays where it opened, before the eyes, with no menu of VRChat's under it to ride with.
pub const SHEET_DISTANCE_M: f32 = 0.75;
pub const SHEET_BELOW_M: f32 = 0.1;
pub const SHEET_UNIT_M: f32 = 0.0015;
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

/// The wheel on the hand that opened it, turning with it as VRChat's does.
pub fn wheel_plane(grip: &Matrix34, hand: usize, tune: &Tune, size: f32) -> Plane {
    Plane { frame: space::compose(grip, &tune.wheel(hand)), unit: tune.wheel_radius * size / 100.0 }
}

/// The Quick Menu on the hand that opened it, the same way.
pub fn quick_menu_plane(grip: &Matrix34, hand: usize, tune: &Tune, size: f32) -> Plane {
    Plane { frame: space::compose(grip, &tune.quick_menu(hand)), unit: tune.qm_unit * size }
}

/// A sheet with no menu of VRChat's under it: before the eyes, where it opened.
pub fn sheet_plane(head: &Matrix34, size: f32) -> Plane {
    Plane { frame: space::before_eyes(head, SHEET_DISTANCE_M, SHEET_BELOW_M), unit: SHEET_UNIT_M * size }
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
    /// How long a frame of the headset's is, in seconds.
    pub frame: f32,
    /// What the panel on the sheet shows of the 3D view, which lives with the overlay's own 3D worlds.
    pub(in crate::overlay) world_status: super::super::worlds::panel::Status,
    pub prediction: f32,
}

/// What a turn asks of the overlay's loop.
#[derive(Clone, Debug, Default)]
pub struct Turn {
    /// A press happened (for the click's sound).
    pub clicked: bool,
    /// What the panel on the sheet asked of the 3D view. The sheet has its own panel, so nothing else opens beside it, and N4N0's menu stays out of this.
    pub(in crate::overlay) worlds: Vec<super::super::worlds::Relayed>,
}

pub struct OrbitVr {
    key: String,
    panels: Panels,
    tune: Tune,
    tune_seen: Option<std::time::SystemTime>,
    tune_read: Instant,
    aligning: Option<Aligning>,
    /// Each part on its spring.
    jelly: HashMap<Zone, Jelly>,
    /// Where each hand was over the last moments.
    hand_was: [std::collections::VecDeque<(Instant, Matrix34)>; 2],
    /// VRChat's time between frames, as last read, and when.
    game_frame: f32,
    game_frame_read: Instant,
    /// Since when the left stick has been pushed far enough to walk.
    walking_since: Option<Instant>,
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
            panels: Panels::new(backend, 2.0, true),
            tune: Tune::read().unwrap_or_default(),
            tune_seen: None,
            tune_read: now,
            aligning: None,
            jelly: HashMap::new(),
            hand_was: Default::default(),
            game_frame: 1.0 / 45.0,
            game_frame_read: now,
            walking_since: None,
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

    /// With [`Tune::align`]: the free hand's grip holds the plane; letting go saves where it lies on the hand that holds the menu.
    fn align(&mut self, open: Open, grips: [bool; 2], devices: &[Option<u32>; 2], poses: &[Pose; MAX_DEVICES], size: f32) {
        let hand = match open {
            Open::Wheel(hand) | Open::QuickMenu(hand) => hand,
            Open::Closed => {
                self.aligning = None;
                return;
            }
        };
        let (Some(holder), Some(free)) = (devices[hand], devices[1 - hand]) else { return };
        let (holder, free) = (poses[holder as usize].device_to_absolute, poses[free as usize].device_to_absolute);
        let wheel = matches!(open, Open::Wheel(_));
        match self.aligning {
            None if grips[1 - hand] => {
                if let Some(plane) = self.plane {
                    self.aligning = Some(Aligning { held: space::compose(&space::inverse(&free), &plane.frame), scaling: None });
                }
            }
            Some(mut aligning) if grips[1 - hand] => {
                let apart = space::length(space::sub(space::position(&free), space::position(&holder)));
                if grips[hand] {
                    let from = aligning.scaling.get_or_insert((apart.max(0.01), if wheel { self.tune.wheel_radius } else { self.tune.qm_unit }));
                    let scaled = from.1 * apart / from.0;
                    if wheel {
                        self.tune.wheel_radius = scaled;
                    } else {
                        self.tune.qm_unit = scaled;
                    }
                } else {
                    aligning.scaling = None;
                }
                self.aligning = Some(aligning);
                let unit = if wheel { self.tune.wheel_radius / 100.0 } else { self.tune.qm_unit };
                self.plane = Some(Plane { frame: space::compose(&free, &aligning.held), unit: unit * size });
            }
            Some(aligning) => {
                self.aligning = None;
                let on_hand = to_saved(&space::compose(&space::inverse(&holder), &space::compose(&free, &aligning.held)));
                if wheel {
                    self.tune.wheel_on[hand] = Some(on_hand);
                } else {
                    self.tune.qm_on[hand] = Some(on_hand);
                }
                self.tune.write();
                self.tune_seen = std::fs::metadata(Tune::file()).and_then(|m| m.modified()).ok();
            }
            None => {}
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
        if now.duration_since(self.tune_read) >= Duration::from_secs(1) {
            self.tune_read = now;
            let seen = std::fs::metadata(Tune::file()).and_then(|m| m.modified()).ok();
            if seen != self.tune_seen {
                self.tune_seen = seen;
                self.tune = Tune::read().unwrap_or_default();
            }
        }
        let down = if cx.input_read { cx.actions.orbit_menu_buttons(cx.steamvr) } else { [false; 2] };
        if cx.input_read {
            self.buttons.step(down, now);
            // Walking closes VRChat's Quick Menu. A moment's touch of the stick does not; laying the parts on it is not walking.
            let stick = cx.actions.pads(cx.steamvr).map_or(0.0, |pads| pads.sticks[0].x.hypot(pads.sticks[0].y));
            self.walking_since = (stick >= WALKS_AT && self.aligning.is_none()).then(|| self.walking_since.unwrap_or(now));
            if self.walking_since.is_some_and(|since| now.duration_since(since) >= WALKS_AFTER) {
                self.buttons.walking();
            }
        }
        let open = self.buttons.open();
        let menu = match open {
            Open::QuickMenu(_) if settings.around_quick_menu() => Some(Around::QuickMenu),
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
            (Some(Around::Wheel), Open::Wheel(hand)) => devices[hand].map(|d| wheel_plane(&poses[d as usize].device_to_absolute, hand, &self.tune, settings.vr_size)),
            (Some(Around::QuickMenu), Open::QuickMenu(hand)) => devices[hand].map(|d| quick_menu_plane(&poses[d as usize].device_to_absolute, hand, &self.tune, settings.vr_size)),
            _ => None,
        };
        // A hand out of tracking for a moment: the parts stay where it was.
        self.plane = plane.or(self.plane);
        let grips = if self.tune.align && cx.input_read { cx.actions.grabs(cx.steamvr) } else { [false; 2] };
        self.align(open, grips, &devices, &poses, settings.vr_size);
        let mut places: Vec<Placed> = match (menu, open) {
            (Some(Around::QuickMenu), Open::QuickMenu(hand)) => quick_menu_places(hand),
            (Some(m), _) => Zone::around(m).iter().map(|z| zones::in_units(*z)).collect(),
            (None, _) => Vec::new(),
        };
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
        let Some(plane) = self.plane else {
            self.stuck.beat(false);
            return Turn::default();
        };
        // On the Quick Menu's hand a sheet rides with it, as the Quick Menu does. From the wheel it has nothing to ride.
        if self.panels.takeover.is_some() && self.takeover_plane.is_none() && menu == Some(Around::Wheel) {
            self.takeover_plane = Some(sheet_plane(&head, settings.vr_size));
        }
        if self.panels.takeover.is_none() || menu != Some(Around::Wheel) {
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

        // How the parts on the hand follow it (Enzo: "both options"). Exact, SteamVR moves each with the controller's own pose as it draws. VRChat's menu is drawn from a pose taken a frame
        // before that, by how long a frame takes on this computer: so a delay is in VRChat's own frames, read as it runs, and a spring is in step with nothing, on purpose.
        if now.duration_since(self.game_frame_read) >= Duration::from_millis(500) {
            self.game_frame_read = now;
            if let Ok(compositor) = cx.steamvr.compositor() {
                let mut timings = [vr::compositor::FrameTiming::default(); 16];
                let n = compositor.frame_timings(&mut timings);
                let mut intervals: Vec<f32> = timings[..n].iter().map(|t| t.client_frame_interval_ms).collect();
                self.game_frame = game_frame_seconds(&mut intervals).unwrap_or(self.game_frame);
            }
        }
        let hand_now = match open {
            Open::Wheel(hand) | Open::QuickMenu(hand) => devices[hand].map(|d| (hand, d)),
            Open::Closed => None,
        };
        let on_this_hand = hand_now.map(|(hand, _)| if menu == Some(Around::Wheel) { (self.tune.wheel(hand), self.tune.wheel_radius * settings.vr_size / 100.0) } else { (self.tune.quick_menu(hand), self.tune.qm_unit * settings.vr_size) });
        let springy = settings.vr_motion == VrMotion::Springy && !cx.snap.settings.reduce_motion;
        // Where the hand holding the menu was, and where it is drawn from.
        let behind = settings.vr_delay_frames.max(0.0) * self.game_frame;
        let late = hand_now.and_then(|(hand, device)| {
            let pose = poses[device as usize].device_to_absolute;
            let history = &mut self.hand_was[hand];
            history.push_back((now, pose));
            while history.front().is_some_and(|(t, _)| now.duration_since(*t) > Duration::from_millis(300)) {
                history.pop_front();
            }
            hand_at(history, now.checked_sub(Duration::from_secs_f32(behind))?).map(|then| (pose, then))
        });
        let jelly = &mut self.jelly;
        let aligning = self.aligning.is_some();
        let from_wheel = self.takeover_plane.is_some();
        let placing: Vec<Placing> = self
            .panels
            .all()
            .zip(&quads)
            .map(|(p, (quad, _))| {
                let sheet = matches!(p.zone, Zone::Takeover | Zone::SheetBack);
                // A sheet from the wheel stays in the room. What is being laid on VRChat's menu goes with the other hand.
                if aligning || (sheet && from_wheel) {
                    return Placing::Room;
                }
                let (Some((_, device)), Some((frame, unit))) = (hand_now, on_this_hand) else { return Placing::Room };
                // What lies in front of VRChat's menu keeps with it. What lies around it can dance (Enzo).
                if springy && !sheet {
                    let c = p.placed.rect.center();
                    let soft = (c.x.hypot(c.y) * unit / 0.25).min(1.0);
                    let state = jelly.entry(p.zone).or_insert_with(|| Jelly::at(&quad.transform));
                    return Placing::At(state.step(&quad.transform, soft, character(p.zone), dt));
                }
                let near = match p.zone {
                    Zone::Takeover => TAKEOVER_NEAR_M,
                    Zone::SheetBack => TAKEOVER_NEAR_M - 0.004,
                    _ => NEAR_M,
                };
                let on_hand = Plane { frame, unit }.quad(&p.placed, -p.outward(), near).transform;
                match &late {
                    Some((now, then)) if behind > 0.0 => Placing::Hand(device, behind_the_hand(&on_hand, now, then)),
                    _ => Placing::Hand(device, on_hand),
                }
            })
            .collect();
        if hand_now.is_none() || !springy {
            self.jelly.clear();
        }
        if hand_now.is_none() {
            self.hand_was = Default::default();
        }

        // Who points at what.
        let lasers = if cx.input_read { cx.actions.lasers(cx.steamvr, cx.prediction) } else { [None; 2] };
        let triggers = cx.actions.orbit_triggers(cx.steamvr, self.block.blocked());
        let hits: [Option<(usize, [f32; 2], space::Hit)>; 2] = std::array::from_fn(|i| {
            // The hand that holds the menu points at nothing.
            if matches!(open, Open::Wheel(h) | Open::QuickMenu(h) if h == i) {
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
            self.buttons.reset();
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
                // Its close says nothing is open: what puts these right when a menu closed some way this could not see.
                if pressed.hide {
                    self.buttons.reset();
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

        turn.worlds = self.panels.world_relay(cx.world_status.clone());
        // Pictures, then each overlay where it goes.
        self.show(&mut cx, &quads, &placing);
        self.last_shown = true;
        turn
    }

    fn show(&mut self, cx: &mut Cx<'_, '_>, quads: &[(Quad, Placed)], placing: &[Placing]) {
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
        for (((quad, _), panel), place) in quads.iter().zip(self.panels.all()).zip(placing) {
            if let Some(handle) = self.handles.get(&panel.zone) {
                if !panel.painted || !panel.shown {
                    let _ = cx.overlay.hide(*handle);
                    continue;
                }
                let _ = cx.overlay.set_width_in_meters(*handle, quad.width);
                let _ = match place {
                    Placing::Hand(device, transform) => cx.overlay.set_transform_device_relative(*handle, *device, transform),
                    Placing::At(transform) => cx.overlay.set_transform_absolute(*handle, transform),
                    Placing::Room => cx.overlay.set_transform_absolute(*handle, &quad.transform),
                };
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
        self.jelly.clear();
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
        assert_eq!(run(&mut fresh(t0), t0, &[(1, 100, 250)], 400), Open::QuickMenu(1), "on the hand that opened it");
        assert_eq!(run(&mut fresh(t0), t0, &[(0, 100, 250)], 400), Open::QuickMenu(0));
        assert_eq!(run(&mut fresh(t0), t0, &[(0, 100, 900)], 700), Open::Wheel(0), "while still held");
        assert_eq!(run(&mut fresh(t0), t0, &[(1, 100, 900)], 1000), Open::Wheel(1), "and after it");
    }

    /// What Enzo saw: the Quick Menu's parts going when it opened and coming when it closed.
    #[test]
    fn opening_the_other_menu_takes_the_first_s_place_and_never_turns_it_inside_out() {
        let t0 = Instant::now();
        let mut b = fresh(t0);
        // The wheel over the Quick Menu: it takes its place, and when it closes nothing is open.
        assert_eq!(run(&mut b, t0, &[(1, 100, 200)], 300), Open::QuickMenu(1));
        assert_eq!(run(&mut b, t0, &[(0, 400, 1100)], 1200), Open::Wheel(0));
        assert_eq!(run(&mut b, t0, &[(0, 1300, 1400)], 1500), Open::Closed);
        // Then the Quick Menu opens with its first press, and closes with its second.
        assert_eq!(run(&mut b, t0, &[(1, 1600, 1700)], 1800), Open::QuickMenu(1));
        assert_eq!(run(&mut b, t0, &[(1, 1900, 2000)], 2100), Open::Closed);
        // The Quick Menu from the other hand over a wheel: it takes the wheel's place.
        assert_eq!(run(&mut b, t0, &[(0, 2200, 2900)], 3000), Open::Wheel(0));
        assert_eq!(run(&mut b, t0, &[(1, 3100, 3200)], 3300), Open::QuickMenu(1));
        // Enzo: a Quick Menu opened on the other hand closes the first, and is the one open: the next press there closes it.
        assert_eq!(run(&mut b, t0, &[(0, 3400, 3500)], 3600), Open::QuickMenu(0));
        assert_eq!(run(&mut b, t0, &[(1, 3700, 3800)], 3900), Open::QuickMenu(1));
        assert_eq!(run(&mut b, t0, &[(1, 4000, 4100)], 4200), Open::Closed);
        // A wheel on the other hand takes the place of the first.
        assert_eq!(run(&mut b, t0, &[(0, 4300, 5000)], 5100), Open::Wheel(0));
        assert_eq!(run(&mut b, t0, &[(1, 5200, 5900)], 6000), Open::Wheel(1));
    }

    /// Enzo: walking closes VRChat's menu, and left the parts inside out.
    #[test]
    fn walking_closes_the_quick_menu_and_the_next_press_opens_it() {
        let t0 = Instant::now();
        let mut b = fresh(t0);
        assert_eq!(run(&mut b, t0, &[(1, 100, 200)], 300), Open::QuickMenu(1));
        b.walking();
        assert_eq!(b.open(), Open::Closed);
        assert_eq!(run(&mut b, t0, &[(1, 400, 500)], 600), Open::QuickMenu(1), "open with the next press, not inside out");
        // The wheel stays: its stick is what picks its slices.
        let mut b = fresh(t0);
        assert_eq!(run(&mut b, t0, &[(0, 100, 900)], 1000), Open::Wheel(0));
        b.walking();
        assert_eq!(b.open(), Open::Wheel(0));
    }

    #[test]
    fn a_button_already_held_when_this_starts_presses_nothing() {
        let t0 = Instant::now();
        let mut b = MenuButtons::default();
        assert_eq!(b.step([true, false], t0), Open::Closed);
        assert_eq!(b.step([true, false], at(t0, 900)), Open::Closed);
        assert_eq!(b.step([false, false], at(t0, 950)), Open::Closed);
        assert_eq!(b.step([true, false], at(t0, 1000)), Open::Closed, "a new press");
        assert_eq!(b.step([false, false], at(t0, 1100)), Open::QuickMenu(0));
    }

    /// When a menu closed some way that cannot be seen, the parts' close puts it right, in one press and for good.
    #[test]
    fn the_close_button_says_nothing_is_open_and_the_next_press_opens() {
        let t0 = Instant::now();
        let mut b = fresh(t0);
        assert_eq!(run(&mut b, t0, &[(1, 100, 200)], 300), Open::QuickMenu(1));
        b.reset();
        assert_eq!(b.open(), Open::Closed);
        assert_eq!(run(&mut b, t0, &[(1, 400, 500)], 600), Open::QuickMenu(1), "open with the next press, not inside out");
    }

    fn turned(grip: &Matrix34, degrees: f32) -> Matrix34 {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let turn = |v: [f32; 3]| [v[0] * cos + v[2] * sin, v[1], -v[0] * sin + v[2] * cos];
        space::from_axes(turn(space::axis(grip, 0)), turn(space::axis(grip, 1)), turn(space::axis(grip, 2)), space::position(grip))
    }

    fn grip() -> Matrix34 {
        space::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.3, 1.2, -0.4])
    }

    #[test]
    fn the_wheel_and_the_quick_menu_turn_with_the_hand_that_opened_them() {
        let tune = Tune::default();
        for plane_of in [wheel_plane as fn(&Matrix34, usize, &Tune, f32) -> Plane, quick_menu_plane] {
            let (a, b) = (plane_of(&grip(), 1, &tune, 1.0), plane_of(&turned(&grip(), 90.0), 1, &tune, 1.0));
            // Turning the hand turns the plane just as much: seen from the hand, nothing moved.
            let seen = |p: &Plane, g: &Matrix34| space::compose(&space::inverse(g), &p.frame);
            let (from_a, from_b) = (seen(&a, &grip()), seen(&b, &turned(&grip(), 90.0)));
            for i in 0..3 {
                for j in 0..4 {
                    assert!((from_a.m[i][j] - from_b.m[i][j]).abs() < 0.001, "it turned with the hand");
                }
            }
            // And it stays as far from the hand, wherever the hand is.
            let distance = |p: &Plane, g: &Matrix34| space::length(space::sub(space::position(&p.frame), space::position(g)));
            assert!((distance(&a, &grip()) - distance(&b, &turned(&grip(), 90.0))).abs() < 0.001);
            // A proper frame: its axes square, its +Z what they make.
            let (x, y, z) = (space::axis(&a.frame, 0), space::axis(&a.frame, 1), space::axis(&a.frame, 2));
            assert!(space::dot(x, y).abs() < 0.01 && space::dot(y, z).abs() < 0.01);
            assert!(space::length(space::sub(space::cross(x, y), z)) < 0.01, "right-handed: not a mirror image");
        }
    }

    #[test]
    fn a_plane_keeps_its_share_of_the_wheel_and_grip_holding_undoes_it() {
        let tune = Tune::default();
        let plane = wheel_plane(&grip(), 1, &tune, 1.0);
        let top = plane.quad(&zones::in_units(Zone::WheelWorlds), 0.0, NEAR_M);
        let up = space::dot(space::sub(space::position(&top.transform), space::position(&plane.frame)), space::axis(&plane.frame, 1));
        assert!(up > 0.0, "the first cell is toward the picture's top");
        assert!((top.width / tune.wheel_radius - zones::Zone::WheelWorlds.units().width() / 100.0).abs() < 0.01);
        let large = wheel_plane(&grip(), 1, &tune, 1.25).quad(&zones::in_units(Zone::WheelWorlds), 0.0, 0.0).width;
        assert!((large / top.width - 1.25).abs() < 0.01);
        assert!(OrbitSettings::VR_SIZES.contains(&1.25));
        let back = grip_holding(&plane.frame, &tune.wheel(1));
        assert!(space::length(space::sub(space::position(&back), space::position(&grip()))) < 0.001);
        assert!(space::dot(space::axis(&back, 1), space::axis(&grip(), 1)) > 0.999);
    }

    /// What is laid on one hand: exact on that hand, mirrored on the other until it has its own.
    #[test]
    fn a_plane_laid_on_a_hand_is_kept_and_mirrored_on_the_other() {
        let laid = space::from_axes([0.6, 0.0, 0.8], [0.0, 1.0, 0.0], [-0.8, 0.0, 0.6], [0.05, 0.1, -0.2]);
        let mut tune = Tune { wheel_on: [None; 2], ..Tune::default() };
        tune.wheel_on[1] = Some(to_saved(&laid));
        let right = tune.wheel(1);
        assert_eq!(right.m, laid.m);
        let left = tune.wheel(0);
        assert!((space::position(&left)[0] + 0.05).abs() < 0.001, "the other side of the controller");
        assert!((space::position(&left)[1] - 0.1).abs() < 0.001);
        let (x, y, z) = (space::axis(&left, 0), space::axis(&left, 1), space::axis(&left, 2));
        assert!(space::length(space::sub(space::cross(x, y), z)) < 0.001, "still not a mirror image of a picture");
        // The measured Quick Menus are nearly each other's mirror image, as they should be.
        let (qm_left, qm_right) = (Tune::default().quick_menu(0), mirrored(&Tune::default().quick_menu(1)));
        assert!(space::length(space::sub(space::position(&qm_left), space::position(&qm_right))) < 0.01);
        for axis in 0..3 {
            assert!(space::dot(space::axis(&qm_left, axis), space::axis(&qm_right, axis)) > 0.99, "{axis}");
        }
        // Its own, once it has one; and the Quick Menu is apart from all this.
        tune.wheel_on[0] = Some(to_saved(&laid));
        assert_eq!(tune.wheel(0).m, laid.m);
        assert_eq!(tune.quick_menu(1).m, Tune::default().quick_menu(1).m);
    }

    fn moved(m: &Matrix34, by: f32) -> Matrix34 {
        space::from_axes(space::axis(m, 0), space::axis(m, 1), space::axis(m, 2), space::add(space::position(m), [by, 0.0, 0.0]))
    }

    #[test]
    fn a_tile_on_its_spring_trails_the_hand_goes_past_a_little_and_settles() {
        let start = grip();
        let mut tile = Jelly::at(&start);
        // At rest it stays exactly there.
        let still = tile.step(&start, 0.5, 0.0, 0.011);
        assert!(space::length(space::sub(space::position(&still), space::position(&start))) < 0.0001);
        // The hand jumps 3 cm: it trails, then passes it a little, then settles.
        let to = moved(&start, 0.03);
        let first = space::position(&tile.step(&to, 0.5, 0.0, 0.011))[0] - space::position(&start)[0];
        assert!(first > 0.0 && first < 0.03, "it trails: {first}");
        let (mut most, mut end) = (first, first);
        for _ in 0..120 {
            end = space::position(&tile.step(&to, 0.5, 0.0, 0.011))[0] - space::position(&start)[0];
            most = most.max(end);
        }
        assert!(most > 0.03, "a little bounce: {most}");
        assert!(most < 0.042, "a little: {most}");
        assert!((end - 0.03).abs() < 0.0005, "settled: {end}");
    }

    #[test]
    fn a_hand_flung_about_does_not_leave_a_tile_behind_and_far_tiles_are_looser() {
        let start = grip();
        let (mut firm, mut loose) = (Jelly::at(&start), Jelly::at(&start));
        let to = moved(&start, 2.0);
        let trail = |at: &Matrix34| space::length(space::sub(space::position(at), space::position(&to)));
        let (mut a, mut b) = (0.0, 0.0);
        for _ in 0..3 {
            a = trail(&firm.step(&to, 0.0, 0.0, 0.011));
            b = trail(&loose.step(&to, 1.0, 0.0, 0.011));
        }
        assert!(a <= JELLY_REACH_M + 0.0001 && b <= JELLY_REACH_M + 0.0001, "{a} {b}");
        // A hand's small move, to see the difference within reach.
        let near = moved(&start, 0.02);
        let (mut firm, mut loose) = (Jelly::at(&start), Jelly::at(&start));
        let (x, y) = (space::position(&firm.step(&near, 0.0, 0.0, 0.02))[0], space::position(&loose.step(&near, 1.0, 0.0, 0.02))[0]);
        assert!(x > y, "{x} {y}");
    }

    /// Enzo: "this delay is not hardcoded in VRChat but more of a real lag, so it would change for each people".
    #[test]
    fn vrchat_s_frame_time_is_read_as_it_runs() {
        // The middle of what the compositor saw, as a hitch says nothing of the rate.
        assert_eq!(game_frame_seconds(&mut [27.0, 27.0, 400.0, 0.0, 27.0, 90.0, 27.0]), Some(0.027));
        assert_eq!(game_frame_seconds(&mut [0.0, 0.0]), None);
    }

    /// Enzo: 0 frames too early, 1 too late, and 0.05 no different from 1. A small delay must be a small move.
    #[test]
    fn a_delay_moves_a_part_by_what_the_hand_moved_in_that_time_and_no_delay_not_at_all() {
        let t0 = Instant::now();
        let at = |x: f32| moved(&grip(), x);
        let history: std::collections::VecDeque<(Instant, Matrix34)> = (0..10).map(|i| (t0 + Duration::from_millis(i * 10), at(i as f32 * 0.01))).collect();
        // Between two samples, where the hand was.
        let then = hand_at(&history, t0 + Duration::from_millis(45)).unwrap();
        assert!((space::position(&then)[0] - space::position(&at(0.045))[0]).abs() < 0.0001);
        // Before and after what is kept, the nearest.
        assert_eq!(hand_at(&history, t0 - Duration::from_millis(0)).unwrap().m, at(0.0).m);
        assert_eq!(hand_at(&history, t0 + Duration::from_secs(9)).unwrap().m, at(0.09).m);
        let on_hand = Tune::default().quick_menu(0);
        let now = at(0.09);
        // No delay: the place itself, exactly.
        let none = behind_the_hand(&on_hand, &now, &now);
        for i in 0..3 {
            for j in 0..4 {
                assert!((none.m[i][j] - on_hand.m[i][j]).abs() < 0.0001);
            }
        }
        // Shown from the hand as it is now, it lands where the hand was: 3 cm behind for a hand that moved 3 cm, and nothing jumps at a small delay.
        let small = behind_the_hand(&on_hand, &now, &at(0.089));
        let shown = |m: &Matrix34| space::position(&space::compose(&now, m));
        let gap = space::length(space::sub(shown(&small), shown(&on_hand)));
        assert!((gap - 0.001).abs() < 0.0001, "{gap}");
        let large = behind_the_hand(&on_hand, &now, &at(0.06));
        assert!((space::length(space::sub(shown(&large), shown(&on_hand))) - 0.03).abs() < 0.0001);
    }

    #[test]
    fn no_two_tiles_of_the_wing_have_the_same_character() {
        for (i, a) in Zone::WING.iter().enumerate() {
            assert!(character(*a).abs() <= 1.0);
            for b in &Zone::WING[i + 1..] {
                assert!((character(*a) - character(*b)).abs() > 0.02, "{a:?} {b:?}");
            }
        }
    }

    /// As Enzo laid them.
    #[test]
    fn the_wheel_lies_flat_on_the_controller_and_the_wing_on_the_side_away_from_the_hand() {
        let wheel = Tune::default().wheel(1);
        assert!(space::axis(&wheel, 2)[1] > 0.99, "its face is the controller's top");
        assert!(space::axis(&wheel, 1)[2] < -0.99, "its first slice toward where it points");
        let wheel_left = Tune::default().wheel(0);
        assert!(space::axis(&wheel_left, 2)[1] > 0.99 && space::axis(&wheel_left, 1)[2] < -0.99, "the same on the other hand");
        let tile = |hand: usize, zone: Zone| quick_menu_places(hand).into_iter().find(|p| p.zone == zone).unwrap().rect;
        for zone in Zone::WING {
            assert!(tile(0, zone).left() > 0.0 && tile(1, zone).right() < 0.0, "{zone:?}");
            assert_eq!(tile(0, zone).size(), tile(1, zone).size());
        }
        // The same order on both: it reads the same, on the other side.
        assert!(tile(1, Zone::WingWorlds).left() < tile(1, Zone::WingYouTube).left());
        // The sheet is not moved: it stays on the menu, whichever hand.
        assert!(quick_menu_places(1).iter().all(|p| !matches!(p.zone, Zone::Takeover | Zone::SheetBack)));
    }

    #[test]
    fn the_tune_file_changes_what_it_names_keeps_what_was_laid_and_only_that() {
        let tune: Tune = serde_json::from_str(r#"{ "wheel_tilt": 70.0, "align": true }"#).unwrap();
        assert_eq!(tune.wheel_tilt, 70.0);
        assert!(tune.align);
        assert_eq!(tune.qm_ahead, Tune::default().qm_ahead);
        let mut laid = tune;
        laid.qm_on[0] = Some(to_saved(&grip()));
        let back: Tune = serde_json::from_str(&serde_json::to_string(&laid).unwrap()).unwrap();
        assert_eq!(back, laid);
    }

    #[test]
    fn a_laser_finds_the_nearest_panel_and_only_on_its_shape() {
        let head = space::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.6, 0.0]);
        let plane = sheet_plane(&head, 1.0);
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
        let under = zones::in_units(Zone::WingPlaying);
        let back = zones::in_units(Zone::SheetBack);
        let quads = [(plane.quad(&under, 0.0, 0.0), under), (plane.quad(&back, 0.0, TAKEOVER_NEAR_M - 0.004), back), (plane.quad(&front, 0.0, TAKEOVER_NEAR_M), front)];
        let target = space::position(&quads[2].0.transform);
        assert_eq!(laser_on(&quads, eyes, space::unit(space::sub(target, eyes)).unwrap()).map(|h| h.0), Some(2));
    }
}
