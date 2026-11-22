//! Where a project stands between Unity 2022 and Unity 6, read from its own files only: the editor its
//! `ProjectSettings/ProjectVersion.txt` names, the VRChat SDK its VPM manifest asks for and locks, and the SDK folders
//! actually installed in `Packages/`. Nothing is resolved, downloaded or written: this runs beside the scan and must cost
//! no more than three small reads (CLAUDE.md: scans never wait on the network).
//!
//! The state says what the files show today, never how they got there: a project VCC upgraded in place, a new Unity 6
//! project and a restored copy look the same. Only Orbiters' own record of a trial ([`super::trial`]) says "made by
//! Orbiters".

use super::matrix::Matrix;
use serde::{Deserialize, Serialize};
use std::path::Path;
use vrc_get_vpm::version::{UnityVersion, Version};

/// The SDK packages VRChat publishes in its official listing.
pub const SDK_BASE: &str = "com.vrchat.base";
pub const SDK_AVATARS: &str = "com.vrchat.avatars";
pub const SDK_WORLDS: &str = "com.vrchat.worlds";

/// What `ProjectVersion.txt` says.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorStamp {
    /// `m_EditorVersion` as written; empty when the file or the line is missing.
    pub text: String,
    /// The changeset `m_EditorVersionWithRevision` names, when the file has that line (Unity 2019 and older do not).
    pub revision: Option<String>,
    /// Whether `text` is a Unity version (vrc-get-vpm's parser, the one VCC's resolver uses).
    pub parsed: bool,
}

impl EditorStamp {
    pub fn read(project: &Path) -> Self {
        match std::fs::read_to_string(project.join("ProjectSettings/ProjectVersion.txt")) {
            Ok(text) => Self::parse(&text),
            Err(_) => Self::default(),
        }
    }

    pub fn parse(file: &str) -> Self {
        let text = crate::discovery::parse_version(file).unwrap_or_default();
        let revision = file.lines().find_map(|l| {
            let rest = l.trim().strip_prefix("m_EditorVersionWithRevision:")?;
            let inside = rest.split_once('(')?.1.split_once(')')?.0.trim();
            (!inside.is_empty()).then(|| inside.to_string())
        });
        let parsed = UnityVersion::parse(&text).is_some();
        Self { text, revision, parsed }
    }

    pub fn version(&self) -> Option<UnityVersion> {
        UnityVersion::parse(&self.text)
    }

    /// The editor's line, `2022.3` or `6000.0`.
    pub fn line(&self) -> Option<String> {
        self.version().map(|v| format!("{}.{}", v.major(), v.minor()))
    }
}

/// What a project is for, by the SDK package it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Content {
    Avatars,
    Worlds,
    /// The base SDK alone (or another `com.vrchat.*` package).
    Base,
}

/// The SDK as the project's files show it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkEvidence {
    pub content: Option<Content>,
    /// What `vpm-manifest.json` asks for (its `dependencies`), for the content package or the base.
    pub requested: Option<String>,
    /// What it locked for `com.vrchat.base`.
    pub locked: Option<String>,
    /// What `Packages/com.vrchat.base/package.json` says is installed.
    pub installed: Option<String>,
    /// The Unity line the installed base SDK declares it needs (its `unity` field, `2022.3` or `6000.0`).
    pub needs: Option<String>,
    /// SDK packages the lock names that have no folder in `Packages/`.
    pub missing: Vec<String>,
    /// SDK packages whose installed version is not the one locked.
    pub mismatched: Vec<String>,
}

impl SdkEvidence {
    /// The SDK version to go by: what is installed, else what is locked.
    pub fn version(&self) -> Option<&str> {
        self.installed.as_deref().or(self.locked.as_deref())
    }
}

/// Where a project stands. Each state is what the files show; none is a guess about the project's history.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    /// No VRChat SDK anywhere: a plain Unity project, left alone.
    #[default]
    NotVrchat,
    /// The SDK lives in `Assets/VRCSDK` (made before Creator Companion): not part of this transition.
    LegacySdk,
    /// `ProjectVersion.txt` is missing or names no Unity version.
    EditorUnknown,
    /// The lock names the base SDK, but no `Packages/com.vrchat.base` is there.
    SdkMissing,
    /// Part of the SDK is missing, or is not the version the lock names.
    SdkIncomplete,
    /// On the Unity 2022 line with an SDK that needs it.
    On2022,
    /// On the Unity 6 line with an SDK that needs it.
    OnUnity6,
    /// The editor's line is not the one the installed SDK needs (in either direction).
    Disagree,
    /// The editor and the SDK agree on a line before 2022 (Unity 2019): not part of this transition.
    OtherLine,
    /// The SDK says no line and the release list does not know its version.
    LineUnknown,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standing {
    pub editor: EditorStamp,
    pub sdk: SdkEvidence,
    pub state: State,
    /// Whether the SDK is a beta: from the release list when it knows the version, else from the version itself (a
    /// pre-release is one); `None` when neither says.
    pub beta: Option<bool>,
}

