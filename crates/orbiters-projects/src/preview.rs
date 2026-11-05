//! Project previews (contracts' Docs/project-preview.md): the small ready-to-draw copy of a Unity project's last saved
//! scene that Orbiters Toolkit keeps under `UserSettings/Orbiters/Previews`. A fixed start, a JSON header, then blobs
//! whose layout is the GPU's (20-byte vertices, `u16`/`u32` indices, column-major matrices) and JPEG/PNG pictures.
//! Reading one is a single file read and a header parse; nothing is converted.

use anyhow::{bail, ensure, Context as _};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Where a project keeps its previews, from the project's folder.
pub const FOLDER: &str = "UserSettings/Orbiters/Previews";
pub const EXTENSION: &str = "preview";
pub const MAGIC: &[u8; 8] = b"N4N0PRV\0";
/// The start of previews written before Orbiters Toolkit 0.4.11 (same layout): still read.
const LEGACY_MAGIC: &[u8; 8] = b"ORBPREV\0";
/// The schema written: 2 added the materials' look (shading, metallic, smoothness, normal, surface and emission maps)
/// and linear pictures.
pub const SCHEMA: u32 = 2;
/// The oldest schema still read (its materials draw with the defaults of the fields 2 added).
pub const OLDEST_SCHEMA: u32 = 1;
/// Position (3 × f32), octahedral normal (2 × snorm16), UV (2 × f16).
pub const VERTEX_SIZE: usize = 20;
/// Lightmap UV (2 × unorm16), in a stream of its own.
pub const LIGHTMAP_UV_SIZE: usize = 4;
const START: usize = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blob {
    pub offset: u64,
    pub length: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Default for Bounds {
    fn default() -> Self {
        Self { min: [0.0; 3], max: [0.0; 3] }
    }
}

impl Bounds {
    pub fn center(&self) -> [f32; 3] {
        [0, 1, 2].map(|k| (self.min[k] + self.max[k]) * 0.5)
    }

    pub fn size(&self) -> [f32; 3] {
        [0, 1, 2].map(|k| (self.max[k] - self.min[k]).max(0.0))
    }

    /// Half the diagonal.
    pub fn radius(&self) -> f32 {
        let s = self.size();
        0.5 * (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Avatars,
    World,
    #[default]
    Scene,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Scene {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub guid: String,
}

/// The last active Scene view's camera.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneCamera {
    pub position: [f32; 3],
    /// x, y, z, w.
    pub rotation: [f32; 4],
    pub pivot: [f32; 3],
    #[serde(default = "sixty")]
    pub fov_y: f32,
}

fn sixty() -> f32 {
    60.0
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AvatarPlace {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub id: Option<String>,
    pub position: [f32; 3],
    pub forward: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Jpeg,
    Png,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub format: ImageFormat,
    /// Data stored as they are (a normal or metallic-smoothness picture), not sRGB colours.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub linear: bool,
    pub data: Blob,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Alpha {
    #[default]
    Opaque,
    Cutout,
    Blend,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cull {
    #[default]
    Back,
    Front,
    Off,
}

/// How a material takes the light (schema 2; a schema 1 material is `Lit`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shading {
    /// Unity's Standard: Lambert and a GGX highlight by metallic and smoothness.
    #[default]
    Lit,
    /// Two bands, the shadowed one tinted by `shade`.
    Toon,
    /// Lit evenly whichever way it faces.
    Flat,
    /// The colour as it is.
    Unlit,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Material {
    #[serde(default)]
    pub name: String,
    /// sRGB, straight alpha.
    #[serde(default = "white")]
    pub color: [f32; 4],
    /// An index into the textures, or -1.
    #[serde(default = "none")]
    pub texture: i32,
    #[serde(default = "ones")]
    pub tiling: [f32; 2],
    #[serde(default)]
    pub offset: [f32; 2],
    #[serde(default)]
    pub alpha: Alpha,
    #[serde(default = "half")]
    pub cutoff: f32,
    #[serde(default)]
    pub cull: Cull,
    #[serde(default)]
    pub shading: Shading,
    /// Linear, as the shader computes with it; times `surface_texture`'s R.
    #[serde(default)]
    pub metallic: f32,
    /// Perceptual (Unity's slider); times `surface_texture`'s G. Missing (schema 1): a little, most avatars are matte.
    #[serde(default = "default_smoothness")]
    pub smoothness: f32,
    /// A linear picture: R metallic, G smoothness; or -1.
    #[serde(default = "none")]
    pub surface_texture: i32,
    /// A linear tangent-space normal picture, or -1.
    #[serde(default = "none")]
    pub normal_texture: i32,
    #[serde(default = "one")]
    pub normal_scale: f32,
    /// Linear RGB, HDR.
    #[serde(default)]
    pub emission: [f32; 3],
    #[serde(default = "none")]
    pub emission_texture: i32,
    /// Toon: the shadowed band's tint (linear RGB, multiplied), where it starts on the half-Lambert term, how soft.
    #[serde(default = "default_shade")]
    pub shade: [f32; 3],
    #[serde(default = "half")]
    pub shade_border: f32,
    #[serde(default = "default_blur")]
    pub shade_blur: f32,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            name: String::new(),
            color: white(),
            texture: -1,
            tiling: ones(),
            offset: [0.0; 2],
            alpha: Alpha::Opaque,
            cutoff: 0.5,
            cull: Cull::Back,
            shading: Shading::Lit,
            metallic: 0.0,
            smoothness: default_smoothness(),
            surface_texture: -1,
            normal_texture: -1,
            normal_scale: 1.0,
            emission: [0.0; 3],
            emission_texture: -1,
            shade: default_shade(),
            shade_border: 0.5,
            shade_blur: default_blur(),
        }
    }
}

fn default_smoothness() -> f32 {
    0.2
}
fn default_shade() -> [f32; 3] {
    [0.7; 3]
}
fn default_blur() -> f32 {
    0.1
}
fn one() -> f32 {
    1.0
}

fn white() -> [f32; 4] {
    [1.0; 4]
}
fn none() -> i32 {
    -1
}
fn ones() -> [f32; 2] {
    [1.0; 2]
}
fn half() -> f32 {
    0.5
}

/// A directional light of the scene.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneLight {
    /// The way the light travels (world space).
    pub direction: [f32; 3],
    /// Linear RGB as shaders receive it, the intensity included.
    pub color: [f32; 3],
    #[serde(default = "one")]
    pub intensity: f32,
    #[serde(default)]
    pub shadows: bool,
    #[serde(default = "one")]
    pub shadow_strength: f32,
    /// Baked: what samples a lightmap has this light in it already, what does not still gets it (as Unity's light
    /// probes would give it).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub baked: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AmbientMode {
    #[default]
    Skybox,
    Trilight,
    Flat,
}

/// The scene's ambient light as shaders receive it (linear RGB): facing up, round the horizon, facing down.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ambient {
    #[serde(default)]
    pub mode: AmbientMode,
    pub sky: [f32; 3],
    pub equator: [f32; 3],
    pub ground: [f32; 3],
}

/// The light the scene gives shaders in Unity (optional within schema 2; previews without it are drawn in the
/// desktop's studio light).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lighting {
    #[serde(default)]
    pub lights: Vec<SceneLight>,
    #[serde(default)]
    pub ambient: Option<Ambient>,
    #[serde(default = "one")]
    pub reflection_intensity: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fog: Option<Fog>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FogMode {
    #[default]
    Linear,
    Exponential,
    Squared,
}

/// The scene's fog, as Unity's built-in shaders add it (colour linear).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fog {
    #[serde(default)]
    pub mode: FogMode,
    pub color: [f32; 3],
    #[serde(default)]
    pub start: f32,
    #[serde(default)]
    pub end: f32,
    #[serde(default)]
    pub density: f32,
}

/// A baked lightmap: a picture whose colours, read as sRGB, are the light divided by `scale`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lightmap {
    pub texture: u32,
    pub scale: f32,
}

