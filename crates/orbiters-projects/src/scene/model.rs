//! Models (FBX, binary or ASCII, and OBJ) as Unity imports them, read with ufbx: the node hierarchy Unity makes
//! GameObjects of, each node's local transform, meshes with their submeshes (one per material used), skinning and blend
//! shapes, and the model's own materials. Everything is converted to Unity's space here, once:
//!
//! - **Handedness.** FBX is right-handed and Unity left-handed: Unity mirrors the X axis, so a position `(x, y, z)`
//!   becomes `(-x, y, z)`, a rotation quaternion `(x, y, z, w)` becomes `(x, -y, -z, w)`, and each triangle's corners
//!   are reversed (mirroring turns a face's winding round; Unity's front faces are clockwise in its left-handed space).
//! - **Units.** With "Convert Units" (`useFileScale`, the default) the file's unit becomes metres; `globalScale` (Scale
//!   Factor) multiplies it. Unity scales translations and vertices, never a node's scale (a Blender model whose objects
//!   have a scale of 100 keeps it), so overrides of bone positions in prefabs, in metres, apply as written.
//! - **Axes.** A model whose file says Z is up (3ds Max) is turned upright on its top nodes, as the FBX SDK Unity uses
//!   does. Blender writes Y-up files with its own rotation on objects, which is kept.
//!
//! Unity names each object it makes from a model by a hash of its place in the hierarchy (Unity's own scheme for
//! `fileIdsGeneration: 2`, the default since Unity 2019.3): `xxHash64("Type:<class>-><path><n>")` read as a signed 64-bit number, the
//! path being `//RootNode/root` for the model's root and `/<node name>` more per level, a component adding
//! `/<class>`, and `<n>` 0 unless that ID was already taken. A model with a single top node merges it into the root.
//! Prefabs and scenes refer to a model's objects by those IDs, so they are what [`super::prefab`] resolves.

use super::Limits;
use anyhow::{bail, Context as _};
use std::collections::HashMap;
use std::path::Path;

/// Unity's class IDs of the objects a model makes.
pub const GAME_OBJECT: u32 = 1;
pub const TRANSFORM: u32 = 4;
pub const MESH_RENDERER: u32 = 23;
pub const MESH_FILTER: u32 = 33;
pub const MESH: u32 = 43;
pub const SKINNED_MESH_RENDERER: u32 = 137;

/// A model's object ID: `xxHash64("Type:<class>-><identifier><n>")` as Unity computes it.
pub fn file_id(class: &str, identifier: &str, n: u32) -> i64 {
    xxh64(format!("Type:{class}->{identifier}{n}").as_bytes(), 0) as i64
}

