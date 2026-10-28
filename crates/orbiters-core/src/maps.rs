//! The Map explorer on Orbiters (server `routes/mapsRouter.js`): a world's public page, what this member added, and
//! adding 3D previews and pictures. Reading is public; adding uses the account the Unity tools share.

use crate::account::{Api, ApiError};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A world id fit for a path of the API: VRChat's own shape, nothing else ([`crate::vrchat_log::is_world_id`]).
fn world_path(world_id: &str) -> anyhow::Result<&str> {
    anyhow::ensure!(crate::vrchat_log::is_world_id(world_id), "Not a VRChat world: {world_id:?}");
    Ok(world_id)
}

/// A count or a frame rate that the server may send as a decimal (a weighted median): rounded. Read strictly, one
/// such number would leave the whole page unread.
fn whole<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let value = Option::<f64>::deserialize(d)?.unwrap_or(0.0);
    Ok(if value.is_finite() { value.round().clamp(0.0, u32::MAX as f64) as u32 } else { 0 })
}

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
    /// How smoothly VRChat runs there for members who share their frame rate (contracts `Docs/world-performance.md`).
    #[serde(default)]
    pub performance: Option<WorldPerformance>,
}

/// A world's frame rate statistics (snake_case, as the contract writes them).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorldPerformance {
    #[serde(deserialize_with = "whole")]
    pub fps: u32,
    #[serde(deserialize_with = "whole")]
    pub fps_low: u32,
    /// Typical frame rate over the headset's refresh rate (0 to 1).
    #[serde(default)]
    pub ratio: f32,
    #[serde(default, deserialize_with = "whole")]
    pub minutes: u32,
    #[serde(default, deserialize_with = "whole")]
    pub members: u32,
    #[serde(default)]
    pub by_crowd: Vec<CrowdPerformance>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CrowdPerformance {
    pub players: String,
    #[serde(deserialize_with = "whole")]
    pub fps: u32,
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

/// What a 3D preview sends: the web `.splat` and VRChat's splat trees for PC, Quest and the coarse first view.
pub struct PreviewFiles {
    pub splat: Vec<u8>,
    pub vrchat_pc: Vec<u8>,
    pub vrchat_quest: Vec<u8>,
    pub vrchat_coarse: Vec<u8>,
}

impl PreviewFiles {
    pub fn len(&self) -> u64 {
        (self.splat.len() + self.vrchat_pc.len() + self.vrchat_quest.len() + self.vrchat_coarse.len()) as u64
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One file of a multipart upload.
pub struct Part {
    pub field: &'static str,
    pub name: String,
    pub content_type: &'static str,
    data: PartData,
}

enum PartData {
    Bytes(Vec<u8>),
    /// Read while it is sent, never whole in memory: this many bytes of it.
    File(PathBuf, u64),
}

impl Part {
    pub fn bytes(field: &'static str, name: impl Into<String>, content_type: &'static str, bytes: Vec<u8>) -> Self {
        Self { field, name: name.into(), content_type, data: PartData::Bytes(bytes) }
    }

    /// A file sent as it is on disk, read while the request goes (twelve 25 MB pictures do not sit in memory twice).
    pub fn file(field: &'static str, name: impl Into<String>, content_type: &'static str, path: &Path) -> std::io::Result<Self> {
        let len = std::fs::metadata(path)?.len();
        Ok(Self { field, name: name.into(), content_type, data: PartData::File(path.to_path_buf(), len) })
    }
}

/// The website's page of a world.
pub fn world_page_url(server: crate::account::Server, world_id: &str) -> String {
    format!("{}/maps/{}", server.website().trim_end_matches('/'), world_id)
}

impl Api {
    /// A world's page; `None` when Orbiters does not list it (not public on VRChat). A server without the Map
    /// explorer (its 404 is not Orbiters' JSON error) is an error, not an unlisted world.
    pub fn world(&self, world_id: &str) -> anyhow::Result<Option<WorldPage>> {
        let response = self.agent().get(&self.url(&format!("maps/worlds/{}", world_path(world_id)?))).call()?;
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
        let response = self.multipart(token, &format!("maps/worlds/{}/pictures", world_path(world_id)?), &metadata, pictures, progress)?;
        Self::json(response)
    }

    /// Adds a 3D preview with its sky map (a reconstruction's sky.png, when there is one), replacing this member's
    /// earlier one of the world: the web `.splat` and VRChat's splat trees (OrbitersContracts Docs/splat-tree.md).
    /// `metadata` holds the start view (`{"view":{"position":[…],"target":[…]}}`), quality and capture date.
    pub fn add_preview(
        &self,
        token: &str,
        world_id: &str,
        files: PreviewFiles,
        sky: Option<&Path>,
        metadata: &serde_json::Value,
        progress: impl FnMut(u64, u64) + Send + 'static,
    ) -> anyhow::Result<()> {
        let world_id = world_path(world_id)?;
        let tree = |field: &'static str, name: &str, bytes: Vec<u8>| Part::bytes(field, format!("{world_id}-{name}.n4tree"), "application/octet-stream", bytes);
        let mut parts = vec![
            Part::bytes("splat", format!("{world_id}.splat"), "application/octet-stream", files.splat),
            tree("vrchatPc", "pc", files.vrchat_pc),
            tree("vrchatQuest", "quest", files.vrchat_quest),
            tree("vrchatCoarse", "coarse", files.vrchat_coarse),
        ];
        if let Some(sky) = sky {
            parts.push(Part::file("sky", "sky.png", "image/png", sky)?);
        }
        let response = self.multipart(token, &format!("maps/worlds/{world_id}/previews"), &metadata.to_string(), parts, progress)?;
        Self::json::<serde_json::Value>(response).map(|_| ())
    }

    /// A `multipart/form-data` POST with its JSON in a "metadata" field, reporting the bytes sent. The body is made as
    /// it is sent, a part after the other, each let go once sent (a file part read from its file).
    fn multipart(
        &self,
        token: &str,
        path: &str,
        metadata: &str,
        parts: Vec<Part>,
        progress: impl FnMut(u64, u64) + Send + 'static,
    ) -> anyhow::Result<ureq::http::Response<ureq::Body>> {
        let boundary = format!("----orbiters-{}", uuid::Uuid::new_v4().simple());
        let mut pieces = vec![Piece::bytes(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{metadata}\r\n"))];
        for p in parts {
            let name = p.name.replace(['"', '\r', '\n'], "_");
            let head = format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{name}\"\r\nContent-Type: {}\r\n\r\n",
                p.field, p.content_type
            );
            pieces.push(Piece::bytes(head));
            pieces.push(match p.data {
                PartData::Bytes(bytes) => Piece::bytes(bytes),
                PartData::File(path, len) => Piece::File { path, file: None, left: len },
            });
            pieces.push(Piece::bytes(&b"\r\n"[..]));
        }
        pieces.push(Piece::bytes(format!("--{boundary}--\r\n")));
        let total = pieces.iter().map(Piece::len).sum::<u64>();
        pieces.reverse();
        let reader = Progress { pieces, sent: 0, total, report: Box::new(progress) };
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

    /// A public picture (VRChat's world thumbnails), at most 16 MB, over https only: a redirect to plain http is
    /// refused too.
    pub fn image(&self, url: &str) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(url.starts_with("https://"), "not an https address");
        let response = self.agent().get(url).config().https_only(true).build().call()?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(ApiError { status, message: String::new() }.into());
        }
        Ok(response.into_body().with_config().limit(16 << 20).read_to_vec()?)
    }
}

/// A piece of a request body.
enum Piece {
    Bytes(std::io::Cursor<Vec<u8>>),
    /// Opened when its turn comes; exactly `left` bytes more are sent from it.
    File {
        path: PathBuf,
        file: Option<std::fs::File>,
        left: u64,
    },
}

impl Piece {
    fn bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Piece::Bytes(std::io::Cursor::new(bytes.into()))
    }

    fn len(&self) -> u64 {
        match self {
            Piece::Bytes(c) => c.get_ref().len() as u64 - c.position(),
            Piece::File { left, .. } => *left,
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Piece::Bytes(c) => c.read(buf),
            Piece::File { path, file, left } => {
                if *left == 0 {
                    return Ok(0);
                }
                let f = match file {
                    Some(f) => f,
                    None => file.insert(std::fs::File::open(&*path)?),
                };
                let max = buf.len().min(usize::try_from(*left).unwrap_or(usize::MAX));
                let n = f.read(&mut buf[..max])?;
                // Shorter than when it was measured: the request would not match its own length.
                if n == 0 {
                    return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, format!("{} changed while it was sent", path.display())));
                }
                *left -= n as u64;
                Ok(n)
            }
        }
    }
}

