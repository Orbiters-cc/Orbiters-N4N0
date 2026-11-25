//! What the library pages read, in the window process: the sessions (with their sizes kept between listings, for the
//! Sessions and Visited worlds pages alike); the worlds visited (the tray service keeps them from VRChat's logs), the
//! screenshots taken in each and the sessions recorded there; each world's public page on the Map explorer (cached
//! for a few days); and pictures, decoded off the interface thread by a few loaders, with small thumbnails kept on
//! disk so a library of 4K screenshots opens at once the second time.

pub mod at_once;
mod picture_cache;
pub mod profile_pictures;

use egui::{ColorImage, TextureHandle, TextureOptions};
use orbiters_contracts::qoi;
use orbiters_core::account::{Api, Server};
use orbiters_core::bundle::{self, SessionSummary};
use orbiters_core::maps::WorldPage;
use orbiters_core::visited::{self, Shot};
use orbiters_i18n::tr;
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
    /// Newest first. A multi-layer picture is one shot (its picture, else its bottom layer), its layers in `layers`.
    pub shots: Vec<Shot>,
    /// The layers of the shots that have them ([`group_layers`]), by the shot's path.
    pub layers: HashMap<PathBuf, Vec<Layer>>,
    /// Newest first.
    pub sessions: Vec<SessionSummary>,
}

/// One file of a VRChat multi-layer picture. VRChat's camera in its Multi Layer mode saves, beside the picture itself,
/// one PNG per mask turned on, named as the picture with the mask after it: `VRChat_2026-10-29_21-04-11.532_3840x2160.png`
/// with `…_3840x2160_Environment.png` (the world), `…_Player.png` (the people, transparent around them) and `…_UI.png`
/// (transparent). Photos & captures shows them as one picture ([`crate::controller::shares::Entry::layers`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub path: PathBuf,
    /// VRChat's name for it, as its file has it ("Environment", "Player", "UI"): shown as is.
    pub name: String,
    /// Its size in pixels (the picture's).
    pub width: u32,
    pub height: u32,
    /// What Orbiters has of it when it was shared on its own (this account's upload record; the listing fills it).
    #[serde(default)]
    pub shared: Option<orbiters_core::shares::Item>,
}

impl Layer {
    /// Saved with transparency (every layer but the world): shown over a checkerboard.
    pub fn transparent(&self) -> bool {
        !self.name.eq_ignore_ascii_case("Environment")
    }

    /// How it is shown: with its transparency, at most `width` wide.
    pub fn picture(&self, width: u32) -> Picture {
        Picture::Layer { path: self.path.clone(), width }
    }

    /// Back to front: the world, the people, other masks, the interface on top.
    fn depth(&self) -> u8 {
        match self.name.to_ascii_lowercase().as_str() {
            "environment" => 0,
            "player" => 1,
            "ui" => 3,
            _ => 2,
        }
    }
}

/// A VRChat picture's file name as the name its files share and, for one of a multi-layer picture's layers, the layer:
/// `VRChat_2026-10-29_21-04-11.532_3840x2160.png` is the picture, `…_3840x2160_Player.png` its Player layer, and
/// `…_3840x2160_wrld_….png` (VRCX's renamed copy, with the world in its name) the picture too. None for other names
/// (VRChat's old ones put the size first and had no layers).
pub fn shot_name(file: &str) -> Option<(&str, Option<&str>)> {
    let stem = file.strip_suffix(".png").or_else(|| file.strip_suffix(".PNG"))?;
    let rest = stem.strip_prefix("VRChat_")?;
    // The time (`2026-10-29_21-04-11.532`), then the size (`3840x2160`).
    let time = rest.get(..23)?.as_bytes();
    let separators = [(4, b'-'), (7, b'-'), (10, b'_'), (13, b'-'), (16, b'-'), (19, b'.')];
    let fits = time.iter().enumerate().all(|(i, c)| separators.iter().find(|(at, _)| *at == i).map_or(c.is_ascii_digit(), |(_, s)| c == s));
    if !fits || rest.as_bytes().get(23) != Some(&b'_') {
        return None;
    }
    let after = &rest[24..];
    let size = &after[..after.find('_').unwrap_or(after.len())];
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    if !size.split_once('x').is_some_and(|(w, h)| digits(w) && digits(h)) {
        return None;
    }
    let base = &stem[.."VRChat_".len() + 24 + size.len()];
    match after[size.len()..].strip_prefix('_').map(|s| s.split("_wrld_").next().unwrap_or(s)) {
        None => Some((base, None)),
        Some(s) if s.starts_with("wrld_") => Some((base, None)),
        Some(s) if !s.is_empty() && s.len() <= 32 && s.bytes().all(|c| c.is_ascii_alphabetic()) => Some((base, Some(s))),
        // Something else after the size (`_2`, a copy): a picture of its own.
        Some(_) => None,
    }
}

/// The files of each multi-layer picture as one shot and its layers (back to front), the other pictures as they are,
/// in the order given. A picture's files share its timestamp and size across the configured folders; the shot is the picture itself,
/// else (deleted) its bottom layer, and it takes the world any of them has (VRChat may write it in one only). A layer
/// alone is a shot of its own whose one layer is itself, so that it shows as a layer still.
pub fn group_layers(shots: Vec<Shot>) -> Vec<(Shot, Vec<Layer>)> {
    // Each picture's files: the picture, and its layers by name.
    let mut sets: Vec<(Option<Shot>, Vec<(Layer, Shot)>)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for shot in shots {
        let file = shot.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some((base, layer)) = shot_name(&file) else {
            sets.push((Some(shot), Vec::new()));
            continue;
        };
        let key = base.to_ascii_lowercase();
        let layer = layer.map(str::to_owned);
        let i = *at.entry(key).or_insert_with(|| {
            sets.push((None, Vec::new()));
            sets.len() - 1
        });
        match layer {
            Some(name) => {
                let l = Layer { path: shot.path.clone(), name, width: shot.info.width, height: shot.info.height, shared: None };
                sets[i].1.push((l, shot));
            }
            None if sets[i].0.is_none() => sets[i].0 = Some(shot),
            // A second file named as the picture (VRCX's copy beside VRChat's): a picture of its own.
            None => sets.push((Some(shot), Vec::new())),
        }
    }
    sets.into_iter()
        .filter_map(|(picture, mut layers)| {
            layers.sort_by(|a, b| a.0.depth().cmp(&b.0.depth()).then_with(|| a.0.name.cmp(&b.0.name)));
            let world = picture.iter().chain(layers.iter().map(|(_, s)| s)).find(|s| s.info.world_id.is_some()).map(|s| s.info.clone());
            let mut face = picture.or_else(|| layers.first().map(|(_, s)| s.clone()))?;
            if let Some(w) = world.filter(|_| face.info.world_id.is_none()) {
                face.info.world_id = w.world_id;
                face.info.world_name = w.world_name;
                face.info.instance = w.instance;
            }
            Some((face, layers.into_iter().map(|(l, _)| l).collect()))
        })
        .collect()
}

impl VisitedWorld {
    pub fn reconstructed(&self) -> usize {
        self.sessions.iter().filter(|s| s.reconstruction.is_some()).count()
    }
}

/// Every world visited, pictured or recorded, the latest first.
pub fn load(sessions_dir: &Path) -> Vec<VisitedWorld> {
    let file = visited::shot_index_file();
    let index = {
        let _one = shot_index_lock();
        let mut index = visited::ShotIndex::load(&file);
        if index.refresh(&visited::screenshot_dirs()) { let _ = index.save(&file); }
        index
    };
    picture_cache::set_index(&index);
    from_index(sessions_dir, index, false)
}

/// Photos & captures can show the cached listing while an archive is being checked, including pictures with no world.
pub fn load_gallery(sessions_dir: &Path, folders: &[PathBuf]) -> Vec<VisitedWorld> {
    from_index(sessions_dir, picture_index(&visited::shot_index_file(), visited::screenshot_dirs_with(folders)), true)
}

/// [`load_gallery`] from a pictures' index and a visit history handed over, not read from where the app keeps them.
pub fn gallery_from(sessions_dir: &Path, index: visited::ShotIndex, history: &visited::History) -> Vec<VisitedWorld> {
    from_index_with(sessions_dir, index, history, true)
}

