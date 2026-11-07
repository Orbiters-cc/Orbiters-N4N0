//! The 3D captures of worlds the overlay can show: the listing the VRChat world viewer reads (Orbiters'
//! `GET /maps/vrchat/previews`, contracts `Docs/splat-tree.md`), searched like its menu does (world, author or sharer
//! containing what is typed), with each world's VRChat numbers and its feedback for everyone (likes, labels, comments:
//! contracts `Docs/world-feedback.md`, For VRChat worlds), a capture shared with some players only listed for them, and
//! a capture's files: the slot's PC tree and sky map at the public storage addresses the world loads (production), or
//! the API's web `.splat` and sky map of the preview elsewhere (a development server publishes its slots to storage the
//! app has no address for).

use super::cache::{Disk, Validators};
use super::stereo::{self, StartView};
use orbiters_core::account::{Api, Server};
use orbiters_reconstruct::gaussians::Gaussians;
use orbiters_reconstruct::sky::SkyMap;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where production's slots are published.
const PUBLIC_FILES: &str = "https://files.orbiters.cc/";
/// Bytes a capture's file holds at most (a PC tree of 2 097 152 splats and its boxes).
const MAX_FILE: u64 = 72 << 20;
/// The listing is read again after this long, when the panel opens.
const LISTING_FOR: Duration = Duration::from_secs(60);

/// A world the panel lists: a 3D capture of it, as the listing gives it, or (`capture` false) a VRChat world without one
/// shown, as a search or one of VRChat's lists found it ([`Entry::world`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// A 3D capture (its listing id, slot and files): shown around the person. Otherwise a world's page only.
    pub capture: bool,
    /// The VRChat slot its files are published at (production's storage), when it holds one.
    pub slot: Option<u32>,
    /// Names the upload: its files carry the same.
    pub key: u32,
    pub id: u64,
    pub world_id: String,
    pub name: String,
    pub author: Option<String>,
    /// The world's description on VRChat (cut to 400 characters by the listing).
    pub description: String,
    /// Who shared it.
    pub uploader: Option<String>,
    /// The PC tree's splats.
    pub splats: u32,
    pub view: StartView,
    pub sky: bool,
    /// Shared by the world's own author.
    pub official: bool,
    /// When it was captured (RFC 3339).
    pub captured_at: Option<String>,
    /// The world's numbers on VRChat.
    pub capacity: Option<u32>,
    pub visits: Option<u64>,
    pub favorites: Option<u64>,
    /// Members' pictures of the world on Orbiters.
    pub pictures: u32,
    /// Shared with some players only (their VRChat names): the listing is public, so the viewer keeps it to them, as
    /// the world does.
    pub only_for: Option<Vec<String>>,
    /// The world's feedback for everyone, as the listing gives it (the server's, at most 30 s old then).
    pub feedback: Aggregate,
    /// Its picture on VRChat, when the list that found it said (a capture's comes with its world's page).
    pub thumbnail: Option<String>,
    /// VRChat's tags of it, when the list said.
    pub tags: Vec<String>,
    /// People in it now, when VRChat said.
    pub occupants: Option<u32>,
    /// How smoothly VRChat runs there, when Orbiters' answer said.
    pub performance: Option<orbiters_core::maps::WorldPerformance>,
    /// Orbiters has a 3D preview of it (the Map explorer's answer), though not one the overlay lists.
    pub preview: bool,
    /// Unix seconds of the person's latest visit, and how many visits (the Visited list).
    pub visited: Option<(u64, usize)>,
    /// The search (lower case) that found it: it stays a result of that search whatever its name.
    pub found_by: Option<String>,
    /// Lower-case text a search looks in: the world, its author and the sharer.
    search: String,
    /// A capture on this computer: its session, read from there and not downloaded.
    pub local: Option<std::path::PathBuf>,
}

/// Ids of worlds without a capture have this bit (the listing's ids are a database's, far under it).
pub const WORLD_BIT: u64 = 1 << 63;

/// The id of the world `world_id` as an entry without a capture: stable, never a capture's.
pub fn world_entry_id(world_id: &str) -> u64 {
    // FNV-1a: the same id in every run (the panel keeps a selection by it).
    let hash = world_id.to_ascii_lowercase().bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
    WORLD_BIT | (hash >> 1)
}

