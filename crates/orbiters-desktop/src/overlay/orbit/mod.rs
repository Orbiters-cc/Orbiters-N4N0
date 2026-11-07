//! The Orbiters menu around VRChat's own menus: a second ring around the Action Menu's wheel and one wing docked to the Quick Menu, showing as they open and gone as they close, so that
//! world search, YouTube, previous instances, what plays and 3D pictures are right where the person already is (docs/orbit-overlays.md).
//!
//! The same parts are drawn for both modes: a borderless topmost window each in desktop mode ([`desktop`]), a SteamVR overlay each in VR ([`vr`]). What is common is here: [`Panels`] paints
//! them and does what they are pressed for; [`zones`] says where they go, [`panels`] what they hold, [`detect`] how a menu is known to be open on the desktop, [`block`] how the trigger
//! is taken from VRChat in VR.
//!
//! **Nothing here touches the game**: no injection, no reading of its memory, ordinary windows and overlays, its own window's picture through Windows Graphics Capture only while a menu is
//! showing or a key has just asked for one. Closed, nothing is painted, and no window or overlay shows.

pub mod block;
pub mod detect;
#[cfg(windows)]
pub mod desktop;
pub mod panels;
pub mod vr;
pub mod zones;

use super::menu_ui::{self, Skin, View};
use super::menu_retro;
use super::worlds::Worlds;
use super::Canvas;
use crate::backend::Ctl;
use crate::ipc::N4n0Command;
use crate::menu_host::MenuHost;
use crate::state::{CaptureState, Snapshot};
use egui::{pos2, vec2, ColorImage, Pos2, TextureHandle, TextureId, TextureOptions, Vec2};
use orbiters_core::settings::MenuLook;
use orbiters_n4n0_core::menu::layout::{self, ScreenPage};
use orbiters_n4n0_core::menu::{Effect, Page, Target};
use panels::{Action, Facts, Item, Recent, Sheet};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use zones::{Around, Group, Placed, Zone};

/// Whether the desktop's panels are wanted: VRChat runs without SteamVR, on this computer, and the setting says so.
pub fn wanted_on_desktop(snap: &Snapshot) -> bool {
    orbiters_core::osc::VRCHAT_RUNS_HERE && snap.vrchat.running && !snap.vrchat.remote && !snap.steamvr.running && snap.settings.orbit.any()
}

/// N4N0's pages in Orbiters' own look: the design system's, as everywhere else in the app.
pub fn skin() -> &'static Skin {
    Skin::english()
}

/// A part's appearing: a spring from nothing to its place, a little past it. Each turn moves the window or overlay and its alpha, no new picture.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Appear {
    position: f32,
    velocity: f32,
    /// Seconds before it starts.
    wait: f32,
}

/// The spring's stiffness and damping: settled in about 300 ms, about 6% past its place on the way.
const SPRING: (f32, f32) = (320.0, 24.0);
/// How far a part comes from, toward the menu's centre, in units.
pub const APPEAR_FROM: f32 = 26.0;
/// Between one part's start and the next's: the bloom goes round the wheel.
const APPEAR_STAGGER: f32 = 0.022;

impl Appear {
    pub fn new(order: usize, live: bool) -> Self {
        if live {
            Self { position: 0.0, velocity: 0.0, wait: order as f32 * APPEAR_STAGGER }
        } else {
            Self { position: 1.0, velocity: 0.0, wait: 0.0 }
        }
    }

    pub fn step(&mut self, dt: f32) {
        let mut dt = dt.min(0.05);
        if self.wait > 0.0 {
            let used = self.wait.min(dt);
            self.wait -= used;
            dt -= used;
        }
        // Small steps: a turn can be long, and the spring must not blow up.
        while dt > 0.0 {
            let h = dt.min(0.004);
            self.velocity += (SPRING.0 * (1.0 - self.position) - SPRING.1 * self.velocity) * h;
            self.position += self.velocity * h;
            dt -= h;
        }
    }

    pub fn settled(&self) -> bool {
        self.wait <= 0.0 && (self.position - 1.0).abs() < 0.002 && self.velocity.abs() < 0.02
    }

    /// Its place along the way: 0 far, 1 home, a little over in between.
    pub fn position(&self) -> f32 {
        if self.settled() {
            1.0
        } else {
            self.position
        }
    }

    pub fn alpha(&self) -> f32 {
        (self.position() * 1.6).clamp(0.0, 1.0)
    }
}


/// One part: where it is and its picture.
pub struct Panel {
    pub zone: Zone,
    pub placed: Placed,
    /// Its picture's size in pixels.
    pub px: [u32; 2],
    /// Its size in the units its content is drawn in.
    pub logical: Vec2,
    canvas: Canvas,
    /// The picture as the platform takes it: the canvas's, cut to its shape.
    out: Vec<u8>,
    /// Its shape as a cell of the ring, one byte a pixel, multiplied into each picture.
    mask: Option<Vec<u8>>,
    pub items: Vec<Item>,
    pub hover: Option<usize>,
    pub pressed: Option<usize>,
    /// The next paint must draw.
    pub dirty: bool,
    pub appear: Appear,
    /// A shown picture: the platform has it.
    pub painted: bool,
    /// It shows now (a second ring's cell only while it is bloomed, the veil only while the mouse points here).
    pub shown: bool,
    /// Textures of pictures, by name.
    pictures: HashMap<String, TextureHandle>,
}

impl Panel {
    /// `sharp` pixels of picture a pixel of the placed rectangle (1 on the desktop; more in the headset, whose overlays are looked at closely).
    fn new(placed: Placed, sharp: f32, order: usize, live: bool) -> Self {
        let size = placed.rect.size() * sharp;
        let px = [size.x.round().max(1.0) as u32, size.y.round().max(1.0) as u32];
        let logical = if placed.zone == Zone::Takeover { vec2(layout::WIDTH as f32, layout::HEIGHT as f32) } else { placed.logical() };
        let zoom = px[0] as f32 / logical.x;
        let mask = placed.zone.ring().map(|ring| panels::cell_mask(ring, px, zoom));
        let shown = placed.zone.group().is_none() && placed.zone != Zone::WheelVeil;
        Self {
            zone: placed.zone,
            placed,
            px,
            logical,
            canvas: Canvas::zoomed(px[0], px[1], zoom),
            out: vec![0; px[0] as usize * px[1] as usize * 4],
            mask,
            items: Vec::new(),
            hover: None,
            pressed: None,
            dirty: true,
            appear: Appear::new(order, live),
            painted: false,
            shown,
            pictures: HashMap::new(),
        }
    }

    /// The picture (RGBA, premultiplied), as painted last.
    pub fn pixels(&self) -> &[u8] {
        &self.out
    }

    /// Takes what the canvas painted: the changed part, cut to the panel's shape. Returns it (x0, y0, x1, y1).
    fn take_picture(&mut self) -> Option<[u32; 4]> {
        let changed = self.canvas.damage()?;
        let [w, h] = self.px;
        let pixels = self.canvas.pixels();
        for y in changed[1]..changed[3].min(h) {
            let (a, b) = ((y * w + changed[0]) as usize * 4, (y * w + changed[2].min(w)) as usize * 4);
            self.out[a..b].copy_from_slice(&pixels[a..b]);
        }
        if let Some(mask) = &self.mask {
            panels::apply_mask(&mut self.out, mask, self.px, changed);
        }
        Some(changed)
    }

    /// How far from its place it is now, in units along the way out from the menu's centre: its spring coming, and the pointer's pop.
    pub fn outward(&self) -> f32 {
        -(1.0 - self.appear.position()) * APPEAR_FROM
    }

    /// `uv` over the panel (0 to 1 from its top left) in the units its content is drawn in.
    pub fn to_logical(&self, uv: [f32; 2]) -> Pos2 {
        pos2(uv[0] * self.logical.x, uv[1] * self.logical.y)
    }
}

/// How long a second ring stays open after the pointer left it and its cell: a hand crossing the gap must not close it.
pub const BLOOM_GRACE: Duration = Duration::from_millis(350);

/// Which second ring is open: the pointer on a cell that blooms opens it at once; it stays while the pointer is on it or its cell, and a moment after.
#[derive(Clone, Copy, Debug, Default)]
pub struct Bloom {
    open: Option<Group>,
    left: Option<Instant>,
}

impl Bloom {
    pub fn open(&self) -> Option<Group> {
        self.open
    }

    /// The pointer is on `on` now.
    pub fn step(&mut self, on: Option<Zone>, now: Instant) -> Option<Group> {
        let wanted = on.and_then(|z| z.blooms().or(z.group()));
        if let Some(group) = wanted {
            self.open = Some(group);
            self.left = None;
        } else if self.open.is_some() {
            match self.left {
                Some(at) if now.duration_since(at) >= BLOOM_GRACE => {
                    self.open = None;
                    self.left = None;
                }
                None => self.left = Some(now),
                _ => {}
            }
        }
        self.open
    }
}

/// What a press asks of the platform beyond the service.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pressed {
    /// A press happened (the click's sound).
    pub clicked: bool,
    /// The parts go for this opening.
    pub hide: bool,
    /// In VR: the 3D worlds panel, which lives with N4N0's menu there.
    pub worlds: bool,
}

/// Every capture, the newest first, listed off the folder on a thread of its own as listing takes a moment, once for each change of the library. Their pictures come one by one as the gallery
/// reaches them, and go again once it is far past them: it can hold any number of captures.
#[derive(Default)]
struct Recents {
    /// All there is, newest first.
    loaded: Vec<LoadedRecent>,
    /// What the gallery shows of it, in its filter, and that as tiles.
    shown_index: Vec<usize>,
    shown: panels::Gallery,
    arranged: Option<(panels::GalleryFilter, orbiters_core::settings::GalleryLayout)>,
    seen: Option<(u64, u64)>,
    /// When it was last listed: a first scan of the pictures says it found more all the time.
    listed: Option<Instant>,
    rx: Option<mpsc::Receiver<Vec<LoadedRecent>>>,
    /// Those whose picture was asked for.
    asked: Vec<usize>,
    /// What reads pictures: what to read, newest last, and what it read.
    reader: Option<(PicturesAsked, mpsc::Receiver<(crate::worlds::Picture, Option<ColorImage>)>)>,
    /// The picture shown large, as it is asked for, and it when it came.
    big: Option<(crate::worlds::Picture, Option<ColorImage>)>,
}

type PicturesAsked = std::sync::Arc<(std::sync::Mutex<Vec<crate::worlds::Picture>>, std::sync::Condvar)>;

/// How many threads read pictures. Reading VRChat's for the first time means decoding their PNGs.
const PICTURE_READERS: usize = 2;
/// How wide a picture is read to show large: what the app's own window reads.
const LARGE_PICTURE: u32 = 2048;
/// How often the list is read again while there is news of it.
const LIST_AGAIN: Duration = Duration::from_secs(5);

struct LoadedRecent {
    /// Its session's folder, or its file.
    key: PathBuf,
    title: String,
    image: Option<ColorImage>,
    asked: bool,
    has_view: bool,
    /// A capture, and of those a 3D picture, not one of VRChat's pictures.
    session: bool,
    picture: bool,
    taken: u64,
    modified: u64,
    source: crate::worlds::Picture,
    aspect: f32,
    /// Its place among the tiles; none when its filter hides it.
    at: Option<usize>,
}

impl LoadedRecent {
    fn kind(&self) -> panels::RecentKind {
        match (self.session, self.has_view) {
            (true, true) => panels::RecentKind::Capture,
            (true, false) => panels::RecentKind::NoView,
            (false, _) => panels::RecentKind::Picture,
        }
    }
}

/// All the pictures and captures on this computer, as the app's Photos & captures lists them, newest first.
fn list_pictures(sessions: &std::path::Path, folders: &[PathBuf]) -> Vec<LoadedRecent> {
    use crate::worlds::Picture;
    let mut list = Vec::new();
    for world in crate::worlds::load_gallery(sessions, folders) {
        let title = if world.name.is_empty() { "A VRChat world".to_string() } else { world.name.clone() };
        for s in &world.sessions {
            let aspect = crate::controller::shares::session_picture_size(s.manifest.capture.width, s.manifest.capture.height).map_or(16.0 / 9.0, |[w, h]| w as f32 / h.max(1) as f32);
            list.push(LoadedRecent {
                key: s.dir.clone(),
                title: title.clone(),
                image: None,
                asked: false,
                has_view: s.reconstruction.is_some(),
                session: true,
                picture: s.picture.is_some(),
                taken: crate::controller::shares::unix(&s.manifest.created_utc),
                modified: s.modified,
                source: Picture::Session(s.dir.clone()),
                aspect,
                at: None,
            });
        }
        for shot in &world.shots {
            let aspect = if shot.info.width > 0 && shot.info.height > 0 { shot.info.width as f32 / shot.info.height as f32 } else { 16.0 / 9.0 };
            list.push(LoadedRecent {
                key: shot.path.clone(),
                title: title.clone(),
                image: None,
                asked: false,
                has_view: false,
                session: false,
                picture: false,
                taken: shot.info.taken,
                modified: shot.info.taken,
                source: Picture::Shot { path: shot.path.clone(), size: shot.size, width: crate::worlds::THUMB_WIDTH },
                aspect,
                at: None,
            });
        }
    }
    list.sort_by_key(|r| std::cmp::Reverse(r.taken));
    list
}

