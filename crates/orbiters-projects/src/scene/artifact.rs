//! What Unity made of a model when it imported it, read from its `Library`: a project opened once keeps every imported
//! asset as a binary file of its own (`Library/Artifacts/<first two digits>/<hash>`), holding the GameObjects,
//! transforms, renderers, meshes and materials the import made, under the very IDs prefabs and scenes refer to them by.
//!
//! That is the only way to read a model only Unity's importer understands: a `.blend` file is opened by Blender at
//! import (Enzo's Avatorbit world is one, 130 MB of artifact out of a 37 MB compressed Blender file), and nothing of it
//! can be read without Blender and the modifiers it applies. It is also what lightmaps need, since their UV set may
//! have been generated at import (`generateSecondaryUV`), and it is the import's exact result, so it is preferred for
//! any model whose artifact is at least as new as the file and its `.meta`.
//!
//! Which file holds an asset's artifact comes from `Library/ArtifactDB`, an LMDB file of hundreds of megabytes, read
//! here a page at a time without LMDB: `CurrentRevisions` (every asset's current artifact, keyed by a hash, so it is
//! walked once) and `ArtifactIDToArtifactMetaInfo` (looked up per artifact, its record holding the file's hash).
//! Unity may have the project open: the file is only read, from the newer of its two meta pages, and anything that
//! does not add up is a model not found, never a failed preview.

use orbiters_core::locks::*;
use super::model::ModelMesh;
use super::prefab::{self, Graph, MatSource, MeshSource, Obj};
use super::serialized::{ObjectInfo, SerializedFile};
use super::yaml::{Ref, Value};
use anyhow::{ensure, Context as _};
use orbiters_i18n::tr;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAGIC: u32 = 0xBEEF_C0DE;
const HEADER: usize = 16;
/// Where a metadata record of `ArtifactIDToArtifactMetaInfo` holds the hash of its first file.
const META_FILE_HASH: usize = 96;
/// Pages read from the database, at most (each is read once): 2 GB, far more than the tables looked at hold.
const MAX_PAGES: usize = 1 << 19;

/// A database read a page at a time.
struct Pages {
    file: RefCell<std::fs::File>,
    page: usize,
    pages: u64,
    cache: RefCell<HashMap<u64, std::rc::Rc<Vec<u8>>>>,
}

impl Pages {
    fn open(path: &Path) -> Option<Self> {
        let mut file = std::fs::File::open(path).ok()?;
        let size = file.metadata().ok()?.len();
        let mut head = [0u8; 64];
        file.read_exact(&mut head).ok()?;
        let page = u32::from_le_bytes(head[40..44].try_into().ok()?) as usize;
        if !(512..=65536).contains(&page) || !page.is_power_of_two() {
            return None;
        }
        Some(Self { file: RefCell::new(file), page, pages: size / page as u64, cache: RefCell::new(HashMap::new()) })
    }

    fn get(&self, n: u64) -> Option<std::rc::Rc<Vec<u8>>> {
        if n >= self.pages {
            return None;
        }
        if let Some(p) = self.cache.borrow().get(&n) {
            return Some(p.clone());
        }
        let mut cache = self.cache.borrow_mut();
        if cache.len() >= MAX_PAGES {
            return None;
        }
        let mut b = vec![0u8; self.page];
        let mut f = self.file.borrow_mut();
        f.seek(SeekFrom::Start(n * self.page as u64)).ok()?;
        f.read_exact(&mut b).ok()?;
        let p = std::rc::Rc::new(b);
        cache.insert(n, p.clone());
        Some(p)
    }

    /// The tree root of each named table, from the newer meta page.
    fn tables(&self) -> Option<HashMap<Vec<u8>, u64>> {
        let (mut best, mut root) = (None, 0u64);
        for m in 0..2u64 {
            let p = self.get(m)?;
            if u32::from_le_bytes(p.get(HEADER..HEADER + 4)?.try_into().ok()?) != MAGIC {
                continue;
            }
            let txn = u64::from_le_bytes(p.get(144..152)?.try_into().ok()?);
            if best.is_none_or(|b| txn > b) {
                best = Some(txn);
                root = u64::from_le_bytes(p.get(128..136)?.try_into().ok()?);
            }
        }
        best?;
        let mut out = HashMap::new();
        self.walk(root, 0, &mut |key, flags, value| {
            if flags & 0x02 != 0 && value.len() >= 48 {
                let name = key.strip_suffix(&[0]).unwrap_or(key).to_vec();
                if let Ok(b) = value[40..48].try_into() {
                    out.insert(name, u64::from_le_bytes(b));
                }
            }
            true
        })?;
        Some(out)
    }