/// The request body, made as it is read, counting what was read from it.
struct Progress {
    /// What is left to send, the next piece last.
    pieces: Vec<Piece>,
    sent: u64,
    total: u64,
    report: Box<dyn FnMut(u64, u64) + Send>,
}

impl Read for Progress {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // Small reads keep the progress smooth.
        let max = buf.len().min(64 << 10);
        if max == 0 {
            return Ok(0);
        }
        while let Some(piece) = self.pieces.last_mut() {
            let n = piece.read(&mut buf[..max])?;
            if n == 0 {
                // Sent: let it go (a picture's bytes, an open file).
                self.pieces.pop();
                continue;
            }
            self.sent += n as u64;
            (self.report)(self.sent, self.total);
            return Ok(n);
        }
        Ok(0)
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
        // A weighted median comes as a decimal: the page is still read.
        let perf: WorldPerformance =
            serde_json::from_str(r#"{"fps":71.6,"fps_low":40,"ratio":0.8,"minutes":12.0,"members":3,"by_crowd":[{"players":"1-4","fps":80.4}]}"#).unwrap();
        assert_eq!((perf.fps, perf.fps_low, perf.minutes, perf.by_crowd[0].fps), (72, 40, 12, 80));
    }

    #[test]
    fn world_ids_go_into_paths_only_as_ids() {
        assert!(world_path("wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b").is_ok());
        for bad in ["wrld_../../users/me", "wrld_4cf554b4?x=1", "", "avtr_4cf554b4-430c-4f8f-b53e-1f294eed230b"] {
            assert!(world_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_multipart_body_is_made_as_it_is_sent() {
        let dir = std::env::temp_dir().join(format!("orbiters-multipart-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let picture = dir.join("shot.png");
        std::fs::write(&picture, vec![7u8; 200_000]).unwrap();
        let parts =
            [Part::bytes("splat", "a.splat", "application/octet-stream", b"abc".to_vec()), Part::file("pictures", "shot.png", "image/png", &picture).unwrap()];
        let mut pieces: Vec<Piece> = parts
            .into_iter()
            .map(|p| match p.data {
                PartData::Bytes(b) => Piece::bytes(b),
                PartData::File(path, left) => Piece::File { path, file: None, left },
            })
            .collect();
        let total: u64 = pieces.iter().map(Piece::len).sum();
        assert_eq!(total, 200_003);
        pieces.reverse();
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let s = seen.clone();
        let mut body = Progress { pieces, sent: 0, total, report: Box::new(move |sent, _| s.store(sent, std::sync::atomic::Ordering::Relaxed)) };
        let mut out = Vec::new();
        body.read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), 200_003);
        assert!(out.starts_with(b"abc\x07"));
        assert_eq!(seen.load(std::sync::atomic::Ordering::Relaxed), 200_003);
        // A file that shrank after it was measured fails the request rather than sending less than announced.
        let part = Part::file("pictures", "shot.png", "image/png", &picture).unwrap();
        std::fs::write(&picture, b"short").unwrap();
        let PartData::File(path, left) = part.data else { unreachable!() };
        let mut body = Progress { pieces: vec![Piece::File { path, file: None, left }], sent: 0, total: left, report: Box::new(|_, _| {}) };
        assert!(body.read_to_end(&mut Vec::new()).is_err());
        std::fs::remove_dir_all(dir).ok();
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
        let hash = crate::visited::sha256_file(std::path::Path::new(&path)).unwrap();
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let s = seen.clone();
        let part = Part::file("pictures", "shot.png", "image/png", std::path::Path::new(&path)).unwrap();
        let added = api
            .add_pictures(&token, &world, vec![part], &["2026-10-18T18:37:41Z".into()], move |sent, _| s.store(sent, std::sync::atomic::Ordering::Relaxed))
            .unwrap();
        println!("added {} skipped {}", added.added, added.skipped);
        assert!(seen.load(std::sync::atomic::Ordering::Relaxed) > 0);
        let mine = api.mine(&token).unwrap();
        assert!(mine.worlds.iter().any(|w| w.world_id == world && w.pictures.iter().any(|p| p.source_hash == hash)));
    }
}
