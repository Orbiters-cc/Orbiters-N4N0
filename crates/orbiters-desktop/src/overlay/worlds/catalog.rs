//! The 3D captures of worlds the overlay can show: the listing the VRChat world viewer reads (Orbiters'
//! `GET /maps/vrchat/previews`, contracts `Docs/splat-tree.md`), searched like its menu does (world, author or sharer
//! containing what is typed), and a capture's files: the slot's PC tree and sky map at the public storage addresses the
//! world loads (production), or the API's web `.splat` and sky map of the preview elsewhere (a development server
//! publishes its slots to storage the app has no address for).

use super::stereo::{self, StartView};
use orbiters_core::account::{Api, Server};
use orbiters_reconstruct::gaussians::Gaussians;
use orbiters_reconstruct::sky::SkyMap;
use serde::Deserialize;
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

/// A 3D capture of a world, as the listing gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
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
    /// Lower-case text a search looks in: the world, its author and the sharer.
    search: String,
}

impl Entry {
    /// Whether `query` (any case) is in its world's name, its author's or its sharer's.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        q.is_empty() || self.search.contains(&q)
    }
}

#[derive(Deserialize)]
struct Listing {
    previews: Vec<RawEntry>,
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

/// The listing's entries that have a start view (newest first, as listed).
pub fn parse(json: &str) -> anyhow::Result<Vec<Entry>> {
    let listing: Listing = serde_json::from_str(json)?;
    Ok(listing
        .previews
        .into_iter()
        .filter_map(|r| {
            let (position, target) = (r.view.position?, r.view.target?);
            let search = format!("{}\n{}\n{}", r.name, r.author.as_deref().unwrap_or(""), r.uploader.as_deref().unwrap_or("")).to_lowercase();
            Some(Entry {
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
                search,
            })
        })
        .collect())
}

/// Where the listing and the files come from: the app's server (or `ORBITERS_PUBLIC_API`, as the window's public reads),
/// and production's public storage when that is the server.
#[derive(Clone)]
pub struct Source {
    api: Api,
    files: Option<String>,
}

impl Source {
    pub fn current() -> Self {
        match std::env::var("ORBITERS_PUBLIC_API") {
            Ok(base) if !base.is_empty() => Self { api: Api::at(&base), files: None },
            _ => {
                let server = Server::current();
                Self { api: Api::new(server), files: (server == Server::Production).then(|| PUBLIC_FILES.to_string()) }
            }
        }
    }

    pub fn listing(&self) -> anyhow::Result<Vec<Entry>> {
        let response = self.api.agent().get(&self.api.url("maps/vrchat/previews")).call()?;
        let status = response.status().as_u16();
        let text = response.into_body().with_config().limit(16 << 20).read_to_string()?;
        anyhow::ensure!((200..300).contains(&status), "Orbiters answered {status}");
        parse(&text)
    }

    /// Downloads `entry`'s splats and sky map, `progress` counting bytes (and their total once known).
    pub fn download(&self, entry: &Entry, progress: &Progress) -> anyhow::Result<Capture> {
        let (g, sky) = match (&self.files, entry.slot) {
            (Some(files), Some(slot)) => {
                let tree = self.fetch(&format!("{files}public/vrchat/slots/{slot}/pc"), progress)?;
                let g = tree_leaves(&tree, entry.key)?;
                drop(tree);
                let sky = entry.sky.then(|| self.fetch(&format!("{files}public/vrchat/slots/{slot}/sky.jpg"), &Progress::default()).ok()).flatten();
                (g, sky)
            }
            _ => {
                let bytes = self.fetch(&self.api.url(&format!("maps/previews/{}/splat", entry.id)), progress)?;
                let g = orbiters_reconstruct::web::read_splat(&bytes)?;
                drop(bytes);
                let sky = entry.sky.then(|| self.fetch(&self.api.url(&format!("maps/previews/{}/sky.jpg", entry.id)), &Progress::default()).ok()).flatten();
                (g, sky)
            }
        };
        progress.stage.store(STAGE_PREPARING, Ordering::Relaxed);
        let sky = sky.and_then(|bytes| sky_map(&bytes).map_err(|e| eprintln!("3D worlds: sky map of {}: {e:#}", entry.name)).ok());
        Ok(Capture::new(g, sky, entry.view))
    }

