//! Unity text serialization reader. Supports direct components and prefab-instance overrides (the Canine scene
//! stores its uploaded blueprint in an override). Never writes scenes or opens them in Unity.
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Avatar {
    pub id: String,
    pub name: String,
    pub scene: String,
    pub path: String,
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
fn valid_id(s: &str) -> bool {
    s.strip_prefix("avtr_").is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
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
        let mut parts = vec![self.names.get(go).cloned().unwrap_or_else(|| "Avatar".into())];
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
                Some(Avatar { id, name: self.names.get(&go).cloned().unwrap_or_else(|| "Avatar".into()), scene: scene.into(), path: self.path(&go) })
            })
            .collect()
    }
}
pub fn extract(text: &str, scene: &str) -> Vec<Avatar> {
    if !text.starts_with("%YAML") {
        return vec![];
    }
    Model::new(text).direct(scene)
}

pub fn in_project(project: &Path, cache: &mut SceneCache) -> Vec<Avatar> {
    let assets = project.join("Assets");
    let mut scenes = vec![];
    let mut prefabs = HashMap::new();
    for entry in walkdir::WalkDir::new(&assets)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            !crate::discovery::skip_dir(&e.file_name().to_string_lossy())
                && (!e.file_type().is_dir() || !e.metadata().ok().is_some_and(|m| crate::discovery::is_link(&m)))
        })
        .flatten()
    {
        let p = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }
        if p.extension().is_some_and(|e| e == "unity") {
            scenes.push(p.to_path_buf());
        }
        if p.to_string_lossy().ends_with(".prefab.meta") {
            if let Ok(s) = std::fs::read_to_string(p) {
                if let Some(g) = s.lines().find_map(|l| l.strip_prefix("guid: ")) {
                    prefabs.insert(g.to_string(), p.with_extension(""));
                }
            }
        }
    }
    let mut out = vec![];
    let mut prefab_models: HashMap<PathBuf, Option<Model>> = HashMap::new();
    for p in scenes {
        let Ok(meta) = std::fs::metadata(&p) else { continue };
        let modified = crate::discovery::fingerprint(&p);
        if let Some(c) = cache
            .get(&p)
            .filter(|c| c.modified == modified && c.len == meta.len() && c.dependencies.iter().all(|(p, m)| crate::discovery::fingerprint(p) == *m))
        {
            out.extend(c.avatars.clone());
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        if !text.starts_with("%YAML") {
            continue;
        }
        let scene = p.strip_prefix(project).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        let model = Model::new(&text);
        let mut avatars = model.direct(&scene);
        let mut dependencies = vec![];
        for d in model.docs.iter().filter(|d| d.kind == 1001) {
            let Some(source) = value(d, "m_SourcePrefab").and_then(|s| guid(&s)).and_then(|g| prefabs.get(&g)) else { continue };
            dependencies.push((source.clone(), crate::discovery::fingerprint(source)));
            let prefab = prefab_models.entry(source.clone()).or_insert_with(|| {
                let s = std::fs::read_to_string(source).ok()?;
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
                avatars.push(Avatar { id, name, scene: scene.clone(), path: hierarchy });
            }
        }
        avatars.sort_by(|a, b| (&a.id, &a.path).cmp(&(&b.id, &b.path)));
        avatars.dedup();
        cache.insert(p, CachedScene { modified, len: meta.len(), avatars: avatars.clone(), dependencies });
        out.extend(avatars);
    }
    out
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
