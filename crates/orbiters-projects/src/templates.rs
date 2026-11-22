use orbiters_i18n::{mark, tr};
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    pub path: PathBuf,
    pub unity: String,
    pub builtin: bool,
    /// Its package name (com.vrchat.template.avatar…).
    #[serde(default)]
    pub id: String,
    /// The cover Creator Companion shows for it, when VCC is installed.
    #[serde(default)]
    pub banner: Option<PathBuf>,
}
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Progress {
    pub copied: usize,
    pub total: usize,
    pub file: String,
    pub phase: String,
}
pub type SharedProgress = Arc<Mutex<Progress>>;

pub fn list(root: &Path) -> Vec<Template> {
    let mut out = vec![];
    for (dir, builtin) in [("VRCTemplates", true), ("Templates", false)] { // i18n: not shown (folder names)
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else { continue };
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_dir() || std::fs::symlink_metadata(&p).ok().is_some_and(|m| crate::discovery::is_link(&m)) {
                continue;
            }
            let Ok(bytes) = std::fs::read(p.join("package.json")) else { continue };
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else { continue };
            let name = v["displayName"].as_str().or_else(|| v["name"].as_str()).unwrap_or(tr("Unity template")).to_string();
            let unity = std::fs::read_to_string(p.join("ProjectSettings/ProjectVersion.txt"))
                .ok()
                .and_then(|s| crate::discovery::parse_version(&s))
                .unwrap_or_default();
            let id = v["name"].as_str().unwrap_or_default().to_string();
            out.push(Template { name, path: p, unity, builtin, id, banner: None });
        }
    }
    if !out.iter().any(|t| t.builtin) {
        out.extend(builtins(&crate::data_dir().join("unity-templates/alcom-v1.9.3")).unwrap_or_default());
    }
    if let Some(app) = crate::vcc_app_dir() {
        for t in &mut out {
            t.banner = banner(&app, &t.id);
        }
    }
    order(&mut out);
    out
}

/// VRChat's own templates first, by id (the ones made here have translated names, so the order and the template the window opens on must not
/// follow the language), then the person's by name.
fn order(templates: &mut [Template]) {
    templates.sort_by(|a, b| (!a.builtin, if a.builtin { &a.id } else { &a.name }).cmp(&(!b.builtin, if b.builtin { &b.id } else { &b.name })));
}

/// The cover Creator Companion (installed in `app`) shows on a template's card: VRChat's own templates have theirs,
/// any other one VCC's "Custom" cover (as VCC does).
pub fn banner(app: &Path, id: &str) -> Option<PathBuf> {
    let file = match id {
        "com.vrchat.template.avatar" => "template-avatar-2022.png",
        "com.vrchat.template.world" => "template-world-2022.png",
        "com.vrchat.template.avatar.2019" => "template-avatar.png",
        "com.vrchat.template.world.2019" => "template-world-udon.png",
        _ => "template-custom.png",
    };
    Some(app.join("WebApp/Dist/template-covers").join(file)).filter(|p| p.is_file())
}

/// A fresh install has no VCC template directory. Bundle ALCOM's MIT-licensed modern template assets so these are
/// available offline; VPM still downloads and resolves the SDK through its normal official repository.
pub fn builtins(cache: &Path) -> anyhow::Result<Vec<Template>> {
    static MATERIALIZE: Mutex<()> = Mutex::new(());
    let _one = MATERIALIZE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut out = vec![];
    for (id, files) in crate::builtin_templates::TEMPLATES {
        let path = cache.join(id);
        std::fs::create_dir_all(&path)?;
        for (relative, content) in *files {
            let p = path.join(relative);
            if !p.is_file() {
                std::fs::create_dir_all(p.parent().unwrap())?;
                std::fs::write(&p, content)?;
            }
        }
        let avatars = id.starts_with("avatars");
        out.push(Template {
            name: if avatars { tr("VRChat Avatar Project") } else { tr("VRChat World Project") }.into(),
            path,
            unity: "2022.3.22f1".into(),
            builtin: true,
            id: if avatars { "com.vrchat.template.avatar" } else { "com.vrchat.template.world" }.into(),
            banner: None,
        });
    }
    // Found by its id: its name is translated, so it is the one thing that cannot be compared.
    let avatar = out.iter().find(|t| t.id == "com.vrchat.template.avatar").with_context(|| tr("Bundled avatar template is missing"))?;
    let path = cache.join("base-2022.3.22f1");
    if !path.join("ProjectSettings/ProjectVersion.txt").is_file() {
        copy(&avatar.path, &path, &AtomicBool::new(false), &Arc::new(Mutex::default()))?;
    }
    // Written when it is not this already (PROJECTS-24 a): every look at a project's packages lists the templates, and each
    // wrote, flushed to disk and replaced the same file. The avatar template's manifest copied in first is replaced.
    let manifest = path.join("Packages/vpm-manifest.json");
    let base = serde_json::json!({"dependencies":{"com.vrchat.base":{"version":"3.x"}}});
    if std::fs::read(&manifest).ok().and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok()).as_ref() != Some(&base) {
        crate::write_json(&manifest, &base)?;
    }
    out.push(Template {
        name: String::from(tr("VRChat Base Project")),
        path,
        unity: "2022.3.22f1".into(),
        builtin: true,
        id: "com.vrchat.template.base".into(),
        banner: None,
    });
    Ok(out)
}
pub fn validate_name(name: &str) -> anyhow::Result<()> {
    let stem = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
    if name.trim() != name
        || name.is_empty()
        || name.ends_with(['.', ' '])
        || name.chars().any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || matches!(name, "." | "..")
        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|s| stem.strip_prefix(s).is_some_and(|v| v.len() == 1 && v.chars().all(|c| c.is_ascii_digit())))
    {
        // Projects travel: the rule is Windows' on every computer, though only there is it the system's own.
        if cfg!(windows) {
            bail!("{}", tr("Choose a valid Windows folder name."));
        }
        bail!("{}", tr("Choose a folder name that also works on Windows: none of < > : \" / \\ | ? *, no space or dot at its end, not CON or NUL."));
    }
    Ok(())
}
/// What a copy of a project takes along besides its own files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keep {
    /// Library, which Unity otherwise makes again from the project's files when the copy is first opened (long for a
    /// large project).
    pub library: bool,
}

