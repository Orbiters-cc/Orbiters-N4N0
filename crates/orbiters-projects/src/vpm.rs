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

/// Puts the project at `path` on the list VCC and ALCOM share, or takes it off: the gallery's, the bridge's, Duplicate's and New
/// project's. Never a Unity 6 trial's: where it stands as one is asked first, before anything is opened.
pub async fn register(root: &Path, path: &Path, add: bool) -> anyhow::Result<()> {
    use crate::unity6::authority::{allow, Action};
    allow(&crate::data_dir(), path, if add { Action::List } else { Action::Unlist }).map_err(|why| anyhow::anyhow!("{why}"))?;
    register_any(root, path, add).await?;
    if add {
        crate::copies::listed(&crate::data_dir(), path);
    }
    Ok(())
}

async fn register_any(root: &Path, path: &Path, add: bool) -> anyhow::Result<()> {
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

/// Whether VPM's project loader leaves the project as it is: vrc-get's `UnityProject::load` rewrites a manifest holding the
/// "3.2-3.8" range spelling an old vrc-get wrote.
pub(crate) fn loads_unchanged(path: &Path) -> std::io::Result<bool> {
    const REWRITTEN: &[u8] = br#""3.2-3.8""#;
    match std::fs::read(path.join("Packages/vpm-manifest.json")) {
        Ok(manifest) => Ok(!manifest.windows(REWRITTEN.len()).any(|w| w == REWRITTEN)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(e),
    }
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
        // Listings through the same rule as archives: a repository's headers to its own origin only, redirects included.
        packages = PackageCollection::load(&settings, &io, Some(&crate::vpm_http::Client::new(http()?, &settings)?)).await?;
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
    let client = crate::vpm_http::Client::new(http()?, &settings)?.with_new_repository(&url, &headers);
    environment::add_remote_repo(&mut settings, url, None, headers.into_iter().map(|(k, v)| (k.into(), v.into())).collect(), &io, &client).await?;
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
pub(crate) fn may_change(path: &Path) -> anyhow::Result<()> {
    use crate::unity6::authority::{allow, Action};
    allow(&crate::data_dir(), path, Action::Packages).map(|_| ()).map_err(|why| anyhow::anyhow!("{why}"))
}

pub async fn change(root: &Path, path: &Path, action: Change) -> anyhow::Result<()> {
    // A Unity 6 trial keeps its packages until Creator Companion migrated it, whoever asks (the page, the bridge, the
    // Toolkit's update).
    may_change(path)?;
    // Serialize mutations across this app's windows. VCC's database has its own upstream shared LiteDB mutex.
    let io = DefaultEnvironmentIo::new(root.into());
    let _lock = named_lock(&io, root, "Global\\Orbiters.Vpm.Packages").await?;
    // A plan an earlier run stopped halfway is put back first, from its journal (kept outside the project), before any
    // repository is reached: a network failure must not hold that up.
    recover_plans(path, &plan_journals(root))?;
    let (settings, packages) = collection(root, true).await?;
    // A private repository's headers go to its own origin only, not to wherever its listing puts an archive (AS-11).
    let client = crate::vpm_http::Client::new(http()?, &settings)?;
    let mut project = UnityProject::load(DefaultProjectIo::new(path.into())).await?;
    if let Change::Plan { install, remove, prerelease } = &action {
        return apply_plan(&io, &client, &packages, path, project, install, remove, *prerelease, &plan_journals(root)).await;
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
    journals: &Path,
) -> anyhow::Result<()> {
    // Every version asked for is looked up before anything changes: one gone from the repositories changes nothing.
    let mut requested = vec![];
    for (name, version) in install {
        let v: vrc_get_vpm::version::Version = version.parse()?;
        requested.push(
            packages
                .find_package_by_name(name, VersionSelector::specific_version(&v))
                .with_context(|| tr!("{name} {version} is no longer in the enabled repositories.", name = name, version = version))?,
        );
    }
    // Everything the plan changes is kept as it is (local edits, bytes no repository has) until the whole plan has gone
    // through (PROJECTS-35): the manifest, then before each phase the folders and old assets VPM is about to remove or
    // replace there (packages removed only because nothing else needed them any more included). A journal beside them
    // lets the next package action finish putting them back if the app stops halfway.
    let mut kept = Kept::begin(path, journals)?;
    let installer = PackageInstaller::new(io, Some(client));
    let result: anyhow::Result<()> = async {
        let mut project = project;
        if !remove.is_empty() {
            let names: Vec<&str> = remove.iter().map(String::as_str).collect();
            let changes = project.remove_request(&names).await?;
            kept.take(&affected(&changes))?;
            project.apply_pending_changes(&installer, changes).await?;
            project.save().await?;
            project = UnityProject::load(DefaultProjectIo::new(path.into())).await?;
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
        kept.take(&affected(&changes))?;
        project.apply_pending_changes(&installer, changes).await?;
        project.save().await?;
        Ok(())
    }
    .await;
    let Err(e) = result else {
        return kept.commit();
    };
    // Rolled back: the manifest as it was, whatever else is missing restored by VPM, then the kept entries over what
    // the resolver put there (the exact bytes the project had win). Each step is tried whatever the others did.
    let mut problems: Vec<String> = vec![];
    if let Err(r) = kept.put_back_manifest() {
        problems.push(r.to_string());
    }
    let restored: anyhow::Result<()> = async {
        let mut project = UnityProject::load(DefaultProjectIo::new(path.into())).await?;
        let changes = project.resolve_request(packages).await?;
        project.apply_pending_changes(&installer, changes).await?;
        project.save().await?;
        Ok(())
    }
    .await;
    problems.extend(kept.put_back());
    if problems.is_empty() {
        kept.finish();
    }
    let dir = kept.dir.clone();
    Err(match (restored, problems.is_empty()) {
        (Ok(()), true) => e.context(tr("Nothing was changed: the project's packages are as they were")),
        (Err(r), true) => e.context(tr!("The package list was put back, but restoring its packages failed ({why}): use Restore", why = format!("{r:#}"))),
        (_, false) => e.context(tr!(
            "Not everything could be put back: {problems}. What the project had is kept in {path}; the next package change puts it back.",
            problems = problems.join("; "),
            path = dir.display()
        )),
    })
}

/// What `changes` removes or replaces in the project, relative to it: each package's folder and each old asset (with
/// its `.meta`).
fn affected(changes: &vrc_get_vpm::unity_project::PendingProjectChanges<'_>) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = changes.package_changes().keys().map(|name| Path::new("Packages").join(name.as_ref())).collect();
    for (p, _) in changes.remove_legacy_files().iter().chain(changes.remove_legacy_folders()) {
        paths.push(p.to_path_buf());
        let mut meta = p.as_os_str().to_owned();
        meta.push(".meta");
        paths.push(PathBuf::from(meta));
    }
    paths
}

/// `bytes` written over `path` through a file beside it, renamed over it: never a half-written file.
fn replace_bytes(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("json.orbiters-{}", uuid::Uuid::new_v4().simple()));
    let written = (|| {
        use std::io::Write as _;
        let mut file = std::fs::File::options().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Where the journals of package plans are: beside the settings of the Creator Companion folder `root` this app
/// changes packages with, outside every project (verifier round 2). A journal found anywhere else (in a project copied
/// or imported from elsewhere) is never acted on.
fn plan_journals(root: &Path) -> PathBuf {
    root.join("Orbiters").join("package-plans")
}
/// The manifest as it was, in the plan's folder of copies.
const MANIFEST_KEPT: &str = "vpm-manifest.json";

/// One thing a plan may change, as it was before.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Before {
    /// There, copied under `entries/`.
    Kept,
    /// Not there: what the plan puts there goes on a rollback.
    Absent,
    /// A link (a package folder linked from elsewhere) to this target: made again on a rollback.
    Link(PathBuf),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Item {
    path: PathBuf,
    before: Before,
    /// Put back already (a rollback that stopped halfway is finished from here, never redone).
    #[serde(default)]
    restored: bool,
}

/// A plan's journal: the project it was written for (by place and folder identity), its folder of copies (the same),
/// its state and items.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Journal {
    project: PathBuf,
    project_id: crate::owned::FileId,
    staging: PathBuf,
    staging_id: crate::owned::FileId,
    state: String,
    items: Vec<Item>,
}

/// What a plan changes, kept as it was: the manifest and each entry in a folder of the project this plan made
/// (`.orbiters-plan-<id>`, same disk for the moves back), described by a journal outside the project.
struct Kept {
    project: PathBuf,
    dir: PathBuf,
    journal_path: PathBuf,
    journal: Journal,
}

/// Whether `rel` is a place a plan may change: under the project's Packages or Assets, plain names only.
fn allowed(rel: &Path) -> bool {
    rel.components().all(|c| matches!(c, std::path::Component::Normal(_)))
        && rel.components().next().is_some_and(|c| c.as_os_str() == "Packages" || c.as_os_str() == "Assets") // i18n: not shown (folder names)
        && rel.components().count() >= 2
}

impl Kept {
    /// The folder, the manifest kept and the journal ("pending"): before anything changes.
    fn begin(project: &Path, journals: &Path) -> anyhow::Result<Kept> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let dir = project.join(format!(".orbiters-plan-{id}"));
        let started = (|| -> anyhow::Result<Kept> {
            std::fs::create_dir_all(journals)?;
            std::fs::create_dir(&dir)?;
            std::fs::copy(project.join("Packages/vpm-manifest.json"), dir.join(MANIFEST_KEPT))?;
            let journal = Journal {
                project: project.to_path_buf(),
                project_id: crate::owned::id_of(project)?,
                staging: dir.clone(),
                staging_id: crate::owned::id_of(&dir)?,
                state: "pending".into(),
                items: vec![],
            };
            let kept = Kept { project: project.to_path_buf(), dir: dir.clone(), journal_path: journals.join(format!("{id}.json")), journal };
            kept.write()?;
            Ok(kept)
        })();
        started.map_err(|e| {
            let _ = std::fs::remove_dir_all(&dir);
            e.context(tr("Could not keep a copy of the package list before changing it: nothing was changed"))
        })
    }
    fn write(&self) -> std::io::Result<()> {
        replace_bytes(&self.journal_path, &serde_json::to_vec_pretty(&self.journal).unwrap_or_default())
    }
    /// What `paths` (relative to the project) are now, before the phase that changes them, for those not kept yet: a
    /// copy, a link's target, or that nothing is there. Fails, changing nothing more, when one cannot be read or copied.
    fn take(&mut self, paths: &[PathBuf]) -> anyhow::Result<()> {
        for rel in paths {
            if self.journal.items.iter().any(|i| &i.path == rel) || !allowed(rel) {
                continue;
            }
            let from = self.project.join(rel);
            let failed = |e: anyhow::Error| e.context(tr!("Could not keep a copy of {name} before changing it: nothing was changed", name = rel.display()));
            let before = match std::fs::symlink_metadata(&from) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Before::Absent,
                Err(e) => return Err(failed(e.into())),
                Ok(m) if m.file_type().is_symlink() || is_link(&from) => Before::Link(link_target(&from).map_err(|e| failed(e.into()))?),
                Ok(m) => {
                    let to = self.dir.join("entries").join(rel);
                    std::fs::create_dir_all(to.parent().unwrap_or(&self.dir))
                        .map_err(anyhow::Error::from)
                        .and_then(|()| if m.is_dir() { copy_tree(&from, &to) } else { std::fs::copy(&from, &to).map(drop).map_err(anyhow::Error::from) })
                        .map_err(failed)?;
                    Before::Kept
                }
            };
            self.journal.items.push(Item { path: rel.clone(), before, restored: false });
            self.write()?;
        }
        Ok(())
    }
    fn put_back_manifest(&self) -> anyhow::Result<()> {
        let bytes = std::fs::read(self.dir.join(MANIFEST_KEPT))?;
        replace_bytes(&self.project.join("Packages/vpm-manifest.json"), &bytes)
            .map_err(|r| anyhow::anyhow!("{}", tr!("The package list could not be put back ({why}): use Restore", why = r)))
    }
    /// Every item back as it was, in place of what the plan left there; each tried whatever the others did and marked
    /// done in the journal. What could not be put back, named (its copy stays).
    fn put_back(&mut self) -> Vec<String> {
        let mut problems = vec![];
        for i in 0..self.journal.items.len() {
            let item = self.journal.items[i].clone();
            if item.restored {
                continue;
            }
            let at = self.project.join(&item.path);
            let from = self.dir.join("entries").join(&item.path);
            let put = (|| -> std::io::Result<()> {
                // A copy that cannot be read is not taken as put back: the rollback stops there, the journal stays.
                if item.before == Before::Kept {
                    std::fs::symlink_metadata(&from)?;
                }
                match std::fs::symlink_metadata(&at) {
                    Ok(m) if m.file_type().is_symlink() || is_link(&at) => std::fs::remove_dir(&at).or_else(|_| std::fs::remove_file(&at))?,
                    Ok(m) if m.is_dir() => std::fs::remove_dir_all(&at)?,
                    Ok(_) => std::fs::remove_file(&at)?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
                match &item.before {
                    Before::Absent => Ok(()),
                    Before::Kept => {
                        if let Some(parent) = at.parent() {
                            std::fs::create_dir_all(parent)?;
                        }
                        std::fs::rename(&from, &at)
                    }
                    Before::Link(target) => make_link(target, &at),
                }
            })();
            match put {
                Ok(()) => {
                    self.journal.items[i].restored = true;
                    let _ = self.write();
                }
                Err(e) => problems.push(tr!("{name} could not be put back ({why}): its copy is kept at {path}", name = item.path.display(), why = e, path = from.display())),
            }
        }
        problems
    }
    /// The plan went through: its journal says so (or goes), then the copies go. Should neither be possible, the next
    /// change could take the finished plan for a stopped one: that is an error, never ignored.
    fn commit(mut self) -> anyhow::Result<()> {
        self.journal.state = "committed".into();
        if self.write().is_err() && std::fs::remove_file(&self.journal_path).is_err() {
            bail!("{}", tr!("The packages were changed, but the record of the change at {path} could not be closed: remove it before the next package change", path = self.journal_path.display()));
        }
        self.finish();
        Ok(())
    }
    /// The copies and the journal go (nothing is waiting in them any more).
    fn finish(&self) {
        if let Err(e) = std::fs::remove_dir_all(&self.dir) {
            eprintln!("the copies kept for a package plan could not be removed from {}: {e}", self.dir.display()); // i18n: not shown (a log line)
        }
        let _ = std::fs::remove_file(&self.journal_path);
    }
}

fn link_target(path: &Path) -> std::io::Result<PathBuf> {
    #[cfg(windows)]
    {
        junction::get_target(path).or_else(|_| std::fs::read_link(path))
    }
    #[cfg(not(windows))]
    {
        std::fs::read_link(path)
    }
}

fn make_link(target: &Path, at: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        junction::create(target, at)
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, at)
    }
}

/// Finishes putting back what a plan stopped halfway had kept (the app closed or crashed during it): run before the next
/// change to `project`, under the packages' lock. Only a journal of this app's (in `journals`) written for this very
/// project folder, whose folder of copies is the very one it made and whose every item is a place a plan may change,
/// is acted on; anything else (a plan folder that came with a copied or imported project) is left alone. A plan whose
/// journal says it went through is only tidied away.
fn recover_plans(project: &Path, journals: &Path) -> anyhow::Result<()> {
    let Ok(entries) = std::fs::read_dir(journals) else { return Ok(()) };
    for e in entries.flatten() {
        let path = e.path();
        let Some(journal) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Journal>(&b).ok()) else { continue };
        if crate::discovery::key(&journal.project) != crate::discovery::key(project) {
            continue;
        }
        // The same folder it was written for, and its own folder of copies inside it: by identity, not by name.
        let staging_inside = journal.staging.parent().is_some_and(|p| crate::discovery::key(p) == crate::discovery::key(project))
            && journal.staging.file_name().is_some_and(|n| n.to_string_lossy().starts_with(".orbiters-plan-"));
        if !crate::owned::is_same(project, journal.project_id) || !staging_inside || !crate::owned::is_same(&journal.staging, journal.staging_id) {
            continue;
        }
        let kept = Kept { project: project.to_path_buf(), dir: journal.staging.clone(), journal_path: path.clone(), journal };
        if kept.journal.state == "committed" {
            kept.finish();
            continue;
        }
        anyhow::ensure!(
            kept.journal.state == "pending" && kept.journal.items.iter().all(|i| allowed(&i.path)),
            "{}",
            tr!("A package change stopped halfway and could not be fully put back: {problems}. What the project had is kept in {path}.", problems = tr("its record cannot be used"), path = kept.dir.display())
        );
        let mut kept = kept;
        let mut problems: Vec<String> = kept.put_back_manifest().err().map(|e| e.to_string()).into_iter().collect();
        problems.extend(kept.put_back());
        anyhow::ensure!(
            problems.is_empty(),
            "{}",
            tr!("A package change stopped halfway and could not be fully put back: {problems}. What the project had is kept in {path}.", problems = problems.join("; "), path = kept.dir.display())
        );
        kept.finish();
    }
    Ok(())
}

/// Whether `path` is a junction or another reparse point (Windows), which `symlink_metadata` does not always call a link.
fn is_link(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        std::fs::symlink_metadata(path).is_ok_and(|m| m.file_attributes() & REPARSE_POINT != 0)
    }
    #[cfg(not(windows))]
    {
        std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
    }
}

/// `from` copied to `to` (new) as it is, file times kept; a link inside is refused (it could not be put back as it was).
fn copy_tree(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::create_dir(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        let meta = std::fs::symlink_metadata(&source)?;
        if meta.file_type().is_symlink() || is_link(&source) {
            bail!("{}", tr!("{path} is a link", path = source.display()));
        } else if meta.is_dir() {
            copy_tree(&source, &target)?;
        } else {
            std::fs::copy(&source, &target)?;
            if let Ok(modified) = meta.modified() {
                let _ = std::fs::File::options().write(true).open(&target).and_then(|f| f.set_modified(modified));
            }
        }
    }
    Ok(())
}
/// Installs the latest releases of `names` from the Orbiters repository, adding it (or turning it back on) first.
pub async fn install_orbiters(root: &Path, path: &Path, names: &[&str]) -> anyhow::Result<()> {
    // Before the repository is added or turned on for it.
    may_change(path)?;
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

    fn plan_project(p: &Path) {
        std::fs::create_dir_all(p.join("Packages/com.a")).unwrap();
        std::fs::write(p.join("Packages/com.a/Mine.cs"), "mine").unwrap();
        std::fs::write(p.join("Packages/vpm-manifest.json"), r#"{"locked":{"com.a":{}}}"#).unwrap();
    }

    /// A plan stopped halfway (the app closed after a removal and an install) is put back at the next change, from its
    /// journal kept outside the project; one whose journal says it went through is only tidied away.
    #[test]
    fn a_plan_stopped_halfway_is_put_back_next_time() {
        let temp = tempfile::tempdir().unwrap();
        let (p, journals) = (temp.path().join("Project"), temp.path().join("journals"));
        plan_project(&p);
        let mut kept = Kept::begin(&p, &journals).unwrap();
        kept.take(&[PathBuf::from("Packages/com.a"), PathBuf::from("Packages/com.new")]).unwrap();
        // The removal happened, a new package came, then the app stopped.
        std::fs::remove_dir_all(p.join("Packages/com.a")).unwrap();
        std::fs::create_dir_all(p.join("Packages/com.new")).unwrap();
        std::fs::write(p.join("Packages/vpm-manifest.json"), "{}").unwrap();
        recover_plans(&p, &journals).unwrap();
        assert_eq!(std::fs::read_to_string(p.join("Packages/com.a/Mine.cs")).unwrap(), "mine");
        assert!(!p.join("Packages/com.new").exists(), "what the plan added goes");
        assert!(std::fs::read_to_string(p.join("Packages/vpm-manifest.json")).unwrap().contains("com.a"));
        assert!(!kept.dir.exists() && std::fs::read_dir(&journals).unwrap().next().is_none());
        // A plan that went through, its copies left by a failed tidy-up: nothing is put back.
        let kept = Kept::begin(&p, &journals).unwrap();
        let mut done = kept.journal.clone();
        done.state = "committed".into();
        replace_bytes(&kept.journal_path, &serde_json::to_vec(&done).unwrap()).unwrap();
        std::fs::write(p.join("Packages/vpm-manifest.json"), r#"{"new":1}"#).unwrap();
        recover_plans(&p, &journals).unwrap();
        assert_eq!(std::fs::read_to_string(p.join("Packages/vpm-manifest.json")).unwrap(), r#"{"new":1}"#);
        assert!(!kept.dir.exists());
    }

    /// Verifier round 2: a plan folder that came with a copied or imported project (its old in-project journal, or a
    /// journal of this app's naming the same place but written for another folder) is never acted on; a journal whose
    /// items leave the places a plan may change is refused before anything changes.
    #[test]
    fn a_journal_not_written_for_this_very_folder_is_never_acted_on() {
        let temp = tempfile::tempdir().unwrap();
        let (p, journals) = (temp.path().join("Project"), temp.path().join("journals"));
        plan_project(&p);
        // An older version's journal inside the project, with a destructive item.
        let foreign = p.join(".orbiters-plan-0000");
        std::fs::create_dir_all(foreign.join("entries/Assets")).unwrap();
        std::fs::write(foreign.join("plan.json"), r#"{"state":"pending","items":["Assets"]}"#).unwrap();
        std::fs::write(foreign.join("vpm-manifest.json"), "{}").unwrap();
        std::fs::create_dir_all(p.join("Assets")).unwrap();
        std::fs::write(p.join("Assets/work.txt"), "work").unwrap();
        recover_plans(&p, &journals).unwrap();
        assert!(p.join("Assets/work.txt").is_file());
        // A journal of ours for a project that was at this place before, its folder since replaced by a copy.
        let mut kept = Kept::begin(&p, &journals).unwrap();
        kept.take(&[PathBuf::from("Packages/com.a")]).unwrap();
        let journal = kept.journal_path.clone();
        let moved = temp.path().join("Original");
        std::fs::rename(&p, &moved).unwrap();
        plan_project(&p);
        std::fs::write(p.join("Packages/com.a/Mine.cs"), "newer work").unwrap();
        recover_plans(&p, &journals).unwrap();
        assert_eq!(std::fs::read_to_string(p.join("Packages/com.a/Mine.cs")).unwrap(), "newer work");
        assert!(journal.is_file(), "left for its own folder");
        // Its own folder again, but an item outside Packages and Assets: refused, nothing changed.
        std::fs::remove_dir_all(&p).unwrap();
        std::fs::rename(&moved, &p).unwrap();
        let mut bad = kept.journal.clone();
        bad.items.push(Item { path: PathBuf::from("ProjectSettings"), before: Before::Absent, restored: false });
        replace_bytes(&journal, &serde_json::to_vec(&bad).unwrap()).unwrap();
        assert!(recover_plans(&p, &journals).is_err());
        assert!(p.join("Packages/com.a/Mine.cs").is_file());
    }

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