fn from_index(sessions_dir: &Path, index: visited::ShotIndex, unplaced: bool) -> Vec<VisitedWorld> {
    from_index_with(sessions_dir, index, &visited::History::load(&visited::history_file()), unplaced)
}

fn from_index_with(sessions_dir: &Path, index: visited::ShotIndex, history: &visited::History, unplaced: bool) -> Vec<VisitedWorld> {
    let mut worlds: BTreeMap<String, VisitedWorld> = BTreeMap::new();
    fn entry<'a>(worlds: &'a mut BTreeMap<String, VisitedWorld>, id: &str) -> &'a mut VisitedWorld {
        worlds.entry(id.to_string()).or_insert_with(|| VisitedWorld {
            id: id.to_string(),
            name: String::new(),
            last: 0,
            visits: 0,
            shots: Vec::new(),
            layers: HashMap::new(),
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
    for (shot, layers) in group_layers(index.every_shot()) {
        if shot.info.world_id.is_none() && !unplaced { continue; }
        let id = shot.info.world_id.clone().unwrap_or_default();
        let w = entry(&mut worlds, &id);
        w.last = w.last.max(shot.info.taken);
        if w.name.is_empty() {
            w.name = shot.info.world_name.clone().unwrap_or_default();
        }
        if !layers.is_empty() {
            w.layers.insert(shot.path.clone(), layers);
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
            w.name = if w.id.is_empty() { tr("Pictures without a world") } else { tr("Unnamed world") }.into();
        }
    }
    out.sort_by(|a, b| b.last.cmp(&a.last).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Changes when the background folder scan found pictures (as it goes, and when it finishes). Pages may keep showing
/// their cached listing meanwhile.
pub fn picture_revision() -> u64 {
    PICTURE_REVISION.load(std::sync::atomic::Ordering::Acquire)
}
static PICTURE_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

static LIVE_PICTURES: Mutex<Option<visited::ShotIndex>> = Mutex::new(None);

/// The interfaces drawn again when the scan found pictures ([`wake_on_pictures`]): the window's.
static PICTURE_WAKE: Mutex<Vec<egui::Context>> = Mutex::new(Vec::new());

/// Has the folder scan ask `ctx` for a frame whenever [`picture_revision`] changes. Nothing told a page that the scan
/// had found pictures, so Photos & captures drew itself ten times a second to look.
pub fn wake_on_pictures(ctx: &egui::Context) {
    let mut woken = PICTURE_WAKE.lock().unwrap_or_else(|e| e.into_inner());
    if !woken.contains(ctx) {
        woken.push(ctx.clone());
    }
}

/// What the scan found so far, for the pages: the index, a new revision, and a frame to show it.
fn publish_pictures(index: &visited::ShotIndex) {
    *LIVE_PICTURES.lock().unwrap_or_else(|e| e.into_inner()) = Some(index.clone());
    pictures_changed();
}

/// A new revision of the pictures, and a frame for the pages that show them.
fn pictures_changed() {
    PICTURE_REVISION.fetch_add(1, std::sync::atomic::Ordering::Release);
    for ctx in &*PICTURE_WAKE.lock().unwrap_or_else(|e| e.into_inner()) {
        orbiters_ui::anim::request_frame(ctx);
    }
}

fn picture_index(file: &Path, dirs: Vec<PathBuf>) -> visited::ShotIndex {
    let index = LIVE_PICTURES.lock().unwrap_or_else(|e| e.into_inner()).clone()
        .unwrap_or_else(|| visited::ShotIndex::load(file));
    picture_cache::set_index(&index);
    scan_pictures(file, dirs);
    index
}

/// Walks the picture folders again on a thread, for what was taken or removed since the last walk: Photos & captures
/// asks every few seconds while it shows. [`picture_revision`] changes if the walk found something, and only then.
pub fn rescan_pictures(folders: &[PathBuf]) {
    scan_pictures(&visited::shot_index_file(), visited::screenshot_dirs_with(folders));
}

/// One walk of `dirs` at a time, on a thread: a walk asked for while one runs is not started (the next one asked sees
/// what it would have).
fn scan_pictures(file: &Path, dirs: Vec<PathBuf>) {
    static SCANNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let file = file.to_path_buf();
    one_at_a_time(&SCANNING, "picture-scan", move || {
        let _one = shot_index_lock();
        let mut current = visited::ShotIndex::load(&file);
        if current.refresh_progress(&dirs, publish_pictures) {
            let _ = current.save(&file);
            publish_pictures(&current);
        }
    });
}

/// Runs `work` on a thread named `name` unless one started with the same `running` flag still runs; says whether it
/// started. The flag is cleared however the work ends: a panic in it (or a thread the system refuses) used to leave
/// the flag set, and no folder scan ran again for the rest of the process.
fn one_at_a_time(running: &'static std::sync::atomic::AtomicBool, name: &str, work: impl FnOnce() + Send + 'static) -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Clear(&'static AtomicBool);
    impl Drop for Clear {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    if running.swap(true, Ordering::AcqRel) {
        return false;
    }
    // Dropped with the closure when the thread cannot start, at the end of the work, or as a panic unwinds through it.
    let clear = Clear(running);
    // Work of the window while it runs (it writes the pictures' index): the window does not close to the tray meanwhile.
    let working = crate::pages::Working::begin();
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let _clear = clear;
            let _working = working;
            work();
        })
        .is_ok()
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
    // Layers without a world of their own are shared too.
    paths.into_iter().filter_map(|p| index.hashes_of(&p).0.map(str::to_owned).map(|hash| (p, hash))).collect()
}

/// Every hash Orbiters may know each screenshot by: the file's own (what was sent before PRIV-8, and a gallery share's
/// `sourceKey`) and its copy without metadata's (what is sent now): the Map explorer recognises a picture by either.
pub fn map_hashes(paths: Vec<PathBuf>) -> HashMap<PathBuf, Vec<String>> {
    let index_file = visited::shot_index_file();
    let _one = shot_index_lock();
    let mut index = visited::ShotIndex::load(&index_file);
    if index.hash(&paths) {
        let _ = index.save(&index_file);
    }
    paths
        .into_iter()
        .map(|p| {
            let (own, clean) = index.hashes_of(&p);
            let hashes: Vec<String> = own.into_iter().chain(clean).map(str::to_owned).collect();
            (p, hashes)
        })
        .filter(|(_, hashes)| !hashes.is_empty())
        .collect()
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
    public_api_of(Server::current())
}

/// [`public_api`] of `server`.
fn public_api_of(server: Server) -> (Api, bool) {
    match std::env::var("ORBITERS_PUBLIC_API") {
        Ok(base) if !base.is_empty() => (Api::at(&base), false),
        _ => (Api::new(server), true),
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

/// A world's picture address as the Map explorer's page kept on this computer says it, reading nothing but that file
/// (never Orbiters): what a list that must not send world ids before a choice shows (verify-r21 Major 6).
pub fn kept_thumbnail(world_id: &str) -> Option<String> {
    let server = Server::current();
    let file = cache_dir().join("worlds").join(server.cache_name()).join(format!("{world_id}.json"));
    let cached: CachedInfo = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    let page = cached.page?;
    page.world.thumbnail_url.or(page.world.image_url).filter(|u| u.starts_with("https://"))
}

/// Tests: every world id this process asked Orbiters or VRChat about (a page, a picture), to count what a view sends.
#[cfg(test)]
pub static OUTBOUND: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn note_outbound(world_id: &str) {
    #[cfg(test)]
    OUTBOUND.lock().unwrap_or_else(|e| e.into_inner()).push(world_id.to_string());
    let _ = world_id;
}

/// Listed worlds are fetched again after three days, unlisted ones after one.
const LISTED_FOR: u64 = 3 * 86_400;
const UNLISTED_FOR: u64 = 86_400;

/// Worlds' pages, fetched one at a time on a background thread as cards come into view.
/// Each environment's pages apart (in memory and on disk, a folder each): one server's page is never shown for the
/// other, and a request goes to the server of the environment it was made in.
pub struct Infos {
    known: Arc<Mutex<[HashMap<String, Info>; 2]>>,
    asked: [HashSet<String>; 2],
    tx: mpsc::Sender<(Server, String)>,
    ctx: egui::Context,
}

/// Where an environment's pages are in [`Infos`].
fn env_index(server: Server) -> usize {
    match server {
        Server::Production => 0,
        Server::Development => 1,
    }
}

impl Infos {
    pub fn new(ctx: &egui::Context) -> Self {
        let known: Arc<Mutex<[HashMap<String, Info>; 2]>> = Arc::default();
        let (tx, rx) = mpsc::channel::<(Server, String)>();
        let clock = ctx.clone();
        let (k, ctx) = (known.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("world-infos".into())
            .spawn(move || {
                while let Ok((server, id)) = rx.recv() {
                    let (api, keep) = public_api_of(server);
                    let dir = cache_dir().join("worlds").join(server.cache_name());
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
                    k.lock().unwrap()[env_index(server)].insert(id, info);
                    ctx.request_repaint();
                    if !fresh {
                        // Gentle on Orbiters (and on VRChat behind it).
                        std::thread::sleep(Duration::from_millis(150));
                    }
                }
            })
            .expect("world infos thread");
        Self { known, asked: Default::default(), tx, ctx: clock }
    }

    /// The world's page once fetched; asks for it the first time.
    pub fn get(&mut self, id: &str) -> Option<Info> {
        // Called for every card in view at every frame: the id is copied only the first time.
        let server = Server::current();
        let e = env_index(server);
        if !self.asked[e].contains(id) {
            self.asked[e].insert(id.to_string());
            // What is kept on this computer shows in this very frame, however old; a new page takes its place.
            let kept = at_once::read(self.ctx.input(|i| i.time), false, || {
                let (_, keep) = public_api_of(server);
                let file = cache_dir().join("worlds").join(server.cache_name()).join(format!("{id}.json"));
                keep.then(|| std::fs::read(file).ok().and_then(|b| serde_json::from_slice::<CachedInfo>(&b).ok())).flatten()?.page
            });
            if let Some(Some(page)) = kept {
                self.known.lock().unwrap()[e].entry(id.to_string()).or_insert_with(|| Info::Listed(Box::new(page)));
            }
            note_outbound(id);
            let _ = self.tx.send((server, id.to_string()));
        }
        self.known.lock().unwrap()[e].get(id).cloned()
    }

    /// The world's page when it was fetched already; asks for nothing.
    pub fn known(&self, id: &str) -> Option<Info> {
        self.known.lock().unwrap()[env_index(Server::current())].get(id).cloned()
    }

    /// Fetches it again (after sharing something there).
    pub fn refresh(&mut self, id: &str) {
        let server = Server::current();
        let _ = std::fs::remove_file(cache_dir().join("worlds").join(server.cache_name()).join(format!("{id}.json")));
        self.asked[env_index(server)].remove(id);
    }
}

/// A picture to show.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Picture {
    /// A VRChat person's picture (`user`: theirs, a `usr_…` id, when known), read through the service with the session of
    /// `scope`, kept small on this computer and completed from VRCX's and VRCNext's copies ([`profile_pictures`]).
    Profile { scope: crate::controller::vrchat_data::Scope, url: String, user: Option<String> },
    /// A protected sharing-contract picture, fetched with the current environment's account on a loader thread.
    Capture { development: bool, id: u64, large: bool },
    /// A 3D capture's own picture on Orbiters (its item's `capture.pictureUrl`, which names its revision), its thumbnail
    /// unless `large`: what the card of a capture that is on Orbiters only shows (its folder on this computer deleted,
    /// or never here). Read with the account when signed in, and kept on this computer once read
    /// ([`capture_view_file`]): deleting a capture's local files never takes its card's picture away.
    CaptureView { development: bool, id: u64, url: String, large: bool },
    /// A screenshot, at most `width` wide.
    Shot { path: PathBuf, size: u64, width: u32 },
    /// A layer of a multi-layer picture ([`Layer`]), at most `width` wide, with its transparency.
    Layer { path: PathBuf, width: u32 },
    /// A session's thumbnail.
    Session(PathBuf),
    /// A world's picture on VRChat (as Orbiters lists it): its thumbnail, or the full picture when `large`.
    Remote { world_id: String, url: String, large: bool },
    /// The picture of the world VRChat will start in, at the address VRChat gave for it (a friend's place, an invite's):
    /// read with the session of `scope`, at the size given, and kept on this computer as [`Picture::Remote`] is.
    World { scope: crate::controller::vrchat_data::Scope, world_id: String, url: String },
    /// A world's thumbnail only if it is already kept on this computer: never fetched (verify-r21 Major 6).
    RemoteKept { world_id: String, url: String },
    /// An avatar's thumbnail at the address VRChat gave for it (the avatar gun's picker, its cards): kept on this computer
    /// once read, read through the service with the session of `scope` (no cookie leaves it), as [`Picture::World`] is.
    Avatar { scope: crate::controller::vrchat_data::Scope, avatar_id: String, url: String },
    /// An event's cover on Orbiters, read with the account's credential when signed in (members-only covers need it).
    EventCover { development: bool, id: String },
    /// Another picture a few pixels wide, to draw large and soft as its light (behind an event, its window).
    Glow(Box<Picture>),
    /// A picture of a community gallery on Orbiters: its preview, or the full picture when `large` (kept in memory).
    /// `id` is its placement (0: a picture that is not a gallery's, such as an author's); an empty `url` or one that
    /// does not load (Discord's links expire) is renewed by Orbiters with the member's credential. `asset`: the asset
    /// whose showcase gallery it is in (0: a community gallery's), where its link is renewed.
    Gallery { development: bool, asset: u64, id: u64, url: String, large: bool },
}

/// Where a picture stands ([`Pictures::look`]).
pub enum Look {
    NotAsked,
    Loading,
    Ready(TextureHandle),
    Missing,
}

enum Slot {
    /// Being read; `tries`: the failed readings before this one (a retry is not "loading" for the page).
    Loading { tries: u8 },
    Ready(TextureHandle),
    /// Could not be read (`since`: when it failed, egui's clock), `tries` times: read again when asked after
    /// [`retry_after`]. [`NEVER`] tries: its reading panicked, it is not read again.
    Missing { since: f64, tries: u8 },
}

/// A picture whose reading panicked is not read again (the same file would do it again).
const NEVER: u8 = u8::MAX;

/// A picture that could not be read is read again when asked this long after (Night PAGES-UI-4: one network hiccup,
/// a server answering 502 during a deploy, left its card blank until the window was reopened): from 5 s for what comes
/// over the network, 30 s for a file of this computer (a missing file stays missing, mostly), doubling, at most 5 min.
fn retry_after(picture: &Picture, tries: u8) -> Option<f64> {
    if tries == NEVER {
        return None;
    }
    let local = match picture {
        Picture::Glow(inner) => matches!(**inner, Picture::Shot { .. } | Picture::Layer { .. } | Picture::Session(_)),
        p => matches!(p, Picture::Shot { .. } | Picture::Layer { .. } | Picture::Session(_)),
    };
    let first = if local { 30.0 } else { 5.0 };
    Some((first * 2f64.powi(tries.saturating_sub(1).min(16) as i32)).min(300.0))
}

/// A picture that could not be read and nobody asked for since this long is forgotten (its next ask reads it).
const FORGET_MISSING: f64 = 60.0;

/// A picture as the pages keep it, and when they last asked for it (egui's clock).
struct Kept {
    slot: Slot,
    used: f64,
    /// Shown while it is not read yet: a person's picture before their new one ([`profile_pictures`]).
    interim: Option<TextureHandle>,
}

/// A picture read by a loader: its image, or none; `Err` when its reading panicked. `true`: only a stand-in shown until
/// the picture itself arrives.
type Loaded = (Picture, Result<Option<ColorImage>, ()>, bool);

/// Bytes of picture textures kept (about ninety thumbnails, three screens of a world's pictures): those asked for
/// most recently. The others are decoded again (from the small copies kept on disk) when they come back into view.
const KEEP_BYTES: usize = 48 << 20;

/// A waiting picture nobody asked for since this long (scrolled past) is not decoded after all.
const STALE_REQUEST: f64 = 2.0;

/// Pictures decoded by three loader threads (newest requests first), uploaded as textures on the interface thread.
/// The threads start with the first picture asked for: a set made and never used (a picture window makes one that its
/// first frame replaces) costs none.
pub struct Pictures {
    slots: HashMap<Picture, Kept>,
    queue: Arc<(Mutex<Vec<Picture>>, std::sync::Condvar)>,
    done: Arc<Mutex<Vec<Loaded>>>,
    /// When the pictures were last trimmed to [`KEEP_BYTES`] (egui's clock).
    trimmed: f64,
    /// The clock of the frame asking now, and of the frame before it: what that one asked for is on screen. Trimming
    /// runs at a frame's first ask, before its other pictures were asked; after the window stood still (no frame drawn)
    /// for more than a second, "asked in the last second" protected none of them (PAGES-UI-5).
    frame: f64,
    frame_before: f64,
    stopped: Arc<std::sync::atomic::AtomicBool>,
    /// What the loaders start with (the interface they wake, where profile pictures are read), until they started.
    unstarted: Option<(egui::Context, Option<crate::backend::Ctl>)>,
    /// Where profile pictures are read (the background's too).
    backend: Option<crate::backend::Ctl>,
    /// The interface to wake when a picture that could not be read may be read again.
    wake: egui::Context,
}

impl Pictures {
    pub fn new(ctx: &egui::Context) -> Self {
        Self::with_backend(ctx, None)
    }
    pub fn with_backend(ctx: &egui::Context, backend: Option<crate::backend::Ctl>) -> Self {
        Self {
            slots: HashMap::new(),
            queue: Arc::default(),
            done: Arc::default(),
            trimmed: f64::NEG_INFINITY,
            frame: f64::NEG_INFINITY,
            frame_before: f64::NEG_INFINITY,
            stopped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            unstarted: Some((ctx.clone(), backend.clone())),
            backend,
            wake: ctx.clone(),
        }
    }

    /// Starts the loaders, once.
    fn start(&mut self) {
        let Some((ctx, backend)) = self.unstarted.take() else { return };
        for i in 0..3 {
            let (queue, done, ctx) = (self.queue.clone(), self.done.clone(), ctx.clone());
            let backend = backend.clone();
            let stopped = self.stopped.clone();
            std::thread::Builder::new()
                .name(format!("pictures-{i}"))
                .spawn(move || {
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
                        // A decoder that panics on a file of any origin ends this picture, not the loader (three
                        // such files stopped every picture of the page).
                        let image = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            if let Picture::Profile { scope, url, user } = &picture {
                                // Kept, or VRCX's or VRCNext's copy: now. Else asked of VRChat; the answer comes later.
                                let deliver = |image: Option<ColorImage>, interim: bool| {
                                    done.lock().unwrap_or_else(|e| e.into_inner()).push((picture.clone(), Ok(image), interim));
                                    ctx.request_repaint();
                                };
                                let reply: profile_pictures::Reply = {
                                    let (done, ctx, picture) = (done.clone(), ctx.clone(), picture.clone());
                                    Box::new(move |image| {
                                        done.lock().unwrap_or_else(|e| e.into_inner()).push((picture, Ok(image), false));
                                        ctx.request_repaint();
                                    })
                                };
                                let via = backend.as_ref().map(|b| profile_fetch(b, scope));
                                profile_pictures::Profiles::of(scope.account().as_ref())
                                    .load(url, user.as_deref(), via, |old| deliver(Some(old), true), reply)
                                    .map(|image| Some(Some(image)))
                                    .unwrap_or(None)
                            } else if let Picture::World { scope, world_id, url } = &picture {
                                Some(world_picture(backend.as_ref(), scope, world_id, url))
                            } else if let Picture::Avatar { scope, avatar_id, url } = &picture {
                                Some(avatar_picture(backend.as_ref(), scope, avatar_id, url))
                            } else {
                                // The environment of the moment, for each picture (one chosen since the loader started is
                                // the one asked; CORE2-13).
                                let (api, keep) = public_api();
                                Some(decode(&api, keep, &picture))
                            }
                        }))
                        .map_err(|_| ());
                        // `None`: asked of VRChat, delivered when it comes.
                        let image = match image {
                            Ok(Some(image)) => Ok(image),
                            Ok(None) => continue,
                            Err(()) => Err(()),
                        };
                        done.lock().unwrap_or_else(|e| e.into_inner()).push((picture, image, false));
                        ctx.request_repaint();
                    }
                })
                .expect("picture loader");
        }
    }

    /// Whether the loader threads were started (tests).
    #[cfg(test)]
    pub(crate) fn started(&self) -> bool {
        self.unstarted.is_none()
    }

    /// The picture as a texture once decoded; asks for it the first time (and again to bring it forward while it
    /// waits, so what is on screen loads first).
    pub fn get(&mut self, ctx: &egui::Context, picture: &Picture) -> Option<TextureHandle> {
        let now = ctx.input(|i| i.time);
        self.get_at(&crate::overlay::PaintTextures::of(ctx), now, picture)
    }

    pub(crate) fn get_at(&mut self, textures: &crate::overlay::PaintTextures, now: f64, picture: &Picture) -> Option<TextureHandle> {
        if now != self.frame {
            (self.frame_before, self.frame) = (self.frame, now);
        }
        for (p, image, interim) in std::mem::take(&mut *self.done.lock().unwrap_or_else(|e| e.into_inner())) {
            // One no longer wanted (scrolled past while it was decoded) is dropped.
            let Some(kept) = self.slots.get_mut(&p) else { continue };
            if interim {
                if let (Ok(Some(image)), Slot::Loading { .. }) = (image, &kept.slot) {
                    kept.interim = Some(textures.load_texture(format!("picture-{:x}-before", key(&p)), image, TextureOptions::LINEAR));
                }
                continue;
            }
            let tries = match kept.slot {
                Slot::Loading { tries } => tries,
                _ => 0,
            };
            kept.slot = match image {
                Ok(Some(image)) => {
                    kept.interim = None;
                    let texture = textures.load_texture(format!("picture-{:x}", key(&p)), image, TextureOptions::LINEAR);
                    if let Some(name) = at_once::name(&p) {
                        at_once::keep(textures, name, &texture);
                    }
                    Slot::Ready(texture)
                }
                Ok(None) => Slot::Missing { since: now, tries: tries.saturating_add(1).min(NEVER - 1) },
                Err(()) => Slot::Missing { since: now, tries: NEVER },
            };
        }
        if now - self.trimmed > 0.5 {
            self.trimmed = now;
            self.trim(now);
        }
        match self.slots.get_mut(picture) {
            Some(kept) => {
                kept.used = now;
                let interim = kept.interim.clone();
                match &kept.slot {
                    Slot::Ready(t) => Some(t.clone()),
                    &Slot::Missing { since, tries } => {
                        // Asked again (it is on screen): read again once its wait is over; woken then.
                        let Some(wait) = retry_after(picture, tries) else { return interim };
                        if now - since >= wait {
                            kept.slot = Slot::Loading { tries };
                            let (lock, ready) = &*self.queue;
                            lock.lock().unwrap_or_else(|e| e.into_inner()).push(picture.clone());
                            ready.notify_one();
                        } else {
                            self.wake.request_repaint_after(std::time::Duration::from_secs_f64(since + wait - now));
                        }
                        interim
                    }
                    Slot::Loading { .. } => {
                        // (Not in the queue: a loader has it, or VRChat is being asked for it.)
                        let (lock, _) = &*self.queue;
                        let mut q = lock.lock().unwrap();
                        if let Some(i) = q.iter().position(|p| p == picture) {
                            let p = q.remove(i);
                            q.push(p);
                        } else if let Picture::Profile { scope, url, user } = picture {
                            profile_pictures::Profiles::of(scope.account().as_ref()).bump(url, user.as_deref());
                        }
                        interim
                    }
                }
            }
            None => {
                // One of VRChat's, shown on another page or under another state of the sign-in already, or kept on this
                // computer: in this very frame (REQUEST28).
                let name = at_once::name(picture);
                let mut interim = None;
                if let Some(name) = name {
                    let texture = at_once::texture(textures, &name).or_else(|| {
                        let (image, exact) = at_once::kept(picture, now)?;
                        let texture = textures.load_texture(format!("picture-{:x}", key(picture)), image, TextureOptions::LINEAR);
                        if exact {
                            at_once::keep(textures, name, &texture);
                            return Some(texture);
                        }
                        interim = Some(texture);
                        None
                    });
                    if let Some(texture) = texture {
                        self.slots.insert(picture.clone(), Kept { slot: Slot::Ready(texture.clone()), used: now, interim: None });
                        return Some(texture);
                    }
                }
                self.start();
                self.slots.insert(picture.clone(), Kept { slot: Slot::Loading { tries: 0 }, used: now, interim: interim.clone() });
                let (lock, ready) = &*self.queue;
                lock.lock().unwrap().push(picture.clone());
                ready.notify_one();
                interim
            }
        }
    }

    /// Where `picture` stands, asking nothing.
    pub fn look(&self, picture: &Picture) -> Look {
        match self.slots.get(picture).map(|k| &k.slot) {
            None => Look::NotAsked,
            Some(Slot::Loading { .. }) => Look::Loading,
            Some(Slot::Ready(t)) => Look::Ready(t.clone()),
            Some(Slot::Missing { .. }) => Look::Missing,
        }
    }

    /// One that could not be read is read again at its next ask, without its wait (the person asked for it anew).
    pub fn again(&mut self, picture: &Picture) {
        if let Some(Slot::Missing { since, tries }) = self.slots.get_mut(picture).map(|k| &mut k.slot) {
            if *tries != NEVER {
                (*since, *tries) = (f64::NEG_INFINITY, 0);
            }
        }
    }

    /// The pictures of VRChat people a page will show (the whole friends list, in its order: `(address, usr_ id)`), read
    /// with the session of `scope`: those not kept on this computer are completed from VRCX's and VRCNext's copies, and
    /// the rest asked of VRChat in the background, slowly ([`profile_pictures::Profiles::warm`]).
    /// A person's picture that could not be had is asked again at its next look, not after its wait (up to five minutes):
    /// a new list, or the session back after waiting, is when it can be had.
    pub fn warm_profiles(&mut self, scope: &crate::controller::vrchat_data::Scope, items: Vec<(String, Option<String>)>) {
        for (picture, kept) in &mut self.slots {
            if let (Picture::Profile { .. }, Slot::Missing { since, tries }) = (picture, &mut kept.slot) {
                if *tries != NEVER {
                    (*since, *tries) = (f64::NEG_INFINITY, 0);
                }
            }
        }
        if let Some(backend) = &self.backend {
            profile_pictures::Profiles::of(scope.account().as_ref()).warm(items, profile_fetch(backend, scope));
        }
    }

    /// Whether pictures asked for are still being decoded.
    pub fn loading(&self) -> bool {
        self.slots.values().any(|k| matches!(k.slot, Slot::Loading { tries: 0 }))
    }

    /// Requests nobody repeated for [`STALE_REQUEST`] leave the queue, and the textures asked for longest ago go
    /// beyond [`KEEP_BYTES`] (a big library browsed end to end would otherwise keep hundreds).
    fn trim(&mut self, now: f64) {
        // What the frame before asked for is on screen, however long ago that frame was drawn.
        let shown = self.frame_before;
        let stale = |k: &Kept| k.used < shown && match k.slot {
            Slot::Loading { .. } => now - k.used > STALE_REQUEST,
            // Not read, and nobody asks any more: forgotten (a page browsed end to end kept every one).
            Slot::Missing { .. } => now - k.used > FORGET_MISSING,
            Slot::Ready(_) => false,
        };
        {
            let (lock, _) = &*self.queue;
            let slots = &self.slots;
            lock.lock().unwrap().retain(|p| slots.get(p).is_some_and(|k| !stale(k)));
        }
        self.slots.retain(|_, k| !stale(k));
        let bytes = |k: &Kept| {
            let shown = match &k.slot {
                Slot::Ready(t) => t.byte_size(),
                _ => 0,
            };
            shown + k.interim.as_ref().map_or(0, |t| t.byte_size())
        };
        let mut total: usize = self.slots.values().map(bytes).sum();
        if total <= KEEP_BYTES {
            return;
        }
        // What was on screen in the last second, or in the frame before, stays, whatever it weighs.
        let mut old: Vec<(f64, Picture, usize)> =
            self.slots.iter().filter(|(_, k)| matches!(k.slot, Slot::Ready(_)) && k.used < now - 1.0 && k.used < shown).map(|(p, k)| (k.used, p.clone(), bytes(k))).collect();
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
        self.slots.retain(|p, _| !matches!(p, Picture::Shot { width, .. } | Picture::Layer { width, .. } if *width > THUMB_WIDTH));
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

/// How a person's picture is asked of VRChat: through the service, with the session of `scope`.
fn profile_fetch(backend: &crate::backend::Ctl, scope: &crate::controller::vrchat_data::Scope) -> profile_pictures::Fetch {
    use crate::controller::vrchat_data::{Request, Response};
    let (backend, scope) = (backend.clone(), scope.clone());
    std::sync::Arc::new(move |url: &str| match backend.vrchat_data(scope.clone(), Request::Image { url: url.to_string() }) {
        Ok(Response::Image(bytes)) => Ok(bytes),
        Ok(_) => Err(String::new()),
        Err(e) => Err(e),
    })
}

/// A picture on this computer (a screenshot, from its kept thumbnail, or a session's thumbnail) decoded on the calling
/// thread, for the drag image of Photos & captures' cards; None for pictures from the network.
pub fn decode_local(picture: &Picture) -> Option<ColorImage> {
    match picture {
        Picture::Shot { .. } | Picture::Layer { .. } | Picture::Session(_) => decode(&Api::new(Server::Production), true, picture),
        _ => None,
    }
}

/// Thumbnails are this wide (kept on disk).
pub const THUMB_WIDTH: u32 = 560;

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
            let (api, token) = crate::member::reading(server)?.api();
            let r = api.agent().get(&api.url(&format!("captures/picture/{id}/file"))).header("Authorization", &format!("Bearer {token}")).call().ok()?;
            if !r.status().is_success() {
                return None;
            }
            let bytes = r.into_body().with_config().limit(32 << 20).read_to_vec().ok()?;
            let image = image::load_from_memory(&bytes).ok()?.thumbnail(if *large { 2048 } else { 560 }, if *large { 4096 } else { 1120 }).to_rgba8();
            Some(ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw()))
        }
        Picture::CaptureView { development, id, url, large } => {
            let server = if *development { Server::Development } else { Server::Production };
            // As the other public reads: another server when `ORBITERS_PUBLIC_API` names one (screenshots), asked
            // without the account and kept in memory only.
            let (api, token) = match std::env::var("ORBITERS_PUBLIC_API") {
                Ok(base) if !base.is_empty() => (Api::at(&base), None),
                _ => match crate::member::reading(server).map(|m| m.api()) {
                    Some((api, token)) => (api, Some(token)),
                    None => (Api::new(server), None),
                },
            };
            let file = keep.then(|| capture_view_file(*development, *id, url, *large));
            let image = capture_view(&api, token.as_deref(), url, *large, file.as_deref())?;
            Some(ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw()))
        }
        Picture::EventCover { development, id } => {
            let server = if *development { Server::Development } else { Server::Production };
            let api = crate::member::reading(server).map_or_else(|| Api::new(server), |m| m.api().0);
            let r = api.agent().get(&api.event_banner_url(id).ok()?).call().ok()?;
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
        Picture::Gallery { development, asset, id, url, large } => {
            let given = (!url.is_empty()).then(|| api.image(url).ok()).flatten();
            let bytes = match given {
                Some(b) => b,
                None if *id != 0 => {
                    let server = if *development { Server::Development } else { Server::Production };
                    let (api, token) = crate::member::reading(server)?.api();
                    let renewed = api.picture_source(&token, *asset, *id, !url.is_empty()).ok()?;
                    let url = if *large { renewed.full_url.or(renewed.src) } else { renewed.src }?;
                    api.image(&url).ok()?
                }
                None => return None,
            };
            let image = image::load_from_memory(&bytes).ok()?.thumbnail(if *large { 2048 } else { 560 }, if *large { 4096 } else { 1120 }).to_rgba8();
            Some(ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw()))
        }
        Picture::Session(dir) => {
            let (w, h, rgba) = bundle::read_thumbnail(dir)?;
            Some(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba))
        }
        Picture::Shot { path, width, .. } => {
            if let Some((h, rgba)) = picture_cache::read(picture, path, *width) {
                return Some(ColorImage::from_rgba_unmultiplied([h.width as usize, h.height as usize], &rgba));
            }
            let before = picture_cache::stamp(path);
            let (w, h, rgba) = decode_png(&std::fs::read(path).ok()?)?;
            let (tw, th, rgb) = bundle::downscale(&rgba, w, h, *width);
            picture_cache::write(picture, path, *width, before, &qoi::encode(&rgb, tw, th, 3));
            Some(ColorImage::from_rgb([tw as usize, th as usize], &rgb))
        }
        Picture::Layer { path, width } => {
            if let Some((h, rgba)) = picture_cache::read(picture, path, *width) {
                return Some(ColorImage::from_rgba_premultiplied([h.width as usize, h.height as usize], &rgba));
            }
            let before = picture_cache::stamp(path);
            let (w, h, rgba) = decode_png(&std::fs::read(path).ok()?)?;
            let (tw, th, rgba) = downscale_premultiplied(&rgba, w, h, *width);
            if tw == 0 {
                return None;
            }
            picture_cache::write(picture, path, *width, before, &qoi::encode(&rgba, tw, th, 4));
            Some(ColorImage::from_rgba_premultiplied([tw as usize, th as usize], &rgba))
        }
        Picture::Remote { world_id, url, large } => {
            let file = cache_dir().join("worlds").join(remote_file_name(world_id, url, *large));
            // One kept that cannot be read (cut short) is fetched again.
            let kept = keep.then(|| std::fs::read(&file).ok()).flatten().and_then(|b| decode_image(&b));
            let (w, h, rgba) = match kept {
                Some(picture) => picture,
                None => {
                    note_outbound(world_id);
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
        // (Read with a session, by the loader itself.)
        Picture::World { world_id, url, .. } => kept_world(world_id, url),
        Picture::Avatar { avatar_id, url, .. } => kept_avatar(avatar_id, url),
        Picture::RemoteKept { world_id, url } => {
            let file = cache_dir().join("worlds").join(remote_file_name(world_id, url, false));
            let (w, h, rgba) = decode_image(&std::fs::read(&file).ok()?)?;
            Some(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba))
        }
    }
}

