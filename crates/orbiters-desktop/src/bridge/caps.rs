//! What this computer's VRChat can be made to do through the bridge: the one place shared logic asks, instead of `cfg`.
//! Settings, MCP discovery (the tool list, the guide, the prompt) and every tool call read it, and a tool this computer
//! cannot serve is refused here, before anything happens (no instance made, no game started or closed).
//!
//! Windows has every capability, so nothing there changes. A Mac (VRChat under Obelisk's engine, `orbiters_core::obelisk`)
//! observes and travels: the game started in a private instance, its window looked at, closed. What it cannot do yet, and why
//! (docs/vrchat-bridge.md, "On a Mac"): input (no listener of the person's own input that the guard could rely on, no
//! engine input interface with targets and releases), 3D pictures (no Spout: the capture kit's strip only reaches a recording
//! through the stream camera's own render target), side sessions and VR sessions (one Steam game per engine library, no
//! SteamVR), and handing a link to a running game (no link pipe).

use super::game::Failure;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which kind of computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Macos,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub platform: Platform,
    /// VRChats of their own beside others (`open_session`, `close_session`).
    pub side_sessions: bool,
    /// VRChat's `--profile` for the main session.
    pub profiles: bool,
    /// A running VRChat handed an instance's link (`send_link`).
    pub travel_by_link: bool,
    /// OSC input into the game: `walk`, `turn`, the OSC inputs of `press`, `wear_avatar`, `aim_camera`.
    pub osc_input: bool,
    /// Keys and clicks into the game's window: the keys of `press`, `mouse`, `focus_game`.
    pub keys_and_mouse: bool,
    /// 3D pictures through the app's capture (`take_3d_picture`, `open_camera`).
    pub capture_3d: bool,
    /// VR sessions in the developer sandbox (`open_session` with `vr`, `vr_sandbox`).
    pub vr_sandbox: bool,
    /// A hotkey that pauses the agent (Windows' Ctrl+Alt+F12).
    pub hotkey: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self::here()
    }
}

impl Capabilities {
    /// Everything: Windows, as the bridge always was.
    pub const WINDOWS: Self = Self {
        platform: Platform::Windows,
        side_sessions: true,
        profiles: true,
        travel_by_link: true,
        osc_input: true,
        keys_and_mouse: true,
        capture_3d: true,
        vr_sandbox: true,
        hotkey: true,
    };

    /// A Mac: observe and travel (Phase 1 of docs/vrchat-bridge.md's Mac plan).
    pub const MAC: Self = Self {
        platform: Platform::Macos,
        side_sessions: false,
        profiles: false,
        travel_by_link: false,
        osc_input: false,
        keys_and_mouse: false,
        capture_3d: false,
        vr_sandbox: false,
        hotkey: false,
    };

    /// This computer's.
    pub fn here() -> Self {
        if cfg!(target_os = "macos") {
            Self::MAC
        } else {
            Self::WINDOWS
        }
    }

    pub fn all(&self) -> bool {
        *self == Self { platform: self.platform, ..Self::WINDOWS }
    }

    /// Whether the tool `name` can be offered here at all (MCP's list). A tool whose every use needs what is missing is left
    /// out; one that only some arguments make impossible stays, and [`Capabilities::refusal`] answers those.
    pub fn offers(&self, name: &str) -> bool {
        match name {
            "walk" | "turn" | "wear_avatar" | "aim_camera" => self.osc_input,
            "mouse" | "focus_game" => self.keys_and_mouse,
            "press" => self.osc_input || self.keys_and_mouse,
            "take_3d_picture" => self.capture_3d,
            "open_camera" => self.capture_3d && self.vr_sandbox,
            "open_session" | "close_session" => self.side_sessions,
            "vr_sandbox" => self.vr_sandbox,
            _ => true,
        }
    }