/// Left out of a copy: what Unity or the IDE make again at the project's top (Library unless `keep` asks for it,
/// Temp, Logs, obj, Visual Studio's .vs, the .csproj and .sln files), this computer's editor settings (UserSettings),
/// and Git's history and Node's packages wherever they are. A folder of the same name deeper in (an asset folder
/// called Library or Temp) is copied.
/// What a copy leaves out: [`excluded`] for a project (`project`), only Git's and Node's folders for a package folder
/// (PROJECTS-37: a package's own Library, Logs or Temp folder is part of it), and never a copy's marker at the top.
fn left_out(name: &str, depth: usize, dir: bool, keep: Keep, project: bool) -> bool {
    if depth == 1 && !dir && name == INCOMPLETE {
        return true;
    }
    if project {
        excluded(name, depth, dir, keep)
    } else {
        matches!(name.to_ascii_lowercase().as_str(), ".git" | "node_modules")
    }
}

fn excluded(name: &str, depth: usize, dir: bool, keep: Keep) -> bool {
    let n = name.to_ascii_lowercase();
    if matches!(n.as_str(), ".git" | "node_modules") {
        return true;
    }
    if depth != 1 {
        return false;
    }
    match n.as_str() {
        "library" => dir && !keep.library,
        "temp" | "logs" | "obj" | "usersettings" | ".vs" => dir,
        _ => !dir && (n.ends_with(".csproj") || n.ends_with(".sln")),
    }
}
/// The file a copy keeps at the top of its folder until every file is in (PROJECTS-4): a copy stopped, failed or cut by
/// a kill is then never taken for a whole project ([`crate::discovery::is_project`]). It holds the copy's own identity
/// ([`Marker`]); one copied into or restored in another folder names another destination and hides nothing.
pub const INCOMPLETE: &str = ".orbiters-incomplete-copy";

/// What [`INCOMPLETE`] holds: an id made by this copy (and kept in its memory), its destination, when it began.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Marker {
    pub copy: String,
    pub destination: PathBuf,
    pub started: u64,
    /// The identity of the folder the copy made: a folder restored or moved to that place later is not it.
    #[serde(default)]
    pub folder: Option<crate::owned::FileId>,
}
impl Marker {
    /// The marker at the top of `folder`, if it is one.
    pub fn read(folder: &Path) -> Option<Marker> {
        serde_json::from_slice(&std::fs::read(folder.join(INCOMPLETE)).ok()?).ok()
    }
}

/// Copy into a newly reserved folder only. Links and junctions in the source are followed: what they lead to is copied
/// as real files, so that the copy does not depend on them.
pub fn copy(source: &Path, destination: &Path, cancel: &AtomicBool, progress: &SharedProgress) -> anyhow::Result<()> {
    copy_keeping(source, destination, Keep::default(), cancel, progress)
}
/// [`copy`], taking along what `keep` asks for.
pub fn copy_keeping(source: &Path, destination: &Path, keep: Keep, cancel: &AtomicBool, progress: &SharedProgress) -> anyhow::Result<()> {
    copy_keeping_then(source, destination, keep, true, cancel, progress, |_| {})
}

/// What a running copy made, so that a Stop removes exactly that (PROJECTS-4, Enzo 2026-11-19: "Yes").
struct Made {
    marker: Marker,
    /// The folder it created, held from the moment it was made: on Windows it cannot be renamed, moved or replaced
    /// while the copy runs, nor any folder above it, so every name below it stays this very folder's.
    root: crate::owned::Pinned,
    /// The marker file, by identity.
    marker_id: crate::owned::FileId,
    /// Each folder it created, held the same way, in the order made (parents first), relative to the root.
    folders: Vec<(PathBuf, crate::owned::Pinned)>,
    /// Each file it created and its identity.
    files: Vec<(PathBuf, crate::owned::FileId)>,
}

/// Removes what `made` created at `destination`, each entry through a handle whose identity is the one it made (a
/// folder through the handle held since it was made, a file through one opened once and checked); anything else
/// found there stays. The paths that stayed (none: the folder is gone).
fn discard_partial(destination: &Path, made: Made) -> std::io::Result<Vec<PathBuf>> {
    use crate::owned::{delete_file_if, is_same};
    let Made { marker, root, marker_id, mut folders, files } = made;
    if !is_same(destination, root.id) || Marker::read(destination).as_ref() != Some(&marker) {
        return Err(std::io::Error::other(tr("it is not the folder this copy made")));
    }
    let mut left = vec![];
    for (rel, id) in files.iter().rev() {
        let path = destination.join(rel);
        if !delete_file_if(&path, *id).unwrap_or(false) && std::fs::symlink_metadata(&path).is_ok() {
            left.push(path);
        }
    }
    // Deepest first; a folder that holds what is not the copy's is not empty and stays.
    while let Some((rel, folder)) = folders.pop() {
        if folder.delete().is_err() {
            left.push(destination.join(rel));
        }
    }
    delete_file_if(&destination.join(INCOMPLETE), marker_id)?;
    if root.delete().is_err() || destination.exists() {
        // What is still there is not the copy's: named, never removed.
        let mut extra: Vec<PathBuf> = walkdir::WalkDir::new(destination).min_depth(1).follow_links(false).into_iter().flatten().map(|e| e.into_path()).collect();
        extra.retain(|p| !left.contains(p));
        left.extend(extra);
        left.sort();
        left.dedup();
        if left.is_empty() {
            left.push(destination.to_path_buf());
        }
    }
    Ok(left)
}

/// A Stop during the copy into `destination`: what it made goes, or says what stayed and why.
fn stopped(destination: &Path, made: Made) -> anyhow::Error {
    match discard_partial(destination, made) {
        Ok(left) if left.is_empty() => anyhow::anyhow!("{}", tr("Copy stopped. The incomplete copy was removed.")),
        Ok(left) => anyhow::anyhow!(
            "{}",
            tr!(
                "Copy stopped. What it copied was removed; {path} keeps what was not the copy's: {files}",
                path = destination.display(),
                files = left.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
            )
        ),
        Err(e) => anyhow::anyhow!("{}", tr!("Copy stopped. The incomplete copy at {path} could not be removed ({why}); it is not listed as a project.", path = destination.display(), why = e)),
    }
}

