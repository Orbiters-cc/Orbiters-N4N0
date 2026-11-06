//! The Orbiters menu around VRChat's own menus: panels around the Action Menu's wheel and the Quick Menu, showing as they open and gone as
//! they close, so that 3D pictures, recording, recent captures and N4N0's pages are right where the person already is (docs/orbit-overlays.md).
//!
//! The same panels are drawn for both modes: a borderless topmost window each in desktop mode ([`desktop`]), a SteamVR overlay each in VR ([`vr`]).
//! What is common is here: [`Panels`] paints them and does what they are pressed for; [`zones`] says where they go, [`panels`] what they hold,
//! [`detect`] how a menu is known to be open on the desktop, [`block`] how the trigger is taken from VRChat in VR.
//!
//! **Nothing here touches the game**: no injection, no reading of its memory, ordinary windows and overlays, its own window's picture through Windows
//! Graphics Capture only while a menu is showing or a key has just asked for one. Closed, nothing is painted, and no window or overlay shows.

pub mod block;
pub mod detect;
#[cfg(windows)]
pub mod desktop;
pub mod panels;
pub mod vr;
pub mod zones;

use super::menu_retro;
use super::menu_ui::{self, Skin, View};
use super::Canvas;
use crate::backend::Ctl;
use crate::ipc::N4n0Command;
use crate::menu_host::MenuHost;
use crate::state::{CaptureState, Snapshot};
use egui::{pos2, vec2, ColorImage, Pos2, TextureHandle, TextureId, TextureOptions, Vec2};
use orbiters_core::settings::MenuLook;
use orbiters_n4n0_core::menu::layout::{self, ScreenPage};
use panels::{Action, Facts, Item, Recent};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;
use zones::{Around, Placed, Zone};

/// Whether the desktop's panels are wanted: VRChat runs without SteamVR, on this computer, and the setting says so.
pub fn wanted_on_desktop(snap: &Snapshot) -> bool {
    orbiters_core::osc::VRCHAT_RUNS_HERE && snap.vrchat.running && !snap.vrchat.remote && !snap.steamvr.running && snap.settings.orbit.any()
}

/// A panel's appearing: a spring from nothing to its place, a little past it. Each turn moves the window or overlay and its alpha, no new picture.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Appear {
    position: f32,
    velocity: f32,
    /// Seconds before it starts.
    wait: f32,
}

/// The spring's stiffness and damping: settled in about 300 ms, about 6% past its place on the way.
const SPRING: (f32, f32) = (320.0, 24.0);
/// How far a panel comes from, toward the menu's centre, in units.
pub const APPEAR_FROM: f32 = 22.0;
/// Between one panel's start and the next's.
const APPEAR_STAGGER: f32 = 0.016;

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

/// One panel: where it is and its picture.
pub struct Panel {
    pub zone: Zone,
    pub placed: Placed,
    /// Its picture's size in pixels.
    pub px: [u32; 2],
    /// Its size in the units its content is drawn in.
    pub logical: Vec2,
    canvas: Canvas,
    /// The picture as the platform takes it: the canvas's, with the wheel's bite cut out.
    out: Vec<u8>,
    /// Where the wheel's bite is in its pixels.
    bite: Option<(Pos2, f32)>,
    pub items: Vec<Item>,
    pub hover: Option<usize>,
    pub pressed: Option<usize>,
    /// The next paint must draw.
    pub dirty: bool,
    pub appear: Appear,
    /// A shown picture: the platform has it.
    pub painted: bool,
    /// Textures for recent captures, by folder.
    pictures: HashMap<PathBuf, TextureHandle>,
}

