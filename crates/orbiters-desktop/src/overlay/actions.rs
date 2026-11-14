//! The overlay's SteamVR actions (`IVRInput`): one action manifest, as SteamVR takes one per process, with the walk
//! boost's sticks, what opens N4N0's menu (a press of whatever the person binds, or two taps on a touch sensor: the
//! defaults, [`MENU_OPEN`]), each hand's trigger (it presses where that index finger points
//! on N4N0's screen, and where its laser points on the floating panel), each hand's touchpad and stick for the avatar
//! (contracts `Docs/n4n0-input.md`), each hand's laser (its controller's tip) and grip for the floating panel, and their
//! default bindings for each kind of controller (people change them in SteamVR's controller settings, under Orbiters).
//! SteamVR gives the same inputs to VRChat as well. Every set is read once a frame, together.

use orbiters_i18n::tr;
use orbiters_n4n0::input::{Pad, Pads};
use orbiters_openvr::input::{ActionHandle, ActionSetHandle, ActiveSet, OVERLAY_GLOBAL_PRIORITY_MIN};
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
    { "name": "/actions/panel/in/grab_right", "type": "boolean" },
    { "name": "/actions/orbit/in/menu_left", "type": "boolean" },
    { "name": "/actions/orbit/in/menu_right", "type": "boolean" },
    { "name": "/actions/orbit_block/in/trigger_left", "type": "boolean" },
    { "name": "/actions/orbit_block/in/trigger_right", "type": "boolean" },
    { "name": "/actions/orbit_block/in/pull_left", "type": "vector1" },
    { "name": "/actions/orbit_block/in/pull_right", "type": "vector1" },
    { "name": "/actions/grip/in/value_left", "type": "vector1" },
    { "name": "/actions/grip/in/value_right", "type": "vector1" }
  ],
  "action_sets": [ { "name": "/actions/walk", "usage": "leftright" }, { "name": "/actions/menu", "usage": "leftright" }, { "name": "/actions/avatar", "usage": "leftright" },
    { "name": "/actions/panel", "usage": "leftright" }, { "name": "/actions/orbit", "usage": "leftright" }, { "name": "/actions/orbit_block", "usage": "leftright" },
    { "name": "/actions/grip", "usage": "leftright" } ],
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
    "/actions/panel/in/grab_left": "Grab the Orbiters panel (left hand)", "/actions/panel/in/grab_right": "Grab the Orbiters panel (right hand)",
    "/actions/orbit": "Panels around VRChat's menus",
    "/actions/orbit/in/menu_left": "VRChat's menu button (left hand)", "/actions/orbit/in/menu_right": "VRChat's menu button (right hand)",
    "/actions/orbit_block": "Trigger on the panels (taken from VRChat while pointing at them)",
    "/actions/orbit_block/in/trigger_left": "Press on a panel (left hand)", "/actions/orbit_block/in/trigger_right": "Press on a panel (right hand)",
    "/actions/orbit_block/in/pull_left": "Trigger's pull (left hand)", "/actions/orbit_block/in/pull_right": "Trigger's pull (right hand)",
    "/actions/grip": "How far each grip is pulled (to grip VRChat's camera as VRChat does)",
    "/actions/grip/in/value_left": "Grip's pull (left hand)", "/actions/grip/in/value_right": "Grip's pull (right hand)" } ]
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
        Some((_, input, MenuOpen::Taps)) => format!("two quick taps on the left {input} open"), // i18n: not shown (a line of the bindings file SteamVR reads)
        Some((_, input, MenuOpen::Double)) => format!("a double press of the left {} opens", if *input == "x" { "X button" } else { input }), // i18n: not shown (the bindings file)
        None => "a double press of the left stick opens".to_string(), // i18n: not shown (the bindings file)
    };
    let press = |hand: &str| {
        format!(r#"{{ "path": "/user/hand/{hand}/input/trigger", "mode": "trigger", "inputs": {{ "click": {{ "output": "/actions/menu/in/press_{hand}" }} }} }}"#)
    };
    let avatar = avatar_sources(controller).join(", ");
    let panel = panel_bindings(controller);
    let orbit = orbit_bindings(controller);
    let orbit_block = orbit_block_bindings();
    let grip = grip_bindings();
    format!(
        r#"{{ "controller_type": "{controller}", "name": "Orbiters", "description": "Walk boost's sticks; {opens} N4N0's menu; the triggers press on N4N0's screen and the Orbiters panel; the touchpads and sticks reach the avatar; the grips move the Orbiters panel",
  "bindings": {{ "/actions/walk": {{ "sources": [ {}, {} ] }}, "/actions/menu": {{ "sources": [ {menu}, {}, {} ] }}, "/actions/avatar": {{ "sources": [ {avatar} ] }}, "/actions/panel": {panel}, "/actions/orbit": {orbit}, "/actions/orbit_block": {orbit_block}, "/actions/grip": {grip} }} }}"#,
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

