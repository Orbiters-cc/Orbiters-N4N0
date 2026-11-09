//! A package change that conflicts (an installed package's dependency range does not accept a version it installs)
//! made coherent instead of cancelled: the requested versions, then for each installed package whose range the
//! change breaks, its newest version that accepts what the plan installs, until nothing conflicts. A package with no
//! such version blocks the plan, named with the range it holds, unless the person chooses to remove it too. Nothing
//! is written here; `vpm::Change::Plan` applies a solved plan through the usual install path.
use orbiters_i18n::tr;
use crate::updates::Step;
use std::collections::HashMap;
use std::path::Path;
use vrc_get_vpm::io::DefaultProjectIo;
use vrc_get_vpm::unity_project::pending_project_changes::PackageChange;
use vrc_get_vpm::unity_project::AddPackageOperation;
use vrc_get_vpm::version::{PrereleaseAcceptance, Version, VersionRange};
use vrc_get_vpm::{PackageCollection, PackageInfo, UnityProject, VersionSelector};

/// A package the change would install at a version some installed packages do not accept.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Conflict {
    pub name: String,
    pub display_name: String,
    /// The version the change installs.
    pub version: String,
    /// Who needs another version, and the range each accepts.
    pub needed_by: Vec<Need>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Need {
    pub name: String,
    pub display_name: String,
    pub version: String,
    pub range: String,
}
/// A package no version of which works with the plan: it can be removed too (when nothing else needs it), or the
/// change given up.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Blocker {
    pub name: String,
    pub display_name: String,
    pub version: String,
    /// What it holds back: that package, the range it accepts and the version the plan needs.
    pub dependency: String,
    pub range: String,
    pub wanted: String,
    /// Nothing else that stays needs it and it was not asked for: removing it solves this.
    pub removable: bool,
}
/// A change that conflicted, made coherent: everything that has to change with it, why, and what still blocks it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Plan {
    /// What the person asked for.
    pub requested: Vec<(String, String)>,
    /// What the plan installs: that and the versions chosen to go with it.
    pub install: Vec<(String, String)>,
    /// Packages removed (blockers the person chose to remove).
    pub remove: Vec<String>,
    /// The conflicts the change alone had.
    pub conflicts: Vec<Conflict>,
    /// Every package that changes, with why (once solved).
    pub steps: Vec<Step>,
    pub blockers: Vec<Blocker>,
    /// Why no plan could be made at all (a package missing from the repositories, the Unity version).
    pub error: Option<String>,
    pub prerelease: bool,
}
impl Plan {
    pub fn solved(&self) -> bool {
        self.blockers.is_empty() && self.error.is_none() && !self.steps.is_empty()
    }
}

/// The conflict of a change `vpm::change` refused, with the plan that solves it: the error the desktop turns into
/// its conflict dialog.
#[derive(Debug)]
pub struct Conflicted {
    pub plan: Plan,
}
impl std::fmt::Display for Conflicted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.plan.conflicts.iter().map(|c| c.name.as_str()).collect();
        write!(f, "{}", tr!("These package changes conflict with installed packages: {names}. No changes were applied.", names = names.join(", ")))
    }
}
impl std::error::Error for Conflicted {}

fn accepts(range: &VersionRange, v: &Version) -> bool {
    range.match_pre(v, if v.is_pre() { PrereleaseAcceptance::Allow } else { PrereleaseAcceptance::Minimum })
}