impl Recents {
    /// Takes a new list when one is ready, and asks for one when `version` says there is news. Says whether it changed.
    fn follow(&mut self, version: (u64, u64), sessions: PathBuf, folders: Vec<PathBuf>) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.rx {
            if let Ok(mut list) = rx.try_recv() {
                // What was read already stays.
                let mut had: HashMap<crate::worlds::Picture, Option<ColorImage>> = HashMap::new();
                for i in &self.asked {
                    if let Some(old) = self.loaded.get_mut(*i) {
                        had.insert(old.source.clone(), old.image.take());
                    }
                }
                self.asked.clear();
                for (i, new) in list.iter_mut().enumerate() {
                    if let Some(image) = had.remove(&new.source) {
                        (new.image, new.asked) = (image, true);
                        self.asked.push(i);
                    }
                }
                self.loaded = list;
                self.arranged = None;
                self.rx = None;
                changed = true;
            }
        }
        if self.rx.is_none() && self.seen != Some(version) && self.listed.is_none_or(|t| t.elapsed() >= LIST_AGAIN) {
            self.seen = Some(version);
            self.listed = Some(Instant::now());
            let (tx, rx) = mpsc::channel();
            self.rx = Some(rx);
            let _ = std::thread::Builder::new().name("orbit-recent".into()).spawn(move || {
                let _ = tx.send(list_pictures(&sessions, &folders));
            });
        }
        changed
    }

    /// Makes the tiles again when the list, its filter or its layout changed. Says whether they did.
    fn arrange(&mut self, filter: panels::GalleryFilter, layout: orbiters_core::settings::GalleryLayout) -> bool {
        if self.arranged == Some((filter, layout)) {
            return false;
        }
        self.arranged = Some((filter, layout));
        self.shown_index.clear();
        let mut items = Vec::new();
        for (i, r) in self.loaded.iter_mut().enumerate() {
            r.at = None;
            if filter == panels::GalleryFilter::All || r.session {
                r.at = Some(self.shown_index.len());
                self.shown_index.push(i);
                items.push(Recent { dir: r.key.clone(), title: r.title.clone(), state: String::new(), progress: None, picture: None, aspect: r.aspect, kind: r.kind() });
            }
        }
        self.shown = panels::Gallery::new(items, layout);
        true
    }

    /// Tile `n`.
    fn tile(&self, n: usize) -> Option<&LoadedRecent> {
        self.shown_index.get(n).and_then(|i| self.loaded.get(*i))
    }

    /// The pictures that came. Says whether any did.
    fn pictures_came(&mut self) -> bool {
        let Some((_, got)) = &self.reader else { return false };
        let mut any = false;
        while let Ok((picture, image)) = got.try_recv() {
            if let Some(big) = self.big.as_mut().filter(|b| b.0 == picture) {
                big.1 = image;
                any = true;
            } else if let Some(i) = self.asked.iter().copied().find(|i| self.loaded.get(*i).is_some_and(|r| r.source == picture)) {
                self.loaded[i].image = image;
                any = true;
            }
        }
        any
    }

    /// What reads pictures, started the first time. It reads what was asked last first: what is on the page now.
    fn reader(&mut self) -> &PicturesAsked {
        &self
            .reader
            .get_or_insert_with(|| {
                let asked: PicturesAsked = Default::default();
                let (give, got) = mpsc::channel();
                for _ in 0..PICTURE_READERS {
                    let (asked, give) = (asked.clone(), give.clone());
                    let _ = std::thread::Builder::new().name("orbit-pictures".into()).spawn(move || loop {
                        let picture = {
                            let mut queue = asked.0.lock().unwrap_or_else(|e| e.into_inner());
                            loop {
                                if let Some(picture) = queue.pop() {
                                    break picture;
                                }
                                // Those here and the readers alone are left: this is over.
                                if std::sync::Arc::strong_count(&asked) <= PICTURE_READERS {
                                    return;
                                }
                                queue = asked.1.wait_timeout(queue, Duration::from_secs(1)).unwrap_or_else(|e| e.into_inner()).0;
                            }
                        };
                        let image = crate::worlds::decode_local(&picture);
                        if give.send((picture, image)).is_err() {
                            return;
                        }
                    });
                }
                (asked, got)
            })
            .0
    }

    /// Asks for the pictures of the tiles from `first` to `last` not here yet.
    fn ask(&mut self, first: usize, last: usize) {
        let new: Vec<usize> = self.shown_index.iter().skip(first).take(last.saturating_sub(first) + 1).copied().filter(|i| !self.loaded[*i].asked).collect();
        if new.is_empty() {
            return;
        }
        let pictures: Vec<_> = new.iter().rev().map(|i| self.loaded[*i].source.clone()).collect();
        for i in new {
            self.loaded[i].asked = true;
            self.asked.push(i);
        }
        let asked = self.reader().clone();
        asked.0.lock().unwrap_or_else(|e| e.into_inner()).extend(pictures);
        asked.1.notify_all();
    }

    /// Lets go of the pictures of the tiles outside `first` to `last`, and of asking for them.
    fn keep(&mut self, first: usize, last: usize) {
        let before = self.asked.len();
        let loaded = &mut self.loaded;
        self.asked.retain(|i| {
            let Some(recent) = loaded.get_mut(*i) else { return false };
            let stays = recent.at.is_some_and(|at| at >= first && at <= last);
            if !stays {
                recent.image = None;
                recent.asked = false;
            }
            stays
        });
        if self.asked.len() != before {
            let wanted: std::collections::HashSet<&crate::worlds::Picture> = self.asked.iter().filter_map(|i| self.loaded.get(*i)).map(|r| &r.source).chain(self.big.iter().map(|b| &b.0)).collect();
            if let Some((asked, _)) = &self.reader {
                asked.0.lock().unwrap_or_else(|e| e.into_inner()).retain(|p| wanted.contains(p));
            }
        }
    }

    /// Shows the picture of tile `n` large.
    fn open_big(&mut self, n: usize) {
        let Some(crate::worlds::Picture::Shot { path, size, .. }) = self.tile(n).map(|r| r.source.clone()) else { return };
        let picture = crate::worlds::Picture::Shot { path, size, width: LARGE_PICTURE };
        self.big = Some((picture.clone(), None));
        let asked = self.reader().clone();
        asked.0.lock().unwrap_or_else(|e| e.into_inner()).push(picture);
        asked.1.notify_all();
    }
}

/// How long ago, in a word.
fn ago(seconds: u64) -> String {
    match seconds {
        0..=89 => "just now".into(),
        90..=3599 => format!("{} min ago", seconds / 60),
        3600..=86399 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86400),
    }
}

/// What a recent capture reads as.
fn recent_state(r: &LoadedRecent, snap: &Snapshot, now: u64) -> String {
    if !r.session {
        return ago(now.saturating_sub(r.taken));
    }
    let kind = if r.picture { "3D picture" } else { "Recording" };
    if let Some(job) = snap.reconstruction.as_ref().filter(|j| j.session_dir == r.key) {
        return format!("Making 3D {:.0}%", job.progress.clamp(0.0, 1.0) * 100.0);
    }
    if snap.reconstruct_queue.contains(&r.key) {
        return format!("{kind}, waiting");
    }
    if r.has_view {
        format!("{kind}, {}", ago(now.saturating_sub(r.taken)))
    } else {
        format!("{kind}, no 3D yet")
    }
}

/// What the sheet's page is drawn on: its canvas, and what draws there, as their textures are that canvas's own. Made anew when the window's size changes the canvas's.
struct Stage {
    canvas: Canvas,
    host: MenuHost,
    worlds: Option<Worlds>,
    px: [u32; 2],
    retro: bool,
}

impl Stage {
    fn new(px: [u32; 2], with_worlds: bool) -> Self {
        let canvas = Canvas::zoomed(px[0], px[1], px[0] as f32 / layout::WIDTH as f32);
        let host = MenuHost::new(canvas.ctx().clone());
        let worlds = with_worlds.then(|| Worlds::alone(canvas.ctx().clone()));
        Self { canvas, host, worlds, px, retro: false }
    }
}

/// The parts of whichever menu is open, and what they do. The platform says what is open, where the pointer is and when it presses; it takes the pictures.
pub struct Panels {
    backend: Ctl,
    stage: Stage,
    list: Vec<Panel>,
    /// What stopped a 3D picture, said for a moment.
    note: PictureNote,
    /// A 3D picture is being taken: its end is said.
    was_flying: bool,
    /// Where the gallery is going to, as its wheel said.
    gallery_to: f32,
    /// What was last asked for YouTube's results' pictures, and how many there were.
    videos_asked: Option<std::ops::Range<usize>>,
    videos_had: usize,
    /// How many lines the conversation had.
    talk_had: usize,
    /// What play or pause was last asked, and when; the place held along what plays; where it was let go; where it was moved to, and when.
    play_asked: Option<(bool, Instant)>,
    seeking: Option<f32>,
    seek_late: Option<f32>,
    seek_asked: Option<(u32, Instant)>,
    /// Captures of this computer's gallery asked to show in 3D, for the view that shows them.
    local_asked: Vec<super::worlds::Relayed>,
    /// Lists give at their ends and come back: not with Reduce motion.
    elastic: bool,
    /// How fast the list was thrown, in its units a second.
    throw: f32,
    /// The invite asked for last, and when; the result whose link was copied, and when.
    invited: Option<(String, Instant)>,
    copied: Option<(usize, Instant)>,
    /// A press on a list, which may yet be a drag; where the pointer is on the sheet; and what it turned out to press.
    drag: Option<Drag>,
    sheet_pointer: Option<Pos2>,
    pressed_late: Option<Action>,
    /// The Instances sheet's worlds' pictures so far.
    visit_pictures: HashMap<String, egui::TextureId>,
    /// The page on the sheet (its back is in `list`).
    pub takeover: Option<Panel>,
    open: Option<Around>,
    recents: Recents,
    pub facts: Facts,
    /// Pixels of picture a placed pixel.
    sharp: f32,
    /// World search is the platform's in VR (where its 3D view lives), here on the desktop.
    worlds_here: bool,
    /// The person's Hide: nothing shows for this opening.
    hidden: bool,
    /// Talk is held.
    talking: bool,
    started: Instant,
    bloom: Bloom,
    /// Where the sheet's back and its page go.
    pub back_place: Option<Placed>,
    pub hint_place: Option<Placed>,
    pub takeover_place: Option<Placed>,
    /// On the desktop's search page the on-screen keyboard was asked for.
    show_keys: bool,
    /// What the service was last told of the sheet (open, dictation, keyboard).
    reported: Option<(bool, bool, bool)>,
    /// What plays's picture as last made, by its name: panels make their own textures of it.
    art: Option<(String, ColorImage)>,
}

/// Where on the page the keyboard starts (N4N0's search page: `layout::KEY_ROWS_Y`, a little over its first row).
const KEYS_TOP: f32 = 300.0;

impl Panels {
    pub fn new(backend: Ctl, sharp: f32, worlds_here: bool) -> Self {
        Self {
            backend,
            stage: Stage::new([1, 1], false),
            list: Vec::new(),
            takeover: None,
            note: PictureNote::default(),
            was_flying: false,
            gallery_to: 0.0,
            videos_asked: None,
            videos_had: 0,
            talk_had: 0,
            play_asked: None,
            seeking: None,
            seek_late: None,
            seek_asked: None,
            local_asked: Vec::new(),
            elastic: true,
            throw: 0.0,
            invited: None,
            copied: None,
            drag: None,
            sheet_pointer: None,
            pressed_late: None,
            visit_pictures: HashMap::new(),
            open: None,
            recents: Recents::default(),
            facts: Facts::default(),
            sharp,
            worlds_here,
            hidden: false,
            talking: false,
            started: Instant::now(),
            bloom: Bloom::default(),
            back_place: None,
            hint_place: None,
            takeover_place: None,
            show_keys: false,
            reported: None,
            art: None,
        }
    }

    pub fn open_menu(&self) -> Option<Around> {
        self.open
    }

    /// Nothing shows.
    pub fn is_empty(&self) -> bool {
        self.list.is_empty() && self.takeover.is_none()
    }

    /// Every part, the sheet's last: its page in front.
    pub fn all(&self) -> impl Iterator<Item = &Panel> {
        self.list.iter().chain(self.takeover.iter())
    }

    /// The sheet is open.
    pub fn sheet_open(&self) -> bool {
        self.takeover.is_some()
    }

    /// Whether a part shows now, the sheet aside: the Quick Menu's caption lies behind it.
    fn caption_shown(&self) -> bool {
        self.takeover.is_none()
    }