/// [`copy_keeping`], running `made` once its folder exists (a test stops the copy, or swaps the folder, there).
/// `project`: the project rules of what is left out (a package's folder is copied whole but for Git's and Node's).
pub(crate) fn copy_keeping_then(
    source: &Path,
    destination: &Path,
    keep: Keep,
    project: bool,
    cancel: &AtomicBool,
    progress: &SharedProgress,
    on_made: impl FnOnce(&Path),
) -> anyhow::Result<()> {
    if !source.is_dir() {
        bail!("{}", tr("The source folder no longer exists."));
    }
    let source = std::fs::canonicalize(source)?;
    let parent = destination.parent().with_context(|| tr("Choose a destination folder"))?;
    std::fs::create_dir_all(parent)?;
    let parent = std::fs::canonicalize(parent)?;
    if parent.starts_with(&source) {
        bail!("{}", tr("Create the copy outside the source project."));
    }
    anyhow::ensure!(!destination.exists(), "{}", tr("The destination already exists; choose a new project name."));
    let mut files = vec![];
    // Folders too, empty ones included (PROJECTS-41): an empty asset folder has its .meta beside it.
    let mut folders = vec![];
    for entry in walkdir::WalkDir::new(&source)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| !left_out(&e.file_name().to_string_lossy(), e.depth(), e.file_type().is_dir(), keep, project))
    {
        if cancel.load(Ordering::Relaxed) {
            bail!("{}", tr!("Copy stopped before anything was copied to {path}.", path = destination.display()));
        }
        let e = entry?;
        if e.file_type().is_file() {
            files.push(e.path().to_path_buf());
        } else if e.file_type().is_dir() && e.depth() > 0 {
            folders.push(e.path().to_path_buf());
        }
    }
    std::fs::create_dir(destination).with_context(|| tr("The destination already exists or cannot be created; choose a new project name."))?;
    // Held at once: its identity is the held folder's, and nothing can take its place until the copy is done.
    let root = crate::owned::Pinned::open(destination)?;
    let marker = Marker { copy: uuid::Uuid::new_v4().simple().to_string(), destination: destination.to_path_buf(), started: crate::discovery::now(), folder: Some(root.id) };
    let marker_id = {
        let mut file = std::fs::File::options().write(true).create_new(true).open(destination.join(INCOMPLETE))?;
        file.write_all(&serde_json::to_vec(&marker)?)?;
        crate::owned::id_of_file(&file)?
    };
    let mut made = Made { marker, root, marker_id, folders: vec![], files: vec![] };
    for folder in &folders {
        let rel = folder.strip_prefix(&source)?.to_path_buf();
        let at = destination.join(&rel);
        // Made here, and held: one already there is not the copy's (and is never removed by it).
        match std::fs::create_dir(&at) {
            Ok(()) => made.folders.push((rel, crate::owned::Pinned::open(&at)?)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    on_made(destination);
    progress.lock().unwrap().total = files.len();
    // One buffer for every file (PROJECTS-6): most are a few KB.
    let mut buffer = vec![0u8; 1024 * 1024];
    for (i, p) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(stopped(destination, made));
        }
        // Elsewhere than Windows nothing holds the folder: it must still be the one made.
        #[cfg(not(windows))]
        anyhow::ensure!(crate::owned::is_same(destination, made.root.id), "{}", tr("it is not the folder this copy made"));
        let rel = p.strip_prefix(&source)?;
        let dst = destination.join(rel);
        let mut input = std::fs::File::open(p)?;
        // Never over a file that is there already: it is not this copy's.
        let mut output = std::fs::File::options()
            .write(true)
            .create_new(true)
            .open(&dst)
            .with_context(|| tr!("Could not copy {path}", path = rel.display()))?;
        made.files.push((rel.to_path_buf(), crate::owned::id_of_file(&output)?));
        loop {
            if cancel.load(Ordering::Relaxed) {
                // The file being written is let go first: Windows removes no file still open.
                drop(output);
                return Err(stopped(destination, made));
            }
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            output.write_all(&buffer[..n]).with_context(|| tr!("Could not copy {path}", path = rel.display()))?;
        }
        // The source's modification time: Unity compares it with what it recorded, and a copy with Library then has
        // nothing to look at again. Where a disk cannot keep it, the copy is still whole.
        if let Ok(modified) = input.metadata().and_then(|m| m.modified()) {
            let _ = output.set_modified(modified);
        }
        // A new file is not executable here, and a copy must be usable: a package's tools, a plugin's programs.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let executable = input.metadata()?.permissions().mode() & 0o111;
            if executable != 0 {
                let mut permissions = output.metadata()?.permissions();
                permissions.set_mode(permissions.mode() | executable);
                output.set_permissions(permissions)?;
            }
        }
        *progress.lock().unwrap() = Progress { copied: i + 1, total: files.len(), file: rel.display().to_string(), phase: String::from(mark("Copying project")) };
    }
    // Its own marker only, through the held folder.
    crate::owned::delete_file_if(&destination.join(INCOMPLETE), made.marker_id)?;
    drop(made);
    Ok(())
}
/// A new project in `parent`/`name` from the template (or, with `duplicate`, the project) in `source`, its packages
/// restored and registered.
#[allow(clippy::too_many_arguments)]
pub async fn create(
    root: &Path,
    source: &Path,
    parent: &Path,
    name: &str,
    unity: &str,
    duplicate: bool,
    cancel: &AtomicBool,
    progress: &SharedProgress,
) -> anyhow::Result<PathBuf> {
    make(root, source, parent, name, unity, duplicate.then_some(Keep::default()), cancel, progress).await
}

/// A copy of the project in `source` named `name` in `parent`, taking along what `keep` asks for: its packages
/// restored and registered in the list VCC and ALCOM share. Its scenes keep their blueprint IDs.
pub async fn duplicate(
    root: &Path,
    source: &Path,
    parent: &Path,
    name: &str,
    keep: Keep,
    cancel: &AtomicBool,
    progress: &SharedProgress,
) -> anyhow::Result<PathBuf> {
    let unity = std::fs::read_to_string(source.join("ProjectSettings/ProjectVersion.txt"))
        .ok()
        .and_then(|s| crate::discovery::parse_version(&s))
        .unwrap_or_default();
    make(root, source, parent, name, &unity, Some(keep), cancel, progress).await
}

