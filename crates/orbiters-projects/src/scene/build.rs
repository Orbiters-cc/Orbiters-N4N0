//! A scene's preview built from the project's files: the resolved scene ([`super::prefab`]) posed as Unity draws it
//! (each transform's world matrix; skinned meshes deformed by their bones at the pose the scene gives them, after
//! their blend shapes at the weights the scene sets), each renderer's materials read ([`super::material`]) and their
//! pictures made small ([`super::texture`]), written in the format Orbiters Toolkit writes (contracts'
//! Docs/project-preview.md) so that the desktop draws both alike.
//!
//! What a thumbnail does not need is left out as Toolkit leaves it out: renderers that are off, objects inactive in
//! the hierarchy or tagged `EditorOnly`, particles, lines, UI. The budget is Toolkit's: 2,000,000 triangles, the
//! avatars' renderers first, then the rest from the largest down; pictures at most 1024, 512 or 256 pixels on a side
//! as the scene has more of them.

use super::assets::Assets;
use super::material::{self, Look};
use super::model::{ModelMesh, Skin};
use super::prefab::{self, Graph, Loader, MatSource, MeshSource, Obj};
use super::{math, primitives, texture, Limits};
use crate::preview::{self as file, Bounds, Draw, Header, ImageFormat, IndexFormat, Instances, Kind, Mesh, Submesh, Texture, Writer};
use anyhow::Context as _;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// The scene to build.
#[derive(Clone, Debug)]
pub struct SceneFile {
    pub path: PathBuf,
    pub guid: String,
    pub name: String,
}

/// What a build made and read.
pub struct Built {
    pub bytes: Vec<u8>,
    pub header: Header,
    /// Every file the preview depends on (the scene, prefabs, models, materials, textures and their `.meta` files).
    pub files: Vec<PathBuf>,
    pub report: Report,
}

/// What a build found, for diagnostics and the validation table.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Report {
    pub objects: usize,
    pub renderers: usize,
    pub drawn: usize,
    pub meshes: usize,
    pub models: usize,
    pub materials: usize,
    pub textures: usize,
    pub triangles: u64,
    pub prefab_instances: usize,
    pub missing: Vec<String>,
    pub ms: f64,
}

const BUDGET: u64 = 2_000_000;

struct Candidate {
    renderer: i64,
    transform: i64,
    mesh: Arc<ModelMesh>,
    key: String,
    skinned: bool,
    materials: Vec<MatSource>,
    bones: Vec<i64>,
    blend: Vec<f32>,
    avatar: Option<u32>,
    world_bounds: Bounds,
    triangles: u64,
}