impl Panel {
    /// `sharp` pixels of picture a pixel of the placed rectangle (1 on the desktop; more in the headset, whose overlays are looked at closely).
    fn new(placed: Placed, sharp: f32, order: usize, live: bool) -> Self {
        let size = placed.rect.size() * sharp;
        let px = [size.x.round().max(1.0) as u32, size.y.round().max(1.0) as u32];
        let logical = if placed.zone == Zone::Takeover { vec2(layout::WIDTH as f32, layout::HEIGHT as f32) } else { placed.logical() };
        let zoom = px[0] as f32 / logical.x;
        let bite = placed.zone.bite().map(|b| (((placed.center - placed.rect.min) * sharp).to_pos2(), b * placed.unit * sharp));
        Self {
            zone: placed.zone,
            placed,
            px,
            logical,
            canvas: Canvas::zoomed(px[0], px[1], zoom),
            out: vec![0; px[0] as usize * px[1] as usize * 4],
            bite,
            items: Vec::new(),
            hover: None,
            pressed: None,
            dirty: true,
            appear: Appear::new(order, live),
            painted: false,
            pictures: HashMap::new(),
        }
    }

    /// The picture (RGBA, premultiplied), as painted last.
    pub fn pixels(&self) -> &[u8] {
        &self.out
    }

    /// Takes what the canvas painted: the changed part, cut where the wheel bites. Returns it (x0, y0, x1, y1).
    fn take_picture(&mut self) -> Option<[u32; 4]> {
        let changed = self.canvas.damage()?;
        let [w, h] = self.px;
        let pixels = self.canvas.pixels();
        for y in changed[1]..changed[3].min(h) {
            let (a, b) = ((y * w + changed[0]) as usize * 4, (y * w + changed[2].min(w)) as usize * 4);
            self.out[a..b].copy_from_slice(&pixels[a..b]);
        }
        if let Some((center, radius)) = self.bite {
            panels::bite(&mut self.out, self.px, center, radius, changed);
        }
        Some(changed)
    }

    /// `uv` over the panel (0 to 1 from its top left) in the units its content is drawn in.
    pub fn to_logical(&self, uv: [f32; 2]) -> Pos2 {
        pos2(uv[0] * self.logical.x, uv[1] * self.logical.y)
    }
}

/// What a press on a panel asks of the platform beyond the service.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pressed {
    /// A press happened (the click's sound).
    pub clicked: bool,
    /// The panels go for this opening.
    pub hide: bool,
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
                    .take(4)
                    .map(|s| {
                        let image = orbiters_core::bundle::read_thumbnail(&s.dir).map(|(w, h, rgba)| ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba));
                        let title = s.manifest.world.as_ref().and_then(|w| w.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| "Capture".to_string());
                        LoadedRecent { dir: s.dir, title, image, has_view: s.reconstruction.is_some(), picture: s.picture.is_some() }
                    })
                    .collect();
                let _ = tx.send(loaded);
            });
        }
        changed
    }
}

/// What the state of a recent capture reads as.
fn recent_state(r: &LoadedRecent, snap: &Snapshot) -> String {
    let kind = if r.picture { "3D picture" } else { "Recording" };
    if let Some(job) = snap.reconstruction.as_ref().filter(|j| j.session_dir == r.dir) {
        return format!("Reconstructing {:.0}%", job.progress.clamp(0.0, 1.0) * 100.0);
    }
    if snap.reconstruct_queue.contains(&r.dir) {
        return format!("{kind}, waiting");
    }
    if r.has_view {
        format!("{kind}, ready")
    } else {
        kind.to_string()
    }
}

/// The panels of whichever menu is open, and what they do. The platform says what is open, where the pointer is and when it presses; it takes the pictures.
pub struct Panels {
    backend: Ctl,
    /// N4N0's menu, as drawn in front of the Quick Menu and for what it does (a 3D picture, fast walk). Its pictures are textures of `menu_ctx`.
    host: MenuHost,
    menu_ctx: egui::Context,
    list: Vec<Panel>,
    pub takeover: Option<Panel>,
    open: Option<Around>,
    recents: Recents,
    pub facts: Facts,
    /// Pixels of picture a placed pixel.
    sharp: f32,
    /// The person's Hide: nothing shows for this opening.
    hidden: bool,
    /// Talk is held.
    talking: bool,
    started: Instant,
    /// Where the takeover's menu looks retro.
    retro: bool,
    menu_opened: bool,
    /// Where N4N0's menu goes when pressed for (the platform's: the window's, or the headset's plane).
    pub takeover_place: Option<Placed>,
    /// What the service was last told of the takeover (open, dictation, keyboard).
    reported: Option<(bool, bool, bool)>,
}

