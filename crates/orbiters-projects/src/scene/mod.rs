//! Previews of a Unity project's scenes built from its files, for projects Orbiters Toolkit never wrote one for
//! (never opened, opened without Toolkit, or a scene other than the one last saved). Nothing is written into the
//! project: the previews, in the same format Toolkit writes (contracts' Docs/project-preview.md), and what they were
//! built from are kept in the app's cache folder (`%LOCALAPPDATA%/Orbiters/Desktop/cache/project-previews/<project>/`).
//!
//! - [`scenes`] lists a project's scenes: the one Unity last had open first (`Library/LastSceneManagerSetup.txt`),
//!   then those in the build settings in their order, then the others under `Assets`, the most recently saved first.
//! - [`list`] pairs each scene with its preview: Toolkit's when there is one at least as new as the scene file (it
//!   is exact: Unity drew it), else the one built here when nothing it was built from changed, else none yet (or the
//!   reason the last build failed, kept until a file it read changes).
//! - [`build_scene`] builds one on the calling thread, within [`Limits`] (time, files, model and texture sizes,
//!   memory for models), and keeps it. A failure is a short reason ("No preview: …" in the app), never a panic or a
//!   hang.
//!
//! How a scene is read: [`yaml`] (Unity's text format), [`assets`] (GUIDs to files), [`prefab`] (instances, nested
//! prefabs, variants and overrides), [`model`] (FBX and OBJ through ufbx, as Unity imports them), [`material`] and
//! [`texture`] (a material's look and pictures), [`build`] (posing, skinning and writing the file).

pub mod assets;
pub mod build;
pub mod material;
pub mod math;
mod mesh_asset;
pub mod model;
pub mod prefab;
mod primitives;
pub mod serialized;
pub mod texture;
pub mod yaml;

use crate::preview;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How far a build may go before it gives up with a reason.
#[derive(Clone, Debug)]
pub struct Limits {
    pub deadline: Instant,
    pub cancel: Option<Arc<AtomicBool>>,
    pub max_files: usize,
    pub max_model_bytes: u64,
    /// ufbx's own memory for one model (its file's data and what it makes of it).
    pub max_model_memory: usize,
    pub max_model_triangles: usize,
    pub max_nodes: usize,
    pub max_yaml_bytes: u64,
    pub max_prefab_instances: usize,
    /// Objects copied into prefab instances, nested ones included (a world of 50 × 50 × 50 nested copies).
    pub max_objects: usize,
    pub max_texture_bytes: u64,
    /// Threads decoding pictures.
    pub threads: usize,
    /// Its threads run below normal priority (a build in the background never slows the window).
    pub background: bool,
}

impl Limits {
    pub fn new(time: Duration) -> Self {
        Self {
            deadline: Instant::now() + time,
            cancel: None,
            max_files: 400_000,
            max_model_bytes: 600 << 20,
            max_model_memory: 2 << 30,
            max_model_triangles: 6_000_000,
            max_nodes: 200_000,
            max_yaml_bytes: 256 << 20,
            max_prefab_instances: 50_000,
            max_objects: 1_000_000,
            max_texture_bytes: 256 << 20,
            threads: 4,
            background: false,
        }
    }

    /// Lowers the calling thread's priority when the build runs in the background.
    pub fn lower_priority(&self) {
        #[cfg(windows)]
        if self.background {
            use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
            unsafe {
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
            }
        }
    }

    /// Fails once the time is up or the build was cancelled, saying what it was doing.
    pub fn check(&self, doing: &str) -> anyhow::Result<()> {
        if self.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
            anyhow::bail!("stopped");
        }
        if Instant::now() > self.deadline {
            anyhow::bail!("it took too long ({doing})");
        }
        Ok(())
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new(Duration::from_secs(60))
    }
}

/// A scene of a project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneInfo {
    pub path: PathBuf,
    /// As Unity names it (`Assets/Scenes/Main.unity`).
    pub rel: String,
    pub guid: String,
    pub name: String,
    #[serde(with = "system_time")]
    pub modified: SystemTime,
    /// The scene Unity had open last.
    pub last_open: bool,
    /// In the build settings' list.
    pub in_build: bool,
}

mod system_time {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        Ok(UNIX_EPOCH + Duration::from_millis(u64::deserialize(d)?))
    }
}