/// The lightmap a draw samples.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrawLightmap {
    /// An index into the header's `lightmaps`.
    pub index: u32,
    /// `uv2 * [x, y] + [z, w]`: where in the lightmap, as Unity's `m_LightmapTilingOffset`.
    pub st: [f32; 4],
}

/// What is behind the scene: its skybox (a latitude-longitude picture, or colours when only those could be read).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sky {
    /// A picture of the whole sphere, or -1: rows from straight up to straight down, its colours read as sRGB and
    /// multiplied by `scale` (linear).
    #[serde(default = "none")]
    pub texture: i32,
    #[serde(default = "one")]
    pub scale: f32,
    /// Degrees it is turned round the vertical, as Unity's `_Rotation`.
    #[serde(default)]
    pub rotation: f32,
    /// Linear colours straight up, at the horizon, straight down: drawn without a picture.
    pub top: [f32; 3],
    pub horizon: [f32; 3],
    pub bottom: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IndexFormat {
    U16,
    U32,
}

impl IndexFormat {
    pub fn size(self) -> usize {
        match self {
            IndexFormat::U16 => 2,
            IndexFormat::U32 => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Submesh {
    pub start: u32,
    pub count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mesh {
    pub vertices: u32,
    pub data: Blob,
    pub index_format: IndexFormat,
    pub indices: Blob,
    pub submeshes: Vec<Submesh>,
    #[serde(default)]
    pub bounds: Bounds,
    /// The second UV set (lightmaps'): two `unorm16` a vertex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lightmap_uv: Option<Blob>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Instances {
    pub count: u32,
    pub matrices: Blob,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Draw {
    pub instance: u32,
    pub mesh: u32,
    /// One per submesh: an index into the materials, or -1 (not drawn).
    pub materials: Vec<i32>,
    #[serde(default = "yes")]
    pub shadows: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lightmap: Option<DrawLightmap>,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Stats {
    #[serde(default)]
    pub renderers: u32,
    #[serde(default)]
    pub triangles: u64,
    #[serde(default)]
    pub vertices: u64,
    #[serde(default)]
    pub skipped: std::collections::BTreeMap<String, u32>,
}

/// What the capture cost the editor (diagnostics).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    #[serde(default)]
    pub main_thread_ms: f32,
    #[serde(default)]
    pub steps: u32,
    #[serde(default)]
    pub longest_step_ms: f32,
    #[serde(default)]
    pub encode_ms: f32,
    #[serde(default)]
    pub total_ms: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    #[serde(default)]
    pub written_utc: String,
    #[serde(default)]
    pub generator: String,
    #[serde(default)]
    pub unity: String,
    #[serde(default)]
    pub scene: Scene,
    #[serde(default)]
    pub kind: Kind,
    pub bounds: Bounds,
    pub focus: Bounds,
    #[serde(default)]
    pub ground: Option<f32>,
    #[serde(default)]
    pub camera: Option<SceneCamera>,
    #[serde(default)]
    pub avatars: Vec<AvatarPlace>,
    #[serde(default)]
    pub textures: Vec<Texture>,
    #[serde(default)]
    pub materials: Vec<Material>,
    #[serde(default)]
    pub meshes: Vec<Mesh>,
    pub instances: Instances,
    #[serde(default)]
    pub draws: Vec<Draw>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lighting: Option<Lighting>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lightmaps: Vec<Lightmap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sky: Option<Sky>,
    #[serde(default)]
    pub stats: Stats,
    #[serde(default)]
    pub cost: Option<Cost>,
}

/// A preview read whole: its header and the bytes its blobs point into.
pub struct Preview {
    pub header: Header,
    bytes: Vec<u8>,
    data: usize,
}

impl Preview {
    /// A blob's bytes (checked when read).
    pub fn blob(&self, b: Blob) -> &[u8] {
        let start = self.data + b.offset as usize;
        &self.bytes[start..start + b.length as usize]
    }

    /// Instance `i`'s local-to-world matrix, column-major.
    pub fn matrix(&self, i: u32) -> [f32; 16] {
        let m = self.blob(self.header.instances.matrices);
        let at = i as usize * 64;
        std::array::from_fn(|k| f32::from_le_bytes(m[at + k * 4..at + k * 4 + 4].try_into().unwrap()))
    }

    /// The file's size.
    pub fn size(&self) -> usize {
        self.bytes.len()
    }
}

/// Reads and checks a preview file.
pub fn read(path: &Path) -> anyhow::Result<Preview> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    parse(bytes)
}

/// Checks `bytes` as a preview: its start, its header, and that every blob lies in the file and fits what uses it.
pub fn parse(bytes: Vec<u8>) -> anyhow::Result<Preview> {
    let (header, data) = parse_header(&bytes)?;
    let preview = Preview { header, bytes, data };
    check(&preview)?;
    Ok(preview)
}

fn parse_header(bytes: &[u8]) -> anyhow::Result<(Header, usize)> {
    ensure!(bytes.len() >= START && (&bytes[..8] == MAGIC || &bytes[..8] == LEGACY_MAGIC), "not an Orbiters project preview");
    let schema = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    ensure!((OLDEST_SCHEMA..=SCHEMA).contains(&schema), "a project preview of schema {schema}, this version reads {OLDEST_SCHEMA} to {SCHEMA}");
    let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    ensure!(START + len <= bytes.len(), "the preview's header runs past its end");
    let header: Header = serde_json::from_slice(&bytes[START..START + len]).context("the preview's header")?;
    Ok((header, align(START + len)))
}

fn check(p: &Preview) -> anyhow::Result<()> {
    let h = &p.header;
    let room = p.bytes.len().saturating_sub(p.data) as u64;
    let inside = |b: Blob, what: &str| -> anyhow::Result<()> {
        ensure!(b.offset.checked_add(b.length).is_some_and(|end| end <= room), "the preview's {what} lies outside the file");
        Ok(())
    };
    for (i, t) in h.textures.iter().enumerate() {
        inside(t.data, &format!("texture {i}"))?;
    }
    for (i, m) in h.meshes.iter().enumerate() {
        inside(m.data, &format!("mesh {i}"))?;
        inside(m.indices, &format!("mesh {i}'s indices"))?;
        ensure!(m.data.length == m.vertices as u64 * VERTEX_SIZE as u64, "mesh {i} has {} vertices in {} bytes", m.vertices, m.data.length);
        let count = m.indices.length / m.index_format.size() as u64;
        ensure!(m.indices.length % m.index_format.size() as u64 == 0, "mesh {i}'s indices are cut short");
        if let Some(uv) = m.lightmap_uv {
            inside(uv, &format!("mesh {i}'s lightmap UVs"))?;
            ensure!(uv.length == m.vertices as u64 * LIGHTMAP_UV_SIZE as u64, "mesh {i} has {} vertices in {} bytes of lightmap UVs", m.vertices, uv.length);
        }
        for s in &m.submeshes {
            ensure!(s.start as u64 + s.count as u64 <= count && s.count % 3 == 0, "mesh {i} has a submesh outside its indices");
        }
    }
    inside(h.instances.matrices, "matrices")?;
    ensure!(h.instances.matrices.length == h.instances.count as u64 * 64, "{} matrices in {} bytes", h.instances.count, h.instances.matrices.length);
    for d in &h.draws {
        ensure!(d.instance < h.instances.count, "a draw of instance {} of {}", d.instance, h.instances.count);
        let Some(mesh) = h.meshes.get(d.mesh as usize) else { bail!("a draw of mesh {} of {}", d.mesh, h.meshes.len()) };
        ensure!(d.materials.len() <= mesh.submeshes.len(), "a draw with more materials than its mesh has submeshes");
        ensure!(d.materials.iter().all(|&m| m < h.materials.len() as i32), "a draw of a material the preview does not have");
        if let Some(l) = d.lightmap {
            ensure!((l.index as usize) < h.lightmaps.len(), "a draw of lightmap {} of {}", l.index, h.lightmaps.len());
        }
    }
    for (i, m) in h.materials.iter().enumerate() {
        for t in [m.texture, m.surface_texture, m.normal_texture, m.emission_texture] {
            ensure!(t < h.textures.len() as i32, "material {i} uses a texture the preview does not have");
        }
    }
    for l in &h.lightmaps {
        ensure!((l.texture as usize) < h.textures.len(), "a lightmap uses a texture the preview does not have");
    }
    if let Some(s) = h.sky {
        ensure!(s.texture < h.textures.len() as i32, "the sky uses a texture the preview does not have");
    }
    Ok(())
}

fn align(n: usize) -> usize {
    n.div_ceil(16) * 16
}

/// A project's preview file: which scene, when written.
#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub modified: SystemTime,
    pub size: u64,
    pub header: Header,
}

/// A project's previews (one per scene), the most recently written first. Only their headers are read.
pub fn list(project: &Path) -> Vec<Entry> {
    let Ok(dir) = std::fs::read_dir(project.join(FOLDER)) else { return vec![] };
    let mut out: Vec<Entry> = dir
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == EXTENSION))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            let header = read_header(&e.path()).ok()?;
            Some(Entry { path: e.path(), modified: meta.modified().ok()?, size: meta.len(), header })
        })
        .collect();
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