impl Panels {
    pub fn new(backend: Ctl, sharp: f32) -> Self {
        let menu_ctx = egui::Context::default();
        orbiters_ui::install(&menu_ctx);
        let host = MenuHost::new(menu_ctx.clone());
        Self {
            backend,
            host,
            menu_ctx,
            list: Vec::new(),
            takeover: None,
            open: None,
            recents: Recents::default(),
            facts: Facts::default(),
            sharp,
            hidden: false,
            talking: false,
            started: Instant::now(),
            retro: false,
            menu_opened: false,
            takeover_place: None,
            reported: None,
        }
    }

    pub fn open_menu(&self) -> Option<Around> {
        self.open
    }

    pub fn panels(&self) -> &[Panel] {
        &self.list
    }

    pub fn panels_mut(&mut self) -> &mut [Panel] {
        &mut self.list
    }

    /// Nothing shows.
    pub fn is_empty(&self) -> bool {
        self.list.is_empty() && self.takeover.is_none()
    }

    /// Every panel and the takeover in front (last).
    pub fn all(&self) -> impl Iterator<Item = &Panel> {
        self.list.iter().chain(self.takeover.iter())
    }

    /// What is open: panels for it in the places `places` gives, or none. Hide lasts until the menu closes. Whatever was held lets go.
    pub fn set_open(&mut self, around: Option<Around>, places: Vec<Placed>, snap: &Snapshot) {
        if around != self.open {
            self.hidden = false;
            self.release();
            self.takeover = None;
            self.open = around;
            self.list.clear();
        }
        if self.hidden || around.is_none() {
            self.list.clear();
            self.takeover = None;
            return;
        }
        // Moved or another size: the panels of a size stay as they are, those changed are made anew.
        let live = !snap.settings.reduce_motion;
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
        if !self.menu_opened {
            // The menu's logic is what a 3D picture or fast walk goes through: it wants to be open, on its Home, whether it is drawn or not.
            self.host.open(snap);
            self.menu_opened = true;
        }
    }

    /// A menu's page in front of the Quick Menu.
    fn open_page(&mut self, target: orbiters_n4n0_core::menu::Target, place: Option<Placed>, snap: &Snapshot) {
        let Some(place) = place else { return };
        self.host.open(snap);
        let effects = self.host.press(target);
        self.host.effects(effects, snap, &self.backend);
        let mut panel = Panel::new(place, self.sharp, 0, !snap.settings.reduce_motion);
        panel.canvas = Canvas::zoomed(panel.px[0], panel.px[1], panel.px[0] as f32 / panel.logical.x);
        self.takeover = Some(panel);
        self.host.dirty = true;
    }

    /// Follows the service's state: the panels' facts, recent captures, N4N0's menu in front of the Quick Menu when it is. Returns whether anything is to be painted.
    pub fn follow(&mut self, snap: &Snapshot, library: u64) {
        if self.open.is_none() {
            return;
        }
        if self.recents.follow(library, snap.settings.sessions_dir.clone()) {
            for panel in &mut self.list {
                panel.dirty = true;
            }
        }
        self.host.follow(snap);
        self.host.follow_media();
        let facts = self.facts_of(snap);
        if facts != self.facts {
            self.facts = facts;
            for panel in &mut self.list {
                panel.dirty = true;
            }
        }
    }

