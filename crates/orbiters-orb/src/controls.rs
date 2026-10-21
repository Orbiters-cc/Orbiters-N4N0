//! An avatar's controls as Orb sees them: from its manifest (menu labels and final parameter names, contracts
//! `Docs/avatar-manifest.md`) or, without one, from the parameters VRChat lists. Finds the control a person or the
//! assistant names ("Hoodie", "Clothes / Hoodie") and turns a setting into the OSC messages that apply it.

use crate::text::Message;
use orbiters_contracts::avatar::{AvatarManifest, ControlType, ParameterType, Tag};
use orbiters_core::osc::{OscValue, PARAMETERS_PREFIX};
use rosc::OscType;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Toggle,
    Button,
    Submenu,
    Radial,
    TwoAxis,
    FourAxis,
    /// A bare parameter (an avatar without a manifest).
    Parameter,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Toggle => "toggle",
            Kind::Button => "button",
            Kind::Submenu => "submenu",
            Kind::Radial => "radial 0-1",
            Kind::TwoAxis => "two-axis puppet",
            Kind::FourAxis => "four-axis puppet",
            Kind::Parameter => "parameter",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    /// Labels from the main menu down to the control (a bare parameter: its name).
    pub path: Vec<String>,
    pub kind: Kind,
    pub parameter: Option<String>,
    /// What an active toggle, button or submenu sets its parameter to.
    pub value: f32,
    pub sub_parameters: Vec<String>,
    pub tags: Vec<Tag>,
}

impl Control {
    pub fn label(&self) -> &str {
        self.path.last().map(String::as_str).unwrap_or("")
    }

    /// The labels joined with " / ", without rich text tags ("Clothes / Hoodie").
    pub fn path_text(&self) -> String {
        self.path.iter().map(|l| plain(l)).collect::<Vec<_>>().join(" / ")
    }

    /// The parameter a setting writes: a radial's first sub-parameter, otherwise its own.
    pub fn target(&self) -> Option<&str> {
        match self.kind {
            Kind::Radial => self.sub_parameters.first().or(self.parameter.as_ref()).map(String::as_str),
            _ => self.parameter.as_deref(),
        }
    }
}

/// What to do with a control.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Setting {
    On,
    Off,
    /// A button's press (a toggle: on).
    Press,
    /// A radial's position (0 to 1) or a bare parameter's value.
    Value(f32),
}

/// The messages that apply a setting now, and those that end it a moment later (a button's release).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub now: Vec<Message>,
    pub later: Vec<Message>,
}

/// Parameters VRChat, face tracking and OSC apps set by themselves: never offered as controls.
pub fn is_system_parameter(name: &str) -> bool {
    const BUILT_IN: &[&str] = &[
        "IsLocal",
        "PreviewMode",
        "Viseme",
        "Voice",
        "GestureLeft",
        "GestureRight",
        "GestureLeftWeight",
        "GestureRightWeight",
        "AngularY",
        "VelocityX",
        "VelocityY",
        "VelocityZ",
        "VelocityMagnitude",
        "Upright",
        "Grounded",
        "Seated",
        "AFK",
        "TrackingType",
        "VRMode",
        "MuteSelf",
        "InStation",
        "Earmuffs",
        "IsOnFriendsList",
        "AvatarVersion",
        "IsAnimatorEnabled",
        "ScaleModified",
        "ScaleFactor",
        "ScaleFactorInverse",
        "EyeHeightAsMeters",
        "EyeHeightAsPercent",
        "EyeTrackingActive",
        "LipTrackingActive",
    ];
    const PREFIXES: &[&str] = &["Orb/", "KAT_", "OSC", "VRCOSC/", "FT/", "OSCm/", "BFI/", "Orbiters"];
    const SUFFIXES: &[&str] = &["_IsGrabbed", "_IsPosed", "_Angle", "_Stretch", "_Squish"];
    BUILT_IN.contains(&name) || name.contains("v2/") || PREFIXES.iter().any(|p| name.starts_with(p)) || SUFFIXES.iter().any(|s| name.ends_with(s))
}

