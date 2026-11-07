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
//! filters, skinned mesh renderers (mesh, materials, bones, blend shape weights, the lightmap a renderer samples),
//! terrains, LOD groups, directional lights, the scene's render settings (ambient light, fog, skybox), Bakery's list
//! of the lightmaps it baked and which behaviours are VRChat avatar or world descriptors. Particles, UI, scripts and
//! the rest are skipped unread.

use super::artifact::{Artifact, ArtifactDb};
use super::assets::Assets;
use super::model::{self, ImportSettings, Model};
use super::yaml::{self, Ref, Value};
use super::Limits;
use anyhow::{bail, Context as _};
use orbiters_i18n::tr;
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
/// Bakery's `ftLightmapsStorage`: the lightmaps it baked and which renderer samples which.
const BAKERY_STORAGE: &str = "b7fa80e7116296f4eb4f49ec1544ee22";
/// A renderer without a lightmap (`m_LightmapIndex`; 65534 is lit by realtime GI alone).
pub const NO_LIGHTMAP: i64 = 65534;

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
    /// A look the build made itself (a terrain's blended picture), by its number.
    Baked(usize),
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
    /// The scene's lightmap it samples (65534 and above: none), and where in it (`uv2 * xy + zw`).
    pub lightmap_index: i64,
    pub lightmap_st: [f32; 4],
    pub casts_shadows: bool,
}

/// A terrain: its data lives in a `TerrainData` asset.
#[derive(Clone, Debug)]
pub struct TerrainObject {
    pub game_object: i64,
    pub enabled: bool,
    pub data: Option<Ref>,
    pub trees: bool,
    pub casts_shadows: bool,
    pub lightmap_index: i64,
    pub lightmap_st: [f32; 4],
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
    /// Baked (`m_Lightmapping` 2): its light is in the lightmaps, not added again to what samples them.
    pub baked: bool,
}

#[derive(Clone, Debug)]
pub struct Behaviour {
    pub game_object: i64,
    pub enabled: bool,
    pub script: Option<Ref>,
    /// Holds VRChat avatar descriptor fields (an SDK whose script is not the one known).
    pub avatar_fields: bool,
}

/// A LOD group: only its most detailed level is drawn (the others stand in for it from far away, and would draw over
/// it). The renderers of the levels below it.
#[derive(Clone, Debug)]
pub struct LodGroup {
    pub game_object: i64,
    pub lower: Vec<i64>,
}

