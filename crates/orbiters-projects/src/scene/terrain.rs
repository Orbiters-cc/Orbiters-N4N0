//! Unity terrains as a preview draws them: a `TerrainData` asset (saved in Unity's binary format or as text, read alike)
//! becomes one mesh, one picture and a list of trees.
//!
//! - **The ground.** The heightmap (`m_Heights`: 16-bit, 0 to 32766 up to the terrain's height) thinned to at most
//!   [`MAX_SIDE`] samples on a side, so a 4097² terrain costs 511² quads at most, half a million triangles, with its
//!   normals from the full heightmap's slopes.
//! - **Its picture.** Unity blends the terrain layers' pictures by the splat maps per pixel. A preview has one base
//!   picture per material, so the blend is done once here, on the CPU, into a picture of at most [`MAX_PICTURE`]
//!   pixels on a side: each layer's picture tiled over the terrain (`m_TileSize`, `m_TileOffset`), weighted by its
//!   splat channel. Up close it is softer than Unity's; a preview looks from above.
//! - **Its trees.** `m_TreeInstances` (where, how large, turned how far, which prototype), each prototype's prefab
//!   drawn at each place, at most [`MAX_TREES`] of them: more are thinned evenly and counted. Grass and other details
//!   (`m_DetailDatabase`) are counted, not drawn.

use super::assets::Assets;
use super::model::ModelMesh;
use super::serialized::SerializedFile;
use super::texture;
use super::yaml::{self, Ref, Value};
use super::Limits;
use anyhow::{ensure, Context as _};
use orbiters_i18n::tr;
use image::RgbaImage;
use std::path::{Path, PathBuf};

/// The most height samples on a side of the mesh.
const MAX_SIDE: usize = 512;
/// The most pixels on a side of the terrain's picture.
const MAX_PICTURE: u32 = 2048;
/// The most trees one terrain draws.
pub const MAX_TREES: usize = 4000;

pub struct TreeInstance {
    /// Over the terrain, 0 to 1 on each axis.
    pub position: [f32; 3],
    pub width: f32,
    pub height: f32,
    /// Round the vertical, radians.
    pub rotation: f32,
    pub prototype: usize,
}

pub struct Layer {
    pub texture: Option<String>,
    pub tile_size: [f32; 2],
    pub tile_offset: [f32; 2],
}

pub struct TerrainData {
    pub name: String,
    /// Metres across, up, along.
    pub size: [f32; 3],
    pub resolution: usize,
    /// `resolution²` samples, row after row along Z, 0 to 1.
    pub heights: Vec<f32>,
    pub layers: Vec<Layer>,
    /// The splat maps: each RGBA holds four layers' weights.
    pub alpha: Vec<RgbaImage>,
    pub prototypes: Vec<Ref>,
    pub trees: Vec<TreeInstance>,
    /// Detail patches holding something (grass, small rocks): not drawn.
    pub detail_patches: usize,
}

