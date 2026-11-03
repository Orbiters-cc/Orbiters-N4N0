//! Which file each GUID names in a Unity project. Every asset's `.meta` file holds its GUID, so a reference
//! (`{fileID: …, guid: …}`) is resolved by reading them; Unity looks in `Assets`, the packages in `Packages` (VPM
//! packages are folders there; a `file:` entry of `Packages/manifest.json` points elsewhere) and those the package
//! manager keeps in `Library/PackageCache`. Folders ending in `~` or starting with `.` are not assets.
//!
//! Reading thousands of `.meta` files is slow on a hard disk (about 12 ms each, cold: over a minute for an avatar
//! project), so the index starts from Unity's own: a project opened once has `Library/SourceAssetDB`, an LMDB file
//! whose `GuidToPath` table maps every GUID to its path, read in one go ([`lmdb`]). It can be stale (a file moved while
//! Unity was closed), so a path is checked to exist, and a GUID it lacks makes the index read the `.meta` files, the
//! kinds a preview reads first (models, prefabs, materials, textures, shaders, meshes), the rest only if still
//! needed, on several threads. The GUIDs read are kept in the app's cache with each `.meta` file's modification time
//! (never in the project), so the next index only lists the folders and reads the metas that changed.

use super::Limits;
use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

/// A project's assets by GUID, and by file name (a model's textures are looked up by name, as Unity's importer does).
pub struct Assets {
    pub project: PathBuf,
    /// From `Library/SourceAssetDB`, when the project has one.
    library: HashMap<String, PathBuf>,
    state: Mutex<State>,
    cache: Option<PathBuf>,
    limits: Limits,
    /// `.meta` files read (not taken from the cache) by this index.
    pub metas_read: AtomicUsize,
    /// The GUIDs came from Unity's database.
    pub from_library: bool,
}

#[derive(Default)]
struct State {
    /// GUIDs read from `.meta` files: none yet, the likely kinds (1), everything (2).
    phase: u8,
    scanned: HashMap<String, PathBuf>,
    /// Every file by lower-case name, once listed.
    names: Option<HashMap<String, Vec<PathBuf>>>,
    exists: HashMap<PathBuf, bool>,
    inserted: HashMap<String, PathBuf>,
}

impl Assets {
    /// An index with nothing in it yet (tests fill it with [`Assets::insert`]); it reads nothing from the disk.
    pub fn empty(project: &Path) -> Self {
        Self {
            project: project.to_path_buf(),
            library: HashMap::new(),
            state: Mutex::new(State { phase: 2, names: Some(HashMap::new()), ..Default::default() }),
            cache: None,
            limits: Limits::default(),
            metas_read: AtomicUsize::new(0),
            from_library: false,
        }
    }

    /// The file a GUID names.
    pub fn path(&self, guid: &str) -> Option<PathBuf> {
        let mut s = self.state.lock().unwrap();
        if let Some(p) = s.inserted.get(guid) {
            return Some(p.clone());
        }
        if let Some(p) = self.library.get(guid) {
            let ok = *s.exists.entry(p.clone()).or_insert_with(|| p.exists());
            if ok {
                return Some(p.clone());
            }
        }
        loop {
            if let Some(p) = s.scanned.get(guid) {
                return Some(p.clone());
            }
            if s.phase >= 2 || self.limits.check("").is_err() {
                return None;
            }
            let phase = s.phase + 1;
            let found = scan(&self.project, self.cache.as_deref(), phase, &self.limits, &self.metas_read);
            s.scanned.extend(found);
            s.phase = phase;
        }
    }

    pub fn len(&self) -> usize {
        let s = self.state.lock().unwrap();
        self.library.len().max(s.scanned.len()) + s.inserted.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The project's files named `name` (any case), nearest to `near` first (the same folder, then up the tree).
    pub fn named(&self, name: &str, near: Option<&Path>) -> Vec<PathBuf> {
        let mut s = self.state.lock().unwrap();
        if s.names.is_none() {
            // Within the build's limits: a folder linked into Assets can lead to a whole disk.
            let mut names: HashMap<String, Vec<PathBuf>> = HashMap::new();
            let mut files = 0usize;
            'roots: for root in roots(&self.project) {
                for e in walk(&root) {
                    files += 1;
                    if files > self.limits.max_files || (files.is_multiple_of(4096) && self.limits.check("").is_err()) {
                        break 'roots;
                    }
                    if !e.file_type().is_file() {
                        continue;
                    }
                    let n = e.file_name().to_string_lossy().to_ascii_lowercase();
                    if !n.ends_with(".meta") {
                        names.entry(n).or_default().push(e.into_path());
                    }
                }
            }
            s.names = Some(names);
        }
        let mut out: Vec<PathBuf> = s.names.as_ref().and_then(|n| n.get(&name.to_ascii_lowercase())).cloned().unwrap_or_default();
        if let Some(near) = near {
            let shared = |p: &Path| p.components().zip(near.components()).take_while(|(a, b)| a == b).count();
            out.sort_by_key(|p| std::cmp::Reverse(shared(p)));
        }
        out
    }

