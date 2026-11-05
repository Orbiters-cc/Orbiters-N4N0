//! Saved avatar states (N4N0's States, contracts `Docs/n4n0.md`, States): every parameter of the avatar worn that OSC may
//! write, and its eye height, kept by avatar under names, so that the avatar can be put back as it was, by hand or on its
//! own after each world join and each switch to it (its default state). GoGoLoco's pose is saved with the rest and put
//! back only when the state says so; so is the avatar's size. N4N0's own parameters are never saved nor sent.
//!
//! Data and planning only: Orbiters - N4N0 reads VRChat's values (its OSCQuery tree), keeps the file, and sends what
//! [`plan`] says, paced ([`paced`]).

use orbiters_core::osc::OscValue;
use rosc::OscType;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The file's format.
pub const VERSION: u32 = 1;
/// VRChat's avatar eye height (metres): read and written over OSC; writes are ignored where the world forbids scaling.
pub const EYE_HEIGHT: &str = "/avatar/eyeheight";
/// VRChat's own: whether the world lets avatars be scaled (read only).
pub const EYE_HEIGHT_SCALING_ALLOWED: &str = "/avatar/eyeheightscalingallowed";
/// GoGoLoco's parameters all start so; the pose itself is VRChat's `VRCEmote`, which GoGoLoco drives.
pub const POSE_PREFIX: &str = "Go/";
pub const POSE_EMOTE: &str = "VRCEmote";
/// N4N0's own parameters (contracts `Docs/n4n0.md`): never saved, never sent.
pub const N4N0_PREFIX: &str = "N4N0/";
/// States one avatar keeps at most.
pub const MAX_STATES: usize = 50;
/// Characters of a state's name at most.
pub const NAME_MAX: usize = 40;
/// Messages a step of an apply sends at most (one OSC bundle each, as N4N0's own steps go).
pub const STEP_MESSAGES: usize = 16;

/// One saved state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedState {
    pub id: String,
    pub name: String,
    pub saved_utc: String,
    /// Applying it puts the GoGoLoco pose back too (`Go/…` and `VRCEmote`): off by default.
    #[serde(default)]
    pub pose: bool,
    /// Applying it puts the avatar's size back too (its eye height): off by default.
    #[serde(default)]
    pub size: bool,
    /// Every parameter OSC could write as it was, by name (GoGoLoco's included, whatever `pose` says, so the choice can
    /// change later).
    #[serde(default)]
    pub parameters: BTreeMap<String, OscValue>,
    /// The eye height, metres, when VRChat said it.
    #[serde(default)]
    pub eye_height: Option<f32>,
}

impl SavedState {
    /// How many of its parameters are GoGoLoco's pose.
    pub fn pose_parameters(&self) -> usize {
        self.parameters.keys().filter(|n| is_pose(n)).count()
    }
}

/// One avatar's states.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AvatarStates {
    /// The avatar's name when a state was last saved (VRChat's log), for the window.
    #[serde(default)]
    pub name: Option<String>,
    /// The state applied after each world join and each switch to this avatar.
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub states: Vec<SavedState>,
}

/// Every avatar's states: `avatar-states.json` beside the settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Store {
    pub version: u32,
    #[serde(default)]
    pub avatars: BTreeMap<String, AvatarStates>,
}

impl Default for Store {
    fn default() -> Self {
        Self { version: VERSION, avatars: BTreeMap::new() }
    }
}

/// `<config>/avatar-states.json`, beside `settings.json`.
pub fn file() -> PathBuf {
    orbiters_core::paths::config_dir().join("avatar-states.json")
}

/// How reading the file went.
#[derive(Clone, Debug, PartialEq)]
pub enum Loaded {
    /// All of it (nothing when there was no file yet).
    Whole,
    /// Some of it could not be read (a hand edit, a newer version's file): what could be is kept, and the file was
    /// copied aside first (`<name>.bad-<seconds>`), so the next save loses nothing.
    Partial(Option<PathBuf>),
    /// The file could not be read at all (another program holds it): saving over it would lose it.
    Unreadable(String),
}

/// Why a change to the states was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    NoSuchState,
    Full,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::NoSuchState => f.write_str("That state is not saved for this avatar any more."),
            Refused::Full => write!(f, "This avatar has {MAX_STATES} states already: delete one first."),
        }
    }
}