    /// Why `name` with `args` cannot be done on this computer, before anything is done; `None` when it can.
    pub fn refusal(&self, name: &str, args: &Value) -> Option<Failure> {
        let missing: Option<&str> = match name {
            _ if !self.offers(name) => Some(match name {
                "walk" | "turn" | "wear_avatar" | "aim_camera" => "OSC input into the game",
                "mouse" | "focus_game" | "press" => "keyboard and mouse input into the game",
                "take_3d_picture" | "open_camera" => "3D pictures",
                "vr_sandbox" => "the VR sandbox",
                _ => "sessions beside the main one",
            }),
            "press" => {
                let input = args["input"].as_str().unwrap_or_default();
                let osc = super::input::OSC_INPUTS.iter().any(|(n, _, _)| *n == input);
                match (osc, self.osc_input, self.keys_and_mouse) {
                    (true, false, _) => Some("OSC input into the game"),
                    (false, _, false) => Some("keyboard and mouse input into the game"),
                    _ => None,
                }
            }
            "open_session" if args["vr"].as_bool() == Some(true) && !self.vr_sandbox => Some("the VR sandbox"),
            "go_to_world" if args["profile"].as_u64().is_some_and(|p| p > 0) && !self.profiles => Some("VRChat profiles"),
            "go_to_world" if args["relaunch"].as_bool() == Some(true) && !self.side_sessions => Some("sessions beside the main one"),
            _ => None,
        };
        missing.map(|what| {
            Failure::new(
                "not_on_this_computer",
                format!("This computer's VRChat ({}) cannot do this through the bridge yet: {what} is not available here, so nothing was done.", self.platform_name()), // i18n: not shown (an AI client reads it in the tool's answer)
                "Use what tools/list offers here: get_state, find_worlds, go_to_world, screenshot, wait_for and close_vrchat. Tell the person what you could not do.", // i18n: not shown (an AI client reads it in the tool's answer)
            )
        })
    }

    fn platform_name(&self) -> &'static str {
        if self.platform == Platform::Macos {
            "VRChat on a Mac, under Obelisk's engine" // i18n: not shown (an AI client reads it)
        } else {
            "VRChat on Windows" // i18n: not shown (an AI client reads it)
        }
    }

    /// What this computer cannot do, for `get_state` and the guide.
    pub fn missing(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        for (have, what) in [
            (self.osc_input, "walk, turn, wear_avatar, aim_camera (OSC input)"),
            (self.keys_and_mouse, "press, mouse, focus_game (keys and clicks)"),
            (self.capture_3d, "take_3d_picture, open_camera (3D pictures)"),
            (self.side_sessions, "open_session, close_session (sessions beside the main one)"),
            (self.vr_sandbox, "VR sessions"),
        ] {
            if !have {
                out.push(what);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn windows_has_everything_and_refuses_nothing() {
        let windows = Capabilities::WINDOWS;
        assert!(windows.all());
        for name in ["walk", "turn", "press", "mouse", "focus_game", "take_3d_picture", "open_camera", "open_session", "close_session", "vr_sandbox", "wear_avatar", "aim_camera"] {
            assert!(windows.offers(name), "{name}");
            assert!(windows.refusal(name, &json!({ "input": "esc", "vr": true, "profile": 2, "relaunch": true })).is_none(), "{name}");
        }
        assert!(windows.missing().is_empty());
        #[cfg(windows)]
        assert_eq!(Capabilities::here(), windows);
    }

    #[test]
    fn a_mac_observes_and_travels_and_refuses_the_rest_by_name() {
        let mac = Capabilities::MAC;
        assert!(!mac.all());
        for name in ["get_state", "find_worlds", "go_to_world", "close_vrchat", "wait_for", "screenshot", "list_sessions", "pictures", "map_plan", "world_labels"] {
            assert!(mac.offers(name), "{name}");
            assert!(mac.refusal(name, &json!({})).is_none(), "{name}");
        }
        for name in ["walk", "turn", "press", "mouse", "focus_game", "take_3d_picture", "open_camera", "open_session", "close_session", "vr_sandbox", "wear_avatar", "aim_camera"] {
            assert!(!mac.offers(name), "{name}");
            assert_eq!(mac.refusal(name, &json!({})).map(|f| f.code), Some("not_on_this_computer"), "{name}");
        }
        assert!(mac.refusal("go_to_world", &json!({ "world_id": "wrld_x", "profile": 1 })).is_some(), "no profiles");
        assert!(mac.refusal("go_to_world", &json!({ "world_id": "wrld_x", "profile": 0 })).is_none());
        assert_eq!(mac.missing().len(), 5);
        #[cfg(target_os = "macos")]
        assert_eq!(Capabilities::here(), mac);
    }
}