#[allow(clippy::too_many_arguments)]
async fn make(
    root: &Path,
    source: &Path,
    parent: &Path,
    name: &str,
    unity: &str,
    copy: Option<Keep>,
    cancel: &AtomicBool,
    progress: &SharedProgress,
) -> anyhow::Result<PathBuf> {
    validate_name(name)?;
    if !parent.is_absolute() {
        bail!("{}", tr("Choose an absolute destination folder."));
    }
    let destination = parent.join(name);
    let required = duplication_bytes(source, copy.unwrap_or_default())?;
    let free = crate::size::free_space(parent).with_context(|| tr("Cannot check available disk space. Choose an accessible destination."))?;
    anyhow::ensure!(
        free >= required,
        "{}",
        tr!("Not enough disk space: free at least {bytes} more bytes before duplicating.", bytes = required.saturating_sub(free))
    );
    copy_keeping(source, &destination, copy.unwrap_or_default(), cancel, progress)?;
    #[cfg(test)]
    if STOP_AFTER_COPY.with(|s| s.get()) {
        cancel.store(true, Ordering::Relaxed);
    }
    let version = destination.join("ProjectSettings/ProjectVersion.txt");
    if !version.is_file() {
        if vrc_get_vpm::version::UnityVersion::parse(unity).is_none() {
            bail!("{}", tr!("Choose a Unity editor version. The copied folder is at {path}.", path = destination.display()));
        }
        std::fs::create_dir_all(version.parent().unwrap())
            .and_then(|()| std::fs::write(&version, format!("m_EditorVersion: {unity}\n")))
            .with_context(|| tr!("Choose a Unity editor version. The copied folder is at {path}.", path = destination.display()))?;
    }
    // Every file is in: a Stop pressed now no longer leaves a whole copy unlisted (AGENTS.md: a retained project is
    // visible); it is registered like any other.
    // Register the completed copy before any fallible restoration (AGENTS.md): from here on every failure leaves a
    // project the gallery lists, and says where it is.
    progress.lock().unwrap().phase = mark("Registering project").into();
    crate::vpm::register(root, &destination, true).await
        .with_context(|| tr!("The copy is at {path}, but registration failed", path = destination.display()))?;
    let listed = || tr!("The copy is listed at {path}, but its local packages could not be copied", path = destination.display());
    if copy.is_some() { copy_local_packages(source, &destination, cancel, progress).with_context(listed)?; }
    // A product GUID of its own; a template's product name always becomes the project's, a copy's when it was the
    // copied project's name.
    let path = destination.join("ProjectSettings/ProjectSettings.asset");
    if let Ok(s) = std::fs::read_to_string(&path) {
        let from = copy.map(|_| source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        std::fs::write(&path, retarget(&s, name, from.as_deref()))
            .with_context(|| tr!("The copy is listed at {path}, but its project settings could not be updated", path = destination.display()))?;
    }
    // Duplication preserves installed package contents and exact locked versions, including packages whose
    // repositories are no longer available. Resolution is needed only for genuinely missing packages/templates.
    let complete = installed_dependencies_complete(&destination)
        .with_context(|| tr!("The copy is listed at {path}, but missing packages could not be restored", path = destination.display()))?;
    if copy.is_none() || !complete {
        progress.lock().unwrap().phase = mark("Restoring missing packages").into();
        crate::vpm::change(root, &destination, crate::vpm::Change::Resolve).await
            .with_context(|| tr!("The copy is listed at {path}, but missing packages could not be restored", path = destination.display()))?;
    }
    Ok(destination)
}

#[cfg(test)]
thread_local! {
    /// A test's Stop pressed the moment the last file is in.
    static STOP_AFTER_COPY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Fresh copy estimate using the same exclusions and link traversal as the copier, including local packages.
pub fn duplication_bytes(source: &Path, keep: Keep) -> anyhow::Result<u64> {
    fn bytes(path: &Path, keep: Keep, project: bool) -> anyhow::Result<u64> {
        let mut total = 0u64;
        for e in walkdir::WalkDir::new(path).follow_links(true).into_iter()
            .filter_entry(|e| !left_out(&e.file_name().to_string_lossy(), e.depth(), e.file_type().is_dir(), keep, project)) {
            let e = e?;
            if e.file_type().is_file() { total = total.saturating_add(e.metadata()?.len()); }
        }
        Ok(total)
    }
    let mut total = bytes(source, keep, true)?;
    let manifest = source.join("Packages/manifest.json");
    if manifest.is_file() {
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(manifest)?)?;
        for (name, value) in manifest["dependencies"].as_object().into_iter().flatten() {
            if let Some(local) = value.as_str().and_then(|s| s.strip_prefix("file:")) {
                let at = source.join("Packages").join(local);
                let path = at.canonicalize().with_context(|| tr!("Required local package {name} is unavailable at {path}", name = name, path = at.display()))?;
                // As the copier does: a folder unless the project has one of that name already, an archive always.
                if path.is_file() {
                    total = total.saturating_add(std::fs::metadata(&path)?.len());
                } else if !source.join("Packages").join(name).exists() {
                    total = total.saturating_add(bytes(&path, Keep::default(), false)?);
                }
            }
        }
    }
    Ok(total)
}

/// Materialize external UPM folder dependencies so a moved duplicate does not depend on its source location.
pub(crate) fn copy_local_packages(source: &Path, destination: &Path, cancel: &AtomicBool, progress: &SharedProgress) -> anyhow::Result<()> {
    let path = destination.join("Packages/manifest.json");
    if !path.is_file() { return Ok(()); }
    let mut manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    let mut changed = false;
    if let Some(dependencies) = manifest["dependencies"].as_object_mut() {
        for (name, value) in dependencies {
            let Some(local) = value.as_str().and_then(|s| s.strip_prefix("file:")) else { continue };
            anyhow::ensure!(!name.contains(['/', '\\']) && name != "..", "{}", tr!("Invalid package name: {name}", name = name));
            let at = source.join("Packages").join(local);
            let origin = at.canonicalize().with_context(|| tr!("Required local package {name} is unavailable at {path}", name = name, path = at.display()))?;
            let target = destination.join("Packages").join(name);
            if origin.is_dir() {
                if !target.exists() { copy_keeping_then(&origin, &target, Keep::default(), false, cancel, progress, |_| {})?; }
                *value = serde_json::Value::String(format!("file:{name}"));
            } else {
                let filename = format!("{name}.tgz");
                let to = destination.join("Packages").join(&filename);
                // Never over an archive the project already had under that name (PROJECTS-37): the same bytes are
                // kept, other bytes stop the copy here, named.
                if to.exists() {
                    anyhow::ensure!(
                        std::fs::read(&to)? == std::fs::read(&origin)?,
                        "{}",
                        tr!("{file} in the copy's Packages folder is another archive than the one {name} needs ({path})", file = filename, name = name, path = origin.display())
                    );
                } else {
                    std::fs::copy(&origin, &to)?;
                }
                *value = serde_json::Value::String(format!("file:{filename}"));
            }
            changed = true;
        }
    }
    if changed { crate::write_json(&path, &manifest)?; }
    Ok(())
}

/// The Unity packages of `project` that only its Library holds (PROJECTS-22): those `Packages/packages-lock.json` takes
/// from Git or from a registry other than Unity's, and that are not in `Packages/` itself. A copy made without Library
/// has them only after Unity downloads them again from their source, so it is not "ready" while that is unsure.
pub fn packages_only_in_library(project: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(project.join("Packages/packages-lock.json")) else { return vec![] };
    let Ok(lock) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return vec![] };
    let mut names: Vec<String> = lock["dependencies"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, entry)| {
            let from_elsewhere = match entry["source"].as_str() {
                Some("git") => true,
                Some("registry") => entry["url"].as_str().is_some_and(|u| !u.trim_end_matches('/').eq_ignore_ascii_case("https://packages.unity.com")),
                _ => false,
            };
            from_elsewhere && !name.contains(['/', '\\']) && !project.join("Packages").join(name).is_dir()
        })
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();
    names
}

