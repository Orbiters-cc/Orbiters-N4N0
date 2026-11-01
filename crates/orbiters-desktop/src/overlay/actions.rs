//! The overlay's SteamVR actions (`IVRInput`): one action manifest, as SteamVR takes one per process, with the walk
//! boost's sticks, what opens N4N0's menu (a press of whatever the person binds, or two taps on a touch sensor: the
//! defaults, [`MENU_OPEN`]), each hand's trigger (it presses where that index finger points
//! on N4N0's screen, and where its laser points on the floating panel), each hand's touchpad and stick for the avatar
//! (contracts `Docs/n4n0-input.md`), each hand's laser (its controller's tip) and grip for the floating panel, and their
//! default bindings for each kind of controller (people change them in SteamVR's controller settings, under Orbiters).
//! SteamVR gives the same inputs to VRChat as well. Every set is read once a frame, together.

use orbiters_n4n0::input::{Pad, Pads};
use orbiters_openvr::input::{ActionHandle, ActionSetHandle};
use orbiters_openvr::{self as vr};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Where the action manifest and its default bindings are written for SteamVR.
fn manifest_dir() -> PathBuf {
    orbiters_core::paths::local_data_root().join("SteamVR")
}

const ACTIONS: &str = r#"{
  "default_bindings": [
    { "controller_type": "knuckles", "binding_url": "bindings_knuckles.json" },
    { "controller_type": "oculus_touch", "binding_url": "bindings_oculus_touch.json" },
    { "controller_type": "vive_controller", "binding_url": "bindings_vive_controller.json" },
    { "controller_type": "holographic_controller", "binding_url": "bindings_holographic_controller.json" },
    { "controller_type": "hpmotioncontroller", "binding_url": "bindings_hpmotioncontroller.json" },
    { "controller_type": "vive_cosmos_controller", "binding_url": "bindings_vive_cosmos_controller.json" },
    { "controller_type": "vive_focus3_controller", "binding_url": "bindings_vive_focus3_controller.json" },
    { "controller_type": "pico_controller", "binding_url": "bindings_pico_controller.json" },
    { "controller_type": "frame_controller", "binding_url": "bindings_frame_controller.json" }
  ],
  "actions": [
    { "name": "/actions/walk/in/move_left", "type": "vector2" },
    { "name": "/actions/walk/in/move_right", "type": "vector2" },
    { "name": "/actions/menu/in/open", "type": "boolean" },
    { "name": "/actions/menu/in/open_tap", "type": "boolean" },
    { "name": "/actions/menu/in/press_left", "type": "boolean" },
    { "name": "/actions/menu/in/press_right", "type": "boolean" },
    { "name": "/actions/avatar/in/touchpad_left", "type": "vector2" },
    { "name": "/actions/avatar/in/touchpad_left_touch", "type": "boolean" },
    { "name": "/actions/avatar/in/touchpad_left_click", "type": "boolean" },
    { "name": "/actions/avatar/in/touchpad_left_force", "type": "vector1" },
    { "name": "/actions/avatar/in/touchpad_right", "type": "vector2" },
    { "name": "/actions/avatar/in/touchpad_right_touch", "type": "boolean" },
    { "name": "/actions/avatar/in/touchpad_right_click", "type": "boolean" },
    { "name": "/actions/avatar/in/touchpad_right_force", "type": "vector1" },
    { "name": "/actions/avatar/in/stick_left", "type": "vector2" },
    { "name": "/actions/avatar/in/stick_left_touch", "type": "boolean" },
    { "name": "/actions/avatar/in/stick_left_click", "type": "boolean" },
    { "name": "/actions/avatar/in/stick_right", "type": "vector2" },
    { "name": "/actions/avatar/in/stick_right_touch", "type": "boolean" },
    { "name": "/actions/avatar/in/stick_right_click", "type": "boolean" },
    { "name": "/actions/panel/in/laser_left", "type": "pose" },
    { "name": "/actions/panel/in/laser_right", "type": "pose" },
    { "name": "/actions/panel/in/grab_left", "type": "boolean" },
    { "name": "/actions/panel/in/grab_right", "type": "boolean" }
  ],
  "action_sets": [ { "name": "/actions/walk", "usage": "leftright" }, { "name": "/actions/menu", "usage": "leftright" }, { "name": "/actions/avatar", "usage": "leftright" },
    { "name": "/actions/panel", "usage": "leftright" } ],
  "localization": [ { "language_tag": "en_US", "/actions/walk": "Walk boost",
    "/actions/walk/in/move_left": "Walking stick (left hand)", "/actions/walk/in/move_right": "Walking stick (right hand)",
    "/actions/menu": "N4N0's menu", "/actions/menu/in/open": "Open N4N0's menu (a press)",
    "/actions/menu/in/open_tap": "Open N4N0's menu (two quick taps on a touch sensor)",
    "/actions/menu/in/press_left": "Press on N4N0's screen (left hand)", "/actions/menu/in/press_right": "Press on N4N0's screen (right hand)",
    "/actions/avatar": "Avatar input",
    "/actions/avatar/in/touchpad_left": "Touchpad (left hand)", "/actions/avatar/in/touchpad_left_touch": "Touchpad touched (left hand)",
    "/actions/avatar/in/touchpad_left_click": "Touchpad pressed (left hand)", "/actions/avatar/in/touchpad_left_force": "Touchpad force (left hand)",
    "/actions/avatar/in/touchpad_right": "Touchpad (right hand)", "/actions/avatar/in/touchpad_right_touch": "Touchpad touched (right hand)",
    "/actions/avatar/in/touchpad_right_click": "Touchpad pressed (right hand)", "/actions/avatar/in/touchpad_right_force": "Touchpad force (right hand)",
    "/actions/avatar/in/stick_left": "Stick (left hand)", "/actions/avatar/in/stick_left_touch": "Stick touched (left hand)",
    "/actions/avatar/in/stick_left_click": "Stick pressed (left hand)",
    "/actions/avatar/in/stick_right": "Stick (right hand)", "/actions/avatar/in/stick_right_touch": "Stick touched (right hand)",
    "/actions/avatar/in/stick_right_click": "Stick pressed (right hand)",
    "/actions/panel": "Orbiters panel",
    "/actions/panel/in/laser_left": "Laser (left hand)", "/actions/panel/in/laser_right": "Laser (right hand)",
    "/actions/panel/in/grab_left": "Grab the Orbiters panel (left hand)", "/actions/panel/in/grab_right": "Grab the Orbiters panel (right hand)" } ]
}"#;

