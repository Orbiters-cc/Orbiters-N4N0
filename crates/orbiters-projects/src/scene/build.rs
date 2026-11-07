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
use super::{math, primitives, terrain, texture, world as place, Limits};
use crate::preview::{self as file, Bounds, Draw, Header, ImageFormat, IndexFormat, Instances, Kind, Mesh, Submesh, Texture, Writer};
use anyhow::Context as _;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

// Which of a scene's renderers are drawn and why the others are not is counted (`Report::dropped`), so that "99 % of the
// world is missing" has an answer per reason rather than a guess: Avatorbit's world was one `.blend` model, 307 of its
// renderers and all of its ground, water, rocks and buildings, its sun among them.

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
    /// What was left out and why, by reason: every renderer the scene holds is either drawn or counted here.
    #[serde(default)]
    pub dropped: std::collections::BTreeMap<String, u32>,
    pub ms: f64,
}

const BUDGET: u64 = 2_000_000;

struct Candidate {
    renderer: i64,
    /// Where it stands (its transform's world matrix, or a terrain tree's place).
    matrix: [f32; 16],
    mesh: Arc<ModelMesh>,
    key: String,
    skinned: bool,
    materials: Vec<MatSource>,
    bones: Vec<i64>,
    blend: Vec<f32>,
    avatar: Option<u32>,
    world_bounds: Bounds,
    triangles: u64,
    shadows: bool,
    /// The lightmap it samples (its picture's GUID) and where in it.
    lightmap: Option<(String, [f32; 4])>,
}