/// Solves `requested` (exact versions) for `project`, removing `remove` too. See the module's notes.
pub async fn solve_with(project: &UnityProject, packages: &impl PackageCollection, requested: &[(String, String)], remove: &[String], prerelease: bool) -> Plan {
    let names = crate::updates::display_names(project, packages);
    let display = |n: &str| names.get(n).cloned().unwrap_or_else(|| n.to_string());
    let mut out = Plan { requested: requested.to_vec(), remove: remove.to_vec(), prerelease, ..Default::default() };
    let mut install: Vec<(String, String)> = requested.to_vec();
    let mut why: HashMap<String, String> = requested.iter().map(|(n, _)| (n.clone(), tr("asked for").to_string())).collect();
    let unity = project.unity_version();
    for round in 0..12 {
        let mut infos = vec![];
        for (n, v) in &install {
            let Ok(version) = v.parse::<Version>() else {
                out.error = Some(tr!("{v} is not a version", v = v));
                return out;
            };
            match packages.find_package_by_name(n, VersionSelector::specific_version(&version)) {
                Some(p) => infos.push(p),
                None => {
                    out.error = Some(tr!("{package} {v} is in none of your repositories", package = display(n), v = v));
                    return out;
                }
            }
        }
        let changes = match project.add_package_request(packages, &infos, AddPackageOperation::AutoDetected, prerelease).await {
            Ok(c) => c,
            Err(e) => {
                out.error = Some(e.to_string());
                return out;
            }
        };
        // The versions the plan ends with, and their dependency ranges: what it installs over what is locked.
        let mut planned: HashMap<String, Version> = project.locked_packages().map(|l| (l.name().to_string(), l.version().clone())).collect();
        let mut deps: HashMap<String, Vec<(String, VersionRange)>> = project
            .locked_packages()
            .map(|l| (l.name().to_string(), l.dependencies().iter().map(|(k, v)| (k.to_string(), v.clone())).collect()))
            .collect();
        for (name, change) in changes.package_changes() {
            if let PackageChange::Install(i) = change {
                if let Some(p) = i.install_package() {
                    planned.insert(name.to_string(), p.version().clone());
                    deps.insert(name.to_string(), p.vpm_dependencies().iter().map(|(k, v)| (k.to_string(), v.clone())).collect());
                }
            }
        }
        for r in remove {
            planned.remove(r);
            deps.remove(r);
        }
        let range_of = |requirer: &str, dep: &str| deps.get(requirer).and_then(|d| d.iter().find(|(k, _)| k == dep)).map(|(_, r)| r.to_string()).unwrap_or_default();
        let mut conflicts: Vec<(String, Vec<String>, bool)> = changes
            .conflicts()
            .iter()
            .map(|(d, info)| {
                let mut cs: Vec<String> = info.conflicting_packages().iter().map(|c| c.to_string()).filter(|c| !remove.contains(c)).collect();
                cs.sort();
                (d.to_string(), cs, info.conflicts_with_unity())
            })
            .filter(|(d, cs, unity)| !remove.contains(d) && (!cs.is_empty() || *unity))
            .collect();
        conflicts.sort();
        if round == 0 {
            out.conflicts = conflicts
                .iter()
                .map(|(d, cs, _)| Conflict {
                    name: d.clone(),
                    display_name: display(d),
                    version: planned.get(d).map(ToString::to_string).unwrap_or_default(),
                    needed_by: cs
                        .iter()
                        .map(|c| Need {
                            name: c.clone(),
                            display_name: display(c),
                            version: project.get_locked(c).map(|l| l.version().to_string()).unwrap_or_default(),
                            range: project.get_locked(c).and_then(|l| l.dependencies().get(d.as_str()).map(ToString::to_string)).unwrap_or_else(|| range_of(c, d)),
                        })
                        .collect(),
                })
                .collect();
        }
        if conflicts.is_empty() {
            let mut steps: Vec<Step> = vec![];
            for (name, change) in changes.package_changes() {
                let from = project.get_locked(name).map(|l| l.version().to_string());
                let to = match change {
                    PackageChange::Install(i) => i.install_package().map(|p| p.version().to_string()),
                    PackageChange::Remove(_) => None,
                };
                if from == to || remove.iter().any(|r| r.as_str() == name.as_ref()) {
                    continue;
                }
                let reason = why.get(name.as_ref()).cloned().or_else(|| {
                    // Brought along: who in the plan depends on it.
                    let mut by: Vec<&String> = deps.iter().filter(|(p, d)| p.as_str() != name.as_ref() && d.iter().any(|(k, _)| k == name.as_ref())).map(|(p, _)| p).collect();
                    by.sort_by_key(|p| (!install.iter().any(|(n, _)| n == *p), (*p).clone()));
                    by.first().map(|p| tr!("needed by {package} {version}", package = display(p), version = planned[p.as_str()]))
                });
                steps.push(Step { name: name.to_string(), display_name: display(name), from, to, why: reason });
            }
            for r in remove {
                if let Some(l) = project.get_locked(r) {
                    steps.push(Step {
                        name: r.clone(),
                        display_name: display(r),
                        from: Some(l.version().to_string()),
                        to: None,
                        why: Some(tr("removed: no version of it works with the others").into()),
                    });
                }
            }
            steps.sort_by_key(|s| (requested.iter().position(|(n, _)| *n == s.name).unwrap_or(usize::MAX), s.to.is_none(), s.name.clone()));
            out.steps = steps;
            out.install = install;
            out.blockers.clear();
            return out;
        }
        out.blockers.clear();
        let mut progress = false;
        for (d, requirers, unity_conflict) in &conflicts {
            let wanted = planned.get(d).map(ToString::to_string).unwrap_or_default();
            if requirers.is_empty() && *unity_conflict {
                out.error = Some(tr!("{package} {wanted} needs another Unity version than {unity}", package = display(d), wanted = wanted, unity = unity));
                return out;
            }
            for c in requirers {
                let current = project.get_locked(c).map(|l| l.version().to_string()).unwrap_or_default();
                let fixed = install.iter().any(|(n, _)| n == c);
                let selector = VersionSelector::latest_for(Some(unity), prerelease);
                let mut candidates: Vec<PackageInfo<'_>> = packages.find_packages(c).filter(|p| selector.satisfies(p.package_json())).collect();
                candidates.sort_by(|a, b| b.version().cmp(a.version()));
                // Its newest version accepting every version the plan installs.
                let fits = |p: &PackageInfo<'_>| p.vpm_dependencies().iter().all(|(dep, range)| planned.get(dep.as_ref()).is_none_or(|v| accepts(range, v)));
                match candidates.iter().find(|p| fits(p)).filter(|_| !fixed) {
                    Some(p) => {
                        let v = p.version().to_string();
                        why.insert(c.clone(), tr!("{current} accepts {package} {range} only; {v} works with {wanted}", current = current, package = display(d), range = range_of(c, d), v = v, wanted = wanted));
                        install.push((c.clone(), v));
                        progress = true;
                    }
                    None if out.blockers.iter().any(|b| b.name == *c) => {}
                    None => {
                        // Removable when no other package that stays needs it.
                        let needed = deps.iter().any(|(p, ds)| p != c && ds.iter().any(|(dep, _)| dep == c));
                        out.blockers.push(Blocker {
                            name: c.clone(),
                            display_name: display(c),
                            version: current,
                            dependency: display(d),
                            range: range_of(c, d),
                            wanted: wanted.clone(),
                            removable: !needed && !requested.iter().any(|(n, _)| n == c),
                        });
                    }
                }
            }
        }
        if !progress {
            out.install = install;
            return out;
        }
    }
    out.error = Some(tr("No coherent set of versions was found.").into());
    out
}

/// [`solve_with`] for the project at `path`, against the repositories as last downloaded.
pub async fn solve(root: &Path, path: &Path, requested: &[(String, String)], remove: &[String], prerelease: bool) -> anyhow::Result<Plan> {
    let (_, packages) = crate::vpm::collection(root, false).await?;
    let project = UnityProject::load(DefaultProjectIo::new(path.into())).await?;
    Ok(solve_with(&project, &packages, requested, remove, prerelease).await)
}