    #[cfg(test)]
    pub fn insert(&mut self, guid: &str, path: PathBuf) {
        let mut s = self.state.lock().unwrap();
        if let Some(name) = path.file_name() {
            s.names.get_or_insert_with(HashMap::new).entry(name.to_string_lossy().to_ascii_lowercase()).or_default().push(path.clone());
        }
        s.inserted.insert(guid.to_string(), path);
    }
}

/// The folders Unity takes assets from, in the order it does.
pub fn roots(project: &Path) -> Vec<PathBuf> {
    let mut out = vec![project.join("Assets")];
    if let Ok(dir) = std::fs::read_dir(project.join("Packages")) {
        let mut packages: Vec<PathBuf> = dir.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        packages.sort();
        out.extend(packages);
    }
    // Local packages the manifest names (`"name": "file:../somewhere"`).
    if let Ok(bytes) = std::fs::read(project.join("Packages/manifest.json")) {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            if let Some(deps) = v["dependencies"].as_object() {
                for spec in deps.values().filter_map(|s| s.as_str()) {
                    if let Some(rel) = spec.strip_prefix("file:") {
                        let p = project.join("Packages").join(rel);
                        if p.is_dir() && !out.contains(&p) {
                            out.push(p);
                        }
                    }
                }
            }
        }
    }
    if let Ok(dir) = std::fs::read_dir(project.join("Library/PackageCache")) {
        let mut packages: Vec<PathBuf> = dir.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        packages.sort();
        out.extend(packages);
    }
    out
}

/// Unity leaves these out of the asset database.
fn ignored(name: &str) -> bool {
    name.starts_with('.') || name.ends_with('~') || name.eq_ignore_ascii_case("cvs")
}

fn walk(root: &Path) -> impl Iterator<Item = walkdir::DirEntry> {
    walkdir::WalkDir::new(root)
        .follow_links(true)
        .max_depth(40)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !ignored(&e.file_name().to_string_lossy()))
        .flatten()
}

/// The kinds of asset a preview reads, whose `.meta` files are read first.
fn likely(asset: &str) -> bool {
    let ext = asset.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "prefab" | "fbx" | "obj" | "mat" | "png" | "jpg" | "jpeg" | "tga" | "psd" | "psb" | "bmp" | "tif" | "tiff" | "gif" | "shader" | "mesh"
    )
}

/// Indexes `project`'s assets: Unity's database when the project has one, else (and for what it lacks) the `.meta`
/// files, their GUIDs read before taken from `cache` (a file in the app's cache folder).
pub fn index(project: &Path, cache: Option<&Path>, limits: &Limits) -> anyhow::Result<Assets> {
    let library = lmdb::guid_to_path(project).unwrap_or_default();
    let from_library = !library.is_empty();
    Ok(Assets {
        project: project.to_path_buf(),
        library,
        state: Mutex::new(State::default()),
        cache: cache.map(Path::to_path_buf),
        limits: limits.clone(),
        metas_read: AtomicUsize::new(0),
        from_library,
    })
}