pub struct ModelNode {
    pub name: String,
    pub parent: Option<usize>,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    /// Its mesh, drawn by a SkinnedMeshRenderer (skinned or with blend shapes) or a MeshRenderer.
    pub mesh: Option<usize>,
    /// The model's materials of its submeshes, in the submeshes' order, which is Unity's ([`mesh_of`]) (indices into
    /// [`Model::materials`]).
    pub materials: Vec<usize>,
    /// Hidden in the file (Unity turns the renderer off when it imports visibility).
    pub visible: bool,
    /// Unity's IDs of the node's GameObject, Transform and renderer.
    pub ids: NodeIds,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NodeIds {
    pub game_object: i64,
    pub transform: i64,
    pub renderer: i64,
    pub filter: i64,
}

pub struct Skin {
    /// The bones (node indices), in the order the mesh's bind poses are.
    pub bones: Vec<usize>,
    /// Their names (a model's; empty for a mesh asset, whose bind poses are already in the renderer's bones' order).
    /// A renderer saved in a file lists its bones in Unity's order, which drops the bones without weights
    /// (`optimizeBones`), so a model's bones are matched to the renderer's by name.
    pub names: Vec<String>,
    /// Unity's bind poses: from the mesh's space to each bone's, at bind time (column-major).
    pub bind_poses: Vec<[f32; 16]>,
    /// Up to four (bone, weight) a vertex, the weights summing to one. Unity binds a vertex the file gives no weight to
    /// (Ultirex 4.1's tail has 1,743 of them, hidden inside the body) wholly to its first bone, the one nearest the
    /// root: such a vertex here is bound the same way, so it moves with the skeleton. Left unbound, it stayed where the
    /// mesh's own space put it, tens of metres away for a mesh bound with a small skew in its bind poses.
    pub weights: Vec<[(u16, f32); 4]>,
}

pub struct BlendShape {
    pub name: String,
    /// The shape's frames: (weight 0–100 where it is whole, sparse position offsets).
    pub frames: Vec<(f32, Vec<(u32, [f32; 3])>)>,
    /// The weight a renderer starts with: 0, as Unity imports every blend shape (a value the file keeps, a shape key
    /// left on in Blender, is not applied).
    pub default_weight: f32,
}

pub struct ModelMesh {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// The second UV set, which lightmaps use (empty without one: Unity then uses the first).
    pub uv2: Vec<[f32; 2]>,
    /// Triangles of each submesh (Unity's winding).
    pub submeshes: Vec<Vec<u32>>,
    pub skin: Option<Skin>,
    pub blend_shapes: Vec<BlendShape>,
}

impl ModelMesh {
    pub fn triangles(&self) -> usize {
        self.submeshes.iter().map(|s| s.len() / 3).sum()
    }
}

#[derive(Clone, Debug, Default)]
pub struct ModelMaterial {
    pub name: String,
    /// sRGB.
    pub color: [f32; 4],
    /// The base colour texture's file name as the model stores it (Unity looks it up by name in the project).
    pub texture: Option<String>,
}

pub struct Model {
    pub nodes: Vec<ModelNode>,
    pub meshes: Vec<ModelMesh>,
    pub materials: Vec<ModelMaterial>,
    /// Unity's IDs of the model's meshes (`Type:Mesh-><name>0`) → mesh index.
    pub mesh_ids: HashMap<i64, usize>,
}

impl Model {
    /// The node whose GameObject, Transform or renderer has Unity's ID `id`.
    pub fn node_of(&self, id: i64) -> Option<usize> {
        self.nodes.iter().position(|n| n.ids.game_object == id || n.ids.transform == id || n.ids.renderer == id || n.ids.filter == id)
    }
}

/// What the model's `.meta` file says about its import (the settings that change what is drawn).
#[derive(Clone, Debug)]
pub struct ImportSettings {
    pub global_scale: f32,
    pub use_file_scale: bool,
    pub import_blend_shapes: bool,
    pub import_visibility: bool,
    pub preserve_hierarchy: bool,
    /// Normals: 0 imported, 1 calculated, 2 none.
    pub normals: i64,
    /// Extracted materials by the model's material name (the meta's `externalObjects`): their GUIDs.
    pub remapped: HashMap<String, String>,
    /// `materialImportMode` 0: none (Unity draws the default material).
    pub import_materials: bool,
    /// `materialLocation` 0: the legacy external materials, found by name next to the model. A `.meta` older than that
    /// setting has materials settings without it, and means the same (Bakery's examples: `Materials/Light.mat`).
    pub legacy_materials: bool,
    /// 0: by base texture name, 1: the model's material name, 2: both, 3: model name + material name.
    pub material_name: i64,
    /// 0: local Materials folder, 1: recursive up, 2: everywhere.
    pub material_search: i64,
    /// The names of the model's own materials by the IDs an older import gave them (`fileIDToRecycleName`,
    /// `internalIDToNameTable`: 2100000, 2100002, …), which prefabs and scenes saved then still use.
    pub material_ids: HashMap<i64, String>,
}

impl Default for ImportSettings {
    fn default() -> Self {
        Self {
            global_scale: 1.0,
            use_file_scale: true,
            import_blend_shapes: true,
            import_visibility: true,
            preserve_hierarchy: false,
            normals: 0,
            remapped: HashMap::new(),
            import_materials: true,
            legacy_materials: false,
            material_name: 1,
            material_search: 1,
            material_ids: HashMap::new(),
        }
    }
}

impl ImportSettings {
    /// From a model's `.meta` (missing: Unity's defaults).
    pub fn read(meta: &str) -> Self {
        let v = super::yaml::parse(meta);
        let m = v.get("ModelImporter");
        let mut s = Self::default();
        let meshes = m.get("meshes");
        s.global_scale = meshes.get("globalScale").f32_or(1.0);
        s.use_file_scale = meshes.get("useFileScale").as_bool().unwrap_or(true);
        s.import_blend_shapes = meshes.get("importBlendShapes").as_bool().unwrap_or(true);
        s.import_visibility = meshes.get("importVisibility").as_bool().unwrap_or(true);
        s.preserve_hierarchy = meshes.get("preserveHierarchy").as_bool().unwrap_or(false);
        s.normals = m.at(&["tangentSpace", "normalImportMode"]).as_i64().unwrap_or(0);
        let mats = m.get("materials");
        s.import_materials = mats.get("materialImportMode").as_i64().unwrap_or(1) != 0 && mats.get("importMaterials").as_i64().unwrap_or(1) != 0;
        s.legacy_materials = match mats.get("materialLocation").as_i64() {
            Some(location) => location == 0,
            None => !mats.entries().is_empty(),
        };
        s.material_name = mats.get("materialName").as_i64().unwrap_or(1);
        s.material_search = mats.get("materialSearch").as_i64().unwrap_or(1);
        for (id, name) in m.get("fileIDToRecycleName").entries() {
            if let (Ok(id), Some(name)) = (id.parse::<i64>(), name.as_str()) {
                if (2_100_000..2_200_000).contains(&id) {
                    s.material_ids.insert(id, name.to_string());
                }
            }
        }
        for e in m.get("internalIDToNameTable").items() {
            let Some(name) = e.get("second").as_str() else { continue };
            if let Some(id) = e.get("first").get("21").as_i64() {
                s.material_ids.insert(id, name.to_string());
            }
        }
        for e in m.get("externalObjects").items() {
            let first = e.get("first");
            if !first.get("type").str_or("").ends_with("Material") {
                continue;
            }
            let (Some(name), Some(r)) = (first.get("name").as_str(), e.get("second").as_ref()) else { continue };
            if let Some(g) = r.guid {
                s.remapped.insert(name.to_string(), g);
            }
        }
        s
    }
}

/// Loads a model as Unity imports it.
pub fn load(path: &Path, settings: &ImportSettings, limits: &Limits) -> anyhow::Result<Model> {
    let size = std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?.len();
    anyhow::ensure!(size <= limits.max_model_bytes, "{} is {} MB, more than a preview reads", file_name(path), size >> 20);
    limits.check("loading the models")?; // i18n: not shown (what a time-limit error says it was doing)
    let deadline = limits.deadline;
    let cancel = limits.cancel.clone();
    let progress = move |_: &ufbx::Progress| {
        if std::time::Instant::now() > deadline || cancel.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            ufbx::ProgressResult::Cancel
        } else {
            ufbx::ProgressResult::Continue
        }
    };
    let opts = ufbx::LoadOpts {
        ignore_animation: true,
        ignore_embedded: true,
        load_external_files: false,
        generate_missing_normals: true,
        // Z-up files are turned upright on their top nodes; units and handedness are converted below, as Unity does.
        target_axes: ufbx::CoordinateAxes {
            right: ufbx::CoordinateAxis::PositiveX,
            up: ufbx::CoordinateAxis::PositiveY,
            front: ufbx::CoordinateAxis::PositiveZ,
        },
        space_conversion: ufbx::SpaceConversion::AdjustTransforms,
        obj_unit_meters: 1.0,
        temp_allocator: ufbx::AllocatorOpts { memory_limit: limits.max_model_memory, ..Default::default() },
        result_allocator: ufbx::AllocatorOpts { memory_limit: limits.max_model_memory, ..Default::default() },
        progress_cb: ufbx::ProgressCb::Ref(&progress),
        progress_interval_hint: 1 << 20,
        ..Default::default()
    };
    let path_str = path.to_str().context("a model path that is not Unicode")?;
    let scene = match ufbx::load_file(path_str, opts) {
        Ok(s) => s,
        Err(e) => {
            limits.check("loading the models")?; // i18n: not shown (what a time-limit error says it was doing)
            bail!("{} could not be read: {}", file_name(path), e.description);
        }
    };
    let unit = settings.global_scale as f64 * if settings.use_file_scale { scene.settings.unit_meters.max(1e-6) } else { 1.0 };
    convert(&scene, settings, unit as f32, limits)
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Unity's space: X mirrored, translations in metres.
fn pos(v: ufbx::Vec3, unit: f32) -> [f32; 3] {
    [-(v.x as f32) * unit, v.y as f32 * unit, v.z as f32 * unit]
}

fn dir(v: ufbx::Vec3) -> [f32; 3] {
    [-(v.x as f32), v.y as f32, v.z as f32]
}

/// A ufbx affine matrix as Unity's column-major 4×4, conjugated into Unity's space (`C · m · C⁻¹`, `C` the mirror and
/// unit scale).
fn unity_matrix(m: &ufbx::Matrix, unit: f32) -> [f32; 16] {
    let s = [-1.0f32, 1.0, 1.0];
    let cols = [[m.m00, m.m10, m.m20], [m.m01, m.m11, m.m21], [m.m02, m.m12, m.m22]];
    let mut out = [0.0f32; 16];
    for c in 0..3 {
        for r in 0..3 {
            out[c * 4 + r] = s[r] * cols[c][r] as f32 * s[c];
        }
    }
    let t = [m.m03, m.m13, m.m23];
    for r in 0..3 {
        out[12 + r] = s[r] * t[r] as f32 * unit;
    }
    out[15] = 1.0;
    out
}

fn transform_point(m: &ufbx::Matrix, v: ufbx::Vec3) -> ufbx::Vec3 {
    ufbx::Vec3 {
        x: m.m00 * v.x + m.m01 * v.y + m.m02 * v.z + m.m03,
        y: m.m10 * v.x + m.m11 * v.y + m.m12 * v.z + m.m13,
        z: m.m20 * v.x + m.m21 * v.y + m.m22 * v.z + m.m23,
    }
}

fn transform_dir(m: &ufbx::Matrix, v: ufbx::Vec3) -> ufbx::Vec3 {
    ufbx::Vec3 { x: m.m00 * v.x + m.m01 * v.y + m.m02 * v.z, y: m.m10 * v.x + m.m11 * v.y + m.m12 * v.z, z: m.m20 * v.x + m.m21 * v.y + m.m22 * v.z }
}

fn invert(m: &ufbx::Matrix) -> ufbx::Matrix {
    let a = [[m.m00, m.m01, m.m02], [m.m10, m.m11, m.m12], [m.m20, m.m21, m.m22]];
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    let d = if det.abs() < 1e-30 { 1.0 } else { 1.0 / det };
    let i = [
        [(a[1][1] * a[2][2] - a[1][2] * a[2][1]) * d, (a[0][2] * a[2][1] - a[0][1] * a[2][2]) * d, (a[0][1] * a[1][2] - a[0][2] * a[1][1]) * d],
        [(a[1][2] * a[2][0] - a[1][0] * a[2][2]) * d, (a[0][0] * a[2][2] - a[0][2] * a[2][0]) * d, (a[0][2] * a[1][0] - a[0][0] * a[1][2]) * d],
        [(a[1][0] * a[2][1] - a[1][1] * a[2][0]) * d, (a[0][1] * a[2][0] - a[0][0] * a[2][1]) * d, (a[0][0] * a[1][1] - a[0][1] * a[1][0]) * d],
    ];
    let t = [m.m03, m.m13, m.m23];
    let it = [0, 1, 2].map(|r| -(i[r][0] * t[0] + i[r][1] * t[1] + i[r][2] * t[2]));
    ufbx::Matrix {
        m00: i[0][0],
        m01: i[0][1],
        m02: i[0][2],
        m03: it[0],
        m10: i[1][0],
        m11: i[1][1],
        m12: i[1][2],
        m13: it[1],
        m20: i[2][0],
        m21: i[2][1],
        m22: i[2][2],
        m23: it[2],
    }
}

fn mul(a: &ufbx::Matrix, b: &ufbx::Matrix) -> ufbx::Matrix {
    let ar = [[a.m00, a.m01, a.m02, a.m03], [a.m10, a.m11, a.m12, a.m13], [a.m20, a.m21, a.m22, a.m23]];
    let bc = |c: usize| match c {
        0 => [b.m00, b.m10, b.m20, 0.0],
        1 => [b.m01, b.m11, b.m21, 0.0],
        2 => [b.m02, b.m12, b.m22, 0.0],
        _ => [b.m03, b.m13, b.m23, 1.0],
    };
    let e = |r: usize, c: usize| {
        let col = bc(c);
        ar[r][0] * col[0] + ar[r][1] * col[1] + ar[r][2] * col[2] + ar[r][3] * col[3]
    };
    ufbx::Matrix {
        m00: e(0, 0),
        m01: e(0, 1),
        m02: e(0, 2),
        m03: e(0, 3),
        m10: e(1, 0),
        m11: e(1, 1),
        m12: e(1, 2),
        m13: e(1, 3),
        m20: e(2, 0),
        m21: e(2, 1),
        m22: e(2, 2),
        m23: e(2, 3),
    }
}

fn convert(scene: &ufbx::Scene, settings: &ImportSettings, unit: f32, limits: &Limits) -> anyhow::Result<Model> {
    // Unity's root is ufbx's root; a single top node is merged into it.
    let root = &scene.root_node;
    let top: Vec<&ufbx::Node> = root.children.iter().map(|n| &**n).collect();
    let merged = (top.len() == 1 && !settings.preserve_hierarchy).then(|| top[0].element.element_id);
    let mut nodes: Vec<ModelNode> = Vec::new();
    let mut taken: HashMap<i64, ()> = HashMap::new();
    let mut unique = |class: &str, ident: &str| -> i64 {
        let mut n = 0;
        loop {
            let id = file_id(class, ident, n);
            if taken.insert(id, ()).is_none() {
                return id;
            }
            n += 1;
        }
    };
    // Depth first, parents before children; (ufbx node, Unity parent, its path).
    let mut ufbx_of: Vec<&ufbx::Node> = Vec::new();
    let mut stack: Vec<(&ufbx::Node, Option<usize>, String)> = Vec::new();
    match merged {
        Some(_) => stack.push((top[0], None, "//RootNode/root".into())),
        None => stack.push((root, None, "//RootNode/root".into())),
    }
    while let Some((node, parent, path)) = stack.pop() {
        if nodes.len() >= limits.max_nodes {
            bail!("the model has more than {} nodes", limits.max_nodes);
        }
        let index = nodes.len();
        let t = &node.local_transform;
        let is_root = parent.is_none();
        let (position, rotation, scale) = if is_root && merged.is_none() {
            ([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0; 3])
        } else {
            (
                pos(t.translation, unit),
                [t.rotation.x as f32, -(t.rotation.y as f32), -(t.rotation.z as f32), t.rotation.w as f32],
                [t.scale.x as f32, t.scale.y as f32, t.scale.z as f32],
            )
        };
        let skinned = node.mesh.as_ref().is_some_and(|m| !m.skin_deformers.is_empty() || (settings.import_blend_shapes && !m.blend_deformers.is_empty()));
        let has_mesh = node.mesh.as_ref().is_some_and(|m| m.num_triangles > 0);
        let ids = NodeIds {
            game_object: unique("GameObject", &path),
            transform: unique("Transform", &format!("{path}/Transform")), // i18n: not shown (Unity ids and names of a model's parts)
            renderer: if !has_mesh {
                0
            } else if skinned {
                unique("SkinnedMeshRenderer", &format!("{path}/SkinnedMeshRenderer"))
            } else {
                unique("MeshRenderer", &format!("{path}/MeshRenderer"))
            },
            filter: if has_mesh && !skinned { unique("MeshFilter", &format!("{path}/MeshFilter")) } else { 0 },
        };
        let name = if is_root { String::new() } else { node.element.name.to_string() };
        nodes.push(ModelNode {
            name,
            parent,
            position,
            rotation,
            scale,
            mesh: None,
            materials: vec![],
            visible: !settings.import_visibility || node.visible,
            ids,
        });
        ufbx_of.push(node);
        // Children pushed in reverse so they come out in the file's order.
        for child in node.children.iter().rev() {
            let child_path = format!("{path}/{}", child.element.name);
            stack.push((child, Some(index), child_path));
        }
    }

    let mut materials: Vec<ModelMaterial> = Vec::new();
    let mut material_index: HashMap<u32, usize> = HashMap::new();
    let mut meshes: Vec<ModelMesh> = Vec::new();
    let mut mesh_ids = HashMap::new();
    let mut triangles = 0usize;
    let node_index: HashMap<u32, usize> = ufbx_of.iter().enumerate().map(|(i, n)| (n.element.element_id, i)).collect();
    for i in 0..nodes.len() {
        let node = ufbx_of[i];
        let Some(mesh) = node.mesh.as_ref() else { continue };
        if mesh.num_triangles == 0 {
            continue;
        }
        limits.check("reading the models' meshes")?; // i18n: not shown (what a time-limit error says it was doing)
        triangles += mesh.num_triangles;
        anyhow::ensure!(triangles <= limits.max_model_triangles, "the model has more than {} triangles", limits.max_model_triangles);
        let (m, used) = mesh_of(node, mesh, settings, unit, &node_index)?;
        let slots: Vec<usize> = used
            .iter()
            .map(|&k| {
                let mat = node.materials.get(k as usize).or_else(|| mesh.materials.get(k as usize));
                match mat {
                    Some(mat) => *material_index.entry(mat.element.element_id).or_insert_with(|| {
                        materials.push(material_of(mat));
                        materials.len() - 1
                    }),
                    None => *material_index.entry(u32::MAX).or_insert_with(|| {
                        materials.push(ModelMaterial { name: "No Name".into(), color: [1.0; 4], texture: None }); // i18n: not shown (Unity ids and names of a model's parts)
                        materials.len() - 1
                    }),
                }
            })
            .collect();
        let name = node.element.name.to_string();
        let index = meshes.len();
        mesh_ids.entry(file_id("Mesh", &name, 0)).or_insert(index); // i18n: not shown (Unity ids and names of a model's parts)
        meshes.push(m);
        nodes[i].mesh = Some(index);
        nodes[i].materials = slots;
    }
    Ok(Model { nodes, meshes, materials, mesh_ids })
}

fn material_of(m: &ufbx::Material) -> ModelMaterial {
    let c = &m.fbx.diffuse_color;
    let f = if m.fbx.diffuse_factor.has_value { m.fbx.diffuse_factor.value_vec4.x as f32 } else { 1.0 };
    let base = if c.has_value { [c.value_vec4.x as f32 * f, c.value_vec4.y as f32 * f, c.value_vec4.z as f32 * f] } else { [1.0; 3] };
    let alpha = if m.fbx.transparency_factor.has_value { 1.0 - m.fbx.transparency_factor.value_vec4.x as f32 } else { 1.0 };
    let texture = [&m.fbx.diffuse_color, &m.pbr.base_color].iter().find_map(|map| map.texture.as_ref()).map(|t| {
        let name = if !t.relative_filename.is_empty() { t.relative_filename.to_string() } else { t.filename.to_string() };
        name.rsplit(['/', '\\']).next().unwrap_or(&name).to_string()
    });
    // FBX colours are linear-ish values Unity shows as they are; clamp what exporters overdrive.
    ModelMaterial {
        name: m.element.name.to_string(),
        color: [base[0].clamp(0.0, 1.0), base[1].clamp(0.0, 1.0), base[2].clamp(0.0, 1.0), alpha.clamp(0.0, 1.0)],
        texture: texture.filter(|t| !t.is_empty()),
    }
}

/// A node's mesh in Unity's space, split by material (the material indices used, in order).
///
/// One submesh a material, in the order the faces first use the materials, as in Unity's import, not in the order of the
/// file's material slots: a renderer's list of materials goes by submesh, and every prefab and scene overriding that list
/// was saved against Unity's order. Ultirex 4.1's body has the slots Body, Misc, and its first face is Misc's; sorted by
/// slot, the scene's Misc material landed on the 176,661 triangles of the body until Unity had imported the model.
fn mesh_of(
    node: &ufbx::Node,
    mesh: &ufbx::Mesh,
    settings: &ImportSettings,
    unit: f32,
    node_index: &HashMap<u32, usize>,
) -> anyhow::Result<(ModelMesh, Vec<u32>)> {
    let g2n = node.geometry_to_node;
    let normal_matrix = normal_matrix_t(&invert(&g2n));
    // Vertices are welded on (control point, normal, both UV sets), as Unity's importer does.
    let mut map: HashMap<(u32, [u32; 3], [u32; 2], [u32; 2]), u32> = HashMap::with_capacity(mesh.num_vertices * 2);
    let mut corner_point: Vec<u32> = Vec::new();
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut uv2 = Vec::new();
    let second = mesh.uv_sets.get(1).filter(|s| s.vertex_uv.exists);
    let has_normals = mesh.vertex_normal.exists && settings.normals != 1;
    let has_uv = mesh.vertex_uv.exists;
    let mut parts: Vec<(u32, Vec<u32>)> = Vec::new();
    let mut tri = vec![0u32; mesh.max_face_triangles.max(1) * 3];
    let material_of_face = |f: usize| -> u32 { mesh.face_material.get(f).copied().unwrap_or(0) };
    // Two slots holding one material are one submesh in Unity's import (`1 FBX.fbx` in Enzo's Zika project: 226 and 82
    // triangles of beat_head, one submesh of 308, so a list of materials saved against it would shift by one).
    let material_id = |k: u32| node.materials.get(k as usize).or_else(|| mesh.materials.get(k as usize)).map_or(u32::MAX, |m| m.element.element_id);
    let mut part_of: HashMap<u32, usize> = HashMap::new();
    for (f, face) in mesh.faces.iter().enumerate() {
        if face.num_indices < 3 {
            continue;
        }
        let n = mesh.triangulate_face(&mut tri, *face) as usize;
        let mat = material_of_face(f);
        let part = *part_of.entry(material_id(mat)).or_insert_with(|| {
            parts.push((mat, Vec::new()));
            parts.len() - 1
        });
        // Mirrored, a front face's corners run the other way round: Unity reverses each triangle.
        for corner in tri[..n * 3].chunks_exact(3).flat_map(|t| [t[0], t[2], t[1]]) {
            let ix = corner as usize;
            let point = mesh.vertex_indices[ix];
            let p = mesh.vertex_position[ix];
            let nrm = if has_normals { mesh.vertex_normal[ix] } else { ufbx::Vec3 { x: 0.0, y: 0.0, z: 0.0 } };
            let uv = if has_uv { mesh.vertex_uv[ix] } else { ufbx::Vec2 { x: 0.0, y: 0.0 } };
            let lit = second.map_or([0.0; 2], |s| {
                let t = s.vertex_uv[ix];
                [t.x as f32, t.y as f32]
            });
            let key = (point, [nrm.x as f32, nrm.y as f32, nrm.z as f32].map(f32::to_bits), [uv.x as f32, uv.y as f32].map(f32::to_bits), lit.map(f32::to_bits));
            let v = *map.entry(key).or_insert_with(|| {
                positions.push(pos(transform_point(&g2n, p), unit));
                let n = transform_dir(&normal_matrix, nrm);
                normals.push(normalize(dir(n)));
                uvs.push([uv.x as f32, uv.y as f32]);
                if second.is_some() {
                    uv2.push(lit);
                }
                corner_point.push(point);
                (positions.len() - 1) as u32
            });
            parts[part].1.push(v);
        }
    }
    if !has_normals || normals.iter().all(|n| *n == [0.0; 3]) {
        smooth_normals(&positions, &parts, &mut normals);
    }
    let used: Vec<u32> = parts.iter().map(|p| p.0).collect();
    let submeshes: Vec<Vec<u32>> = parts.into_iter().map(|p| p.1).collect();

    let skin = mesh.skin_deformers.first().map(|skin| {
        let mut bones = Vec::new();
        let mut names = Vec::new();
        let mut bind_poses = Vec::new();
        let n2g = invert(&g2n);
        for cluster in skin.clusters.iter() {
            let bone = cluster.bone_node.as_ref().and_then(|b| node_index.get(&b.element.element_id).copied()).unwrap_or(0);
            bones.push(bone);
            names.push(cluster.bone_node.as_ref().map(|b| b.element.name.to_string()).unwrap_or_default());
            // Mesh (node) space → bone space at bind: geometry_to_bone · node_to_geometry.
            bind_poses.push(unity_matrix(&mul(&cluster.geometry_to_bone, &n2g), unit));
        }
        // The bone nearest the root (Unity's first, its list follows the hierarchy).
        let root = (0..skin.clusters.len()).min_by_key(|&i| skin.clusters[i].bone_node.as_ref().map_or(u32::MAX, |b| b.node_depth)).unwrap_or(0) as u16;
        let weights = corner_point
            .iter()
            .map(|&p| {
                let mut w = [(root, 0.0f32); 4];
                w[0].1 = 1.0;
                if let Some(sv) = skin.vertices.get(p as usize).filter(|sv| (0..sv.num_weights).any(|k| skin.weights.get((sv.weight_begin + k) as usize).is_some_and(|sw| sw.weight > 0.0))) {
                    let mut list: Vec<(u16, f32)> = (0..sv.num_weights)
                        .filter_map(|k| skin.weights.get((sv.weight_begin + k) as usize))
                        .map(|sw| (sw.cluster_index as u16, sw.weight as f32))
                        .collect();
                    list.sort_by(|a, b| b.1.total_cmp(&a.1));
                    let sum: f32 = list.iter().take(4).map(|x| x.1).sum();
                    w = [(0, 0.0); 4];
                    for (k, (b, wt)) in list.into_iter().take(4).enumerate() {
                        w[k] = (b, if sum > 0.0 { wt / sum } else { 0.0 });
                    }
                }
                w
            })
            .collect();
        Skin { bones, names, bind_poses, weights }
    });

    let mut blend_shapes = Vec::new();
    if settings.import_blend_shapes {
        // Each control point's vertices, to spread a shape's offsets over the welded vertices.
        let mut first: HashMap<u32, Vec<u32>> = HashMap::new();
        for (v, &p) in corner_point.iter().enumerate() {
            first.entry(p).or_default().push(v as u32);
        }
        for deformer in mesh.blend_deformers.iter() {
            for channel in deformer.channels.iter() {
                let mut frames = Vec::new();
                for key in channel.keyframes.iter() {
                    let shape = &key.shape;
                    let mut offsets = Vec::with_capacity(shape.num_offsets);
                    for (k, &point) in shape.offset_vertices.iter().enumerate() {
                        let Some(d) = shape.position_offsets.get(k) else { continue };
                        let d = transform_dir(&g2n, *d);
                        let d = [-(d.x as f32) * unit, d.y as f32 * unit, d.z as f32 * unit];
                        if d == [0.0; 3] {
                            continue;
                        }
                        for &v in first.get(&point).map(Vec::as_slice).unwrap_or(&[]) {
                            offsets.push((v, d));
                        }
                    }
                    frames.push((key.target_weight as f32 * 100.0, offsets));
                }
                frames.sort_by(|a, b| a.0.total_cmp(&b.0));
                blend_shapes.push(BlendShape { name: channel.element.name.to_string(), frames, default_weight: 0.0 });
            }
        }
    }
    Ok((ModelMesh { name: node.element.name.to_string(), positions, normals, uvs, uv2, submeshes, skin, blend_shapes }, used))
}

/// The transpose of a matrix's linear part (normals go through the inverse transpose).
fn normal_matrix_t(inv: &ufbx::Matrix) -> ufbx::Matrix {
    ufbx::Matrix {
        m00: inv.m00,
        m01: inv.m10,
        m02: inv.m20,
        m03: 0.0,
        m10: inv.m01,
        m11: inv.m11,
        m12: inv.m21,
        m13: 0.0,
        m20: inv.m02,
        m21: inv.m12,
        m22: inv.m22,
        m23: 0.0,
    }
}

pub fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-20 {
        v.map(|c| c / l)
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// Area-weighted normals shared by vertices at the same place (a model without normals, or Unity told to calculate
/// them).
fn smooth_normals(positions: &[[f32; 3]], parts: &[(u32, Vec<u32>)], normals: &mut [[f32; 3]]) {
    let mut by_place: HashMap<[u32; 3], [f32; 3]> = HashMap::new();
    for (_, idx) in parts {
        for t in idx.chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| positions[i as usize]);
            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            // A clockwise front face in Unity's left-handed space: its normal is e1 × e2.
            let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
            for i in [t[0], t[1], t[2]] {
                let e = by_place.entry(positions[i as usize].map(f32::to_bits)).or_insert([0.0; 3]);
                for k in 0..3 {
                    e[k] += n[k];
                }
            }
        }
    }
    for (i, p) in positions.iter().enumerate() {
        normals[i] = normalize(by_place.get(&p.map(f32::to_bits)).copied().unwrap_or([0.0, 1.0, 0.0]));
    }
}

