//! When VRChat's grip takes and lets go of its camera panel, as VRChat itself decides it: from the binding VRChat uses (read from disk, as
//! SteamVR keeps it), not from a click of our own. Enzo, 2026-11-14: the tools drift from the panel "with all the grab and releases".
//!
//! VRChat's `Grab` action is a click on the grip's pull with its own thresholds: its shipped Touch binding takes at 0.7 and lets go below 0.55,
//! and the Workshop binding Enzo's VRChat uses has 0.7/0.55 on the left grip and SteamVR's defaults (0.25/0.2) on the right. Our grab was a click
//! at SteamVR's defaults on both hands, so a left grip pulled between 0.25 and 0.7 moved our copy and not VRChat's panel, and a left grip eased
//! to between 0.55 and 0.2 kept carrying ours after VRChat had let go (sandbox 2026-11-14, camera5/a: left taken at 0.75 not 0.65, let go at
//! 0.5 not 0.6; right taken at 0.27 not 0.22, let go at 0.17 not 0.22; ours took at every one of them from 0.27). Reading the pull and
//! applying VRChat's own thresholds makes both decide on the same frame.

use std::path::Path;

/// A click on a pull, as SteamVR's button and trigger modes make one: down once the pull reaches `take`, up once it falls below `let_go`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Click {
    pub take: f32,
    pub let_go: f32,
}

/// SteamVR's thresholds for a click whose binding names none (its binding interface's defaults: 0.25 and 0.2).
pub const STEAMVR_CLICK: Click = Click { take: 0.25, let_go: 0.2 };
/// What VRChat ships for its grab on a pulled grip (its Touch, Cosmos and Focus 3 bindings, and its Workshop default for Touch).
pub const VRCHAT_SHIPPED: Click = Click { take: 0.7, let_go: 0.55 };
/// VRChat's application key, under which SteamVR keeps the binding chosen for it.
pub const VRCHAT_APP_KEY: &std::ffi::CStr = c"steam.app.438100";

fn number(value: Option<&serde_json::Value>) -> Option<f32> {
    let value = value?;
    value.as_f64().map(|v| v as f32).or_else(|| value.as_str()?.trim().parse().ok())
}

/// How VRChat's `Grab` clicks on each hand (left, right) by a binding's text, where it is bound to that hand's grip pulled (button or trigger
/// mode). `None` for a hand whose grab is not a pull of the grip (a wand's trigger, an Index's grab mode): our own grip click applies there.
pub fn from_binding(text: &str) -> Option<[Option<Click>; 2]> {
    let binding: serde_json::Value = serde_json::from_str(text).ok()?;
    let sets = binding.get("bindings")?.as_object()?;
    let global = sets.iter().find(|(name, _)| name.eq_ignore_ascii_case("/actions/global")).map(|(_, set)| set)?;
    let sources = global.get("sources")?.as_array()?;
    Some(["left", "right"].map(|hand| {
        let path = format!("/user/hand/{hand}/input/grip");
        sources.iter().find_map(|source| {
            let on_grip = source.get("path")?.as_str()?.eq_ignore_ascii_case(&path);
            let mode = source.get("mode")?.as_str()?;
            let grabs = source.get("inputs")?.get("click")?.get("output")?.as_str()?.eq_ignore_ascii_case("/actions/global/in/grab");
            if !(on_grip && grabs && (mode == "button" || mode == "trigger")) {
                return None;
            }
            let parameters = source.get("parameters");
            let take = number(parameters.and_then(|p| p.get("click_activate_threshold"))).unwrap_or(STEAMVR_CLICK.take);
            let let_go = number(parameters.and_then(|p| p.get("click_deactivate_threshold"))).unwrap_or(STEAMVR_CLICK.let_go);
            Some(Click { take, let_go: let_go.min(take) })
        })
    }))
}

/// VRChat's own grab on `controller` (SteamVR's controller type), and where it was read: the binding SteamVR keeps as chosen for VRChat
/// (`<controller>_250820_CurrentURL_steamvrinput` in its settings, a Workshop item or a file), else what VRChat ships.
pub fn vrchat_grab(current_url: Option<&str>, runtime: Option<&Path>, controller: &str) -> ([Option<Click>; 2], String) {
    if let Some(url) = current_url {
        if let Some(text) = super::super::actions::saved_binding_file(url, runtime).and_then(|file| std::fs::read_to_string(file).ok()) {
            if let Some(clicks) = from_binding(&text) {
                return (clicks, url.to_string());
            }
        }
    }
    let shipped = match controller {
        "oculus_touch" | "vive_cosmos_controller" | "vive_focus3_controller" => Some(VRCHAT_SHIPPED),
        "holographic_controller" => Some(STEAMVR_CLICK),
        _ => None,
    };
    ([shipped; 2], "what VRChat ships".to_string()) // i18n: not shown (a log's line)
}

/// Each hand's grip as VRChat's grab has it, frame by frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Grips {
    held: [bool; 2],
}

