//! What the Visited worlds page reads, in the window process: the worlds visited (the tray service keeps them from
//! VRChat's logs), the screenshots taken in each and the sessions recorded there; each world's public page on the Map
//! explorer (cached for a few days); and pictures, decoded off the interface thread by a few loaders, with small
//! thumbnails kept on disk so a library of 4K screenshots opens at once the second time.

use egui::{ColorImage, TextureHandle, TextureOptions};
use orbiters_contracts::qoi;
use orbiters_core::account::{Api, Server};
use orbiters_core::bundle::{self, SessionSummary};
use orbiters_core::maps::WorldPage;
use orbiters_core::visited::{self, Shot};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// A world with what this computer has of it.
#[derive(Clone, Debug)]
pub struct VisitedWorld {
    pub id: String,
    pub name: String,
    /// Unix seconds of the latest visit, picture or session.
    pub last: u64,
    pub visits: usize,
    /// Newest first.
    pub shots: Vec<Shot>,
    /// Newest first.
    pub sessions: Vec<SessionSummary>,
}

impl VisitedWorld {
    pub fn reconstructed(&self) -> usize {
        self.sessions.iter().filter(|s| s.reconstruction.is_some()).count()
    }
}

/// Every world visited, pictured or recorded, the latest first.
pub fn load(sessions_dir: &Path) -> Vec<VisitedWorld> {
    let history = visited::History::load(&visited::history_file());
    let index_file = visited::shot_index_file();
    let mut index = visited::ShotIndex::load(&index_file);
    if index.refresh(&visited::screenshot_dirs()) {
        let _ = index.save(&index_file);
    }
    let mut worlds: BTreeMap<String, VisitedWorld> = BTreeMap::new();
    fn entry<'a>(worlds: &'a mut BTreeMap<String, VisitedWorld>, id: &str) -> &'a mut VisitedWorld {
        worlds.entry(id.to_string()).or_insert_with(|| VisitedWorld {
            id: id.to_string(),
            name: String::new(),
            last: 0,
            visits: 0,
            shots: Vec::new(),
            sessions: Vec::new(),
        })
    }
    for (id, v) in &history.worlds {
        let w = entry(&mut worlds, id);
        w.visits = v.visits.len();
        w.last = w.last.max(v.last().unwrap_or(0));
        if let Some(n) = v.name.as_ref().filter(|n| !n.is_empty()) {
            w.name = n.clone();
        }
    }
    for shot in index.shots() {
        let Some(id) = shot.info.world_id.clone() else { continue };
        let w = entry(&mut worlds, &id);
        w.last = w.last.max(shot.info.taken);
        if w.name.is_empty() {
            w.name = shot.info.world_name.clone().unwrap_or_default();
        }
        w.shots.push(shot);
    }
    for s in bundle::list_sessions(sessions_dir) {
        let Some(world) = s.manifest.world.as_ref() else { continue };
        let Some(id) = world.id.clone().filter(|id| id.starts_with("wrld_")) else { continue };
        let w = entry(&mut worlds, &id);
        w.last = w.last.max(s.modified);
        if w.name.is_empty() {
            w.name = world.name.clone().unwrap_or_default();
        }
        w.sessions.push(s);
    }
    let mut out: Vec<VisitedWorld> = worlds.into_values().collect();
    for w in &mut out {
        w.sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
        if w.name.is_empty() {
            w.name = "Unnamed world".into();
        }
    }
    out.sort_by(|a, b| b.last.cmp(&a.last).then_with(|| a.name.cmp(&b.name)));
    out
}

/// The SHA-256 of each screenshot (computed once, then kept in the index): Orbiters knows the pictures this member
/// shared by it.
pub fn hashes(paths: Vec<PathBuf>) -> HashMap<PathBuf, String> {
    let index_file = visited::shot_index_file();
    let mut index = visited::ShotIndex::load(&index_file);
    if index.hash(&paths) {
        let _ = index.save(&index_file);
    }
    let wanted: HashSet<&PathBuf> = paths.iter().collect();
    index.shots().into_iter().filter(|s| wanted.contains(&s.path)).filter_map(|s| Some((s.path, s.sha256?))).collect()
}

fn cache_dir() -> PathBuf {
    orbiters_core::paths::config_dir().join("cache")
}

/// Orbiters for public reads. `ORBITERS_PUBLIC_API` points them at another server (screenshots against a disposable
/// one); what that server says is then kept in memory only.
fn public_api() -> (Api, bool) {
    match std::env::var("ORBITERS_PUBLIC_API") {
        Ok(base) if !base.is_empty() => (Api::at(&base), false),
        _ => (Api::new(Server::current()), true),
    }
}

