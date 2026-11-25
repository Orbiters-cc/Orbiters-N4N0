//! The repositories' listings as they were last downloaded, read once for each version of them (PRJ-20): each look at a project's
//! packages read every listing three or four times, twice only to learn that it is one. What is read is kept while a project is shown
//! ([`hold`]) and let go of after.
use crate::vpm::Package;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;
use vrc_get_vpm::environment::{PackageCollection, Settings};
use vrc_get_vpm::io::DefaultEnvironmentIo;

/// What tells one version of a file from another: its length and when it was written. `None`: it is not there.
type Stamp = Option<(u64, SystemTime)>;

pub(crate) fn stamp(path: &Path) -> Stamp {
    std::fs::metadata(path).ok().and_then(|m| Some((m.len(), m.modified().ok()?)))
}

fn lock<T>(of: &Mutex<T>) -> MutexGuard<'_, T> {
    of.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether each listing looked at is one ([`crate::vpm::unusable_cache`]), by its stamp.
static UNUSABLE: Mutex<Option<HashMap<PathBuf, (Stamp, bool)>>> = Mutex::new(None);

/// Whether the file at `path` is there and `is` says it is not a listing, asked once for each version of it.
pub(crate) fn unusable(path: &Path, is: impl FnOnce() -> bool) -> bool {
    let now = stamp(path);
    if now.is_none() {
        return false;
    }
    if let Some((then, unusable)) = lock(&UNUSABLE).get_or_insert_default().get(path) {
        if *then == now {
            return *unusable;
        }
    }
    let answer = is();
    // (Written meanwhile: asked again next time.)
    if stamp(path) == now {
        lock(&UNUSABLE).get_or_insert_default().insert(path.into(), (now, answer));
    }
    answer
}

/// A package's websites, which vrc-get's manifests leave out, in a repository's listing: by package and version.
type Sites = HashMap<(String, String), Package>;

/// Every listing of an environment, read.
pub(crate) struct Listings {
    pub packages: PackageCollection,
    sites: Mutex<HashMap<String, Arc<Sites>>>,
}

impl Listings {
    pub fn new(packages: PackageCollection) -> Arc<Self> {
        Arc::new(Self { packages, sites: Mutex::default() })
    }

    /// The websites `p`'s listing gives it.
    pub fn websites(&self, p: &vrc_get_vpm::PackageInfo<'_>) -> Option<Package> {
        let r = p.repo()?;
        let key = r.url().map(ToString::to_string).or_else(|| r.id().map(ToOwned::to_owned)).unwrap_or_default();
        let known = lock(&self.sites).get(&key).cloned();
        let sites = known.unwrap_or_else(|| {
            let listing = serde_json::to_value(r.repo()).unwrap_or_default();
            let mut sites = Sites::new();
            for (name, package) in listing["packages"].as_object().into_iter().flatten() {
                for (version, manifest) in package["versions"].as_object().into_iter().flatten() {
                    let mut row = Package::default();
                    row.websites_from(manifest);
                    if row.author_url.is_some() || row.site_url.is_some() {
                        sites.insert((name.clone(), version.clone()), row);
                    }
                }
            }
            let sites = Arc::new(sites);
            lock(&self.sites).insert(key, sites.clone());
            sites
        });
        sites.get(&(p.name().to_string(), p.version().to_string())).cloned()
    }
}

struct Kept {
    files: Vec<(PathBuf, Stamp)>,
    listings: Arc<Listings>,
}

static HELD: AtomicBool = AtomicBool::new(false);
static KEPT: Mutex<Vec<(PathBuf, Kept)>> = Mutex::new(Vec::new());
/// More than one environment is only a test's.
const ENVIRONMENTS: usize = 2;

/// Whether what is read of the listings is kept for the next look (while a project is shown): tens of megabytes where there are many
/// repositories.
pub fn hold(on: bool) {
    if HELD.swap(on, Ordering::Relaxed) && !on {
        lock(&KEPT).clear();
    }
}

/// The files the listings of the environment in `here` are read from.
fn files(here: &Path, settings: &Settings, caches: &[PathBuf]) -> Vec<(PathBuf, Stamp)> {
    let mut files = vec![here.join("settings.json"), here.join("vrc-get/settings.json")];
    files.extend(caches.iter().cloned());
    files.extend(settings.get_user_repos().iter().map(|r| here.join(r.local_path())));
    files.extend(settings.user_package_folders().iter().map(|p| p.join("package.json")));
    files.into_iter().map(|p| (p.clone(), stamp(&p))).collect()
}

