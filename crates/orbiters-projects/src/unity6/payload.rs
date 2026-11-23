//! What a trial is made of, read from the original and nothing else: every file and folder a copy takes, what the original
//! links to or takes packages from outside it (where a trial must never go), its content IDs, and the few files whose bytes
//! are the trial's own (its product GUID, the entries of its local packages). The original is only read ([`ReadRoot`]).
//!
//! A linked folder is copied as ordinary files: the trial holds no link at all, so nothing in it is anything the original
//! can see. (Refusing such projects instead would leave everyone who shares assets between projects without a trial; what
//! made links dangerous is the trial's place, which is checked against every one of them before anything is made.)

use super::gate::{Entry, Kind, Protected, ReadFile, ReadRoot, TrialRoot};
use super::journal::hex;
use orbiters_i18n::tr;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Where one file's bytes come from.
#[derive(Clone, Debug)]
pub struct Origin {
    /// The place in [`Payload::roots`].
    root: usize,
    rel: PathBuf,
    /// Through a link (or through what keeps it online).
    follow: bool,
}

#[derive(Clone, Debug)]
pub enum Item {
    Dir,
    File(Origin),
    /// The trial's own bytes, made from the original's.
    Made(Vec<u8>),
}

/// A link of the original, and where it leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Linked {
    pub at: String,
    pub to: PathBuf,
}

/// A local (`file:`) package from outside the original.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outside {
    pub name: String,
    pub from: PathBuf,
}

/// Everything a trial of one project holds.
pub struct Payload {
    roots: Vec<ReadRoot>,
    /// By its path in the trial, folders before what is in them.
    pub items: BTreeMap<PathBuf, Item>,
    pub bytes: u64,
    pub linked: Vec<Linked>,
    pub outside: Vec<Outside>,
    /// Local packages whose `Packages/manifest.json` entry is changed in the trial.
    pub localized: Vec<String>,
    /// What a trial must never go into: the original, what it links to, its local packages.
    pub protected: Vec<Protected>,
    /// The content IDs (`avtr_…`, `wrld_…`) the project's scenes and prefabs were uploaded as.
    pub content: BTreeSet<String>,
    /// Layouts a copy cannot keep whole.
    pub problems: Vec<String>,
    /// Local packages that are not where the project says.
    pub missing: Vec<String>,
}

/// What a copy without its Library leaves out, at any depth ([`crate::templates`]' rules).
fn left_out(entry: &Entry, depth: usize, project: bool) -> bool {
    let dir = matches!(entry.kind, Kind::Dir | Kind::Link { dir: true });
    crate::templates::left_out(&entry.name.to_string_lossy(), depth, dir, crate::templates::Keep::default(), project)
}

/// Files Unity cannot do without: a project missing one is not copied.
pub const NEEDED: [&str; 3] = ["ProjectSettings/ProjectVersion.txt", "ProjectSettings/ProjectSettings.asset", "Packages/manifest.json"];
const SETTINGS: &str = "ProjectSettings/ProjectSettings.asset";
const MANIFEST: &str = "Packages/manifest.json";

/// How deep links may lead through other links.
const DEEPEST: usize = 64;

struct Scan<'a> {
    cancel: &'a AtomicBool,
    out: Payload,
    /// The folders being gone through, so that a link to one of them is refused instead of followed for ever.
    open: Vec<crate::owned::FileId>,
}