/// Reads the `.meta` files of `phase` (1: the likely kinds, 2: all) on several threads, those unchanged since the
/// cache's taken from it.
fn scan(project: &Path, cache: Option<&Path>, phase: u8, limits: &Limits, read: &AtomicUsize) -> HashMap<String, PathBuf> {
    let known = cache.map(read_cache).unwrap_or_default();
    let mut metas: Vec<(PathBuf, String, i64)> = Vec::new();
    let mut files = 0usize;
    'roots: for root in roots(project) {
        for entry in walk(&root) {
            files += 1;
            if files > limits.max_files || (files.is_multiple_of(4096) && limits.check("").is_err()) {
                break 'roots;
            }
            if entry.file_type().is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy();
            let Some(asset) = name.strip_suffix(".meta") else { continue };
            if phase == 1 && !likely(asset) {
                continue;
            }
            let rel = entry.path().strip_prefix(project).unwrap_or(entry.path()).to_string_lossy().replace('\\', "/");
            let mtime =
                entry.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
            metas.push((entry.into_path(), rel, mtime));
        }
    }
    // In path order (a folder's files tend to lie together on the disk), split among threads.
    metas.sort_by(|a, b| a.1.cmp(&b.1));
    let todo: Vec<usize> = (0..metas.len()).filter(|&i| known.get(&metas[i].1).is_none_or(|(t, _)| *t != metas[i].2)).collect();
    let guids: Vec<Mutex<Option<String>>> = metas.iter().map(|m| Mutex::new(known.get(&m.1).filter(|(t, _)| *t == m.2).map(|(_, g)| g.clone()))).collect();
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..8.min(todo.len()) {
            scope.spawn(|| loop {
                let k = next.fetch_add(1, Ordering::Relaxed);
                let Some(&i) = todo.get(k) else { break };
                if k.is_multiple_of(64) && limits.check("").is_err() {
                    break;
                }
                *guids[i].lock().unwrap() = read_meta_guid(&metas[i].0);
                read.fetch_add(1, Ordering::Relaxed);
            });
        }
    });
    let mut out = HashMap::new();
    let mut fresh = known.clone();
    for (m, g) in metas.iter().zip(guids) {
        let Some(g) = g.into_inner().unwrap() else { continue };
        fresh.insert(m.1.clone(), (m.2, g.clone()));
        let asset = m.0.with_file_name(m.0.file_name().unwrap().to_string_lossy().trim_end_matches(".meta"));
        // The first asset Unity would find for a GUID wins (a copied folder keeps its metas' GUIDs: Unity then gives
        // the copy new ones, so the original is the one references mean).
        out.entry(g).or_insert(asset);
    }
    if let Some(cache) = cache {
        if !todo.is_empty() {
            write_cache(cache, &fresh);
        }
    }
    out
}

fn read_meta_guid(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 256];
    let n = f.read(&mut buf).ok()?;
    super::yaml::meta_guid(&String::from_utf8_lossy(&buf[..n]))
}

const CACHE_VERSION: &str = "orbiters-asset-guids 1";

fn read_cache(path: &Path) -> HashMap<String, (i64, String)> {
    let Ok(text) = std::fs::read_to_string(path) else { return HashMap::new() };
    let mut lines = text.lines();
    if lines.next() != Some(CACHE_VERSION) {
        return HashMap::new();
    }
    lines
        .filter_map(|l| {
            let mut parts = l.splitn(3, '\t');
            let t = parts.next()?.parse().ok()?;
            let g = parts.next()?.to_string();
            let rel = parts.next()?.to_string();
            Some((rel, (t, g)))
        })
        .collect()
}

