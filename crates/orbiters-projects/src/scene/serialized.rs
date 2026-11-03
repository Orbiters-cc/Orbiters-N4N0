//! Unity's binary serialization (a `SerializedFile`): assets a tool saved in binary even in a Force Text project
//! (MCB's generated bodies, 160 MB `.asset` files), read through the type trees the editor writes into them, into
//! the same [`Value`] tree the YAML reader gives, so that [`super::mesh_asset`] reads both alike. Byte arrays come
//! out as [`Value::Bytes`]; so do large arrays of plain records (a mesh's blend shape vertices, millions of them),
//! left for their reader to decode by their layout.
//!
//! The layout (Unity 2019 to 2022, versions 17 to 22): a big-endian header (version 22 moved sizes to 64 bits), the
//! metadata (Unity's version, the target, then each type: class ID, script index, hash and its type tree as nodes
//! with a string table, names under `0x80000000` being Unity's common strings), the objects (path ID, where their data
//! is and their type), then each object's fields in the order of its type tree, little-endian, aligned to 4 bytes after
//! fields flagged so.

use super::yaml::Value;
use anyhow::{bail, ensure, Context as _};

/// Unity's common strings, in order: a type tree names a field by an offset into this when its high bit is set.
const COMMON: &[&str] = &[
    "AABB",
    "AnimationClip",
    "AnimationCurve",
    "AnimationState",
    "Array",
    "Base",
    "BitField",
    "bitset",
    "bool",
    "char",
    "ColorRGBA",
    "Component",
    "data",
    "deque",
    "double",
    "dynamic_array",
    "FastPropertyName",
    "first",
    "float",
    "Font",
    "GameObject",
    "Generic Mono",
    "GradientNEW",
    "GUID",
    "GUIStyle",
    "int",
    "list",
    "long long",
    "map",
    "Matrix4x4f",
    "MdFour",
    "MonoBehaviour",
    "MonoScript",
    "m_ByteSize",
    "m_Curve",
    "m_EditorClassIdentifier",
    "m_EditorHideFlags",
    "m_Enabled",
    "m_ExtensionPtr",
    "m_GameObject",
    "m_Index",
    "m_IsArray",
    "m_IsStatic",
    "m_MetaFlag",
    "m_Name",
    "m_ObjectHideFlags",
    "m_PrefabInternal",
    "m_PrefabParentObject",
    "m_Script",
    "m_StaticEditorFlags",
    "m_Type",
    "m_Version",
    "Object",
    "pair",
    "PPtr<Component>",
    "PPtr<GameObject>",
    "PPtr<Material>",
    "PPtr<MonoBehaviour>",
    "PPtr<MonoScript>",
    "PPtr<Object>",
    "PPtr<Prefab>",
    "PPtr<Sprite>",
    "PPtr<TextAsset>",
    "PPtr<Texture>",
    "PPtr<Texture2D>",
    "PPtr<Transform>",
    "Prefab",
    "Quaternionf",
    "Rectf",
    "RectInt",
    "RectOffset",
    "second",
    "set",
    "short",
    "size",
    "SInt16",
    "SInt32",
    "SInt64",
    "SInt8",
    "staticvector",
    "string",
    "TextAsset",
    "TextMesh",
    "Texture",
    "Texture2D",
    "Transform",
    "TypelessData",
    "UInt16",
    "UInt32",
    "UInt64",
    "UInt8",
    "unsigned int",
    "unsigned long long",
    "unsigned short",
    "vector",
    "Vector2f",
    "Vector3f",
    "Vector4f",
    "m_ScriptingClassIdentifier",
    "Gradient",
    "Type*",
    "int2_storage",
    "int3_storage",
    "BoundsInt",
    "m_CorrespondingSourceObject",
    "m_PrefabInstance",
    "m_PrefabAsset",
    "FileSize",
    "Hash128",
    "RenderingLayerMask",
];