/// Check every locked dependency, including transitive dependencies, against the copied package metadata.
pub(crate) fn installed_dependencies_complete(project: &Path) -> anyhow::Result<bool> {
    let path = project.join("Packages/vpm-manifest.json");
    if !path.exists() { return Ok(true); }
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let locked = manifest["locked"].as_object();
    if let Some(dependencies) = manifest["dependencies"].as_object() {
        if dependencies.keys().any(|name| !locked.is_some_and(|l| l.contains_key(name))) { return Ok(false); }
    }
    for (name, package) in locked.into_iter().flatten() {
        anyhow::ensure!(!name.contains(['/', '\\']) && name != "..", "{}", tr!("Invalid package name: {name}", name = name));
        let installed = project.join("Packages").join(name).join("package.json");
        let Ok(bytes) = std::fs::read(installed) else { return Ok(false) };
        let installed: serde_json::Value = serde_json::from_slice(&bytes)?;
        if installed["name"].as_str() != Some(name) || installed["version"] != package["version"] { return Ok(false); }
    }
    Ok(true)
}

/// A project's `ProjectSettings.asset` (text) for a new project named `name`: a product GUID of its own, and its
/// product name (under which the editor keeps the project's PlayerPrefs) changed to `name` when it was `from` (the
/// copied project's name), or always without `from` (a template's). The other lines and every line's ending stay.
pub fn retarget(settings: &str, name: &str, from: Option<&str>) -> String {
    let mut out = String::with_capacity(settings.len() + 64);
    for line in settings.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let end = &line[body.len()..];
        let trimmed = body.trim_start();
        let indent = &body[..body.len() - trimmed.len()];
        if trimmed.starts_with("productGUID:") {
            out.push_str(&format!("{indent}productGUID: {}{end}", uuid::Uuid::new_v4().simple()));
        } else if trimmed.strip_prefix("productName:").is_some_and(|v| from.is_none_or(|f| unquote(v.trim()) == f)) {
            out.push_str(&format!("{indent}productName: {}{end}", serde_json::to_string(name).unwrap()));
        } else {
            out.push_str(line);
        }
    }
    out
}

