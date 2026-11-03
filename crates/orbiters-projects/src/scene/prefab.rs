//! A scene (or prefab) with every prefab instance in it resolved, the way Unity builds it when it opens the file:
//! each `PrefabInstance` stands for a copy of its source (a prefab, a prefab variant, or a model, which Unity treats
//! as a prefab too), with the instance's overrides on top.
//!
//! Unity names the objects of an instance in the file that holds it by `(instance ID ^ source object's ID) &
//! 0x7fffffffffffffff`, which is how the file's own objects reach them: a child added under an instance's bone has
//! that bone's mixed ID as its parent, an override (`m_Modifications`) targets a source object by its ID in the source
//! (`{fileID, guid of the source}`), `m_RemovedComponents` and `m_RemovedGameObjects` take objects out, and
//! `m_TransformParent` hangs the copy's root under a transform of the file. A source that holds instances itself is
//! resolved first, so nesting and variants come out of the same rule.
//!
//! Only what a preview draws is kept: GameObjects (name, active, `EditorOnly`), transforms, mesh renderers, mesh
//! filters, skinned mesh renderers (mesh, materials, bones, blend shape weights), directional lights and which
//! behaviours are VRChat avatar or world descriptors. Particles, UI, scripts and the rest are skipped unread.

use super::assets::Assets;
use super::model::{self, ImportSettings, Model};
use super::yaml::{self, Ref, Value};
use super::Limits;
use anyhow::{bail, Context as _};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MASK: i64 = 0x7fff_ffff_ffff_ffff;

/// An object of a source as the file holding its instance `instance` names it.
pub fn mixed(instance: i64, id: i64) -> i64 {
    if id == 0 {
        0
    } else {
        (instance ^ id) & MASK
    }
}

/// VRChat's descriptors (SDK 3): an avatar's root, a world's scene.
pub const AVATAR_DESCRIPTOR: (i64, &str) = (542108242, "67cc4cb7839cd3741b63733d5adf0442");
pub const SCENE_DESCRIPTOR: (i64, &str) = (-17141911, "661092b4961be7145bfbe56e1e62337b");

#[derive(Clone, Debug, PartialEq)]
pub enum MeshSource {
    None,
    /// Mesh `mesh` of the model whose GUID is `guid`.
    Model {
        guid: String,
        mesh: usize,
    },
    /// A reference written in a file (a model's mesh by Unity's ID, a mesh asset, a built-in primitive).
    Asset(Ref),
}

#[derive(Clone, Debug, PartialEq)]
pub enum MatSource {
    None,
    /// Material slot `material` of the model `guid` (its import settings decide what Unity draws there).
    Model {
        guid: String,
        material: usize,
    },
    Asset(Ref),
}

#[derive(Clone, Debug)]
pub struct GameObject {
    pub name: String,
    pub active: bool,
    pub editor_only: bool,
}

#[derive(Clone, Debug)]
pub struct Transform {
    pub game_object: i64,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub father: i64,
}

#[derive(Clone, Debug)]
pub struct Renderer {
    pub game_object: i64,
    pub enabled: bool,
    pub skinned: bool,
    pub materials: Vec<MatSource>,
    /// A skinned renderer's own mesh (a mesh renderer's is its filter's).
    pub mesh: MeshSource,
    pub bones: Vec<i64>,
    pub blend_weights: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct Filter {
    pub game_object: i64,
    pub mesh: MeshSource,
}

#[derive(Clone, Debug)]
pub struct Light {
    pub game_object: i64,
    pub enabled: bool,
    /// Unity's `LightType`: 1 directional.
    pub kind: i64,
    pub color: [f32; 4],
    pub intensity: f32,
    pub shadows: bool,
    pub shadow_strength: f32,
}

#[derive(Clone, Debug)]
pub struct Behaviour {
    pub game_object: i64,
    pub enabled: bool,
    pub script: Option<Ref>,
    /// Holds VRChat avatar descriptor fields (an SDK whose script is not the one known).
    pub avatar_fields: bool,
}

#[derive(Clone, Debug)]
pub enum Obj {
    GameObject(GameObject),
    Transform(Transform),
    Renderer(Renderer),
    Filter(Filter),
    Light(Light),
    Behaviour(Behaviour),
}

impl Obj {
    pub fn game_object(&self) -> Option<i64> {
        match self {
            Obj::GameObject(_) => None,
            Obj::Transform(t) => Some(t.game_object),
            Obj::Renderer(r) => Some(r.game_object),
            Obj::Filter(f) => Some(f.game_object),
            Obj::Light(l) => Some(l.game_object),
            Obj::Behaviour(b) => Some(b.game_object),
        }
    }

    fn remap(&mut self, k: impl Fn(i64) -> i64) {
        match self {
            Obj::GameObject(_) => {}
            Obj::Transform(t) => {
                t.game_object = k(t.game_object);
                t.father = k(t.father);
            }
            Obj::Renderer(r) => {
                r.game_object = k(r.game_object);
                for b in &mut r.bones {
                    *b = k(*b);
                }
            }
            Obj::Filter(f) => f.game_object = k(f.game_object),
            Obj::Light(l) => l.game_object = k(l.game_object),
            Obj::Behaviour(b) => b.game_object = k(b.game_object),
        }
    }
}

/// The scene's ambient light (`RenderSettings`).
#[derive(Clone, Debug, Default)]
pub struct Ambient {
    /// 0 skybox, 1 trilight (gradient), 3 flat (colour).
    pub mode: i64,
    pub sky: [f32; 4],
    pub equator: [f32; 4],
    pub ground: [f32; 4],
    pub intensity: f32,
}

/// A file's objects with its instances resolved, by ID in the file.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub objects: HashMap<i64, Obj>,
    pub ambient: Option<Ambient>,
}