    fn facts_of(&self, snap: &Snapshot) -> Facts {
        let recording = matches!(snap.capture.state, CaptureState::Recording | CaptureState::Paused) && !snap.capture.photo_3d;
        Facts {
            recording,
            recording_seconds: recording.then(|| snap.capture.stats.as_ref().map_or(0, |s| s.elapsed_s as u32)),
            capture_busy: !matches!(snap.capture.state, CaptureState::Idle),
            countdown: snap.n4n0.photo_countdown,
            picture_flying: snap.capture.photo_3d && !matches!(snap.capture.state, CaptureState::Idle),
            // Their pictures are looked up per panel, in `paint`.
            recent: self.recents.loaded.iter().map(|r| Recent { dir: r.dir.clone(), title: r.title.clone(), state: recent_state(r, snap), picture: None }).collect(),
            reconstructing: snap.reconstruction.as_ref().map(|j| j.progress.clamp(0.0, 1.0)),
            waiting: snap.reconstruct_queue.len(),
            desktop: snap.n4n0.desktop_mode || !snap.steamvr.running,
            fast_walk: snap.settings.walk_boost.enabled,
            n4n0: snap.settings.n4n0.enabled,
            music: self.host.menu.media.as_ref().map(|m| (m.title.clone(), m.playing)),
            timer: snap.settings.photo_3d_timer,
        }
    }

    /// A pointer on a panel (`on`: its index in [`Panels::all`], and where in units), or on none.
    pub fn hover(&mut self, on: Option<(usize, Pos2)>) {
        let count = self.list.len();
        for (i, panel) in self.list.iter_mut().chain(self.takeover.iter_mut()).enumerate() {
            let hover = on.filter(|(at, _)| *at == i).and_then(|(_, p)| {
                if i == count {
                    let page = layout::cached(ScreenPage::of(&self.host.menu));
                    layout::hit_pointable(page, &self.host.menu, p.x, p.y).map(|(element, _)| element)
                } else {
                    panels::hit(&panel.items, p)
                }
            });
            if hover != panel.hover {
                panel.hover = hover;
                panel.dirty = true;
                if i == count {
                    self.host.dirty = true;
                }
            }
        }
    }

