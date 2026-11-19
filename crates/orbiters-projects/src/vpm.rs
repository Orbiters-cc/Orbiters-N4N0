//! Uses ALCOM's VPM resolver and its shared VCC environment. Never hand-edit a VPM manifest.
use anyhow::{bail, Context};
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use vrc_get_vpm::environment::{self, PackageCollection, PackageInstaller, Settings, VccDatabaseConnection};
use vrc_get_vpm::io::{DefaultEnvironmentIo, DefaultProjectIo};
use vrc_get_vpm::{PackageCollection as _, UnityProject, VersionSelector};

pub const ORBITERS_REPOSITORY: &str = "https://orbiters.cc/vpm/orbiters/index.json";

/// One operation on the shared folder at a time, across this app's processes, until the guard is dropped.
#[cfg(windows)]
async fn named_lock(io: &DefaultEnvironmentIo, _root: &Path, name: &str) -> std::io::Result<Box<dyn Send + Sync>> {
    Ok(Box::new(io.new_mutex(name.as_ref()).await?))
}

/// Off Windows upstream's `new_mutex` ignores the name: it is one lock for the process, which
/// `VccDatabaseConnection::connect` takes too, so holding it while connecting waits on itself for ever. A file of our own
/// beside the settings does the job here.
#[cfg(not(windows))]
async fn named_lock(_io: &DefaultEnvironmentIo, root: &Path, name: &str) -> std::io::Result<std::fs::File> {
    let path = root.join(format!(".{}.lock", name.rsplit('\\').next().unwrap_or(name)));
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(path.parent().unwrap_or(&path))?;
        let file = std::fs::File::options().create(true).write(true).truncate(false).open(&path)?;
        file.lock()?;
        Ok(file)
    })
    .await
    .map_err(std::io::Error::other)?
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Repository {
    pub name: String,
    pub url: String,
    pub id: String,
    pub enabled: bool,
    pub builtin: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub display_name: String,
    pub installed: Option<String>,
    pub versions: Vec<String>,
    /// Where its icon comes from ([`crate::icons`]): the listing of its newest version (a listing Orbiters hosts has
    /// the logos creators set) and the websites its manifest names, its author's (`author.url`) and its own
    /// (`siteUrl`).
    pub repository: Option<String>,
    pub author_url: Option<String>,
    pub site_url: Option<String>,
}
impl Package {
    /// Fills the websites from a package manifest where none is known yet.
    fn websites_from(&mut self, manifest: &serde_json::Value) {
        let text = |v: &serde_json::Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(ToOwned::to_owned);
        self.author_url = self.author_url.take().or_else(|| text(&manifest["author"]["url"]));
        self.site_url = self.site_url.take().or_else(|| text(&manifest["siteUrl"]));
    }
}
#[derive(Clone, Debug, Default)]
pub struct View {
    pub repos: Vec<Repository>,
    pub packages: Vec<Package>,
    pub default_path: PathBuf,
    pub warnings: Vec<String>,
}

fn http() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder().user_agent(format!("Orbiters-N4N0/{}", orbiters_core::app_version())).timeout(std::time::Duration::from_secs(90)).build()?)
}
fn gui_config(root: &Path) -> PathBuf {
    root.join("vrc-get/gui-config.json")
}
fn read_gui(root: &Path) -> serde_json::Value {
    std::fs::read(gui_config(root)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| serde_json::json!({}))
}
fn hidden(root: &Path) -> HashSet<String> {
    read_gui(root)["guiHiddenRepositories"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(ToOwned::to_owned)).collect()).unwrap_or_default()
}

/// Read-only: deliberately does not call migrate/save, or UnityProject::load (upstream may repair its manifest).
pub async fn registered(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let io = DefaultEnvironmentIo::new(root.into());
    let settings = Settings::load(&io).await?;
    let db = VccDatabaseConnection::connect(&io).await?;
    let mut projects: Vec<PathBuf> = db.get_projects().iter().filter_map(|p| p.path().map(PathBuf::from)).collect();
    if let Some(paths) = settings.user_projects() {
        projects.extend(paths.iter().map(|s| PathBuf::from(s.as_ref())));
    }
    let mut seen = HashSet::new();
    projects.retain(|p| seen.insert(crate::discovery::key(p)));
    Ok(projects)
}

