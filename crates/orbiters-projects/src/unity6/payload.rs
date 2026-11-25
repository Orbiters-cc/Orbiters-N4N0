//! What a copy of a project is made of, read from the original and nothing else: every file and folder a copy takes, what the
//! original links to or takes packages from outside it (where a copy must never go), and the few files whose bytes are the
//! copy's own (its product GUID, the entries of its local packages, packages it was missing). The original is only read
//! ([`ReadRoot`]). A trial and a duplicate are both made of one.
//!
//! A linked folder is copied as ordinary files: the copy holds no link at all, so nothing in it is anything the original
//! can see. (Refusing such projects instead would leave everyone who shares assets between projects without a copy; what
//! made links dangerous is the copy's place, which is checked against every one of them before anything is made.)

use super::content::{self, Content};
use super::gate::{Entry, Kind, Protected, ReadFile, ReadRoot, WriteRoot};
use super::journal::hex;
use crate::size::Need;
use crate::templates::Keep;
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
    /// The copy's own bytes, made from the original's.
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

/// What a copy is for.
#[derive(Clone, Copy, Default)]
pub struct Making<'a> {
    pub keep: Keep,
    /// What makes it a project of its own (`None`: the files as they are).
    pub own: Option<Own<'a>>,
    /// A trial: everything it cannot do without must be there and understood, or it is refused. Else a project is copied as it
    /// is, its own faults included.
    pub strict: bool,
    /// The Unity version of a project that does not say one.
    pub unity: Option<&'a str>,
    /// Packages only its Library holds (from Git, another registry) become packages of the copy's own, as they are installed: a
    /// copy without the Library has them without the place they came from.
    pub installed: bool,
}

/// A copy's own settings.
#[derive(Clone, Copy)]
pub struct Own<'a> {
    pub name: &'a str,
    /// The product name it takes `name` in the place of (`None`: whatever it was).
    pub from: Option<&'a str>,
    pub guid: &'a str,
}

/// Everything a copy of one project holds.
pub struct Payload {
    roots: Vec<ReadRoot>,
    /// By its path in the copy, folders before what is in them.
    pub items: BTreeMap<PathBuf, Item>,
    pub bytes: u64,
    /// What it takes on a disk (PRJ-4).
    pub need: Need,
    pub linked: Vec<Linked>,
    /// Links that lead nowhere (what they led to was removed) or back to a folder they are in: Unity does without, and so does a copy
    /// that is not [`Making::strict`] (PRJ-7).
    pub nowhere: Vec<PathBuf>,
    pub outside: Vec<Outside>,
    /// Local packages whose `Packages/manifest.json` entry is changed in the copy.
    pub localized: Vec<String>,
    /// What a copy must never go into: the original, what it links to, its local packages.
    pub protected: Vec<Protected>,
    /// Layouts a copy cannot keep whole.
    pub problems: Vec<String>,
    /// Local packages that are not where the project says, and where that is.
    pub missing: Vec<(String, PathBuf)>,
    /// Why a local package is not in a copy that is made without it (not [`Making::strict`]).
    pub late: Option<String>,
    /// Packages only its Library should hold, and does not (or not one that is surely theirs): whoever opens the copy downloads
    /// them.
    pub absent: Vec<String>,
}

/// What a copy leaves out, at any depth ([`crate::templates`]' rules).
fn left_out(entry: &Entry, depth: usize, keep: Keep, project: bool) -> bool {
    let dir = matches!(entry.kind, Kind::Dir | Kind::Link { dir: true });
    crate::templates::left_out(&entry.name.to_string_lossy(), depth, dir, keep, project)
}

/// Files Unity cannot do without: a project missing one is not copied.
pub const NEEDED: [&str; 3] = ["ProjectSettings/ProjectVersion.txt", "ProjectSettings/ProjectSettings.asset", "Packages/manifest.json"];
const SETTINGS: &str = "ProjectSettings/ProjectSettings.asset";
const MANIFEST: &str = "Packages/manifest.json";
const VPM_MANIFEST: &str = "Packages/vpm-manifest.json";
pub const VERSION_FILE: &str = "ProjectSettings/ProjectVersion.txt";