/// Lowercase, without accents, rich text tags or extra spaces, separators as `/`: what names are compared on.
pub fn normalize(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            '>' | '\\' | '|' | '›' | '»' | ':' => plain.push('/'),
            _ => match orbiters_contracts::orb::unaccented(c) {
                Some(base) => plain.push_str(&base.to_lowercase()),
                None => plain.extend(c.to_lowercase()),
            },
        }
    }
    let parts: Vec<String> = plain
        .split('/')
        .map(|p| p.split(|c: char| c.is_whitespace() || c == '_' || c == '-').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" "))
        .map(|p| p.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|p| !p.is_empty())
        .collect();
    parts.join("/")
}

#[derive(Clone, Debug, Default)]
pub struct Controls {
    pub controls: Vec<Control>,
    /// Parameter types, for the messages' argument types.
    types: HashMap<String, ParameterType>,
    /// From the avatar's manifest (labels) rather than its bare parameters.
    pub from_manifest: bool,
}

impl Controls {
    /// The controls of a manifest: every control that sets a parameter (folders only when they do).
    pub fn from_manifest(manifest: &AvatarManifest) -> Self {
        let types = manifest.parameters.iter().map(|p| (p.name.clone(), p.kind)).collect();
        let controls = manifest
            .controls
            .iter()
            .filter(|c| c.parameter.is_some() || !c.sub_parameters.is_empty())
            .map(|c| Control {
                path: c.path.clone(),
                kind: match c.kind {
                    ControlType::Toggle => Kind::Toggle,
                    ControlType::Button => Kind::Button,
                    ControlType::Submenu => Kind::Submenu,
                    ControlType::Radial => Kind::Radial,
                    ControlType::TwoAxis => Kind::TwoAxis,
                    ControlType::FourAxis => Kind::FourAxis,
                },
                parameter: c.parameter.clone(),
                value: c.value.unwrap_or(1.0),
                sub_parameters: c.sub_parameters.clone(),
                tags: c.tags.clone(),
            })
            .collect();
        Self { controls, types, from_manifest: true }
    }