fn kept_world(world_id: &str, url: &str) -> Option<ColorImage> {
    let (w, h, rgba) = decode_image(&std::fs::read(cache_dir().join("worlds").join(remote_file_name(world_id, url, false))).ok()?)?;
    Some(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba))
}

/// [`Picture::World`], on a loader thread: kept, or asked once of VRChat through the service (without a session, as any
/// world's public picture) and kept.
fn world_picture(backend: Option<&crate::backend::Ctl>, scope: &crate::controller::vrchat_data::Scope, world_id: &str, url: &str) -> Option<ColorImage> {
    if let Some(kept) = kept_world(world_id, url) {
        return Some(kept);
    }
    note_outbound(world_id);
    let bytes = match backend.filter(|_| scope.signed_in()) {
        Some(backend) => profile_fetch(backend, scope)(url).ok()?,
        // Tests reach nothing but their own stand-in for the service.
        None if cfg!(test) => return None,
        None => public_api().0.image(url).ok()?,
    };
    let (w, h, rgba) = decode_image(&bytes)?;
    let file = cache_dir().join("worlds").join(remote_file_name(world_id, url, false));
    if file.parent().is_some_and(|dir| std::fs::create_dir_all(dir).is_ok()) {
        let _ = orbiters_core::write_atomic(&file, &bytes);
    }
    Some(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba))
}