pub fn build(project: &Path, scene: &SceneFile, assets: &Assets, limits: &Limits) -> anyhow::Result<Built> {
    let start = Instant::now();
    let mut loader = Loader::new(assets, limits);
    loader.scene = Some(scene.path.clone());
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
    // The levels of LOD groups below the most detailed: they stand in for it from far away, and would draw over it.
    let mut lower_lods = std::collections::HashSet::new();
    for obj in graph.objects.values() {
        if let Obj::Lod(l) = obj {
            lower_lods.extend(l.lower.iter().copied());
        }
    }
    let unity_lightmaps = place::lighting_data_maps(&mut loader, &graph);
    let mut dropped: std::collections::BTreeMap<String, u32> = Default::default();
    let mut drop = |why: &str| *dropped.entry(why.to_string()).or_insert(0) += 1;
    let mut meshes: HashMap<String, Option<Arc<ModelMesh>>> = HashMap::new();
    let mut candidates = Vec::new();
    let mut ids: Vec<&i64> = graph.objects.keys().collect();
    ids.sort();
    for id in ids {
        let Obj::Renderer(r) = &graph.objects[id] else { continue };
        report.renderers += 1;
        let Some(&t) = world.transform_of.get(&r.game_object) else {
            drop("no transform"); // i18n: not shown (a counter key the report reads, not text)
            continue;
        };
        if !r.enabled {
            drop("renderer off"); // i18n: not shown (a counter key the report reads, not text)
            continue;
        }
        if !world.shown(t) {
            drop("inactive or EditorOnly"); // i18n: not shown (a counter key the report reads, not text)
            continue;
        }
        if lower_lods.contains(id) {
            drop("lower LOD"); // i18n: not shown (a counter key the report reads, not text)
            continue;
        }
        let source = if r.skinned { &r.mesh } else { filters.get(&r.game_object).copied().unwrap_or(&MeshSource::None) };
        let key = match source {
            MeshSource::None => {
                drop("no mesh set (UI text, or made at run time)"); // i18n: not shown (a counter key the report reads, not text)
                continue;
            }
            MeshSource::Model { guid, mesh } => format!("{guid}:{mesh}"),
            MeshSource::Asset(r) => format!("{}:{}", r.guid.as_deref().unwrap_or("-"), r.file_id),
        };
        if !meshes.contains_key(&key) {
            limits.check("reading the meshes")?; // i18n: not shown (what a time-limit error says it was doing)
            let m = match resolve_mesh(&mut loader, source) {
                Ok(m) => Some(m),
                Err(e) => {
                    limits.check("reading the meshes")?; // i18n: not shown (what a time-limit error says it was doing)
                    let what = format!("{e:#}");
                    if !loader.missing.contains(&what) && loader.missing.len() < 64 {
                        loader.missing.push(what);
                    }
                    None
                }
            };
            meshes.insert(key.clone(), m);
        }
        let Some(mesh) = meshes[&key].clone() else {
            drop("mesh could not be read"); // i18n: not shown (a counter key the report reads, not text)
            continue;
        };
        if mesh.positions.is_empty() || mesh.triangles() == 0 {
            drop("empty mesh"); // i18n: not shown (a counter key the report reads, not text)
            continue;
        }
        let avatar = avatars.iter().position(|(at, _)| world.under(t, *at)).map(|i| i as u32);
        let m = world.matrix(t);
        let local = bounds_of(&mesh.positions);
        let world_bounds = transform_bounds(&m, &local);
        // A skinned mesh moves: Unity lights it by probes, not a lightmap.
        let lightmap = if r.skinned { None } else { place::lightmap_of(&graph, &unity_lightmaps, *id, r.lightmap_index, r.lightmap_st) };
        candidates.push(Candidate {
            renderer: *id,
            matrix: m,
            shadows: r.casts_shadows,
            lightmap,
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
    // Terrains: a mesh and a blended picture each, their trees at their places.
    let mut baked_looks: Vec<(Look, image::RgbaImage)> = Vec::new();
    add_terrains(&graph, &world, &mut loader, &unity_lightmaps, &mut candidates, &mut baked_looks, &mut dropped)?;
    report.renderers += graph.objects.values().filter(|o| matches!(o, Obj::Terrain(_))).count();
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
    if skipped_budget > 0 {
        dropped.insert("budget".to_string(), skipped_budget);
    }

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
                MatSource::Baked(k) => format!("b:{k}"),
            };
            if !looks.contains_key(&key) {
                limits.check("reading the materials")?; // i18n: not shown (what a time-limit error says it was doing)
                let resolved = match src {
                    MatSource::Baked(k) => Ok(baked_looks[*k].0.clone()),
                    src => resolve_material(&mut loader, src, &mut shader_names),
                };
                let look = resolved.unwrap_or_else(|e| {
                    let what = format!("{e:#}");
                    if !loader.missing.contains(&what) && loader.missing.len() < 64 {
                        loader.missing.push(what);
                    }
                    Look { material: file::Material { name: "Missing".into(), color: [0.75, 0.75, 0.75, 1.0], ..Default::default() }, ..Default::default() } // i18n: not shown (a stored name or detail of the scene file)
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
    let mut wanted: Vec<(String, Use)> = Vec::new();
    for l in &look_list {
        let normal = l.normal_texture.as_ref().filter(|_| kind != Kind::Avatars);
        for (t, how) in [(l.main_texture.as_ref(), Use::Main), (l.emission_texture.as_ref(), Use::Emission), (normal, Use::Normal)] {
            if let Some(g) = t {
                if !wanted.iter().any(|(w, m)| w == g && *m == how) {
                    wanted.push((g.clone(), how));
                }
            }
        }
    }
    // Toolkit's sizes for a scene with avatars, by how many pictures it has. A world has many, most on small props, and
    // is seen from far off: those that cover the most of it keep their detail, the sand before a fruit's skin.
    let mut coverage: HashMap<&str, f32> = HashMap::new();
    if kind != Kind::Avatars {
        for (c, slots) in candidates.iter().zip(&draws_materials) {
            let s = c.world_bounds.size();
            let area = (s[0] * s[2]).max(s[0] * s[1]).max(s[1] * s[2]).max(0.0);
            for &m in slots {
                if let Some(g) = usize::try_from(m).ok().and_then(|m| look_list.get(m)).and_then(|l| l.main_texture.as_deref()) {
                    *coverage.entry(g).or_insert(0.0) += area;
                }
            }
        }
    }
    let mut largest: Vec<(&String, f32)> = wanted.iter().filter(|w| w.1 == Use::Main).map(|w| (&w.0, coverage.get(w.0.as_str()).copied().unwrap_or(0.0))).collect();
    largest.sort_by(|a, b| b.1.total_cmp(&a.1));
    let rank: HashMap<&String, usize> = largest.iter().enumerate().map(|(i, (g, _))| (*g, i)).collect();
    let count_side = match wanted.len() {
        0..=8 => 1024,
        9..=32 => 512,
        _ => 256,
    };
    let side_of = |g: &String| -> u32 {
        match rank.get(g) {
            Some(r) if kind != Kind::Avatars => match *r {
                0..=7 => 1024,
                8..=31 => 512,
                _ => 256,
            },
            _ => count_side,
        }
    };
    let jobs: Vec<(String, Use, Option<PathBuf>, bool, u32)> = wanted
        .iter()
        .filter(|(g, _)| !g.starts_with("baked:"))
        .map(|(g, how)| {
            let keep_alpha = *how == Use::Main && look_list.iter().any(|l| l.main_texture.as_deref() == Some(g) && l.material.alpha != file::Alpha::Opaque);
            (g.clone(), *how, assets.path(g), keep_alpha, side_of(g))
        })
        .collect();
    for (_, _, p, _, _) in &jobs {
        if let Some(p) = p {
            loader.files.push(p.clone());
            loader.files.push(prefab::meta_path(p));
        }
    }
    limits.check("reading the pictures")?; // i18n: not shown (what a time-limit error says it was doing)
    let decoded = decode_all(&jobs, limits);
    let mut writer = Writer::default();
    let mut textures: Vec<Texture> = Vec::new();
    let mut texture_index: HashMap<(String, Use), i32> = HashMap::new();
    for (k, (_, image)) in baked_looks.iter().enumerate() {
        let e = texture::encode(image, 2048, false)?;
        let data = writer.push(&e.bytes);
        texture_index.insert((format!("baked:{k}"), Use::Main), textures.len() as i32);
        textures.push(Texture { width: e.width, height: e.height, format: ImageFormat::Jpeg, linear: false, data });
    }
    for ((g, how, path, _, _), result) in jobs.iter().zip(decoded) {
        match result {
            Ok(e) => {
                let data = writer.push(&e.bytes);
                texture_index.insert((g.clone(), *how), textures.len() as i32);
                textures.push(Texture {
                    width: e.width,
                    height: e.height,
                    format: if e.png { ImageFormat::Png } else { ImageFormat::Jpeg },
                    linear: *how == Use::Normal,
                    data,
                });
            }
            Err(e) => {
                let name = path.as_ref().map(|p| prefab::rel(assets, p)).unwrap_or_else(|| format!("a texture ({g})")); // i18n: not shown (a stored name or detail of the scene file)
                let what = format!("{name}: {e:#}");
                if !loader.missing.contains(&what) && loader.missing.len() < 64 {
                    loader.missing.push(what);
                }
            }
        }
    }
    for (i, l) in look_list.iter().enumerate() {
        materials[i].texture = l.main_texture.as_ref().and_then(|g| texture_index.get(&(g.clone(), Use::Main))).copied().unwrap_or(-1);
        materials[i].emission_texture = l.emission_texture.as_ref().and_then(|g| texture_index.get(&(g.clone(), Use::Emission))).copied().unwrap_or(-1);
        materials[i].normal_texture = l.normal_texture.as_ref().and_then(|g| texture_index.get(&(g.clone(), Use::Normal))).copied().unwrap_or(-1);
        if l.emission_texture.is_some() && materials[i].emission_texture < 0 {
            // A glow masked by a picture that could not be read would light the whole surface: none.
            materials[i].emission = [0.0; 3];
        }
    }
    report.textures = textures.len();

    // Lightmaps: read on several threads, the same one once.
    let mut lightmap_guids: Vec<String> = Vec::new();
    for c in &candidates {
        if let Some((g, _)) = &c.lightmap {
            if !lightmap_guids.contains(g) && lightmap_guids.len() < place::MAX_LIGHTMAPS {
                lightmap_guids.push(g.clone());
            }
        }
    }
    let lightmap_side = place::lightmap_side(lightmap_guids.len());
    let lightmap_paths: Vec<Option<PathBuf>> = lightmap_guids.iter().map(|g| assets.path(g)).collect();
    for p in lightmap_paths.iter().flatten() {
        loader.files.push(p.clone());
    }
    // Two at a time: a 4096² lightmap takes 200 MB while it is decoded, a build in the background should not hold more.
    let lightmap_limits = Limits { threads: limits.threads.min(2), ..limits.clone() };
    let read_lightmaps = in_parallel(&lightmap_paths, &lightmap_limits, |p| {
        let p = p.as_ref().context("not in the project")?;
        place::lightmap(p, lightmap_side, limits)
    });
    let mut lightmaps: Vec<file::Lightmap> = Vec::new();
    let mut lightmap_slot: HashMap<&str, u32> = HashMap::new();
    for ((g, p), result) in lightmap_guids.iter().zip(&lightmap_paths).zip(read_lightmaps) {
        match result {
            Ok((e, scale)) => {
                let data = writer.push(&e.bytes);
                lightmap_slot.insert(g.as_str(), lightmaps.len() as u32);
                lightmaps.push(file::Lightmap { texture: textures.len() as u32, scale });
                textures.push(Texture { width: e.width, height: e.height, format: ImageFormat::Jpeg, linear: false, data });
            }
            Err(e) => {
                let name = p.as_ref().map(|p| prefab::rel(assets, p)).unwrap_or_else(|| format!("a lightmap ({g})")); // i18n: not shown (a stored name or detail of the scene file)
                let what = format!("{name}: {e:#}");
                if !loader.missing.contains(&what) && loader.missing.len() < 64 {
                    loader.missing.push(what);
                }
            }
        }
    }

    // The sky.
    limits.check("reading the sky")?; // i18n: not shown (what a time-limit error says it was doing)
    let built_sky = place::sky(&mut loader, &graph);
    let mut sky = None;
    let mut sky_ambient = None;
    if let Some(b) = built_sky {
        loader.files.extend(b.files);
        let mut s = b.sky;
        match b.picture {
            Some(e) => {
                let data = writer.push(&e.bytes);
                s.texture = textures.len() as i32;
                textures.push(Texture { width: e.width, height: e.height, format: ImageFormat::Jpeg, linear: false, data });
            }
            None => s.texture = -1,
        }
        sky = Some(s);
        sky_ambient = b.ambient;
    }

    // Meshes: skinned ones posed into the world, the others once each with their matrices.
    let mut out_meshes: Vec<Mesh> = Vec::new();
    let mut mesh_slot: HashMap<String, u32> = HashMap::new();
    let mut matrices: Vec<[f32; 16]> = Vec::new();
    let mut matrix_slot: HashMap<[u32; 16], u32> = HashMap::new();
    let mut points: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let scene_has_lightmaps = !lightmaps.is_empty();
    let mut draws: Vec<Draw> = Vec::new();
    let mut bounds = Bounds { min: [f32::MAX; 3], max: [f32::MIN; 3] };
    let mut focus = Bounds { min: [f32::MAX; 3], max: [f32::MIN; 3] };
    let mut vertices_total = 0u64;
    let identity = math::IDENTITY;
    for (c, slots) in candidates.iter().zip(&draws_materials) {
        limits.check("posing the meshes")?; // i18n: not shown (what a time-limit error says it was doing)
        let m = c.matrix;
        let posed = c.skinned && (c.mesh.skin.is_some() || c.blend.iter().any(|w| *w != 0.0));
        let (mesh_index, matrix, world_b) = if posed {
            let (positions, normals) = pose(&c.mesh, &c.blend, &c.bones, &world, m);
            let (mesh, b) = push_mesh(&mut writer, &positions, &normals, &c.mesh.uvs, &[], &c.mesh.submeshes);
            out_meshes.push(mesh);
            vertices_total += positions.len() as u64;
            let wb = if c.mesh.skin.is_some() { b } else { transform_bounds(&m, &b) };
            (out_meshes.len() as u32 - 1, if c.mesh.skin.is_some() { identity } else { m }, wb)
        } else {
            let index = match mesh_slot.get(&c.key) {
                Some(&i) => i,
                None => {
                    // The second UV set when something samples a lightmap through it (Unity falls back on the first).
                    let lit = scene_has_lightmaps && candidates.iter().any(|o| o.key == c.key && o.lightmap.is_some());
                    let second: &[[f32; 2]] = match (lit, c.mesh.uv2.is_empty()) {
                        (false, _) => &[],
                        (true, false) => &c.mesh.uv2,
                        (true, true) => &c.mesh.uvs,
                    };
                    let (mesh, _) = push_mesh(&mut writer, &c.mesh.positions, &c.mesh.normals, &c.mesh.uvs, second, &c.mesh.submeshes);
                    out_meshes.push(mesh);
                    vertices_total += c.mesh.positions.len() as u64;
                    let i = out_meshes.len() as u32 - 1;
                    mesh_slot.insert(c.key.clone(), i);
                    i
                }
            };
            (index, m, c.world_bounds)
        };
        let instance = *matrix_slot.entry(matrix.map(f32::to_bits)).or_insert_with(|| {
            matrices.push(matrix);
            matrices.len() as u32 - 1
        });
        grow(&mut bounds, &world_b);
        if c.avatar.is_some() {
            grow(&mut focus, &world_b);
        }
        if kind != Kind::Avatars {
            // A few of each object's points, wherever they stand (a skinned one's are in the world already).
            let step = (c.mesh.positions.len() / 32).max(1);
            for p in c.mesh.positions.iter().step_by(step) {
                let q = if posed && c.mesh.skin.is_some() { p_center(&world_b) } else { math::point(&matrix, *p) };
                for k in 0..3 {
                    points[k].push(q[k]);
                }
            }
        }
        let lightmap = c.lightmap.as_ref().and_then(|(g, st)| lightmap_slot.get(g.as_str()).map(|&index| file::DrawLightmap { index, st: *st }));
        draws.push(Draw { instance, mesh: mesh_index, materials: slots.clone(), shadows: c.shadows, avatar: c.avatar, lightmap });
    }
    report.drawn = draws.len();
    report.meshes = out_meshes.len();
    report.models = loader.models.len();
    report.triangles = used;
    if bounds.min[0] > bounds.max[0] {
        bounds = Bounds { min: [-0.5, 0.0, -0.5], max: [0.5, 1.0, 0.5] };
    }
    if focus.min[0] > focus.max[0] {
        focus = place::framing(&mut points, bounds);
    }
    let matrices_blob = writer.push(&matrices.iter().flatten().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>());
    let ground = (kind == Kind::Avatars).then(|| avatars.iter().map(|a| a.1.position[1]).fold(f32::MAX, f32::min));
    let lighting = lighting_of(&graph, &world, sky_ambient, scene_has_lightmaps);
    // Where the Scene view looked, for the scene it had open, when that lies in the scene.
    let camera = if kind == Kind::Avatars || !place::is_last_open(project, &scene.path, assets) {
        None
    } else {
        place::scene_view_camera(project)
            .filter(|c| {
                (0..3).all(|k| {
                    let margin = (bounds.max[k] - bounds.min[k]).max(1.0) * 0.25;
                    c.pivot[k] >= bounds.min[k] - margin && c.pivot[k] <= bounds.max[k] + margin
                })
            })
            .map(|c| place::far_enough(c, focus.radius()))
    };
    let mut skipped = std::collections::BTreeMap::new();
    if skipped_budget > 0 {
        skipped.insert("budget".to_string(), skipped_budget);
    }
    for (why, key) in [("terrain trees thinned", "trees"), ("terrain details", "details")] { // i18n: not shown (a counter key the report reads, not text)
        if let Some(&n) = dropped.get(why) {
            skipped.insert(key.to_string(), n);
        }
    }
    let missing_meshes = meshes.values().filter(|m| m.is_none()).count() as u32;
    if missing_meshes > 0 {
        skipped.insert("missing".to_string(), missing_meshes);
    }
    report.prefab_instances = loader.prefab_instances;
    report.missing = loader.missing.clone();
    report.dropped = dropped;
    report.ms = start.elapsed().as_secs_f64() * 1000.0;
    let header = Header {
        written_utc: now_utc(),
        generator: format!("Orbiters - N4N0 {} (from the project's files)", env!("CARGO_PKG_VERSION")), // i18n: not shown (a stored name or detail of the scene file)
        unity: unity_version(project),
        scene: file::Scene { name: scene.name.clone(), path: prefab::rel(assets, &scene.path), guid: scene.guid.clone() },
        kind,
        bounds,
        focus,
        ground,
        camera,
        avatars: avatars.into_iter().map(|a| a.1).collect(),
        textures,
        materials,
        meshes: out_meshes,
        instances: Instances { count: matrices.len() as u32, matrices: matrices_blob },
        draws,
        lighting,
        lightmaps,
        sky,
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

    /// The name of a transform's GameObject.
    pub fn name_of(&self, t: i64) -> Option<&'g str> {
        let Some(Obj::Transform(tr)) = self.graph.objects.get(&t) else { return None };
        match self.graph.objects.get(&tr.game_object) {
            Some(Obj::GameObject(g)) => Some(g.name.as_str()),
            _ => None,
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
            // A mesh saved in the scene itself (a tool's mesh, a ProBuilder shape).
            let Some(guid) = r.guid.as_deref() else {
                let path = loader.scene.clone().context("a mesh in the scene itself")?;
                return super::mesh_asset::read(&path, r.file_id, loader.limits).map(Arc::new).with_context(|| prefab::rel(loader.assets, &path));
            };
            if guid == "0000000000000000e000000000000000" {
                return primitives::builtin(r.file_id).map(Arc::new).with_context(|| format!("Unity's built-in mesh {}", r.file_id));
            }
            let path = loader.assets.path(guid).with_context(|| format!("a mesh that is not in the project ({guid})"))?;
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if ext == "fbx" || ext == "obj" || prefab::only_unity_reads(&ext) {
                // What Unity's import made, by Unity's ID of it; an FBX falls back to reading the file.
                if let Some(a) = loader.artifact(guid, &path, prefab::only_unity_reads(&ext)) {
                    loader.files.push(path.clone());
                    match a.mesh(r.file_id) {
                        Ok(m) => return Ok(Arc::new(m)),
                        Err(e) if prefab::only_unity_reads(&ext) => return Err(e).with_context(|| format!("a mesh of {}", prefab::rel(loader.assets, &path))),
                        Err(_) => {}
                    }
                }
            }
            match ext.as_str() {
                "prefab" | "unity" => {
                    loader.files.push(path.clone());
                    super::mesh_asset::read(&path, r.file_id, loader.limits).map(Arc::new).with_context(|| prefab::rel(loader.assets, &path))
                }
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
                other if prefab::only_unity_reads(other) => {
                    anyhow::bail!("{} is a .{other} model only Unity's import reads: open the project in Unity once", prefab::rel(loader.assets, &path))
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
        uv2: m.uv2.clone(),
        submeshes: m.submeshes.clone(),
        skin: m.skin.as_ref().map(|s| Skin { bones: s.bones.clone(), names: s.names.clone(), bind_poses: s.bind_poses.clone(), weights: s.weights.clone() }),
        blend_shapes: m
            .blend_shapes
            .iter()
            .map(|b| super::model::BlendShape { name: b.name.clone(), frames: b.frames.clone(), default_weight: b.default_weight })
            .collect(),
    }
}

/// A material slot's name and base picture, as a preview would resolve them (diagnostics: `scene_preview --materials`).
pub fn material_of_slot(loader: &mut Loader, src: &MatSource, shader_names: &mut HashMap<String, String>) -> String {
    match resolve_material(loader, src, shader_names) {
        Ok(look) => format!("{} [{}]", look.material.name, look.main_texture.unwrap_or_default()),
        Err(e) => format!("(failed: {e:#})"),
    }
}

/// A material slot's look.
fn resolve_material(loader: &mut Loader, src: &MatSource, shader_names: &mut HashMap<String, String>) -> anyhow::Result<Look> {
    match src {
        MatSource::None => {
            Ok(Look { material: file::Material { name: "None".into(), color: [0.75, 0.75, 0.75, 1.0], ..Default::default() }, ..Default::default() }) // i18n: not shown (a stored name or detail of the scene file)
        }
        MatSource::Asset(r) => {
            let guid = r.guid.as_deref().context("a material in the scene itself")?;
            if guid == "0000000000000000f000000000000000" {
                // Unity's Default-Material and Default-Diffuse.
                return Ok(Look { material: file::Material { name: "Default-Material".into(), ..Default::default() }, ..Default::default() });
            }
            let path = loader.assets.path(guid).with_context(|| format!("a material that is not in the project ({guid})"))?;
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if ext == "fbx" || ext == "obj" || prefab::only_unity_reads(&ext) {
                // A model's own material as its import made it, its properties read as a `.mat` file's.
                if let Some(a) = loader.artifact(guid, &path, prefab::only_unity_reads(&ext)) {
                    if let Ok(v) = a.object(r.file_id, 21) {
                        let (name, shader, props) = material::props_of(&v);
                        let shader_name = shader_name_of(loader, shader, shader_names);
                        return Ok(material::look(&name, &shader_name, &props));
                    }
                }
                if prefab::only_unity_reads(&ext) {
                    anyhow::bail!("a material of {}, only in its import", prefab::rel(loader.assets, &path));
                }
                // A model's own material, by Unity's ID of it: its name's hash, or a name an older import numbered.
                let m = loader.model(guid)?;
                let named = loader.model_settings(guid).material_ids.get(&r.file_id).cloned();
                let k = m.materials.iter().position(|mm| super::model::file_id("Material", &mm.name, 0) == r.file_id || named.as_deref() == Some(mm.name.as_str())).unwrap_or(0); // i18n: not shown (a stored name or detail of the scene file)
                return model_slot(loader, guid, k, shader_names);
            }
            mat_file(loader, &path, shader_names)
        }
        MatSource::Model { guid, material } => model_slot(loader, guid, *material, shader_names),
        MatSource::Baked(_) => anyhow::bail!("a look the build keeps itself"),
    }
}

fn mat_file(loader: &mut Loader, path: &Path, shader_names: &mut HashMap<String, String>) -> anyhow::Result<Look> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", prefab::rel(loader.assets, path)))?;
    loader.files.push(path.to_path_buf());
    let (name, shader, props) =
        material::props(&text).with_context(|| format!("{} is not a material Unity saved as text", prefab::rel(loader.assets, path)))?;
    let shader_name = shader_name_of(loader, shader, shader_names);
    Ok(material::look(&name, &shader_name, &props))
}

/// A material's shader's name, lower-case: the built-in ones by their ID, a `.shader` file by its first line, a Shader
/// Graph or an Amplify file by its file's name.
fn shader_name_of(loader: &mut Loader, shader: Option<super::yaml::Ref>, shader_names: &mut HashMap<String, String>) -> String {
    match shader {
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
    }
}

/// What Unity draws in a model's material slot: nothing of the file's when its import makes no materials, the material
/// its import remaps there, the legacy external material found by name, or the model's own (its colour and its texture
/// found by name in the project).
fn model_slot(loader: &mut Loader, guid: &str, k: usize, shader_names: &mut HashMap<String, String>) -> anyhow::Result<Look> {
    let model = loader.model(guid)?;
    let settings = loader.model_settings(guid);
    if !settings.import_materials {
        return Ok(Look { material: file::Material { name: "Default-Material".into(), ..Default::default() }, ..Default::default() });
    }
    let mm = model.materials.get(k).cloned().unwrap_or_default();
    if let Some(remap) = settings.remapped.get(&mm.name) {
        // A remap to a material no longer in the project leaves the slot empty in Unity's import, not the file's own.
        return match loader.assets.path(remap) {
            Some(p) => mat_file(loader, &p, shader_names),
            None => resolve_material(loader, &MatSource::None, shader_names),
        };
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
    // A model's bones are found among the renderer's by name (its list may drop and reorder them); a mesh asset's by
    // index. A name two of the renderer's bones share falls back to the index.
    let mut by_name: HashMap<&str, Option<i64>> = HashMap::new();
    if !skin.names.is_empty() {
        for &t in bones {
            if let Some(n) = world.name_of(t) {
                by_name.entry(n).and_modify(|e| *e = None).or_insert(Some(t));
            }
        }
    }
    let skinning: Vec<[f32; 16]> = (0..skin.bind_poses.len())
        .map(|b| {
            let named = skin.names.get(b).and_then(|n| by_name.get(n.as_str()).copied().flatten());
            let t = if skin.names.is_empty() { bones.get(b).copied() } else { named.or_else(|| bones.get(b).copied().filter(|_| by_name.is_empty())) };
            match t.filter(|t| *t != 0 && world.matrices.contains_key(t)) {
                Some(t) => math::mul(&world.matrix(t), &skin.bind_poses[b]),
                None => renderer,
            }
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

fn push_mesh(writer: &mut Writer, positions: &[[f32; 3]], normals: &[[f32; 3]], uvs: &[[f32; 2]], lightmap_uvs: &[[f32; 2]], submeshes: &[Vec<u32>]) -> (Mesh, Bounds) {
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
        lightmap_uv: (lightmap_uvs.len() == positions.len()).then(|| {
            let unorm = |v: f32| ((v.clamp(0.0, 1.0) * 65535.0).round() as u16).to_le_bytes();
            writer.push(&lightmap_uvs.iter().flat_map(|uv| [unorm(uv[0]), unorm(uv[1])]).flatten().collect::<Vec<u8>>())
        }),
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
fn lighting_of(graph: &Graph, world: &World, sky_ambient: Option<[[f32; 3]; 3]>, lightmaps: bool) -> Option<file::Lighting> {
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
        lights.push(file::SceneLight { direction, color: c, intensity: l.intensity, shadows: l.shadows, shadow_strength: l.shadow_strength, baked: l.baked });
    }
    lights.sort_by(|a, b| b.intensity.total_cmp(&a.intensity));
    lights.truncate(4);
    let ambient = graph.ambient.as_ref().and_then(|a| {
        let lin = |c: [f32; 4]| [0, 1, 2].map(|k| material::srgb_to_linear(c[k]) * a.intensity);
        match a.mode {
            1 => Some(file::Ambient { mode: file::AmbientMode::Trilight, sky: lin(a.sky), equator: lin(a.equator), ground: lin(a.ground) }),
            3 => Some(file::Ambient { mode: file::AmbientMode::Flat, sky: lin(a.sky), equator: lin(a.sky), ground: lin(a.sky) }),
            // From the sky: its picture's light when it has one. Otherwise only Unity computes it: the desktop uses the
            // default sky's.
            _ => sky_ambient.map(|[sky, equator, ground]| {
                let k = |c: [f32; 3]| c.map(|v| v * a.intensity);
                file::Ambient { mode: file::AmbientMode::Skybox, sky: k(sky), equator: k(equator), ground: k(ground) }
            }),
        }
    });
    if lights.is_empty() && ambient.is_none() && !lightmaps {
        // Nothing recorded worth drawing in (no directional light, the default sky): the studio's light.
        return None;
    }
    Some(file::Lighting { lights, ambient, reflection_intensity: graph.settings.reflection_intensity.unwrap_or(1.0), fog: place::fog(graph) })
}

/// `f` on each of `jobs` on several threads below normal priority when the build is: results in the same order.
fn in_parallel<T: Sync, R: Send>(jobs: &[T], limits: &Limits, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, limits.threads.max(1)).min(jobs.len().max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<R>>> = jobs.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                limits.lower_priority();
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(job) = jobs.get(i) else { break };
                    *results[i].lock().unwrap() = Some(f(job));
                }
            });
        }
    });
    results.into_iter().map(|r| r.into_inner().unwrap().expect("every job ran")).collect()
}

/// What a material uses a picture for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Use {
    Main,
    Emission,
    Normal,
}

fn decode_all(jobs: &[(String, Use, Option<PathBuf>, bool, u32)], limits: &Limits) -> Vec<anyhow::Result<texture::Encoded>> {
    in_parallel(jobs, limits, |(_, how, path, keep_alpha, side)| {
        limits.check("reading the pictures")?; // i18n: not shown (what a time-limit error says it was doing)
        let path = path.as_ref().context("not in the project")?;
        let img = texture::decode(path, limits.max_texture_bytes)?;
        // An emission mask needs less detail than the colour, ripples and grain a little less.
        let side = match how {
            Use::Main => *side,
            Use::Emission => (*side).min(256),
            Use::Normal => (*side).min(512),
        };
        texture::encode(&img, side, *keep_alpha)
    })
}

fn p_center(b: &Bounds) -> [f32; 3] {
    b.center()
}

/// Each terrain's ground and picture as a candidate of its own, its trees as their prefabs' renderers at their places.
#[allow(clippy::too_many_arguments)]
fn add_terrains(
    graph: &Graph,
    world: &World,
    loader: &mut Loader,
    unity_lightmaps: &[Option<String>],
    candidates: &mut Vec<Candidate>,
    baked: &mut Vec<(Look, image::RgbaImage)>,
    dropped: &mut std::collections::BTreeMap<String, u32>,
) -> anyhow::Result<()> {
    let mut ids: Vec<i64> = graph.objects.iter().filter(|(_, o)| matches!(o, Obj::Terrain(_))).map(|(id, _)| *id).collect();
    ids.sort();
    for id in ids {
        let Some(Obj::Terrain(t)) = graph.objects.get(&id) else { continue };
        let Some(&tr) = world.transform_of.get(&t.game_object) else { continue };
        if !t.enabled || !world.shown(tr) {
            continue;
        }
        loader.limits.check("reading the terrain")?; // i18n: not shown (what a time-limit error says it was doing)
        let Some(r) = &t.data else { continue };
        let Some(path) = r.guid.as_deref().and_then(|g| loader.assets.path(g)) else {
            let what = "a terrain whose data is not in the project".to_string(); // i18n: not shown (a stored name or detail of the scene file)
            if !loader.missing.contains(&what) {
                loader.missing.push(what);
            }
            *dropped.entry("mesh could not be read".to_string()).or_insert(0) += 1; // i18n: not shown (a counter key the report reads, not text)
            continue;
        };
        let name = prefab::rel(loader.assets, &path);
        let mut files = Vec::new();
        let data = match terrain::read(&path, r.file_id, loader.assets, loader.limits, &mut files) {
            Ok(d) => d,
            Err(e) => {
                let what = format!("{name}: {e:#}");
                if !loader.missing.contains(&what) && loader.missing.len() < 64 {
                    loader.missing.push(what);
                }
                *dropped.entry("mesh could not be read".to_string()).or_insert(0) += 1; // i18n: not shown (a counter key the report reads, not text)
                continue;
            }
        };
        let picture = data.picture(loader.assets, loader.limits, &mut files);
        loader.files.extend(files);
        let k = baked.len();
        let material = file::Material { name: data.name.clone(), smoothness: 0.0, ..Default::default() };
        baked.push((Look { material, main_texture: Some(format!("baked:{k}")), ..Default::default() }, picture));
        let mesh = Arc::new(data.mesh());
        // Terrains stand at their transform's place, turned and scaled by nothing else.
        let m = world.matrix(tr);
        let at = [m[12], m[13], m[14]];
        let matrix = math::trs(at, [0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0]);
        let local = bounds_of(&mesh.positions);
        candidates.push(Candidate {
            renderer: id,
            matrix,
            triangles: mesh.triangles() as u64,
            world_bounds: transform_bounds(&matrix, &local),
            key: format!("terrain:{id}"),
            mesh,
            skinned: false,
            materials: vec![MatSource::Baked(k)],
            bones: Vec::new(),
            blend: Vec::new(),
            avatar: None,
            shadows: t.casts_shadows,
            lightmap: place::lightmap_of(graph, unity_lightmaps, id, t.lightmap_index, t.lightmap_st),
        });
        if data.detail_patches > 0 {
            *dropped.entry("terrain details".to_string()).or_insert(0) += data.detail_patches as u32; // i18n: not shown (a counter key the report reads, not text)
        }
        if !t.trees {
            continue;
        }
        // Each prototype's renderers relative to its prefab's root.
        let mut protos: Vec<Vec<(String, Arc<ModelMesh>, Vec<MatSource>, [f32; 16], bool)>> = Vec::new();
        for p in &data.prototypes {
            let mut parts = Vec::new();
            if let Some(g) = p.guid.as_deref() {
                match loader.source(g, 0) {
                    Ok(src) => parts = tree_parts(&src, loader),
                    Err(e) => {
                        let what = format!("{e:#}");
                        if !loader.missing.contains(&what) && loader.missing.len() < 64 {
                            loader.missing.push(what);
                        }
                    }
                }
            }
            protos.push(parts);
        }
        let step = data.trees.len().div_ceil(terrain::MAX_TREES).max(1);
        if step > 1 {
            *dropped.entry("terrain trees thinned".to_string()).or_insert(0) += (data.trees.len() - data.trees.len() / step) as u32; // i18n: not shown (a counter key the report reads, not text)
        }
        for (i, tree) in data.trees.iter().step_by(step).enumerate() {
            let Some(parts) = protos.get(tree.prototype) else { continue };
            loader.limits.check("placing the trees")?; // i18n: not shown (what a time-limit error says it was doing)
            let place = [at[0] + tree.position[0] * data.size[0], at[1] + tree.position[1] * data.size[1], at[2] + tree.position[2] * data.size[2]];
            let (s, c) = (tree.rotation * 0.5).sin_cos();
            let m = math::trs(place, [0.0, s, 0.0, c], [tree.width, tree.height, tree.width]);
            for (key, mesh, materials, local, shadows) in parts {
                let matrix = math::mul(&m, local);
                candidates.push(Candidate {
                    renderer: id ^ ((i as i64 + 1) << 32),
                    matrix,
                    triangles: mesh.triangles() as u64,
                    world_bounds: transform_bounds(&matrix, &bounds_of(&mesh.positions)),
                    key: key.clone(),
                    mesh: mesh.clone(),
                    skinned: false,
                    materials: materials.clone(),
                    bones: Vec::new(),
                    blend: Vec::new(),
                    avatar: None,
                    shadows: *shadows,
                    lightmap: None,
                });
            }
        }
    }
    Ok(())
}

type TreePart = (String, Arc<ModelMesh>, Vec<MatSource>, [f32; 16], bool);

/// What a tree's prefab draws, each part relative to its root, its most detailed level only.
fn tree_parts(src: &Graph, loader: &mut Loader) -> Vec<TreePart> {
    let w = World::new(src);
    let mut lower = std::collections::HashSet::new();
    let mut filters: HashMap<i64, &MeshSource> = HashMap::new();
    for o in src.objects.values() {
        match o {
            Obj::Lod(l) => lower.extend(l.lower.iter().copied()),
            Obj::Filter(f) => {
                filters.insert(f.game_object, &f.mesh);
            }
            _ => {}
        }
    }
    let mut parts = Vec::new();
    for (id, o) in &src.objects {
        let Obj::Renderer(r) = o else { continue };
        if r.skinned || !r.enabled || lower.contains(id) {
            continue;
        }
        let Some(&t) = w.transform_of.get(&r.game_object) else { continue };
        if !w.shown(t) {
            continue;
        }
        let Some(source) = filters.get(&r.game_object).copied() else { continue };
        let key = match source {
            MeshSource::None => continue,
            MeshSource::Model { guid, mesh } => format!("{guid}:{mesh}"),
            MeshSource::Asset(r) => format!("{}:{}", r.guid.as_deref().unwrap_or("-"), r.file_id),
        };
        let Ok(mesh) = resolve_mesh(loader, source) else { continue };
        parts.push((key, mesh, r.materials.clone(), w.matrix(t), r.casts_shadows));
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::prefab::tests::project;

    const BODY_MODEL: &str = "abab0000000000000000000000000001";
    const MISC_MAT: &str = "abab0000000000000000000000000002";
    const BODY_MAT: &str = "abab0000000000000000000000000003";

    /// A project never opened in Unity: the model whose first face is its second slot's ([`FIRST_FACE_MISC`]), with this
    /// `.meta`, two materials of the project, and a scene placing the model with these overrides of its renderer.
    ///
    /// [`FIRST_FACE_MISC`]: crate::scene::model::tests::FIRST_FACE_MISC
    fn body_scene(dir: &Path, meta: &str, overrides: &str) -> Built {
        use crate::scene::model::{self, tests::FIRST_FACE_MISC};
        let mut assets = Assets::empty(dir);
        let a = dir.join("Assets");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(a.join("Body.fbx"), FIRST_FACE_MISC).unwrap();
        std::fs::write(a.join("Body.fbx.meta"), format!("fileFormatVersion: 2\nguid: {BODY_MODEL}\nModelImporter:\n{meta}")).unwrap();
        assets.insert(BODY_MODEL, a.join("Body.fbx"));
        for (guid, name) in [(MISC_MAT, "misc"), (BODY_MAT, "body")] {
            let text = format!("%YAML 1.1\n--- !u!21 &2100000\nMaterial:\n  m_Name: {name}\n  m_Shader: {{fileID: 46, guid: 0000000000000000f000000000000000, type: 0}}\n");
            let path = a.join(format!("{name}.mat"));
            std::fs::write(&path, text).unwrap();
            assets.insert(guid, path);
        }
        let renderer = model::file_id("MeshRenderer", "//RootNode/root/MeshRenderer", 0);
        let overrides = overrides.replace("RENDERER", &renderer.to_string());
        let file = scene_file(
            dir,
            &mut assets,
            &format!(
                "%YAML 1.1
--- !u!1001 &500
PrefabInstance:
  m_Modification:
    m_TransformParent: {{fileID: 0}}
    m_Modifications:
{overrides}    m_RemovedComponents: []
  m_SourcePrefab: {{fileID: 100100000, guid: {BODY_MODEL}, type: 3}}
"
            ),
        );
        build(dir, &file, &assets, &Limits::default()).unwrap()
    }

    /// Each submesh of the only draw: its triangles and its material's name.
    fn parts_of(built: &Built) -> Vec<(u32, String)> {
        let h = &built.header;
        assert_eq!(h.draws.len(), 1, "{:?}", built.report);
        let d = &h.draws[0];
        h.meshes[d.mesh as usize].submeshes.iter().zip(&d.materials).map(|(s, &m)| (s.count / 3, h.materials[m as usize].name.clone())).collect()
    }

    /// What Enzo saw on Ultirex 4.1 before opening it: the scene gives slot 0 the misc material and slot 1 the body's, as
    /// saved against Unity's order of the submeshes, and the body drew the misc material.
    #[test]
    fn a_scenes_materials_land_on_the_submeshes_unity_gave_them() {
        let dir = tempfile::tempdir().unwrap();
        let overrides = format!(
            "    - target: {{fileID: RENDERER, guid: {BODY_MODEL}, type: 3}}
      propertyPath: m_Materials.Array.data[0]
      value:
      objectReference: {{fileID: 2100000, guid: {MISC_MAT}, type: 2}}
    - target: {{fileID: RENDERER, guid: {BODY_MODEL}, type: 3}}
      propertyPath: m_Materials.Array.data[1]
      value:
      objectReference: {{fileID: 2100000, guid: {BODY_MAT}, type: 2}}
"
        );
        let built = body_scene(dir.path(), "  materials:\n    materialImportMode: 2\n", &overrides);
        assert_eq!(parts_of(&built), [(1, "misc".to_string()), (2, "body".to_string())]);
    }

    /// A model changed after its last import: its artifact is passed over and the file is read, which must come out as
    /// the import of that file will, or the preview changes again once Unity has imported it.
    #[test]
    fn a_stale_import_is_passed_over_and_the_scenes_materials_still_land_right() {
        use crate::scene::artifact::tests::{install, model_artifact};
        let dir = tempfile::tempdir().unwrap();
        // One submesh, unlike the model: drawn from this, the test could not see two.
        install(dir.path(), BODY_MODEL, &model_artifact());
        let artifact = dir.path().join("Library/Artifacts/1b").join("1b".repeat(16));
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options().write(true).open(&artifact).unwrap().set_modified(old).unwrap();
        let overrides = format!(
            "    - target: {{fileID: RENDERER, guid: {BODY_MODEL}, type: 3}}
      propertyPath: m_Materials.Array.data[0]
      value:
      objectReference: {{fileID: 2100000, guid: {MISC_MAT}, type: 2}}
    - target: {{fileID: RENDERER, guid: {BODY_MODEL}, type: 3}}
      propertyPath: m_Materials.Array.data[1]
      value:
      objectReference: {{fileID: 2100000, guid: {BODY_MAT}, type: 2}}
"
        );
        let built = body_scene(dir.path(), "  materials:\n    materialImportMode: 2\n", &overrides);
        assert_eq!(parts_of(&built), [(1, "misc".to_string()), (2, "body".to_string())]);
    }

    #[test]
    fn a_models_own_materials_stay_on_their_submeshes() {
        let dir = tempfile::tempdir().unwrap();
        let built = body_scene(dir.path(), "  materials:\n    materialImportMode: 2\n", "");
        assert_eq!(parts_of(&built), [(1, "MiscMatt".to_string()), (2, "BodyMatt".to_string())]);
    }

    /// A scene saved against an older import names a model's own material by that import's number, kept in the `.meta`
    /// (as `1 FBX.fbx` in Enzo's Ulti Zika Wolf V2 does): the first material, always, was not it.
    #[test]
    fn a_models_own_material_is_found_by_an_older_imports_number() {
        let dir = tempfile::tempdir().unwrap();
        let overrides = format!(
            "    - target: {{fileID: RENDERER, guid: {BODY_MODEL}, type: 3}}
      propertyPath: m_Materials.Array.data[0]
      value:
      objectReference: {{fileID: 2100002, guid: {BODY_MODEL}, type: 3}}
"
        );
        let meta = "  fileIDToRecycleName:\n    2100000: MiscMatt\n    2100002: BodyMatt\n";
        let built = body_scene(dir.path(), meta, &overrides);
        assert_eq!(parts_of(&built), [(1, "BodyMatt".to_string()), (2, "BodyMatt".to_string())]);
    }

    /// As Bakery's example models in Enzo's Avatorbit: a `.meta` from before `materialLocation` uses the external materials
    /// found by name, and a model without such a file keeps its own (Unity's default for a new model, a `.meta` without
    /// materials settings at all).
    #[test]
    fn a_meta_older_than_material_location_finds_its_materials_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Assets/Materials");
        std::fs::create_dir_all(&folder).unwrap();
        let text = "%YAML 1.1\n--- !u!21 &2100000\nMaterial:\n  m_Name: external body\n  m_Shader: {fileID: 46, guid: 0000000000000000f000000000000000, type: 0}\n";
        std::fs::write(folder.join("BodyMatt.mat"), text).unwrap();
        let built = body_scene(dir.path(), "  materials:\n    importMaterials: 1\n    materialName: 1\n    materialSearch: 1\n", "");
        assert_eq!(parts_of(&built), [(1, "MiscMatt".to_string()), (2, "external body".to_string())]);
        let built = body_scene(dir.path(), "  materials:\n    materialImportMode: 2\n    materialLocation: 1\n", "");
        assert_eq!(parts_of(&built), [(1, "MiscMatt".to_string()), (2, "BodyMatt".to_string())], "kept in the model");
        let built = body_scene(dir.path(), "  meshes:\n    globalScale: 1\n", "");
        assert_eq!(parts_of(&built), [(1, "MiscMatt".to_string()), (2, "BodyMatt".to_string())], "no materials settings at all");
    }

    /// As Hoodie.fbx's import in Enzo's orbit v4 did: told to make no materials, Unity draws its default one.
    #[test]
    fn a_model_imported_without_materials_draws_unitys_default_one() {
        for meta in ["  materials:\n    materialImportMode: 0\n", "  materials:\n    importMaterials: 0\n"] {
            let dir = tempfile::tempdir().unwrap();
            let built = body_scene(dir.path(), meta, "");
            assert_eq!(parts_of(&built), [(1, "Default-Material".to_string()), (2, "Default-Material".to_string())], "{meta}");
        }
    }

    /// As ALController.fbx's in AudioLink: a remap to a material that is not there leaves the slot empty in Unity's import.
    #[test]
    fn a_remap_to_a_missing_material_leaves_its_slot_empty() {
        let dir = tempfile::tempdir().unwrap();
        let meta = format!(
            "  externalObjects:\n  - first:\n      type: UnityEngine:Material\n      assembly: UnityEngine.CoreModule\n      name: BodyMatt\n    second: {{fileID: 2100000, guid: abab0000000000000000000000000099, type: 2}}\n  - first:\n      type: UnityEngine:Material\n      assembly: UnityEngine.CoreModule\n      name: MiscMatt\n    second: {{fileID: 2100000, guid: {MISC_MAT}, type: 2}}\n"
        );
        let built = body_scene(dir.path(), &meta, "");
        assert_eq!(parts_of(&built), [(1, "misc".to_string()), (2, "None".to_string())]);
    }

    /// An unpacked avatar's renderer, saved in the scene: its bones listed in Unity's order (a bone first that the
    /// model's skin does not have), matched to the model's skin by name.
    #[test]
    fn a_saved_renderers_bones_are_matched_by_name() {
        use crate::scene::model;
        use crate::scene::prefab::tests::{MODEL_GUID, RED_GUID};
        let dir = tempfile::tempdir().unwrap();
        let mut assets = project(dir.path());
        let mesh = model::file_id("Mesh", "Body", 0);
        let scene = format!(
            "%YAML 1.1
--- !u!1 &1
GameObject:
  m_Name: Other
  m_IsActive: 1
--- !u!4 &2
Transform:
  m_GameObject: {{fileID: 1}}
  m_LocalPosition: {{x: 9, y: 9, z: 9}}
  m_Father: {{fileID: 0}}
--- !u!1 &3
GameObject:
  m_Name: Bone
  m_IsActive: 1
--- !u!4 &4
Transform:
  m_GameObject: {{fileID: 3}}
  m_LocalPosition: {{x: 0, y: 5, z: 0}}
  m_Father: {{fileID: 0}}
--- !u!1 &5
GameObject:
  m_Name: Body
  m_IsActive: 1
--- !u!4 &6
Transform:
  m_GameObject: {{fileID: 5}}
  m_Father: {{fileID: 0}}
--- !u!137 &7
SkinnedMeshRenderer:
  m_GameObject: {{fileID: 5}}
  m_Enabled: 1
  m_Materials:
  - {{fileID: 2100000, guid: {RED_GUID}, type: 2}}
  m_Mesh: {{fileID: {mesh}, guid: {MODEL_GUID}, type: 3}}
  m_Bones:
  - {{fileID: 2}}
  - {{fileID: 4}}
"
        );
        std::fs::write(dir.path().join("Assets/Unpacked.unity"), scene).unwrap();
        assets.insert("ffff0000000000000000000000000010", dir.path().join("Assets/Unpacked.unity"));
        let file = SceneFile { path: dir.path().join("Assets/Unpacked.unity"), guid: "f".repeat(32), name: "Unpacked".into() };
        let built = build(dir.path(), &file, &assets, &Limits::default()).unwrap();
        let h = &built.header;
        assert_eq!(h.draws.len(), 1);
        let b = h.meshes[h.draws[0].mesh as usize].bounds;
        // Bound one metre up, its bone now five metres up: the quad four to five metres up.
        assert!((b.min[1] - 4.0).abs() < 1e-3 && (b.max[1] - 5.0).abs() < 1e-3, "{b:?}");
    }

    const WORLD_MODEL: &str = "eeee0000000000000000000000000001";
    const LIGHTMAP: &str = "eeee0000000000000000000000000002";
    const LIGHTING: &str = "eeee0000000000000000000000000003";
    const SKY_MAT: &str = "eeee0000000000000000000000000004";
    const SKY_PICTURE: &str = "eeee0000000000000000000000000005";
    const TERRAIN: &str = "eeee0000000000000000000000000006";
    const TREE: &str = "eeee0000000000000000000000000007";

    fn scene_file(dir: &Path, assets: &mut Assets, text: &str) -> SceneFile {
        let p = dir.join("Assets/World.unity");
        std::fs::write(&p, text).unwrap();
        assets.insert("ffff0000000000000000000000000020", p.clone());
        SceneFile { path: p, guid: "f".repeat(32), name: "World".into() }
    }

    /// A world whose ground is a `.blend` model: a scene placing it 10 m along, with a descriptor saying it is a world.
    fn blend_world(dir: &Path, assets: &mut Assets, extra: &str) -> SceneFile {
        let a = dir.join("Assets");
        std::fs::write(a.join("World.blend"), b"BLENDER").unwrap();
        std::fs::write(a.join("World.blend.meta"), format!("fileFormatVersion: 2\nguid: {WORLD_MODEL}\n")).unwrap();
        assets.insert(WORLD_MODEL, a.join("World.blend"));
        let text = format!(
            "%YAML 1.1
--- !u!1001 &500
PrefabInstance:
  m_Modification:
    m_TransformParent: {{fileID: 0}}
    m_Modifications:
    - target: {{fileID: 11, guid: {WORLD_MODEL}, type: 3}}
      propertyPath: m_LocalPosition.z
      value: 10
      objectReference: {{fileID: 0}}
  m_SourcePrefab: {{fileID: 100100000, guid: {WORLD_MODEL}, type: 3}}
{extra}"
        );
        scene_file(dir, assets, &text)
    }

    #[test]
    fn a_model_only_unity_reads_comes_from_its_import_with_its_sun() {
        use crate::scene::artifact::tests::{install, model_artifact};
        let dir = tempfile::tempdir().unwrap();
        let mut assets = project(dir.path());
        let file = blend_world(dir.path(), &mut assets, "");
        // Never opened in Unity: said plainly.
        let e = build(dir.path(), &file, &assets, &Limits::default()).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(e.contains("open the project in Unity once"), "{e}");
        install(dir.path(), WORLD_MODEL, &model_artifact());
        let built = build(dir.path(), &file, &assets, &Limits::default()).unwrap();
        let h = &built.header;
        assert_eq!(h.draws.len(), 1, "{:?}", built.report);
        // The part stands 3 m from the root, which stands 2 m along X and, by the scene's override, 10 m along Z.
        let p = file::parse(built.bytes.clone()).unwrap();
        let m = p.matrix(h.draws[0].instance);
        assert_eq!([m[12], m[13], m[14]], [5.0, 0.0, 10.0]);
        // The model's material is the project's red one.
        assert_eq!(h.materials[0].color, [1.0, 0.0, 0.0, 1.0]);
        let l = h.lighting.as_ref().expect("the model's sun");
        assert_eq!(l.lights.len(), 1);
        assert_eq!(l.lights[0].intensity, 2.0);
        assert!(built.files.iter().any(|f| f.ends_with("World.blend")), "built again when the model changes");
    }

    #[test]
    fn a_renderers_lightmap_comes_from_the_scene_and_its_uv2_goes_along() {
        use crate::scene::artifact::tests::{install, model_artifact};
        use crate::scene::prefab::mixed;
        let dir = tempfile::tempdir().unwrap();
        let mut assets = project(dir.path());
        install(dir.path(), WORLD_MODEL, &model_artifact());
        let a = dir.path().join("Assets");
        image::RgbImage::from_pixel(8, 8, image::Rgb([128, 64, 32])).save(a.join("Lightmap-0.png")).unwrap();
        assets.insert(LIGHTMAP, a.join("Lightmap-0.png"));
        std::fs::write(a.join("LightingData.asset"), format!("%YAML 1.1\n--- !u!1120 &112000000\nLightingDataAsset:\n  m_Lightmaps:\n  - m_Lightmap: {{fileID: 2800000, guid: {LIGHTMAP}, type: 3}}\n")).unwrap();
        assets.insert(LIGHTING, a.join("LightingData.asset"));
        // Unity's own: the renderer's index and place, overridden on the instance.
        let renderer = 23;
        let extra = format!(
            "--- !u!157 &3
LightmapSettings:
  m_LightingDataAsset: {{fileID: 112000000, guid: {LIGHTING}, type: 2}}
"
        );
        let file = blend_world(dir.path(), &mut assets, &extra);
        let text = std::fs::read_to_string(&file.path).unwrap().replace(
            "  m_SourcePrefab:",
            &format!(
                "    - target: {{fileID: {renderer}, guid: {WORLD_MODEL}, type: 3}}\n      propertyPath: m_LightmapIndex\n      value: 0\n      objectReference: {{fileID: 0}}\n    - target: {{fileID: {renderer}, guid: {WORLD_MODEL}, type: 3}}\n      propertyPath: m_LightmapTilingOffset.x\n      value: 0.5\n      objectReference: {{fileID: 0}}\n  m_SourcePrefab:"
            ),
        );
        std::fs::write(&file.path, text).unwrap();
        let built = build(dir.path(), &file, &assets, &Limits::default()).unwrap();
        let h = &built.header;
        let l = h.draws[0].lightmap.expect("a lightmap");
        assert_eq!((l.index, l.st), (0, [0.5, 1.0, 0.0, 0.0]));
        assert_eq!(h.lightmaps.len(), 1);
        assert!(h.lightmaps[0].scale > 0.0);
        let uv = h.meshes[h.draws[0].mesh as usize].lightmap_uv.expect("its second UV set");
        assert_eq!(uv.length, 3 * file::LIGHTMAP_UV_SIZE as u64);
        let p = file::parse(built.bytes.clone()).expect("a valid preview");
        assert_eq!(u16::from_le_bytes(p.blob(uv)[..2].try_into().unwrap()), (0.25f32 * 65535.0).round() as u16);
        assert!(built.files.iter().any(|f| f.ends_with("Lightmap-0.png")));
        // Bakery's list counts over the renderer's: its storage names the same renderer in another place.
        let bakery = format!(
            "--- !u!114 &900
MonoBehaviour:
  m_GameObject: {{fileID: 0}}
  m_Enabled: 1
  m_Script: {{fileID: 11500000, guid: b7fa80e7116296f4eb4f49ec1544ee22, type: 3}}
  maps:
  - {{fileID: 2800000, guid: {LIGHTMAP}, type: 3}}
  bakedRenderers:
  - {{fileID: {}}}
  bakedIDs: 00000000
  bakedScaleOffset:
  - {{x: 0.25, y: 0.25, z: 0.5, w: 0.5}}
",
            mixed(500, renderer)
        );
        let text = std::fs::read_to_string(&file.path).unwrap() + &bakery;
        std::fs::write(&file.path, text).unwrap();
        let built = build(dir.path(), &file, &assets, &Limits::default()).unwrap();
        assert_eq!(built.header.draws[0].lightmap.unwrap().st, [0.25, 0.25, 0.5, 0.5]);
    }

    #[test]
    fn a_scene_keeps_its_sky_fog_and_ambient_light_from_its_picture() {
        let dir = tempfile::tempdir().unwrap();
        let mut assets = project(dir.path());
        let a = dir.path().join("Assets");
        // Blue above, brown below.
        let mut sky = image::RgbImage::new(16, 8);
        for (_, y, p) in sky.enumerate_pixels_mut() {
            *p = if y < 4 { image::Rgb([60, 120, 240]) } else { image::Rgb([120, 80, 40]) };
        }
        sky.save(a.join("Sky.png")).unwrap();
        assets.insert(SKY_PICTURE, a.join("Sky.png"));
        std::fs::write(
            a.join("Sky.mat"),
            format!(
                "%YAML 1.1\n--- !u!21 &2100000\nMaterial:\n  m_Name: Sky\n  m_Shader: {{fileID: 108, guid: 0000000000000000f000000000000000, type: 0}}\n  m_SavedProperties:\n    m_TexEnvs:\n    - _MainTex:\n        m_Texture: {{fileID: 2800000, guid: {SKY_PICTURE}, type: 3}}\n        m_Scale: {{x: 1, y: 1}}\n        m_Offset: {{x: 0, y: 0}}\n    m_Floats:\n    - _Exposure: 1\n    - _Rotation: 90\n    m_Colors:\n    - _Tint: {{r: 0.5, g: 0.5, b: 0.5, a: 1}}\n"
            ),
        )
        .unwrap();
        assets.insert(SKY_MAT, a.join("Sky.mat"));
        let text = format!(
            "%YAML 1.1
--- !u!104 &2
RenderSettings:
  m_Fog: 1
  m_FogColor: {{r: 1, g: 1, b: 1, a: 1}}
  m_FogMode: 1
  m_LinearFogStart: 5
  m_LinearFogEnd: 50
  m_AmbientMode: 0
  m_AmbientIntensity: 1
  m_SkyboxMaterial: {{fileID: 2100000, guid: {SKY_MAT}, type: 2}}
  m_ReflectionIntensity: 0.5
--- !u!1 &1
GameObject:
  m_Name: Cube
  m_IsActive: 1
--- !u!4 &4
Transform:
  m_GameObject: {{fileID: 1}}
  m_Father: {{fileID: 0}}
--- !u!33 &5
MeshFilter:
  m_GameObject: {{fileID: 1}}
  m_Mesh: {{fileID: 10202, guid: 0000000000000000e000000000000000, type: 0}}
--- !u!23 &6
MeshRenderer:
  m_GameObject: {{fileID: 1}}
  m_Enabled: 1
"
        );
        let file = scene_file(dir.path(), &mut assets, &text);
        let built = build(dir.path(), &file, &assets, &Limits::default()).unwrap();
        let h = &built.header;
        let s = h.sky.expect("a sky");
        assert_eq!(s.rotation, 90.0);
        assert!(s.texture >= 0 && (s.texture as usize) < h.textures.len());
        let l = h.lighting.as_ref().expect("its light");
        assert_eq!(l.reflection_intensity, 0.5);
        let fog = l.fog.expect("its fog");
        assert_eq!((fog.mode, fog.start, fog.end), (file::FogMode::Linear, 5.0, 50.0));
        // The ambient light the picture gives: blue from above, brown from below.
        let am = l.ambient.expect("from the sky's picture");
        assert_eq!(am.mode, file::AmbientMode::Skybox);
        assert!(am.sky[2] > am.sky[0] && am.ground[0] > am.ground[2], "{am:?}");
    }

    #[test]
    fn a_terrain_draws_its_ground_and_its_trees() {
        let dir = tempfile::tempdir().unwrap();
        let mut assets = project(dir.path());
        let a = dir.path().join("Assets");
        std::fs::write(
            a.join("Tree.prefab"),
            "%YAML 1.1
--- !u!1 &1
GameObject:
  m_Name: Tree
  m_IsActive: 1
--- !u!4 &2
Transform:
  m_GameObject: {fileID: 1}
  m_Father: {fileID: 0}
--- !u!33 &3
MeshFilter:
  m_GameObject: {fileID: 1}
  m_Mesh: {fileID: 10202, guid: 0000000000000000e000000000000000, type: 0}
--- !u!23 &4
MeshRenderer:
  m_GameObject: {fileID: 1}
  m_Enabled: 1
--- !u!1 &5
GameObject:
  m_Name: TreeFar
  m_IsActive: 1
--- !u!4 &6
Transform:
  m_GameObject: {fileID: 5}
  m_Father: {fileID: 2}
--- !u!33 &7
MeshFilter:
  m_GameObject: {fileID: 5}
  m_Mesh: {fileID: 10202, guid: 0000000000000000e000000000000000, type: 0}
--- !u!23 &8
MeshRenderer:
  m_GameObject: {fileID: 5}
  m_Enabled: 1
--- !u!205 &9
LODGroup:
  m_GameObject: {fileID: 1}
  m_LODs:
  - renderers:
    - renderer: {fileID: 4}
  - renderers:
    - renderer: {fileID: 8}
",
        )
        .unwrap();
        assets.insert(TREE, a.join("Tree.prefab"));
        std::fs::write(a.join("Small.asset"), crate::scene::terrain::tests::asset(TREE)).unwrap();
        assets.insert(TERRAIN, a.join("Small.asset"));
        let text = format!(
            "%YAML 1.1
--- !u!1 &1
GameObject:
  m_Name: Ground
  m_IsActive: 1
--- !u!4 &2
Transform:
  m_GameObject: {{fileID: 1}}
  m_LocalPosition: {{x: 100, y: 1, z: 0}}
  m_Father: {{fileID: 0}}
--- !u!218 &3
Terrain:
  m_GameObject: {{fileID: 1}}
  m_Enabled: 1
  m_TerrainData: {{fileID: 15600000, guid: {TERRAIN}, type: 2}}
  m_DrawTreesAndFoliage: 1
"
        );
        let file = scene_file(dir.path(), &mut assets, &text);
        let built = build(dir.path(), &file, &assets, &Limits::default()).unwrap();
        let h = &built.header;
        // The ground and two trees: each the prefab's most detailed level only.
        assert_eq!(h.draws.len(), 3, "{:?}", built.report);
        let p = file::parse(built.bytes.clone()).unwrap();
        let places: Vec<[f32; 3]> = h.draws.iter().map(|d| {
            let m = p.matrix(d.instance);
            [m[12], m[13], m[14]]
        }).collect();
        // Ground at the transform; a tree at the terrain's middle, 2 wide and 3 tall; one at its far corner, half up.
        assert!(places.contains(&[100.0, 1.0, 0.0]), "{places:?}");
        assert!(places.contains(&[105.0, 1.0, 5.0]), "{places:?}");
        assert!(places.contains(&[110.0, 3.0, 10.0]), "{places:?}");
        let tree = h.draws.iter().find(|d| p.matrix(d.instance)[12] == 105.0).unwrap();
        let m = p.matrix(tree.instance);
        assert_eq!((m[0], m[5]), (2.0, 3.0));
        // The ground's picture was made by the build.
        let ground = h.draws.iter().find(|d| p.matrix(d.instance)[12] == 100.0).unwrap();
        assert!(h.materials[ground.materials[0] as usize].texture >= 0);
        assert!(built.files.iter().any(|f| f.ends_with("Small.asset")));
    }

    /// Ultirex 4.1's tail: a skin whose file gives some vertices no weight. Unity binds them wholly to the first bone;
    /// so does the reader, so they move with the skeleton instead of staying in the mesh's own space, metres away.
    #[test]
    fn a_vertex_without_weights_follows_the_first_bone_as_in_unity() {
        use crate::scene::model;
        let dir = tempfile::tempdir().unwrap();
        let p = model::tests::rig_file_with_weights(dir.path(), "0,1", "1,1");
        let m = model::load(&p, &model::ImportSettings::default(), &Limits::default()).unwrap();
        let quad = m.meshes.iter().find(|x| x.skin.is_some()).unwrap();
        let skin = quad.skin.as_ref().unwrap();
        assert!(skin.weights.iter().all(|w| w[0] == (0, 1.0)), "{:?}", skin.weights);
        // Posed with its bone moved up 4 m: all of the mesh moves up 4 m, also what has no weight in the file.
        let t = math::trs([0.0, 5.0, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
        let mut objects = HashMap::new();
        objects.insert(1, Obj::GameObject(prefab::GameObject { name: "Bone".into(), active: true, editor_only: false }));
        objects.insert(2, Obj::Transform(prefab::Transform { game_object: 1, position: [0.0, 5.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3], father: 0 }));
        let g = Graph { objects, ..Default::default() };
        let w = World::new(&g);
        let (posed, _) = pose(quad, &[], &[2], &w, t);
        let b = bounds_of(&posed);
        assert!((b.min[1] - 4.0).abs() < 1e-3 && (b.max[1] - 5.0).abs() < 1e-3, "{b:?}");
    }

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