    /// The mouse points at the parts rather than at VRChat's wheel: the veil comes over the wheel. Nothing changes for what is not on the wheel.
    pub fn set_pointing(&mut self, pointing: bool, live: bool) {
        for panel in self.list.iter_mut().filter(|p| p.zone == Zone::WheelVeil) {
            if panel.shown != pointing {
                panel.shown = pointing;
                if pointing {
                    panel.appear = Appear::new(0, live);
                }
            }
        }
    }

    /// What is open: parts for it in the places `places` gives, or none. Hide lasts until the menu closes. Whatever was held lets go.
    pub fn set_open(&mut self, around: Option<Around>, places: Vec<Placed>, snap: &Snapshot) {
        if around != self.open {
            self.stop_talking();
            self.hidden = false;
            self.facts.pointed = None;
            self.facts.sheet = None;
            self.release();
            self.takeover = None;
            self.open = around;
            self.list.clear();
            self.bloom = Bloom::default();
            self.show_keys = false;
        }
        if self.hidden || around.is_none() {
            self.list.clear();
            self.takeover = None;
            self.facts.sheet = None;
            return;
        }
        let live = !snap.settings.reduce_motion;
        // Opened from the wheel, the sheet is all that shows: its cut-off cells would stick out from under it.
        let places: Vec<Placed> = if self.takeover.is_some() && around == Some(Around::Wheel) { Vec::new() } else { places };
        let back = self.takeover.is_some().then_some(self.back_place).flatten();
        // On the desktop VRChat lights what lies under the pointer, sheet or no sheet. Where the wing leaves the row under its tabs free.
        let band = self.hint_place.filter(|_| back.is_some() && around == Some(Around::QuickMenu) && self.facts.desktop && !places.iter().any(|p| p.zone == Zone::QmStrip));
        let places: Vec<Placed> = places.into_iter().chain(back).chain(band).collect();
        // Moved or another size: the parts of a size stay as they are, those changed are made anew.
        let unchanged = self.list.len() == places.len() && self.list.iter().zip(&places).all(|(a, b)| a.placed == *b);
        if !unchanged {
            let first = self.list.is_empty();
            let mut old: Vec<Panel> = std::mem::take(&mut self.list);
            self.list = places
                .into_iter()
                .enumerate()
                .map(|(i, placed)| {
                    if let Some(at) = old.iter().position(|p| p.zone == placed.zone && p.placed.rect.size() == placed.rect.size()) {
                        let mut kept = old.swap_remove(at);
                        kept.placed = placed;
                        kept
                    } else {
                        let mut panel = Panel::new(placed, self.sharp, i, live && first);
                        panel.appear = if first { panel.appear } else { Appear::new(0, false) };
                        panel
                    }
                })
                .collect();
        }
    }

    /// Opens `sheet` on the sheet, with `title`.
    fn open_sheet(&mut self, sheet: Sheet, title: &str, snap: &Snapshot) {
        let Some(place) = self.takeover_place else { return };
        let size = place.rect.size() * self.sharp;
        let px = [size.x.round() as u32, size.y.round() as u32];
        if self.stage.px != px || (self.worlds_here && self.stage.worlds.is_none()) {
            self.stage = Stage::new(px, self.worlds_here);
        }
        self.facts.sheet = Some(sheet);
        self.facts.sheet_title = title.to_string();
        self.facts.gallery_scroll = 0.0;
        self.gallery_to = 0.0;
        self.throw = 0.0;
        self.facts.opened = None;
        self.recents.big = None;
        // What was taken since shows when it opens.
        if sheet == Sheet::Pictures {
            self.recents.seen = None;
            self.recents.listed = None;
        }
        if self.takeover.is_none() {
            self.takeover = Some(Panel::new(place, self.sharp, 0, !snap.settings.reduce_motion));
        }
        self.stage.host.dirty = true;
        if let Some(panel) = self.takeover.as_mut() {
            panel.dirty = true;
            panel.hover = None;
            panel.items = panels::sheet_items(sheet, &self.facts);
        }
        for panel in &mut self.list {
            panel.dirty = true;
        }
        self.stage.canvas.invalidate();
    }

    /// What is being said ends with its page: nobody should be listened to with no sign of it.
    fn stop_talking(&mut self) {
        if std::mem::take(&mut self.talking) {
            self.backend.n4n0(N4n0Command::Talk { down: false });
        }
    }

    fn close_sheet(&mut self) {
        self.stop_talking();
        self.takeover = None;
        self.facts.sheet = None;
        self.facts.sheet_title.clear();
        self.list.retain(|p| !matches!(p.zone, Zone::SheetBack | Zone::SheetHint));
        for panel in &mut self.list {
            panel.dirty = true;
        }
        self.show_keys = false;
    }

    /// A page of N4N0's menu on the sheet, from its Home as the menu starts there.
    fn open_page(&mut self, target: Target, snap: &Snapshot) {
        let title = match target {
            Target::YouTube => "YouTube",
            Target::Music => "Now playing",
            Target::History => "Your captures",
            Target::Call => "Call",
            Target::States => "Saved looks",
            Target::Light => "Light",
            Target::Tracking => "Smooth tracking",
            Target::WorldFeedback => "World feedback",
            Target::Qr => "QR code",
            Target::Suggest => "Suggest",
            _ => "",
        };
        // Where the person was has a sheet of its own here, on the menu's own list and invites.
        if target == Target::Instances {
            self.open_sheet(Sheet::Instances, "Instances", snap);
        } else {
            self.open_sheet(Sheet::Menu, title, snap);
        }
        let host = &mut self.stage.host;
        let _ = host.menu.close();
        host.open(snap);
        let effects = host.press(target);
        host.effects(effects, snap, &self.backend);
    }

    /// Follows the service's state: the parts' facts, recent captures, what is on the sheet.
    /// What stopped a 3D picture: said whether a menu is up or not, as with the camera open none can be.
    pub fn follow_note(&mut self, snap: &Snapshot) {
        let now = Instant::now();
        let mut said = self.note.follow(&snap.n4n0.photo_note, now);
        // The flight's end, which nothing else says: the camera just stops.
        let flying = snap.capture.photo_3d && !matches!(snap.capture.state, CaptureState::Idle);
        if self.was_flying && !flying {
            said = Some(Some(flight_words(snap.capture.error.as_deref())));
            self.note.until = Some(now + NOTE_FOR);
        }
        self.was_flying = flying;
        if let Some(note) = said {
            self.facts.note = note;
            for panel in self.list.iter_mut().chain(self.takeover.iter_mut()) {
                panel.dirty = true;
            }
        }
    }

    /// The mouse's wheel, in notches, up positive: moves the sheet's list, when it has one. At its end it gives a little.
    pub fn wheel(&mut self, notches: f32) {
        if let Some(sheet) = self.facts.sheet.filter(|_| self.facts.opened.is_none()) {
            let max = panels::max_scroll(sheet, &self.facts);
            let to = self.gallery_to - notches * 180.0;
            self.gallery_to = panels::snap_scroll(sheet, &self.facts, to.clamp(0.0, max));
            if self.elastic && to != to.clamp(0.0, max) {
                self.throw = (to - to.clamp(0.0, max)).signum() * WHEEL_KICK;
            }
        }
    }

    /// A 3D picture waits for VRChat's camera.
    pub fn picture_waiting(&self) -> bool {
        self.facts.picture_wait.is_some()
    }

    /// The gallery's page paints again.
    fn gallery_dirty(&mut self) {
        if let Some(panel) = self.takeover.as_mut() {
            panel.dirty = true;
            panel.hover = None;
        }
        for panel in &mut self.list {
            panel.dirty = true;
        }
    }

    /// VR: the panel on the sheet relays to the overlay's own 3D worlds, whose view `status` says what it does. What it asked since last time.
    pub(in crate::overlay) fn world_relay(&mut self, status: super::worlds::panel::Status) -> Vec<super::worlds::Relayed> {
        let showing = status.current.is_some();
        // Asked from the gallery, which has no page of the view's own to say it on.
        let said = match (&status.error, &status.stage) {
            (Some(why), _) => Some(("It cannot show in 3D".to_string(), why.clone())),
            (None, Some(stage)) => Some(("Getting it ready".to_string(), stage.clone())),
            (None, None) => None,
        };
        // The sheet's back has the same hole as the page on it: page units to its own, the page lying 52 under its top.
        let hole = self.stage.worlds.as_ref().filter(|_| self.facts.sheet == Some(Sheet::Worlds)).and_then(|w| w.small_hole());
        let hole = hole.map(|h| egui::Rect::from_min_max(pos2(h.min.x * 0.6875, h.min.y * 0.6875 + panels::SHEET_HEAD), pos2(h.max.x * 0.6875, h.max.y * 0.6875 + panels::SHEET_HEAD)));
        if showing != self.facts.view_showing || said != self.facts.view_said || status.small != self.facts.view_small || hole != self.facts.sheet_hole {
            self.facts.view_showing = showing;
            self.facts.view_said = said;
            self.facts.view_small = status.small;
            self.facts.sheet_hole = hole;
            for panel in self.list.iter_mut().chain(self.takeover.iter_mut()) {
                panel.dirty = true;
            }
        }
        let mut asked = self.stage.worlds.as_mut().map_or_else(Vec::new, |worlds| worlds.relay_to(status));
        asked.append(&mut self.local_asked);
        asked
    }

    /// Where on the sheet's page a capture standing small may lie.
    pub(in crate::overlay) fn small_place_on_sheet(&self) -> Option<egui::Rect> {
        self.stage.worlds.as_ref().filter(|_| self.facts.sheet == Some(Sheet::Worlds)).and_then(|w| w.small_place_on_panel())
    }

    /// A number key on the wheel: as a press on that cell, clockwise from the top.
    pub fn press_cell(&mut self, cell: usize, snap: &Snapshot) -> Pressed {
        const CELLS: [Zone; 6] = [Zone::WheelWorlds, Zone::WheelYouTube, Zone::WheelPlaying, Zone::WheelAll, Zone::WheelInstances, Zone::WheelPictures];
        let Some(zone) = CELLS.get(cell) else { return Pressed::default() };
        for panel in self.list.iter_mut() {
            panel.hover = (panel.zone == *zone).then_some(0);
        }
        let pressed = self.press(snap);
        self.release();
        pressed
    }

    /// What to say with no menu up, in the pill over the game.
    pub fn away_words(&self, snap: &Snapshot) -> Option<(String, String)> {
        away_words(snap, &self.facts.note)
    }

