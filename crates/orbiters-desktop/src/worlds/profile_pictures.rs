//! VRChat people's pictures kept on this computer (REQUEST17: "cache them so it doesn't take ages to load them every
//! time I open Orbiters N4N0"), for every page that shows one through [`super::Picture::Profile`]: the friends list and
//! its boops and invite requests, the account's own profile card, a person's profile, Sharing's friend chips.
//!
//! - **Kept small, one per person.** A picture is kept at most [`SIDE`] pixels on its longer side (the largest place one
//!   shows is the profile's 104-point picture), as PNG, in `<cache>/profile-pictures/`: `u-<usr id>.pic` for a person's
//!   picture (one file each: a new picture replaces the old one), `k-<digest>.pic` for one that is nobody's. Each file
//!   names the picture it holds (the VRChat file and version, or a digest of another address; never the address).
//!   A photo-like picture is about 20-35 KB: 224 friends cost about 5 to 8 MB. The folder is capped at [`CAP_BYTES`]: past it, the files used longest ago go.
//! - **Shown at once.** A picture kept is read and decoded on a loader thread, without the network. A person whose
//!   picture changed (another file or version in VRChat's address) shows the picture kept until the new one arrived.
//! - **Completed from VRCX and VRCNext** ([`orbiters_core::picture_sources`]), when they keep exactly that picture,
//!   before anything is downloaded.
//! - **Asked of VRChat politely** ([`Fetcher`]): through the service's signed-in session (paced there as every VRChat
//!   call), at most [`FETCHERS`] at a time, what is on screen first, the rest of the list in the background one every
//!   [`BACKGROUND_EVERY`], waiting longer after each failure ([`backoff`]), and never what is kept and unchanged.
//!
//! Picture addresses are never written to a log. What happened to each person's picture is ([`note`]): a short digest
//! of their id, the kind of address and the outcome, so the next session's log says why a missing picture is missing.

use egui::ColorImage;
use image::RgbaImage;
use orbiters_core::picture_sources::OtherCaches;
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// The longer side a picture is kept at, in pixels.
pub const SIDE: u32 = 128;
/// The kept pictures' folder at most (about 1 500 people).
pub const CAP_BYTES: u64 = 32 << 20;
/// Pictures asked of VRChat at once.
pub const FETCHERS: usize = 2;
/// A picture asked for by a page within this long is on screen: asked before the background ones.
pub const VISIBLE: Duration = Duration::from_secs(2);
/// The background asks one picture this often.
pub const BACKGROUND_EVERY: Duration = Duration::from_secs(2);
/// After a failure nothing is asked for this long, doubling with each failure in a row, at most five minutes.
const FIRST_BACKOFF: Duration = Duration::from_secs(5);
const LONGEST_BACKOFF: Duration = Duration::from_secs(300);
/// A background picture that failed this often is not asked again in this run (a page showing it still asks).
const BACKGROUND_TRIES: u8 = 3;
/// The folder is measured at most this often.
const EVICT_EVERY: Duration = Duration::from_secs(20);
const MAGIC: &[u8; 8] = b"ORBPIC1\n";
/// The mark a folder gets once the decoration previews it kept before REQUEST20 were swept ([`Store::sweep_previews`]).
const SWEPT: &str = ".previews-swept-1";

/// A line in `logs/profile-pictures.log` (kept under 1 MB, the one before renamed `.old`): what happened to a person's
/// picture. Only [`who`], the kind of address ([`kind`]), the outcome and VRChat's answer: never an address (VRChat's
/// carry signatures), a name or a picture. In tests, the test build's own folder ([`orbiters_core::paths::logs_dir`]).
pub fn note(line: &str) {
    use std::io::Write;
    static LOCK: Mutex<()> = Mutex::new(());
    let _one = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = orbiters_core::paths::logs_dir();
    let path = dir.join("profile-pictures.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::rename(&path, dir.join("profile-pictures.old.log"));
    }
    if let Ok(mut f) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::OpenOptions::new().create(true).append(true).open(&path)) {
        let _ = writeln!(f, "{} {line}", orbiters_core::now_utc_iso());
    }
}

/// Whose picture, for [`note`]: `u:` and 8 characters of a digest of the person's id (`k` for a picture that is nobody's).
pub fn who(user: Option<&str>) -> String {
    user.map_or_else(|| "k".to_string(), |u| format!("u:{}", &digest(u)[..8]))
}

/// The kind of an address, for [`note`]: `file` (a VRChat file and version) or `other`.
pub fn kind(url: &str) -> &'static str {
    if orbiters_core::picture_sources::vrchat_file(url).is_some() { "file" } else { "other" }
}

/// Why a picture could not be had, for [`note`]: the answer's first line without any address, short.
fn reason(error: &str) -> String {
    let line = error.lines().next().unwrap_or_default();
    let cut = line.find("http").map_or(line, |i| &line[..i]);
    let short: String = cut.chars().take(120).collect();
    if short.trim().is_empty() { "no reason given".into() } else { short.trim().to_string() } // i18n: not shown (a log line)
}

/// What names a picture: its VRChat file and version (the same picture at any size), or a digest of another address.
pub fn picture_key(url: &str) -> String {
    match orbiters_core::picture_sources::vrchat_file(url) {
        Some((file, version)) => format!("{file}/{version}"),
        None => format!("url-{}", digest(url)),
    }
}

fn digest(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes()).iter().take(12).map(|b| format!("{b:02x}")).collect()
}

/// A picture's bytes (PNG, JPEG or WebP) at most [`SIDE`] pixels on its longer side; None when they are not a whole
/// picture (cut short, another kind of file).
pub fn small(bytes: &[u8]) -> Option<RgbaImage> {
    // A JPEG cut short decodes to a grey bottom without an error: one without its end marker is not taken.
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let end = bytes.iter().rposition(|b| !matches!(b, 0 | b' ' | b'\t' | b'\n' | b'\r')).map_or(0, |i| i + 1);
        if !bytes[..end].ends_with(&[0xFF, 0xD9]) {
            return None;
        }
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(512 << 20);
    reader.limits(limits);
    let picture = reader.decode().ok()?;
    if picture.width() == 0 || picture.height() == 0 {
        return None;
    }
    Some(if picture.width() > SIDE || picture.height() > SIDE { picture.thumbnail(SIDE, SIDE) } else { picture }.to_rgba8())
}

