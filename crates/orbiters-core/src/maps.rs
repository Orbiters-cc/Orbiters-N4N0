//! The Map explorer on Orbiters (server `routes/mapsRouter.js`): a world's public page, what this member added, and
//! adding 3D previews and pictures. Reading is public; adding uses the account the Unity tools share.

use crate::account::{Api, ApiError};
use anyhow::Context;
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A world id fit for a path of the API: VRChat's own shape, nothing else ([`crate::vrchat_log::is_world_id`]).
fn world_path(world_id: &str) -> anyhow::Result<&str> {
    anyhow::ensure!(crate::vrchat_log::is_world_id(world_id), "{}", tr!("Not a VRChat world: {world}", world = format!("{world_id:?}")));
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
    /// The first page of the members' pictures of it (newest first, 48 at most).
    #[serde(default, deserialize_with = "pictures")]
    pub pictures: Vec<WorldPicture>,
}

/// The server's list as its length, or the number a kept copy wrote (the page's `previews` is written as a count: read
/// strictly as a list, a kept page never read back).
fn count<'de, D: serde::Deserializer<'de>>(d: D) -> Result<usize, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Array(items) => items.len(),
        serde_json::Value::Number(n) => n.as_u64().map_or(0, |n| n as usize),
        _ => 0,
    })
}

/// A member's picture of a world on the Map explorer. Its addresses may be relative to the API.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldPicture {
    pub id: u64,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// RFC 3339.
    #[serde(default)]
    pub taken_at: Option<String>,
    /// Who shared it.
    #[serde(default)]
    pub by: Option<String>,
}

/// The page's pictures, each read on its own: one the server shapes otherwise is left out, never the page. Reads the
/// server's (`uploader: { name }`) and the kept copy's (`by`).
fn pictures<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<WorldPicture>, D::Error> {
    let raw = Option::<Vec<serde_json::Value>>::deserialize(d)?.unwrap_or_default();
    Ok(raw
        .into_iter()
        .filter_map(|v| {
            let text = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
            let whole = |k: &str| v.get(k).and_then(|x| x.as_u64()).and_then(|n| u32::try_from(n).ok());
            Some(WorldPicture {
                id: v.get("id")?.as_u64()?,
                url: text("url"),
                thumbnail_url: text("thumbnailUrl"),
                width: whole("width"),
                height: whole("height"),
                taken_at: text("takenAt"),
                by: text("by").or_else(|| v.get("uploader").and_then(|u| u.get("name")).and_then(|n| n.as_str()).map(str::to_string)),
            })
        })
        .collect())
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

/// Identity returned by an upload, persisted before a subsequent grant request can fail.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Uploaded {
    #[serde(rename = "type")]
    pub kind: crate::shares::Kind,
    pub id: u64,
    pub visibility: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UploadedPictures {
    pub added: Vec<Uploaded>,
    pub skipped: usize,
}

/// What a 3D preview sends: VRChat's splat trees for PC, Quest and the coarse first view (the server makes the website's
/// `.splat` from the PC tree's leaves: OrbitersContracts Docs/capture-sharing.md).
pub struct PreviewFiles {
    pub vrchat_pc: Vec<u8>,
    pub vrchat_quest: Vec<u8>,
    pub vrchat_coarse: Vec<u8>,
}

impl PreviewFiles {
    pub fn len(&self) -> u64 {
        (self.vrchat_pc.len() + self.vrchat_quest.len() + self.vrchat_coarse.len()) as u64
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

/// A list of VRChat worlds the overlay's world search shows: a search, or one of VRChat's own lists.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorldList {
    /// Worlds whose name matches (trimmed, at most [`SEARCH_MAX`] characters).
    Search(String),
    /// VRChat's popular worlds (Orbiters' featured worlds without a VRChat session).
    Popular,
    /// The worlds with the most people in them now (VRChat only).
    Active,
    /// What is hot right now: VRChat's own measure of a world's recent activity, `sort=heat` (VRChat only).
    Trending,
    /// The signed-in VRChat account's favorite worlds (VRChat only).
    Favorites,
    /// What VRChat highlights: the worlds of one of its Spotlight rows, latest updated first (VRChat only).
    Spotlight(Spotlight),
    /// Another of VRChat's own world rows ([`Row`]).
    Row(Row),
    /// One world favourite group of the signed-in VRChat account (a playlist of worlds in VRChat's menu), by its tag
    /// (`worlds1`…: [`crate::vrchat_api::Client::favorite_world_groups`]).
    FavoriteGroup(String),
}

/// VRChat's other world rows, as its client's Worlds menu has them (`GET /config`, `dynamicWorldRows`, read 2026-11-15),
/// with the sort and tag each asks `/worlds` for. Its other rows have lists of their own (Hot: trending, the Spotlight rows,
/// Active: active), or are the account's own (Recent: what this computer visited; Mine).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Row {
    CommunityLabs,
    Random,
    AvatarWorlds,
    Games,
    UpdatedRecently,
    New,
    Classics,
}

impl Row {
    /// The row's `sort`, and its `tag` when it has one.
    pub fn query(self) -> (&'static str, Option<&'static str>) {
        match self {
            Row::CommunityLabs => ("labs", None),
            Row::Random => ("shuffle", None),
            Row::AvatarWorlds => ("heat", Some("author_tag_avatar")),
            Row::Games => ("heat", Some("author_tag_game")),
            Row::UpdatedRecently => ("updated", None),
            Row::New => ("publication", None),
            Row::Classics => ("trending", None),
        }
    }
}

/// VRChat's Spotlight rows, as its client's world rows name them (`GET /config`, `dynamicWorldRows`, read 2026-11-15:
/// "Spotlight - PC" is `tag=admin_spotlight_pc`, "Spotlight - Cross Platform" `tag=admin_spotlight_xplat`, both sorted by
/// `updated`, descending).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Spotlight {
    Pc,
    CrossPlatform,
}

