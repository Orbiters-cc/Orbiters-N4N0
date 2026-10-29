use orbiters_core::account::{self, Api, Server};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Listing {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub description: String,
    /// Listings Orbiters hosts: the listing's logo and those creators set for its packages (paths on the API).
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub logos: HashMap<String, String>,
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
pub fn catalog(server: Server) -> Catalog {
    let api = Api::at(server.api());
    let mut out = Catalog::default();
    for (endpoint, key) in [("vpm/known", "repositories"), ("vpm/public", "listings")] {
        let r = (|| -> anyhow::Result<Vec<Listing>> {
            let response = api.agent().get(api.url(endpoint)).call()?;
            let v: serde_json::Value = Api::json(response)?;
            Ok(serde_json::from_value(v[key].clone())?)
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
    for chunk in missing.chunks(50) {
        let response = api
            .agent()
            .get(api.url(&format!("vrchat/avatars/thumbnails?ids={}", chunk.join(","))))
            .header("Authorization", &format!("Bearer {}", account.token))
            .call()?;
        let v: serde_json::Value = Api::json(response)?;
        for id in chunk {
            let Some(url) = v["avatars"][id]["thumbnailUrl"].as_str() else { continue };
            // Images are public CDN resources: never forward the Orbiters bearer token to them.
            let parsed = url::Url::parse(url)?;
            if parsed.scheme() != "https" {
                continue;
            }
            let response = api.agent().get(url).call()?;
            if !response.status().is_success() {
                continue;
            }
            let bytes = response.into_body().with_config().limit(12 * 1024 * 1024).read_to_vec()?;
            std::fs::create_dir_all(&cache)?;
            let p = cache.join(format!("{}.image", crate::avatars::filename(id)));
            let tmp = p.with_extension("tmp");
            std::fs::write(&tmp, &bytes)?;
            std::fs::rename(tmp, &p)?;
            out.insert(id.clone(), Thumbnail { path: p, size: bytes.len() as u64 });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

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