impl Store {
    /// The store in `path`, and how reading it went.
    pub fn read(path: &Path) -> (Self, Loaded) {
        let text = match orbiters_core::read_text(path) {
            Ok(None) => return (Self::default(), Loaded::Whole),
            Ok(Some(text)) => text,
            Err(e) => return (Self::default(), Loaded::Unreadable(e.to_string())),
        };
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        if let Ok(store) = serde_json::from_str::<Store>(text) {
            if store.version <= VERSION {
                return (Self { version: VERSION, ..store }, Loaded::Whole);
            }
        }
        // A state at a time: what can be read stays (a newer version's file is read so too, and kept aside).
        let value = serde_json::from_str::<serde_json::Value>(text).unwrap_or_default();
        let mut store = Self::default();
        if let Some(avatars) = value.get("avatars").and_then(|a| a.as_object()) {
            for (id, avatar) in avatars {
                let states: Vec<SavedState> = avatar
                    .get("states")
                    .and_then(|s| s.as_array())
                    .map(|list| list.iter().filter_map(|s| serde_json::from_value(s.clone()).ok()).collect())
                    .unwrap_or_default();
                let name = avatar.get("name").and_then(|n| n.as_str()).map(str::to_string);
                let default = avatar.get("default").and_then(|d| d.as_str()).filter(|d| states.iter().any(|s| s.id == *d)).map(str::to_string);
                store.avatars.insert(id.clone(), AvatarStates { name, default, states });
            }
        }
        (store, Loaded::Partial(orbiters_core::keep_aside(path)))
    }

    /// Writes the whole file at once: a crash leaves the old one or this one.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        orbiters_core::write_atomic(path, serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }

    /// The states of `avatar`, when it has some.
    pub fn avatar(&self, avatar: &str) -> Option<&AvatarStates> {
        self.avatars.get(avatar)
    }

    pub fn state(&self, avatar: &str, id: &str) -> Option<&SavedState> {
        self.avatar(avatar)?.states.iter().find(|s| s.id == id)
    }

    /// The state applied on its own after a world join or a switch to `avatar`.
    pub fn default_of(&self, avatar: &str) -> Option<&SavedState> {
        let states = self.avatar(avatar)?;
        let id = states.default.as_deref()?;
        states.states.iter().find(|s| s.id == id)
    }

    /// Adds a state for `avatar` (named `name`, or "State n" when it has none), and returns it. `avatar_name` is what
    /// VRChat's log calls the avatar.
    pub fn add(
        &mut self,
        avatar: &str,
        avatar_name: Option<&str>,
        name: Option<&str>,
        parameters: BTreeMap<String, OscValue>,
        eye_height: Option<f32>,
        saved_utc: String,
    ) -> Result<&SavedState, Refused> {
        let entry = self.avatars.entry(avatar.to_string()).or_default();
        if entry.states.len() >= MAX_STATES {
            return Err(Refused::Full);
        }
        if let Some(n) = avatar_name.map(str::trim).filter(|n| !n.is_empty()) {
            entry.name = Some(n.to_string());
        }
        let name = match name.map(clean_name).filter(|n| !n.is_empty()) {
            Some(name) => name,
            None => crate::menu::next_state_name(&rows(entry)),
        };
        let id = next_id(entry);
        entry.states.push(SavedState { id, name, saved_utc, pose: false, size: false, parameters, eye_height });
        Ok(entry.states.last().expect("just added"))
    }

    pub fn rename(&mut self, avatar: &str, id: &str, name: &str) -> Result<(), Refused> {
        let name = clean_name(name);
        let state = self.state_mut(avatar, id)?;
        if !name.is_empty() {
            state.name = name;
        }
        Ok(())
    }

    /// Whether applying state `id` puts the GoGoLoco pose and the avatar's size back too.
    pub fn set_options(&mut self, avatar: &str, id: &str, pose: bool, size: bool) -> Result<(), Refused> {
        let state = self.state_mut(avatar, id)?;
        state.pose = pose;
        state.size = size;
        Ok(())
    }

    /// Makes `id` the avatar's default state (`None`: none).
    pub fn set_default(&mut self, avatar: &str, id: Option<&str>) -> Result<(), Refused> {
        if let Some(id) = id {
            self.state_mut(avatar, id)?;
        }
        if let Some(entry) = self.avatars.get_mut(avatar) {
            entry.default = id.map(str::to_string);
        }
        Ok(())
    }