/// Only the header of a preview file.
pub fn read_header(path: &Path) -> anyhow::Result<Header> {
    use std::io::Read as _;
    let mut f = std::fs::File::open(path)?;
    let mut start = [0u8; START];
    f.read_exact(&mut start)?;
    let len = u32::from_le_bytes(start[12..16].try_into().unwrap()) as usize;
    ensure!(len < 64 << 20, "the preview's header is too large");
    let mut bytes = start.to_vec();
    bytes.resize(START + len, 0);
    f.read_exact(&mut bytes[START..])?;
    Ok(parse_header(&bytes)?.0)
}

/// Builds preview files (tests, stand-ins for screenshots; Toolkit writes the real ones).
#[derive(Default)]
pub struct Writer {
    data: Vec<u8>,
}

impl Writer {
    /// Appends a blob (aligned to 16 bytes).
    pub fn push(&mut self, bytes: &[u8]) -> Blob {
        let offset = self.data.len() as u64;
        self.data.extend_from_slice(bytes);
        self.data.resize(align(self.data.len()), 0);
        Blob { offset, length: bytes.len() as u64 }
    }

    pub fn finish(self, header: &Header) -> Vec<u8> {
        let json = serde_json::to_vec(header).expect("header");
        let mut out = Vec::with_capacity(align(START + json.len()) + self.data.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&SCHEMA.to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(&json);
        out.resize(align(out.len()), 0);
        out.extend_from_slice(&self.data);
        out
    }
}

/// One vertex as the file stores it.
pub fn vertex(position: [f32; 3], normal: [f32; 3], uv: [f32; 2]) -> [u8; VERTEX_SIZE] {
    let mut v = [0u8; VERTEX_SIZE];
    for k in 0..3 {
        v[k * 4..k * 4 + 4].copy_from_slice(&position[k].to_le_bytes());
    }
    let n = octahedral(normal);
    v[12..14].copy_from_slice(&n[0].to_le_bytes());
    v[14..16].copy_from_slice(&n[1].to_le_bytes());
    v[16..18].copy_from_slice(&f16_bits(uv[0]).to_le_bytes());
    v[18..20].copy_from_slice(&f16_bits(uv[1]).to_le_bytes());
    v
}

/// A unit normal as two snorm16 (octahedral).
pub fn octahedral(n: [f32; 3]) -> [i16; 2] {
    let l1 = n[0].abs() + n[1].abs() + n[2].abs();
    let (mut x, mut y) = if l1 > 0.0 { (n[0] / l1, n[1] / l1) } else { (0.0, 0.0) };
    if n[2] < 0.0 {
        let (ox, oy) = (x, y);
        x = (1.0 - oy.abs()) * if ox >= 0.0 { 1.0 } else { -1.0 };
        y = (1.0 - ox.abs()) * if oy >= 0.0 { 1.0 } else { -1.0 };
    }
    [x, y].map(|v| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16)
}

/// The normal of [`octahedral`].
pub fn from_octahedral(e: [i16; 2]) -> [f32; 3] {
    let (x, y) = ((e[0] as f32 / 32767.0).max(-1.0), (e[1] as f32 / 32767.0).max(-1.0));
    let z = 1.0 - x.abs() - y.abs();
    let t = (-z).max(0.0);
    let (x, y) = (x + if x >= 0.0 { -t } else { t }, y + if y >= 0.0 { -t } else { t });
    let l = (x * x + y * y + z * z).sqrt().max(1e-12);
    [x / l, y / l, z / l]
}

/// IEEE half-float bits of `v` (rounded to nearest).
pub fn f16_bits(v: f32) -> u16 {
    let b = v.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32;
    let mant = b & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = (mant | 0x80_0000) >> (1 - e);
        return sign | ((m + 0x1000) >> 13) as u16;
    }
    let h = sign as u32 | ((e as u32) << 10) | (mant >> 13);
    // Round half up on the dropped bits (carries into the exponent as it should).
    (h + ((mant >> 12) & 1)) as u16
}