/// A world's likes, labels (highest count first, then the list's order) and comments for everyone, with its newest
/// comments: (by, text, RFC 3339).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Aggregate {
    pub likes: u32,
    pub labels: Vec<(String, u32)>,
    pub comments: u32,
    pub recent: Vec<(String, String, Option<String>)>,
}

impl Aggregate {
    pub fn label(&self, id: &str) -> u32 {
        self.labels.iter().find(|l| l.0 == id).map_or(0, |l| l.1)
    }
}

impl Entry {
    /// A world without a capture the overlay lists, as a list found it (`found_by`: the search that did), with what
    /// Orbiters says of it for everyone (`feedback`).
    pub fn world(found: &orbiters_core::maps::FoundWorld, found_by: Option<&str>, feedback: Aggregate) -> Self {
        let w = &found.world;
        let search = format!("{}\n{}", w.name, w.author_name.as_deref().unwrap_or("")).to_lowercase();
        Self {
            capture: false,
            slot: None,
            key: 0,
            id: world_entry_id(&w.world_id),
            world_id: w.world_id.clone(),
            name: w.name.clone(),
            author: w.author_name.clone(),
            description: w.description.clone().unwrap_or_default(),
            uploader: None,
            splats: 0,
            view: StartView { position: [0.0, 1.6, 0.0], target: [0.0, 1.6, -1.0], floor: None, eye_height: None },
            sky: false,
            official: false,
            captured_at: None,
            capacity: w.capacity.filter(|c| *c > 0),
            visits: w.visits,
            favorites: w.favorites,
            pictures: found.pictures.min(u32::MAX as usize) as u32,
            only_for: None,
            feedback,
            thumbnail: w.thumbnail_url.clone().or_else(|| w.image_url.clone()),
            tags: w.tags.clone(),
            occupants: found.occupants,
            performance: w.performance.clone().filter(|p| p.fps > 0),
            preview: found.preview,
            visited: None,
            found_by: found_by.map(|q| q.trim().to_lowercase()),
            search,
            local: None,
        }
    }

    /// A capture of this computer's gallery, for the 3D view. `changed` tells its results apart: made again, it is read again.
    pub fn local(dir: std::path::PathBuf, name: String, changed: u64) -> Self {
        let id = dir.to_string_lossy().to_ascii_lowercase().bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
        Self {
            capture: true,
            slot: None,
            key: changed as u32,
            // Neither a listing's id nor a world's.
            id: (id >> 2) | (1 << 62),
            world_id: String::new(),
            search: name.to_lowercase(),
            name,
            author: None,
            description: String::new(),
            uploader: None,
            splats: 0,
            view: StartView { position: [0.0, 1.6, 0.0], target: [0.0, 1.6, -1.0], floor: None, eye_height: None },
            sky: false,
            official: false,
            captured_at: None,
            capacity: None,
            visits: None,
            favorites: None,
            pictures: 0,
            only_for: None,
            feedback: Aggregate::default(),
            thumbnail: None,
            tags: Vec::new(),
            occupants: None,
            performance: None,
            preview: false,
            visited: None,
            found_by: None,
            local: Some(dir),
        }
    }

    /// Whether `query` (any case) is in its world's name, its author's or its sharer's, or found it.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        q.is_empty() || self.search.contains(&q) || self.found_by.as_deref() == Some(q.as_str())
    }

    /// Whether the player whose VRChat name is `me` may see it: everyone, or one of those it is shared with.
    pub fn for_player(&self, me: Option<&str>) -> bool {
        match &self.only_for {
            None => true,
            Some(names) => me.is_some_and(|me| names.iter().any(|n| n.trim().eq_ignore_ascii_case(me.trim()))),
        }
    }
}

