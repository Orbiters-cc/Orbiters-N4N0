//! Explorer (REQUEST13, `night-1119/explorer-contract.md`), in the tray service: the notice on arriving in a world that has
//! no public 3D picture on Orbiters yet, the two share buttons right after a 3D picture is done ([`Offer`]), and the
//! member's Explorer points. Every surface (the window, the SteamVR overlay's band and camera tools, the desktop's camera
//! tools) draws what [`Status`] says and sends a [`Command`] back; nothing here draws.
//!
//! The notice is welcome, not nagging: at most once per world for this computer's account (remembered on disk), never
//! within [`QUIET`] of the last one, only after the person has been in the world [`SETTLE`], never while a recording, a
//! call, VRChat's camera or N4N0's menu is in the way (the overlay holds it back while its own menus are up,
//! [`Command::NoticeHeld`]), and not at all with Settings' switch off. The world is asked about by its id alone,
//! without signing in.
//!
//! The share buttons belong to one picture, named by [`Offer::id`]: a press says which picture it was for, and a press
//! for another picture than the one offered now does nothing. A share is a Photos & captures job (the same upload
//! records, preparation and server calls, through N4N0's menu's [`super::menu_shares`], which waits for the 3D view);
//! it is said done only when Orbiters' answer shows it (`item.public`, the grants asked for), and what it earned is what
//! THAT answer says (its `explorer`, with the upload's for a picture uploaded by this share): reading the points before
//! and after would count another share or a removal in between.

use orbiters_core::app_log::OrLog as _;
use orbiters_core::locks::*;
use super::Shared;
use crate::controller::shares::{Outcome, ShareJob};
use orbiters_core::account::Server;
use orbiters_core::explorer::{self as ex, Explorer, Title, WorldLookup};
use orbiters_i18n::{mark, tr};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long the person is in a world before it may be said that it has no 3D picture yet: they look around first, and
/// someone only passing through hears nothing.
pub const SETTLE: Duration = Duration::from_secs(25);
/// The least time between two notices, whatever the worlds.
pub const QUIET: Duration = Duration::from_secs(10 * 60);
/// How long a notice waits behind what is in the way before it is let go for this visit.
pub const HOLD_MAX: Duration = Duration::from_secs(3 * 60);
/// How long a notice shows.
pub const NOTICE_MS: u64 = 12_000;
/// How long the share buttons stay after a picture with nothing pressed, and after a share ended.
pub const OFFER_MS: u64 = 3 * 60_000;
pub const RESULT_MS: u64 = 30_000;
/// How long a world's answer is kept (the server caches it five minutes too).
const LOOKUP_KEPT: Duration = Duration::from_secs(5 * 60);
/// Worlds remembered as told, per account (the oldest go first).
const REMEMBERED: usize = 4000;
/// How often the member's points are read again while signed in.
const POINTS_EVERY: Duration = Duration::from_secs(10 * 60);

pub const NOTICE_TITLE: &str = mark("No 3D picture of this world yet");
pub const NOTICE_WHY: &str = mark("Be the first to share one publicly");
pub const NOTICE_EARNS: &str = mark("Be the first to share one publicly: +1 Explorer point");
pub const TAKE: &str = mark("Take a 3D picture");
pub const EXPLORER_POINTS: &str = mark("Explorer points");
pub const NOTICES_SWITCH: &str = mark("Worlds without a 3D picture");
pub const NOTICES_SWITCH_WHY: &str = mark("When you arrive in a world that has no public 3D picture yet, N4N0 says so once, so you can take the first one.");
pub const NOT_NOW: &str = mark("Not now");

/// The time now, Unix ms.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// What every surface shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub notice: Option<WorldNotice>,
    pub offer: Option<Offer>,
    /// The signed-in member's points and title, once read.
    pub points: Option<Explorer>,
    /// "Share 3D capture" on the Quick Menu's side tiles (REQUEST18 3), while it is offered ([`share_here`]).
    #[serde(default)]
    pub share_here: Option<ShareHere>,
}

/// The capture "Share 3D capture" offers (REQUEST18 3: "when a 3D capture have been done but none uploaded publicly, a
/// button Share 3D capture on the side panel with a small thumbnail of the capture"): the world the person is in has a
/// capture on this computer that can be shared, and none of this world's captures here was shared publicly by this account.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShareHere {
    /// The capture: its folder (its thumbnail is there), as History names it.
    pub key: String,
    pub world_id: String,
    pub world_name: String,
    /// How its last share from the tile ended when it did not go (a failure, the daily limit, explicit content): said on
    /// the tile until it is pressed again.
    #[serde(default)]
    pub last: Option<Step>,
}

/// One of this computer's captures of the world the person is in, as [`share_here`] weighs it.
#[derive(Clone, Debug, PartialEq)]
pub struct HereCapture {
    pub key: String,
    pub world_name: String,
    /// When it was taken, Unix seconds.
    pub taken: u64,
    /// It has its 3D view, or what makes one (its frames, depth and keyframes are all still there).
    pub shareable: bool,
    /// This account's upload records say it is public on Orbiters.
    pub public: bool,
}

/// What "Share 3D capture" offers in world `world_id`, from this computer's captures of it (REQUEST18 3). Nothing when no
/// capture of it is here, nor when one of them is shared publicly already (by this account, as its upload records say,
/// or by a share confirmed in this run); else the newest that can be shared.
pub fn share_here(world_id: &str, captures: &[HereCapture], public_now: bool) -> Option<ShareHere> {
    if public_now || captures.iter().any(|c| c.public) {
        return None;
    }
    let newest = captures.iter().filter(|c| c.shareable).max_by_key(|c| c.taken)?;
    Some(ShareHere { key: newest.key.clone(), world_id: world_id.to_string(), world_name: newest.world_name.clone(), last: None })
}

/// This computer's captures of world `world_id` in `sessions_dir`, and whether this account shared each publicly (`uploads`:
/// its upload records). Only their manifests are read, not their files' sizes: it runs when the world changes, a capture is
/// saved or a share ends.
pub fn captures_of(sessions_dir: &Path, world_id: &str, uploads: &[(PathBuf, orbiters_core::shares::Item)]) -> Vec<HereCapture> {
    let Ok(entries) = std::fs::read_dir(sessions_dir) else { return Vec::new() };
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut out = Vec::new();
    for dir in entries.flatten().map(|e| e.path()).filter(|d| d.is_dir()) {
        let Ok(manifest) = orbiters_core::bundle::read_manifest(&dir) else { continue };
        let Some(world) = manifest.world.as_ref().filter(|w| w.id.as_deref() == Some(world_id)) else { continue };
        let ready = orbiters_core::reconstruction::read_report(&dir).is_some();
        let stats = &manifest.stats;
        let makes_one = !orbiters_core::cleanup::trimmed(&dir) && stats.depth_frames > 0 && !(stats.frames > 0 && stats.keyframes == 0);
        let folder = name(&dir);
        let public = uploads.iter().any(|(path, item)| item.public && (*path == dir || name(path) == folder));
        out.push(HereCapture {
            key: dir.to_string_lossy().into_owned(),
            world_name: world.name.clone().unwrap_or_default(),
            taken: crate::controller::shares::unix(&manifest.created_utc),
            shareable: ready || makes_one,
            public,
        });
    }
    out
}

/// "No 3D picture of this world yet", while it is up.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldNotice {
    pub id: u64,
    pub world_id: String,
    pub world_name: String,
    /// What a picture here would earn, when the server said.
    pub earns: Option<f64>,
    pub until_ms: u64,
    pub for_ms: u64,
}

impl WorldNotice {
    pub fn up(&self, now_ms: u64) -> bool {
        now_ms < self.until_ms
    }
}

/// The two buttons after a 3D picture, and how its shares went.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Offer {
    /// The picture's identity: what every press names ([`offer_id`]).
    pub id: u64,
    /// Its folder, as History names it.
    pub key: String,
    pub world_id: String,
    /// What sharing it publicly would earn, when the world's lookup said ([`ex::WorldLookup::next_earns`]).
    pub would_earn: Option<f64>,
    pub until_ms: u64,
    pub public: Step,
    pub people: Step,
}