fn color(image: &RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw())
}

/// What is kept for a person's picture.
pub enum Kept {
    /// That picture.
    Exact(RgbaImage),
    /// The person's picture before (shown until the new one arrived).
    Older(RgbaImage),
    Nothing,
}

/// The kept pictures' folder.
pub struct Store {
    dir: PathBuf,
    cap: u64,
    /// The files marked used in this run (once each: a file is not written to at every read).
    touched: Mutex<HashSet<PathBuf>>,
    measured: Mutex<Option<Instant>>,
}

impl Store {
    pub fn at(dir: PathBuf, cap: u64) -> Self {
        Self { dir, cap, touched: Mutex::default(), measured: Mutex::default() }
    }

    fn file(&self, user: Option<&str>, key: &str) -> PathBuf {
        match user.filter(|u| orbiters_core::vrchat_accounts::is_user_id(u)) {
            Some(user) => self.dir.join(format!("u-{user}.pic")),
            None => self.dir.join(format!("k-{}.pic", digest(key))),
        }
    }

    /// The name of the picture in a kept file, and (`whole`) its PNG.
    fn read(path: &Path, whole: bool) -> Option<(String, Vec<u8>)> {
        let mut file = std::fs::File::open(path).ok()?;
        let mut head = [0u8; 10];
        file.read_exact(&mut head).ok()?;
        if &head[..8] != MAGIC {
            return None;
        }
        let mut key = vec![0u8; u16::from_le_bytes([head[8], head[9]]) as usize];
        file.read_exact(&mut key).ok()?;
        let mut png = Vec::new();
        if whole {
            file.take(8 << 20).read_to_end(&mut png).ok()?;
        }
        Some((String::from_utf8(key).ok()?, png))
    }

    /// The picture at `url` for `user` (a `usr_…` id, or none), as kept.
    pub fn get(&self, user: Option<&str>, url: &str) -> Kept {
        let key = picture_key(url);
        let path = self.file(user, &key);
        let Some((kept, png)) = Self::read(&path, true) else { return Kept::Nothing };
        let Some(image) = image::load_from_memory_with_format(&png, image::ImageFormat::Png).ok().map(|i| i.to_rgba8()) else {
            // Our own file, damaged: made again.
            let _ = std::fs::remove_file(&path);
            return Kept::Nothing;
        };
        if kept == key {
            self.touch(&path);
            Kept::Exact(image)
        } else {
            Kept::Older(image)
        }
    }

    /// Whether exactly that picture is kept (its name only is read).
    pub fn has(&self, user: Option<&str>, url: &str) -> bool {
        let key = picture_key(url);
        Self::read(&self.file(user, &key), false).is_some_and(|(kept, _)| kept == key)
    }

    /// Keeps `image` as the picture at `url` for `user`, replacing what was kept for them.
    pub fn put(&self, user: Option<&str>, url: &str, image: &RgbaImage) -> bool {
        let key = picture_key(url);
        let mut png = Vec::with_capacity(16 << 10);
        if image::DynamicImage::ImageRgba8(image.clone()).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).is_err() {
            return false;
        }
        let mut bytes = Vec::with_capacity(png.len() + key.len() + 10);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(key.len() as u16).to_le_bytes());
        bytes.extend_from_slice(key.as_bytes());
        bytes.extend_from_slice(&png);
        let path = self.file(user, &key);
        let written = std::fs::create_dir_all(&self.dir).is_ok() && orbiters_core::write_atomic(&path, &bytes).is_ok();
        if written {
            self.touched.lock().unwrap_or_else(|e| e.into_inner()).insert(path);
            let due = {
                let mut measured = self.measured.lock().unwrap_or_else(|e| e.into_inner());
                let due = measured.is_none_or(|at| at.elapsed() >= EVICT_EVERY);
                if due {
                    *measured = Some(Instant::now());
                }
                due
            };
            if due {
                self.evict();
            }
        }
        written
    }

    /// Once per folder (it is marked after): the pictures kept for nobody (`k-` files, never a person's `u-` file) with
    /// see-through pixels go. Before REQUEST20 the friends list drew VRChat's shop PREVIEW of each icon frame and nameplate
    /// (the decoration around a grey stand-in person on a dark disc, or with grey stand-in lines) and kept it here as a
    /// picture of nobody; a decoration is drawn with its own asset since, so those files would only wait for the cap. A
    /// see-through picture of nobody is that shape (a photo is opaque); one that was something else is asked again when
    /// shown. Our folder only: VRCX's and VRCNext's are never written. Returns how many went.
    pub fn sweep_previews(&self) -> usize {
        let mark = self.dir.join(SWEPT);
        if mark.exists() {
            return 0;
        }
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return 0 };
        let mut gone = 0;
        for e in entries.flatten() {
            let path = e.path();
            let nobodys = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("k-") && n.ends_with(".pic"));
            if !nobodys {
                continue;
            }
            let see_through = Self::read(&path, true)
                .and_then(|(_, png)| image::load_from_memory_with_format(&png, image::ImageFormat::Png).ok())
                .is_some_and(|i| i.to_rgba8().pixels().any(|p| p.0[3] < 255));
            if see_through && std::fs::remove_file(&path).is_ok() {
                gone += 1;
            }
        }
        if std::fs::create_dir_all(&self.dir).is_ok() {
            let _ = std::fs::write(&mark, b"");
        }
        gone
    }

    /// A kept picture shown now is used: it goes last when the folder is too large.
    fn touch(&self, path: &Path) {
        if self.touched.lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_path_buf()) {
            if let Ok(file) = std::fs::File::options().write(true).open(path) {
                let _ = file.set_modified(SystemTime::now());
            }
        }
    }

    /// Past [`Store::cap`], the files used longest ago go until a quarter is free (and leftovers of an interrupted
    /// write older than an hour).
    pub fn evict(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return };
        let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        for e in entries.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let path = e.path();
            if path.extension().is_some_and(|x| x == "tmp") {
                if SystemTime::now().duration_since(modified).unwrap_or_default() > Duration::from_secs(3600) {
                    let _ = std::fs::remove_file(&path);
                }
                continue;
            }
            if path.extension().is_some_and(|x| x == "pic") {
                files.push((modified, meta.len(), path));
            }
        }
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        if total <= self.cap {
            return;
        }
        files.sort_by_key(|f| f.0);
        for (_, size, path) in files {
            if total <= self.cap / 4 * 3 {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total -= size;
                self.touched.lock().unwrap_or_else(|e| e.into_inner()).remove(&path);
            }
        }
    }
}