    /// Calls `f` with each (key, flags, value, first bytes for one on overflow pages) under `pg`, until it says stop.
    fn walk(&self, pg: u64, depth: usize, f: &mut dyn FnMut(&[u8], u16, &[u8]) -> bool) -> Option<bool> {
        if depth > 32 {
            return None;
        }
        let p = self.get(pg)?;
        let (flags, lower) = (u16::from_le_bytes(p.get(10..12)?.try_into().ok()?), u16::from_le_bytes(p.get(12..14)?.try_into().ok()?) as usize);
        for i in 0..lower.checked_sub(HEADER)? / 2 {
            let node = u16::from_le_bytes(p.get(HEADER + 2 * i..HEADER + 2 * i + 2)?.try_into().ok()?) as usize;
            let h: [u8; 8] = p.get(node..node + 8)?.try_into().ok()?;
            let (lo, hi, fl, ks) =
                (u16::from_le_bytes([h[0], h[1]]) as u64, u16::from_le_bytes([h[2], h[3]]) as u64, u16::from_le_bytes([h[4], h[5]]), u16::from_le_bytes([h[6], h[7]]) as usize);
            let key = p.get(node + 8..node + 8 + ks)?;
            if flags & 0x01 != 0 {
                if !self.walk(lo | hi << 16 | (fl as u64) << 32, depth + 1, f)? {
                    return Some(false);
                }
            } else if flags & 0x02 != 0 {
                let size = (lo | hi << 16) as usize;
                let at = node + 8 + ks;
                let keep = if fl & 0x01 != 0 {
                    let over = u64::from_le_bytes(p.get(at..at + 8)?.try_into().ok()?);
                    let q = self.get(over)?;
                    f(key, fl, q.get(HEADER..(HEADER + size).min(q.len()))?)
                } else {
                    f(key, fl, p.get(at..at + size)?)
                };
                if !keep {
                    return Some(false);
                }
            }
        }
        Some(true)
    }

    /// The first bytes of the value of `key` in the tree under `pg` (keys in byte order).
    fn find(&self, pg: u64, key: &[u8], depth: usize) -> Option<Vec<u8>> {
        if depth > 32 {
            return None;
        }
        let p = self.get(pg)?;
        let (flags, lower) = (u16::from_le_bytes(p.get(10..12)?.try_into().ok()?), u16::from_le_bytes(p.get(12..14)?.try_into().ok()?) as usize);
        let keys = lower.checked_sub(HEADER)? / 2;
        let node_at = |i: usize| -> Option<(usize, u64, u64, u16, usize)> {
            let node = u16::from_le_bytes(p.get(HEADER + 2 * i..HEADER + 2 * i + 2)?.try_into().ok()?) as usize;
            let h: [u8; 8] = p.get(node..node + 8)?.try_into().ok()?;
            Some((node, u16::from_le_bytes([h[0], h[1]]) as u64, u16::from_le_bytes([h[2], h[3]]) as u64, u16::from_le_bytes([h[4], h[5]]), u16::from_le_bytes([h[6], h[7]]) as usize))
        };
        if flags & 0x01 != 0 {
            // The last branch whose key is not above: the first has no key.
            let mut chosen = 0;
            for i in 1..keys {
                let (node, _, _, _, ks) = node_at(i)?;
                if p.get(node + 8..node + 8 + ks)? <= key {
                    chosen = i;
                } else {
                    break;
                }
            }
            let (_, lo, hi, fl, _) = node_at(chosen)?;
            return self.find(lo | hi << 16 | (fl as u64) << 32, key, depth + 1);
        }
        for i in 0..keys {
            let (node, lo, hi, fl, ks) = node_at(i)?;
            if p.get(node + 8..node + 8 + ks)? != key {
                continue;
            }
            let size = (lo | hi << 16) as usize;
            let at = node + 8 + ks;
            if fl & 0x01 != 0 {
                let over = u64::from_le_bytes(p.get(at..at + 8)?.try_into().ok()?);
                let q = self.get(over)?;
                return Some(q.get(HEADER..(HEADER + size).min(q.len()))?.to_vec());
            }
            return Some(p.get(at..at + size)?.to_vec());
        }
        None
    }
}

/// A GUID as `.meta` files write it, as the databases key it: each byte's two hex digits swapped.
fn guid_bytes(guid: &str) -> Option<[u8; 16]> {
    let g = guid.as_bytes();
    if g.len() != 32 {
        return None;
    }
    let v = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = v(g[2 * i])? | v(g[2 * i + 1])? << 4;
    }
    Some(out)
}