impl Scan<'_> {
    /// Takes in the folder `rel` of root `root`, which is `at` in the trial.
    fn folder(&mut self, root: usize, rel: &Path, at: &Path, depth: usize, project: bool) -> anyhow::Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            anyhow::bail!("{}", tr("Stopped."));
        }
        let entries = self.out.roots[root].entries(rel).map_err(|e| anyhow::anyhow!("{}", tr!("{path} cannot be read: {error}", path = at.display(), error = e)))?;
        for e in entries {
            let (from, to) = (rel.join(&e.name), at.join(&e.name));
            let out = left_out(&e, depth, project);
            if let Kind::Link { .. } = e.kind {
                // What it leads to is never a trial's place, copied or not (a Library kept on another disk is one).
                let place = self.out.roots[root].protected_at(&from).map_err(|e| anyhow::anyhow!("{}", tr!("The link {path} leads nowhere that can be read: {error}", path = to.display(), error = e)));
                match place {
                    Ok(p) => {
                        if !out {
                            self.out.linked.push(Linked { at: to.display().to_string(), to: p.shown.clone() });
                        }
                        if !self.out.protected.contains(&p) {
                            self.out.protected.push(p);
                        }
                    }
                    Err(_) if out => continue,
                    Err(e) => return Err(e),
                }
            }
            if out {
                continue;
            }
            match e.kind {
                Kind::Dir => {
                    self.put(to.clone(), Item::Dir);
                    self.folder(root, &from, &to, depth + 1, project)?;
                }
                Kind::Link { dir: true } => {
                    let target = self.out.roots[root].follow_dir(&from)?;
                    if self.open.contains(&target.id) || self.open.len() > DEEPEST {
                        anyhow::bail!("{}", tr!("The link {path} leads back to a folder it is in: a copy of it would have no end.", path = to.display()));
                    }
                    self.open.push(target.id);
                    self.out.roots.push(target);
                    let new = self.out.roots.len() - 1;
                    self.put(to.clone(), Item::Dir);
                    self.folder(new, Path::new(""), &to, depth + 1, project)?;
                    self.open.pop();
                }
                Kind::File { .. } | Kind::Link { dir: false } => {
                    let follow = matches!(e.kind, Kind::Link { .. } | Kind::File { online: true });
                    let len = if matches!(e.kind, Kind::Link { .. }) { size_of(&mut self.out.roots[root].file(&from, true)?)? } else { e.len };
                    self.out.bytes = self.out.bytes.saturating_add(len);
                    let origin = Origin { root, rel: from, follow };
                    if project {
                        self.content(&to, &origin)?;
                    }
                    self.put(to, Item::File(origin));
                }
                Kind::Other => anyhow::bail!("{}", tr!("{path} is not a file or a folder: a copy cannot keep it.", path = to.display())),
            }
        }
        Ok(())
    }

    fn put(&mut self, at: PathBuf, item: Item) {
        if self.out.items.insert(at.clone(), item).is_some() {
            self.out.problems.push(at.display().to_string());
        }
    }

    /// The content IDs a scene or prefab was uploaded as.
    fn content(&mut self, at: &Path, origin: &Origin) -> anyhow::Result<()> {
        let is = |x: &str| at.extension().is_some_and(|e| e.eq_ignore_ascii_case(x));
        if is("unity") || is("prefab") {
            let mut bytes = vec![];
            self.out.open(origin)?.read_to_end(&mut bytes)?;
            self.out.content.extend(content_ids(&bytes));
        }
        Ok(())
    }
}

fn size_of(file: &mut ReadFile) -> anyhow::Result<u64> {
    Ok(std::io::copy(file, &mut std::io::sink())?)
}

/// The content IDs written in a scene or prefab as what it was uploaded as. In Unity's text only where a blueprint ID is
/// (a portal names other worlds); in a binary file anything that looks like one (it cannot be told, and a warning too many
/// costs less than one too few).
pub fn content_ids(bytes: &[u8]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut find = |text: &[u8]| {
        for prefix in [&b"avtr_"[..], b"wrld_", b"prop_"] {
            let mut rest = text;
            while let Some(i) = rest.windows(prefix.len()).position(|w| w == prefix) {
                let id = &rest[i..rest.len().min(i + prefix.len() + 36)];
                let uuid = &id[prefix.len()..];
                if uuid.len() == 36 && uuid.iter().all(|b| b.is_ascii_hexdigit() || *b == b'-') {
                    out.insert(String::from_utf8_lossy(id).to_lowercase());
                }
                rest = &rest[i + prefix.len()..];
            }
        }
    };
    if bytes.starts_with(b"%YAML") || bytes.starts_with(b"\xef\xbb\xbf%YAML") || !bytes.contains(&0) {
        let mut property = false;
        for line in bytes.split(|b| *b == b'\n') {
            let says = |word: &[u8]| line.windows(word.len()).any(|w| w == word);
            // A component's own field, or a prefab override's value under its property's name.
            if says(b"blueprintId:") || (property && says(b"value:")) {
                find(line);
            }
            if says(b"propertyPath:") {
                property = says(b"blueprintId");
            }
        }
    } else {
        find(bytes);
    }
    out
}