/// How a picture is asked of VRChat (the service, with the page's session): its bytes, or why not.
pub type Fetch = Arc<dyn Fn(&str) -> Result<Vec<u8>, String> + Send + Sync>;
/// Where a picture asked for goes: decoded, or None when it could not be had now.
pub type Reply = Box<dyn FnOnce(Option<ColorImage>) + Send>;

struct Job {
    /// The picture and the person: one job each.
    id: String,
    url: String,
    user: Option<String>,
    via: Fetch,
    /// When a page last asked for it (None: only the background wants it).
    asked: Option<Instant>,
    order: u64,
    replies: Vec<Reply>,
    tries: u8,
}

fn job_id(url: &str, user: Option<&str>) -> String {
    format!("{}|{}", picture_key(url), user.unwrap_or_default())
}

/// After `failures` failures in a row, how long nothing is asked.
pub fn backoff(failures: u32) -> Duration {
    FIRST_BACKOFF.saturating_mul(1u32 << failures.saturating_sub(1).min(16)).min(LONGEST_BACKOFF)
}

/// The pictures to ask for, and how asking goes.
#[derive(Default)]
struct Line {
    jobs: Vec<Job>,
    order: u64,
    running: HashSet<String>,
    most_running: usize,
    blocked_until: Option<Instant>,
    failures: u32,
    last_background: Option<Instant>,
    /// Pictures VRChat sent that are not pictures: not asked again in this run.
    unreadable: HashSet<String>,
    stopped: bool,
    started: bool,
}

impl Line {
    /// The next picture to ask for at `now`: one on screen (the most recently asked), else, at most one every
    /// [`BACKGROUND_EVERY`], the first queued; or how long to wait (None: until something is queued).
    fn pick(&mut self, now: Instant) -> Result<Job, Option<Duration>> {
        if let Some(until) = self.blocked_until.filter(|u| *u > now) {
            return Err(Some(until - now));
        }
        let free = |j: &&Job| !self.running.contains(&j.id);
        let visible = self
            .jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| free(j) && j.asked.is_some_and(|a| now.saturating_duration_since(a) < VISIBLE))
            .max_by_key(|(_, j)| (j.asked, j.order))
            .map(|(i, _)| i);
        if let Some(i) = visible {
            return Ok(self.jobs.remove(i));
        }
        let Some(i) = self.jobs.iter().enumerate().filter(|(_, j)| free(j)).min_by_key(|(_, j)| j.order).map(|(i, _)| i) else {
            return Err(None);
        };
        if let Some(next) = self.last_background.map(|t| t + BACKGROUND_EVERY).filter(|n| *n > now) {
            return Err(Some(next - now));
        }
        self.last_background = Some(now);
        Ok(self.jobs.remove(i))
    }

    fn add(&mut self, url: &str, user: Option<&str>, via: Fetch, asked: Option<Instant>, reply: Option<Reply>) {
        let id = job_id(url, user);
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            if asked.is_some() {
                job.asked = asked;
                job.via = via;
            }
            job.replies.extend(reply);
            return;
        }
        self.order += 1;
        self.jobs.push(Job { id, url: url.to_string(), user: user.map(str::to_string), via, asked, order: self.order, replies: reply.into_iter().collect(), tries: 0 });
    }
}

enum Outcome {
    Got(RgbaImage),
    Unreadable,
    Failed,
}

/// Asks VRChat for the pictures nobody keeps: [`FETCHERS`] threads, started with the first picture asked.
pub struct Fetcher {
    line: Arc<(Mutex<Line>, Condvar)>,
    store: Arc<Store>,
}