impl Spotlight {
    /// The tag VRChat gives the worlds of the row.
    pub fn tag(self) -> &'static str {
        match self {
            Spotlight::Pc => "admin_spotlight_pc",
            Spotlight::CrossPlatform => "admin_spotlight_xplat",
        }
    }
}

/// Worlds a page of a [`WorldList`] holds, as the Map explorer pages them.
pub const FOUND_PAGE: usize = 24;
/// Pages of a list read at most (VRChat answers deep offsets slowly, and nobody pages that far in a headset).
pub const FOUND_PAGES: usize = 20;
/// Characters a search sends at most (the Map explorer keeps 100).
pub const SEARCH_MAX: usize = 100;

/// Who answered a [`WorldList`]: VRChat through the member's own session, or Orbiters' public Map explorer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FoundBy {
    Vrchat,
    #[default]
    Orbiters,
}

/// A world of a [`WorldList`], with what Orbiters knows of it when Orbiters answered (VRChat's answer knows none).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FoundWorld {
    pub world: World,
    /// People in it now, when VRChat said.
    #[serde(default)]
    pub occupants: Option<u32>,
    /// Members' pictures of it on Orbiters.
    #[serde(default)]
    pub pictures: usize,
    /// Orbiters has a 3D preview of it.
    #[serde(default)]
    pub preview: bool,
}

/// A page of a [`WorldList`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FoundWorlds {
    pub items: Vec<FoundWorld>,
    /// Another page follows.
    pub more: bool,
    pub source: FoundBy,
}

/// The Map explorer's path for a page of `list` (`maps/search?q=…&page=…`, `maps/featured?page=…`); `None` for the
/// lists only VRChat has (a member's favorites, the active worlds).
pub fn found_path(list: &WorldList, page: usize) -> Option<String> {
    let page = page.min(FOUND_PAGES);
    match list {
        WorldList::Search(q) => {
            let q: String = q.trim().chars().take(SEARCH_MAX).collect();
            Some(format!("maps/search?q={}&page={page}", crate::vrchat_api::encode(&q)))
        }
        WorldList::Popular => Some(format!("maps/featured?page={page}")),
        WorldList::Active | WorldList::Trending | WorldList::Favorites | WorldList::Spotlight(_) | WorldList::Row(_) | WorldList::FavoriteGroup(_) => None,
    }
}

/// A Map explorer answer (`{ items: [world + { preview, pictures, performance }], hasMore, source }`): each item read on
/// its own, one shaped otherwise left out, never the page.
pub fn parse_found(json: &str) -> anyhow::Result<FoundWorlds> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    let items = v.get("items").and_then(|i| i.as_array()).ok_or_else(|| anyhow::anyhow!("{}", tr("not a list of worlds")))?;
    let items = items
        .iter()
        .filter_map(|item| {
            let world: World = serde_json::from_value(item.clone()).ok()?;
            crate::vrchat_log::is_world_id(&world.world_id).then_some(())?;
            let pictures = item.get("pictures").and_then(|p| p.as_f64()).filter(|n| n.is_finite() && *n >= 0.0).map_or(0, |n| n as usize);
            Some(FoundWorld { world, occupants: None, pictures, preview: item.get("preview").is_some_and(|p| p.is_object()) })
        })
        .collect();
    Ok(FoundWorlds { items, more: v.get("hasMore").and_then(|m| m.as_bool()).unwrap_or(false), source: FoundBy::Orbiters })
}