/// Reads what a trial of `original` holds. `guid`: the trial's product GUID; `name`: its folder's name.
pub fn scan(original: &Path, name: &str, guid: &str, cancel: &AtomicBool) -> anyhow::Result<Payload> {
    let root = ReadRoot::open(original).map_err(|e| anyhow::anyhow!("{}", tr!("{path} cannot be read: {error}", path = original.display(), error = e)))?;
    let (id, protected) = (root.id, root.protected());
    let mut scan = Scan {
        cancel,
        out: Payload {
            roots: vec![root],
            items: BTreeMap::new(),
            bytes: 0,
            linked: vec![],
            outside: vec![],
            localized: vec![],
            protected: vec![protected],
            content: BTreeSet::new(),
            problems: vec![],
            missing: vec![],
        },
        open: vec![id],
    };
    scan.folder(0, Path::new(""), Path::new(""), 1, true)?;
    for needed in NEEDED {
        if !matches!(scan.out.items.get(Path::new(needed)), Some(Item::File(_))) {
            scan.out.problems.push(needed.to_string());
        }
    }
    local_packages(&mut scan, original)?;
    settings(&mut scan.out, original, name, guid)?;
    // Two names the trial's disk would take for one (a package of the project's own in the place of a local one).
    let mut seen = BTreeSet::new();
    let twice: Vec<String> = scan.out.items.keys().filter(|k| !seen.insert(k.to_string_lossy().to_lowercase())).map(|k| k.display().to_string()).collect();
    scan.out.problems.extend(twice);
    scan.out.problems.sort();
    scan.out.problems.dedup();
    Ok(scan.out)
}

/// `rel` (below `Packages`) without its `..`, as a path of the project; `None` when it leaves the project.
fn lexical(rel: &str) -> Option<PathBuf> {
    let mut out = PathBuf::from("Packages"); // i18n: not shown (a folder's name)
    for c in Path::new(rel).components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(out)
}

fn slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// A `file:` entry naming `rel` of the project, from `Packages`.
fn entry_for(rel: &Path) -> String {
    match rel.strip_prefix("Packages") {
        Ok(inside) => format!("file:{}", slashes(inside)),
        Err(_) => format!("file:../{}", slashes(rel)),
    }
}