/// How each controller opens N4N0's menu by default (its left hand), never with a stick's click: clicking a stick
/// wears its mechanism and makes it drift. Two quick taps on a touch sensor that is no stick (an Index's or a WMR
/// controller's trackpad, a Touch or Steam Frame thumbrest) cost nothing and are not made by resting a thumb there;
/// SteamVR's "double" press of the X button on controllers with neither (X is little used by VRChat); a Vive wand's
/// trackpad, its only pad, pressed twice, as before (a touchpad has no stick to drift). People bind whatever they like
/// in SteamVR's settings: one press of anything bound to `open` opens it, and a touch sensor bound to `open_tap` opens
/// it with two taps. (A Touch thumbrest has no click in SteamVR: its profile says `"click": false`.)
pub const MENU_OPEN: [(&str, &str, MenuOpen); 9] = [
    ("knuckles", "trackpad", MenuOpen::Taps),
    ("oculus_touch", "thumbrest", MenuOpen::Taps),
    ("vive_controller", "trackpad", MenuOpen::Double),
    ("holographic_controller", "trackpad", MenuOpen::Taps),
    ("hpmotioncontroller", "x", MenuOpen::Double),
    ("vive_cosmos_controller", "x", MenuOpen::Double),
    ("vive_focus3_controller", "x", MenuOpen::Double),
    ("pico_controller", "x", MenuOpen::Double),
    ("frame_controller", "thumbrest", MenuOpen::Taps),
];

/// How a default binding opens N4N0's menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuOpen {
    /// Two quick taps on the input's touch sensor (`open_tap`, [`Taps`]).
    Taps,
    /// SteamVR's double press of the input's click (`open`).
    Double,
}