#[derive(Deserialize)]
struct Listing {
    previews: Vec<RawEntry>,
    /// Read world by world: one shaped otherwise is left out, never the listing.
    #[serde(default)]
    feedback: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEntry {
    slot: Option<u32>,
    key: u64,
    id: u64,
    world_id: String,
    name: String,
    author: Option<String>,
    #[serde(default)]
    description: Option<String>,
    uploader: Option<String>,
    #[serde(default)]
    splats: RawSplats,
    view: RawView,
    #[serde(default)]
    sky: bool,
    #[serde(default)]
    official: bool,
    #[serde(default)]
    captured_at: Option<String>,
    #[serde(default)]
    capacity: Option<f64>,
    #[serde(default)]
    visits: Option<f64>,
    #[serde(default)]
    favorites: Option<f64>,
    #[serde(default)]
    pictures: Option<f64>,
    #[serde(default)]
    shared: bool,
    #[serde(default, rename = "for")]
    only_for: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
struct RawSplats {
    pc: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawView {
    position: Option<[f32; 3]>,
    target: Option<[f32; 3]>,
    floor: Option<f32>,
    eye_height: Option<f32>,
}

/// A world's feedback in the listing (`{ likes, labels: { id: count }, comments, recent: [{ by, text, at }] }`).
fn aggregate(v: &serde_json::Value) -> Aggregate {
    let whole = |v: Option<&serde_json::Value>| count(v.and_then(|n| n.as_f64())).map_or(0, |n| n.min(u32::MAX as u64) as u32);
    let order = |id: &str| orbiters_contracts::world_feedback::labels().iter().position(|l| l.id == id).unwrap_or(usize::MAX);
    let mut labels: Vec<(String, u32)> = v
        .get("labels")
        .and_then(|l| l.as_object())
        .map(|m| m.iter().map(|(id, n)| (id.clone(), whole(Some(n)))).filter(|l| l.1 > 0).collect())
        .unwrap_or_default();
    labels.sort_by(|a, b| b.1.cmp(&a.1).then(order(&a.0).cmp(&order(&b.0))));
    let text = |c: &serde_json::Value, k: &str| c.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let recent = v
        .get("recent")
        .and_then(|r| r.as_array())
        .map(|r| r.iter().filter_map(|c| Some((text(c, "by").unwrap_or_default(), text(c, "text")?, text(c, "at")))).collect())
        .unwrap_or_default();
    Aggregate { likes: whole(v.get("likes")), labels, comments: whole(v.get("comments")), recent }
}

/// A count the server may send as a decimal, or not at all.
fn count(n: Option<f64>) -> Option<u64> {
    n.filter(|n| n.is_finite() && *n >= 0.0).map(|n| n.round() as u64)
}

/// The listing's entries that have a start view (newest first, as listed).
pub fn parse(json: &str) -> anyhow::Result<Vec<Entry>> {
    let listing: Listing = serde_json::from_str(json)?;
    let feedback: HashMap<String, Aggregate> = listing.feedback.iter().map(|(world, v)| (world.to_ascii_lowercase(), aggregate(v))).collect();
    Ok(listing
        .previews
        .into_iter()
        .filter_map(|r| {
            let (position, target) = (r.view.position?, r.view.target?);
            let search = format!("{}\n{}\n{}", r.name, r.author.as_deref().unwrap_or(""), r.uploader.as_deref().unwrap_or("")).to_lowercase();
            let feedback = feedback.get(&r.world_id.to_ascii_lowercase()).cloned().unwrap_or_default();
            Some(Entry {
                capture: true,
                slot: r.slot,
                key: r.key as u32,
                id: r.id,
                world_id: r.world_id,
                name: r.name,
                author: r.author,
                description: r.description.unwrap_or_default(),
                uploader: r.uploader,
                splats: r.splats.pc.unwrap_or(0),
                view: StartView { position, target, floor: r.view.floor, eye_height: r.view.eye_height },
                sky: r.sky,
                official: r.official,
                captured_at: r.captured_at,
                capacity: count(r.capacity).map(|n| n.min(u32::MAX as u64) as u32),
                visits: count(r.visits),
                favorites: count(r.favorites),
                pictures: count(r.pictures).map_or(0, |n| n.min(u32::MAX as u64) as u32),
                only_for: r.shared.then(|| r.only_for.unwrap_or_default()),
                feedback,
                thumbnail: None,
                tags: Vec::new(),
                occupants: None,
                performance: None,
                preview: true,
                visited: None,
                found_by: None,
                search,
                local: None,
            })
        })
        .collect())
}

/// Orbiters' feedback for everyone on the worlds given some lately (`GET /maps/vrchat/feedback`: `{ feedback: { world:
/// { likes, labels, comments, recent } } }`), by world id in lower case: a world without a capture shows it too.
pub fn parse_feedback(json: &str) -> anyhow::Result<HashMap<String, Aggregate>> {
    #[derive(Deserialize)]
    struct All {
        #[serde(default)]
        feedback: HashMap<String, serde_json::Value>,
    }
    let all: All = serde_json::from_str(json)?;
    Ok(all.feedback.iter().filter(|(_, v)| v.is_object()).map(|(world, v)| (world.to_ascii_lowercase(), aggregate(v))).collect())
}

/// Where the listing and the files come from: the app's server (or `ORBITERS_PUBLIC_API`, as the window's public reads),
/// and production's public storage when that is the server; the files downloaded kept on disk ([`super::cache`]).
#[derive(Clone)]
pub struct Source {
    api: Api,
    files: Option<String>,
    disk: Option<Arc<Disk>>,
}

impl Source {
    pub fn current() -> Self {
        let disk = Some(Arc::new(Disk::in_cache()));
        match std::env::var("ORBITERS_PUBLIC_API") {
            Ok(base) if !base.is_empty() => Self { api: Api::at(&base), files: None, disk },
            _ => {
                let server = Server::current();
                Self { api: Api::new(server), files: (server == Server::Production).then(|| PUBLIC_FILES.to_string()), disk }
            }
        }
    }

    /// An address the API gave, relative to it or whole.
    pub fn absolute(&self, url: &str) -> String {
        if url.starts_with("https://") || url.starts_with("http://") {
            url.to_string()
        } else {
            self.api.url(url)
        }
    }

    pub fn listing(&self) -> anyhow::Result<Vec<Entry>> {
        let response = self.api.agent().get(&self.api.url("maps/vrchat/previews")).call()?;
        let status = response.status().as_u16();
        let text = response.into_body().with_config().limit(16 << 20).read_to_string()?;
        anyhow::ensure!((200..300).contains(&status), "Orbiters answered {status}");
        parse(&text)
    }

    /// Orbiters' feedback for everyone on the worlds given some lately ([`parse_feedback`]).
    pub fn all_feedback(&self) -> anyhow::Result<HashMap<String, Aggregate>> {
        let response = self.api.agent().get(&self.api.url("maps/vrchat/feedback")).call()?;
        let status = response.status().as_u16();
        let text = response.into_body().with_config().limit(8 << 20).read_to_string()?;
        anyhow::ensure!((200..300).contains(&status), "Orbiters answered {status}");
        parse_feedback(&text)
    }

    /// Downloads `entry`'s splats and sky map, `progress` counting bytes (and their total once known).
    /// Downloads `entry`'s splats and sky map (or takes the files kept on disk), `progress` counting bytes (and their
    /// total once known).
    pub fn download(&self, entry: &Entry, progress: &Progress) -> anyhow::Result<Capture> {
        let key = super::cache::Key::of(entry);
        let (g, sky) = match (&self.files, entry.slot) {
            (Some(files), Some(slot)) => {
                let name = key.file("pc");
                let tree = self.fetch_kept(&format!("{files}public/vrchat/slots/{slot}/pc"), &name, progress)?;
                let g = tree_leaves(&tree, entry.key).inspect_err(|_| self.forget(&name))?;
                drop(tree);
                let sky = entry.sky.then(|| self.fetch_kept(&format!("{files}public/vrchat/slots/{slot}/sky.jpg"), &key.file("sky"), &Progress::default()).ok()).flatten();
                (g, sky)
            }
            _ => {
                let name = key.file("splat");
                let bytes = self.fetch_kept(&self.api.url(&format!("maps/previews/{}/splat", entry.id)), &name, progress)?;
                let g = orbiters_reconstruct::web::read_splat(&bytes).inspect_err(|_| self.forget(&name))?;
                drop(bytes);
                let sky = entry
                    .sky
                    .then(|| self.fetch_kept(&self.api.url(&format!("maps/previews/{}/sky.jpg", entry.id)), &key.file("sky"), &Progress::default()).ok())
                    .flatten();
                (g, sky)
            }
        };
        progress.stage.store(STAGE_PREPARING, Ordering::Relaxed);
        let sky = sky.and_then(|bytes| sky_map(&bytes).map_err(|e| eprintln!("3D worlds: sky map of {}: {e:#}", entry.name)).ok());
        Ok(Capture::new(g, sky, entry.view))
    }

    fn forget(&self, name: &str) {
        if let Some(disk) = &self.disk {
            disk.remove(name);
        }
    }

    /// The file at `url`: the one kept on disk as `name` when it is still the storage's (asked when the storage said
    /// how, a `304` sending nothing; trusted otherwise, its name being its upload's), else downloaded and kept.
    fn fetch_kept(&self, url: &str, name: &str, progress: &Progress) -> anyhow::Result<Vec<u8>> {
        let Some(disk) = self.disk.as_deref() else { return self.fetch(url, None, progress).map(|(b, _)| b.unwrap_or_default()) };
        let kept = disk.read(name).filter(|(_, v)| v.url == url);
        let whole = |bytes: &[u8]| {
            progress.total.store(bytes.len() as u64, Ordering::Relaxed);
            progress.read.store(bytes.len() as u64, Ordering::Relaxed);
        };
        if let Some((bytes, validators)) = &kept {
            if !validators.askable() {
                disk.touch(name);
                whole(bytes);
                return Ok(bytes.clone());
            }
        }
        match self.fetch(url, kept.as_ref().map(|(_, v)| v), progress) {
            Ok((None, _)) => {
                // Not modified.
                let (bytes, _) = kept.expect("asked with what was kept");
                disk.touch(name);
                whole(&bytes);
                Ok(bytes)
            }
            Ok((Some(bytes), validators)) => {
                disk.write(name, &bytes, &validators);
                Ok(bytes)
            }
            // Unreachable: what was kept is the listing's upload all the same.
            Err(e) => match kept {
                Some((bytes, _)) if !format!("{e:#}").contains("answered 4") => {
                    disk.touch(name);
                    whole(&bytes);
                    Ok(bytes)
                }
                _ => Err(e),
            },
        }
    }

    /// Downloads `url` (`None` when `kept` is still what the storage has), with what the storage said of it.
    fn fetch(&self, url: &str, kept: Option<&Validators>, progress: &Progress) -> anyhow::Result<(Option<Vec<u8>>, Validators)> {
        let mut request = self.api.agent().get(url);
        if let Some(v) = kept {
            if let Some(etag) = &v.etag {
                request = request.header("If-None-Match", etag);
            }
            if let Some(date) = &v.last_modified {
                request = request.header("If-Modified-Since", date);
            }
        }
        let response = request.call()?;
        let status = response.status().as_u16();
        if status == 304 && kept.is_some() {
            return Ok((None, kept.cloned().unwrap_or_default()));
        }
        anyhow::ensure!((200..300).contains(&status), "{url} answered {status}");
        let header = |k: &str| response.headers().get(k).and_then(|v| v.to_str().ok()).map(str::to_string);
        let validators = Validators { url: url.to_string(), etag: header("etag"), last_modified: header("last-modified") };
        let total = response.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
        if let Some(total) = total {
            anyhow::ensure!(total <= MAX_FILE, "{url} is {total} bytes");
            progress.total.store(total, Ordering::Relaxed);
        }
        let mut reader = response.into_body().into_with_config().limit(MAX_FILE).reader();
        let mut out = Vec::with_capacity(total.unwrap_or(0) as usize);
        let mut buf = vec![0u8; 256 << 10];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
            progress.read.store(out.len() as u64, Ordering::Relaxed);
        }
        Ok((Some(out), validators))
    }
}

/// How a download goes: bytes read, bytes in all (0 until known), and its stage.
#[derive(Default)]
pub struct Progress {
    pub read: AtomicU64,
    pub total: AtomicU64,
    pub stage: AtomicU64,
}

pub const STAGE_PREPARING: u64 = 1;

impl Progress {
    /// 0 to 1 over the download (a capture's file is most of the wait).
    pub fn fraction(&self) -> f32 {
        let (read, total) = (self.read.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed));
        if total == 0 {
            0.0
        } else {
            (read as f64 / total as f64).clamp(0.0, 1.0) as f32
        }
    }
}

/// The finest splats of a PC tree (contracts `Docs/splat-tree.md`, version 2): its leaf boxes', in the renderer's
/// coordinates, as their records decode. A file whose key is not `key` was replaced at its slot since the listing; one
/// of version 1 (a capture shared before the 20-byte records) has to be shared again.
pub fn tree_leaves(bytes: &[u8], key: u32) -> anyhow::Result<Gaussians> {
    anyhow::ensure!(orbiters_reconstruct::tree_file::key(bytes)? == key, "this capture was replaced on Orbiters: refresh the list");
    Ok(orbiters_reconstruct::tree::Tree::parse(bytes)?.leaf_splats())
}

/// A sky map JPEG (2:1, the world's `sky.jpg`) as the renderer's map: resized to a width that is a multiple of four
/// when it is not, at most 2048 across.
pub fn sky_map(bytes: &[u8]) -> anyhow::Result<SkyMap> {
    let image = image::load_from_memory(bytes)?.to_rgb8();
    let width = (image.width().min(2048) / 4 * 4).max(4);
    let image = if image.width() != width || image.height() != width / 2 {
        image::imageops::resize(&image, width, width / 2, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    Ok(SkyMap { width, rgb: image.pixels().map(|p| p.0.map(|c| c as f32 / 255.0)).collect() })
}

/// A capture ready to draw: its splats ordered outward from the start eye (the reveal draws the first ones), their
/// distances, its sky map and start view.
pub struct Capture {
    pub gaussians: Gaussians,
    /// Each splat's distance from the start eye, ascending.
    pub distances: Vec<f32>,
    pub sky: Option<SkyMap>,
    pub view: StartView,
}

impl Capture {
    pub fn new(g: Gaussians, sky: Option<SkyMap>, view: StartView) -> Self {
        let eye = view.eye();
        let distance = |i: usize| stereo::length(stereo::sub(g.means[i], eye));
        let mut order: Vec<(f32, usize)> = (0..g.len()).filter(|&i| g.is_finite(i)).map(|i| (distance(i), i)).collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut sorted = Gaussians::default();
        for &(_, i) in &order {
            sorted.push(g.means[i], g.log_scales[i], g.rotations[i], g.opacity_logits[i], g.colors[i]);
        }
        Self { gaussians: sorted, distances: order.into_iter().map(|(d, _)| d).collect(), sky, view }
    }

    /// The same capture to show small: in order of distance from what its start view looks at, around which it stands.
    pub fn around_subject(&self) -> Self {
        let ahead = stereo::add(self.view.target, stereo::sub(self.view.target, self.view.position));
        let view = StartView { position: self.view.target, target: ahead, floor: self.view.floor, eye_height: self.view.eye_height };
        Self::new(self.gaussians.clone(), None, view)
    }

    /// What it shows small at first, around its subject: up to where it was seen from. Seen from outside, a capture is a
    /// shell of what surrounded its camera, and past that point what lay behind the camera would stand in the way.
    pub fn small_reach(&self) -> f32 {
        (stereo::length(stereo::sub(self.view.target, self.view.position)) * 1.1).clamp(1.0, 30.0)
    }

    /// The splats within `radius` of the start eye (the first ones).
    pub fn within(&self, radius: f32) -> usize {
        self.distances.partition_point(|d| *d <= radius)
    }

    pub fn farthest(&self) -> f32 {
        self.distances.last().copied().unwrap_or(1.0)
    }
}

/// A session of this computer as the 3D view takes it: its splats less what was edited out, its sky, and the start view its upload would say.
pub fn read_local(dir: &std::path::Path, progress: &Progress) -> anyhow::Result<Capture> {
    use orbiters_core::reconstruction;
    progress.stage.store(STAGE_PREPARING, Ordering::Relaxed);
    let result = reconstruction::result_dir(dir);
    let (mut g, _, found) = orbiters_reconstruct::edit::read_for_view(&result.join(reconstruction::PLY_FILE))?;
    // Edits of an earlier reconstruction are no edits of this one.
    if let orbiters_reconstruct::edit::Found::Current(edit) = &found {
        orbiters_reconstruct::edit::apply(&mut g, edit);
    }
    let sky_file = result.join(reconstruction::SKY_FILE);
    let sky = if sky_file.is_file() { Some(SkyMap::read(&sky_file)?) } else { None };
    let (views, start) = crate::views::recorded(dir)?;
    let (v, ground) = crate::views::eye_level(dir, &g, &views[start]);
    let depth = crate::views::depth_ahead(&g, &v);
    let target = [v.position[0] + v.forward[0] * depth, v.position[1] + v.forward[1] * depth, v.position[2] + v.forward[2] * depth];
    let flip = |p: [f32; 3]| [p[0], p[1], -p[2]];
    Ok(Capture::new(g, sky, StartView { position: flip(v.position), target: flip(target), floor: Some(ground.floor), eye_height: Some(ground.eye_height) }))
}

/// The listing, read on a thread of its own when asked (at most once a minute).
#[derive(Default)]
pub struct Catalog {
    state: Arc<Mutex<Listed>>,
}

#[derive(Default)]
struct Listed {
    entries: Vec<Entry>,
    /// Every world's feedback for everyone Orbiters gave (kept from the last answer when it fails).
    feedback: HashMap<String, Aggregate>,
    error: Option<String>,
    fetching: bool,
    fetched: Option<Instant>,
    /// Bumped with each new answer.
    version: u64,
}

impl Catalog {
    /// Reads the listing again unless it was read lately or is being read.
    pub fn refresh(&self, source: &Source) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if s.fetching || s.fetched.is_some_and(|t| t.elapsed() < LISTING_FOR) {
            return;
        }
        s.fetching = true;
        let (state, source) = (self.state.clone(), source.clone());
        let _ = std::thread::Builder::new().name("3d-worlds-listing".into()).spawn(move || {
            let answer = source.listing();
            let feedback = source.all_feedback().map_err(|e| eprintln!("3D worlds: worlds' feedback: {e:#}")).ok();
            let mut s = state.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(feedback) = feedback {
                s.feedback = feedback;
            }
            s.fetching = false;
            s.version += 1;
            match answer {
                Ok(entries) => {
                    s.entries = entries;
                    s.error = None;
                    s.fetched = Some(Instant::now());
                }
                Err(e) => s.error = Some(format!("{e:#}")),
            }
        });
    }

    /// The entries, whether a read is under way, the last error, and the answer's number.
    pub fn snapshot(&self) -> (Vec<Entry>, bool, Option<String>, u64) {
        let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        (s.entries.clone(), s.fetching, s.error.clone(), s.version)
    }

    /// Whether a read is under way, and the last error (without copying the entries).
    pub fn state(&self) -> (bool, Option<String>) {
        let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        (s.fetching, s.error.clone())
    }

    /// A world's feedback for everyone as Orbiters last gave it: its capture's listing's, else the one of every world.
    pub fn feedback(&self, world_id: &str) -> Aggregate {
        let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let key = world_id.to_ascii_lowercase();
        s.entries
            .iter()
            .find(|e| e.world_id.eq_ignore_ascii_case(world_id))
            .map(|e| e.feedback.clone())
            .or_else(|| s.feedback.get(&key).cloned())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = r#"{"slots":1024,"previews":[
        {"slot":0,"key":2140057312,"id":7,"worldId":"wrld_home","name":"VRChat Home","author":"vrchat","uploader":"blackorbit",
         "splats":{"pc":1026271,"quest":513098,"coarse":42801},
         "view":{"position":[-0.148,1.469,-0.35],"target":[2.627,1.046,2.227],"floor":null,"eyeHeight":null},"sky":false,"official":false},
        {"slot":null,"key":5,"id":9,"worldId":"wrld_dawn","name":"Silent Dawn","author":"Mei","uploader":"someone",
         "splats":{},"view":{"position":[1,2,3],"target":[1,2,4],"floor":-4.33,"eyeHeight":1.66},"sky":true},
        {"slot":3,"key":6,"id":10,"worldId":"wrld_x","name":"No view","author":null,"uploader":null,"view":{"position":null,"target":null}}
    ],"feedback":{"WRLD_HOME":{"likes":41,"labels":{"mirror":9,"meetup":12,"pens":9,"gone":0},"comments":7,
        "recent":[{"by":"Kana","text":"The mirror room is perfect for photos.","at":"2026-10-29T21:04:00Z"},{"text":"no author"},{"by":"x"}]},
        "wrld_dawn":"not an object"}}"#;

    #[test]
    fn the_listing_keeps_entries_with_a_start_view() {
        let entries = parse(LISTING).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].slot, Some(0));
        assert_eq!(entries[0].key, 2140057312);
        assert_eq!(entries[0].splats, 1026271);
        assert_eq!(entries[0].view.floor, None);
        assert_eq!(entries[1].slot, None);
        assert_eq!(entries[1].view.floor, Some(-4.33));
        assert_eq!(entries[1].view.eye_height, Some(1.66));
        assert!(entries[1].sky);
    }

    #[test]
    fn the_listing_gives_each_world_its_numbers_and_feedback() {
        let entries = parse(LISTING).unwrap();
        let home = &entries[0].feedback;
        assert_eq!((home.likes, home.comments), (41, 7));
        assert_eq!(home.labels, [("meetup".to_string(), 12), ("mirror".to_string(), 9), ("pens".to_string(), 9)], "highest first, then the list's order");
        assert_eq!(home.label("mirror"), 9);
        assert_eq!(home.recent.len(), 2, "a comment without its text is left out");
        assert_eq!(home.recent[1].0, "");
        assert_eq!(entries[1].feedback, Aggregate::default(), "feedback shaped otherwise: none, the listing still read");
        let json = r#"{"previews":[{"slot":1,"key":1,"id":1,"worldId":"wrld_a","name":"A","author":null,"uploader":null,
            "view":{"position":[0,1,0],"target":[0,1,1]},"capturedAt":"2026-10-12T20:00:00Z","capacity":32,"visits":12345.0,"favorites":-1,
            "pictures":4,"shared":true,"for":["Mei"," Kana "]}]}"#;
        let a = &parse(json).unwrap()[0];
        assert_eq!((a.capacity, a.visits, a.favorites, a.pictures), (Some(32), Some(12345), None, 4));
        assert_eq!(a.captured_at.as_deref(), Some("2026-10-12T20:00:00Z"));
        assert!(a.for_player(Some("kana")) && a.for_player(Some("Mei")));
        assert!(!a.for_player(Some("someone")) && !a.for_player(None), "shared with others: not listed for this player");
        assert!(entries[0].for_player(None), "public: everyone");
    }

    #[test]
    fn search_looks_in_the_world_its_author_and_the_sharer() {
        let entries = parse(LISTING).unwrap();
        let find = |q: &str| entries.iter().filter(|e| e.matches(q)).map(|e| e.id).collect::<Vec<_>>();
        assert_eq!(find(""), vec![7, 9]);
        assert_eq!(find("home"), vec![7]);
        assert_eq!(find("MEI"), vec![9]);
        assert_eq!(find("blackorbit"), vec![7]);
        assert_eq!(find("  dawn "), vec![9]);
        assert!(find("nothing like it").is_empty());
    }

    #[test]
    fn a_tree_gives_its_leaf_boxes_splats() {
        use orbiters_reconstruct::tree::{Node, Tree};
        // A root with one merged splat and two leaves of two and one.
        let mut splats = Gaussians::default();
        for x in [9.0f32, 1.0, 2.0, 3.0] {
            splats.push([x, 0.0, 0.0], [0.1f32.ln(); 3], [1.0, 0.0, 0.0, 0.0], 3.0, [200.0 / 255.0, 100.0 / 255.0, 50.0 / 255.0]);
        }
        let node = |error: f32, parent: Option<u32>, start: u32, count: u32| Node {
            center: [2.0, 0.0, 0.0],
            radius: 9.0,
            error,
            parent,
            depth: u32::from(parent.is_some()),
            start,
            count,
            leaf: error == 0.0,
        };
        let tree = Tree { nodes: vec![node(0.5, None, 0, 1), node(0.0, Some(0), 1, 2), node(0.0, Some(0), 3, 1)], splats };
        let file = tree.file(77, 0);
        let g = tree_leaves(&file, 77).unwrap();
        let xs: Vec<f32> = g.means.iter().map(|m| m[0]).collect();
        assert!(xs.len() == 3 && xs.iter().zip([1.0, 2.0, 3.0]).all(|(a, b)| (a - b).abs() < 1e-4), "the merged root splat is left out: {xs:?}");
        assert!(tree_leaves(&file, 78).unwrap_err().to_string().contains("replaced"));
        assert!(tree_leaves(&file[..100], 77).is_err());
        let mut v1 = file.clone();
        v1[8] = 1;
        assert!(tree_leaves(&v1, 77).unwrap_err().to_string().contains("share the capture again"), "no reader of version 1");
    }

    #[test]
    fn a_capture_is_ordered_outward_from_its_start_eye() {
        let mut g = Gaussians::default();
        for x in [5.0f32, -1.0, 3.0, 0.5] {
            g.push([x, 1.6, 0.0], [-2.0; 3], [1.0, 0.0, 0.0, 0.0], 0.0, [0.5; 3]);
        }
        let view = StartView { position: [0.0, 1.6, 0.0], target: [0.0, 1.6, -1.0], floor: None, eye_height: None };
        let c = Capture::new(g, None, view);
        assert_eq!(c.gaussians.means.iter().map(|m| m[0]).collect::<Vec<_>>(), vec![0.5, -1.0, 3.0, 5.0]);
        assert_eq!(c.within(1.0), 2);
        assert_eq!(c.within(100.0), 4);
        assert_eq!(c.farthest(), 5.0);
    }
}
