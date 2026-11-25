use orbiters_core::account::{self, Api, Server};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A repository the server suggests. `GET /vpm/known` answers `{ repositories: [...] }` and `GET /vpm/public`
/// `{ listings: [...] }`, the same fields in each but for `packageCount` (known) and the logos (public). Everything but
/// the name and the address may be missing or null: a server older than these fields still answers, and one repository
/// without an owner must not lose the person the 329 others.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub name: String,
    pub url: String,
    #[serde(default, deserialize_with = "or_default")]
    pub description: String,
    /// Who keeps the repository.
    #[serde(default, deserialize_with = "or_default")]
    pub owner: String,
    /// What it holds, at least as far as the server looked: searched, and named on a card found through one of them.
    #[serde(default, deserialize_with = "or_default")]
    pub packages: Vec<ListingPackage>,
    /// How many packages it has, where the server counts them apart from `packages`.
    #[serde(default)]
    pub package_count: Option<usize>,
    /// Listings Orbiters hosts: the listing's logo and those creators set for its packages (paths on the API).
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default, deserialize_with = "or_default")]
    pub logos: HashMap<String, String>,
}

impl Listing {
    /// How many packages the repository has, when the server said anything about them.
    pub fn package_total(&self) -> Option<usize> {
        self.package_count.or((!self.packages.is_empty()).then_some(self.packages.len()))
    }
}

/// A package of a [`Listing`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingPackage {
    /// Its id (`nadena.dev.modular-avatar`).
    pub name: String,
    #[serde(default, deserialize_with = "or_default")]
    pub display_name: String,
}

/// `null` as the type's default: serde's own `default` covers a missing field only, and one null would fail the whole
/// answer.
fn or_default<'de, D: serde::Deserializer<'de>, T: Default + Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// The Orbiters server hosting the listing at `url` (its feed is `<website>vpm/<slug>/index.json`), if one does.
pub fn hosting_server(url: &str) -> Option<Server> {
    [Server::Production, Server::Development].into_iter().find(|s| {
        url.strip_prefix(s.website())
            .and_then(|rest| rest.strip_prefix("vpm/"))
            .and_then(|rest| rest.strip_suffix("/index.json"))
            .is_some_and(|slug| !slug.is_empty() && !slug.contains('/'))
    })
}

/// The logo the creator set on Orbiters for `package` of the listing at `listing` (an absolute URL), when Orbiters
/// hosts that listing. The server's listings are kept on disk for an hour (stale ones when it cannot be reached).
pub fn package_logo(listing: &str, package: &str) -> Option<String> {
    let server = hosting_server(listing)?;
    let listings = public_listings(server);
    let path = listings.iter().find(|l| l.url == listing)?.logos.get(package)?;
    Some(format!("{}{}", server.api().trim_end_matches('/'), path))
}

/// The logo Orbiters keeps for the listing at `listing` (an absolute URL), when Orbiters hosts that listing.
pub fn listing_logo(listing: &str) -> Option<String> {
    let server = hosting_server(listing)?;
    let path = public_listings(server).into_iter().find(|l| l.url == listing)?.logo?;
    Some(format!("{}{}", server.api().trim_end_matches('/'), path))
}