/// xxHash64 (Yann Collet's), the hash Unity names imported objects with.
pub fn xxh64(data: &[u8], seed: u64) -> u64 {
    const P1: u64 = 11400714785074694791;
    const P2: u64 = 14029467366897019727;
    const P3: u64 = 1609587929392839161;
    const P4: u64 = 9650029242287828579;
    const P5: u64 = 2870177450012600261;
    let round = |acc: u64, v: u64| acc.wrapping_add(v.wrapping_mul(P2)).rotate_left(31).wrapping_mul(P1);
    let merge = |acc: u64, v: u64| (acc ^ round(0, v)).wrapping_mul(P1).wrapping_add(P4);
    let u64_at = |i: usize| u64::from_le_bytes(data[i..i + 8].try_into().unwrap());
    let u32_at = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().unwrap()) as u64;
    let n = data.len();
    let mut i = 0;
    let mut h = if n >= 32 {
        let mut v = [seed.wrapping_add(P1).wrapping_add(P2), seed.wrapping_add(P2), seed, seed.wrapping_sub(P1)];
        while i + 32 <= n {
            for (k, acc) in v.iter_mut().enumerate() {
                *acc = round(*acc, u64_at(i + 8 * k));
            }
            i += 32;
        }
        let mut h = v[0].rotate_left(1).wrapping_add(v[1].rotate_left(7)).wrapping_add(v[2].rotate_left(12)).wrapping_add(v[3].rotate_left(18));
        for acc in v {
            h = merge(h, acc);
        }
        h
    } else {
        seed.wrapping_add(P5)
    };
    h = h.wrapping_add(n as u64);
    while i + 8 <= n {
        h ^= round(0, u64_at(i));
        h = h.rotate_left(27).wrapping_mul(P1).wrapping_add(P4);
        i += 8;
    }
    if i + 4 <= n {
        h ^= u32_at(i).wrapping_mul(P1);
        h = h.rotate_left(23).wrapping_mul(P2).wrapping_add(P3);
        i += 4;
    }
    while i < n {
        h ^= (data[i] as u64).wrapping_mul(P5);
        h = h.rotate_left(11).wrapping_mul(P1);
        i += 1;
    }
    h ^= h >> 33;
    h = h.wrapping_mul(P2);
    h ^= h >> 29;
    h = h.wrapping_mul(P3);
    h ^= h >> 32;
    h
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tiny ASCII FBX: a two-node rig (`Armature`, its child `Bone` one metre up) and a quad skinned to the bone,
    /// in centimetres (UnitScaleFactor 1), with one red material; and a second, unskinned triangle.
    pub(crate) const RIG: &str = r#"; FBX 7.4.0 project file