    pub fn follow(&mut self, snap: &Snapshot, library: u64) {
        // Along what plays, now it was let go.
        if let Some(fraction) = self.seek_late.take() {
            let host = &mut self.stage.host;
            if let Some(of) = host.menu.media.as_ref().and_then(|m| m.duration) {
                let seconds = (fraction * of as f32) as u32;
                self.seek_asked = Some((seconds, Instant::now()));
                host.effects(vec![Effect::Media(orbiters_n4n0_core::menu::MediaCommand::Seek(seconds))], snap, &self.backend);
            }
        }
        // What a press in a list turned out to press, now it was let go without moving.
        if let Some(action) = self.pressed_late.take() {
            self.act(action, snap, &mut Pressed::default());
        }
        if self.open.is_none() {
            return;
        }
        self.elastic = !snap.settings.reduce_motion;
        // All tools shows the newest of them too.
        let in_gallery = matches!(self.facts.sheet, Some(Sheet::Pictures | Sheet::All));
        let listed = in_gallery && self.recents.follow((library, crate::worlds::picture_revision()), snap.settings.sessions_dir.clone(), snap.settings.picture_folders.clone());
        let arranged = in_gallery && self.recents.arrange(self.facts.gallery_filter, snap.settings.photos_layout);
        if listed || arranged {
            for panel in &mut self.list {
                panel.dirty = true;
            }
            if let Some(panel) = self.takeover.as_mut() {
                panel.dirty = true;
            }
        }
        self.follow_note(snap);
        // The gallery's pictures: those it shows and the next few, and it lets go of those far behind.
        if self.recents.pictures_came() {
            if let Some(panel) = self.takeover.as_mut() {
                panel.dirty = true;
            }
        }
        if self.facts.sheet == Some(Sheet::All) {
            self.recents.ask(0, 0);
            self.recents.keep(0, 0);
        } else if self.facts.sheet == Some(Sheet::Pictures) {
            if let Some(panel) = self.takeover.as_mut() {
                let shown = || panel.items.iter().filter_map(|i| if let panels::Look::Capture(n) | panels::Look::BigPicture(n) = i.look { Some(n) } else { None });
                if let (Some(first), Some(last)) = (shown().min(), shown().max()) {
                    self.recents.ask(first, last + 6);
                    self.recents.keep(first.saturating_sub(36), last + 36);
                    let here: std::collections::HashSet<String> = self.recents.asked.iter().filter_map(|i| self.recents.loaded.get(*i)).filter(|r| r.image.is_some()).map(|r| r.key.to_string_lossy().to_string()).collect();
                    let art = self.art.as_ref().map(|a| a.0.clone());
                    let large = self.recents.big.as_ref().and_then(|b| if let crate::worlds::Picture::Shot { path, .. } = &b.0 { Some(format!("large {}", path.display())) } else { None });
                    panel.pictures.retain(|k, _| here.contains(k) || art.as_ref() == Some(k) || large.as_ref() == Some(k));
                }
            }
        } else if !self.recents.asked.is_empty() {
            // Its pictures do not stay in memory once it is left.
            self.recents.keep(1, 0);
            let art = self.art.as_ref().map(|a| a.0.clone());
            if let Some(panel) = self.takeover.as_mut() {
                panel.pictures.retain(|k, _| art.as_ref() == Some(k));
            }
        }
        // N4N0's menu is what plays and its controls go through: open, drawn or not.
        if !self.stage.host.menu.open {
            self.stage.host.open(snap);
        }
        self.stage.host.follow(snap);
        self.stage.host.follow_media();
        // What plays's picture, for the panels' own textures.
        let key = self.stage.host.menu.media.as_ref().and_then(|m| m.art.clone());
        if self.art.as_ref().map(|(k, _)| k) != key.as_ref() {
            self.art = key.and_then(|k| self.stage.host.image(&k).map(|i| (k, i)));
            if self.art.is_some() {
                for panel in &mut self.list {
                    panel.dirty = true;
                }
            }
        }
        let facts = self.facts_of(snap);
        if facts != self.facts {
            self.facts = facts;
            for panel in &mut self.list {
                panel.dirty = true;
            }
            if let Some(panel) = self.takeover.as_mut() {
                panel.dirty = true;
            }
        }
        // The menu going back to its Home, or to its own More, means back to All tools here: those two pages do not show.
        if self.facts.sheet == Some(Sheet::Menu) && matches!(self.stage.host.menu.page, Page::Home | Page::More) {
            self.open_sheet(Sheet::All, "All tools", snap);
        }
        // YouTube's results are one list here, the rest of it the menu's own pages.
        if self.facts.sheet == Some(Sheet::Menu) && self.stage.host.menu.page == Page::Results {
            self.open_sheet(Sheet::Results, "YouTube", snap);
        } else if self.facts.sheet == Some(Sheet::Results) && self.stage.host.menu.page != Page::Results {
            self.open_sheet(Sheet::Menu, "YouTube", snap);
        }
        if self.facts.sheet == Some(Sheet::Results) {
            let shown = panels::videos_shown(self.facts.gallery_scroll);
            if self.videos_asked != Some(shown.clone()) || self.videos_had != self.stage.host.menu.results.len() {
                self.videos_asked = Some(shown.clone());
                self.videos_had = self.stage.host.menu.results.len();
                let host = &mut self.stage.host;
                let ids: Vec<String> = host.menu.results.iter().skip(shown.start).take(shown.len()).map(|v| v.id.clone()).collect();
                // Near the end, the next ones: the list has no end while YouTube has more.
                let mut effects = host.menu.results.len().saturating_sub(shown.end).lt(&6).then(|| host.menu.want_more_results()).unwrap_or_default();
                effects.push(orbiters_n4n0_core::menu::Effect::Thumbnails(ids));
                host.effects(effects, snap, &self.backend);
            }
            if let Some(panel) = self.takeover.as_mut() {
                // Their pictures arrive as they do.
                panel.dirty = panel.dirty || self.stage.host.dirty;
            }
        }
        // The conversation keeps its newest in view as it grows.
        if self.facts.sheet == Some(Sheet::Talk) && self.talk_had != self.facts.talk_lines.len() {
            self.talk_had = self.facts.talk_lines.len();
            self.gallery_to = panels::max_scroll(Sheet::Talk, &self.facts);
        }
        // An invite that needs Orbiters as a friend first goes on the menu's own page.
        if self.facts.sheet == Some(Sheet::Instances) && self.stage.host.menu.page != Page::Instances {
            self.open_sheet(Sheet::Menu, "Add Orbiters as a friend", snap);
        }
        if let Some(worlds) = self.stage.worlds.as_mut() {
            if matches!(self.facts.sheet, Some(Sheet::Instances | Sheet::All)) {
                let shown = panels::visits_shown(self.facts.gallery_scroll);
                for visit in self.facts.worlds_visited.iter().skip(shown.start).take(shown.len()) {
                    if !self.visit_pictures.contains_key(&visit.world_id) {
                        if let Some(picture) = worlds.world_thumbnail(&visit.world_id) {
                            self.visit_pictures.insert(visit.world_id.clone(), picture);
                        }
                    }
                }
            }
            worlds.update_alone(snap, &self.backend);
            if self.facts.sheet == Some(Sheet::Worlds) && !worlds.open {
                self.close_sheet();
            }
        }
    }

    fn facts_of(&self, snap: &Snapshot) -> Facts {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let progress = |dir: &PathBuf| snap.reconstruction.as_ref().filter(|j| j.session_dir == *dir).map(|j| j.progress.clamp(0.0, 1.0));
        let recording = matches!(snap.capture.state, CaptureState::Recording | CaptureState::Paused) && !snap.capture.photo_3d;
        let menu = &self.stage.host.menu;
        let visited = if matches!(self.facts.sheet, Some(Sheet::Instances | Sheet::All)) { visited_worlds(&menu.instances, now, &self.visit_pictures) } else { Vec::new() };
        Facts {
            recording,
            recording_seconds: recording.then(|| snap.capture.stats.as_ref().map_or(0, |s| s.elapsed_s as u32)),
            capture_busy: !matches!(snap.capture.state, CaptureState::Idle),
            countdown: snap.n4n0.photo_countdown,
            picture_flying: snap.capture.photo_3d && !matches!(snap.capture.state, CaptureState::Idle),
            // Their pictures are looked up per part, in `paint`.
            recent: if self.facts.sheet == Some(Sheet::Pictures) { self.recents.shown.clone() } else { Default::default() },
            gallery_filter: self.facts.gallery_filter,
            gallery_reading: self.recents.rx.is_some() && self.recents.loaded.is_empty(),
            opened: self.facts.opened,
            opened_picture: None,
            // Only what is on the page: they change as it is made and as time goes, and there may be tens of thousands.
            recent_words: self
                .takeover
                .iter()
                .filter(|_| self.facts.sheet == Some(Sheet::Pictures))
                .flat_map(|panel| &panel.items)
                .filter_map(|i| if let panels::Look::Capture(n) | panels::Look::BigPicture(n) = i.look { Some(n) } else { None })
                .filter_map(|n| self.recents.tile(n).map(|r| (n, recent_state(r, snap, now), progress(&r.key))))
                .collect(),
            recent_pictures: Vec::new(),
            view_said: self.facts.view_said.clone(),
            view_small: self.facts.view_small,
            sheet_hole: self.facts.sheet_hole,
            hint_band: self.list.iter().any(|p| p.zone == Zone::SheetHint),
            hero_pictures: Vec::new(),
            reconstructing: snap.reconstruction.as_ref().map(|j| j.progress.clamp(0.0, 1.0)),
            waiting: snap.reconstruct_queue.len(),
            desktop: snap.n4n0.desktop_mode || !snap.steamvr.running,
            fast_walk: snap.settings.walk_boost.enabled,
            n4n0: snap.settings.n4n0.enabled,
            music: menu.media.as_ref().map(|m| (m.title.clone(), shown_playing(m.playing, self.play_asked, Instant::now()))),
            music_by: menu.media.as_ref().map_or(String::new(), |m| m.artist.clone()),
            art: None,
            no_kit: snap.worn_kit == Some(false),
            timer: snap.settings.photo_3d_timer,
            can_call: menu.calls_ready,
            can_states: menu.states_ready,
            can_light: menu.light.is_some(),
            vr_close: self.sharp > 1.0,
            picture_mode: self.facts.picture_mode,
            hint: self.facts.hint,
            view_showing: self.facts.view_showing,
            invite_chip: self.invited.as_ref().filter(|(_, at)| at.elapsed() < Duration::from_secs(6)).map(|(place, _)| {
                let state = if menu.invite_waiting {
                    panels::InviteChip::Asking
                } else {
                    match menu.invite_went {
                        Some(true) => panels::InviteChip::Sent,
                        Some(false) => panels::InviteChip::Failed,
                        None => panels::InviteChip::Asking,
                    }
                };
                (place.clone(), state)
            }),
            video_copied: self.copied.filter(|(_, at)| at.elapsed() < Duration::from_secs(2)).map(|(i, _)| i),
            videos: if self.facts.sheet == Some(Sheet::Results) {
                menu.results
                    .iter()
                    .map(|v| panels::VideoTile { title: v.title.clone(), channel: v.channel.clone(), duration: v.duration.clone(), picture: self.stage.host.picture(&v.id, false) })
                    .collect()
            } else {
                Vec::new()
            },
            videos_for: menu.results_for.clone(),
            talk: talk_now(snap),
            talk_lines: if self.facts.sheet == Some(Sheet::Talk) {
                snap.n4n0.lines.iter().map(|l| (l.who == orbiters_n4n0::status::N4n0Who::You, l.text.clone())).collect()
            } else {
                Vec::new()
            },
            // On its own page it is all there already.
            n4n0_says: snap.n4n0.panel.clone().filter(|_| self.facts.sheet != Some(Sheet::Talk)),
            wake_word: snap.settings.n4n0.wake_words.first().filter(|_| snap.settings.n4n0.wake_word).cloned(),
            gallery_scroll: self.facts.gallery_scroll,
            gallery_layout: snap.settings.photos_layout,
            volume: menu.volume.as_ref().map(|v| (v.percent, v.muted)),
            music_times: menu.media.as_ref().and_then(|m| {
                let of = m.duration?;
                // Under the pointer it is where the pointer is; just moved, where it was moved to until the app says so too.
                let at = self
                    .seeking
                    .map(|f| (f * of as f32) as u32)
                    .or(self.seek_asked.filter(|(_, at)| at.elapsed() < MEDIA_CATCHES_UP).map(|(s, _)| s))
                    .or(m.position)?;
                Some((at, of))
            }),
            can_seek: menu.media.as_ref().is_some_and(|m| m.can_seek),
            world_video: menu.world_video.as_ref().map(|v| v.title.clone().unwrap_or_else(|| v.url.clone())),
            digits: self.open == Some(Around::Wheel) && self.takeover.is_none() && (snap.n4n0.desktop_mode || !snap.steamvr.running),
            beside_wing: self.open == Some(Around::QuickMenu),
            worlds_visited: visited,
            invite_said: matches!(self.facts.sheet, Some(Sheet::Instances | Sheet::Results)).then(|| menu.notice.clone()).flatten(),
            note: self.facts.note.clone(),
            picture_wait: snap.n4n0.photo_wait,
            pointed: self.facts.pointed,
            sheet: self.facts.sheet,
            sheet_title: self.facts.sheet_title.clone(),
        }
    }

    /// What the caption says with nothing pointed at.
    pub fn set_hint(&mut self, hint: Option<(&'static str, &'static str)>) {
        if self.facts.hint != hint {
            self.facts.hint = hint;
            self.caption_dirty();
        }
    }

    fn caption_dirty(&mut self) {
        for panel in &mut self.list {
            // A sheet's head says it too.
            if panel.zone.passes_clicks() || panel.zone == Zone::SheetBack {
                panel.dirty = true;
            }
        }
    }