pub fn build(project: &Path, scene: &SceneFile, assets: &Assets, limits: &Limits) -> anyhow::Result<Built> {
    let start = Instant::now();
    let mut loader = Loader::new(assets, limits);
    let graph = loader.file(&scene.path)?;
    let meta = prefab::meta_path(&scene.path);
    loader.files.push(meta);
    let mut report = Report { objects: graph.objects.len(), ..Default::default() };
    let world = World::new(&graph);

    // Avatars (their descriptors) and the world's.
    let mut avatars: Vec<(i64, file::AvatarPlace)> = Vec::new();
    let mut world_scene = false;
    for obj in graph.objects.values() {
        let Obj::Behaviour(b) = obj else { continue };
        let script = b.script.as_ref().map(|s| (s.file_id, s.guid.as_deref().unwrap_or("")));
        let is_avatar = script == Some((prefab::AVATAR_DESCRIPTOR.0, prefab::AVATAR_DESCRIPTOR.1)) || b.avatar_fields;
        let is_world = script == Some((prefab::SCENE_DESCRIPTOR.0, prefab::SCENE_DESCRIPTOR.1));
        let Some(&t) = world.transform_of.get(&b.game_object) else { continue };
        if !world.shown(t) {
            continue;
        }
        if is_avatar {
            let m = world.matrix(t);
            let name = match graph.objects.get(&b.game_object) {
                Some(Obj::GameObject(g)) => g.name.clone(),
                _ => String::new(),
            };
            let f = math::normalize([m[8], m[9], m[10]]);
            avatars.push((t, file::AvatarPlace { name, id: None, position: [m[12], m[13], m[14]], forward: f }));
        }
        world_scene |= is_world;
    }
    avatars.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    let kind = if !avatars.is_empty() {
        Kind::Avatars
    } else if world_scene {
        Kind::World
    } else {
        Kind::Scene
    };

    // Renderers to draw, with their meshes.
    let mut filters: HashMap<i64, &MeshSource> = HashMap::new();
    for obj in graph.objects.values() {
        if let Obj::Filter(f) = obj {
            filters.insert(f.game_object, &f.mesh);
        }
    }
    let mut meshes: HashMap<String, Option<Arc<ModelMesh>>> = HashMap::new();
    let mut candidates = Vec::new();
    let mut ids: Vec<&i64> = graph.objects.keys().collect();
    ids.sort();
    for id in ids {
        let Obj::Renderer(r) = &graph.objects[id] else { continue };
        report.renderers += 1;
        let Some(&t) = world.transform_of.get(&r.game_object) else { continue };
        if !r.enabled || !world.shown(t) {
            continue;
        }
        let source = if r.skinned { &r.mesh } else { filters.get(&r.game_object).copied().unwrap_or(&MeshSource::None) };
        let key = match source {
            MeshSource::None => continue,
            MeshSource::Model { guid, mesh } => format!("{guid}:{mesh}"),
            MeshSource::Asset(r) => format!("{}:{}", r.guid.as_deref().unwrap_or("-"), r.file_id),
        };
        if !meshes.contains_key(&key) {
            limits.check("reading the meshes")?;
            let m = match resolve_mesh(&mut loader, source) {
                Ok(m) => Some(m),
                Err(e) => {
                    limits.check("reading the meshes")?;
                    let what = format!("{e:#}");
                    if !loader.missing.contains(&what) && loader.missing.len() < 64 {
                        loader.missing.push(what);
                    }
                    None
                }
            };
            meshes.insert(key.clone(), m);
        }
        let Some(mesh) = meshes[&key].clone() else { continue };
        if mesh.positions.is_empty() || mesh.triangles() == 0 {
            continue;
        }
        let avatar = avatars.iter().position(|(at, _)| world.under(t, *at)).map(|i| i as u32);
        let m = world.matrix(t);
        let local = bounds_of(&mesh.positions);
        let world_bounds = transform_bounds(&m, &local);
        candidates.push(Candidate {
            renderer: *id,
            transform: t,
            triangles: mesh.triangles() as u64,
            mesh,
            key,
            skinned: r.skinned,
            materials: r.materials.clone(),
            bones: r.bones.clone(),
            blend: r.blend_weights.clone(),
            avatar,
            world_bounds,
        });
    }
    if candidates.is_empty() {
        match loader.missing.first() {
            Some(why) => anyhow::bail!("nothing could be drawn: {why}"),
            None => anyhow::bail!("the scene has nothing to draw"),
        }
    }
    // Toolkit's order for the budget: avatars first, then the largest.
    candidates.sort_by(|a, b| {
        b.avatar.is_some().cmp(&a.avatar.is_some()).then(b.world_bounds.radius().total_cmp(&a.world_bounds.radius())).then(a.renderer.cmp(&b.renderer))
    });
    let mut skipped_budget = 0u32;
    let mut used = 0u64;
    candidates.retain(|c| {
        if used + c.triangles > BUDGET {
            skipped_budget += 1;
            return false;
        }
        used += c.triangles;
        true
    });

    // Materials and the pictures they need.
    let mut looks: HashMap<String, (usize, Look)> = HashMap::new();
    let mut materials: Vec<file::Material> = Vec::new();
    let mut look_list: Vec<Look> = Vec::new();
    let mut shader_names: HashMap<String, String> = HashMap::new();
    let mut draws_materials: Vec<Vec<i32>> = Vec::new();
    for c in &candidates {
        let mut slots = Vec::new();
        for s in 0..c.mesh.submeshes.len() {
            let Some(src) = c.materials.get(s) else {
                // Fewer materials than submeshes: Unity draws the rest with none.
                slots.push(-1);
                continue;
            };
            let key = match src {
                MatSource::None => "none".to_string(),
                MatSource::Model { guid, material } => format!("m:{guid}:{material}"),
                MatSource::Asset(r) => format!("a:{}:{}", r.guid.as_deref().unwrap_or("-"), r.file_id),
            };
            if !looks.contains_key(&key) {
                limits.check("reading the materials")?;
                let look = resolve_material(&mut loader, src, &mut shader_names).unwrap_or_else(|e| {
                    let what = format!("{e:#}");
                    if !loader.missing.contains(&what) && loader.missing.len() < 64 {
                        loader.missing.push(what);
                    }
                    Look { material: file::Material { name: "Missing".into(), color: [0.75, 0.75, 0.75, 1.0], ..Default::default() }, ..Default::default() }
                });
                looks.insert(key.clone(), (materials.len(), look.clone()));
                materials.push(look.material.clone());
                look_list.push(look);
            }
            slots.push(looks[&key].0 as i32);
        }
        draws_materials.push(slots);
    }
    report.materials = materials.len();

    // Pictures: decoded on several threads, at the budget's size.
    let mut wanted: Vec<(String, bool)> = Vec::new();
    for l in &look_list {
        for (t, main) in [(&l.main_texture, true), (&l.emission_texture, false)] {
            if let Some(g) = t {
                if !wanted.iter().any(|(w, m)| w == g && *m == main) {
                    wanted.push((g.clone(), main));
                }
            }
        }
    }
    let side = match wanted.len() {
        0..=8 => 1024,
        9..=32 => 512,
        _ => 256,
    };
    let jobs: Vec<(String, bool, Option<PathBuf>, bool)> = wanted
        .iter()
        .map(|(g, main)| {
            let keep_alpha = look_list.iter().any(|l| l.main_texture.as_deref() == Some(g) && l.material.alpha != file::Alpha::Opaque);
            (g.clone(), *main, assets.path(g), keep_alpha)
        })
        .collect();
    for (_, _, p, _) in &jobs {
        if let Some(p) = p {
            loader.files.push(p.clone());
            loader.files.push(prefab::meta_path(p));
        }
    }
    limits.check("reading the pictures")?;
    let decoded = decode_all(&jobs, side, limits);
    let mut writer = Writer::default();
    let mut textures: Vec<Texture> = Vec::new();
    let mut texture_index: HashMap<(String, bool), i32> = HashMap::new();
    for ((g, main, path, _), result) in jobs.iter().zip(decoded) {
        match result {
            Ok(e) => {
                let data = writer.push(&e.bytes);
                texture_index.insert((g.clone(), *main), textures.len() as i32);
                textures.push(Texture {
                    width: e.width,
                    height: e.height,
                    format: if e.png { ImageFormat::Png } else { ImageFormat::Jpeg },
                    linear: false,
                    data,
                });
            }
            Err(e) => {
                let name = path.as_ref().map(|p| prefab::rel(assets, p)).unwrap_or_else(|| format!("a texture ({g})"));
                let what = format!("{name}: {e:#}");
                if !loader.missing.contains(&what) && loader.missing.len() < 64 {
                    loader.missing.push(what);
                }
            }
        }
    }
    for (i, l) in look_list.iter().enumerate() {
        materials[i].texture = l.main_texture.as_ref().and_then(|g| texture_index.get(&(g.clone(), true))).copied().unwrap_or(-1);
        materials[i].emission_texture = l.emission_texture.as_ref().and_then(|g| texture_index.get(&(g.clone(), false))).copied().unwrap_or(-1);
        if l.emission_texture.is_some() && materials[i].emission_texture < 0 {
            // A glow masked by a picture that could not be read would light the whole surface: none.
            materials[i].emission = [0.0; 3];
        }
    }
    report.textures = textures.len();

    // Meshes: skinned ones posed into the world, the others once each with their matrices.
    let mut out_meshes: Vec<Mesh> = Vec::new();
    let mut mesh_slot: HashMap<String, u32> = HashMap::new();
    let mut matrices: Vec<[f32; 16]> = Vec::new();
    let mut draws: Vec<Draw> = Vec::new();
    let mut bounds = Bounds { min: [f32::MAX; 3], max: [f32::MIN; 3] };
    let mut focus = Bounds { min: [f32::MAX; 3], max: [f32::MIN; 3] };
    let mut vertices_total = 0u64;
    let identity = math::IDENTITY;
    for (c, slots) in candidates.iter().zip(&draws_materials) {
        limits.check("posing the meshes")?;
        let m = world.matrix(c.transform);
        let posed = c.skinned && (c.mesh.skin.is_some() || c.blend.iter().any(|w| *w != 0.0));
        let (mesh_index, matrix, world_b) = if posed {
            let (positions, normals) = pose(&c.mesh, &c.blend, &c.bones, &world, m);
            let (mesh, b) = push_mesh(&mut writer, &positions, &normals, &c.mesh.uvs, &c.mesh.submeshes);
            out_meshes.push(mesh);
            vertices_total += positions.len() as u64;
            let wb = if c.mesh.skin.is_some() { b } else { transform_bounds(&m, &b) };
            (out_meshes.len() as u32 - 1, if c.mesh.skin.is_some() { identity } else { m }, wb)
        } else {
            let index = match mesh_slot.get(&c.key) {
                Some(&i) => i,
                None => {
                    let (mesh, _) = push_mesh(&mut writer, &c.mesh.positions, &c.mesh.normals, &c.mesh.uvs, &c.mesh.submeshes);
                    out_meshes.push(mesh);
                    vertices_total += c.mesh.positions.len() as u64;
                    let i = out_meshes.len() as u32 - 1;
                    mesh_slot.insert(c.key.clone(), i);
                    i
                }
            };
            (index, m, c.world_bounds)
        };
        let instance = match matrices.iter().position(|x| *x == matrix) {
            Some(i) => i as u32,
            None => {
                matrices.push(matrix);
                matrices.len() as u32 - 1
            }
        };
        grow(&mut bounds, &world_b);
        if c.avatar.is_some() {
            grow(&mut focus, &world_b);
        }
        draws.push(Draw { instance, mesh: mesh_index, materials: slots.clone(), shadows: true, avatar: c.avatar });
    }
    report.drawn = draws.len();
    report.meshes = out_meshes.len();
    report.models = loader.models.len();
    report.triangles = used;
    if bounds.min[0] > bounds.max[0] {
        bounds = Bounds { min: [-0.5, 0.0, -0.5], max: [0.5, 1.0, 0.5] };
    }
    if focus.min[0] > focus.max[0] {
        focus = bounds;
    }
    let matrices_blob = writer.push(&matrices.iter().flatten().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>());
    let ground = (kind == Kind::Avatars).then(|| avatars.iter().map(|a| a.1.position[1]).fold(f32::MAX, f32::min));
    let lighting = lighting_of(&graph, &world);
    let mut skipped = std::collections::BTreeMap::new();
    if skipped_budget > 0 {
        skipped.insert("budget".to_string(), skipped_budget);
    }
    let missing_meshes = meshes.values().filter(|m| m.is_none()).count() as u32;
    if missing_meshes > 0 {
        skipped.insert("missing".to_string(), missing_meshes);
    }
    report.prefab_instances = loader.prefab_instances;
    report.missing = loader.missing.clone();
    report.ms = start.elapsed().as_secs_f64() * 1000.0;
    let header = Header {
        written_utc: now_utc(),
        generator: format!("Orbiters - N4N0 {} (from the project's files)", env!("CARGO_PKG_VERSION")),
        unity: unity_version(project),
        scene: file::Scene { name: scene.name.clone(), path: prefab::rel(assets, &scene.path), guid: scene.guid.clone() },
        kind,
        bounds,
        focus,
        ground,
        camera: None,
        avatars: avatars.into_iter().map(|a| a.1).collect(),
        textures,
        materials,
        meshes: out_meshes,
        instances: Instances { count: matrices.len() as u32, matrices: matrices_blob },
        draws,
        lighting,
        stats: file::Stats { renderers: report.renderers as u32, triangles: used, vertices: vertices_total, skipped },
        cost: Some(file::Cost { total_ms: report.ms as f32, ..Default::default() }),
    };
    let bytes = writer.finish(&header);
    let mut files = loader.files;
    files.sort();
    files.dedup();
    Ok(Built { bytes, header, files, report })
}

