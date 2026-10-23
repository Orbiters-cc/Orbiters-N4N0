//! Orb's menu in the play space (when the avatar has no Orb screen): a SteamVR overlay that opens half a metre in
//! front of the headset, upright and facing it, when the person calls Orb (the controller binding, the wake word on
//! its own, a hand on the orb). SteamVR's laser points at it whenever it shows. It draws the same pages as the
//! avatar's screen ([`super::menu_ui`]) and runs the same menu (`orbiters_orb::menu`): YouTube is fetched here, the
//! rest (calls, the clipboard, a 3D picture, fast walk) goes to the tray service.

use super::menu_retro;
use super::menu_ui::{self, View};
use super::{Canvas, Presenter};
use crate::backend::Ctl;
use crate::controller::screen as shared;
use crate::ipc::OrbCommand;
use crate::state::{CallLink, CallPhase, Snapshot};
use egui::{pos2, ColorImage, TextureHandle, TextureId, TextureOptions};
use orbiters_core::settings::MenuLook;
use orbiters_openvr::{self as vr, overlay_flag, Matrix34, Pose, Universe, MAX_DEVICES};
use orbiters_orb::menu::layout::{self, ScreenPage};
use orbiters_orb::menu::{Call, Contact, Effect, Menu, Page, SuggestQueue};
use orbiters_orb::{picture, youtube};
use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const WIDTH: u32 = layout::WIDTH as u32;
const HEIGHT: u32 = layout::HEIGHT as u32;
/// The menu's width in the play space.
pub const WIDTH_M: f32 = 0.62;
/// How far in front of the eyes, and how far below them, it opens.
const DISTANCE_M: f32 = 0.5;
const BELOW_EYES_M: f32 = 0.16;
/// The menu closes after this long without a press or a word.
const IDLE_CLOSE: Duration = Duration::from_secs(120);
/// A thumbnail that failed is tried again after this (times the tries), so many times.
const THUMB_RETRY: Duration = Duration::from_secs(3);
const THUMB_TRIES: u8 = 3;

enum Done {
    Suggestions { id: u64, text: String, result: anyhow::Result<Vec<String>> },
    Results { id: u64, more: bool, result: anyhow::Result<youtube::Results> },
    Thumbnail { id: String, tries: u8, image: Option<ColorImage> },
}

enum Thumb {
    Loading,
    /// The picture, and the same as the retro look's green dots.
    Ready(TextureHandle, TextureHandle),
    /// The last try failed at `at`.
    Failed {
        at: Instant,
        tries: u8,
    },
}

/// Sets `slot` to `value`, saying whether that changed it.
fn set<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}

pub struct MenuOverlay {
    handle: vr::OverlayHandle,
    canvas: Canvas,
    pub menu: Menu,
    hover: Option<usize>,
    pressed: Option<usize>,
    thumbs: HashMap<String, Thumb>,
    suggest: SuggestQueue,
    done_tx: mpsc::Sender<Done>,
    done_rx: mpsc::Receiver<Done>,
    shown: bool,
    dirty: bool,
    last_paint: Option<Instant>,
    last_activity: Instant,
    /// The service's numbered requests and dictations seen so far (the first snapshot sets them).
    request_seen: Option<u32>,
    dictation_seen: Option<u32>,
    /// What the service was last told (open, dictation).
    reported: Option<(bool, bool)>,
    /// When the menu last closed: a request to close it that comes just after (sent before the service heard) finds
    /// it closed, and must not open it again.
    closed_at: Option<Instant>,
    /// The media reading the menu shows (its version: [`crate::media::Watcher::reading`]).
    media_seen: u64,
    /// Where the menu was opened (it stays there), and whether the head is behind it: it then shows its back.
    placed: Option<Matrix34>,
    behind: bool,
    /// The menu is drawn in the retro look (Orbiters desktop's Orb › Orb's menu › Look).
    retro: bool,
}

/// A toggling request this soon after the menu closed is the close it meant, already done.
const TOGGLE_AFTER_CLOSE: Duration = Duration::from_millis(500);