    /// Bare controls from the parameters VRChat lists (`/avatar/parameters/<name>` and their values), without the
    /// ones VRChat, face tracking or OSC apps set.
    pub fn from_parameters<'a>(values: impl IntoIterator<Item = (&'a str, &'a OscValue)>) -> Self {
        let mut controls = Vec::new();
        let mut types = HashMap::new();
        for (address, value) in values {
            let Some(name) = address.strip_prefix(PARAMETERS_PREFIX) else { continue };
            let kind = match value {
                OscValue::Bool(_) => ParameterType::Bool,
                OscValue::Int(_) => ParameterType::Int,
                OscValue::Float(_) => ParameterType::Float,
                OscValue::Text(_) => continue,
            };
            types.insert(name.to_string(), kind);
            if !is_system_parameter(name) {
                controls.push(Control {
                    path: vec![name.to_string()],
                    kind: Kind::Parameter,
                    parameter: Some(name.to_string()),
                    value: 1.0,
                    sub_parameters: Vec::new(),
                    tags: Vec::new(),
                });
            }
        }
        controls.sort_by(|a, b| a.path.cmp(&b.path));
        Self { controls, types, from_manifest: false }
    }

    /// Adds the types VRChat reports (a manifest from another build may lack a parameter).
    pub fn learn_types<'a>(&mut self, values: impl IntoIterator<Item = (&'a str, &'a OscValue)>) {
        for (address, value) in values {
            let Some(name) = address.strip_prefix(PARAMETERS_PREFIX) else { continue };
            let kind = match value {
                OscValue::Bool(_) => ParameterType::Bool,
                OscValue::Int(_) => ParameterType::Int,
                OscValue::Float(_) => ParameterType::Float,
                OscValue::Text(_) => continue,
            };
            self.types.entry(name.to_string()).or_insert(kind);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.controls.is_empty()
    }

    pub fn parameter_type(&self, name: &str) -> Option<ParameterType> {
        self.types.get(name).copied()
    }

    /// The control `query` names: its path ("Clothes / Hoodie"), its label, the end of its path, or a single
    /// control whose label holds the words asked for. An error names the closest controls.
    pub fn find(&self, query: &str) -> Result<&Control, String> {
        let q = normalize(query);
        if q.is_empty() {
            return Err("Name a control.".into());
        }
        let keyed: Vec<(String, String, &Control)> = self.controls.iter().map(|c| (normalize(&c.path.join("/")), normalize(c.label()), c)).collect();
        fn pick<'c>(found: Vec<&'c Control>, query: &str) -> Option<Result<&'c Control, String>> {
            match found.len() {
                0 => None,
                1 => Some(Ok(found[0])),
                _ => Some(Err(format!("Several controls match \"{query}\": {}.", list(&found)))),
            }
        }
        // Stages from the strictest: the whole path, the label, the end of the path, then the label or path holding it.
        let matches = |stage: usize, path: &str, label: &str| match stage {
            0 => path == q,
            1 => label == q,
            2 => path.ends_with(&format!("/{q}")),
            _ => (label.len() >= 3 && q.contains(label)) || (q.len() >= 3 && (label.contains(q.as_str()) || path.contains(q.as_str()))),
        };
        for stage in 0..4 {
            let found: Vec<&Control> = keyed.iter().filter(|(p, l, _)| matches(stage, p, l)).map(|(_, _, c)| *c).collect();
            if let Some(result) = pick(found, query) {
                return result;
            }
        }
        // Words in common, the most first.
        let words: Vec<&str> = q.split(['/', ' ']).filter(|w| w.len() >= 2).collect();
        let mut scored: Vec<(usize, &Control)> =
            keyed.iter().map(|(p, _, c)| (p.split(['/', ' ']).filter(|w| words.contains(w)).count(), *c)).filter(|(n, _)| *n > 0).collect();
        scored.sort_by_key(|s| std::cmp::Reverse(s.0));
        match scored.as_slice() {
            [] => Err(format!("No control named \"{query}\" on this avatar.")),
            [(_, c)] => Ok(*c),
            [(a, c), (b, _), ..] if a > b => Ok(*c),
            _ => Err(format!("Several controls match \"{query}\": {}.", list(&scored.iter().map(|(_, c)| *c).take(6).collect::<Vec<_>>()))),
        }
    }

    /// Controls whose path holds `query`'s words (all of them when it is empty), at most `limit`.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&Control> {
        let q = normalize(query);
        let words: Vec<&str> = q.split(['/', ' ']).filter(|w| !w.is_empty()).collect();
        self.controls
            .iter()
            .filter(|c| {
                let p = normalize(&c.path.join("/"));
                words.iter().all(|w| p.contains(w))
            })
            .take(limit)
            .collect()
    }

    /// The value a setting writes into `parameter`, typed as the avatar declares it.
    fn typed(&self, parameter: &str, value: f32) -> OscType {
        match self.types.get(parameter) {
            Some(ParameterType::Bool) => OscType::Bool(value > 0.5),
            Some(ParameterType::Int) => OscType::Int(value.round().clamp(0.0, 255.0) as i32),
            _ => OscType::Float(value.clamp(-1.0, 1.0)),
        }
    }

    /// The messages that apply `setting` to `control`.
    pub fn plan(&self, control: &Control, setting: Setting) -> Result<Plan, String> {
        let name = control.target().ok_or_else(|| format!("\"{}\" sets no parameter.", control.path_text()))?;
        let address = format!("{PARAMETERS_PREFIX}{name}");
        let message = |v: f32| (address.clone(), self.typed(name, v));
        Ok(match (control.kind, setting) {
            (Kind::TwoAxis | Kind::FourAxis, _) => return Err(format!("\"{}\" is a puppet: Orb cannot hold it.", control.path_text())),
            (Kind::Button, Setting::Off) => Plan { now: vec![message(0.0)], later: Vec::new() },
            (Kind::Button, _) => Plan { now: vec![message(control.value)], later: vec![message(0.0)] },
            (Kind::Radial, Setting::Value(v)) => Plan { now: vec![message(v.clamp(0.0, 1.0))], later: Vec::new() },
            (Kind::Radial, Setting::On | Setting::Press) => Plan { now: vec![message(1.0)], later: Vec::new() },
            (Kind::Radial, Setting::Off) => Plan { now: vec![message(0.0)], later: Vec::new() },
            (Kind::Parameter, Setting::Value(v)) => Plan { now: vec![message(v)], later: Vec::new() },
            (_, Setting::On | Setting::Press) => Plan { now: vec![message(control.value)], later: Vec::new() },
            (_, Setting::Off) => Plan { now: vec![message(0.0)], later: Vec::new() },
            (_, Setting::Value(v)) => Plan { now: vec![message(if v > 0.5 { control.value } else { 0.0 })], later: Vec::new() },
        })
    }

    /// How a control stands, from the latest values VRChat sent (`/avatar/parameters/…`).
    pub fn state(&self, control: &Control, latest: &HashMap<String, OscValue>) -> Option<String> {
        let name = control.target()?;
        let value = latest.get(&format!("{PARAMETERS_PREFIX}{name}"))?;
        let v = match value {
            OscValue::Bool(b) => *b as i32 as f32,
            OscValue::Int(i) => *i as f32,
            OscValue::Float(f) => *f,
            OscValue::Text(_) => return None,
        };
        Some(match control.kind {
            Kind::Radial => format!("{:.2}", v),
            Kind::Parameter => match value {
                OscValue::Bool(b) => b.to_string(),
                OscValue::Int(i) => i.to_string(),
                _ => format!("{v:.2}"),
            },
            _ => if (v - control.value).abs() < 0.01 && control.value != 0.0 { "on" } else { "off" }.to_string(),
        })
    }

    /// One line per control, for the assistant ("Clothes / Hoodie (toggle, on)"), at most `max_chars` (the rest
    /// counted). Orb's own controls and system ones are left out.
    pub fn summary(&self, latest: &HashMap<String, OscValue>, max_chars: usize) -> String {
        let mut out = String::new();
        let mut left = 0;
        for c in self.controls.iter().filter(|c| !c.tags.contains(&Tag::Orb) && !c.tags.contains(&Tag::System)) {
            let state = self.state(c, latest).map(|s| format!(", {s}")).unwrap_or_default();
            let tags = if c.tags.is_empty() {
                String::new()
            } else {
                format!(" [{}]", c.tags.iter().filter(|t| **t != Tag::Other).map(tag_word).collect::<Vec<_>>().join(", "))
            };
            let line = format!("{} ({}{state}){tags}\n", c.path_text(), c.kind.word());
            if out.len() + line.len() > max_chars {
                left += 1;
                continue;
            }
            out.push_str(&line);
        }
        if left > 0 {
            out.push_str(&format!("… and {left} more (find_avatar_controls searches them)\n"));
        }
        out
    }
}