impl Api {
    /// A world's page; `None` when Orbiters does not list it (not public on VRChat). A server without the Map
    /// explorer (its 404 is not Orbiters' JSON error) is an error, not an unlisted world.
    pub fn world(&self, world_id: &str) -> anyhow::Result<Option<WorldPage>> {
        let response = self.agent().get(&self.url(&format!("maps/worlds/{}", world_path(world_id)?))).call()?;
        if response.status().as_u16() == 404 {
            let text = response.into_body().with_config().limit(64 * 1024).read_to_string().unwrap_or_default();
            let ours = serde_json::from_str::<serde_json::Value>(&text).ok().is_some_and(|v| v.get("error").is_some_and(|e| e.is_string()));
            anyhow::ensure!(ours, "{}", tr("this Orbiters server has no Map explorer"));
            return Ok(None);
        }
        Ok(Some(Self::json(response)?))
    }

    /// A page of `list` from the Map explorer (public, no credential: VRChat's search through the website's account, or
    /// the worlds Orbiters knows while VRChat cannot be asked). The lists only VRChat has are an error saying so.
    pub fn find_worlds(&self, list: &WorldList, page: usize) -> anyhow::Result<FoundWorlds> {
        let Some(path) = found_path(list, page) else {
            anyhow::bail!(
                "{}",
                match list {
                    WorldList::Favorites => tr("Sign in to VRChat in Orbiters - N4N0's Settings to see your favorite worlds."),
                    _ => tr("Sign in to VRChat in Orbiters - N4N0's Settings to see the worlds people are in now."),
                }
            )
        };
        let response = self.agent().get(&self.url(&path)).call()?;
        let status = response.status().as_u16();
        let text = response.into_body().with_config().limit(4 << 20).read_to_string()?;
        if !(200..300).contains(&status) {
            let message = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v.get("error")?.as_str().map(str::to_string));
            return Err(ApiError { status, message: message.unwrap_or_default() }.into());
        }
        parse_found(&text)
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

    pub fn add_shared_pictures(&self, token: &str, world_id: &str, pictures: Vec<Part>, metadata: &serde_json::Value) -> anyhow::Result<UploadedPictures> {
        let response = self.multipart(token, &format!("maps/worlds/{}/pictures", world_path(world_id)?), &metadata.to_string(), pictures, |_, _| {})?;
        crate::shares::json(response)
    }

    /// Adds a 3D preview with its sky map (a reconstruction's sky.png, when there is one), replacing this member's
    /// earlier one of the world: VRChat's splat trees (OrbitersContracts Docs/splat-tree.md), the PC one also the
    /// website's `.splat`. `metadata` holds the start view (`{"view":{"position":[…],"target":[…],"floor":…,
    /// "eyeHeight":…}}`), quality and capture date.
    pub fn add_preview(
        &self,
        token: &str,
        world_id: &str,
        files: PreviewFiles,
        sky: Option<&Path>,
        picture: Option<Vec<u8>>,
        metadata: &serde_json::Value,
        progress: impl FnMut(u64, u64) + Send + 'static,
    ) -> anyhow::Result<Uploaded> {
        let world_id = world_path(world_id)?;
        let tree = |field: &'static str, name: &str, bytes: Vec<u8>| Part::bytes(field, format!("{world_id}-{name}.n4tree"), "application/octet-stream", bytes);
        let mut parts = vec![
            tree("vrchatPc", "pc", files.vrchat_pc),
            tree("vrchatQuest", "quest", files.vrchat_quest),
            tree("vrchatCoarse", "coarse", files.vrchat_coarse),
        ];
        if let Some(sky) = sky {
            parts.push(Part::file("sky", "sky.png", "image/png", sky)?);
        }
        // The capture's own 3D view (contracts Docs/capture-sharing.md, Capture picture): its link previews and cards.
        if let Some(picture) = picture {
            parts.push(Part::bytes("picture", format!("{world_id}-capture.jpg"), "image/jpeg", picture));
        }
        let response = self.multipart(token, &format!("maps/worlds/{world_id}/previews"), &metadata.to_string(), parts, progress)?;
        crate::shares::json(response)
    }