/// The binding source that opens N4N0's menu on `controller`.
fn menu_source(controller: &str) -> String {
    let (input, how) = MENU_OPEN.iter().find(|(c, _, _)| *c == controller).map_or(("joystick", MenuOpen::Double), |&(_, i, h)| (i, h));
    let (from, action) = match how {
        MenuOpen::Taps => ("touch", "open_tap"),
        MenuOpen::Double => ("double", "open"),
    };
    format!(r#"{{ "path": "/user/hand/left/input/{input}", "mode": "button", "inputs": {{ "{from}": {{ "output": "/actions/menu/in/{action}" }} }} }}"#)
}

/// Default bindings: each hand's stick (a Vive wand's trackpad) to its move action, [`MENU_OPEN`] to N4N0's menu, each
/// hand's trigger click to its press, each hand's touchpad and stick, where the controller has them, to the avatar's
/// input, and each hand's tip and grip to the floating panel's laser and grab.
fn bindings(controller: &str, input: &str, mode: &str) -> String {
    let source = |hand: &str| {
        format!(
            r#"{{ "path": "/user/hand/{hand}/input/{input}", "mode": "{mode}", "inputs": {{ "position": {{ "output": "/actions/walk/in/move_{hand}" }} }} }}"#
        )
    };
    let menu = menu_source(controller);
    let opens = match MENU_OPEN.iter().find(|(c, _, _)| *c == controller) {
        Some((_, input, MenuOpen::Taps)) => format!("two quick taps on the left {input} open"),
        Some((_, input, MenuOpen::Double)) => format!("a double press of the left {} opens", if *input == "x" { "X button" } else { input }),
        None => "a double press of the left stick opens".to_string(),
    };
    let press = |hand: &str| {
        format!(r#"{{ "path": "/user/hand/{hand}/input/trigger", "mode": "trigger", "inputs": {{ "click": {{ "output": "/actions/menu/in/press_{hand}" }} }} }}"#)
    };
    let avatar = avatar_sources(controller).join(", ");
    let panel = panel_bindings(controller);
    format!(
        r#"{{ "controller_type": "{controller}", "name": "Orbiters", "description": "Walk boost's sticks; {opens} N4N0's menu; the triggers press on N4N0's screen and the Orbiters panel; the touchpads and sticks reach the avatar; the grips move the Orbiters panel",
  "bindings": {{ "/actions/walk": {{ "sources": [ {}, {} ] }}, "/actions/menu": {{ "sources": [ {menu}, {}, {} ] }}, "/actions/avatar": {{ "sources": [ {avatar} ] }}, "/actions/panel": {panel} }} }}"#,
        source("left"),
        source("right"),
        press("left"),
        press("right")
    )
}