fn common(offset: u32) -> Option<&'static str> {
    let mut at = 0u32;
    for s in COMMON {
        if at == offset {
            return Some(s);
        }
        at += s.len() as u32 + 1;
    }
    None
}

/// Whether a file starts as a binary `SerializedFile`.
pub fn is_serialized(b: &[u8]) -> bool {
    b.len() > 48 && {
        let v = u32::from_be_bytes(b[8..12].try_into().unwrap());
        (9..=50).contains(&v)
    }
}

#[derive(Clone, Debug)]
struct Node {
    ty: String,
    name: String,
    size: i32,
    level: u8,
    flags: u8,
    meta: u32,
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
    big: bool,
    /// Values made so far ([`MAX_VALUES`]).
    values: usize,
}

/// The most values one object is read into: a mesh of four million vertices, its blend shapes and bones make a few
/// thousand (its large arrays stay bytes), so this only stops a damaged file whose nested arrays of small records
/// would make billions (tens of gigabytes).
const MAX_VALUES: usize = 4_000_000;

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> anyhow::Result<&'a [u8]> {
        let s = self.b.get(self.at..self.at.checked_add(n).context("cut short")?).context("cut short")?;
        self.at += n;
        Ok(s)
    }
    fn u8(&mut self) -> anyhow::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> anyhow::Result<u16> {
        let s: [u8; 2] = self.take(2)?.try_into()?;
        Ok(if self.big { u16::from_be_bytes(s) } else { u16::from_le_bytes(s) })
    }
    fn u32(&mut self) -> anyhow::Result<u32> {
        let s: [u8; 4] = self.take(4)?.try_into()?;
        Ok(if self.big { u32::from_be_bytes(s) } else { u32::from_le_bytes(s) })
    }
    fn i32(&mut self) -> anyhow::Result<i32> {
        Ok(self.u32()? as i32)
    }
    fn u64(&mut self) -> anyhow::Result<u64> {
        let s: [u8; 8] = self.take(8)?.try_into()?;
        Ok(if self.big { u64::from_be_bytes(s) } else { u64::from_le_bytes(s) })
    }
    fn cstr(&mut self) -> anyhow::Result<String> {
        let rest = self.b.get(self.at..).context("cut short")?;
        let n = rest.iter().position(|&c| c == 0).context("an unterminated string")?;
        let s = String::from_utf8_lossy(&rest[..n]).into_owned();
        self.at += n + 1;
        Ok(s)
    }
    fn align(&mut self) {
        self.at = self.at.div_ceil(4) * 4;
    }
}

struct Type {
    class: i32,
    nodes: Vec<Node>,
}