/// How deep links may lead through other links.
const DEEPEST: usize = 64;

struct Scan<'a> {
    cancel: &'a AtomicBool,
    how: Making<'a>,
    out: Payload,
    /// The folders being gone through, so that a link to one of them is refused instead of followed for ever.
    open: Vec<crate::owned::FileId>,
    /// The folders the links followed to here are in.
    ways: Vec<Protected>,
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
            let out = left_out(&e, depth, self.how.keep, project);
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
                    Err(_) if !self.how.strict && self.leads_nowhere(root, &from) => {
                        self.out.nowhere.push(to);
                        continue;
                    }
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
                    let here = Protected { id: None, real: self.out.roots[root].protected().real.join(rel), shown: PathBuf::new() };
                    let back = self.open.contains(&target.id) || std::iter::once(&here).chain(&self.ways).any(|way| target.holds(way));
                    if back && !self.how.strict {
                        self.out.nowhere.push(to);
                        continue;
                    }
                    if back || self.open.len() > DEEPEST {
                        anyhow::bail!("{}", tr!("The link {path} leads back to a folder it is in: a copy of it would have no end.", path = to.display()));
                    }
                    self.open.push(target.id);
                    self.ways.push(here);
                    self.out.roots.push(target);
                    let new = self.out.roots.len() - 1;
                    self.put(to.clone(), Item::Dir);
                    self.folder(new, Path::new(""), &to, depth + 1, project)?;
                    self.ways.pop();
                    self.open.pop();
                }
                Kind::File { .. } | Kind::Link { dir: false } => {
                    let follow = matches!(e.kind, Kind::Link { .. } | Kind::File { online: true });
                    let len = if matches!(e.kind, Kind::Link { .. }) { size_of(&mut self.out.roots[root].file(&from, true)?)? } else { e.len };
                    self.out.takes(len);
                    self.put(to, Item::File(Origin { root, rel: from, follow }));
                }
                Kind::Other if !self.how.strict => {}
                Kind::Other => anyhow::bail!("{}", tr!("{path} is not a file or a folder: a copy cannot keep it.", path = to.display())),
            }
        }
        Ok(())
    }

    /// Whether the link `rel` of root `root` leads to nothing that is there. Where it says is no place for the copy either: made there,
    /// the copy would be what the original links to.
    fn leads_nowhere(&mut self, root: usize, rel: &Path) -> bool {
        let link = self.out.roots[root].shown.join(rel);
        if link.exists() {
            return false;
        }
        let to = std::fs::read_link(&link).ok().and_then(|to| Some(link.parent()?.join(to)));
        // Its place, from the nearest folder of it that is there.
        let there = to.as_deref().and_then(|to| to.ancestors().find(|a| a.is_dir()).map(|a| (a, to.strip_prefix(a).unwrap_or(Path::new("")))));
        if let Some((above, rest)) = there {
            if let Ok(p) = ReadRoot::open(above).map(|r| r.protected()) {
                self.out.protected.push(Protected { id: None, real: p.real.join(rest), shown: p.shown.join(rest) });
            }
        }
        true
    }

    fn put(&mut self, at: PathBuf, item: Item) {
        if let Item::Dir = item {
            self.out.need.folder();
        }
        if self.out.items.insert(at.clone(), item).is_some() {
            self.out.problems.push(at.display().to_string());
        }
    }

    /// All of the file that `item` is, or `None` when it is not one.
    fn read(&self, at: &Path) -> anyhow::Result<Option<Vec<u8>>> {
        self.out.read(at)
    }

    /// `name` is a fault of the project: a trial is not made of it; any other copy is, and says so (`why`).
    fn fault(&mut self, name: &str, why: impl FnOnce() -> String) {
        if self.how.strict {
            self.out.problems.push(name.to_string());
        } else if self.out.late.is_none() {
            self.out.late = Some(why());
        }
    }
}