/// Resolves files and their sources, each source once (a prefab placed fifty times in a world is read once).
pub struct Loader<'a> {
    pub assets: &'a Assets,
    pub limits: &'a Limits,
    sources: HashMap<String, Arc<Graph>>,
    /// Sources that could not be resolved, and why: not read again for each of their instances.
    failed: HashMap<String, String>,
    pub models: HashMap<String, Arc<Model>>,
    pub model_settings: HashMap<String, Arc<ImportSettings>>,
    resolving: HashSet<String>,
    /// Every file read (the preview is built again when one changes).
    pub files: Vec<PathBuf>,
    /// What could not be resolved, once each (shown in the preview's stats).
    pub missing: Vec<String>,
    pub prefab_instances: usize,
    /// Objects copied into instances so far ([`Limits::max_objects`]).
    pub objects_made: usize,
    /// Triangles of the models loaded so far ([`Limits::max_loaded_triangles`]).
    pub model_triangles: usize,
}

const MAX_DEPTH: usize = 24;

impl<'a> Loader<'a> {
    pub fn new(assets: &'a Assets, limits: &'a Limits) -> Self {
        Self {
            assets,
            limits,
            sources: HashMap::new(),
            failed: HashMap::new(),
            models: HashMap::new(),
            model_settings: HashMap::new(),
            resolving: HashSet::new(),
            files: Vec::new(),
            missing: Vec::new(),
            prefab_instances: 0,
            objects_made: 0,
            model_triangles: 0,
        }
    }

    fn note_missing(&mut self, what: String) {
        if !self.missing.contains(&what) && self.missing.len() < 64 {
            self.missing.push(what);
        }
    }

    /// Reads a scene or prefab file and resolves its instances.
    pub fn file(&mut self, path: &Path) -> anyhow::Result<Graph> {
        self.file_at(path, 0)
    }

    fn read_text(&mut self, path: &Path) -> anyhow::Result<String> {
        let size = std::fs::metadata(path).with_context(|| format!("{} is missing", rel(self.assets, path)))?.len();
        anyhow::ensure!(size <= self.limits.max_yaml_bytes, "{} is {} MB, more than a preview reads", rel(self.assets, path), size >> 20);
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", rel(self.assets, path)))?;
        self.files.push(path.to_path_buf());
        let text = String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
        if !yaml::is_text(&text) {
            bail!("{} is saved in Unity's binary format (Asset Serialization is not Force Text)", rel(self.assets, path));
        }
        Ok(text)
    }

