//! The overlay's SteamVR actions (`IVRInput`): one action manifest, as SteamVR takes one per process, with the walk
//! boost's sticks, the button that opens N4N0's menu and each hand's trigger (it presses where that index finger points
//! on N4N0's screen), and their default bindings for each kind of controller (people change them in SteamVR's
//! controller settings, under Orbiters). SteamVR gives the trigger to VRChat as well. Every set is read once a frame,
//! together.

use orbiters_openvr::input::{ActionHandle, ActionSetHandle};
use orbiters_openvr::{self as vr};
use std::path::{Path, PathBuf};

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
    { "name": "/actions/menu/in/press_left", "type": "boolean" },
    { "name": "/actions/menu/in/press_right", "type": "boolean" }
  ],
  "action_sets": [ { "name": "/actions/walk", "usage": "leftright" }, { "name": "/actions/menu", "usage": "leftright" } ],
  "localization": [ { "language_tag": "en_US", "/actions/walk": "Walk boost",
    "/actions/walk/in/move_left": "Walking stick (left hand)", "/actions/walk/in/move_right": "Walking stick (right hand)",
    "/actions/menu": "N4N0's menu", "/actions/menu/in/open": "Open N4N0's menu",
    "/actions/menu/in/press_left": "Press on N4N0's screen (left hand)", "/actions/menu/in/press_right": "Press on N4N0's screen (right hand)" } ]
}"#;

/// Default bindings: each hand's stick (a Vive wand's trackpad) to its move action, a double press of the left stick
/// (rarely used by VRChat) to N4N0's menu, and each hand's trigger click to its press.
fn bindings(controller: &str, input: &str, mode: &str) -> String {
    let source = |hand: &str| {
        format!(
            r#"{{ "path": "/user/hand/{hand}/input/{input}", "mode": "{mode}", "inputs": {{ "position": {{ "output": "/actions/walk/in/move_{hand}" }} }} }}"#
        )
    };
    let menu = format!(r#"{{ "path": "/user/hand/left/input/{input}", "mode": "button", "inputs": {{ "double": {{ "output": "/actions/menu/in/open" }} }} }}"#);
    let press = |hand: &str| {
        format!(r#"{{ "path": "/user/hand/{hand}/input/trigger", "mode": "trigger", "inputs": {{ "click": {{ "output": "/actions/menu/in/press_{hand}" }} }} }}"#)
    };
    format!(
        r#"{{ "controller_type": "{controller}", "name": "Orbiters", "description": "Walk boost's sticks; a double press of the left stick opens N4N0's menu; the triggers press on N4N0's screen",
  "bindings": {{ "/actions/walk": {{ "sources": [ {}, {} ] }}, "/actions/menu": {{ "sources": [ {menu}, {}, {} ] }} }} }}"#,
        source("left"),
        source("right"),
        press("left"),
        press("right")
    )
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

/// The walk boost's actions.
#[derive(Clone, Copy, Debug)]
pub struct Walk {
    pub set: ActionSetHandle,
    pub left: ActionHandle,
    pub right: ActionHandle,
}

pub struct Actions {
    pub walk: Option<Walk>,
    menu: Option<(ActionSetHandle, ActionHandle)>,
    /// Each hand's trigger (left, right) and how it was at the last read.
    presses: Option<[ActionHandle; 2]>,
    press_down: [bool; 2],
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
            let menu = (input.action_set("/actions/menu").map_err(fail)?, input.action("/actions/menu/in/open").map_err(fail)?);
            let presses = [input.action("/actions/menu/in/press_left").map_err(fail)?, input.action("/actions/menu/in/press_right").map_err(fail)?];
            Ok((walk, menu, presses))
        })();
        match loaded {
            Ok((walk, menu, presses)) => Self {
                walk: Some(walk),
                menu: Some(menu),
                presses: Some(presses),
                press_down: [false; 2],
                sets: vec![walk.set, menu.0],
                failed: None,
                menu_down: false,
            },
            Err(e) => {
                eprintln!("SteamVR actions unavailable: {e}");
                Self { walk: None, menu: None, presses: None, press_down: [false; 2], sets: Vec::new(), failed: Some(e), menu_down: false }
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

    /// The menu button was pressed since the last read (its rising edge; `changed` also catches a press that went
    /// up and down again between two reads).
    pub fn menu_pressed(&mut self, steamvr: &vr::Client) -> bool {
        let Some((_, action)) = self.menu else { return false };
        let Ok(input) = steamvr.input() else { return false };
        let Ok(data) = input.digital(action) else { return false };
        let down = data.active && data.state;
        let pressed = down && (!self.menu_down || data.changed);
        self.menu_down = down;
        pressed
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
        assert_eq!(manifest["actions"].as_array().unwrap().len(), 5);
        // Every controller listed has its file, and every file is listed.
        let listed: Vec<&str> = manifest["default_bindings"].as_array().unwrap().iter().map(|b| b["controller_type"].as_str().unwrap()).collect();
        assert_eq!(listed, CONTROLLERS.map(|(c, _, _)| c));
        for b in manifest["default_bindings"].as_array().unwrap() {
            assert!(dir.join(b["binding_url"].as_str().unwrap()).is_file());
        }
        std::fs::remove_dir_all(dir).ok();
    }
}