/// The listings of the environment in `here` (its `caches`), read unless those kept are of the same files.
pub(crate) async fn load(here: &Path, settings: &Settings, caches: &[PathBuf], io: &DefaultEnvironmentIo) -> std::io::Result<Arc<Listings>> {
    let now = files(here, settings, caches);
    if let Some((_, kept)) = lock(&KEPT).iter().find(|(p, k)| p == here && k.files == now) {
        return Ok(kept.listings.clone());
    }
    let listings = Listings::new(PackageCollection::load_cache(settings, io).await?);
    keep(here, settings, caches, now, &listings);
    Ok(listings)
}

/// Keeps `listings`, just read or downloaded, when they are those of files that did not change meanwhile.
fn keep(here: &Path, settings: &Settings, caches: &[PathBuf], of: Vec<(PathBuf, Stamp)>, listings: &Arc<Listings>) {
    if !HELD.load(Ordering::Relaxed) || files(here, settings, caches) != of {
        return;
    }
    let mut kept = lock(&KEPT);
    kept.retain(|(p, _)| p != here);
    kept.truncate(ENVIRONMENTS - 1);
    kept.insert(0, (here.into(), Kept { files: of, listings: listings.clone() }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn listing(version: &str) -> serde_json::Value {
        let package = json!({ "name": "test.a", "version": version, "displayName": "A", "url": "http://127.0.0.1:9/a.zip", "author": { "name": "A", "url": "https://author.example" } });
        json!({ "repo": { "id": "r", "name": "R", "url": "http://127.0.0.1:9/index.json", "packages": { "test.a": { "versions": { version: package } } } }, "headers": {} })
    }

    /// PRJ-20: the same listings are read once, however many looks there are; another version of one is read again, and nothing is
    /// kept once no project is shown.
    #[test]
    fn a_listing_is_read_once_for_each_version_of_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("vcc");
        let file = root.join("Repos/r.json");
        crate::write_json(&root.join("vrc-get/settings.json"), &json!({ "ignoreOfficialRepository": true, "ignoreCuratedRepository": true })).unwrap();
        crate::write_json(&file, &listing("1.0.0")).unwrap();
        crate::write_json(&root.join("settings.json"), &json!({ "userRepos": [{ "localPath": file, "url": "http://127.0.0.1:9/index.json", "id": "r", "headers": {} }] })).unwrap();
        let rt = crate::runtime().unwrap();
        let look = || rt.block_on(crate::vpm::collection(&root, false)).unwrap().1.inner;
        let versions = |l: &Arc<Listings>| {
            use vrc_get_vpm::PackageCollection as _;
            l.packages.get_all_packages().map(|p| p.version().to_string()).collect::<Vec<_>>()
        };
        hold(true);
        let first = look();
        assert_eq!(versions(&first), ["1.0.0"]);
        assert!(Arc::ptr_eq(&first, &look()), "the same listings");
        // The websites are read once too, and are what the listing says.
        let view = || rt.block_on(crate::vpm::view(&root, None, false)).unwrap().packages[0].author_url.clone();
        assert_eq!((view(), view()), (Some("https://author.example".into()), Some("https://author.example".into())));
        // Creator Companion downloads a newer one.
        crate::write_json(&file, &listing("1.10.0")).unwrap();
        let second = look();
        assert_eq!(versions(&second), ["1.10.0"]);
        assert!(Arc::ptr_eq(&second, &look()));
        hold(false);
        assert!(!Arc::ptr_eq(&second, &look()), "not kept once no project is shown");
        assert_eq!(Arc::strong_count(&second), 1);
    }

    #[test]
    fn a_file_is_looked_at_once_for_each_version_of_it() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("r.json");
        assert!(!unusable(&file, || panic!("not there")));
        std::fs::write(&file, "{").unwrap();
        assert!(unusable(&file, || true));
        assert!(unusable(&file, || panic!("the same file")));
        std::fs::write(&file, "{}").unwrap();
        assert!(!unusable(&file, || false));
    }
}