    /// A pointer on a part (`on`: its index in [`Panels::all`], and where in units), or on none.
    pub fn hover(&mut self, on: Option<(usize, Pos2)>) {
        let count = self.list.len();
        let (sheet, show_keys, desktop) = (self.facts.sheet, self.show_keys, self.facts.desktop);
        let mut worlds_hover = None;
        let mut on_zone = None;
        self.sheet_pointer = on.filter(|(at, _)| *at == count).map(|(_, p)| p);
        if let (Some(_), Some(p)) = (self.seeking, self.sheet_pointer) {
            self.seeking = panels::timeline_fraction(&self.facts, p.x).or(self.seeking);
        }
        if let (Some(drag), Some(p), Some(sheet)) = (self.drag.as_mut(), self.sheet_pointer, self.facts.sheet) {
            let now = Instant::now();
            let to = if drag.bar {
                let to = panels::scroll_from_bar(sheet, &self.facts, p.y);
                drag.measure(to, now);
                Some(to)
            } else {
                drag.follow(p.y, panels::max_scroll(sheet, &self.facts), self.elastic, now)
            };
            if let Some(to) = to {
                self.gallery_to = to;
                self.facts.gallery_scroll = to;
                if let Some(panel) = self.takeover.as_mut() {
                    panel.dirty = true;
                }
            }
        }
        for (i, panel) in self.list.iter_mut().chain(self.takeover.iter_mut()).enumerate() {
            let hover = on.filter(|(at, _)| *at == i).and_then(|(_, p)| {
                on_zone = Some(panel.zone);
                if i == count {
                    match sheet {
                        Some(Sheet::Menu) => {
                            if desktop && !show_keys && self.stage.host.menu.shows_keyboard() && p.y >= KEYS_TOP {
                                return None;
                            }
                            let page = layout::cached(ScreenPage::of(&self.stage.host.menu));
                            layout::hit_pointable(page, &self.stage.host.menu, p.x, p.y).map(|(element, _)| element)
                        }
                        Some(Sheet::Worlds) => {
                            worlds_hover = Some((p.x, p.y));
                            None
                        }
                        _ => panels::hit(&panel.items, p),
                    }
                } else {
                    panels::hit(&panel.items, p)
                }
            });
            if hover != panel.hover {
                panel.hover = hover;
                panel.dirty = true;
                if i == count {
                    self.stage.host.dirty = true;
                }
            }
        }
        if sheet == Some(Sheet::Worlds) {
            if let Some(worlds) = self.stage.worlds.as_mut() {
                worlds.hover_alone(worlds_hover);
            }
        }
        // A second ring blooms from its cell.
        let was = self.bloom.open();
        let group = self.bloom.step(on_zone, Instant::now());
        let live = !self.facts.desktop || group.is_some();
        for panel in &mut self.list {
            if let Some(of) = panel.zone.group() {
                let show = group == Some(of);
                if panel.shown != show {
                    panel.shown = show;
                    if show {
                        let order = of.cells().iter().position(|z| *z == panel.zone).unwrap_or(0);
                        panel.appear = Appear::new(order, live);
                    }
                }
            }
        }
        let _ = was;
        // VRChat reacts to where the pointer is only while it is on what hides it.
        let on_parts = on.is_some();
        for panel in self.list.iter_mut().filter(|p| p.zone == Zone::SheetHint) {
            panel.shown = on_parts;
        }
        // The caption says what is pointed at.
        let pointed = self.list.iter().find_map(|p| p.hover.and_then(|i| p.items.get(i)).map(|item| item.action)).or_else(|| self.takeover.as_ref().filter(|_| matches!(sheet, Some(Sheet::All | Sheet::Pictures))).and_then(|p| p.hover.and_then(|i| p.items.get(i)).map(|item| item.action)));
        if pointed != self.facts.pointed {
            self.facts.pointed = pointed;
            self.caption_dirty();
        }
    }

    /// The pointer's button went down on what it hovers.
    pub fn press(&mut self, snap: &Snapshot) -> Pressed {
        let mut out = Pressed::default();
        // In front of everything, what the sheet shows takes it.
        match self.facts.sheet {
            Some(Sheet::Worlds) => {
                if let Some(worlds) = self.stage.worlds.as_mut() {
                    if worlds.press_alone() {
                        out.clicked = true;
                        return out;
                    }
                }
            }
            Some(Sheet::Menu) => {
                if let Some(panel) = self.takeover.as_mut() {
                    if let Some(element) = panel.hover {
                        let host = &mut self.stage.host;
                        let page = layout::cached(ScreenPage::of(&host.menu));
                        if let Some(e) = page.elements.get(element) {
                            panel.pressed = Some(element);
                            out.clicked = true;
                            let effects = host.press(e.target);
                            let closing = host.effects(effects, snap, &self.backend) || !host.menu.open;
                            host.dirty = true;
                            if closing {
                                let _ = host;
                                self.close_sheet();
                            }
                        }
                        return out;
                    }
                }
            }
            _ => {}
        }
        let mut action = None;
        let mut look = None;
        for panel in self.list.iter_mut().chain(self.takeover.iter_mut()) {
            if let Some(i) = panel.hover {
                panel.pressed = Some(i);
                panel.dirty = true;
                action = panel.items.get(i).map(|item| item.action);
                look = panel.items.get(i).map(|item| item.look);
                break;
            }
        }
        // A list's own empty space can be dragged too.
        if look.is_none() && self.facts.opened.is_none() && self.sheet_pointer.is_some() && matches!(self.facts.sheet, Some(Sheet::Pictures | Sheet::Instances | Sheet::Results | Sheet::Talk)) {
            self.throw = 0.0;
            self.drag = Some(Drag::new(self.facts.gallery_scroll, None, false));
            return out;
        }
        // Along what plays: where it is held, and moved there when let go.
        if action == Some(Action::Timeline) {
            if let Some(p) = self.sheet_pointer {
                self.seeking = panels::timeline_fraction(&self.facts, p.x);
            }
            return out;
        }
        // The scroll bar goes where the pointer is.
        if action == Some(Action::ScrollBar) {
            self.throw = 0.0;
            self.drag = Some(Drag::new(self.facts.gallery_scroll, None, true));
            return out;
        }
        // In a list a press waits: it may be a drag, of the list and not of what it began on.
        if let (Some(sheet), Some(look)) = (self.facts.sheet, look) {
            if panels::scrolls_with_a_drag(sheet, &look) {
                self.throw = 0.0;
                self.drag = Some(Drag::new(self.facts.gallery_scroll, action, false));
                return out;
            }
        }
        let Some(action) = action else { return out };
        self.act(action, snap, &mut out);
        out
    }

    /// What a press on `action` does.
    fn act(&mut self, action: Action, snap: &Snapshot, out: &mut Pressed) {
        out.clicked = action != Action::Idle;
        match action {
            Action::Worlds => {
                if self.worlds_here {
                    self.open_sheet(Sheet::Worlds, "Worlds in 3D", snap);
                    if let Some(worlds) = self.stage.worlds.as_mut() {
                        worlds.open_panel(self.stage.host.menu.layout);
                    }
                } else {
                    out.worlds = true;
                }
            }
            Action::All => self.open_sheet(Sheet::All, "All tools", snap),
            Action::Pictures => self.open_sheet(Sheet::Pictures, "Pictures", snap),
            Action::Capture(n) => {
                let Some(recent) = self.recents.tile(n) else { return };
                match recent.kind() {
                    panels::RecentKind::Capture if self.worlds_here => {
                        self.local_asked.push(super::worlds::Relayed::ShowLocal { dir: recent.key.clone(), name: recent.title.clone(), changed: recent.modified });
                    }
                    // Its caption says why not.
                    panels::RecentKind::NoView => out.clicked = false,
                    // Nothing shows around the person with no headset: seen here first, as a picture is, with the app's window as what is next.
                    panels::RecentKind::Capture | panels::RecentKind::Picture => {
                        self.facts.opened = Some(n);
                        self.recents.big = None;
                        self.recents.open_big(n);
                        self.gallery_dirty();
                    }
                }
            }
            Action::Filter(filter) => {
                if self.facts.gallery_filter != filter {
                    self.facts.gallery_filter = filter;
                    self.facts.gallery_scroll = 0.0;
                    self.gallery_to = 0.0;
                    self.throw = 0.0;
                    self.gallery_dirty();
                }
            }
            Action::PictureBack => {
                self.facts.opened = None;
                self.recents.big = None;
                self.gallery_dirty();
            }
            Action::PictureStep(after) => {
                if let Some(n) = self.facts.opened.and_then(|n| panels::picture_beside(&self.facts, n, after)) {
                    self.facts.opened = Some(n);
                    self.recents.open_big(n);
                    self.gallery_dirty();
                }
            }
            // It says why not, under the pointer.
            Action::Picture(_) if self.facts.no_kit => out.clicked = false,
            Action::Picture(mode) => {
                self.facts.picture_mode = Some(mode);
                for panel in self.list.iter_mut().chain(self.takeover.iter_mut()) {
                    panel.dirty = true;
                }
                let host = &mut self.stage.host;
                let mut effects = host.press(Target::TakeOf(mode));
                // On the desktop the camera cannot be open under these menus, so the picture is asked for and waits for it.
                let mut waits = None;
                if self.facts.desktop {
                    for effect in &mut effects {
                        if let Effect::Photo3d { timer, length, mode } = *effect {
                            waits = Some(timer);
                            *effect = Effect::PhotoChoice { timer, length, mode };
                        }
                    }
                }
                host.effects(effects, snap, &self.backend);
                if let Some(seconds) = waits {
                    self.backend.n4n0(crate::ipc::N4n0Command::Photo3DWhenCamera { seconds });
                }
            }
            Action::Record => {
                if self.facts.recording {
                    self.backend.stop_capture();
                } else if !self.facts.capture_busy {
                    self.backend.start_capture();
                }
            }
            Action::Page(target) => self.open_page(target, snap),
            Action::Invite(group, visit) => {
                let asked = self.facts.worlds_visited.get(group).and_then(|w| {
                    let location = if visit == 0 { Some(w.location.clone()) } else { w.earlier.get(visit - 1).map(|e| e.0.clone()) };
                    location.map(|l| (l, w.name.clone()))
                });
                if let Some((location, name)) = asked {
                    self.invited = Some((location.clone(), Instant::now()));
                    let host = &mut self.stage.host;
                    let effects = host.menu.invite_to(location, &name);
                    host.effects(effects, snap, &self.backend);
                }
            }
            Action::Volume(up) => {
                let host = &mut self.stage.host;
                let effects = host.press(if up { Target::VolumeUp } else { Target::VolumeDown });
                host.effects(effects, snap, &self.backend);
            }
            Action::Mute => {
                let host = &mut self.stage.host;
                let effects = host.press(Target::Mute);
                host.effects(effects, snap, &self.backend);
            }
            Action::FastWalk => {
                let host = &mut self.stage.host;
                let effects = host.press(Target::FastWalk);
                host.effects(effects, snap, &self.backend);
            }
            Action::PlayPause | Action::Previous | Action::Next => {
                let target = match action {
                    Action::Previous => Target::MediaPrevious,
                    Action::Next => Target::MediaNext,
                    _ => Target::MediaPlayPause,
                };
                let host = &mut self.stage.host;
                // What it was asked to become stays said until what is read catches up.
                if target == Target::MediaPlayPause {
                    if let Some(media) = host.menu.media.as_ref().filter(|m| m.can_play_pause) {
                        self.play_asked = Some((!shown_playing(media.playing, self.play_asked, Instant::now()), Instant::now()));
                    }
                }
                let effects = host.press(target);
                host.effects(effects, snap, &self.backend);
            }
            Action::Timeline => {}
            Action::Talk => {
                if self.facts.n4n0 {
                    self.open_sheet(Sheet::Talk, "Ask N4N0", snap);
                }
            }
            Action::TalkToggle => {
                if !matches!(self.facts.talk, panels::TalkNow::Off(_)) {
                    let down = !snap.n4n0.listening;
                    self.talking = down;
                    self.backend.n4n0(N4n0Command::Talk { down });
                }
            }
            Action::NewConversation => self.backend.n4n0(N4n0Command::NewConversation),
            Action::SmallView(on) => {
                if let Some(worlds) = self.stage.worlds.as_mut() {
                    worlds.view_small(on);
                }
            }
            Action::HideView => {
                if let Some(worlds) = self.stage.worlds.as_mut() {
                    worlds.hide_view();
                }
            }
            Action::ScrollBar => {}
            Action::Video(i) | Action::VideoDrop(i) => {
                if matches!(action, Action::Video(_)) {
                    self.copied = Some((i, Instant::now()));
                }
                // The menu's page of results is what its rows press on.
                let host = &mut self.stage.host;
                host.menu.result_page = i / orbiters_n4n0_core::menu::RESULTS_PER_PAGE;
                let row = (i % orbiters_n4n0_core::menu::RESULTS_PER_PAGE) as u8;
                let effects = host.press(if matches!(action, Action::Video(_)) { Target::Copy(row) } else { Target::Drop(row) });
                host.effects(effects, snap, &self.backend);
            }
            Action::ResultsBack => {
                let host = &mut self.stage.host;
                let effects = host.press(Target::Back);
                host.effects(effects, snap, &self.backend);
            }
            Action::OpenApp => self.backend.show_window(),
            Action::CloseSheet => self.close_sheet(),
            Action::Hide => {
                self.hidden = true;
                out.hide = true;
                self.release();
            }
            Action::Idle => {}
        }
    }

    /// The pointer's button went up.
    pub fn release(&mut self) {
        if let Some(fraction) = self.seeking.take() {
            self.seek_late = Some(fraction);
        }
        if let Some(drag) = self.drag.take() {
            if !drag.moved {
                self.pressed_late = drag.action;
            }
            self.throw = drag.throw(Instant::now());
            // Let go with no throw: to rest as well. Past an end it comes back by itself.
            if drag.moved && !drag.bar && self.throw == 0.0 {
                if let Some(sheet) = self.facts.sheet {
                    let (at, max) = (self.facts.gallery_scroll, panels::max_scroll(sheet, &self.facts));
                    if at >= 0.0 && at <= max {
                        self.gallery_to = panels::snap_scroll(sheet, &self.facts, at);
                    }
                }
            }
        }
        for panel in self.list.iter_mut().chain(self.takeover.iter_mut()) {
            if panel.pressed.take().is_some() {
                panel.dirty = true;
            }
        }
        if let Some(worlds) = self.stage.worlds.as_mut() {
            worlds.release_alone();
        }
        self.stage.host.dirty = true;
    }