/// Watches a project's previews: `changed` runs (on the watcher's thread) shortly after anything in the project's
/// `UserSettings` changes (Toolkit replacing a preview, the folder first made), until the watch is dropped. Windows'
/// change notifications; elsewhere a look at the folder twice a second (the page also lists again now and then).
pub struct Watch {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

pub fn watch(project: &Path, changed: impl Fn() + Send + 'static) -> Watch {
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    #[cfg(windows)]
    {
        let (project, stop) = (project.to_path_buf(), stop.clone());
        std::thread::Builder::new().name("project-preview-watch".into()).spawn(move || watch_loop(&project, &stop, changed)).ok();
    }
    #[cfg(not(windows))]
    {
        let (project, stop) = (project.to_path_buf(), stop.clone());
        std::thread::Builder::new().name("project-preview-watch".into()).spawn(move || poll_loop(&project, &stop, changed)).ok();
    }
    Watch { stop }
}

/// What `UserSettings` holds, to tell it changed: every entry's name, length and modification time; a small folder.
#[cfg(not(windows))]
fn settings_state(settings: &Path) -> Option<Vec<(PathBuf, u64, Option<std::time::SystemTime>)>> {
    settings.is_dir().then(|| {
        walkdir::WalkDir::new(settings)
            .into_iter()
            .flatten()
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                Some((e.into_path(), m.len(), m.modified().ok()))
            })
            .collect()
    })
}