fn now_utc() -> String {
    time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default()
}

fn unity_version(project: &Path) -> String {
    std::fs::read_to_string(project.join("ProjectSettings/ProjectVersion.txt"))
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("m_EditorVersion:").map(|v| v.trim().to_string())))
        .unwrap_or_default()
}

/// Every transform's world matrix and whether it shows (active in the hierarchy, not `EditorOnly`).
pub struct World<'g> {
    graph: &'g Graph,
    pub transform_of: HashMap<i64, i64>,
    matrices: HashMap<i64, [f32; 16]>,
    shown: HashMap<i64, bool>,
    parent: HashMap<i64, i64>,
}

impl<'g> World<'g> {
    pub fn new(graph: &'g Graph) -> Self {
        let mut transform_of = HashMap::new();
        let mut parent = HashMap::new();
        for (id, o) in &graph.objects {
            if let Obj::Transform(t) = o {
                transform_of.insert(t.game_object, *id);
                parent.insert(*id, t.father);
            }
        }
        let mut w = Self { graph, transform_of, matrices: HashMap::new(), shown: HashMap::new(), parent };
        let ids: Vec<i64> = w.parent.keys().copied().collect();
        for id in ids {
            w.resolve(id);
        }
        w
    }

    /// Computes a transform and its ancestors (iteratively: hierarchies can be hundreds deep).
    fn resolve(&mut self, id: i64) {
        let mut chain = vec![id];
        let mut seen = std::collections::HashSet::new();
        while let Some(&top) = chain.last() {
            if self.matrices.contains_key(&top) {
                chain.pop();
                break;
            }
            let p = self.parent.get(&top).copied().unwrap_or(0);
            if p == 0 || !self.parent.contains_key(&p) || !seen.insert(top) || chain.len() > 4096 {
                break;
            }
            chain.push(p);
        }
        while let Some(t) = chain.pop() {
            let Some(Obj::Transform(tr)) = self.graph.objects.get(&t) else { continue };
            let local = math::trs(tr.position, tr.rotation, tr.scale);
            let (pm, ps) = match self.parent.get(&t).copied().filter(|p| *p != 0) {
                Some(p) => match (self.matrices.get(&p), self.shown.get(&p)) {
                    (Some(m), Some(s)) => (*m, *s),
                    // A parent that is not there (removed, or a source that could not be read): not drawn.
                    _ => (math::IDENTITY, false),
                },
                None => (math::IDENTITY, true),
            };
            let own = match self.graph.objects.get(&tr.game_object) {
                Some(Obj::GameObject(g)) => g.active && !g.editor_only,
                _ => false,
            };
            self.matrices.insert(t, math::mul(&pm, &local));
            self.shown.insert(t, ps && own);
        }
    }