impl Fetcher {
    pub fn new(store: Arc<Store>) -> Self {
        Self { line: Arc::default(), store }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Line> {
        self.line.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn start(&self, line: &mut Line) {
        if std::mem::replace(&mut line.started, true) {
            return;
        }
        for i in 0..FETCHERS {
            let (line, store) = (self.line.clone(), self.store.clone());
            let _ = std::thread::Builder::new().name(format!("profile-pictures-{i}")).spawn(move || work(&line, &store));
        }
    }

    /// A page shows this picture now: asked before the background's, `reply` called with it.
    pub fn ask(&self, url: &str, user: Option<&str>, via: Fetch, reply: Reply) {
        let mut line = self.lock();
        if line.unreadable.contains(&job_id(url, user)) {
            drop(line);
            reply(None);
            return;
        }
        line.add(url, user, via, Some(Instant::now()), Some(reply));
        self.start(&mut line);
        self.line.1.notify_all();
    }

    /// The page asked for it again (it is still on screen).
    pub fn bump(&self, url: &str, user: Option<&str>) {
        let id = job_id(url, user);
        let mut line = self.lock();
        if let Some(job) = line.jobs.iter_mut().find(|j| j.id == id) {
            job.asked = Some(Instant::now());
            self.line.1.notify_all();
        }
    }

    /// Kept for later: asked in the background, in the order queued.
    pub fn queue(&self, url: &str, user: Option<&str>, via: Fetch) {
        let id = job_id(url, user);
        let mut line = self.lock();
        if line.unreadable.contains(&id) || line.running.contains(&id) {
            return;
        }
        line.add(url, user, via, None, None);
        self.start(&mut line);
        self.line.1.notify_all();
    }

    /// VRChat may be asked again now (a new list or a session back after waiting): the wait after failures ends and the
    /// failures in a row are forgotten. Without it a picture asked after the session came back waited out a back-off of
    /// up to five minutes that the session's absence had built up.
    pub fn resume(&self) {
        let mut line = self.lock();
        line.blocked_until = None;
        line.failures = 0;
        self.line.1.notify_all();
    }

    /// The background's pictures (not asked by a page) leave: another list replaces them.
    pub fn clear_background(&self) {
        self.lock().jobs.retain(|j| j.asked.is_some() || !j.replies.is_empty());
    }

    /// The most pictures asked at once so far (tests).
    #[cfg(test)]
    fn most_running(&self) -> usize {
        self.lock().most_running
    }
}

impl Drop for Fetcher {
    fn drop(&mut self) {
        self.lock().stopped = true;
        self.line.1.notify_all();
    }
}

fn work(line: &(Mutex<Line>, Condvar), store: &Store) {
    let (lock, ready) = line;
    loop {
        let job = {
            let mut l = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if l.stopped {
                    return;
                }
                match l.pick(Instant::now()) {
                    Ok(job) => {
                        l.running.insert(job.id.clone());
                        l.most_running = l.most_running.max(l.running.len());
                        break job;
                    }
                    Err(Some(wait)) => l = ready.wait_timeout(l, wait).unwrap_or_else(|e| e.into_inner()).0,
                    Err(None) => l = ready.wait(l).unwrap_or_else(|e| e.into_inner()),
                }
            }
        };
        // Kept meanwhile (another page, VRCX's copy): no network.
        let (whose, what) = (who(job.user.as_deref()), kind(&job.url));
        let outcome = match store.get(job.user.as_deref(), &job.url) {
            Kept::Exact(image) => Outcome::Got(image),
            _ => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (job.via)(&job.url))) {
                Ok(Ok(bytes)) => match small(&bytes) {
                    Some(image) => {
                        store.put(job.user.as_deref(), &job.url, &image);
                        note(&format!("fetched {whose} {what}")); // i18n: not shown (a log line)
                        Outcome::Got(image)
                    }
                    None => {
                        note(&format!("not a picture {whose} {what}: {} bytes, not asked again in this run", bytes.len())); // i18n: not shown (a log line)
                        Outcome::Unreadable
                    }
                },
                Ok(Err(e)) => {
                    note(&format!("failed {whose} {what}: {}", reason(&e))); // i18n: not shown (a log line)
                    Outcome::Failed
                }
                Err(_) => {
                    note(&format!("failed {whose} {what}: the picture's decoder stopped")); // i18n: not shown (a log line)
                    Outcome::Failed
                }
            },
        };
        let Job { id, url, user, via, replies, tries, .. } = job;
        let image = {
            let mut l = lock.lock().unwrap_or_else(|e| e.into_inner());
            l.running.remove(&id);
            let image = match outcome {
                Outcome::Got(image) => {
                    l.failures = 0;
                    Some(color(&image))
                }
                Outcome::Unreadable => {
                    l.unreadable.insert(id);
                    None
                }
                Outcome::Failed => {
                    l.failures += 1;
                    let wait = backoff(l.failures);
                    l.blocked_until = Some(Instant::now() + wait);
                    if tries + 1 < BACKGROUND_TRIES {
                        l.add(&url, user.as_deref(), via, None, None);
                        if let Some(j) = l.jobs.iter_mut().find(|j| j.id == id) {
                            j.tries = tries + 1;
                        }
                    }
                    None
                }
            };
            ready.notify_all();
            image
        };
        for reply in replies {
            reply(image.clone());
        }
    }
}

/// The kept pictures, VRCX's and VRCNext's, and the asking.
pub struct Profiles {
    store: Arc<Store>,
    others: OtherCaches,
    fetcher: Fetcher,
    /// The background list now (a newer one stops the walk through an older one).
    warming: Arc<std::sync::atomic::AtomicU64>,
}

impl Profiles {
    pub fn new(store: Arc<Store>, others: OtherCaches) -> Arc<Self> {
        Arc::new(Self { fetcher: Fetcher::new(store.clone()), store, others, warming: Arc::default() })
    }