pub async fn register(root: &Path, path: &Path, add: bool) -> anyhow::Result<()> {
    std::fs::create_dir_all(root)?;
    let io = DefaultEnvironmentIo::new(root.into());
    let _lock = named_lock(&io, root, "Global\\Orbiters.Vpm.Settings").await?;
    let mut settings = Settings::load(&io).await?;
    let mut db = VccDatabaseConnection::connect(&io).await?;
    if add {
        if !crate::discovery::is_project(path) {
            bail!("{}", tr("Choose a Unity project containing ProjectSettings/ProjectVersion.txt."));
        }
        let project = UnityProject::load(DefaultProjectIo::new(path.into())).await?;
        if db.find_project(path.to_str().with_context(|| tr("Project path is not Unicode"))?)?.is_none() {
            db.add_project(&project).await?;
        }
        settings.add_user_project(path.to_str().with_context(|| tr("Project path is not Unicode"))?);
    } else {
        if let Some(p) = db.find_project(path.to_str().with_context(|| tr("Project path is not Unicode"))?)? {
            db.remove_project(&p);
        }
        settings.retain_user_projects(|p| crate::discovery::key(Path::new(p)) != crate::discovery::key(path));
    }
    db.save(&io).await?;
    settings.save(&io).await?;
    Ok(())
}

