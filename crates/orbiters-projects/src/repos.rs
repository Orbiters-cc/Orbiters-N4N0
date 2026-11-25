//! Orbiters' own VPM environment, a folder of its data folder (`vpm`): the settings vrc-get works with, the repositories' listings
//! and the packages it downloads. Its settings are made from Creator Companion's and ALCOM's, read and never written, and from
//! what was added, removed, turned on and off in Orbiters (`vpm/repositories.json`). Listings and packages those programs
//! downloaded already are copied here rather than downloaded again.
use crate::{own, theirs};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

/// What was chosen in Orbiters about repositories.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Chosen {
    #[serde(default)]
    pub added: Vec<Added>,
    /// Ids and URLs of theirs removed here.
    #[serde(default)]
    pub removed: BTreeSet<String>,
    /// Ids turned off and on here: whatever ALCOM has.
    #[serde(default)]
    pub off: BTreeSet<String>,
    #[serde(default)]
    pub on: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Added {
    pub url: String,
    pub name: Option<String>,
    pub id: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Its listing's file in `Repos`.
    pub file: String,
}

/// Orbiters' environment for the Creator Companion folder `root`.
pub(crate) fn dir(root: &Path) -> PathBuf {
    own::dir(root).join("vpm")
}

fn file(root: &Path) -> PathBuf {
    dir(root).join("repositories.json")
}

pub(crate) fn chosen(root: &Path) -> Chosen {
    own::read(&file(root)).unwrap_or_default()
}

pub(crate) fn choose(root: &Path, edit: impl FnOnce(&mut Chosen)) -> anyhow::Result<()> {
    own::change(&file(root), Chosen::default, edit)
}

/// The ids of the repositories left out: ALCOM's choice, then Orbiters' own.
pub(crate) fn hidden(root: &Path) -> HashSet<String> {
    let chosen = chosen(root);
    theirs::hidden_repositories(root).into_iter().chain(chosen.off).filter(|id| !chosen.on.contains(id)).collect()
}

/// Their settings as last read: what they are while they cannot be (in use, damaged, their folder gone).
fn their_settings(root: &Path) -> Value {
    let kept = dir(root).join("their-settings.json");
    match theirs::settings(root) {
        Ok(Some(mut v)) => {
            // Their projects are the gallery's (`gallery`); nothing of the resolver's.
            if let Some(o) = v.as_object_mut() {
                for other in ["userProjects", "windowSizeData", "lastUIState"] {
                    o.remove(other);
                }
            }
            let _ = own::change(&kept, || Value::Null, |now: &mut Value| *now = v.clone());
            v
        }
        _ => own::read(&kept).filter(Value::is_object).unwrap_or_else(|| json!({})),
    }
}

/// The id of the repository of theirs at `url`, as their settings have it.
pub(crate) fn their_repository(root: &Path, url: &str) -> Option<String> {
    let settings = their_settings(root);
    settings["userRepos"].as_array()?.iter().find(|r| r["url"].as_str() == Some(url))?["id"].as_str().map(ToOwned::to_owned)
}

/// The ids and URLs of their repositories.
pub(crate) fn their_repositories(root: &Path) -> HashSet<String> {
    let settings = their_settings(root);
    let all = settings["userRepos"].as_array().into_iter().flatten();
    all.flat_map(|r| [&r["id"], &r["url"]]).filter_map(|v| v.as_str().map(ToOwned::to_owned)).collect()
}

/// Where the listing of a repository of theirs, kept at `theirs`, is kept here.
fn listing(theirs: &Path) -> String {
    let name = theirs.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    format!("theirs-{}-{name}", own::short(&crate::discovery::key(theirs)))
}

/// The files of theirs that repositories without an address are in, by where they are kept here.
pub(crate) fn files(root: &Path) -> BTreeMap<PathBuf, PathBuf> {
    let here = dir(root).join("Repos");
    let settings = their_settings(root);
    let all = settings["userRepos"].as_array().into_iter().flatten().filter(|r| !r["url"].is_string());
    all.filter_map(|r| r["localPath"].as_str().map(|p| root.join(p))).map(|at| (here.join(listing(&at)), at)).collect()
}

