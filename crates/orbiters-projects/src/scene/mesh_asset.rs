//! Meshes saved as Unity assets (`.asset`, class 43): what avatar tools write when they edit a mesh (a body with
//! parts deleted, merged clothing). Already in Unity's space and winding. Vertex data is one hex blob of streams, each
//! stream's channels interleaved (position, normal, UV0, bone weights and indices as Unity 2019 and newer lay them
//! out); indices another, 16 or 32 bits; bind poses and blend shapes as YAML.

use super::model::{BlendShape, ModelMesh, Skin};
use super::yaml::{self, Value};
use super::Limits;
use anyhow::{bail, ensure, Context as _};
use std::path::Path;

pub fn read(path: &Path, file_id: i64, limits: &Limits) -> anyhow::Result<ModelMesh> {
    let size = std::fs::metadata(path)?.len();
    ensure!(size <= limits.max_yaml_bytes, "{} MB", size >> 20);
    let bytes = std::fs::read(path)?;
    if super::serialized::is_serialized(&bytes) {
        // Saved in Unity's binary format (a tool's generated mesh).
        let (class, v) = super::serialized::object(&bytes, file_id, 43)?;
        ensure!(class == 43, "the object is not a mesh");
        return parse(&v);
    }
    let text = String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("neither Unity text nor Unity's binary format"))?;
    ensure!(yaml::is_text(&text), "not a Unity asset");
    let docs = yaml::documents(&text);
    let doc = docs.iter().find(|d| d.class == 43 && d.id == file_id).or_else(|| docs.iter().find(|d| d.class == 43)).context("no mesh in it")?;
    parse(&doc.parse())
}

fn hex(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let v = |c: u8| match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0,
    };
    b.chunks_exact(2).map(|p| v(p[0]) << 4 | v(p[1])).collect()
}

/// Bytes of one component of Unity's `VertexAttributeFormat`.
fn format_size(format: i64) -> usize {
    match format {
        0 | 10 | 11 => 4,
        1 | 4 | 5 | 8 | 9 => 2,
        _ => 1,
    }
}

fn component(data: &[u8], at: usize, format: i64) -> f32 {
    let get = |n: usize| data.get(at..at + n);
    match format {
        0 => get(4).map_or(0.0, |b| f32::from_le_bytes(b.try_into().unwrap())),
        1 => get(2).map_or(0.0, |b| half(u16::from_le_bytes(b.try_into().unwrap()))),
        2 => get(1).map_or(0.0, |b| b[0] as f32 / 255.0),
        3 => get(1).map_or(0.0, |b| (b[0] as i8 as f32 / 127.0).max(-1.0)),
        4 => get(2).map_or(0.0, |b| u16::from_le_bytes(b.try_into().unwrap()) as f32 / 65535.0),
        5 => get(2).map_or(0.0, |b| (i16::from_le_bytes(b.try_into().unwrap()) as f32 / 32767.0).max(-1.0)),
        6 => get(1).map_or(0.0, |b| b[0] as f32),
        7 => get(1).map_or(0.0, |b| b[0] as i8 as f32),
        8 => get(2).map_or(0.0, |b| u16::from_le_bytes(b.try_into().unwrap()) as f32),
        9 => get(2).map_or(0.0, |b| i16::from_le_bytes(b.try_into().unwrap()) as f32),
        10 => get(4).map_or(0.0, |b| u32::from_le_bytes(b.try_into().unwrap()) as f32),
        11 => get(4).map_or(0.0, |b| i32::from_le_bytes(b.try_into().unwrap()) as f32),
        _ => 0.0,
    }
}

fn half(h: u16) -> f32 {
    let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = (h >> 10) & 0x1f;
    let m = (h & 0x3ff) as f32;
    s * match e {
        0 => m * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + m / 1024.0) * 2f32.powi(e as i32 - 15),
    }
}