pub(crate) struct EnabledCollection {
    pub(crate) inner: PackageCollection,
    pub(crate) hidden: HashSet<String>,
}
impl EnabledCollection {
    fn enabled(&self, p: vrc_get_vpm::PackageInfo<'_>) -> bool {
        !p.repo().and_then(|r| r.id()).is_some_and(|id| self.hidden.contains(id))
    }
}
impl vrc_get_vpm::PackageCollection for EnabledCollection {
    fn get_all_packages(&self) -> impl Iterator<Item = vrc_get_vpm::PackageInfo<'_>> {
        self.inner.get_all_packages().filter(|p| self.enabled(*p))
    }
    fn find_packages(&self, name: &str) -> impl Iterator<Item = vrc_get_vpm::PackageInfo<'_>> {
        self.inner.find_packages(name).filter(|p| self.enabled(*p))
    }
    fn find_package_by_name(&self, name: &str, selector: VersionSelector) -> Option<vrc_get_vpm::PackageInfo<'_>> {
        self.find_packages(name).filter(|p| selector.satisfies(p.package_json())).max_by_key(|p| p.version())
    }
}
/// The predefined repositories VCC downloads itself, as cache files under `root`.
fn predefined_caches(root: &Path, settings: &Settings) -> Vec<PathBuf> {
    [
        (!settings.ignore_official_repository()).then_some("Repos/vrc-official.json"),
        (!settings.ignore_curated_repository()).then_some("Repos/vrc-curated.json"),
    ]
    .into_iter()
    .flatten()
    .map(|p| root.join(p))
    .collect()
}
/// Every cache of a repository that is downloaded from somewhere: VRChat's and the person's remote ones (a local
/// repository's file is the repository itself, nothing to download).
fn remote_caches(root: &Path, settings: &Settings) -> Vec<PathBuf> {
    let mut caches = predefined_caches(root, settings);
    caches.extend(settings.get_user_repos().iter().filter(|r| r.url().is_some()).map(|r| root.join(r.local_path())));
    caches
}
/// A cache file that is there but cannot be used: empty (written while its disk was full: vrc-get writes it in place),
/// cut short, or not a listing at all. vrc-get takes such a file for a repository never downloaded, and only a refresh
/// downloads it again: until then its packages have no versions and no updates.
fn unusable_cache(path: &Path) -> bool {
    match std::fs::read(path) {
        Ok(bytes) => {
            let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
            !serde_json::from_slice::<serde_json::Value>(text).is_ok_and(|v| v.is_object())
        }
        Err(_) => false,
    }
}
/// The remote repositories' caches that are there and unusable ([`unusable_cache`]).
fn broken_caches(root: &Path, settings: &Settings) -> Vec<PathBuf> {
    remote_caches(root, settings).into_iter().filter(|p| unusable_cache(p)).collect()
}
/// The broken caches last downloaded again, and when: a repository whose server is down keeps its broken cache, and must
/// not hold every look at the packages with a download.
static BROKEN_TRIED: std::sync::Mutex<Vec<(PathBuf, std::time::Instant)>> = std::sync::Mutex::new(Vec::new());
const BROKEN_RETRY: std::time::Duration = std::time::Duration::from_secs(5 * 60);
/// Whether some of `broken` was not tried lately; those are counted as tried now.
fn retry_broken(broken: &[PathBuf]) -> bool {
    let mut tried = BROKEN_TRIED.lock().unwrap_or_else(|e| e.into_inner());
    tried.retain(|(_, at)| at.elapsed() < BROKEN_RETRY);
    let due: Vec<&PathBuf> = broken.iter().filter(|p| !tried.iter().any(|(t, _)| t == *p)).collect();
    let any = !due.is_empty();
    for p in due {
        tried.push((p.clone(), std::time::Instant::now()));
    }
    any
}
/// VRChat's own repositories never downloaded: a computer without Creator Companion (or before its first start). A
/// person's repositories are downloaded when added (a dead one must not hold every look at the packages), but one whose
/// cache is broken counts as never downloaded (at most every few minutes each).
fn missing_cache(root: &Path, settings: &Settings) -> bool {
    predefined_caches(root, settings).iter().any(|p| !p.is_file()) || retry_broken(&broken_caches(root, settings))
}
/// The usable caches as they are now, to put back any that a download leaves broken.
fn usable_caches(root: &Path, settings: &Settings) -> Vec<(PathBuf, Vec<u8>)> {
    remote_caches(root, settings).into_iter().filter(|p| p.is_file() && !unusable_cache(p)).filter_map(|p| Some((p.clone(), std::fs::read(&p).ok()?))).collect()
}
/// Puts back each cache a download left unusable (vrc-get writes over the old file in place, so a full disk leaves it
/// empty): through a file beside it, renamed over it, so that a disk still full leaves it as it is (downloaded again
/// next time) rather than worse. The paths put back.
fn restore_usable(before: &[(PathBuf, Vec<u8>)]) -> Vec<PathBuf> {
    let mut restored = vec![];
    for (path, bytes) in before {
        if !unusable_cache(path) {
            continue;
        }
        let tmp = path.with_extension("json.restore");
        let put = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
        match put {
            Ok(()) => restored.push(path.clone()),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                eprintln!("the repository cache {} could not be put back: {e}", path.display()); // i18n: not shown (a log line)
            }
        }
    }
    restored
}
pub(crate) async fn collection(root: &Path, refresh: bool) -> anyhow::Result<(Settings, EnabledCollection)> {
    let io = DefaultEnvironmentIo::new(root.into());
    let settings = Settings::load(&io).await?;
    // Disabled repositories remain in VCC's settings; only the collection supplied to the resolver omits them.
    let hidden = hidden(root);
    let mut packages = PackageCollection::load_cache(&settings, &io).await?;
    // Without a cache the repository's packages are unknown: download it, as ALCOM does on its first start. `load`
    // downloads every remote repository and writes its cache (its `update_cache`).
    if refresh || missing_cache(root, &settings) {
        let before = usable_caches(root, &settings);
        packages = PackageCollection::load(&settings, &io, Some(&http()?)).await?;
        restore_usable(&before);
    }
    Ok((settings, EnabledCollection { inner: packages, hidden }))
}
pub async fn view(root: &Path, project: Option<&Path>, refresh: bool) -> anyhow::Result<View> {
    let (settings, collection) = collection(root, refresh).await?;
    let hidden = hidden(root);
    let mut repos = vec![];
    for r in settings.get_user_repos() {
        let id = r.id().unwrap_or_default().to_string();
        repos.push(Repository {
            name: r.name().unwrap_or(&id).into(),
            url: r.url().map(ToString::to_string).unwrap_or_else(|| r.local_path().display().to_string()),
            enabled: !hidden.contains(&id),
            id,
            builtin: false,
        });
    }
    for r in collection.inner.get_remote() {
        let id = r.id().unwrap_or_default();
        if repos.iter().all(|r| r.id != id) {
            repos.push(Repository {
                name: r.name().unwrap_or(id).into(),
                url: r.url().map(ToString::to_string).unwrap_or_default(),
                id: id.into(),
                enabled: !hidden.contains(id),
                builtin: true,
            });
        }
    }
    let mut packages: BTreeMap<String, Package> = BTreeMap::new();
    // The iterator contains every version; sort with VPM's SemVer ordering before producing strings.
    let mut all: Vec<_> = collection.get_all_packages().collect();
    all.sort_by(|a, b| b.version().cmp(a.version()));
    // Listings as their JSON (vrc-get's manifests leave out the author), read once each for the websites of the
    // packages' newest versions.
    let mut listings: HashMap<String, serde_json::Value> = HashMap::new();
    for p in all {
        let row = packages.entry(p.name().into()).or_insert_with(|| {
            let mut row = Package {
                name: p.name().into(),
                display_name: p.package_json().display_name().unwrap_or(p.name()).into(),
                repository: p.repo().and_then(|r| r.url()).map(ToString::to_string),
                ..Default::default()
            };
            if let Some(r) = p.repo() {
                let key = r.url().map(ToString::to_string).or_else(|| r.id().map(ToOwned::to_owned)).unwrap_or_default();
                let listing = listings.entry(key).or_insert_with(|| serde_json::to_value(r.repo()).unwrap_or_default());
                row.websites_from(&listing["packages"][p.name()]["versions"][p.version().to_string()]);
            }
            row
        });
        let v = p.version().to_string();
        if !row.versions.contains(&v) {
            row.versions.push(v);
        }
    }
    if let Some(project) = project {
        if let Ok(s) = std::fs::read_to_string(project.join("Packages/vpm-manifest.json")) {
            let v: serde_json::Value = serde_json::from_str(&s)?;
            if let Some(locked) = v["locked"].as_object() {
                for (name, p) in locked {
                    let row = packages.entry(name.clone()).or_insert_with(|| Package { name: name.clone(), display_name: name.clone(), ..Default::default() });
                    row.installed = p["version"].as_str().map(ToOwned::to_owned);
                    if let Ok(b) = std::fs::read(project.join("Packages").join(name).join("package.json")) {
                        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b) {
                            if let Some(n) = v["displayName"].as_str() {
                                row.display_name = n.into();
                            }
                            row.websites_from(&v);
                        }
                    }
                }
            }
        }
    }
    Ok(View {
        repos,
        packages: packages.into_values().collect(),
        default_path: settings
            .default_project_path()
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::document_dir().unwrap_or_else(std::env::temp_dir).join("VRChat Projects")),
        warnings: vec![],
    })
}