/// A YAML scalar as Unity writes it: plain, 'single' or "double" quoted.
fn unquote(s: &str) -> String {
    if s.starts_with('"') {
        serde_json::from_str(s).unwrap_or_else(|_| s.trim_matches('"').into())
    } else if s.starts_with('\'') {
        s.trim_matches('\'').replace("''", "'")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_keep_their_order_in_every_language() {
        // "Projet de monde VRChat" sorts before "Projet d’avatar VRChat": by name the avatar template would no longer be first in French.
        for language in orbiters_i18n::Language::TRANSLATED.into_iter().chain([orbiters_i18n::Language::En]) {
            let ids = orbiters_i18n::in_language(language, || {
                let mut all = vec![
                    Template { name: tr("VRChat World Project").into(), id: "com.vrchat.template.world".into(), builtin: true, ..Default::default() },
                    Template { name: "Zeta".into(), id: "z".into(), ..Default::default() },
                    Template { name: tr("VRChat Avatar Project").into(), id: "com.vrchat.template.avatar".into(), builtin: true, ..Default::default() },
                    Template { name: "Alpha".into(), id: "a".into(), ..Default::default() },
                ];
                order(&mut all);
                all.into_iter().map(|t| t.id).collect::<Vec<_>>()
            });
            assert_eq!(ids, ["com.vrchat.template.avatar", "com.vrchat.template.world", "a", "z"], "{language:?}");
        }
    }

    #[test]
    fn the_bundled_templates_are_found_in_every_language() {
        // The avatar template used to be looked up by its (translated) name: in French it was "missing".
        for language in orbiters_i18n::Language::TRANSLATED {
            let temp = tempfile::tempdir().unwrap();
            let found = orbiters_i18n::in_language(language, || builtins(temp.path()).map(|all| all.len()));
            assert!(found.is_ok_and(|n| n >= 2), "{language:?}");
        }
    }

    /// PROJECTS-24 a: the base template's package list is written once, not again at every listing of the templates; one that
    /// is not the base's (the avatar template's, copied in first, or changed since) is written.
    #[test]
    fn the_base_template_s_manifest_is_written_only_when_it_differs() {
        let temp = tempfile::tempdir().unwrap();
        builtins(temp.path()).unwrap();
        let manifest = temp.path().join("base-2022.3.22f1/Packages/vpm-manifest.json");
        let base: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        assert_eq!(base, serde_json::json!({"dependencies":{"com.vrchat.base":{"version":"3.x"}}}));
        let long_ago = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        std::fs::File::options().write(true).open(&manifest).unwrap().set_modified(long_ago).unwrap();
        builtins(temp.path()).unwrap();
        assert_eq!(std::fs::metadata(&manifest).unwrap().modified().unwrap(), long_ago, "listed again: not written again");
        std::fs::write(&manifest, r#"{"dependencies":{"com.vrchat.avatars":{"version":"3.x"}}}"#).unwrap();
        builtins(temp.path()).unwrap();
        let again: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        assert_eq!(again, base, "another one is replaced");
    }

    #[test]
    fn an_error_is_said_in_the_language_of_the_moment_it_happens() {
        use orbiters_i18n::{in_language, Language};
        let english = validate_name("").unwrap_err().to_string();
        let french = in_language(Language::Fr, || validate_name("").unwrap_err().to_string());
        let japanese = in_language(Language::Ja, || validate_name("").unwrap_err().to_string());
        assert_ne!(english, french);
        assert_ne!(english, japanese);
        assert!(japanese.contains("フォルダー"), "{japanese}");
        // A value in the sentence stays: the path of a copy that was kept.
        let cancel = AtomicBool::new(true);
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("Source/Assets")).unwrap();
        std::fs::write(temp.path().join("Source/Assets/a.txt"), "a").unwrap();
        let said = in_language(Language::Es, || copy(&temp.path().join("Source"), &temp.path().join("Copy"), &cancel, &Arc::new(Mutex::default())).unwrap_err().to_string());
        assert!(said.contains("Copia detenida") && said.contains("Copy"), "{said}");
    }

    #[test]
    fn a_copy_phase_is_the_english_text_that_the_window_translates_where_it_draws_it() {
        // The windows compare the phase with "Copying project" to show Stop: it must not be the translation.
        let phase = String::from(mark("Copying project"));
        let progress = Progress { phase: phase.clone(), ..Default::default() };
        assert_eq!(progress.phase, "Copying project");
        let drawn = orbiters_i18n::in_language(orbiters_i18n::Language::Fr, || tr(&progress.phase).to_string());
        assert_ne!(drawn, "Copying project", "the French window shows its own words");
    }

    #[test]
    fn external_local_dependencies_are_counted_and_materialized() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        let external = temp.path().join("External");
        std::fs::create_dir_all(source.join("Packages")).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        std::fs::write(external.join("payload.bin"), vec![42u8; 4096]).unwrap();
        crate::write_json(&source.join("Packages/manifest.json"), &serde_json::json!({"dependencies":{"external.package":"file:../../External"}})).unwrap();
        let manifest_bytes = std::fs::metadata(source.join("Packages/manifest.json")).unwrap().len();
        assert_eq!(duplication_bytes(&source, Keep::default()).unwrap(), manifest_bytes + 4096);
        let destination = temp.path().join("Copy");
        let progress = Arc::new(Mutex::default());
        copy(&source, &destination, &AtomicBool::new(false), &progress).unwrap();
        copy_local_packages(&source, &destination, &AtomicBool::new(false), &progress).unwrap();
        assert_eq!(std::fs::read(destination.join("Packages/external.package/payload.bin")).unwrap(), vec![42u8; 4096]);
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(destination.join("Packages/manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["dependencies"]["external.package"], "file:external.package");
    }

    #[test]
    fn a_name_that_would_not_travel_is_refused_in_words_for_this_computer() {
        for name in ["a:b", "a/b", "a?", "CON", "name.", "name "] {
            let said = validate_name(name).unwrap_err().to_string();
            assert_eq!(said.contains("Windows folder name"), cfg!(windows), "{said}");
            assert!(cfg!(windows) || said.contains("also works on Windows"), "{said}");
        }
        validate_name("My avatars copy").unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_copy_keeps_programs_runnable_and_stays_writable() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir_all(source.join("Packages/tool/bin")).unwrap();
        let tool = source.join("Packages/tool/bin/run");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let locked = source.join("Packages/locked.json");
        std::fs::write(&locked, "{}").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o444)).unwrap();
        let copy = temp.path().join("copy");
        copy_keeping(&source, &copy, Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default())).unwrap();
        assert_eq!(std::fs::metadata(copy.join("Packages/tool/bin/run")).unwrap().permissions().mode() & 0o111, 0o111);
        // As on Windows, what comes next may write to it: a copy's settings are retargeted.
        assert!(!std::fs::metadata(copy.join("Packages/locked.json")).unwrap().permissions().readonly());
    }

    #[test]
    fn duplicate_preserves_unlisted_dependencies_and_registers_offline() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        let packages = source.join("Packages");
        std::fs::create_dir_all(packages.join("private.package")).unwrap();
        std::fs::create_dir_all(source.join("ProjectSettings")).unwrap();
        std::fs::write(source.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
        crate::write_json(&packages.join("private.package/package.json"), &serde_json::json!({"name":"private.package","version":"1.2.3"})).unwrap();
        crate::write_json(&packages.join("vpm-manifest.json"), &serde_json::json!({"dependencies":{"private.package":{"version":"1.2.3"}},"locked":{"private.package":{"version":"1.2.3","dependencies":{}}}})).unwrap();
        let root = temp.path().join("vcc");
        std::fs::create_dir_all(&root).unwrap();
        let rt = crate::runtime().unwrap();
        let copy = rt.block_on(duplicate(&root, &source, temp.path(), "Copy", Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default()))).unwrap();
        assert!(installed_dependencies_complete(&copy).unwrap());
        assert!(rt.block_on(crate::vpm::registered(&root)).unwrap().iter().any(|p| p.canonicalize().unwrap() == copy.canonicalize().unwrap()));
        std::fs::remove_file(copy.join("Packages/private.package/package.json")).unwrap();
        assert!(!installed_dependencies_complete(&copy).unwrap());
    }

    /// A project on disk in `dir`: its version file and a `Packages/manifest.json` with `dependencies`.
    fn project(dir: &Path, dependencies: serde_json::Value) {
        std::fs::create_dir_all(dir.join("ProjectSettings")).unwrap();
        std::fs::create_dir_all(dir.join("Assets")).unwrap();
        std::fs::write(dir.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
        std::fs::write(dir.join("Assets/a.txt"), "a").unwrap();
        crate::write_json(&dir.join("Packages/manifest.json"), &serde_json::json!({ "dependencies": dependencies })).unwrap();
    }
    fn is_registered(rt: &tokio::runtime::Runtime, root: &Path, path: &Path) -> bool {
        let wanted = crate::discovery::key(path);
        rt.block_on(crate::vpm::registered(root)).unwrap().iter().any(|p| crate::discovery::key(p) == wanted)
    }

    #[test]
    fn a_copy_that_fails_after_its_files_are_copied_is_listed_and_named() {
        // PROJECTS-1: a failure in the steps after the copy (here its local packages) left the copy on disk, unregistered.
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        std::fs::create_dir_all(temp.path().join("External")).unwrap();
        std::fs::write(temp.path().join("External/package.json"), "{}").unwrap();
        project(&source, serde_json::json!({ "bad/name": "file:../../External" }));
        let root = temp.path().join("vcc");
        let rt = crate::runtime().unwrap();
        let said = rt
            .block_on(duplicate(&root, &source, temp.path(), "Copy", Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default())))
            .map(|_| ())
            .map_err(|e| format!("{e:#}"))
            .unwrap_err();
        let copy = temp.path().join("Copy");
        assert!(said.contains(&copy.display().to_string()), "{said}");
        assert!(is_registered(&rt, &root, &copy), "the copy is in the gallery's list");
    }

    /// PROJECTS-4: a copy that stops halfway is not a project the gallery lists, and its folder is kept as it is.
    #[cfg(windows)]
    #[test]
    fn a_copy_cut_halfway_is_never_taken_for_a_project() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({}));
        std::fs::create_dir_all(source.join("Zz")).unwrap();
        std::fs::write(source.join("Zz/locked.bin"), "x").unwrap();
        // Held by another program: the copy fails on it, after ProjectSettings is in.
        let _held = std::fs::File::options().read(true).share_mode(0).open(source.join("Zz/locked.bin")).unwrap();
        let copy = temp.path().join("Copy");
        assert!(copy_keeping(&source, &copy, Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default())).is_err());
        assert!(copy.join("ProjectSettings/ProjectVersion.txt").is_file(), "kept as it is");
        assert!(!crate::discovery::is_project(&copy));
        drop(_held);
        let whole = temp.path().join("Whole");
        copy_keeping(&source, &whole, Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default())).unwrap();
        assert!(crate::discovery::is_project(&whole) && !whole.join(INCOMPLETE).exists());
    }

    /// A copy of `source` into `copy`, stopped once its folder exists after `meanwhile` ran: what Stop says.
    fn stopped_after(source: &Path, copy: &Path, meanwhile: impl FnOnce(&Path)) -> String {
        let cancel = AtomicBool::new(false);
        copy_keeping_then(source, copy, Keep::default(), true, &cancel, &Arc::new(Mutex::default()), |dest| {
            meanwhile(dest);
            cancel.store(true, Ordering::Relaxed);
        })
        .unwrap_err()
        .to_string()
    }

    /// PROJECTS-4: Stop removes what that copy made, and only that.
    #[test]
    fn stop_removes_the_incomplete_copy_and_nothing_else() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({}));
        let neighbour = temp.path().join("Neighbour");
        std::fs::create_dir_all(&neighbour).unwrap();
        std::fs::write(neighbour.join("mine.txt"), "mine").unwrap();
        let copy = temp.path().join("Copy");
        let said = stopped_after(&source, &copy, |_| {});
        assert!(said.contains("removed"), "{said}");
        assert!(!copy.exists(), "the incomplete copy is gone");
        assert!(source.join("Assets/a.txt").is_file() && neighbour.join("mine.txt").is_file());
    }

    /// A file the person put in the copy while it ran stays, named; the copy's own go.
    #[test]
    fn stop_keeps_a_file_added_during_the_copy() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({}));
        let copy = temp.path().join("Copy");
        let said = stopped_after(&source, &copy, |dest| std::fs::write(dest.join("Assets/mine.txt"), "mine").unwrap());
        assert_eq!(std::fs::read_to_string(copy.join("Assets/mine.txt")).unwrap(), "mine");
        assert!(said.contains("mine.txt"), "{said}");
        assert!(!copy.join(INCOMPLETE).exists() && !copy.join("ProjectSettings").exists(), "the copy's own entries went");
    }

    /// The copy's folder moved away and another put at its name (a restored backup, a junction to elsewhere): Stop
    /// removes nothing in either.
    #[test]
    fn stop_never_touches_a_folder_put_in_the_copys_place() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({}));
        std::fs::create_dir_all(source.join("Assets/Deep")).unwrap();
        std::fs::write(source.join("Assets/Deep/x.txt"), "x").unwrap();
        let copy = temp.path().join("Copy");
        let moved = temp.path().join("Moved");
        #[cfg(windows)]
        {
            // Verifier round 2: the copy's root, and every folder it made, are held while it runs: renaming either (to
            // put another in its place, or to have later writes land in a second copy) is refused.
            let said = stopped_after(&source, &copy, |dest| {
                assert!(std::fs::rename(dest, &moved).is_err(), "the root cannot be renamed while the copy runs");
                assert!(std::fs::rename(dest.join("Assets/Deep"), temp.path().join("Away")).is_err(), "nor a folder it made");
                assert!(std::fs::remove_dir(dest.join("Assets/Deep")).is_err());
            });
            assert!(said.contains("removed"), "{said}");
            assert!(!copy.exists() && !moved.exists() && !temp.path().join("Away").exists());
        }
        #[cfg(not(windows))]
        {
            // Without holding: the name is checked against the identity, and a substitute is never touched.
            let said = stopped_after(&source, &copy, |dest| {
                std::fs::rename(dest, &moved).unwrap();
                std::fs::create_dir_all(dest.join("Assets")).unwrap();
                std::fs::copy(moved.join(INCOMPLETE), dest.join(INCOMPLETE)).unwrap();
                std::fs::write(dest.join("Assets/a.txt"), "theirs").unwrap();
            });
            assert!(said.contains("could not be removed"), "{said}");
            assert_eq!(std::fs::read_to_string(copy.join("Assets/a.txt")).unwrap(), "theirs");
        }
    }

    /// A copy's marker copied into a real project (or left in one finished by hand elsewhere) does not hide it: it
    /// names another folder. Our copies never take a marker along.
    #[test]
    fn a_marker_from_another_folder_hides_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({}));
        let other = Marker { copy: "x".into(), destination: temp.path().join("Elsewhere"), started: 1, folder: None };
        std::fs::write(source.join(INCOMPLETE), serde_json::to_vec(&other).unwrap()).unwrap();
        assert!(crate::discovery::is_project(&source), "a marker naming another folder");
        let here = Marker { copy: "x".into(), destination: source.clone(), started: 1, folder: None };
        std::fs::write(source.join(INCOMPLETE), serde_json::to_vec(&here).unwrap()).unwrap();
        assert!(!crate::discovery::is_project(&source), "its own marker: an unfinished copy");
        // A marker naming this place but another folder (a project restored here with it): not hidden.
        let restored = Marker { copy: "x".into(), destination: source.clone(), started: 1, folder: Some(crate::owned::id_of(&temp.path().join("Source/Assets")).unwrap()) };
        std::fs::write(source.join(INCOMPLETE), serde_json::to_vec(&restored).unwrap()).unwrap();
        assert!(crate::discovery::is_project(&source), "the marker's folder identity is another folder's");
        let copy = temp.path().join("Copy");
        copy_keeping(&source, &copy, Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default())).unwrap();
        assert!(!copy.join(INCOMPLETE).exists() && crate::discovery::is_project(&copy));
    }

    #[test]
    fn packages_kept_only_in_library_are_named() {
        let temp = tempfile::tempdir().unwrap();
        let p = temp.path();
        std::fs::create_dir_all(p.join("Packages/com.me.embedded")).unwrap();
        crate::write_json(
            &p.join("Packages/packages-lock.json"),
            &serde_json::json!({"dependencies": {
                "com.unity.textmeshpro": {"version": "3.0.6", "source": "registry", "url": "https://packages.unity.com"},
                "com.me.tool": {"version": "https://github.com/me/tool.git", "source": "git", "hash": "abc"},
                "com.scoped.thing": {"version": "1.0.0", "source": "registry", "url": "https://npm.example.com"},
                "com.me.embedded": {"version": "https://github.com/me/e.git", "source": "git"},
                "com.unity.modules.ui": {"version": "1.0.0", "source": "builtin"}
            }}),
        )
        .unwrap();
        assert_eq!(packages_only_in_library(p), ["com.me.tool", "com.scoped.thing"]);
        assert!(packages_only_in_library(&p.join("nothing")).is_empty());
    }

    /// Verifier, point 7: Stop pressed once every file is in still lists the copy (a whole copy is never left unlisted).
    #[test]
    fn a_stop_after_the_last_file_still_lists_the_copy() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({}));
        let root = temp.path().join("vcc");
        let rt = crate::runtime().unwrap();
        STOP_AFTER_COPY.with(|s| s.set(true));
        let made = rt.block_on(duplicate(&root, &source, temp.path(), "Copy", Keep::default(), &AtomicBool::new(false), &Arc::new(Mutex::default())));
        STOP_AFTER_COPY.with(|s| s.set(false));
        let _ = made;
        assert!(is_registered(&rt, &root, &temp.path().join("Copy")));
    }

    /// A package folder is copied whole (its own Logs or Library are part of it); an archive the project already holds
    /// under the name the copy needs is never overwritten; a local package that is gone is named before anything is made.
    #[test]
    fn local_packages_are_copied_whole_and_never_over_another() {
        let temp = tempfile::tempdir().unwrap();
        let external = temp.path().join("External");
        std::fs::create_dir_all(external.join("Logs")).unwrap();
        std::fs::write(external.join("package.json"), "{}").unwrap();
        std::fs::write(external.join("Logs/part-of-the-package.txt"), "kept").unwrap();
        let source = temp.path().join("Source");
        project(&source, serde_json::json!({ "my.tool": "file:../../External" }));
        let dst = temp.path().join("Copy");
        copy(&source, &dst, &AtomicBool::new(false), &Arc::new(Mutex::default())).unwrap();
        copy_local_packages(&source, &dst, &AtomicBool::new(false), &Arc::new(Mutex::default())).unwrap();
        assert!(dst.join("Packages/my.tool/Logs/part-of-the-package.txt").is_file());
        // An archive: the source holds another file under the name the copy would give it.
        std::fs::write(temp.path().join("tool.tgz"), "the archive").unwrap();
        let source = temp.path().join("Source2");
        project(&source, serde_json::json!({ "my.archive": "file:../../tool.tgz" }));
        std::fs::write(source.join("Packages/my.archive.tgz"), "something else").unwrap();
        let estimate = duplication_bytes(&source, Keep::default()).unwrap();
        assert!(estimate >= ("the archive".len() + "something else".len()) as u64, "both counted: {estimate}");
        let dst = temp.path().join("Copy2");
        copy(&source, &dst, &AtomicBool::new(false), &Arc::new(Mutex::default())).unwrap();
        let said = copy_local_packages(&source, &dst, &AtomicBool::new(false), &Arc::new(Mutex::default())).unwrap_err().to_string();
        assert!(said.contains("my.archive.tgz"), "{said}");
        assert_eq!(std::fs::read_to_string(dst.join("Packages/my.archive.tgz")).unwrap(), "something else");
        // Gone: named with where it was looked for.
        let source = temp.path().join("Source3");
        project(&source, serde_json::json!({ "my.gone": "file:../../Gone" }));
        let said = format!("{:#}", duplication_bytes(&source, Keep::default()).unwrap_err());
        assert!(said.contains("my.gone") && said.contains("Gone"), "{said}");
    }

    #[test]
    fn a_copy_leaves_out_what_unity_makes_again() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("Source");
        for file in [
            "Assets/a.txt",
            "Assets/Library/kept.txt",
            "Assets/Tools/Temp/kept.txt",
            "Library/ArtifactDB",
            "Temp/x",
            "Logs/x",
            "obj/x",
            "UserSettings/x",
            ".vs/x",
            ".git/x",
            "Packages/local/node_modules/x",
            "Source.sln",
            "Assembly-CSharp.csproj",
            "Assets/Plugins/Tool.csproj",
            "ProjectSettings/ProjectVersion.txt",
        ] {
            let p = src.join(file);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "x").unwrap();
        }
        std::fs::create_dir_all(src.join("Assets/Empty")).unwrap();
        std::fs::create_dir_all(src.join("Temp/EmptyToo")).unwrap();
        let progress: SharedProgress = Arc::new(Mutex::default());
        let dst = temp.path().join("Copy");
        copy(&src, &dst, &AtomicBool::new(false), &progress).unwrap();
        assert!(dst.join("Assets/Empty").is_dir() && !dst.join("Temp").exists());
        let time = |p: &Path| std::fs::metadata(p).unwrap().modified().unwrap();
        assert_eq!(time(&dst.join("Assets/a.txt")), time(&src.join("Assets/a.txt")), "times kept");
        let has = |f: &str| dst.join(f).exists();
        for kept in ["Assets/a.txt", "Assets/Library/kept.txt", "Assets/Tools/Temp/kept.txt", "Assets/Plugins/Tool.csproj", "ProjectSettings/ProjectVersion.txt"] {
            assert!(has(kept), "{kept}");
        }
        for left in ["Library", "Temp", "Logs", "obj", "UserSettings", ".vs", ".git", "Packages/local/node_modules", "Source.sln", "Assembly-CSharp.csproj"] {
            assert!(!has(left), "{left}");
        }
        assert_eq!(progress.lock().unwrap().total, 5);
        // An empty asset folder stays, beside its .meta (PROJECTS-41).
        assert!(!has("Library") && has("Assets/Library"));
        let with = temp.path().join("With Library");
        copy_keeping(&src, &with, Keep { library: true }, &AtomicBool::new(false), &progress).unwrap();
        assert!(with.join("Library/ArtifactDB").is_file());
        assert!(!with.join("Temp").exists());
        assert!(copy_keeping(&src, &with, Keep { library: true }, &AtomicBool::new(false), &progress).is_err(), "never over a folder");
    }

    #[test]
    fn a_new_project_gets_its_own_product_and_a_copy_its_name_when_it_carried_the_old_one() {
        let settings = "PlayerSettings:\r\n  productGUID: 0123456789abcdef0123456789abcdef\r\n  companyName: DefaultCompany\r\n  productName: My avatars\r\n  defaultScreenWidth: 1024\r\n";
        let copy = retarget(settings, "My avatars copy", Some("My avatars"));
        assert!(copy.contains("  productName: \"My avatars copy\"\r\n"), "{copy}");
        assert!(!copy.contains("0123456789abcdef0123456789abcdef"));
        assert!(copy.contains("  companyName: DefaultCompany\r\n") && copy.ends_with("  defaultScreenWidth: 1024\r\n"));
        assert_eq!(copy.lines().count(), settings.lines().count());
        // A product name of its own stays; a template's always becomes the project's.
        let custom = "  productName: 'Studio''s base'\n  productGUID: aa\n";
        assert!(retarget(custom, "Copy", Some("Base")).contains("productName: 'Studio''s base'\n"));
        assert!(retarget(custom, "Copy", Some("Studio's base")).contains("productName: \"Copy\"\n"));
        assert!(retarget(custom, "New", None).contains("productName: \"New\"\n"));
    }
}
