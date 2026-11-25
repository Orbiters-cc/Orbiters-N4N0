//! The tools Orbiters - N4N0 lends N4N0's assistant (contracts `Docs/n4n0-assistant.md`, client tools): what the
//! assistant may ask the desktop to do, as JSON Schemas for the server, and the actions its calls become.

use serde_json::{json, Value};

/// A mood N4N0 may show while the assistant's answer is up (`N4N0/State`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mood {
    Neutral,
    Happy,
    Annoyed,
    Surprised,
    Sleepy,
}

/// What a tool call asks the desktop to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    SetControl {
        control: String,
        on: Option<bool>,
        value: Option<f32>,
    },
    FindControls {
        query: String,
    },
    SetParameter {
        name: String,
        value: f32,
    },
    TakePhoto {
        delay_seconds: u32,
    },
    SaveVideo,
    ListSavedVideos,
    CopyToClipboard {
        text: String,
    },
    SetAfkMessage {
        text: Option<String>,
        enabled: Option<bool>,
    },
    Hold {
        control: String,
        on: Option<bool>,
        value: Option<f32>,
        release: bool,
    },
    Mood {
        mood: Mood,
    },
    /// The computer's music: `None` only tells what plays.
    Media {
        command: Option<crate::menu::MediaCommand>,
        volume_change: Option<i8>,
    },
    /// N4N0's light: what is given changes, the rest stays.
    Light {
        on: Option<bool>,
        color: Option<u8>,
        level: Option<f32>,
    },
    /// One of the app's own features, from the catalogue of intents ([`crate::intents`]): its gate is its risk class.
    App(crate::intents::Intent),
    /// `orbiters_more`: the model asks for more of the app's tools (r23); they come with the next request of the turn.
    OpenTools {
        groups: Vec<crate::intents::route::ToolGroup>,
    },
}

/// What a tool call does to the world around it, declared for every tool: the one taint rule
/// (verify3-r21 B4: after reading other people's text, a turn changes nothing without the person's own yes) and the switch of the
/// account's actions read it, whatever the tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effect {
    /// It changes something: the avatar, the computer, the settings, a file, what others see.
    pub changes: bool,
    /// Its answer holds text other people wrote (a title, a label, a name).
    pub reads_others: bool,
    /// It is sent from, or acts on, the VRChat or the Orbiters account, or joins or starts somewhere (risk class b).
    pub from_account: bool,
    /// It waits for the person's yes every time, whatever was read (it lasts, shows to others, or is not undone in one step).
    pub asks: bool,
}

const fn effect(changes: bool, reads_others: bool) -> Effect {
    Effect { changes, reads_others, from_account: false, asks: false }
}

const fn asks(changes: bool, reads_others: bool) -> Effect {
    Effect { changes, reads_others, from_account: false, asks: true }
}

/// The tools that are not the app's intents, each with its effect (one of them for every call of it). The catalogue's other
/// half is [`crate::intents::CATALOGUE`]. A tool offered to the model and not declared fails a test; an action without
/// an effect does not compile ([`Action::effect`]).
pub const TOOLS: &[(&str, Effect)] = &[
    (MORE, effect(false, false)),
    ("set_avatar_control", effect(true, true)),
    ("find_avatar_controls", effect(false, true)),
    ("hold_avatar_control", asks(true, true)),
    ("set_avatar_parameter", effect(true, true)),
    ("take_photo", effect(true, false)),
    ("save_video", effect(true, true)),
    ("list_saved_videos", effect(false, true)),
    ("copy_to_clipboard", asks(true, false)),
    ("set_afk_message", asks(true, false)),
    ("control_media", effect(true, true)),
    ("n4n0_light", effect(true, false)),
    ("n4n0_mood", effect(true, false)),
];

