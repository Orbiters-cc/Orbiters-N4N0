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