impl Standing {
    /// A VRChat project of the transition (2022 or Unity 6, or one between them).
    pub fn in_transition(&self) -> bool {
        !matches!(self.state, State::NotVrchat | State::LegacySdk | State::OtherLine)
    }
}

fn json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn version_of(entry: &serde_json::Value) -> Option<String> {
    entry.as_str().or_else(|| entry["version"].as_str()).map(str::trim).filter(|s| !s.is_empty()).map(ToOwned::to_owned)
}

/// The line `major.minor` of a package's `unity` field (`2022.3`, `6000.0`; `2019.4.30` gives `2019.4`).
fn line_of_field(field: &str) -> Option<String> {
    let mut parts = field.trim().split('.');
    let major: u16 = parts.next()?.parse().ok()?;
    let minor: u8 = parts.next()?.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()?;
    Some(format!("{major}.{minor}"))
}

/// Reads where `project` stands. Opens files for reading only (shared, so never in Unity's or VCC's way) and writes
/// nothing.
pub fn read(project: &Path, matrix: &Matrix) -> Standing {
    let editor = EditorStamp::read(project);
    let manifest = json(&project.join("Packages/vpm-manifest.json"));
    let section = |name: &str| manifest.as_ref().and_then(|m| m[name].as_object().cloned()).unwrap_or_default();
    let (dependencies, locked) = (section("dependencies"), section("locked"));
    let installed_version = |name: &str| json(&project.join("Packages").join(name).join("package.json")).and_then(|p| version_of(&p["version"]));
    let named = |name: &str| dependencies.contains_key(name) || locked.contains_key(name);
    let on_disk = |name: &str| project.join("Packages").join(name).join("package.json").is_file();
    let any_vrchat = dependencies.keys().chain(locked.keys()).any(|k| k.starts_with("com.vrchat."))
        || [SDK_BASE, SDK_AVATARS, SDK_WORLDS].iter().any(|n| on_disk(n));
    if !any_vrchat {
        let state = if project.join("Assets/VRCSDK").is_dir() { State::LegacySdk } else { State::NotVrchat };
        return Standing { editor, state, ..Default::default() };
    }
    let content = if named(SDK_AVATARS) || on_disk(SDK_AVATARS) {
        Content::Avatars
    } else if named(SDK_WORLDS) || on_disk(SDK_WORLDS) {
        Content::Worlds
    } else {
        Content::Base
    };
    let content_name = match content {
        Content::Avatars => SDK_AVATARS,
        Content::Worlds => SDK_WORLDS,
        Content::Base => SDK_BASE,
    };
    let requested = dependencies.get(content_name).or(dependencies.get(SDK_BASE)).and_then(version_of);
    let locked_base = locked.get(SDK_BASE).and_then(version_of);
    let installed = installed_version(SDK_BASE);
    let needs = json(&project.join("Packages").join(SDK_BASE).join("package.json")).and_then(|p| p["unity"].as_str().and_then(line_of_field));
    let mut missing = vec![];
    let mut mismatched = vec![];
    for name in [SDK_BASE, SDK_AVATARS, SDK_WORLDS] {
        let Some(want) = locked.get(name).and_then(version_of) else { continue };
        match installed_version(name) {
            None => missing.push(name.to_string()),
            Some(have) if have != want => mismatched.push(name.to_string()),
            Some(_) => {}
        }
    }
    // Asked for and neither locked nor installed: what VCC would have to restore first.
    if missing.is_empty() && installed.is_none() && locked_base.is_none() && requested.is_some() {
        missing.push(SDK_BASE.to_string());
    }
    let sdk = SdkEvidence { content: Some(content), requested, locked: locked_base, installed, needs, missing, mismatched };
    let known = sdk.version().and_then(|v| matrix.row_for(v));
    let beta = match known {
        Some(row) => Some(row.channel == super::matrix::Channel::Beta),
        None => sdk.version().and_then(|v| v.parse::<Version>().ok()).map(|v| v.is_pre()),
    };
    let state = if !editor.parsed {
        State::EditorUnknown
    } else if sdk.missing.iter().any(|m| m == SDK_BASE) {
        State::SdkMissing
    } else if !sdk.missing.is_empty() || !sdk.mismatched.is_empty() {
        State::SdkIncomplete
    } else {
        let needs = sdk.needs.clone().or_else(|| known.map(|r| r.line()));
        match (needs, editor.line()) {
            (None, _) => State::LineUnknown,
            (Some(needs), Some(line)) if needs == line => {
                if needs.starts_with("2022.") {
                    State::On2022
                } else if needs.split('.').next().and_then(|m| m.parse::<u16>().ok()).is_some_and(|m| (6000..7000).contains(&m)) {
                    State::OnUnity6
                } else {
                    State::OtherLine
                }
            }
            (Some(_), _) => State::Disagree,
        }
    };
    Standing { editor, sdk, state, beta }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A VRChat project in a temporary folder: its editor stamp, its manifest, the SDK packages installed (name,
    /// version, the Unity line they declare).
    pub(crate) fn project(root: &Path, editor: Option<&str>, locked: &[(&str, &str)], installed: &[(&str, &str, &str)]) {
        std::fs::create_dir_all(root.join("ProjectSettings")).unwrap();
        std::fs::create_dir_all(root.join("Assets")).unwrap();
        if let Some(editor) = editor {
            std::fs::write(root.join("ProjectSettings/ProjectVersion.txt"), editor).unwrap();
        }
        if !locked.is_empty() {
            let deps: serde_json::Map<_, _> = locked.iter().map(|(n, v)| (n.to_string(), serde_json::json!({ "version": v }))).collect();
            let lock: serde_json::Map<_, _> = locked.iter().map(|(n, v)| (n.to_string(), serde_json::json!({ "version": v, "dependencies": {} }))).collect();
            std::fs::create_dir_all(root.join("Packages")).unwrap();
            std::fs::write(root.join("Packages/vpm-manifest.json"), serde_json::json!({ "dependencies": deps, "locked": lock }).to_string()).unwrap();
        }
        for (name, version, unity) in installed {
            let dir = root.join("Packages").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("package.json"), serde_json::json!({ "name": name, "version": version, "unity": unity }).to_string()).unwrap();
        }
    }

    pub(crate) const U2022: &str = "m_EditorVersion: 2022.3.22f1\nm_EditorVersionWithRevision: 2022.3.22f1 (887be4894c44)\n";
    pub(crate) const U6: &str = "m_EditorVersion: 6000.0.67f1\nm_EditorVersionWithRevision: 6000.0.67f1 (78a1c2bbeb6a)\n";

    fn standing(editor: Option<&str>, locked: &[(&str, &str)], installed: &[(&str, &str, &str)]) -> Standing {
        let temp = tempfile::tempdir().unwrap();
        project(temp.path(), editor, locked, installed);
        read(temp.path(), &Matrix::built_in())
    }

    #[test]
    fn every_state_is_what_the_files_show() {
        let sdk2022 = [(SDK_BASE, "3.10.6"), (SDK_AVATARS, "3.10.6")];
        let installed2022 = [(SDK_BASE, "3.10.6", "2022.3"), (SDK_AVATARS, "3.10.6", "2022.3")];
        let sdk6 = [(SDK_BASE, "3.11.0-beta.1"), (SDK_WORLDS, "3.11.0-beta.1")];
        let installed6 = [(SDK_BASE, "3.11.0-beta.1", "6000.0"), (SDK_WORLDS, "3.11.0-beta.1", "6000.0")];

        let s = standing(Some(U2022), &sdk2022, &installed2022);
        assert_eq!((s.state, s.beta, s.sdk.content), (State::On2022, Some(false), Some(Content::Avatars)));
        assert_eq!(s.editor.revision.as_deref(), Some("887be4894c44"));

        let s = standing(Some(U6), &sdk6, &installed6);
        assert_eq!((s.state, s.beta, s.sdk.content), (State::OnUnity6, Some(true), Some(Content::Worlds)));

        // A Unity 6 editor on a 2022 SDK, and the other way round.
        assert_eq!(standing(Some(U6), &sdk2022, &installed2022).state, State::Disagree);
        assert_eq!(standing(Some(U2022), &sdk6, &installed6).state, State::Disagree);

        // The lock names the SDK, nothing is installed; part installed; another version installed.
        assert_eq!(standing(Some(U2022), &sdk2022, &[]).state, State::SdkMissing);
        assert_eq!(standing(Some(U2022), &sdk2022, &installed2022[..1]).state, State::SdkIncomplete);
        assert_eq!(standing(Some(U2022), &sdk2022, &[(SDK_BASE, "3.10.5", "2022.3"), (SDK_AVATARS, "3.10.6", "2022.3")]).state, State::SdkIncomplete);

        // No ProjectVersion.txt, or one naming no version.
        assert_eq!(standing(None, &sdk2022, &installed2022).state, State::EditorUnknown);
        assert_eq!(standing(Some("m_EditorVersion: six\n"), &sdk2022, &installed2022).state, State::EditorUnknown);

        // Not a VRChat project; an SDK in Assets.
        assert_eq!(standing(Some(U2022), &[], &[]).state, State::NotVrchat);
        let temp = tempfile::tempdir().unwrap();
        project(temp.path(), Some(U2022), &[], &[]);
        std::fs::create_dir_all(temp.path().join("Assets/VRCSDK")).unwrap();
        assert_eq!(read(temp.path(), &Matrix::built_in()).state, State::LegacySdk);

        // An old SDK on Unity 2019.
        let old = "m_EditorVersion: 2019.4.31f1\n";
        assert_eq!(standing(Some(old), &[(SDK_BASE, "3.1.13")], &[(SDK_BASE, "3.1.13", "2019.4")]).state, State::OtherLine);

        // An SDK that declares no line, of a version the release list does not know.
        assert_eq!(standing(Some(U6), &[(SDK_BASE, "3.12.0")], &[(SDK_BASE, "3.12.0", "")]).state, State::LineUnknown);
        // The same version without a declared line, but known to the release list: its line is the list's.
        assert_eq!(standing(Some(U6), &[(SDK_BASE, "3.11.0-beta.1")], &[(SDK_BASE, "3.11.0-beta.1", "")]).state, State::OnUnity6);
    }

    #[test]
    fn an_unknown_sdk_is_called_beta_only_when_it_is_a_pre_release() {
        // Not in the release list: a pre-release is a beta, a release says nothing either way here.
        let s = standing(Some(U6), &[(SDK_BASE, "3.11.0-beta.2")], &[(SDK_BASE, "3.11.0-beta.2", "6000.0")]);
        assert_eq!((s.state, s.beta), (State::OnUnity6, Some(true)));
        let s = standing(Some(U6), &[(SDK_BASE, "3.11.0")], &[(SDK_BASE, "3.11.0", "6000.0")]);
        assert_eq!((s.state, s.beta), (State::OnUnity6, Some(false)));
    }

    #[test]
    fn the_editor_stamp_keeps_its_revision_and_parse_apart() {
        let s = EditorStamp::parse("m_EditorVersion: 6000.0.67f1\n");
        assert_eq!((s.text.as_str(), s.revision.as_deref(), s.parsed), ("6000.0.67f1", None, true), "no revision line (older editors)");
        let s = EditorStamp::parse(U6);
        assert_eq!(s.revision.as_deref(), Some("78a1c2bbeb6a"));
        assert_eq!(s.line().as_deref(), Some("6000.0"));
        let s = EditorStamp::parse("m_EditorVersion: 6000.0.x\n");
        assert_eq!((s.text.as_str(), s.parsed, s.line()), ("6000.0.x", false, None));
        // Numeric patches: 100 after 67, not before (a string comparison puts "100" first).
        let (a, b) = (UnityVersion::parse("6000.0.100f1").unwrap(), UnityVersion::parse("6000.0.67f1").unwrap());
        assert!(a > b);
        assert!(UnityVersion::parse("6000.0.67f1").unwrap() > UnityVersion::parse("2022.3.62f1").unwrap());
        assert_eq!(line_of_field("2019.4.30"), Some("2019.4".into()));
        assert_eq!(line_of_field(""), None);
    }

    /// A scan's look at a project writes nothing there: every file's bytes and the folders' entries are the same after.
    #[test]
    fn reading_a_standing_writes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        project(temp.path(), Some(U2022), &[(SDK_BASE, "3.10.6")], &[(SDK_BASE, "3.10.6", "2022.3")]);
        let before = super::super::tests::snapshot(temp.path());
        let _ = read(temp.path(), &Matrix::built_in());
        assert_eq!(super::super::tests::snapshot(temp.path()), before);
    }
}