/// The project's local packages: those outside it become part of the trial, under `Packages`, and their entries name them
/// there; those inside it keep (or get) an entry that names them inside the trial.
fn local_packages(scan: &mut Scan, original: &Path) -> anyhow::Result<()> {
    for lock in ["Packages/packages-lock.json", "Packages/vpm-manifest.json"] {
        if let Some(Item::File(o)) = scan.out.items.get(Path::new(lock)).cloned() {
            let mut bytes = vec![];
            scan.out.open(&o)?.read_to_end(&mut bytes)?;
            match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(v) if v.is_object() => {}
                _ => scan.out.problems.push(lock.to_string()),
            }
        }
    }
    let Some(Item::File(o)) = scan.out.items.get(Path::new(MANIFEST)).cloned() else { return Ok(()) };
    let mut bytes = vec![];
    scan.out.open(&o)?.read_to_end(&mut bytes)?;
    let mut manifest: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v @ serde_json::Value::Object(_)) => v,
        _ => {
            scan.out.problems.push(MANIFEST.to_string());
            return Ok(());
        }
    };
    let dependencies = match manifest.get("dependencies") {
        None => return Ok(()),
        Some(serde_json::Value::Object(d)) => d.clone(),
        Some(_) => {
            scan.out.problems.push(MANIFEST.to_string());
            return Ok(());
        }
    };
    let mut changed = false;
    for (name, value) in dependencies {
        let Some(value) = value.as_str() else {
            scan.out.problems.push(name.clone());
            continue;
        };
        let Some(local) = value.strip_prefix("file:") else { continue };
        if name.is_empty() || name.contains(['/', '\\', ':']) || name.contains("..") {
            scan.out.problems.push(name.clone());
            continue;
        }
        let (inside, absolute) = (lexical(local), Path::new(local).is_absolute());
        // Copied with the project, whatever links lead to it: its entry names it in the trial as it did in the original.
        if !absolute && inside.as_ref().is_some_and(|i| scan.out.items.contains_key(i)) {
            let package = inside.unwrap();
            if nested(scan, &package)? {
                scan.out.problems.push(name.clone());
            }
            continue;
        }
        let at = if absolute { PathBuf::from(local) } else { original.join("Packages").join(local) };
        let Ok(real) = std::fs::canonicalize(&at) else {
            scan.out.missing.push(name.clone());
            continue;
        };
        let folder = real.is_dir();
        let parent = if folder { real.clone() } else { real.parent().unwrap_or(&real).to_path_buf() };
        let root = ReadRoot::open(&parent).map_err(|e| anyhow::anyhow!("{}", tr!("{path} cannot be read: {error}", path = at.display(), error = e)))?;
        // Named by its whole path though it is inside the original: the trial names its own copy.
        let own = scan.out.roots[0].holds(&root.protected()).then(|| crate::owned::id_of(&real).ok()).flatten();
        if let Some(id) = own {
            let found = scan.out.items.iter().find_map(|(at, item)| match item {
                Item::File(o) if !folder => scan.out.open(o).ok().and_then(|f| f.id().ok()).filter(|i| *i == id).map(|_| at.clone()),
                Item::Dir if folder => crate::owned::id_of(&original.join(at)).ok().filter(|i| *i == id).map(|_| at.clone()),
                _ => None,
            });
            match found {
                Some(rel) => {
                    if nested(scan, &rel)? {
                        scan.out.problems.push(name.clone());
                    }
                    manifest["dependencies"][&name] = entry_for(&rel).into();
                    changed = true;
                    scan.out.localized.push(name);
                }
                // Somewhere a copy leaves out (Library, Temp).
                None => scan.out.problems.push(name.clone()),
            }
            continue;
        }
        scan.out.outside.push(Outside { name: name.clone(), from: real.clone() });
        // A folder the original takes a package from is never a trial's place (an archive has no inside to write in).
        let p = root.protected();
        if folder && !scan.out.protected.contains(&p) {
            scan.out.protected.push(p);
        }
        let target = PathBuf::from("Packages").join(if folder { name.clone() } else { format!("{name}.tgz") });
        let taken = scan.out.items.keys().any(|k| k.to_string_lossy().eq_ignore_ascii_case(&target.to_string_lossy()));
        if taken {
            scan.out.problems.push(target.display().to_string());
            continue;
        }
        let id = root.id;
        scan.out.roots.push(root);
        let new = scan.out.roots.len() - 1;
        if folder {
            scan.put(target.clone(), Item::Dir);
            scan.open.push(id);
            scan.folder(new, Path::new(""), &target, 1, false)?;
            scan.open.pop();
        } else {
            let file = real.file_name().map(PathBuf::from).unwrap_or_default();
            let origin = Origin { root: new, rel: file, follow: true };
            scan.out.bytes = scan.out.bytes.saturating_add(size_of(&mut scan.out.open(&origin)?)?);
            scan.put(target.clone(), Item::File(origin));
        }
        if nested(scan, &target)? {
            scan.out.problems.push(name.clone());
        }
        manifest["dependencies"][&name] = entry_for(&target).into();
        changed = true;
        scan.out.localized.push(name);
    }
    if changed {
        scan.out.items.insert(PathBuf::from(MANIFEST), Item::Made(serde_json::to_vec_pretty(&manifest)?));
    }
    Ok(())
}

/// Whether the package at `at` of the trial asks for local packages of its own, or cannot be read as a package.
fn nested(scan: &Scan, at: &Path) -> anyhow::Result<bool> {
    let manifest = match scan.out.items.get(at) {
        Some(Item::Dir) => match scan.out.items.get(&at.join("package.json")) {
            Some(Item::File(o)) => {
                let mut bytes = vec![];
                scan.out.open(o)?.read_to_end(&mut bytes)?;
                bytes
            }
            _ => return Ok(true),
        },
        Some(Item::File(o)) => match archive_manifest(scan.out.open(o)?) {
            Some(bytes) => bytes,
            None => return Ok(true),
        },
        _ => return Ok(true),
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&manifest) else { return Ok(true) };
    let deps = match v.get("dependencies") {
        None => return Ok(false),
        Some(serde_json::Value::Object(d)) => d,
        Some(_) => return Ok(true),
    };
    Ok(deps.values().any(|d| d.as_str().is_none_or(|s| s.starts_with("file:"))))
}