impl Offer {
    /// Something runs: its buttons wait.
    pub fn busy(&self) -> bool {
        matches!(self.public, Step::Busy { .. }) || matches!(self.people, Step::Busy { .. })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Step {
    #[default]
    Idle,
    /// On its way: its 3D view still being made, or how far its files went (0 to 1, none before they go).
    Busy { making: bool, sent: Option<f32> },
    Done(Done),
    /// It is explicit: confirming that is done in Photos & captures.
    Explicit,
    /// Refused: 30 first public shares in 24 hours. Nothing changed; tomorrow, or to friends now.
    Limited,
    Failed(String),
}

/// A share Orbiters confirmed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Done {
    /// Friends it was shared with (a "Share to …").
    pub people: u32,
    /// What it earned (a public share): none when the points could not be read before or after.
    pub earned: Option<f64>,
    /// Why it earned nothing, when it did not.
    pub nothing: Option<Nothing>,
    /// The member's points after it.
    pub after: Option<Explorer>,
    /// A title reached by it.
    pub reached: Option<Title>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Nothing {
    /// The world already has its three ranked pictures.
    WorldHasThree,
    /// This picture was shared publicly before: it earned then.
    SharedBefore,
    /// Orbiters did not say why.
    Unknown,
}

/// Why nothing was earned, from what the world's lookup said before the share and whether the picture was public before.
pub fn why_nothing(would_earn: Option<f64>, public_before: bool) -> Nothing {
    if public_before {
        Nothing::SharedBefore
    } else if would_earn == Some(0.0) {
        Nothing::WorldHasThree
    } else {
        Nothing::Unknown
    }
}

/// What a surface asks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// "Take a 3D picture" on notice `id`.
    NoticeTake { id: u64 },
    /// Its close.
    NoticeDismiss { id: u64 },
    /// It could not show: a menu is up. It waits (a little).
    NoticeHeld { id: u64 },
    /// "Share publicly" on the picture `offer`; `confirmed` the first time's question was answered yes (the settings
    /// remember it).
    SharePublic { offer: u64, confirmed: bool },
    /// "Share to …" sent: the friends chosen (VRChat user id, name).
    ShareTo { offer: u64, recipients: Vec<(String, String)> },
    /// The buttons put away.
    OfferDismiss { offer: u64 },
    /// "Share 3D capture" on the Quick Menu's side tiles (REQUEST18 3) for the capture `key` it offers: shared publicly
    /// as "Share publicly" shares a picture, its question answered when `confirmed`.
    ShareCapture { key: String, confirmed: bool },
}

/// A picture's identity from its folder's name (whichever way its path is written).
pub fn offer_id(key: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let name = Path::new(key).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    h.finish() | 1
}

/// What stands in the notice's way now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InTheWay {
    pub recording: bool,
    pub call: bool,
    pub camera_open: bool,
    pub menu_open: bool,
}
impl InTheWay {
    pub fn any(self) -> bool {
        self.recording || self.call || self.camera_open || self.menu_open
    }
}

/// What to do with a world that has no public 3D picture, now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Show,
    /// Not yet: in the world too short a time, something in the way, too soon after the last.
    Wait,
    /// Not on this visit.
    Drop,
}

/// The notice's rules (tested alone): `since` in the world this long, `told` already about this world, the last notice
/// `last` ago, `held` the notice waited this long behind something.
pub fn decide(on: bool, told: bool, since: Duration, last: Option<Duration>, way: InTheWay, waited: Duration) -> Decision {
    if !on || told {
        return Decision::Drop;
    }
    if since < SETTLE {
        return Decision::Wait;
    }
    if waited > HOLD_MAX {
        return Decision::Drop;
    }
    if way.any() || last.is_some_and(|l| l < QUIET) {
        return Decision::Wait;
    }
    Decision::Show
}

/// What a press on the notice did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pressed {
    /// For no notice up now (another one, or it went): nothing.
    Nothing,
    /// "Take a 3D picture" (the card): one is taken.
    Take,
    /// Its cross: it goes, and that world is not told again.
    Closed,
}

/// The press `command` on the notice `up` (Request20, tested alone): the card takes a 3D picture; the cross closes it,
/// and its world is put with this account's worlds told (it was when the notice came: again here, so that a write that
/// failed then cannot bring it back on the next visit), never another world. A press for a notice no longer up does
/// nothing.
fn notice_pressed(command: &Command, up: Option<&WorldNotice>, told: &mut Told, account: &str) -> Pressed {
    let (id, take) = match *command {
        Command::NoticeTake { id } => (id, true),
        Command::NoticeDismiss { id } => (id, false),
        _ => return Pressed::Nothing,
    };
    let Some(notice) = up.filter(|n| n.id == id) else { return Pressed::Nothing };
    if take {
        return Pressed::Take;
    }
    told.add(account, &notice.world_id);
    Pressed::Closed
}

/// Worlds told about, per account, on disk.
#[derive(Default, Serialize, Deserialize)]
struct Told {
    accounts: BTreeMap<String, Vec<String>>,
}

fn told_path() -> PathBuf {
    orbiters_core::paths::local_data_root().join("explorer-worlds.json")
}

impl Told {
    fn read(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    fn has(&self, account: &str, world: &str) -> bool {
        self.accounts.get(account).is_some_and(|w| w.iter().any(|x| x == world))
    }
    fn add(&mut self, account: &str, world: &str) {
        let worlds = self.accounts.entry(account.to_string()).or_default();
        if !worlds.iter().any(|x| x == world) {
            worlds.push(world.to_string());
        }
        let over = worlds.len().saturating_sub(REMEMBERED);
        worlds.drain(..over);
    }
    fn write(&self, path: &Path) {
        if let Ok(bytes) = serde_json::to_vec(self) {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).or_log("a folder could not be made");
            }
            orbiters_core::write_atomic(path, &bytes).or_log("a file could not be written");
        }
    }
}

/// A share asked from the buttons.
struct Asked {
    public: bool,
    people: Vec<String>,
    since: Instant,
    /// What was asked, to ask it again once after a "not ready" ([`after_refusal`]).
    ask: orbiters_n4n0_core::captures::ShareAsk,
    /// Whose it is.
    epoch: u64,
    retried: bool,
    retry_at: Option<Instant>,
    /// The refused job, while its retry is not listed yet.
    skip_job: Option<String>,
}

/// How long a share refused because Orbiters is not ready waits before it is asked again, once.
pub const RETRY_AFTER: Duration = Duration::from_secs(15);
pub const LIMIT_TITLE: &str = mark("Daily limit reached");
pub const LIMIT_WHY: &str = mark("Share it publicly tomorrow, or to friends now.");
pub const NOT_READY: &str = mark("Orbiters is getting ready: try again in a moment");

/// What follows a share refused as `refused`, `retried` already or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterRefusal {
    /// Not a refusal said in its own way: as the answer says.
    AsAnswered,
    /// Orbiters not ready: asked again once, by itself, in a moment.
    RetryOnce,
    /// Still not ready after that: said, and pressing tries again.
    NotReady,
    /// The daily limit: said plainly.
    Limited,
}

pub fn after_refusal(refused: Option<crate::controller::shares::Refusal>, retried: bool) -> AfterRefusal {
    use crate::controller::shares::Refusal;
    match refused {
        Some(Refusal::DailyLimit) => AfterRefusal::Limited,
        Some(Refusal::NotReady) if !retried => AfterRefusal::RetryOnce,
        Some(Refusal::NotReady) => AfterRefusal::NotReady,
        None => AfterRefusal::AsAnswered,
    }
}

