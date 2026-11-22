//! What can be said about a project's packages and content before trying it on Unity 6, as separate facts and never
//! as one verdict: where each package comes from, what its own manifest declares (a Unity minimum is a minimum, not a
//! promise), what its publisher or a dated report says (display only, [`super::matrix::Issue`]), and that Orbiters
//! has no test evidence for it. Then the changes VRChat's migration guide lists, tailored to avatars or worlds, each
//! "found in this project", "not found" or "cannot tell" from a bounded read of the project's text files.
//!
//! Local files only: no repository, resolver or network. Read on a worker when the panel shows the project.

use super::matrix::Matrix;
use super::standing::{Content, SDK_AVATARS, SDK_BASE, SDK_WORLDS};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use vrc_get_vpm::version::{PrereleaseAcceptance, Version, VersionRange};

/// Where an installed package comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// Locked in `vpm-manifest.json` and installed by VPM (VCC, ALCOM, Orbiters).
    Vpm,
    /// A folder in `Packages/` that no lock names: embedded, kept as it is.
    Embedded,
    /// From Unity's package manager, a registry.
    UpmRegistry,
    /// From a Git address.
    UpmGit,
    /// A local folder or archive (`file:`).
    UpmLocal,
}

/// Whether a declared range takes the target SDK.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Admits {
    Yes,
    No,
    /// A beta target is inside the range only when pre-releases count (`<3.11.0` and `3.11.0-beta.1`): how Creator
    /// Companion's beta counts them for this resolution is not known here.
    DependsOnPrerelease,
}

/// What a package's own manifest declares about the SDK: a range over `com.vrchat.*`, and whether the target SDK is in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkRange {
    pub range: String,
    pub admits_target: Option<Admits>,
}

fn admits(range: &str, target: &Version) -> Option<Admits> {
    let r = range.parse::<VersionRange>().ok()?;
    Some(match (r.match_pre(target, PrereleaseAcceptance::Allow), r.match_pre(target, PrereleaseAcceptance::Minimum)) {
        (true, true) => Admits::Yes,
        (false, false) => Admits::No,
        _ => Admits::DependsOnPrerelease,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub name: String,
    pub display: String,
    pub version: Option<String>,
    pub source: Source,
    /// Its manifest's `unity` (with `unityRelease`): the oldest editor it asks for.
    pub unity_min: Option<String>,
    pub sdk_range: Option<SdkRange>,
    /// Indices into the release list's issues that are about this package and version.
    pub issues: Vec<usize>,
}

fn json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn text(v: &serde_json::Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(ToOwned::to_owned)
}

/// The installed packages of `project` and what each declares, the SDK's own packages left out (the standing says
/// those). `target`: the SDK version a trial would install.
pub fn packages(project: &Path, matrix: &Matrix, target: &str) -> Vec<Row> {
    let vpm = json(&project.join("Packages/vpm-manifest.json")).unwrap_or_default();
    let locked = vpm["locked"].as_object().cloned().unwrap_or_default();
    let upm = json(&project.join("Packages/manifest.json")).unwrap_or_default();
    let upm_deps = upm["dependencies"].as_object().cloned().unwrap_or_default();
    let target = target.parse::<Version>().ok();
    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    let sdk = |n: &str| [SDK_BASE, SDK_AVATARS, SDK_WORLDS].contains(&n);
    let read_manifest = |name: &str| json(&project.join("Packages").join(name).join("package.json"));
    let describe = |name: &str, source: Source, manifest: Option<serde_json::Value>| -> Row {
        let m = manifest.unwrap_or_default();
        let unity_min = text(&m["unity"]).map(|u| match text(&m["unityRelease"]) {
            Some(r) => format!("{u}.{r}"),
            None => u,
        });
        let sdk_range = m["vpmDependencies"].as_object().and_then(|deps| {
            deps.iter().find(|(k, _)| sdk(k)).and_then(|(_, v)| text(v)).map(|range| SdkRange {
                admits_target: target.as_ref().and_then(|t| admits(&range, t)),
                range,
            })
        });
        let version = text(&m["version"]);
        let issues = matrix.data.issues.iter().enumerate().filter(|(_, i)| i.package == name && i.applies(version.as_deref())).map(|(k, _)| k).collect();
        Row { name: name.to_string(), display: text(&m["displayName"]).unwrap_or_else(|| name.to_string()), version, source, unity_min, sdk_range, issues }
    };
    for name in locked.keys().filter(|n| !sdk(n) && !n.contains(['/', '\\'])) {
        rows.insert(name.clone(), describe(name, Source::Vpm, read_manifest(name)));
    }
    if let Ok(entries) = std::fs::read_dir(project.join("Packages")) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if sdk(&name) || rows.contains_key(&name) || !e.path().join("package.json").is_file() {
                continue;
            }
            let source = if upm_deps.get(&name).and_then(|v| v.as_str()).is_some_and(|v| v.starts_with("file:")) { Source::UpmLocal } else { Source::Embedded };
            rows.insert(name.clone(), describe(&name, source, read_manifest(&name)));
        }
    }
    for (name, value) in &upm_deps {
        if rows.contains_key(name) || name.starts_with("com.unity.modules.") || sdk(name) {
            continue;
        }
        let v = value.as_str().unwrap_or_default();
        let source = if v.starts_with("file:") {
            Source::UpmLocal
        } else if v.starts_with("git") || v.contains(".git") || v.starts_with("https://") {
            Source::UpmGit
        } else {
            Source::UpmRegistry
        };
        // Unity's own packages are listed by name and version only: the editor brings their Unity 6 versions.
        if source == Source::UpmRegistry && name.starts_with("com.unity.") {
            continue;
        }
        let mut row = describe(name, source, None);
        row.version = (source == Source::UpmRegistry).then(|| v.to_string());
        rows.insert(name.clone(), row);
    }
    rows.into_values().collect()
}