fn public_listings(server: Server) -> Vec<Listing> {
    use std::sync::Mutex;
    // One look at a time, so a page asking for many packages fetches the directory once.
    static ONE: Mutex<()> = Mutex::new(());
    let _one = ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = crate::icons::cache_dir().join(match server {
        Server::Production => "orbiters-listings.json",
        Server::Development => "orbiters-listings-development.json",
    });
    let kept: Option<serde_json::Value> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let listings = |v: &serde_json::Value| serde_json::from_value::<Vec<Listing>>(v["listings"].clone()).unwrap_or_default();
    if let Some(v) = &kept {
        if crate::discovery::now().saturating_sub(v["checked"].as_u64().unwrap_or(0)) < 3600 {
            return listings(v);
        }
    }
    let api = Api::at(server.api());
    let fresh = (|| -> anyhow::Result<Vec<Listing>> {
        let v: serde_json::Value = Api::json(api.agent().get(api.url("vpm/public")).call()?)?;
        Ok(serde_json::from_value(v["listings"].clone())?)
    })();
    // Unreachable: what was kept, asked again in five minutes (not by every package of the page).
    let (checked, list) = match fresh {
        Ok(fresh) => (crate::discovery::now(), fresh),
        Err(_) => (crate::discovery::now().saturating_sub(3600 - 300), kept.as_ref().map(listings).unwrap_or_default()),
    };
    let _ = crate::write_json(&path, &serde_json::json!({ "checked": checked, "listings": list }));
    list
}
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub known: Vec<Listing>,
    pub public: Vec<Listing>,
    pub errors: Vec<String>,
}
impl Catalog {
    /// This answer, keeping from `before` the list it could not get: the dialog asks again each time it opens, and a server
    /// that does not answer this time is no reason for suggestions already shown to go.
    pub fn or_what_was_loaded(mut self, before: &Catalog) -> Self {
        if !self.errors.is_empty() {
            if self.known.is_empty() {
                self.known.clone_from(&before.known);
            }
            if self.public.is_empty() {
                self.public.clone_from(&before.public);
            }
        }
        self
    }
}

pub fn catalog(server: Server) -> Catalog {
    let api = Api::at(server.api());
    let mut out = Catalog::default();
    for (endpoint, key) in [("vpm/known", "repositories"), ("vpm/public", "listings")] {
        let r = (|| -> anyhow::Result<Vec<Listing>> {
            let response = api.agent().get(api.url(endpoint)).call()?;
            parse_listings(&Api::json(response)?, key)
        })();
        match r {
            Ok(v) => {
                if key == "repositories" {
                    out.known = v
                } else {
                    out.public = v
                }
            }
            Err(e) => out.errors.push(format!("{endpoint}: {e}")),
        }
    }
    out
}

/// The listings under `key` of one of the server's answers.
fn parse_listings(answer: &serde_json::Value, key: &str) -> anyhow::Result<Vec<Listing>> {
    Ok(serde_json::from_value(answer[key].clone())?)
}

#[derive(Clone, Debug, Default)]
pub struct Thumbnail {
    pub path: PathBuf,
    pub size: u64,
}
pub fn thumbnails(server: Server, ids: Vec<String>) -> anyhow::Result<HashMap<String, Thumbnail>> {
    let api = Api::at(server.api());
    let mut out = HashMap::new();
    let mut missing = vec![];
    let cache = crate::data_dir().join("avatar-thumbnails").join(match server {
        Server::Production => "production",
        Server::Development => "development",
    });
    for id in ids {
        let p = crate::avatars::local_thumbnail(&id).unwrap_or_else(|| cache.join(format!("{}.image", crate::avatars::filename(&id))));
        if let Ok(m) = std::fs::metadata(&p) {
            out.insert(id, Thumbnail { path: p, size: m.len() });
        } else {
            missing.push(id);
        }
    }
    if missing.is_empty() {
        return Ok(out);
    }
    let Some(account) = account::read(server) else { return Ok(out) };
    let mut failed = None;
    for chunk in missing.chunks(50) {
        let asked = (|| -> anyhow::Result<serde_json::Value> {
            let response = api
                .agent()
                .get(api.url(&format!("vrchat/avatars/thumbnails?ids={}", chunk.join(","))))
                .header("Authorization", &format!("Bearer {}", account.token))
                .call()?;
            Ok(Api::json(response)?)
        })();
        let answer = match asked {
            Ok(answer) => answer,
            Err(e) => {
                failed = Some(e);
                break;
            }
        };
        let pictures: Vec<(String, String)> = chunk.iter().filter_map(|id| Some((id.clone(), answer["avatars"][id]["thumbnailUrl"].as_str()?.to_string()))).collect();
        // Images are public CDN resources: never forward the Orbiters bearer token to them.
        let fetch = |url: &str| -> anyhow::Result<Vec<u8>> {
            anyhow::ensure!(url::Url::parse(url)?.scheme() == "https", "not over https"); // i18n: not shown (the picture is left out)
            let response = api.agent().get(url).call()?;
            anyhow::ensure!(response.status().is_success(), "{}", response.status());
            Ok(response.into_body().with_config().limit(12 * 1024 * 1024).read_to_vec()?)
        };
        out.extend(pictures_of(&pictures, &cache, &fetch));
    }
    match failed {
        // What was found stays, what could not be asked for is asked for again later.
        Some(e) if out.is_empty() => Err(e),
        _ => Ok(out),
    }
}