fn write_cache(path: &Path, entries: &HashMap<String, (i64, String)>) {
    let mut out = String::with_capacity(entries.len() * 80 + 32);
    out.push_str(CACHE_VERSION);
    out.push('\n');
    let mut list: Vec<_> = entries.iter().collect();
    list.sort();
    for (rel, (t, g)) in list {
        if rel.contains('\n') || rel.contains('\t') {
            continue;
        }
        out.push_str(&format!("{t}\t{g}\t{rel}\n"));
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = orbiters_core::write_atomic(path, out.as_bytes());
}

/// Unity's asset database (`Library/SourceAssetDB`, an LMDB file), read without LMDB: the current B-tree of its
/// `GuidToPath` table, from the newer of its two meta pages (so pages freed by later transactions, which still hold
/// old records, are never read).
pub mod lmdb {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    const MAGIC: u32 = 0xBEEF_C0DE;
    const HEADER: usize = 16;

    struct Db<'a> {
        b: &'a [u8],
        page: usize,
        /// Pages read so far: a tree reads each of its pages once, so more than the file has means a damaged one
        /// whose branches point back up (each level of 32 would otherwise multiply the reads by thousands: hours).
        visits: std::cell::Cell<usize>,
    }

    /// The largest database read (Unity's is tens of megabytes for a large project): it is read whole.
    const MAX_BYTES: u64 = 1 << 30;

    impl Db<'_> {
        fn u16(&self, at: usize) -> Option<u16> {
            Some(u16::from_le_bytes(self.b.get(at..at + 2)?.try_into().ok()?))
        }
        fn u32(&self, at: usize) -> Option<u32> {
            Some(u32::from_le_bytes(self.b.get(at..at + 4)?.try_into().ok()?))
        }
        fn u64(&self, at: usize) -> Option<u64> {
            Some(u64::from_le_bytes(self.b.get(at..at + 8)?.try_into().ok()?))
        }

        /// Every (key, flags, value) of the tree under page `pg`.
        fn walk(&self, pg: u64, depth: usize, out: &mut Vec<(Vec<u8>, u16, Vec<u8>)>) -> Option<()> {
            if depth > 32 || out.len() > 2_000_000 {
                return None;
            }
            self.visits.set(self.visits.get() + 1);
            if self.visits.get() > self.b.len() / self.page + 16 {
                return None;
            }
            let p = usize::try_from(pg).ok()?.checked_mul(self.page)?;
            let flags = self.u16(p + 10)?;
            let lower = self.u16(p + 12)? as usize;
            let keys = lower.checked_sub(HEADER)? / 2;
            for i in 0..keys {
                let node = p + self.u16(p + HEADER + 2 * i)? as usize;
                let (lo, hi, fl, ks) = (self.u16(node)? as u64, self.u16(node + 2)? as u64, self.u16(node + 4)?, self.u16(node + 6)? as usize);
                let key = self.b.get(node + 8..node + 8 + ks)?.to_vec();
                if flags & 0x01 != 0 {
                    self.walk(lo | hi << 16 | (fl as u64) << 32, depth + 1, out)?;
                } else if flags & 0x02 != 0 {
                    let size = (lo | hi << 16) as usize;
                    let at = node + 8 + ks;
                    let value = if fl & 0x01 != 0 {
                        // On overflow pages.
                        let ov = usize::try_from(self.u64(at)?).ok()?.checked_mul(self.page)? + HEADER;
                        self.b.get(ov..ov + size)?.to_vec()
                    } else {
                        self.b.get(at..at + size)?.to_vec()
                    };
                    out.push((key, fl, value));
                }
            }
            Some(())
        }
    }

    /// The named tables' roots.
    fn tables(b: &[u8]) -> Option<(Db<'_>, HashMap<Vec<u8>, u64>)> {
        let page = u32::from_le_bytes(b.get(40..44)?.try_into().ok()?) as usize;
        if !(512..=65536).contains(&page) || !page.is_power_of_two() {
            return None;
        }
        let db = Db { b, page, visits: std::cell::Cell::new(0) };
        let meta = (0..2usize).filter(|m| db.u32(m * page + HEADER) == Some(MAGIC)).max_by_key(|m| db.u64(m * page + 144).unwrap_or(0))?;
        let root = db.u64(meta * page + 128)?;
        let mut main = Vec::new();
        db.walk(root, 0, &mut main)?;
        let mut out = HashMap::new();
        for (key, fl, value) in main {
            if fl & 0x02 != 0 && value.len() >= 48 {
                let name = key.strip_suffix(&[0]).unwrap_or(&key).to_vec();
                out.insert(name, u64::from_le_bytes(value[40..48].try_into().ok()?));
            }
        }
        Some((db, out))
    }

    /// A GUID as Unity's database keys it (16 bytes, each byte's two hex digits swapped) as `.meta` files write it.
    fn guid_hex(k: &[u8]) -> String {
        k.iter().map(|b| format!("{:x}{:x}", b & 0xf, b >> 4)).collect()
    }

    fn c_string(v: &[u8]) -> String {
        String::from_utf8_lossy(v.split(|&c| c == 0).next().unwrap_or(v)).into_owned()
    }

    /// Every asset Unity knew of in `project`: its GUID and file (packages at the folders Unity found them in).
    pub fn guid_to_path(project: &Path) -> Option<HashMap<String, PathBuf>> {
        let path = project.join("Library/SourceAssetDB");
        if std::fs::metadata(&path).ok()?.len() > MAX_BYTES {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        let (db, tables) = tables(&bytes)?;
        // Where each root folder is (`Packages/com.x` may be in Library/PackageCache under a versioned name).
        let mut roots: Vec<(String, PathBuf)> = Vec::new();
        if let Some(&r) = tables.get(b"RootFolders".as_slice()) {
            let mut list = Vec::new();
            db.walk(r, 0, &mut list)?;
            for (k, _, v) in list {
                let logical = c_string(&k);
                // The value ends with the folder's absolute path.
                let physical = v
                    .split(|&c| c == 0)
                    .filter(|s| s.len() > 2 && (s[1] == b':' || s[0] == b'/'))
                    .next_back()
                    .map(|s| PathBuf::from(String::from_utf8_lossy(s).into_owned()));
                let physical = physical.filter(|p| p.is_dir()).unwrap_or_else(|| project.join(&logical));
                roots.push((logical, physical));
            }
        }
        roots.sort_by_key(|r| std::cmp::Reverse(r.0.len()));
        let &table = tables.get(b"GuidToPath".as_slice())?;
        let mut list = Vec::new();
        db.walk(table, 0, &mut list)?;
        let mut out = HashMap::with_capacity(list.len());
        for (k, _, v) in list {
            if k.len() != 16 {
                continue;
            }
            let rel = c_string(&v);
            let path = roots
                .iter()
                .find(|(l, _)| rel.len() > l.len() && rel.starts_with(l.as_str()) && rel.as_bytes()[l.len()] == b'/')
                .map(|(l, p)| p.join(&rel[l.len() + 1..]))
                .unwrap_or_else(|| project.join(&rel));
            out.insert(guid_hex(&k), path);
        }
        Some(out)
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn guids_read_as_meta_files_write_them() {
            let raw = [0x19u8, 0x1b, 0x00, 0xbe, 0xb3, 0x3c, 0x02, 0x48, 0xa3, 0xb1, 0x2a, 0xc0, 0x83, 0xe0, 0x3a, 0x2a];
            assert_eq!(super::guid_hex(&raw), "91b100eb3bc320843a1ba20c380ea3a2");
            assert!(super::tables(&[0u8; 100]).is_none());
            assert!(super::tables(&vec![0xffu8; 70000]).is_none());
        }

        #[test]
        fn a_tree_whose_branches_point_back_up_is_given_up_at_once() {
            // Two meta pages, the newer's root page 2: a branch page whose 2000 keys all point at page 2 itself.
            let page = 4096usize;
            let mut b = vec![0u8; page * 3];
            for m in 0..2 {
                let at = m * page;
                b[at + 16..at + 20].copy_from_slice(&super::MAGIC.to_le_bytes());
                b[at + 40..at + 44].copy_from_slice(&(page as u32).to_le_bytes());
                b[at + 128..at + 136].copy_from_slice(&2u64.to_le_bytes());
                b[at + 144..at + 152].copy_from_slice(&(m as u64).to_le_bytes());
            }
            let p = 2 * page;
            let keys = 2000usize;
            b[p + 10..p + 12].copy_from_slice(&1u16.to_le_bytes());
            b[p + 12..p + 14].copy_from_slice(&((super::HEADER + 2 * keys) as u16).to_le_bytes());
            let node = super::HEADER + 2 * keys;
            for i in 0..keys {
                b[p + super::HEADER + 2 * i..p + super::HEADER + 2 * i + 2].copy_from_slice(&(node as u16).to_le_bytes());
            }
            // The node: child page 2 (lo 2, hi 0, flags 0), no key.
            b[p + node..p + node + 2].copy_from_slice(&2u16.to_le_bytes());
            let start = std::time::Instant::now();
            assert!(super::tables(&b).is_none());
            assert!(start.elapsed() < std::time::Duration::from_secs(1), "{:?}", start.elapsed());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(dir: &Path, rel: &str, guid: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"x").unwrap();
        std::fs::write(dir.join(format!("{rel}.meta")), format!("fileFormatVersion: 2\nguid: {guid}\n")).unwrap();
    }

    #[test]
    fn guids_come_from_assets_and_packages_and_are_cached() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        meta(p, "Assets/Models/Body.fbx", "11111111111111111111111111111111");
        meta(p, "Packages/com.poiyomi.toon/Shaders/Poi.shader", "22222222222222222222222222222222");
        meta(p, "Assets/Skip~/Hidden.mat", "33333333333333333333333333333333");
        meta(p, "Library/PackageCache/com.unity.x@1.0.0/Runtime/A.mat", "44444444444444444444444444444444");
        meta(p, "Assets/Scripts/Thing.cs", "55555555555555555555555555555555");
        let cache = p.join("cache/guids.txt");
        let limits = Limits::default();
        let a = index(p, Some(&cache), &limits).unwrap();
        assert!(!a.from_library);
        assert_eq!(a.path("11111111111111111111111111111111"), Some(p.join("Assets/Models/Body.fbx")));
        assert_eq!(a.metas_read.load(Ordering::Relaxed), 3, "the likely kinds first: a model, a shader, a material");
        assert!(a.path("22222222222222222222222222222222").is_some());
        assert!(a.path("44444444444444444444444444444444").is_some());
        assert!(a.path("33333333333333333333333333333333").is_none(), "folders ending in ~ are not assets");
        assert_eq!(a.path("55555555555555555555555555555555"), Some(p.join("Assets/Scripts/Thing.cs")), "the rest when still needed");
        assert_eq!(a.named("body.FBX", None).len(), 1);
        let again = index(p, Some(&cache), &limits).unwrap();
        assert!(again.path("55555555555555555555555555555555").is_some());
        assert_eq!(again.metas_read.load(Ordering::Relaxed), 0, "nothing changed: every GUID from the cache");
    }
}