pub async fn add_repo(root: &Path, url: &str, headers: BTreeMap<String, String>) -> anyhow::Result<()> {
    let io = DefaultEnvironmentIo::new(root.into());
    let _lock = named_lock(&io, root, "Global\\Orbiters.Vpm.Settings").await?;
    let mut settings = Settings::load(&io).await?;
    let url = url::Url::parse(url)?;
    if !matches!(url.scheme(), "https" | "http") {
        bail!("{}", tr("A repository URL must use HTTPS or HTTP."));
    }
    if settings.get_user_repos().iter().any(|r| r.url() == Some(&url)) {
        return Ok(());
    }
    environment::add_remote_repo(&mut settings, url, None, headers.into_iter().map(|(k, v)| (k.into(), v.into())).collect(), &io, &http()?).await?;
    settings.save(&io).await?;
    Ok(())
}
pub async fn remove_repo(root: &Path, id: &str) -> anyhow::Result<()> {
    let io = DefaultEnvironmentIo::new(root.into());
    let _lock = named_lock(&io, root, "Global\\Orbiters.Vpm.Settings").await?;
    let mut settings = Settings::load(&io).await?;
    settings.remove_repo(|r| r.id() == Some(id));
    settings.save(&io).await?;
    Ok(())
}
pub async fn set_enabled(root: &Path, id: &str, enabled: bool) -> anyhow::Result<()> {
    let io = DefaultEnvironmentIo::new(root.into());
    let _lock = named_lock(&io, root, "Global\\Orbiters.Vpm.Settings").await?;
    let mut config = read_gui(root);
    let mut ids = hidden(root);
    if enabled {
        ids.remove(id);
    } else {
        ids.insert(id.into());
    }
    config["guiHiddenRepositories"] = serde_json::to_value(ids)?;
    crate::write_json(&gui_config(root), &config)
}