    fn file_at(&mut self, path: &Path, depth: usize) -> anyhow::Result<Graph> {
        self.limits.check("reading the scene")?;
        let text = self.read_text(path)?;
        let mut graph = Graph::default();
        let mut instances = Vec::new();
        let mut aliases: HashMap<i64, i64> = HashMap::new();
        for doc in yaml::documents(&text) {
            if doc.stripped {
                // A stand-in for an object of an instance: older files give it an ID of its own (the instance's plus
                // one, …), newer ones the mixed ID. The file's own objects refer to it by that ID.
                let v = doc.parse();
                let source = v.get("m_CorrespondingSourceObject").as_ref().map_or(0, |r| r.file_id);
                let instance = v.get("m_PrefabInstance").as_ref().map_or(0, |r| r.file_id);
                let real = mixed(instance, source);
                if source != 0 && instance != 0 && real != doc.id {
                    aliases.insert(doc.id, real);
                }
                continue;
            }
            match doc.class {
                1 => {
                    let v = doc.parse();
                    graph.objects.insert(
                        doc.id,
                        Obj::GameObject(GameObject {
                            name: v.get("m_Name").str_or("").to_string(),
                            active: v.get("m_IsActive").as_bool().unwrap_or(true),
                            editor_only: v.get("m_TagString").as_str() == Some("EditorOnly"),
                        }),
                    );
                }
                4 | 224 => {
                    let v = doc.parse();
                    graph.objects.insert(doc.id, Obj::Transform(transform_of(&v)));
                }
                23 | 137 => {
                    let v = doc.parse();
                    let skinned = doc.class == 137;
                    graph.objects.insert(
                        doc.id,
                        Obj::Renderer(Renderer {
                            game_object: go_of(&v),
                            enabled: v.get("m_Enabled").as_bool().unwrap_or(true),
                            skinned,
                            materials: v.get("m_Materials").items().iter().map(|m| m.as_ref().map_or(MatSource::None, mat_source)).collect(),
                            mesh: if skinned { mesh_source(v.get("m_Mesh").as_ref()) } else { MeshSource::None },
                            bones: v.get("m_Bones").items().iter().filter_map(|b| b.as_ref()).map(|r| if r.guid.is_none() { r.file_id } else { 0 }).collect(),
                            blend_weights: v.get("m_BlendShapeWeights").items().iter().map(|w| w.f32_or(0.0)).collect(),
                        }),
                    );
                }
                33 => {
                    let v = doc.parse();
                    graph.objects.insert(doc.id, Obj::Filter(Filter { game_object: go_of(&v), mesh: mesh_source(v.get("m_Mesh").as_ref()) }));
                }
                108 => {
                    let v = doc.parse();
                    graph.objects.insert(
                        doc.id,
                        Obj::Light(Light {
                            game_object: go_of(&v),
                            enabled: v.get("m_Enabled").as_bool().unwrap_or(true),
                            kind: v.get("m_Type").as_i64().unwrap_or(2),
                            color: v.get("m_Color").color().unwrap_or([1.0; 4]),
                            intensity: v.get("m_Intensity").f32_or(1.0),
                            shadows: v.at(&["m_Shadows", "m_Type"]).as_i64().unwrap_or(0) != 0,
                            shadow_strength: v.at(&["m_Shadows", "m_Strength"]).f32_or(1.0),
                        }),
                    );
                }
                114 => {
                    // Only its first lines: behaviours can be megabytes of a tool's data.
                    let head: String = doc.body.lines().take(16).collect::<Vec<_>>().join("\n");
                    let v = yaml::parse(&head);
                    let v = v.entries().first().map(|(_, v)| v.clone()).unwrap_or_default();
                    let avatar_fields = doc.mentions("\n  ViewPosition:") && doc.mentions("\n  Animations:");
                    graph.objects.insert(
                        doc.id,
                        Obj::Behaviour(Behaviour {
                            game_object: go_of(&v),
                            enabled: v.get("m_Enabled").as_bool().unwrap_or(true),
                            script: v.get("m_Script").as_ref(),
                            avatar_fields,
                        }),
                    );
                }
                104 => {
                    let v = doc.parse();
                    graph.ambient = Some(Ambient {
                        mode: v.get("m_AmbientMode").as_i64().unwrap_or(0),
                        sky: v.get("m_AmbientSkyColor").color().unwrap_or([0.212, 0.227, 0.259, 1.0]),
                        equator: v.get("m_AmbientEquatorColor").color().unwrap_or([0.114, 0.125, 0.133, 1.0]),
                        ground: v.get("m_AmbientGroundColor").color().unwrap_or([0.047, 0.043, 0.035, 1.0]),
                        intensity: v.get("m_AmbientIntensity").f32_or(1.0),
                    });
                }
                1001 => instances.push((doc.id, doc.parse())),
                _ => {}
            }
        }
        drop(text);
        for (id, pi) in instances {
            self.instantiate(id, &pi, &mut graph, depth)?;
        }
        if !aliases.is_empty() {
            // References to stand-ins (a parent, a GameObject, a bone, an instance's parent) lead to the real objects.
            for obj in graph.objects.values_mut() {
                obj.remap(|x| aliases.get(&x).copied().unwrap_or(x));
            }
        }
        Ok(graph)
    }

    /// The resolved objects of the asset `guid` in its own IDs: a prefab's, or a model's.
    fn source(&mut self, guid: &str, depth: usize) -> anyhow::Result<Arc<Graph>> {
        if let Some(g) = self.sources.get(guid) {
            return Ok(g.clone());
        }
        if let Some(why) = self.failed.get(guid) {
            bail!("{why}");
        }
        anyhow::ensure!(depth <= MAX_DEPTH, "prefabs nested more than {MAX_DEPTH} deep");
        anyhow::ensure!(self.resolving.insert(guid.to_string()), "a prefab that contains itself");
        let result = (|| {
            let path = self.assets.path(guid).with_context(|| format!("an asset that is not in the project ({guid})"))?;
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            match ext.as_str() {
                "prefab" | "unity" => self.file_at(&path, depth + 1),
                "fbx" | "obj" => {
                    let m = self.model(guid)?;
                    Ok(model_graph(guid, &m, &path))
                }
                other => bail!("{} is a .{other} model, which previews do not read", rel(self.assets, &path)),
            }
        })();
        self.resolving.remove(guid);
        if let Err(e) = &result {
            // Not a time limit (that stops the whole preview): the same reason for its other instances.
            if self.limits.check("").is_ok() {
                self.failed.insert(guid.to_string(), format!("{e:#}"));
            }
        }
        let g = Arc::new(result?);
        self.sources.insert(guid.to_string(), g.clone());
        Ok(g)
    }

    /// A model, loaded once.
    pub fn model(&mut self, guid: &str) -> anyhow::Result<Arc<Model>> {
        if let Some(m) = self.models.get(guid) {
            return Ok(m.clone());
        }
        let path = self.assets.path(guid).with_context(|| format!("a model that is not in the project ({guid})"))?;
        // Every model a scene's renderers use is loaded before the drawing budget picks among them: a world of hundreds
        // of models would hold them all (gigabytes) for a preview that draws 2 million triangles.
        anyhow::ensure!(
            self.model_triangles < self.limits.max_loaded_triangles,
            "{} left out: the scene's models have more than {} million triangles",
            rel(self.assets, &path),
            self.limits.max_loaded_triangles / 1_000_000
        );
        let settings = self.model_settings(guid);
        self.files.push(path.clone());
        let m = Arc::new(model::load(&path, &settings, self.limits)?);
        self.model_triangles += m.meshes.iter().map(|x| x.triangles()).sum::<usize>();
        self.models.insert(guid.to_string(), m.clone());
        Ok(m)
    }