/// Makes the environment's settings from theirs and what was chosen here, with the listings downloaded already.
pub(crate) fn prepare(root: &Path) -> anyhow::Result<PathBuf> {
    let here = dir(root);
    let _held = own::lock(&here.join("environment"))?;
    let chosen = chosen(root);
    let mut settings = their_settings(root);
    let mut listings = vec![(root.join("Repos/vrc-official.json"), here.join("Repos/vrc-official.json")), (root.join("Repos/vrc-curated.json"), here.join("Repos/vrc-curated.json"))];
    let mut repos: Vec<Value> = vec![];
    for mut r in settings["userRepos"].as_array().cloned().unwrap_or_default() {
        let is_removed = |k: &str| r[k].as_str().is_some_and(|v| chosen.removed.contains(v));
        if is_removed("id") || is_removed("url") {
            continue;
        }
        let Some(at) = r["localPath"].as_str().map(|p| root.join(p)) else { continue };
        // A repository in a file is read from a copy too: one that says where it comes from is refreshed there.
        let own = here.join("Repos").join(listing(&at));
        r["localPath"] = json!(own);
        listings.push((at, own));
        repos.push(r);
    }
    for a in &chosen.added {
        let same = |r: &Value| r["url"].as_str() == Some(&a.url) || (a.id.is_some() && r["id"].as_str() == a.id.as_deref());
        if !repos.iter().any(same) {
            repos.push(json!({ "localPath": here.join("Repos").join(&a.file), "name": a.name, "url": a.url, "id": a.id, "headers": a.headers }));
        }
    }
    settings["userRepos"] = Value::Array(repos);
    write(&here.join("settings.json"), &settings)?;
    if let Ok(v) = theirs::json(&root.join("vrc-get/settings.json")) {
        write(&here.join("vrc-get/settings.json"), &v.unwrap_or_else(|| json!({})))?;
    }
    crate::disk::create_dir_all(here.join("Repos"))?;
    for (from, to) in listings {
        newer(&from, &to);
    }
    Ok(here)
}

fn write(file: &Path, value: &Value) -> anyhow::Result<()> {
    if theirs::json(file).ok().flatten().as_ref() == Some(value) {
        return Ok(());
    }
    crate::write_json(file, value)
}

/// Copies a listing they downloaded, when it is whole and newer than the one here.
fn newer(from: &Path, to: &Path) {
    let when = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let Some(theirs) = when(from) else { return };
    if !crate::vpm::unusable_cache(to) && when(to).is_some_and(|here| here >= theirs) {
        return;
    }
    let Ok(Some(bytes)) = own::steady(from) else { return };
    if crate::vpm::is_listing(&bytes) {
        let _ = crate::vpm::replace_bytes(to, &bytes);
    }
}

/// Copies the archive of a package they downloaded, so that it is not asked of its repository again (which may be gone). What it
/// holds is checked against the listing as a download is.
pub(crate) fn archive(root: &Path, name: &str, version: &str) {
    let at = Path::new("Repos").join(name).join(format!("vrc-get-{name}-{version}.zip")); // i18n: not shown (a file's name)
    let (from, to) = (root.join(&at), dir(root).join(&at));
    if name.contains(['/', '\\']) || name == ".." || to.is_file() || !from.is_file() {
        return;
    }
    let Some(parent) = to.parent() else { return };
    let sum = |p: &Path| p.with_extension("zip.sha256");
    let copied = crate::disk::create_dir_all(parent).and_then(|()| crate::disk::copy(sum(&from), sum(&to))).and_then(|_| crate::disk::copy(&from, &to));
    if copied.is_err() {
        let _ = crate::disk::remove_file(&to);
        let _ = crate::disk::remove_file(sum(&to));
    }
}