    /// Replaces the files of the capture `id` (its owner's: contracts Docs/capture-sharing.md, Updating a capture) with a
    /// complete new set made after its splats were edited: VRChat's three trees under a key of their own, the sky map and
    /// the capture's picture (the old ones go when these have none), `metadata` as [`Api::add_preview`]'s, naming the
    /// `revision` replaced. Answers its item at the next revision. Refused as a [`crate::shares::RevisionConflict`] when
    /// its files are at another revision; any refusal leaves its files as they were.
    #[allow(clippy::too_many_arguments)]
    pub fn replace_capture_files(
        &self,
        token: &str,
        id: u64,
        revision: u64,
        files: PreviewFiles,
        sky: Option<&Path>,
        picture: Option<Vec<u8>>,
        metadata: &serde_json::Value,
        progress: impl FnMut(u64, u64) + Send + 'static,
    ) -> anyhow::Result<crate::shares::Item> {
        #[derive(serde::Deserialize)]
        struct Answer {
            item: crate::shares::Item,
        }
        let tree = |field: &'static str, name: &str, bytes: Vec<u8>| Part::bytes(field, format!("capture-{id}-{name}.n4tree"), "application/octet-stream", bytes);
        let mut parts = vec![
            tree("vrchatPc", "pc", files.vrchat_pc),
            tree("vrchatQuest", "quest", files.vrchat_quest),
            tree("vrchatCoarse", "coarse", files.vrchat_coarse),
        ];
        if let Some(sky) = sky {
            parts.push(Part::file("sky", "sky.png", "image/png", sky)?);
        }
        if let Some(picture) = picture {
            parts.push(Part::bytes("picture", format!("capture-{id}.jpg"), "image/jpeg", picture));
        }
        let mut metadata = metadata.clone();
        anyhow::ensure!(metadata.is_object(), "{}", tr("The capture's details are not an object"));
        metadata["revision"] = serde_json::json!(revision);
        let response = self.multipart_with("PUT", token, &format!("captures/capture/{id}/files"), &metadata.to_string(), parts, progress)?;
        Ok(crate::shares::json::<Answer>(response)?.item)
    }

    /// Gives a capture uploaded without one its picture (a JPEG of its 3D view), or a new one; answers its item.
    pub fn set_capture_picture(&self, token: &str, id: u64, jpeg: Vec<u8>) -> anyhow::Result<crate::shares::Item> {
        #[derive(serde::Deserialize)]
        struct Answer {
            item: crate::shares::Item,
        }
        let parts = vec![Part::bytes("picture", format!("capture-{id}.jpg"), "image/jpeg", jpeg)];
        let response = self.multipart(token, &format!("captures/capture/{id}/picture"), "{}", parts, |_, _| {})?;
        Ok(crate::shares::json::<Answer>(response)?.item)
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
        self.multipart_with("POST", token, path, metadata, parts, progress)
    }