/// VRChat's menu button as its own default bindings have it (`VRChat_Data/StreamingAssets/SteamVR/bindings_*.json`, its `Menu` action): what the panels' guess at
/// its menus follows. Read alongside VRChat, taking nothing from it.
fn menu_button(controller: &str, hand: &str) -> &'static str {
    match controller {
        "knuckles" => "b",
        "vive_controller" | "holographic_controller" => "application_menu",
        _ => {
            if hand == "left" {
                "y"
            } else {
                "b"
            }
        }
    }
}

/// Each hand's menu button for the panels around VRChat's menus.
fn orbit_bindings(controller: &str) -> String {
    let source = |hand: &str| {
        let button = menu_button(controller, hand);
        format!(r#"{{ "path": "/user/hand/{hand}/input/{button}", "mode": "button", "inputs": {{ "click": {{ "output": "/actions/orbit/in/menu_{hand}" }} }} }}"#)
    };
    format!(r#"{{ "sources": [ {}, {} ] }}"#, source("left"), source("right"))
}

/// Each hand's trigger, whole: this set made active at SteamVR's overlay priority takes that source from VRChat (`overlay::orbit::block`).
fn orbit_block_bindings() -> String {
    let source = |hand: &str| {
        format!(r#"{{ "path": "/user/hand/{hand}/input/trigger", "mode": "trigger", "inputs": {{ "click": {{ "output": "/actions/orbit_block/in/trigger_{hand}" }}, "pull": {{ "output": "/actions/orbit_block/in/pull_{hand}" }} }} }}"#)
    };
    format!(r#"{{ "sources": [ {}, {} ] }}"#, source("left"), source("right"))
}

/// Each hand's grip, how far it is pulled (0 to 1): VRChat takes and lets go of its camera panel at its own thresholds of that pull
/// (`overlay::orbit::vrchat_grip`), which a click at SteamVR's thresholds does not follow. A grip with no pull (a wand's button) leaves it inactive.
fn grip_bindings() -> String {
    let source = |hand: &str| format!(r#"{{ "path": "/user/hand/{hand}/input/grip", "mode": "trigger", "inputs": {{ "pull": {{ "output": "/actions/grip/in/value_{hand}" }} }} }}"#);
    format!(r#"{{ "sources": [ {}, {} ] }}"#, source("left"), source("right"))
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

/// The application's key, under which SteamVR keeps its settings.
const APP_KEY_SECTION: &std::ffi::CStr = c"cc.orbiters.n4n0";

/// A binding saved for this application, which SteamVR's own autosave also makes, is used in place of the default ones, and knows nothing of the action sets added since: on 2026-11-07
/// Enzo's Touch controllers had one from two days before, so `/actions/orbit` was not bound and no panel ever opened in VR, with nothing to say so. So what a saved binding lacks is added to
/// a copy of it, and the copy becomes the saved one: what the person chose stays. One that cannot be read goes, and the defaults apply. Before the manifest is set, which is when it is read.
fn keep_saved_bindings_whole(steamvr: &vr::Client, dir: &Path) {
    use std::ffi::CString;
    let runtime = vr::runtime_dir();
    // Every section SteamVR may read for this process: the application's, and its program's name, by which SteamVR knows a connection it does not take for the application.
    let program = std::env::current_exe().ok().and_then(|exe| exe.file_name().map(|name| format!("system.generated.{}", name.to_string_lossy().to_lowercase())));
    let mut sections = vec![APP_KEY_SECTION.to_owned()];
    for name in steamvr.own_application_key().into_iter().chain(program) {
        if let Ok(name) = CString::new(name) {
            if !sections.contains(&name) {
                sections.push(name);
            }
        }
    }
    for section in &sections {
    let section: &std::ffi::CStr = section;
    let other = if section == APP_KEY_SECTION { String::new() } else { format!("{}_", section.to_string_lossy().replace(|c: char| !c.is_ascii_alphanumeric(), "_")) };
    for (controller, _, _) in CONTROLLERS {
        let key = |what: &str| CString::new(format!("{controller}_250820_{what}_steamvrinput")).expect("no NUL in these");
        let Some(url) = steamvr.setting_string(section, &key("CurrentURL")) else { continue };
        let saved = saved_binding_file(&url, runtime.as_deref()).and_then(|file| std::fs::read_to_string(file).ok());
        let Ok(ours) = std::fs::read_to_string(dir.join(format!("bindings_{controller}.json"))) else { continue };
        let Some(saved) = saved else {
            eprintln!("SteamVR's saved binding for {controller} cannot be read ({url}): the default one applies");
            steamvr.remove_setting(section, &key("CurrentURL"));
            steamvr.remove_setting(section, &key("AutosaveURL"));
            continue;
        };
        let Some(merged) = merged_binding(&saved, &ours) else { continue };
        let file = dir.join(format!("merged_bindings_{other}{controller}.json"));
        let Ok(()) = std::fs::write(&file, merged) else { continue };
        let url = format!("file:///{}", file.to_string_lossy().replace('\\', "/"));
        eprintln!("SteamVR's saved binding for {controller} lacked some of this application's actions: they were added ({url})");
        if let Ok(url) = CString::new(url) {
            steamvr.set_setting_string(section, &key("CurrentURL"), &url);
            steamvr.set_setting_string(section, &key("AutosaveURL"), &url);
        }
    }
    }
}

/// The file a saved binding's address stands for: a Workshop item's (the one file in its folder, under SteamVR's own Steam library) or a file's.
pub(super) fn saved_binding_file(url: &str, runtime: Option<&Path>) -> Option<PathBuf> {
    if let Some(item) = url.strip_prefix("vr-input-workshop://") {
        let library = runtime?.parent()?.parent()?;
        let folder = library.join("workshop").join("content").join("250820").join(item);
        return std::fs::read_dir(folder).ok()?.filter_map(Result::ok).map(|e| e.path()).find(|p| p.is_file());
    }
    let path = url.strip_prefix("file:///").or_else(|| url.strip_prefix("file://"))?;
    Some(PathBuf::from(path))
}

/// `saved` with the action sets of `ours` that it lacks, the rest as it is: `None` when it lacks none, or when either is not a binding.
fn merged_binding(saved: &str, ours: &str) -> Option<String> {
    let mut saved: serde_json::Value = serde_json::from_str(saved).ok()?;
    let ours: serde_json::Value = serde_json::from_str(ours).ok()?;
    let theirs = saved.get_mut("bindings")?.as_object_mut()?;
    let mut added = false;
    for (set, binding) in ours.get("bindings")?.as_object()? {
        if !theirs.contains_key(set) {
            theirs.insert(set.clone(), binding.clone());
            added = true;
        }
    }
    added.then(|| serde_json::to_string_pretty(&saved).ok()).flatten()
}

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
    /// VRChat's menu button of each hand, the set that takes the trigger from VRChat, each hand's device and trigger in it.
    orbit: Option<Orbit>,
    /// Each hand's grip pull (left, right).
    grip: Option<[ActionHandle; 2]>,
    /// Hands whose trigger is taken from VRChat from the next read on.
    blocked: [bool; 2],
    sets: Vec<ActionSetHandle>,
    /// The manifest could not be loaded: why (said once).
    pub failed: Option<Unavailable>,
    menu_down: bool,
}

/// Why SteamVR's input could not be set up. SteamVR's error stays a code and is put into words where it is drawn
/// ([`Unavailable::text`]), so that a change of language shows in the Walk boost card while the failure stays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// An `EVRInputError` of SteamVR.
    Input(i32),
    /// Something else said why (a file that could not be written, SteamVR's interface missing), in its own words.
    Said(String),
}

impl Unavailable {
    /// What a person reads, in the language now.
    pub fn text(&self) -> String {
        match self {
            Unavailable::Input(code) => tr!("SteamVR input error {code}", code = code),
            Unavailable::Said(said) => said.clone(),
        }
    }
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text())
    }
}

/// The panels around VRChat's menus' actions.
#[derive(Clone, Copy, Debug)]
struct Orbit {
    menu: [ActionHandle; 2],
    block: ActionSetHandle,
    hands: [orbiters_openvr::input::InputSourceHandle; 2],
    triggers: [ActionHandle; 2],
}

impl Actions {
    pub fn load(steamvr: &vr::Client) -> Self {
        let loaded = (|| -> Result<_, Unavailable> {
            let input = steamvr.input().map_err(|e| Unavailable::Said(e.to_string()))?;
            let path = write_manifest(&manifest_dir()).map_err(|e| Unavailable::Said(e.to_string()))?;
            keep_saved_bindings_whole(steamvr, &manifest_dir());
            let fail = Unavailable::Input;
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
            let hand = |hand: &str| -> Result<HandPads, Unavailable> {
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
            let orbit_set = input.action_set("/actions/orbit").map_err(fail)?;
            let orbit = Orbit {
                menu: [input.action("/actions/orbit/in/menu_left").map_err(fail)?, input.action("/actions/orbit/in/menu_right").map_err(fail)?],
                block: input.action_set("/actions/orbit_block").map_err(fail)?,
                hands: [input.input_source("/user/hand/left").map_err(fail)?, input.input_source("/user/hand/right").map_err(fail)?],
                triggers: [input.action("/actions/orbit_block/in/trigger_left").map_err(fail)?, input.action("/actions/orbit_block/in/trigger_right").map_err(fail)?],
            };
            let grip_set = input.action_set("/actions/grip").map_err(fail)?;
            let grip = [input.action("/actions/grip/in/value_left").map_err(fail)?, input.action("/actions/grip/in/value_right").map_err(fail)?];
            Ok((walk, menu, presses, (avatar, pads), panel, (orbit_set, orbit), (grip_set, grip)))
        })();
        match loaded {
            Ok((walk, menu, presses, (avatar, pads), (panel, lasers, grabs), (orbit_set, orbit), (grip_set, grip))) => Self {
                walk: Some(walk),
                menu: Some(menu),
                taps: Taps::default(),
                presses: Some(presses),
                press_down: [false; 2],
                pads: Some(pads),
                panel: Some((lasers, grabs)),
                orbit: Some(orbit),
                grip: Some(grip),
                blocked: [false; 2],
                sets: vec![walk.set, menu.0, avatar, panel, orbit_set, grip_set],
                failed: None,
                menu_down: false,
            },
            Err(e) => {
                eprintln!("SteamVR actions unavailable: {e}");
                Self { walk: None, menu: None, taps: Taps::default(), presses: None, press_down: [false; 2], pads: None, panel: None, orbit: None, grip: None, blocked: [false; 2], sets: Vec::new(), failed: Some(e), menu_down: false }
            }
        }
    }

    /// Reads every set for this frame (before reading any action).
    pub fn update(&self, steamvr: &vr::Client) -> bool {
        if self.sets.is_empty() {
            return false;
        }
        let Ok(input) = steamvr.input() else { return false };
        // Every turn of the overlay's loop (some ninety a second): on the stack, as SteamVR's own copy of them is.
        let mut active = [ActiveSet::default(); MOST_SETS];
        let mut count = 0;
        let own = self.sets.iter().map(|&set| ActiveSet { set, ..Default::default() });
        let blocking = self.orbit.into_iter().flat_map(|orbit| blocked_sets(orbit.block, orbit.hands, self.blocked));
        for (slot, set) in active.iter_mut().zip(own.chain(blocking)) {
            *slot = set;
            count += 1;
        }
        input.update_active(&active[..count]).is_ok()
    }

    /// Which hands' triggers VRChat does not get from the next read on (`overlay::orbit::block`). Said anew at each read: it ends by not being said.
    pub fn set_block(&mut self, blocked: [bool; 2]) {
        self.blocked = blocked;
    }

    /// VRChat's menu button of each hand, held.
    pub fn orbit_menu_buttons(&self, steamvr: &vr::Client) -> [bool; 2] {
        let (Some(orbit), Ok(input)) = (self.orbit, steamvr.input()) else { return [false; 2] };
        orbit.menu.map(|action| input.digital(action).is_ok_and(|d| d.active && d.state))
    }

    /// Each hand's trigger held. A hand blocked reads it from the set that takes it (it takes it from this application's lower sets too).
    pub fn orbit_triggers(&self, steamvr: &vr::Client, blocked: [bool; 2]) -> [bool; 2] {
        let (Some(orbit), Ok(input)) = (self.orbit, steamvr.input()) else { return self.press_down };
        std::array::from_fn(|hand| if blocked[hand] { input.digital(orbit.triggers[hand]).is_ok_and(|d| d.active && d.state) } else { self.press_down[hand] })
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

    /// Missing/inactive action data is unknown, distinct from a released grip.
    pub fn grab_states(&self,steamvr:&vr::Client) -> [Option<bool>;2] {
        let (Some((_,grabs)),Ok(input))=(self.panel,steamvr.input()) else{return [None;2];};
        grabs.map(|action|input.digital(action).ok().filter(|d|d.active).map(|d|d.state))
    }

    /// How far each hand's grip is pulled, 0 to 1 (left, right): `None` where SteamVR gives no pull (unbound, a grip with only a click).
    pub fn grip_pulls(&self, steamvr: &vr::Client) -> [Option<f32>; 2] {
        let (Some(grip), Ok(input)) = (self.grip, steamvr.input()) else { return [None; 2] };
        grip.map(|action| input.analog(action).ok().filter(|d| d.active).map(|d| d.x))
    }

    /// Each hand's grip held (left, right).
    pub fn grabs(&self, steamvr: &vr::Client) -> [bool; 2] {
        let (Some((_, grabs)), Ok(input)) = (self.panel, steamvr.input()) else { return [false; 2] };
        grabs.map(|action| input.digital(action).is_ok_and(|d| d.active && d.state))
    }
}

/// The set that takes a trigger from VRChat, for each hand blocked and that hand only: the other keeps its trigger.
fn blocked_sets(set: ActionSetHandle, hands: [orbiters_openvr::input::InputSourceHandle; 2], blocked: [bool; 2]) -> impl Iterator<Item = ActiveSet> {
    (0..2).filter(move |&hand| blocked[hand]).map(move |hand| ActiveSet { set, device: hands[hand], priority: OVERLAY_GLOBAL_PRIORITY_MIN + 1 })
}

/// The sets made active in a turn at most: the six of [`Actions`], and the one that takes a trigger from VRChat for
/// each hand.
const MOST_SETS: usize = 8;

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
    fn a_steamvr_input_error_is_put_into_the_language_it_is_drawn_in() {
        use orbiters_i18n::{in_language, Language};
        let failed = Unavailable::Input(5);
        assert_eq!(failed.text(), "SteamVR input error 5");
        assert_eq!(in_language(Language::Fr, || failed.text()), "erreur d’entrée SteamVR 5");
        assert_eq!(in_language(Language::Ja, || failed.to_string()), "SteamVRの入力エラー 5");
        // What something else said is its own words.
        assert_eq!(in_language(Language::Es, || Unavailable::Said("disk full".into()).text()), "disk full");
    }

    #[test]
    fn a_saved_binding_gets_what_it_lacks_and_keeps_what_the_person_chose() {
        let saved = r#"{ "app_key": "cc.orbiters.n4n0", "name": "mine", "controller_type": "oculus_touch",
            "bindings": { "/actions/menu": { "sources": ["chosen"] } } }"#;
        let ours = r#"{ "bindings": { "/actions/menu": { "sources": ["default"] }, "/actions/orbit": { "sources": ["new"] }, "/actions/orbit_block": { "sources": [] } } }"#;
        let merged: serde_json::Value = serde_json::from_str(&merged_binding(saved, ours).expect("two sets lacking")).unwrap();
        assert_eq!(merged["bindings"]["/actions/menu"]["sources"][0], "chosen", "what was chosen stays");
        assert_eq!(merged["bindings"]["/actions/orbit"]["sources"][0], "new");
        assert!(merged["bindings"]["/actions/orbit_block"].is_object());
        assert_eq!((&merged["name"], &merged["app_key"]), (&serde_json::json!("mine"), &serde_json::json!("cc.orbiters.n4n0")));
        // Complete: left alone, and so nothing is written. Not a binding: left alone too.
        assert_eq!(merged_binding(ours, ours), None);
        assert_eq!(merged_binding("not json", ours), None);
        assert_eq!(merged_binding("{}", ours), None);
    }

    #[test]
    fn a_saved_binding_s_address_is_found_as_steamvr_keeps_it() {
        let root = std::env::temp_dir().join(format!("orbit-saved-binding-{}", std::process::id()));
        let item = root.join("steamapps/workshop/content/250820/3830136132");
        std::fs::create_dir_all(&item).unwrap();
        std::fs::write(item.join("9429_legacy.bin"), "{}").unwrap();
        let runtime = root.join("steamapps/common/SteamVR");
        let found = saved_binding_file("vr-input-workshop://3830136132", Some(&runtime)).unwrap();
        assert_eq!(found.file_name().unwrap(), "9429_legacy.bin");
        assert_eq!(saved_binding_file("vr-input-workshop://1", Some(&runtime)), None, "no such item");
        assert_eq!(saved_binding_file("file:///C:/x/y.json", None), Some(PathBuf::from("C:/x/y.json")));
        assert_eq!(saved_binding_file("something else", Some(&runtime)), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// With this application's real bindings: the very case that hid the panels in VR.
    #[test]
    fn enzos_saved_touch_binding_would_now_have_the_menu_button() {
        let dir = std::env::temp_dir().join(format!("orbit-real-bindings-{}", std::process::id()));
        write_manifest(&dir).unwrap();
        let ours = std::fs::read_to_string(dir.join("bindings_oculus_touch.json")).unwrap();
        let saved = r#"{ "app_key": "cc.orbiters.n4n0", "bindings": { "/actions/avatar": {}, "/actions/menu": {}, "/actions/panel": {}, "/actions/walk": {} } }"#;
        let merged: serde_json::Value = serde_json::from_str(&merged_binding(saved, &ours).unwrap()).unwrap();
        let sources = merged["bindings"]["/actions/orbit"]["sources"].as_array().unwrap();
        assert!(sources.iter().any(|s| s["inputs"]["click"]["output"] == "/actions/orbit/in/menu_left"));
        assert!(sources.iter().any(|s| s["inputs"]["click"]["output"] == "/actions/orbit/in/menu_right"));
        assert!(merged["bindings"]["/actions/orbit_block"]["sources"].as_array().is_some_and(|s| !s.is_empty()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

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
        assert_eq!(manifest["actions"].as_array().unwrap().len(), 32);
        // Every controller gives each grip's pull, for VRChat's own grab thresholds (`orbit::vrchat_grip`).
        for (controller, _, _) in CONTROLLERS {
            let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join(format!("bindings_{controller}.json"))).unwrap()).unwrap();
            let sources = value["bindings"]["/actions/grip"]["sources"].as_array().unwrap();
            assert_eq!(sources[1]["path"], "/user/hand/right/input/grip");
            assert_eq!(sources[1]["inputs"]["pull"]["output"], "/actions/grip/in/value_right");
        }
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
    fn the_trigger_is_taken_from_vrchat_only_for_the_hand_pointing_and_above_scene_priority() {
        let sets: Vec<ActiveSet> = blocked_sets(7, [11, 22], [false, true]).collect();
        assert_eq!(sets, vec![ActiveSet { set: 7, device: 22, priority: OVERLAY_GLOBAL_PRIORITY_MIN + 1 }]);
        assert!(sets[0].priority >= 0x0100_0000 && sets[0].priority < 0x0200_0000, "openvr.h's overlay global range");
        assert_eq!(blocked_sets(7, [11, 22], [false, false]).count(), 0, "free: nothing said");
        assert_eq!(blocked_sets(7, [11, 22], [true, true]).count(), 2);
    }

    #[test]
    fn every_controller_binds_vrchat_s_menu_button_and_its_whole_trigger() {
        let dir = std::env::temp_dir().join(format!("orbiters-actions-orbit-{}", std::process::id()));
        write_manifest(&dir).unwrap();
        for (controller, _, _) in CONTROLLERS {
            let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join(format!("bindings_{controller}.json"))).unwrap()).unwrap();
            let menu = value["bindings"]["/actions/orbit"]["sources"].as_array().unwrap();
            assert_eq!(menu.len(), 2, "{controller}");
            assert!(menu[0]["path"].as_str().unwrap().starts_with("/user/hand/left/input/"));
            let block = value["bindings"]["/actions/orbit_block"]["sources"].as_array().unwrap();
            assert_eq!(block[1]["path"], "/user/hand/right/input/trigger", "{controller}");
            assert_eq!(block[1]["inputs"]["click"]["output"], "/actions/orbit_block/in/trigger_right");
            assert_eq!(block[1]["inputs"]["pull"]["output"], "/actions/orbit_block/in/pull_right");
        }
        // As VRChat's own bindings have them.
        assert_eq!((menu_button("knuckles", "left"), menu_button("oculus_touch", "left"), menu_button("oculus_touch", "right"), menu_button("vive_controller", "right")), ("b", "y", "b", "application_menu"));
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
