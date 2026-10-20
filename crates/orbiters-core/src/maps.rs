//! The Map explorer on Orbiters (server `routes/mapsRouter.js`): a world's public page, what this member added, and
//! adding 3D previews and pictures. Reading is public; adding uses the account the Unity tools share.

use crate::account::{Api, ApiError};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::time::Duration;

/// A world as Orbiters describes it (VRChat's public details).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct World {
    pub world_id: String,
    pub name: String,
    #[serde(default)]
    pub author_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub image_url: Option<String>,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub capacity: Option<u32>,
    #[serde(default)]
    pub visits: Option<u64>,
    #[serde(default)]
    pub favorites: Option<u64>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// A world's page on the Map explorer, reduced to what the desktop shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldPage {
    pub world: World,
    #[serde(default, deserialize_with = "count")]
    pub previews: usize,
    #[serde(default)]
    pub picture_count: usize,
}

fn count<'de, D: serde::Deserializer<'de>>(d: D) -> Result<usize, D::Error> {
    Ok(Vec::<serde::de::IgnoredAny>::deserialize(d)?.len())
}

/// What this member added to one world.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MineWorld {
    pub world_id: String,
    #[serde(default)]
    pub preview: Option<MinePreview>,
    #[serde(default)]
    pub pictures: Vec<MinePicture>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinePreview {
    pub id: u64,
    #[serde(default)]
    pub splats: usize,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinePicture {
    pub id: u64,
    /// SHA-256 of the original file this member sent.
    pub source_hash: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Mine {
    pub worlds: Vec<MineWorld>,
}

/// What a picture upload did.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Added {
    #[serde(default, deserialize_with = "count")]
    pub added: usize,
    #[serde(default)]
    pub skipped: usize,
}

/// One file of a multipart upload.
pub struct Part {
    pub field: &'static str,
    pub name: String,
    pub content_type: &'static str,
    pub bytes: Vec<u8>,
}

/// The website's page of a world.
pub fn world_page_url(server: crate::account::Server, world_id: &str) -> String {
    format!("{}/maps/{}", server.website().trim_end_matches('/'), world_id)
}

impl Api {
    /// A world's page; `None` when Orbiters does not list it (not public on VRChat). A server without the Map
    /// explorer (its 404 is not Orbiters' JSON error) is an error, not an unlisted world.
    pub fn world(&self, world_id: &str) -> anyhow::Result<Option<WorldPage>> {
        let response = self.agent().get(&self.url(&format!("maps/worlds/{world_id}"))).call()?;
        if response.status().as_u16() == 404 {
            let text = response.into_body().with_config().limit(64 * 1024).read_to_string().unwrap_or_default();
            let ours = serde_json::from_str::<serde_json::Value>(&text).ok().is_some_and(|v| v.get("error").is_some_and(|e| e.is_string()));
            anyhow::ensure!(ours, "this Orbiters server has no Map explorer");
            return Ok(None);
        }
        Ok(Some(Self::json(response)?))
    }

    /// What this member added, per world.
    pub fn mine(&self, token: &str) -> anyhow::Result<Mine> {
        let response = self.agent().get(&self.url("maps/mine")).header("Authorization", &format!("Bearer {token}")).call()?;
        Self::json(response)
    }

    /// Adds pictures (at most 12 at a time): the original files, so that Orbiters recognises them later by their hash.
    /// `taken` are their ISO 8601 dates.
    pub fn add_pictures(
        &self,
        token: &str,
        world_id: &str,
        pictures: Vec<Part>,
        taken: &[String],
        progress: impl FnMut(u64, u64) + Send + 'static,
    ) -> anyhow::Result<Added> {
        let metadata = serde_json::json!({ "takenAt": taken }).to_string();
        let response = self.multipart(token, &format!("maps/worlds/{world_id}/pictures"), &metadata, pictures, progress)?;
        Self::json(response)
    }

    /// Adds a 3D preview (`.splat`), replacing this member's earlier one of the world. `metadata` holds the start
    /// view (`{"view":{"position":[…],"target":[…]}}`), quality and capture date.
    pub fn add_preview(
        &self,
        token: &str,
        world_id: &str,
        splat: Vec<u8>,
        metadata: &serde_json::Value,
        progress: impl FnMut(u64, u64) + Send + 'static,
    ) -> anyhow::Result<()> {
        let part = Part { field: "splat", name: format!("{world_id}.splat"), content_type: "application/octet-stream", bytes: splat };
        let response = self.multipart(token, &format!("maps/worlds/{world_id}/previews"), &metadata.to_string(), vec![part], progress)?;
        Self::json::<serde_json::Value>(response).map(|_| ())
    }

    /// A `multipart/form-data` POST with its JSON in a "metadata" field, reporting the bytes sent.
    fn multipart(
        &self,
        token: &str,
        path: &str,
        metadata: &str,
        parts: Vec<Part>,
        progress: impl FnMut(u64, u64) + Send + 'static,
    ) -> anyhow::Result<ureq::http::Response<ureq::Body>> {
        let boundary = format!("----orbiters-{}", uuid::Uuid::new_v4().simple());
        let mut body = Vec::with_capacity(parts.iter().map(|p| p.bytes.len() + 256).sum::<usize>() + metadata.len() + 256);
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{metadata}\r\n").as_bytes());
        for p in parts {
            let name = p.name.replace(['"', '\r', '\n'], "_");
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{name}\"\r\nContent-Type: {}\r\n\r\n",
                    p.field, p.content_type
                )
                .as_bytes(),
            );
            body.extend_from_slice(&p.bytes);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let total = body.len() as u64;
        let reader = Progress { body: std::io::Cursor::new(body), sent: 0, total, report: Box::new(progress) };
        self.agent()
            .post(&self.url(path))
            .header("Authorization", &format!("Bearer {token}"))
            .header("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
            .header("Content-Length", &total.to_string())
            .config()
            .timeout_global(Some(Duration::from_secs(30 * 60)))
            .build()
            .send(ureq::SendBody::from_owned_reader(reader))
            .context("Orbiters could not be reached")
    }

    /// A public picture (VRChat's world thumbnails), at most 16 MB.
    pub fn image(&self, url: &str) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(url.starts_with("https://"), "not an https address");
        let response = self.agent().get(url).call()?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(ApiError { status, message: String::new() }.into());
        }
        Ok(response.into_body().with_config().limit(16 << 20).read_to_vec()?)
    }
}