    /// Deletes state `id` (the default goes with it); an avatar left without states goes too.
    pub fn delete(&mut self, avatar: &str, id: &str) -> Result<(), Refused> {
        let entry = self.avatars.get_mut(avatar).ok_or(Refused::NoSuchState)?;
        let before = entry.states.len();
        entry.states.retain(|s| s.id != id);
        if entry.states.len() == before {
            return Err(Refused::NoSuchState);
        }
        if entry.default.as_deref() == Some(id) {
            entry.default = None;
        }
        if entry.states.is_empty() {
            self.avatars.remove(avatar);
        }
        Ok(())
    }

    fn state_mut(&mut self, avatar: &str, id: &str) -> Result<&mut SavedState, Refused> {
        self.avatars.get_mut(avatar).and_then(|a| a.states.iter_mut().find(|s| s.id == id)).ok_or(Refused::NoSuchState)
    }
}

/// An avatar's states as the menu lists them.
pub fn rows(states: &AvatarStates) -> Vec<crate::menu::StateRow> {
    states
        .states
        .iter()
        .map(|s| crate::menu::StateRow {
            id: s.id.clone(),
            name: s.name.clone(),
            default: states.default.as_deref() == Some(s.id.as_str()),
            pose: s.pose,
            size: s.size,
        })
        .collect()
}

/// A name as typed, kept: one line, trimmed, at most [`NAME_MAX`] characters.
fn clean_name(name: &str) -> String {
    let one_line: String = name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    crate::menu::cut(one_line.split_whitespace().collect::<Vec<_>>().join(" ").as_str(), NAME_MAX)
}

/// `s<n>`, one past the highest number an id of the avatar's has.
fn next_id(states: &AvatarStates) -> String {
    let highest = states.states.iter().filter_map(|s| s.id.strip_prefix('s').and_then(|n| n.parse::<u64>().ok())).max().unwrap_or(0);
    format!("s{}", highest + 1)
}

/// Whether `name` is part of GoGoLoco's pose (put back only when the state says so).
pub fn is_pose(name: &str) -> bool {
    name.starts_with(POSE_PREFIX) || name == POSE_EMOTE
}

/// Whether VRChat's avatar parameter `name` belongs in a saved state: OSC may write it (VRChat's own, `VRMode` or
/// `GestureLeft`, are read only), it is not N4N0's own, nor one of `withheld` (Orbiters - N4N0's capture kit toggles:
/// putting one back would start a recording).
pub fn kept(name: &str, writable: bool, withheld: &[&str]) -> bool {
    writable && !name.is_empty() && !name.starts_with(N4N0_PREFIX) && !withheld.contains(&name)
}

/// An avatar parameter as VRChat's OSCQuery tree says it now.
#[derive(Clone, Debug, PartialEq)]
pub struct Current {
    pub value: OscValue,
    /// OSC may write it.
    pub writable: bool,
}

/// What to save of the avatar's parameters (`current`, by name): those [`kept`] says.
pub fn capture(current: &BTreeMap<String, Current>, withheld: &[&str]) -> BTreeMap<String, OscValue> {
    current
        .iter()
        .filter(|(name, c)| kept(name, c.writable, withheld) && !matches!(c.value, OscValue::Text(_)))
        .map(|(name, c)| (name.clone(), c.value.clone()))
        .collect()
}

/// What happened to the avatar's size in an apply.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    /// The state does not put it back.
    NotAsked,
    /// The state has none (VRChat did not say it when it was saved).
    NotSaved,
    /// The world does not let avatars be scaled: VRChat would ignore it.
    Forbidden,
    /// Sent.
    Sent(f32),
}

/// What an apply sends, and what it leaves out.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The messages, in order: the parameters, then the eye height.
    pub messages: Vec<(String, OscType)>,
    /// Saved parameters the avatar no longer has, or no longer lets OSC write: left out.
    pub missing: Vec<String>,
    /// Of those sent, how many the avatar seemed to hold already.
    pub unchanged: usize,
    /// GoGoLoco's pose parameters left out (the state does not put the pose back).
    pub pose_left: usize,
    pub size: Size,
}

