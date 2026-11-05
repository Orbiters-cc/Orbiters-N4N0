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
#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub copied: usize,
    pub total: usize,
    pub file: String,
    pub phase: String,
}
pub type SharedProgress = Arc<Mutex<Progress>>;

pub fn list(root: &Path) -> Vec<Template> {
    let mut out = vec![];
    for (dir, builtin) in [("VRCTemplates", true), ("Templates", false)] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else { continue };
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_dir() || std::fs::symlink_metadata(&p).ok().is_some_and(|m| crate::discovery::is_link(&m)) {
                continue;
            }
            let Ok(bytes) = std::fs::read(p.join("package.json")) else { continue };
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else { continue };
            let name = v["displayName"].as_str().or_else(|| v["name"].as_str()).unwrap_or("Unity template").to_string();
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
    out.sort_by(|a, b| (!a.builtin, &a.name).cmp(&(!b.builtin, &b.name)));
    out
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
            name: if avatars { "VRChat Avatar Project" } else { "VRChat World Project" }.into(),
            path,
            unity: "2022.3.22f1".into(),
            builtin: true,
            id: if avatars { "com.vrchat.template.avatar" } else { "com.vrchat.template.world" }.into(),
            banner: None,
        });
    }
    let avatar = out.iter().find(|t| t.name == "VRChat Avatar Project").context("Bundled avatar template is missing")?;
    let path = cache.join("base-2022.3.22f1");
    if !path.join("ProjectSettings/ProjectVersion.txt").is_file() {
        copy(&avatar.path, &path, &AtomicBool::new(false), &Arc::new(Mutex::default()))?;
    }
    crate::write_json(&path.join("Packages/vpm-manifest.json"), &serde_json::json!({"dependencies":{"com.vrchat.base":{"version":"3.x"}}}))?;
    out.push(Template {
        name: "VRChat Base Project".into(),
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
        bail!("Choose a valid Windows folder name.");
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
/// Copy into a newly reserved folder only. Reparse points are excluded even when Windows reports them as dirs.
pub fn copy(source: &Path, destination: &Path, cancel: &AtomicBool, progress: &SharedProgress) -> anyhow::Result<()> {
    copy_keeping(source, destination, Keep::default(), cancel, progress)
}
/// [`copy`], taking along what `keep` asks for.
pub fn copy_keeping(source: &Path, destination: &Path, keep: Keep, cancel: &AtomicBool, progress: &SharedProgress) -> anyhow::Result<()> {
    if !source.is_dir() {
        bail!("The source folder no longer exists.");
    }
    let source = std::fs::canonicalize(source)?;
    let parent = destination.parent().context("Choose a destination folder")?;
    std::fs::create_dir_all(parent)?;
    let parent = std::fs::canonicalize(parent)?;
    if parent.starts_with(&source) {
        bail!("Create the copy outside the source project.");
    }
    anyhow::ensure!(!destination.exists(), "The destination already exists; choose a new project name.");
    let mut files = vec![];
    for entry in walkdir::WalkDir::new(&source)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            !excluded(&e.file_name().to_string_lossy(), e.depth(), e.file_type().is_dir(), keep)
        })
    {
        if cancel.load(Ordering::Relaxed) {
            bail!("Copy cancelled. The incomplete folder was kept at {} and was not registered.", destination.display());
        }
        let e = entry?;
        if e.file_type().is_file() {
            files.push(e.path().to_path_buf());
        }
    }
    std::fs::create_dir(destination).context("The destination already exists or cannot be created; choose a new project name.")?;
    progress.lock().unwrap().total = files.len();
    for (i, p) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            bail!("Copy cancelled. The incomplete folder was kept at {} and was not registered.", destination.display());
        }
        let rel = p.strip_prefix(&source)?;
        let dst = destination.join(rel);
        std::fs::create_dir_all(dst.parent().unwrap())?;
        let mut input = std::fs::File::open(p)?;
        let mut output = std::fs::File::create(&dst)?;
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("Copy cancelled. The incomplete folder was kept at {} and was not registered.", destination.display());
            }
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            output.write_all(&buffer[..n]).with_context(|| format!("Could not copy {}", rel.display()))?;
        }
        *progress.lock().unwrap() = Progress { copied: i + 1, total: files.len(), file: rel.display().to_string(), phase: "Copying project".into() };
    }
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
        bail!("Choose an absolute destination folder.");
    }
    let destination = parent.join(name);
    let required = duplication_bytes(source, copy.unwrap_or_default())?;
    let free = crate::size::free_space(parent).context("Cannot check available disk space. Choose an accessible destination.")?;
    anyhow::ensure!(free >= required, "Not enough disk space: free at least {} more bytes before duplicating.", required.saturating_sub(free));
    copy_keeping(source, &destination, copy.unwrap_or_default(), cancel, progress)?;
    if copy.is_some() { copy_local_packages(source, &destination, cancel, progress)?; }
    let version = destination.join("ProjectSettings/ProjectVersion.txt");
    if !version.is_file() {
        if vrc_get_vpm::version::UnityVersion::parse(unity).is_none() {
            bail!("Choose a Unity editor version. The copied folder is at {}.", destination.display());
        }
        std::fs::create_dir_all(version.parent().unwrap())?;
        std::fs::write(version, format!("m_EditorVersion: {unity}\n"))?;
    }
    // A product GUID of its own; a template's product name always becomes the project's, a copy's when it was the
    // copied project's name.
    let path = destination.join("ProjectSettings/ProjectSettings.asset");
    if let Ok(s) = std::fs::read_to_string(&path) {
        let from = copy.map(|_| source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        std::fs::write(&path, retarget(&s, name, from.as_deref()))?;
    }
    if cancel.load(Ordering::Relaxed) {
        bail!("Cancelled before resolving. The folder was kept and was not registered.");
    }
    // Register the completed copy before any optional restoration: a retained project must remain discoverable.
    progress.lock().unwrap().phase = "Registering project".into();
    crate::vpm::register(root, &destination, true).await
        .with_context(|| format!("The copy is at {}, but registration failed", destination.display()))?;
    // Duplication preserves installed package contents and exact locked versions, including packages whose
    // repositories are no longer available. Resolution is needed only for genuinely missing packages/templates.
    if copy.is_none() || !installed_dependencies_complete(&destination)? {
        progress.lock().unwrap().phase = "Restoring missing packages".into();
        crate::vpm::change(root, &destination, crate::vpm::Change::Resolve).await
            .with_context(|| format!("The copy is listed at {}, but missing packages could not be restored", destination.display()))?;
    }
    Ok(destination)
}

