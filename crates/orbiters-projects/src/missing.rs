//! Packages a project needs that none of the person's package repositories list ("Following dependencies are not
//! found: …", from the resolver): which repository provides each, so the panel can add them and resolve again.
//!
//! Looked up, in order, in Orbiters' Known VPM catalog (`GET /avatar-assets/known-dependencies`: the approved
//! packages of the approved repositories with their versions, the repositories' names from `GET /vpm/known`), the
//! person's own repositories turned off on this computer, then the listings creators publish on Orbiters
//! (`GET /vpm/public`, each listing's feed read). A pinned version (`=1.1.5`) is found only in a repository listing
//! that version; the Known VPM catalog keeps a package's 40 newest versions in its supported range, so a repository
//! it names without the version is read itself before saying it does not have it.
use orbiters_core::account::{Api, Server};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use vrc_get_vpm::version::{PrereleaseAcceptance, Version, VersionRange};

const NOT_FOUND: &str = "Following dependencies are not found: ";
/// Orbiters listings read when the catalog and the person's repositories had nothing.
const LISTINGS_READ: usize = 40;

/// A package the project needs and the versions it accepts (the resolver's range, `=1.1.5` for a locked one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    pub name: String,
    pub range: String,
}
impl Missing {
    /// The exact version a locked package needs, when the range is one.
    pub fn exact(&self) -> Option<&str> {
        let r = self.range.trim().strip_prefix('=')?.trim();
        (!r.is_empty() && !r.contains(' ')).then_some(r)
    }
}

/// The packages a resolver error says no repository has; none for any other error.
pub fn parse(error: &str) -> Vec<Missing> {
    let Some(at) = error.find(NOT_FOUND) else { return vec![] };
    let list = error[at + NOT_FOUND.len()..].lines().next().unwrap_or_default();
    list.split(", ")
        .filter_map(|item| {
            let (name, range) = item.trim().split_once('@')?;
            (!name.is_empty()).then(|| Missing { name: name.trim().into(), range: range.trim().into() })
        })
        .collect()
}

/// Whether `version` is one `range` accepts, as the resolver reads it (pre-releases only when the range names one).
pub fn accepts(range: &str, version: &str) -> bool {
    let (Ok(range), Ok(version)) = (range.parse::<VersionRange>(), version.parse::<Version>()) else { return false };
    let pre = if range.contains_pre() { PrereleaseAcceptance::Allow } else { PrereleaseAcceptance::Minimum };
    range.match_pre(&version, pre)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Orbiters' Known VPM catalog.
    Known,
    /// One of the person's repositories, turned off on this computer.
    TurnedOff,
    /// A listing a creator publishes on Orbiters.
    Orbiters,
}
/// A repository and the versions of each package it lists.
#[derive(Clone, Debug)]
pub struct Source {
    pub name: String,
    pub url: String,
    pub kind: Kind,
    /// The person's repository's id, when it is one of theirs turned off.
    pub id: Option<String>,
    pub packages: HashMap<String, Vec<String>>,
}
/// A package found: the version that will be installed and the repository adding it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub package: Missing,
    pub version: String,
    pub repository: String,
    pub url: String,
    pub kind: Kind,
    pub turn_on: Option<String>,
}
/// A package a repository provides, but not at a version the project accepts: the versions it lists (newest first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotListed {
    pub package: Missing,
    pub repository: String,
    pub versions: Vec<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lookup {
    pub found: Vec<Found>,
    pub not_listed: Vec<NotListed>,
    pub not_provided: Vec<Missing>,
}
/// A repository to add (or turn back on) and the packages it brings, for the confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Addition {
    pub name: String,
    pub url: String,
    pub turn_on: Option<String>,
    pub packages: Vec<(String, String)>,
}
impl Lookup {
    /// The repositories to add, each once, in the order their first package was found.
    pub fn additions(&self) -> Vec<Addition> {
        let mut out: Vec<Addition> = vec![];
        for f in &self.found {
            match out.iter_mut().find(|a| same_url(&a.url, &f.url)) {
                Some(a) => a.packages.push((f.package.name.clone(), f.version.clone())),
                None => out.push(Addition {
                    name: f.repository.clone(),
                    url: f.url.clone(),
                    turn_on: f.turn_on.clone(),
                    packages: vec![(f.package.name.clone(), f.version.clone())],
                }),
            }
        }
        out
    }
    pub fn complete(&self) -> bool {
        self.not_listed.is_empty() && self.not_provided.is_empty()
    }
}

pub fn same_url(a: &str, b: &str) -> bool {
    let n = |s: &str| s.trim().trim_end_matches('/').to_ascii_lowercase();
    n(a) == n(b)
}