/// A world's page on the Map explorer as last fetched.
#[derive(Clone, Debug, PartialEq)]
pub enum Info {
    Listed(Box<WorldPage>),
    /// Orbiters does not list it: not public on VRChat.
    Unlisted,
    /// Orbiters could not be reached.
    Unknown,
}

#[derive(Serialize, Deserialize)]
struct CachedInfo {
    fetched: u64,
    page: Option<WorldPage>,
}

/// Listed worlds are fetched again after three days, unlisted ones after one.
const LISTED_FOR: u64 = 3 * 86_400;
const UNLISTED_FOR: u64 = 86_400;

/// Worlds' pages, fetched one at a time on a background thread as cards come into view.
pub struct Infos {
    known: Arc<Mutex<HashMap<String, Info>>>,
    asked: HashSet<String>,
    tx: mpsc::Sender<String>,
}

impl Infos {
    pub fn new(ctx: &egui::Context) -> Self {
        let known: Arc<Mutex<HashMap<String, Info>>> = Arc::default();
        let (tx, rx) = mpsc::channel::<String>();
        let (k, ctx) = (known.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("world-infos".into())
            .spawn(move || {
                let (api, keep) = public_api();
                let dir = cache_dir().join("worlds");
                while let Ok(id) = rx.recv() {
                    let file = dir.join(format!("{id}.json"));
                    let cached = keep.then(|| std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<CachedInfo>(&b).ok())).flatten();
                    let fresh =
                        cached.as_ref().is_some_and(|c| visited::now().saturating_sub(c.fetched) < if c.page.is_some() { LISTED_FOR } else { UNLISTED_FOR });
                    let info = if fresh {
                        cached.and_then(|c| c.page).map_or(Info::Unlisted, |p| Info::Listed(Box::new(p)))
                    } else {
                        match api.world(&id) {
                            Ok(page) => {
                                if keep {
                                    let _ = std::fs::create_dir_all(&dir);
                                    let _ = std::fs::write(
                                        &file,
                                        serde_json::to_vec(&CachedInfo { fetched: visited::now(), page: page.clone() }).unwrap_or_default(),
                                    );
                                }
                                page.map_or(Info::Unlisted, |p| Info::Listed(Box::new(p)))
                            }
                            // Offline: the older answer, when there is one.
                            Err(_) => cached.and_then(|c| c.page).map_or(Info::Unknown, |p| Info::Listed(Box::new(p))),
                        }
                    };
                    k.lock().unwrap().insert(id, info);
                    ctx.request_repaint();
                    if !fresh {
                        // Gentle on Orbiters (and on VRChat behind it).
                        std::thread::sleep(Duration::from_millis(150));
                    }
                }
            })
            .expect("world infos thread");
        Self { known, asked: HashSet::new(), tx }
    }

    /// The world's page once fetched; asks for it the first time.
    pub fn get(&mut self, id: &str) -> Option<Info> {
        if self.asked.insert(id.to_string()) {
            let _ = self.tx.send(id.to_string());
        }
        self.known.lock().unwrap().get(id).cloned()
    }

    /// Fetches it again (after sharing something there).
    pub fn refresh(&mut self, id: &str) {
        let _ = std::fs::remove_file(cache_dir().join("worlds").join(format!("{id}.json")));
        self.asked.remove(id);
    }
}

/// A picture to show.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Picture {
    /// A screenshot, at most `width` wide.
    Shot { path: PathBuf, size: u64, width: u32 },
    /// A session's thumbnail.
    Session(PathBuf),
    /// A world's picture on VRChat (as Orbiters lists it): its thumbnail, or the full picture when `large`.
    Remote { world_id: String, url: String, large: bool },
}

enum Slot {
    Loading,
    Ready(TextureHandle),
    Missing,
}

type Loaded = (Picture, Option<ColorImage>);

/// Pictures decoded by three loader threads (newest requests first), uploaded as textures on the interface thread.
pub struct Pictures {
    slots: HashMap<Picture, Slot>,
    queue: Arc<(Mutex<Vec<Picture>>, std::sync::Condvar)>,
    done: Arc<Mutex<Vec<Loaded>>>,
}