/// What the explorer keeps between ticks.
#[derive(Default)]
pub(super) struct ExplorerState {
    /// The world the person is in, since when, and whether it was asked about on this visit.
    here: Option<(String, Instant, bool)>,
    /// Worlds' answers, when.
    lookups: HashMap<String, (Option<WorldLookup>, Instant)>,
    /// A world answered as having none, waiting to be told (since when it waits).
    candidate: Option<(String, Instant)>,
    last_notice: Option<Instant>,
    next_id: u64,
    told: Option<Told>,
    /// The share asked per offer side.
    public: Option<Asked>,
    people: Option<Asked>,
    points_at: Option<(Instant, String)>,
    /// The overlay held the notice back at this time last.
    held: Option<Instant>,
    /// What "Share 3D capture" was last looked for with (the world, the account, the captures and shares then), and when.
    share_scan: Option<(String, Instant)>,
    /// Worlds a public share was confirmed for in this run: the tile is not offered there again, whatever the records
    /// say yet.
    shared_worlds: std::collections::HashSet<String>,
}

/// The explorer's own thread: a look once a second while the service runs.
pub(super) fn spawn(ctl: Shared) {
    let started = std::thread::Builder::new().name("explorer".into()).spawn(move || {
        while !ctl.quitting.load(std::sync::atomic::Ordering::Relaxed) {
            ctl.explorer_tick();
            std::thread::sleep(Duration::from_secs(1));
        }
    });
    if let Err(e) = started {
        log_warn!("explorer: {e}", e = e); // i18n: not shown (a log's line)
    }
}

impl super::Controller {
    /// Once a second, on its own thread: arrival in a world, the offer's shares and timeouts, the points.
    pub(super) fn explorer_tick(self: &Shared) {
        let snap = self.view();
        let server = if snap.settings.development { Server::Development } else { Server::Production };
        let now = Instant::now();
        let mut s = self.explorer.locked();
        // Arrival.
        let world = &snap.vrchat.world;
        let here = world.world_id.clone().filter(|_| snap.vrchat.running && world.in_world).filter(|w| ex::is_world_id(w));
        if here.as_deref() != s.here.as_ref().map(|h| h.0.as_str()) {
            s.here = here.map(|w| (w, now, false));
            s.candidate = None;
        }
        let account = account_key(&snap);
        let on = snap.settings.explorer.world_notices;
        if let Some((world_id, since, asked)) = s.here.clone() {
            let told = s.told.get_or_insert_with(|| Told::read(&told_path())).has(&account, &world_id);
            if on && !told && !asked && now.duration_since(since) >= SETTLE {
                s.here.as_mut().unwrap().2 = true;
                let fresh = s.lookups.get(&world_id).filter(|(_, at)| at.elapsed() < LOOKUP_KEPT).map(|(l, _)| *l);
                match fresh {
                    Some(lookup) => {
                        if lookup.is_some_and(|l| !l.has_public) {
                            s.candidate = Some((world_id.clone(), now));
                        }
                    }
                    None => self.explorer_look_up(server, world_id.clone(), true),
                }
            }
            if let Some((cand, waiting)) = s.candidate.clone().filter(|(c, _)| *c == world_id) {
                let way = InTheWay {
                    recording: snap.is_capturing() || snap.capture.state == crate::state::CaptureState::Stopping,
                    call: snap.n4n0.calls.call.is_some(),
                    camera_open: snap.camera_open_said == Some(true),
                    menu_open: snap.n4n0.screen_open || snap.n4n0.calls.menu_open,
                };
                let waited = now.duration_since(waiting);
                match decide(on, told, now.duration_since(since), s.last_notice.map(|l| now.duration_since(l)), way, waited) {
                    Decision::Show if self.explorer_status().notice.is_none() => {
                        s.next_id += 1;
                        let earns = s.lookups.get(&cand).and_then(|(l, _)| l.and_then(|l| l.next_earns));
                        let notice = WorldNotice {
                            id: s.next_id,
                            world_id: cand.clone(),
                            world_name: world.world_name.clone().unwrap_or_default(),
                            earns,
                            until_ms: now_ms() + NOTICE_MS,
                            for_ms: NOTICE_MS,
                        };
                        s.candidate = None;
                        s.held = None;
                        s.last_notice = Some(now);
                        let told = s.told.get_or_insert_with(Told::default);
                        told.add(&account, &cand);
                        told.write(&told_path());
                        self.update(|st| st.explorer.notice = Some(notice.clone()));
                    }
                    Decision::Drop => s.candidate = None,
                    _ => {}
                }
            }
        }
        // A notice whose time is over goes; one for another world than this one too.
        let status = self.explorer_status();
        if let Some(n) = status.notice.as_ref() {
            if !n.up(now_ms()) || s.here.as_ref().is_none_or(|h| h.0 != n.world_id) {
                self.update(|st| st.explorer.notice = None);
            }
        }
        // The offer: its shares, then its time.
        self.explorer_follow_shares(&mut s, &snap);
        let status = self.explorer_status();
        if let Some(offer) = status.offer.as_ref() {
            let recording = snap.is_capturing();
            // VRChat's camera closed: the buttons go with it (the picture is still in Photos & captures).
            let closed = snap.camera_open_said == Some(false) && offer.until_ms.saturating_sub(OFFER_MS) + 5_000 < now_ms();
            if ((now_ms() >= offer.until_ms || closed) && !offer.busy()) || recording {
                self.update(|st| st.explorer.offer = None);
                s.public = None;
                s.people = None;
            }
        }
        // "Share 3D capture": looked for again when what it depends on changed.
        self.explorer_share_here(&mut s, &snap, &account);
        // The points: while signed in, now and then; gone at sign-out.
        if account.is_empty() {
            if status.points.is_some() {
                self.update(|st| st.explorer.points = None);
            }
            s.points_at = None;
        } else if s.points_at.as_ref().is_none_or(|(at, who)| *who != account || at.elapsed() > POINTS_EVERY) {
            s.points_at = Some((now, account.clone()));
            self.explorer_read_points();
        }
    }

    fn explorer_status(&self) -> Status {
        self.view().explorer.clone()
    }

    /// Asks whether `world_id` has a public 3D picture (by its id alone), on a thread; `notice`: for the arrival.
    fn explorer_look_up(self: &Shared, server: Server, world_id: String, notice: bool) {
        let ctl = self.clone();
        std::thread::spawn(move || {
            let lookup = orbiters_core::account::Api::new(server).world_public_capture(&world_id).ok().flatten();
            let mut s = ctl.explorer.locked();
            s.lookups.insert(world_id.clone(), (lookup, Instant::now()));
            if notice && lookup.is_some_and(|l| !l.has_public) && s.here.as_ref().is_some_and(|h| h.0 == world_id) {
                s.candidate = Some((world_id.clone(), Instant::now()));
            }
            drop(s);
            // The offer of a picture of this world says what it would earn.
            if let Some(earns) = lookup.and_then(|l| l.next_earns) {
                ctl.update_if(|st| match st.explorer.offer.as_mut().filter(|o| o.world_id == world_id && o.would_earn != Some(earns)) {
                    Some(o) => {
                        o.would_earn = Some(earns);
                        true
                    }
                    None => false,
                });
            }
        });
    }

    /// Reads the member's points on a thread; with `then`, a public share's result for offer `then` is made from them.
    /// Reads the member's points on a thread (Settings' line; the share buttons say what a share's own answer said).
    fn explorer_read_points(self: &Shared) {
        let ctl = self.clone();
        std::thread::spawn(move || {
            if let Some(p) = ctl.account_api().and_then(|(api, token)| api.explorer(&token).ok()) {
                ctl.update(|st| st.explorer.points = Some(p));
            }
        });
    }