    pub fn matrix(&self, t: i64) -> [f32; 16] {
        self.matrices.get(&t).copied().unwrap_or(math::IDENTITY)
    }

    pub fn shown(&self, t: i64) -> bool {
        self.shown.get(&t).copied().unwrap_or(false)
    }

    /// Whether `t` is `root` or under it.
    pub fn under(&self, mut t: i64, root: i64) -> bool {
        for _ in 0..4096 {
            if t == root {
                return true;
            }
            match self.parent.get(&t) {
                Some(&p) if p != 0 => t = p,
                _ => return false,
            }
        }
        false
    }
}

fn resolve_mesh(loader: &mut Loader, source: &MeshSource) -> anyhow::Result<Arc<ModelMesh>> {
    match source {
        MeshSource::None => anyhow::bail!("no mesh"),
        MeshSource::Model { guid, mesh } => {
            let m = loader.model(guid)?;
            Ok(Arc::new(clone_mesh(&m.meshes[*mesh])))
        }
        MeshSource::Asset(r) => {
            let guid = r.guid.as_deref().context("a mesh in the scene itself")?;
            if guid == "0000000000000000e000000000000000" {
                return primitives::builtin(r.file_id).map(Arc::new).with_context(|| format!("Unity's built-in mesh {}", r.file_id));
            }
            let path = loader.assets.path(guid).with_context(|| format!("a mesh that is not in the project ({guid})"))?;
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            match ext.as_str() {
                "fbx" | "obj" => {
                    let m = loader.model(guid)?;
                    let index = m
                        .mesh_ids
                        .get(&r.file_id)
                        .copied()
                        // Models imported before Unity 2019.3 number their meshes 4300000, 4300002, …
                        .or_else(|| (r.file_id >= 4_300_000 && r.file_id < 4_400_000 && r.file_id % 2 == 0).then(|| ((r.file_id - 4_300_000) / 2) as usize))
                        .filter(|i| *i < m.meshes.len())
                        .with_context(|| format!("a mesh {} does not have", prefab::rel(loader.assets, &path)))?;
                    Ok(Arc::new(clone_mesh(&m.meshes[index])))
                }
                "asset" | "mesh" => {
                    loader.files.push(path.clone());
                    super::mesh_asset::read(&path, r.file_id, loader.limits).map(Arc::new).with_context(|| prefab::rel(loader.assets, &path))
                }
                other => anyhow::bail!("{} is a .{other} mesh, which previews do not read", prefab::rel(loader.assets, &path)),
            }
        }
    }
}

