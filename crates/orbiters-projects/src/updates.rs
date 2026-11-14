//! Which of a project's packages have a newer version it can take, found with the same resolver the version lists
//! install with (`vpm::change`): every candidate is tried as the version list would install it, keeping only those
//! the other packages' declared dependency ranges and the project's Unity version allow, without changing the
//! VRChat SDK behind the person's back. Nothing is written: applying an update is `vpm::Change::Install` with the
//! versions found here.
use orbiters_i18n::tr;
use std::collections::HashMap;
use std::path::Path;
use vrc_get_vpm::io::DefaultProjectIo;
use vrc_get_vpm::unity_project::pending_project_changes::PackageChange;
use vrc_get_vpm::unity_project::AddPackageOperation;
use vrc_get_vpm::{PackageCollection, PackageInfo, UnityProject, VersionSelector};

/// Versions tried per package, newest first, before it is said to have none it can take.
const TRIES: usize = 8;

/// One package's change in a plan: from its installed version to another (`from` none: added as a dependency; `to`
/// none: removed).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Step {
    pub name: String,
    pub display_name: String,
    pub from: Option<String>,
    pub to: Option<String>,
    /// Why it changes, in a plan solving a conflict ([`coherent`]).
    pub why: Option<String>,
}
/// A newer version a package can take, and everything installing it changes (itself first).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Update {
    pub name: String,
    pub display_name: String,
    pub from: String,
    pub to: String,
    /// One of the VRChat SDK's packages: updated only from its own row, never by Update all.
    pub sdk: bool,
    pub steps: Vec<Step>,
}
impl Update {
    /// The other packages this update changes.
    pub fn also(&self) -> impl Iterator<Item = &Step> {
        self.steps.iter().filter(|s| s.name != self.name)
    }
}
/// A newer version the project cannot take as it is, and why.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Held {
    pub name: String,
    pub display_name: String,
    pub version: String,
    pub why: String,
}
/// Update all: the versions asked for at once and everything that changes.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Plan {
    pub targets: Vec<(String, String)>,
    pub steps: Vec<Step>,
    /// Updates not in it: the VRChat SDK's, and those that cannot go together with the others.
    pub left_out: Vec<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Updates {
    pub updates: Vec<Update>,
    pub held: Vec<Held>,
    pub all: Plan,
}
impl Updates {
    pub fn of(&self, name: &str) -> Option<&Update> {
        self.updates.iter().find(|u| u.name == name)
    }
}
/// The answer for releases only and with pre-releases (the panel's switch picks one; no new check when it turns).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Check {
    pub stable: Updates,
    pub prerelease: Updates,
    /// Why the project was not checked (its manifest is one upstream would rewrite on reading it).
    pub skipped: Option<String>,
}
impl Check {
    pub fn get(&self, prerelease: bool) -> &Updates {
        if prerelease {
            &self.prerelease
        } else {
            &self.stable
        }
    }
}

/// The VRChat SDK's packages (base, avatars, worlds, the resolver).
pub fn is_sdk(name: &str) -> bool {
    name.starts_with("com.vrchat.")
}

/// Checks the project at `path` against the repositories as they were last downloaded (`vpm::view` with `refresh`
/// downloads them again). Reads only.
pub async fn check(root: &Path, path: &Path) -> anyhow::Result<Check> {
    // vrc-get's UnityProject::load rewrites a manifest holding the "3.2-3.8" range spelling an old vrc-get wrote; an
    // automatic check must not change the project, so such a project is left to an explicit change.
    let manifest = std::fs::read(path.join("Packages/vpm-manifest.json"))?;
    const REWRITTEN: &[u8] = br#""3.2-3.8""#;
    if manifest.windows(REWRITTEN.len()).any(|w| w == REWRITTEN) {
        return Ok(Check { skipped: Some(tr("Its package list uses an old range spelling; change a package once to check for updates.").into()), ..Default::default() });
    }
    let (_, packages) = crate::vpm::collection(root, false).await?;
    let project = UnityProject::load(DefaultProjectIo::new(path.into())).await?;
    Ok(Check { stable: find(&project, &packages, false).await, prerelease: find(&project, &packages, true).await, skipped: None })
}

/// Every installed package's newest version the project can take (releases only unless `prerelease`), what holds the
/// others back, and Update all's plan.
pub async fn find(project: &UnityProject, packages: &impl PackageCollection, prerelease: bool) -> Updates {
    let unity = project.unity_version();
    let names = display_names(project, packages);
    let mut locked: Vec<(String, vrc_get_vpm::version::Version)> = project.locked_packages().map(|l| (l.name().to_string(), l.version().clone())).collect();
    locked.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = Updates::default();
    for (name, current) in &locked {
        let selector = VersionSelector::latest_for(Some(unity), prerelease);
        let mut candidates: Vec<PackageInfo<'_>> =
            packages.find_packages(name).filter(|p| p.version() > current && selector.satisfies(p.package_json())).collect();
        candidates.sort_by(|a, b| b.version().cmp(a.version()));
        candidates.dedup_by(|a, b| a.version() == b.version());
        let Some(newest) = candidates.first().copied() else { continue };
        let mut first_why = None;
        let mut found = None;
        for candidate in candidates.iter().take(TRIES) {
            match attempt(project, packages, &[*candidate], prerelease, &names).await {
                Ok(steps) => {
                    // A package's update that would also move the VRChat SDK is not offered: the SDK changes from
                    // its own row only.
                    if !is_sdk(name) {
                        if let Some(sdk) = steps.iter().find(|s| is_sdk(&s.name)) {
                            first_why.get_or_insert_with(|| match sdk.to.as_deref() {
                                Some(version) => tr!("needs {package} {version}", package = sdk.display_name, version = version),
                                None => tr!("needs {package} removed", package = sdk.display_name),
                            });
                            continue;
                        }
                    }
                    found = Some((candidate.version().to_string(), steps));
                    break;
                }
                Err(why) => {
                    first_why.get_or_insert(why);
                }
            }
        }
        let display = names.get(name).cloned().unwrap_or_else(|| name.clone());
        match found {
            Some((to, steps)) => {
                if to != newest.version().to_string() {
                    out.held.push(Held {
                        name: name.clone(),
                        display_name: display.clone(),
                        version: newest.version().to_string(),
                        why: first_why.unwrap_or_default(),
                    });
                }
                out.updates.push(Update { name: name.clone(), display_name: display, from: current.to_string(), to, sdk: is_sdk(name), steps });
            }
            None => out.held.push(Held {
                name: name.clone(),
                display_name: display,
                version: newest.version().to_string(),
                why: first_why.unwrap_or_else(|| tr("cannot be installed with the other packages").into()),
            }),
        }
    }
    out.all = plan(project, packages, &out.updates, prerelease, &names).await;
    out
}