/// A label without VRChat's rich text tags (`<b>`, `<color=#fff>`).
pub fn plain(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut in_tag = false;
    for c in label.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

fn list(controls: &[&Control]) -> String {
    controls.iter().take(6).map(|c| format!("\"{}\"", c.path_text())).collect::<Vec<_>>().join(", ")
}

pub fn tag_word(tag: &Tag) -> &'static str {
    match tag {
        Tag::Clothing => "clothing",
        Tag::Accessory => "accessory",
        Tag::Hair => "hair",
        Tag::Body => "body",
        Tag::Face => "face",
        Tag::Nsfw => "nsfw",
        Tag::Pose => "pose",
        Tag::Emote => "emote",
        Tag::Locomotion => "locomotion",
        Tag::Prop => "prop",
        Tag::Effect => "effect",
        Tag::System => "system",
        Tag::Orb => "orb",
        Tag::Other => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> AvatarManifest {
        AvatarManifest::parse(
            r#"{ "format": "orbiters.avatar", "version": 1, "avatar_id": "avtr_x",
              "parameters": [
                { "name": "VF1_Clothes/Hoodie", "type": "bool" }, { "name": "VF2_Hat", "type": "bool" },
                { "name": "Outfit", "type": "int" }, { "name": "Size", "type": "float" }, { "name": "VRCEmote", "type": "int" },
                { "name": "Orb/Visible", "type": "bool" } ],
              "controls": [
                { "path": ["Clothes"], "type": "submenu", "tags": ["clothing"] },
                { "path": ["Clothes", "<b>Hoodie</b>"], "type": "toggle", "parameter": "VF1_Clothes/Hoodie", "value": 1, "tags": ["clothing"] },
                { "path": ["Clothes", "Chapeau à plumes"], "type": "toggle", "parameter": "VF2_Hat", "value": 1, "tags": ["clothing"] },
                { "path": ["Outfits", "Summer"], "type": "toggle", "parameter": "Outfit", "value": 2 },
                { "path": ["Outfits", "Winter"], "type": "toggle", "parameter": "Outfit", "value": 3 },
                { "path": ["Size"], "type": "radial", "parameter": "Size", "sub_parameters": ["Size"] },
                { "path": ["GoGo Loco", "Poses", "Sit"], "type": "button", "parameter": "VRCEmote", "value": 129, "tags": ["pose"] },
                { "path": ["Orb", "Orb"], "type": "toggle", "parameter": "Orb/Visible", "value": 1, "tags": ["orb"] }
              ] }"#,
        )
        .unwrap()
    }

    #[test]
    fn normalizes_labels() {
        assert_eq!(normalize("  <b>Clothes</b> >  Chapeau  à_plumes! "), "clothes/chapeau a plumes");
        assert_eq!(normalize("Clothes / Hoodie"), "clothes/hoodie");
    }

    #[test]
    fn finds_controls_by_path_label_or_words() {
        let c = Controls::from_manifest(&manifest());
        assert_eq!(c.controls.len(), 7, "the folder sets nothing");
        assert_eq!(c.find("Clothes / Hoodie").unwrap().parameter.as_deref(), Some("VF1_Clothes/Hoodie"));
        assert_eq!(c.find("hoodie").unwrap().label(), "<b>Hoodie</b>");
        assert_eq!(c.find("chapeau a plumes").unwrap().parameter.as_deref(), Some("VF2_Hat"));
        assert_eq!(c.find("the chapeau").unwrap().parameter.as_deref(), Some("VF2_Hat"));
        assert_eq!(c.find("sit pose").unwrap().value, 129.0);
        assert!(c.find("Outfits").unwrap_err().contains("Several"));
        assert!(c.find("jetpack").unwrap_err().contains("No control"));
    }

    #[test]
    fn plans_typed_messages() {
        let c = Controls::from_manifest(&manifest());
        let at = |p: &str| format!("{PARAMETERS_PREFIX}{p}");
        let hoodie = c.find("hoodie").unwrap();
        assert_eq!(c.plan(hoodie, Setting::On).unwrap().now, vec![(at("VF1_Clothes/Hoodie"), OscType::Bool(true))]);
        assert_eq!(c.plan(hoodie, Setting::Off).unwrap().now, vec![(at("VF1_Clothes/Hoodie"), OscType::Bool(false))]);
        let winter = c.find("winter").unwrap();
        assert_eq!(c.plan(winter, Setting::On).unwrap().now, vec![(at("Outfit"), OscType::Int(3))]);
        let size = c.find("size").unwrap();
        assert_eq!(c.plan(size, Setting::Value(0.25)).unwrap().now, vec![(at("Size"), OscType::Float(0.25))]);
        let sit = c.find("sit").unwrap();
        let plan = c.plan(sit, Setting::Press).unwrap();
        assert_eq!(plan.now, vec![(at("VRCEmote"), OscType::Int(129))]);
        assert_eq!(plan.later, vec![(at("VRCEmote"), OscType::Int(0))]);
    }

    #[test]
    fn summarises_with_state_and_leaves_orb_out() {
        let c = Controls::from_manifest(&manifest());
        let latest: HashMap<String, OscValue> =
            [(format!("{PARAMETERS_PREFIX}Outfit"), OscValue::Int(3)), (format!("{PARAMETERS_PREFIX}VF1_Clothes/Hoodie"), OscValue::Bool(true))].into();
        let text = c.summary(&latest, 10_000);
        assert!(text.contains("Clothes / Hoodie (toggle, on) [clothing]"), "{text}");
        assert!(text.contains("Outfits / Winter (toggle, on)"));
        assert!(text.contains("Outfits / Summer (toggle, off)"));
        assert!(!text.contains("Orb / Orb"));
        let short = c.summary(&latest, 60);
        assert!(short.contains("more (find_avatar_controls"), "{short}");
    }

    #[test]
    fn bare_parameters_without_a_manifest() {
        let values = [
            ("/avatar/parameters/Hoodie".to_string(), OscValue::Bool(false)),
            ("/avatar/parameters/VelocityX".to_string(), OscValue::Float(0.0)),
            ("/avatar/parameters/v2/JawOpen".to_string(), OscValue::Float(0.0)),
            ("/avatar/parameters/Orb/State".to_string(), OscValue::Int(0)),
            ("/avatar/parameters/Tail_IsGrabbed".to_string(), OscValue::Bool(false)),
            ("/avatar/change".to_string(), OscValue::Text("avtr_x".into())),
        ];
        let c = Controls::from_parameters(values.iter().map(|(a, v)| (a.as_str(), v)));
        assert_eq!(c.controls.len(), 1);
        assert_eq!(c.plan(c.find("hoodie").unwrap(), Setting::Value(1.0)).unwrap().now, vec![("/avatar/parameters/Hoodie".to_string(), OscType::Bool(true))]);
    }
}