/// How far past the overlay's plane (metres) the head goes before it shows the other side: standing in the plane
/// doesn't flicker it.
const SIDE_MARGIN_M: f32 = 0.02;

/// Whether `head` is behind the overlay at `placed` (its +Z faces the front), `was` the last answer.
pub fn behind(placed: &Matrix34, head: &Matrix34, was: bool) -> bool {
    let (o, h) = (&placed.m, &head.m);
    let d: f32 = (0..3).map(|i| (h[i][3] - o[i][3]) * o[i][2]).sum();
    if was {
        d < SIDE_MARGIN_M
    } else {
        d < -SIDE_MARGIN_M
    }
}

/// The overlay's transform: `distance` in front of `head` (level, along where it looks), `below` lower, facing it.
pub fn in_front(head: &Matrix34, distance: f32, below: f32) -> Matrix34 {
    let m = head.m;
    // The head looks along its −Z; level it.
    let (fx, fz) = (-m[0][2], -m[2][2]);
    let length = (fx * fx + fz * fz).sqrt();
    let (fx, fz) = if length > 1e-4 { (fx / length, fz / length) } else { (0.0, -1.0) };
    let position = [m[0][3] + fx * distance, m[1][3] - below, m[2][3] + fz * distance];
    // The overlay faces its +Z: back toward the head. X = Y × Z.
    let z = [-fx, 0.0, -fz];
    let x = [z[2], 0.0, -z[0]];
    Matrix34 { m: [[x[0], 0.0, z[0], position[0]], [x[1], 1.0, z[1], position[1]], [x[2], 0.0, z[2], position[2]]] }
}

impl MenuOverlay {
    pub fn new(overlay: &vr::Overlay, key: &str) -> anyhow::Result<Self> {
        let fail = |what: &str, code: i32| anyhow::anyhow!("{what}: SteamVR overlay error {code}");
        let handle = overlay.create(&format!("{key}.menu"), "Orb's menu").map_err(|c| fail("menu", c))?;
        let _ = overlay.set_width_in_meters(handle, WIDTH_M);
        overlay.set_mouse_input(handle).map_err(|c| fail("menu input", c))?;
        overlay.set_mouse_scale(handle, WIDTH as f32, HEIGHT as f32).map_err(|c| fail("menu mouse scale", c))?;
        let _ = overlay.set_flag(handle, overlay_flag::MAKE_OVERLAYS_INTERACTIVE_IF_VISIBLE, true);
        let _ = overlay.set_flag(handle, overlay_flag::ENABLE_CLICK_STABILIZATION, true);
        let _ = overlay.set_flag(handle, overlay_flag::IS_PREMULTIPLIED, true);
        let _ = overlay.set_sort_order(handle, 10);
        let (done_tx, done_rx) = mpsc::channel();
        Ok(Self {
            handle,
            canvas: Canvas::new(WIDTH, HEIGHT),
            menu: Menu::default(),
            hover: None,
            pressed: None,
            thumbs: HashMap::new(),
            suggest: SuggestQueue::default(),
            done_tx,
            done_rx,
            shown: false,
            dirty: true,
            last_paint: None,
            last_activity: Instant::now(),
            request_seen: None,
            dictation_seen: None,
            reported: None,
            closed_at: None,
            media_seen: 0,
            placed: None,
            behind: false,
            retro: false,
        })
    }

    pub fn destroy(&self, overlay: &vr::Overlay) {
        let _ = overlay.destroy(self.handle);
    }