    /// Paints what changed and hands each new picture to `show`, with the part that changed. Nothing changed: nothing happens, at no cost.
    pub fn paint(&mut self, snap: &Snapshot, mut show: impl FnMut(&Panel, [u32; 4])) {
        let time = self.started.elapsed().as_secs_f64();
        let live = !snap.settings.reduce_motion;
        let art = self.art.clone();
        for panel in &mut self.list {
            if !panel.dirty {
                continue;
            }
            let first = !panel.painted;
            panel.dirty = false;
            panel.items = panels::items(panel.zone, panel.logical, &self.facts);
            let mut facts = self.facts.clone();
            if matches!(panel.zone, Zone::WheelPlaying | Zone::WingPlaying) {
                if let Some((key, image)) = &art {
                    let ctx = panel.canvas.ctx();
                    facts.art = Some(panel.pictures.entry(key.clone()).or_insert_with(|| ctx.load_texture("art", image.clone(), TextureOptions::LINEAR)).id());
                }
            }
            orbiters_ui::anim::set_live(panel.canvas.ctx(), live);
            let (zone, size, hover, pressed) = (panel.zone, panel.logical, panel.hover, panel.pressed);
            panel.canvas.paint(time, |p| panels::paint(p, zone, size, &facts, hover, pressed));
            if let Some(changed) = panel.take_picture() {
                panel.painted = true;
                show(panel, changed);
            }
            // What loads on a first frame (the mark's pictures) shows on the second.
            panel.dirty = first;
        }
        if self.takeover.is_none() {
            return;
        }
        let sheet = self.facts.sheet;
        let stage = &mut self.stage;
        let panel = self.takeover.as_mut().expect("just looked");
        let retro_now = snap.settings.n4n0.menu.look == MenuLook::Retro;
        if retro_now != stage.retro {
            stage.retro = retro_now;
            stage.host.dirty = true;
        }
        match sheet {
            Some(Sheet::Worlds) => {
                if let Some(worlds) = stage.worlds.as_mut() {
                    if worlds.wants_paint() || !panel.painted {
                        worlds.paint(&mut stage.canvas);
                        stage.canvas_to(panel, &mut show);
                    }
                }
            }
            Some(Sheet::Menu) => {
                let animating = stage.host.menu.listening || stage.host.menu.popping().is_some();
                if stage.host.dirty || animating || !panel.painted {
                    stage.host.dirty = false;
                    let (hover, pressed, retro) = (panel.hover, panel.pressed, stage.retro);
                    let host = &stage.host;
                    let picture = |id: &str| -> Option<TextureId> { host.picture(id, retro) };
                    let view = View { menu: &host.menu, hover, pressed, picture: &picture, time, skin: skin() };
                    let cover = self.facts.desktop && !self.show_keys && host.menu.shows_keyboard();
                    let world_video = host.menu.world_video.as_ref().map(|v| v.title.clone().unwrap_or_else(|| v.url.clone()));
                    stage.canvas.paint(time, |p| {
                        if retro {
                            menu_retro::draw(p, pos2(0.0, 0.0), &view);
                        } else {
                            menu_ui::draw(p, pos2(0.0, 0.0), &view);
                        }
                        if cover {
                            type_hint(p, world_video.as_deref());
                        }
                    });
                    stage.canvas_to(panel, &mut show);
                }
            }
            Some(sheet) => {
                if panel.dirty || !panel.painted {
                    let first = !panel.painted;
                    panel.dirty = false;
                    panel.items = panels::sheet_items(sheet, &self.facts);
                    let mut facts = self.facts.clone();
                    if sheet == Sheet::All {
                        // What All tools' large tiles hold, for real.
                        if let Some(id) = facts.worlds_visited.first().and_then(|v| v.picture) {
                            facts.hero_pictures.push((Action::Page(Target::Instances), id));
                        }
                        if let Some(loaded) = self.recents.tile(0).filter(|r| r.image.is_some()) {
                            let ctx = stage.canvas.ctx();
                            let name = loaded.key.to_string_lossy().to_string();
                            let image = loaded.image.as_ref().expect("just looked");
                            let id = panel.pictures.entry(name).or_insert_with(|| ctx.load_texture("recent", image.clone(), TextureOptions::LINEAR)).id();
                            facts.hero_pictures.push((Action::Pictures, id));
                        }
                    }
                    if sheet == Sheet::Pictures {
                        let ctx = stage.canvas.ctx();
                        for item in &panel.items {
                            let (panels::Look::Capture(n) | panels::Look::BigPicture(n)) = item.look else { continue };
                            let Some(loaded) = self.recents.tile(n) else { continue };
                            if let Some(image) = &loaded.image {
                                let name = loaded.key.to_string_lossy().to_string();
                                let id = panel.pictures.entry(name).or_insert_with(|| ctx.load_texture("recent", image.clone(), TextureOptions::LINEAR)).id();
                                facts.recent_pictures.push((n, id));
                            }
                        }
                        if let Some((crate::worlds::Picture::Shot { path, .. }, Some(image))) = &self.recents.big {
                            let name = format!("large {}", path.display());
                            facts.opened_picture = Some(panel.pictures.entry(name).or_insert_with(|| ctx.load_texture("large", image.clone(), TextureOptions::LINEAR)).id());
                        }
                    }
                    if let (Some((key, image)), Sheet::All) = (&art, sheet) {
                        let ctx = stage.canvas.ctx();
                        facts.art = Some(panel.pictures.entry(key.clone()).or_insert_with(|| ctx.load_texture("art", image.clone(), TextureOptions::LINEAR)).id());
                    }
                    let (hover, pressed) = (panel.hover, panel.pressed);
                    stage.canvas.paint(time, |p| panels::paint_sheet(p, sheet, &facts, hover, pressed));
                    stage.canvas_to(panel, &mut show);
                    panel.dirty = first;
                }
            }
            None => {}
        }
    }

    /// Takes the menu's own business for a turn while it is on the sheet: what it fetches, what comes back, and telling the service: only its keyboard needs it to know (keys go to the
    /// keyboard page). Opening a page does not start N4N0 listening: that is what Ask N4N0 is for.
    pub fn menu_turn(&mut self, snap: &Snapshot) {
        let on_sheet = self.facts.sheet == Some(Sheet::Menu);
        let keyboard = (on_sheet && self.stage.host.menu.shows_keyboard()) || (self.facts.sheet == Some(Sheet::Worlds) && self.stage.worlds.as_ref().is_some_and(|w| w.typing()));
        let state = (keyboard, false, keyboard);
        if self.reported.unwrap_or((false, false, false)) != state {
            self.reported = Some(state);
            self.backend.n4n0(N4n0Command::MenuState { open: state.0, dictation: state.1, keyboard: state.2 });
        }
        if let Some(worlds) = self.stage.worlds.as_mut().filter(|w| w.open) {
            worlds.keys(&snap.n4n0.keys);
        }
        let host = &mut self.stage.host;
        let mut effects = if on_sheet { host.take_keys(&snap.n4n0) } else { Vec::new() };
        effects.extend(host.poll());
        if host.effects(effects, snap, &self.backend) && on_sheet {
            self.close_sheet();
        }
    }

    /// Whether any picture is due or any part still moving.
    pub fn busy(&self) -> bool {
        self.all().any(|p| p.dirty || !p.appear.settled()) || self.stage.host.dirty
    }

    /// Moves each part's spring on.
    pub fn step_springs(&mut self, dt: f32) {
        let caption = self.caption_shown();
        for panel in self.list.iter_mut().filter(|p| matches!(p.zone, Zone::QmCaption | Zone::WheelCaption)) {
            panel.shown = caption;
        }
        for panel in self.list.iter_mut().chain(self.takeover.iter_mut()) {
            panel.appear.step(dt);
        }
        // A list thrown goes on, slowing, past its end and back; or it glides to where its wheel sent it. Only its page paints.
        // A picture shown large leaves the list where it was.
        if self.drag.is_none() && self.facts.opened.is_none() {
            let max = self.facts.sheet.map_or(0.0, |sheet| panels::max_scroll(sheet, &self.facts));
            let was = self.facts.gallery_scroll;
            let moving = self.throw != 0.0;
            (self.facts.gallery_scroll, self.throw, self.gallery_to) = scroll_step(was, self.throw, self.gallery_to, max, dt, self.elastic);
            // Come to rest: with its first row whole.
            if moving && self.throw == 0.0 {
                if let Some(sheet) = self.facts.sheet {
                    self.gallery_to = panels::snap_scroll(sheet, &self.facts, self.facts.gallery_scroll);
                }
            }
            if self.facts.gallery_scroll != was {
                if let Some(panel) = self.takeover.as_mut() {
                    panel.dirty = true;
                }
            }
        }
    }
}

impl Stage {
    /// What the canvas painted, to the page on the sheet.
    fn canvas_to(&mut self, panel: &mut Panel, show: &mut impl FnMut(&Panel, [u32; 4])) {
        if let Some(changed) = self.canvas.damage() {
            let [w, h] = panel.px;
            let pixels = self.canvas.pixels();
            for y in changed[1]..changed[3].min(h) {
                let (a, b) = ((y * w + changed[0]) as usize * 4, (y * w + changed[2].min(w)) as usize * 4);
                panel.out[a..b].copy_from_slice(&pixels[a..b]);
            }
            panel.painted = true;
            show(panel, changed);
        }
    }
}

/// Over the keyboard of N4N0's search page on a desktop, where a real one is under the hands: what to do instead.
fn type_hint(p: &egui::Painter, playing: Option<&str>) {
    use orbiters_ui::{icons, theme, widgets};
    let area = egui::Rect::from_min_max(pos2(0.0, KEYS_TOP), pos2(layout::WIDTH as f32, layout::HEIGHT as f32));
    p.rect_filled(area, 0.0, panels::look::OPAQUE);
    widgets::icon(p, area.center() - vec2(0.0, 26.0), icons::KEYBOARD, 56.0, panels::look::LABEL);
    p.text(area.center() + vec2(0.0, 24.0), egui::Align2::CENTER_CENTER, "Type on your keyboard", theme::font(22.0, theme::weight::BOLD), panels::look::LABEL);
    p.text(area.center() + vec2(0.0, 54.0), egui::Align2::CENTER_CENTER, "Enter searches", theme::font(15.0, theme::weight::MEDIUM), panels::look::MUTED);
    // What the world's player has now, which is what a search here would replace.
    if let Some(title) = playing {
        let row = egui::Rect::from_center_size(pos2(area.center().x, area.bottom() - 42.0), vec2(area.width() - 96.0, 56.0));
        p.rect_filled(row, theme::radius(10.0), panels::look::TILE);
        widgets::icon(p, pos2(row.left() + 30.0, row.center().y), icons::MONITOR_PLAY, 26.0, panels::look::LABEL);
        let font = theme::font(13.0, theme::weight::SEMIBOLD);
        p.text(pos2(row.left() + 56.0, row.center().y - 11.0), egui::Align2::LEFT_CENTER, "In this world's player", font, panels::look::MUTED);
        let font = theme::font(16.0, theme::weight::SEMIBOLD);
        let room = row.width() - 72.0;
        let mut shown = title.to_string();
        while shown.chars().count() > 8 && p.layout_no_wrap(shown.clone(), font.clone(), egui::Color32::WHITE).size().x > room {
            shown.pop();
            shown = format!("{}…", shown.trim_end_matches('…'));
            shown.pop();
            shown.push('…');
        }
        p.text(pos2(row.left() + 56.0, row.center().y + 11.0), egui::Align2::LEFT_CENTER, shown, font, panels::look::LABEL);
    }
}

impl Drop for Panels {
    fn drop(&mut self) {
        self.release();
        if self.reported.is_some_and(|r| r.0) {
            self.backend.n4n0(N4n0Command::MenuState { open: false, dictation: false, keyboard: false });
        }
    }
}

/// Where talking with N4N0 stands, and why it cannot when it cannot.
fn talk_now(snap: &Snapshot) -> panels::TalkNow {
    let settings = &snap.settings.n4n0;
    if snap.n4n0.listening {
        return panels::TalkNow::Listening((snap.n4n0.voice.level * 20.0).round() / 20.0);
    }
    if !settings.enabled || !settings.assistant {
        return panels::TalkNow::Off("Turn on N4N0's assistant in Orbiters");
    }
    if snap.n4n0.voice.downloading.is_some() {
        return panels::TalkNow::Off("Its speech models are downloading");
    }
    if !snap.n4n0.voice.installed {
        return panels::TalkNow::Off("Install its speech in Orbiters, N4N0");
    }
    if snap.n4n0.thinking {
        return panels::TalkNow::Thinking;
    }
    panels::TalkNow::Ready
}

/// What must be said with no menu up: a 3D picture under way or waiting, or what stopped one. With VRChat's camera open on the desktop no menu can be.
fn away_words(snap: &Snapshot, note: &Option<(String, String)>) -> Option<(String, String)> {
    if let Some(seconds) = snap.n4n0.photo_countdown {
        return Some((format!("3D picture in {seconds}"), "VRChat's camera then flies around by itself.".into()));
    }
    if snap.capture.photo_3d && !matches!(snap.capture.state, CaptureState::Idle) {
        return Some(("3D picture in progress".into(), "The camera is flying. Stay where you are for a moment.".into()));
    }
    note.clone().or_else(|| snap.n4n0.photo_wait.map(panels::wait_words)).or_else(|| snap.n4n0.panel.clone().map(|said| ("N4N0".to_string(), said)))
}