/// Whether an item of the checklist was seen in the project's files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Seen {
    Found,
    NotFound,
    /// The files that would show it are not text, or the look stopped before the end.
    CannotTell,
}

/// The changes VRChat's migration guide and announcement list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Item {
    /// UdonSharp scripts: Unity's API updater renames `velocity` (automatic).
    UdonSharp,
    /// Udon Graph programs: the Upgrade Wizard migrates them.
    UdonGraph,
    /// Hand-written or generated Udon Assembly with the old velocity externs: a manual fix.
    CustomUasm,
    /// Scripts using `velocity` on a Rigidbody (renamed `linearVelocity`).
    Velocity,
    /// AddForce and friends with Acceleration or VelocityChange: a manual fix.
    ForceModes,
    /// Particle systems set to Loop Until Replaced: they now stop at Max Particles.
    RingBuffer,
    /// TextMeshPro: its package updated, and a colour conversion in every scene.
    TextMeshPro,
    /// A NavMeshObstacle on a NavMeshAgent's object.
    NavMesh,
    /// Synced avatar parameters are matched by name again for avatars uploaded with SDK 3.11 and later.
    ParameterNames,
    /// Package Maker moved from the VPM Resolver into the base SDK.
    PackageMaker,
    /// Shaders: materials to look at on each platform.
    Shaders,
}

