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
use orbiters_i18n::mark;
use super::Panels;
use crate::backend::Ctl;
use super::camera_frames::{CameraWatch, Source};
use super::camera_track;
use crate::state::{OrbitStatus, Snapshot};
use egui::Pos2;
use orbiters_openvr::{self as vr, ControllerRole, Matrix34, Pose, Universe, MAX_DEVICES};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A press this long is the wheel's, shorter the Quick Menu's. VRChat does not publish it: to be confirmed in the headset.
pub const HOLD: Duration = Duration::from_millis(500);
/// When a press with the Quick Menu open closes it, in case it becomes a long one: as late as can be, so that hardly any short press lasts this long.
pub const CLOSE_AHEAD: Duration = Duration::from_millis(380);
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
    /// The Quick Menu this closed for a press that may be long, on which hand.
    closed_ahead: Option<usize>,
    /// What VRChat must be told now: its Quick Menu toggled, on that hand.
    toggle: Option<usize>,
}

impl Default for MenuButtons {
    fn default() -> Self {
        // Buttons taken as held: one already down when this starts presses nothing.
        Self { open: Open::Closed, down: [true; 2], press: None, closed_ahead: None, toggle: None }
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

    /// VRChat's camera opened: its wheel goes then (Enzo).
    pub fn camera_opened(&mut self) {
        if matches!(self.open, Open::Wheel(_)) {
            self.open = Open::Closed;
        }
    }

    /// VRChat's Quick Menu to toggle now, on which hand, once.
    pub fn quick_menu_to_toggle(&mut self) -> Option<usize> {
        self.toggle.take()
    }

    pub fn step(&mut self, down: [bool; 2], now: Instant) -> Open {
        for hand in 0..2 {
            if down[hand] && !self.down[hand] && self.press.is_none() {
                self.press = Some((hand, now, self.open == Open::Wheel(hand)));
                self.closed_ahead = None;
            }
        }
        if let Some((hand, since, on_wheel)) = self.press {
            let held = now.duration_since(since);
            if down[hand] {
                // Enzo: a long press with the Quick Menu open only closed it. VRChat opens its wheel for a press that finds its Quick Menu closed, and nothing here can open its wheel, so
                // the Quick Menu is closed just before the press becomes a long one.
                if let Open::QuickMenu(on) = self.open {
                    if held >= CLOSE_AHEAD && !on_wheel && self.closed_ahead.is_none() {
                        self.closed_ahead = Some(on);
                        self.toggle = Some(on);
                    }
                }
                // The wheel takes the place of what was open.
                if held >= HOLD && !on_wheel {
                    self.open = Open::Wheel(hand);
                }
            } else {
                // A slow short press after all: VRChat toggles its Quick Menu at the release, back open. It was to close.
                if let Some(on) = self.closed_ahead.take().filter(|_| held < HOLD) {
                    self.toggle = Some(on);
                }
                if on_wheel {
                    self.open = Open::Closed;
                } else if held < HOLD {
                    // As VRChat must, this waits for the release: only then is it known not to be the wheel's. With a wheel open on either hand it only closes the wheel, and it
                    // takes another press for the Quick Menu (Enzo). A Quick Menu on the other hand gives way to it ("opening the right quick menu closes the left").
                    self.open = match self.open {
                        Open::Wheel(_) => Open::Closed,
                        open if open == Open::QuickMenu(hand) => Open::Closed,
                        _ => Open::QuickMenu(hand),
                    };
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
    /// VRChat's camera panel: how wide it is in metres, which only moves how far away the tiles seem.
    pub camera_panel_m: f32,
    /// How old what is read of the eye, and of VRChat's window, is by the time it is read, in milliseconds. Tiles that swim as the head turns and settle when it stops: this is too little or too much.
    pub camera_eye_age_ms: f32,
    pub camera_window_age_ms: f32,
    /// Reads VRChat's window and not what the compositor has of the eye.
    pub camera_use_window: bool,
    /// How much of the eye's width VRChat's window shows, and how far its middle lies from the eye's, right and down, of the eye's width. Only for reading the window.
    pub camera_window_share: f32,
    pub camera_window_moved: [f32; 2],
    /// Keeps a frame a second in the logs' folder (`camera-frames`), for measuring.
    pub camera_keep_frames: bool,
}

impl Default for Tune {
    fn default() -> Self {
        Self { align: false, wheel_on: [None, Some(WHEEL_RIGHT)], qm_on: [Some(QM_LEFT), Some(QM_RIGHT)], wheel_tilt: 0.0, wheel_ahead: 0.062, wheel_up: -0.018, wheel_radius: 0.101_8, qm_tilt: 10.0, qm_ahead: 0.127, qm_up: -0.03, qm_unit: 0.0009 , camera_panel_m: camera_track::PANEL_M, camera_eye_age_ms: 15.0, camera_window_age_ms: 45.0, camera_use_window: false, camera_window_share: WINDOW_SHARE, camera_window_moved: WINDOW_MOVED, camera_keep_frames: false}
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

/// How far a wing's tile stands off VRChat's panels, in menu units, at least a tile's gap; and off a sheet up beside it, which is nearer the eyes and would seem to run into it.
const WING_OFF_ITS_PANEL: f32 = 12.0;
const WING_OFF_A_SHEET: f32 = 22.0 + (VR_SHEET_SCALE - 1.0) * 352.0;
/// How much larger a sheet is in a headset than on a screen, about its foot's middle (its foot stays over VRChat's tab row). Enzo, 2026-11-11: at 1.25 (and 1.16 before
/// it, both from judges' renders) the pages were "in the face", larger than VRChat's own Quick Menu; they are its size again, as until Nov 10, and their words are made to
/// read at that size rather than the surfaces larger.
pub const VR_SHEET_SCALE: f32 = 1.0;
/// The wing's tiles against a screen's, about its inner edge's middle: at VRChat's own scale, a tile about 1.1 times a Launch Pad button and its name at most 1.3
/// times VRChat's labels (round 19's judges, with Fable's figures: a quarter larger, the wing read as 1.5 times VRChat's buttons and louder than the menu it stands
/// beside; round 18's had asked for readable names, not for larger tiles). Settled here: it is not to swing again.
pub const VR_WING_SCALE: f32 = 1.08;
/// How far up the wing lies in a headset against a screen, in the menu's units: turned towards the eyes it is nearer them than the menu, and its top and its foot
/// then lie on VRChat's main panel's (round 19's judges).
const VR_WING_RAISED: f32 = 30.0;
/// What is read over the wheel's ring, its one head, against a screen's.
const VR_WHEEL_HEAD_SCALE: f32 = 1.25;
/// Where that head's caption ends and its Hide begins, in the wheel's units.
const WHEEL_HEAD_SPLIT: f32 = 106.0;
/// How far each side of the ring's axis that head reaches: wide enough for what a pointed cell says, whole.
const WHEEL_HEAD_HALF: f32 = 190.0;

/// `placed` scaled about `anchor`, what is drawn on it as large with it.
fn scaled(mut placed: Placed, anchor: egui::Pos2, by: f32) -> Placed {
    let at = |p: egui::Pos2| anchor + (p - anchor) * by;
    placed.rect = egui::Rect::from_min_max(at(placed.rect.min), at(placed.rect.max));
    placed.unit *= by;
    placed
}

/// A part of the wheel where a headset has it: what is read over the ring, its caption and Hide ring, a quarter larger than on a screen about the ring's top (their
/// words read at about 11 px in a headset's picture, the round 19 pre-check); the ring's cells as they are.
pub fn wheel_place(zone: Zone) -> Placed {
    let mut placed = zones::in_units(zone);
    // One head over the ring (round 19's judges: two stacked pills rode on the hand): the caption, and Hide at its right end, side by side with nothing between.
    let head = zones::in_units(Zone::WheelCaption).rect;
    let head = egui::Rect::from_min_max(egui::pos2(-WHEEL_HEAD_HALF, head.min.y), egui::pos2(WHEEL_HEAD_HALF, head.max.y));
    match zone {
        Zone::WheelCaption => placed.rect = egui::Rect::from_min_max(head.min, egui::pos2(WHEEL_HEAD_SPLIT, head.max.y)),
        Zone::WheelHide => placed.rect = egui::Rect::from_min_max(egui::pos2(WHEEL_HEAD_SPLIT, head.min.y), head.max),
        _ => return placed,
    }
    scaled(placed, egui::pos2(0.0, -196.0), VR_WHEEL_HEAD_SCALE)
}

/// A sheet's page or head (`Zone::Takeover`, `Zone::SheetBack`) where a headset has it.
pub fn sheet_place(zone: Zone) -> Placed {
    scaled(zones::in_units(zone), egui::pos2(0.0, 200.0), VR_SHEET_SCALE)
}
/// How far a sheet lies over where it does on a screen: nearer the eyes, its foot would seem to reach VRChat's tab row.
const SHEET_OVER_THE_TABS: f32 = 9.0;

/// Where a part lies in a headset, from where it does on a screen.
pub fn clear_of_sheet(mut placed: Placed, sheet_up: bool) -> Placed {
    if Zone::WING.contains(&placed.zone) || placed.zone == Zone::QmCaption {
        let side = if placed.rect.center().x < 0.0 { -1.0 } else { 1.0 };
        placed.rect = placed.rect.translate(egui::vec2(side * if sheet_up { WING_OFF_A_SHEET } else { WING_OFF_ITS_PANEL }, 0.0));
    } else if matches!(placed.zone, Zone::Takeover | Zone::SheetBack | Zone::SheetPeek) {
        placed.rect = placed.rect.translate(egui::vec2(0.0, -SHEET_OVER_THE_TABS));
    }
    placed
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

/// A tile's character now, from -1 to 1. It keeps changing, slowly, each tile on its own course, so that its springiness is never something to recognise (Enzo).
fn character(zone: Zone, seconds: f32) -> f32 {
    let order = Zone::WHEEL.iter().position(|z| *z == zone).or_else(|| Zone::QUICK_MENU.iter().position(|z| *z == zone)).unwrap_or(0) as f32;
    let slow = (seconds * (0.23 + 0.09 * (order * 1.7).sin().abs()) + order * 2.4).sin();
    let quick = (seconds * (0.71 + 0.23 * (order * 0.9).cos().abs()) + order * 4.1).sin();
    slow * 0.65 + quick * 0.35
}

/// How a rigid part follows the hand: smoothly, as VRChat's menu seems to. A delay did not match it (Enzo: 1.5 frames was still ahead, and it jerked), as a delay has every jitter of the hand in it.
#[derive(Clone, Copy, Debug)]
pub struct Smooth {
    at: V3,
    axes: [V3; 3],
}

impl Smooth {
    pub fn at(pose: &Matrix34) -> Self {
        Self { at: space::position(pose), axes: [space::axis(pose, 0), space::axis(pose, 1), space::axis(pose, 2)] }
    }

    /// `dt` further toward `pose`, with this time constant in seconds. Where it is now.
    pub fn step(&mut self, pose: &Matrix34, constant: f32, dt: f32) -> Matrix34 {
        let k = if constant <= 0.0 { 1.0 } else { 1.0 - (-dt / constant).exp() };
        self.at = space::add(self.at, space::scale(space::sub(space::position(pose), self.at), k));
        for (i, axis) in self.axes.iter_mut().enumerate() {
            *axis = space::add(*axis, space::scale(space::sub(space::axis(pose, i), *axis), k));
        }
        space::orthonormal(&space::from_axes(self.axes[0], self.axes[1], self.axes[2], self.at)).unwrap_or(*pose)
    }
}

/// How a part is told where it is.
#[derive(Clone, Copy)]
enum Placing {
    /// In the room, where the quad says.
    Room,
    /// In the room, somewhere else than the quad: smoothed, or on its spring.
    At(Matrix34),
}

/// The Quick Menu's parts as a row under its tabs, its words under that: wherever the hand, it is on neither side.
pub fn strip_places() -> Vec<Placed> {
    let mut words = zones::in_units(Zone::QmCaption);
    words.rect = egui::Rect::from_min_max(egui::pos2(-262.0, 326.0), egui::pos2(262.0, 364.0));
    vec![zones::in_units(Zone::QmStrip), words]
}

/// How much further out than on a screen the wing lies in a headset, in the menu's units: a clear gap from My Profile, which reaches past the main panel.
// VRChat's own gap between its panels, with what the tiles stand off them (round 19's judges: it was four times that).
const VR_WING_CLEAR: f32 = -10.0;
/// Where the wing turns, in the menu's units from its middle (just inside its inner edge), and by how much, in radians: about as much as VRChat's side panels.
const WING_HINGE: f32 = 400.0;
const WING_YAW: f32 = 0.3;

/// The Quick Menu's parts on this hand. The wing lies on the side of the menu away from the hand that holds it: on the right hand it is on the left (as Enzo laid it there). The sheet is not one of
/// these: it stays on the menu.
pub fn quick_menu_places(hand: usize) -> Vec<Placed> {
    Zone::around(Around::QuickMenu)
        .iter()
        .map(|zone| {
            let mut placed = zones::in_units(*zone);
            // In a headset VRChat's My Profile reaches past the menu's main panel, and the tiles move on their springs: a clear gap from it (a judge saw them overlap).
            placed.rect = placed.rect.translate(egui::vec2(VR_WING_CLEAR, 0.0));
            // Its caption over the name pill, which holds Hide tiles here.
            if *zone == Zone::QmCaption {
                placed.rect = placed.rect.translate(egui::vec2(0.0, -zones::QM_CAPTION_OVER_THE_NAME));
            }
            if hand == 1 {
                let rect = placed.rect;
                // The wing as a whole, its tiles in the same order; the caption over it.
                placed.rect = if *zone == Zone::QmCaption {
                    egui::Rect::from_min_max(egui::pos2(-rect.right(), rect.top()), egui::pos2(-rect.left(), rect.bottom()))
                } else {
                    rect.translate(egui::vec2(-(362.0 + 524.0 + VR_WING_CLEAR * 2.0), 0.0))
                };
            }
            // Larger than on a screen, away from the menu: about the middle of the edge it has towards it.
            let inner = if hand == 1 { -(362.0 + VR_WING_CLEAR) } else { 362.0 + VR_WING_CLEAR };
            let mut placed = scaled(placed, egui::pos2(inner, 27.0), VR_WING_SCALE);
            placed.rect = placed.rect.translate(egui::vec2(0.0, -VR_WING_RAISED));
            placed
        })
        .collect()
}

/// The sheet opened from the wheel: it stays where it opened, before the eyes, with no menu of VRChat's under it to ride with. At 0.0015 a unit its page was 1.06 m wide at
/// 0.75 m (70 degrees, 82 with the 1.25 of Nov 10): 0.001 makes it 0.70 m, about 50 degrees, the width the Quick Menu's own page takes held at a forearm's length.
pub const SHEET_DISTANCE_M: f32 = 0.75;
pub const SHEET_BELOW_M: f32 = 0.1;
pub const SHEET_UNIT_M: f32 = 0.001;
/// Panels hang this little in front of the plane of what they are around; a sheet (its head and its page, one picture) further, clear of VRChat's own menu.
const NEAR_M: f32 = 0.004;
const TAKEOVER_NEAR_M: f32 = 0.02;

/// How far in front of its plane a part hangs. The sheet's page and its back with its head lie in one plane, as one picture ([`compose_sheet`]): Enzo saw the page float over
/// its frame, 4 mm apart (IMG6). With a sheet up the rest is in that plane too: nearer than it, it would seem to run into it.
fn near(zone: Zone, sheet_up: bool) -> f32 {
    match zone {
        Zone::Takeover | Zone::SheetBack => TAKEOVER_NEAR_M,
        _ if sheet_up => TAKEOVER_NEAR_M,
        _ => NEAR_M,
    }
}
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
        // The wing turns towards who looks about its inner edge, as VRChat's own side panels do (round 10's judges: flat beside an angled My Profile, it read as
        // pasted in front rather than a neighbour).
        if Zone::WING.contains(&placed.zone) || placed.zone == Zone::QmCaption {
            let side = if x < 0.0 { -1.0 } else { 1.0 };
            let hinge = side * WING_HINGE;
            let out = x - hinge;
            let (sin, cos) = WING_YAW.sin_cos();
            let at = space::offset(&self.frame, [(hinge + out * cos) * self.unit, -y * self.unit, near + out.abs() * sin * self.unit]);
            let (s, c) = (-side * WING_YAW).sin_cos();
            let turn = Matrix34 { m: [[c, 0.0, s, 0.0], [0.0, 1.0, 0.0, 0.0], [-s, 0.0, c, 0.0]] };
            return Quad { transform: space::compose(&at, &turn), width: placed.rect.width() * self.unit, height: placed.rect.height() * self.unit };
        }
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
/// A part as a pointer finds it.
fn seen_at(quad: Quad, place: &Placing, shown: bool) -> Quad {
    let transform = match place {
        Placing::Room => quad.transform,
        Placing::At(at) => *at,
    };
    if shown {
        Quad { transform, ..quad }
    } else {
        Quad { transform, width: 0.0, height: 0.0 }
    }
}

/// Hits nearer than this to each other are on one plane.
const SAME_PLANE_M: f32 = 0.0005;

fn laser_on(quads: &[(Quad, Placed)], origin: V3, direction: V3) -> Option<(usize, [f32; 2], space::Hit)> {
    let mut best: Option<(usize, [f32; 2], space::Hit)> = None;
    for (i, (quad, placed)) in quads.iter().enumerate() {
        let Some(hit) = quad.hit(origin, direction, 0.0) else { continue };
        let at = placed.rect.min + egui::vec2(hit.uv[0], hit.uv[1]) * placed.rect.size();
        // In one plane the later is in front (the page is the last part, over its back): a tie is not decided by rounding.
        if placed.contains(at) && best.is_none_or(|(_, _, b)| hit.distance < b.distance + SAME_PLANE_M) {
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
    /// The touchpads and sticks as this turn's read has them, once someone asked: the loop's own, which it and the 3D view read too. Fourteen actions asked of SteamVR each
    /// time, and the same answer all through a turn (they change only at the loop's one `Actions::update`).
    pub pads: &'a mut Option<Option<orbiters_n4n0::input::Pads>>,
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
    /// The hand holding VRChat's menu, whose grip means nothing to what lies in a panel's hole.
    pub menu_hand: Option<usize>,
    /// What the panel on the sheet asked of the 3D view. The sheet has its own panel, so nothing else opens beside it, and N4N0's menu stays out of this.
    pub(in crate::overlay) worlds: Vec<super::super::worlds::Relayed>,
    /// The menus up now: what a capture must not show over. None with no menu.
    pub(in crate::overlay) menus_up: Vec<super::super::worlds::stereo::MenuBox>,
    /// Where the sheet has a place for a capture standing small: the middle of its hole, its Z towards who looks, and how far it can reach there.
    pub(in crate::overlay) small_place: Option<(Matrix34, f32)>,
}

/// How much of the eye VRChat's window shows, as worked back from Enzo's two pictures: its tiles were 1.4 times the panel's scale, and how far they were off grows with the place on the picture by that.
const WINDOW_SHARE: f32 = 0.716;
const WINDOW_MOVED: [f32; 2] = [0.0, 0.0];
/// VRChat's `/input/QuickMenuToggle*`, as a button going to `value`, sent straight to it on this computer.
fn send_quick_menu_toggle(hand: usize, value: i32, port: u16) {
    let Ok(socket) = std::net::UdpSocket::bind(("127.0.0.1", 0)) else { return };
    let _ = socket.send_to(&osc_int(if hand == 0 { "/input/QuickMenuToggleLeft" } else { "/input/QuickMenuToggleRight" }, value), ("127.0.0.1", port));
}

/// An OSC message of one integer.
fn osc_int(address: &str, value: i32) -> Vec<u8> {
    let mut out = address.as_bytes().to_vec();
    out.extend(std::iter::repeat_n(0u8, 4 - out.len() % 4));
    out.extend_from_slice(b",i\0\0");
    out.extend_from_slice(&value.to_be_bytes());
    out
}

fn settings_walk_left(snap: &Snapshot) -> bool {
    snap.settings.walk_boost.hand == orbiters_core::settings::Hand::Left
}

/// How long the tiles stay where they are with no look seeing the panel (a hand over it, a white world behind its X, a head turned away): let go it stays still in the room, so what is known of it
/// stays true for long. And how many looks it takes to show them at first. They go at once when VRChat says its camera closed.
const CAMERA_LOST_AFTER: Duration = Duration::from_secs(20);
/// How far within the eye's view the tiles must lie, of its width: its very edge is not seen through the lens.
const CAMERA_VIEW_MARGIN: f32 = 0.1;
const CAMERA_LOOKS_TO_SHOW: u32 = 2;

impl OrbitVr {
    /// What is up, for a note in the log when a turn took too long.
    pub fn doing(&self) -> String {
        format!("open {:?}, {}", self.panels.open_menu(), self.panels.doing()) // i18n: not shown (a note in the log)
    }

    /// Why the tiles around VRChat's camera cannot show, for its switch in Settings.
    fn camera_note(&self, snap: &Snapshot) -> Option<String> {
        let settings = &snap.settings;
        if !settings.orbit.camera_tiles || !snap.steamvr.running {
            return None;
        }
        if !settings.osc.enabled {
            return Some(orbiters_i18n::mark("OSC is off in these settings, so Orbiters cannot tell that the camera is open.").to_string());
        }
        if snap.vrchat.running && snap.camera_open_said.is_none() {
            return Some(orbiters_i18n::mark("VRChat does not say whether its camera is open: turn on OSC in its Action Menu.").to_string());
        }
        self.camera_watch.as_ref().and_then(|watch| watch.problem()).map(str::to_string)
    }

    /// Where VRChat's camera panel is now, while it is being watched and was seen.
    fn camera_turn(&mut self, steamvr: &vr::Client, actions: &Actions, input_read: bool, prediction: f32, now: Instant, dt: f32) -> Option<Plane> {
        let watch = self.camera_watch.as_ref()?;
        let mut poses = [Pose::default(); MAX_DEVICES];
        steamvr.predicted_poses(Universe::Standing, prediction, &mut poses);
        if poses[0].pose_is_valid {
            // Where the head will be `prediction` from now, which is when this pose is for.
            self.camera_heads.push_back((now + Duration::from_secs_f32(prediction.max(0.0)), poses[0].device_to_absolute));
            while self.camera_heads.len() > 60 {
                self.camera_heads.pop_front();
            }
        }
        if let Some(seen) = watch.take() {
            let age = if seen.source == Source::Eye { self.tune.camera_eye_age_ms } else { self.tune.camera_window_age_ms };
            let then = seen.at.checked_sub(Duration::from_secs_f32(age.max(0.0) / 1000.0)).unwrap_or(seen.at);
            let head = self.camera_heads.iter().min_by_key(|(at, _)| if *at > then { *at - then } else { then - *at }).map(|(_, h)| *h);
            if let Some(head) = head {
                let eye_on_head = steamvr.eye_to_head(vr::compositor::Eye::Left);
                let raw = steamvr.projection_raw(vr::compositor::Eye::Left);
                let whole = camera_track::Mirror::whole_eye(raw);
                let mirror = match seen.source {
                    Source::Eye => whole,
                    Source::Window => camera_track::Mirror::window(raw, seen.size[0] / seen.size[1].max(1.0), self.tune.camera_window_share, self.tune.camera_window_moved),
                };
                // On the side with room in the eye's whole view, a little within it.
                let tangent_of = |u: [f32; 2]| {
                    let p = seen.found.at(u);
                    mirror.tangent(p[0] / seen.size[0], p[1] / seen.size[1])
                };
                let sides = camera_track::eye_sides(raw);
                let within = (sides[1] - sides[0]) * CAMERA_VIEW_MARGIN;
                self.camera_side = camera_track::side_with_room(tangent_of, [sides[0] + within, sides[1] - within, sides[2], sides[3]], self.camera_side);
                let _ = whole;
                let (frame, unit) = camera_track::panel_in_room(&seen.found, seen.size, &mirror, &space::compose(&head, &eye_on_head), self.tune.camera_panel_m, self.camera_side);
                self.camera_follow.seen(frame, unit);
                self.camera_seen = Some(now);
                self.camera_looks = self.camera_looks.saturating_add(1);
            }
        }
        // What tracking knows, and what grabs it. Only while it shows: a grip means nothing to it otherwise.
        let device = |role| steamvr.controller_index(role).filter(|&d| (d as usize) < MAX_DEVICES && poses[d as usize].pose_is_valid).map(|d| poses[d as usize].device_to_absolute);
        let hands = [device(ControllerRole::LeftHand), device(ControllerRole::RightHand)];
        let grips = if input_read { actions.grabs(steamvr) } else { [false; 2] };
        let lasers = if input_read { actions.lasers(steamvr, prediction) } else { [None; 2] };
        let pointers = lasers.map(|tip| tip.map(|tip| { let (o, d) = space::ray(&tip); (o, d) }));
        let placed = self.camera_follow.step(dt, hands, grips, pointers);
        // While a hand has it, it is known, seen or not.
        if self.camera_follow.is_held() {
            self.camera_seen = Some(now);
        }
        if self.camera_seen.is_none_or(|at| now.duration_since(at) > CAMERA_LOST_AFTER) {
            self.camera_follow.clear();
            self.camera_looks = 0;
            return None;
        }
        if self.camera_looks < CAMERA_LOOKS_TO_SHOW {
            return None;
        }
        placed.map(|(frame, unit)| Plane { frame, unit })
    }
}

/// A part's overlay, made the first time it shows. Its order is said to SteamVR and read back into the VR test log: Enzo's IMG2 had a pane of order 0 drawn over the
/// sheet's head (15) and page (16), which only an order by distance does, and the log says what SteamVR took.
fn part_handle(handles: &mut HashMap<Zone, vr::OverlayHandle>, overlay: &vr::Overlay, key: &str, zone: Zone) -> vr::OverlayHandle {
    *handles.entry(zone).or_insert_with(|| {
        let made = overlay.create(&format!("{key}.{zone:?}"), &format!("Orbiters {zone:?}")).unwrap_or(0);
        let _ = overlay.set_flag(made, vr::overlay_flag::IS_PREMULTIPLIED, true);
        let order = SORT_ORDER + u32::from(zone == Zone::Takeover);
        let said = overlay.set_sort_order(made, order);
        super::super::vr_log::line(&format!("overlay {zone:?}: handle {made} sort order {order} set {said:?} read back {:?}", overlay.sort_order(made))); // i18n: not shown (a log's line)
        made
    })
}

/// Where a sheet's page lies on its back's picture, in rows from its top: one picture only where they are of one width and the page lies whole inside it.
pub fn page_row(back: &super::Panel, page: &super::Panel) -> Option<u32> {
    let rows_a_unit = back.px[1] as f32 / back.placed.rect.height();
    let top = ((page.placed.rect.top() - back.placed.rect.top()) * rows_a_unit).round();
    let fits = back.px[0] == page.px[0] && (page.placed.rect.left() - back.placed.rect.left()).abs() < 0.01 && top >= 0.0 && top as u32 + page.px[1] <= back.px[1];
    fits.then_some(top as u32)
}

/// The sheet as SteamVR is given it: the back's picture (its head, its foot, its hole) with the page's over it in its place, premultiplied. The back is cut where the page
/// is (a capture's hole) and the page to the back's corners ([`super::panels::page_mask`]): one shape.
#[derive(Default)]
pub struct SheetPicture {
    pub pixels: Vec<u8>,
    pub px: [u32; 2],
    /// What each part last gave: the back's whole picture, and the page's with where it lies.
    under: Vec<u8>,
    over: Vec<u8>,
    page_px: [u32; 2],
    top: u32,
    /// The page is in it.
    pub(super) page: bool,
    /// What changed since it was last shown.
    pub(super) changed: Option<[u32; 4]>,
}

impl SheetPicture {
    /// The whole again in `rect` (x0, y0, x1, y1 of the back's pixels), and it is to be shown.
    fn redo(&mut self, rect: [u32; 4]) {
        let w = self.px[0] as usize;
        let page_rows = self.top..self.top + if self.page { self.page_px[1] } else { 0 };
        for y in rect[1]..rect[3].min(self.px[1]) {
            let (a, b) = ((y as usize * w + rect[0] as usize) * 4, (y as usize * w + rect[2].min(self.px[0]) as usize) * 4);
            if !page_rows.contains(&y) {
                self.pixels[a..b].copy_from_slice(&self.under[a..b]);
                continue;
            }
            let from = (y - self.top) as usize * w * 4 + rect[0] as usize * 4;
            for ((out, under), over) in self.pixels[a..b].chunks_exact_mut(4).zip(self.under[a..b].chunks_exact(4)).zip(self.over[from..from + (b - a)].chunks_exact(4)) {
                // Pages are opaque across almost all their area. Preserve the exact blend
                // at their cut corners and holes, and copy an opaque pixel directly.
                if over[3] == 255 { out.copy_from_slice(over); continue; }
                let left = 255 - over[3] as u32;
                for c in 0..4 {
                    out[c] = (over[c] as u32 + (under[c] as u32 * left + 127) / 255).min(255) as u8;
                }
            }
        }
        self.changed = Some(self.changed.map_or(rect, |c| [c[0].min(rect[0]), c[1].min(rect[1]), c[2].max(rect[2]), c[3].max(rect[3])]));
    }

    /// The back's picture changed in `changed`.
    pub fn back(&mut self, back: &[u8], px: [u32; 2], page_top: u32, changed: [u32; 4]) {
        if self.px != px {
            self.px = px;
            self.pixels = vec![0; back.len()];
            self.under = vec![0; back.len()];
            self.page = false;
        }
        self.top = page_top;
        let w = px[0] as usize;
        for y in changed[1]..changed[3].min(px[1]) {
            let (a, b) = ((y as usize * w + changed[0] as usize) * 4, (y as usize * w + changed[2].min(px[0]) as usize) * 4);
            self.under[a..b].copy_from_slice(&back[a..b]);
        }
        self.redo(changed);
    }

    /// The page's picture changed in `changed`; it lies `top` rows down the back's.
    pub fn page(&mut self, page: &[u8], px: [u32; 2], top: u32, changed: [u32; 4]) {
        if self.px[0] != px[0] || top + px[1] > self.px[1] {
            return;
        }
        // The first picture of a page goes in whole.
        let changed = if self.page && self.page_px == px && self.top == top { changed } else { [0, 0, px[0], px[1]] };
        if self.over.len() != page.len() {
            self.over = vec![0; page.len()];
        }
        (self.page, self.page_px, self.top) = (true, px, top);
        let w = px[0] as usize;
        for y in changed[1]..changed[3].min(px[1]) {
            let (a, b) = ((y as usize * w + changed[0] as usize) * 4, (y as usize * w + changed[2].min(px[0]) as usize) * 4);
            self.over[a..b].copy_from_slice(&page[a..b]);
        }
        self.redo([changed[0], changed[1] + top, changed[2], changed[3] + top]);
    }
}

/// The whole sheet as one picture, from its back's and its page's: for the renders and the tests, as [`SheetPicture`] makes it.
pub fn compose_sheet(back: &[u8], back_px: [u32; 2], page: &[u8], page_px: [u32; 2], top: u32) -> Vec<u8> {
    let mut sheet = SheetPicture::default();
    sheet.back(back, back_px, top, [0, 0, back_px[0], back_px[1]]);
    sheet.page(page, page_px, top, [0, 0, page_px[0], page_px[1]]);
    sheet.pixels
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
    /// The hand holding the menu, smoothed; when this began, for the springs' changing characters; whether this asked for a capture in 3D, which goes when the menu goes; whether the camera
    /// was open.
    smooth: Option<(usize, Smooth)>,
    born: Instant,
    showed_view: bool,
    camera_was: Option<bool>,
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
    /// VRChat's camera panel: what reads its window while its camera is open, where it was last seen, and how many looks in a row saw it.
    camera_watch: Option<CameraWatch>,
    camera_follow: super::camera_track::Follow,
    camera_side: camera_track::Side,
    hole_logged: super::super::vr_log::Every,
    /// What the headset shows, kept when asked (`ORBITERS_VR_PICTURES`).
    moments: super::super::eye_picture::Moments,
    osc_toggle: Option<(usize, Instant)>,
    camera_seen: Option<Instant>,
    camera_looks: u32,
    /// The head lately, for where it was when a look's frame was taken.
    camera_heads: std::collections::VecDeque<(Instant, Matrix34)>,
    trigger_was: [bool; 2],
    pointing_since: Instant,
    last_turn: Instant,
    instance: Option<String>,
    told: Option<OrbitStatus>,
    /// Where the hands last pointed.
    pointers: [Option<vr::Matrix34>; 2],
    overrides_asked: bool,
    /// The hand whose laser moves the pointer.
    active: Option<usize>,
    last_shown: bool,
    /// The sheet as one picture: its back with its head, its page on it.
    sheet: SheetPicture,
}

impl OrbitVr {
    pub fn new(overlay: &vr::Overlay, key: &str, backend: Ctl) -> anyhow::Result<Self> {
        let now = Instant::now();
        Ok(Self {
            key: format!("{key}.orbit"),
            panels: Panels::new(backend, 2.0, true).with_page_worker(),
            tune: Tune::read().unwrap_or_default(),
            tune_seen: None,
            tune_read: now,
            aligning: None,
            jelly: HashMap::new(),
            smooth: None,
            born: now,
            showed_view: false,
            camera_was: None,
            walking_since: None,
            buttons: MenuButtons::default(),
            block: Block::default(),
            stuck: Stuck::start(),
            lasers: Lasers::new(overlay, &format!("{key}.orbit"))?,
            handles: HashMap::new(),
            plane: None,
            takeover_plane: None,
            camera_watch: None,
            pointers: [None; 2],
            camera_follow: Default::default(),
            camera_side: Default::default(),
            hole_logged: Default::default(),
            moments: Default::default(),
            osc_toggle: None,
            camera_seen: None,
            camera_looks: 0,
            camera_heads: Default::default(),
            trigger_was: [true; 2],
            pointing_since: now,
            last_turn: now,
            instance: None,
            told: None,
            overrides_asked: false,
            active: None,
            last_shown: false,
            sheet: SheetPicture::default(),
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
            note: (on != Some(true)).then(|| mark("SteamVR's \"Experimental overlay input overrides\" (Settings, Developer) is off: pointing at a panel still presses in VRChat too.").to_string()),
            steamvr_overrides: on,
            camera_note: self.camera_note(cx.snap),
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
        // The parts of menus that closed are kept for a while; with no menu up nothing else comes by to let them go.
        self.panels.let_go(now);
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
            if let Some(hand) = self.buttons.quick_menu_to_toggle() {
                super::super::vr_log::line(&format!("buttons: toggling VRChat's Quick Menu on hand {hand}, open {:?}", self.buttons.open())); // i18n: not shown (a log's line)
                self.osc_toggle = Some((hand, now));
                send_quick_menu_toggle(hand, 1, cx.snap.settings.osc.vrchat_port);
            }
            // Walking closes VRChat's Quick Menu. A moment's touch of the stick does not; laying the parts on it is not walking.
            let stick = cx.pads.get_or_insert_with(|| cx.actions.pads(cx.steamvr)).map_or(0.0, |pads| pads.sticks[0].x.hypot(pads.sticks[0].y));
            self.walking_since = (stick >= WALKS_AT && self.aligning.is_none()).then(|| self.walking_since.unwrap_or(now));
            if self.walking_since.is_some_and(|since| now.duration_since(since) >= WALKS_AFTER) {
                self.buttons.walking();
            }
        }
        // VRChat's wheel goes when its camera opens.
        let camera = cx.snap.camera_open_said;
        if camera == Some(true) && self.camera_was == Some(false) {
            self.buttons.camera_opened();
        }
        self.camera_was = camera;
        let open = self.buttons.open();
        // VRChat's camera panel, seen on its window while it says its camera is open, with neither of its menus up.
        // Looked for even with a menu up: VRChat's camera is opened from one, and what follows the buttons may still say it is open.
        let camera_wanted = settings.camera_tiles && camera == Some(true);
        if camera_wanted != self.camera_watch.is_some() {
            self.camera_watch = camera_wanted.then(|| {
                let mirror = if self.tune.camera_use_window { None } else { cx.steamvr.compositor().ok().map(|c| c.mirror_source()) };
                CameraWatch::start(super::camera_frames::From { mirror, adapter: cx.steamvr.dxgi_adapter_index(), keep_frames: self.tune.camera_keep_frames })
            });
            self.camera_follow.clear();
            self.camera_seen = None;
            self.camera_looks = 0;
        }
        let camera_plane = self.camera_turn(cx.steamvr, &*cx.actions, cx.input_read, cx.prediction, now, dt);
        let menu = match open {
            Open::QuickMenu(_) if settings.around_quick_menu() => Some(Around::QuickMenu),
            Open::Wheel(_) if settings.around_wheel() => Some(Around::Wheel),
            Open::Closed if camera_plane.is_some() => Some(Around::Camera),
            _ => None,
        };
        if menu.is_none() && self.panels.is_empty() {
            if self.last_shown {
                self.close(&mut cx);
            }
            self.stuck.beat(false);
            // A capture shown from these menus goes with them (Enzo). One shown from N4N0's own menu is not theirs to hide.
            let mut turn = Turn::default();
            // One taken in a hand is out of the menu's: it stays until Hide 3D.
            if self.showed_view && !cx.world_status.small_taken {
                self.showed_view = false;
                turn.worlds.push(super::super::worlds::Relayed::View(super::super::worlds::panel::Action::Hide));
            }
            return turn;
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
            (Some(Around::Camera), _) => camera_plane,
            (Some(Around::QuickMenu), Open::QuickMenu(hand)) => devices[hand].map(|d| quick_menu_plane(&poses[d as usize].device_to_absolute, hand, &self.tune, settings.vr_size)),
            _ => None,
        };
        // A hand out of tracking for a moment: the parts stay where it was.
        self.plane = plane.or(self.plane);
        let grips = if self.tune.align && cx.input_read { cx.actions.grabs(cx.steamvr) } else { [false; 2] };
        self.align(open, grips, &devices, &poses, settings.vr_size);
        let mut places: Vec<Placed> = match (menu, open) {
            (Some(Around::QuickMenu), Open::QuickMenu(_)) if settings.layout == orbiters_core::settings::OrbitLayout::Strip => strip_places(),
            (Some(Around::QuickMenu), Open::QuickMenu(hand)) => quick_menu_places(hand),
            (Some(Around::Camera), _) if self.camera_side == camera_track::Side::Right => Zone::CAMERA.iter().map(|z| zones::in_units(*z).mirrored()).collect(),
            (Some(Around::Wheel), _) => Zone::around(Around::Wheel).iter().map(|z| wheel_place(*z)).collect(),
            (Some(m), _) => Zone::around(m).iter().map(|z| zones::in_units(*z)).collect(),
            (None, _) => Vec::new(),
        };
        // Here VRChat's wheel can close unseen: the ring has its own close.
        if menu == Some(Around::Wheel) && !self.panels.sheet_open() {
            places.push(wheel_place(Zone::WheelHide));
        }
        // Hidden: only the way back, and nothing else, whatever VRChat's menu does.
        self.panels.way_back(menu, &mut places);
        self.panels.takeover_place = Some(sheet_place(Zone::Takeover));
        self.panels.back_place = Some(sheet_place(Zone::SheetBack));
        self.panels.view_taken = cx.world_status.small_taken;
        self.panels.set_open(menu, places, cx.snap);
        if self.panels.is_empty() {
            self.close(&mut cx);
            self.stuck.beat(false);
            // A capture held in a hand with every menu down is a moment of its own.
            let held = if cx.world_status.small_taken { "no-menu small-taken" } else { "no-menu" };
            self.moments.see(cx.steamvr, held.to_string());
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
        // From the camera, beside it: a code is read through it, so it must stay in view.
        if self.panels.takeover.is_some() && self.takeover_plane.is_none() && menu == Some(Around::Camera) {
            let sheet = sheet_plane(&head, settings.vr_size);
            let left = -(301.0 * plane.unit + 360.0 * sheet.unit);
            let beside = Matrix34 { m: [[1.0, 0.0, 0.0, left], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]] };
            self.takeover_plane = Some(Plane { frame: space::compose(&plane.frame, &beside), unit: sheet.unit });
        }
        if self.panels.takeover.is_none() || !matches!(menu, Some(Around::Wheel | Around::Camera)) {
            self.takeover_plane = None;
        }
        let sheet_up = self.panels.takeover.is_some();
        let quads: Vec<(Quad, Placed)> = self
            .panels
            .all()
            .map(|p| {
                let sheet = matches!(p.zone, Zone::Takeover | Zone::SheetBack | Zone::SheetPeek);
                let on = if sheet { self.takeover_plane.unwrap_or(plane) } else { plane };
                let near = near(p.zone, sheet_up);
                (on.quad(&clear_of_sheet(p.placed, sheet_up), -p.outward(), near), p.placed)
            })
            .collect();

        // How the parts on the hand follow it (Enzo: "both options"). The pose asked for lies ahead of now by what placing an overlay in the room takes to show, so that with no smoothing nothing
        // trails, and every move of the slider is then that much softer, with no jitter: what is told to SteamVR is where it lies, and it goes there at the next frame.
        let hand_now = match open {
            Open::Wheel(hand) | Open::QuickMenu(hand) => devices[hand].map(|d| (hand, d)),
            Open::Closed => None,
        };
        let on_this_hand = hand_now.map(|(hand, _)| if menu == Some(Around::Wheel) { (self.tune.wheel(hand), self.tune.wheel_radius * settings.vr_size / 100.0) } else { (self.tune.quick_menu(hand), self.tune.qm_unit * settings.vr_size) });
        let springy = settings.vr_motion == VrMotion::Springy && !cx.snap.settings.reduce_motion;
        let seconds = now.duration_since(self.born).as_secs_f32();
        let mut ahead = [Pose::default(); MAX_DEVICES];
        cx.steamvr.predicted_poses(Universe::Standing, cx.prediction + 2.0 * cx.frame, &mut ahead);
        // The hand holding the menu, where it is shown from.
        let hand_shown = hand_now.map(|(hand, device)| {
            let pose = ahead[device as usize].device_to_absolute;
            let smooth = match &mut self.smooth {
                Some((was, smooth)) if *was == hand => smooth,
                held => &mut held.insert((hand, Smooth::at(&pose))).1,
            };
            let ms = if menu == Some(Around::Wheel) { settings.vr_smoothing_wheel_ms } else { settings.vr_smoothing_quick_menu_ms };
            (pose, smooth.step(&pose, ms.max(0.0) / 1000.0, dt))
        });
        let jelly = &mut self.jelly;
        let aligning = self.aligning.is_some();
        let from_wheel = self.takeover_plane.is_some();
        let placing: Vec<Placing> = self
            .panels
            .all()
            .zip(&quads)
            .map(|(p, _)| {
                let sheet = matches!(p.zone, Zone::Takeover | Zone::SheetBack | Zone::SheetPeek);
                // A sheet from the wheel stays in the room. What is being laid on VRChat's menu goes with the other hand.
                if aligning || (sheet && from_wheel) {
                    return Placing::Room;
                }
                let (Some((sharp, smooth)), Some((frame, unit))) = (hand_shown, on_this_hand) else { return Placing::Room };
                let near = near(p.zone, sheet_up);
                let on_hand = Plane { frame, unit }.quad(&clear_of_sheet(p.placed, sheet_up), -p.outward(), near).transform;
                // What lies in front of VRChat's menu keeps with it. What lies around it can dance (Enzo).
                if springy && !sheet {
                    let target = space::compose(&sharp, &on_hand);
                    let c = p.placed.rect.center();
                    let soft = (c.x.hypot(c.y) * unit / 0.25).min(1.0);
                    let state = jelly.entry(p.zone).or_insert_with(|| Jelly::at(&target));
                    return Placing::At(state.step(&target, soft, character(p.zone, seconds), dt));
                }
                Placing::At(space::compose(&smooth, &on_hand))
            })
            .collect();
        if hand_now.is_none() || !springy {
            self.jelly.clear();
        }
        if hand_now.is_none() {
            self.smooth = None;
        }

        // Who points at what.
        // A read that failed is not a hand gone: dropping the hover for it paints twice and shows it late.
        if cx.input_read {
            self.pointers = cx.actions.lasers(cx.steamvr, cx.prediction);
        }
        let lasers = self.pointers;
        let triggers = cx.actions.orbit_triggers(cx.steamvr, self.block.blocked());
        // Where each part is seen, smoothed or on its spring, and not where it is going: a pointer on what it sees is on it.
        let where_seen: Vec<(Quad, Placed)> = quads.iter().zip(self.panels.all()).zip(&placing).map(|((q, p), place)| (seen_at(q.0, place, p.shown), q.1)).collect();
        let hits: [Option<(usize, [f32; 2], space::Hit)>; 2] = std::array::from_fn(|i| {
            // The hand that holds the menu points at nothing.
            if matches!(open, Open::Wheel(h) | Open::QuickMenu(h) if h == i) {
                return None;
            }
            lasers[i].and_then(|tip| {
                let (o, d) = space::ray(&tip);
                laser_on(&where_seen, o, d)
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
                surface: where_seen[panel].0.transform,
                pressed: triggers[hand],
            });
            self.lasers.update(cx.overlay, cx.presenter, hand, beam);
        }

        turn.worlds = self.panels.world_relay(cx.world_status.clone());
        for asked in &turn.worlds {
            match asked {
                super::super::worlds::Relayed::Show(_) | super::super::worlds::Relayed::ShowLocal { .. } => self.showed_view = true,
                super::super::worlds::Relayed::View(super::super::worlds::panel::Action::Hide) => self.showed_view = false,
                super::super::worlds::Relayed::View(_) => {}
            }
        }
        // VRChat's own menu, as far as it reaches round its plane, and each part of ours that shows. What is in a sheet's hole is told apart where it is used.
        turn.menus_up = quads.iter().zip(self.panels.all()).filter(|(_, p)| p.shown).map(|((q, _), _)| super::super::worlds::stereo::MenuBox { place: q.transform, width: q.width, height: q.height }).collect();
        if let Some(plane) = self.plane.as_ref() {
            let (width, height) = match menu {
                Some(Around::Wheel) => (218.0, 218.0),
                Some(Around::Camera) => (super::camera_track::PANEL_SIZE[0], super::camera_track::PANEL_SIZE[1]),
                _ => (704.0, 524.0),
            };
            turn.menus_up.push(super::super::worlds::stereo::MenuBox { place: plane.frame, width: width * plane.unit, height: height * plane.unit });
        }
        turn.small_place = self.panels.small_place_on_sheet().and_then(|hole| {
            let i = self.panels.all().position(|p| p.zone == Zone::Takeover)?;
            let quad = &quads[i].0;
            let frame = match &placing[i] {
                Placing::At(m) => *m,
                Placing::Room => quad.transform,
            };
            let (x, y) = ((hole.center().x / 1024.0 - 0.5) * quad.width, (0.5 - hole.center().y / 576.0) * quad.height);
            let there = Matrix34 { m: [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, y], [0.0, 0.0, 1.0, 0.0]] };
            Some((space::compose(&frame, &there), hole.width() / 1024.0 * quad.width / 2.0 * super::super::worlds::SMALL_FILLS_HOLE))
        });
        // The stick's up and down scrolls what is on the sheet, unless a hand holds what shows small: that is zoomed with it. The stick that does not walk, as VRChat's turning one.
        if self.panels.sheet_open() && !cx.world_status.small_held && cx.input_read {
            let hand = if settings_walk_left(cx.snap) { 1 } else { 0 };
            if let Some(pads) = *cx.pads.get_or_insert_with(|| cx.actions.pads(cx.steamvr)) {
                self.panels.stick(pads.sticks[hand].y, dt, cx.snap);
            }
        }
        // The toggle's button lets go a moment later, as a finger does.
        if let Some((hand, at)) = self.osc_toggle.filter(|(_, at)| now.duration_since(*at) >= Duration::from_millis(60)) {
            let _ = at;
            send_quick_menu_toggle(hand, 0, cx.snap.settings.osc.vrchat_port);
            self.osc_toggle = None;
        }
        turn.menu_hand = match open {
            Open::Wheel(hand) | Open::QuickMenu(hand) => Some(hand),
            Open::Closed => None,
        };
        if self.hole_logged.due(Duration::from_secs(1)) && cx.world_status.small {
            super::super::vr_log::line(&format!("panel: {} small place {:?} world panel status small {} docked {} shown {} stage {:?}", self.panels.hole_report(), turn.small_place.map(|(at, r)| ([at.m[0][3], at.m[1][3], at.m[2][3]], r)), cx.world_status.small, cx.world_status.small_docked, cx.world_status.shown, cx.world_status.stage)); // i18n: not shown (a log's line)
        }
        // Pictures, then each overlay where it goes.
        self.show(&mut cx, &quads, &placing);
        self.last_shown = true;
        let facts = &self.panels.facts;
        let small = if cx.world_status.small_taken { "small-taken" } else if cx.world_status.small_docked { "small-in-its-hole" } else if cx.world_status.small { "small" } else { "" };
        self.moments.see(cx.steamvr, format!("{menu:?} {:?} {:?} {small}", facts.sheet, facts.pointed)); // i18n: not shown (a file's name)
        turn
    }

    fn show(&mut self, cx: &mut Cx<'_, '_>, quads: &[(Quad, Placed)], placing: &[Placing]) {
        let (overlay, presenter) = (cx.overlay, &mut *cx.presenter);
        let (handles, key) = (&mut self.handles, &self.key);
        // The sheet's back and its page are one picture, shown by the back's overlay.
        let page_at = self.panels.all().find(|p| p.zone == Zone::SheetBack).zip(self.panels.takeover.as_ref()).and_then(|(back, page)| page_row(back, page));
        let sheet = &mut self.sheet;
        if page_at.is_none() {
            sheet.page = false;
        }
        self.panels.paint(cx.snap, |panel, changed| {
            if let (Some(top), Zone::SheetBack | Zone::Takeover) = (page_at, panel.zone) {
                match panel.zone {
                    Zone::SheetBack => sheet.back(panel.pixels(), panel.px, top, changed),
                    _ => sheet.page(panel.pixels(), panel.px, top, changed),
                }
                return;
            }
            // A newly opened sheet gets its back on the next layout turn; wait for that
            // rather than submitting its page as a second, overlapping overlay.
            if panel.zone == Zone::Takeover { return; }
            let handle = part_handle(&mut *handles, overlay, key, panel.zone);
            if handle != 0 {
                presenter.show_part(overlay, handle, panel.pixels(), panel.px[0], panel.px[1], changed);
            }
        });
        if let Some(top) = page_at {
            if !self.sheet.page {
                if let Some(page) = self.panels.takeover.as_ref().filter(|p| p.painted) {
                    self.sheet.page(page.pixels(), page.px, top, [0, 0, page.px[0], page.px[1]]);
                }
            }
        }
        if let Some(changed) = self.sheet.changed.take() {
            let handle = part_handle(&mut self.handles, overlay, &self.key, Zone::SheetBack);
            if handle != 0 {
                presenter.show_part(overlay, handle, &self.sheet.pixels, self.sheet.px[0], self.sheet.px[1], changed);
            }
        }
        let shown: Vec<Zone> = self.panels.all().map(|p| p.zone).filter(|z| *z != Zone::Takeover).collect();
        for (((quad, _), panel), place) in quads.iter().zip(self.panels.all()).zip(placing) {
            if let Some(handle) = self.handles.get(&panel.zone) {
                // In the back's picture.
                if panel.zone == Zone::Takeover {
                    continue;
                }
                if !panel.painted || !panel.shown {
                    let _ = cx.overlay.hide(*handle);
                    continue;
                }
                let _ = cx.overlay.set_width_in_meters(*handle, quad.width);
                let _ = match place {
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
        self.sheet.page = false;
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

    /// Round 5's judges: the wing's tiles ran into a sheet's right edge. It moves away, and only with a sheet up.
    #[test]
    fn a_wing_stands_off_a_sheet_and_only_a_wing() {
        let place = |zone, x: f32| Placed { zone, rect: egui::Rect::from_min_size(egui::pos2(x, 0.0), egui::vec2(80.0, 80.0)), center: egui::Pos2::ZERO, unit: 1.0 };
        let right = place(Zone::WingWorlds, 400.0);
        let (with, without) = (clear_of_sheet(right, true).rect.left(), clear_of_sheet(right, false).rect.left());
        assert!(with > without && without >= 400.0 + 6.0, "at least a tile's gap: {with} {without}");
        assert!(clear_of_sheet(place(Zone::WingWorlds, -480.0), true).rect.left() < -480.0);
        let sheet = place(Zone::Takeover, 0.0);
        assert_eq!(clear_of_sheet(sheet, true).rect.left(), sheet.rect.left());
        assert!(clear_of_sheet(sheet, true).rect.top() < sheet.rect.top(), "clear of VRChat's tab row");
    }

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
        // Enzo: a press with a wheel open closes it, whichever hand, and it takes another for the Quick Menu.
        assert_eq!(run(&mut b, t0, &[(0, 2200, 2900)], 3000), Open::Wheel(0));
        assert_eq!(run(&mut b, t0, &[(1, 3100, 3200)], 3300), Open::Closed);
        assert_eq!(run(&mut b, t0, &[(1, 3310, 3390)], 3400), Open::QuickMenu(1));
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

    /// Enzo: "the vrchat one might actually have some smoothing to it".
    #[test]
    fn a_rigid_part_follows_the_hand_softly_and_a_shake_is_no_jitter() {
        let start = grip();
        // No smoothing: where the hand is.
        let mut none = Smooth::at(&start);
        let to = moved(&start, 0.03);
        assert!(space::length(space::sub(space::position(&none.step(&to, 0.0, 0.011)), space::position(&to))) < 0.0001);
        // Smoothing: on the way there, and there in the end, never past it.
        let mut soft = Smooth::at(&start);
        let first = space::position(&soft.step(&to, 0.03, 0.011))[0] - space::position(&start)[0];
        assert!(first > 0.0 && first < 0.03, "{first}");
        let mut most = first;
        for _ in 0..100 {
            most = most.max(space::position(&soft.step(&to, 0.03, 0.011))[0] - space::position(&start)[0]);
        }
        assert!((most - 0.03).abs() < 0.0005 && most <= 0.03 + 0.0001, "{most}");
        // A hand shaking 1 cm at 15 Hz moves it far less than that.
        let mut shaken = Smooth::at(&start);
        let (mut low, mut high) = (f32::MAX, f32::MIN);
        for i in 0..200 {
            let t = i as f32 * 0.011;
            let x = space::position(&shaken.step(&moved(&start, 0.01 * (t * 15.0 * std::f32::consts::TAU).sin()), 0.03, 0.011))[0];
            if i > 100 {
                (low, high) = (low.min(x), high.max(x));
            }
        }
        assert!(high - low < 0.008, "{}", high - low);
        // A longer constant, softer still.
        let (mut a, mut b) = (Smooth::at(&start), Smooth::at(&start));
        let (x, y) = (space::position(&a.step(&to, 0.02, 0.011))[0], space::position(&b.step(&to, 0.1, 0.011))[0]);
        assert!(x > y);
    }

    /// Enzo: "constantly vary so [...] we don't start to spot the individual springyness of each tile".
    #[test]
    fn a_tile_s_springiness_keeps_changing_and_differs_from_its_neighbours() {
        let at = |zone: Zone, t: f32| character(zone, t);
        for zone in Zone::WING {
            let values: Vec<f32> = (0..200).map(|i| at(zone, i as f32 * 0.25)).collect();
            assert!(values.iter().all(|v| v.abs() <= 1.0));
            let (low, high) = values.iter().fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
            assert!(high - low > 0.8, "{zone:?} changes: {low} {high}");
        }
        for t in [0.0, 3.0, 17.0] {
            for (i, a) in Zone::WING.iter().enumerate() {
                for b in &Zone::WING[i + 1..] {
                    assert!((at(*a, t) - at(*b, t)).abs() > 0.0001, "{a:?} {b:?} at {t}");
                }
            }
        }
    }

    /// Enzo: "When long clicking on the hand wheel menu button while the quick menu is open, it doesn't open the wheel menu but just closes the quick menu".
    #[test]
    fn a_long_press_closes_the_quick_menu_ahead_of_the_wheel() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let open_quick = |b: &mut MenuButtons| {
            b.step([false, false], at(0));
            b.step([false, true], at(10));
            b.step([false, false], at(60));
            assert_eq!(b.open(), Open::QuickMenu(1));
            assert_eq!(b.quick_menu_to_toggle(), None);
        };
        // Long: closed just before it becomes long, once, and then it is the wheel.
        let mut b = MenuButtons::default();
        open_quick(&mut b);
        b.step([false, true], at(1000));
        b.step([false, true], at(1200));
        assert_eq!(b.quick_menu_to_toggle(), None, "a short press is no business of this");
        b.step([false, true], at(1000 + CLOSE_AHEAD.as_millis() as u64));
        assert_eq!(b.quick_menu_to_toggle(), Some(1));
        b.step([false, true], at(1450));
        assert_eq!(b.quick_menu_to_toggle(), None, "once");
        assert_eq!(b.step([false, true], at(1500)), Open::Wheel(1));
        assert_eq!(b.step([false, false], at(1600)), Open::Wheel(1));
        // A slow short one: VRChat opens it again at the release, so it is told again; what is open is nothing.
        let mut b = MenuButtons::default();
        open_quick(&mut b);
        b.step([false, true], at(1000));
        b.step([false, true], at(1400));
        assert_eq!(b.quick_menu_to_toggle(), Some(1));
        assert_eq!(b.step([false, false], at(1450)), Open::Closed);
        assert_eq!(b.quick_menu_to_toggle(), Some(1));
        // A quick one: VRChat closes it itself, as ever.
        let mut b = MenuButtons::default();
        open_quick(&mut b);
        b.step([false, true], at(1000));
        assert_eq!(b.step([false, false], at(1100)), Open::Closed);
        assert_eq!(b.quick_menu_to_toggle(), None);
    }

    #[test]
    fn an_osc_message_is_padded_as_osc_has_it() {
        let m = osc_int("/input/Jump", 1);
        assert_eq!(m.len() % 4, 0);
        assert_eq!(&m[..12], b"/input/Jump\0");
        assert_eq!(&m[12..], &[b',', b'i', 0, 0, 0, 0, 0, 1]);
    }

    /// Enzo: "when I open the camera, the wheel menu disappears".
    #[test]
    fn the_camera_opening_closes_the_wheel_and_not_the_quick_menu() {
        let t0 = Instant::now();
        let mut b = fresh(t0);
        assert_eq!(run(&mut b, t0, &[(1, 100, 900)], 1000), Open::Wheel(1));
        b.camera_opened();
        assert_eq!(b.open(), Open::Closed);
        assert_eq!(run(&mut b, t0, &[(1, 1100, 1200)], 1300), Open::QuickMenu(1), "and the next press is the Quick Menu's");
        b.camera_opened();
        assert_eq!(b.open(), Open::QuickMenu(1));
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
            assert!((tile(0, zone).size() - tile(1, zone).size()).length() < 1e-3, "{zone:?}");
        }
        // The same order on both: it reads the same, on the other side.
        assert!(tile(1, Zone::WingWorlds).left() < tile(1, Zone::WingYouTube).left());
        // The sheet is not moved: it stays on the menu, whichever hand.
        assert!(quick_menu_places(1).iter().all(|p| !matches!(p.zone, Zone::Takeover | Zone::SheetBack | Zone::SheetPeek)));
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
        // In one plane, the page (the last) over its back: a tie is the page's, from either side of its middle.
        let page = sheet_place(Zone::Takeover);
        let back = sheet_place(Zone::SheetBack);
        let quads = [(plane.quad(&back, 0.0, near(Zone::SheetBack, true)), back), (plane.quad(&page, 0.0, near(Zone::Takeover, true)), page)];
        for at in [[0.1, 0.2], [0.5, 0.5], [0.9, 0.9]] {
            let target = space::apply(&quads[1].0.transform, [(at[0] - 0.5) * quads[1].0.width, (0.5 - at[1]) * quads[1].0.height, 0.0]);
            assert_eq!(laser_on(&quads, eyes, space::unit(space::sub(target, eyes)).unwrap()).map(|h| h.0), Some(1), "{at:?}");
        }
        // Its head is still its back's.
        let head = space::apply(&quads[0].0.transform, [0.0, quads[0].0.height / 2.0 - 0.01, 0.0]);
        assert_eq!(laser_on(&quads, eyes, space::unit(space::sub(head, eyes)).unwrap()).map(|h| h.0), Some(0));
    }

    /// Enzo, 2026-11-11 (F1): the pages were "in the face", 1.25 times VRChat's Quick Menu since Nov 10. On the hand the sheet is the Quick Menu's own size again, 704 of its
    /// units (0.634 m at the calibrated unit), whichever hand and at every size setting; the wing's tiles keep their own scale; a sheet from the wheel is 0.70 m at 0.75 m.
    #[test]
    fn a_sheet_is_the_size_of_vrchat_s_quick_menu() {
        let tune = Tune::default();
        for hand in 0..2 {
            for size in OrbitSettings::VR_SIZES {
                let plane = quick_menu_plane(&grip(), hand, &tune, size);
                let page = plane.quad(&clear_of_sheet(sheet_place(Zone::Takeover), true), 0.0, near(Zone::Takeover, true));
                let back = plane.quad(&clear_of_sheet(sheet_place(Zone::SheetBack), true), 0.0, near(Zone::SheetBack, true));
                let menu = 704.0 * tune.qm_unit * size;
                assert!((page.width - menu).abs() < 1e-4 && (back.width - menu).abs() < 1e-4, "hand {hand} size {size}: page {} back {} menu {menu}", page.width, back.width);
                assert!((page.height - 396.0 * tune.qm_unit * size).abs() < 1e-4);
            }
        }
        assert!((tune.qm_unit * 704.0 - 0.6336).abs() < 1e-4, "the calibrated unit");
        assert_eq!(VR_SHEET_SCALE, 1.0);
        let head = space::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.6, 0.0]);
        let alone = sheet_plane(&head, 1.0).quad(&sheet_place(Zone::Takeover), 0.0, near(Zone::Takeover, true));
        let degrees = 2.0 * (alone.width / 2.0 / SHEET_DISTANCE_M).atan().to_degrees();
        assert!((0.65..0.75).contains(&alone.width) && (45.0..55.0).contains(&degrees), "from the wheel: {} m, {degrees} degrees", alone.width);
    }

    /// Enzo's IMG6 (F25): the page floated 4 mm over its frame. The page, its back with its head and the way back to VRChat's menus lie in one plane, on the hand and
    /// before the eyes alike, and SteamVR is given them as one picture: the page's pixels over the back's, in their place, with nothing between them at any angle.
    #[test]
    fn a_sheet_and_its_page_are_one_flat_picture() {
        let tune = Tune::default();
        let head = space::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.6, 0.0]);
        for plane in [quick_menu_plane(&grip(), 0, &tune, 1.0), quick_menu_plane(&grip(), 1, &tune, 1.2), sheet_plane(&head, 1.0)] {
            let quad = |zone| plane.quad(&clear_of_sheet(sheet_place(zone), true), 0.0, near(zone, true));
            let (page, back, peek) = (quad(Zone::Takeover), quad(Zone::SheetBack), quad(Zone::SheetPeek));
            let normal = space::axis(&back.transform, 2);
            for other in [page, peek] {
                assert!(space::dot(space::axis(&other.transform, 2), normal) > 0.99999);
                let apart = space::dot(space::sub(space::position(&other.transform), space::position(&back.transform)), normal);
                assert!(apart.abs() < 1e-5, "{apart} m off the back's plane");
            }
        }
        // The pictures: the page lies whole on the back's, 44 of its units (the head) down, of its width.
        let page = super::super::Panel::new(clear_of_sheet(sheet_place(Zone::Takeover), true), 2.0, 0, false);
        let back = super::super::Panel::new(clear_of_sheet(sheet_place(Zone::SheetBack), true), 2.0, 0, false);
        assert_eq!(page_row(&back, &page), Some(88));
        let (bw, bh) = (back.px[0], back.px[1]);
        let mut back_pixels = vec![0u8; (bw * bh * 4) as usize];
        for y in 0..bh {
            for x in 0..bw {
                let i = ((y * bw + x) * 4) as usize;
                back_pixels[i..i + 4].copy_from_slice(&[10, 20, 30, 255]);
            }
        }
        let (pw, ph) = (page.px[0], page.px[1]);
        let mut page_pixels = vec![0u8; (pw * ph * 4) as usize];
        for (i, p) in page_pixels.chunks_exact_mut(4).enumerate() {
            let (x, y) = (i as u32 % pw, i as u32 / pw);
            // A hole through it, clear on the back too.
            let hole = (100..200).contains(&x) && (100..200).contains(&y);
            p.copy_from_slice(if hole { &[0, 0, 0, 0] } else { &[200, 100, 50, 255] });
            if hole {
                let j = (((y + 88) * bw + x) * 4) as usize;
                back_pixels[j..j + 4].copy_from_slice(&[0, 0, 0, 0]);
            }
        }
        let whole = compose_sheet(&back_pixels, back.px, &page_pixels, page.px, 88);
        let at = |x: u32, y: u32| &whole[((y * bw + x) * 4) as usize..((y * bw + x) * 4 + 4) as usize];
        assert_eq!(at(10, 10), &[10, 20, 30, 255], "its head");
        assert_eq!(at(10, 88 + 10), &[200, 100, 50, 255], "its page");
        assert_eq!(at(150, 88 + 150), &[0, 0, 0, 0], "its hole, through both");
        assert_eq!(at(10, bh - 2), &[10, 20, 30, 255], "its foot");
        // Changed a little later, only what changed moves, and the rest stays.
        let mut sheet = SheetPicture::default();
        sheet.back(&back_pixels, back.px, 88, [0, 0, bw, bh]);
        sheet.page(&page_pixels, page.px, 88, [0, 0, pw, ph]);
        sheet.changed = None;
        page_pixels[0..4].copy_from_slice(&[1, 2, 3, 255]);
        sheet.page(&page_pixels, page.px, 88, [0, 0, 1, 1]);
        assert_eq!(sheet.changed, Some([0, 88, 1, 89]));
        assert_eq!(&sheet.pixels[((88 * bw) * 4) as usize..((88 * bw) * 4 + 4) as usize], &[1, 2, 3, 255]);
        sheet.back(&back_pixels, back.px, 88, [0, 0, bw, 1]);
        assert_eq!(sheet.changed, Some([0, 0, bw, 89]), "one submission covers header and translated page damage");
        assert_eq!(&sheet.pixels[((88 * bw) * 4) as usize..((88 * bw) * 4 + 4) as usize], &[1, 2, 3, 255], "the page stays over a back painted again");
    }
    #[test]
    fn composed_opaque_and_translucent_pixels_match_the_scalar_reference() {
        let under = [17, 33, 61, 255].repeat(256 * 3);
        let mut page = Vec::new();
        let mut expected = under.clone();
        for y in 0..2 {
            for alpha in 0..=255u32 {
                let over = [(93 * alpha / 255) as u8, (47 * alpha / 255) as u8, (211 * alpha / 255) as u8, alpha as u8];
                page.extend_from_slice(&over);
                let at = ((y + 1) * 256 + alpha as usize) * 4;
                for c in 0..4 { expected[at + c] = (over[c] as u32 + (under[at + c] as u32 * (255 - alpha) + 127) / 255).min(255) as u8; }
            }
        }
        assert_eq!(compose_sheet(&under, [256, 3], &page, [256, 2], 1), expected);
    }

}