FBXHeaderExtension:  {
	FBXHeaderVersion: 1003
	FBXVersion: 7400
}
GlobalSettings:  {
	Version: 1000
	Properties70:  {
		P: "UpAxis", "int", "Integer", "",1
		P: "UpAxisSign", "int", "Integer", "",1
		P: "FrontAxis", "int", "Integer", "",2
		P: "FrontAxisSign", "int", "Integer", "",1
		P: "CoordAxis", "int", "Integer", "",0
		P: "CoordAxisSign", "int", "Integer", "",1
		P: "UnitScaleFactor", "double", "Number", "",1
	}
}
Objects:  {
	Geometry: 1000, "Geometry::Quad", "Mesh" {
		Vertices: *12 {
			a: 0,0,0,100,0,0,100,100,0,0,100,0
		}
		PolygonVertexIndex: *4 {
			a: 0,1,2,-4
		}
		LayerElementNormal: 0 {
			MappingInformationType: "ByControlPoint"
			ReferenceInformationType: "Direct"
			Normals: *12 {
				a: 0,0,1,0,0,1,0,0,1,0,0,1
			}
		}
		LayerElementUV: 0 {
			MappingInformationType: "ByControlPoint"
			ReferenceInformationType: "Direct"
			UV: *8 {
				a: 0,0,1,0,1,1,0,1
			}
		}
		LayerElementMaterial: 0 {
			MappingInformationType: "AllSame"
			ReferenceInformationType: "IndexToDirect"
			Materials: *1 {
				a: 0
			}
		}
		Layer: 0 {
			LayerElement:  {
				Type: "LayerElementNormal"
				TypedIndex: 0
			}
			LayerElement:  {
				Type: "LayerElementUV"
				TypedIndex: 0
			}
			LayerElement:  {
				Type: "LayerElementMaterial"
				TypedIndex: 0
			}
		}
	}
	Geometry: 1001, "Geometry::Tri", "Mesh" {
		Vertices: *9 {
			a: 0,0,0,10,0,0,0,10,0
		}
		PolygonVertexIndex: *3 {
			a: 0,1,-3
		}
	}
	Model: 2000, "Model::Armature", "Null" {
		Version: 232
	}
	Model: 2001, "Model::Bone", "LimbNode" {
		Version: 232
		Properties70:  {
			P: "Lcl Translation", "Lcl Translation", "", "A",0,100,0
		}
	}
	Model: 2002, "Model::Body", "Mesh" {
		Version: 232
	}
	Model: 2003, "Model::Prop", "Mesh" {
		Version: 232
		Properties70:  {
			P: "Lcl Translation", "Lcl Translation", "", "A",200,0,0
		}
	}
	Material: 3000, "Material::Red", "" {
		Version: 102
		ShadingModel: "phong"
		Properties70:  {
			P: "DiffuseColor", "Color", "", "A",1,0,0
		}
	}
	Deformer: 4000, "Deformer::Skin", "Skin" {
		Version: 101
	}
	Deformer: 4001, "SubDeformer::Bone", "Cluster" {
		Version: 100
		Indexes: *4 {
			a: 0,1,2,3
		}
		Weights: *4 {
			a: 1,1,1,1
		}
		Transform: *16 {
			a: 1,0,0,0,0,1,0,0,0,0,1,0,0,-100,0,1
		}
		TransformLink: *16 {
			a: 1,0,0,0,0,1,0,0,0,0,1,0,0,100,0,1
		}
	}
}
Connections:  {
	C: "OO",2000,0
	C: "OO",2001,2000
	C: "OO",2002,0
	C: "OO",2003,0
	C: "OO",1000,2002
	C: "OO",1001,2003
	C: "OO",3000,2002
	C: "OO",4000,1000
	C: "OO",4001,4000
	C: "OO",2001,4001
}
"#;

    pub(crate) fn rig_file(dir: &Path) -> std::path::PathBuf {
        let p = dir.join("Rig.fbx");
        std::fs::write(&p, RIG).unwrap();
        p
    }

    /// The rig with its cluster naming only some of the mesh's vertices.
    pub(crate) fn rig_file_with_weights(dir: &Path, indexes: &str, weights: &str) -> std::path::PathBuf {
        let n = indexes.split(',').count();
        let text = String::from_utf8_lossy(RIG.as_bytes()).replace("Indexes: *4 {\n\t\t\ta: 0,1,2,3", &format!("Indexes: *{n} {{\n\t\t\ta: {indexes}")).replace("Weights: *4 {\n\t\t\ta: 1,1,1,1", &format!("Weights: *{n} {{\n\t\t\ta: {weights}"));
        assert!(text.contains(&format!("a: {indexes}\n")), "the rig's cluster changed");
        let p = dir.join("RigPart.fbx");
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn unity_names_a_models_objects_by_their_place() {
        // Every model's root (seen in thousands of prefabs).
        assert_eq!(file_id("GameObject", "//RootNode/root", 0), 919132149155446097);
        assert_eq!(file_id("Transform", "//RootNode/root/Transform", 0), -8679921383154817045);
        assert_eq!(xxh64(b"", 0), 0xef46db3751d8e999);
        assert_eq!(xxh64(b"Nobody inspects the spammish repetition", 0), 0xfbcea83c8a378bf1);
    }

    #[test]
    fn a_model_reads_in_unitys_space_with_its_ids() {
        let dir = tempfile::tempdir().unwrap();
        let m = load(&rig_file(dir.path()), &ImportSettings::default(), &Limits::default()).unwrap();
        let names: Vec<&str> = m.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["", "Armature", "Bone", "Body", "Prop"]);
        let bone = &m.nodes[2];
        assert_eq!(bone.parent, Some(1));
        // Centimetres to metres on translations.
        assert!((bone.position[1] - 1.0).abs() < 1e-5, "{:?}", bone.position);
        assert_eq!(bone.ids.game_object, file_id("GameObject", "//RootNode/root/Armature/Bone", 0));
        assert_eq!(bone.ids.transform, file_id("Transform", "//RootNode/root/Armature/Bone/Transform", 0));
        let body = &m.nodes[3];
        assert_eq!(body.ids.renderer, file_id("SkinnedMeshRenderer", "//RootNode/root/Body/SkinnedMeshRenderer", 0));
        let prop = &m.nodes[4];
        assert_eq!(prop.ids.renderer, file_id("MeshRenderer", "//RootNode/root/Prop/MeshRenderer", 0));
        assert_eq!(prop.ids.filter, file_id("MeshFilter", "//RootNode/root/Prop/MeshFilter", 0));
        // X mirrored.
        assert!((prop.position[0] + 2.0).abs() < 1e-5, "{:?}", prop.position);
        let quad = &m.meshes[body.mesh.unwrap()];
        assert_eq!(quad.triangles(), 2);
        assert!(quad.positions.iter().any(|p| (p[0] + 1.0).abs() < 1e-5 && (p[1] - 1.0).abs() < 1e-5), "{:?}", quad.positions);
        // The quad faced +Z in the file; mirrored it still does, wound as Unity's front faces.
        assert!(quad.normals.iter().all(|n| (n[2] - 1.0).abs() < 1e-4), "{:?}", quad.normals);
        for t in quad.submeshes[0].chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| quad.positions[i as usize]);
            let (e1, e2) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
            assert!(e1[0] * e2[1] - e1[1] * e2[0] > 0.0, "clockwise seen from +Z in Unity's space");
        }
        assert_eq!(m.materials[body.materials[0]].name, "Red");
        assert_eq!(m.materials[body.materials[0]].color[..3], [1.0, 0.0, 0.0]);
        let skin = quad.skin.as_ref().unwrap();
        assert_eq!(skin.bones, [2]);
        // The bind pose takes the mesh's space into the bone's: one metre down.
        assert!((skin.bind_poses[0][13] + 1.0).abs() < 1e-4, "{:?}", skin.bind_poses[0]);
        assert!(skin.weights.iter().all(|w| w[0] == (0, 1.0)));
        assert_eq!(m.mesh_ids.get(&file_id("Mesh", "Body", 0)), Some(&body.mesh.unwrap()));
    }

    /// The shape of Ultirex 4.1's body, small: the slots Body then Misc, the first face Misc's and the two after it Body's.
    /// One top node, merged into the model's root, with a plain MeshRenderer.
    pub(crate) const FIRST_FACE_MISC: &str = r#"; FBX 7.4.0 project file
