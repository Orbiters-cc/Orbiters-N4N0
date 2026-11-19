//! N4N0's menu in the play space (when the avatar has no N4N0 screen): a SteamVR overlay that opens [`DISTANCE_M`] in
//! front of the headset, upright and facing it, when the person calls N4N0 (the controller binding, the watch on a
//! wrist, the wake word on its own, a hand on N4N0). The controllers' own lasers point at it ([`super::floating`]:
//! SteamVR's laser mouse would take VRChat's input while it shows), and a grip moves it as the floating panel moves:
//! held, it rides the hand turned toward the eyes; let go, it stays, and opens next time where it was left, as seen
//! from the eyes. It draws the same pages as the avatar's screen ([`super::menu_ui`]) and runs the same menu
//! (`orbiters_n4n0_core::menu`, through [`crate::menu_host`] as Home in the window does): YouTube is fetched here, the rest
//! (calls, the clipboard, a 3D picture, fast walk) goes to the tray service.

use super::menu_retro;
use super::menu_ui::{self, Skin, View};
use super::floating::{space, Pointable};
use super::{Canvas, Presenter};
use crate::backend::Ctl;
use crate::ipc::N4n0Command;
use crate::menu_host::MenuHost;
use crate::state::Snapshot;
use egui::{pos2, TextureId};
use orbiters_core::settings::MenuLook;
use orbiters_openvr::{self as vr, overlay_flag, Matrix34, Pose, Universe, MAX_DEVICES};
use space::Quad;
use orbiters_n4n0_core::menu::layout::{self, ScreenPage};

use std::time::{Duration, Instant};

const WIDTH: u32 = layout::WIDTH as u32;
const HEIGHT: u32 = layout::HEIGHT as u32;
/// The menu's width in the play space: about as wide to the eyes at [`DISTANCE_M`] as it was 0.62 m wide at 0.5 m
/// (59 degrees rather than 63), its words as large.
pub const WIDTH_M: f32 = 0.9;
/// How far in front of the eyes, and how far below them, it opens (at about the same angle below the eyes as it did
/// 0.16 m below at 0.5 m).
pub const DISTANCE_M: f32 = 0.8;
const BELOW_EYES_M: f32 = 0.24;
/// A place it was left at is kept for the next opening (relative to where the eyes look) while it is this near and far
/// from the eyes and faces them.
const KEPT_NEAR_M: f32 = 0.3;
const KEPT_FAR_M: f32 = 1.6;
/// The menu closes after this long without a press or a word.
const IDLE_CLOSE: Duration = Duration::from_secs(120);
pub struct MenuOverlay {
    handle: vr::OverlayHandle,
    canvas: Canvas,
    /// The menu, kept in step with the service (thumbnails become textures of the canvas's context).
    pub host: MenuHost,
    hover: Option<usize>,
    pressed: Option<usize>,
    /// The laser's press held on the music's timeline: the segment it is on (moving along it moves the music).
    dragging: Option<u8>,
    shown: bool,
    last_paint: Option<Instant>,
    last_activity: Instant,
    /// The service's numbered requests and dictations seen so far (the first snapshot sets them).
    request_seen: Option<u32>,
    dictation_seen: Option<u32>,
    /// What the service was last told (open, dictation).
    reported: Option<(bool, bool, bool)>,
    /// When the menu last closed: a request to close it that comes just after (sent before the service heard) finds
    /// it closed, and must not open it again.
    closed_at: Option<Instant>,
    /// Where the menu was opened or carried to (it stays there), and whether the head is behind it: it then shows its
    /// back.
    placed: Option<Matrix34>,
    behind: bool,
    /// Where it was last let go of, relative to the eyes' level frame then ([`level`]): it opens there again.
    kept: Option<Matrix34>,
    /// The menu is drawn in the retro look (Orbiters - N4N0 › N4N0 › N4N0's menu › Look).
    retro: bool,
    /// 3D worlds: its pill under the menu, its panel in the menu's place, the capture shown around the person.
    pub worlds: super::worlds::Worlds,
}