    /// A 3D picture was saved (`dir`): its two buttons are offered, in place of the last picture's.
    pub(super) fn explorer_offer(self: &Shared, dir: &Path) {
        let snap = self.view();
        let key = snap.settings.sessions_dir.join(dir.file_name().unwrap_or_default()).to_string_lossy().into_owned();
        let world_id = snap.vrchat.world.world_id.clone().unwrap_or_default();
        let server = if snap.settings.development { Server::Development } else { Server::Production };
        let would_earn = {
            let mut s = self.explorer.locked();
            s.public = None;
            s.people = None;
            s.lookups.get(&world_id).filter(|(_, at)| at.elapsed() < LOOKUP_KEPT).and_then(|(l, _)| l.and_then(|l| l.next_earns))
        };
        let offer = Offer { id: offer_id(&key), key, world_id: world_id.clone(), would_earn, until_ms: now_ms() + OFFER_MS, public: Step::Idle, people: Step::Idle };
        self.update(|st| st.explorer.offer = Some(offer.clone()));
        if would_earn.is_none() && ex::is_world_id(&world_id) {
            self.explorer_look_up(server, world_id, false);
        }
    }

    /// What a surface asked, while it showed the member of `epoch`: what is shared is shared as them, or not at all.
    pub fn explorer_command(self: &Shared, command: Command, epoch: u64) {
        let shares = matches!(command, Command::SharePublic { .. } | Command::ShareCapture { .. } | Command::ShareTo { .. });
        let acting = self.n4n0.acting(epoch);
        if shares && acting.is_none() {
            return self.n4n0.refuse_other_identity();
        }
        let status = self.explorer_status();
        match command {
            Command::NoticeTake { .. } | Command::NoticeDismiss { .. } => {
                let account = account_key(&self.view());
                let pressed = {
                    let mut s = self.explorer.locked();
                    let told = s.told.get_or_insert_with(|| Told::read(&told_path()));
                    let pressed = notice_pressed(&command, status.notice.as_ref(), told, &account);
                    if pressed == Pressed::Closed {
                        told.write(&told_path());
                    }
                    pressed
                };
                if pressed != Pressed::Nothing {
                    self.update(|st| st.explorer.notice = None);
                }
                if pressed == Pressed::Take {
                    // The existing flow: a 3D picture once VRChat's camera is open (it waits for it, and says so).
                    self.n4n0_command(crate::ipc::N4n0Command::Photo3DWhenCamera { seconds: 3 });
                }
            }
            Command::NoticeHeld { id } => {
                // Held behind a menu: its time starts again, up to HOLD_MAX after it came.
                let mut s = self.explorer.locked();
                let first = *s.held.get_or_insert_with(Instant::now);
                drop(s);
                if first.elapsed() < HOLD_MAX {
                    self.update_if(|st| match st.explorer.notice.as_mut().filter(|n| n.id == id) {
                        Some(n) => {
                            n.until_ms = now_ms() + n.for_ms;
                            true
                        }
                        None => false,
                    });
                }
            }
            Command::OfferDismiss { offer } => {
                if status.offer.as_ref().is_some_and(|o| o.id == offer && !o.busy()) {
                    self.update(|st| st.explorer.offer = None);
                }
            }
            Command::SharePublic { offer, confirmed } => self.explorer_share(offer, true, Vec::new(), confirmed, epoch),
            Command::ShareCapture { key, confirmed } => self.explorer_share_capture(key, confirmed, epoch),
            Command::ShareTo { offer, recipients } => self.explorer_share(offer, false, recipients, false, epoch),
        }
    }

    /// "Share 3D capture" (REQUEST18 3): the capture the tile offers now, and only it, becomes the offer (the one a picture
    /// just taken has, kept when it is that capture) and is shared publicly through it: the same question the first time,
    /// the same Photos & captures job, the same confirmation by Orbiters' answer. Not while another picture's share runs.
    fn explorer_share_capture(self: &Shared, key: String, confirmed: bool, epoch: u64) {
        let status = self.explorer_status();
        let Some(here) = status.share_here.filter(|h| offer_id(&h.key) == offer_id(&key)) else { return };
        let id = offer_id(&here.key);
        match status.offer.as_ref() {
            Some(o) if o.id == id => {}
            Some(o) if o.busy() => return,
            _ => {
                let would_earn = {
                    let mut s = self.explorer.locked();
                    s.public = None;
                    s.people = None;
                    s.lookups.get(&here.world_id).filter(|(_, at)| at.elapsed() < LOOKUP_KEPT).and_then(|(l, _)| l.and_then(|l| l.next_earns))
                };
                let offer = Offer { id, key: here.key.clone(), world_id: here.world_id.clone(), would_earn, until_ms: now_ms() + OFFER_MS, public: Step::Idle, people: Step::Idle };
                self.update(|st| st.explorer.offer = Some(offer.clone()));
            }
        }
        // Pressed again: what it said last goes.
        self.update_if(|st| match st.explorer.share_here.as_mut().filter(|h| h.last.is_some()) {
            Some(h) => {
                h.last = None;
                true
            }
            None => false,
        });
        self.explorer_share(id, true, Vec::new(), confirmed, epoch);
    }

    /// What "Share 3D capture" offers, looked for again (on a thread) when the world, the account, a capture being made or
    /// a share's end changed, and every five minutes; gone at once out of a world, signed out, or in another world.
    fn explorer_share_here(self: &Shared, s: &mut ExplorerState, snap: &crate::state::Snapshot, account: &str) {
        let world = s.here.as_ref().map(|h| h.0.clone()).filter(|_| !account.is_empty());
        let shown = snap.explorer.share_here.as_ref();
        if shown.is_some_and(|h| Some(&h.world_id) != world.as_ref() || s.shared_worlds.contains(&h.world_id)) {
            self.update(|st| st.explorer.share_here = None);
        }
        let Some(world) = world else {
            s.share_scan = None;
            return;
        };
        let ended = snap.capture_shares.iter().filter(|j| j.outcome.is_some()).count();
        let why = format!("{world} {account} {} {:?} {:?} {ended}", snap.settings.development, snap.capture.session_dir, snap.capture.state); // i18n: not shown (a key)
        if s.share_scan.as_ref().is_some_and(|(was, at)| *was == why && at.elapsed() < Duration::from_secs(5 * 60)) {
            return;
        }
        s.share_scan = Some((why, Instant::now()));
        let public_now = s.shared_worlds.contains(&world);
        let server = if snap.settings.development { Server::Development } else { Server::Production };
        let sessions_dir = snap.settings.sessions_dir.clone();
        let ctl = self.clone();
        std::thread::Builder::new().name("share-here".into()).spawn(move || {
            let uploads = crate::controller::shares::uploads(server);
            let found = share_here(&world, &captures_of(&sessions_dir, &world, &uploads), public_now);
            // Still in that world: else the next look is under way.
            let here = ctl.explorer.locked().here.as_ref().map(|h| h.0.clone());
            if here.as_deref() != Some(world.as_str()) {
                return;
            }
            ctl.update_if(|st| {
                // The same capture keeps what its last share said.
                let found = found.clone().map(|mut f| {
                    f.last = st.explorer.share_here.as_ref().filter(|h| offer_id(&h.key) == offer_id(&f.key)).and_then(|h| h.last.clone());
                    f
                });
                if st.explorer.share_here == found {
                    return false;
                }
                st.explorer.share_here = found;
                true
            });
        }).or_log("a thread could not be started");
    }