    fn fetch(&self, url: &str, progress: &Progress) -> anyhow::Result<Vec<u8>> {
        let response = self.api.agent().get(url).call()?;
        let status = response.status().as_u16();
        anyhow::ensure!((200..300).contains(&status), "{url} answered {status}");
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
        Ok(out)
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

/// The finest splats of a PC tree (contracts `Docs/splat-tree.md`): its leaf boxes', in the renderer's coordinates. A
/// file whose key is not `key` was replaced at its slot since the listing.
pub fn tree_leaves(bytes: &[u8], key: u32) -> anyhow::Result<Gaussians> {
    const RECORD: usize = 32;
    anyhow::ensure!(bytes.len() >= RECORD && &bytes[..8] == b"N4N0TREE", "not a splat tree");
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    anyhow::ensure!(word(8) == 1, "a splat tree of a newer version");
    let (boxes, splats) = (word(12) as usize, word(16) as usize);
    anyhow::ensure!(boxes >= 1 && splats >= 1 && bytes.len() >= (1 + boxes + splats) * RECORD, "a splat tree cut short");
    anyhow::ensure!(word(20) == key, "this capture was replaced on Orbiters: refresh the list");
    let first_splat = (1 + boxes) * RECORD;
    let mut leaves = Vec::new();
    for b in 0..boxes {
        let at = (1 + b) * RECORD;
        let error = f32::from_le_bytes(bytes[at + 16..at + 20].try_into().unwrap());
        let (first, count) = (word(at + 24) as usize, word(at + 28) as usize);
        anyhow::ensure!(first + count <= splats, "a box past the tree's splats");
        if error == 0.0 {
            leaves.extend_from_slice(&bytes[first_splat + first * RECORD..first_splat + (first + count) * RECORD]);
        }
    }
    orbiters_reconstruct::web::read_splat(&leaves)
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

    /// The splats within `radius` of the start eye (the first ones).
    pub fn within(&self, radius: f32) -> usize {
        self.distances.partition_point(|d| *d <= radius)
    }

    pub fn farthest(&self) -> f32 {
        self.distances.last().copied().unwrap_or(1.0)
    }
}

/// The listing, read on a thread of its own when asked (at most once a minute).
#[derive(Default)]
pub struct Catalog {
    state: Arc<Mutex<Listed>>,
}

#[derive(Default)]
struct Listed {
    entries: Vec<Entry>,
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
            let mut s = state.lock().unwrap_or_else(|p| p.into_inner());
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
    ]}"#;

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

    fn splat_bytes(x: f32) -> [u8; 32] {
        let mut b = [0u8; 32];
        for (i, v) in [x, 0.0, 0.0, 0.1, 0.1, 0.1].iter().enumerate() {
            b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        b[24..28].copy_from_slice(&[200, 100, 50, 255]);
        b[28..32].copy_from_slice(&[255, 128, 128, 128]);
        b
    }

    #[test]
    fn a_tree_gives_its_leaf_boxes_splats() {
        // A root with one merged splat and two leaves of two and one.
        let mut file = Vec::new();
        let word = |v: u32| v.to_le_bytes();
        let mut header = [0u8; 32];
        header[..8].copy_from_slice(b"N4N0TREE");
        header[8..12].copy_from_slice(&word(1));
        header[12..16].copy_from_slice(&word(3));
        header[16..20].copy_from_slice(&word(4));
        header[20..24].copy_from_slice(&word(77));
        file.extend_from_slice(&header);
        for (error, parent, first, count) in [(0.5f32, u32::MAX, 0u32, 1u32), (0.0, 0, 1, 2), (0.0, 0, 3, 1)] {
            let mut b = [0u8; 32];
            b[12..16].copy_from_slice(&5f32.to_le_bytes());
            b[16..20].copy_from_slice(&error.to_le_bytes());
            b[20..24].copy_from_slice(&word(parent));
            b[24..28].copy_from_slice(&word(first));
            b[28..32].copy_from_slice(&word(count));
            file.extend_from_slice(&b);
        }
        for x in [9.0f32, 1.0, 2.0, 3.0] {
            file.extend_from_slice(&splat_bytes(x));
        }
        let g = tree_leaves(&file, 77).unwrap();
        assert_eq!(g.means.iter().map(|m| m[0]).collect::<Vec<_>>(), vec![1.0, 2.0, 3.0], "the merged root splat is left out");
        assert!(tree_leaves(&file, 78).unwrap_err().to_string().contains("replaced"));
        assert!(tree_leaves(&file[..100], 77).is_err());
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