    fn open(&mut self, overlay: &vr::Overlay, steamvr: &vr::Client, snap: &Snapshot) {
        let mut poses = [Pose::default(); MAX_DEVICES];
        steamvr.poses(Universe::Standing, &mut poses);
        let head = if poses[0].pose_is_valid {
            poses[0].device_to_absolute
        } else {
            Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.6], [0.0, 0.0, 1.0, 0.0]] }
        };
        let placed = in_front(&head, DISTANCE_M, BELOW_EYES_M);
        let _ = overlay.set_transform_absolute(self.handle, &placed);
        self.placed = Some(placed);
        if self.behind {
            self.behind = false;
            let _ = overlay.set_texture_bounds(self.handle, 0.0, 0.0, 1.0, 1.0);
        }
        let settings = &snap.settings;
        self.menu.layout = shared::keyboard(settings.orb.menu.keyboard);
        self.menu.photo_timer = settings.photo_3d_timer;
        self.menu.photo_length = shared::photo_length(settings.photo_3d);
        // The overlay draws every page: Home has its six tiles.
        self.menu.six_tiles = true;
        self.menu.open();
        self.hover = None;
        self.pressed = None;
        self.last_activity = Instant::now();
        self.dirty = true;
    }

    fn close(&mut self, overlay: &vr::Overlay) {
        // (The menu's own close, idle or its button, closed `menu` already: this hides the overlay.)
        self.closed_at = Some(Instant::now());
        self.menu.close();
        if self.shown {
            let _ = overlay.hide(self.handle);
            self.shown = false;
        }
    }

    /// Keeps the menu's picture of what the service knows (calls, fast walk, listening, the picture's countdown),
    /// field by field: the menu (its results and suggestions included) is not copied every frame to find a change.
    fn follow(&mut self, snap: &Snapshot) {
        let m = &mut self.menu;
        let mut changed = set(&mut m.listening, snap.orb.listening);
        // What Orb says shows on Home too.
        if let Some(panel) = &snap.orb.panel {
            if m.heard_private || !m.heard.as_deref().is_some_and(|h| h.split_whitespace().eq(panel.split_whitespace())) {
                m.heard = Some(panel.split_whitespace().collect::<Vec<_>>().join(" "));
                m.heard_private = false;
                changed = true;
            }
        }
        changed |= set(&mut m.fast_walk, snap.settings.walk_boost.enabled);
        changed |= set(&mut m.desktop_mode, snap.orb.desktop_mode);
        changed |= set(&mut m.light, snap.orb.light.map(|l| orbiters_orb::menu::Light { on: l.on, color: l.color, level: l.level }));
        changed |= set(&mut m.countdown, snap.orb.photo_countdown);
        changed |= set(&mut m.photo_timer, snap.settings.photo_3d_timer);
        changed |= set(&mut m.photo_length, shared::photo_length(snap.settings.photo_3d));
        let calls = &snap.orb.calls;
        let (ready, note) = match &calls.link {
            CallLink::Linked { .. } => (true, ""),
            CallLink::Off => (false, "Calls are off: Orbiters desktop › Orb › Calls"),
            CallLink::SignedOut => (false, "Sign in to Orbiters to call friends"),
            CallLink::NoAccount => (false, "VRChat's account here is unknown: open VRChat"),
            CallLink::NotLinked { .. } => (false, "Link this VRChat account on Orbiters to call"),
        };
        let same_contacts = m.contacts.len() == calls.contacts.len() && m.contacts.iter().zip(&calls.contacts).all(|(a, b)| a.id == b.id && a.name == b.name);
        if !same_contacts || m.calls_ready != ready || m.call_note != note {
            m.set_contacts(calls.contacts.iter().map(|c| Contact { id: c.id.clone(), name: c.name.clone() }).collect(), ready, note);
            changed = true;
        }
        if calls.call.is_some() || m.call.is_some() {
            let call = calls.call.as_ref().map(|c| match c.phase {
                CallPhase::Ringing => Call::Ringing { name: c.name.clone() },
                CallPhase::Calling | CallPhase::Connecting => Call::Calling { name: c.name.clone() },
                CallPhase::Connected => Call::Connected {
                    name: c.name.clone(),
                    seconds: c.connected_at_ms.map_or(0, |at| crate::controller::unix_ms().saturating_sub(at) / 1000),
                    muted: c.muted,
                },
            });
            if m.call != call {
                m.set_call(call);
                changed = true;
            }
        }
        if changed {
            self.dirty = true;
        }
    }

    /// The music the menu shows, from this process's media reading (read only while the menu is open), and its art as a
    /// texture.
    fn follow_media(&mut self) {
        let media = crate::media::watcher();
        media.want();
        let (version, reading) = media.reading();
        if version == self.media_seen {
            return;
        }
        self.media_seen = version;
        self.menu.media = reading.media;
        self.menu.volume = reading.volume;
        if let Some(art) = reading.art {
            if !matches!(self.thumbs.get(&art.key), Some(Thumb::Ready(..))) {
                let rgba: Vec<u8> = art.rgb.pixels.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect();
                let image = ColorImage::from_rgba_unmultiplied([art.rgb.width, art.rgb.height], &rgba);
                let ready = self.ready(art.key.clone(), image);
                self.thumbs.insert(art.key.clone(), ready);
            }
        }
        self.dirty = true;
    }

    /// Fetches the thumbnails among `ids` not here yet (one that failed is tried again a little later, a few times).
    fn thumbnails(&mut self, ids: Vec<String>) {
        if self.thumbs.len() > 60 {
            self.thumbs.retain(|id, _| ids.contains(id));
        }
        let now = Instant::now();
        for id in ids {
            let tries = match self.thumbs.get(&id) {
                None => 0,
                Some(Thumb::Failed { at, tries }) if *tries < THUMB_TRIES && now >= *at + THUMB_RETRY * *tries as u32 => *tries,
                Some(_) => continue,
            };
            self.thumbs.insert(id.clone(), Thumb::Loading);
            let tx = self.done_tx.clone();
            std::thread::spawn(move || {
                let image = youtube::thumbnail(&id).ok().and_then(|jpeg| picture::rgb_jpeg(&jpeg).ok()).map(|rgb| {
                    let rgba: Vec<u8> = rgb.pixels.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect();
                    ColorImage::from_rgba_unmultiplied([rgb.width, rgb.height], &rgba)
                });
                let _ = tx.send(Done::Thumbnail { id, tries, image });
            });
        }
    }

    fn effects(&mut self, effects: Vec<Effect>, overlay: &vr::Overlay, snap: &Snapshot, backend: &Ctl) {
        let language = language_now;
        for effect in effects {
            match effect {
                // Asked for once typing pauses, one request at a time ([`MenuOverlay::update`] sends it).
                Effect::Suggest { id, text } => self.suggest.want(id, text, Instant::now()),
                Effect::Search { id, query } => {
                    let (tx, lang) = (self.done_tx.clone(), language());
                    std::thread::spawn(move || {
                        let result = youtube::search(&query, &lang);
                        let _ = tx.send(Done::Results { id, more: false, result });
                    });
                }
                Effect::More { id, token, .. } => {
                    let (tx, lang) = (self.done_tx.clone(), language());
                    std::thread::spawn(move || {
                        let result = youtube::more(&token, &lang);
                        let _ = tx.send(Done::Results { id, more: true, result });
                    });
                }
                Effect::Thumbnails(ids) => self.thumbnails(ids),
                Effect::Copy { link, .. } => backend.orb(OrbCommand::CopyText { text: link }),
                Effect::FastWalk(on) => {
                    let mut settings = snap.settings.clone();
                    settings.walk_boost.enabled = on;
                    backend.save_changes(&snap.settings, &settings);
                }
                Effect::CallContact { id } => backend.orb(OrbCommand::Call { contact: id }),
                Effect::Answer => backend.orb(OrbCommand::AnswerCall),
                Effect::HangUp => backend.orb(OrbCommand::EndCall),
                Effect::PhotoChoice { timer, length } | Effect::Photo3d { timer, length } => {
                    let take = matches!(effect, Effect::Photo3d { .. });
                    let mut settings = snap.settings.clone();
                    settings.photo_3d_timer = timer;
                    settings.photo_3d = shared::photo_setting(length);
                    backend.save_changes(&snap.settings, &settings);
                    if take {
                        backend.orb(OrbCommand::Photo3DIn { seconds: timer });
                    }
                }
                Effect::Media(command) => crate::media::watcher().command(command),
                Effect::Light(light) => backend.orb(OrbCommand::Light { on: light.on, color: light.color, level: light.level }),
                Effect::Close => self.close(overlay),
            }
        }
    }

    /// One turn of the overlay's loop: the button (`button`: the binding was pressed), the service's requests, the
    /// laser, what came back from YouTube, then the picture when something changed. Returns true when a press
    /// happened (for the click sound).
    pub fn update(&mut self, overlay: &vr::Overlay, steamvr: &vr::Client, presenter: &mut Presenter, snap: &Snapshot, backend: &Ctl, button: bool) -> bool {
        let mut clicked = false;
        // The service asks for the menu (the wake word alone, a hand on the orb) or, with Orb's screen, keeps it.
        let request = snap.orb.menu_request;
        let asked = self.request_seen.is_some_and(|seen| seen != request);
        self.request_seen = Some(request);
        if button && snap.settings.orb.menu.button {
            if snap.orb.screen {
                backend.orb(OrbCommand::ToggleScreen);
            } else if self.menu.open {
                self.close(overlay);
            } else {
                self.open(overlay, steamvr, snap);
            }
        } else if asked && !snap.orb.screen {
            let just_closed = self.closed_at.is_some_and(|t| t.elapsed() < TOGGLE_AFTER_CLOSE);
            if self.menu.open && snap.orb.menu_toggle {
                self.close(overlay);
            } else if !self.menu.open && !(snap.orb.menu_toggle && just_closed) {
                self.open(overlay, steamvr, snap);
            }
        }
        if snap.orb.screen && self.menu.open {
            // The avatar now has Orb's screen: the menu lives there.
            self.close(overlay);
        }
        // What the service reads: open, and whether a sentence is for the search field.
        let state = (self.menu.open, self.menu.open && self.menu.page == Page::Search);
        if self.reported != Some(state) {
            self.reported = Some(state);
            backend.orb(OrbCommand::MenuState { open: state.0, dictation: state.1 });
        }
        if !self.menu.open {
            // Drain the laser's events of a hidden overlay.
            while overlay.poll_event(self.handle).is_some() {}
            return false;
        }
        self.follow(snap);
        self.follow_media();
        let mut effects = Vec::new();
        if let Some((n, text)) = &snap.orb.dictation {
            if self.dictation_seen.is_some_and(|seen| seen != *n) {
                self.last_activity = Instant::now();
                if let Some(more) = self.menu.heard(text) {
                    effects.extend(more);
                }
                self.dirty = true;
            }
            self.dictation_seen = Some(*n);
        } else {
            self.dictation_seen = Some(0);
        }
        // Seen from behind it shows its back (mirrored by SteamVR, so drawn flipped) and the laser does nothing there.
        if let Some(placed) = &self.placed {
            let mut poses = [Pose::default(); MAX_DEVICES];
            steamvr.poses(Universe::Standing, &mut poses);
            if poses[0].pose_is_valid {
                let now = behind(placed, &poses[0].device_to_absolute, self.behind);
                if now != self.behind {
                    self.behind = now;
                    let (u_min, u_max) = if now { (1.0, 0.0) } else { (0.0, 1.0) };
                    let _ = overlay.set_texture_bounds(self.handle, u_min, 0.0, u_max, 1.0);
                    self.hover = None;
                    self.pressed = None;
                    self.dirty = true;
                }
            }
        }
        // The laser: SteamVR's mouse, from the bottom left.
        let page = layout::cached(ScreenPage::of(&self.menu));
        while let Some(e) = overlay.poll_event(self.handle) {
            if self.behind {
                continue;
            }
            match e.event_type {
                vr::event::MOUSE_MOVE => {
                    let (x, y, _) = e.mouse();
                    let hover = layout::hit(page, x, HEIGHT as f32 - y).map(|(i, _)| i).filter(|&i| layout::state(&self.menu, &page.elements[i]).pointable());
                    if hover != self.hover {
                        self.hover = hover;
                        self.dirty = true;
                    }
                }
                vr::event::MOUSE_BUTTON_DOWN => {
                    let (x, y, _) = e.mouse();
                    if let Some((i, target)) = layout::hit(page, x, HEIGHT as f32 - y) {
                        if layout::state(&self.menu, &page.elements[i]).pointable() {
                            // The press shows at once; what it does follows.
                            self.pressed = Some(i);
                            effects.extend(self.menu.press(target));
                            clicked = true;
                            self.last_activity = Instant::now();
                        }
                    }
                    self.dirty = true;
                }
                vr::event::MOUSE_BUTTON_UP => {
                    self.pressed = None;
                    self.dirty = true;
                }
                vr::event::FOCUS_LEAVE => {
                    self.hover = None;
                    self.pressed = None;
                    self.dirty = true;
                }
                _ => {}
            }
        }
        while let Ok(done) = self.done_rx.try_recv() {
            match done {
                Done::Suggestions { id, text, result } => {
                    self.suggest.answered(id);
                    match result {
                        Ok(list) => self.menu.suggestions_arrived(id, &text, list),
                        Err(e) => eprintln!("orb menu: YouTube suggestions: {e:#}"),
                    }
                }
                Done::Results { id, more, result: Ok(results) } => effects.extend(self.menu.results_arrived(id, results, more)),
                Done::Results { id, result: Err(e), .. } => self.menu.results_failed(id, &e.to_string()),
                Done::Thumbnail { id, tries, image } => {
                    let thumb = match image {
                        Some(image) => self.ready(format!("yt-{id}"), image),
                        None => Thumb::Failed { at: Instant::now(), tries: tries + 1 },
                    };
                    self.thumbs.insert(id, thumb);
                }
            }
            self.dirty = true;
        }
        // Suggestions once typing paused, one request at a time.
        if let Some((id, text)) = self.suggest.due(Instant::now()) {
            let (tx, lang) = (self.done_tx.clone(), language_now());
            std::thread::spawn(move || {
                let result = youtube::suggestions(&text, &lang);
                let _ = tx.send(Done::Suggestions { id, text, result });
            });
        }
        // Thumbnails of the results shown that failed, tried again once their wait is over.
        if self.menu.page == Page::Results {
            let now = Instant::now();
            let retry: Vec<String> = self
                .menu
                .page_results()
                .iter()
                .filter(
                    |v| matches!(self.thumbs.get(&v.id), Some(Thumb::Failed { at, tries }) if *tries < THUMB_TRIES && now >= *at + THUMB_RETRY * *tries as u32),
                )
                .map(|v| v.id.clone())
                .collect();
            if !retry.is_empty() {
                self.thumbnails(retry);
            }
        }
        if self.menu.listening {
            self.last_activity = Instant::now();
        }
        if Instant::now().duration_since(self.last_activity) > IDLE_CLOSE {
            effects.extend(self.menu.close());
        }
        self.effects(effects, overlay, snap, backend);
        if !self.menu.open {
            return clicked;
        }
        // A look switched in Orbiters desktop: the whole picture again.
        let retro_now = snap.settings.orb.menu.look == MenuLook::Retro;
        if retro_now != self.retro {
            self.retro = retro_now;
            self.canvas.invalidate();
            self.dirty = true;
        }
        // The microphone breathes while Orb listens: about 20 pictures a second then, else only on changes.
        let animating = self.menu.listening;
        let due = self.last_paint.is_none_or(|t| t.elapsed() >= Duration::from_millis(if animating { 50 } else { 22 }));
        if (self.dirty || animating) && due {
            self.dirty = false;
            self.last_paint = Some(Instant::now());
            let time = self.last_activity.elapsed().as_secs_f64();
            if !self.shown {
                // Shown again: SteamVR gets a whole picture first.
                self.canvas.invalidate();
            }
            let (menu, hover, pressed, thumbs, retro) = (&self.menu, self.hover, self.pressed, &self.thumbs, self.retro);
            let picture = |id: &str| -> Option<TextureId> {
                match thumbs.get(id) {
                    Some(Thumb::Ready(photo, dots)) => Some(if retro { dots.id() } else { photo.id() }),
                    _ => None,
                }
            };
            let view = View { menu, hover, pressed, picture: &picture, time };
            self.canvas.paint(time, |p| match (self.behind, retro) {
                (true, false) => menu_ui::draw_back(p, pos2(0.0, 0.0)),
                (true, true) => menu_retro::draw_back(p, pos2(0.0, 0.0)),
                (false, false) => menu_ui::draw(p, pos2(0.0, 0.0), &view),
                (false, true) => menu_retro::draw(p, pos2(0.0, 0.0), &view),
            });
            // Only what changed goes to SteamVR, straight from the canvas.
            if let Some(changed) = self.canvas.damage() {
                presenter.show_part(overlay, self.handle, self.canvas.pixels(), WIDTH, HEIGHT, changed);
            }
            if !self.shown {
                let _ = overlay.show(self.handle);
                self.shown = true;
            }
        }
        clicked
    }
}