/// The request body, counting what was read from it.
struct Progress {
    body: std::io::Cursor<Vec<u8>>,
    sent: u64,
    total: u64,
    report: Box<dyn FnMut(u64, u64) + Send>,
}

impl Read for Progress {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // Small reads keep the progress smooth.
        let max = buf.len().min(64 << 10);
        let n = self.body.read(&mut buf[..max])?;
        self.sent += n as u64;
        (self.report)(self.sent, self.total);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_world_page_and_what_a_member_added() {
        let page: WorldPage = serde_json::from_str(
            r#"{"world":{"worldId":"wrld_a","name":"Palm Point","authorName":"Sam","thumbnailUrl":"https://api.vrchat.cloud/x/256","capacity":32,"visits":12,"tags":["chill"]},
               "previews":[{"id":1},{"id":2}],"pictures":[],"pictureCount":7,"hasMorePictures":false}"#,
        )
        .unwrap();
        assert_eq!(page.world.name, "Palm Point");
        assert_eq!((page.previews, page.picture_count), (2, 7));
        let mine: Mine = serde_json::from_str(
            r#"{"worlds":[{"worldId":"wrld_a","preview":{"id":3,"url":"/files/serve/9","splats":600000,"createdAt":"2026-10-20T20:00:00.000Z"},
               "pictures":[{"id":4,"url":"/files/serve/10","sourceHash":"ab12"}]}]}"#,
        )
        .unwrap();
        assert_eq!(mine.worlds[0].preview.as_ref().unwrap().splats, 600_000);
        assert_eq!(mine.worlds[0].pictures[0].source_hash, "ab12");
        let added: Added = serde_json::from_str(r#"{"added":[{"id":1},{"id":2}],"skipped":1}"#).unwrap();
        assert_eq!((added.added, added.skipped), (2, 1));
    }

    /// Against a disposable server: `ORBITERS_TEST_API=http://127.0.0.1:4230 ORBITERS_TEST_TOKEN=orbit-… ORBITERS_TEST_WORLD=wrld_…
    /// ORBITERS_TEST_PICTURE=<png> cargo test -p orbiters-core adds_a_picture -- --ignored`
    #[test]
    #[ignore]
    fn adds_a_picture() {
        let api = Api::at(&std::env::var("ORBITERS_TEST_API").unwrap());
        let token = std::env::var("ORBITERS_TEST_TOKEN").unwrap();
        let world = std::env::var("ORBITERS_TEST_WORLD").unwrap();
        let path = std::env::var("ORBITERS_TEST_PICTURE").unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let hash = crate::visited::sha256_file(std::path::Path::new(&path)).unwrap();
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let s = seen.clone();
        let part = Part { field: "pictures", name: "shot.png".into(), content_type: "image/png", bytes };
        let added = api
            .add_pictures(&token, &world, vec![part], &["2026-10-18T18:37:41Z".into()], move |sent, _| s.store(sent, std::sync::atomic::Ordering::Relaxed))
            .unwrap();
        println!("added {} skipped {}", added.added, added.skipped);
        assert!(seen.load(std::sync::atomic::Ordering::Relaxed) > 0);
        let mine = api.mine(&token).unwrap();
        assert!(mine.worlds.iter().any(|w| w.world_id == world && w.pictures.iter().any(|p| p.source_hash == hash)));
    }
}