/// How many pictures are fetched at once.
const AT_ONCE: usize = 6;

/// `pictures` (id and address) fetched and kept in `cache`, each by itself: one that fails costs no other (PRJ-28).
fn pictures_of(pictures: &[(String, String)], cache: &Path, fetch: &(dyn Fn(&str) -> anyhow::Result<Vec<u8>> + Sync)) -> HashMap<String, Thumbnail> {
    let one = |(id, url): &(String, String)| -> anyhow::Result<(String, Thumbnail)> {
        let bytes = fetch(url)?;
        let path = cache.join(format!("{}.image", crate::avatars::filename(id)));
        crate::vpm::replace_bytes(&path, &bytes)?;
        Ok((id.clone(), Thumbnail { path, size: bytes.len() as u64 }))
    };
    if crate::disk::create_dir_all(cache).is_err() {
        return HashMap::new();
    }
    let mut out = HashMap::new();
    for some in pictures.chunks(AT_ONCE) {
        std::thread::scope(|s| {
            let fetching: Vec<_> = some.iter().map(|p| s.spawn(|| one(p))).collect();
            out.extend(fetching.into_iter().filter_map(|t| t.join().ok()?.ok()));
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PRJ-28: one picture that cannot be fetched (its address, its connection) or kept costs none of the others.
    #[test]
    fn one_failed_picture_costs_no_other() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let pictures: Vec<(String, String)> = (0..20).map(|i| (format!("avtr_{i}"), format!("https://pictures.example/{i}"))).collect();
        // One that cannot be written: a folder is at its place.
        std::fs::create_dir_all(cache.join(format!("{}.image", crate::avatars::filename("avtr_7")))).unwrap();
        let fetch = |url: &str| {
            anyhow::ensure!(!url.ends_with("/3"), "no connection");
            Ok(url.as_bytes().to_vec())
        };
        let got = pictures_of(&pictures, &cache, &fetch);
        assert_eq!(got.len(), 18);
        assert!(!got.contains_key("avtr_3") && !got.contains_key("avtr_7"));
        assert_eq!(std::fs::read(&got["avtr_19"].path).unwrap(), b"https://pictures.example/19");
        let left = std::fs::read_dir(&cache).unwrap().count();
        assert_eq!(left, 19, "no half-written file");
    }

    #[test]
    fn the_known_repositories_answer_gives_owner_count_and_packages() {
        let answer = serde_json::json!({ "repositories": [{
            "name": "bd_",
            "url": "https://vpm.nadena.dev/vpm.json",
            "description": "Non-destructive avatar tools",
            "owner": "bdunderscore",
            "packageCount": 4,
            "packages": [{ "name": "nadena.dev.modular-avatar", "displayName": "Modular Avatar" }, { "name": "nadena.dev.ndmf" }],
        }]});
        let list = parse_listings(&answer, "repositories").unwrap();
        assert_eq!(list[0].owner, "bdunderscore");
        assert_eq!(list[0].package_total(), Some(4), "its own count, more than the packages it lists");
        assert_eq!(list[0].packages[0], ListingPackage { name: "nadena.dev.modular-avatar".into(), display_name: "Modular Avatar".into() });
        assert_eq!(list[0].packages[1].display_name, "");
    }

    #[test]
    fn the_public_listings_answer_counts_its_packages_and_keeps_its_logos() {
        let answer = serde_json::json!({ "listings": [{
            "name": "Orbiters",
            "url": "https://orbiters.cc/vpm/orbiters/index.json",
            "description": "",
            "owner": "Orbiters",
            "logo": "/vpm/orbiters/logo.png",
            "logos": { "orbiters.toolkit": "/vpm/orbiters/toolkit.png" },
            "packages": [{ "name": "orbiters.toolkit", "displayName": "Toolkit" }, { "name": "orbiters.myavatar", "displayName": "My Avatar" }],
        }]});
        let list = parse_listings(&answer, "listings").unwrap();
        assert_eq!(list[0].package_total(), Some(2));
        assert_eq!(list[0].logo.as_deref(), Some("/vpm/orbiters/logo.png"));
        assert_eq!(list[0].logos["orbiters.toolkit"], "/vpm/orbiters/toolkit.png");
    }

    /// What the server answers until its new fields land, and what it may send for a repository it knows little about.
    #[test]
    fn an_answer_without_the_new_fields_or_with_nulls_still_parses() {
        let older = serde_json::json!({ "repositories": [{ "name": "32ba VPM repository", "url": "https://vpm.32ba.net/index.json", "description": "" }] });
        let list = parse_listings(&older, "repositories").unwrap();
        assert_eq!((list[0].owner.as_str(), list[0].packages.len(), list[0].package_total()), ("", 0, None));
        let nulls = serde_json::json!({ "listings": [{ "name": "A", "url": "https://a/", "description": null, "owner": null, "packages": null, "logo": null, "logos": null, "packageCount": null }] });
        let list = parse_listings(&nulls, "listings").unwrap();
        assert_eq!((list[0].description.as_str(), list[0].owner.as_str(), list[0].package_total()), ("", "", None));
        // An answer that is not a list at all is an error the dialog shows, not an empty list.
        assert!(parse_listings(&serde_json::json!({ "error": "nope" }), "listings").is_err());
    }

    #[test]
    fn suggestions_already_loaded_stay_when_the_server_stops_answering() {
        let listing = |name: &str| Listing { name: name.into(), ..Default::default() };
        let before = Catalog { known: vec![listing("old known")], public: vec![listing("old public")], errors: vec![] };
        // Neither list: both stay, and the person is told.
        let failed = Catalog { errors: vec!["no".into()], ..Default::default() }.or_what_was_loaded(&before);
        assert_eq!((failed.known[0].name.as_str(), failed.public[0].name.as_str(), failed.errors.len()), ("old known", "old public", 1));
        // One list: the fresh one, and the other as it was.
        let partly = Catalog { known: vec![listing("new known")], errors: vec!["no".into()], ..Default::default() }.or_what_was_loaded(&before);
        assert_eq!((partly.known[0].name.as_str(), partly.public[0].name.as_str()), ("new known", "old public"));
        // An answer saying a list is now empty is believed.
        let empty = Catalog::default().or_what_was_loaded(&before);
        assert!(empty.known.is_empty() && empty.public.is_empty());
    }

    /// The directory is kept on disk: what was written must read back whole.
    #[test]
    fn a_listing_kept_on_disk_reads_back() {
        let l = Listing { name: "A".into(), url: "https://a/".into(), package_count: Some(3), packages: vec![ListingPackage { name: "a.b".into(), display_name: "B".into() }], ..Default::default() };
        let back: Listing = serde_json::from_value(serde_json::to_value(&l).unwrap()).unwrap();
        assert_eq!((back.package_count, back.packages), (Some(3), l.packages));
    }

    #[test]
    fn listings_orbiters_hosts_are_told_by_their_address() {
        assert_eq!(hosting_server("https://orbiters.cc/vpm/orbiters/index.json"), Some(Server::Production));
        assert_eq!(hosting_server("https://dev.orbiters.cc/vpm/nova/index.json"), Some(Server::Development));
        for other in [
            "https://vcc.vrcfury.com/",
            "https://orbiters.cc/vpm/index.json",
            "https://orbiters.cc/vpm/a/b/index.json",
            "https://orbiters.cc.evil.com/vpm/a/index.json",
        ] {
            assert_eq!(hosting_server(other), None, "{other}");
        }
    }
}