pub fn parse(m: &Value) -> anyhow::Result<ModelMesh> {
    let vd = m.get("m_VertexData");
    let count = vd.get("m_VertexCount").as_i64().unwrap_or(0).max(0) as usize;
    ensure!(count <= 4_000_000, "{count} vertices");
    let data = match (vd.get("_typelessdata"), vd.get("m_DataSize")) {
        (Value::Str(h), _) => hex(h),
        (_, Value::Bytes(b)) => b.clone(),
        _ => Vec::new(),
    };
    let channels: Vec<(i64, i64, i64, i64)> = vd
        .get("m_Channels")
        .items()
        .iter()
        .map(|c| {
            (
                c.get("stream").as_i64().unwrap_or(0),
                c.get("offset").as_i64().unwrap_or(0),
                c.get("format").as_i64().unwrap_or(0),
                c.get("dimension").as_i64().unwrap_or(0) & 0xf,
            )
        })
        .collect();
    if channels.is_empty() || data.is_empty() {
        bail!("its vertices are not saved in it (an older Unity's format)");
    }
    // Each stream's stride, and where it starts (after the previous, aligned to 16 bytes).
    let streams = channels.iter().map(|c| c.0).max().unwrap_or(0) as usize + 1;
    let mut stride = vec![0usize; streams];
    for &(s, off, fmt, dim) in &channels {
        if dim > 0 {
            stride[s as usize] = stride[s as usize].max(off as usize + format_size(fmt) * dim as usize);
        }
    }
    let mut start = vec![0usize; streams];
    let mut at = 0usize;
    for s in 0..streams {
        start[s] = at;
        at += stride[s] * count;
        at = at.div_ceil(16) * 16;
    }
    let read = |ch: usize, v: usize, k: usize| -> f32 {
        let Some(&(s, off, fmt, dim)) = channels.get(ch) else { return 0.0 };
        if k as i64 >= dim {
            return 0.0;
        }
        let s = s as usize;
        component(&data, start[s] + v * stride[s] + off as usize + k * format_size(fmt), fmt)
    };
    let has = |ch: usize| channels.get(ch).is_some_and(|c| c.3 > 0);
    ensure!(has(0), "no positions");
    let positions: Vec<[f32; 3]> = (0..count).map(|v| [read(0, v, 0), read(0, v, 1), read(0, v, 2)]).collect();
    let normals: Vec<[f32; 3]> = if has(1) {
        (0..count).map(|v| super::model::normalize([read(1, v, 0), read(1, v, 1), read(1, v, 2)])).collect()
    } else {
        vec![[0.0, 1.0, 0.0]; count]
    };
    let uvs: Vec<[f32; 2]> = if has(4) { (0..count).map(|v| [read(4, v, 0), read(4, v, 1)]).collect() } else { vec![[0.0; 2]; count] };

    let index_bytes = match m.get("m_IndexBuffer") {
        Value::Bytes(b) => b.clone(),
        v => hex(v.str_or("")),
    };
    let wide = m.get("m_IndexFormat").as_i64() == Some(1);
    let index_size = if wide { 4 } else { 2 };
    let mut submeshes = Vec::new();
    for s in m.get("m_SubMeshes").items() {
        if s.get("topology").as_i64().unwrap_or(0) != 0 {
            submeshes.push(Vec::new());
            continue;
        }
        let first = s.get("firstByte").as_i64().unwrap_or(0).max(0) as usize;
        let n = s.get("indexCount").as_i64().unwrap_or(0).max(0) as usize;
        let base = s.get("baseVertex").as_i64().unwrap_or(0).max(0) as u32;
        let mut idx = Vec::with_capacity(n);
        for k in 0..n - n % 3 {
            let at = first + k * index_size;
            let Some(b) = index_bytes.get(at..at + index_size) else { break };
            let i = if wide { u32::from_le_bytes(b.try_into().unwrap()) } else { u16::from_le_bytes(b.try_into().unwrap()) as u32 } + base;
            idx.push(i.min(count.saturating_sub(1) as u32));
        }
        submeshes.push(idx);
    }

    let bind: Vec<[f32; 16]> = m
        .get("m_BindPose")
        .items()
        .iter()
        .map(|b| std::array::from_fn(|k| b.get(&format!("e{}{}", k % 4, k / 4)).f32_or(if k % 5 == 0 { 1.0 } else { 0.0 })))
        .collect();
    let skin = (has(12) && has(13) && !bind.is_empty()).then(|| {
        let weights = (0..count)
            .map(|v| {
                let mut w = [(0u16, 0.0f32); 4];
                let dims = channels[12].3.min(4) as usize;
                for (k, slot) in w.iter_mut().enumerate().take(dims.max(1)) {
                    let weight = if dims == 1 && k == 0 { 1.0 } else { read(12, v, k) };
                    *slot = (read(13, v, k) as u16, weight);
                }
                w
            })
            .collect();
        Skin { bones: (0..bind.len()).collect(), names: Vec::new(), bind_poses: bind.clone(), weights }
    });

    let shapes = m.get("m_Shapes");
    let deltas: Vec<(u32, [f32; 3])> = match shapes.get("vertices") {
        // A binary file's: records of a position, a normal and a tangent offset (3 floats each) and the vertex (u32).
        Value::Bytes(b) => b
            .chunks_exact(40)
            .map(|r| {
                let f = |k: usize| f32::from_le_bytes(r[k * 4..k * 4 + 4].try_into().unwrap());
                (u32::from_le_bytes(r[36..40].try_into().unwrap()), [f(0), f(1), f(2)])
            })
            .collect(),
        v => v.items().iter().map(|v| (v.get("index").as_i64().unwrap_or(0) as u32, v.get("vertex").vec3().unwrap_or([0.0; 3]))).collect(),
    };
    let frames_of: Vec<(usize, usize)> = shapes
        .get("shapes")
        .items()
        .iter()
        .map(|s| (s.get("firstVertex").as_i64().unwrap_or(0) as usize, s.get("vertexCount").as_i64().unwrap_or(0) as usize))
        .collect();
    let full: Vec<f32> = shapes.get("fullWeights").items().iter().map(|w| w.f32_or(100.0)).collect();
    let mut blend_shapes = Vec::new();
    for c in shapes.get("channels").items() {
        let first = c.get("frameIndex").as_i64().unwrap_or(0).max(0) as usize;
        let n = c.get("frameCount").as_i64().unwrap_or(0).max(0) as usize;
        let frames = (first..first + n)
            .filter_map(|f| {
                let (a, k) = *frames_of.get(f)?;
                Some((full.get(f).copied().unwrap_or(100.0), deltas.get(a..a + k)?.to_vec()))
            })
            .collect();
        blend_shapes.push(BlendShape { name: c.get("name").str_or("").to_string(), frames, default_weight: 0.0 });
    }
    Ok(ModelMesh { name: m.get("m_Name").str_or("").to_string(), positions, normals, uvs, submeshes, skin, blend_shapes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_mesh_reads_its_streams_and_indices() {
        // One triangle: positions and normals in stream 0 (24 bytes a vertex), UV0 in stream 1.
        let mut v = Vec::new();
        for (p, n) in [([0.0f32, 0.0, 0.0], [0.0f32, 0.0, -1.0]), ([0.0, 1.0, 0.0], [0.0, 0.0, -1.0]), ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0])] {
            for c in p.iter().chain(&n) {
                v.extend_from_slice(&c.to_le_bytes());
            }
        }
        v.resize(80, 0);
        for uv in [[0.0f32, 0.0], [0.0, 1.0], [1.0, 0.0]] {
            for c in uv {
                v.extend_from_slice(&c.to_le_bytes());
            }
        }
        let hexed: String = v.iter().map(|b| format!("{b:02x}")).collect();
        let text = format!(
            "Mesh:\n  m_Name: Tri\n  m_SubMeshes:\n  - serializedVersion: 2\n    firstByte: 0\n    indexCount: 3\n    topology: 0\n    baseVertex: 0\n  m_IndexFormat: 0\n  m_IndexBuffer: 000001000200\n  m_VertexData:\n    serializedVersion: 3\n    m_VertexCount: 3\n    m_Channels:\n    - stream: 0\n      offset: 0\n      format: 0\n      dimension: 3\n    - stream: 0\n      offset: 12\n      format: 0\n      dimension: 3\n    - stream: 0\n      offset: 0\n      format: 0\n      dimension: 0\n    - stream: 0\n      offset: 0\n      format: 0\n      dimension: 0\n    - stream: 1\n      offset: 0\n      format: 0\n      dimension: 2\n    m_DataSize: {}\n    _typelessdata: {hexed}\n",
            v.len()
        );
        let m = parse(yaml::parse(&text).get("Mesh")).unwrap();
        assert_eq!(m.positions, [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]]);
        assert_eq!(m.normals[2], [0.0, 0.0, -1.0]);
        assert_eq!(m.uvs[1], [0.0, 1.0]);
        assert_eq!(m.submeshes, [vec![0, 1, 2]]);
        assert!(m.skin.is_none());
        assert!(parse(yaml::parse("Mesh:\n  m_Name: Empty\n").get("Mesh")).is_err());
    }
}