/// Meshes are shared through the model (`Arc`); a renderer's own copy is only its reference.
fn clone_mesh(m: &ModelMesh) -> ModelMesh {
    ModelMesh {
        name: m.name.clone(),
        positions: m.positions.clone(),
        normals: m.normals.clone(),
        uvs: m.uvs.clone(),
        submeshes: m.submeshes.clone(),
        skin: m.skin.as_ref().map(|s| Skin { bones: s.bones.clone(), bind_poses: s.bind_poses.clone(), weights: s.weights.clone() }),
        blend_shapes: m
            .blend_shapes
            .iter()
            .map(|b| super::model::BlendShape { name: b.name.clone(), frames: b.frames.clone(), default_weight: b.default_weight })
            .collect(),
    }
}

/// A material slot's look.
fn resolve_material(loader: &mut Loader, src: &MatSource, shader_names: &mut HashMap<String, String>) -> anyhow::Result<Look> {
    match src {
        MatSource::None => {
            Ok(Look { material: file::Material { name: "None".into(), color: [0.75, 0.75, 0.75, 1.0], ..Default::default() }, ..Default::default() })
        }
        MatSource::Asset(r) => {
            let guid = r.guid.as_deref().context("a material in the scene itself")?;
            if guid == "0000000000000000f000000000000000" {
                // Unity's Default-Material and Default-Diffuse.
                return Ok(Look { material: file::Material { name: "Default-Material".into(), ..Default::default() }, ..Default::default() });
            }
            let path = loader.assets.path(guid).with_context(|| format!("a material that is not in the project ({guid})"))?;
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if ext == "fbx" || ext == "obj" {
                // A model's own material, by Unity's ID of it.
                let m = loader.model(guid)?;
                let k = m.materials.iter().position(|mm| super::model::file_id("Material", &mm.name, 0) == r.file_id).unwrap_or(0);
                return model_slot(loader, guid, k, shader_names);
            }
            mat_file(loader, &path, shader_names)
        }
        MatSource::Model { guid, material } => model_slot(loader, guid, *material, shader_names),
    }
}

fn mat_file(loader: &mut Loader, path: &Path, shader_names: &mut HashMap<String, String>) -> anyhow::Result<Look> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", prefab::rel(loader.assets, path)))?;
    loader.files.push(path.to_path_buf());
    let (name, shader, props) =
        material::props(&text).with_context(|| format!("{} is not a material Unity saved as text", prefab::rel(loader.assets, path)))?;
    let shader_name = match shader {
        Some(s) if s.guid.as_deref() == Some("0000000000000000f000000000000000") => material::builtin_shader(s.file_id).to_string(),
        Some(s) => match s.guid {
            Some(g) => match shader_names.get(&g) {
                Some(n) => n.clone(),
                None => {
                    let n = loader
                        .assets
                        .path(&g)
                        .and_then(|p| {
                            let ext = p.extension()?.to_string_lossy().to_ascii_lowercase();
                            if ext == "shader" {
                                let mut head = vec![0u8; 8192];
                                use std::io::Read as _;
                                let n = std::fs::File::open(p).ok()?.read(&mut head).ok()?;
                                material::shader_name(&String::from_utf8_lossy(&head[..n]))
                            } else {
                                // A Shader Graph or an Amplify file: its name is the file's.
                                Some(p.file_stem()?.to_string_lossy().to_ascii_lowercase())
                            }
                        })
                        .unwrap_or_default();
                    shader_names.insert(g.clone(), n.clone());
                    n
                }
            },
            None => String::new(),
        },
        None => String::new(),
    };
    Ok(material::look(&name, &shader_name, &props))
}