impl Action {
    /// What this call does. (A tool's own line in [`TOOLS`] is the most any of its calls does: asking what plays changes
    /// nothing.)
    pub fn effect(&self) -> Effect {
        match self {
            Action::Hold { release: false, .. } => asks(true, true),
            Action::SetControl { .. } | Action::SetParameter { .. } | Action::Hold { .. } => effect(true, true),
            Action::CopyToClipboard { .. } | Action::SetAfkMessage { text: Some(_), .. } => asks(true, false),
            Action::FindControls { .. } | Action::ListSavedVideos => effect(false, true),
            Action::TakePhoto { .. } | Action::SetAfkMessage { .. } | Action::Light { .. } | Action::Mood { .. } => effect(true, false),
            Action::SaveVideo => effect(true, true),
            Action::Media { command: None, volume_change: None } => effect(false, true),
            Action::Media { .. } => effect(true, true),
            Action::OpenTools { .. } => effect(false, false),
            Action::App(intent) => {
                use crate::intents::Risk;
                let (spec, risk) = (intent.spec(), intent.risk());
                Effect { changes: spec.changes || intent.listens(), reads_others: spec.reads_others, from_account: risk == Risk::Confirm, asks: matches!(risk, Risk::Ask | Risk::Confirm) }
            }
        }
    }
}

/// The name of the tool that opens more of the app's tools.
pub const MORE: &str = "orbiters_more";

/// `orbiters_more`, sent with every request: the groups not offered yet, and the rules of the app's tools (once).
pub fn more_spec(open: &std::collections::BTreeSet<crate::intents::route::ToolGroup>) -> Value {
    use crate::intents::route::ToolGroup;
    let closed: Vec<ToolGroup> = ToolGroup::ALL.into_iter().filter(|g| !open.contains(g)).collect();
    let listed: Vec<String> = closed.iter().map(|g| format!("{} ({})", g.key(), g.about())).collect();
    json!({
        "name": MORE,
        "description": format!("More of Orbiters - N4N0's tools for the owner's request, when the ones offered do not do it: {}. They come with your next request. {}", listed.join("; "), crate::intents::model::RULES), // i18n: not shown
        "input_schema": { "type": "object", "properties": { "groups": { "type": "array", "items": { "enum": closed.iter().map(|g| g.key()).collect::<Vec<_>>() }, "minItems": 1, "maxItems": 4 } }, "required": ["groups"] }
    })
}

/// The client tools of a turn that carries `groups` (r23): `orbiters_more` first, then the tools of those groups only.
/// `actions`: what is sent from an account is offered too (`orbiters_core::features::N4N0_ASSISTANT_ACTIONS`).
pub fn specs_for(bare_parameters: bool, groups: &std::collections::BTreeSet<crate::intents::route::ToolGroup>, actions: bool) -> Vec<Value> {
    let mut out = vec![more_spec(groups)];
    out.extend(specs(bare_parameters, actions).iter().filter(|t| t["name"].as_str().and_then(crate::intents::route::group_of).is_some_and(|g| groups.contains(&g))).cloned());
    out
}

/// The client tools, for `tools` of a turn, made once (N4N-6). `bare_parameters`: the avatar has no manifest, so its parameters are
/// offered by name too. `actions`: as in [`specs_for`].
pub fn specs(bare_parameters: bool, actions: bool) -> &'static [Value] {
    static SPECS: [std::sync::OnceLock<Vec<Value>>; 4] = [const { std::sync::OnceLock::new() }; 4];
    SPECS[bare_parameters as usize * 2 + actions as usize].get_or_init(|| make_specs(bare_parameters, actions))
}

