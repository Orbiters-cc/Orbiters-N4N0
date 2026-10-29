//! What the library pages read, in the window process: the sessions (with their sizes kept between listings, for the
//! Sessions and Visited worlds pages alike); the worlds visited (the tray service keeps them from VRChat's logs), the
//! screenshots taken in each and the sessions recorded there; each world's public page on the Map explorer (cached
//! for a few days); and pictures, decoded off the interface thread by a few loaders, with small thumbnails kept on
//! disk so a library of 4K screenshots opens at once the second time.

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

/// Sessions under `root`, newest first ([`bundle::list_sessions`]), each session's size kept from the last listing
/// while it has not changed (`bundle::session_size`, which the tray service's storage count shares): entering a library
/// page reads the manifests again and walks only the sessions written since, not every file of every session.
pub fn sessions(root: &Path) -> Vec<SessionSummary> {
    bundle::list_sessions(root)
}

/// The screenshot index is read, changed and saved by two jobs of this window (the worlds, a world's hashes): one at
/// a time, so neither saves over what the other just added.
fn shot_index_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

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
    let index = {
        let _one = shot_index_lock();
        let mut index = visited::ShotIndex::load(&index_file);
        if index.refresh(&visited::screenshot_dirs()) {
            let _ = index.save(&index_file);
        }
        index
    };
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
    for s in sessions(sessions_dir) {
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
        w.sessions.sort_by_key(|s| std::cmp::Reverse(s.modified));
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
    let _one = shot_index_lock();
    let mut index = visited::ShotIndex::load(&index_file);
    if index.hash(&paths) {
        let _ = index.save(&index_file);
    }
    let wanted: HashSet<&PathBuf> = paths.iter().collect();
    index.shots().into_iter().filter(|s| wanted.contains(&s.path)).filter_map(|s| Some((s.path, s.sha256?))).collect()
}