/// What Unity draws in a model's material slot: the material its import remaps there, the legacy external material
/// found by name, or the model's own (its colour and its texture found by name in the project).
fn model_slot(loader: &mut Loader, guid: &str, k: usize, shader_names: &mut HashMap<String, String>) -> anyhow::Result<Look> {
    let model = loader.model(guid)?;
    let settings = loader.model_settings(guid);
    let mm = model.materials.get(k).cloned().unwrap_or_default();
    if let Some(remap) = settings.remapped.get(&mm.name) {
        if let Some(p) = loader.assets.path(remap) {
            return mat_file(loader, &p, shader_names);
        }
    }
    let model_path = loader.assets.path(guid).unwrap_or_default();
    let dir = model_path.parent().map(Path::to_path_buf).unwrap_or_default();
    if settings.legacy_materials {
        let texture_stem = mm.texture.as_deref().and_then(|t| Path::new(t).file_stem()).map(|s| s.to_string_lossy().into_owned());
        let model_stem = model_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let name = match settings.material_name {
            0 => texture_stem.unwrap_or_else(|| mm.name.clone()),
            2 => format!("{}-{}", texture_stem.unwrap_or_default(), mm.name),
            3 => format!("{model_stem}-{}", mm.name),
            _ => mm.name.clone(),
        };
        let file = format!("{name}.mat");
        // The model's Materials folder, then each folder up, then (search everywhere) anywhere.
        let mut at = Some(dir.as_path());
        while let Some(d) = at {
            let p = d.join("Materials").join(&file);
            if p.is_file() {
                return mat_file(loader, &p, shader_names);
            }
            if settings.material_search == 0 || d == loader.assets.project.join("Assets") {
                break;
            }
            at = d.parent();
        }
        if settings.material_search == 2 {
            if let Some(p) = loader.assets.named(&file, Some(&dir)).into_iter().next() {
                return mat_file(loader, &p, shader_names);
            }
        }
    }
    // The model's own material: Standard with its colour and its texture.
    let mut look = Look { material: file::Material { name: mm.name.clone(), color: mm.color, ..Default::default() }, ..Default::default() };
    if let Some(t) = mm.texture.as_deref() {
        if let Some(p) = loader.assets.named(t, Some(&dir)).into_iter().next() {
            look.main_texture = super::assets_guid_of_file(&p);
            if look.main_texture.is_some() {
                look.material.color = [1.0; 4];
            }
        }
    }
    Ok(look)
}

/// Blend shapes at `weights`, then the bones' pose: positions and normals in world space for a skinned mesh, else in
/// the renderer's own space.
fn pose(mesh: &ModelMesh, weights: &[f32], bones: &[i64], world: &World, renderer: [f32; 16]) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let mut positions = mesh.positions.clone();
    for (i, shape) in mesh.blend_shapes.iter().enumerate() {
        let w = weights.get(i).copied().unwrap_or(shape.default_weight);
        if w.abs() < 1e-4 || shape.frames.is_empty() {
            continue;
        }
        // Between the frames around the weight (a single frame at 100: linear from nothing).
        let mut lower: Option<&(f32, Vec<(u32, [f32; 3])>)> = None;
        let mut upper = &shape.frames[0];
        for f in &shape.frames {
            if f.0 <= w {
                lower = Some(f);
            }
            if f.0 >= w {
                upper = f;
                break;
            }
            upper = f;
        }
        let (lw, uw) = (lower.map_or(0.0, |l| l.0), upper.0);
        let k = if (uw - lw).abs() < 1e-6 {
            if uw.abs() > 1e-6 {
                w / uw
            } else {
                1.0
            }
        } else {
            (w - lw) / (uw - lw)
        };
        if let Some(l) = lower.filter(|l| !std::ptr::eq(*l, upper)) {
            for (v, d) in &l.1 {
                if let Some(p) = positions.get_mut(*v as usize) {
                    for c in 0..3 {
                        p[c] += d[c] * (1.0 - k);
                    }
                }
            }
        }
        for (v, d) in &upper.1 {
            if let Some(p) = positions.get_mut(*v as usize) {
                for c in 0..3 {
                    p[c] += d[c] * k;
                }
            }
        }
    }
    let Some(skin) = &mesh.skin else { return (positions, mesh.normals.clone()) };
    // Each bone's skinning matrix: its pose times its bind pose; a bone the renderer does not have stays where the
    // mesh is (the renderer's matrix).
    let skinning: Vec<[f32; 16]> = (0..skin.bind_poses.len())
        .map(|b| match bones.get(b).copied().filter(|t| *t != 0 && world.matrices.contains_key(t)) {
            Some(t) => math::mul(&world.matrix(t), &skin.bind_poses[b]),
            None => renderer,
        })
        .collect();
    let mut out_p = Vec::with_capacity(positions.len());
    let mut out_n = Vec::with_capacity(positions.len());
    for (i, p) in positions.iter().enumerate() {
        let w = skin.weights.get(i).copied().unwrap_or([(0, 0.0); 4]);
        let total: f32 = w.iter().map(|x| x.1).sum();
        if total < 1e-6 {
            out_p.push(math::point(&renderer, *p));
            out_n.push(math::normalize(math::vector(&renderer, mesh.normals[i])));
            continue;
        }
        let mut acc = [0.0f32; 3];
        let mut nacc = [0.0f32; 3];
        for (b, wt) in w {
            if wt <= 0.0 {
                continue;
            }
            let m = skinning.get(b as usize).unwrap_or(&renderer);
            let q = math::point(m, *p);
            let n = math::vector(m, mesh.normals[i]);
            for c in 0..3 {
                acc[c] += q[c] * wt;
                nacc[c] += n[c] * wt;
            }
        }
        out_p.push(acc.map(|c| c / total));
        out_n.push(math::normalize(nacc));
    }
    (out_p, out_n)
}