fn size_of(file: &mut ReadFile) -> anyhow::Result<u64> {
    Ok(std::io::copy(file, &mut std::io::sink())?)
}

/// Reads what a copy of `original` holds.
pub fn scan(original: &Path, how: Making, cancel: &AtomicBool) -> anyhow::Result<Payload> {
    let root = ReadRoot::open(original).map_err(|e| anyhow::anyhow!("{}", tr!("{path} cannot be read: {error}", path = original.display(), error = e)))?;
    let (id, protected) = (root.id, root.protected());
    let mut scan = Scan { cancel, how, out: Payload::of(root, protected), open: vec![id], ways: vec![] };
    scan.folder(0, Path::new(""), Path::new(""), 1, true)?;
    if how.strict {
        for needed in NEEDED {
            if !matches!(scan.out.items.get(Path::new(needed)), Some(Item::File(_))) {
                scan.out.problems.push(needed.to_string());
            }
        }
    }
    if let Some(own) = how.own {
        local_packages(&mut scan, original)?;
        settings(&mut scan, own)?;
    }
    if how.installed {
        installed(&mut scan, original)?;
    }
    if let (Some(unity), false) = (how.unity, scan.out.items.contains_key(Path::new(VERSION_FILE))) {
        scan.out.items.entry(PathBuf::from("ProjectSettings")).or_insert(Item::Dir); // i18n: not shown (a folder's name)
        scan.out.items.insert(PathBuf::from(VERSION_FILE), Item::Made(format!("m_EditorVersion: {unity}\n").into_bytes()));
    }
    if how.strict {
        // Two names the copy's disk would take for one (a package of the project's own in the place of a local one).
        let mut seen = BTreeSet::new();
        let twice: Vec<String> = scan.out.items.keys().filter(|k| !seen.insert(k.to_string_lossy().to_lowercase())).map(|k| k.display().to_string()).collect();
        scan.out.problems.extend(twice);
    }
    scan.out.problems.sort();
    scan.out.problems.dedup();
    Ok(scan.out)
}

const CACHE: &str = "PackageCache";

/// The folder of the `library`'s package cache that is surely `name` as its lock has it: the only one of that name, or the one
/// its lock's version or Git commit names.
fn cached(library: &ReadRoot, lock: &serde_json::Value, name: &str) -> Option<PathBuf> {
    let kept = library.entries(Path::new(CACHE)).ok()?;
    let mine: Vec<&Entry> = kept.iter().filter(|e| e.kind == Kind::Dir && e.name.to_string_lossy().split('@').next() == Some(name)).collect();
    let entry = &lock["dependencies"][name];
    let is = |e: &&&Entry| {
        let full = e.name.to_string_lossy();
        let Some((_, suffix)) = full.split_once('@') else { return false };
        let named = |field: &str| entry[field].as_str().is_some_and(|v| v == suffix || (suffix.len() >= 8 && v.starts_with(suffix)));
        named("version") || named("hash")
    };
    let one = match mine.len() {
        1 => mine.first(),
        _ => mine.iter().find(is),
    }?;
    Some(Path::new(CACHE).join(&one.name))
}