fn avatar_file(avatar_id: &str, url: &str) -> PathBuf {
    cache_dir().join("avatars").join(remote_file_name(avatar_id, url, false))
}

fn kept_avatar(avatar_id: &str, url: &str) -> Option<ColorImage> {
    if !orbiters_contracts::avatar_gun::is_avatar_id(avatar_id) {
        return None;
    }
    let (w, h, rgba) = decode_image(&std::fs::read(avatar_file(avatar_id, url)).ok()?)?;
    Some(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba))
}

/// [`Picture::Avatar`], on a loader thread: kept, or read through the service (VRChat's image hosts with the person's
/// session) and kept; without one, as any public picture.
fn avatar_picture(backend: Option<&crate::backend::Ctl>, scope: &crate::controller::vrchat_data::Scope, avatar_id: &str, url: &str) -> Option<ColorImage> {
    if let Some(kept) = kept_avatar(avatar_id, url) {
        return Some(kept);
    }
    if !orbiters_contracts::avatar_gun::vrchat_picture(url) || !orbiters_contracts::avatar_gun::is_avatar_id(avatar_id) {
        return None;
    }
    let bytes = match backend.filter(|_| scope.signed_in()) {
        Some(backend) => profile_fetch(backend, scope)(url).ok()?,
        None if cfg!(test) => return None,
        None => public_api().0.image(url).ok()?,
    };
    let (w, h, rgba) = decode_image(&bytes)?;
    let file = avatar_file(avatar_id, url);
    if file.parent().is_some_and(|dir| std::fs::create_dir_all(dir).is_ok()) {
        let _ = orbiters_core::write_atomic(&file, &bytes);
    }
    Some(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba))
}