    fn explorer_share(self: &Shared, offer: u64, public: bool, recipients: Vec<(String, String)>, confirmed: bool, epoch: u64) {
        let Some(o) = self.explorer_status().offer.filter(|o| o.id == offer) else { return };
        let step = if public { &o.public } else { &o.people };
        // One share of a picture at a time (N4N0's menu keeps one ask per capture); a public one done stays done.
        if o.busy() || (public && matches!(step, Step::Done(_))) {
            return;
        }
        // Public and irreversible in practice: asked once, the first time.
        let settings = self.settings();
        if public && !settings.explorer.public_confirmed {
            if !confirmed {
                return;
            }
            let mut changed = settings.clone();
            changed.explorer.public_confirmed = true;
            self.save_settings(changed);
        }
        if !public && recipients.is_empty() {
            return;
        }
        let set = move |st: &mut crate::state::Snapshot, step: Step| {
            if let Some(o) = st.explorer.offer.as_mut().filter(|o| o.id == offer) {
                if public {
                    o.public = step;
                } else {
                    o.people = step;
                }
                o.until_ms = o.until_ms.max(now_ms() + OFFER_MS);
            }
        };
        self.update(|st| set(st, Step::Busy { making: false, sent: None }));
        let ctl = self.clone();
        let key = o.key.clone();
        std::thread::spawn(move || {
            let ask = orbiters_n4n0_core::captures::ShareAsk {
                key: key.clone(),
                public,
                recipients: recipients.iter().map(|(id, name)| orbiters_n4n0_core::captures::Friend { id: id.clone(), name: name.clone() }).collect(),
                acknowledge_explicit: false,
                ..Default::default()
            };
            let ask_again = ask.clone();
            match ctl.menu_capture_share(ask, epoch) {
                Ok(()) => {
                    let asked = Asked { public, people: recipients.iter().map(|r| r.0.clone()).collect(), since: Instant::now(), ask: ask_again, epoch, retried: false, retry_at: None, skip_job: None };
                    let mut s = ctl.explorer.locked();
                    if public {
                        s.public = Some(asked);
                    } else {
                        s.people = Some(asked);
                    }
                }
                Err(message) => ctl.update(|st| set(st, Step::Failed(message.clone()))),
            }
        });
    }

    /// Where the offer's shares stand, from N4N0's menu's asks and Photos & captures' jobs; a share confirmed by Orbiters'
    /// answer only.
    fn explorer_follow_shares(self: &Shared, s: &mut ExplorerState, snap: &crate::state::Snapshot) {
        let Some(offer) = snap.explorer.offer.as_ref() else { return };
        for public in [true, false] {
            let Some(asked) = (if public { s.public.as_mut() } else { s.people.as_mut() }) else { continue };
            // Asked again once after "not ready": when its moment comes, on a thread (it lists the computer's captures).
            if let Some(at) = asked.retry_at {
                if Instant::now() >= at {
                    asked.retry_at = None;
                    asked.since = Instant::now();
                    let (ctl, ask, id, epoch) = (self.clone(), asked.ask.clone(), offer.id, asked.epoch);
                    std::thread::spawn(move || {
                        if let Err(message) = ctl.menu_capture_share(ask, epoch) {
                            ctl.update(|st| {
                                if let Some(o) = st.explorer.offer.as_mut().filter(|o| o.id == id) {
                                    if public {
                                        o.public = Step::Failed(message.clone());
                                    } else {
                                        o.people = Step::Failed(message.clone());
                                    }
                                }
                            });
                        }
                    });
                }
                continue;
            }
            let state = self.menu_share_state(&offer.key);
            let state = match state {
                Some(super::menu_shares::AskState::Job(id)) if asked.skip_job.as_ref() == Some(&id) => None,
                other => other,
            };
            let step = match state {
                Some(super::menu_shares::AskState::Waiting) => Step::Busy { making: true, sent: None },
                Some(super::menu_shares::AskState::Failed(message)) => Step::Failed(message),
                Some(super::menu_shares::AskState::Job(id)) => match snap.capture_shares.iter().find(|j| j.id == id) {
                    None | Some(ShareJob { outcome: None, .. }) => {
                        let sent = snap.capture_shares.iter().find(|j| j.id == id).and_then(|j| match j.stage {
                            crate::controller::shares::Stage::Sending { sent, total } if total > 0 => Some(sent as f32 / total as f32),
                            _ => None,
                        });
                        Step::Busy { making: false, sent }
                    }
                    Some(ShareJob { outcome: Some(outcome), refused, .. }) => match after_refusal(*refused, asked.retried) {
                        AfterRefusal::AsAnswered => verified(outcome, public, &asked.people, offer.would_earn),
                        AfterRefusal::Limited => Step::Limited,
                        AfterRefusal::NotReady => Step::Failed(tr(NOT_READY).into()),
                        AfterRefusal::RetryOnce => {
                            asked.retried = true;
                            asked.retry_at = Some(Instant::now() + RETRY_AFTER);
                            asked.skip_job = Some(id.clone());
                            Step::Busy { making: false, sent: None }
                        }
                    },
                },
                None if asked.since.elapsed() < Duration::from_secs(10) || asked.skip_job.is_some() && asked.since.elapsed() < Duration::from_secs(30) => Step::Busy { making: false, sent: None },
                None => Step::Failed(tr("The share did not start: try again from Photos & captures").into()),
            };
            // A public share Orbiters confirmed: its points are the member's now, and a toast says what it earned (the
            // buttons may be gone before it is read).
            if let (true, Step::Done(done)) = (public, &step) {
                if let Some(after) = done.after.clone() {
                    self.update(|st| st.explorer.points = Some(after.clone()));
                }
                self.notify(super::NoticeKind::Success, result_words(done));
            }
            {
                {
                    let finished = matches!(step, Step::Done(_) | Step::Failed(_) | Step::Explicit | Step::Limited);
                    let id = offer.id;
                    self.update_if(|st| match st.explorer.offer.as_mut().filter(|o| o.id == id) {
                        Some(o) => {
                            let slot = if public { &mut o.public } else { &mut o.people };
                            if *slot == step {
                                return false;
                            }
                            *slot = step.clone();
                            if finished {
                                o.until_ms = now_ms() + RESULT_MS;
                            }
                            true
                        }
                        None => false,
                    });
                    if finished {
                        if public {
                            s.public = None;
                        } else {
                            s.people = None;
                        }
                    }
                    // "Share 3D capture" for this capture: gone once it is public, else it says how it ended.
                    if public && finished {
                        if matches!(step, Step::Done(_)) {
                            s.shared_worlds.insert(offer.world_id.clone());
                        }
                        let (key, done) = (offer.key.clone(), matches!(step, Step::Done(_)));
                        self.update_if(|st| {
                            if !st.explorer.share_here.as_ref().is_some_and(|h| offer_id(&h.key) == offer_id(&key)) {
                                return false;
                            }
                            if done {
                                st.explorer.share_here = None;
                            } else if let Some(h) = st.explorer.share_here.as_mut() {
                                h.last = Some(step.clone());
                            }
                            true
                        });
                    }
                }
            }
        }
    }
}

/// What a share job's outcome is, believed only as far as Orbiters' answer shows it: public, its id there, the friends
/// asked among its grants (verify-core 4: a 2xx without them is no success).
pub fn verified(outcome: &Outcome, public: bool, people: &[String], would: Option<f64>) -> Step {
    use orbiters_core::shares::Shared;
    match outcome {
        Outcome::Shared(Shared::Item { item, explorer }) if item.id != 0 && (!public || item.public) => {
            let missing = people.iter().filter(|p| !item.grants.iter().any(|g| &g.vrchat_user_id == *p)).count();
            if public {
                // What THIS share's answer says it earned; none when it says nothing (the share stands, said without points).
                let mut done = Done { people: 0, earned: None, nothing: None, after: None, reached: None };
                if let Some(e) = explorer.as_ref() {
                    done.earned = e.earned.map(|x| (x * 2.0).round() / 2.0);
                    done.after = Some(e.standing());
                    done.reached = e.title_reached.clone();
                    if done.earned == Some(0.0) {
                        done.nothing = Some(why_nothing(would, false));
                    }
                }
                Step::Done(done)
            } else if missing == 0 {
                Step::Done(Done { people: people.len() as u32, earned: None, nothing: None, after: None, reached: None })
            } else {
                Step::Failed(tr("Orbiters did not confirm who sees it: look in Photos & captures").into())
            }
        }
        Outcome::Shared(Shared::Item { .. }) => Step::Failed(tr("Orbiters did not confirm the share: look in Photos & captures").into()),
        Outcome::Shared(Shared::Request { .. }) => Step::Failed(tr("It went as a request: look in Photos & captures").into()),
        Outcome::Failed { explicit: true, .. } => Step::Explicit,
        Outcome::Failed { message, .. } => Step::Failed(message.clone()),
        _ => Step::Failed(tr("Orbiters did not confirm the share: look in Photos & captures").into()),
    }
}

