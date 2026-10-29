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
            out.push(Template { name, path: p, unity, builtin });
        }
    }
    if !out.iter().any(|t| t.builtin) {
        out.extend(builtins(&crate::data_dir().join("unity-templates/alcom-v1.9.3")).unwrap_or_default());
    }
    out.sort_by(|a, b| (!a.builtin, &a.name).cmp(&(!b.builtin, &b.name)));
    out
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
        out.push(Template {
            name: if id.starts_with("avatars") { "VRChat Avatar Project" } else { "VRChat World Project" }.into(),
            path,
            unity: "2022.3.22f1".into(),
            builtin: true,
        });
    }
    let avatar = out.iter().find(|t| t.name == "VRChat Avatar Project").context("Bundled avatar template is missing")?;
    let path = cache.join("base-2022.3.22f1");
    if !path.join("ProjectSettings/ProjectVersion.txt").is_file() {
        copy(&avatar.path, &path, &AtomicBool::new(false), &Arc::new(Mutex::default()))?;
    }
    crate::write_json(&path.join("Packages/vpm-manifest.json"), &serde_json::json!({"dependencies":{"com.vrchat.base":{"version":"3.x"}}}))?;
    out.push(Template { name: "VRChat Base Project".into(), path, unity: "2022.3.22f1".into(), builtin: true });
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
fn excluded(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().as_str(), "library" | "temp" | "logs" | "obj" | "usersettings" | ".git" | "node_modules")
}
/// Copy into a newly reserved folder only. Reparse points are excluded even when Windows reports them as dirs.
pub fn copy(source: &Path, destination: &Path, cancel: &AtomicBool, progress: &SharedProgress) -> anyhow::Result<()> {
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
    std::fs::create_dir(destination).context("The destination already exists or cannot be created; choose a new project name.")?;
    let mut files = vec![];
    for entry in walkdir::WalkDir::new(&source)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !excluded(&e.file_name().to_string_lossy()) && !e.metadata().ok().is_some_and(|m| crate::discovery::is_link(&m)))
    {
        if cancel.load(Ordering::Relaxed) {
            bail!("Copy cancelled. The incomplete folder was kept at {} and was not registered.", destination.display());
        }
        let e = entry?;
        if e.file_type().is_file() {
            files.push(e.path().to_path_buf());
        }
    }
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
    validate_name(name)?;
    if !parent.is_absolute() {
        bail!("Choose an absolute destination folder.");
    }
    let destination = parent.join(name);
    copy(source, &destination, cancel, progress)?;
    let version = destination.join("ProjectSettings/ProjectVersion.txt");
    if !version.is_file() {
        if vrc_get_vpm::version::UnityVersion::parse(unity).is_none() {
            bail!("Choose a Unity editor version. The copied folder is at {}.", destination.display());
        }
        std::fs::create_dir_all(version.parent().unwrap())?;
        std::fs::write(version, format!("m_EditorVersion: {unity}\n"))?;
    }
    if !duplicate {
        let path = destination.join("ProjectSettings/ProjectSettings.asset");
        if let Ok(s) = std::fs::read_to_string(&path) {
            let s = s
                .lines()
                .map(|l| {
                    if l.trim_start().starts_with("productGUID:") {
                        format!("  productGUID: {}", uuid::Uuid::new_v4().simple())
                    } else if l.trim_start().starts_with("productName:") {
                        format!("  productName: {}", serde_json::to_string(name).unwrap())
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            std::fs::write(path, s)?;
        }
    }
    if cancel.load(Ordering::Relaxed) {
        bail!("Cancelled before resolving. The folder was kept and was not registered.");
    }
    progress.lock().unwrap().phase = "Resolving packages".into();
    crate::vpm::change(root, &destination, crate::vpm::Change::Resolve).await.context("The copy was kept, but package resolution failed")?;
    progress.lock().unwrap().phase = "Registering project".into();
    crate::vpm::register(root, &destination, true).await?;
    Ok(destination)
}