/// Scenes listed at most (a project with hundreds of sample scenes lists its own first).
const MAX_SCENES: usize = 200;

/// A project's scenes, the one Unity last had open first, then the build settings' in order, then the others under
/// `Assets` from the most recently saved.
pub fn scenes(project: &Path) -> Vec<SceneInfo> {
    let last = last_open_scene(project);
    let build = build_scenes(project);
    let mut found: Vec<PathBuf> = Vec::new();
    let assets = project.join("Assets");
    let walk = walkdir::WalkDir::new(&assets).follow_links(true).max_depth(24).into_iter().filter_entry(|e| {
        let n = e.file_name().to_string_lossy();
        e.depth() == 0 || !(n.starts_with('.') || n.ends_with('~'))
    });
    for e in walk.flatten().take(500_000) {
        if e.file_type().is_file() && e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("unity")) {
            found.push(e.into_path());
        }
    }
    for (rel, _) in &build {
        // A build settings' path out of the project (`../`) or absolute is not one of its scenes.
        if Path::new(rel).components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
            continue;
        }
        let p = project.join(rel);
        if p.is_file() && !found.contains(&p) {
            found.push(p);
        }
    }
    let mut out: Vec<SceneInfo> = found
        .into_iter()
        .filter_map(|path| {
            let rel = path.strip_prefix(project).ok()?.to_string_lossy().replace('\\', "/");
            let guid = std::fs::read_to_string(prefab::meta_path(&path)).ok().and_then(|m| yaml::meta_guid(&m))?;
            let modified = std::fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH);
            let name = path.file_stem()?.to_string_lossy().into_owned();
            let last_open = last.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(&rel));
            let in_build = build.iter().any(|(r, g)| r.eq_ignore_ascii_case(&rel) || g.as_deref() == Some(guid.as_str()));
            Some(SceneInfo { path, rel, guid, name, modified, last_open, in_build })
        })
        .collect();
    let build_order =
        |s: &SceneInfo| build.iter().position(|(r, g)| r.eq_ignore_ascii_case(&s.rel) || g.as_deref() == Some(s.guid.as_str())).unwrap_or(usize::MAX);
    out.sort_by(|a, b| b.last_open.cmp(&a.last_open).then(build_order(a).cmp(&build_order(b))).then(b.modified.cmp(&a.modified)).then(a.rel.cmp(&b.rel)));
    // One scene a GUID (a scene copied with its .meta in the Explorer has its original's: Unity gives the copy a new
    // one when it opens the project), the first in the order above; not only neighbours, which a sort by time leaves.
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| seen.insert(s.guid.clone()));
    out.truncate(MAX_SCENES);
    out
}

/// The scene Unity had active when it last closed the project.
pub fn last_open_scene(project: &Path) -> Option<String> {
    let text = std::fs::read_to_string(project.join("Library/LastSceneManagerSetup.txt")).ok()?;
    let v = yaml::parse(&text);
    let setups = v.get("sceneSetups").items();
    setups
        .iter()
        .find(|s| s.get("isActive").as_bool() == Some(true))
        .or_else(|| setups.first())
        .and_then(|s| s.get("path").as_str())
        .map(str::to_string)
        .filter(|p| p.starts_with("Assets/"))
}

/// The build settings' enabled scenes: path and GUID.
fn build_scenes(project: &Path) -> Vec<(String, Option<String>)> {
    let Ok(text) = std::fs::read_to_string(project.join("ProjectSettings/EditorBuildSettings.asset")) else { return vec![] };
    let v = yaml::parse(&text);
    v.get("EditorBuildSettings")
        .get("m_Scenes")
        .items()
        .iter()
        .filter(|s| s.get("enabled").as_bool().unwrap_or(true))
        .filter_map(|s| Some((s.get("path").as_str()?.to_string(), s.get("guid").as_str().map(str::to_ascii_lowercase))))
        .collect()
}

/// Where this computer keeps built previews: the app's cache folder (`orbiters_core::paths::cache_dir`, what can be
/// made again: a screenshot run gives it a folder of its own, so it never builds into or trims the person's),
/// `ORBITERS_PREVIEW_CACHE` when set (measurements); tests a folder of their own.
pub fn cache_root() -> PathBuf {
    if cfg!(test) {
        return std::env::temp_dir().join(format!("orbiters-preview-cache-test-{}", std::process::id()));
    }
    match std::env::var_os("ORBITERS_PREVIEW_CACHE") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => orbiters_core::paths::cache_dir().join("project-previews"),
    }
}