    /// This app's: in its cache folder (a test's own in tests), VRCX's and VRCNext's of this computer (none in tests).
    pub fn shared() -> &'static Arc<Profiles> {
        static SHARED: OnceLock<Arc<Profiles>> = OnceLock::new();
        SHARED.get_or_init(|| {
            let store = Store::at(orbiters_core::paths::cache_dir().join("profile-pictures"), CAP_BYTES);
            let swept = store.sweep_previews();
            if swept > 0 {
                note(&format!("swept {swept} decoration previews kept before (once)")); // i18n: not shown (a log line)
            }
            Self::new(Arc::new(store), OtherCaches::on_this_computer())
        })
    }

    /// The picture at `url` of `user`, on a loader thread: `Some` when kept (or in VRCX's or VRCNext's copy); else
    /// `interim` is handed the person's picture before (if one is kept) and `reply` gets the picture once asked of
    /// VRChat through `via` (None: it cannot be asked, `reply` is told at once).
    pub fn load(&self, url: &str, user: Option<&str>, via: Option<Fetch>, interim: impl FnOnce(ColorImage), reply: Reply) -> Option<ColorImage> {
        match self.store.get(user, url) {
            Kept::Exact(image) => return Some(color(&image)),
            Kept::Older(image) => interim(color(&image)),
            Kept::Nothing => {}
        }
        if let Some(image) = self.from_elsewhere(url, user) {
            return Some(color(&image));
        }
        match via {
            Some(via) => self.fetcher.ask(url, user, via, reply),
            None => reply(None),
        }
        None
    }

    /// VRCX's or VRCNext's copy of exactly that picture, kept here too.
    fn from_elsewhere(&self, url: &str, user: Option<&str>) -> Option<RgbaImage> {
        let (_, bytes) = self.others.find(url)?;
        let image = small(&bytes)?;
        self.store.put(user, url, &image);
        Some(image)
    }

    /// Keeps `image` as the picture at `url` of `user` (pictures of the page: a cache filled with made-up people).
    pub fn keep(&self, url: &str, user: Option<&str>, image: &RgbaImage) -> bool {
        self.store.put(user, url, image)
    }

    /// A page asked again for a picture being asked of VRChat: it is on screen.
    pub fn bump(&self, url: &str, user: Option<&str>) {
        self.fetcher.bump(url, user);
    }

    /// The pictures a page will show (the whole friends list, in its order): those not kept are taken from VRCX's and
    /// VRCNext's copies, the rest asked of VRChat in the background. On a thread of its own; a newer list replaces this.
    pub fn warm(self: &Arc<Self>, items: Vec<(String, Option<String>)>, via: Fetch) {
        use std::sync::atomic::Ordering;
        let generation = self.warming.fetch_add(1, Ordering::AcqRel) + 1;
        let me = self.clone();
        // A new list, or the session back: asked again at once.
        self.fetcher.resume();
        let _ = std::thread::Builder::new().name("profile-pictures-warm".into()).spawn(move || {
            me.fetcher.clear_background();
            let (mut kept, mut elsewhere, mut queued) = (0, 0, 0);
            let total = items.len();
            for (url, user) in items {
                if me.warming.load(Ordering::Acquire) != generation {
                    return;
                }
                if me.store.has(user.as_deref(), &url) {
                    kept += 1;
                    continue;
                }
                if me.from_elsewhere(&url, user.as_deref()).is_some() {
                    elsewhere += 1;
                    continue;
                }
                queued += 1;
                me.fetcher.queue(&url, user.as_deref(), via.clone());
            }
            note(&format!("list of {total} pictures: {kept} kept, {elsewhere} from VRCX or VRCNext, {queued} to ask of VRChat")); // i18n: not shown (a log line)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const A: &str = "usr_00000000-0000-4000-8000-00000000000a";
    const B: &str = "usr_00000000-0000-4000-8000-00000000000b";

    fn url(file: u32, version: u32) -> String {
        format!("https://api.vrchat.cloud/api/1/file/file_00000000-0000-4000-8000-{file:012}/{version}/file")
    }

    /// A made-up picture: one colour, `w` × `h`, encoded as `format`.
    fn picture(w: u32, h: u32, rgb: [u8; 3], format: image::ImageFormat) -> Vec<u8> {
        let image = image::RgbImage::from_pixel(w, h, image::Rgb(rgb));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image).write_to(&mut std::io::Cursor::new(&mut bytes), format).unwrap();
        bytes
    }

    fn colour_of(image: &ColorImage) -> [u8; 3] {
        let c = image.pixels[image.pixels.len() / 2];
        [c.r(), c.g(), c.b()]
    }

    fn near(a: [u8; 3], b: [u8; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 6)
    }

    /// A folder of the test's own under the test build's temporary folders, removed after it.
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let dir = orbiters_core::paths::cache_dir().join(format!("profile-pictures-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A fetch that counts its calls and answers `bytes` (or fails).
    fn counting(bytes: Option<Vec<u8>>) -> (Fetch, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        (Arc::new(move |_url: &str| {
            c.fetch_add(1, Ordering::SeqCst);
            bytes.clone().ok_or_else(|| "offline".to_string())
        }), calls)
    }

    fn reply_channel() -> (Reply, std::sync::mpsc::Receiver<Option<ColorImage>>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (Box::new(move |image: Option<ColorImage>| {
            let _ = tx.send(image);
        }), rx)
    }

    fn profiles(dir: &Path, others: OtherCaches) -> Arc<Profiles> {
        Profiles::new(Arc::new(Store::at(dir.join("profile-pictures"), CAP_BYTES)), others)
    }

    fn none() -> OtherCaches {
        OtherCaches::at(None, None)
    }

    #[test]
    fn a_picture_asked_once_shows_after_a_restart_without_the_network() {
        let temp = Temp::new();
        let first = profiles(&temp.0, none());
        let (via, calls) = counting(Some(picture(1024, 1024, [200, 40, 90], image::ImageFormat::Png)));
        let (reply, got) = reply_channel();
        assert!(first.load(&url(1, 1), Some(A), Some(via), |_| panic!("nothing kept before"), reply).is_none());
        let image = got.recv_timeout(Duration::from_secs(10)).unwrap().expect("asked of VRChat");
        assert_eq!(image.size, [SIDE as usize, SIDE as usize], "kept at the size shown, not the full file");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        drop(first);
        // The next start: the same folder, a fetch that must not be called.
        let again = profiles(&temp.0, none());
        let never: Fetch = Arc::new(|_: &str| panic!("a kept picture is not asked again"));
        let shown = again.load(&url(1, 1), Some(A), Some(never), |_| panic!("it is the same picture"), Box::new(|_: Option<ColorImage>| panic!("no reply: shown at once")));
        assert!(near(colour_of(&shown.expect("kept")), [200, 40, 90]));
        // The same picture at another size (VRChat's 256 thumbnail of that file and version) is the same picture.
        let thumb = url(1, 1).replace("/api/1/file/", "/api/1/image/").replace("/1/file", "/1/256");
        assert!(again.store.has(Some(A), &thumb));
        let size: u64 = std::fs::read_dir(temp.0.join("profile-pictures")).unwrap().flatten().map(|e| e.metadata().unwrap().len()).sum();
        assert!(size < 16 << 10, "a one-colour picture is small on disk: {size}");
    }

    #[test]
    fn a_changed_picture_shows_the_old_one_until_the_new_one_arrived_and_replaces_it() {
        let temp = Temp::new();
        let p = profiles(&temp.0, none());
        assert!(p.store.put(Some(A), &url(1, 1), &image::RgbaImage::from_pixel(8, 8, image::Rgba([10, 200, 10, 255]))));
        let (via, calls) = counting(Some(picture(64, 64, [10, 10, 220], image::ImageFormat::Jpeg)));
        let (reply, got) = reply_channel();
        let mut interim = None;
        // A's picture is now another file: the old one shows meanwhile.
        assert!(p.load(&url(2, 1), Some(A), Some(via), |old| interim = Some(old), reply).is_none());
        assert!(near(colour_of(&interim.expect("the old picture meanwhile")), [10, 200, 10]));
        assert!(near(colour_of(&got.recv_timeout(Duration::from_secs(10)).unwrap().unwrap()), [10, 10, 220]));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(p.store.has(Some(A), &url(2, 1)) && !p.store.has(Some(A), &url(1, 1)), "replaced, one file a person");
        // A new version of the same file is a change too.
        assert!(matches!(p.store.get(Some(A), &url(2, 2)), Kept::Older(_)));
        // B's picture is B's own.
        assert!(matches!(p.store.get(Some(B), &url(2, 1)), Kept::Nothing));
    }

    #[test]
    fn past_the_cap_the_pictures_used_longest_ago_go() {
        let temp = Temp::new();
        let store = Store::at(temp.0.clone(), CAP_BYTES);
        let noise = |seed: u32| image::RgbaImage::from_fn(SIDE, SIDE, |x, y| {
            let v = (x * 7919 + y * 104_729 + seed * 13).wrapping_mul(2_654_435_761);
            image::Rgba([v as u8, (v >> 8) as u8, (v >> 16) as u8, 255])
        });
        let users: Vec<String> = (0..8).map(|n| format!("usr_00000000-0000-4000-8000-{n:012}")).collect();
        for (n, user) in users.iter().enumerate() {
            assert!(store.put(Some(user), &url(n as u32 + 1, 1), &noise(n as u32)));
            let file = store.file(Some(user), &picture_key(&url(n as u32 + 1, 1)));
            // Used n hours ago, newest last.
            std::fs::File::options().write(true).open(&file).unwrap().set_modified(SystemTime::now() - Duration::from_secs(3600 * (8 - n as u64))).unwrap();
        }
        let one = std::fs::metadata(store.file(Some(&users[0]), &picture_key(&url(1, 1)))).unwrap().len();
        let capped = Store::at(temp.0.clone(), one * 5);
        capped.evict();
        let kept: Vec<bool> = users.iter().enumerate().map(|(n, u)| capped.has(Some(u), &url(n as u32 + 1, 1))).collect();
        assert_eq!(kept.iter().filter(|k| **k).count(), 3, "down to three quarters of the cap: {kept:?}");
        assert_eq!(kept, [false, false, false, false, false, true, true, true], "the oldest went");
        // A kept picture shown is used now: it stays when the others go.
        let reread = Store::at(temp.0.clone(), one * 2);
        assert!(matches!(reread.get(Some(&users[5]), &url(6, 1)), Kept::Exact(_)));
        reread.evict();
        assert!(reread.has(Some(&users[5]), &url(6, 1)) && !reread.has(Some(&users[6]), &url(7, 1)));
    }

    /// A fake VRCX folder: `ImageCache/<file id>/<version>.png`, settled an hour ago.
    fn fake_vrcx(root: &Path, file: u32, version: u32, bytes: &[u8]) -> PathBuf {
        let images = root.join("VRCX").join("ImageCache");
        let path = images.join(format!("file_00000000-0000-4000-8000-{file:012}")).join(format!("{version}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(SystemTime::now() - Duration::from_secs(3600)).unwrap();
        images
    }

    #[test]
    fn vrcxs_copy_completes_ours_without_the_network_and_its_folder_is_left_as_it_was() {
        let temp = Temp::new();
        let images = fake_vrcx(&temp.0, 3, 2, &picture(256, 192, [250, 180, 20], image::ImageFormat::Jpeg));
        let before = std::fs::read(images.join("file_00000000-0000-4000-8000-000000000003").join("2.png")).unwrap();
        let p = profiles(&temp.0, OtherCaches::at(Some(images.clone()), None));
        let never: Fetch = Arc::new(|_: &str| panic!("VRCX had it"));
        let shown = p.load(&url(3, 2), Some(A), Some(never), |_| {}, Box::new(|_: Option<ColorImage>| panic!("shown at once"))).expect("from VRCX");
        assert_eq!(shown.size, [SIDE as usize, 96]);
        assert!(near(colour_of(&shown), [250, 180, 20]));
        assert!(p.store.has(Some(A), &url(3, 2)), "kept in ours");
        assert_eq!(std::fs::read(images.join("file_00000000-0000-4000-8000-000000000003").join("2.png")).unwrap(), before);
        assert_eq!(std::fs::read_dir(&images).unwrap().count(), 1, "nothing added to VRCX's folder");
    }

    #[test]
    fn vrcnexts_copy_completes_ours_through_its_table() {
        let temp = Temp::new();
        let root = temp.0.join("VRCNext");
        let picture_file = root.join("Caches").join("ImageCache").join("Users").join(format!("{A}.png"));
        std::fs::create_dir_all(picture_file.parent().unwrap()).unwrap();
        std::fs::write(&picture_file, picture(800, 800, [30, 160, 160], image::ImageFormat::Png)).unwrap();
        std::fs::File::options().write(true).open(&picture_file).unwrap().set_modified(SystemTime::now() - Duration::from_secs(3600)).unwrap();
        let stored = url(4, 3).replace("/api/1/file/", "/api/1/image/").replace("/3/file", "/3/800");
        orbiters_core::picture_sources::fake_vrcnext_table(&root.join("VRCNData.db"), &[(&format!("Users/{A}"), &stored)]);
        let p = profiles(&temp.0, OtherCaches::at(None, Some(root)));
        let never: Fetch = Arc::new(|_: &str| panic!("VRCNext had it"));
        let shown = p.load(&url(4, 3), Some(A), Some(never.clone()), |_| {}, Box::new(|_: Option<ColorImage>| panic!("shown at once"))).expect("from VRCNext");
        assert!(near(colour_of(&shown), [30, 160, 160]));
        // Another version is not VRCNext's picture: asked of VRChat.
        let (via, calls) = counting(None);
        let (reply, got) = reply_channel();
        assert!(p.load(&url(4, 4), Some(A), Some(via), |_| {}, reply).is_none());
        assert!(got.recv_timeout(Duration::from_secs(10)).unwrap().is_none(), "offline: none, no error");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn half_written_or_broken_copies_are_skipped_without_an_error() {
        let temp = Temp::new();
        let whole = picture(256, 256, [90, 90, 250], image::ImageFormat::Jpeg);
        let images = fake_vrcx(&temp.0, 5, 1, &whole[..whole.len() / 2]);
        fake_vrcx(&temp.0, 6, 1, b"not a picture at all");
        let png = picture(64, 64, [1, 2, 3], image::ImageFormat::Png);
        fake_vrcx(&temp.0, 7, 1, &png[..png.len() - 20]);
        // VRCNext: a file being written now (changed just now) and its table.
        let root = temp.0.join("VRCNext");
        let users = root.join("Caches").join("ImageCache").join("Users");
        std::fs::create_dir_all(&users).unwrap();
        std::fs::write(users.join(format!("{B}.png")), &png).unwrap();
        orbiters_core::picture_sources::fake_vrcnext_table(&root.join("VRCNData.db"), &[(&format!("Users/{B}"), &url(8, 1))]);
        let p = profiles(&temp.0, OtherCaches::at(Some(images), Some(root)));
        for n in [5, 6, 7, 8] {
            let (via, calls) = counting(Some(picture(32, 32, [5, 250, 5], image::ImageFormat::Png)));
            let (reply, got) = reply_channel();
            assert!(p.load(&url(n, 1), Some(A), Some(via), |_| {}, reply).is_none(), "{n}: not taken from the other app");
            assert!(near(colour_of(&got.recv_timeout(Duration::from_secs(10)).unwrap().expect("asked of VRChat instead")), [5, 250, 5]));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }

    fn job(id: &str, asked: Option<Instant>, order: u64) -> Job {
        Job { id: id.into(), url: String::new(), user: None, via: Arc::new(|_: &str| Err(String::new())), asked, order, replies: Vec::new(), tries: 0 }
    }

    #[test]
    fn what_is_on_screen_is_asked_first_and_the_rest_slowly() {
        let now = Instant::now();
        let mut line = Line::default();
        line.jobs = vec![
            job("background-1", None, 1),
            job("scrolled-past", Some(now - VISIBLE - Duration::from_millis(1)), 2),
            job("on-screen", Some(now - Duration::from_millis(500)), 3),
            job("on-screen-newest", Some(now - Duration::from_millis(100)), 4),
            job("background-2", None, 5),
        ];
        let order: Vec<String> = std::iter::from_fn(|| line.pick(now).ok()).map(|j| j.id).collect();
        // Two on screen, newest first, then one background picture: the next waits.
        assert_eq!(order, ["on-screen-newest", "on-screen", "background-1"]);
        assert_eq!(line.pick(now).err(), Some(Some(BACKGROUND_EVERY)));
        // A row coming on screen meanwhile goes at once.
        line.jobs.push(job("new-row", Some(now), 6));
        assert_eq!(line.pick(now).unwrap().id, "new-row");
        // Then the background's next, in its order (a row scrolled past is one of them).
        assert_eq!(line.pick(now + BACKGROUND_EVERY).unwrap().id, "scrolled-past");
        assert_eq!(line.pick(now + BACKGROUND_EVERY * 2).unwrap().id, "background-2");
        assert_eq!(line.pick(now + BACKGROUND_EVERY * 3).err(), Some(None));
        // After failures nothing is asked, waiting longer each time, at most five minutes.
        line.jobs.push(job("after", Some(now), 7));
        line.blocked_until = Some(now + backoff(1));
        assert_eq!(line.pick(now).err(), Some(Some(Duration::from_secs(5))));
        assert_eq!((backoff(2), backoff(3), backoff(30)), (Duration::from_secs(10), Duration::from_secs(20), Duration::from_secs(300)));
        // Still on screen when the wait is over (the page asked again): asked then.
        line.jobs.last_mut().unwrap().asked = Some(now + backoff(1));
        assert_eq!(line.pick(now + backoff(1)).unwrap().id, "after");
        // One being asked is not asked twice at once.
        line.running.insert("twice".into());
        line.jobs.push(job("twice", Some(now), 8));
        assert_eq!(line.pick(now + backoff(1)).err(), Some(None));
    }

    #[test]
    fn at_most_two_pictures_are_asked_at_once_and_a_failure_backs_off() {
        let temp = Temp::new();
        let fetcher = Fetcher::new(Arc::new(Store::at(temp.0.clone(), CAP_BYTES)));
        let (now, most) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (n, m) = (now.clone(), most.clone());
        let bytes = picture(16, 16, [1, 1, 1], image::ImageFormat::Png);
        let via: Fetch = Arc::new(move |_: &str| {
            let at = n.fetch_add(1, Ordering::SeqCst) + 1;
            m.fetch_max(at, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(40));
            n.fetch_sub(1, Ordering::SeqCst);
            Ok(bytes.clone())
        });
        let (tx, rx) = std::sync::mpsc::channel();
        for k in 0..8u32 {
            let tx = tx.clone();
            fetcher.ask(&url(10 + k, 1), None, via.clone(), Box::new(move |image: Option<ColorImage>| tx.send(image.is_some()).unwrap()));
        }
        for _ in 0..8 {
            assert!(rx.recv_timeout(Duration::from_secs(10)).unwrap());
        }
        assert_eq!(most.load(Ordering::SeqCst), FETCHERS, "as many as allowed, no more");
        assert_eq!(fetcher.most_running(), FETCHERS);
        // A failure: the next is not asked before the wait.
        let (failing, calls) = counting(None);
        let (reply, got) = reply_channel();
        fetcher.ask(&url(30, 1), None, failing.clone(), reply);
        assert!(got.recv_timeout(Duration::from_secs(10)).unwrap().is_none());
        let (reply, got) = reply_channel();
        fetcher.ask(&url(31, 1), None, failing, reply);
        assert!(got.recv_timeout(Duration::from_millis(1500)).is_err(), "waiting out the back-off");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_background_takes_the_other_apps_copies_and_queues_only_what_is_missing() {
        let temp = Temp::new();
        let images = fake_vrcx(&temp.0, 40, 1, &picture(64, 64, [200, 200, 0], image::ImageFormat::Png));
        let p = profiles(&temp.0, OtherCaches::at(Some(images), None));
        assert!(p.store.put(Some(B), &url(41, 1), &image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 1, 1, 255]))));
        let (via, calls) = counting(Some(picture(16, 16, [9, 9, 9], image::ImageFormat::Png)));
        p.warm(vec![(url(40, 1), Some(A.to_string())), (url(41, 1), Some(B.to_string())), (url(42, 1), None)], via);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !(p.store.has(None, &url(42, 1)) && p.store.has(Some(A), &url(40, 1))) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(p.store.has(Some(A), &url(40, 1)), "from VRCX");
        assert!(p.store.has(None, &url(42, 1)), "asked in the background");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "only the one nobody kept");
    }

    /// A made-up see-through picture: a ring of colour around a clear middle and clear corners (a decoration's shape).
    fn see_through(rgb: [u8; 3]) -> image::RgbaImage {
        image::RgbaImage::from_fn(32, 32, |x, y| {
            let d = ((x as f32 - 15.5).powi(2) + (y as f32 - 15.5).powi(2)).sqrt();
            if (11.0..15.0).contains(&d) { image::Rgba([rgb[0], rgb[1], rgb[2], 255]) } else { image::Rgba([0, 0, 0, 0]) }
        })
    }

    #[test]
    fn decoration_previews_kept_before_are_swept_once_and_peoples_pictures_stay() {
        let temp = Temp::new();
        let store = Store::at(temp.0.join("profile-pictures"), CAP_BYTES);
        let opaque = image::RgbaImage::from_pixel(8, 8, image::Rgba([40, 90, 160, 255]));
        // Before REQUEST20: a frame's shop preview and a nameplate's, kept for nobody; a person's see-through VRC+ icon;
        // a person's photo kept for nobody (Sharing's chips, a boop's other picture).
        assert!(store.put(None, &url(50, 1), &see_through([0, 220, 120])));
        assert!(store.put(None, &url(51, 1), &see_through([200, 40, 200])));
        assert!(store.put(Some(A), &url(52, 1), &see_through([250, 250, 250])));
        assert!(store.put(None, &url(53, 1), &opaque));
        assert_eq!(store.sweep_previews(), 2);
        assert!(!store.has(None, &url(50, 1)) && !store.has(None, &url(51, 1)), "the previews went");
        assert!(store.has(Some(A), &url(52, 1)), "a person's picture is never swept, see-through or not");
        assert!(store.has(None, &url(53, 1)), "a photo of nobody stays");
        // Once: a decoration kept from now on (its own asset, see-through) stays.
        assert!(store.put(None, &url(54, 1), &see_through([0, 0, 255])));
        assert_eq!(store.sweep_previews(), 0);
        assert!(store.has(None, &url(54, 1)));
        // An empty or missing folder: nothing to do, and no error.
        assert_eq!(Store::at(temp.0.join("never-made"), CAP_BYTES).sweep_previews(), 0);
    }

    #[test]
    fn after_the_session_waited_pictures_are_asked_again_at_once() {
        let temp = Temp::new();
        let p = profiles(&temp.0, none());
        // The session is waiting for Orbiters: every ask fails, the wait grows (5 s, then 10 s...).
        let waiting = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (w, calls) = (waiting.clone(), Arc::new(AtomicUsize::new(0)));
        let c = calls.clone();
        let bytes = picture(16, 16, [30, 160, 90], image::ImageFormat::Png);
        let via: Fetch = Arc::new(move |_: &str| {
            c.fetch_add(1, Ordering::SeqCst);
            if w.load(Ordering::SeqCst) { Err("Your VRChat session expired. Sign in again.".into()) } else { Ok(bytes.clone()) }
        });
        let (reply, got) = reply_channel();
        assert!(p.load(&url(60, 1), Some(A), Some(via.clone()), |_| {}, reply).is_none());
        assert!(got.recv_timeout(Duration::from_secs(10)).unwrap().is_none(), "failed while waiting");
        // The session is back: the page's next look still waits out the back-off...
        waiting.store(false, Ordering::SeqCst);
        let (reply, got) = reply_channel();
        assert!(p.load(&url(60, 1), Some(A), Some(via.clone()), |_| {}, reply).is_none());
        assert!(got.recv_timeout(Duration::from_millis(1500)).is_err(), "still in the back-off");
        // ...until the list of the session back is handed in: asked at once, and kept.
        p.warm(vec![(url(61, 1), Some(B.to_string()))], via.clone());
        let image = got.recv_timeout(Duration::from_secs(3)).expect("asked again at once").expect("the picture");
        assert!(near(colour_of(&image), [30, 160, 90]));
        assert!(p.store.has(Some(A), &url(60, 1)));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !p.store.has(Some(B), &url(61, 1)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(p.store.has(Some(B), &url(61, 1)), "the background goes on too");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn the_log_says_why_without_an_address_or_an_id() {
        let temp = Temp::new();
        let p = profiles(&temp.0, none());
        // A person of this test only (their digest is looked for in the test build's log).
        let person = format!("usr_00000000-0000-4000-8000-{:012}", std::process::id() as u64 * 1000 + 7);
        let refused: Fetch = Arc::new(|_: &str| Err("VRChat refused the request (HTTP 403: see https://files.vrchat.cloud/x?Signature=abc).".into()));
        let (reply, got) = reply_channel();
        p.load(&url(70, 1), Some(&person), Some(refused), |_| {}, reply);
        assert!(got.recv_timeout(Duration::from_secs(10)).unwrap().is_none());
        let log = std::fs::read_to_string(orbiters_core::paths::logs_dir().join("profile-pictures.log")).unwrap_or_default();
        let line = log.lines().find(|l| l.contains(&who(Some(&person)))).expect("a line about that person");
        assert!(line.contains("failed") && line.contains("HTTP 403") && line.contains(" file"), "{line}");
        assert!(!line.contains("http") && !line.contains("Signature") && !line.contains(&person) && !line.contains("file_"), "{line}");
        assert!(orbiters_core::paths::logs_dir().starts_with(std::env::temp_dir()), "the test build's own folder");
    }
}