/// `package.json` of a package archive (`.tgz`), read in a stream.
fn archive_manifest(file: ReadFile) -> Option<Vec<u8>> {
    const MOST: u64 = 512 * 1024 * 1024;
    let mut tar = flate2::read::GzDecoder::new(file).take(MOST);
    loop {
        let mut head = [0u8; 512];
        tar.read_exact(&mut head).ok()?;
        if head.iter().all(|b| *b == 0) {
            return None;
        }
        let text = |b: &[u8]| String::from_utf8_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]).trim().to_string();
        let size = u64::from_str_radix(&text(&head[124..136]), 8).ok()?;
        let name = text(&head[..100]);
        let blocks = size.div_ceil(512) * 512;
        if name.trim_start_matches("./") == "package/package.json" {
            let mut bytes = vec![];
            (&mut tar).take(size.min(8 * 1024 * 1024)).read_to_end(&mut bytes).ok()?;
            return Some(bytes);
        }
        std::io::copy(&mut (&mut tar).take(blocks), &mut std::io::sink()).ok()?;
    }
}

/// The trial's own project settings: its product GUID and, when it was the project's, its name.
fn settings(out: &mut Payload, original: &Path, name: &str, guid: &str) -> anyhow::Result<()> {
    let Some(Item::File(o)) = out.items.get(Path::new(SETTINGS)).cloned() else { return Ok(()) };
    let mut bytes = vec![];
    out.open(&o)?.read_to_end(&mut bytes)?;
    let Ok(text) = String::from_utf8(bytes) else {
        out.problems.push(SETTINGS.to_string());
        return Ok(());
    };
    let from = original.file_name().map(|n| n.to_string_lossy().into_owned());
    let own = crate::templates::retarget_with(&text, name, from.as_deref(), guid);
    out.items.insert(PathBuf::from(SETTINGS), Item::Made(own.into_bytes()));
    Ok(())
}

/// What a file is, byte for byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Digest {
    Dir,
    File { len: u64, sha: [u8; 32] },
}

/// Every file and folder of a tree by its path, with the SHA-256 of each file.
pub type Digests = BTreeMap<PathBuf, Digest>;

const PIECE: usize = 1024 * 1024;

fn digest(mut file: impl Read, buffer: &mut [u8], cancel: &AtomicBool, mut each: impl FnMut(u64)) -> anyhow::Result<Digest> {
    let mut sha = ring::digest::Context::new(&ring::digest::SHA256);
    let mut len = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            anyhow::bail!("{}", tr("Stopped."));
        }
        let n = file.read(buffer)?;
        if n == 0 {
            break;
        }
        sha.update(&buffer[..n]);
        len += n as u64;
        each(n as u64);
    }
    Ok(Digest::File { len, sha: sha.finish().as_ref().try_into().expect("a SHA-256") })
}

impl Payload {
    pub fn open(&self, origin: &Origin) -> std::io::Result<ReadFile> {
        self.roots[origin.root].file(&origin.rel, origin.follow)
    }

    /// Files to make.
    pub fn files(&self) -> usize {
        self.items.values().filter(|i| !matches!(i, Item::Dir)).count()
    }

    /// What every file of the trial must be, read from the original in one pass (one buffer, whatever the sizes).
    pub fn digests(&self, cancel: &AtomicBool, mut progress: impl FnMut(&Path)) -> anyhow::Result<Digests> {
        let mut buffer = vec![0u8; PIECE];
        let mut out = Digests::new();
        for (at, item) in &self.items {
            progress(at);
            let d = match item {
                Item::Dir => Digest::Dir,
                Item::File(o) => digest(self.open(o)?, &mut buffer, cancel, |_| {}).map_err(|e| anyhow::anyhow!("{}", tr!("{path} cannot be read: {error}", path = at.display(), error = format!("{e:#}"))))?,
                Item::Made(bytes) => digest(&bytes[..], &mut buffer, cancel, |_| {})?,
            };
            out.insert(at.clone(), d);
        }
        Ok(out)
    }
}