    /// The pointer's button went down on what it hovers.
    pub fn press(&mut self, snap: &Snapshot) -> Pressed {
        let mut out = Pressed::default();
        // In front of everything, N4N0's own menu takes it.
        if let Some(panel) = self.takeover.as_mut() {
            if let Some(element) = panel.hover {
                let page = layout::cached(ScreenPage::of(&self.host.menu));
                if let Some(e) = page.elements.get(element) {
                    panel.pressed = Some(element);
                    out.clicked = true;
                    let effects = self.host.press(e.target);
                    if self.host.effects(effects, snap, &self.backend) || !self.host.menu.open {
                        self.takeover = None;
                        self.host.open(snap);
                    }
                    self.host.dirty = true;
                }
                return out;
            }
        }
        let mut action = None;
        for panel in &mut self.list {
            if let Some(i) = panel.hover {
                panel.pressed = Some(i);
                panel.dirty = true;
                action = Some(panel.items[i].action);
                break;
            }
        }
        let Some(action) = action else { return out };
        out.clicked = action != Action::Idle;
        match action {
            Action::Picture(mode) => {
                let effects = self.host.press(orbiters_n4n0_core::menu::Target::TakeOf(mode));
                self.host.effects(effects, snap, &self.backend);
            }
            Action::Record => {
                if self.facts.recording {
                    self.backend.stop_capture();
                } else if !self.facts.capture_busy {
                    self.backend.start_capture();
                }
            }
            Action::Page(target) => {
                self.open_page(target, self.takeover_place, snap);
            }
            Action::FastWalk => {
                let effects = self.host.press(orbiters_n4n0_core::menu::Target::FastWalk);
                self.host.effects(effects, snap, &self.backend);
            }
            Action::PlayPause => {
                let effects = self.host.press(orbiters_n4n0_core::menu::Target::MediaPlayPause);
                self.host.effects(effects, snap, &self.backend);
            }
            Action::Talk => {
                if self.facts.n4n0 {
                    self.talking = true;
                    self.backend.n4n0(N4n0Command::Talk { down: true });
                }
            }
            Action::OpenApp => self.backend.show_window(),
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
        self.host.dirty = true;
    }

    /// Paints what changed and hands each new picture to `show`, with the part that changed. Nothing changed: nothing happens, at no cost.
    pub fn paint(&mut self, snap: &Snapshot, mut show: impl FnMut(&Panel, [u32; 4])) {
        let time = self.started.elapsed().as_secs_f64();
        let live = !snap.settings.reduce_motion;
        for panel in &mut self.list {
            if !panel.dirty {
                continue;
            }
            let first = !panel.painted;
            panel.dirty = false;
            panel.items = panels::items(panel.zone, panel.logical, &self.facts);
            let mut facts = self.facts.clone();
            if matches!(panel.zone, Zone::WheelLeft | Zone::QmLeft) {
                // Recent captures' pictures: textures of this panel's own canvas.
                for (recent, loaded) in facts.recent.iter_mut().zip(&self.recents.loaded) {
                    if let Some(image) = &loaded.image {
                        let ctx = panel.canvas.ctx();
                        let texture = panel.pictures.entry(loaded.dir.clone()).or_insert_with(|| ctx.load_texture("recent", image.clone(), TextureOptions::LINEAR));
                        recent.picture = Some(texture.id());
                    }
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
        if self.takeover.is_some() {
            let retro_now = snap.settings.n4n0.menu.look == MenuLook::Retro;
            if retro_now != self.retro {
                self.retro = retro_now;
                self.host.dirty = true;
            }
            let animating = self.host.menu.listening || self.host.menu.popping().is_some();
            if self.host.dirty || animating {
                self.host.dirty = false;
                let (host, retro) = (&self.host, self.retro);
                if let Some(panel) = self.takeover.as_mut() {
                    let (hover, pressed) = (panel.hover, panel.pressed);
                    let picture = |id: &str| -> Option<TextureId> { host.picture(id, retro) };
                    let skin = Skin::english();
                    let view = View { menu: &host.menu, hover, pressed, picture: &picture, time, skin };
                    panel.canvas.paint(time, |p| {
                        if retro {
                            menu_retro::draw(p, pos2(0.0, 0.0), &view);
                        } else {
                            menu_ui::draw(p, pos2(0.0, 0.0), &view);
                        }
                    });
                    if let Some(changed) = panel.take_picture() {
                        panel.painted = true;
                        show(panel, changed);
                    }
                }
            }
        }
    }

    /// Takes the menu's own business for a turn while it is in front of the Quick Menu: what it fetches, what comes back, and telling the service
    /// (it listens, and keys go to the keyboard page).
    pub fn menu_turn(&mut self, snap: &Snapshot) {
        let state = self.takeover.is_some().then(|| (true, self.host.menu.takes_speech(), self.host.menu.shows_keyboard())).unwrap_or((false, false, false));
        if self.reported.unwrap_or((false, false, false)) != state {
            self.reported = Some(state);
            self.backend.n4n0(N4n0Command::MenuState { open: state.0, dictation: state.1, keyboard: state.2 });
        }
        if self.takeover.is_none() {
            return;
        }
        let mut effects = self.host.take_keys(&snap.n4n0);
        effects.extend(self.host.poll());
        if self.host.effects(effects, snap, &self.backend) {
            self.takeover = None;
            self.host.open(snap);
        }
    }

    /// Whether any picture is due or any panel still moving.
    pub fn busy(&self) -> bool {
        self.all().any(|p| p.dirty || !p.appear.settled()) || self.host.dirty
    }

    /// Moves each panel's spring on.
    pub fn step_springs(&mut self, dt: f32) {
        for panel in self.list.iter_mut().chain(self.takeover.iter_mut()) {
            panel.appear.step(dt);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panel_comes_on_a_spring_and_settles_home() {
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
    fn panels_start_one_after_another_and_at_once_with_less_motion() {
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
}