/// Whether N4N0's menu lives on the avatar's N4N0 screen: it has one, and Settings does not ask for it to float.
pub fn on_avatar_screen(snap: &Snapshot) -> bool {
    snap.n4n0.screen && snap.settings.orbit.display != orbiters_core::settings::MenuDisplay::Floating
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

/// The eyes' level frame: at the eyes, turned as the head is about the vertical only (its −Z where they look, level).
pub fn level(head: &Matrix34) -> Matrix34 {
    // What faces the eyes from nought ahead looks where they look (its +Z back toward them).
    in_front(head, 0.0, 0.0)
}

/// Where the menu opens for the eyes at `head`: where it was last let go of, as seen from the eyes then (`kept`, in
/// their level frame), while that is in reach and facing them; else [`DISTANCE_M`] before them.
pub fn opening_place(head: &Matrix34, kept: Option<&Matrix34>) -> Matrix34 {
    let fresh = in_front(head, DISTANCE_M, BELOW_EYES_M);
    let Some(kept) = kept else { return fresh };
    let placed = space::compose(&level(head), kept);
    let to_eyes = space::sub(space::position(head), space::position(&placed));
    let distance = space::length(to_eyes);
    let faces = space::dot(space::axis(&placed, 2), to_eyes) > 0.3 * distance;
    if (KEPT_NEAR_M..=KEPT_FAR_M).contains(&distance) && faces {
        placed
    } else {
        fresh
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
        let fail = |what: &str, code: i32| anyhow::anyhow!("{what}: SteamVR overlay error {code}"); // i18n: not shown (the overlay process's own stderr, a log nobody reads in a window)
        let handle = super::slots::create(overlay, &format!("{key}.menu"), "N4N0's menu").map_err(|c| fail("menu", c))?; // i18n: not shown (a name for SteamVR)
        let _ = overlay.set_width_in_meters(handle, WIDTH_M);
        // No SteamVR input method: its laser mouse would take VRChat's input while the menu shows. The controllers'
        // own lasers send it the same mouse events instead (`Overlay::inject_event`, from `overlay::floating`).
        let _ = overlay.set_flag(handle, overlay_flag::IS_PREMULTIPLIED, true);
        let _ = overlay.set_sort_order(handle, 10);
        let canvas = Canvas::new(WIDTH, HEIGHT);
        let host = MenuHost::new(canvas.ctx().clone()).holding(orbiters_n4n0::MenuHolder::Overlay);
        let worlds = super::worlds::Worlds::new(overlay, key, canvas.ctx().clone()).inspect_err(|_| {
            let _ = overlay.destroy(handle);
        })?;
        Ok(Self {
            handle,
            canvas,
            host,
            hover: None,
            pressed: None,
            dragging: None,
            shown: false,
            last_paint: None,
            last_activity: Instant::now(),
            request_seen: None,
            dictation_seen: None,
            reported: None,
            closed_at: None,
            placed: None,
            behind: false,
            kept: None,
            retro: false,
            worlds,
        })
    }

    pub fn destroy(&mut self, overlay: &vr::Overlay) {
        self.worlds.destroy(overlay);
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
        let placed = opening_place(&head, self.kept.as_ref());
        let _ = overlay.set_transform_absolute(self.handle, &placed);
        self.placed = Some(placed);
        if self.behind {
            self.behind = false;
            let _ = overlay.set_texture_bounds(self.handle, 0.0, 0.0, 1.0, 1.0);
        }
        self.host.open(snap);
        self.hover = None;
        self.pressed = None;
        self.last_activity = Instant::now();
    }

    /// What the lasers point at here while the menu shows: the menu (its back too: a grip there takes it), then what
    /// hangs under it.
    pub fn pointables(&self) -> Vec<Pointable> {
        let Some(placed) = self.placed.filter(|_| self.host.menu.open && self.shown) else { return Vec::new() };
        let height = WIDTH_M * HEIGHT as f32 / WIDTH as f32;
        let mut out = vec![Pointable { handle: self.handle, quad: Quad { transform: placed, width: WIDTH_M, height }, px: [WIDTH, HEIGHT] }];
        out.extend(self.worlds.pointables());
        out
    }

    /// The menu held by a hand is `to` now, or was let go of (`released`, the eyes at `head`): it stays there, and
    /// opens there again as seen from the eyes.
    pub fn carry(&mut self, overlay: &vr::Overlay, to: Option<Matrix34>, released: bool, head: Option<&Matrix34>) {
        if let Some(to) = to.filter(|_| self.host.menu.open) {
            let _ = overlay.set_transform_absolute(self.handle, &to);
            self.placed = Some(to);
            self.last_activity = Instant::now();
        }
        if let (true, Some(placed), Some(head)) = (released, self.placed, head) {
            self.kept = Some(space::compose(&space::inverse(&level(head)), &placed));
        }
    }

    /// The 3D worlds panel now, in the menu's place: asked for from the panels around VRChat's menus.
    pub fn open_worlds(&mut self, overlay: &vr::Overlay, steamvr: &vr::Client, snap: &Snapshot) {
        if !self.host.menu.open {
            self.open(overlay, steamvr, snap);
        }
        self.worlds.open_panel(self.host.menu.layout);
        self.canvas.invalidate();
        self.host.dirty = true;
        self.last_activity = Instant::now();
    }

    fn close(&mut self, overlay: &vr::Overlay, snap: &Snapshot, backend: &Ctl) {
        // (The menu's own close, idle or its button, closed `menu` already: this hides the overlay.) Closed here (the
        // button, the watch, the wake word, the avatar's screen), what its page held is let go (MENU-1).
        self.closed_at = Some(Instant::now());
        let effects = self.host.menu.close();
        self.host.effects(effects, snap, backend);
        self.worlds.open = false;
        if self.shown {
            let _ = overlay.hide(self.handle);
            self.shown = false;
        }
    }

    /// One turn of the overlay's loop: the button (`button`: the binding was pressed; `watch`: the watch on a wrist
    /// was), the service's requests, the lasers, what came back from YouTube, then the picture when something
    /// changed. Returns true when a press happened (for the click sound).
    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, overlay: &vr::Overlay, steamvr: &vr::Client, presenter: &mut Presenter, snap: &Snapshot, backend: &Ctl, button: bool, watch: bool) -> bool {
        let mut clicked = false;
        // The service asks for the menu (the wake word alone, a hand on N4N0) or, with N4N0's screen, keeps it.
        let request = snap.n4n0.menu_request;
        let asked = self.request_seen.is_some_and(|seen| seen != request);
        self.request_seen = Some(request);
        if (button && snap.settings.n4n0.menu.button) || watch {
            if on_avatar_screen(snap) {
                backend.n4n0(N4n0Command::ToggleScreen);
            } else if self.host.menu.open {
                self.close(overlay, snap, backend);
            } else {
                self.open(overlay, steamvr, snap);
            }
        } else if asked && !on_avatar_screen(snap) {
            let just_closed = self.closed_at.is_some_and(|t| t.elapsed() < TOGGLE_AFTER_CLOSE);
            if self.host.menu.open && snap.n4n0.menu_toggle {
                self.close(overlay, snap, backend);
            } else if !self.host.menu.open && !(snap.n4n0.menu_toggle && just_closed) {
                self.open(overlay, steamvr, snap);
            }
        }
        if on_avatar_screen(snap) && self.host.menu.open {
            // The avatar now has N4N0's screen: the menu lives there.
            self.close(overlay, snap, backend);
        }
        // 3D worlds: the capture around the person and its download whether the menu shows or not, the pill under the
        // menu while it shows its front (not over its keyboard).
        let menu_at = self.placed.filter(|_| self.host.menu.open && self.shown && !self.behind && !self.host.menu.shows_keyboard());
        let (open_worlds, pressed) = self.worlds.update(overlay, steamvr, presenter, snap, backend, menu_at.as_ref());
        clicked |= pressed;
        if open_worlds {
            self.worlds.open_panel(self.host.menu.layout);
            self.canvas.invalidate();
            self.last_activity = Instant::now();
        }
        // What the service reads: open, and whether a sentence is for the search field (the 3D worlds panel types only).
        let worlds = self.worlds.open;
        let state = (self.host.menu.open, self.host.menu.takes_speech() && !worlds, if worlds { self.worlds.typing() } else { self.host.menu.shows_keyboard() });
        if self.reported != Some(state) {
            self.reported = Some(state);
            backend.n4n0(N4n0Command::MenuState { open: state.0, dictation: state.1, keyboard: state.2 });
        }
        if !self.host.menu.open {
            // Drain the laser's events of a hidden overlay.
            while overlay.poll_event(self.handle).is_some() {}
            return false;
        }
        self.host.follow(snap);
        self.host.follow_media();
        // The computer's keyboard typing on the menu's keyboard (the service took the keys from VRChat).
        self.worlds.keys(&snap.n4n0.keys);
        let mut effects = self.host.take_keys(&snap.n4n0);
        if !effects.is_empty() {
            self.last_activity = Instant::now();
        }
        if let Some((n, text)) = &snap.n4n0.dictation {
            if self.dictation_seen.is_some_and(|seen| seen != *n) {
                self.last_activity = Instant::now();
                if let Some(more) = self.host.menu.heard(text) {
                    effects.extend(more);
                }
                self.host.dirty = true;
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
                    self.host.dirty = true;
                }
            }
        }
        // The lasers: the controllers' own, as SteamVR's mouse events (from the bottom left).
        let page = layout::cached(ScreenPage::of(&self.host.menu));
        while let Some(e) = overlay.poll_event(self.handle) {
            if self.behind {
                continue;
            }
            if self.worlds.open {
                if self.worlds.mouse(&e, overlay, steamvr) {
                    clicked = true;
                    self.last_activity = Instant::now();
                }
                if !self.worlds.open {
                    // Back to the menu's page.
                    self.canvas.invalidate();
                    self.host.dirty = true;
                }
                continue;
            }
            match e.event_type {
                vr::event::MOUSE_MOVE => {
                    let (x, y, _) = e.mouse();
                    let hit = layout::hit_pointable(page, &self.host.menu, x, HEIGHT as f32 - y);
                    let hover = hit.map(|(i, _)| i);
                    if hover != self.hover {
                        self.hover = hover;
                        self.host.dirty = true;
                    }
                    // Held and moved along the music's timeline: the music follows, a segment at a time.
                    if let (Some(from), Some((i, target))) = (self.dragging, hit) {
                        let (on, more) = self.host.menu.drag(from, target);
                        if !more.is_empty() {
                            self.pressed = Some(i);
                            self.host.dirty = true;
                            self.last_activity = Instant::now();
                        }
                        self.dragging = Some(on);
                        effects.extend(more);
                    }
                }
                vr::event::MOUSE_BUTTON_DOWN => {
                    let (x, y, _) = e.mouse();
                    if let Some((i, target)) = layout::hit_pointable(page, &self.host.menu, x, HEIGHT as f32 - y) {
                        // The press shows at once; what it does follows.
                        self.pressed = Some(i);
                        self.dragging = orbiters_n4n0_core::menu::timeline_segment(target);
                        effects.extend(self.host.press(target));
                        clicked = true;
                        self.last_activity = Instant::now();
                    }
                    self.host.dirty = true;
                }
                vr::event::MOUSE_BUTTON_UP => {
                    self.pressed = None;
                    self.dragging = None;
                    self.host.dirty = true;
                }
                vr::event::FOCUS_LEAVE => {
                    self.hover = None;
                    self.pressed = None;
                    self.dragging = None;
                    self.host.dirty = true;
                }
                _ => {}
            }
        }
        effects.extend(self.host.poll());
        if self.host.menu.listening {
            self.last_activity = Instant::now();
        }
        if Instant::now().duration_since(self.last_activity) > IDLE_CLOSE {
            effects.extend(self.host.menu.close());
        }
        // A close, or a link dropped as a card (it is in the world now, on the avatar's screen).
        if self.host.effects(effects, snap, backend) || std::mem::take(&mut self.host.dropped) {
            self.close(overlay, snap, backend);
        }
        if !self.host.menu.open {
            return clicked;
        }
        // A look switched in Orbiters - N4N0: the whole picture again.
        let retro_now = snap.settings.n4n0.menu.look == MenuLook::Retro;
        if retro_now != self.retro {
            self.retro = retro_now;
            self.canvas.invalidate();
            self.host.dirty = true;
        }
        if self.worlds.open && !self.behind {
            if self.worlds.wants_paint() || !self.shown {
                self.last_paint = Some(Instant::now());
                if !self.shown {
                    self.canvas.invalidate();
                }
                self.worlds.paint(&mut self.canvas);
                if let Some(changed) = self.canvas.damage() {
                    presenter.show_part(overlay, self.handle, self.canvas.pixels(), WIDTH, HEIGHT, changed);
                }
                if !self.shown {
                    let _ = overlay.show(self.handle);
                    self.shown = true;
                }
            }
            return clicked;
        }
        // The microphone breathes while N4N0 listens: about 20 pictures a second then, else only on changes.
        // A like or a label switched on pops for a moment too.
        let animating = self.host.menu.listening || self.host.menu.popping().is_some();
        let due = self.last_paint.is_none_or(|t| t.elapsed() >= if animating { Duration::from_millis(50) } else { super::booster::panel_pace() });
        if (self.host.dirty || animating) && due {
            self.host.dirty = false;
            self.last_paint = Some(Instant::now());
            let time = self.last_activity.elapsed().as_secs_f64();
            if !self.shown {
                // Shown again: SteamVR gets a whole picture first.
                self.canvas.invalidate();
            }
            let (host, hover, pressed, retro) = (&self.host, self.hover, self.pressed, self.retro);
            let menu = &host.menu;
            let picture = |id: &str| -> Option<TextureId> { host.picture(id, retro) };
            let skin = Skin::current();
            let view = View { menu, hover, pressed, picture: &picture, time, skin };
            self.canvas.paint(time, |p| match (self.behind, retro) {
                (true, false) => menu_ui::draw_back(p, pos2(0.0, 0.0), skin),
                (true, true) => menu_retro::draw_back(p, pos2(0.0, 0.0), skin),
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
    fn it_opens_further_and_where_it_was_left_as_seen_from_the_eyes() {
        let head = Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.7], [0.0, 0.0, 1.0, 0.0]] };
        let fresh = opening_place(&head, None);
        assert!((space::position(&fresh)[2] + DISTANCE_M).abs() < 1e-5 && DISTANCE_M >= 0.8, "0.3 m further than the 0.5 m it was");
        // As wide to the eyes as before (within 5 degrees), so its words read as before.
        let angle = |width: f32, distance: f32| 2.0 * (width / 2.0 / distance).atan().to_degrees();
        assert!((angle(WIDTH_M, DISTANCE_M) - angle(0.62, 0.5)).abs() < 5.0, "{}", angle(WIDTH_M, DISTANCE_M));
        // Carried 30 cm to the left and let go there; the head then turns a quarter left and walks a metre: it opens
        // 30 cm to the left of where the eyes look again, facing them.
        let left = space::facing([-0.3, 1.45, -0.75], [0.0, 1.7, 0.0], [0.0, 1.0, 0.0]).unwrap();
        let kept = space::compose(&space::inverse(&level(&head)), &left);
        assert!(space::near(&opening_place(&head, Some(&kept)), &left, 1e-4, 1e-5));
        let turned = Matrix34 { m: [[0.0, 0.0, 1.0, 1.0], [0.0, 1.0, 0.0, 1.7], [-1.0, 0.0, 0.0, 0.0]] };
        let again = opening_place(&turned, Some(&kept));
        let p = space::position(&again);
        assert!((p[0] - 0.25).abs() < 1e-4 && (p[2] - 0.3).abs() < 1e-4, "ahead along −X, its left is +Z: {p:?}");
        assert!(space::dot(space::axis(&again, 2), space::sub(space::position(&turned), p)) > 0.0, "facing the eyes");
        // Left too far away (or behind the back), it opens before the eyes instead.
        let far = space::facing([0.0, 1.5, -2.5], [0.0, 1.7, 0.0], [0.0, 1.0, 0.0]).unwrap();
        let kept = space::compose(&space::inverse(&level(&head)), &far);
        assert!(space::near(&opening_place(&head, Some(&kept)), &fresh, 1e-5, 1e-6));
        let away = space::facing([0.0, 1.5, -0.8], [0.0, 1.5, -2.0], [0.0, 1.0, 0.0]).unwrap();
        let kept = space::compose(&space::inverse(&level(&head)), &away);
        assert!(space::near(&opening_place(&head, Some(&kept)), &fresh, 1e-5, 1e-6), "its back to the eyes");
    }

    #[test]
    #[ignore = "a timing, run by hand: cargo test -p orbiters-desktop call_page_timing -- --ignored --nocapture"]
    fn call_page_timing() {
        use orbiters_n4n0_core::menu::{KeyboardLayout, Menu, PhotoLength, Target};
        let mut canvas = Canvas::new(WIDTH, HEIGHT);
        let mut menu = Menu::new(KeyboardLayout::Qwerty, 5, PhotoLength::Standard);
        menu.open();
        let none = |_: &str| -> Option<TextureId> { None };
        let mut paint = |menu: &Menu, what: &str| {
            let at = Instant::now();
            let view = View { menu, hover: None, pressed: None, picture: &none, time: 0.0, skin: Skin::english() };
            canvas.paint(0.0, |p| menu_ui::draw(p, pos2(0.0, 0.0), &view));
            eprintln!("{what}: {:.1} ms", at.elapsed().as_secs_f64() * 1000.0);
        };
        paint(&menu, "Home, first");
        paint(&menu, "Home, again");
        let at = Instant::now();
        let effects = menu.press(Target::Call);
        eprintln!("press Call: {:.3} ms, effects {effects:?}", at.elapsed().as_secs_f64() * 1000.0);
        paint(&menu, "Call, first");
        paint(&menu, "Call, again");
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