/// World pages, their pictures and thumbnails: made again when missing, so they live on this computer only
/// ([`orbiters_core::paths::cache_dir`]), not in the roaming config folder where they were before. What was kept there
/// is moved over once (a rename on the same disk; when it cannot be, it is made again).
fn cache_dir() -> PathBuf {
    static MOVED: std::sync::Once = std::sync::Once::new();
    let dir = orbiters_core::paths::cache_dir();
    if !cfg!(test) {
        MOVED.call_once(|| {
            let old = orbiters_core::paths::config_dir().join("cache");
            for part in ["worlds", "thumbs", "screenshots.json"] {
                let (from, to) = (old.join(part), dir.join(part));
                if from.exists() && !to.exists() && std::fs::create_dir_all(&dir).is_ok() {
                    let _ = std::fs::rename(&from, &to);
                }
            }
        });
    }
    dir
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
                                    let _ = orbiters_core::write_atomic(
                                        &file,
                                        &serde_json::to_vec(&CachedInfo { fetched: visited::now(), page: page.clone() }).unwrap_or_default(),
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
    /// A session-scoped profile image. Never persisted in the public world image cache.
    Profile { scope: crate::controller::vrchat_data::Scope, url: String },
    /// A protected sharing-contract picture, fetched with the current environment's account on a loader thread.
    Capture { development: bool, id: u64, large: bool },
    /// A screenshot, at most `width` wide.
    Shot { path: PathBuf, size: u64, width: u32 },
    /// A session's thumbnail.
    Session(PathBuf),
    /// A world's picture on VRChat (as Orbiters lists it): its thumbnail, or the full picture when `large`.
    Remote { world_id: String, url: String, large: bool },
    /// An event's cover on Orbiters, read with the account's credential when signed in (members-only covers need it).
    EventCover { development: bool, id: String },
    /// Another picture a few pixels wide, to draw large and soft as its light (behind an event, its window).
    Glow(Box<Picture>),
    /// A picture of a community gallery on Orbiters: its preview, or the full picture when `large` (kept in memory).
    Gallery { id: u64, url: String, large: bool },
}

enum Slot {
    Loading,
    Ready(TextureHandle),
    Missing,
}

/// A picture as the pages keep it, and when they last asked for it (egui's clock).
struct Kept {
    slot: Slot,
    used: f64,
}

type Loaded = (Picture, Option<ColorImage>);

/// Bytes of picture textures kept (about ninety thumbnails, three screens of a world's pictures): those asked for
/// most recently. The others are decoded again (from the small copies kept on disk) when they come back into view.
const KEEP_BYTES: usize = 48 << 20;

/// A waiting picture nobody asked for since this long (scrolled past) is not decoded after all.
const STALE_REQUEST: f64 = 2.0;

/// Pictures decoded by three loader threads (newest requests first), uploaded as textures on the interface thread.
pub struct Pictures {
    slots: HashMap<Picture, Kept>,
    queue: Arc<(Mutex<Vec<Picture>>, std::sync::Condvar)>,
    done: Arc<Mutex<Vec<Loaded>>>,
    /// When the pictures were last trimmed to [`KEEP_BYTES`] (egui's clock).
    trimmed: f64,
    stopped: Arc<std::sync::atomic::AtomicBool>,
}

impl Pictures {
    pub fn new(ctx: &egui::Context) -> Self {
        Self::with_backend(ctx, None)
    }
    pub fn with_backend(ctx: &egui::Context, backend: Option<crate::backend::Ctl>) -> Self {
        let queue: Arc<(Mutex<Vec<Picture>>, std::sync::Condvar)> = Arc::default();
        let done: Arc<Mutex<Vec<Loaded>>> = Arc::default();
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        for i in 0..3 {
            let (queue, done, ctx) = (queue.clone(), done.clone(), ctx.clone());
            let backend = backend.clone();
            let stopped = stopped.clone();
            std::thread::Builder::new()
                .name(format!("pictures-{i}"))
                .spawn(move || {
                    let (api, keep) = public_api();
                    loop {
                        let next = {
                            let (lock, ready) = &*queue;
                            let mut q = lock.lock().unwrap();
                            while q.is_empty() {
                                if stopped.load(std::sync::atomic::Ordering::Acquire) {
                                    return;
                                }
                                q = ready.wait(q).unwrap();
                            }
                            if stopped.load(std::sync::atomic::Ordering::Acquire) {
                                return;
                            }
                            q.pop()
                        };
                        let Some(picture) = next else { continue };
                        let image = if let Picture::Profile { scope, url } = &picture {
                            use crate::controller::vrchat_data::{Request, Response};
                            backend
                                .as_ref()
                                .and_then(|b| b.vrchat_data(scope.clone(), Request::Image { url: url.clone() }).ok())
                                .and_then(|r| if let Response::Image(bytes) = r { decode_image(&bytes) } else { None })
                                .map(|(w, h, rgba)| ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba))
                        } else {
                            decode(&api, keep, &picture)
                        };
                        done.lock().unwrap().push((picture, image));
                        ctx.request_repaint();
                    }
                })
                .expect("picture loader");
        }
        Self { slots: HashMap::new(), queue, done, trimmed: f64::NEG_INFINITY, stopped }
    }

    /// The picture as a texture once decoded; asks for it the first time (and again to bring it forward while it
    /// waits, so what is on screen loads first).
    pub fn get(&mut self, ctx: &egui::Context, picture: &Picture) -> Option<TextureHandle> {
        let now = ctx.input(|i| i.time);
        for (p, image) in std::mem::take(&mut *self.done.lock().unwrap()) {
            // One no longer wanted (scrolled past while it was decoded) is dropped.
            let Some(kept) = self.slots.get_mut(&p) else { continue };
            kept.slot = match image {
                Some(image) => Slot::Ready(ctx.load_texture(format!("picture-{:x}", key(&p)), image, TextureOptions::LINEAR)),
                None => Slot::Missing,
            };
        }
        if now - self.trimmed > 0.5 {
            self.trimmed = now;
            self.trim(now);
        }
        match self.slots.get_mut(picture) {
            Some(kept) => {
                kept.used = now;
                match &kept.slot {
                    Slot::Ready(t) => Some(t.clone()),
                    Slot::Missing => None,
                    Slot::Loading => {
                        // (Not in the queue: a loader has it.)
                        let (lock, _) = &*self.queue;
                        let mut q = lock.lock().unwrap();
                        if let Some(i) = q.iter().position(|p| p == picture) {
                            let p = q.remove(i);
                            q.push(p);
                        }
                        None
                    }
                }
            }
            None => {
                self.slots.insert(picture.clone(), Kept { slot: Slot::Loading, used: now });
                let (lock, ready) = &*self.queue;
                lock.lock().unwrap().push(picture.clone());
                ready.notify_one();
                None
            }
        }
    }

    /// Whether pictures asked for are still being decoded.
    pub fn loading(&self) -> bool {
        self.slots.values().any(|k| matches!(k.slot, Slot::Loading))
    }

    /// Requests nobody repeated for [`STALE_REQUEST`] leave the queue, and the textures asked for longest ago go
    /// beyond [`KEEP_BYTES`] (a big library browsed end to end would otherwise keep hundreds).
    fn trim(&mut self, now: f64) {
        let stale = |k: &Kept| matches!(k.slot, Slot::Loading) && now - k.used > STALE_REQUEST;
        {
            let (lock, _) = &*self.queue;
            let slots = &self.slots;
            lock.lock().unwrap().retain(|p| slots.get(p).is_some_and(|k| !stale(k)));
        }
        self.slots.retain(|_, k| !stale(k));
        let bytes = |k: &Kept| match &k.slot {
            Slot::Ready(t) => t.byte_size(),
            _ => 0,
        };
        let mut total: usize = self.slots.values().map(bytes).sum();
        if total <= KEEP_BYTES {
            return;
        }
        // What was on screen in the last second stays, whatever it weighs.
        let mut old: Vec<(f64, Picture, usize)> =
            self.slots.iter().filter(|(_, k)| matches!(k.slot, Slot::Ready(_)) && k.used < now - 1.0).map(|(p, k)| (k.used, p.clone(), bytes(k))).collect();
        old.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, p, size) in old {
            if total <= KEEP_BYTES {
                break;
            }
            self.slots.remove(&p);
            total -= size;
        }
    }

    /// Lets go of the large pictures (the viewer closed).
    pub fn forget_large(&mut self) {
        self.slots.retain(|p, _| !matches!(p, Picture::Shot { width, .. } if *width > THUMB_WIDTH));
    }
}