/// Where this computer keeps a project's built previews.
pub fn cache_dir(project: &Path) -> PathBuf {
    let key = project.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase();
    cache_root().join(format!("{:016x}", model::xxh64(key.as_bytes(), 0)))
}

/// Bumped when a build would come out differently: older previews are built again.
const BUILDER_VERSION: u32 = 4;
/// A Toolkit preview this much older than its scene file is still the scene's (file times copied across disks).
const SLACK: Duration = Duration::from_secs(2);
/// A built preview is built again after this long, whatever changed (an asset added since that it lacked).
const MAX_AGE: Duration = Duration::from_secs(14 * 86_400);
/// A build that ran out of time is tried again after this long (or when a file it read changed), not after
/// [`MAX_AGE`]: the first build of a project never opened reads every `.meta` file of a hard disk, and the GUIDs it read
/// are kept, so the next try gets further. Kept that long, a slow first try said "No preview" for two weeks.
const RETRY_AFTER: Duration = Duration::from_secs(30 * 60);

/// What a build read and when, beside its preview (`<scene GUID>.json`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub version: u32,
    pub scene: String,
    pub built_utc: String,
    pub built_ms: u64,
    /// (path, size, modified ms) of each file it read.
    pub files: Vec<(String, u64, i64)>,
    pub error: Option<String>,
    /// The error is one a later try may not meet (time ran out): tried again after [`RETRY_AFTER`].
    #[serde(default)]
    pub retry: bool,
    pub report: build::Report,
}

fn stamp(path: &Path) -> Option<(u64, i64)> {
    let m = std::fs::metadata(path).ok()?;
    let t = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
    Some((m.len(), t))
}

impl Record {
    /// Nothing it was built from changed since (and it is not too old, from this builder).
    pub fn fresh(&self) -> bool {
        let age = SystemTime::now().duration_since(UNIX_EPOCH + Duration::from_millis(self.built_ms)).unwrap_or_default();
        self.version == BUILDER_VERSION
            && age < if self.retry { RETRY_AFTER } else { MAX_AGE }
            && self.files.iter().all(|(p, size, t)| stamp(Path::new(p)) == Some((*size, *t)))
    }
}

/// A scene's preview as the desktop shows it.
#[derive(Clone, Debug)]
pub enum Source {
    /// Orbiters Toolkit's, at least as new as the scene: exact.
    Toolkit(preview::Entry),
    /// Built here and still up to date.
    Built(preview::Entry),
    /// Built here, but something it was built from changed: shown while it is built again.
    Stale(preview::Entry),
    /// The last build failed (and nothing changed since): why.
    Failed(String),
    /// None yet: to build.
    None,
}

impl Source {
    pub fn entry(&self) -> Option<&preview::Entry> {
        match self {
            Source::Toolkit(e) | Source::Built(e) | Source::Stale(e) => Some(e),
            _ => None,
        }
    }

    /// A build would bring something newer.
    pub fn wants_build(&self) -> bool {
        matches!(self, Source::Stale(_) | Source::None)
    }
}

#[derive(Clone, Debug)]
pub struct Slot {
    pub scene: SceneInfo,
    pub source: Source,
}

/// A project's scenes and their previews, in [`scenes`]' order. Toolkit previews of scenes no longer listed (a scene
/// in a package) come last.
pub fn list(project: &Path) -> Vec<Slot> {
    let toolkit = preview::list(project);
    let dir = cache_dir(project);
    let mut out: Vec<Slot> = scenes(project)
        .into_iter()
        .map(|scene| {
            let exact = toolkit.iter().find(|e| e.header.scene.guid == scene.guid && e.modified + SLACK >= scene.modified).cloned();
            let source = match exact {
                Some(e) => Source::Toolkit(e),
                None => built(&dir, &scene.guid),
            };
            Slot { scene, source }
        })
        .collect();
    for e in toolkit {
        if !out.iter().any(|s| s.scene.guid == e.header.scene.guid) && e.header.scene.guid.len() == 32 {
            let h = &e.header.scene;
            let scene = SceneInfo {
                path: project.join(&h.path),
                rel: h.path.clone(),
                guid: h.guid.clone(),
                name: if h.name.is_empty() { "Scene".into() } else { h.name.clone() },
                modified: e.modified,
                last_open: false,
                in_build: false,
            };
            out.push(Slot { scene, source: Source::Toolkit(e) });
        }
    }
    out
}