/// Fresh copy estimate using the same exclusions and link traversal as the copier, including local packages.
pub fn duplication_bytes(source: &Path, keep: Keep) -> anyhow::Result<u64> {
    fn bytes(path: &Path, keep: Keep) -> anyhow::Result<u64> {
        let mut total = 0u64;
        for e in walkdir::WalkDir::new(path).follow_links(true).into_iter()
            .filter_entry(|e| !excluded(&e.file_name().to_string_lossy(), e.depth(), e.file_type().is_dir(), keep)) {
            let e = e?;
            if e.file_type().is_file() { total = total.saturating_add(e.metadata()?.len()); }
        }
        Ok(total)
    }
    let mut total = bytes(source, keep)?;
    let manifest = source.join("Packages/manifest.json");
    if manifest.is_file() {
        let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(manifest)?)?;
        for (name, value) in manifest["dependencies"].as_object().into_iter().flatten() {
            if let Some(local) = value.as_str().and_then(|s| s.strip_prefix("file:")) {
                let path = source.join("Packages").join(local).canonicalize()?;
                if !source.join("Packages").join(name).exists() { total = total.saturating_add(bytes(&path, Keep::default())?); }
            }
        }
    }
    Ok(total)
}

/// Materialize external UPM folder dependencies so a moved duplicate does not depend on its source location.
fn copy_local_packages(source: &Path, destination: &Path, cancel: &AtomicBool, progress: &SharedProgress) -> anyhow::Result<()> {
    let path = destination.join("Packages/manifest.json");
    if !path.is_file() { return Ok(()); }
    let mut manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    let mut changed = false;
    if let Some(dependencies) = manifest["dependencies"].as_object_mut() {
        for (name, value) in dependencies {
            let Some(local) = value.as_str().and_then(|s| s.strip_prefix("file:")) else { continue };
            anyhow::ensure!(!name.contains(['/', '\\']) && name != "..", "Invalid package name: {name}");
            let origin = source.join("Packages").join(local).canonicalize()
                .with_context(|| format!("Required local package {name} is unavailable"))?;
            let target = destination.join("Packages").join(name);
            if origin.is_dir() {
                if !target.exists() { copy_keeping(&origin, &target, Keep::default(), cancel, progress)?; }
                *value = serde_json::Value::String(format!("file:{name}"));
            } else {
                let filename = format!("{name}.tgz");
                std::fs::copy(origin, destination.join("Packages").join(&filename))?;
                *value = serde_json::Value::String(format!("file:{filename}"));
            }
            changed = true;
        }
    }
    if changed { crate::write_json(&path, &manifest)?; }
    Ok(())
}

/// Check every locked dependency, including transitive dependencies, against the copied package metadata.
fn installed_dependencies_complete(project: &Path) -> anyhow::Result<bool> {
    let path = project.join("Packages/vpm-manifest.json");
    if !path.exists() { return Ok(true); }
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let locked = manifest["locked"].as_object();
    if let Some(dependencies) = manifest["dependencies"].as_object() {
        if dependencies.keys().any(|name| !locked.is_some_and(|l| l.contains_key(name))) { return Ok(false); }
    }
    for (name, package) in locked.into_iter().flatten() {
        anyhow::ensure!(!name.contains(['/', '\\']) && name != "..", "Invalid package name: {name}");
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
        let progress: SharedProgress = Arc::new(Mutex::default());
        let dst = temp.path().join("Copy");
        copy(&src, &dst, &AtomicBool::new(false), &progress).unwrap();
        let has = |f: &str| dst.join(f).exists();
        for kept in ["Assets/a.txt", "Assets/Library/kept.txt", "Assets/Tools/Temp/kept.txt", "Assets/Plugins/Tool.csproj", "ProjectSettings/ProjectVersion.txt"] {
            assert!(has(kept), "{kept}");
        }
        for left in ["Library", "Temp", "Logs", "obj", "UserSettings", ".vs", ".git", "Packages/local/node_modules", "Source.sln", "Assembly-CSharp.csproj"] {
            assert!(!has(left), "{left}");
        }
        assert_eq!(progress.lock().unwrap().total, 5);
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
