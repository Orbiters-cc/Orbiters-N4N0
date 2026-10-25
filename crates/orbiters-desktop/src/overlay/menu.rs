//! N4N0's menu in the play space (when the avatar has no N4N0 screen): a SteamVR overlay that opens half a metre in
//! front of the headset, upright and facing it, when the person calls N4N0 (the controller binding, the wake word on
//! its own, a hand on N4N0). SteamVR's laser points at it whenever it shows. It draws the same pages as the
//! avatar's screen ([`super::menu_ui`]) and runs the same menu (`orbiters_orb::menu`, through [`crate::menu_host`] as
//! Home in the window does): YouTube is fetched here, the rest (calls, the clipboard, a 3D picture, fast walk) goes
//! to the tray service.

use super::menu_retro;
use super::menu_ui::{self, View};
use super::{Canvas, Presenter};
use crate::backend::Ctl;
use crate::ipc::OrbCommand;
use crate::menu_host::MenuHost;
use crate::state::Snapshot;
use egui::{pos2, TextureId};
use orbiters_core::settings::MenuLook;
use orbiters_openvr::{self as vr, overlay_flag, Matrix34, Pose, Universe, MAX_DEVICES};
use orbiters_orb::menu::layout::{self, ScreenPage};
use orbiters_orb::menu::Page;
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
pub struct MenuOverlay {
    handle: vr::OverlayHandle,
    canvas: Canvas,
    /// The menu, kept in step with the service (thumbnails become textures of the canvas's context).
    pub host: MenuHost,
    hover: Option<usize>,
    pressed: Option<usize>,
    shown: bool,
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
    /// Where the menu was opened (it stays there), and whether the head is behind it: it then shows its back.
    placed: Option<Matrix34>,
    behind: bool,
    /// The menu is drawn in the retro look (Orbiters - N4N0 › N4N0 › N4N0's menu › Look).
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
        let handle = overlay.create(&format!("{key}.menu"), "N4N0's menu").map_err(|c| fail("menu", c))?;
        let _ = overlay.set_width_in_meters(handle, WIDTH_M);
        overlay.set_mouse_input(handle).map_err(|c| fail("menu input", c))?;
        overlay.set_mouse_scale(handle, WIDTH as f32, HEIGHT as f32).map_err(|c| fail("menu mouse scale", c))?;
        let _ = overlay.set_flag(handle, overlay_flag::MAKE_OVERLAYS_INTERACTIVE_IF_VISIBLE, true);
        let _ = overlay.set_flag(handle, overlay_flag::ENABLE_CLICK_STABILIZATION, true);
        let _ = overlay.set_flag(handle, overlay_flag::IS_PREMULTIPLIED, true);
        let _ = overlay.set_sort_order(handle, 10);
        let canvas = Canvas::new(WIDTH, HEIGHT);
        let host = MenuHost::new(canvas.ctx().clone());
        Ok(Self {
            handle,
            canvas,
            host,
            hover: None,
            pressed: None,
            shown: false,
            last_paint: None,
            last_activity: Instant::now(),
            request_seen: None,
            dictation_seen: None,
            reported: None,
            closed_at: None,
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
        self.host.open(snap);
        self.hover = None;
        self.pressed = None;
        self.last_activity = Instant::now();
    }

    fn close(&mut self, overlay: &vr::Overlay) {
        // (The menu's own close, idle or its button, closed `menu` already: this hides the overlay.)
        self.closed_at = Some(Instant::now());
        self.host.menu.close();
        if self.shown {
            let _ = overlay.hide(self.handle);
            self.shown = false;
        }
    }

    /// One turn of the overlay's loop: the button (`button`: the binding was pressed), the service's requests, the
    /// laser, what came back from YouTube, then the picture when something changed. Returns true when a press
    /// happened (for the click sound).
    pub fn update(&mut self, overlay: &vr::Overlay, steamvr: &vr::Client, presenter: &mut Presenter, snap: &Snapshot, backend: &Ctl, button: bool) -> bool {
        let mut clicked = false;
        // The service asks for the menu (the wake word alone, a hand on N4N0) or, with N4N0's screen, keeps it.
        let request = snap.orb.menu_request;
        let asked = self.request_seen.is_some_and(|seen| seen != request);
        self.request_seen = Some(request);
        if button && snap.settings.orb.menu.button {
            if snap.orb.screen {
                backend.orb(OrbCommand::ToggleScreen);
            } else if self.host.menu.open {
                self.close(overlay);
            } else {
                self.open(overlay, steamvr, snap);
            }
        } else if asked && !snap.orb.screen {
            let just_closed = self.closed_at.is_some_and(|t| t.elapsed() < TOGGLE_AFTER_CLOSE);
            if self.host.menu.open && snap.orb.menu_toggle {
                self.close(overlay);
            } else if !self.host.menu.open && !(snap.orb.menu_toggle && just_closed) {
                self.open(overlay, steamvr, snap);
            }
        }
        if snap.orb.screen && self.host.menu.open {
            // The avatar now has N4N0's screen: the menu lives there.
            self.close(overlay);
        }
        // What the service reads: open, and whether a sentence is for the search field.
        let state = (self.host.menu.open, self.host.menu.open && self.host.menu.page == Page::Search);
        if self.reported != Some(state) {
            self.reported = Some(state);
            backend.orb(OrbCommand::MenuState { open: state.0, dictation: state.1 });
        }
        if !self.host.menu.open {
            // Drain the laser's events of a hidden overlay.
            while overlay.poll_event(self.handle).is_some() {}
            return false;
        }
        self.host.follow(snap);
        self.host.follow_media();
        let mut effects = Vec::new();
        if let Some((n, text)) = &snap.orb.dictation {
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
        // The laser: SteamVR's mouse, from the bottom left.
        let page = layout::cached(ScreenPage::of(&self.host.menu));
        while let Some(e) = overlay.poll_event(self.handle) {
            if self.behind {
                continue;
            }
            match e.event_type {
                vr::event::MOUSE_MOVE => {
                    let (x, y, _) = e.mouse();
                    let hover = layout::hit_pointable(page, &self.host.menu, x, HEIGHT as f32 - y).map(|(i, _)| i);
                    if hover != self.hover {
                        self.hover = hover;
                        self.host.dirty = true;
                    }
                }
                vr::event::MOUSE_BUTTON_DOWN => {
                    let (x, y, _) = e.mouse();
                    if let Some((i, target)) = layout::hit_pointable(page, &self.host.menu, x, HEIGHT as f32 - y) {
                        // The press shows at once; what it does follows.
                        self.pressed = Some(i);
                        effects.extend(self.host.press(target));
                        clicked = true;
                        self.last_activity = Instant::now();
                    }
                    self.host.dirty = true;
                }
                vr::event::MOUSE_BUTTON_UP => {
                    self.pressed = None;
                    self.host.dirty = true;
                }
                vr::event::FOCUS_LEAVE => {
                    self.hover = None;
                    self.pressed = None;
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
            self.close(overlay);
        }
        if !self.host.menu.open {
            return clicked;
        }
        // A look switched in Orbiters - N4N0: the whole picture again.
        let retro_now = snap.settings.orb.menu.look == MenuLook::Retro;
        if retro_now != self.retro {
            self.retro = retro_now;
            self.canvas.invalidate();
            self.host.dirty = true;
        }
        // The microphone breathes while N4N0 listens: about 20 pictures a second then, else only on changes.
        let animating = self.host.menu.listening;
        let due = self.last_paint.is_none_or(|t| t.elapsed() >= Duration::from_millis(if animating { 50 } else { 22 }));
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