/// Who is signed in, as the told worlds are kept by: empty when nobody.
fn account_key(snap: &crate::state::Snapshot) -> String {
    match &snap.account.state {
        crate::state::AccountState::SignedIn { username } => username.clone(),
        crate::state::AccountState::Offline { username: Some(u) } | crate::state::AccountState::Checking { username: Some(u) } => u.clone(),
        _ => String::new(),
    }
}

pub const SHARED_PUBLICLY: &str = mark("Shared publicly");
pub const EARNED_ONE: &str = mark("+1 Explorer point");
pub const EARNED_SOME: &str = mark("+{points} Explorer points");
pub const NONE_WORLD_FULL: &str = mark("No points: this world already has three public 3D pictures");
pub const NONE_SHARED_BEFORE: &str = mark("No points: this picture was shared publicly before");
pub const NONE_UNKNOWN: &str = mark("No new points this time");
pub const ONE_POINT: &str = mark("1 point");
pub const POINTS: &str = mark("{points} points");
pub const TO_NEXT: &str = mark("{remaining} to {title}");
pub const NEW_TITLE: &str = mark("New title: {title}");

/// What one share earned, in words: "+1 Explorer point", "+0.5 Explorer points", or why nothing.
pub fn earned_words(done: &Done) -> Option<String> {
    match (done.earned, done.nothing) {
        (Some(e), _) if e == 1.0 => Some(tr(EARNED_ONE).into()),
        (Some(e), _) if e > 0.0 => Some(tr!("+{points} Explorer points", points = orbiters_ui::format::points(e))),
        (_, Some(Nothing::WorldHasThree)) => Some(tr(NONE_WORLD_FULL).into()),
        (_, Some(Nothing::SharedBefore)) => Some(tr(NONE_SHARED_BEFORE).into()),
        (_, Some(Nothing::Unknown)) => Some(tr(NONE_UNKNOWN).into()),
        _ => None,
    }
}

/// A total of points in words: "1 point", "12.5 points", "-1 points".
pub fn points_words(points: f64) -> String {
    if (points - 1.0).abs() < 1e-9 {
        tr(ONE_POINT).into()
    } else {
        tr!("{points} points", points = orbiters_ui::format::points(points))
    }
}

/// A title's name in the language now, by its stable key (the server's name when the key is not one of these).
pub fn title_name(title: &Title) -> String {
    let english = match title.key.as_str() {
        "explorer" => mark("Explorer"),
        "navigator" => mark("Navigator"),
        "discoverer" => mark("Discoverer"),
        "marcoPolo" => mark("Marco polo"),
        "multiverseTraveler" => mark("Multiverse traveler"),
        _ => return title.name.clone(),
    };
    tr(english).into()
}

/// Where the member stands, in words: "12.5 points · 37.5 to Navigator", or the title at the top.
pub fn standing_words(e: &Explorer) -> String {
    let total = points_words(e.points);
    match (e.next_title(), e.title.as_ref()) {
        (Some(next), _) => {
            let title = Title { key: next.key.clone(), points: next.points, name: next.name.clone() };
            format!("{total} · {}", tr!("{remaining} to {title}", remaining = orbiters_ui::format::points(next.remaining), title = title_name(&title)))
        }
        (None, Some(t)) => format!("{total} · {}", title_name(t)),
        (None, None) => total,
    }
}

