//! Unity text serialization reader: the uploaded avatars and worlds of a project's scenes (their PipelineManager
//! blueprint IDs), and the world thumbnail My World keeps. Supports direct components and prefab-instance overrides
//! (the Canine scene stores its uploaded blueprint in an override). Never writes scenes or opens them in Unity.
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Avatar {
    pub id: String,
    pub name: String,
    pub scene: String,
    pub path: String,
    /// A world's own picture on disk: the thumbnail My World keeps for the scene (`MyWorld.thumbnail`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<PathBuf>,
}
impl Avatar {
    /// An uploaded world (`wrld_…`) rather than an avatar.
    pub fn is_world(&self) -> bool {
        self.id.starts_with("wrld_")
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CachedScene {
    pub modified: u64,
    pub len: u64,
    pub avatars: Vec<Avatar>,
    pub dependencies: Vec<(PathBuf, u64)>,
}
pub type SceneCache = HashMap<PathBuf, CachedScene>;

#[derive(Default, Clone)]
struct Doc {
    id: String,
    kind: u32,
    lines: Vec<String>,
}
fn docs(text: &str) -> Vec<Doc> {
    let mut out = vec![];
    let mut current = None;
    for l in text.lines() {
        if let Some(h) = l.strip_prefix("--- !u!") {
            if let Some(d) = current.take() {
                out.push(d);
            }
            if let Some((kind, id)) = h.split_once(" &") {
                current = Some(Doc { id: id.split_whitespace().next().unwrap_or_default().into(), kind: kind.parse().unwrap_or(0), lines: vec![] });
            }
        } else if let Some(d) = current.as_mut() {
            d.lines.push(l.into());
        }
    }
    if let Some(d) = current {
        out.push(d);
    }
    out
}
fn value(d: &Doc, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    d.lines.iter().find_map(|l| l.trim().strip_prefix(&prefix).map(|s| unquote(s.trim())))
}
fn unquote(s: &str) -> String {
    if s.starts_with('"') {
        serde_json::from_str(s).unwrap_or_else(|_| s.trim_matches('"').into())
    } else {
        s.trim_matches('\'').replace("''", "'")
    }
}
fn fileid(s: &str) -> String {
    s.split("fileID:").nth(1).unwrap_or_default().trim().split([',', '}']).next().unwrap_or_default().trim().into()
}
fn guid(s: &str) -> Option<String> {
    Some(s.split("guid:").nth(1)?.trim().split([',', '}']).next()?.trim().into())
}
/// An avatar's or a world's blueprint ID.
fn valid_id(s: &str) -> bool {
    s.strip_prefix("avtr_").or_else(|| s.strip_prefix("wrld_")).is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
}
fn overrides(d: &Doc, property_name: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut target = String::new();
    let mut property = String::new();
    for l in &d.lines {
        let l = l.trim();
        if let Some(v) = l.strip_prefix("- target:") {
            target = fileid(v);
        }
        if let Some(v) = l.strip_prefix("propertyPath:") {
            property = v.trim().into();
        }
        if let Some(v) = l.strip_prefix("value:") {
            if property == property_name {
                out.insert(target.clone(), unquote(v.trim()));
            }
        }
    }
    out
}

struct Model {
    docs: Vec<Doc>,
    names: HashMap<String, String>,
    transforms: HashMap<String, (String, String)>,
}
impl Model {
    fn new(text: &str) -> Self {
        let docs = docs(text);
        let mut names = HashMap::new();
        let mut transforms = HashMap::new();
        for d in &docs {
            if d.kind == 1 {
                names.insert(d.id.clone(), value(d, "m_Name").unwrap_or_default());
            }
            if matches!(d.kind, 4 | 224) {
                transforms.insert(d.id.clone(), (fileid(&value(d, "m_GameObject").unwrap_or_default()), fileid(&value(d, "m_Father").unwrap_or_default())));
            }
        }
        // A prefab variant's root GameObject is stripped; its name is an override on the source GameObject.
        for d in docs.iter().filter(|d| d.kind == 1 && value(d, "m_Name").is_none()) {
            let source = fileid(&value(d, "m_CorrespondingSourceObject").unwrap_or_default());
            let instance = fileid(&value(d, "m_PrefabInstance").unwrap_or_default());
            if let Some(name) = docs.iter().find(|d| d.id == instance && d.kind == 1001).and_then(|d| overrides(d, "m_Name").remove(&source)) {
                names.insert(d.id.clone(), name);
            }
        }
        Self { docs, names, transforms }
    }
    fn path(&self, go: &str) -> String {
        let mut parts = vec![self.names.get(go).cloned().unwrap_or_else(|| tr("Avatar").into())];
        let mut parent = self.transforms.values().find(|(g, _)| g == go).map(|(_, p)| p.as_str()).unwrap_or("0");
        let mut seen = HashSet::new();
        while parent != "0" && seen.insert(parent.to_string()) {
            let Some((g, p)) = self.transforms.get(parent) else { break };
            if let Some(n) = self.names.get(g) {
                parts.push(n.clone());
            }
            parent = p;
        }
        parts.reverse();
        parts.join("/")
    }
    fn direct(&self, scene: &str) -> Vec<Avatar> {
        self.docs
            .iter()
            .filter(|d| d.kind == 114)
            .filter_map(|d| {
                let id = value(d, "blueprintId")?;
                if !valid_id(&id) {
                    return None;
                }
                let go = fileid(&value(d, "m_GameObject")?);
                Some(Avatar {
                    id,
                    name: self.names.get(&go).cloned().unwrap_or_else(|| tr("Avatar").into()),
                    scene: scene.into(),
                    path: self.path(&go),
                    ..Default::default()
                })
            })
            .collect()
    }
    /// The GUID of the picture My World's component keeps as the world's thumbnail (its `thumbnail` and
    /// `thumbnailCamera` fields), if the scene has one.
    fn world_thumbnail(&self) -> Option<String> {
        self.docs
            .iter()
            .filter(|d| d.kind == 114 && d.lines.iter().any(|l| l.trim_start().starts_with("thumbnailCamera:")))
            .find_map(|d| value(d, "thumbnail").and_then(|t| guid(&t)).filter(|g| g.len() == 32 && g.bytes().any(|b| b != b'0')))
    }
}
pub fn extract(text: &str, scene: &str) -> Vec<Avatar> {
    if !text.starts_with("%YAML") {
        return vec![];
    }
    Model::new(text).direct(scene)
}

pub fn in_project(project: &Path, cache: &mut SceneCache) -> Vec<Avatar> {
    in_project_checked(project, cache).0
}

/// Also reports incomplete reads, so discovery can retain the previous list during transient I/O failures.
pub(crate) fn in_project_checked(project: &Path, cache: &mut SceneCache) -> (Vec<Avatar>, bool) {
    let mut complete = true;
    let assets = project.join("Assets");
    let mut scenes = vec![];
    let mut prefabs = HashMap::new();
    // Pictures' .meta files, read only when a scene names a My World thumbnail.
    let mut pictures = vec![];
    for entry in walkdir::WalkDir::new(&assets).follow_links(false).into_iter().filter_entry(|e| {
        !crate::discovery::skip_dir(&e.file_name().to_string_lossy())
            && (!e.file_type().is_dir() || !e.metadata().ok().is_some_and(|m| crate::discovery::is_link(&m)))
    }) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        let p = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }
        if p.extension().is_some_and(|e| e == "unity") {
            scenes.push(p.to_path_buf());
        }
        let lower = p.to_string_lossy().to_ascii_lowercase();
        if [".png.meta", ".jpg.meta", ".jpeg.meta"].iter().any(|e| lower.ends_with(e)) {
            pictures.push(p.to_path_buf());
        }
        if p.to_string_lossy().ends_with(".prefab.meta") {
            if let Ok(s) = std::fs::read_to_string(p) {
                if let Some(g) = s.lines().find_map(|l| l.strip_prefix("guid: ")) {
                    prefabs.insert(g.to_string(), p.with_extension(""));
                }
            } else {
                complete = false;
            }
        }
    }
    let mut out = vec![];
    let mut prefab_models: HashMap<PathBuf, Option<Model>> = HashMap::new();
    let mut picture_guids: Option<HashMap<String, PathBuf>> = None;
    for p in scenes {
        let Ok(meta) = std::fs::metadata(&p) else {
            complete = false;
            continue;
        };
        let modified = crate::discovery::fingerprint(&p);
        if let Some(c) = cache
            .get(&p)
            .filter(|c| c.modified == modified && c.len == meta.len() && c.dependencies.iter().all(|(p, m)| crate::discovery::fingerprint(p) == *m))
        {
            out.extend(c.avatars.clone());
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            complete = false;
            continue;
        };
        if !text.starts_with("%YAML") {
            continue;
        }
        let scene = p.strip_prefix(project).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        let model = Model::new(&text);
        let mut avatars = model.direct(&scene);
        let mut dependencies = vec![];
        let thumbnail = model.world_thumbnail().and_then(|g| {
            let guids = picture_guids.get_or_insert_with(|| {
                pictures
                    .iter()
                    .filter_map(|m| {
                        let s = std::fs::read_to_string(m).ok()?;
                        Some((s.lines().find_map(|l| l.strip_prefix("guid: "))?.trim().to_string(), m.with_extension("")))
                    })
                    .collect()
            });
            guids.get(&g).cloned()
        });
        if let Some(t) = &thumbnail {
            dependencies.push((t.clone(), crate::discovery::fingerprint(t)));
        }
        for d in model.docs.iter().filter(|d| d.kind == 1001) {
            let Some(source) = value(d, "m_SourcePrefab").and_then(|s| guid(&s)).and_then(|g| prefabs.get(&g)) else { continue };
            dependencies.push((source.clone(), crate::discovery::fingerprint(source)));
            let prefab = prefab_models.entry(source.clone()).or_insert_with(|| {
                let s = match std::fs::read_to_string(source) {
                    Ok(s) => s,
                    Err(_) => {
                        complete = false;
                        return None;
                    }
                };
                (s.starts_with("%YAML") && s.contains("blueprintId")).then(|| Model::new(&s))
            });
            let Some(prefab) = prefab.as_ref() else { continue };
            let mut names = prefab.names.clone();
            let mut ids: HashMap<String, String> = HashMap::new();
            let mut target = String::new();
            let mut property = String::new();
            for l in &d.lines {
                let l = l.trim();
                if let Some(v) = l.strip_prefix("- target:") {
                    target = fileid(v);
                }
                if let Some(v) = l.strip_prefix("propertyPath:") {
                    property = v.trim().into();
                }
                if let Some(v) = l.strip_prefix("value:") {
                    let v = unquote(v.trim());
                    if property == "m_Name" {
                        names.insert(target.clone(), v.clone());
                    }
                    if property == "blueprintId" {
                        ids.insert(target.clone(), v);
                    }
                }
            }
            for component in prefab.docs.iter().filter(|d| d.kind == 114) {
                let id = ids.get(&component.id).cloned().or_else(|| value(component, "blueprintId")).unwrap_or_default();
                if !valid_id(&id) {
                    continue;
                }
                let go = fileid(&value(component, "m_GameObject").unwrap_or_default());
                let name =
                    names.get(&go).filter(|n| !n.is_empty()).cloned().unwrap_or_else(|| source.file_stem().unwrap_or_default().to_string_lossy().into_owned());
                let mut hierarchy = Model { docs: vec![], names: names.clone(), transforms: prefab.transforms.clone() }.path(&go);
                if let Some(parent) = value(d, "m_TransformParent").map(|s| fileid(&s)).and_then(|t| model.transforms.get(&t)) {
                    hierarchy = format!("{}/{}", model.path(&parent.0), hierarchy);
                }
                avatars.push(Avatar { id, name, scene: scene.clone(), path: hierarchy, ..Default::default() });
            }
        }
        for a in avatars.iter_mut().filter(|a| a.is_world()) {
            a.thumbnail = thumbnail.clone();
        }
        avatars.sort_by(|a, b| (&a.id, &a.path).cmp(&(&b.id, &b.path)));
        avatars.dedup();
        if complete {
            cache.insert(p, CachedScene { modified, len: meta.len(), avatars: avatars.clone(), dependencies });
        }
        out.extend(avatars);
    }
    (out, complete)
}

/// Shared manifest/thumbnail filename contract, including Build & Test names.
pub fn filename(id: &str) -> String {
    let mut changed = false;
    let stem: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                changed = true;
                '_'
            }
        })
        .collect();
    if changed {
        let hash = id.as_bytes().iter().fold(0x811c9dc5u32, |h, b| (h ^ *b as u32).wrapping_mul(0x01000193));
        format!("{stem}-{hash:08x}")
    } else {
        stem
    }
}
pub fn local_thumbnail(id: &str) -> Option<PathBuf> {
    let p = crate::data_dir().join("Avatars").join(format!("{}.png", filename(id)));
    p.is_file().then_some(p)
}

#[cfg(test)]
mod read_completeness_tests {
    use super::*;

    #[test]
    fn missing_assets_is_not_a_successful_empty_scan() {
        let dir = std::env::temp_dir().join(format!("orbiters-read-check-{}", std::process::id()));
        let mut cache = SceneCache::default();
        assert!(!in_project_checked(&dir, &mut cache).1);
        std::fs::create_dir_all(dir.join("Assets")).unwrap();
        assert!(in_project_checked(&dir, &mut cache).1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