impl Pictures {
    pub fn new(ctx: &egui::Context) -> Self {
        let queue: Arc<(Mutex<Vec<Picture>>, std::sync::Condvar)> = Arc::default();
        let done: Arc<Mutex<Vec<Loaded>>> = Arc::default();
        for i in 0..3 {
            let (queue, done, ctx) = (queue.clone(), done.clone(), ctx.clone());
            std::thread::Builder::new()
                .name(format!("pictures-{i}"))
                .spawn(move || {
                    let (api, keep) = public_api();
                    loop {
                        let next = {
                            let (lock, ready) = &*queue;
                            let mut q = lock.lock().unwrap();
                            while q.is_empty() {
                                q = ready.wait(q).unwrap();
                            }
                            q.pop()
                        };
                        let Some(picture) = next else { continue };
                        let image = decode(&api, keep, &picture);
                        done.lock().unwrap().push((picture, image));
                        ctx.request_repaint();
                    }
                })
                .expect("picture loader");
        }
        Self { slots: HashMap::new(), queue, done }
    }

    /// The picture as a texture once decoded; asks for it the first time (and again to bring it forward while it
    /// waits, so what is on screen loads first).
    pub fn get(&mut self, ctx: &egui::Context, picture: &Picture) -> Option<TextureHandle> {
        for (p, image) in std::mem::take(&mut *self.done.lock().unwrap()) {
            let slot = match image {
                Some(image) => Slot::Ready(ctx.load_texture(format!("picture-{:x}", key(&p)), image, TextureOptions::LINEAR)),
                None => Slot::Missing,
            };
            self.slots.insert(p, slot);
        }
        match self.slots.get(picture) {
            Some(Slot::Ready(t)) => Some(t.clone()),
            Some(Slot::Missing) => None,
            Some(Slot::Loading) => {
                let (lock, _) = &*self.queue;
                let mut q = lock.lock().unwrap();
                if let Some(i) = q.iter().position(|p| p == picture) {
                    let p = q.remove(i);
                    q.push(p);
                }
                None
            }
            None => {
                self.slots.insert(picture.clone(), Slot::Loading);
                let (lock, ready) = &*self.queue;
                lock.lock().unwrap().push(picture.clone());
                ready.notify_one();
                None
            }
        }
    }

    /// Lets go of the large pictures (the viewer closed).
    pub fn forget_large(&mut self) {
        self.slots.retain(|p, _| !matches!(p, Picture::Shot { width, .. } if *width > THUMB_WIDTH));
    }
}

/// Thumbnails are this wide (kept on disk).
pub const THUMB_WIDTH: u32 = 480;

fn key(p: &Picture) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.hash(&mut h);
    h.finish()
}

fn decode(api: &Api, keep: bool, picture: &Picture) -> Option<ColorImage> {
    match picture {
        Picture::Session(dir) => {
            let (w, h, rgba) = bundle::read_thumbnail(dir)?;
            Some(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba))
        }
        Picture::Shot { path, width, .. } => {
            let cached = (*width <= THUMB_WIDTH).then(|| cache_dir().join("thumbs").join(format!("{:016x}.qoi", key(picture))));
            if let Some((h, rgba)) = cached.as_ref().and_then(|c| std::fs::read(c).ok()).and_then(|d| qoi::decode_rgba(&d).ok()) {
                return Some(ColorImage::from_rgba_unmultiplied([h.width as usize, h.height as usize], &rgba));
            }
            let (w, h, rgba) = decode_png(&std::fs::read(path).ok()?)?;
            let (tw, th, rgb) = bundle::downscale(&rgba, w, h, *width);
            if let Some(c) = cached {
                let _ = std::fs::create_dir_all(c.parent()?);
                let _ = std::fs::write(&c, qoi::encode(&rgb, tw, th, 3));
            }
            Some(ColorImage::from_rgb([tw as usize, th as usize], &rgb))
        }
        Picture::Remote { world_id, url, large } => {
            let file = cache_dir().join("worlds").join(format!("{world_id}{}.png", if *large { "-large" } else { "" }));
            let bytes = match std::fs::read(&file) {
                Ok(b) if keep => b,
                _ => {
                    let b = api.image(url).ok()?;
                    if keep {
                        let _ = std::fs::create_dir_all(file.parent()?);
                        let _ = std::fs::write(&file, &b);
                    }
                    b
                }
            };
            let (w, h, rgba) = decode_png(&bytes)?;
            Some(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba))
        }
    }
}

/// A PNG as RGBA.
pub fn decode_png(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let px = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.as_chunks::<3>().0.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.as_chunks::<2>().0.iter().flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|g| [*g, *g, *g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((info.width, info.height, rgba))
}