/// The toast after a public share (the buttons may be gone by then).
pub fn result_words(done: &Done) -> String {
    let mut words = vec![tr(SHARED_PUBLICLY).to_string()];
    words.extend(earned_words(done));
    if let Some(t) = done.reached.as_ref() {
        words.push(tr!("New title: {title}", title = title_name(t)));
    } else if let Some(after) = done.after.as_ref() {
        words.push(standing_words(after));
    }
    words.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_core::shares::{Grant, Item, ShareRequest, Shared};

    const S: Duration = Duration::from_secs(1);
    const NOTHING: InTheWay = InTheWay { recording: false, call: false, camera_open: false, menu_open: false };

    /// Once per world, after the settling time, never within the quiet time of the last, never with something in the
    /// way (it waits, then lets go), never with the switch off.
    #[test]
    fn the_notice_comes_once_per_world_and_keeps_its_quiet() {
        let settled = SETTLE + S;
        assert_eq!(decide(true, false, settled, None, NOTHING, Duration::ZERO), Decision::Show);
        assert_eq!(decide(true, false, SETTLE - S, None, NOTHING, Duration::ZERO), Decision::Wait, "just arrived");
        assert_eq!(decide(true, true, settled, None, NOTHING, Duration::ZERO), Decision::Drop, "told about this world already");
        assert_eq!(decide(false, false, settled, None, NOTHING, Duration::ZERO), Decision::Drop, "switched off");
        assert_eq!(decide(true, false, settled, Some(QUIET - S), NOTHING, Duration::ZERO), Decision::Wait, "too soon after the last");
        assert_eq!(decide(true, false, settled, Some(QUIET + S), NOTHING, Duration::ZERO), Decision::Show);
        for way in [
            InTheWay { recording: true, ..NOTHING },
            InTheWay { call: true, ..NOTHING },
            InTheWay { camera_open: true, ..NOTHING },
            InTheWay { menu_open: true, ..NOTHING },
        ] {
            assert_eq!(decide(true, false, settled, None, way, Duration::ZERO), Decision::Wait, "{way:?}");
            assert_eq!(decide(true, false, settled, None, way, HOLD_MAX + S), Decision::Drop, "{way:?}: not on this visit");
        }
    }

    /// The worlds told about are kept on disk per account, and one account's are not another's.
    #[test]
    fn worlds_told_are_remembered_per_account() {
        let dir = std::env::temp_dir().join(format!("orbiters-explorer-{}", uuid::Uuid::new_v4().simple()));
        let path = dir.join("explorer-worlds.json");
        let mut told = Told::read(&path);
        assert!(!told.has("ada", "wrld_a"));
        told.add("ada", "wrld_a");
        told.add("ada", "wrld_a");
        told.write(&path);
        let again = Told::read(&path);
        assert!(again.has("ada", "wrld_a") && !again.has("bram", "wrld_a") && !again.has("ada", "wrld_b"));
        assert_eq!(again.accounts["ada"].len(), 1);
        let mut many = Told::default();
        for i in 0..REMEMBERED + 3 {
            many.add("ada", &format!("wrld_{i}"));
        }
        assert!(!many.has("ada", "wrld_0") && many.has("ada", &format!("wrld_{}", REMEMBERED + 2)), "the oldest go first");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Request20: the cross closes the notice for that world for good (kept on disk with the worlds told, per account):
    /// on the next visit there it does not come; another world's still does. A press for another notice does nothing.
    #[test]
    fn closing_the_notice_keeps_that_world_quiet_and_only_that_world() {
        let dir = std::env::temp_dir().join(format!("orbiters-explorer-{}", uuid::Uuid::new_v4().simple()));
        let path = dir.join("explorer-worlds.json");
        let notice = WorldNotice { id: 4, world_id: "wrld_a".into(), world_name: "A".into(), earns: None, until_ms: u64::MAX, for_ms: NOTICE_MS };
        let mut told = Told::read(&path);
        assert_eq!(notice_pressed(&Command::NoticeDismiss { id: 3 }, Some(&notice), &mut told, "ada"), Pressed::Nothing, "another notice's cross");
        assert_eq!(notice_pressed(&Command::NoticeDismiss { id: 4 }, None, &mut told, "ada"), Pressed::Nothing, "it went already");
        assert!(!told.has("ada", "wrld_a"));
        assert_eq!(notice_pressed(&Command::NoticeDismiss { id: 4 }, Some(&notice), &mut told, "ada"), Pressed::Closed);
        told.write(&path);
        let again = Told::read(&path);
        assert!(again.has("ada", "wrld_a"), "remembered on disk");
        assert!(!again.has("ada", "wrld_b") && !again.has("bram", "wrld_a"), "that world, that account only");
        let settled = SETTLE + S;
        assert_eq!(decide(true, again.has("ada", "wrld_a"), settled, None, NOTHING, Duration::ZERO), Decision::Drop, "the next visit there: nothing");
        assert_eq!(decide(true, again.has("ada", "wrld_b"), settled, None, NOTHING, Duration::ZERO), Decision::Show, "another world: it comes");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Request20: a press on the card itself still takes a 3D picture (the controller then asks N4N0's flow for one, which
    /// waits for VRChat's camera); a press for a notice no longer up does not.
    #[test]
    fn the_press_on_the_notice_takes_a_3d_picture() {
        let notice = WorldNotice { id: 9, world_id: "wrld_a".into(), world_name: "A".into(), earns: Some(1.0), until_ms: u64::MAX, for_ms: NOTICE_MS };
        let mut told = Told::default();
        assert_eq!(notice_pressed(&Command::NoticeTake { id: 9 }, Some(&notice), &mut told, "ada"), Pressed::Take);
        assert_eq!(notice_pressed(&Command::NoticeTake { id: 8 }, Some(&notice), &mut told, "ada"), Pressed::Nothing);
        assert_eq!(notice_pressed(&Command::NoticeTake { id: 9 }, None, &mut told, "ada"), Pressed::Nothing);
        assert_eq!(notice_pressed(&Command::NoticeHeld { id: 9 }, Some(&notice), &mut told, "ada"), Pressed::Nothing);
    }

    fn item(id: u64, public: bool, grants: &[&str]) -> Item {
        Item {
            id,
            public,
            mine: true,
            grants: grants.iter().map(|g| Grant { vrchat_user_id: (*g).into(), ..Default::default() }).collect(),
            ..Default::default()
        }
    }

    /// A share is done only when Orbiters' answer shows it (verify-core 4): an empty 2xx, a public share answered
    /// private, a grant missing, a request instead are no success.
    #[test]
    fn a_share_is_done_only_when_the_answer_shows_it() {
        let done = |o: Outcome, public, people: &[&str]| verified(&o, public, &people.iter().map(|p| p.to_string()).collect::<Vec<_>>(), Some(1.0));
        assert!(matches!(done(Outcome::Shared(Shared::Item { explorer: None, item: item(5, true, &[]) }), true, &[]), Step::Done(_)));
        assert!(matches!(done(Outcome::Shared(Shared::Item { explorer: None, item: item(0, true, &[]) }), true, &[]), Step::Failed(_)), "{{\"item\":{{}}}}");
        assert!(matches!(done(Outcome::Shared(Shared::Item { explorer: None, item: item(5, false, &[]) }), true, &[]), Step::Failed(_)), "not public");
        assert!(matches!(done(Outcome::Shared(Shared::Item { explorer: None, item: item(5, false, &["usr_a", "usr_b"]) }), false, &["usr_a", "usr_b"]), Step::Done(Done { people: 2, .. })));
        assert!(matches!(done(Outcome::Shared(Shared::Item { explorer: None, item: item(5, false, &["usr_a"]) }), false, &["usr_a", "usr_b"]), Step::Failed(_)), "a grant missing");
        assert!(matches!(done(Outcome::Shared(Shared::Request { request: ShareRequest::default() }), true, &[]), Step::Failed(_)));
        assert_eq!(done(Outcome::Failed { message: "Sign in".into(), explicit: false }, true, &[]), Step::Failed("Sign in".into()));
        assert_eq!(done(Outcome::Failed { message: String::new(), explicit: true }, true, &[]), Step::Explicit);
    }

    /// One answer each from a server on this computer.
    fn serve(answers: Vec<(u16, &'static str)>) -> orbiters_core::account::Api {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api = orbiters_core::account::Api::at(&format!("http://{}", listener.local_addr().unwrap()));
        std::thread::spawn(move || {
            for (status, body) in answers {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let mut buf = [0u8; 8192];
                let mut got = Vec::new();
                // The head, then the body its length says.
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&got).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                        if got.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let reply = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        api
    }

    const ITEM: &str = r#"{"type":"capture","id":5,"public":true,"mine":true,"canShare":true,"grants":[],"world":{"id":"wrld_x","name":"W"},"owner":{"id":1,"name":"o"},"createdAt":"2026-11-19T00:00:00Z"}"#;

    fn answer(item: &str, explorer: Option<(i64, i64, Option<i64>, Option<&str>, Option<(&str, u32)>)>) -> &'static str {
        let explorer = explorer.map_or("null".to_string(), |(half, earned_half, before_half, reached, next)| {
            let points = half as f64 / 2.0;
            let title = if points >= 10.0 { r#"{"key":"explorer","points":10,"name":"Explorer"}"# } else { "null" };
            let reached = reached.map_or("null".to_string(), |k| format!(r#"{{"key":"{k}","points":10,"name":"{k}"}}"#));
            let next = next.map_or("null".to_string(), |(k, p)| format!(r#"{{"key":"{k}","points":{p},"name":"{k}","remaining":{}}}"#, p as f64 - points));
            let before = before_half.map_or("null".to_string(), |b| (b as f64 / 2.0).to_string());
            format!(r#"{{"points":{points},"halfPoints":{half},"title":{title},"next":{next},"titles":[],"before":{before},"earned":{},"titleReached":{reached}}}"#, earned_half as f64 / 2.0)
        });
        Box::leak(format!(r#"{{"item":{item},"explorer":{explorer}}}"#).into_boxed_str())
    }

    fn share(api: &orbiters_core::account::Api) -> anyhow::Result<Shared> {
        let request = orbiters_core::shares::Share { public: true, recipients: Vec::new(), acknowledge_explicit: false, message: String::new() };
        api.share_capture("t", orbiters_core::shares::Kind::Capture, 5, &request)
    }

    /// What a public share earned is what ITS answer says, against a server: a first picture of a world reaching
    /// Explorer, a half point, a world with three (nothing, and why), a total below zero, an answer whose points could not
    /// be read (done, said without points), a refusal, an empty 2xx.
    #[test]
    fn a_public_share_says_what_its_own_answer_earned() {
        let api = serve(vec![
            (200, answer(ITEM, Some((21, 2, Some(19), Some("explorer"), Some(("navigator", 50)))))),
            (200, answer(ITEM, Some((25, 1, Some(24), None, Some(("navigator", 50)))))),
            (200, answer(ITEM, Some((-2, 0, Some(-2), None, Some(("explorer", 10)))))),
            (200, answer(ITEM, None)),
            (403, r#"{"error":"Sign in to Orbiters in Settings first."}"#),
            (200, r#"{"item":{}}"#),
        ]);
        let step = |r: anyhow::Result<Shared>, would| match r {
            Ok(shared) => verified(&Outcome::Shared(shared), true, &[], would),
            Err(e) => verified(&Outcome::Failed { message: e.to_string(), explicit: false }, true, &[], would),
        };
        let Step::Done(first) = step(share(&api), Some(1.0)) else { panic!("first") };
        assert_eq!((first.earned, first.reached.as_ref().map(|t| t.key.as_str()), first.after.as_ref().map(|a| a.points)), (Some(1.0), Some("explorer"), Some(10.5)));
        let Step::Done(half) = step(share(&api), Some(0.5)) else { panic!("half") };
        assert_eq!((half.earned, half.reached.is_none(), half.nothing), (Some(0.5), true, None));
        let Step::Done(full) = step(share(&api), Some(0.0)) else { panic!("full") };
        assert_eq!((full.earned, full.nothing, full.after.as_ref().map(|a| a.points)), (Some(0.0), Some(Nothing::WorldHasThree), Some(-1.0)));
        let Step::Done(unread) = step(share(&api), Some(1.0)) else { panic!("unread") };
        assert_eq!((unread.earned, unread.after.is_none(), unread.nothing), (None, true, None));
        assert_eq!(step(share(&api), Some(1.0)), Step::Failed("Sign in to Orbiters in Settings first.".into()));
        assert!(matches!(step(share(&api), Some(1.0)), Step::Failed(_)), "an empty answer is no success");
        orbiters_i18n::in_language(orbiters_i18n::Language::En, || {
            assert_eq!(result_words(&first), "Shared publicly · +1 Explorer point · New title: Explorer");
            assert_eq!(result_words(&half), "Shared publicly · +0.5 Explorer points · 12.5 points · 37.5 to Navigator");
            assert_eq!(result_words(&full), "Shared publicly · No points: this world already has three public 3D pictures · -1 points · 11 to Explorer");
            assert_eq!(result_words(&unread), "Shared publicly");
        });
        assert_eq!(why_nothing(Some(1.0), false), Nothing::Unknown, "nothing earned without a known reason: said plainly");
        assert_eq!(why_nothing(Some(1.0), true), Nothing::SharedBefore);
    }

    /// The daily limit is said plainly at once; "not ready" is asked again once by itself, then said; other failures
    /// are as answered. The refusals are recognised from the errors the share returns.
    #[test]
    fn refusals_are_said_their_own_way_and_not_ready_is_retried_once() {
        use crate::controller::shares::{refusal, Refusal};
        assert_eq!(after_refusal(Some(Refusal::DailyLimit), false), AfterRefusal::Limited);
        assert_eq!(after_refusal(Some(Refusal::NotReady), false), AfterRefusal::RetryOnce);
        assert_eq!(after_refusal(Some(Refusal::NotReady), true), AfterRefusal::NotReady);
        assert_eq!(after_refusal(None, false), AfterRefusal::AsAnswered);
        let limit: anyhow::Error = orbiters_core::shares::ExplorerDailyLimit("x".into()).into();
        let not_ready: anyhow::Error = orbiters_core::account::ApiError { status: 503, message: "x".into(), code: None }.into();
        let forbidden: anyhow::Error = orbiters_core::account::ApiError { status: 403, message: "x".into(), code: None }.into();
        let rate: anyhow::Error = orbiters_core::shares::RateLimited { retry_after_s: 60, message: "x".into() }.into();
        assert_eq!([&limit, &not_ready, &forbidden, &rate].map(|e| refusal(e)), [Some(Refusal::DailyLimit), Some(Refusal::NotReady), None, None]);
        for language in orbiters_i18n::Language::TRANSLATED {
            orbiters_i18n::in_language(language, || {
                for words in [LIMIT_TITLE, LIMIT_WHY, NOT_READY] {
                    assert_ne!(tr(words), words, "{language:?}");
                }
            });
        }
    }

    #[test]
    fn a_picture_s_identity_is_its_folder_s_name() {
        assert_eq!(offer_id("C:/Sessions/2026-11-19_10-00-00"), offer_id(r"D:\elsewhere\2026-11-19_10-00-00"));
        assert_ne!(offer_id("C:/Sessions/2026-11-19_10-00-00"), offer_id("C:/Sessions/2026-11-19_10-00-01"));
    }

    #[test]
    fn titles_and_points_are_in_every_language() {
        for language in orbiters_i18n::Language::TRANSLATED {
            orbiters_i18n::in_language(language, || {
                for key in ["explorer", "navigator", "discoverer", "multiverseTraveler"] {
                    let t = Title { key: key.into(), points: 0.0, name: "server".into() };
                    assert!(title_name(&t) != "server" && !title_name(&t).is_empty());
                }
                assert_eq!(title_name(&Title { key: "marcoPolo".into(), points: 0.0, name: String::new() }), "Marco polo", "kept as Enzo wrote it");
                assert_eq!(title_name(&Title { key: "newOne".into(), points: 0.0, name: "From the server".into() }), "From the server");
                for words in [NOTICE_TITLE, TAKE, EARNED_ONE, NONE_WORLD_FULL, EXPLORER_POINTS, NOTICES_SWITCH] {
                    assert_ne!(tr(words), words, "{language:?}");
                }
            });
        }
    }

    fn here(key: &str, taken: u64, shareable: bool, public: bool) -> HereCapture {
        HereCapture { key: key.into(), world_name: "Sample world".into(), taken, shareable, public }
    }

    /// REQUEST18 3: "Share 3D capture" is offered where this computer has a capture of the world and none of its captures
    /// is public: not without one, not once one is public; the newest that can be shared is the one offered.
    #[test]
    fn share_3d_capture_wants_a_capture_here_and_none_shared_publicly() {
        // No capture of this world here: nothing.
        assert_eq!(share_here("wrld_a", &[], false), None);
        // Captures, none public: the newest that can be shared (one cut short, with nothing to make a view from, is passed by).
        let some = [here("S/old", 10, true, false), here("S/new", 30, true, false), here("S/cut", 40, false, false)];
        let offered = share_here("wrld_a", &some, false).expect("offered");
        assert_eq!((offered.key.as_str(), offered.world_id.as_str(), offered.last.clone()), ("S/new", "wrld_a", None));
        // One of them public already: nothing, whichever it is.
        let one_public = [here("S/old", 10, true, true), here("S/new", 30, true, false)];
        assert_eq!(share_here("wrld_a", &one_public, false), None);
        // A public share confirmed in this run, before the records say it: nothing.
        assert_eq!(share_here("wrld_a", &some, true), None);
        // Only captures that cannot become a 3D view: nothing to share.
        assert_eq!(share_here("wrld_a", &[here("S/cut", 40, false, false)], false), None);
    }

    /// What the tile weighs is read from the sessions' manifests and this account's upload records: only this world's
    /// captures, public as the records say (by the folder, wherever the library was).
    #[test]
    fn captures_of_a_world_are_read_from_their_manifests_and_the_upload_records() {
        use orbiters_core::contracts::manifest::{SessionManifest, WorldInfo, MANIFEST_FILE};
        let root = std::env::temp_dir().join(format!("orbiters-share-here-{}", uuid::Uuid::new_v4().simple()));
        let session = |name: &str, world: &str, created: &str, depth: u64, keyframes: bool| {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            if keyframes {
                std::fs::create_dir_all(dir.join("keyframes")).unwrap();
            }
            let mut m = SessionManifest { format: "orbiters.session".into(), created_utc: created.into(), ..Default::default() };
            m.world = Some(WorldInfo { id: Some(world.into()), name: Some("Sample world".into()), ..Default::default() });
            m.stats.frames = 100;
            m.stats.keyframes = 10;
            m.stats.depth_frames = depth;
            std::fs::write(dir.join(MANIFEST_FILE), serde_json::to_vec(&m).unwrap()).unwrap();
            dir
        };
        let a = session("2026-11-20_10-00-00", "wrld_a", "2026-11-20T10:00:00Z", 100, true);
        let b = session("2026-11-20_11-00-00", "wrld_a", "2026-11-20T11:00:00Z", 100, true);
        session("2026-11-20_12-00-00", "wrld_b", "2026-11-20T12:00:00Z", 100, true);
        let no_depth = session("2026-11-20_13-00-00", "wrld_a", "2026-11-20T13:00:00Z", 0, true);
        let mut found = captures_of(&root, "wrld_a", &[]);
        found.sort_by_key(|c| c.taken);
        assert_eq!(found.iter().map(|c| (c.key.clone(), c.shareable, c.public)).collect::<Vec<_>>(), [
            (a.to_string_lossy().into_owned(), true, false),
            (b.to_string_lossy().into_owned(), true, false),
            (no_depth.to_string_lossy().into_owned(), false, false),
        ]);
        assert_eq!(share_here("wrld_a", &found, false).map(|h| h.key), Some(b.to_string_lossy().into_owned()));
        // Shared to friends only: still offered. Shared publicly (recorded where the library was before): not.
        let friends = Item { id: 5, public: false, ..Default::default() };
        assert!(share_here("wrld_a", &captures_of(&root, "wrld_a", &[(a.clone(), friends)]), false).is_some());
        let elsewhere = std::path::PathBuf::from("D:/Old library").join(a.file_name().unwrap());
        let public = Item { id: 6, public: true, ..Default::default() };
        assert_eq!(share_here("wrld_a", &captures_of(&root, "wrld_a", &[(elsewhere, public)]), false), None);
        let _ = std::fs::remove_dir_all(root);
    }
}