fn make_specs(bare_parameters: bool, actions: bool) -> Vec<Value> {
    let mut tools = vec![
        json!({
            "name": "set_avatar_control",
            "description": "Change the owner's VRChat avatar through its expression menu: turn a toggle on or off, press a button (a pose, an emote), or set a radial from 0 to 1. Name the control by its path from the avatar controls in the context (\"Clothes / Hoodie\") or call find_avatar_controls first. Others in the world see the change. A control tagged nsfw waits for the owner's yes.",
            "input_schema": { "type": "object", "properties": {
                "control": { "type": "string", "description": "The control's path or label." },
                "on": { "type": "boolean", "description": "Toggles: true turns it on, false off. Buttons: true presses it." },
                "value": { "type": "number", "minimum": 0, "maximum": 1, "description": "Radials: the position, 0 to 1." }
            }, "required": ["control"] }
        }),
        json!({
            "name": "find_avatar_controls",
            "description": "Search the owner's avatar menu for controls whose path holds these words (empty: list them all). Returns paths, kinds and current states. N4N0's own controls and the capture kit's are never listed: they are not yours to press.",
            "input_schema": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }
        }),
        json!({
            "name": "hold_avatar_control",
            "description": "Keep an avatar control at a value: the desktop puts it back whenever VRChat, a world or an avatar change resets it, until released. It waits for the owner's yes. release: true stops holding it.",
            "input_schema": { "type": "object", "properties": {
                "control": { "type": "string" },
                "on": { "type": "boolean" },
                "value": { "type": "number", "minimum": 0, "maximum": 1 },
                "release": { "type": "boolean" }
            }, "required": ["control"] }
        }),
        json!({
            "name": "take_photo",
            "description": "Take a picture with VRChat's camera (it opens if needed), after an optional countdown so people can pose.",
            "input_schema": { "type": "object", "properties": { "delay_seconds": { "type": "integer", "minimum": 0, "maximum": 10 } } }
        }),
        json!({
            "name": "save_video",
            "description": "Save the video the world's video player is playing now (or played last) to the owner's saved videos in Orbiters - N4N0.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "list_saved_videos",
            "description": "The owner's saved videos, newest first: titles and links.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "copy_to_clipboard",
            "description": "Put text (such as a video link) in the clipboard of the owner's computer, so they can paste it into a world's video player or VRChat's keyboard. Orbiters - N4N0 shows the owner the text and copies it once they say yes.",
            "input_schema": { "type": "object", "properties": { "text": { "type": "string", "maxLength": 2000 } }, "required": ["text"] }
        }),
        json!({
            "name": "set_afk_message",
            "description": "Set the message N4N0's panel shows to people while the owner is away (AFK), and/or turn it on or off. A new text waits for the owner's yes.",
            "input_schema": { "type": "object", "properties": {
                "text": { "type": "string", "maxLength": 120 },
                "enabled": { "type": "boolean" }
            } }
        }),
        json!({
            "name": "control_media",
            "description": "The music or video playing on the owner's computer (Spotify, a browser, any app Windows' media controls know): play, pause, next, previous, the computer's volume (up or down by about 10 %, or to a percent), mute. Returns what plays and the volume. No action: just tell what plays.",
            "input_schema": { "type": "object", "properties": {
                "action": { "enum": ["status", "play", "pause", "toggle", "next", "previous", "volume_up", "volume_down", "mute", "unmute"] },
                "volume": { "type": "integer", "minimum": 0, "maximum": 100, "description": "Set the volume to this percent." }
            } }
        }),
        json!({
            "name": "n4n0_light",
            "description": "Make N4N0 a light (for a dark world, or a photo): on or off, its colour and brightness. Everyone around sees it. Only for N4N0 with the Light option (the avatar lists N4N0/Light/On); the result says when it has none.",
            "input_schema": { "type": "object", "properties": {
                "on": { "type": "boolean" },
                "color": { "enum": ["white", "warm", "sunset", "pink", "violet", "blue", "cyan", "green"] },
                "brightness": { "type": "number", "minimum": 0, "maximum": 1 }
            } }
        }),
        json!({
            "name": "n4n0_mood",
            "description": "How N4N0 looks while your answer is up.",
            "input_schema": { "type": "object", "properties": { "mood": { "enum": ["neutral", "happy", "annoyed", "surprised", "sleepy"] } }, "required": ["mood"] }
        }),
    ];
    // The app's own features, a tool per domain (REQUEST20 6).
    tools.extend(crate::intents::specs(actions));
    if bare_parameters {
        tools.push(json!({
            "name": "set_avatar_parameter",
            "description": "This avatar has no menu labels (no Orbiters manifest): set one of its parameters by name, from the parameters in the context. Bool: 1 or 0; int: 0 to 255; float: -1 to 1.",
            "input_schema": { "type": "object", "properties": { "name": { "type": "string" }, "value": { "type": "number" } }, "required": ["name", "value"] }
        }));
    }
    tools
}