impl Item {
    /// Whether the item concerns this kind of project.
    pub fn applies(self, content: Content) -> bool {
        match self {
            Item::UdonSharp | Item::UdonGraph | Item::CustomUasm | Item::NavMesh => content != Content::Avatars,
            Item::ParameterNames => content == Content::Avatars,
            _ => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub item: Item,
    pub seen: Seen,
    /// Up to a few files where it was seen, relative to the project.
    pub at: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checklist {
    pub content: Option<Content>,
    pub checks: Vec<Check>,
    /// How many scenes the project has: none of them was opened or built by Orbiters.
    pub scenes: usize,
    /// Assets are saved as text (Unity's Force Text): without it, scenes and prefabs cannot be read here.
    pub text_assets: bool,
    /// The look stopped at its limits before the end.
    pub cut: bool,
}

/// The most files and bytes the look reads, and of one file.
const MAX_FILES: usize = 40_000;
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_FILE: u64 = 4 * 1024 * 1024;
const AT_MOST: usize = 3;

/// Looks through `project`'s Assets (and its local packages) for what the checklist asks, within the limits above.
pub fn checklist(project: &Path, content: Content, cancel: &AtomicBool) -> Checklist {
    let text_assets = std::fs::read_to_string(project.join("ProjectSettings/EditorSettings.asset")).is_ok_and(|s| s.lines().any(|l| l.trim() == "m_SerializationMode: 2"));
    let upm = std::fs::read_to_string(project.join("Packages/manifest.json")).unwrap_or_default();
    let vpm = std::fs::read_to_string(project.join("Packages/vpm-manifest.json")).unwrap_or_default();
    let mut found: BTreeMap<u8, Vec<String>> = BTreeMap::new();
    let mut note = |item: Item, rel: &str| {
        let list = found.entry(item as u8).or_default();
        if list.len() < AT_MOST && !list.iter().any(|r| r == rel) {
            list.push(rel.to_string());
        }
    };
    let (mut files, mut bytes, mut scenes, mut cut) = (0usize, 0u64, 0usize, false);
    let mut tmp_in_assets = false;
    let walker = walkdir::WalkDir::new(project.join("Assets")).follow_links(false).into_iter().filter_entry(|e| {
        let n = e.file_name().to_string_lossy();
        !(e.file_type().is_dir() && (n.starts_with('.') || n.ends_with('~')))
    });
    for e in walker.flatten() {
        if cancel.load(Ordering::Relaxed) {
            cut = true;
            break;
        }
        if !e.file_type().is_file() {
            if e.file_type().is_dir() && e.file_name() == "TextMesh Pro" {
                tmp_in_assets = true;
            }
            continue;
        }
        let name = e.file_name().to_string_lossy().to_ascii_lowercase();
        let ext = name.rsplit_once('.').map(|(_, x)| x.to_string()).unwrap_or_default();
        if ext == "unity" {
            scenes += 1;
        }
        let wanted = matches!(ext.as_str(), "cs" | "uasm" | "unity" | "prefab" | "asset");
        if !wanted {
            continue;
        }
        let len = e.metadata().map(|m| m.len()).unwrap_or(0);
        if len > MAX_FILE {
            continue;
        }
        files += 1;
        bytes += len;
        if files > MAX_FILES || bytes > MAX_BYTES {
            cut = true;
            break;
        }
        let Ok(body) = std::fs::read(e.path()) else { continue };
        let body = String::from_utf8_lossy(&body);
        let rel = e.path().strip_prefix(project).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        match ext.as_str() {
            "cs" => {
                if body.contains("UdonSharpBehaviour") {
                    note(Item::UdonSharp, &rel);
                }
                if body.contains(".velocity") {
                    note(Item::Velocity, &rel);
                }
                if body.contains("ForceMode.Acceleration") || body.contains("ForceMode.VelocityChange") {
                    note(Item::ForceModes, &rel);
                }
                if body.contains("__get_velocity") || body.contains("__set_velocity") {
                    note(Item::CustomUasm, &rel);
                }
            }
            "uasm" => {
                if body.contains("__get_velocity") || body.contains("__set_velocity") {
                    note(Item::CustomUasm, &rel);
                }
            }
            _ => {
                if body.contains("graphDataJson") || body.contains("UdonGraphProgramAsset") {
                    note(Item::UdonGraph, &rel);
                }
                if body.contains("__get_velocity") || body.contains("__set_velocity") {
                    note(Item::CustomUasm, &rel);
                }
                if body.lines().any(|l| l.trim() == "ringBufferMode: 2") {
                    note(Item::RingBuffer, &rel);
                }
                // NavMeshAgent is class 195, NavMeshObstacle 208.
                if body.contains("--- !u!195 ") && body.contains("--- !u!208 ") {
                    note(Item::NavMesh, &rel);
                }
                if body.contains("m_fontAsset:") && body.contains("m_fontColor") {
                    note(Item::TextMeshPro, &rel);
                }
            }
        }
    }
    let has = |item: Item| found.contains_key(&(item as u8));
    let tmp = upm.contains("\"com.unity.textmeshpro\"") || tmp_in_assets || has(Item::TextMeshPro);
    let shaders = ["com.poiyomi.toon", "jp.lilxyzw.liltoon", "com.poiyomi.pro"].iter().any(|p| vpm.contains(&format!("\"{p}\"")) || project.join("Packages").join(p).is_dir())
        || project.join("Assets/_PoiyomiShaders").is_dir();
    let generators = ["com.vrcfury.vrcfury", "nadena.dev.modular-avatar"].iter().any(|p| vpm.contains(&format!("\"{p}\"")));
    let resolver = vpm.contains("\"com.vrchat.core.vpm-resolver\"");
    let mut checks = vec![];
    let items = [
        Item::UdonSharp,
        Item::UdonGraph,
        Item::CustomUasm,
        Item::Velocity,
        Item::ForceModes,
        Item::RingBuffer,
        Item::TextMeshPro,
        Item::NavMesh,
        Item::ParameterNames,
        Item::PackageMaker,
        Item::Shaders,
    ];
    for item in items.into_iter().filter(|i| i.applies(content)) {
        let needs_text = matches!(item, Item::UdonGraph | Item::RingBuffer | Item::NavMesh);
        let seen = match item {
            Item::TextMeshPro if tmp => Seen::Found,
            Item::Shaders if shaders => Seen::Found,
            Item::PackageMaker if resolver => Seen::Found,
            // Names generated at build time (VRCFury, Modular Avatar) are only known once built.
            Item::ParameterNames => if generators { Seen::Found } else { Seen::CannotTell },
            _ if has(item) => Seen::Found,
            _ if (needs_text && !text_assets) || cut => Seen::CannotTell,
            _ => Seen::NotFound,
        };
        checks.push(Check { item, seen, at: found.get(&(item as u8)).cloned().unwrap_or_default() });
    }
    Checklist { content: Some(content), checks, scenes, text_assets, cut }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unity6::standing::tests::{project, U2022};

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn each_package_shows_its_facts_apart_and_no_verdict() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        project(root, Some(U2022), &[("com.vrchat.base", "3.10.6"), ("com.vrchat.avatars", "3.10.6"), ("com.poiyomi.toon", "9.3.64"), ("vrchat.blackstartx.gesture-manager", "3.9.8")], &[("com.vrchat.base", "3.10.6", "2022.3")]);
        write(root, "Packages/com.poiyomi.toon/package.json", r#"{"name":"com.poiyomi.toon","displayName":"Poiyomi Toon Shader","version":"9.3.64","unity":"2019.4"}"#);
        write(
            root,
            "Packages/vrchat.blackstartx.gesture-manager/package.json",
            r#"{"name":"vrchat.blackstartx.gesture-manager","version":"3.9.8","unity":"2019.4","vpmDependencies":{"com.vrchat.avatars":">=3.5.0 <3.11.0"}}"#,
        );
        write(root, "Packages/my.embedded/package.json", r#"{"name":"my.embedded","version":"1.0.0","unity":"2022.3","unityRelease":"0f1"}"#);
        write(root, "Packages/manifest.json", r#"{"dependencies":{"com.unity.timeline":"1.7.5","com.unity.modules.ai":"1.0.0","com.example.git":"https://example.com/repo.git","com.example.reg":"2.0.0"}}"#);
        let rows = packages(root, &Matrix::built_in(), "3.11.0-beta.1");
        let by = |n: &str| rows.iter().find(|r| r.name == n).unwrap_or_else(|| panic!("{n} missing: {rows:?}"));
        assert!(rows.iter().all(|r| !r.name.starts_with("com.vrchat.")), "the SDK is the standing's");
        assert!(rows.iter().all(|r| r.name != "com.unity.timeline" && r.name != "com.unity.modules.ai"), "Unity's own come with the editor");
        let poi = by("com.poiyomi.toon");
        assert_eq!((poi.source, poi.unity_min.as_deref(), &poi.sdk_range), (Source::Vpm, Some("2019.4"), &None));
        assert!(poi.issues.is_empty(), "9.3.64 is not what the publisher's statement is about");
        let gm = by("vrchat.blackstartx.gesture-manager");
        // `<3.11.0` leaves out 3.11.0, and takes 3.11.0-beta.1 only when pre-releases count: said as such.
        assert_eq!(gm.sdk_range, Some(SdkRange { range: ">=3.5.0 <3.11.0".into(), admits_target: Some(Admits::DependsOnPrerelease) }));
        let beta: Version = "3.11.0-beta.1".parse().unwrap();
        assert_eq!(admits(">=3.2.0 <3.10.0", &beta), Some(Admits::No), "AV3 Manager 2.1.2's kind of range");
        // A beta target is inside an open range only when pre-releases count (vrc-get's default leaves them out).
        assert_eq!(admits(">=3.5.0", &beta), Some(Admits::DependsOnPrerelease));
        assert_eq!(admits(">=3.11.0-beta.1", &beta), Some(Admits::Yes));
        assert_eq!(admits(">=3.5.0", &"3.11.0".parse().unwrap()), Some(Admits::Yes));
        assert_eq!(admits("not a range", &beta), None);
        let embedded = by("my.embedded");
        assert_eq!((embedded.source, embedded.unity_min.as_deref()), (Source::Embedded, Some("2022.3.0f1")));
        assert_eq!(by("com.example.git").source, Source::UpmGit);
        assert_eq!((by("com.example.reg").source, by("com.example.reg").version.as_deref()), (Source::UpmRegistry, Some("2.0.0")));
    }

    #[test]
    fn the_checklist_says_found_not_found_or_cannot_tell() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        project(root, Some(U2022), &[("com.vrchat.base", "3.10.6"), ("com.vrchat.worlds", "3.10.6")], &[]);
        write(root, "Assets/Scripts/Door.cs", "class Door : UdonSharpBehaviour { void F(Rigidbody r) { r.velocity = v; r.AddForce(f, ForceMode.VelocityChange); } }");
        write(root, "Assets/Main.unity", "--- !u!195 &1\nNavMeshAgent:\n--- !u!208 &2\nNavMeshObstacle:\n  ringBufferMode: 2\n");
        let cancel = AtomicBool::new(false);
        // Assets not saved as text: what only scenes and prefabs show cannot be told, unless it was seen anyway.
        let c = checklist(root, Content::Worlds, &cancel);
        let seen = |c: &Checklist, i: Item| c.checks.iter().find(|x| x.item == i).map(|x| x.seen);
        assert_eq!(seen(&c, Item::UdonSharp), Some(Seen::Found));
        assert_eq!(seen(&c, Item::Velocity), Some(Seen::Found));
        assert_eq!(seen(&c, Item::ForceModes), Some(Seen::Found));
        assert_eq!(seen(&c, Item::NavMesh), Some(Seen::Found));
        assert_eq!(seen(&c, Item::UdonGraph), Some(Seen::CannotTell));
        assert_eq!(seen(&c, Item::CustomUasm), Some(Seen::NotFound));
        assert_eq!(seen(&c, Item::ParameterNames), None, "an avatar matter, not a world's");
        assert_eq!((c.scenes, c.text_assets, c.cut), (1, false, false));
        assert_eq!(c.checks.iter().find(|x| x.item == Item::ForceModes).unwrap().at, ["Assets/Scripts/Door.cs"]);
        // Saved as text: not found is said.
        write(root, "ProjectSettings/EditorSettings.asset", "EditorSettings:\n  m_SerializationMode: 2\n");
        let c = checklist(root, Content::Worlds, &cancel);
        assert_eq!(seen(&c, Item::UdonGraph), Some(Seen::NotFound));
        assert_eq!(seen(&c, Item::RingBuffer), Some(Seen::Found));
        // An avatar project: the world items go, the parameter names come.
        let c = checklist(root, Content::Avatars, &cancel);
        assert_eq!(seen(&c, Item::UdonSharp), None);
        assert_eq!(seen(&c, Item::ParameterNames), Some(Seen::CannotTell));
        // Stopped: what was not seen cannot be told.
        cancel.store(true, Ordering::Relaxed);
        let c = checklist(root, Content::Worlds, &cancel);
        assert!(c.cut);
        assert_eq!(seen(&c, Item::CustomUasm), Some(Seen::CannotTell));
    }
}