impl Grips {
    /// This frame: each hand's pull (when SteamVR gives one) and VRChat's click on it (when known); `click_held` is our own click, for a hand
    /// with neither. Whether each grip is held, as VRChat decides it.
    pub fn step(&mut self, pulls: [Option<f32>; 2], clicks: [Option<Click>; 2], click_held: [bool; 2]) -> [bool; 2] {
        for hand in 0..2 {
            self.held[hand] = match (pulls[hand], clicks[hand]) {
                (Some(pull), Some(click)) => if self.held[hand] { pull >= click.let_go } else { pull >= click.take },
                _ => click_held[hand],
            };
        }
        self.held
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The grip sources of the Workshop binding Enzo's VRChat uses for Touch controllers (3309790171, "metaquest Default Bindings for Meta Touch
    /// Controllers", as on his disk 2026-11-14), with the gesture and axis sources beside them that must not be taken for the grab.
    const ENZOS_TOUCH: &str = r#"{ "controller_type": "oculus_touch", "bindings": {
        "/actions/global": { "sources": [
          { "inputs": { "click": { "output": "/actions/global/in/grab" } }, "mode": "button", "parameters": { "click_activate_threshold": "0.7", "click_deactivate_threshold": "0.55" }, "path": "/user/hand/left/input/grip" },
          { "inputs": { "pull": { "output": "/actions/global/in/grip_axis" } }, "mode": "trigger", "path": "/user/hand/left/input/grip" },
          { "inputs": { "click": { "output": "/actions/global/in/grab" } }, "mode": "button", "path": "/user/hand/right/input/grip" },
          { "inputs": { "pull": { "output": "/actions/global/in/gesture_grip_axis" }, "touch": { "output": "/actions/global/in/gesture_activator_grip" } }, "mode": "trigger", "path": "/user/hand/right/input/grip" } ] },
        "/actions/one_hand": { "sources": [
          { "inputs": { "click": { "output": "/actions/one_hand/in/grab" } }, "mode": "button", "parameters": { "click_activate_threshold": "0.7", "click_deactivate_threshold": "0.55" }, "path": "/user/hand/right/input/grip" } ] } } }"#;

    #[test]
    fn vrchats_grab_is_read_from_the_binding_it_uses() {
        assert_eq!(from_binding(ENZOS_TOUCH), Some([Some(VRCHAT_SHIPPED), Some(STEAMVR_CLICK)]));
        // VRChat's shipped Index binding grabs in SteamVR's grab mode, and a wand's grab is its trigger: our own click stays there.
        let index = r#"{ "bindings": { "/actions/Global": { "sources": [ { "path": "/user/hand/left/input/grip", "mode": "grab", "inputs": { "grab": { "output": "/actions/Global/in/Grab" } } } ] } } }"#;
        assert_eq!(from_binding(index), Some([None, None]));
        let wand = r#"{ "bindings": { "/actions/global": { "sources": [ { "path": "/user/hand/left/input/trigger", "mode": "button", "inputs": { "click": { "output": "/actions/global/in/grab" } } } ] } } }"#;
        assert_eq!(from_binding(wand), Some([None, None]));
        assert_eq!(from_binding("not json"), None);
        // Thresholds written as numbers count too.
        let numbers = r#"{ "bindings": { "/actions/global": { "sources": [ { "path": "/user/hand/right/input/grip", "mode": "trigger", "parameters": { "click_activate_threshold": 0.5, "click_deactivate_threshold": 0.4 }, "inputs": { "click": { "output": "/actions/global/in/Grab" } } } ] } } }"#;
        assert_eq!(from_binding(numbers), Some([None, Some(Click { take: 0.5, let_go: 0.4 })]));
    }

    #[test]
    fn without_a_chosen_binding_what_vrchat_ships_applies() {
        assert_eq!(vrchat_grab(None, None, "oculus_touch").0, [Some(VRCHAT_SHIPPED); 2]);
        assert_eq!(vrchat_grab(Some("file:///no/such/binding.json"), None, "oculus_touch").0, [Some(VRCHAT_SHIPPED); 2]);
        assert_eq!(vrchat_grab(None, None, "knuckles").0, [None, None]);
    }

    /// What real VRChat did in the sandbox with Enzo's binding (camera5/a, eye pictures `a/grid.png`): each grip value held, whether its panel
    /// was taken (from released) or still held (from fully pulled). Our old click took at every value from 0.27 and let go only below 0.2.
    #[test]
    fn the_grip_takes_and_lets_go_where_vrchat_did() {
        let [left, right] = from_binding(ENZOS_TOUCH).unwrap();
        let clicks = [left, right];
        let taken = |hand: usize, pull: f32| {
            let mut grips = Grips::default();
            let mut pulls = [None; 2];
            pulls[hand] = Some(pull);
            grips.step(pulls, clicks, [false; 2])[hand]
        };
        let kept = |hand: usize, pull: f32| {
            let mut grips = Grips::default();
            let mut pulls = [None; 2];
            pulls[hand] = Some(1.0);
            grips.step(pulls, clicks, [false; 2]);
            pulls[hand] = Some(pull);
            grips.step(pulls, clicks, [false; 2])[hand]
        };
        for (pull, vrchat) in [(0.22, false), (0.3, false), (0.5, false), (0.65, false), (0.75, true)] {
            assert_eq!(taken(0, pull), vrchat, "left taken at {pull}");
        }
        for (pull, vrchat) in [(0.6, true), (0.5, false), (0.3, false), (0.22, false), (0.17, false)] {
            assert_eq!(kept(0, pull), vrchat, "left kept at {pull}");
        }
        for (pull, vrchat) in [(0.22, false), (0.27, true), (0.3, true), (0.5, true)] {
            assert_eq!(taken(1, pull), vrchat, "right taken at {pull}");
        }
        for (pull, vrchat) in [(0.22, true), (0.17, false)] {
            assert_eq!(kept(1, pull), vrchat, "right kept at {pull}");
        }
        // No pull from SteamVR (a grip with only a click): our click decides.
        let mut grips = Grips::default();
        assert_eq!(grips.step([None, Some(0.9)], [Some(VRCHAT_SHIPPED), None], [true, false]), [true, false]);
    }
}