impl Plan {
    /// A line for the person: what was put back and what not.
    pub fn summary(&self, name: &str) -> String {
        let mut parts = Vec::new();
        if !self.missing.is_empty() {
            let n = self.missing.len();
            parts.push(format!("{n} parameter{} this avatar no longer has left out", if n == 1 { "" } else { "s" }));
        }
        match self.size {
            Size::Forbidden => parts.push("this world does not allow changing your size".into()),
            Size::NotSaved => parts.push("no size was saved with it".into()),
            _ => {}
        }
        if parts.is_empty() {
            format!("Applied \"{name}\"")
        } else {
            format!("Applied \"{name}\": {}", parts.join(", "))
        }
    }
}

/// The messages that put the avatar back as `state` holds it: every saved parameter, forced. `current` is the avatar's
/// parameters as VRChat's tree has them now (by name); when it is empty (VRChat's tree not read), every saved parameter
/// goes as saved. A saved parameter the avatar lacks, or that OSC may no longer write, is left out; the others go in the
/// type the avatar has now. N4N0's parameters never go; GoGoLoco's pose only with `pose`, the eye height only with
/// `size` and where the world allows scaling (`scaling`: as VRChat said, `None` when unknown: then it is tried).
pub fn plan(state: &SavedState, current: &BTreeMap<String, Current>, scaling: Option<bool>) -> Plan {
    let prefix = orbiters_core::osc::PARAMETERS_PREFIX;
    let mut plan = Plan { messages: Vec::new(), missing: Vec::new(), unchanged: 0, pose_left: 0, size: Size::NotAsked };
    for (name, saved) in &state.parameters {
        if name.starts_with(N4N0_PREFIX) || matches!(saved, OscValue::Text(_)) {
            continue;
        }
        if is_pose(name) && !state.pose {
            plan.pose_left += 1;
            continue;
        }
        let arg = if current.is_empty() {
            as_osc(saved)
        } else {
            match current.get(name) {
                Some(now) if now.writable => {
                    let Some(arg) = like(saved, &now.value) else {
                        plan.missing.push(name.clone());
                        continue;
                    };
                    // Forced, as saved, even where the avatar seems to hold it already (VRChat's tree may be a moment
                    // late): applying a state is a promise of how the avatar ends up.
                    if same(&arg, &now.value) {
                        plan.unchanged += 1;
                    }
                    arg
                }
                _ => {
                    plan.missing.push(name.clone());
                    continue;
                }
            }
        };
        plan.messages.push((format!("{prefix}{name}"), arg));
    }
    if state.size {
        plan.size = match state.eye_height {
            None => Size::NotSaved,
            Some(_) if scaling == Some(false) => Size::Forbidden,
            Some(height) => {
                plan.messages.push((EYE_HEIGHT.to_string(), OscType::Float(height)));
                Size::Sent(height)
            }
        };
    }
    plan
}

/// A saved value as OSC sends it.
fn as_osc(value: &OscValue) -> OscType {
    match value {
        OscValue::Bool(b) => OscType::Bool(*b),
        OscValue::Int(i) => OscType::Int(*i),
        OscValue::Float(f) => OscType::Float(*f),
        OscValue::Text(t) => OscType::String(t.clone()),
    }
}

/// `saved` in the type of `now` (a parameter whose type changed since: a bool that became an int, …); `None` for a
/// text, which no avatar parameter is.
fn like(saved: &OscValue, now: &OscValue) -> Option<OscType> {
    let number = |v: &OscValue| -> Option<f32> {
        Some(match v {
            OscValue::Bool(b) => *b as i32 as f32,
            OscValue::Int(i) => *i as f32,
            OscValue::Float(f) if f.is_finite() => *f,
            _ => return None,
        })
    };
    Some(match now {
        OscValue::Bool(_) => OscType::Bool(match saved {
            OscValue::Bool(b) => *b,
            other => number(other)? > 0.5,
        }),
        OscValue::Int(_) => OscType::Int(number(saved)?.round() as i32),
        OscValue::Float(_) => OscType::Float(number(saved)?),
        OscValue::Text(_) => return None,
    })
}

/// Whether `arg` is what the avatar holds already (floats within a 255th, the step of a synced float).
fn same(arg: &OscType, now: &OscValue) -> bool {
    match (arg, now) {
        (OscType::Bool(a), OscValue::Bool(b)) => a == b,
        (OscType::Int(a), OscValue::Int(b)) => a == b,
        (OscType::Float(a), OscValue::Float(b)) => (a - b).abs() < 0.5 / 255.0,
        _ => false,
    }
}