impl Drop for Pictures {
    fn drop(&mut self) {
        let (lock, ready) = &*self.queue;
        let mut queue = lock.lock().unwrap();
        queue.clear();
        self.stopped.store(true, std::sync::atomic::Ordering::Release);
        ready.notify_all();
    }
}

/// A picture on this computer (a screenshot, from its kept thumbnail, or a session's thumbnail) decoded on the calling
/// thread, for the drag image of Photos & captures' cards; None for pictures from the network.
pub fn decode_local(picture: &Picture) -> Option<ColorImage> {
    match picture {
        Picture::Shot { .. } | Picture::Session(_) => decode(&Api::new(Server::Production), true, picture),
        _ => None,
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
        Picture::Profile { .. } => None, // Requires the service backend, handled by the loader.
        Picture::Capture { development, id, large } => {
            let server = if *development { Server::Development } else { Server::Production };
            let account = orbiters_core::account::read(server)?;
            let api = Api::new(server);
            let r =
                api.agent().get(&api.url(&format!("captures/picture/{id}/file"))).header("Authorization", &format!("Bearer {}", account.token)).call().ok()?;
            if !r.status().is_success() {
                return None;
            }
            let bytes = r.into_body().with_config().limit(32 << 20).read_to_vec().ok()?;
            let image = image::load_from_memory(&bytes).ok()?.thumbnail(if *large { 2048 } else { 560 }, if *large { 4096 } else { 1120 }).to_rgba8();
            Some(ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw()))
        }
        Picture::EventCover { development, id } => {
            let server = if *development { Server::Development } else { Server::Production };
            let api = Api::new(server);
            let mut request = api.agent().get(&api.event_banner_url(id).ok()?);
            if let Some(account) = orbiters_core::account::read(server) {
                request = request.header("Authorization", &format!("Bearer {}", account.token));
            }
            let r = request.call().ok()?;
            if !r.status().is_success() {
                return None;
            }
            let bytes = r.into_body().with_config().limit(16 << 20).read_to_vec().ok()?;
            let image = image::load_from_memory(&bytes).ok()?.thumbnail(1600, 900).to_rgba8();
            Some(ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw()))
        }
        Picture::Glow(inner) => {
            let full = decode(api, keep, inner)?;
            let [w, h] = full.size;
            let rgba: Vec<u8> = full.pixels.iter().flat_map(|c| c.to_array()).collect();
            let full = image::RgbaImage::from_raw(w as u32, h as u32, rgba)?;
            let (tw, th) = if w >= h { (16, (16 * h / w.max(1)).max(1) as u32) } else { ((16 * w / h.max(1)).max(1) as u32, 16) };
            let small = image::imageops::thumbnail(&full, tw, th);
            Some(ColorImage::from_rgba_premultiplied([small.width() as usize, small.height() as usize], small.as_raw()))
        }
        Picture::Gallery { url, large, .. } => {
            let bytes = api.image(url).ok()?;
            let image = image::load_from_memory(&bytes).ok()?.thumbnail(if *large { 2048 } else { 560 }, if *large { 4096 } else { 1120 }).to_rgba8();
            Some(ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw()))
        }
        Picture::Session(dir) => {
            let (w, h, rgba) = bundle::read_thumbnail(dir)?;
            Some(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba))
        }
        Picture::Shot { path, width, .. } => {
            let cached = (*width <= THUMB_WIDTH).then(|| cache_dir().join("thumbs").join(format!("{:016x}.qoi", key(picture))));
            // A thumbnail is at most THUMB_WIDTH wide (and, a picture standing up, four times as tall): a file saying it
            // is bigger is not one, and is made again rather than given its memory.
            let most = u64::from(THUMB_WIDTH) * u64::from(THUMB_WIDTH) * 4;
            if let Some((h, rgba)) = cached.as_ref().and_then(|c| std::fs::read(c).ok()).and_then(|d| qoi::decode_rgba_max(&d, most).ok()) {
                return Some(ColorImage::from_rgba_unmultiplied([h.width as usize, h.height as usize], &rgba));
            }
            let (w, h, rgba) = decode_png(&std::fs::read(path).ok()?)?;
            let (tw, th, rgb) = bundle::downscale(&rgba, w, h, *width);
            if let Some(c) = cached {
                let _ = std::fs::create_dir_all(c.parent()?);
                let _ = orbiters_core::write_atomic(&c, &qoi::encode(&rgb, tw, th, 3));
            }
            Some(ColorImage::from_rgb([tw as usize, th as usize], &rgb))
        }
        Picture::Remote { world_id, url, large } => {
            let file = cache_dir().join("worlds").join(format!("{world_id}{}.png", if *large { "-large" } else { "" }));
            // One kept that cannot be read (cut short) is fetched again.
            let kept = keep.then(|| std::fs::read(&file).ok()).flatten().and_then(|b| decode_image(&b));
            let (w, h, rgba) = match kept {
                Some(picture) => picture,
                None => {
                    let b = api.image(url).ok()?;
                    if keep {
                        let _ = std::fs::create_dir_all(file.parent()?);
                        let _ = orbiters_core::write_atomic(&file, &b);
                    }
                    decode_image(&b)?
                }
            };
            Some(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba))
        }
    }
}

/// Remote thumbnails may be JPEG or WebP as well as PNG.
fn decode_image(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    Some((image.width(), image.height(), image.into_raw()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_sizes_are_kept_until_the_session_changes() {
        let root = std::env::temp_dir().join(format!("orbiters-window-library-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let m = orbiters_contracts::manifest::SessionManifest::new("id-1".into(), orbiters_core::now_utc_iso(), "0.1.0");
        let (dir, _) = bundle::BundleWriter::create(&root, "World: test", m).unwrap().finish().unwrap();
        let first = sessions(&root);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].size_bytes, bundle::folder_size(&dir));
        assert_eq!(sessions(&root)[0].size_bytes, first[0].size_bytes);
        // A file added to the session: its folder changed, its size is walked again (past the file system clock's
        // tick, about 16 ms on Windows, so that the change has a time of its own).
        std::thread::sleep(Duration::from_millis(40));
        std::fs::write(dir.join("extra.bin"), vec![0u8; 1000]).unwrap();
        assert_eq!(sessions(&root)[0].size_bytes, first[0].size_bytes + 1000);
        // Gone from the folder, gone from the list.
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(sessions(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