/// The object `path_id` of a binary serialized file (else its first of class `class`): its class and fields.
pub fn object(b: &[u8], path_id: i64, class: i32) -> anyhow::Result<(i32, Value)> {
    ensure!(is_serialized(b), "not a Unity serialized file");
    let mut h = Reader { b, at: 0, big: true, values: 0 };
    let _meta_size = h.u32()?;
    let _file_size = h.u32()?;
    let version = h.u32()?;
    let mut data_offset = h.u32()? as u64;
    ensure!((17..=22).contains(&version), "Unity's binary format version {version} (17 to 22 are read)");
    let big = h.u8()? != 0;
    h.take(3)?;
    if version >= 22 {
        let _meta = h.u32()?;
        let _size = h.u64()?;
        data_offset = h.u64()?;
        h.u64()?;
    }
    let mut r = Reader { b, at: h.at, big, values: 0 };
    let _unity = r.cstr()?;
    let _platform = r.u32()?;
    let trees = r.u8()? != 0;
    ensure!(trees, "saved without type trees");
    let types_n = r.i32()?;
    ensure!((0..10_000).contains(&types_n), "{types_n} types");
    let mut types = Vec::new();
    for _ in 0..types_n {
        let class_id = r.i32()?;
        let _stripped = r.u8()?;
        let _script = r.u16()?;
        if class_id == 114 {
            r.take(16)?;
        }
        r.take(16)?;
        let nodes_n = r.i32()?;
        let strings_n = r.i32()?;
        ensure!((0..100_000).contains(&nodes_n) && (0..10_000_000).contains(&strings_n), "a type tree of {nodes_n} nodes");
        let node_size = if version >= 19 { 32 } else { 24 };
        let raw = r.take(nodes_n as usize * node_size)?;
        let strings = r.take(strings_n as usize)?;
        let name = |off: u32| -> String {
            if off & 0x8000_0000 != 0 {
                return common(off & 0x7fff_ffff).unwrap_or("?").to_string();
            }
            let s = strings.get(off as usize..).unwrap_or(&[]);
            String::from_utf8_lossy(s.split(|&c| c == 0).next().unwrap_or(&[])).into_owned()
        };
        let mut nodes = Vec::with_capacity(nodes_n as usize);
        for k in 0..nodes_n as usize {
            let mut n = Reader { b: &raw[k * node_size..(k + 1) * node_size], at: 0, big, values: 0 };
            let _v = n.u16()?;
            let level = n.u8()?;
            let flags = n.u8()?;
            let ty = n.u32()?;
            let nm = n.u32()?;
            let size = n.i32()?;
            let _index = n.i32()?;
            let meta = n.u32()?;
            nodes.push(Node { ty: name(ty), name: name(nm), size, level, flags, meta });
        }
        if version >= 21 {
            let deps = r.i32()?;
            ensure!((0..100_000).contains(&deps), "{deps} type dependencies");
            r.take(deps as usize * 4)?;
        }
        types.push(Type { class: class_id, nodes });
    }
    let objects_n = r.i32()?;
    ensure!((0..10_000_000).contains(&objects_n), "{objects_n} objects");
    let mut found: Option<(u64, u32, usize)> = None;
    let mut first_of_class: Option<(u64, u32, usize)> = None;
    for _ in 0..objects_n {
        r.align();
        let id = r.u64()? as i64;
        let start = if version >= 22 { r.u64()? } else { r.u32()? as u64 };
        let size = r.u32()?;
        let ty = r.i32()? as usize;
        if id == path_id {
            found = Some((start, size, ty));
        }
        if first_of_class.is_none() && types.get(ty).is_some_and(|t| t.class == class) {
            first_of_class = Some((start, size, ty));
        }
    }
    let (start, size, ty) = found.or(first_of_class).context("the object is not in the file")?;
    let t = types.get(ty).context("an object of an unknown type")?;
    let begin = usize::try_from(data_offset.checked_add(start).context("an object past the file's end")?)?;
    let data = b.get(begin..begin.checked_add(size as usize).context("an object past the file's end")?).context("an object past the file's end")?;
    let mut d = Reader { b: data, at: 0, big, values: 0 };
    let (value, _) = read(&t.nodes, 0, &mut d, 0)?;
    Ok((t.class, value))
}

/// The children of node `i` (the nodes one level down until the level comes back up).
fn children(nodes: &[Node], i: usize) -> Vec<usize> {
    let level = nodes[i].level;
    let mut out = Vec::new();
    let mut k = i + 1;
    while k < nodes.len() && nodes[k].level > level {
        if nodes[k].level == level + 1 {
            out.push(k);
        }
        k += 1;
    }
    out
}

/// The node after `i`'s subtree.
fn after(nodes: &[Node], i: usize) -> usize {
    let mut k = i + 1;
    while k < nodes.len() && nodes[k].level > nodes[i].level {
        k += 1;
    }
    k
}