/// Every imported asset's current artifact in a project.
pub struct ArtifactDb {
    pages: Pages,
    project: PathBuf,
    /// An asset's GUID (the database's bytes) → its current artifact's ID.
    current: HashMap<[u8; 16], [u8; 16]>,
    info: u64,
}

impl ArtifactDb {
    /// Reads the project's database; none when it has no usable one.
    pub fn open(project: &Path) -> Option<Self> {
        let pages = Pages::open(&project.join("Library/ArtifactDB"))?;
        let tables = pages.tables()?;
        let &revisions = tables.get(b"CurrentRevisions".as_slice())?;
        let &info = tables.get(b"ArtifactIDToArtifactMetaInfo".as_slice())?;
        let mut current = HashMap::new();
        pages.walk(revisions, 0, &mut |_, _, v| {
            // The asset's GUID first, the artifact's ID last.
            if v.len() >= 32 {
                if let (Ok(g), Ok(a)) = (v[..16].try_into(), v[v.len() - 16..].try_into()) {
                    current.insert(g, a);
                }
            }
            true
        })?;
        Some(Self { pages, project: project.to_path_buf(), current, info })
    }

    /// The file holding the current artifact of the asset `guid`, when it is there.
    pub fn file_of(&self, guid: &str) -> Option<PathBuf> {
        let id = self.current.get(&guid_bytes(guid)?)?;
        let record = self.pages.find(self.info, id, 0)?;
        let hash = record.get(META_FILE_HASH..META_FILE_HASH + 16)?;
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        let path = self.project.join("Library/Artifacts").join(&hex[..2]).join(&hex);
        path.is_file().then_some(path)
    }
}

/// An imported model's artifact: its objects' list, the objects read as they are needed.
pub struct Artifact {
    pub path: PathBuf,
    /// The asset's GUID.
    pub guid: String,
    file: Mutex<std::fs::File>,
    info: SerializedFile,
}