    /// [`Api::multipart`] with another method (`PUT`: a capture's files replaced).
    fn multipart_with(
        &self,
        method: &str,
        token: &str,
        path: &str,
        metadata: &str,
        parts: Vec<Part>,
        progress: impl FnMut(u64, u64) + Send + 'static,
    ) -> anyhow::Result<ureq::http::Response<ureq::Body>> {
        let boundary = format!("----orbiters-{}", uuid::Uuid::new_v4().simple());
        let mut pieces = vec![Piece::bytes(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{metadata}\r\n"))];
        // These are multipart fields in the sharing contract, alongside the existing metadata JSON.
        let fields: serde_json::Value = serde_json::from_str(metadata)?;
        for field in ["visibility", "explicit", "explicitReasons", "acknowledgeExplicit", "instanceId", "sourceKey"] {
            if let Some(value) = fields.get(field).filter(|v| !v.is_null()) {
                let value = value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string());
                pieces.push(Piece::bytes(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"\r\n\r\n{value}\r\n")));
            }
        }
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
        let url = self.url(path);
        let request = if method == "PUT" { self.agent().put(&url) } else { self.agent().post(&url) };
        request
            .header("Authorization", &format!("Bearer {token}"))
            .header("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
            .header("Content-Length", &total.to_string())
            .config()
            .timeout_global(Some(Duration::from_secs(30 * 60)))
            .build()
            .send(ureq::SendBody::from_owned_reader(reader))
            .with_context(|| tr("Orbiters could not be reached"))
    }

    /// A public picture (VRChat's world thumbnails), at most 16 MB, over https only: a redirect to plain http is
    /// refused too.
    pub fn image(&self, url: &str) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(url.starts_with("https://"), "{}", tr("not an https address"));
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
                    return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, tr!("{path} changed while it was sent", path = path.display())));
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

    /// A capture uploaded without its picture gets one (contracts Docs/capture-sharing.md, Capture picture), against a
    /// disposable Orbiters server: ORBITERS_TEST_API, ORBITERS_TEST_TOKEN, ORBITERS_TEST_CAPTURE (an id the token's
    /// member owns) and ORBITERS_TEST_PICTURE (a JPEG).
    #[test]
    #[ignore = "needs a disposable Orbiters server"]
    fn a_capture_gets_its_picture() {
        let env = |k: &str| std::env::var(k).unwrap();
        let api = crate::account::Api::at(&env("ORBITERS_TEST_API"));
        let jpeg = std::fs::read(env("ORBITERS_TEST_PICTURE")).unwrap();
        let item = api.set_capture_picture(&env("ORBITERS_TEST_TOKEN"), env("ORBITERS_TEST_CAPTURE").parse().unwrap(), jpeg).unwrap();
        let capture = item.capture.unwrap();
        assert_eq!(capture.picture_url.as_deref(), Some(format!("/captures/capture/{}/file?part=picture", item.id).as_str()));
    }

    #[test]
    fn reads_a_world_page_and_what_a_member_added() {
        let page: WorldPage = serde_json::from_str(
            r#"{"world":{"worldId":"wrld_a","name":"Palm Point","authorName":"Sam","thumbnailUrl":"https://api.vrchat.cloud/x/256","capacity":32,"visits":12,"tags":["chill"]},
               "previews":[{"id":1},{"id":2}],"pictures":[],"pictureCount":7,"hasMorePictures":false}"#,
        )
        .unwrap();
        assert_eq!(page.world.name, "Palm Point");
        assert_eq!((page.previews, page.picture_count), (2, 7));
        // Its pictures, each on its own: one without an id is left out, the page still read; the kept copy reads back.
        let page: WorldPage = serde_json::from_str(
            r#"{"world":{"worldId":"wrld_a","name":"Palm Point"},"pictureCount":2,"pictures":[
                {"id":41,"url":"https://files.orbiters.cc/p/41.jpg","thumbnailUrl":"/files/serve/41?thumbnail=true","width":1920,"height":1080,
                 "takenAt":"2026-10-18T18:37:41.000Z","uploader":{"id":3,"name":"Kana"}},{"url":"no id"}]}"#,
        )
        .unwrap();
        assert_eq!(page.pictures.len(), 1);
        assert_eq!((page.pictures[0].id, page.pictures[0].by.as_deref(), page.pictures[0].width), (41, Some("Kana"), Some(1920)));
        let kept: WorldPage = serde_json::from_str(&serde_json::to_string(&page).unwrap()).unwrap();
        assert_eq!(kept, page);
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
    fn world_lists_go_to_the_map_explorer_where_it_has_them() {
        assert_eq!(found_path(&WorldList::Search(" Palm Point ".into()), 2).as_deref(), Some("maps/search?q=Palm%20Point&page=2"));
        assert_eq!(found_path(&WorldList::Popular, 99).as_deref(), Some("maps/featured?page=20"));
        assert_eq!(found_path(&WorldList::Favorites, 0), None);
        assert_eq!(found_path(&WorldList::Active, 0), None);
        assert_eq!(found_path(&WorldList::Spotlight(Spotlight::Pc), 0), None, "only VRChat knows its Spotlight");
        let found = parse_found(
            r#"{"items":[{"worldId":"wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b","name":"Palm Point","pictures":2.0,"preview":null},
            {"worldId":"wrld_../x","name":"bad"},{"worldId":"wrld_4cf554b4-430c-4f8f-b53e-1f294eed230c"}],"hasMore":false}"#,
        )
        .unwrap();
        assert_eq!(found.items.len(), 1);
        assert_eq!((found.items[0].pictures, found.items[0].preview, found.more), (2, false, false));
        assert!(parse_found(r#"{"error":"Type at least two characters."}"#).is_err());
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