/// A record of plain values only (no arrays, strings or alignment inside): its size in bytes.
fn plain_size(nodes: &[Node], i: usize) -> Option<usize> {
    let kids = children(nodes, i);
    if nodes[i].meta & 0x4000 != 0 || nodes[i].flags & 1 != 0 || nodes[i].ty == "Array" || nodes[i].ty == "string" {
        return None;
    }
    if kids.is_empty() {
        return (nodes[i].size > 0).then_some(nodes[i].size as usize);
    }
    kids.iter().map(|&k| plain_size(nodes, k)).sum()
}

fn primitive(ty: &str, r: &mut Reader, size: i32) -> anyhow::Result<Value> {
    let s = match ty {
        "bool" | "UInt8" | "char" => r.u8()?.to_string(),
        "SInt8" => (r.u8()? as i8).to_string(),
        "SInt16" | "short" => (r.u16()? as i16).to_string(),
        "UInt16" | "unsigned short" => r.u16()?.to_string(),
        "int" | "SInt32" | "Type*" => r.i32()?.to_string(),
        "UInt32" | "unsigned int" => r.u32()?.to_string(),
        "float" => f32::from_bits(r.u32()?).to_string(),
        "double" => f64::from_bits(r.u64()?).to_string(),
        "SInt64" | "long long" => (r.u64()? as i64).to_string(),
        "UInt64" | "unsigned long long" | "FileSize" => r.u64()?.to_string(),
        _ => {
            r.take(size.max(0) as usize)?;
            String::new()
        }
    };
    Ok(Value::Str(s))
}

const MAX_DEPTH: usize = 48;

/// Reads node `i` from `r`: its value and the node after its subtree.
fn read(nodes: &[Node], i: usize, r: &mut Reader, depth: usize) -> anyhow::Result<(Value, usize)> {
    ensure!(depth < MAX_DEPTH, "a type tree too deep");
    r.values += 1;
    ensure!(r.values <= MAX_VALUES, "more than {MAX_VALUES} values in one object");
    let node = &nodes[i];
    let kids = children(nodes, i);
    let value = if node.ty == "string" {
        let n = r.i32()?;
        ensure!(n >= 0, "a string of {n} bytes");
        let s = String::from_utf8_lossy(r.take(n as usize)?).into_owned();
        if kids.iter().any(|&k| nodes[k].meta & 0x4000 != 0) {
            r.align();
        }
        Value::Str(s)
    } else if node.ty == "TypelessData" {
        let n = r.i32()?;
        ensure!(n >= 0, "{n} bytes");
        Value::Bytes(r.take(n as usize)?.to_vec())
    } else if node.flags & 1 != 0 || node.ty == "Array" {
        let n = r.i32()?;
        ensure!(n >= 0, "an array of {n}");
        let element = *kids.get(1).context("an array without its element")?;
        let plain = plain_size(nodes, element);
        // Bytes, and long runs of plain records, as they are.
        let v = match plain {
            Some(size) if size == 1 || n > 4096 => Value::Bytes(r.take(size.checked_mul(n as usize).context("cut short")?)?.to_vec()),
            _ => {
                let mut items = Vec::with_capacity((n as usize).min(1 << 16));
                for k in 0..n {
                    let at = r.at;
                    items.push(read(nodes, element, r, depth + 1)?.0);
                    // Records that take no bytes (a damaged type tree) would repeat for as long as the count says,
                    // two billion times: one says what they all are.
                    if k == 0 && r.at == at {
                        break;
                    }
                }
                Value::Seq(items)
            }
        };
        if node.meta & 0x4000 != 0 {
            r.align();
        }
        return Ok((v, after(nodes, i)));
    } else if kids.is_empty() {
        primitive(&node.ty, r, node.size)?
    } else if kids.len() == 1 && (nodes[kids[0]].flags & 1 != 0 || nodes[kids[0]].ty == "Array") {
        // vector, set, map, staticvector: the array itself.
        read(nodes, kids[0], r, depth + 1)?.0
    } else if node.ty.starts_with("PPtr<") {
        let mut fields = Vec::new();
        for &k in &kids {
            let (v, _) = read(nodes, k, r, depth + 1)?;
            let key = match nodes[k].name.as_str() {
                "m_FileID" => "fileID".to_string(),
                "m_PathID" => "pathID".to_string(),
                other => other.to_string(),
            };
            fields.push((key, v));
        }
        Value::Map(fields)
    } else {
        let mut fields = Vec::with_capacity(kids.len());
        for &k in &kids {
            let (v, _) = read(nodes, k, r, depth + 1)?;
            fields.push((nodes[k].name.clone(), v));
        }
        Value::Map(fields)
    };
    if node.meta & 0x4000 != 0 {
        r.align();
    }
    Ok((value, after(nodes, i)))
}

