//! VRChat people's pictures kept on this computer (REQUEST17: "cache them so it doesn't take ages to load them every
//! time I open Orbiters N4N0"), for every page that shows one through [`super::Picture::Profile`]: the friends list and
//! its boops and invite requests, the account's own profile card, a person's profile, Sharing's friend chips.
//!
//! - **Kept by what they are.** Everything from VRChat's file hosts (faces, icons, groups' icons and banners) is kept in the
//!   account's own folder on this computer, `<cache>/vrchat/<account>/pictures/` (`orbiters_core::vrchat_store`), named by
//!   its VRChat file and version: another address of the same picture is the same file, never downloaded again. Beside what
//!   VRChat sent, a small copy ready to be shown (QOI: quick to decode) at the size it is drawn ([`Size`]). Where no VRChat
//!   account is (VRCX's or VRCNext's copy of the friends), `<cache>/vrchat-local-pictures/`. Past the size Settings allows
//!   ([`set_cap`]), the ones used longest ago go. Signing out removes the account's. Pictures kept before 0.3.10 (in
//!   `profile-pictures/`) move there as they are shown.
//! - **Shown at once.** A picture kept is in the very frame that asks for it ([`Profiles::now`]: a few a frame, all of the
//!   first screen in the first one), without the network; the others of a long scroll come from the loader threads. A person whose
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
use orbiters_contracts::qoi;
use orbiters_core::picture_sources::OtherCaches;
pub use orbiters_core::vrchat_store::{picture_key, Account, Kept, Size};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// The longer side a person's picture is kept at, in pixels.
pub const SIDE: u32 = 128;
/// What the kept pictures of an account take at most, unless Settings says otherwise.
pub const CAP_BYTES: u64 = 256 << 20;
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
/// The start of a picture kept before 0.3.10.
const MAGIC: &[u8; 8] = b"ORBPIC1\n";

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

fn digest(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes()).iter().take(12).map(|b| format!("{b:02x}")).collect()
}

/// A picture's bytes (PNG, JPEG or WebP) at most [`SIDE`] pixels on its longer side; None when they are not a whole
/// picture (cut short, another kind of file).
pub fn small(bytes: &[u8]) -> Option<RgbaImage> {
    smaller(bytes, Size::Face)
}

/// [`small`], at `size`.
pub fn smaller(bytes: &[u8], size: Size) -> Option<RgbaImage> {
    let side = size.pixels();
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
    Some(if picture.width() > side || picture.height() > side { picture.thumbnail(side, side) } else { picture }.to_rgba8())
}

fn color(image: &RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw())
}

/// What Settings allows the kept pictures ([`set_cap`]).
static CAP: AtomicU64 = AtomicU64::new(CAP_BYTES);

/// What the kept pictures may take, as Settings has it now.
pub fn set_cap(bytes: u64) {
    if CAP.swap(bytes, Ordering::Relaxed) != bytes {
        for (_, p) in shared().iter() {
            p.store.pictures.set_cap(bytes);
        }
    }
}

/// The kept pictures of an account (or of none).
pub struct Store {
    pictures: orbiters_core::vrchat_store::Pictures,
    /// Where they were kept before 0.3.10.
    before: Option<PathBuf>,
}

fn decode(body: &[u8]) -> Option<RgbaImage> {
    let (head, rgba) = qoi::decode_rgba_max(body, 4 << 20).ok()?;
    RgbaImage::from_raw(head.width, head.height, rgba)
}

impl Store {
    pub fn at(dir: PathBuf, cap: u64) -> Self {
        Self { pictures: orbiters_core::vrchat_store::Pictures::at(dir, cap), before: None }
    }

    /// The picture at `url` for `user` (a `usr_…` id, or none), as kept.
    pub fn get(&self, user: Option<&str>, url: &str, size: Size) -> Kept<RgbaImage> {
        self.pictures.small_copy(user, url, size, decode)
    }

    /// Whether exactly that picture is kept.
    pub fn has(&self, url: &str, size: Size) -> bool {
        self.pictures.has(url, size)
    }