#[derive(Clone, Debug)]
pub enum Obj {
    GameObject(GameObject),
    Transform(Transform),
    Renderer(Renderer),
    Filter(Filter),
    Light(Light),
    Behaviour(Behaviour),
    Terrain(TerrainObject),
    Lod(LodGroup),
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
            Obj::Terrain(t) => Some(t.game_object),
            Obj::Lod(l) => Some(l.game_object),
        }
    }

    /// A mesh saved inside the file this object came from (a tool's mesh, a ProBuilder shape), reached from elsewhere.
    fn own_references(&mut self, source: &str) {
        let own = |m: &mut MeshSource| {
            if let MeshSource::Asset(r) = m {
                if r.guid.is_none() && r.file_id != 0 {
                    r.guid = Some(source.to_string());
                }
            }
        };
        match self {
            Obj::Renderer(r) => own(&mut r.mesh),
            Obj::Filter(f) => own(&mut f.mesh),
            _ => {}
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
            Obj::Terrain(t) => t.game_object = k(t.game_object),
            Obj::Lod(l) => {
                l.game_object = k(l.game_object);
                for r in &mut l.lower {
                    *r = k(*r);
                }
            }
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

/// The scene's fog (`RenderSettings`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Fog {
    /// 1 linear, 2 exponential, 3 exponential squared.
    pub mode: i64,
    pub color: [f32; 4],
    pub start: f32,
    pub end: f32,
    pub density: f32,
}

/// The rest of a scene's `RenderSettings` and `LightmapSettings`, of a scene file only (a prefab has none).
#[derive(Clone, Debug, Default)]
pub struct SceneSettings {
    pub fog: Option<Fog>,
    pub skybox: Option<Ref>,
    pub reflection_intensity: Option<f32>,
    pub lighting_data: Option<Ref>,
}

/// What Bakery's storage says of a scene's lightmaps: it sets them when the scene loads, so it is its word that counts,
/// whatever an earlier bake left in the renderers.
#[derive(Clone, Debug, Default)]
pub struct Bakery {
    /// The lightmaps' GUIDs.
    pub maps: Vec<String>,
    /// Which renderer (or terrain) samples which of them, and where.
    pub renderers: HashMap<i64, (usize, [f32; 4])>,
}

/// A file's objects with its instances resolved, by ID in the file.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub objects: HashMap<i64, Obj>,
    pub ambient: Option<Ambient>,
    pub settings: SceneSettings,
    pub bakery: Option<Bakery>,
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
    /// Unity's database of its imports, opened when first needed (none in a project never opened).
    artifact_db: Option<Option<ArtifactDb>>,
    /// Models' artifacts by GUID, once looked for.
    artifacts: HashMap<String, Option<Arc<Artifact>>>,
    /// Models read from their artifacts so far.
    pub artifact_models: usize,
    /// The scene file (a mesh saved in it is read from it).
    pub scene: Option<PathBuf>,
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
            artifact_db: None,
            artifacts: HashMap::new(),
            artifact_models: 0,
            scene: None,
        }
    }

    /// What Unity's import made of the model `guid`, when the project has it and it is not older than the model (a
    /// model changed while Unity was closed keeps its old artifact until the next import): the file's own format
    /// decides how far that matters, so `stale_too` takes one that is older (a format only Unity's import reads).
    pub fn artifact(&mut self, guid: &str, path: &Path, stale_too: bool) -> Option<Arc<Artifact>> {
        if !self.limits.use_artifacts {
            return None;
        }
        if let Some(a) = self.artifacts.get(guid) {
            return a.clone();
        }
        let db = self.artifact_db.get_or_insert_with(|| ArtifactDb::open(&self.assets.project));
        let found = db.as_ref().and_then(|db| db.file_of(guid)).and_then(|file| {
            let time = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            let source = time(path).into_iter().chain(time(&meta_path(path))).max();
            let fresh = match (time(&file), source) {
                (Some(a), Some(s)) => a + std::time::Duration::from_secs(2) >= s,
                _ => false,
            };
            if !fresh && !stale_too {
                return None;
            }
            match Artifact::open(&file, guid, self.limits.max_model_bytes) {
                Ok(a) => Some(Arc::new(a)),
                Err(e) => {
                    // Only worth saying when nothing else can read the model: an FBX is then read from its file.
                    if stale_too {
                        let what = tr!("{name}: its import in Library could not be read: {error}", name = rel(self.assets, path), error = format!("{e:#}"));
                        self.note_missing(what);
                    }
                    None
                }
            }
        });
        if found.is_some() {
            self.artifact_models += 1;
        }
        self.artifacts.insert(guid.to_string(), found.clone());
        found
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
        let size = std::fs::metadata(path).with_context(|| tr!("{name} is missing", name = rel(self.assets, path)))?.len();
        anyhow::ensure!(size <= self.limits.max_yaml_bytes, "{}", tr!("{name} is {size} MB, more than a preview reads", name = rel(self.assets, path), size = size >> 20));
        let bytes = std::fs::read(path).with_context(|| tr!("reading {name}", name = rel(self.assets, path)))?;
        self.files.push(path.to_path_buf());
        let text = String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
        if !yaml::is_text(&text) {
            bail!("{}", tr!("{name} is saved in Unity's binary format (Asset Serialization is not Force Text)", name = rel(self.assets, path)));
        }
        Ok(text)
    }

    fn file_at(&mut self, path: &Path, depth: usize) -> anyhow::Result<Graph> {
        self.limits.check(tr("reading the scene"))?;
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
                114 => {
                    // Only its first lines: behaviours can be megabytes of a tool's data.
                    let head: String = doc.body.lines().take(16).collect::<Vec<_>>().join("\n");
                    let v = yaml::parse(&head);
                    let v = v.entries().first().map(|(_, v)| v.clone()).unwrap_or_default();
                    let avatar_fields = doc.mentions("\n  ViewPosition:") && doc.mentions("\n  Animations:"); // i18n: not shown (a key of Unity's YAML)
                    let script = v.get("m_Script").as_ref();
                    if script.as_ref().and_then(|s| s.guid.as_deref()) == Some(BAKERY_STORAGE) {
                        graph.bakery = bakery_of(&doc.parse());
                    }
                    graph.objects.insert(
                        doc.id,
                        Obj::Behaviour(Behaviour {
                            game_object: go_of(&v),
                            enabled: v.get("m_Enabled").as_bool().unwrap_or(true),
                            script,
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
                    if v.get("m_Fog").as_bool().unwrap_or(false) {
                        graph.settings.fog = Some(Fog {
                            mode: v.get("m_FogMode").as_i64().unwrap_or(3),
                            color: v.get("m_FogColor").color().unwrap_or([0.5, 0.5, 0.5, 1.0]),
                            start: v.get("m_LinearFogStart").f32_or(0.0),
                            end: v.get("m_LinearFogEnd").f32_or(300.0),
                            density: v.get("m_FogDensity").f32_or(0.01),
                        });
                    }
                    graph.settings.skybox = v.get("m_SkyboxMaterial").as_ref().filter(|r| !r.is_null());
                    graph.settings.reflection_intensity = v.get("m_ReflectionIntensity").as_f32();
                }
                157 => {
                    let v = doc.parse();
                    graph.settings.lighting_data = v.get("m_LightingDataAsset").as_ref().filter(|r| !r.is_null());
                }
                1 | 4 | 224 | 23 | 137 | 33 | 108 | 205 | 218 => {
                    if let Some(obj) = object_of(doc.class as i32, &doc.parse()) {
                        graph.objects.insert(doc.id, obj);
                    }
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
            if let Some(b) = &mut graph.bakery {
                b.renderers = std::mem::take(&mut b.renderers).into_iter().map(|(k, v)| (aliases.get(&k).copied().unwrap_or(k), v)).collect();
            }
        }
        Ok(graph)
    }

    /// The resolved objects of the asset `guid` in its own IDs: a prefab's, or a model's.
    pub fn source(&mut self, guid: &str, depth: usize) -> anyhow::Result<Arc<Graph>> {
        if let Some(g) = self.sources.get(guid) {
            return Ok(g.clone());
        }
        if let Some(why) = self.failed.get(guid) {
            bail!("{why}");
        }
        anyhow::ensure!(depth <= MAX_DEPTH, "{}", tr!("prefabs nested more than {depth} deep", depth = MAX_DEPTH));
        anyhow::ensure!(self.resolving.insert(guid.to_string()), "{}", tr("a prefab that contains itself"));
        let result = (|| {
            let path = self.assets.path(guid).with_context(|| tr!("an asset that is not in the project ({guid})", guid = guid))?;
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            match ext.as_str() {
                "prefab" | "unity" => self.file_at(&path, depth + 1),
                "fbx" | "obj" => {
                    if let Some(g) = self.artifact(guid, &path, false).and_then(|a| artifact_graph(&a).ok()) {
                        self.files.push(path.clone());
                        return Ok(g);
                    }
                    let m = self.model(guid)?;
                    Ok(model_graph(guid, &m, &path))
                }
                other if only_unity_reads(other) => {
                    let name = rel(self.assets, &path);
                    let a = self.artifact(guid, &path, true).with_context(|| tr!("{name} is a .{other} model only Unity's import reads: open the project in Unity once", name = name, other = other))?;
                    self.files.push(path.clone());
                    self.files.push(meta_path(&path));
                    artifact_graph(&a).with_context(|| name)
                }
                other => bail!("{}", tr!("{name} is a .{other} model, which previews do not read", name = rel(self.assets, &path), other = other)),
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
        let path = self.assets.path(guid).with_context(|| tr!("a model that is not in the project ({guid})", guid = guid))?;
        // Every model a scene's renderers use is loaded before the drawing budget picks among them: a world of hundreds
        // of models would hold them all (gigabytes) for a preview that draws 2 million triangles.
        anyhow::ensure!(
            self.model_triangles < self.limits.max_loaded_triangles,
            "{}",
            tr!(
                "{name} left out: the scene's models have more than {millions} million triangles",
                name = rel(self.assets, &path),
                millions = self.limits.max_loaded_triangles / 1_000_000
            )
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
        self.limits.check(tr("resolving prefabs"))?;
        self.prefab_instances += 1;
        anyhow::ensure!(self.prefab_instances <= self.limits.max_prefab_instances, "{}", tr!("more than {count} prefab instances", count = self.limits.max_prefab_instances));
        let Some(source) = pi.get("m_SourcePrefab").as_ref().and_then(|r| r.guid) else { return Ok(()) };
        let src = match self.source(&source, depth) {
            Ok(g) => g,
            Err(e) => {
                // The time limit stops the whole preview; a missing or unreadable source only leaves its copy out.
                self.limits.check(tr("resolving prefabs"))?;
                let name = self.assets.path(&source).map(|p| rel(self.assets, &p)).unwrap_or_else(|| tr!("a prefab ({guid})", guid = source));
                self.note_missing(format!("{name}: {e:#}"));
                return Ok(());
            }
        };
        // Each instance copies its source's objects, and a source holding fifty instances of a prefab holding fifty
        // of another is 2,500 copies of that one: a few levels of that would fill the memory, which the count of
        // instances (each source is resolved once) never sees.
        anyhow::ensure!(self.objects_made + src.objects.len() <= self.limits.max_objects, "{}", tr!("more than {count} objects in its prefabs", count = self.limits.max_objects));
        self.objects_made += src.objects.len();
        let m = pi.get("m_Modification");
        let parent = m.get("m_TransformParent").as_ref().map_or(0, |r| r.file_id);
        let mut objects: HashMap<i64, Obj> = HashMap::with_capacity(src.objects.len());
        for (id, obj) in &src.objects {
            let mut o = obj.clone();
            o.remap(|x| mixed(instance, x));
            o.own_references(&source);
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

/// Model formats Unity hands to another program at import (Blender, Maya, 3ds Max…) or reads with its own importers:
/// only the result of that import can be read.
pub fn only_unity_reads(ext: &str) -> bool {
    matches!(ext, "blend" | "max" | "ma" | "mb" | "c4d" | "lxo" | "3ds" | "dae" | "dxf" | "skp" | "jas" | "lwo" | "lws")
}

/// A model's objects from its artifact.
fn artifact_graph(a: &Artifact) -> anyhow::Result<Graph> {
    a.graph()
}

/// An object of the classes a preview draws, from its fields (a text file's or a binary file's: both read alike).
pub fn object_of(class: i32, v: &Value) -> Option<Obj> {
    Some(match class {
        1 => Obj::GameObject(GameObject {
            name: v.get("m_Name").str_or("").to_string(),
            active: v.get("m_IsActive").as_bool().unwrap_or(true),
            editor_only: v.get("m_TagString").as_str() == Some("EditorOnly"),
        }),
        4 | 224 => Obj::Transform(transform_of(v)),
        23 | 137 => {
            let skinned = class == 137;
            Obj::Renderer(Renderer {
                game_object: go_of(v),
                enabled: v.get("m_Enabled").as_bool().unwrap_or(true),
                skinned,
                materials: v.get("m_Materials").items().iter().map(|m| m.as_ref().map_or(MatSource::None, mat_source)).collect(),
                mesh: if skinned { mesh_source(v.get("m_Mesh").as_ref()) } else { MeshSource::None },
                bones: v.get("m_Bones").items().iter().filter_map(|b| b.as_ref()).map(|r| if r.guid.is_none() { r.file_id } else { 0 }).collect(),
                blend_weights: v.get("m_BlendShapeWeights").items().iter().map(|w| w.f32_or(0.0)).collect(),
                lightmap_index: v.get("m_LightmapIndex").as_i64().unwrap_or(65535),
                lightmap_st: scale_offset(v.get("m_LightmapTilingOffset")),
                casts_shadows: v.get("m_CastShadows").as_i64().unwrap_or(1) != 0,
            })
        }
        33 => Obj::Filter(Filter { game_object: go_of(v), mesh: mesh_source(v.get("m_Mesh").as_ref()) }),
        108 => Obj::Light(Light {
            game_object: go_of(v),
            enabled: v.get("m_Enabled").as_bool().unwrap_or(true),
            kind: v.get("m_Type").as_i64().unwrap_or(2),
            color: v.get("m_Color").color().unwrap_or([1.0; 4]),
            intensity: v.get("m_Intensity").f32_or(1.0),
            shadows: v.at(&["m_Shadows", "m_Type"]).as_i64().unwrap_or(0) != 0,
            shadow_strength: v.at(&["m_Shadows", "m_Strength"]).f32_or(1.0),
            baked: v.get("m_Lightmapping").as_i64() == Some(2),
        }),
        205 => {
            let mut lower = Vec::new();
            let levels = v.get("m_LODs").items();
            let first: Vec<i64> = levels.first().map(lod_renderers).unwrap_or_default();
            for level in levels.iter().skip(1) {
                lower.extend(lod_renderers(level).into_iter().filter(|r| !first.contains(r)));
            }
            Obj::Lod(LodGroup { game_object: go_of(v), lower })
        }
        218 => Obj::Terrain(TerrainObject {
            game_object: go_of(v),
            enabled: v.get("m_Enabled").as_bool().unwrap_or(true),
            data: v.get("m_TerrainData").as_ref().filter(|r| !r.is_null()),
            trees: v.get("m_DrawTreesAndFoliage").as_bool().unwrap_or(true),
            casts_shadows: v.get("m_CastShadows").as_i64().unwrap_or(1) != 0,
            lightmap_index: v.get("m_LightmapIndex").as_i64().unwrap_or(65535),
            lightmap_st: scale_offset(v.get("m_LightmapTilingOffset")),
        }),
        _ => return None,
    })
}

fn lod_renderers(level: &Value) -> Vec<i64> {
    level.get("renderers").items().iter().filter_map(|r| r.get("renderer").as_ref()).filter(|r| r.guid.is_none() && r.file_id != 0).map(|r| r.file_id).collect()
}

/// `{x, y, z, w}`: a scale and an offset (a mesh's default, whole and in place, without one).
fn scale_offset(v: &Value) -> [f32; 4] {
    v.quat().unwrap_or([1.0, 1.0, 0.0, 0.0])
}

/// Bakery's storage: a list of lightmaps, then each baked renderer with the index of its lightmap and where in it.
/// Indices read as a hex run of little-endian 32-bit numbers (Unity's text for a list of ints), or as a list.
fn bakery_of(v: &Value) -> Option<Bakery> {
    let maps: Vec<String> = v.get("maps").items().iter().map(|m| m.as_ref().and_then(|r| r.guid).unwrap_or_default()).collect();
    let ints = |v: &Value| -> Vec<i64> {
        match v {
            Value::Str(h) => h
                .as_bytes()
                .chunks_exact(8)
                .map(|c| {
                    let s = std::str::from_utf8(c).unwrap_or("");
                    let raw = u32::from_str_radix(s, 16).unwrap_or(0);
                    u32::from_le_bytes(raw.to_be_bytes()) as i32 as i64
                })
                .collect(),
            v => v.items().iter().filter_map(Value::as_i64).collect(),
        }
    };
    let mut renderers = HashMap::new();
    for (list, ids, st) in [("bakedRenderers", "bakedIDs", "bakedScaleOffset"), ("bakedRenderersTerrain", "bakedIDsTerrain", "bakedScaleOffsetTerrain")] {
        let ids = ints(v.get(ids));
        let offsets = v.get(st).items();
        for (i, r) in v.get(list).items().iter().enumerate() {
            let (Some(r), Some(&id), Some(so)) = (r.as_ref(), ids.get(i), offsets.get(i)) else { continue };
            let Ok(id) = usize::try_from(id) else { continue };
            if r.guid.is_none() && r.file_id != 0 && id < maps.len() {
                renderers.insert(r.file_id, (id, scale_offset(so)));
            }
        }
    }
    (!maps.is_empty()).then_some(Bakery { maps, renderers })
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
            } else if path == "m_LightmapIndex" {
                r.lightmap_index = value.as_i64().unwrap_or(r.lightmap_index);
            } else if path == "m_CastShadows" {
                r.casts_shadows = value.as_i64().unwrap_or(1) != 0;
            } else if let Some(a) = path.strip_prefix("m_LightmapTilingOffset.") {
                match a {
                    "x" => set(&mut r.lightmap_st[0]),
                    "y" => set(&mut r.lightmap_st[1]),
                    "z" => set(&mut r.lightmap_st[2]),
                    "w" => set(&mut r.lightmap_st[3]),
                    _ => {}
                }
            }
        }
        Obj::Terrain(t) => match path {
            "m_Enabled" => t.enabled = value.as_bool().unwrap_or(t.enabled),
            "m_TerrainData" => t.data = reference.filter(|r| !r.is_null()),
            "m_LightmapIndex" => t.lightmap_index = value.as_i64().unwrap_or(t.lightmap_index),
            "m_LightmapTilingOffset.x" => set(&mut t.lightmap_st[0]),
            "m_LightmapTilingOffset.y" => set(&mut t.lightmap_st[1]),
            "m_LightmapTilingOffset.z" => set(&mut t.lightmap_st[2]),
            "m_LightmapTilingOffset.w" => set(&mut t.lightmap_st[3]),
            _ => {}
        },
        Obj::Lod(_) => {}
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
            "m_Lightmapping" => l.baked = value.as_i64() == Some(2),
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
                lightmap_index: 65535,
                lightmap_st: [1.0, 1.0, 0.0, 0.0],
                casts_shadows: true,
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