#[derive(Clone, Debug)]
pub enum Change {
    Install { names: Vec<(String, Option<String>)>, prerelease: bool },
    Remove(String),
    Resolve,
    /// A conflict's plan ([`crate::coherent`]), confirmed: `remove` first, then the exact versions of `install`.
    /// Any failure puts the project's manifest back and restores its packages.
    Plan { install: Vec<(String, String)>, remove: Vec<String>, prerelease: bool },
}
pub async fn change(root: &Path, path: &Path, action: Change) -> anyhow::Result<()> {
    // Serialize mutations across this app's windows. VCC's database has its own upstream shared LiteDB mutex.
    let io = DefaultEnvironmentIo::new(root.into());
    let _lock = named_lock(&io, root, "Global\\Orbiters.Vpm.Packages").await?;
    let (settings, packages) = collection(root, true).await?;
    // A private repository's headers go to its own origin only, not to wherever its listing puts an archive (AS-11).
    let client = crate::vpm_http::Client::new(http()?, &settings)?;
    let mut project = UnityProject::load(DefaultProjectIo::new(path.into())).await?;
    if let Change::Plan { install, remove, prerelease } = &action {
        return apply_plan(&io, &client, &packages, path, project, install, remove, *prerelease).await;
    }
    let mut asked: Vec<(String, String)> = vec![];
    let changes = match &action {
        Change::Plan { .. } => unreachable!(),
        Change::Resolve => project.resolve_request(&packages).await?,
        Change::Remove(name) => project.remove_request(&[name.as_str()]).await?,
        Change::Install { names, prerelease } => {
            let mut requested = vec![];
            for (name, version) in names {
                let version = version.as_deref().map(str::parse).transpose()?;
                let selector = version
                    .as_ref()
                    .map(VersionSelector::specific_version)
                    .unwrap_or_else(|| VersionSelector::latest_for(Some(project.unity_version()), *prerelease));
                let p = packages
                    .find_package_by_name(name, selector)
                    .with_context(|| tr!("No compatible version of {name} is available in enabled repositories.", name = name))?;
                asked.push((p.name().to_string(), p.version().to_string()));
                requested.push(p);
            }
            project.add_package_request(&packages, &requested, vrc_get_vpm::unity_project::AddPackageOperation::AutoDetected, *prerelease).await?
        }
    };
    if !changes.conflicts().is_empty() {
        // An install that conflicts: refused, with the plan that makes it coherent (the desktop shows it to confirm).
        if let Change::Install { prerelease, .. } = &action {
            let plan = crate::coherent::solve_with(&project, &packages, &asked, &[], *prerelease).await;
            return Err(crate::coherent::Conflicted { plan }.into());
        }
        bail!(
            "{}",
            tr!(
                "These package changes conflict with installed packages: {names}. No changes were applied.",
                names = changes.conflicts().keys().map(|s| s.as_ref()).collect::<Vec<_>>().join(", ")
            )
        );
    }
    project.apply_pending_changes(&PackageInstaller::new(&io, Some(&client)), changes).await?;
    project.save().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn apply_plan(
    io: &DefaultEnvironmentIo,
    client: &crate::vpm_http::Client,
    packages: &EnabledCollection,
    path: &Path,
    project: UnityProject,
    install: &[(String, String)],
    remove: &[String],
    prerelease: bool,
) -> anyhow::Result<()> {
    let manifest = path.join("Packages/vpm-manifest.json");
    let backup = std::fs::read(&manifest)?;
    let installer = PackageInstaller::new(io, Some(client));
    let result: anyhow::Result<()> = async {
        let mut project = project;
        if !remove.is_empty() {
            let names: Vec<&str> = remove.iter().map(String::as_str).collect();
            let changes = project.remove_request(&names).await?;
            project.apply_pending_changes(&installer, changes).await?;
            project.save().await?;
            project = UnityProject::load(DefaultProjectIo::new(path.into())).await?;
        }
        let mut requested = vec![];
        for (name, version) in install {
            let v: vrc_get_vpm::version::Version = version.parse()?;
            requested.push(
                packages
                    .find_package_by_name(name, VersionSelector::specific_version(&v))
                    .with_context(|| tr!("{name} {version} is no longer in the enabled repositories.", name = name, version = version))?,
            );
        }
        let changes = project.add_package_request(packages, &requested, vrc_get_vpm::unity_project::AddPackageOperation::AutoDetected, prerelease).await?;
        if !changes.conflicts().is_empty() {
            bail!(
                "{}",
                tr!(
                    "The plan conflicts again with: {names} (the repositories changed since it was made).",
                    names = changes.conflicts().keys().map(|s| s.as_ref()).collect::<Vec<_>>().join(", ")
                )
            );
        }
        project.apply_pending_changes(&installer, changes).await?;
        project.save().await?;
        Ok(())
    }
    .await;
    if let Err(e) = result {
        // Rolled back: the manifest as it was, its locked packages restored.
        std::fs::write(&manifest, &backup)?;
        let restored: anyhow::Result<()> = async {
            let mut project = UnityProject::load(DefaultProjectIo::new(path.into())).await?;
            let changes = project.resolve_request(packages).await?;
            project.apply_pending_changes(&installer, changes).await?;
            project.save().await?;
            Ok(())
        }
        .await;
        return Err(match restored {
            Ok(()) => e.context(tr("Nothing was changed: the project's packages are as they were")),
            Err(r) => e.context(tr!("The package list was put back, but restoring its packages failed ({why}): use Restore", why = format!("{r:#}"))),
        });
    }
    Ok(())
}
/// Installs the latest releases of `names` from the Orbiters repository, adding it (or turning it back on) first.
pub async fn install_orbiters(root: &Path, path: &Path, names: &[&str]) -> anyhow::Result<()> {
    add_repo(root, ORBITERS_REPOSITORY, BTreeMap::new()).await?;
    // Ensure an existing disabled Orbiters repo is enabled before resolving its packages.
    let io = DefaultEnvironmentIo::new(root.into());
    let settings = Settings::load(&io).await?;
    if let Some(id) = settings.get_user_repos().iter().find(|r| r.url().is_some_and(|u| u.as_str() == ORBITERS_REPOSITORY)).and_then(|r| r.id()) {
        set_enabled(root, id, true).await?;
    }
    change(root, path, Change::Install { names: names.iter().map(|n| (n.to_string(), None)).collect(), prerelease: false }).await
}
pub async fn install_my_avatar(root: &Path, path: &Path) -> anyhow::Result<()> {
    install_orbiters(root, path, &["orbiters.toolkit", "orbiters.myavatar"]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A VCC folder with VRChat's caches and one remote repository of the person's, its cache holding `cache`.
    fn environment(cache: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Repos")).unwrap();
        let listing = br#"{"name":"VRChat","url":"https://example.invalid/vrc.json","packages":{}}"#;
        std::fs::write(root.join("Repos/vrc-official.json"), listing).unwrap();
        std::fs::write(root.join("Repos/vrc-curated.json"), listing).unwrap();
        let mine = root.join("Repos/cc.orbiters.vpm-11a3a2f7.json");
        std::fs::write(&mine, cache).unwrap();
        let settings = serde_json::json!({ "userRepos": [{ "localPath": mine, "name": "Orbiters", "url": ORBITERS_REPOSITORY, "id": "cc.orbiters.vpm" }] });
        std::fs::write(root.join("settings.json"), serde_json::to_vec(&settings).unwrap()).unwrap();
        (dir, mine)
    }

    fn settings(root: &Path) -> Settings {
        runtime_for_tests().block_on(Settings::load(&DefaultEnvironmentIo::new(root.into()))).unwrap()
    }

    fn runtime_for_tests() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    /// 2026-11-09: the Orbiters repository's cache was a 0-byte file (written while C: was full), and orbiters.mcb showed
    /// no versions and no update. An empty, cut or foreign cache counts as missing, and is downloaded again.
    #[test]
    fn a_broken_cache_of_a_persons_repository_is_downloaded_again() {
        for broken in [&b""[..], b"  \r\n", b"{\"name\":\"Orbit", b"[]", b"\xEF\xBB\xBF"] {
            let (dir, mine) = environment(broken);
            let settings = settings(dir.path());
            assert_eq!(broken_caches(dir.path(), &settings), vec![mine.clone()], "{broken:?}");
            assert!(predefined_caches(dir.path(), &settings).iter().all(|p| p.is_file()));
        }
        let (dir, _) = environment(br#"{"name":"Orbiters","packages":{}}"#);
        assert!(broken_caches(dir.path(), &settings(dir.path())).is_empty(), "a good cache is left alone");
        // A local repository's file is the repository itself: nothing to download.
        let (dir, mine) = environment(b"");
        let local = serde_json::json!({ "userRepos": [{ "localPath": mine, "name": "Mine" }] });
        std::fs::write(dir.path().join("settings.json"), serde_json::to_vec(&local).unwrap()).unwrap();
        assert!(broken_caches(dir.path(), &settings(dir.path())).is_empty());
    }

    /// Asking again for the same broken cache waits a few minutes: a server that is down must not hold every look.
    #[test]
    fn a_broken_cache_is_tried_again_only_now_and_then() {
        let path = PathBuf::from(format!("Z:/no/such/{}.json", uuid::Uuid::new_v4()));
        assert!(retry_broken(std::slice::from_ref(&path)));
        assert!(!retry_broken(std::slice::from_ref(&path)));
        assert!(!retry_broken(&[]));
    }

    /// A download that leaves a cache empty (a full disk) never replaces a good one: the good one is put back.
    #[test]
    fn a_failed_write_never_replaces_a_good_cache() {
        let good = br#"{"name":"Orbiters","url":"https://orbiters.cc/vpm/orbiters/index.json","packages":{"orbiters.mcb":{"versions":{}}}}"#;
        let (dir, mine) = environment(good);
        let settings = settings(dir.path());
        let before = usable_caches(dir.path(), &settings);
        assert_eq!(before.len(), 3, "VRChat's two and the person's");
        std::fs::write(&mine, b"").unwrap();
        assert_eq!(restore_usable(&before), vec![mine.clone()]);
        assert_eq!(std::fs::read(&mine).unwrap(), good);
        assert!(!mine.with_extension("json.restore").exists());
        // A download that wrote a new good cache keeps it.
        let newer = br#"{"name":"Orbiters","packages":{}}"#;
        std::fs::write(&mine, newer).unwrap();
        assert!(restore_usable(&before).is_empty());
        assert_eq!(std::fs::read(&mine).unwrap(), newer);
    }
}