/// What the trial holds, read in one pass. Anything that is not a plain file or folder is an error of its own.
pub fn trial_digests(trial: &TrialRoot, cancel: &AtomicBool, mut progress: impl FnMut(&Path)) -> anyhow::Result<(Digests, Vec<String>)> {
    let mut buffer = vec![0u8; PIECE];
    let (mut out, mut odd) = (Digests::new(), vec![]);
    let mut folders = vec![PathBuf::new()];
    while let Some(folder) = folders.pop() {
        for e in trial.entries(&folder)? {
            let at = folder.join(&e.name);
            progress(&at);
            match e.kind {
                Kind::Dir => {
                    out.insert(at.clone(), Digest::Dir);
                    folders.push(at);
                }
                Kind::File { .. } => {
                    out.insert(at.clone(), digest(trial.file(&at)?, &mut buffer, cancel, |_| {})?);
                }
                Kind::Link { .. } | Kind::Other => odd.push(at.display().to_string()),
            }
        }
    }
    Ok((out, odd))
}

/// How the trial differs from what it must be, one sentence per path (the marker of a copy in progress left out).
pub fn differences(wanted: &Digests, found: &Digests, odd: &[String]) -> Vec<String> {
    let mut out: Vec<String> = odd.iter().map(|p| tr!("{path}: a link or something else that is not a file", path = p)).collect();
    for (at, want) in wanted {
        match found.get(at) {
            None => out.push(tr!("{path}: missing", path = at.display())),
            Some(have) if have != want => out.push(tr!("{path}: not the same contents", path = at.display())),
            Some(_) => {}
        }
    }
    for at in found.keys().filter(|k| !wanted.contains_key(*k) && k.as_path() != Path::new(crate::templates::INCOMPLETE)) {
        out.push(tr!("{path}: not from the original", path = at.display()));
    }
    out
}

/// A tree's digests as text, one line each, for the file kept beside a trial's logs.
pub fn to_text(digests: &Digests) -> String {
    let mut out = String::new();
    for (at, d) in digests {
        match d {
            Digest::Dir => out.push_str(&format!("d - {}\n", slashes(at))),
            Digest::File { len, sha } => out.push_str(&format!("{len} {} {}\n", hex(sha), slashes(at))),
        }
    }
    out
}

pub fn from_text(text: &str) -> Option<Digests> {
    let mut out = Digests::new();
    for line in text.lines() {
        let mut parts = line.splitn(3, ' ');
        let (len, sha, at) = (parts.next()?, parts.next()?, parts.next()?);
        let digest = if len == "d" {
            Digest::Dir
        } else {
            let bytes: Vec<u8> = (0..32).map(|i| u8::from_str_radix(sha.get(i * 2..i * 2 + 2)?, 16).ok()).collect::<Option<_>>()?;
            Digest::File { len: len.parse().ok()?, sha: bytes.try_into().ok()? }
        };
        out.insert(at.split('/').collect(), digest);
    }
    Some(out)
}

/// The content IDs of the project at `project` now, and whether all of it could be read: what an upload from it is of.
pub fn content_of(project: &Path, cancel: &AtomicBool) -> anyhow::Result<BTreeSet<String>> {
    let root = ReadRoot::open(project).map_err(|e| anyhow::anyhow!("{}", tr!("{path} cannot be read: {error}", path = project.display(), error = e)))?;
    let id = root.id;
    let mut scan = Scan {
        cancel,
        out: Payload {
            roots: vec![root],
            items: BTreeMap::new(),
            bytes: 0,
            linked: vec![],
            outside: vec![],
            localized: vec![],
            protected: vec![],
            content: BTreeSet::new(),
            problems: vec![],
            missing: vec![],
        },
        open: vec![id],
    };
    for top in ["Assets", "Packages"] { // i18n: not shown (folder names)
        let has = scan.out.roots[0].entries(Path::new(""))?.into_iter().find(|e| e.name.eq_ignore_ascii_case(top));
        match has.map(|e| e.kind) {
            Some(Kind::Dir) => scan.folder(0, Path::new(top), Path::new(top), 2, true)?,
            Some(Kind::Link { dir: true }) => {
                let target = scan.out.roots[0].follow_dir(Path::new(top))?;
                scan.out.roots.push(target);
                let new = scan.out.roots.len() - 1;
                scan.folder(new, Path::new(""), Path::new(top), 2, true)?;
            }
            _ => {}
        }
    }
    Ok(scan.out.content)
}

#[cfg(test)]
mod tests;