    /// A model's import settings, from its `.meta`.
    pub fn model_settings(&mut self, guid: &str) -> Arc<ImportSettings> {
        if let Some(s) = self.model_settings.get(guid) {
            return s.clone();
        }
        let s = Arc::new(match self.assets.path(guid) {
            Some(p) => {
                let meta = meta_path(&p);
                self.files.push(meta.clone());
                std::fs::read_to_string(&meta).map(|t| ImportSettings::read(&t)).unwrap_or_default()
            }
            None => ImportSettings::default(),
        });
        self.model_settings.insert(guid.to_string(), s.clone());
        s
    }

    fn instantiate(&mut self, instance: i64, pi: &Value, into: &mut Graph, depth: usize) -> anyhow::Result<()> {
        self.limits.check("resolving prefabs")?;
        self.prefab_instances += 1;
        anyhow::ensure!(self.prefab_instances <= self.limits.max_prefab_instances, "more than {} prefab instances", self.limits.max_prefab_instances);
        let Some(source) = pi.get("m_SourcePrefab").as_ref().and_then(|r| r.guid) else { return Ok(()) };
        let src = match self.source(&source, depth) {
            Ok(g) => g,
            Err(e) => {
                // The time limit stops the whole preview; a missing or unreadable source only leaves its copy out.
                self.limits.check("resolving prefabs")?;
                let name = self.assets.path(&source).map(|p| rel(self.assets, &p)).unwrap_or_else(|| format!("a prefab ({source})"));
                self.note_missing(format!("{name}: {e:#}"));
                return Ok(());
            }
        };
        // Each instance copies its source's objects, and a source holding fifty instances of a prefab holding fifty
        // of another is 2,500 copies of that one: a few levels of that would fill the memory, which the count of
        // instances (each source is resolved once) never sees.
        anyhow::ensure!(self.objects_made + src.objects.len() <= self.limits.max_objects, "more than {} objects in its prefabs", self.limits.max_objects);
        self.objects_made += src.objects.len();
        let m = pi.get("m_Modification");
        let parent = m.get("m_TransformParent").as_ref().map_or(0, |r| r.file_id);
        let mut objects: HashMap<i64, Obj> = HashMap::with_capacity(src.objects.len());
        for (id, obj) in &src.objects {
            let mut o = obj.clone();
            o.remap(|x| mixed(instance, x));
            if let Obj::Transform(t) = &mut o {
                if t.father == 0 {
                    t.father = parent;
                }
            }
            objects.insert(mixed(instance, *id), o);
        }
        let in_source = |r: &Ref| r.guid.as_deref() == Some(source.as_str());
        for r in m.get("m_RemovedComponents").items().iter().filter_map(Value::as_ref).filter(in_source) {
            objects.remove(&mixed(instance, r.file_id));
        }
        let removed_gos: HashSet<i64> =
            m.get("m_RemovedGameObjects").items().iter().filter_map(Value::as_ref).filter(in_source).map(|r| mixed(instance, r.file_id)).collect();
        if !removed_gos.is_empty() {
            objects.retain(|id, o| !removed_gos.contains(id) && !o.game_object().is_some_and(|g| removed_gos.contains(&g)));
        }
        for modification in m.get("m_Modifications").items() {
            let Some(target) = modification.get("target").as_ref().filter(in_source) else { continue };
            let Some(obj) = objects.get_mut(&mixed(instance, target.file_id)) else { continue };
            let path = modification.get("propertyPath").str_or("");
            apply(obj, path, modification.get("value"), modification.get("objectReference").as_ref());
        }
        into.objects.extend(objects);
        if into.ambient.is_none() {
            into.ambient = src.ambient.clone();
        }
        Ok(())
    }
}

pub fn meta_path(asset: &Path) -> PathBuf {
    let mut s = asset.as_os_str().to_os_string();
    s.push(".meta");
    PathBuf::from(s)
}