/// The floating panel's set: each hand's laser from its controller's tip (where SteamVR's own laser starts, pointing
/// as it points) and its grip as each kind of controller has one: an Index's squeezed (its grab), a wand's button, the
/// others' pulled past their click.
fn panel_bindings(controller: &str) -> String {
    let (mode, input) = match controller {
        "knuckles" => ("grab", "grab"),
        "vive_controller" => ("button", "click"),
        _ => ("trigger", "click"),
    };
    let laser = |hand: &str| format!(r#"{{ "output": "/actions/panel/in/laser_{hand}", "path": "/user/hand/{hand}/pose/tip" }}"#);
    let grab =
        |hand: &str| format!(r#"{{ "path": "/user/hand/{hand}/input/grip", "mode": "{mode}", "inputs": {{ "{input}": {{ "output": "/actions/panel/in/grab_{hand}" }} }} }}"#);
    format!(r#"{{ "poses": [ {}, {} ], "sources": [ {}, {} ] }}"#, laser("left"), laser("right"), grab("left"), grab("right"))
}

/// The controllers' touchpads (with a force sensor or not) and sticks (their input path) for the avatar's input; the
/// controllers [`CONTROLLERS`] lists, in its order.
const PADS: [(&str, Option<bool>, Option<&str>); 9] = [
    ("knuckles", Some(true), Some("thumbstick")),
    ("oculus_touch", None, Some("joystick")),
    ("vive_controller", Some(false), None),
    ("holographic_controller", Some(false), Some("joystick")),
    ("hpmotioncontroller", None, Some("joystick")),
    ("vive_cosmos_controller", None, Some("joystick")),
    ("vive_focus3_controller", None, Some("joystick")),
    ("pico_controller", None, Some("joystick")),
    ("frame_controller", None, Some("thumbstick")),
];

/// The avatar input's binding sources for `controller`: each hand's touchpad (position, touch, click, and its force
/// sensor) and stick (position, touch, click).
fn avatar_sources(controller: &str) -> Vec<String> {
    let Some(&(_, touchpad, stick)) = PADS.iter().find(|(c, _, _)| *c == controller) else { return Vec::new() };
    let mut out = Vec::new();
    for hand in ["left", "right"] {
        if let Some(force) = touchpad {
            let a = format!("/actions/avatar/in/touchpad_{hand}");
            out.push(format!(
                r#"{{ "path": "/user/hand/{hand}/input/trackpad", "mode": "trackpad", "inputs": {{ "position": {{ "output": "{a}" }}, "touch": {{ "output": "{a}_touch" }}, "click": {{ "output": "{a}_click" }} }} }}"#
            ));
            if force {
                out.push(format!(r#"{{ "path": "/user/hand/{hand}/input/trackpad", "mode": "force_sensor", "inputs": {{ "force": {{ "output": "{a}_force" }} }} }}"#));
            }
        }
        if let Some(input) = stick {
            let a = format!("/actions/avatar/in/stick_{hand}");
            out.push(format!(
                r#"{{ "path": "/user/hand/{hand}/input/{input}", "mode": "joystick", "inputs": {{ "position": {{ "output": "{a}" }}, "touch": {{ "output": "{a}_touch" }}, "click": {{ "output": "{a}_click" }} }} }}"#
            ));
        }
    }
    out
}

/// The controllers with default bindings, the stick's input path and its mode. VIVE Focus 3 and Pico controllers as
/// Steam Link and Pico's streaming drivers describe them, the Steam Frame's as its driver does.
const CONTROLLERS: [(&str, &str, &str); 9] = [
    ("knuckles", "thumbstick", "joystick"),
    ("oculus_touch", "joystick", "joystick"),
    ("vive_controller", "trackpad", "trackpad"),
    ("holographic_controller", "joystick", "joystick"),
    ("hpmotioncontroller", "joystick", "joystick"),
    ("vive_cosmos_controller", "joystick", "joystick"),
    ("vive_focus3_controller", "joystick", "joystick"),
    ("pico_controller", "joystick", "joystick"),
    ("frame_controller", "thumbstick", "joystick"),
];

pub(super) fn write_manifest(dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let files = CONTROLLERS;
    for (controller, input, mode) in files {
        std::fs::write(dir.join(format!("bindings_{controller}.json")), bindings(controller, input, mode))?;
    }
    let path = dir.join("actions.json");
    std::fs::write(&path, ACTIONS)?;
    Ok(path)
}

/// One hand's touchpad (position, touch, click, force) and stick (position, touch, click) for the avatar's input.
#[derive(Clone, Copy, Debug)]
struct HandPads {
    touchpad: [ActionHandle; 4],
    stick: [ActionHandle; 3],
}

/// The walk boost's actions.
#[derive(Clone, Copy, Debug)]
pub struct Walk {
    pub set: ActionSetHandle,
    pub left: ActionHandle,
    pub right: ActionHandle,
}

pub struct Actions {
    pub walk: Option<Walk>,
    /// The menu's set, its press (`open`) and its taps (`open_tap`).
    menu: Option<(ActionSetHandle, ActionHandle, ActionHandle)>,
    taps: Taps,
    /// Each hand's trigger (left, right) and how it was at the last read.
    presses: Option<[ActionHandle; 2]>,
    press_down: [bool; 2],
    /// Each hand's touchpad and stick for the avatar's input (left, right).
    pads: Option<[HandPads; 2]>,
    /// Each hand's laser (its tip's pose) and grip for the floating panel (left, right).
    panel: Option<([ActionHandle; 2], [ActionHandle; 2])>,
    sets: Vec<ActionSetHandle>,
    /// The manifest could not be loaded: why (said once).
    pub failed: Option<String>,
    menu_down: bool,
}

impl Actions {
    pub fn load(steamvr: &vr::Client) -> Self {
        let loaded = (|| -> Result<_, String> {
            let input = steamvr.input().map_err(|e| e.to_string())?;
            let path = write_manifest(&manifest_dir()).map_err(|e| e.to_string())?;
            let fail = |c: i32| format!("SteamVR input error {c}");
            input.set_action_manifest(&path).map_err(fail)?;
            let walk = Walk {
                set: input.action_set("/actions/walk").map_err(fail)?,
                left: input.action("/actions/walk/in/move_left").map_err(fail)?,
                right: input.action("/actions/walk/in/move_right").map_err(fail)?,
            };
            let menu = (
                input.action_set("/actions/menu").map_err(fail)?,
                input.action("/actions/menu/in/open").map_err(fail)?,
                input.action("/actions/menu/in/open_tap").map_err(fail)?,
            );
            let presses = [input.action("/actions/menu/in/press_left").map_err(fail)?, input.action("/actions/menu/in/press_right").map_err(fail)?];
            let avatar = input.action_set("/actions/avatar").map_err(fail)?;
            let hand = |hand: &str| -> Result<HandPads, String> {
                let a = |name: String| input.action(&format!("/actions/avatar/in/{name}")).map_err(fail);
                Ok(HandPads {
                    touchpad: [a(format!("touchpad_{hand}"))?, a(format!("touchpad_{hand}_touch"))?, a(format!("touchpad_{hand}_click"))?, a(format!("touchpad_{hand}_force"))?],
                    stick: [a(format!("stick_{hand}"))?, a(format!("stick_{hand}_touch"))?, a(format!("stick_{hand}_click"))?],
                })
            };
            let pads = [hand("left")?, hand("right")?];
            let panel = (
                input.action_set("/actions/panel").map_err(fail)?,
                [input.action("/actions/panel/in/laser_left").map_err(fail)?, input.action("/actions/panel/in/laser_right").map_err(fail)?],
                [input.action("/actions/panel/in/grab_left").map_err(fail)?, input.action("/actions/panel/in/grab_right").map_err(fail)?],
            );
            Ok((walk, menu, presses, (avatar, pads), panel))
        })();
        match loaded {
            Ok((walk, menu, presses, (avatar, pads), (panel, lasers, grabs))) => Self {
                walk: Some(walk),
                menu: Some(menu),
                taps: Taps::default(),
                presses: Some(presses),
                press_down: [false; 2],
                pads: Some(pads),
                panel: Some((lasers, grabs)),
                sets: vec![walk.set, menu.0, avatar, panel],
                failed: None,
                menu_down: false,
            },
            Err(e) => {
                eprintln!("SteamVR actions unavailable: {e}");
                Self { walk: None, menu: None, taps: Taps::default(), presses: None, press_down: [false; 2], pads: None, panel: None, sets: Vec::new(), failed: Some(e), menu_down: false }
            }
        }
    }

    /// Reads every set for this frame (before reading any action).
    pub fn update(&self, steamvr: &vr::Client) -> bool {
        if self.sets.is_empty() {
            return false;
        }
        steamvr.input().is_ok_and(|input| input.update_sets(&self.sets).is_ok())
    }

    /// What opens N4N0's menu was done since the last read: `open` pressed (its rising edge; `changed` also catches a
    /// press that went up and down again between two reads), or two quick taps on what `open_tap` is bound to.
    pub fn menu_pressed(&mut self, steamvr: &vr::Client) -> bool {
        let Some((_, open, tap)) = self.menu else { return false };
        let Ok(input) = steamvr.input() else { return false };
        let tapped = input.digital(tap).is_ok_and(|d| self.taps.step(d.active && d.state, Instant::now()));
        let Ok(data) = input.digital(open) else { return tapped };
        let down = data.active && data.state;
        let pressed = down && (!self.menu_down || data.changed);
        self.menu_down = down;
        pressed || tapped
    }

    /// Each hand's touchpad and stick as this frame's read says (contracts `Docs/n4n0-input.md`): nothing when an action
    /// could not be read. A position is 0 while untouched; a touchpad without a force sensor presses with 1 while
    /// clicked; a stick without a touch sensor counts as touched off centre or clicked.
    pub fn pads(&self, steamvr: &vr::Client) -> Option<Pads> {
        let (actions, input) = (self.pads?, steamvr.input().ok()?);
        let mut pads = Pads::default();
        for (hand, a) in actions.iter().enumerate() {
            let analog = |h: ActionHandle| input.analog(h).ok().filter(|d| d.active);
            let digital = |h: ActionHandle| input.digital(h).ok().map(|d| d.active && d.state);
            let position = analog(a.touchpad[0]);
            let (touch, click) = (digital(a.touchpad[1])?, digital(a.touchpad[2])?);
            let force = input.analog(a.touchpad[3]).ok()?;
            pads.touchpads[hand] = pad(position.map(|d| (d.x, d.y)), touch, click, force.active.then_some(force.x), false);
            let position = analog(a.stick[0]);
            let (touch, click) = (digital(a.stick[1])?, digital(a.stick[2])?);
            pads.sticks[hand] = pad(position.map(|d| (d.x, d.y)), touch, click, None, true);
        }
        Some(pads)
    }

    /// Each hand's trigger that changed since the last read (left, right): `Some(down)`.
    pub fn presses(&mut self, steamvr: &vr::Client) -> [Option<bool>; 2] {
        let mut changed = [None; 2];
        let (Some(actions), Ok(input)) = (self.presses, steamvr.input()) else { return changed };
        for (hand, action) in actions.into_iter().enumerate() {
            let Ok(data) = input.digital(action) else { continue };
            let down = data.active && data.state;
            if down != self.press_down[hand] {
                self.press_down[hand] = down;
                changed[hand] = Some(down);
            }
        }
        changed
    }

    /// Each hand's trigger held (left, right), as [`Actions::presses`] last read it.
    pub fn triggers(&self) -> [bool; 2] {
        self.press_down
    }

    /// Each hand's laser (left, right): its controller's tip in the standing play space, its −Z the way it points, as
    /// SteamVR predicts it `seconds` from now. `None` for a hand whose tip is not bound or not tracked.
    pub fn lasers(&self, steamvr: &vr::Client, seconds: f32) -> [Option<vr::Matrix34>; 2] {
        let (Some((lasers, _)), Ok(input)) = (self.panel, steamvr.input()) else { return [None; 2] };
        lasers.map(|action| {
            let data = input.pose(action, vr::Universe::Standing, seconds).ok()?;
            (data.active && data.pose.pose_is_valid).then_some(data.pose.device_to_absolute)
        })
    }

    /// Each hand's grip held (left, right).
    pub fn grabs(&self, steamvr: &vr::Client) -> [bool; 2] {
        let (Some((_, grabs)), Ok(input)) = (self.panel, steamvr.input()) else { return [false; 2] };
        grabs.map(|action| input.digital(action).is_ok_and(|d| d.active && d.state))
    }
}

/// Two quick taps on a touch sensor (a thumbrest, a trackpad touched without pressing it): what opens N4N0's menu
/// by default. A tap is a touch from [`TAP_SHORTEST`] to [`TAP_LONGEST`] long; the second must start within
/// [`TAP_GAP`] of the first's end, and opens the menu as it starts. A thumb resting on the sensor (a longer touch)
/// is no tap, and a sensor flickering at the edge of a touch (off for less than [`TAP_FLICKER`]) is one touch.
#[derive(Clone, Copy, Debug, Default)]
pub struct Taps {
    /// The touch under way, since when.
    touch: Option<Instant>,
    /// The touch just let go (when, and what it lasted): a tap once it stays off past a flicker.
    off: Option<(Instant, Duration)>,
    /// When the last tap ended (the first of two).
    tapped: Option<Instant>,
    /// The touch under way opened the menu: its end is no tap.
    fired: bool,
}

pub const TAP_SHORTEST: Duration = Duration::from_millis(25);
pub const TAP_LONGEST: Duration = Duration::from_millis(350);
pub const TAP_GAP: Duration = Duration::from_millis(400);
pub const TAP_FLICKER: Duration = Duration::from_millis(40);

impl Taps {
    /// The sensor `touched` at `now` (read once a turn); true as the second tap starts.
    pub fn step(&mut self, touched: bool, now: Instant) -> bool {
        if let Some((off, lasted)) = self.off {
            if now.duration_since(off) < TAP_FLICKER {
                if touched {
                    // Back after a flicker: the same touch goes on.
                    self.off = None;
                    self.touch = Some(off.checked_sub(lasted).unwrap_or(off));
                }
                return false;
            }
            self.off = None;
            self.tapped = (TAP_SHORTEST..=TAP_LONGEST).contains(&lasted).then_some(off);
        }
        match (touched, self.touch) {
            (true, None) => {
                self.touch = Some(now);
                // The second tap, within the gap after a first one.
                self.fired = self.tapped.take().is_some_and(|t| now.duration_since(t) <= TAP_GAP);
                self.fired
            }
            (false, Some(since)) => {
                self.touch = None;
                if !std::mem::take(&mut self.fired) {
                    self.off = Some((now, now.duration_since(since)));
                }
                false
            }
            _ => false,
        }
    }
}

/// A touchpad or stick from its actions: `position` when bound, `force` when a force sensor is; a stick (`stick`)
/// without a touch sensor counts as touched off centre.
fn pad(position: Option<(f32, f32)>, touch: bool, click: bool, force: Option<f32>, stick: bool) -> Pad {
    use orbiters_contracts::n4n0::input::STICK_TOUCHED;
    let (x, y) = position.unwrap_or_default();
    let (x, y) = (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0));
    let touch = touch || click || (stick && x.hypot(y) >= STICK_TOUCHED);
    // A touchpad untouched lies at its centre (SteamVR keeps the last position of some).
    let (x, y) = if touch || stick { (x, y) } else { (0.0, 0.0) };
    let force = if stick { 0.0 } else { force.map_or(if click { 1.0 } else { 0.0 }, |f| f.clamp(0.0, 1.0)) };
    Pad { x, y, touch, click, force }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_are_json() {
        let dir = std::env::temp_dir().join(format!("orbiters-actions-{}", std::process::id()));
        let path = write_manifest(&dir).unwrap();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            if let Some(bindings) = value.get("bindings") {
                assert!(bindings.get("/actions/menu").is_some(), "every controller opens the menu");
            }
        }
        let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(manifest["actions"].as_array().unwrap().len(), 24);
        // Every controller has the floating panel's lasers (each hand's tip) and grips.
        for (controller, _, _) in CONTROLLERS {
            let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join(format!("bindings_{controller}.json"))).unwrap()).unwrap();
            let panel = &value["bindings"]["/actions/panel"];
            assert_eq!(panel["poses"].as_array().map(Vec::len), Some(2), "{controller}");
            assert_eq!(panel["poses"][1]["path"], "/user/hand/right/pose/tip");
            assert_eq!(panel["sources"].as_array().map(Vec::len), Some(2), "{controller}");
            assert_eq!(panel["sources"][0]["path"], "/user/hand/left/input/grip");
        }
        // Every avatar input action is bound on the controllers that have its input, and only those.
        let index: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("bindings_knuckles.json")).unwrap()).unwrap();
        assert_eq!(index["bindings"]["/actions/avatar"]["sources"].as_array().unwrap().len(), 6, "trackpad, its force, stick, both hands");
        let wand: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("bindings_vive_controller.json")).unwrap()).unwrap();
        assert_eq!(wand["bindings"]["/actions/avatar"]["sources"].as_array().unwrap().len(), 2);
        let touch: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("bindings_oculus_touch.json")).unwrap()).unwrap();
        assert!(touch.to_string().contains("/actions/avatar/in/stick_right_click") && !touch.to_string().contains("/actions/avatar/in/touchpad"));
        let actions: Vec<&str> = manifest["actions"].as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap()).collect();
        for (controller, _, _) in CONTROLLERS {
            let text = std::fs::read_to_string(dir.join(format!("bindings_{controller}.json"))).unwrap();
            for output in text.split("\"output\": \"").skip(1) {
                let name = &output[..output.find('"').unwrap()];
                assert!(actions.contains(&name), "{controller} binds {name}, which the manifest lacks");
            }
        }
        assert_eq!(PADS.map(|(c, _, _)| c), CONTROLLERS.map(|(c, _, _)| c));
        // Every controller listed has its file, and every file is listed.
        let listed: Vec<&str> = manifest["default_bindings"].as_array().unwrap().iter().map(|b| b["controller_type"].as_str().unwrap()).collect();
        assert_eq!(listed, CONTROLLERS.map(|(c, _, _)| c));
        for b in manifest["default_bindings"].as_array().unwrap() {
            assert!(dir.join(b["binding_url"].as_str().unwrap()).is_file());
        }
        std::fs::remove_dir_all(dir).ok();
    }

    /// Each controller's menu input as its SteamVR input profile describes it (`drivers/*/resources/input/*_profile.json`:
    /// the Index, Touch, wand, Focus 3, Pico and Steam Frame ones read from SteamVR here; the WMR, HP Reverb G2 and
    /// Cosmos ones as their drivers publish them): (its touch sensor, its click).
    const PROFILES: [(&str, &str, bool, bool); 9] = [
        ("knuckles", "trackpad", true, false),
        ("oculus_touch", "thumbrest", true, false),
        ("vive_controller", "trackpad", true, true),
        ("holographic_controller", "trackpad", true, true),
        ("hpmotioncontroller", "x", false, true),
        ("vive_cosmos_controller", "x", false, true),
        ("vive_focus3_controller", "x", true, true),
        ("pico_controller", "x", true, true),
        ("frame_controller", "thumbrest", true, false),
    ];

    #[test]
    fn every_controller_opens_the_menu_without_clicking_a_stick() {
        let dir = std::env::temp_dir().join(format!("orbiters-actions-menu-{}", std::process::id()));
        write_manifest(&dir).unwrap();
        assert_eq!(MENU_OPEN.map(|(c, _, _)| c), CONTROLLERS.map(|(c, _, _)| c));
        for ((controller, input, how), (profile, component, touch, click)) in MENU_OPEN.into_iter().zip(PROFILES) {
            assert_eq!((controller, input), (profile, component));
            let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join(format!("bindings_{controller}.json"))).unwrap()).unwrap();
            let opens: Vec<&serde_json::Value> = value["bindings"]["/actions/menu"]["sources"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|s| s.to_string().contains("/actions/menu/in/open"))
                .collect();
            assert_eq!(opens.len(), 1, "{controller}: one way to open the menu");
            let source = opens[0];
            let path = source["path"].as_str().unwrap();
            assert_eq!(path, format!("/user/hand/left/input/{input}"), "{controller}");
            assert!(!path.ends_with("joystick") && !path.ends_with("thumbstick"), "{controller}: never a stick's click");
            assert_eq!(source["mode"], "button");
            match how {
                MenuOpen::Taps => {
                    assert!(touch, "{controller}: {input} has a touch sensor");
                    assert_eq!(source["inputs"]["touch"]["output"], "/actions/menu/in/open_tap");
                }
                MenuOpen::Double => {
                    assert!(click, "{controller}: {input} has a click");
                    assert_eq!(source["inputs"]["double"]["output"], "/actions/menu/in/open");
                }
            }
            assert!(value["description"].as_str().unwrap().contains(&format!("left {}", if input == "x" { "X button" } else { input })), "{controller}");
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn two_quick_taps_open_the_menu_and_a_resting_thumb_does_not() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        // Sampled every 11 ms, as the overlay's turns read it: `touches` are (from, to) in ms.
        let run = |touches: &[(u64, u64)], until: u64| -> Vec<u64> {
            let mut taps = Taps::default();
            (0..=until / 11).map(|i| i * 11).filter(|&t| taps.step(touches.iter().any(|&(a, b)| (a..b).contains(&t)), ms(t))).collect()
        };
        assert_eq!(run(&[(0, 120), (300, 420)], 800), vec![308], "two taps: open as the second starts");
        assert!(run(&[(0, 2000)], 2500).is_empty(), "a thumb resting");
        assert!(run(&[(0, 900), (1000, 1100)], 1500).is_empty(), "a rest, then a tap");
        assert!(run(&[(0, 100), (900, 1000)], 1500).is_empty(), "two taps too far apart");
        assert!(run(&[(0, 100), (300, 1500)], 1600).len() == 1, "a tap, then resting there: opens once");
        // The sensor flickering off for a moment mid-rest is one touch, so no taps.
        assert!(run(&[(0, 600), (620, 1500)], 1600).is_empty());
        // Three taps open once (the third starts no new pair); four open twice.
        assert_eq!(run(&[(0, 100), (250, 350), (500, 600)], 900).len(), 1);
        assert_eq!(run(&[(0, 100), (250, 350), (500, 600), (750, 850)], 1100).len(), 2);
    }

    #[test]
    fn pads_rest_at_their_centre_and_fill_in_missing_sensors() {
        // A touchpad let go keeps no position; one without force presses with 1 while clicked.
        assert_eq!(pad(Some((0.4, -0.2)), false, false, None, false), Pad::default());
        assert_eq!(pad(Some((0.4, -0.2)), false, true, None, false), Pad { x: 0.4, y: -0.2, touch: true, click: true, force: 1.0 });
        assert_eq!(pad(Some((0.4, 0.0)), true, false, Some(0.3), false).force, 0.3);
        // A stick without a touch sensor: touched off centre.
        assert!(pad(Some((0.0, 0.5)), false, false, None, true).touch);
        assert!(!pad(Some((0.05, 0.0)), false, false, None, true).touch);
        assert_eq!(pad(None, false, false, None, true), Pad::default());
        assert_eq!(pad(Some((2.0, -3.0)), true, false, None, true).x, 1.0);
    }
}