/// The preview built here for `guid`, when nothing it was built from changed since (a build asked twice runs once).
pub fn fresh(project: &Path, guid: &str) -> Option<preview::Entry> {
    match built(&cache_dir(project), guid) {
        Source::Built(e) => Some(e),
        _ => None,
    }
}

/// The preview built here for a scene, if any, and how it stands.
fn built(dir: &Path, guid: &str) -> Source {
    let Some(record) = std::fs::read(dir.join(format!("{guid}.json"))).ok().and_then(|b| serde_json::from_slice::<Record>(&b).ok()) else {
        return Source::None;
    };
    let fresh = record.fresh();
    if let Some(e) = record.error.clone() {
        return if fresh { Source::Failed(e) } else { Source::None };
    }
    let path = dir.join(format!("{guid}.{}", preview::EXTENSION));
    let entry = (|| {
        let meta = std::fs::metadata(&path).ok()?;
        Some(preview::Entry { header: preview::read_header(&path).ok()?, modified: meta.modified().ok()?, size: meta.len(), path: path.clone() })
    })();
    match (entry, fresh) {
        (Some(e), true) => Source::Built(e),
        (Some(e), false) => Source::Stale(e),
        (None, _) => Source::None,
    }
}

/// What [`build_scene`] answers when [`Limits::cancel`] stopped it: nothing was kept, it is to be built again.
pub const STOPPED: &str = "stopped";

/// Keeps `why` as the reason `scene` has no preview, until the scene or the project's manifests change (as a failed
/// build is kept): the desktop's worker, for a build that panicked, so that it is not tried again in a loop.
pub fn keep_failure(project: &Path, scene: &SceneInfo, why: &str) {
    let dir = cache_dir(project);
    let _ = std::fs::create_dir_all(&dir);
    let mut files: Vec<PathBuf> = vec![scene.path.clone(), prefab::meta_path(&scene.path)];
    for manifest in ["Packages/manifest.json", "Packages/vpm-manifest.json", "ProjectSettings/ProjectSettings.asset"] {
        files.push(project.join(manifest));
    }
    let record = Record {
        version: BUILDER_VERSION,
        scene: scene.rel.clone(),
        built_utc: time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        built_ms: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64),
        files: files.iter().filter_map(|p| stamp(p).map(|(s, t)| (p.to_string_lossy().into_owned(), s, t))).collect(),
        error: Some(why.to_string()),
        retry: false,
        report: build::Report::default(),
    };
    let _ = write_atomic(&dir.join(format!("{}.json", scene.guid)), &serde_json::to_vec_pretty(&record).unwrap_or_default());
}