impl MenuOverlay {
    /// A picture ready to show: its texture, and its green dots for the retro look (crisp, so drawn with nearest pixels).
    fn ready(&self, name: String, image: ColorImage) -> Thumb {
        let dots = menu_retro::dots(&image, 1);
        let ctx = self.canvas.ctx();
        Thumb::Ready(ctx.load_texture(name.clone(), image, TextureOptions::LINEAR), ctx.load_texture(name + "-dots", dots, TextureOptions::NEAREST))
    }
}

/// The language YouTube is asked in (`fr`): `LANG` when set, else Windows' locale.
fn language_now() -> String {
    let locale = std::env::var("LANG").unwrap_or_default();
    let code = locale.split(['-', '_', '.']).next().unwrap_or("").to_lowercase();
    if code.len() == 2 {
        code
    } else {
        sys_language()
    }
}

/// This computer's language, for YouTube (`fr`), from Windows' locale.
fn sys_language() -> String {
    #[cfg(windows)]
    {
        use windows::Win32::Globalization::GetUserDefaultLocaleName;
        let mut buf = [0u16; 85];
        // SAFETY: the buffer is LOCALE_NAME_MAX_LENGTH wide, as the call requires.
        let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
        if n > 1 {
            let name = String::from_utf16_lossy(&buf[..n as usize - 1]);
            if let Some(code) = name.split('-').next().filter(|c| c.len() == 2) {
                return code.to_lowercase();
            }
        }
    }
    "en".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_in_front_of_the_head_facing_it() {
        // A head at 1.7 m looking down −Z: the menu half a metre ahead, lower, its face (+Z) toward the head.
        let head = Matrix34 { m: [[1.0, 0.0, 0.0, 0.2], [0.0, 1.0, 0.0, 1.7], [0.0, 0.0, 1.0, 0.0]] };
        let t = in_front(&head, 0.5, 0.16);
        assert!((t.m[0][3] - 0.2).abs() < 1e-5 && (t.m[1][3] - 1.54).abs() < 1e-5 && (t.m[2][3] + 0.5).abs() < 1e-5);
        assert!((t.m[2][2] - 1.0).abs() < 1e-5, "facing +Z, back at the head");
        assert!((t.m[0][0] - 1.0).abs() < 1e-5, "its right is the head's right");
        // Turned a quarter left (looking down −X): ahead is −X.
        let turned = Matrix34 { m: [[0.0, 0.0, 1.0, 0.0], [0.0, 1.0, 0.0, 1.7], [-1.0, 0.0, 0.0, 0.0]] };
        let t = in_front(&turned, 0.5, 0.0);
        assert!((t.m[0][3] + 0.5).abs() < 1e-5 && t.m[2][3].abs() < 1e-5, "{:?}", t.m);
    }

    #[test]
    fn the_back_shows_once_the_head_is_past_the_plane() {
        let head = Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.7], [0.0, 0.0, 1.0, 0.0]] };
        let placed = in_front(&head, 0.5, 0.16);
        let at = |z: f32| Matrix34 { m: [[1.0, 0.0, 0.0, 0.3], [0.0, 1.0, 0.0, 1.6], [0.0, 0.0, 1.0, z]] };
        assert!(!behind(&placed, &head, false), "where it opened: the front");
        assert!(!behind(&placed, &at(-0.51), false), "in the plane: still the front");
        assert!(behind(&placed, &at(-0.6), false), "walked past it");
        assert!(behind(&placed, &at(-0.49), true), "back to the plane: still the back");
        assert!(!behind(&placed, &at(-0.4), true), "in front again");
    }
}