/// Reads a binary file's object for a reference (its path ID is the reference's file ID).
pub fn read_file(path: &std::path::Path, file_id: i64, class: i32, max_bytes: u64) -> anyhow::Result<Value> {
    let size = std::fs::metadata(path)?.len();
    ensure!(size <= max_bytes, "{} MB", size >> 20);
    let bytes = std::fs::read(path)?;
    let (c, v) = object(&bytes, file_id, class)?;
    if c != class {
        bail!("a reference to a class {c} object, not {class}");
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unitys_common_strings_sit_where_type_trees_point() {
        assert_eq!(common(0), Some("AABB"));
        assert_eq!(common(49), Some("Array"));
        assert_eq!(common(106), Some("data"));
        assert_eq!(common(222), Some("int"));
        assert_eq!(common(427), Some("m_Name"));
        assert_eq!(common(795), Some("size"));
        assert_eq!(common(840), Some("string"));
        assert_eq!(common(928), Some("UInt8"));
        assert_eq!(common(981), Some("vector"));
        assert_eq!(common(1), None);
    }

    /// A version 22 file with one object of a small type: `{ m_Name: string, m_Bytes: vector<UInt8>, m_Value: float }`.
    fn tiny() -> Vec<u8> {
        tiny_with(false, 5)
    }

    /// [`tiny`], its byte array's elements of no size (`empty`, a damaged type tree) and `count` of them.
    fn tiny_with(empty: bool, count: i32) -> Vec<u8> {
        let mut strings = Vec::new();
        let mut local = |s: &str| {
            let off = strings.len() as u32;
            strings.extend_from_slice(s.as_bytes());
            strings.push(0);
            off
        };
        let thing = local("Thing");
        let bytes_name = local("m_Bytes");
        let value_name = local("m_Value");
        let c = |s: &str| {
            let mut at = 0u32;
            for x in COMMON {
                if *x == s {
                    return at | 0x8000_0000;
                }
                at += x.len() as u32 + 1;
            }
            panic!("{s}")
        };
        // (level, flags, type, name, size, meta)
        let nodes = [
            (0u8, 0u8, thing, c("Base"), -1i32, 0u32),
            (1, 0, c("string"), c("m_Name"), -1, 0x8000),
            (2, 1, c("Array"), c("Array"), -1, 0x4001),
            (3, 0, c("int"), c("size"), 4, 0),
            (3, 0, c("char"), c("data"), 1, 0),
            (1, 0, c("vector"), bytes_name, -1, 0x4000),
            (2, 1, c("Array"), c("Array"), -1, 0x4001),
            (3, 0, c("int"), c("size"), 4, 0),
            if empty { (3, 0, thing, c("data"), 0, 0) } else { (3, 0, c("UInt8"), c("data"), 1, 0) },
            (1, 0, c("float"), value_name, 4, 0),
        ];
        let mut meta = Vec::new();
        meta.extend_from_slice(b"2022.3.22f1\0");
        meta.extend_from_slice(&(-2i32).to_le_bytes());
        meta.push(1);
        meta.extend_from_slice(&1i32.to_le_bytes());
        meta.extend_from_slice(&43i32.to_le_bytes());
        meta.push(0);
        meta.extend_from_slice(&(-1i16).to_le_bytes());
        meta.extend_from_slice(&[0; 16]);
        meta.extend_from_slice(&(nodes.len() as i32).to_le_bytes());
        meta.extend_from_slice(&(strings.len() as i32).to_le_bytes());
        for (level, flags, ty, name, size, m) in nodes {
            meta.extend_from_slice(&1u16.to_le_bytes());
            meta.push(level);
            meta.push(flags);
            meta.extend_from_slice(&ty.to_le_bytes());
            meta.extend_from_slice(&name.to_le_bytes());
            meta.extend_from_slice(&size.to_le_bytes());
            meta.extend_from_slice(&0i32.to_le_bytes());
            meta.extend_from_slice(&m.to_le_bytes());
            meta.extend_from_slice(&0u64.to_le_bytes());
        }
        meta.extend_from_slice(&strings);
        meta.extend_from_slice(&0i32.to_le_bytes());
        meta.extend_from_slice(&1i32.to_le_bytes());
        while (48 + meta.len()) % 4 != 0 {
            meta.push(0);
        }
        meta.extend_from_slice(&7i64.to_le_bytes());
        meta.extend_from_slice(&0u64.to_le_bytes());
        let mut obj = Vec::new();
        obj.extend_from_slice(&3i32.to_le_bytes());
        obj.extend_from_slice(b"Abc\0");
        obj.extend_from_slice(&count.to_le_bytes());
        obj.extend_from_slice(&[1, 2, 3, 4, 5, 0, 0, 0]);
        obj.extend_from_slice(&2.5f32.to_le_bytes());
        meta.extend_from_slice(&(obj.len() as u32).to_le_bytes());
        meta.extend_from_slice(&0i32.to_le_bytes());
        let data_offset = (48 + meta.len()).div_ceil(16) * 16;
        let mut f = Vec::new();
        f.extend_from_slice(&0u32.to_be_bytes());
        f.extend_from_slice(&0u32.to_be_bytes());
        f.extend_from_slice(&22u32.to_be_bytes());
        f.extend_from_slice(&0u32.to_be_bytes());
        f.extend_from_slice(&[0, 0, 0, 0]);
        f.extend_from_slice(&(meta.len() as u32).to_be_bytes());
        f.extend_from_slice(&0u64.to_be_bytes());
        f.extend_from_slice(&(data_offset as u64).to_be_bytes());
        f.extend_from_slice(&0u64.to_be_bytes());
        f.extend_from_slice(&meta);
        f.resize(data_offset, 0);
        f.extend_from_slice(&obj);
        f
    }

    #[test]
    fn damaged_files_never_panic_or_hang() {
        let start = std::time::Instant::now();
        for b in crate::scene::damaged(&tiny(), 3000) {
            let _ = object(&b, 7, 43);
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "{:?}", start.elapsed());
    }

    #[test]
    fn records_of_no_size_are_not_repeated_two_billion_times() {
        // An array of i32::MAX elements that take no bytes: once two billion values (and a minute); now one.
        let start = std::time::Instant::now();
        let (_, v) = object(&tiny_with(true, i32::MAX), 7, 43).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(v.get("m_Bytes").items().len(), 1);
    }

    #[test]
    fn a_binary_object_reads_through_its_type_tree() {
        let (class, v) = object(&tiny(), 7, 43).unwrap();
        assert_eq!(class, 43);
        assert_eq!(v.get("m_Name").as_str(), Some("Abc"));
        assert_eq!(v.get("m_Bytes"), &Value::Bytes(vec![1, 2, 3, 4, 5]));
        assert_eq!(v.get("m_Value").as_f32(), Some(2.5));
        let mut cut = tiny();
        cut.truncate(cut.len() - 6);
        assert!(object(&cut, 7, 43).is_err());
        assert!(object(b"%YAML 1.1", 7, 43).is_err());
    }
}