fn push_mesh(writer: &mut Writer, positions: &[[f32; 3]], normals: &[[f32; 3]], uvs: &[[f32; 2]], submeshes: &[Vec<u32>]) -> (Mesh, Bounds) {
    let mut data = Vec::with_capacity(positions.len() * file::VERTEX_SIZE);
    for (i, p) in positions.iter().enumerate() {
        data.extend_from_slice(&file::vertex(*p, normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]), uvs.get(i).copied().unwrap_or([0.0; 2])));
    }
    let wide = positions.len() > 65535;
    let mut indices = Vec::new();
    let mut subs = Vec::new();
    let mut start = 0u32;
    for s in submeshes {
        for &i in s {
            if wide {
                indices.extend_from_slice(&i.to_le_bytes());
            } else {
                indices.extend_from_slice(&(i as u16).to_le_bytes());
            }
        }
        subs.push(Submesh { start, count: s.len() as u32 });
        start += s.len() as u32;
    }
    let b = bounds_of(positions);
    let mesh = Mesh {
        vertices: positions.len() as u32,
        data: writer.push(&data),
        index_format: if wide { IndexFormat::U32 } else { IndexFormat::U16 },
        indices: writer.push(&indices),
        submeshes: subs,
        bounds: b,
    };
    (mesh, b)
}

pub fn bounds_of(points: &[[f32; 3]]) -> Bounds {
    let mut b = Bounds { min: [f32::MAX; 3], max: [f32::MIN; 3] };
    for p in points {
        for k in 0..3 {
            b.min[k] = b.min[k].min(p[k]);
            b.max[k] = b.max[k].max(p[k]);
        }
    }
    if points.is_empty() {
        b = Bounds::default();
    }
    b
}

fn transform_bounds(m: &[f32; 16], b: &Bounds) -> Bounds {
    let mut out = Bounds { min: [f32::MAX; 3], max: [f32::MIN; 3] };
    for i in 0..8 {
        let p = [if i & 1 == 0 { b.min[0] } else { b.max[0] }, if i & 2 == 0 { b.min[1] } else { b.max[1] }, if i & 4 == 0 { b.min[2] } else { b.max[2] }];
        let q = math::point(m, p);
        for k in 0..3 {
            out.min[k] = out.min[k].min(q[k]);
            out.max[k] = out.max[k].max(q[k]);
        }
    }
    out
}

fn grow(into: &mut Bounds, b: &Bounds) {
    for k in 0..3 {
        into.min[k] = into.min[k].min(b.min[k]);
        into.max[k] = into.max[k].max(b.max[k]);
    }
}

