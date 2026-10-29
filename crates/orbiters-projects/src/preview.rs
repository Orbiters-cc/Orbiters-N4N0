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
pub const MAGIC: &[u8; 8] = b"ORBPREV\0";
pub const SCHEMA: u32 = 1;
/// Position (3 × f32), octahedral normal (2 × snorm16), UV (2 × f16).
pub const VERTEX_SIZE: usize = 20;
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

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    ensure!(bytes.len() >= START && &bytes[..8] == MAGIC, "not an Orbiters project preview");
    let schema = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    ensure!(schema == SCHEMA, "a project preview of schema {schema}, this version reads {SCHEMA}");
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
    }
    for (i, m) in h.materials.iter().enumerate() {
        ensure!(m.texture < h.textures.len() as i32, "material {i} uses a texture the preview does not have");
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
/// change notifications; elsewhere nothing (the page also lists again now and then).
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
    let _ = (project, changed);
    Watch { stop }
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
                texture: -1,
                tiling: [1.0; 2],
                offset: [0.0; 2],
                alpha: Alpha::Opaque,
                cutoff: 0.5,
                cull: Cull::Back,
            }],
            meshes: vec![Mesh { vertices: 3, data, index_format: IndexFormat::U16, indices, submeshes: vec![Submesh { start: 0, count: 3 }], bounds: b }],
            instances: Instances { count: 1, matrices },
            draws: vec![Draw { instance: 0, mesh: 0, materials: vec![0], shadows: true, avatar: None }],
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
    }

    #[test]
    fn broken_previews_are_refused() {
        let mut bytes = triangle_scene();
        bytes[0] = b'X';
        assert!(parse(bytes).is_err(), "magic");
        let mut bytes = triangle_scene();
        bytes[8] = 2;
        assert!(parse(bytes).is_err(), "schema");
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