/// `messages` in steps of [`STEP_MESSAGES`]: Orbiters - N4N0 sends one step at a time, a few tens of milliseconds
/// apart, so that VRChat takes them all (a few hundred at once would overflow what it reads).
pub fn paced(messages: Vec<(String, OscType)>) -> Vec<Vec<(String, OscType)>> {
    let mut steps = Vec::new();
    let mut it = messages.into_iter().peekable();
    while it.peek().is_some() {
        steps.push(it.by_ref().take(STEP_MESSAGES).collect());
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current(entries: &[(&str, OscValue, bool)]) -> BTreeMap<String, Current> {
        entries.iter().map(|(n, v, w)| (n.to_string(), Current { value: v.clone(), writable: *w })).collect()
    }

    fn avatar_now() -> BTreeMap<String, Current> {
        current(&[
            ("Hoodie", OscValue::Bool(true), true),
            ("Hue", OscValue::Float(0.25), true),
            ("Outfit", OscValue::Int(3), true),
            ("Go/Stationary", OscValue::Bool(true), true),
            ("Go/Height", OscValue::Float(0.4), true),
            ("VRCEmote", OscValue::Int(7), true),
            ("N4N0/Visible", OscValue::Bool(true), true),
            ("N4N0/Light/Color", OscValue::Int(2), true),
            ("OrbitersRecord", OscValue::Bool(false), true),
            ("VRMode", OscValue::Int(1), false),
            ("GestureLeft", OscValue::Int(2), false),
            ("Viseme", OscValue::Int(0), false),
        ])
    }

    #[test]
    fn saves_what_osc_may_write_but_never_n4n0_s_own_nor_the_capture_kit_s() {
        let saved = capture(&avatar_now(), &["OrbitersRecord", "OrbitersDepthFrames"]);
        let names: Vec<&str> = saved.keys().map(String::as_str).collect();
        assert_eq!(names, vec!["Go/Height", "Go/Stationary", "Hoodie", "Hue", "Outfit", "VRCEmote"], "GoGoLoco's are saved too");
        assert!(
            kept("Hoodie", true, &[]) && !kept("VRMode", false, &[]) && !kept("N4N0/Menu", true, &[]) && !kept("OrbitersRecord", true, &["OrbitersRecord"])
        );
        assert!(is_pose("Go/PoseY") && is_pose("VRCEmote") && !is_pose("Gogo") && !is_pose("Hoodie"));
    }

    fn state(pose: bool, size: bool) -> SavedState {
        SavedState {
            id: "s1".into(),
            name: "Cosy".into(),
            saved_utc: "2026-10-25T10:00:00Z".into(),
            pose,
            size,
            parameters: [
                ("Hoodie", OscValue::Bool(false)),
                ("Hue", OscValue::Float(0.25)),
                ("Outfit", OscValue::Int(1)),
                ("Tail", OscValue::Bool(true)),
                ("Go/Stationary", OscValue::Bool(false)),
                ("VRCEmote", OscValue::Int(0)),
                // From an older file, or by hand: never sent.
                ("N4N0/Visible", OscValue::Bool(false)),
            ]
            .into_iter()
            .map(|(n, v)| (n.to_string(), v))
            .collect(),
            eye_height: Some(1.42),
        }
    }

    fn sent(plan: &Plan) -> Vec<(String, OscType)> {
        plan.messages.clone()
    }

    #[test]
    fn applies_what_changed_and_leaves_out_what_the_avatar_lost() {
        let plan = plan(&state(false, false), &avatar_now(), Some(true));
        let p = orbiters_core::osc::PARAMETERS_PREFIX;
        let expected = vec![(format!("{p}Hoodie"), OscType::Bool(false)), (format!("{p}Hue"), OscType::Float(0.25)), (format!("{p}Outfit"), OscType::Int(1))];
        assert_eq!(sent(&plan), expected, "forced, Hue too though the avatar holds it already");
        assert_eq!(plan.missing, vec!["Tail".to_string()], "the avatar has no Tail any more");
        assert_eq!((plan.unchanged, plan.pose_left, plan.size), (1, 2, Size::NotAsked), "the pose stays as it is without its box");
        assert_eq!(plan.summary("Cosy"), "Applied \"Cosy\": 1 parameter this avatar no longer has left out");
    }

    #[test]
    fn the_pose_and_the_size_go_only_when_asked_and_allowed() {
        let p = orbiters_core::osc::PARAMETERS_PREFIX;
        let plan = plan(&state(true, true), &avatar_now(), Some(true));
        let names: Vec<String> = plan.messages.iter().map(|(a, _)| a.clone()).collect();
        let expected =
            [format!("{p}Go/Stationary"), format!("{p}Hoodie"), format!("{p}Hue"), format!("{p}Outfit"), format!("{p}VRCEmote"), EYE_HEIGHT.to_string()];
        assert_eq!(names, expected);
        assert_eq!(plan.messages.last().unwrap().1, OscType::Float(1.42));
        assert_eq!(plan.size, Size::Sent(1.42));
        let forbidden = super::plan(&state(false, true), &avatar_now(), Some(false));
        assert_eq!(forbidden.size, Size::Forbidden);
        assert!(forbidden.messages.iter().all(|(a, _)| a != EYE_HEIGHT));
        assert!(forbidden.summary("Cosy").contains("does not allow changing your size"));
        let unknown = super::plan(&state(false, true), &avatar_now(), None);
        assert_eq!(unknown.size, Size::Sent(1.42), "not known: tried");
        let mut no_height = state(false, true);
        no_height.eye_height = None;
        assert_eq!(super::plan(&no_height, &avatar_now(), Some(true)).size, Size::NotSaved);
        assert!(plan.messages.iter().all(|(a, _)| !a.contains("N4N0/")), "N4N0's own never go");
    }

    #[test]
    fn values_go_in_the_type_the_avatar_has_now_and_unknown_avatars_get_them_as_saved() {
        let now = current(&[("Hoodie", OscValue::Int(1), true), ("Outfit", OscValue::Float(0.0), true), ("Hue", OscValue::Bool(false), true)]);
        let mut s = state(false, false);
        s.parameters.retain(|n, _| ["Hoodie", "Outfit", "Hue"].contains(&n.as_str()));
        let plan = plan(&s, &now, None);
        let args: Vec<OscType> = plan.messages.iter().map(|(_, a)| a.clone()).collect();
        assert_eq!(args, vec![OscType::Int(0), OscType::Bool(false), OscType::Float(1.0)], "Hoodie as an int, Hue as a bool, Outfit as a float");
        assert_eq!(plan.unchanged, 1, "Hue held false already");
        // VRChat's tree not read: everything as saved (the pose still waits for its box).
        let blind = super::plan(&state(false, false), &BTreeMap::new(), None);
        assert_eq!(blind.messages.len(), 4);
        assert!(blind.missing.is_empty());
        // A parameter OSC may no longer write is left out.
        let read_only = current(&[("Hoodie", OscValue::Bool(true), false)]);
        let mut only_hoodie = state(false, false);
        only_hoodie.parameters.retain(|n, _| n == "Hoodie");
        assert_eq!(super::plan(&only_hoodie, &read_only, None).missing, vec!["Hoodie".to_string()]);
    }

    #[test]
    fn applies_go_in_steps_never_all_at_once() {
        let messages: Vec<(String, OscType)> = (0..40).map(|i| (format!("/avatar/parameters/P{i}"), OscType::Int(i))).collect();
        let steps = paced(messages.clone());
        assert_eq!(steps.iter().map(Vec::len).collect::<Vec<_>>(), vec![16, 16, 8]);
        assert_eq!(steps.concat(), messages, "in order");
        assert!(paced(Vec::new()).is_empty());
    }

    #[test]
    fn keeps_states_by_avatar_with_one_default() {
        let mut store = Store::default();
        let params = || BTreeMap::from([("Hoodie".to_string(), OscValue::Bool(true))]);
        let a = "avtr_a";
        let id1 = store.add(a, Some("Rexouium"), None, params(), Some(1.6), "t1".into()).unwrap().id.clone();
        let id2 = store.add(a, None, Some("  Hoodie\non  "), params(), None, "t2".into()).unwrap().id.clone();
        assert_eq!((id1.as_str(), id2.as_str()), ("s1", "s2"));
        assert_eq!(store.state(a, &id1).unwrap().name, "State 1");
        assert_eq!(store.state(a, &id2).unwrap().name, "Hoodie on", "one line, trimmed");
        assert_eq!(store.avatar(a).unwrap().name.as_deref(), Some("Rexouium"), "the name VRChat's log gave it stays");
        assert!(!store.state(a, &id1).unwrap().pose && !store.state(a, &id1).unwrap().size, "the boxes are off by default");
        store.set_default(a, Some(&id2)).unwrap();
        assert_eq!(store.default_of(a).map(|s| s.id.as_str()), Some("s2"));
        assert_eq!(store.set_default(a, Some("s9")), Err(Refused::NoSuchState));
        assert_eq!(store.default_of(a).map(|s| s.id.as_str()), Some("s2"), "a refused default changes nothing");
        store.set_options(a, &id1, true, false).unwrap();
        store.rename(a, &id1, "Sitting").unwrap();
        store.rename(a, &id1, "   ").unwrap();
        assert_eq!(store.state(a, &id1).map(|s| (s.name.as_str(), s.pose, s.size)), Some(("Sitting", true, false)), "an empty name changes nothing");
        let rows = rows(store.avatar(a).unwrap());
        assert_eq!(rows.iter().map(|r| (r.name.as_str(), r.default, r.pose)).collect::<Vec<_>>(), vec![("Sitting", false, true), ("Hoodie on", true, false)]);
        // A third with no name: "State 3"; deleting the default leaves none; ids are never reused.
        assert_eq!(store.add(a, None, None, params(), None, "t3".into()).unwrap().name, "State 3");
        store.delete(a, &id2).unwrap();
        assert!(store.default_of(a).is_none());
        assert_eq!(store.add(a, None, Some(""), params(), None, "t4".into()).unwrap().id, "s4");
        assert_eq!(store.delete(a, "s2"), Err(Refused::NoSuchState));
        for id in ["s1", "s3", "s4"] {
            store.delete(a, id).unwrap();
        }
        assert!(store.avatar(a).is_none(), "an avatar without states goes");
        assert!(store.default_of("avtr_none").is_none());
    }

    #[test]
    fn an_avatar_holds_so_many_states_at_most() {
        let mut store = Store::default();
        for _ in 0..MAX_STATES {
            store.add("avtr_full", None, None, BTreeMap::new(), None, "t".into()).unwrap();
        }
        assert_eq!(store.add("avtr_full", None, None, BTreeMap::new(), None, "t".into()).err(), Some(Refused::Full));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("n4n0-states-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_file_is_versioned_and_read_back_whole() {
        let dir = temp_dir("whole");
        let path = dir.join("avatar-states.json");
        assert_eq!(Store::read(&path), (Store::default(), Loaded::Whole), "no file yet");
        let mut store = Store::default();
        let params = BTreeMap::from([("Hoodie".to_string(), OscValue::Bool(true)), ("Hue".to_string(), OscValue::Float(0.5))]);
        let id = store.add("avtr_a", Some("Canine"), None, params, Some(1.55), "2026-10-25T10:00:00Z".into()).unwrap().id.clone();
        store.set_default("avtr_a", Some(&id)).unwrap();
        store.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"version\": 1") && text.contains("\"type\": \"bool\""), "{text}");
        assert_eq!(Store::read(&path), (store, Loaded::Whole));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_file_read_in_part_is_kept_aside_before_it_is_replaced() {
        let dir = temp_dir("partial");
        let path = dir.join("avatar-states.json");
        // A newer version's file, one of its states in a shape this version does not know.
        let text = r#"{ "version": 7, "avatars": { "avtr_a": { "name": "Canine", "default": "s2", "states": [
            { "id": "s1", "name": "Kept", "saved_utc": "t", "parameters": { "Hoodie": { "type": "bool", "value": true } } },
            { "id": "s2", "name": "Odd", "saved_utc": 12 } ] } } }"#;
        std::fs::write(&path, text).unwrap();
        let (store, loaded) = Store::read(&path);
        let Loaded::Partial(Some(aside)) = loaded else { panic!("{loaded:?}") };
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), text, "the whole file, as it was");
        let avatar = store.avatar("avtr_a").unwrap();
        assert_eq!(avatar.states.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["Kept"]);
        assert_eq!(avatar.default, None, "the default was the state that could not be read");
        assert_eq!(store.version, VERSION, "saved as this version's file");
        std::fs::remove_dir_all(dir).ok();
    }
}