fn newest_first(versions: &[String]) -> Vec<String> {
    let mut v: Vec<(Option<Version>, String)> = versions.iter().map(|s| (s.parse().ok(), s.clone())).collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().map(|(_, s)| s).collect()
}

/// Where each of `missing` is: the first source (in order) listing a version its range accepts, the newest such
/// version. A Known VPM repository naming the package without that version is read with `read(url, package)` (its
/// own listing's versions, `None` when it cannot be read) before it counts as not having it.
pub fn find(missing: &[Missing], sources: &[Source], read: impl Fn(&str, &str) -> Option<Vec<String>>) -> Lookup {
    let mut out = Lookup::default();
    'package: for m in missing {
        let candidates: Vec<&Source> = sources.iter().filter(|s| s.packages.contains_key(&m.name)).collect();
        for s in &candidates {
            if let Some(v) = newest_first(&s.packages[&m.name]).into_iter().find(|v| accepts(&m.range, v)) {
                out.found.push(Found { package: m.clone(), version: v, repository: s.name.clone(), url: s.url.clone(), kind: s.kind, turn_on: s.id.clone() });
                continue 'package;
            }
        }
        let mut listed: Option<(String, Vec<String>)> = None;
        for s in candidates.iter().filter(|s| s.kind == Kind::Known) {
            let Some(versions) = read(&s.url, &m.name) else { continue };
            if let Some(v) = newest_first(&versions).into_iter().find(|v| accepts(&m.range, v)) {
                out.found.push(Found { package: m.clone(), version: v, repository: s.name.clone(), url: s.url.clone(), kind: s.kind, turn_on: s.id.clone() });
                continue 'package;
            }
            listed.get_or_insert((s.name.clone(), versions));
        }
        match listed.or_else(|| candidates.first().map(|s| (s.name.clone(), s.packages[&m.name].clone()))) {
            Some((repository, versions)) => {
                out.not_listed.push(NotListed { package: m.clone(), repository, versions: newest_first(&versions).into_iter().take(6).collect() })
            }
            None => out.not_provided.push(m.clone()),
        }
    }
    out
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KnownPackage {
    id: String,
    repository_url: String,
    #[serde(default)]
    versions: Vec<KnownVersion>,
}
#[derive(Deserialize)]
struct KnownVersion {
    version: String,
}

/// The Known VPM catalog as sources, one per repository (named from `/vpm/known`, else by its host).
pub fn known_sources(catalog: &serde_json::Value, repositories: &[crate::network::Listing]) -> Vec<Source> {
    let packages: Vec<KnownPackage> = serde_json::from_value(catalog["packages"].clone()).unwrap_or_default();
    let mut by_url: BTreeMap<String, Source> = BTreeMap::new();
    let mut order: Vec<String> = vec![];
    for p in packages {
        let key = p.repository_url.trim().to_string();
        let source = by_url.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            let name = repositories
                .iter()
                .find(|r| same_url(&r.url, &key) && !r.name.trim().is_empty())
                .map(|r| r.name.clone())
                .unwrap_or_else(|| url::Url::parse(&key).ok().and_then(|u| u.host_str().map(ToOwned::to_owned)).unwrap_or_else(|| key.clone()));
            Source { name, url: key.clone(), kind: Kind::Known, id: None, packages: HashMap::new() }
        });
        source.packages.entry(p.id).or_default().extend(p.versions.into_iter().map(|v| v.version));
    }
    order.into_iter().filter_map(|k| by_url.remove(&k)).collect()
}

/// A listing's packages and their versions (its `packages.<name>.versions` keys).
pub fn listing_versions(listing: &serde_json::Value) -> HashMap<String, Vec<String>> {
    listing["packages"]
        .as_object()
        .map(|p| p.iter().map(|(name, e)| (name.clone(), e["versions"].as_object().map(|v| v.keys().cloned().collect()).unwrap_or_default())).collect())
        .unwrap_or_default()
}