/// Reads the `TerrainData` of a file (its object `file_id`, else its first).
pub fn read(path: &Path, file_id: i64, assets: &Assets, limits: &Limits, files: &mut Vec<PathBuf>) -> anyhow::Result<TerrainData> {
    let size = std::fs::metadata(path)?.len();
    ensure!(size <= limits.max_yaml_bytes, "{} MB", size >> 20);
    let bytes = std::fs::read(path)?;
    files.push(path.to_path_buf());
    if super::serialized::is_serialized(&bytes) {
        let file = SerializedFile::parse(&bytes)?;
        let value_of = |id: i64, class: i32| -> anyhow::Result<Value> {
            let o = file.objects().iter().find(|o| o.id == id && o.class == class).or_else(|| file.objects().iter().find(|o| class == 156 && o.class == class)).with_context(|| tr("an object the file does not have"))?;
            let at = usize::try_from(file.data_offset + o.start)?;
            file.value(o, bytes.get(at..at + o.size as usize).with_context(|| tr("an object past the file's end"))?)
        };
        return parse(&value_of(file_id, 156)?, &|id| value_of(id, 28).ok(), assets, files);
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    ensure!(yaml::is_text(&text), "{}", tr("neither Unity text nor Unity's binary format"));
    let docs = yaml::documents(&text);
    let doc = docs.iter().find(|d| d.class == 156 && d.id == file_id).or_else(|| docs.iter().find(|d| d.class == 156)).with_context(|| tr("no terrain data in it"))?;
    parse(&doc.parse(), &|id| docs.iter().find(|d| d.class == 28 && d.id == id).map(|d| d.parse()), assets, files)
}

fn parse(v: &Value, sub: &dyn Fn(i64) -> Option<Value>, assets: &Assets, files: &mut Vec<PathBuf>) -> anyhow::Result<TerrainData> {
    let h = v.get("m_Heightmap");
    // `m_Resolution` since Unity 2019, `m_Width` before.
    let resolution = h.get("m_Resolution").as_i64().or_else(|| h.get("m_Width").as_i64()).unwrap_or(0).max(0) as usize;
    ensure!((2..=8193).contains(&resolution), "{}", tr!("a heightmap of {resolution} samples on a side", resolution = resolution));
    let raw = bytes_of(h.get("m_Heights"), 2);
    ensure!(raw.len() >= resolution * resolution * 2, "{}", tr!("{count} heights for {resolution} samples on a side", count = raw.len() / 2, resolution = resolution));
    let heights: Vec<f32> = raw.chunks_exact(2).take(resolution * resolution).map(|c| (i16::from_le_bytes([c[0], c[1]]).max(0) as f32) / 32766.0).collect();
    let scale = h.get("m_Scale").vec3().unwrap_or([1.0; 3]);
    let size = [scale[0] * (resolution - 1) as f32, scale[1], scale[2] * (resolution - 1) as f32];
    ensure!(size.iter().all(|s| s.is_finite() && *s >= 0.0 && *s < 1e6), "{}", tr!("a terrain of size {size}", size = format!("{size:?}")));

    let splat = v.get("m_SplatDatabase");
    let mut layers = Vec::new();
    for r in splat.get("m_TerrainLayers").items() {
        layers.push(r.as_ref().and_then(|r| r.guid).and_then(|g| assets.path(&g)).and_then(|p| layer(&p, files)).unwrap_or(Layer {
            texture: None,
            tile_size: [1.0, 1.0],
            tile_offset: [0.0, 0.0],
        }));
    }
    // Older terrains list their pictures in the data itself.
    if layers.is_empty() {
        for s in splat.get("m_Splats").items() {
            layers.push(Layer {
                texture: s.get("texture").as_ref().and_then(|r| r.guid),
                tile_size: s.get("tileSize").vec2().unwrap_or([1.0, 1.0]),
                tile_offset: s.get("tileOffset").vec2().unwrap_or([0.0, 0.0]),
            });
        }
    }
    let alpha: Vec<RgbaImage> = splat.get("m_AlphaTextures").items().iter().filter_map(|r| r.as_ref()).filter_map(|r| sub(r.file_id)).filter_map(|t| splat_of(&t)).collect();

    let d = v.get("m_DetailDatabase");
    let prototypes: Vec<Ref> = d.get("m_TreePrototypes").items().iter().map(|p| p.get("prefab").as_ref().unwrap_or(Ref { file_id: 0, guid: None })).collect();
    let trees = trees_of(d.get("m_TreeInstances"));
    let detail_patches = d.get("m_Patches").items().iter().filter(|p| !bytes_of(p.get("coverage"), 1).iter().all(|c| *c == 0)).count();
    Ok(TerrainData { name: v.get("m_Name").str_or("Terrain").to_string(), size, resolution, heights, layers, alpha, prototypes, trees, detail_patches }) // i18n: not shown (Unity's default terrain name)
}

/// A list of plain numbers as its bytes, however the file holds it: bytes (binary), hex (text), or a list.
fn bytes_of(v: &Value, element: usize) -> Vec<u8> {
    match v {
        Value::Bytes(b) => b.clone(),
        Value::Str(h) => {
            let b = h.as_bytes();
            let d = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
            b.chunks_exact(2).map(|p| d(p[0]) << 4 | d(p[1])).collect()
        }
        v => {
            let mut out = Vec::with_capacity(v.items().len() * element);
            for item in v.items() {
                let n = item.as_i64().unwrap_or(0);
                out.extend_from_slice(&n.to_le_bytes()[..element]);
            }
            out
        }
    }
}

fn layer(path: &Path, files: &mut Vec<PathBuf>) -> Option<Layer> {
    let text = std::fs::read_to_string(path).ok()?;
    files.push(path.to_path_buf());
    let doc = yaml::documents(&text).into_iter().find(|d| d.class == 1953259897)?;
    let v = doc.parse();
    Some(Layer {
        texture: v.get("m_DiffuseTexture").as_ref().and_then(|r| r.guid),
        tile_size: v.get("m_TileSize").vec2().unwrap_or([1.0, 1.0]),
        tile_offset: v.get("m_TileOffset").vec2().unwrap_or([0.0, 0.0]),
    })
}

/// A splat map: an uncompressed RGBA picture inside the terrain's asset, rows from the bottom.
fn splat_of(t: &Value) -> Option<RgbaImage> {
    let (w, h) = (t.get("m_Width").as_i64()? as u32, t.get("m_Height").as_i64()? as u32);
    if !(1..=8192).contains(&w) || !(1..=8192).contains(&h) {
        return None;
    }
    let format = t.get("m_TextureFormat").as_i64().unwrap_or(4);
    let data = match t.get("image data") {
        Value::Bytes(b) => b.clone(),
        _ => bytes_of(t.get("_typelessdata"), 1),
    };
    let need = w as usize * h as usize * 4;
    if data.len() < need {
        return None;
    }
    let row = w as usize * 4;
    let mut out = vec![0u8; need];
    for y in 0..h as usize {
        let (src, dst) = (&data[y * row..(y + 1) * row], &mut out[(h as usize - 1 - y) * row..(h as usize - y) * row]);
        match format {
            // ARGB32 and BGRA32: the channels to RGBA.
            5 => dst.chunks_exact_mut(4).zip(src.chunks_exact(4)).for_each(|(d, s)| d.copy_from_slice(&[s[1], s[2], s[3], s[0]])),
            14 => dst.chunks_exact_mut(4).zip(src.chunks_exact(4)).for_each(|(d, s)| d.copy_from_slice(&[s[2], s[1], s[0], s[3]])),
            _ => dst.copy_from_slice(src),
        }
    }
    RgbaImage::from_raw(w, h, out)
}

fn trees_of(v: &Value) -> Vec<TreeInstance> {
    match v {
        // A binary file's long list: position (3 floats), width and height scale, rotation, colour (4 bytes), its
        // lightmap colour (4 bytes), prototype (int).
        Value::Bytes(b) => b
            .chunks_exact(36)
            .map(|r| {
                let f = |k: usize| f32::from_le_bytes(r[k * 4..k * 4 + 4].try_into().unwrap());
                TreeInstance { position: [f(0), f(1), f(2)], width: f(3), height: f(4), rotation: f(5), prototype: i32::from_le_bytes(r[32..36].try_into().unwrap()).max(0) as usize }
            })
            .collect(),
        v => v
            .items()
            .iter()
            .map(|t| TreeInstance {
                position: t.get("position").vec3().unwrap_or([0.0; 3]),
                width: t.get("widthScale").f32_or(1.0),
                height: t.get("heightScale").f32_or(1.0),
                rotation: t.get("rotation").f32_or(0.0),
                prototype: t.get("index").as_i64().unwrap_or(0).max(0) as usize,
            })
            .collect(),
    }
}

impl TerrainData {
    fn height_at(&self, x: usize, z: usize) -> f32 {
        self.heights[z.min(self.resolution - 1) * self.resolution + x.min(self.resolution - 1)]
    }

    /// The ground as a mesh, from the terrain's corner: at most [`MAX_SIDE`] samples on a side. The first UV set runs 0
    /// to 1 over the terrain (its picture), and so does the second (a terrain's lightmap).
    pub fn mesh(&self) -> ModelMesh {
        let res = self.resolution;
        let step = (res - 1).div_ceil(MAX_SIDE - 1).max(1);
        let side = (res - 1).div_ceil(step) + 1;
        let cell = [self.size[0] / (res - 1) as f32, self.size[2] / (res - 1) as f32];
        let mut positions = Vec::with_capacity(side * side);
        let mut normals = Vec::with_capacity(side * side);
        let mut uvs = Vec::with_capacity(side * side);
        for j in 0..side {
            for i in 0..side {
                let (x, z) = ((i * step).min(res - 1), (j * step).min(res - 1));
                positions.push([x as f32 * cell[0], self.height_at(x, z) * self.size[1], z as f32 * cell[1]]);
                let (l, r) = (self.height_at(x.saturating_sub(step), z), self.height_at(x + step, z));
                let (d, u) = (self.height_at(x, z.saturating_sub(step)), self.height_at(x, z + step));
                let dx = (r - l) * self.size[1] / (cell[0] * (x + step).min(res - 1).saturating_sub(x.saturating_sub(step)).max(1) as f32);
                let dz = (u - d) * self.size[1] / (cell[1] * (z + step).min(res - 1).saturating_sub(z.saturating_sub(step)).max(1) as f32);
                normals.push(super::model::normalize([-dx, 1.0, -dz]));
                uvs.push([x as f32 / (res - 1) as f32, z as f32 / (res - 1) as f32]);
            }
        }
        let mut triangles = Vec::with_capacity((side - 1) * (side - 1) * 6);
        for j in 0..side - 1 {
            for i in 0..side - 1 {
                let a = (j * side + i) as u32;
                let (b, c, d) = (a + 1, a + side as u32, a + side as u32 + 1);
                // Clockwise from above, Unity's front.
                triangles.extend_from_slice(&[a, c, d, a, d, b]);
            }
        }
        ModelMesh { name: self.name.clone(), positions, normals, uv2: uvs.clone(), uvs, submeshes: vec![triangles], skin: None, blend_shapes: Vec::new() }
    }

    /// The layers' pictures blended by the splat maps, rows from the top (Z's far end), as one picture.
    pub fn picture(&self, assets: &Assets, limits: &Limits, files: &mut Vec<PathBuf>) -> RgbaImage {
        let side = self.alpha.first().map_or(1024, |a| a.width()).clamp(512, MAX_PICTURE).max(1024.min(MAX_PICTURE));
        let mut layers: Vec<Option<RgbaImage>> = Vec::with_capacity(self.layers.len());
        for l in &self.layers {
            let image = l.texture.as_ref().and_then(|g| assets.path(g)).and_then(|p| {
                let image = texture::decode(&p, limits.max_texture_bytes).ok()?;
                files.push(p.clone());
                files.push(super::prefab::meta_path(&p));
                // No more detail than the blend can show: a layer's pixels at a fraction of the terrain.
                Some(image::imageops::thumbnail(&image, image.width().min(512), image.height().min(512)))
            });
            layers.push(image);
        }
        let mut out = RgbaImage::new(side, side);
        let n = self.layers.len();
        let sample = |a: &RgbaImage, u: f32, v: f32, k: usize| -> f32 {
            let (w, h) = a.dimensions();
            let (x, y) = (((u * w as f32) as u32).min(w - 1), ((v * h as f32) as u32).min(h - 1));
            a.get_pixel(x, y)[k] as f32 / 255.0
        };
        for py in 0..side {
            // Rows from the top: the far end of Z first.
            let v_up = 1.0 - (py as f32 + 0.5) / side as f32;
            for px in 0..side {
                let u = (px as f32 + 0.5) / side as f32;
                let mut sum = [0.0f32; 3];
                let mut total = 0.0f32;
                for (i, l) in self.layers.iter().enumerate().take(n) {
                    let w = match self.alpha.get(i / 4) {
                        // The splat map's rows were put from the top when read.
                        Some(a) => sample(a, u, 1.0 - v_up, i % 4),
                        None => {
                            if i == 0 {
                                1.0
                            } else {
                                0.0
                            }
                        }
                    };
                    if w <= 0.002 {
                        continue;
                    }
                    let c = match &layers[i] {
                        Some(img) => {
                            let (lu, lv) = (
                                (u * self.size[0] + l.tile_offset[0]) / l.tile_size[0].abs().max(1e-3),
                                (v_up * self.size[2] + l.tile_offset[1]) / l.tile_size[1].abs().max(1e-3),
                            );
                            let (w0, h0) = img.dimensions();
                            let (x, y) = (((lu.rem_euclid(1.0)) * w0 as f32) as u32, ((1.0 - lv.rem_euclid(1.0)) * h0 as f32) as u32);
                            let p = img.get_pixel(x.min(w0 - 1), y.min(h0 - 1));
                            [p[0] as f32, p[1] as f32, p[2] as f32]
                        }
                        None => [128.0, 128.0, 128.0],
                    };
                    for k in 0..3 {
                        sum[k] += c[k] * w;
                    }
                    total += w;
                }
                let p = if total > 0.0 { sum.map(|s| (s / total) as u8) } else { [128, 128, 128] };
                out.put_pixel(px, py, image::Rgba([p[0], p[1], p[2], 255]));
            }
        }
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small terrain saved as text: 3 × 3 heights rising along X, 10 m across and 4 m tall, two trees.
    pub(crate) fn asset(prefab_guid: &str) -> String {
        // Heights 0, half, whole along each row; in hex, little-endian (16383 = 0x3fff, 32766 = 0x7ffe).
        let row = "0000ff3ffe7f";
        let heights = row.repeat(3);
        format!(
            "%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!156 &15600000
TerrainData:
  m_Name: Small
  m_SplatDatabase:
    m_TerrainLayers: []
    m_AlphaTextures: []
    m_AlphamapResolution: 16
  m_DetailDatabase:
    m_Patches:
    - layerIndices:
      coverage:
    m_TreePrototypes:
    - prefab: {{fileID: 100100000, guid: {prefab_guid}, type: 3}}
      bendFactor: 0
    m_TreeInstances:
    - position: {{x: 0.5, y: 0, z: 0.5}}
      widthScale: 2
      heightScale: 3
      rotation: 0
      index: 0
    - position: {{x: 1, y: 0.5, z: 1}}
      widthScale: 1
      heightScale: 1
      rotation: 1.5
      index: 0
  m_Heightmap:
    m_Heights: {heights}
    m_Resolution: 3
    m_Scale: {{x: 5, y: 4, z: 5}}
"
        )
    }

    #[test]
    fn a_small_terrain_reads_into_a_mesh_facing_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Small.asset");
        std::fs::write(&path, asset("abcd0000000000000000000000000001")).unwrap();
        let assets = Assets::empty(dir.path());
        let mut files = Vec::new();
        let t = read(&path, 15600000, &assets, &Limits::default(), &mut files).unwrap();
        assert_eq!(t.size, [10.0, 4.0, 10.0]);
        assert_eq!(t.resolution, 3);
        assert_eq!(t.trees.len(), 2);
        assert_eq!(t.trees[0].prototype, 0);
        assert_eq!(t.prototypes[0].guid.as_deref(), Some("abcd0000000000000000000000000001"));
        let m = t.mesh();
        assert_eq!(m.positions.len(), 9);
        assert_eq!(m.triangles(), 8);
        // The far corner of the first row: 10 m along X at the terrain's whole height.
        assert!((m.positions[2][0] - 10.0).abs() < 1e-4 && (m.positions[2][1] - 4.0).abs() < 1e-3, "{:?}", m.positions[2]);
        // Unity's front faces are clockwise seen from the front: from above here (+Y), with X right and Z away.
        let t0 = &m.submeshes[0][..3];
        let (a, b, c) = (m.positions[t0[0] as usize], m.positions[t0[1] as usize], m.positions[t0[2] as usize]);
        let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        let cross_y = u[2] * v[0] - u[0] * v[2];
        assert!(cross_y > 0.0, "left-handed: clockwise from above has a normal up, {cross_y}");
        // The slope goes up along X: the normal leans back against it.
        assert!(m.normals[4][0] < 0.0 && m.normals[4][1] > 0.0, "{:?}", m.normals[4]);
    }

    #[test]
    fn a_splat_map_is_read_from_the_bottom_up() {
        // Two rows of two pixels: the file's first row (Z = 0) red, its second blue.
        let mut data = Vec::new();
        for _ in 0..2 {
            data.extend_from_slice(&[255, 0, 0, 0]);
        }
        for _ in 0..2 {
            data.extend_from_slice(&[0, 0, 255, 0]);
        }
        let t = Value::Map(vec![("m_Width".into(), Value::Str("2".into())), ("m_Height".into(), Value::Str("2".into())), ("m_TextureFormat".into(), Value::Str("4".into())), ("image data".into(), Value::Bytes(data))]);
        let a = splat_of(&t).unwrap();
        assert_eq!(a.get_pixel(0, 1)[0], 255, "the bottom of the picture, the near end");
        assert_eq!(a.get_pixel(0, 0)[2], 255);
    }

    #[test]
    fn a_huge_heightmap_is_thinned_and_a_damaged_one_is_refused() {
        let mut t = TerrainData {
            name: "Big".into(),
            size: [1000.0, 100.0, 1000.0],
            resolution: 2049,
            heights: vec![0.5; 2049 * 2049],
            layers: Vec::new(),
            alpha: Vec::new(),
            prototypes: Vec::new(),
            trees: Vec::new(),
            detail_patches: 0,
        };
        let m = t.mesh();
        assert!(m.positions.len() <= MAX_SIDE * MAX_SIDE, "{}", m.positions.len());
        assert!((m.positions.last().unwrap()[0] - 1000.0).abs() < 1e-2, "it still reaches the far corner: {:?}", m.positions.last());
        t.resolution = 3;
        t.heights = vec![0.0; 9];
        let dir = tempfile::tempdir().unwrap();
        let assets = Assets::empty(dir.path());
        for (name, text) in [("no heights", "TerrainData:\n  m_Heightmap:\n    m_Resolution: 9\n"), ("huge", "TerrainData:\n  m_Heightmap:\n    m_Resolution: 900000\n")] {
            let v = yaml::parse(text);
            assert!(parse(v.get("TerrainData"), &|_| None, &assets, &mut Vec::new()).is_err(), "{name}");
        }
    }

    #[test]
    fn the_picture_blends_its_layers_by_the_splat_map() {
        let dir = tempfile::tempdir().unwrap();
        let red = dir.path().join("red.png");
        let blue = dir.path().join("blue.png");
        RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255])).save(&red).unwrap();
        RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 255, 255])).save(&blue).unwrap();
        let mut assets = Assets::empty(dir.path());
        assets.insert("aaaa0000000000000000000000000001", red);
        assets.insert("aaaa0000000000000000000000000002", blue);
        // Left half layer 0, right half layer 1.
        let mut splat = RgbaImage::new(4, 4);
        for (x, _, p) in splat.enumerate_pixels_mut() {
            *p = if x < 2 { image::Rgba([255, 0, 0, 0]) } else { image::Rgba([0, 255, 0, 0]) };
        }
        let t = TerrainData {
            name: "T".into(),
            size: [10.0, 1.0, 10.0],
            resolution: 2,
            heights: vec![0.0; 4],
            layers: vec![
                Layer { texture: Some("aaaa0000000000000000000000000001".into()), tile_size: [1.0, 1.0], tile_offset: [0.0, 0.0] },
                Layer { texture: Some("aaaa0000000000000000000000000002".into()), tile_size: [1.0, 1.0], tile_offset: [0.0, 0.0] },
            ],
            alpha: vec![splat],
            prototypes: Vec::new(),
            trees: Vec::new(),
            detail_patches: 0,
        };
        let p = t.picture(&assets, &Limits::default(), &mut Vec::new());
        let (w, h) = p.dimensions();
        assert_eq!(p.get_pixel(w / 4, h / 2)[0], 255, "left: the first layer");
        assert_eq!(p.get_pixel(w * 3 / 4, h / 2)[2], 255, "right: the second layer");
        // Along Z: a splat map's rows are the far end of Z first once read, as the picture's are.
        let mut t = t;
        let mut splat = RgbaImage::new(4, 4);
        for (_, y, p) in splat.enumerate_pixels_mut() {
            *p = if y < 2 { image::Rgba([255, 0, 0, 0]) } else { image::Rgba([0, 255, 0, 0]) };
        }
        t.alpha = vec![splat];
        let p = t.picture(&assets, &Limits::default(), &mut Vec::new());
        let (w, h) = p.dimensions();
        assert_eq!(p.get_pixel(w / 2, h / 4)[0], 255, "the top of the picture, the far end: the first layer");
        assert_eq!(p.get_pixel(w / 2, h * 3 / 4)[2], 255, "the near end: the second");
    }
}