/// The scene's directional lights and ambient light, as shaders receive them (linear).
fn lighting_of(graph: &Graph, world: &World) -> Option<file::Lighting> {
    let mut lights: Vec<file::SceneLight> = Vec::new();
    for o in graph.objects.values() {
        let Obj::Light(l) = o else { continue };
        if l.kind != 1 || !l.enabled {
            continue;
        }
        let Some(&t) = world.transform_of.get(&l.game_object) else { continue };
        if !world.shown(t) {
            continue;
        }
        let m = world.matrix(t);
        let direction = math::normalize([m[8], m[9], m[10]]);
        let c = [0, 1, 2].map(|k| material::srgb_to_linear(l.color[k]) * l.intensity);
        lights.push(file::SceneLight { direction, color: c, intensity: l.intensity, shadows: l.shadows, shadow_strength: l.shadow_strength });
    }
    lights.sort_by(|a, b| b.intensity.total_cmp(&a.intensity));
    lights.truncate(4);
    let ambient = graph.ambient.as_ref().and_then(|a| {
        let lin = |c: [f32; 4]| [0, 1, 2].map(|k| material::srgb_to_linear(c[k]) * a.intensity);
        match a.mode {
            1 => Some(file::Ambient { mode: file::AmbientMode::Trilight, sky: lin(a.sky), equator: lin(a.equator), ground: lin(a.ground) }),
            3 => Some(file::Ambient { mode: file::AmbientMode::Flat, sky: lin(a.sky), equator: lin(a.sky), ground: lin(a.sky) }),
            // The sky's light is only known once Unity computed it: the desktop uses the default sky's.
            _ => None,
        }
    });
    if lights.is_empty() && ambient.is_none() {
        // Nothing recorded worth drawing in (no directional light, the default sky): the studio's light.
        return None;
    }
    Some(file::Lighting { lights, ambient, reflection_intensity: 1.0 })
}

fn decode_all(jobs: &[(String, bool, Option<PathBuf>, bool)], side: u32, limits: &Limits) -> Vec<anyhow::Result<texture::Encoded>> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, limits.threads.max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<Decoded> = jobs.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..threads.min(jobs.len().max(1)) {
            scope.spawn(|| {
                limits.lower_priority();
                decode_next(jobs, &next, &results, side, limits)
            });
        }
    });
    results.into_iter().map(|r| r.into_inner().unwrap().unwrap_or_else(|| Err(anyhow::anyhow!("not read")))).collect()
}

type Decoded = std::sync::Mutex<Option<anyhow::Result<texture::Encoded>>>;

/// Decodes pictures from `jobs` until none is left (one of [`decode_all`]'s threads).
fn decode_next(jobs: &[(String, bool, Option<PathBuf>, bool)], next: &std::sync::atomic::AtomicUsize, results: &[Decoded], side: u32, limits: &Limits) {
    loop {
        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let Some((_, main, path, keep_alpha)) = jobs.get(i) else { break };
        let r = (|| {
            limits.check("reading the pictures")?;
            let path = path.as_ref().context("not in the project")?;
            let img = texture::decode(path, limits.max_texture_bytes)?;
            // An emission mask needs less detail than the colour.
            texture::encode(&img, if *main { side } else { side.min(256) }, *keep_alpha)
        })();
        *results[i].lock().unwrap() = Some(r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::prefab::tests::project;

    #[test]
    fn a_scene_builds_into_a_preview_the_desktop_reads() {
        let dir = tempfile::tempdir().unwrap();
        let assets = project(dir.path());
        let scene = SceneFile { path: dir.path().join("Assets/Scene.unity"), guid: "f".repeat(32), name: "Scene".into() };
        let built = build(dir.path(), &scene, &assets, &Limits::default()).unwrap();
        let p = file::parse(built.bytes.clone()).expect("a valid preview");
        let h = &p.header;
        assert_eq!(h.kind, Kind::Avatars);
        assert_eq!(h.avatars.len(), 1);
        assert_eq!(h.avatars[0].name, "Avatar");
        // The variant moved the avatar 3 m forward.
        assert!((h.avatars[0].position[2] - 3.0).abs() < 1e-4, "{:?}", h.avatars[0].position);
        // The body (skinned to the bone the variant moved up to 2 m: the quad rises 1 m), the cube; the prop is
        // hidden and the EditorOnly object has nothing to draw.
        assert_eq!(h.draws.len(), 2, "{:?}", built.report);
        let body = h.draws.iter().find(|d| d.avatar == Some(0)).unwrap();
        let mesh = &h.meshes[body.mesh as usize];
        assert!((mesh.bounds.min[1] - 1.0).abs() < 1e-3 && (mesh.bounds.max[1] - 2.0).abs() < 1e-3, "{:?}", mesh.bounds);
        assert!((mesh.bounds.min[2] - 3.0).abs() < 1e-3, "posed in the world: {:?}", mesh.bounds);
        // Its material is the prefab's override (red), the cube's too: stored once.
        assert_eq!(h.materials.len(), 1);
        assert_eq!(h.materials[0].color, [1.0, 0.0, 0.0, 1.0]);
        let cube = h.draws.iter().find(|d| d.avatar.is_none()).unwrap();
        assert_eq!(p.matrix(cube.instance)[12], 4.0);
        // Focus on the avatar, the ground under it.
        assert_eq!(h.ground, Some(0.0));
        assert!(h.focus.max[0] < 1.0);
        assert!(built.files.iter().any(|f| f.ends_with("Rig.fbx")));
        assert!(built.files.iter().any(|f| f.ends_with("Avatar Variant.prefab")));
        assert!(built.report.missing.is_empty(), "{:?}", built.report.missing);
    }
}