/// A path as the project names it (`Assets/…`), for messages.
pub fn rel(assets: &Assets, path: &Path) -> String {
    path.strip_prefix(&assets.project).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

fn go_of(v: &Value) -> i64 {
    v.get("m_GameObject").as_ref().map_or(0, |r| r.file_id)
}

fn transform_of(v: &Value) -> Transform {
    Transform {
        game_object: go_of(v),
        position: v.get("m_LocalPosition").vec3().unwrap_or([0.0; 3]),
        rotation: v.get("m_LocalRotation").quat().unwrap_or([0.0, 0.0, 0.0, 1.0]),
        scale: v.get("m_LocalScale").vec3().unwrap_or([1.0; 3]),
        father: v.get("m_Father").as_ref().map_or(0, |r| r.file_id),
    }
}

fn mesh_source(r: Option<Ref>) -> MeshSource {
    match r {
        Some(r) if !r.is_null() => MeshSource::Asset(r),
        _ => MeshSource::None,
    }
}

fn mat_source(r: Ref) -> MatSource {
    if r.is_null() {
        MatSource::None
    } else {
        MatSource::Asset(r)
    }
}

/// `m_Materials.Array.data[3]` → 3.
fn array_index(path: &str, field: &str) -> Option<usize> {
    path.strip_prefix(field)?.strip_prefix(".Array.data[")?.strip_suffix(']')?.parse().ok()
}

/// One override on an object.
fn apply(obj: &mut Obj, path: &str, value: &Value, reference: Option<Ref>) {
    let f = || value.as_f32();
    let set = |slot: &mut f32| {
        if let Some(v) = f() {
            *slot = v;
        }
    };
    match obj {
        Obj::GameObject(g) => match path {
            "m_Name" => g.name = value.str_or("").to_string(),
            "m_IsActive" => g.active = value.as_bool().unwrap_or(g.active),
            "m_TagString" => g.editor_only = value.as_str() == Some("EditorOnly"),
            _ => {}
        },
        Obj::Transform(t) => {
            let axis = |s: &str| match s {
                "x" => Some(0),
                "y" => Some(1),
                "z" => Some(2),
                "w" => Some(3),
                _ => None,
            };
            if let Some((field, a)) = path.split_once('.') {
                match (field, axis(a)) {
                    ("m_LocalPosition", Some(k)) if k < 3 => set(&mut t.position[k]),
                    ("m_LocalRotation", Some(k)) => set(&mut t.rotation[k]),
                    ("m_LocalScale", Some(k)) if k < 3 => set(&mut t.scale[k]),
                    _ => {}
                }
            }
        }
        Obj::Renderer(r) => {
            if path == "m_Enabled" {
                r.enabled = value.as_bool().unwrap_or(r.enabled);
            } else if path == "m_Materials.Array.size" {
                if let Some(n) = value.as_i64().filter(|n| (0..=256).contains(n)) {
                    r.materials.resize(n as usize, MatSource::None);
                }
            } else if let Some(i) = array_index(path, "m_Materials").filter(|i| *i < 256) {
                if r.materials.len() <= i {
                    r.materials.resize(i + 1, MatSource::None);
                }
                r.materials[i] = reference.map_or(MatSource::None, mat_source);
            } else if path == "m_Mesh" {
                r.mesh = mesh_source(reference);
            } else if let Some(i) = array_index(path, "m_BlendShapeWeights").filter(|i| *i < 8192) {
                if r.blend_weights.len() <= i {
                    r.blend_weights.resize(i + 1, 0.0);
                }
                set(&mut r.blend_weights[i]);
            } else if let Some(i) = array_index(path, "m_Bones").filter(|i| *i < 4096) {
                if r.bones.len() <= i {
                    r.bones.resize(i + 1, 0);
                }
                r.bones[i] = reference.filter(|r| r.guid.is_none()).map_or(0, |r| r.file_id);
            } else if path == "m_Bones.Array.size" {
                if let Some(n) = value.as_i64().filter(|n| (0..=4096).contains(n)) {
                    r.bones.resize(n as usize, 0);
                }
            }
        }
        Obj::Filter(fl) => {
            if path == "m_Mesh" {
                fl.mesh = mesh_source(reference);
            }
        }
        Obj::Light(l) => match path {
            "m_Enabled" => l.enabled = value.as_bool().unwrap_or(l.enabled),
            "m_Type" => l.kind = value.as_i64().unwrap_or(l.kind),
            "m_Intensity" => set(&mut l.intensity),
            "m_Color.r" => set(&mut l.color[0]),
            "m_Color.g" => set(&mut l.color[1]),
            "m_Color.b" => set(&mut l.color[2]),
            "m_Shadows.m_Type" => l.shadows = value.as_i64().unwrap_or(0) != 0,
            "m_Shadows.m_Strength" => set(&mut l.shadow_strength),
            _ => {}
        },
        Obj::Behaviour(b) => {
            if path == "m_Enabled" {
                b.enabled = value.as_bool().unwrap_or(b.enabled);
            }
        }
    }
}

/// A model as the prefab Unity makes of it: a GameObject and Transform per node (the root named after the file), a
/// renderer (and a filter) per node with a mesh, with Unity's IDs.
fn model_graph(guid: &str, m: &Model, path: &Path) -> Graph {
    let mut g = Graph::default();
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    for (i, n) in m.nodes.iter().enumerate() {
        let name = if i == 0 { stem.clone() } else { n.name.clone() };
        g.objects.insert(n.ids.game_object, Obj::GameObject(GameObject { name, active: true, editor_only: false }));
        g.objects.insert(
            n.ids.transform,
            Obj::Transform(Transform {
                game_object: n.ids.game_object,
                position: n.position,
                rotation: n.rotation,
                scale: n.scale,
                father: n.parent.map_or(0, |p| m.nodes[p].ids.transform),
            }),
        );
        let Some(mesh) = n.mesh else { continue };
        let skinned = n.ids.filter == 0;
        let mm = &m.meshes[mesh];
        let source = MeshSource::Model { guid: guid.to_string(), mesh };
        g.objects.insert(
            n.ids.renderer,
            Obj::Renderer(Renderer {
                game_object: n.ids.game_object,
                enabled: n.visible,
                skinned,
                materials: n.materials.iter().map(|&k| MatSource::Model { guid: guid.to_string(), material: k }).collect(),
                mesh: if skinned { source.clone() } else { MeshSource::None },
                bones: mm.skin.as_ref().map(|s| s.bones.iter().map(|&b| m.nodes[b].ids.transform).collect()).unwrap_or_default(),
                blend_weights: mm.blend_shapes.iter().map(|b| b.default_weight).collect(),
            }),
        );
        if !skinned {
            g.objects.insert(n.ids.filter, Obj::Filter(Filter { game_object: n.ids.game_object, mesh: source }));
        }
    }
    g
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const MODEL_GUID: &str = "aaaa0000000000000000000000000001";
    pub(crate) const PREFAB_GUID: &str = "bbbb0000000000000000000000000002";
    pub(crate) const VARIANT_GUID: &str = "cccc0000000000000000000000000003";
    pub(crate) const RED_GUID: &str = "dddd0000000000000000000000000004";

    /// A project with the rig model, a prefab of it with an avatar descriptor and a material override, a variant of
    /// that prefab moving the bone and hiding the prop, and a scene placing the variant and a cube.
    pub(crate) fn project(dir: &Path) -> Assets {
        let a = dir.join("Assets");
        std::fs::create_dir_all(&a).unwrap();
        let model = super::model::tests::rig_file(&a);
        let root_go = model::file_id("GameObject", "//RootNode/root", 0);
        let root_tr = model::file_id("Transform", "//RootNode/root/Transform", 0);
        let body_smr = model::file_id("SkinnedMeshRenderer", "//RootNode/root/Body/SkinnedMeshRenderer", 0);
        let bone_tr = model::file_id("Transform", "//RootNode/root/Armature/Bone/Transform", 0);
        let prop_go = model::file_id("GameObject", "//RootNode/root/Prop", 0);
        let inst = 5000i64;
        let prefab = format!(
            "%YAML 1.1
--- !u!1 &{desc_go} stripped
GameObject:
  m_CorrespondingSourceObject: {{fileID: {root_go}, guid: {MODEL_GUID}, type: 3}}
  m_PrefabInstance: {{fileID: {inst}}}
--- !u!114 &77
MonoBehaviour:
  m_GameObject: {{fileID: {desc_go}}}
  m_Enabled: 1
  m_Script: {{fileID: 542108242, guid: 67cc4cb7839cd3741b63733d5adf0442, type: 3}}
  ViewPosition: {{x: 0, y: 1.6, z: 0.1}}
--- !u!1001 &{inst}
PrefabInstance:
  m_Modification:
    m_TransformParent: {{fileID: 0}}
    m_Modifications:
    - target: {{fileID: {root_go}, guid: {MODEL_GUID}, type: 3}}
      propertyPath: m_Name
      value: Avatar
      objectReference: {{fileID: 0}}
    - target: {{fileID: {body_smr}, guid: {MODEL_GUID}, type: 3}}
      propertyPath: m_Materials.Array.data[0]
      value:
      objectReference: {{fileID: 2100000, guid: {RED_GUID}, type: 2}}
    - target: {{fileID: {root_tr}, guid: {MODEL_GUID}, type: 3}}
      propertyPath: m_LocalPosition.x
      value: 0
      objectReference: {{fileID: 0}}
    m_RemovedComponents: []
  m_SourcePrefab: {{fileID: 100100000, guid: {MODEL_GUID}, type: 3}}
",
            desc_go = mixed(inst, root_go)
        );
        std::fs::write(a.join("Avatar.prefab"), prefab).unwrap();
        let vinst = 6000i64;
        // In the prefab, the model's objects are named by the prefab's instance.
        let p_bone = mixed(inst, bone_tr);
        let p_prop = mixed(inst, prop_go);
        let p_root_tr = mixed(inst, root_tr);
        let variant = format!(
            "%YAML 1.1
--- !u!1001 &{vinst}
PrefabInstance:
  m_Modification:
    m_TransformParent: {{fileID: 0}}
    m_Modifications:
    - target: {{fileID: {p_bone}, guid: {PREFAB_GUID}, type: 3}}
      propertyPath: m_LocalPosition.y
      value: 2
      objectReference: {{fileID: 0}}
    - target: {{fileID: {p_prop}, guid: {PREFAB_GUID}, type: 3}}
      propertyPath: m_IsActive
      value: 0
      objectReference: {{fileID: 0}}
    - target: {{fileID: {p_root_tr}, guid: {PREFAB_GUID}, type: 3}}
      propertyPath: m_LocalPosition.z
      value: 3
      objectReference: {{fileID: 0}}
  m_SourcePrefab: {{fileID: 100100000, guid: {PREFAB_GUID}, type: 3}}
"
        );
        std::fs::write(a.join("Avatar Variant.prefab"), variant).unwrap();
        let scene = format!(
            "%YAML 1.1
--- !u!104 &2
RenderSettings:
  m_AmbientMode: 3
  m_AmbientSkyColor: {{r: 0.5, g: 0.5, b: 0.5, a: 1}}
--- !u!1 &10
GameObject:
  m_Name: Cube
  m_IsActive: 1
  m_TagString: Untagged
--- !u!4 &11
Transform:
  m_GameObject: {{fileID: 10}}
  m_LocalRotation: {{x: 0, y: 0, z: 0, w: 1}}
  m_LocalPosition: {{x: 4, y: 0.5, z: 0}}
  m_LocalScale: {{x: 1, y: 1, z: 1}}
  m_Father: {{fileID: 0}}
--- !u!33 &12
MeshFilter:
  m_GameObject: {{fileID: 10}}
  m_Mesh: {{fileID: 10202, guid: 0000000000000000e000000000000000, type: 0}}
--- !u!23 &13
MeshRenderer:
  m_GameObject: {{fileID: 10}}
  m_Enabled: 1
  m_Materials:
  - {{fileID: 2100000, guid: {RED_GUID}, type: 2}}
--- !u!1 &20
GameObject:
  m_Name: Notes
  m_IsActive: 1
  m_TagString: EditorOnly
--- !u!4 &21
Transform:
  m_GameObject: {{fileID: 20}}
  m_LocalRotation: {{x: 0, y: 0, z: 0, w: 1}}
  m_LocalPosition: {{x: 0, y: 0, z: 0}}
  m_LocalScale: {{x: 1, y: 1, z: 1}}
  m_Father: {{fileID: 0}}
--- !u!1001 &7000
PrefabInstance:
  m_Modification:
    m_TransformParent: {{fileID: 0}}
    m_Modifications: []
  m_SourcePrefab: {{fileID: 100100000, guid: {VARIANT_GUID}, type: 3}}
"
        );
        std::fs::write(a.join("Scene.unity"), scene).unwrap();
        std::fs::write(
            a.join("Red.mat"),
            "%YAML 1.1
--- !u!21 &2100000
Material:
  m_Name: Red
  m_Shader: {fileID: 46, guid: 0000000000000000f000000000000000, type: 0}
  m_SavedProperties:
    m_TexEnvs: []
    m_Floats: []
    m_Colors:
    - _Color: {r: 1, g: 0, b: 0, a: 1}
",
        )
        .unwrap();
        let mut assets = Assets::empty(dir);
        assets.insert(MODEL_GUID, model);
        assets.insert(PREFAB_GUID, a.join("Avatar.prefab"));
        assets.insert(VARIANT_GUID, a.join("Avatar Variant.prefab"));
        assets.insert(RED_GUID, a.join("Red.mat"));
        assets
    }

    #[test]
    fn nested_prefabs_variants_and_overrides_resolve_as_unity_does() {
        let dir = tempfile::tempdir().unwrap();
        let assets = project(dir.path());
        let limits = Limits::default();
        let mut loader = Loader::new(&assets, &limits);
        let g = loader.file(&dir.path().join("Assets/Scene.unity")).unwrap();
        assert!(loader.missing.is_empty(), "{:?}", loader.missing);
        // The variant's instance of the prefab's instance of the model: two levels of mixing.
        let in_scene = |model_id: i64| mixed(7000, mixed(6000, mixed(5000, model_id)));
        let Obj::GameObject(root) = &g.objects[&in_scene(model::file_id("GameObject", "//RootNode/root", 0))] else { panic!() };
        assert_eq!(root.name, "Avatar", "the prefab's rename");
        let Obj::Transform(bone) = &g.objects[&in_scene(model::file_id("Transform", "//RootNode/root/Armature/Bone/Transform", 0))] else { panic!() };
        assert_eq!(bone.position[1], 2.0, "the variant's override");
        let Obj::GameObject(prop) = &g.objects[&in_scene(model::file_id("GameObject", "//RootNode/root/Prop", 0))] else { panic!() };
        assert!(!prop.active);
        let Obj::Renderer(body) = &g.objects[&in_scene(model::file_id("SkinnedMeshRenderer", "//RootNode/root/Body/SkinnedMeshRenderer", 0))] else { panic!() };
        assert_eq!(body.materials, [MatSource::Asset(Ref { file_id: 2100000, guid: Some(RED_GUID.into()) })]);
        assert_eq!(body.bones, [in_scene(model::file_id("Transform", "//RootNode/root/Armature/Bone/Transform", 0))], "bones follow the mixing");
        let Obj::Transform(root_tr) = &g.objects[&in_scene(model::file_id("Transform", "//RootNode/root/Transform", 0))] else { panic!() };
        assert_eq!((root_tr.position[2], root_tr.father), (3.0, 0));
        // The prefab's own descriptor, through the variant.
        assert!(g.objects.values().any(|o| matches!(o, Obj::Behaviour(b) if b.script.as_ref().is_some_and(|s| s.file_id == AVATAR_DESCRIPTOR.0))));
        assert_eq!(g.ambient.as_ref().map(|a| a.mode), Some(3));
        // The model was read once although two prefabs lead to it.
        assert_eq!(loader.models.len(), 1);
    }

    #[test]
    fn models_past_the_loaded_triangle_limit_are_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let mut assets = project(dir.path());
        let other = "eeee0000000000000000000000000005";
        assets.insert(other, dir.path().join("Assets/Rig.fbx"));
        let limits = Limits { max_loaded_triangles: 1, ..Limits::default() };
        let mut loader = Loader::new(&assets, &limits);
        assert!(loader.model(MODEL_GUID).is_ok(), "the first is always read");
        let why = format!("{:#}", loader.model(other).err().expect("past the limit"));
        assert!(why.contains("more than 0 million triangles"), "{why}");
    }

    #[test]
    fn prefabs_nested_fifty_by_fifty_stop_at_the_object_limit() {
        // Six levels of 50 instances of the level below: 50⁵ × 2 objects at the top (600 million) without the limit.
        let dir = tempfile::tempdir().unwrap();
        let mut assets = Assets::empty(dir.path());
        let guid = |k: usize| format!("{:032x}", 0xfeed_0000usize + k);
        let leaf = "%YAML 1.1\n--- !u!1 &1\nGameObject:\n  m_Name: Leaf\n--- !u!4 &2\nTransform:\n  m_GameObject: {fileID: 1}\n";
        std::fs::write(dir.path().join("P0.prefab"), leaf).unwrap();
        assets.insert(&guid(0), dir.path().join("P0.prefab"));
        for level in 1..=5 {
            let mut text = String::from("%YAML 1.1\n");
            for i in 0..50 {
                // Instance IDs as random as Unity's (small ones mix into the same IDs, which hides the growth).
                let id = (model::xxh64(format!("{level}-{i}").as_bytes(), 0) as i64) & MASK;
                text += &format!("--- !u!1001 &{id}\nPrefabInstance:\n  m_SourcePrefab: {{fileID: 100100000, guid: {}, type: 3}}\n", guid(level - 1));
            }
            let p = dir.path().join(format!("P{level}.prefab"));
            std::fs::write(&p, text).unwrap();
            assets.insert(&guid(level), p);
        }
        let limits = Limits { max_objects: 20_000, ..Limits::default() };
        let mut loader = Loader::new(&assets, &limits);
        let start = std::time::Instant::now();
        let g = loader.file(&dir.path().join("P5.prefab")).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "{:?}", start.elapsed());
        assert!(g.objects.len() <= 20_000);
        assert!(loader.missing.iter().any(|m| m.contains("more than 20000 objects")), "{:?}", loader.missing);
        assert!(loader.objects_made <= 20_000, "{}", loader.objects_made);
        assert_eq!(loader.files.iter().filter(|f| f.ends_with("P3.prefab")).count(), 1, "a source that failed is not read again for each instance");
    }

    #[test]
    fn an_older_scenes_stand_ins_with_ids_of_their_own_lead_to_the_real_objects() {
        let dir = tempfile::tempdir().unwrap();
        let mut assets = project(dir.path());
        let root_go = model::file_id("GameObject", "//RootNode/root", 0);
        let bone_tr = model::file_id("Transform", "//RootNode/root/Armature/Bone/Transform", 0);
        // As Unity 2019 saved scenes: the stand-ins numbered after their instance, a descriptor and a child under them.
        let scene = format!(
            "%YAML 1.1
--- !u!1001 &100
PrefabInstance:
  m_Modification:
    m_TransformParent: {{fileID: 0}}
    m_Modifications: []
  m_SourcePrefab: {{fileID: 100100000, guid: {MODEL_GUID}, type: 3}}
--- !u!1 &101 stripped
GameObject:
  m_CorrespondingSourceObject: {{fileID: {root_go}, guid: {MODEL_GUID},
    type: 3}}
  m_PrefabInstance: {{fileID: 100}}
--- !u!4 &102 stripped
Transform:
  m_CorrespondingSourceObject: {{fileID: {bone_tr}, guid: {MODEL_GUID}, type: 3}}
  m_PrefabInstance: {{fileID: 100}}
--- !u!114 &103
MonoBehaviour:
  m_GameObject: {{fileID: 101}}
  m_Enabled: 1
  m_Script: {{fileID: 542108242, guid: 67cc4cb7839cd3741b63733d5adf0442, type: 3}}
--- !u!1 &110
GameObject:
  m_Name: Hat
  m_IsActive: 1
--- !u!4 &111
Transform:
  m_GameObject: {{fileID: 110}}
  m_LocalPosition: {{x: 0, y: 0.1, z: 0}}
  m_Father: {{fileID: 102}}
"
        );
        std::fs::write(dir.path().join("Assets/Old.unity"), scene).unwrap();
        assets.insert("ffff0000000000000000000000000009", dir.path().join("Assets/Old.unity"));
        let limits = Limits::default();
        let mut loader = Loader::new(&assets, &limits);
        let g = loader.file(&dir.path().join("Assets/Old.unity")).unwrap();
        let Obj::Behaviour(d) = &g.objects[&103] else { panic!() };
        assert_eq!(d.game_object, mixed(100, root_go), "the descriptor is on the model's root");
        assert!(g.objects.contains_key(&d.game_object));
        let Obj::Transform(hat) = &g.objects[&111] else { panic!() };
        assert_eq!(hat.father, mixed(100, bone_tr), "the hat hangs under the model's bone");
    }

    #[test]
    fn a_missing_source_leaves_its_copy_out_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut assets = project(dir.path());
        assets = Assets::empty(&assets.project.clone());
        let limits = Limits::default();
        let mut loader = Loader::new(&assets, &limits);
        let g = loader.file(&dir.path().join("Assets/Scene.unity")).unwrap();
        assert_eq!(loader.missing.len(), 1, "{:?}", loader.missing);
        assert!(g.objects.contains_key(&13), "the scene's own objects stay");
    }

    #[test]
    fn a_prefab_containing_itself_does_not_loop() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("Assets");
        std::fs::create_dir_all(&a).unwrap();
        let guid = "eeee0000000000000000000000000005";
        std::fs::write(
            a.join("Loop.prefab"),
            format!("%YAML 1.1\n--- !u!1001 &1\nPrefabInstance:\n  m_Modification:\n    m_Modifications: []\n  m_SourcePrefab: {{fileID: 100100000, guid: {guid}, type: 3}}\n"),
        )
        .unwrap();
        let mut assets = Assets::empty(dir.path());
        assets.insert(guid, a.join("Loop.prefab"));
        let limits = Limits::default();
        let mut loader = Loader::new(&assets, &limits);
        let g = loader.file(&a.join("Loop.prefab")).unwrap();
        assert!(g.objects.is_empty());
        assert!(!loader.missing.is_empty());
    }
}