/// [`Making::installed`].
fn installed(scan: &mut Scan, original: &Path) -> anyhow::Result<()> {
    let names = crate::templates::packages_only_in_library(original);
    if names.is_empty() {
        return Ok(());
    }
    let lock = scan.read(Path::new("Packages/packages-lock.json"))?.and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    for name in names {
        let target = PathBuf::from("Packages").join(&name);
        // (The Library may be a link to another disk: it is opened as what it is.)
        let found = ReadRoot::open(&original.join("Library")).ok().and_then(|library| {
            let rel = cached(&library, &lock, &name)?;
            ReadRoot::open(&library.shown.join(rel)).ok()
        });
        let Some(root) = found.filter(|r| r.entries(Path::new("")).is_ok_and(|e| e.iter().any(|e| e.name == "package.json"))) else {
            scan.out.absent.push(name);
            continue;
        };
        let id = root.id;
        scan.out.roots.push(root);
        let new = scan.out.roots.len() - 1;
        scan.put(target.clone(), Item::Dir);
        scan.open.push(id);
        scan.folder(new, Path::new(""), &target, 1, false)?;
        scan.open.pop();
    }
    Ok(())
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

/// The project's local packages: those outside it become part of the copy, under `Packages`, and their entries name them
/// there; those inside it keep (or get) an entry that names them inside the copy.
fn local_packages(scan: &mut Scan, original: &Path) -> anyhow::Result<()> {
    if scan.how.strict {
        for lock in ["Packages/packages-lock.json", VPM_MANIFEST] {
            if let Some(bytes) = scan.read(Path::new(lock))? {
                match serde_json::from_slice::<serde_json::Value>(&bytes) {
                    Ok(v) if v.is_object() => {}
                    _ => scan.out.problems.push(lock.to_string()),
                }
            }
        }
    }
    let Some(bytes) = scan.read(Path::new(MANIFEST))? else { return Ok(()) };
    let unreadable = || tr!("{path} cannot be read: {error}", path = MANIFEST, error = tr("it is not a package list"));
    let mut manifest: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v @ serde_json::Value::Object(_)) => v,
        _ => {
            scan.fault(MANIFEST, unreadable);
            return Ok(());
        }
    };
    let dependencies = match manifest.get("dependencies") {
        None => return Ok(()),
        Some(serde_json::Value::Object(d)) => d.clone(),
        Some(_) => {
            scan.fault(MANIFEST, unreadable);
            return Ok(());
        }
    };
    let strict = scan.how.strict;
    let mut changed = false;
    for (name, value) in dependencies {
        let Some(value) = value.as_str() else {
            if strict {
                scan.out.problems.push(name);
            }
            continue;
        };
        let Some(local) = value.strip_prefix("file:") else { continue };
        if name.is_empty() || name.contains(['/', '\\', ':']) || name.contains("..") {
            scan.fault(&name, || tr!("Invalid package name: {name}", name = name));
            continue;
        }
        let (inside, absolute) = (lexical(local), Path::new(local).is_absolute());
        // Copied with the project, whatever links lead to it: its entry names it in the copy as it did in the original.
        if let Some(package) = inside.filter(|i| !absolute && scan.out.items.contains_key(i)) {
            if strict && !package_whole(scan, &package, &name)? {
                scan.out.problems.push(name.clone());
            }
            continue;
        }
        let at = if absolute { PathBuf::from(local) } else { original.join("Packages").join(local) };
        let Ok(real) = std::fs::canonicalize(&at) else {
            scan.out.missing.push((name.clone(), at));
            continue;
        };
        let folder = real.is_dir();
        let parent = if folder { real.clone() } else { real.parent().unwrap_or(&real).to_path_buf() };
        let root = ReadRoot::open(&parent).map_err(|e| anyhow::anyhow!("{}", tr!("{path} cannot be read: {error}", path = at.display(), error = e)))?;
        // Named by its whole path though it is inside the original: the copy names its own.
        let own = scan.out.roots[0].holds(&root.protected()).then(|| crate::owned::id_of(&real).ok()).flatten();
        if let Some(id) = own {
            let found = scan.out.items.iter().find_map(|(at, item)| match item {
                Item::File(o) if !folder => scan.out.open(o).ok().and_then(|f| f.id().ok()).filter(|i| *i == id).map(|_| at.clone()),
                Item::Dir if folder => crate::owned::id_of(&original.join(at)).ok().filter(|i| *i == id).map(|_| at.clone()),
                _ => None,
            });
            match found {
                Some(rel) => {
                    if strict && !package_whole(scan, &rel, &name)? {
                        scan.out.problems.push(name.clone());
                    }
                    manifest["dependencies"][&name] = entry_for(&rel).into();
                    changed = true;
                    scan.out.localized.push(name);
                    continue;
                }
                // Somewhere a copy leaves out (Library, Temp).
                None if strict => {
                    scan.out.problems.push(name.clone());
                    continue;
                }
                None => {}
            }
        }
        scan.out.outside.push(Outside { name: name.clone(), from: real.clone() });
        // A folder the original takes a package from is never a copy's place (an archive has no inside to write in).
        let p = root.protected();
        if folder && !scan.out.protected.contains(&p) {
            scan.out.protected.push(p);
        }
        let target = PathBuf::from("Packages").join(if folder { name.clone() } else { format!("{name}.tgz") });
        let taken = scan.out.items.keys().find(|k| k.to_string_lossy().eq_ignore_ascii_case(&target.to_string_lossy())).cloned();
        let file = PathBuf::from(real.file_name().unwrap_or_default());
        match taken {
            Some(_) if strict => {
                scan.out.problems.push(target.display().to_string());
                continue;
            }
            // The project's own folder of that name is the package, as Unity takes it.
            Some(_) if folder => {}
            // The same archive is kept; another one is never written over.
            Some(own) => {
                let bytes = root.read(&file)?;
                if scan.read(&own)?.as_deref() != Some(&bytes[..]) {
                    scan.out.takes(bytes.len() as u64);
                    scan.fault(&name, || {
                        tr!("{file} in the copy's Packages folder is another archive than the one {name} needs ({path})", file = format!("{name}.tgz"), name = name, path = real.display())
                    });
                    continue;
                }
            }
            None => {
                let id = root.id;
                scan.out.roots.push(root);
                let new = scan.out.roots.len() - 1;
                if folder {
                    scan.put(target.clone(), Item::Dir);
                    scan.open.push(id);
                    scan.folder(new, Path::new(""), &target, 1, false)?;
                    scan.open.pop();
                } else {
                    let origin = Origin { root: new, rel: file, follow: true };
                    let len = size_of(&mut scan.out.open(&origin)?)?;
                    scan.out.takes(len);
                    scan.put(target.clone(), Item::File(origin));
                }
                if strict && !package_whole(scan, &target, &name)? {
                    scan.out.problems.push(name.clone());
                }
            }
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

/// Whether the package at `at` of the copy is one Unity takes as `name`: its `package.json` an object that says that name and
/// a version, with no local package of its own (nothing copies those).
fn package_whole(scan: &Scan, at: &Path, name: &str) -> anyhow::Result<bool> {
    let manifest = match scan.out.items.get(at) {
        Some(Item::Dir) => scan.read(&at.join("package.json"))?,
        Some(Item::File(o)) => archive_manifest(scan.out.open(o)?),
        _ => None,
    };
    let package = manifest.and_then(|m| serde_json::from_slice::<serde_json::Value>(&m).ok());
    let Some(serde_json::Value::Object(package)) = package else { return Ok(false) };
    let version = package.get("version").and_then(|v| v.as_str()).is_some_and(|v| v.parse::<vrc_get_vpm::version::Version>().is_ok());
    let dependencies = match package.get("dependencies") {
        None => true,
        Some(serde_json::Value::Object(d)) => d.values().all(|d| d.as_str().is_some_and(|s| !s.starts_with("file:"))),
        Some(_) => false,
    };
    Ok(version && dependencies && package.get("name").and_then(|n| n.as_str()) == Some(name))
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

/// The copy's own project settings: its product GUID and, when it was the project's, its name.
fn settings(scan: &mut Scan, own: Own) -> anyhow::Result<()> {
    let Some(bytes) = scan.read(Path::new(SETTINGS))? else { return Ok(()) };
    let Ok(text) = String::from_utf8(bytes) else {
        scan.fault(SETTINGS, || tr!("{path} cannot be read: {error}", path = SETTINGS, error = tr("it is not text")));
        return Ok(());
    };
    let mine = crate::templates::retarget_with(&text, own.name, own.from, own.guid);
    scan.out.items.insert(PathBuf::from(SETTINGS), Item::Made(mine.into_bytes()));
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
    fn of(root: ReadRoot, protected: Protected) -> Self {
        Payload {
            roots: vec![root],
            items: BTreeMap::new(),
            bytes: 0,
            need: Need::default(),
            linked: vec![],
            nowhere: vec![],
            outside: vec![],
            localized: vec![],
            protected: vec![protected],
            problems: vec![],
            missing: vec![],
            late: None,
            absent: vec![],
        }
    }

    /// A file of `len` bytes is in it.
    fn takes(&mut self, len: u64) {
        self.bytes = self.bytes.saturating_add(len);
        self.need.file(len);
    }

    /// The whole file at `at` of the copy; `None` when it has none.
    pub fn read(&self, at: &Path) -> anyhow::Result<Option<Vec<u8>>> {
        match self.items.get(at) {
            Some(Item::File(o)) => {
                let mut bytes = vec![];
                self.open(o)?.read_to_end(&mut bytes)?;
                Ok(Some(bytes))
            }
            Some(Item::Made(bytes)) => Ok(Some(bytes.clone())),
            _ => Ok(None),
        }
    }

    /// Whether every VPM package its lock names is in it, at that version.
    pub fn packages_complete(&self) -> anyhow::Result<bool> {
        crate::templates::dependencies_complete(|rel| self.read(Path::new(rel)))
    }

    /// The content IDs of everything in it: the copy has the same.
    pub fn content(&self, cancel: &AtomicBool) -> anyhow::Result<Content> {
        let mut out = Content::default();
        let manifest = self.read(Path::new(MANIFEST))?;
        for (name, at) in archives(manifest.as_deref()) {
            match at.as_ref().and_then(|at| Some((at, self.items.get(at)?))) {
                Some((at, Item::File(o))) => {
                    let read = self.open(o).and_then(|f| out.read_archive(at, f));
                    read.map_err(|e| anyhow::anyhow!("{}", tr!("{path} cannot be read: {error}", path = at.display(), error = e)))?;
                }
                Some((_, Item::Dir)) => {}
                _ => out.unreadable(name),
            }
        }
        for (at, item) in &self.items {
            if cancel.load(Ordering::Relaxed) {
                anyhow::bail!("{}", tr("Stopped."));
            }
            let Item::File(o) = item else { continue };
            if content::carries(at) {
                let read = self.open(o).and_then(|f| f.len().and_then(|len| out.read(at, f, len)));
                read.map_err(|e| anyhow::anyhow!("{}", tr!("{path} cannot be read: {error}", path = at.display(), error = e)))?;
            }
        }
        Ok(out)
    }

    /// What was restored in the folder `stage` (a project's own layout): the packages `names`, and the package lists.
    pub fn restored(stage: &Path, names: &[std::ffi::OsString], cancel: &AtomicBool) -> anyhow::Result<Payload> {
        let root = ReadRoot::open(stage)?;
        let (id, protected) = (root.id, root.protected());
        let mut scan = Scan { cancel, how: Making::default(), out: Payload::of(root, protected), open: vec![id], ways: vec![] };
        for name in names {
            let at = Path::new("Packages").join(name); // i18n: not shown (a folder's name)
            scan.put(at.clone(), Item::Dir);
            scan.folder(0, &at, &at, 2, false)?;
        }
        for list in [VPM_MANIFEST, MANIFEST] {
            if let Ok(bytes) = scan.out.roots[0].read(Path::new(list)) {
                scan.out.items.insert(PathBuf::from(list), Item::Made(bytes));
            }
        }
        Ok(scan.out)
    }

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

/// What the copy holds, read in one pass. Anything that is not a plain file or folder is an error of its own.
pub fn trial_digests(trial: &WriteRoot, cancel: &AtomicBool, mut progress: impl FnMut(&Path)) -> anyhow::Result<(Digests, Vec<String>)> {
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

/// The local (`file:`) packages a package list names, and where each is below the project (`None`: not inside it).
fn archives(manifest: Option<&[u8]>) -> Vec<(String, Option<PathBuf>)> {
    let manifest: serde_json::Value = manifest.and_then(|m| serde_json::from_slice(m).ok()).unwrap_or_default();
    let named = manifest["dependencies"].as_object().into_iter().flatten();
    named.filter_map(|(name, v)| Some((name.clone(), lexical(v.as_str()?.strip_prefix("file:")?)))).collect()
}

/// The content IDs of everything in a copy as it is now, read through the proof of its folder.
pub fn copy_content(root: &WriteRoot, cancel: &AtomicBool) -> anyhow::Result<Content> {
    let mut out = Content::default();
    let mut manifest = vec![];
    match root.file(Path::new(MANIFEST)) {
        Ok(mut file) => drop(file.read_to_end(&mut manifest)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    for (name, at) in archives(Some(&manifest)) {
        let Some(at) = at else {
            out.unreadable(name);
            continue;
        };
        match root.file(&at) {
            Ok(file) => out.read_archive(&at, file)?,
            // A folder: looked at with the rest.
            Err(_) if root.entries(&at).is_ok() => {}
            Err(_) => out.unreadable(name),
        }
    }
    let mut folders = vec![PathBuf::new()];
    while let Some(folder) = folders.pop() {
        for e in root.entries(&folder)? {
            if cancel.load(Ordering::Relaxed) {
                anyhow::bail!("{}", tr("Stopped."));
            }
            let at = folder.join(&e.name);
            match e.kind {
                // What Unity makes again from the rest says nothing of its own.
                Kind::Dir if folder.as_os_str().is_empty() && left_out(&e, 1, Keep::default(), true) => {}
                Kind::Dir => folders.push(at),
                Kind::File { .. } if content::carries(&at) => {
                    let file = root.file(&at)?;
                    let len = file.len()?;
                    out.read(&at, file, len)?;
                }
                Kind::File { .. } => {}
                Kind::Link { .. } | Kind::Other => out.unreadable(at.display().to_string()),
            }
        }
    }
    Ok(out)
}

/// The content IDs of the project at `project` now, its local packages and those only its Library holds included: what an
/// upload from it is of.
pub fn content_of(project: &Path, cancel: &AtomicBool) -> anyhow::Result<Content> {
    let own = Own { name: "", from: None, guid: "" };
    let payload = scan(project, Making { own: Some(own), ..Default::default() }, cancel)?;
    let mut out = payload.content(cancel)?;
    for (name, at) in &payload.missing {
        out.unreadable(format!("{name} ({})", at.display()));
    }
    if payload.late.is_some() {
        out.unreadable(MANIFEST.into());
    }
    // Packages from Git or another registry, which Unity keeps in the Library: every one of that name, the one in use among them.
    let names = crate::templates::packages_only_in_library(project);
    let library = if names.is_empty() { None } else { ReadRoot::open(&project.join("Library")).ok() };
    for name in names {
        let kept = library.as_ref().and_then(|l| l.entries(Path::new(CACHE)).ok()).unwrap_or_default();
        let folders: Vec<_> = kept.iter().filter(|e| e.kind == Kind::Dir && e.name.to_string_lossy().split('@').next() == Some(name.as_str())).collect();
        if folders.is_empty() {
            out.unreadable(name);
            continue;
        }
        for folder in folders {
            let Some(library) = &library else { continue };
            let root = ReadRoot::open(&library.shown.join(CACHE).join(&folder.name))?;
            let (id, protected) = (root.id, root.protected());
            let mut scan = Scan { cancel, how: Making::default(), out: Payload::of(root, protected), open: vec![id], ways: vec![] };
            scan.folder(0, Path::new(""), &Path::new("Packages").join(&name), 2, false)?; // i18n: not shown (a folder's name)
            out.add(scan.out.content(cancel)?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