/// The file a world's picture read at `url` is kept in: by world and by address (another environment's picture, or the
/// same world's new one, is another address, so never the file kept for the other; CORE2-13).
fn remote_file_name(world_id: &str, url: &str, large: bool) -> String {
    use sha2::{Digest, Sha256};
    let digest: String = Sha256::digest(url.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("{world_id}-{digest}{}.png", if large { "-large" } else { "" })
}

/// Where a capture's picture on Orbiters is kept once read ([`Picture::CaptureView`]): by environment, capture and the
/// address it was read at (a new picture or revision is another address, so another file).
pub fn capture_view_file(development: bool, id: u64, url: &str, large: bool) -> PathBuf {
    // FNV-1a: the same name in every build.
    let h = url.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    let env = if development { "dev" } else { "prod" };
    cache_dir().join("captures").join(format!("{env}-{id}-{h:016x}{}.img", if large { "-large" } else { "" }))
}

/// A capture's picture on Orbiters as RGBA, at most 560 (or, `large`, 2048) wide: from `file` when kept there and
/// readable, else asked of `api` (with `token`) and then kept in `file`.
pub(crate) fn capture_view(api: &Api, token: Option<&str>, url: &str, large: bool, file: Option<&Path>) -> Option<image::RgbaImage> {
    let kept = file.and_then(|f| std::fs::read(f).ok()).and_then(|b| image::load_from_memory(&b).ok());
    let picture = match kept {
        Some(p) => p,
        None => {
            let bytes = api.capture_picture(token, url, !large).ok()?;
            let picture = image::load_from_memory(&bytes).ok()?;
            if let Some(f) = file {
                if f.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) {
                    let _ = orbiters_core::write_atomic(f, &bytes);
                }
            }
            picture
        }
    };
    let (w, h) = if large { (2048, 4096) } else { (560, 1120) };
    // Made smaller to fit, never larger.
    Some(if picture.width() > w || picture.height() > h { picture.thumbnail(w, h) } else { picture }.to_rgba8())
}