/// Builds the preview of `scene` and keeps it in the cache (a failure too, with its reason). The entry of the file,
/// or why there is none.
pub fn build_scene(project: &Path, scene: &SceneInfo, limits: &Limits) -> Result<preview::Entry, String> {
    let dir = cache_dir(project);
    let _ = std::fs::create_dir_all(&dir);
    let start = Instant::now();
    // The scene's own times as they were before it was read: a save in Unity during the build (the preview is of the
    // file before it) then shows as a change, and the scene is built again.
    let before: Vec<(PathBuf, Option<(u64, i64)>)> = [scene.path.clone(), prefab::meta_path(&scene.path)].into_iter().map(|p| (p.clone(), stamp(&p))).collect();
    let result = (|| -> anyhow::Result<build::Built> {
        let assets = assets::index(project, Some(&dir.join("guids.txt")), limits)?;
        let file = build::SceneFile { path: scene.path.clone(), guid: scene.guid.clone(), name: scene.name.clone() };
        build::build(project, &file, &assets, limits)
    })();
    let mut files: Vec<PathBuf> = vec![scene.path.clone(), prefab::meta_path(&scene.path)];
    for manifest in ["Packages/manifest.json", "Packages/vpm-manifest.json", "ProjectSettings/ProjectSettings.asset"] {
        files.push(project.join(manifest));
    }
    let preview_path = dir.join(format!("{}.{}", scene.guid, preview::EXTENSION));
    let (error, report) = match &result {
        Ok(b) => {
            files.extend(b.files.iter().cloned());
            (None, b.report.clone())
        }
        Err(e) => (Some(short(&format!("{e:#}"))), build::Report::default()),
    };
    // Stopped (the app closing, a build wanted first) or out of time, whatever the message says (a model's reader
    // stopped by the deadline reports that it could not read the file).
    let stopped = error.is_some() && limits.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed));
    let retry = error.is_some() && (Instant::now() > limits.deadline || error.as_deref().is_some_and(|e| e.starts_with("it took too long")));
    files.sort();
    files.dedup();
    let record = Record {
        version: BUILDER_VERSION,
        scene: scene.rel.clone(),
        built_utc: time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        built_ms: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64),
        files: files
            .iter()
            .filter_map(|p| match before.iter().find(|(b, _)| b == p) {
                Some((_, kept)) => kept.map(|(s, t)| (p.to_string_lossy().into_owned(), s, t)),
                None => stamp(p).map(|(s, t)| (p.to_string_lossy().into_owned(), s, t)),
            })
            .collect(),
        error: error.clone(),
        retry,
        report: build::Report { ms: start.elapsed().as_secs_f64() * 1000.0, ..report },
    };
    match result {
        Ok(b) => {
            write_atomic(&preview_path, &b.bytes).map_err(|e| format!("could not keep it: {e}"))?;
            let _ = write_atomic(&dir.join(format!("{}.json", scene.guid)), &serde_json::to_vec_pretty(&record).unwrap_or_default());
            let meta = std::fs::metadata(&preview_path).map_err(|e| e.to_string())?;
            trim(&cache_root());
            Ok(preview::Entry { path: preview_path, modified: meta.modified().unwrap_or_else(|_| SystemTime::now()), size: meta.len(), header: b.header })
        }
        Err(_) => {
            // A cancelled build is not a result to keep, and says so ([`STOPPED`]) whatever stopped where.
            if stopped {
                return Err(STOPPED.into());
            }
            let _ = write_atomic(&dir.join(format!("{}.json", scene.guid)), &serde_json::to_vec_pretty(&record).unwrap_or_default());
            Err(error.unwrap_or_default())
        }
    }
}

/// A reason short enough for a chip (its innermost clause, the context before it dropped), at most 160 characters.
fn short(e: &str) -> String {
    let last = e.rsplit(": ").next().unwrap_or(e);
    let s = if last.len() < 12 { e } else { last };
    s.chars().take(160).collect()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    orbiters_core::write_atomic(path, bytes)
}

/// The cache stays under [`CACHE_BYTES`]: the previews used least recently go first.
const CACHE_BYTES: u64 = 1536 << 20;

/// Notes that the preview built for `guid` was just shown (its record's time; [`trim`] keeps the ones used last).
/// The preview's own time is left alone: the app reloads a preview whose time changed.
pub fn touch(project: &Path, guid: &str) {
    let record = cache_dir(project).join(format!("{guid}.json"));
    if let Ok(f) = std::fs::File::options().write(true).open(&record) {
        let _ = f.set_modified(SystemTime::now());
    }
}

/// Takes the cache under [`CACHE_BYTES`], every file counted (the GUIDs read, the cards' stills): the previews whose
/// record was touched least recently ([`touch`], or when built) go first, with their record; a project folder left
/// without a preview goes whole. Windows does not keep files' last access times on most disks (and never on large
/// ones), so they are not what tells the ones in use.
fn trim(root: &Path) {
    trim_to(root, CACHE_BYTES)
}

fn trim_to(root: &Path, limit: u64) {
    let mut total = 0u64;
    let mut previews: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for e in walkdir::WalkDir::new(root).max_depth(2).into_iter().flatten().filter(|e| e.file_type().is_file()) {
        let Ok(m) = e.metadata() else { continue };
        total += m.len();
        if e.path().extension().is_some_and(|x| x == preview::EXTENSION) {
            let record = e.path().with_extension("json");
            let used = std::fs::metadata(&record).and_then(|r| r.modified()).or_else(|_| m.modified()).unwrap_or(UNIX_EPOCH);
            let size = m.len() + std::fs::metadata(&record).map_or(0, |r| r.len());
            previews.push((used, size, e.into_path()));
        }
    }
    if total <= limit {
        return;
    }
    previews.sort_by_key(|f| f.0);
    for (_, size, path) in previews {
        if total <= limit {
            break;
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("json"));
        total = total.saturating_sub(size);
        if let Some(dir) = path.parent() {
            let left = std::fs::read_dir(dir).map(|d| d.flatten().any(|e| e.path().extension().is_some_and(|x| x == preview::EXTENSION))).unwrap_or(true);
            if !left {
                let rest: u64 = walkdir::WalkDir::new(dir).into_iter().flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum();
                if std::fs::remove_dir_all(dir).is_ok() {
                    total = total.saturating_sub(rest);
                }
            }
        }
    }
}