/// What the end of a flight says.
fn flight_words(error: Option<&str>) -> (String, String) {
    match error {
        Some(error) => ("The 3D picture stopped".into(), error.to_string()),
        None => ("3D picture saved".into(), "It is in your captures. Orbiters makes it 3D next.".into()),
    }
}

/// When a visit was, so that two are told apart: the day and time this week, the date before that.
fn day_and_time(at: u64, now: u64) -> String {
    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    let Ok(then) = time::OffsetDateTime::from_unix_timestamp(at as i64) else { return String::new() };
    let then = then.to_offset(offset);
    if now.saturating_sub(at) >= 6 * 86_400 {
        return format!("{} {}", then.day(), &then.month().to_string()[..3]);
    }
    format!("{} {:02}:{:02}", &then.weekday().to_string()[..3], then.hour(), then.minute())
}

/// Where the person was, as the Instances sheet shows it: a row for each world, its last time first. `rows` come the newest first.
fn visited_worlds(rows: &[orbiters_n4n0_core::menu::InstanceRow], now: u64, pictures: &HashMap<String, egui::TextureId>) -> Vec<panels::WorldVisit> {
    let mut worlds: Vec<panels::WorldVisit> = Vec::new();
    for row in rows {
        let id = row.location.split(':').next().unwrap_or_default();
        let when = ago(now.saturating_sub(row.at));
        match worlds.iter_mut().find(|w| w.world_id == id) {
            Some(world) => world.earlier.push((row.location.clone(), day_and_time(row.at, now))),
            None => {
                let region = row.region().map_or(String::new(), |r| format!(" \u{b7} {r}"));
                worlds.push(panels::WorldVisit {
                    world_id: id.to_string(),
                    name: if row.world.is_empty() { "A world".to_string() } else { row.world.clone() },
                    line: format!("{when}{region} \u{b7} {}", row.kind()),
                    location: row.location.clone(),
                    earlier: Vec::new(),
                    picture: pictures.get(id).copied(),
                });
            }
        }
    }
    worlds
}

/// How long an app gets to do what it was asked before what it says is believed over what was asked: the computer's music is read once a second, and it read the old state just after a press.
const MEDIA_CATCHES_UP: Duration = Duration::from_millis(2500);

/// Whether what plays shows as playing: what was just asked (Enzo: "the play / pause button doesn't switch state correctly"), else what the app says.
fn shown_playing(read: bool, asked: Option<(bool, Instant)>, now: Instant) -> bool {
    asked.filter(|(_, at)| now.duration_since(*at) < MEDIA_CATCHES_UP).map_or(read, |(playing, _)| playing)
}

/// How fast a list thrown slows down: by e every this fraction of a second's inverse.
const THROW_SLOWS: f32 = 2.2;
/// How a list past its end is pulled back, and how much that is held back: a little under critical, so it settles softly.
const BOUNCE_PULL: f32 = 150.0;
const BOUNCE_DAMP: f32 = 21.0;
/// The speed a list runs past its end at, at most; and what a notch of the wheel at its end gives it.
const BOUNCE_SPEED: f32 = 1500.0;
const WHEEL_KICK: f32 = 420.0;

/// The fastest a list can be thrown, in its units a second.
const MAX_THROW: f32 = 6000.0;

/// A press on a list: it scrolls if the pointer moves, and presses what it was on if not.
#[derive(Clone, Copy, Debug)]
struct Drag {
    from: Option<f32>,
    scroll: f32,
    action: Option<Action>,
    moved: bool,
    /// On the scroll bar, which goes where the pointer is rather than by how far it moved.
    bar: bool,
    /// How fast the list goes now, in its units a second, for throwing it; where it was last, and when.
    speed: f32,
    last: Option<(Instant, f32)>,
}

impl Drag {
    /// How far the pointer goes before it is a drag and no press, in the page's units.
    const SLOP: f32 = 12.0;

    fn new(scroll: f32, action: Option<Action>, bar: bool) -> Self {
        Self { from: None, scroll, action, moved: bar, bar, speed: 0.0, last: None }
    }

    /// The pointer at `y`: where the list goes, when it goes. Past its ends it follows less and less.
    fn follow(&mut self, y: f32, max: f32, elastic: bool, now: Instant) -> Option<f32> {
        let from = *self.from.get_or_insert(y);
        self.moved |= (y - from).abs() > Self::SLOP;
        let wanted = self.scroll + from - y;
        let to = self.moved.then(|| if elastic { rubber_band(wanted, max) } else { wanted.clamp(0.0, max) })?;
        self.measure(to, now);
        Some(to)
    }

    /// How fast it goes: the last moments' speed, as what counts in a throw is how it left the hand.
    fn measure(&mut self, to: f32, now: Instant) {
        if let Some((at, was)) = self.last {
            let dt = now.duration_since(at).as_secs_f32();
            if dt > 0.001 {
                self.speed += ((to - was) / dt - self.speed) * (dt / 0.06).min(1.0);
                self.last = Some((now, to));
            }
        } else {
            self.last = Some((now, to));
        }
    }

    /// What it was thrown at when let go, in its units a second: a hand at rest throws nothing.
    fn throw(&self, now: Instant) -> f32 {
        let fresh = self.last.is_some_and(|(at, _)| now.duration_since(at) < Duration::from_millis(120));
        if !self.moved || self.bar || !fresh || self.speed.abs() < 200.0 {
            0.0
        } else {
            self.speed.clamp(-MAX_THROW, MAX_THROW)
        }
    }
}

/// How far a list is shown for a place `wanted` past its ends: all of it within them, and beyond less and less, as on an
/// iPhone or VRChat's own menus (Enzo: "a bouncing / elastic effect when reaching the limit").
fn rubber_band(wanted: f32, max: f32) -> f32 {
    // Enough to see that it gives, and never so much that the list seems to have gone.
    const FOLLOWS: f32 = 0.55;
    const AT_MOST: f32 = 60.0;
    let band = |past: f32| (1.0 - 1.0 / (past * FOLLOWS / AT_MOST + 1.0)) * AT_MOST;
    if wanted < 0.0 {
        -band(-wanted)
    } else if wanted > max {
        max + band(wanted - max)
    } else {
        wanted
    }
}

/// A frame of a list let go: where it is `x`, how fast it goes `v`, and where its wheel sent it `to`, from nought to `max`.
/// Thrown, it slows, runs past its end and is pulled back, without going beyond it the other way. With no give, it stops
/// there.
fn scroll_step(mut x: f32, mut v: f32, mut to: f32, max: f32, dt: f32, elastic: bool) -> (f32, f32, f32) {
    let within = |x: f32| x >= 0.0 && x <= max;
    if !elastic {
        if v != 0.0 {
            to = (x + v * dt).clamp(0.0, max);
            let stopped = to == 0.0 || to == max;
            x = to;
            v *= (-dt * THROW_SLOWS).exp();
            if v.abs() < 30.0 || stopped {
                v = 0.0;
            }
            return (x, v, to);
        }
        let off = to - x;
        return (if off.abs() < 0.5 { to } else { x + off * (1.0 - (-dt * 18.0).exp()) }, 0.0, to.clamp(0.0, max));
    }
    if v == 0.0 && within(x) {
        // Where its wheel sent it, or nowhere.
        let off = to - x;
        return (if off.abs() < 0.5 { to } else { x + off * (1.0 - (-dt * 18.0).exp()) }, 0.0, to.clamp(0.0, max));
    }
    // In small steps, so a slow frame does not fling it.
    let steps = (dt / (1.0 / 240.0)).ceil().clamp(1.0, 16.0);
    let h = dt / steps;
    let end_then = |x: f32| if x < 0.0 { 0.0 } else { max };
    for _ in 0..steps as u32 {
        if within(x) {
            v *= (-h * THROW_SLOWS).exp();
            let next = x + v * h;
            // What runs past its end goes on no faster than comes back in a moment.
            if !within(next) {
                v = v.clamp(-BOUNCE_SPEED, BOUNCE_SPEED);
            }
            x = next;
            if within(x) && v.abs() < 30.0 {
                v = 0.0;
                break;
            }
        } else {
            let end = end_then(x);
            let outside = x < 0.0;
            v += (-BOUNCE_PULL * (x - end) - BOUNCE_DAMP * v) * h;
            x += v * h;
            // Back at its end: it rests there.
            if (x >= end) == outside {
                x = end;
                v = 0.0;
                break;
            }
        }
    }
    // Barely past and barely moving: there.
    if !within(x) && (x - end_then(x)).abs() < 0.3 && v.abs() < 8.0 {
        x = end_then(x);
        v = 0.0;
    }
    (x, v, x.clamp(0.0, max))
}

/// How long what stopped a 3D picture stays said.
const NOTE_FOR: Duration = Duration::from_secs(8);

/// Says what stopped a 3D picture when the service says so, which comes numbered: the press itself shows nothing happening, and this is why.
#[derive(Debug, Default)]
struct PictureNote {
    seen: Option<u32>,
    until: Option<Instant>,
}

impl PictureNote {
    /// What the note becomes now, when that changes. A note from before these panels came up is old news.
    fn follow(&mut self, said: &Option<(u32, String)>, now: Instant) -> Option<Option<(String, String)>> {
        let number = said.as_ref().map_or(0, |(n, _)| *n);
        let first = self.seen.is_none();
        if self.seen != Some(number) {
            self.seen = Some(number);
            if let (false, Some((_, text))) = (first, said) {
                self.until = Some(now + NOTE_FOR);
                return Some(Some(note_words(text)));
            }
        }
        if self.until.is_some_and(|until| now >= until) {
            self.until = None;
            return Some(None);
        }
        None
    }
}