fn text(input: &Value, key: &str) -> Option<String> {
    input.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn number(input: &Value, key: &str) -> Option<f32> {
    input.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()).map(|v| v as f32)
}

fn flag(input: &Value, key: &str) -> Option<bool> {
    input.get(key).and_then(Value::as_bool)
}

/// The action a tool call asks for, or why it cannot be done.
pub fn parse(name: &str, input: &Value) -> Result<Action, String> {
    if name == MORE {
        let groups: Vec<crate::intents::route::ToolGroup> =
            input.get("groups").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).filter_map(crate::intents::route::ToolGroup::of_key).collect()).unwrap_or_default();
        if groups.is_empty() {
            return Err(format!("{MORE} needs \"groups\": one or more of the names it lists.")); // i18n: not shown
        }
        return Ok(Action::OpenTools { groups });
    }
    if let Some(intent) = crate::intents::parse(name, input) {
        return intent.map(Action::App);
    }
    let need = |key: &str| text(input, key).ok_or_else(|| format!("{name} needs \"{key}\".")); // i18n: not shown
    // As it is asked about, and as it is then used (verify4-r21 P6).
    let exact = |key: &str, max: usize| Some(crate::context::exact(&need(key)?, max)).filter(|t| !t.is_empty()).ok_or_else(|| format!("{name} needs \"{key}\".")); // i18n: not shown
    Ok(match name {
        "set_avatar_control" => Action::SetControl { control: need("control")?, on: flag(input, "on"), value: number(input, "value") },
        "find_avatar_controls" => Action::FindControls { query: text(input, "query").unwrap_or_default() },
        "hold_avatar_control" => {
            Action::Hold { control: need("control")?, on: flag(input, "on"), value: number(input, "value"), release: flag(input, "release").unwrap_or(false) }
        }
        "set_avatar_parameter" => Action::SetParameter { name: need("name")?, value: number(input, "value").ok_or("set_avatar_parameter needs \"value\".")? }, // i18n: not shown
        "take_photo" => Action::TakePhoto { delay_seconds: number(input, "delay_seconds").unwrap_or(0.0).clamp(0.0, 10.0) as u32 },
        "save_video" => Action::SaveVideo,
        "list_saved_videos" => Action::ListSavedVideos,
        "copy_to_clipboard" => Action::CopyToClipboard { text: exact("text", 2000)? },
        "set_afk_message" => Action::SetAfkMessage { text: text(input, "text").map(|t| crate::context::exact(&t, 120)).filter(|t| !t.is_empty()), enabled: flag(input, "enabled") },
        "n4n0_mood" => Action::Mood {
            mood: match text(input, "mood").as_deref() {
                Some("happy") => Mood::Happy,
                Some("annoyed") => Mood::Annoyed,
                Some("surprised") => Mood::Surprised,
                Some("sleepy") => Mood::Sleepy,
                _ => Mood::Neutral,
            },
        },
        "control_media" => {
            use crate::menu::MediaCommand;
            let volume = number(input, "volume").map(|v| v.clamp(0.0, 100.0).round() as u8);
            let (command, volume_change) = match text(input, "action").as_deref() {
                None | Some("status") => (volume.map(MediaCommand::Volume), None),
                Some("play") => (Some(MediaCommand::Play), None),
                Some("pause") => (Some(MediaCommand::Pause), None),
                Some("toggle") => (Some(MediaCommand::PlayPause), None),
                Some("next") => (Some(MediaCommand::Next), None),
                Some("previous") => (Some(MediaCommand::Previous), None),
                Some("volume_up") => (None, Some(10)),
                Some("volume_down") => (None, Some(-10)),
                Some("mute") => (Some(MediaCommand::Mute(true)), None),
                Some("unmute") => (Some(MediaCommand::Mute(false)), None),
                Some(other) => return Err(format!("control_media has no action \"{other}\".")), // i18n: not shown
            };
            Action::Media { command, volume_change }
        }
        "n4n0_light" => {
            let color = match text(input, "color") {
                Some(name) => Some(
                    orbiters_contracts::n4n0::LIGHT_COLORS
                        .iter()
                        .position(|(n, _)| n.eq_ignore_ascii_case(&name))
                        .ok_or_else(|| format!("N4N0's light has no colour \"{name}\"."))? as u8, // i18n: not shown
                ),
                None => None,
            };
            Action::Light { on: flag(input, "on"), color, level: number(input, "brightness").map(|v| v.clamp(0.0, 1.0)) }
        }
        other => return Err(format!("Orbiters - N4N0 has no tool \"{other}\".")), // i18n: not shown
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spec_parses_back() {
        for bare in [false, true] {
            let specs = specs(bare, true);
            // The catalogue's tools have tests of their own (`intents::tests`).
            for spec in specs.iter().filter(|s| crate::intents::Group::of_tool(s["name"].as_str().unwrap()).is_none()) {
                let name = spec["name"].as_str().unwrap();
                assert!(spec["description"].as_str().unwrap().len() > 20);
                assert_eq!(spec["input_schema"]["type"], "object", "{name}");
                // Every tool parses with its required fields filled in.
                let mut input = serde_json::Map::new();
                for key in spec["input_schema"]["required"].as_array().cloned().unwrap_or_default() {
                    let key = key.as_str().unwrap();
                    let kind = spec["input_schema"]["properties"][key]["type"].as_str().unwrap_or("string");
                    input.insert(key.into(), if kind == "number" { json!(1) } else { json!("x") });
                }
                assert!(parse(name, &Value::Object(input)).is_ok(), "{name}");
            }
            assert_eq!(specs.iter().any(|s| s["name"] == "set_avatar_parameter"), bare);
        }
    }

    #[test]
    fn reads_calls() {
        assert_eq!(
            parse("set_avatar_control", &json!({ "control": " Clothes / Hoodie ", "on": true })),
            Ok(Action::SetControl { control: "Clothes / Hoodie".into(), on: Some(true), value: None })
        );
        assert_eq!(parse("take_photo", &json!({ "delay_seconds": 99 })), Ok(Action::TakePhoto { delay_seconds: 10 }));
        assert_eq!(parse("n4n0_mood", &json!({ "mood": "annoyed" })), Ok(Action::Mood { mood: Mood::Annoyed }));
        assert!(parse("set_avatar_control", &json!({})).unwrap_err().contains("control"));
        assert_eq!(parse("control_media", &json!({})), Ok(Action::Media { command: None, volume_change: None }));
        assert_eq!(parse("control_media", &json!({ "action": "volume_down" })), Ok(Action::Media { command: None, volume_change: Some(-10) }));
        assert_eq!(
            parse("control_media", &json!({ "volume": 140 })),
            Ok(Action::Media { command: Some(crate::menu::MediaCommand::Volume(100)), volume_change: None })
        );
        assert_eq!(parse("n4n0_light", &json!({ "color": "Pink", "brightness": 2 })), Ok(Action::Light { on: None, color: Some(3), level: Some(1.0) }));
        assert!(parse("n4n0_light", &json!({ "color": "plaid" })).is_err());
        assert!(parse("format_disk", &json!({})).is_err());
    }
}