/// Damaged copies of `sample` for the readers' tests: bytes changed, runs repeated (counts and depths grow), cut short
/// and doubled, from a fixed seed (the same every run).
#[cfg(test)]
pub(crate) fn damaged(sample: &[u8], rounds: usize) -> Vec<Vec<u8>> {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut out = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let mut b = sample.to_vec();
        for _ in 0..1 + next() % 6 {
            if b.is_empty() {
                break;
            }
            let at = (next() as usize) % b.len();
            match next() % 6 {
                0 => b[at] = next() as u8,
                1 => b[at] = [b'9', b'-', b'[', b'{', 0xff, 0x00, b'\n', b' '][(next() % 8) as usize],
                2 => {
                    let end = (at + 1 + (next() as usize) % 16).min(b.len());
                    let run = b[at..end].to_vec();
                    for _ in 0..1 + next() % 40 {
                        b.splice(at..at, run.iter().copied());
                    }
                }
                3 => b.truncate(at),
                4 => {
                    let end = (at + 1 + (next() as usize) % 8).min(b.len());
                    b.drain(at..end);
                }
                _ => {
                    let copy = b.clone();
                    b.extend_from_slice(&copy);
                }
            }
        }
        out.push(b);
    }
    out
}

/// The GUID a file's `.meta` gives it.
pub(crate) fn assets_guid_of_file(path: &Path) -> Option<String> {
    std::fs::read_to_string(prefab::meta_path(path)).ok().and_then(|m| yaml::meta_guid(&m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, text: &str) -> PathBuf {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn scenes_come_last_open_first_then_the_build_settings_then_newest() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        for (rel, guid) in [("Assets/A.unity", "a"), ("Assets/Sub/B.unity", "b"), ("Assets/C.unity", "c"), ("Assets/Skip~/D.unity", "d")] {
            write(p, rel, "%YAML 1.1\n");
            write(p, &format!("{rel}.meta"), &format!("fileFormatVersion: 2\nguid: {}\n", guid.repeat(32)));
            std::thread::sleep(Duration::from_millis(20));
        }
        write(
            p,
            "ProjectSettings/EditorBuildSettings.asset",
            "EditorBuildSettings:\n  m_Scenes:\n  - enabled: 1\n    path: Assets/A.unity\n    guid: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
        );
        write(p, "Library/LastSceneManagerSetup.txt", "sceneSetups:\n- path: Assets/Sub/B.unity\n  isLoaded: 1\n  isActive: 1\n  isSubScene: 0\n");
        let s = scenes(p);
        let names: Vec<&str> = s.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["B", "A", "C"]);
        assert!(s[0].last_open && s[1].in_build && !s[2].in_build);
        assert_eq!(s[0].rel, "Assets/Sub/B.unity");
        std::fs::remove_file(p.join("Library/LastSceneManagerSetup.txt")).unwrap();
        assert_eq!(scenes(p)[0].name, "A", "no record of the last scene: the build settings' first");
    }

    #[test]
    fn a_toolkit_preview_is_the_scenes_until_the_scene_is_saved_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let guid = "1".repeat(32);
        write(p, "Assets/Main.unity", "%YAML 1.1\n");
        write(p, "Assets/Main.unity.meta", &format!("fileFormatVersion: 2\nguid: {guid}\n"));
        let header =
            preview::Header { scene: preview::Scene { name: "Main".into(), path: "Assets/Main.unity".into(), guid: guid.clone() }, ..Default::default() };
        let bytes = preview::Writer::default().finish(&header);
        std::thread::sleep(Duration::from_millis(30));
        write(p, &format!("{}/{guid}.preview", preview::FOLDER), "");
        std::fs::write(p.join(preview::FOLDER).join(format!("{guid}.preview")), &bytes).unwrap();
        let l = list(p);
        assert_eq!(l.len(), 1);
        assert!(matches!(l[0].source, Source::Toolkit(_)), "{:?}", l[0].source);
        // Saved in Unity without Toolkit three seconds later: the scene changed, Toolkit's preview is not it any more.
        let later = SystemTime::now() + Duration::from_secs(3);
        std::fs::File::options().write(true).open(p.join("Assets/Main.unity")).unwrap().set_modified(later).unwrap();
        let l = list(p);
        assert!(matches!(l[0].source, Source::None | Source::Failed(_) | Source::Built(_) | Source::Stale(_)), "{:?}", l[0].source);
    }

    #[test]
    fn a_built_preview_is_kept_until_a_file_it_read_changes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let _assets = prefab::tests::project(p);
        write(p, "Assets/Scene.unity.meta", &format!("fileFormatVersion: 2\nguid: {}\n", "9".repeat(32)));
        for (rel, guid) in [
            ("Assets/Rig.fbx", prefab::tests::MODEL_GUID),
            ("Assets/Avatar.prefab", prefab::tests::PREFAB_GUID),
            ("Assets/Avatar Variant.prefab", prefab::tests::VARIANT_GUID),
            ("Assets/Red.mat", prefab::tests::RED_GUID),
        ] {
            write(p, &format!("{rel}.meta"), &format!("fileFormatVersion: 2\nguid: {guid}\n"));
        }
        let scene = scenes(p).into_iter().next().unwrap();
        // Its own cache folder for this test: the real one is the person's.
        let entry = build_scene(p, &scene, &Limits::default()).unwrap();
        assert_eq!(entry.header.kind, preview::Kind::Avatars);
        assert!(matches!(built(&cache_dir(p), &scene.guid), Source::Built(_)));
        std::thread::sleep(Duration::from_millis(20));
        let mat = p.join("Assets/Red.mat");
        let text = std::fs::read_to_string(&mat).unwrap().replace("g: 0", "g: 1");
        std::fs::write(&mat, text).unwrap();
        assert!(matches!(built(&cache_dir(p), &scene.guid), Source::Stale(_)), "the material changed");
        let _ = std::fs::remove_dir_all(cache_dir(p));
    }

    #[test]
    fn the_cache_keeps_the_previews_used_last_and_drops_emptied_projects_whole() {
        let root = tempfile::tempdir().unwrap();
        let kb = vec![0u8; 1 << 10];
        let old = SystemTime::now() - Duration::from_secs(3600);
        // Three projects of 600 KB each, the cache allowed 1.5 MB.
        for (i, name) in ["a", "b", "c"].iter().enumerate() {
            let dir = root.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{name}.{}", preview::EXTENSION)), vec![0u8; 600 << 10]).unwrap();
            std::fs::write(dir.join(format!("{name}.json")), b"{}").unwrap();
            std::fs::write(dir.join("guids.txt"), &kb).unwrap();
            // b was used last, then c, then a (its record touched an hour ago, minus its place).
            let used = old + Duration::from_secs([0, 120, 60][i]);
            std::fs::File::options().write(true).open(dir.join(format!("{name}.json"))).unwrap().set_modified(used).unwrap();
        }
        trim_to(root.path(), 1536 << 10);
        assert!(!root.path().join("a").exists(), "the least recently used project went whole");
        assert!(root.path().join("b/b.preview").exists() && root.path().join("c/c.preview").exists());
    }

    #[test]
    fn a_build_out_of_time_is_tried_again_and_a_stopped_one_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        write(p, "Assets/Main.unity", "%YAML 1.1\n--- !u!1 &1\nGameObject:\n  m_Name: A\n");
        write(p, "Assets/Main.unity.meta", &format!("fileFormatVersion: 2\nguid: {}\n", "6".repeat(32)));
        let scene = scenes(p).into_iter().next().unwrap();
        let why = build_scene(p, &scene, &Limits::new(Duration::ZERO)).unwrap_err();
        assert!(why.contains("too long"), "{why}");
        let record: Record = serde_json::from_slice(&std::fs::read(cache_dir(p).join(format!("{}.json", scene.guid))).unwrap()).unwrap();
        assert!(record.retry && record.fresh(), "kept a while, so that it is not built again at once");
        let mut later = record.clone();
        later.built_ms -= RETRY_AFTER.as_millis() as u64 + 1000;
        assert!(!later.fresh(), "then tried again");
        let failed = Record { retry: false, ..later };
        assert!(failed.fresh(), "a real failure stays until a file changes");
        // Stopped: nothing kept, and it says so.
        let _ = std::fs::remove_dir_all(cache_dir(p));
        let cancel = Arc::new(AtomicBool::new(true));
        let stopped = Limits { cancel: Some(cancel), ..Limits::default() };
        assert_eq!(build_scene(p, &scene, &stopped).unwrap_err(), STOPPED);
        assert!(!cache_dir(p).join(format!("{}.json", scene.guid)).exists());
        let _ = std::fs::remove_dir_all(cache_dir(p));
    }

    #[test]
    fn a_failure_kept_for_a_build_that_panicked_lasts_until_the_scene_is_saved_again() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        write(p, "Assets/Main.unity", "%YAML 1.1\n");
        write(p, "Assets/Main.unity.meta", &format!("fileFormatVersion: 2\nguid: {}\n", "7".repeat(32)));
        let scene = scenes(p).remove(0);
        keep_failure(p, &scene, "an error while reading it");
        assert!(matches!(&list(p)[0].source, Source::Failed(why) if why == "an error while reading it"), "not built again in a loop");
        write(p, "Assets/Main.unity", "%YAML 1.1\n--- !u!1 &1\nGameObject:\n  m_Name: Saved again\n");
        assert!(matches!(list(p)[0].source, Source::None), "saved again: built again");
        let _ = std::fs::remove_dir_all(cache_dir(p));
    }

    #[test]
    fn a_broken_project_says_why_within_the_time_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        // A scene in binary, one with garbage, a model that is not one, a prefab that loops.
        write(p, "Assets/Binary.unity", "\u{0}\u{0}\u{0}binary");
        write(p, "Assets/Binary.unity.meta", &format!("fileFormatVersion: 2\nguid: {}\n", "2".repeat(32)));
        let looped = "3".repeat(32);
        let model = "4".repeat(32);
        write(
            p,
            "Assets/Odd.unity",
            &format!(
                "%YAML 1.1\n--- !u!1001 &1\nPrefabInstance:\n  m_SourcePrefab: {{fileID: 1, guid: {looped}, type: 3}}\n--- !u!1001 &2\nPrefabInstance:\n  m_SourcePrefab: {{fileID: 1, guid: {model}, type: 3}}\n--- !u!1 &3\nGameObject: {{{{{{\n  - - -\n"
            ),
        );
        write(p, "Assets/Odd.unity.meta", &format!("fileFormatVersion: 2\nguid: {}\n", "5".repeat(32)));
        write(p, "Assets/Loop.prefab", &format!("%YAML 1.1\n--- !u!1001 &1\nPrefabInstance:\n  m_SourcePrefab: {{fileID: 1, guid: {looped}, type: 3}}\n"));
        write(p, "Assets/Loop.prefab.meta", &format!("fileFormatVersion: 2\nguid: {looped}\n"));
        write(p, "Assets/Fake.fbx", "Kaydara FBX Binary  \u{0}\u{1a}\u{0}nope");
        write(p, "Assets/Fake.fbx.meta", &format!("fileFormatVersion: 2\nguid: {model}\n"));
        let start = Instant::now();
        let limits = Limits::new(Duration::from_secs(5));
        for scene in scenes(p) {
            let r = build_scene(p, &scene, &limits);
            let why = r.unwrap_err();
            match scene.name.as_str() {
                "Binary" => assert!(why.contains("binary"), "says why: {why}"),
                // Its prefab loops and its model is not one: nothing to draw, and the first reason why.
                _ => assert!(why.contains("contains itself") || why.contains("could not be read"), "{why}"),
            }
        }
        assert!(start.elapsed() < Duration::from_secs(6));
        // A deadline already past stops a build with its reason.
        let gone = Limits::new(Duration::ZERO);
        let scene = scenes(p).into_iter().find(|s| s.name == "Odd").unwrap();
        assert!(build_scene(p, &scene, &gone).unwrap_err().contains("too long"));
        let _ = std::fs::remove_dir_all(cache_dir(p));
    }
}