/// The note in the caption's two lines. VRChat's camera closed is the common one, and has a way out.
fn note_words(text: &str) -> (String, String) {
    if text == orbiters_n4n0::platform::CAMERA_CLOSED {
        ("Open VRChat's camera first".into(), "Quick Menu, the camera icon, Stream Camera. Then press this again.".into())
    } else {
        ("3D picture".into(), text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Enzo: "when I grab a tile to scroll down, it just opens it".
    #[test]
    fn a_press_that_moves_scrolls_and_one_that_stays_presses() {
        let t = Instant::now();
        let mut drag = Drag::new(100.0, Some(Action::Pictures), false);
        assert_eq!(drag.follow(300.0, 1000.0, true, t), None, "where it starts");
        assert_eq!(drag.follow(305.0, 1000.0, true, t), None, "a hand's shake is still a press");
        assert!(!drag.moved);
        assert_eq!(drag.follow(250.0, 1000.0, true, t), Some(150.0), "up the page, on down the list");
        assert!(drag.moved);
        assert_eq!(drag.follow(305.0, 1000.0, true, t), Some(95.0), "moved once, a drag for good");
        assert_eq!(drag.follow(900.0, 1000.0, false, t), Some(0.0), "with no give, not past the top");
        assert!(drag.follow(900.0, 1000.0, true, t).unwrap() < 0.0, "with it, a little past the top");
    }

    #[test]
    fn play_and_pause_show_what_was_asked_until_the_app_has_caught_up() {
        let t = Instant::now();
        // Pressed to pause: a read of the old state does not flip it back.
        let asked = Some((false, t));
        assert!(!shown_playing(true, asked, t + Duration::from_millis(900)));
        // After a while the app is believed, whichever it says.
        assert!(shown_playing(true, asked, t + MEDIA_CATCHES_UP));
        assert!(shown_playing(true, None, t));
        assert!(!shown_playing(false, None, t));
    }

    fn listed(key: &str, session: bool, has_view: bool) -> LoadedRecent {
        let path = PathBuf::from(key);
        LoadedRecent {
            source: if session { crate::worlds::Picture::Session(path.clone()) } else { crate::worlds::Picture::Shot { path: path.clone(), size: 1, width: 560 } },
            key: path,
            title: key.into(),
            image: None,
            asked: false,
            has_view,
            session,
            picture: session,
            taken: 0,
            modified: 0,
            aspect: 16.0 / 9.0,
            at: None,
        }
    }

    /// Enzo: "a general gallery with all the vrchat pictures and 3D captures shown together (we can have a filter to only show the 3D ones)".
    #[test]
    fn the_gallery_shows_all_or_the_captures_and_tiles_keep_to_what_they_are() {
        use panels::{GalleryFilter, RecentKind};
        let layout = Default::default();
        let mut recents = Recents { loaded: vec![listed("a.png", false, false), listed("cap", true, true), listed("b.png", false, false), listed("raw", true, false)], ..Default::default() };
        assert!(recents.arrange(GalleryFilter::All, layout));
        assert!(!recents.arrange(GalleryFilter::All, layout), "nothing changed: the same tiles");
        assert_eq!(recents.shown.iter().map(|r| r.kind).collect::<Vec<_>>(), [RecentKind::Picture, RecentKind::Capture, RecentKind::Picture, RecentKind::NoView]);
        assert!(recents.arrange(GalleryFilter::Captures, layout));
        assert_eq!(recents.shown.iter().map(|r| r.kind).collect::<Vec<_>>(), [RecentKind::Capture, RecentKind::NoView]);
        // A tile's number is its place among those shown.
        assert_eq!(recents.tile(0).unwrap().key, PathBuf::from("cap"));
        assert_eq!(recents.tile(1).unwrap().key, PathBuf::from("raw"));
        assert!(recents.tile(2).is_none());
    }

    #[test]
    fn pictures_far_from_the_page_are_let_go_and_no_longer_asked_for() {
        let mut recents = Recents { loaded: (0..10).map(|i| listed(&format!("{i}.png"), false, false)).collect(), ..Default::default() };
        recents.arrange(panels::GalleryFilter::All, Default::default());
        // None is read here: the readers wait on an empty queue once they have taken what they could not read.
        recents.ask(0, 3);
        assert_eq!(recents.asked.len(), 4);
        recents.ask(0, 3);
        assert_eq!(recents.asked.len(), 4, "asked once");
        recents.keep(2, 9);
        assert_eq!(recents.asked.len(), 2);
        assert!(!recents.loaded[0].asked && recents.loaded[3].asked);
        // Left: all goes.
        recents.keep(1, 0);
        assert!(recents.asked.is_empty());
    }

    #[test]
    #[ignore = "reads a real session: ORBIT_SESSION=<its folder>"]
    fn a_real_session_reads_for_the_3d_view() {
        let dir = PathBuf::from(std::env::var("ORBIT_SESSION").unwrap());
        let progress = super::super::worlds::catalog::Progress::default();
        let t = Instant::now();
        let capture = super::super::worlds::catalog::read_local(&dir, &progress).unwrap();
        println!("{} splats in {:?}; eye {:?} ground {} sky {} farthest {}", capture.gaussians.len(), t.elapsed(), capture.view.eye(), capture.view.ground(), capture.sky.is_some(), capture.farthest());
        assert!(capture.gaussians.len() > 1000);
        // Its start eye is where there are splats, as the distance sort says.
        assert!(capture.within(30.0) > capture.gaussians.len() / 4);
    }

    /// Enzo: "a bouncing / elastic effect when reaching the limit (top or bottom) or trying to scroll up when we're already all the way up".
    #[test]
    fn a_list_gives_at_its_ends_and_comes_back() {
        // Under the hand: all of it within its ends, less and less past them, and never far.
        assert_eq!(rubber_band(500.0, 1000.0), 500.0);
        assert!(rubber_band(-40.0, 1000.0) < 0.0 && rubber_band(-40.0, 1000.0) > -40.0);
        assert!(rubber_band(1040.0, 1000.0) > 1000.0 && rubber_band(1040.0, 1000.0) < 1040.0);
        assert!(rubber_band(-4000.0, 1000.0) > -60.0);
        assert!(rubber_band(-400.0, 1000.0) < rubber_band(-200.0, 1000.0), "the further, the further");
        // A list that does not scroll gives too.
        assert!(rubber_band(-40.0, 0.0) < 0.0 && rubber_band(40.0, 0.0) > 0.0);
        let run = |mut x: f32, mut v: f32, max: f32, elastic: bool| {
            let (mut to, mut furthest) = (x.clamp(0.0, max), x);
            for _ in 0..600 {
                (x, v, to) = scroll_step(x, v, to, max, 1.0 / 90.0, elastic);
                if (x - max / 2.0).abs() > (furthest - max / 2.0).abs() {
                    furthest = x;
                }
            }
            (x, v, furthest)
        };
        // Let go past the top, it comes back and rests at the very top, and not beyond it into the list.
        let (x, v, _) = run(-60.0, 0.0, 1000.0, true);
        assert_eq!((x, v), (0.0, 0.0));
        let (x, v, _) = run(1060.0, 0.0, 1000.0, true);
        assert_eq!((x, v), (1000.0, 0.0));
        // Thrown at its end, it runs past by something to see, and not by much, and comes to rest there.
        let (x, v, furthest) = run(900.0, 4000.0, 1000.0, true);
        assert_eq!((x, v), (1000.0, 0.0));
        assert!(furthest > 1015.0 && furthest < 1080.0, "{furthest}");
        let (x, _, furthest) = run(100.0, -4000.0, 1000.0, true);
        assert_eq!(x, 0.0);
        assert!(furthest < -15.0 && furthest > -80.0, "{furthest}");
        // Thrown with room, it slows and stops inside.
        let (x, v, furthest) = run(0.0, 900.0, 5000.0, true);
        assert_eq!(v, 0.0);
        assert!(x > 200.0 && x < 500.0 && furthest >= 0.0, "{x}");
        // With Reduce motion, no give at all.
        let (x, _, furthest) = run(900.0, 4000.0, 1000.0, false);
        assert_eq!(x, 1000.0);
        assert!(furthest <= 1000.0);
        // At rest nothing moves, and its wheel still glides.
        assert_eq!(scroll_step(300.0, 0.0, 300.0, 1000.0, 1.0 / 90.0, true), (300.0, 0.0, 300.0));
        let (x, _, to) = scroll_step(300.0, 0.0, 500.0, 1000.0, 1.0 / 90.0, true);
        assert!(x > 300.0 && x < 500.0 && to == 500.0);
        // A slow frame does not fling it.
        let (x, _, _) = scroll_step(1100.0, 0.0, 1000.0, 1000.0, 0.2, true);
        assert!((1000.0..1100.0).contains(&x));
    }

    /// Enzo: "be able to throw the scrolling so we can go faster".
    #[test]
    fn a_list_let_go_while_moving_is_thrown_and_one_let_go_at_rest_is_not() {
        let t = Instant::now();
        let at = |ms: u64| t + Duration::from_millis(ms);
        let mut drag = Drag::new(0.0, None, false);
        drag.follow(500.0, 9000.0, true, at(0));
        for (i, ms) in (10..=100).step_by(10).enumerate() {
            drag.follow(500.0 - 20.0 * (i as f32 + 1.0), 9000.0, true, at(ms));
        }
        let thrown = drag.throw(at(105));
        assert!(thrown > 1000.0, "down the list, about 2000 a second: {thrown}");
        // Held still a moment before letting go: nothing.
        assert_eq!(drag.throw(at(400)), 0.0);
        // A press that never moved, or the bar: nothing.
        assert_eq!(Drag::new(0.0, None, false).throw(at(0)), 0.0);
        let mut bar = Drag::new(0.0, None, true);
        bar.measure(0.0, at(0));
        bar.measure(800.0, at(50));
        assert_eq!(bar.throw(at(60)), 0.0);
    }

    #[test]
    fn where_the_person_was_is_a_row_for_each_world_the_last_time_first() {
        use orbiters_n4n0_core::menu::InstanceRow;
        let row = |world: &str, id: &str, instance: &str, at: u64| InstanceRow { location: format!("wrld_{id}:{instance}~region(eu)"), world: world.into(), at };
        let now = 100_000;
        let rows = [row("Rain", "a", "1", now - 300), row("Sky", "b", "2", now - 600), row("Rain", "a", "3", now - 1320), row("", "c", "4", now - 7200)];
        let worlds = visited_worlds(&rows, now, &HashMap::new());
        assert_eq!(worlds.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["Rain", "Sky", "A world"]);
        assert!(worlds[0].location.contains(":1~"), "the invite goes to the last time");
        assert_eq!(worlds[0].earlier.len(), 1, "the time before is a choice of its own");
        assert!(worlds[0].earlier[0].0.contains(":3~"));
        assert!(worlds[0].line.contains("EU") || worlds[0].line.contains("eu"), "{}", worlds[0].line);
        // All of them: the sheet scrolls.
        let many: Vec<InstanceRow> = (0..120).map(|i| row("W", &i.to_string(), "1", now - i)).collect();
        assert_eq!(visited_worlds(&many, now, &HashMap::new()).len(), 120);
    }

    #[test]
    fn the_end_of_a_flight_is_said_either_way() {
        assert_eq!(flight_words(None).0, "3D picture saved");
        let failed = flight_words(Some("no frames"));
        assert!(failed.0.contains("stopped") && failed.1 == "no frames");
    }

    #[test]
    fn with_no_menu_up_the_pill_says_what_a_picture_is_doing_and_otherwise_nothing() {
        use orbiters_n4n0::status::PhotoWait;
        let mut snap = Snapshot::new(Default::default());
        assert_eq!(away_words(&snap, &None), None, "nothing to say: nothing shows");
        snap.n4n0.photo_wait = Some(PhotoWait::Camera);
        assert!(away_words(&snap, &None).unwrap().0.contains("Stream Camera"));
        snap.n4n0.photo_wait = Some(PhotoWait::Move);
        assert!(away_words(&snap, &None).unwrap().0.contains("Move VRChat's camera"));
        // What stopped it goes before waiting; a countdown goes before anything.
        let note = Some(("a".to_string(), "b".to_string()));
        assert_eq!(away_words(&snap, &note).unwrap().0, "a");
        snap.n4n0.photo_countdown = Some(3);
        assert_eq!(away_words(&snap, &note).unwrap().0, "3D picture in 3");
    }

    #[test]
    fn a_picture_that_cannot_start_says_why_for_a_moment() {
        let t0 = Instant::now();
        let mut note = PictureNote::default();
        let closed = |n| Some((n, orbiters_n4n0::platform::CAMERA_CLOSED.to_string()));
        // What was said before these panels came up is not repeated.
        assert_eq!(note.follow(&closed(3), t0), None);
        assert_eq!(note.follow(&closed(3), t0 + Duration::from_secs(60)), None);
        // A press, the service refuses: said at once, with the way out.
        let said = note.follow(&closed(4), t0).expect("a change").expect("words");
        assert!(said.0.contains("camera") && said.1.contains("Stream Camera"), "{said:?}");
        assert_eq!(note.follow(&closed(4), t0 + Duration::from_secs(1)), None, "said once");
        // Then it goes by itself, and only once.
        assert_eq!(note.follow(&closed(4), t0 + NOTE_FOR), Some(None));
        assert_eq!(note.follow(&closed(4), t0 + NOTE_FOR + Duration::from_secs(1)), None);
        // Another reason says itself.
        let said = note.follow(&Some((5, "Your avatar is in it".into())), t0).expect("a change").expect("words");
        assert_eq!(said.1, "Your avatar is in it");
    }

    #[test]
    fn a_part_comes_on_a_spring_and_settles_home() {
        let mut a = Appear::new(0, true);
        assert_eq!(a.position(), 0.0);
        let (mut most, mut t) = (0.0f32, 0.0);
        while !a.settled() && t < 2.0 {
            a.step(1.0 / 60.0);
            most = most.max(a.position());
            t += 1.0 / 60.0;
        }
        assert!(a.settled() && t < 0.8, "settled in {t} s");
        assert!(most > 1.0 && most < 1.12, "a little past its place: {most}");
        assert_eq!(a.position(), 1.0);
    }

    #[test]
    fn parts_start_one_after_another_and_at_once_with_less_motion() {
        let mut later = Appear::new(5, true);
        later.step(0.016);
        assert_eq!(later.position(), 0.0, "its turn has not come");
        assert!(Appear::new(3, false).settled());
        assert_eq!(Appear::new(3, false).alpha(), 1.0);
    }

    #[test]
    fn a_long_turn_does_not_blow_the_spring_up() {
        let mut a = Appear::new(0, true);
        for _ in 0..60 {
            a.step(1.0);
            assert!(a.position().is_finite() && a.position() < 1.3);
        }
        assert!(a.settled());
    }

    #[test]
    fn a_second_ring_blooms_stays_while_pointed_at_and_goes_a_moment_after() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut b = Bloom::default();
        assert_eq!(b.step(Some(Zone::WheelWorlds), at(0)), None, "a cell that does not bloom");
        assert_eq!(b.step(Some(Zone::WheelPictures), at(10)), Some(Group::Pictures), "at once");
        assert_eq!(b.step(Some(Zone::BloomWorld), at(100)), Some(Group::Pictures), "on its cells");
        // Crossing the gap between two cells does not close it.
        assert_eq!(b.step(None, at(200)), Some(Group::Pictures));
        assert_eq!(b.step(Some(Zone::BloomPeople), at(300)), Some(Group::Pictures));
        assert_eq!(b.step(None, at(400)), Some(Group::Pictures));
        assert_eq!(b.step(None, at(400) + BLOOM_GRACE), None, "gone");
        // Another cell that blooms takes over at once.
        assert_eq!(b.step(Some(Zone::WheelPictures), at(2000)), Some(Group::Pictures));
        assert_eq!(b.step(Some(Zone::WheelPlaying), at(2100)), Some(Group::Playing));
    }

    #[test]
    fn how_long_ago_reads_in_a_word() {
        assert_eq!(ago(10), "just now");
        assert_eq!(ago(600), "10 min ago");
        assert_eq!(ago(7200), "2 h ago");
        assert_eq!(ago(200_000), "2 d ago");
    }

}
