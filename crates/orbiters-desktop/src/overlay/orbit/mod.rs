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
use orbiters_n4n0_core::menu::{Page, Target};
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

/// How far a cell of the ring comes out under the pointer, in units.
pub const POP: f32 = 7.0;
/// Its own spring: quicker than appearing, a little past.
const POP_SPRING: (f32, f32) = (620.0, 30.0);

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
    /// How far it has come out under the pointer, 0 to 1 and its speed. A press pushes it back in.
    pop: (f32, f32),
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
            pop: (0.0, 0.0),
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
        let live = self.placed.zone.ring().is_some();
        -(1.0 - self.appear.position()) * APPEAR_FROM + if live { self.pop.0 * POP } else { 0.0 }
    }

    fn step_pop(&mut self, dt: f32) {
        let target = if self.pressed.is_some() {
            -0.35
        } else if self.hover.is_some() {
            1.0
        } else {
            0.0
        };
        let mut left = dt.min(0.05);
        while left > 0.0 {
            let h = left.min(0.003);
            self.pop.1 += (POP_SPRING.0 * (target - self.pop.0) - POP_SPRING.1 * self.pop.1) * h;
            self.pop.0 += self.pop.1 * h;
            left -= h;
        }
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

/// Newest captures, read off the folder on a thread of their own as listing takes a moment: asked once for each change of the library.
#[derive(Default)]
struct Recents {
    loaded: Vec<LoadedRecent>,
    seen: Option<u64>,
    rx: Option<mpsc::Receiver<Vec<LoadedRecent>>>,
}

struct LoadedRecent {
    dir: PathBuf,
    title: String,
    image: Option<ColorImage>,
    has_view: bool,
    picture: bool,
    modified: u64,
}

impl Recents {
    fn follow(&mut self, version: u64, sessions: PathBuf) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.rx {
            if let Ok(list) = rx.try_recv() {
                self.loaded = list;
                self.rx = None;
                changed = true;
            }
        }
        if self.rx.is_none() && self.seen != Some(version) {
            self.seen = Some(version);
            let (tx, rx) = mpsc::channel();
            self.rx = Some(rx);
            let _ = std::thread::Builder::new().name("orbit-recent".into()).spawn(move || {
                let mut list = crate::controller::list_sessions(&sessions);
                list.sort_by_key(|s| std::cmp::Reverse(s.modified));
                let loaded = list
                    .into_iter()
                    .take(7)
                    .map(|s| {
                        let image = orbiters_core::bundle::read_thumbnail(&s.dir).map(|(w, h, rgba)| ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba));
                        let title = s.manifest.world.as_ref().and_then(|w| w.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| "Capture".to_string());
                        LoadedRecent { dir: s.dir, title, image, has_view: s.reconstruction.is_some(), picture: s.picture.is_some(), modified: s.modified }
                    })
                    .collect();
                let _ = tx.send(loaded);
            });
        }
        changed
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
    let kind = if r.picture { "3D picture" } else { "Recording" };
    if let Some(job) = snap.reconstruction.as_ref().filter(|j| j.session_dir == r.dir) {
        return format!("Making 3D {:.0}%", job.progress.clamp(0.0, 1.0) * 100.0);
    }
    if snap.reconstruct_queue.contains(&r.dir) {
        return format!("{kind}, waiting");
    }
    if r.has_view {
        format!("{kind}, {}", ago(now.saturating_sub(r.modified)))
    } else {
        kind.to_string()
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
        // A note from while these panels were away is old news.
        if around.is_none() {
            self.note = PictureNote::default();
            self.facts.note = None;
        }
        if around != self.open {
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
        let places: Vec<Placed> = places.into_iter().chain(back).collect();
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

    fn close_sheet(&mut self) {
        self.takeover = None;
        self.facts.sheet = None;
        self.facts.sheet_title.clear();
        self.list.retain(|p| p.zone != Zone::SheetBack);
        for panel in &mut self.list {
            panel.dirty = true;
        }
        self.show_keys = false;
    }

    /// A page of N4N0's menu on the sheet, from its Home as the menu starts there.
    fn open_page(&mut self, target: Target, snap: &Snapshot) {
        let title = match target {
            Target::YouTube => "YouTube",
            Target::Instances => "Previous instances",
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
        self.open_sheet(Sheet::Menu, title, snap);
        let host = &mut self.stage.host;
        let _ = host.menu.close();
        host.open(snap);
        let effects = host.press(target);
        host.effects(effects, snap, &self.backend);
    }

    /// Follows the service's state: the parts' facts, recent captures, what is on the sheet.
    pub fn follow(&mut self, snap: &Snapshot, library: u64) {
        if self.open.is_none() {
            return;
        }
        if self.recents.follow(library, snap.settings.sessions_dir.clone()) {
            for panel in &mut self.list {
                panel.dirty = true;
            }
            if let Some(panel) = self.takeover.as_mut() {
                panel.dirty = true;
            }
        }
        if let Some(note) = self.note.follow(&snap.n4n0.photo_note, Instant::now()) {
            self.facts.note = note;
            for panel in self.list.iter_mut().chain(self.takeover.iter_mut()) {
                panel.dirty = true;
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
        if let Some(worlds) = self.stage.worlds.as_mut() {
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
        Facts {
            recording,
            recording_seconds: recording.then(|| snap.capture.stats.as_ref().map_or(0, |s| s.elapsed_s as u32)),
            capture_busy: !matches!(snap.capture.state, CaptureState::Idle),
            countdown: snap.n4n0.photo_countdown,
            picture_flying: snap.capture.photo_3d && !matches!(snap.capture.state, CaptureState::Idle),
            // Their pictures are looked up per part, in `paint`.
            recent: self.recents.loaded.iter().map(|r| Recent { dir: r.dir.clone(), title: r.title.clone(), state: recent_state(r, snap, now), progress: progress(&r.dir), picture: None }).collect(),
            reconstructing: snap.reconstruction.as_ref().map(|j| j.progress.clamp(0.0, 1.0)),
            waiting: snap.reconstruct_queue.len(),
            desktop: snap.n4n0.desktop_mode || !snap.steamvr.running,
            fast_walk: snap.settings.walk_boost.enabled,
            n4n0: snap.settings.n4n0.enabled,
            music: menu.media.as_ref().map(|m| (m.title.clone(), m.playing)),
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
            note: self.facts.note.clone(),
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
            if panel.zone.passes_clicks() {
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
        for panel in self.list.iter_mut().chain(self.takeover.iter_mut()) {
            if let Some(i) = panel.hover {
                panel.pressed = Some(i);
                panel.dirty = true;
                action = panel.items.get(i).map(|item| item.action);
                break;
            }
        }
        let Some(action) = action else { return out };
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
            Action::Pictures => self.open_sheet(Sheet::Pictures, "3D pictures", snap),
            // It says why not, under the pointer.
            Action::Picture(_) if self.facts.no_kit => out.clicked = false,
            Action::Picture(mode) => {
                self.facts.picture_mode = Some(mode);
                for panel in self.list.iter_mut().chain(self.takeover.iter_mut()) {
                    panel.dirty = true;
                }
                let host = &mut self.stage.host;
                let effects = host.press(Target::TakeOf(mode));
                host.effects(effects, snap, &self.backend);
            }
            Action::Record => {
                if self.facts.recording {
                    self.backend.stop_capture();
                } else if !self.facts.capture_busy {
                    self.backend.start_capture();
                }
            }
            Action::Page(target) => self.open_page(target, snap),
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
                let effects = host.press(target);
                host.effects(effects, snap, &self.backend);
            }
            Action::Talk => {
                if self.facts.n4n0 {
                    self.talking = true;
                    self.backend.n4n0(N4n0Command::Talk { down: true });
                }
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
        out
    }

    /// The pointer's button went up.
    pub fn release(&mut self) {
        if std::mem::take(&mut self.talking) {
            self.backend.n4n0(N4n0Command::Talk { down: false });
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
            if matches!(panel.zone, Zone::BloomRecent) {
                for (recent, loaded) in facts.recent.iter_mut().zip(&self.recents.loaded) {
                    if let Some(image) = &loaded.image {
                        let ctx = panel.canvas.ctx();
                        let name = loaded.dir.to_string_lossy().to_string();
                        recent.picture = Some(panel.pictures.entry(name).or_insert_with(|| ctx.load_texture("recent", image.clone(), TextureOptions::LINEAR)).id());
                    }
                }
            }
            if matches!(panel.zone, Zone::WheelPlaying | Zone::QmWing) {
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
                    stage.canvas.paint(time, |p| {
                        if retro {
                            menu_retro::draw(p, pos2(0.0, 0.0), &view);
                        } else {
                            menu_ui::draw(p, pos2(0.0, 0.0), &view);
                        }
                        if cover {
                            type_hint(p);
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
                    for (recent, loaded) in facts.recent.iter_mut().zip(&self.recents.loaded) {
                        if let Some(image) = &loaded.image {
                            let ctx = stage.canvas.ctx();
                            let name = loaded.dir.to_string_lossy().to_string();
                            recent.picture = Some(panel.pictures.entry(name).or_insert_with(|| ctx.load_texture("recent", image.clone(), TextureOptions::LINEAR)).id());
                        }
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
        for panel in self.list.iter_mut().filter(|p| p.zone == Zone::QmCaption) {
            panel.shown = caption;
        }
        for panel in self.list.iter_mut().chain(self.takeover.iter_mut()) {
            panel.appear.step(dt);
            panel.step_pop(dt);
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
fn type_hint(p: &egui::Painter) {
    use orbiters_ui::{icons, theme, widgets};
    let area = egui::Rect::from_min_max(pos2(0.0, KEYS_TOP), pos2(layout::WIDTH as f32, layout::HEIGHT as f32));
    p.rect_filled(area, 0.0, panels::look::OPAQUE);
    widgets::icon(p, area.center() - vec2(0.0, 26.0), icons::KEYBOARD, 56.0, panels::look::LABEL);
    p.text(area.center() + vec2(0.0, 24.0), egui::Align2::CENTER_CENTER, "Type on your keyboard", theme::font(22.0, theme::weight::BOLD), panels::look::LABEL);
    p.text(area.center() + vec2(0.0, 54.0), egui::Align2::CENTER_CENTER, "Enter searches, Esc goes back", theme::font(15.0, theme::weight::MEDIUM), panels::look::MUTED);
}

impl Drop for Panels {
    fn drop(&mut self) {
        self.release();
        if self.reported.is_some_and(|r| r.0) {
            self.backend.n4n0(N4n0Command::MenuState { open: false, dictation: false, keyboard: false });
        }
    }
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
