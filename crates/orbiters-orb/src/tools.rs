//! The tools Orbiters desktop lends Orb's assistant (contracts `Docs/orb-assistant.md`, client tools): what the
//! assistant may ask the desktop to do, as JSON Schemas for the server, and the actions its calls become.

use serde_json::{json, Value};

/// An orb mood the assistant may show while its answer is up (`Orb/State`).
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
    SetControl { control: String, on: Option<bool>, value: Option<f32> },
    FindControls { query: String },
    SetParameter { name: String, value: f32 },
    TakePhoto { delay_seconds: u32 },
    SaveVideo,
    ListSavedVideos,
    CopyToClipboard { text: String },
    SetAfkMessage { text: Option<String>, enabled: Option<bool> },
    Hold { control: String, on: Option<bool>, value: Option<f32>, release: bool },
    Mood { mood: Mood },
}

/// The client tools, for `tools` of a turn. `bare_parameters`: the avatar has no manifest, so its parameters are
/// offered by name too.
pub fn specs(bare_parameters: bool) -> Vec<Value> {
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
            "description": "Search the owner's avatar menu for controls whose path holds these words (empty: list them all). Returns paths, kinds and current states. Orb's own controls and the capture kit's are never listed: they are not yours to press.",
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
            "description": "Save the video the world's video player is playing now (or played last) to the owner's saved videos in Orbiters desktop.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "list_saved_videos",
            "description": "The owner's saved videos, newest first: titles and links.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "copy_to_clipboard",
            "description": "Put text (such as a video link) in the clipboard of the owner's computer, so they can paste it into a world's video player or VRChat's keyboard. Orbiters desktop shows the owner the text and copies it once they say yes.",
            "input_schema": { "type": "object", "properties": { "text": { "type": "string", "maxLength": 2000 } }, "required": ["text"] }
        }),
        json!({
            "name": "set_afk_message",
            "description": "Set the message the orb's panel shows to people while the owner is away (AFK), and/or turn it on or off. A new text waits for the owner's yes.",
            "input_schema": { "type": "object", "properties": {
                "text": { "type": "string", "maxLength": 120 },
                "enabled": { "type": "boolean" }
            } }
        }),
        json!({
            "name": "orb_mood",
            "description": "How the orb looks while your answer is up.",
            "input_schema": { "type": "object", "properties": { "mood": { "enum": ["neutral", "happy", "annoyed", "surprised", "sleepy"] } }, "required": ["mood"] }
        }),
    ];
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
    let need = |key: &str| text(input, key).ok_or_else(|| format!("{name} needs \"{key}\"."));
    Ok(match name {
        "set_avatar_control" => Action::SetControl { control: need("control")?, on: flag(input, "on"), value: number(input, "value") },
        "find_avatar_controls" => Action::FindControls { query: text(input, "query").unwrap_or_default() },
        "hold_avatar_control" => {
            Action::Hold { control: need("control")?, on: flag(input, "on"), value: number(input, "value"), release: flag(input, "release").unwrap_or(false) }
        }
        "set_avatar_parameter" => Action::SetParameter { name: need("name")?, value: number(input, "value").ok_or("set_avatar_parameter needs \"value\".")? },
        "take_photo" => Action::TakePhoto { delay_seconds: number(input, "delay_seconds").unwrap_or(0.0).clamp(0.0, 10.0) as u32 },
        "save_video" => Action::SaveVideo,
        "list_saved_videos" => Action::ListSavedVideos,
        "copy_to_clipboard" => Action::CopyToClipboard { text: need("text")?.chars().take(2000).collect() },
        "set_afk_message" => Action::SetAfkMessage { text: text(input, "text").map(|t| t.chars().take(120).collect()), enabled: flag(input, "enabled") },
        "orb_mood" => Action::Mood {
            mood: match text(input, "mood").as_deref() {
                Some("happy") => Mood::Happy,
                Some("annoyed") => Mood::Annoyed,
                Some("surprised") => Mood::Surprised,
                Some("sleepy") => Mood::Sleepy,
                _ => Mood::Neutral,
            },
        },
        other => return Err(format!("Orbiters desktop has no tool \"{other}\".")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spec_parses_back() {
        for bare in [false, true] {
            let specs = specs(bare);
            for spec in &specs {
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
        assert_eq!(parse("orb_mood", &json!({ "mood": "annoyed" })), Ok(Action::Mood { mood: Mood::Annoyed }));
        assert!(parse("set_avatar_control", &json!({})).unwrap_err().contains("control"));
        assert!(parse("format_disk", &json!({})).is_err());
    }
}