fn read_listing(url: &str) -> Option<serde_json::Value> {
    let api = Api::at("https://orbiters.cc/");
    let response = api.agent().get(url).call().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = response.into_body().with_config().limit(32 * 1024 * 1024).read_to_vec().ok()?;
    serde_json::from_slice(String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').as_bytes()).ok()
}

/// The person's repositories turned off on this computer, from the listings last downloaded.
async fn turned_off(root: &Path) -> Vec<Source> {
    let Ok((settings, collection)) = crate::vpm::collection(root, false).await else { return vec![] };
    let mut out = vec![];
    for r in settings.get_user_repos() {
        let Some(id) = r.id().map(ToOwned::to_owned) else { continue };
        if !collection.hidden.contains(&id) {
            continue;
        }
        let Some(cached) = collection.inner.get_remote().find(|c| c.id() == Some(id.as_str())) else { continue };
        let listing = serde_json::to_value(cached.repo()).unwrap_or_default();
        out.push(Source {
            name: r.name().unwrap_or(&id).to_string(),
            url: r.url().map(ToString::to_string).unwrap_or_default(),
            kind: Kind::TurnedOff,
            id: Some(id),
            packages: listing_versions(&listing),
        });
    }
    out
}

/// Looks `missing` up in every source the app knows (see the module's notes). Network errors leave a source out.
pub async fn lookup(root: &Path, server: Server, missing: &[Missing]) -> anyhow::Result<Lookup> {
    let api = Api::at(server.api());
    let get = |path: &str| -> anyhow::Result<serde_json::Value> { Api::json(api.agent().get(api.url(path)).call()?) };
    let catalog = get("avatar-assets/known-dependencies");
    let repositories: Vec<crate::network::Listing> =
        get("vpm/known").ok().and_then(|v| serde_json::from_value(v["repositories"].clone()).ok()).unwrap_or_default();
    let mut sources = catalog.as_ref().map(|c| known_sources(c, &repositories)).unwrap_or_default();
    sources.extend(turned_off(root).await);
    let read = |url: &str, package: &str| read_listing(url).map(|l| listing_versions(&l).remove(package).unwrap_or_default());
    let mut found = find(missing, &sources, read);
    if !found.not_provided.is_empty() {
        let listings: Vec<crate::network::Listing> =
            get("vpm/public").ok().and_then(|v| serde_json::from_value(v["listings"].clone()).ok()).unwrap_or_default();
        let mut more = vec![];
        for l in listings.iter().filter(|l| !sources.iter().any(|s| same_url(&s.url, &l.url))).take(LISTINGS_READ) {
            if let Some(listing) = read_listing(&l.url) {
                more.push(Source { name: l.name.clone(), url: l.url.clone(), kind: Kind::Orbiters, id: None, packages: listing_versions(&listing) });
            }
        }
        let rest = find(&found.not_provided, &more, |_, _| None);
        found.found.extend(rest.found);
        found.not_listed.extend(rest.not_listed);
        found.not_provided = rest.not_provided;
    }
    if found.found.is_empty() && found.not_listed.is_empty() {
        if let Err(e) = catalog {
            anyhow::bail!("Orbiters' Known VPM catalog could not be read: {e:#}");
        }
    }
    Ok(found)
}

/// Adds the repositories (turning those of the person's turned off back on), in order. Stops at the first that
/// cannot be added, keeping those added before it.
pub async fn add(root: &Path, additions: &[Addition]) -> anyhow::Result<()> {
    for a in additions {
        if let Some(id) = &a.turn_on {
            crate::vpm::set_enabled(root, id, true).await?;
            continue;
        }
        crate::vpm::add_repo(root, &a.url, BTreeMap::new()).await.map_err(|e| anyhow::anyhow!("{} could not be added: {e:#}", a.name))?;
        // Listed already but turned off: turned back on.
        let (settings, collection) = crate::vpm::collection(root, false).await?;
        let hidden: HashSet<&str> = collection.hidden.iter().map(String::as_str).collect();
        if let Some(id) = settings.get_user_repos().iter().find(|r| r.url().is_some_and(|u| same_url(u.as_str(), &a.url))).and_then(|r| r.id()) {
            if hidden.contains(id) {
                crate::vpm::set_enabled(root, id, true).await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(name: &str, kind: Kind, packages: &[(&str, &[&str])]) -> Source {
        Source {
            name: name.into(),
            url: format!("https://{}.example/index.json", name.to_lowercase()),
            kind,
            id: (kind == Kind::TurnedOff).then(|| name.to_lowercase()),
            packages: packages.iter().map(|(n, v)| (n.to_string(), v.iter().map(|s| s.to_string()).collect())).collect(),
        }
    }

    #[test]
    fn the_resolver_message_gives_the_packages_and_their_ranges() {
        let message = "Following dependencies are not found: dev.hai-vr.fuck-osc-config@=1.1.5, d4rkpl4y3r.d4rkavataroptimizer@=3.8.0, \
                       de.thryrallo.vrc.avatar-performance-tools@>=1.3.7 <2.0.0";
        let m = parse(&format!("Packages could not be restored: {message}"));
        assert_eq!(m.len(), 3);
        assert_eq!(m[0], Missing { name: "dev.hai-vr.fuck-osc-config".into(), range: "=1.1.5".into() });
        assert_eq!(m[0].exact(), Some("1.1.5"));
        assert_eq!(m[2].range, ">=1.3.7 <2.0.0");
        assert_eq!(m[2].exact(), None);
        assert!(parse("Unity is open").is_empty());
        // The resolver's own wording, so a change upstream fails here.
        let err = vrc_get_vpm::unity_project::ResolvePackageErr::DependenciesNotFound {
            dependencies: vec![("a.b".into(), "=1.1.5".parse().unwrap()), ("c.d".into(), ">=2.0.0".parse().unwrap())],
        };
        let m = parse(&err.to_string());
        assert_eq!(m.iter().map(|m| (m.name.as_str(), m.range.as_str())).collect::<Vec<_>>(), [("a.b", "=1.1.5"), ("c.d", ">=2.0.0")]);
    }

    #[test]
    fn a_pinned_version_needs_a_repository_listing_it() {
        assert!(accepts("=1.1.5", "1.1.5"));
        assert!(!accepts("=1.1.5", "1.1.6"));
        assert!(accepts(">=1.0.0", "1.4.0"));
        assert!(!accepts(">=1.0.0", "1.5.0-beta.1"));
        let sources = [
            source("Known one", Kind::Known, &[("dev.hai-vr.fuck-osc-config", &["1.1.6", "1.2.0"]), ("d4rkpl4y3r.d4rkavataroptimizer", &["3.8.0", "3.9.1"])]),
            source("Known two", Kind::Known, &[("dev.hai-vr.fuck-osc-config", &["1.1.5"])]),
            source("Mine", Kind::TurnedOff, &[("de.thryrallo.vrc.avatar-performance-tools", &["1.3.7"])]),
        ];
        let missing = parse(
            "Following dependencies are not found: dev.hai-vr.fuck-osc-config@=1.1.5, d4rkpl4y3r.d4rkavataroptimizer@=3.8.0, \
             de.thryrallo.vrc.avatar-performance-tools@=1.3.7, nobody.has.this@=1.0.0",
        );
        let found = find(&missing, &sources, |_, _| None);
        assert_eq!(found.found.len(), 3);
        // The first repository with the version, not the first naming the package.
        assert_eq!((found.found[0].repository.as_str(), found.found[0].version.as_str()), ("Known two", "1.1.5"));
        assert_eq!(found.found[1].repository, "Known one");
        assert_eq!(found.found[2].turn_on.as_deref(), Some("mine"));
        assert_eq!(found.not_provided, vec![Missing { name: "nobody.has.this".into(), range: "=1.0.0".into() }]);
        let additions = found.additions();
        assert_eq!(additions.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Known two", "Known one", "Mine"]);
        assert!(!found.complete());
    }

    #[test]
    fn a_version_the_catalog_does_not_keep_is_read_from_the_repository_itself() {
        let sources = [source("Known", Kind::Known, &[("a.b", &["2.0.0", "1.9.0"])])];
        let missing = [Missing { name: "a.b".into(), range: "=1.0.0".into() }];
        // The catalog keeps the newest versions only; the listing still has 1.0.0.
        let found = find(&missing, &sources, |url, package| {
            assert_eq!((url, package), ("https://known.example/index.json", "a.b"));
            Some(vec!["2.0.0".into(), "1.9.0".into(), "1.0.0".into()])
        });
        assert_eq!(found.found[0].version, "1.0.0");
        // Neither has it: the versions there are, newest first.
        let found = find(&missing, &sources, |_, _| Some(vec!["1.9.0".into(), "2.0.0".into()]));
        assert!(found.found.is_empty());
        assert_eq!(found.not_listed, vec![NotListed { package: missing[0].clone(), repository: "Known".into(), versions: vec!["2.0.0".into(), "1.9.0".into()] }]);
        // Unreadable: what the catalog says.
        let found = find(&missing, &sources, |_, _| None);
        assert_eq!(found.not_listed[0].versions, ["2.0.0", "1.9.0"]);
    }

    #[test]
    fn the_known_catalog_is_grouped_by_repository_and_named() {
        let catalog = serde_json::json!({"packages": [
            {"id":"a.one","displayName":"One","repositoryUrl":"https://vpm.a.example/index.json","versions":[{"version":"1.0.0"},{"version":"1.1.0"}]},
            {"id":"b.two","displayName":"Two","repositoryUrl":"https://b.example/vpm.json","versions":[{"version":"0.1.0"}]},
            {"id":"a.three","displayName":"Three","repositoryUrl":"https://vpm.a.example/index.json","versions":[{"version":"3.0.0"}]}
        ]});
        let named = [crate::network::Listing { name: "A's packages".into(), url: "https://vpm.a.example/index.json/".into(), ..Default::default() }];
        let sources = known_sources(&catalog, &named);
        assert_eq!(sources.len(), 2);
        assert_eq!((sources[0].name.as_str(), sources[0].packages.len()), ("A's packages", 2));
        assert_eq!(sources[1].name, "b.example");
    }
}