#[cfg(not(windows))]
fn poll_loop(project: &Path, stop: &std::sync::atomic::AtomicBool, changed: impl Fn()) {
    use std::sync::atomic::Ordering;
    let settings = project.join("UserSettings");
    let mut last = settings_state(&settings);
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let now = settings_state(&settings);
        if now != last {
            last = now;
            changed();
        }
    }
}

#[cfg(windows)]
fn watch_loop(project: &Path, stop: &std::sync::atomic::AtomicBool, changed: impl Fn()) {
    use std::os::windows::ffi::OsStrExt as _;
    use std::sync::atomic::Ordering;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
    use windows::Win32::Storage::FileSystem::{
        FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
        FILE_NOTIFY_CHANGE_LAST_WRITE,
    };
    use windows::Win32::System::Threading::WaitForSingleObject;
    let settings = project.join("UserSettings");
    // UserSettings when it is there (all of it: the previews' folder may not be yet), else the project's own folder
    // until it appears.
    let open = |dir: &Path, subtree: bool| -> Option<HANDLE> {
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | if subtree { FILE_NOTIFY_CHANGE_LAST_WRITE } else { Default::default() };
        unsafe { FindFirstChangeNotificationW(PCWSTR(wide.as_ptr()), subtree, filter) }.ok()
    };
    while !stop.load(Ordering::Relaxed) {
        let inside = settings.is_dir();
        let Some(handle) = open(if inside { &settings } else { project }, inside) else {
            std::thread::sleep(std::time::Duration::from_secs(2));
            continue;
        };
        while !stop.load(Ordering::Relaxed) {
            if unsafe { WaitForSingleObject(handle, 400) } != WAIT_OBJECT_0 {
                continue;
            }
            // A burst of changes (a file and its replacement) is one.
            std::thread::sleep(std::time::Duration::from_millis(120));
            if unsafe { FindNextChangeNotification(handle) }.is_err() {
                break;
            }
            if !inside {
                if settings.is_dir() {
                    changed();
                    break;
                }
                continue;
            }
            changed();
        }
        unsafe {
            let _ = FindCloseChangeNotification(handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle_scene() -> Vec<u8> {
        let mut w = Writer::default();
        let mut verts = Vec::new();
        for (p, uv) in [([0.0, 0.0, 0.0], [0.0, 0.0]), ([0.0, 1.0, 0.0], [0.0, 1.0]), ([1.0, 0.0, 0.0], [1.0, 0.0])] {
            verts.extend_from_slice(&vertex(p, [0.0, 0.0, -1.0], uv));
        }
        let data = w.push(&verts);
        let indices = w.push(&[0u16, 1, 2].iter().flat_map(|i| i.to_le_bytes()).collect::<Vec<_>>());
        let identity: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.5, 0.0, 0.0, 1.0];
        let matrices = w.push(&identity.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>());
        let b = Bounds { min: [0.0; 3], max: [1.0, 1.0, 0.0] };
        let header = Header {
            scene: Scene { name: "Test".into(), path: "Assets/Test.unity".into(), guid: "0123".into() },
            kind: Kind::Avatars,
            bounds: b,
            focus: b,
            ground: Some(0.0),
            materials: vec![Material {
                name: "M".into(),
                color: [1.0, 0.5, 0.25, 1.0],
                shading: Shading::Toon,
                metallic: 0.25,
                smoothness: 0.5,
                emission: [2.0, 0.0, 0.0],
                ..Default::default()
            }],
            meshes: vec![Mesh { vertices: 3, data, index_format: IndexFormat::U16, indices, submeshes: vec![Submesh { start: 0, count: 3 }], bounds: b, lightmap_uv: None }],
            instances: Instances { count: 1, matrices },
            draws: vec![Draw { instance: 0, mesh: 0, materials: vec![0], shadows: true, avatar: None, lightmap: None }],
            ..Default::default()
        };
        w.finish(&header)
    }

    #[test]
    fn a_written_preview_reads_back() {
        let p = parse(triangle_scene()).unwrap();
        assert_eq!(p.header.scene.name, "Test");
        assert_eq!(p.header.kind, Kind::Avatars);
        assert_eq!(p.blob(p.header.meshes[0].data).len(), 60);
        assert_eq!(p.matrix(0)[12], 0.5);
        assert_eq!(p.header.materials[0].color, [1.0, 0.5, 0.25, 1.0]);
        let m = &p.header.materials[0];
        assert_eq!((m.shading, m.metallic, m.smoothness, m.emission), (Shading::Toon, 0.25, 0.5, [2.0, 0.0, 0.0]));
    }

    /// The same file with another header (the blobs kept), at `schema`.
    fn rewritten(bytes: &[u8], schema: u32, edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
        let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let mut header: serde_json::Value = serde_json::from_slice(&bytes[START..START + json_len]).unwrap();
        edit(&mut header);
        let json = serde_json::to_vec(&header).unwrap();
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&schema.to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(&json);
        out.resize(align(out.len()), 0);
        out.extend_from_slice(&bytes[align(START + json_len)..]);
        out
    }

    /// A file Toolkit 0.4.10 to 0.4.12 wrote: schema 1, materials without their look, which draw lit and nearly matte.
    #[test]
    fn schema_1_previews_read_with_the_looks_defaults() {
        let old = rewritten(&triangle_scene(), 1, |h| {
            let m = h["materials"][0].as_object_mut().unwrap();
            m.retain(|k, _| ["name", "color", "texture", "tiling", "offset", "alpha", "cutoff", "cull"].contains(&k.as_str()));
        });
        let p = parse(old).unwrap();
        let m = &p.header.materials[0];
        assert_eq!((m.shading, m.metallic, m.smoothness), (Shading::Lit, 0.0, 0.2));
        assert_eq!((m.surface_texture, m.normal_texture, m.emission_texture, m.normal_scale), (-1, -1, -1, 1.0));
        assert_eq!(m.emission, [0.0; 3]);
    }

    #[test]
    fn the_scenes_lighting_reads_back_and_is_optional() {
        // Without it (Toolkit 0.4.14 and older): none.
        assert_eq!(parse(triangle_scene()).unwrap().header.lighting, None);
        let lit = rewritten(&triangle_scene(), SCHEMA, |h| {
            h["lighting"] = serde_json::json!({
                "lights": [{ "direction": [0.19, -0.88, -0.44], "color": [1.0, 0.91, 0.67], "shadows": true }],
                "ambient": { "mode": "trilight", "sky": [0.2, 0.3, 0.4], "equator": [0.1, 0.1, 0.1], "ground": [0.0, 0.0, 0.0] },
                "somethingNewer": 1
            });
        });
        let l = parse(lit).unwrap().header.lighting.unwrap();
        assert_eq!(l.lights.len(), 1);
        let k = l.lights[0];
        assert_eq!((k.direction, k.color, k.intensity, k.shadows, k.shadow_strength), ([0.19, -0.88, -0.44], [1.0, 0.91, 0.67], 1.0, true, 1.0));
        let a = l.ambient.unwrap();
        assert_eq!((a.mode, a.sky, a.ground), (AmbientMode::Trilight, [0.2, 0.3, 0.4], [0.0; 3]));
        assert_eq!(l.reflection_intensity, 1.0);
        // A scene without directional lights and an ambient probe never computed.
        let dark = rewritten(&triangle_scene(), SCHEMA, |h| h["lighting"] = serde_json::json!({ "lights": [], "ambient": null }));
        let l = parse(dark).unwrap().header.lighting.unwrap();
        assert!(l.lights.is_empty() && l.ambient.is_none());
    }

    #[test]
    fn a_material_pointing_past_the_textures_is_refused() {
        let bad = rewritten(&triangle_scene(), SCHEMA, |h| h["materials"][0]["normalTexture"] = 3.into());
        assert!(parse(bad).is_err());
        let fine = rewritten(&triangle_scene(), SCHEMA, |h| h["materials"][0]["normalTexture"] = (-1).into());
        assert!(parse(fine).is_ok());
    }

    #[test]
    fn broken_previews_are_refused() {
        let mut bytes = triangle_scene();
        bytes[0] = b'X';
        assert!(parse(bytes).is_err(), "magic");
        let mut bytes = triangle_scene();
        bytes[8] = SCHEMA as u8 + 1;
        assert!(parse(bytes).is_err(), "schema");
        let mut bytes = triangle_scene();
        bytes[8] = 0;
        assert!(parse(bytes).is_err(), "schema 0");
        let mut bytes = triangle_scene();
        bytes.truncate(bytes.len() - 70);
        assert!(parse(bytes).is_err(), "a blob past the end");
    }

    #[test]
    fn the_header_alone_lists_a_projects_previews() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join(FOLDER);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("a.preview"), triangle_scene()).unwrap();
        std::fs::write(folder.join("b.txt"), b"not one").unwrap();
        let list = list(dir.path());
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].header.scene.path, "Assets/Test.unity");
    }

    #[test]
    fn normals_survive_their_octahedral_encoding() {
        for n in [[0.0, 0.0, 1.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.577, -0.577, -0.577], [0.2, 0.9, -0.39]] {
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2] as f32).sqrt();
            let n = n.map(|v: f32| v / l);
            let back = from_octahedral(octahedral(n));
            let dot: f32 = (0..3).map(|k| n[k] * back[k]).sum();
            assert!(dot > 0.99999, "{n:?} -> {back:?}");
        }
    }

    #[test]
    fn half_floats_round_to_nearest() {
        assert_eq!(f16_bits(0.0), 0);
        assert_eq!(f16_bits(1.0), 0x3c00);
        assert_eq!(f16_bits(-2.0), 0xc000);
        assert_eq!(f16_bits(0.5), 0x3800);
        assert_eq!(f16_bits(65504.0), 0x7bff);
        assert_eq!(f16_bits(1e6), 0x7c00);
        assert_eq!(f16_bits(1.0 / 3.0), 0x3555);
        // Subnormal.
        assert_eq!(f16_bits(5.960_464_5e-8), 1);
    }

    #[cfg(windows)]
    #[test]
    fn a_preview_without_lightmaps_or_sky_reads_and_one_with_them_is_checked() {
        // What Toolkit writes has none of these fields.
        let p = parse(triangle_scene()).unwrap();
        assert!(p.header.lightmaps.is_empty() && p.header.sky.is_none());
        assert!(p.header.draws[0].lightmap.is_none() && p.header.meshes[0].lightmap_uv.is_none());
        // A draw of a lightmap that is not there, UVs of the wrong length: refused.
        let mut h = p.header.clone();
        h.draws[0].lightmap = Some(DrawLightmap { index: 0, st: [1.0, 1.0, 0.0, 0.0] });
        let mut w = Writer::default();
        w.push(&vec![0u8; p.size()]);
        assert!(parse(w.finish(&h)).is_err());
        let mut h = p.header.clone();
        h.meshes[0].lightmap_uv = Some(Blob { offset: 0, length: 5 });
        let mut w = Writer::default();
        w.push(&vec![0u8; p.size()]);
        assert!(parse(w.finish(&h)).is_err());
    }

    #[test]
    fn a_new_preview_is_seen_at_once() {
        let temp = tempfile::tempdir().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        // No UserSettings yet: its making is seen, then a preview written into it.
        let _watch = watch(temp.path(), move || {
            let _ = tx.send(());
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        std::fs::create_dir_all(temp.path().join(FOLDER)).unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(5)).expect("the folder made");
        std::thread::sleep(std::time::Duration::from_millis(600));
        while rx.try_recv().is_ok() {}
        std::fs::write(temp.path().join(FOLDER).join("a.preview"), b"x").unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(5)).expect("the preview written");
    }
}