/// Update all: every update but the SDK's, added one after the other while the whole still resolves without
/// conflicts and without moving the SDK.
async fn plan(project: &UnityProject, packages: &impl PackageCollection, updates: &[Update], prerelease: bool, names: &HashMap<String, String>) -> Plan {
    let mut chosen: Vec<PackageInfo<'_>> = vec![];
    let mut out = Plan::default();
    for u in updates {
        if u.sdk {
            out.left_out.push(u.name.clone());
            continue;
        }
        let Ok(version) = u.to.parse() else { continue };
        let Some(info) = packages.find_package_by_name(&u.name, VersionSelector::specific_version(&version)) else {
            out.left_out.push(u.name.clone());
            continue;
        };
        let mut trial = chosen.clone();
        trial.push(info);
        match attempt(project, packages, &trial, prerelease, names).await {
            Ok(steps) if !steps.iter().any(|s| is_sdk(&s.name)) => {
                chosen = trial;
                out.steps = steps;
            }
            _ => out.left_out.push(u.name.clone()),
        }
    }
    out.targets = chosen.iter().map(|p| (p.name().to_string(), p.version().to_string())).collect();
    // The packages asked for first, in the list's order, then what they bring along.
    out.steps.sort_by_key(|s| (out.targets.iter().position(|(n, _)| *n == s.name).unwrap_or(usize::MAX), s.name.clone()));
    out
}

/// What installing `requested` changes, as the version list installs it (`vpm::change`), or why it cannot.
async fn attempt<'env>(
    project: &UnityProject,
    packages: &'env impl PackageCollection,
    requested: &[PackageInfo<'env>],
    prerelease: bool,
    names: &HashMap<String, String>,
) -> Result<Vec<Step>, String> {
    let changes = project.add_package_request(packages, requested, AddPackageOperation::AutoDetected, prerelease).await.map_err(|e| e.to_string())?;
    if !changes.conflicts().is_empty() {
        let mut with: Vec<String> = vec![];
        let mut unity = false;
        for info in changes.conflicts().values() {
            unity |= info.conflicts_with_unity();
            for n in info.conflicting_packages() {
                let n = names.get(n.as_ref()).cloned().unwrap_or_else(|| n.to_string());
                if !with.contains(&n) {
                    with.push(n);
                }
            }
        }
        with.sort();
        return Err(match (with.is_empty(), unity) {
            (true, _) => tr!("needs another Unity version than {unity}", unity = project.unity_version()),
            (false, _) => tr!("conflicts with {packages}", packages = with.join(", ")),
        });
    }
    let display = |n: &str| names.get(n).cloned().unwrap_or_else(|| n.to_string());
    let mut steps: Vec<Step> = changes
        .package_changes()
        .iter()
        .filter_map(|(name, change)| {
            let from = project.get_locked(name).map(|l| l.version().to_string());
            let to = match change {
                PackageChange::Install(i) => Some(i.install_package()?.version().to_string()),
                PackageChange::Remove(_) => None,
            };
            (from != to).then(|| Step { name: name.to_string(), display_name: display(name), from, to, why: None })
        })
        .collect();
    let first: Vec<&str> = requested.iter().map(|p| p.name()).collect();
    steps.sort_by_key(|s| (first.iter().position(|n| *n == s.name).unwrap_or(usize::MAX), s.name.clone()));
    Ok(steps)
}

/// Packages' names as people read them: the installed package's, else the repositories' newest.
pub(crate) fn display_names(project: &UnityProject, packages: &impl PackageCollection) -> HashMap<String, String> {
    let mut out: HashMap<String, (vrc_get_vpm::version::Version, String)> = HashMap::new();
    for p in packages.get_all_packages() {
        if let Some(d) = p.display_name() {
            let e = out.entry(p.name().to_string()).or_insert_with(|| (p.version().clone(), d.to_string()));
            if p.version() > &e.0 {
                *e = (p.version().clone(), d.to_string());
            }
        }
    }
    let mut out: HashMap<String, String> = out.into_iter().map(|(k, (_, d))| (k, d)).collect();
    for (name, manifest) in project.installed_packages() {
        if let Some(d) = manifest.display_name() {
            out.insert(name.to_string(), d.to_string());
        }
    }
    // A folder of the project's Packages that no repository installed (a package in development, a copy put there by
    // hand) is named by its own package.json.
    for manifest in project.unlocked_packages().iter().filter_map(|(_, m)| m.as_ref()) {
        if let Some(d) = manifest.display_name() {
            out.insert(manifest.name().to_string(), d.to_string());
        }
    }
    out
}