/// A picture with transparency at most `max_width` wide (a box filter in whole steps, as [`bundle::downscale`]),
/// premultiplied by its alpha: an edge keeps its colour instead of darkening toward the transparent black around it.
fn downscale_premultiplied(rgba: &[u8], width: u32, height: u32, max_width: u32) -> (u32, u32, Vec<u8>) {
    if width == 0 || height == 0 || (rgba.len() as u64) < u64::from(width) * u64::from(height) * 4 {
        return (0, 0, Vec::new());
    }
    let factor = width.div_ceil(max_width.max(1)).max(1);
    let (tw, th) = ((width / factor).max(1), (height / factor).max(1));
    let n = factor * factor;
    let mut out = vec![0u8; (tw * th * 4) as usize];
    for ty in 0..th {
        for tx in 0..tw {
            let mut acc = [0u32; 4];
            for dy in 0..factor {
                let y = (ty * factor + dy).min(height - 1);
                for dx in 0..factor {
                    let x = (tx * factor + dx).min(width - 1);
                    let i = ((y * width + x) * 4) as usize;
                    let a = u32::from(rgba[i + 3]);
                    for c in 0..3 {
                        acc[c] += u32::from(rgba[i + c]) * a;
                    }
                    acc[3] += a;
                }
            }
            let o = ((ty * tw + tx) * 4) as usize;
            for c in 0..3 {
                out[o + c] = (acc[c] / (255 * n)) as u8;
            }
            out[o + 3] = (acc[3] / n) as u8;
        }
    }
    (tw, th, out)
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
pub(crate) mod tests {
    use super::*;

    /// Verifier (CORE2-13): a world's picture from one environment (or a new one of the same world) is kept apart from
    /// the other's: another address, another file.
    #[test]
    fn a_worlds_picture_is_kept_by_its_address() {
        let w = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd";
        let prod = remote_file_name(w, "https://api.orbiters.cc/files/1.png", false);
        let dev = remote_file_name(w, "http://127.0.0.1:4100/files/1.png", false);
        assert_ne!(prod, dev);
        assert_eq!(prod, remote_file_name(w, "https://api.orbiters.cc/files/1.png", false), "the same address, the same file");
        assert_ne!(prod, remote_file_name(w, "https://api.orbiters.cc/files/1.png", true), "the large one apart");
        assert!(prod.starts_with(w) && prod.ends_with(".png"));
    }

    /// Held by a test that counts on the pictures' revision (one for the process) staying as it left it.
    pub(crate) fn pictures_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What the scan does for the pages when it found pictures, the index left as it is (the pages' tests).
    pub(crate) fn publish_for_test() {
        pictures_changed();
    }

    fn shot(path: &str, world: Option<&str>, taken: u64) -> Shot {
        let info = visited::ShotInfo {
            instance: None,
            world_id: world.map(str::to_owned),
            world_name: world.map(|w| format!("{w} name")),
            taken,
            width: 1920,
            height: 1080,
        };
        Shot { path: PathBuf::from(path), size: 100, info, sha256: None, clean_sha256: None }
    }

    fn names(grouped: &[(Shot, Vec<Layer>)]) -> Vec<(String, Vec<String>)> {
        grouped
            .iter()
            .map(|(s, l)| (s.path.file_name().unwrap().to_string_lossy().into_owned(), l.iter().map(|l| l.name.clone()).collect()))
            .collect()
    }

    /// The folder scan ran once at a time behind a flag that only the end of the scan cleared: a scan that panicked left
    /// it set, and none ran again until the app restarted.
    #[test]
    fn a_scan_that_panics_does_not_stop_the_next_one() {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        static RUNNING: AtomicBool = AtomicBool::new(false);
        static DONE: AtomicU32 = AtomicU32::new(0);
        let wait = |what: &str, done: &dyn Fn() -> bool| {
            let until = std::time::Instant::now() + Duration::from_secs(10);
            while !done() {
                assert!(std::time::Instant::now() < until, "{what}");
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        // While one runs, another is not started.
        let (go, held) = mpsc::channel::<()>();
        assert!(one_at_a_time(&RUNNING, "test-scan", move || {
            let _ = held.recv();
        }));
        assert!(!one_at_a_time(&RUNNING, "test-scan", || unreachable!("one at a time")));
        go.send(()).unwrap();
        wait("the first to end", &|| !RUNNING.load(Ordering::Acquire));
        // One that panics clears the flag as it unwinds.
        assert!(one_at_a_time(&RUNNING, "test-scan", || panic!("a walk that fails (expected in this test)")));
        wait("the flag after a panic", &|| !RUNNING.load(Ordering::Acquire));
        assert!(one_at_a_time(&RUNNING, "test-scan", || {
            DONE.fetch_add(1, Ordering::SeqCst);
        }));
        wait("the next scan", &|| DONE.load(Ordering::SeqCst) == 1);
    }

    /// Set ORBITERS_PICTURE_BENCH to a disposable folder containing representative PNGs.
    #[test]
    #[ignore]
    fn picture_folder_benchmark() {
        let root = PathBuf::from(std::env::var("ORBITERS_PICTURE_BENCH").expect("benchmark folder"));
        let mut index = visited::ShotIndex::default();
        let start = std::time::Instant::now();
        index.refresh(&[root.clone()]);
        println!("listing: {:?}, {} pictures", start.elapsed(), index.every_shot().len());
        let saved = root.join("benchmark-index.json");
        index.save(&saved).unwrap();
        let start = std::time::Instant::now();
        let index = visited::ShotIndex::load(&saved);
        picture_cache::set_index(&index);
        println!("cached listing: {:?}", start.elapsed());
        let pictures: Vec<_> = index.every_shot().into_iter().take(8)
            .map(|s| Picture::Shot { path: s.path, size: s.size, width: 560 }).collect();
        for pass in 0..2 {
            let start = std::time::Instant::now();
            for p in &pictures { assert!(decode_local(p).is_some()); }
            println!("thumbnails pass {pass}: {:?} for {}", start.elapsed(), pictures.len());
        }
    }

    #[test]
    fn vrchat_names_say_their_picture_and_layer() {
        let base = "VRChat_2026-10-29_21-04-11.532_3840x2160";
        assert_eq!(shot_name(&format!("{base}.png")), Some((base, None)));
        assert_eq!(shot_name(&format!("{base}_Player.png")), Some((base, Some("Player"))));
        assert_eq!(shot_name(&format!("{base}_Environment.PNG")), Some((base, Some("Environment"))));
        assert_eq!(shot_name(&format!("{base}_UI.png")), Some((base, Some("UI"))));
        // VRCX's copy with the world in its name is the picture; a layer it renamed so too is still the layer.
        assert_eq!(shot_name(&format!("{base}_wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd.png")), Some((base, None)));
        assert_eq!(shot_name(&format!("{base}_Player_wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd.png")), Some((base, Some("Player"))));
        // Not layers: a numbered copy, VRChat's old names (size first), other files, a broken time.
        assert_eq!(shot_name(&format!("{base}_2.png")), None);
        assert_eq!(shot_name("VRChat_1920x1080_2021-04-10_03-32-22.312.png"), None);
        assert_eq!(shot_name("holiday.png"), None);
        assert_eq!(shot_name("VRChat_2026-10-29_21-04-1x.532_3840x2160_Player.png"), None);
        assert_eq!(shot_name(&format!("{base}.jpg")), None);
    }

    #[test]
    fn a_multi_layer_picture_is_one_shot_with_its_layers_back_to_front() {
        let dir = "C:/Pictures/VRChat/2026-10/";
        let base = "VRChat_2026-10-29_21-04-11.532_1920x1080";
        // Newest first, as the index lists them: the layers in any order, their world on the picture only.
        let shots = vec![
            shot(&format!("{dir}{base}_UI.png"), None, 100),
            shot(&format!("{dir}{base}.png"), Some("wrld_a"), 100),
            shot(&format!("{dir}{base}_Player.png"), None, 100),
            shot(&format!("{dir}{base}_Environment.png"), None, 100),
            shot(&format!("{dir}VRChat_2026-10-29_21-00-00.001_1920x1080.png"), Some("wrld_a"), 90),
        ];
        let grouped = group_layers(shots);
        assert_eq!(
            names(&grouped),
            vec![
                (format!("{base}.png"), vec!["Environment".to_string(), "Player".into(), "UI".into()]),
                ("VRChat_2026-10-29_21-00-00.001_1920x1080.png".into(), vec![]),
            ]
        );
        let (face, layers) = &grouped[0];
        assert_eq!(face.info.world_id.as_deref(), Some("wrld_a"));
        assert!(!layers[0].transparent() && layers[1].transparent() && layers[2].transparent());
        assert_eq!((layers[1].width, layers[1].height), (1920, 1080));
    }

    #[test]
    fn partial_sets_keep_what_is_there() {
        let base = "C:/P/VRChat_2026-10-29_21-04-11.532_1920x1080";
        // The picture and its Player layer only.
        let grouped = group_layers(vec![shot(&format!("{base}.png"), Some("wrld_a"), 5), shot(&format!("{base}_Player.png"), Some("wrld_a"), 5)]);
        assert_eq!(names(&grouped), vec![("VRChat_2026-10-29_21-04-11.532_1920x1080.png".to_string(), vec!["Player".to_string()])]);
        // The picture deleted: the world layer is the shot, both layers listed, the world taken from the one that has it.
        let grouped = group_layers(vec![shot(&format!("{base}_Player.png"), Some("wrld_b"), 5), shot(&format!("{base}_Environment.png"), None, 5)]);
        assert_eq!(names(&grouped), vec![("VRChat_2026-10-29_21-04-11.532_1920x1080_Environment.png".to_string(), vec!["Environment".to_string(), "Player".into()])]);
        assert_eq!(grouped[0].0.info.world_id.as_deref(), Some("wrld_b"));
    }

    #[test]
    fn a_lone_layer_is_a_shot_of_its_own_and_still_a_layer() {
        let path = "C:/P/VRChat_2026-10-29_21-04-11.532_1920x1080_Player.png";
        let grouped = group_layers(vec![shot(path, Some("wrld_a"), 5)]);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].0.path, PathBuf::from(path));
        assert_eq!(grouped[0].1.iter().map(|l| l.path.clone()).collect::<Vec<_>>(), vec![PathBuf::from(path)]);
    }

    #[test]
    fn the_same_time_and_size_groups_across_archive_folders() {
        let shots = vec![
            // Another millisecond, another picture.
            shot("C:/P/VRChat_2026-10-29_21-04-11.533_1920x1080.png", Some("wrld_a"), 5),
            shot("C:/P/VRChat_2026-10-29_21-04-11.532_1920x1080.png", Some("wrld_a"), 5),
            shot("C:/P/VRChat_2026-10-29_21-04-11.532_1920x1080_Player.png", None, 5),
            // Layers moved to another archive folder still belong to the picture.
            shot("C:/Q/VRChat_2026-10-29_21-04-11.532_1920x1080_UI.png", None, 5),
            // Another size is another shot.
            shot("C:/P/VRChat_2026-10-29_21-04-11.532_3840x2160_UI.png", None, 5),
            // VRCX's renamed copy beside VRChat's own: two pictures, the layers go to the first.
            shot("C:/P/VRChat_2026-10-29_21-04-11.532_1920x1080_wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd.png", Some("wrld_a"), 5),
            shot("C:/R/holiday.png", None, 5),
        ];
        let grouped = group_layers(shots);
        assert_eq!(
            names(&grouped),
            vec![
                ("VRChat_2026-10-29_21-04-11.533_1920x1080.png".to_string(), vec![]),
                ("VRChat_2026-10-29_21-04-11.532_1920x1080.png".into(), vec!["Player".into(), "UI".into()]),
                ("VRChat_2026-10-29_21-04-11.532_3840x2160_UI.png".into(), vec!["UI".into()]),
                ("VRChat_2026-10-29_21-04-11.532_1920x1080_wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd.png".into(), vec![]),
                ("holiday.png".into(), vec![]),
            ]
        );
    }

    #[test]
    fn a_layer_keeps_its_transparency_when_shrunk() {
        // Half transparent red beside clear black: shrunk premultiplied, the red stays red (not darkened by the black).
        let rgba = [255, 0, 0, 255, 0, 0, 0, 0, 255, 0, 0, 255, 0, 0, 0, 0];
        let (w, h, out) = downscale_premultiplied(&rgba, 2, 2, 1);
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, [127, 0, 0, 127]);
        let image = ColorImage::from_rgba_premultiplied([1, 1], &out);
        assert_eq!(image.pixels[0].to_srgba_unmultiplied()[0], 255);
    }

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

    /// Night PAGES-UI-4: a picture that could not be read is read again when asked after its wait (doubling), not
    /// before, without showing as loading; one nobody asks for any more is forgotten.
    #[test]
    fn a_picture_that_failed_is_read_again_after_a_wait() {
        let ctx = egui::Context::default();
        let textures = crate::overlay::PaintTextures::of(&ctx);
        let mut pictures = Pictures::new(&ctx);
        let picture = Picture::Session(PathBuf::from("Z:/orbiters-test/no-such-session-retry"));
        let slot = |p: &Pictures| match p.slots.get(&picture).map(|k| &k.slot) {
            Some(Slot::Missing { tries, .. }) => format!("missing {tries}"),
            Some(Slot::Loading { tries }) => format!("loading {tries}"),
            Some(Slot::Ready(_)) => "ready".into(),
            None => "none".into(),
        };
        let settle = |p: &mut Pictures, now: f64, want: &str| {
            let until = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                p.get_at(&textures, now, &picture);
                if slot(p) == want {
                    return;
                }
                assert!(std::time::Instant::now() < until, "{want}: {}", slot(p));
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        settle(&mut pictures, 0.0, "missing 1");
        pictures.get_at(&textures, 20.0, &picture);
        assert_eq!(slot(&pictures), "missing 1", "not before its wait (30 s for a file here)");
        pictures.get_at(&textures, 30.5, &picture);
        assert_eq!(slot(&pictures), "loading 1", "read again once asked after it");
        assert!(!pictures.loading(), "a retry does not show as loading");
        settle(&mut pictures, 30.5, "missing 2");
        pictures.get_at(&textures, 80.0, &picture);
        assert_eq!(slot(&pictures), "missing 2", "the wait doubled");
        pictures.get_at(&textures, 91.0, &picture);
        assert_eq!(slot(&pictures), "loading 2");
        settle(&mut pictures, 91.0, "missing 3");
        // Nobody asks for it any more (a frame passed without it): forgotten at a later trim.
        pictures.get_at(&textures, 199.0, &Picture::Session(PathBuf::from("Z:/orbiters-test/another")));
        pictures.get_at(&textures, 200.0, &Picture::Session(PathBuf::from("Z:/orbiters-test/another")));
        assert_eq!(slot(&pictures), "none");
    }

    /// Night PAGES-UI-5: a big window shows more than [`KEEP_BYTES`] of thumbnails; it stands still five seconds (no frame), and
    /// the next frame's first ask trimmed the others it was about to ask for (all "older than a second"): they flashed their
    /// shimmer and were decoded again. What the frame before asked for stays; what no frame asks for any more still goes.
    #[test]
    fn a_trim_after_the_window_stood_still_keeps_the_pictures_on_screen() {
        let ctx = egui::Context::default();
        let textures = crate::overlay::PaintTextures::of(&ctx);
        let mut pictures = Pictures::new(&ctx);
        // 80 thumbnails of 560 x 315 (0.7 MB each): 56 MB, over the 48 MB kept.
        let shown: Vec<Picture> = (0..80).map(|i| Picture::Session(PathBuf::from(format!("Z:/orbiters-test/shown-{i}")))).collect();
        for (i, p) in shown.iter().enumerate() {
            let texture = textures.load_texture(format!("test-{i}"), egui::ColorImage::filled([560, 315], egui::Color32::GRAY), TextureOptions::LINEAR);
            pictures.slots.insert(p.clone(), Kept { slot: Slot::Ready(texture), used: 0.0, interim: None });
        }
        let frame = |pictures: &mut Pictures, now: f64, which: &[Picture]| which.iter().filter(|p| pictures.get_at(&textures, now, p).is_some()).count();
        assert_eq!(frame(&mut pictures, 10.0, &shown), 80);
        assert_eq!(frame(&mut pictures, 15.0, &shown), 80, "every picture on screen is still there after the pause");
        // Scrolled away from the first 60: they go, down to the budget, at the next trims.
        let rest = &shown[60..];
        frame(&mut pictures, 20.0, rest);
        frame(&mut pictures, 25.0, rest);
        let total: usize = pictures.slots.values().map(|k| if let Slot::Ready(t) = &k.slot { t.byte_size() } else { 0 }).sum();
        assert!(total <= KEEP_BYTES, "{total} bytes kept");
        assert_eq!(frame(&mut pictures, 25.0, rest), 20);
    }

    /// Every picture window started three loader threads that its first frame threw away (the window makes its set
    /// again once it knows whose pictures it shows). A set starts its loaders at the first picture asked for.
    #[test]
    fn pictures_start_their_loaders_at_the_first_picture_asked_for() {
        let ctx = egui::Context::default();
        let mut pictures = Pictures::new(&ctx);
        assert!(!pictures.started(), "made, nothing asked: no thread");
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            // A session folder that is not there: decoded to nothing, on a loader.
            assert!(pictures.get(ui.ctx(), &Picture::Session(PathBuf::from("Z:/orbiters-test/no-such-session"))).is_none());
        });
        out.textures_delta.clear();
        assert!(pictures.started());
        let until = std::time::Instant::now() + Duration::from_secs(10);
        while pictures.loading() {
            assert!(std::time::Instant::now() < until, "the loader answers");
            std::thread::sleep(Duration::from_millis(5));
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                pictures.get(ui.ctx(), &Picture::Session(PathBuf::from("Z:/orbiters-test/no-such-session")));
            });
            out.textures_delta.clear();
        }
    }
}