    /// Keeps `image` as the picture at `url` for `user`, and what VRChat sent.
    pub fn put(&self, user: Option<&str>, url: &str, size: Size, image: &RgbaImage, sent: Option<&[u8]>) -> bool {
        self.pictures.put(user, url, size, &qoi::encode(image.as_raw(), image.width(), image.height(), 4), sent)
    }

    /// The picture made again from what VRChat sent, or as it was kept before 0.3.10.
    fn again(&self, user: Option<&str>, url: &str, size: Size) -> Option<RgbaImage> {
        // A wide copy is not made of a thumbnail's bytes (the same picture, asked at a face's size).
        let whole = self.pictures.original_copy(url).and_then(|b| smaller(&b, size)).filter(|i| size == Size::Face || i.width().max(i.height()) > 2 * SIDE);
        if let Some(image) = whole {
            self.put(user, url, size, &image, None);
            return Some(image);
        }
        if size != Size::Face {
            return None;
        }
        let dir = self.before.as_ref()?;
        let key = picture_key(url);
        let file = match user.filter(|u| orbiters_core::vrchat_accounts::is_user_id(u)) {
            Some(user) => dir.join(format!("u-{user}.pic")),
            None => dir.join(format!("k-{}.pic", digest(&key))),
        };
        let bytes = std::fs::read(&file).ok()?;
        let rest = bytes.strip_prefix(&MAGIC[..])?;
        let n = u16::from_le_bytes([*rest.first()?, *rest.get(1)?]) as usize;
        if rest.get(2..2 + n)? != key.as_bytes() {
            return None;
        }
        let image = image::load_from_memory_with_format(&rest[2 + n..], image::ImageFormat::Png).ok()?.to_rgba8();
        if self.put(user, url, size, &image, None) {
            let _ = std::fs::remove_file(file);
        }
        Some(image)
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
        let outcome = match store.get(job.user.as_deref(), &job.url, Size::Face) {
            Kept::Exact(image) => Outcome::Got(image),
            _ => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (job.via)(&job.url))) {
                Ok(Ok(bytes)) => match small(&bytes) {
                    Some(image) => {
                        store.put(job.user.as_deref(), &job.url, Size::Face, &image, Some(&bytes));
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

type Shared = Vec<(Option<Account>, Arc<Profiles>)>;

/// Those of the accounts asked about in this process.
fn shared() -> std::sync::MutexGuard<'static, Shared> {
    static SHARED: Mutex<Shared> = Mutex::new(Vec::new());
    SHARED.lock().unwrap_or_else(|e| e.into_inner())
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

    /// The pictures of `account` in this process (of none: this computer's own, of a list read from VRCX or VRCNext).
    pub fn of(account: Option<&Account>) -> Arc<Profiles> {
        let mut all = shared();
        if let Some(i) = all.iter().position(|(a, _)| a.as_ref() == account) {
            // The ones asked last are last.
            let found = all.remove(i);
            all.push(found);
            return all.last().unwrap().1.clone();
        }
        let cap = CAP.load(Ordering::Relaxed);
        let cache = orbiters_core::paths::cache_dir();
        let pictures = match account {
            Some(account) => orbiters_core::vrchat_store::Pictures::of(&orbiters_core::vrchat_store::root(), account, cap),
            None => orbiters_core::vrchat_store::Pictures::at(cache.join("vrchat-local-pictures"), cap),
        };
        let made = Self::new(Arc::new(Store { pictures, before: Some(cache.join("profile-pictures")) }), OtherCaches::on_this_computer());
        all.push((account.cloned(), made.clone()));
        // Another account's go from memory (its threads with them) once two more were there since.
        if all.len() > 3 {
            all.remove(0);
        }
        made
    }

    /// The picture at `url`, kept and ready: read here and now (a small file, a fraction of a millisecond), or the person's
    /// before it, or not there. The caller says how many a frame.
    pub fn now(&self, url: &str, user: Option<&str>, size: Size) -> Kept<ColorImage> {
        match self.store.get(user, url, size) {
            Kept::Exact(i) => Kept::Exact(color(&i)),
            Kept::Older(i) => Kept::Older(color(&i)),
            Kept::Nothing => Kept::Nothing,
        }
    }

    /// Whether the picture at `url` is kept at that size.
    pub fn has(&self, url: &str, size: Size) -> bool {
        self.store.has(url, size)
    }

    /// A picture VRChat sent (`bytes`, already made `image`), kept: another asker's than [`Fetcher`]'s.
    pub fn keep_sent(&self, url: &str, size: Size, image: &RgbaImage, bytes: &[u8]) -> bool {
        self.store.put(None, url, size, image, Some(bytes))
    }

    /// The picture at `url` of `user`, on a loader thread: `Some` when kept (or in VRCX's or VRCNext's copy); else
    /// `interim` is handed the person's picture before (if one is kept) and `reply` gets the picture once asked of
    /// VRChat through `via` (None: it cannot be asked, `reply` is told at once).
    pub fn load(&self, url: &str, user: Option<&str>, via: Option<Fetch>, interim: impl FnOnce(ColorImage), reply: Reply) -> Option<ColorImage> {
        let before = match self.store.get(user, url, Size::Face) {
            Kept::Exact(image) => return Some(color(&image)),
            Kept::Older(image) => Some(image),
            Kept::Nothing => None,
        };
        if let Some(image) = self.store.again(user, url, Size::Face) {
            return Some(color(&image));
        }
        if let Some(image) = before {
            interim(color(&image));
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
        self.store.put(user, url, Size::Face, &image, None);
        Some(image)
    }

    /// Keeps `image` as the picture at `url` of `user` (pictures of the page: a cache filled with made-up people).
    pub fn keep(&self, url: &str, user: Option<&str>, image: &RgbaImage) -> bool {
        self.store.put(user, url, Size::Face, image, None)
    }

    /// A page asked again for a picture being asked of VRChat: it is on screen.
    pub fn bump(&self, url: &str, user: Option<&str>) {
        self.fetcher.bump(url, user);
    }

    /// The pictures a page will show (the whole friends list, in its order): those not kept are taken from VRCX's and
    /// VRCNext's copies, the rest asked of VRChat in the background. On a thread of its own; a newer list replaces this.
    pub fn warm(self: &Arc<Self>, items: Vec<(String, Option<String>)>, via: Fetch) {
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
                if me.store.has(&url, Size::Face) || me.store.again(user.as_deref(), &url, Size::Face).is_some() {
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
    use std::sync::atomic::AtomicUsize;
    use std::time::SystemTime;

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
        assert!(again.store.has(&thumb, Size::Face));
        assert!(matches!(again.now(&thumb, Some(A), Size::Face), Kept::Exact(_)), "in the very frame that asks");
        let size: u64 = walk(&temp.0.join("profile-pictures")).iter().map(|f| f.metadata().unwrap().len()).sum();
        assert!(size < 32 << 10, "a one-colour picture is small on disk: {size}");
    }

    #[test]
    fn a_changed_picture_shows_the_old_one_until_the_new_one_arrived_and_replaces_it() {
        let temp = Temp::new();
        let p = profiles(&temp.0, none());
        assert!(p.store.put(Some(A), &url(1, 1), Size::Face, &image::RgbaImage::from_pixel(8, 8, image::Rgba([10, 200, 10, 255])), None));
        let (via, calls) = counting(Some(picture(64, 64, [10, 10, 220], image::ImageFormat::Jpeg)));
        let (reply, got) = reply_channel();
        let mut interim = None;
        // A's picture is now another file: the old one shows meanwhile.
        assert!(p.load(&url(2, 1), Some(A), Some(via), |old| interim = Some(old), reply).is_none());
        assert!(near(colour_of(&interim.expect("the old picture meanwhile")), [10, 200, 10]));
        assert!(near(colour_of(&got.recv_timeout(Duration::from_secs(10)).unwrap().unwrap()), [10, 10, 220]));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(p.store.has(&url(2, 1), Size::Face), "kept");
        // A new version of the same file is a change too.
        assert!(matches!(p.store.get(Some(A), &url(2, 2), Size::Face), Kept::Older(_)));
        // The very same picture of B is the same file; another of theirs never shows A's.
        assert!(matches!(p.store.get(Some(B), &url(2, 1), Size::Face), Kept::Exact(_)));
        assert!(matches!(p.store.get(Some(B), &url(2, 2), Size::Face), Kept::Nothing));
    }

    /// A picture whose small copy went (or is of another version's making) is made again from what VRChat sent; one kept
    /// before 0.3.10 moves here: neither is asked again.
    #[test]
    fn what_is_on_this_computer_is_not_asked_again() {
        let temp = Temp::new();
        let mut store = Store::at(temp.0.join("new"), CAP_BYTES);
        let before = temp.0.join("profile-pictures");
        store.before = Some(before.clone());
        let p = Profiles::new(Arc::new(store), none());
        let never = || -> (Fetch, Reply) { (Arc::new(|_: &str| panic!("not asked")), Box::new(|_: Option<ColorImage>| panic!("shown at once"))) };
        // As it was kept before: one file a person, and one for nobody's.
        std::fs::create_dir_all(&before).unwrap();
        for (name, n, rgb) in [(format!("u-{A}.pic"), 80, [200, 10, 10]), (format!("k-{}.pic", digest(&picture_key(&url(81, 1)))), 81, [10, 10, 200])] {
            let key = picture_key(&url(n, 1));
            let png = picture(32, 32, rgb, image::ImageFormat::Png);
            std::fs::write(before.join(name), [&MAGIC[..], &(key.len() as u16).to_le_bytes(), key.as_bytes(), &png].concat()).unwrap();
        }
        for (n, user, rgb) in [(80, Some(A), [200, 10, 10]), (81, None, [10, 10, 200])] {
            let (via, reply) = never();
            assert!(near(colour_of(&p.load(&url(n, 1), user, Some(via), |_| panic!("the same picture"), reply).expect("as it was kept")), rgb));
            assert!(matches!(p.now(&url(n, 1), user, Size::Face), Kept::Exact(_)), "kept here from then on, in the very frame");
        }
        assert_eq!(std::fs::read_dir(&before).unwrap().count(), 0, "moved");
        // From what was sent.
        let sent = picture(512, 256, [90, 200, 40], image::ImageFormat::Png);
        assert!(p.store.put(None, &url(82, 1), Size::Face, &small(&sent).unwrap(), Some(&sent)));
        let copies = || -> Vec<PathBuf> { walk(&temp.0.join("new")).into_iter().filter(|f| f.extension().is_some_and(|e| e == "s")).collect() };
        let there = copies().len();
        for f in copies() {
            std::fs::write(f, b"damaged").unwrap();
        }
        let (via, reply) = never();
        assert!(near(colour_of(&p.load(&url(82, 1), None, Some(via), |_| {}, reply).expect("made again")), [90, 200, 40]));
        assert_eq!(there, 3);
        // A wide copy is made of it too; never of a face's bytes.
        assert!(p.store.again(None, &url(82, 1), Size::Wide).is_some_and(|i| i.width() == 512));
        let face = picture(128, 128, [1, 2, 3], image::ImageFormat::Png);
        p.store.put(None, &url(83, 1), Size::Face, &small(&face).unwrap(), Some(&face));
        assert!(p.store.again(None, &url(83, 1), Size::Wide).is_none());
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir).map(|e| e.flatten().flat_map(|e| if e.path().is_dir() { walk(&e.path()) } else { vec![e.path()] }).collect()).unwrap_or_default()
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
        assert!(p.store.has(&url(3, 2), Size::Face), "kept in ours");
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
        assert!(p.store.put(Some(B), &url(41, 1), Size::Face, &image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 1, 1, 255])), None));
        let (via, calls) = counting(Some(picture(16, 16, [9, 9, 9], image::ImageFormat::Png)));
        p.warm(vec![(url(40, 1), Some(A.to_string())), (url(41, 1), Some(B.to_string())), (url(42, 1), None)], via);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !(p.store.has(&url(42, 1), Size::Face) && p.store.has(&url(40, 1), Size::Face)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(p.store.has(&url(40, 1), Size::Face), "from VRCX");
        assert!(p.store.has(&url(42, 1), Size::Face), "asked in the background");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "only the one nobody kept");
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
        assert!(p.store.has(&url(60, 1), Size::Face));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !p.store.has(&url(61, 1), Size::Face) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(p.store.has(&url(61, 1), Size::Face), "the background goes on too");
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