FBXHeaderExtension:  {
	FBXHeaderVersion: 1003
	FBXVersion: 7400
}
GlobalSettings:  {
	Version: 1000
	Properties70:  {
		P: "UnitScaleFactor", "double", "Number", "",1
	}
}
Objects:  {
	Geometry: 1000, "Geometry::Body", "Mesh" {
		Vertices: *27 {
			a: 0,0,0,100,0,0,0,100,0,200,0,0,300,0,0,200,100,0,400,0,0,500,0,0,400,100,0
		}
		PolygonVertexIndex: *9 {
			a: 0,1,-3,3,4,-6,6,7,-9
		}
		LayerElementMaterial: 0 {
			MappingInformationType: "ByPolygon"
			ReferenceInformationType: "IndexToDirect"
			Materials: *3 {
				a: 1,0,0
			}
		}
		Layer: 0 {
			LayerElement:  {
				Type: "LayerElementMaterial"
				TypedIndex: 0
			}
		}
	}
	Model: 2000, "Model::Body", "Mesh" {
		Version: 232
	}
	Material: 3000, "Material::BodyMatt", "" {
		Version: 102
		ShadingModel: "phong"
	}
	Material: 3001, "Material::MiscMatt", "" {
		Version: 102
		ShadingModel: "phong"
	}
}
Connections:  {
	C: "OO",2000,0
	C: "OO",1000,2000
	C: "OO",3000,2000
	C: "OO",3001,2000
}
"#;

    #[test]
    fn two_slots_holding_one_material_are_one_submesh() {
        let text = FIRST_FACE_MISC.replace("a: 1,0,0", "a: 1,2,0").replace("\tC: \"OO\",3001,2000\n", "\tC: \"OO\",3001,2000\n\tC: \"OO\",3000,2000\n");
        assert_ne!(text, FIRST_FACE_MISC);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("Body.fbx");
        std::fs::write(&p, text).unwrap();
        let m = load(&p, &ImportSettings::default(), &Limits::default()).unwrap();
        let node = m.nodes.iter().find(|n| n.mesh.is_some()).unwrap();
        let names: Vec<&str> = node.materials.iter().map(|&k| m.materials[k].name.as_str()).collect();
        assert_eq!(names, ["MiscMatt", "BodyMatt"]);
        assert_eq!(m.meshes[node.mesh.unwrap()].submeshes.iter().map(|s| s.len() / 3).collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn an_older_imports_ids_of_a_models_materials_are_read_from_its_meta() {
        let s = ImportSettings::read("ModelImporter:\n  fileIDToRecycleName:\n    100000: Root\n    2100000: Skin\n    2100002: Hair\n");
        assert_eq!(s.material_ids.get(&2100002).map(String::as_str), Some("Hair"));
        assert_eq!(s.material_ids.len(), 2, "materials only");
        let s = ImportSettings::read("ModelImporter:\n  internalIDToNameTable:\n  - first:\n      21: 2100004\n    second: Eyes\n  - first:\n      43: 4300000\n    second: Body\n");
        assert_eq!(s.material_ids.get(&2100004).map(String::as_str), Some("Eyes"));
        assert_eq!(s.material_ids.len(), 1, "materials only");
    }

    #[test]
    fn submeshes_come_in_the_order_the_faces_first_use_them() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("Body.fbx");
        std::fs::write(&p, FIRST_FACE_MISC).unwrap();
        let m = load(&p, &ImportSettings::default(), &Limits::default()).unwrap();
        let node = m.nodes.iter().find(|n| n.mesh.is_some()).unwrap();
        let names: Vec<&str> = node.materials.iter().map(|&k| m.materials[k].name.as_str()).collect();
        assert_eq!(names, ["MiscMatt", "BodyMatt"], "Unity's order, what a renderer's list of materials goes by");
        let mesh = &m.meshes[node.mesh.unwrap()];
        assert_eq!(mesh.submeshes.iter().map(|s| s.len() / 3).collect::<Vec<_>>(), [1, 2]);
        // Each submesh still has its own geometry: the first the face within 1 m of the origin (mirrored), the other the rest.
        assert!(mesh.submeshes[0].iter().all(|&i| mesh.positions[i as usize][0] >= -1.0), "{:?}", mesh.positions);
        assert!(mesh.submeshes[1].iter().all(|&i| mesh.positions[i as usize][0] <= -2.0), "{:?}", mesh.positions);
    }

    #[test]
    fn a_broken_model_is_an_error_not_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("Broken.fbx");
        std::fs::write(&p, b"Kaydara FBX Binary  \x00\x1a\x00garbage").unwrap();
        assert!(load(&p, &ImportSettings::default(), &Limits::default()).is_err());
        std::fs::write(&p, &RIG[..RIG.len() / 2]).unwrap();
        let _ = load(&p, &ImportSettings::default(), &Limits::default());
    }
}