impl Artifact {
    pub fn open(path: &Path, guid: &str, max_bytes: u64) -> anyhow::Result<Self> {
        let mut file = std::fs::File::open(path)?;
        let size = file.metadata()?.len();
        ensure!(size <= max_bytes, "{} MB", size >> 20);
        let mut head = vec![0u8; SerializedFile::HEAD];
        file.read_exact(&mut head)?;
        let meta = SerializedFile::head_size(&head)?;
        ensure!(meta <= size && meta <= 256 << 20, "{}", tr!("metadata of {size} MB", size = meta >> 20));
        let mut b = vec![0u8; meta as usize];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut b)?;
        let info = SerializedFile::parse(&b)?;
        Ok(Self { path: path.to_path_buf(), guid: guid.to_string(), file: Mutex::new(file), info })
    }

    fn read(&self, o: &ObjectInfo) -> anyhow::Result<Value> {
        let mut data = vec![0u8; o.size as usize];
        {
            let mut f = self.file.locked();
            f.seek(SeekFrom::Start(self.info.data_offset + o.start))?;
            f.read_exact(&mut data)?;
        }
        self.info.value(o, &data)
    }

    pub fn object(&self, id: i64, class: i32) -> anyhow::Result<Value> {
        let o = self.info.objects().iter().find(|o| o.id == id && (class == 0 || o.class == class)).with_context(|| tr("an object its import did not make"))?;
        self.read(o)
    }

    /// A mesh the import made, by Unity's ID of it.
    pub fn mesh(&self, id: i64) -> anyhow::Result<ModelMesh> {
        super::mesh_asset::parse(&self.object(id, 43)?)
    }

    /// The model as prefabs see it: its objects, under the IDs they refer to them by.
    pub fn graph(&self) -> anyhow::Result<Graph> {
        let mut g = Graph::default();
        // In the file's order, which is near enough the disk's: small objects, tens of thousands in a large rig.
        let mut list: Vec<ObjectInfo> = self.info.objects().iter().filter(|o| matches!(o.class, 1 | 4 | 23 | 33 | 108 | 137 | 205)).copied().collect();
        list.sort_by_key(|o| o.start);
        for o in &list {
            let v = self.read(o)?;
            let Some(mut obj) = prefab::object_of(o.class, &v) else { continue };
            self.own(&mut obj);
            g.objects.insert(o.id, obj);
        }
        Ok(g)
    }

    /// References to objects of this very file as a text file would write them, with its GUID.
    fn own(&self, obj: &mut Obj) {
        let own = |r: &mut Ref| {
            if r.guid.is_none() && r.file_id != 0 {
                r.guid = Some(self.guid.clone());
            }
        };
        match obj {
            Obj::Renderer(r) => {
                if let MeshSource::Asset(m) = &mut r.mesh {
                    own(m);
                }
                for m in &mut r.materials {
                    if let MatSource::Asset(a) = m {
                        own(a);
                    }
                }
            }
            Obj::Filter(f) => {
                if let MeshSource::Asset(m) = &mut f.mesh {
                    own(m);
                }
            }
            _ => {}
        }
    }

    /// How many objects of a class the import made (diagnostics).
    pub fn count(&self, class: i32) -> usize {
        self.info.objects().iter().filter(|o| o.class == class).count()
    }

    pub fn ids(&self, class: i32) -> Vec<i64> {
        self.info.objects().iter().filter(|o| o.class == class).map(|o| o.id).collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A field of an object in a small binary file written for tests.
    #[derive(Clone)]
    pub(crate) enum F {
        Int(&'static str, i32),
        Byte(&'static str, u8),
        Float(&'static str, f32),
        Text(&'static str, &'static str),
        /// `PPtr`: the other file (0 this one, else one-based into the file's list) and the object's ID.
        Ptr(&'static str, i32, i64),
        /// `vector<UInt8>`.
        Bytes(&'static str, Vec<u8>),
        /// `TypelessData`.
        Typeless(&'static str, Vec<u8>),
        Rec(&'static str, Vec<F>),
        /// A list of records, at least one (the first says what they all are).
        List(&'static str, Vec<Vec<F>>),
        /// A list of `PPtr`s.
        Ptrs(&'static str, Vec<(i32, i64)>),
    }

    struct Tree {
        strings: Vec<u8>,
        nodes: Vec<(u8, u8, u32, u32, i32, u32)>,
    }

    impl Tree {
        fn text(&mut self, s: &str) -> u32 {
            let off = self.strings.len() as u32;
            self.strings.extend_from_slice(s.as_bytes());
            self.strings.push(0);
            off
        }
        fn node(&mut self, level: u8, flags: u8, ty: &str, name: &str, size: i32, meta: u32) {
            let (t, n) = (self.text(ty), self.text(name));
            self.nodes.push((level, flags, t, n, size, meta));
        }
        fn array(&mut self, level: u8, element: &str, size: i32) {
            self.node(level, 1, "Array", "Array", -1, 0x4000);
            self.node(level + 1, 0, "int", "size", 4, 0);
            self.node(level + 1, 0, element, "data", size, 0);
        }
        fn field(&mut self, f: &F, level: u8) {
            match f {
                F::Int(n, _) => self.node(level, 0, "int", n, 4, 0),
                F::Byte(n, _) => self.node(level, 0, "UInt8", n, 1, 0x4000),
                F::Float(n, _) => self.node(level, 0, "float", n, 4, 0),
                F::Text(n, _) => {
                    self.node(level, 0, "string", n, -1, 0x8000);
                    self.array(level + 1, "char", 1);
                }
                F::Ptr(n, _, _) => {
                    self.node(level, 0, "PPtr<Object>", n, 12, 0);
                    self.node(level + 1, 0, "int", "m_FileID", 4, 0);
                    self.node(level + 1, 0, "SInt64", "m_PathID", 8, 0);
                }
                F::Bytes(n, _) => {
                    self.node(level, 0, "vector", n, -1, 0x4000);
                    self.array(level + 1, "UInt8", 1);
                }
                F::Typeless(n, _) => {
                    self.node(level, 0, "TypelessData", n, -1, 0x4000);
                    self.node(level + 1, 0, "int", "size", 4, 0);
                    self.node(level + 1, 0, "UInt8", "data", 1, 0);
                }
                F::Rec(n, fields) => {
                    self.node(level, 0, "Rec", n, -1, 0);
                    for x in fields {
                        self.field(x, level + 1);
                    }
                }
                F::List(n, items) => {
                    self.node(level, 0, "vector", n, -1, 0);
                    self.node(level + 1, 1, "Array", "Array", -1, 0);
                    self.node(level + 2, 0, "int", "size", 4, 0);
                    self.node(level + 2, 0, "Item", "data", -1, 0);
                    for x in &items[0] {
                        self.field(x, level + 3);
                    }
                }
                F::Ptrs(n, _) => {
                    self.node(level, 0, "vector", n, -1, 0);
                    self.node(level + 1, 1, "Array", "Array", -1, 0);
                    self.node(level + 2, 0, "int", "size", 4, 0);
                    self.node(level + 2, 0, "PPtr<Object>", "data", 12, 0);
                    self.node(level + 3, 0, "int", "m_FileID", 4, 0);
                    self.node(level + 3, 0, "SInt64", "m_PathID", 8, 0);
                }
            }
        }
    }

    fn data_of(f: &F, out: &mut Vec<u8>) {
        let align = |out: &mut Vec<u8>| out.resize(out.len().div_ceil(4) * 4, 0);
        match f {
            F::Int(_, v) => out.extend_from_slice(&v.to_le_bytes()),
            F::Byte(_, v) => {
                out.push(*v);
                align(out);
            }
            F::Float(_, v) => out.extend_from_slice(&v.to_le_bytes()),
            F::Text(_, v) => {
                out.extend_from_slice(&(v.len() as i32).to_le_bytes());
                out.extend_from_slice(v.as_bytes());
                align(out);
            }
            F::Ptr(_, file, id) => {
                out.extend_from_slice(&file.to_le_bytes());
                out.extend_from_slice(&id.to_le_bytes());
            }
            F::Bytes(_, b) | F::Typeless(_, b) => {
                out.extend_from_slice(&(b.len() as i32).to_le_bytes());
                out.extend_from_slice(b);
                align(out);
            }
            F::Rec(_, fields) => fields.iter().for_each(|x| data_of(x, out)),
            F::List(_, items) => {
                out.extend_from_slice(&(items.len() as i32).to_le_bytes());
                for item in items {
                    item.iter().for_each(|x| data_of(x, out));
                }
            }
            F::Ptrs(_, items) => {
                out.extend_from_slice(&(items.len() as i32).to_le_bytes());
                for (file, id) in items {
                    out.extend_from_slice(&file.to_le_bytes());
                    out.extend_from_slice(&id.to_le_bytes());
                }
            }
        }
    }

    /// A version 22 serialized file of `objects` (ID, class, fields), each class one type, naming `externals`.
    pub(crate) fn serialized(objects: &[(i64, i32, Vec<F>)], externals: &[&str]) -> Vec<u8> {
        let mut classes: Vec<i32> = objects.iter().map(|o| o.1).collect();
        classes.sort();
        classes.dedup();
        let mut meta = Vec::new();
        meta.extend_from_slice(b"2022.3.22f1\0");
        meta.extend_from_slice(&(-2i32).to_le_bytes());
        meta.push(1);
        meta.extend_from_slice(&(classes.len() as i32).to_le_bytes());
        for class in &classes {
            let fields = &objects.iter().find(|o| o.1 == *class).unwrap().2;
            let mut t = Tree { strings: Vec::new(), nodes: Vec::new() };
            t.node(0, 0, "Thing", "Base", -1, 0);
            fields.iter().for_each(|f| t.field(f, 1));
            meta.extend_from_slice(&class.to_le_bytes());
            meta.push(0);
            meta.extend_from_slice(&(-1i16).to_le_bytes());
            meta.extend_from_slice(&[0; 16]);
            meta.extend_from_slice(&(t.nodes.len() as i32).to_le_bytes());
            meta.extend_from_slice(&(t.strings.len() as i32).to_le_bytes());
            for (level, flags, ty, name, size, m) in &t.nodes {
                meta.extend_from_slice(&1u16.to_le_bytes());
                meta.push(*level);
                meta.push(*flags);
                meta.extend_from_slice(&ty.to_le_bytes());
                meta.extend_from_slice(&name.to_le_bytes());
                meta.extend_from_slice(&size.to_le_bytes());
                meta.extend_from_slice(&0i32.to_le_bytes());
                meta.extend_from_slice(&m.to_le_bytes());
                meta.extend_from_slice(&0u64.to_le_bytes());
            }
            meta.extend_from_slice(&t.strings);
            meta.extend_from_slice(&0i32.to_le_bytes());
        }
        meta.extend_from_slice(&(objects.len() as i32).to_le_bytes());
        let mut body = Vec::new();
        for (id, class, fields) in objects {
            while (48 + meta.len()) % 4 != 0 {
                meta.push(0);
            }
            let mut d = Vec::new();
            fields.iter().for_each(|f| data_of(f, &mut d));
            meta.extend_from_slice(&id.to_le_bytes());
            meta.extend_from_slice(&(body.len() as u64).to_le_bytes());
            meta.extend_from_slice(&(d.len() as u32).to_le_bytes());
            meta.extend_from_slice(&(classes.iter().position(|c| c == class).unwrap() as i32).to_le_bytes());
            body.extend_from_slice(&d);
            body.resize(body.len().div_ceil(8) * 8, 0);
        }
        meta.extend_from_slice(&0i32.to_le_bytes());
        meta.extend_from_slice(&(externals.len() as i32).to_le_bytes());
        for g in externals {
            meta.push(0);
            meta.extend_from_slice(&guid_bytes(g).unwrap());
            meta.extend_from_slice(&0i32.to_le_bytes());
            meta.extend_from_slice(b"x\0");
        }
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
        f.extend_from_slice(&body);
        f
    }

    /// One triangle with two UV sets, as Unity's import lays a mesh out: positions then the UVs, one stream.
    pub(crate) fn mesh_fields(name: &'static str) -> Vec<F> {
        let mut v: Vec<u8> = Vec::new();
        for (p, uv, uv2) in [([0.0f32, 0.0, 0.0], [0.0f32, 0.0], [0.25f32, 0.25]), ([0.0, 1.0, 0.0], [0.0, 1.0], [0.25, 0.75]), ([1.0, 0.0, 0.0], [1.0, 0.0], [0.75, 0.25])] {
            for c in p.iter().chain(&uv).chain(&uv2) {
                v.extend_from_slice(&c.to_le_bytes());
            }
        }
        let channel = |stream, offset, format, dimension| vec![F::Byte("stream", stream), F::Byte("offset", offset), F::Byte("format", format), F::Byte("dimension", dimension)];
        vec![
            F::Text("m_Name", name),
            F::List("m_SubMeshes", vec![vec![F::Int("firstByte", 0), F::Int("indexCount", 3), F::Int("topology", 0), F::Int("baseVertex", 0)]]),
            F::Int("m_IndexFormat", 0),
            F::Bytes("m_IndexBuffer", vec![0, 0, 1, 0, 2, 0]),
            F::Rec(
                "m_VertexData",
                vec![
                    F::Int("m_VertexCount", 3),
                    F::List(
                        "m_Channels",
                        vec![channel(0, 0, 0, 3), channel(0, 0, 0, 0), channel(0, 0, 0, 0), channel(0, 0, 0, 0), channel(0, 12, 0, 2), channel(0, 20, 0, 2)],
                    ),
                    F::Typeless("m_DataSize", v),
                ],
            ),
        ]
    }

    /// A model's import as Unity keeps it: its root object with a mesh, a material of its own and a sun.
    pub(crate) fn model_artifact() -> Vec<u8> {
        let go = |name: &'static str| vec![F::Text("m_Name", name), F::Byte("m_IsActive", 1)];
        let transform = |go: i64, father: i64, x: f32| {
            vec![
                F::Ptr("m_GameObject", 0, go),
                F::Rec("m_LocalRotation", vec![F::Float("x", 0.0), F::Float("y", 0.0), F::Float("z", 0.0), F::Float("w", 1.0)]),
                F::Rec("m_LocalPosition", vec![F::Float("x", x), F::Float("y", 0.0), F::Float("z", 0.0)]),
                F::Rec("m_LocalScale", vec![F::Float("x", 1.0), F::Float("y", 1.0), F::Float("z", 1.0)]),
                F::Ptr("m_Father", 0, father),
            ]
        };
        serialized(
            &[
                (10, 1, go("Root")),
                (11, 4, transform(10, 0, 2.0)),
                (20, 1, go("Part")),
                (21, 4, transform(20, 11, 3.0)),
                (22, 33, vec![F::Ptr("m_GameObject", 0, 20), F::Ptr("m_Mesh", 0, 40)]),
                (23, 23, vec![F::Ptr("m_GameObject", 0, 20), F::Byte("m_Enabled", 1), F::Ptrs("m_Materials", vec![(1, 2100000)])]),
                (30, 1, go("Sun")),
                (31, 4, transform(30, 11, 0.0)),
                (
                    32,
                    108,
                    vec![
                        F::Ptr("m_GameObject", 0, 30),
                        F::Byte("m_Enabled", 1),
                        F::Int("m_Type", 1),
                        F::Rec("m_Color", vec![F::Float("r", 1.0), F::Float("g", 1.0), F::Float("b", 1.0), F::Float("a", 1.0)]),
                        F::Float("m_Intensity", 2.0),
                    ],
                ),
                (40, 43, mesh_fields("Part")),
            ],
            &["dddd0000000000000000000000000004"],
        )
    }

    /// Puts `artifact` in `project`'s `Library` as the current import of `guid`, as Unity keeps it.
    pub(crate) fn install(project: &Path, guid: &str, artifact: &[u8]) {
        let id = [7u8; 16];
        let mut revision = guid_bytes(guid).unwrap().to_vec();
        revision.extend_from_slice(&[0xff; 4]);
        revision.extend_from_slice(&[0; 20]);
        revision.extend_from_slice(&id);
        let mut meta = vec![0u8; META_FILE_HASH];
        meta.extend_from_slice(&[0x1b; 16]);
        std::fs::create_dir_all(project.join("Library/Artifacts/1b")).unwrap();
        std::fs::write(project.join("Library/Artifacts/1b").join("1b".repeat(16)), artifact).unwrap();
        std::fs::write(project.join("Library/ArtifactDB"), database(&[("ArtifactIDToArtifactMetaInfo", vec![(id.to_vec(), meta)]), ("CurrentRevisions", vec![(vec![9; 16], revision)])])).unwrap();
    }

    #[test]
    fn an_artifact_reads_as_prefabs_see_the_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact");
        std::fs::write(&path, model_artifact()).unwrap();
        let a = Artifact::open(&path, "aaaa0000000000000000000000000009", 1 << 20).unwrap();
        assert_eq!(a.count(43), 1);
        let m = a.mesh(40).unwrap();
        assert_eq!(m.name, "Part");
        assert_eq!(m.positions[1], [0.0, 1.0, 0.0]);
        assert_eq!(m.uvs[1], [0.0, 1.0]);
        assert_eq!(m.uv2[2], [0.75, 0.25], "the second UV set, which lightmaps use");
        let g = a.graph().unwrap();
        let Some(Obj::Filter(f)) = g.objects.get(&22) else { panic!("a filter") };
        assert_eq!(f.mesh, MeshSource::Asset(Ref { file_id: 40, guid: Some("aaaa0000000000000000000000000009".into()) }), "a mesh of the model itself");
        let Some(Obj::Renderer(r)) = g.objects.get(&23) else { panic!("a renderer") };
        assert_eq!(r.materials[0], MatSource::Asset(Ref { file_id: 2100000, guid: Some("dddd0000000000000000000000000004".into()) }), "a material in another file");
        let Some(Obj::Light(l)) = g.objects.get(&32) else { panic!("the model's sun") };
        assert_eq!((l.kind, l.intensity), (1, 2.0));
    }

    /// A model's own material as Unity's import keeps it: pairs of a plain name and a value, unlike any text file. Those
    /// read as text files' lost every colour and picture, and an imported model's own materials drew plain white.
    #[test]
    fn a_models_own_material_keeps_its_look_from_the_import() {
        let colour = |r, g, b, a| F::Rec("second", vec![F::Float("r", r), F::Float("g", g), F::Float("b", b), F::Float("a", a)]);
        let scale = |name, x, y| F::Rec(name, vec![F::Float("x", x), F::Float("y", y)]);
        let picture = |name: &'static str, file, id| vec![F::Text("first", name), F::Rec("second", vec![F::Ptr("m_Texture", file, id), scale("m_Scale", 2.0, 3.0), scale("m_Offset", 0.5, 0.25)])];
        let material = serialized(
            &[(
                70,
                21,
                vec![
                    F::Text("m_Name", "BodyMatt.001"),
                    F::Ptr("m_Shader", 0, 0),
                    F::Int("m_CustomRenderQueue", -1),
                    F::List("stringTagMap", vec![vec![F::Text("first", "RenderType"), F::Text("second", "Transparent")]]),
                    F::Rec(
                        "m_SavedProperties",
                        vec![
                            F::List("m_TexEnvs", vec![picture("_MainTex", 1, 2800000), picture("_DetailMask", 0, 0)]),
                            F::List("m_Floats", vec![vec![F::Text("first", "_Metallic"), F::Float("second", 0.75)]]),
                            F::List("m_Colors", vec![vec![F::Text("first", "_Color"), colour(0.5, 0.25, 1.0, 1.0)]]),
                        ],
                    ),
                ],
            )],
            &["dddd0000000000000000000000000004"],
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact");
        std::fs::write(&path, material).unwrap();
        let a = Artifact::open(&path, "aaaa0000000000000000000000000009", 1 << 20).unwrap();
        let (name, _, props) = crate::scene::material::props_of(&a.object(70, 21).unwrap());
        let look = crate::scene::material::look(&name, "standard", &props);
        assert_eq!(look.material.name, "BodyMatt.001");
        assert_eq!(look.main_texture.as_deref(), Some("dddd0000000000000000000000000004"));
        assert_eq!(look.material.color, [0.5, 0.25, 1.0, 1.0]);
        assert_eq!(look.material.tiling, [2.0, 3.0]);
        assert_eq!(look.material.offset, [0.5, 0.25]);
        assert_eq!(look.material.metallic, 0.75);
        assert_eq!(look.material.alpha, crate::preview::Alpha::Blend, "its tag, a list of pairs here");
    }

    /// A one-table LMDB file of `page`-byte pages: two meta pages, the main tree naming `table` at page 3, whose leaf
    /// holds `records`.
    pub(crate) fn database(tables: &[(&str, Vec<(Vec<u8>, Vec<u8>)>)]) -> Vec<u8> {
        let page = 4096usize;
        let mut b = vec![0u8; page * (3 + tables.len())];
        for m in 0..2 {
            let at = m * page;
            b[at + 16..at + 20].copy_from_slice(&MAGIC.to_le_bytes());
            b[at + 40..at + 44].copy_from_slice(&(page as u32).to_le_bytes());
            b[at + 128..at + 136].copy_from_slice(&2u64.to_le_bytes());
            b[at + 144..at + 152].copy_from_slice(&(m as u64).to_le_bytes());
        }
        let leaf = |b: &mut Vec<u8>, pg: usize, nodes: &[(Vec<u8>, u16, Vec<u8>)]| {
            let p = pg * page;
            b[p + 10..p + 12].copy_from_slice(&2u16.to_le_bytes());
            b[p + 12..p + 14].copy_from_slice(&((HEADER + 2 * nodes.len()) as u16).to_le_bytes());
            let mut end = page;
            for (i, (k, fl, v)) in nodes.iter().enumerate() {
                let len = 8 + k.len() + v.len();
                end -= len + len % 2;
                b[p + HEADER + 2 * i..p + HEADER + 2 * i + 2].copy_from_slice(&(end as u16).to_le_bytes());
                let n = p + end;
                b[n..n + 2].copy_from_slice(&(v.len() as u16).to_le_bytes());
                b[n + 4..n + 6].copy_from_slice(&fl.to_le_bytes());
                b[n + 6..n + 8].copy_from_slice(&(k.len() as u16).to_le_bytes());
                b[n + 8..n + 8 + k.len()].copy_from_slice(k);
                b[n + 8 + k.len()..n + 8 + k.len() + v.len()].copy_from_slice(v);
            }
        };
        let mut main = Vec::new();
        for (i, (name, records)) in tables.iter().enumerate() {
            let mut sub = vec![0u8; 48];
            sub[40..48].copy_from_slice(&((3 + i) as u64).to_le_bytes());
            main.push((name.as_bytes().to_vec(), 2u16, sub));
            let nodes: Vec<(Vec<u8>, u16, Vec<u8>)> = records.iter().map(|(k, v)| (k.clone(), 0u16, v.clone())).collect();
            leaf(&mut b, 3 + i, &nodes);
        }
        leaf(&mut b, 2, &main);
        b
    }

    #[test]
    fn an_assets_artifact_is_found_through_unitys_database() {
        let dir = tempfile::tempdir().unwrap();
        let guid = "df3817c7869b19048b444e138dfe4dc3";
        let raw = guid_bytes(guid).unwrap();
        assert_eq!(raw[0], 0xfd, "each byte's digits swapped");
        let artifact = [7u8; 16];
        let mut revision = raw.to_vec();
        revision.extend_from_slice(&[0xff; 4]);
        revision.extend_from_slice(&[0; 20]);
        revision.extend_from_slice(&artifact);
        let mut meta = vec![0u8; META_FILE_HASH];
        let hash = [0x1bu8; 16];
        meta.extend_from_slice(&hash);
        let db = database(&[("ArtifactIDToArtifactMetaInfo", vec![(artifact.to_vec(), meta)]), ("CurrentRevisions", vec![(vec![9; 16], revision)])]);
        std::fs::create_dir_all(dir.path().join("Library/Artifacts/1b")).unwrap();
        std::fs::write(dir.path().join("Library/ArtifactDB"), db).unwrap();
        let file = dir.path().join("Library/Artifacts/1b").join("1b".repeat(16));
        let a = ArtifactDb::open(dir.path()).expect("a database");
        assert_eq!(a.file_of(guid), None, "the artifact's file is not there");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(a.file_of(guid), Some(file));
        assert_eq!(a.file_of("00000000000000000000000000000000"), None);
    }

    #[test]
    fn a_damaged_database_is_no_database() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Library")).unwrap();
        assert!(ArtifactDb::open(dir.path()).is_none());
        std::fs::write(dir.path().join("Library/ArtifactDB"), vec![0xffu8; 70000]).unwrap();
        assert!(ArtifactDb::open(dir.path()).is_none());
        // A branch page pointing at itself gives up at the depth limit (checked by the walk's depth).
        let mut b = database(&[("CurrentRevisions", vec![(vec![1; 16], vec![0; 56])]), ("ArtifactIDToArtifactMetaInfo", vec![])]);
        for chunk in crate::scene::damaged(&b.clone(), 40) {
            b = chunk;
            std::fs::write(dir.path().join("Library/ArtifactDB"), &b).unwrap();
            let _ = ArtifactDb::open(dir.path());
        }
    }
}
