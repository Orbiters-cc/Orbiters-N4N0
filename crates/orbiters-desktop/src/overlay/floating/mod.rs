//! Orbiters beside VRChat in the play space (docs/floating-overlay.md): the floating panel ([`panel`], the SteamVR tab's
//! interface on an overlay that is no dashboard overlay, opened by the tab's Float button), the watch on a wrist that
//! opens and closes N4N0's menu ([`watch`]), and the controllers' own lasers that use them and N4N0's menu in the play
//! space ([`laser`], [`input`]).
//!
//! SteamVR's dashboard, and any overlay that turns SteamVR's laser mouse on, take the controllers from VRChat while
//! they show. These take nothing: SteamVR gives the overlay's actions (each hand's tip, trigger and grip,
//! [`crate::overlay::actions`]) to VRChat as well, and only a laser on the panel, its bar, N4N0's menu or the watch
//! does anything here. A trigger pulled there reaches VRChat too, as it always does. N4N0's menu gets the lasers as
//! the mouse events SteamVR's laser would send it ([`MouseSurfaces`], `Overlay::inject_event`).
//!
//! Every turn costs one read of the poses (predicted to when the turn's changes light the displays) while the watch is
//! on or the panel open, the actions only while there is something to point at, and SteamVR calls only for what moved:
//! the panel paints as the tab does, the watch and the bar when their hover changes, the lasers' pictures once.

pub mod input;
pub mod laser;
pub mod panel;
pub mod prompt;
pub mod space;
pub mod strip;
pub mod watch;

use super::actions::Actions;
use super::ui::{OverlayUi, Poses};
use super::Presenter;
use crate::backend::Ctl;
use crate::state::Snapshot;
use input::{HandIn, Interaction, Outcome, Surface, Target, Targets, MENU, PANEL, SURFACES};
use orbiters_core::settings::Hand;
use orbiters_openvr::{self as vr, ControllerRole, Pose, Universe, MAX_DEVICES};
use panel::{BarButton, Panel};
use space::Quad;
use std::sync::Arc;
use std::time::Instant;
use watch::Watch;

/// What a turn of the overlay's loop hands the floating panel.
pub struct Cx<'a, 'o> {
    pub overlay: &'a vr::Overlay<'o>,
    pub steamvr: &'a vr::Client,
    pub(in crate::overlay) presenter: &'a mut Presenter,
    pub snap: &'a Arc<Snapshot>,
    pub backend: &'a Ctl,
    pub actions: &'a Actions,
    /// This turn's actions were read.
    pub input_read: bool,
    /// SteamVR's dashboard shows (it has the controllers: the lasers rest).
    pub dashboard: bool,
    /// N4N0's menu in the play space and what hangs under it, while it shows (the menu first): what the lasers point
    /// at there.
    pub menu: &'a [Pointable],
    /// N4N0's menu is open (in the play space or on the avatar's screen): the watch's ring is lit.
    pub menu_open: bool,
}

/// An overlay of N4N0's menu that the lasers use as SteamVR's laser mouse would: where it is, and its mouse scale
/// (its pixels; the events count from its bottom left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pointable {
    pub handle: vr::OverlayHandle,
    pub quad: Quad,
    pub px: [u32; 2],
}

/// What a turn did beyond its pictures.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Turn {
    /// A press happened (for the click sound).
    pub clicked: bool,
    /// The watch was pressed: N4N0's menu opens, or closes.
    pub toggle_menu: bool,
    /// N4N0's menu, held, is here now; it was let go this turn.
    pub menu_moved: Option<vr::Matrix34>,
    pub menu_released: bool,
    /// A row of the menu mismatch strip was pressed: what it says is to be reported ([`crate::mismatch`]).
    pub mismatch: Option<crate::mismatch::Kind>,
}

/// The mouse events the lasers make for N4N0's menu's overlays (surfaces [`MENU`] onwards): moves, the button down and
/// up, focus leaving, in each overlay's mouse scale, as SteamVR's laser sends them.
#[derive(Debug, Default)]
pub struct MouseSurfaces {
    /// Each surface's overlay and size as last pointed at, where its mouse is and whether its button is down.
    to: [Option<(vr::OverlayHandle, [u32; 2])>; SURFACES],
    at: [Option<(f32, f32)>; SURFACES],
    down: [bool; SURFACES],
}

impl MouseSurfaces {
    /// The events of this turn's `outcome` for `menu` (its overlays, surface [`MENU`] first), sent with `send`.
    pub fn update(&mut self, menu: &[Pointable], outcome: &Outcome, send: &mut dyn FnMut(vr::OverlayHandle, vr::Event)) {
        for s in MENU..SURFACES {
            if let Some(p) = menu.get(s - MENU) {
                if self.to[s].is_some_and(|(h, _)| h != p.handle) {
                    // Another overlay in this place now: the last one is let go of first.
                    self.let_go(s, send);
                }
                self.to[s] = Some((p.handle, p.px));
            }
            let Some((handle, px)) = self.to[s] else { continue };
            if let Some([u, v]) = outcome.pointer[s] {
                let at = (u * px[0] as f32, (1.0 - v) * px[1] as f32);
                if self.at[s].is_none_or(|(x, y)| (x - at.0).hypot(y - at.1) >= 0.5) {
                    send(handle, vr::Event::mouse_event(vr::event::MOUSE_MOVE, at.0, at.1, 0));
                    self.at[s] = Some(at);
                }
            }
            if let Some(down) = outcome.button[s].filter(|&d| d != self.down[s]) {
                let (x, y) = self.at[s].unwrap_or((-1.0, -1.0));
                let kind = if down { vr::event::MOUSE_BUTTON_DOWN } else { vr::event::MOUSE_BUTTON_UP };
                send(handle, vr::Event::mouse_event(kind, x, y, 1));
                self.down[s] = down;
            }
            if outcome.gone[s] {
                self.let_go(s, send);
            }
            if menu.get(s - MENU).is_none() && !self.down[s] && self.at[s].is_none() {
                self.to[s] = None;
            }
        }
    }

    /// Surface `s` let go of: its button up if it was down, then the focus gone.
    fn let_go(&mut self, s: usize, send: &mut dyn FnMut(vr::OverlayHandle, vr::Event)) {
        let Some((handle, _)) = self.to[s] else { return };
        let (x, y) = self.at[s].unwrap_or((-1.0, -1.0));
        if std::mem::take(&mut self.down[s]) {
            send(handle, vr::Event::mouse_event(vr::event::MOUSE_BUTTON_UP, x, y, 1));
        }
        if self.at[s].take().is_some() {
            send(handle, vr::Event::mouse_event(vr::event::FOCUS_LEAVE, x, y, 0));
        }
    }

    /// Every surface let go of (the lasers rest).
    pub fn let_go_all(&mut self, send: &mut dyn FnMut(vr::OverlayHandle, vr::Event)) {
        for s in MENU..SURFACES {
            self.let_go(s, send);
        }
    }
}

pub struct Floating {
    panel: Panel,
    watch: Watch,
    /// The menu mismatch strip (none when SteamVR would not make its overlay).
    strip: Option<strip::Strip>,
    /// "Talk with N4N0?" (none when SteamVR would not make its overlay).
    prompt: Option<prompt::Prompt>,
    lasers: laser::Lasers,
    input: Interaction,
    mouse: MouseSurfaces,
    /// The headset's frame and its photons' delay after a vsync, for the poses' prediction (read once).
    timing: Option<(f32, f32)>,
    last_turn: Option<Instant>,
    /// The width the settings last said (the bar's own change stands until the settings echo it).
    width_seen: Option<f32>,
    /// The bar's button just pressed, lit for a moment.
    bar_pressed: Option<(BarButton, Instant)>,
    /// Nothing of it shows (the watch off, the panel closed): the last turn put everything away.
    resting: bool,
}

impl Floating {
    pub fn new(overlay: &vr::Overlay, key: &str) -> anyhow::Result<Self> {
        Ok(Self {
            panel: Panel::new(overlay, key)?,
            watch: Watch::new(overlay, key)?,
            strip: strip::Strip::new(overlay, key).map_err(|e| eprintln!("{e}")).ok(),
            prompt: prompt::Prompt::new(overlay, key).map_err(|e| eprintln!("{e}")).ok(),
            lasers: laser::Lasers::new(overlay, key)?,
            input: Interaction::default(),
            mouse: MouseSurfaces::default(),
            timing: None,
            last_turn: None,
            width_seen: None,
            bar_pressed: None,
            resting: false,
        })
    }

    pub fn destroy(&self, overlay: &vr::Overlay) {
        self.panel.destroy(overlay);
        self.watch.destroy(overlay);
        if let Some(strip) = &self.strip {
            strip.destroy(overlay);
        }
        if let Some(prompt) = &self.prompt {
            prompt.destroy(overlay);
        }
        self.lasers.destroy(overlay);
    }

    /// Seconds from now to when what this turn changes lights the displays: the rest of this frame, then from its vsync
    /// to its photons.
    fn prediction(&mut self, steamvr: &vr::Client) -> f32 {
        let (frame, photons) = *self.timing.get_or_insert_with(|| {
            let hz = steamvr.float_property(0, vr::prop::DISPLAY_FREQUENCY).filter(|hz| *hz > 1.0).unwrap_or(90.0);
            (1.0 / hz, steamvr.float_property(0, vr::prop::SECONDS_FROM_VSYNC_TO_PHOTONS).filter(|s| (0.0..0.1).contains(s)).unwrap_or(0.0))
        });
        let since = steamvr.time_since_vsync().unwrap_or(0.0);
        ((frame - since).max(0.0) + photons).clamp(0.0, 0.05)
    }

    /// One turn: the watch, the lasers, the pointers of the panel and of N4N0's menu, a grab, the bar's buttons, then
    /// the panel's picture when something asked. `state` is the tab's interface, shared with the tab.
    pub fn update(&mut self, cx: Cx<'_, '_>, state: &mut OverlayUi, poses: &mut dyn Poses) -> Turn {
        let settings = &cx.snap.settings.floating;
        let now = Instant::now();
        let dt = self.last_turn.map_or(0.0, |t| now.duration_since(t).as_secs_f32());
        self.last_turn = Some(now);
        let asked = state.take_float();
        let was_open = self.panel.is_open();
        let mut turn = Turn::default();
        // The menu mismatch strip shows whenever its switch is on: the moments it is for are those where N4N0's menus are wrong.
        let strip_on = crate::mismatch::switches(&cx.snap.settings).buttons && self.strip.is_some();
        // "Talk with N4N0?", while the wake word heard during a call has it up.
        let now_ms = crate::wake_offer::now_ms();
        let offer = crate::wake_offer::up(cx.snap.n4n0.voice.wake_offer, now_ms).filter(|_| self.prompt.is_some());
        if !settings.watch && !strip_on && offer.is_none() && !self.panel.is_shown() && !asked && cx.menu.is_empty() {
            if !self.resting {
                self.rest(cx.overlay, cx.presenter, state, cx.backend);
            }
            return turn;
        }
        self.resting = false;
        let mut clicked = false;
        // The width chosen elsewhere (the window, another panel's bar).
        if self.width_seen != Some(settings.width_m) {
            self.width_seen = Some(settings.width_m);
            self.panel.resize(cx.overlay, settings.width_m);
        }

        let seconds = self.prediction(cx.steamvr);
        let mut poses_now = [Pose::default(); MAX_DEVICES];
        cx.steamvr.predicted_poses(Universe::Standing, seconds, &mut poses_now);
        let head = poses_now[0].pose_is_valid.then_some(poses_now[0].device_to_absolute);
        let devices = [ControllerRole::LeftHand, ControllerRole::RightHand]
            .map(|role| cx.steamvr.controller_index(role).filter(|&d| (d as usize) < MAX_DEVICES && poses_now[d as usize].pose_is_valid));
        let grip = |hand: usize| devices[hand].map(|d| poses_now[d as usize].device_to_absolute);

        // The tab's Float button: before the eyes (or where it was left, when that is before them).
        if let (true, Some(head)) = (asked, head.as_ref()) {
            self.panel.open(cx.overlay, panel::opening_place(head, settings), settings.width_m);
        }

        // The watch, unless it is off, its hand holds the panel or SteamVR's dashboard has the controllers.
        let watch_right = settings.watch_hand == Hand::Right;
        let watch_hand = watch_right as usize;
        let watch_at = (settings.watch && !cx.dashboard && self.input.holding() != Some(watch_hand))
            .then(|| devices[watch_hand].zip(grip(watch_hand)).map(|(d, g)| (d, watch_right, g)))
            .flatten();
        let live = !cx.snap.settings.reduce_motion;
        let watch = self.watch.place(cx.overlay, watch_at, head.as_ref(), dt, live);
        // The strip on the same wrist, beside it, unless that hand holds the panel or SteamVR's dashboard has the controllers.
        let strip_at = (strip_on && !cx.dashboard && self.input.holding() != Some(watch_hand))
            .then(|| devices[watch_hand].zip(grip(watch_hand)).map(|(d, g)| (d, watch_right, g)))
            .flatten();
        let strip = self.strip.as_mut().and_then(|s| s.place(cx.overlay, strip_at, head.as_ref()));
        // Before the eyes, unless SteamVR's dashboard has the controllers.
        let prompt = self.prompt.as_mut().and_then(|p| p.place(cx.overlay, offer.filter(|_| !cx.dashboard), head.as_ref()));

        // Who points at what.
        let panel_targets = self.panel.targets();
        if let Some(head) = head.as_ref().filter(|_| self.panel.is_open()) {
            self.panel.face(cx.overlay, head);
        }
        let pointing = !cx.dashboard && cx.input_read && head.is_some() && (panel_targets.is_some() || watch.is_some() || strip.is_some() || prompt.is_some() || !cx.menu.is_empty());
        let outcome = match head.filter(|_| pointing) {
            Some(head) => {
                let lasers = cx.actions.lasers(cx.steamvr, seconds);
                let grabs = cx.actions.grabs(cx.steamvr);
                let triggers = cx.actions.triggers();
                let hands: [Option<HandIn>; 2] = std::array::from_fn(|i| {
                    grip(i).map(|g| HandIn { grip: g, tip: lasers[i].unwrap_or(g), trigger: triggers[i], grab: grabs[i] })
                });
                let mut surfaces = [None; SURFACES];
                surfaces[PANEL] = panel_targets.map(|t| Surface { quad: t.0, carries: PANEL });
                for (s, p) in (MENU..SURFACES).zip(cx.menu) {
                    surfaces[s] = Some(Surface { quad: p.quad, carries: MENU });
                }
                let targets = Targets {
                    surfaces,
                    bar: panel_targets.map(|t| t.1),
                    handle: panel_targets.map(|t| (BarButton::handle_uv(), t.2)),
                    watch: watch.map(|q| (watch_hand, q)),
                    strip: strip.map(|q| (watch_hand, q)),
                    prompt,
                };
                let outcome = self.input.step(&head, hands, &targets);
                self.lasers_for(cx.overlay, cx.presenter, &outcome, &hands, &devices, &head, &targets);
                outcome
            }
            None => {
                if self.input.reset() == Some(PANEL) {
                    self.panel.pointer(None, Some(false), true);
                }
                self.mouse.let_go_all(&mut |h, e| cx.overlay.inject_event(h, e));
                for hand in 0..2 {
                    self.lasers.update(cx.overlay, cx.presenter, hand, None);
                }
                Outcome::default()
            }
        };
        // N4N0's menu hears the lasers as SteamVR's laser mouse.
        self.mouse.update(cx.menu, &outcome, &mut |h, e| cx.overlay.inject_event(h, e));
        turn.menu_moved = outcome.moved[MENU];
        turn.menu_released = outcome.released[MENU];

        // A held panel follows its hand; let go, its place is kept (in the play space, for the next time).
        if let Some(moved) = outcome.moved[PANEL] {
            self.panel.place(cx.overlay, moved);
        }
        if outcome.released[PANEL] {
            let mut changed = cx.snap.settings.clone();
            changed.floating.placement = Some(self.panel.placed().m);
            cx.backend.save_changes(&cx.snap.settings, &changed);
        }
        // The watch: N4N0's menu opens, or closes (the caller's, as the controller binding's).
        if outcome.watch_press {
            clicked = true;
            turn.toggle_menu = true;
        }
        // The mismatch strip: a report, nothing else.
        if let Some(kind) = outcome.strip_press.and_then(|uv| strip::row_at(uv[1])) {
            clicked = true;
            turn.mismatch = Some(kind);
            if let Some(s) = self.strip.as_mut() {
                s.pressed(kind, now);
            }
        }
        // "Talk with N4N0?": N4N0 listens.
        if let Some(offer) = offer.filter(|_| outcome.prompt_press) {
            clicked = true;
            cx.backend.n4n0(crate::ipc::N4n0Command::WakeOffer { accept: true });
            if let Some(p) = self.prompt.as_mut() {
                p.pressed(cx.overlay, offer);
            }
        }
        // The bar's buttons.
        if let Some(button) = outcome.bar_press.map(|uv| self.panel.picture_uv(uv)).and_then(BarButton::at) {
            clicked = true;
            self.bar_pressed = Some((button, now));
            match button {
                BarButton::Close => self.panel.close(),
                BarButton::Smaller | BarButton::Larger => {
                    let width = panel::step_width(self.panel.width, if button == BarButton::Larger { 1 } else { -1 });
                    if (width - self.panel.width).abs() > 1e-4 {
                        self.panel.resize(cx.overlay, width);
                        let mut changed = cx.snap.settings.clone();
                        changed.floating.width_m = width;
                        cx.backend.save_changes(&cx.snap.settings, &changed);
                    }
                }
                BarButton::Handle => {}
            }
        }
        if self.panel.is_open() {
            self.panel.pointer(outcome.pointer[PANEL], outcome.button[PANEL], outcome.gone[PANEL]);
        } else if was_open {
            // Closed under a held Talk: its end (the last picture lets go of the rest).
            state.release(cx.backend);
        }

        // Pictures: the panel when something asked, the bar and the watch when their look changed.
        clicked |= self.panel.paint(cx.overlay, cx.presenter, cx.snap, cx.backend, state, poses, dt);
        let lit = self.bar_pressed.filter(|(_, at)| now.duration_since(*at).as_millis() < 150).map(|(b, _)| b);
        let hover = outcome.bar_hover.map(|uv| self.panel.picture_uv(uv)).and_then(BarButton::at);
        self.panel.paint_bar(cx.overlay, cx.presenter, hover, lit);
        self.watch.paint(cx.overlay, cx.presenter, outcome.watch_hover, cx.menu_open);
        if let Some(s) = self.strip.as_mut() {
            s.paint(cx.overlay, cx.presenter, outcome.strip_hover.and_then(|uv| strip::row_at(uv[1])), now);
        }
        if let (Some(p), Some(offer)) = (self.prompt.as_mut(), offer) {
            p.paint(cx.overlay, cx.presenter, outcome.prompt_hover, offer.left(now_ms));
        }
        turn.clicked = clicked;
        turn
    }

    /// Each hand's laser as this turn's outcome says: from its tip to what it is on.
    #[allow(clippy::too_many_arguments)]
    fn lasers_for(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, outcome: &Outcome, hands: &[Option<HandIn>; 2], devices: &[Option<u32>; 2], head: &vr::Matrix34, targets: &Targets) {
        for hand in 0..2 {
            let laser = outcome.lasers[hand].zip(hands[hand]).zip(devices[hand]).and_then(|(((target, hit), h), device)| {
                let surface = match target {
                    Target::Surface(s) => targets.surfaces[s]?.quad.transform,
                    Target::Bar => targets.bar?.transform,
                    Target::Watch => targets.watch?.1.transform,
                    Target::Strip => targets.strip?.1.transform,
                    Target::Prompt => targets.prompt?.transform,
                };
                Some(laser::Laser { device, grip: h.grip, tip: h.tip, eyes: space::position(head), end: hit.point, surface, pressed: h.trigger })
            });
            self.lasers.update(overlay, presenter, hand, laser);
        }
    }

    /// Everything put away (the watch turned off with the panel and the menu closed): nothing more is read or drawn
    /// until something shows.
    fn rest(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, state: &mut OverlayUi, backend: &Ctl) {
        if self.input.reset() == Some(PANEL) {
            state.release(backend);
        }
        self.mouse.let_go_all(&mut |h, e| overlay.inject_event(h, e));
        self.watch.place(overlay, None, None, 0.0, false);
        if let Some(s) = self.strip.as_mut() {
            s.place(overlay, None, None);
        }
        if let Some(p) = self.prompt.as_mut() {
            p.place(overlay, None, None);
        }
        for hand in 0..2 {
            self.lasers.update(overlay, presenter, hand, None);
        }
        self.resting = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_hears_the_lasers_as_steamvr_s_mouse() {
        let quad = Quad { transform: vr::Matrix34::IDENTITY, width: 0.9, height: 0.5 };
        let menu = [Pointable { handle: 7, quad, px: [1024, 576] }];
        let mut mouse = MouseSurfaces::default();
        let mut sent: Vec<(vr::OverlayHandle, u32, (f32, f32, u32))> = Vec::new();
        type Sent = Vec<(vr::OverlayHandle, u32, (f32, f32, u32))>;
        let turn = |mouse: &mut MouseSurfaces, menu: &[Pointable], outcome: Outcome, sent: &mut Sent| {
            mouse.update(menu, &outcome, &mut |h, e: vr::Event| sent.push((h, e.event_type, e.mouse())));
        };
        let mut out = Outcome::default();
        // Its top left quarter: pixels from the bottom left, as SteamVR's.
        out.pointer[MENU] = Some([0.25, 0.25]);
        turn(&mut mouse, &menu, out, &mut sent);
        assert_eq!(sent, vec![(7, vr::event::MOUSE_MOVE, (256.0, 432.0, 0))]);
        // Pressed there, held still (no move), let go.
        out.button[MENU] = Some(true);
        turn(&mut mouse, &menu, out, &mut sent);
        out.button[MENU] = Some(false);
        turn(&mut mouse, &menu, out, &mut sent);
        assert_eq!(sent[1..].iter().map(|s| s.1).collect::<Vec<_>>(), [vr::event::MOUSE_BUTTON_DOWN, vr::event::MOUSE_BUTTON_UP]);
        assert_eq!(sent[1].2, (256.0, 432.0, 1));
        // Pressed again, and the menu closes under it: up, then the focus leaves.
        sent.clear();
        out.button[MENU] = Some(true);
        turn(&mut mouse, &menu, out, &mut sent);
        let gone = Outcome { button: [None, Some(false), None, None], gone: [false, true, false, false], ..Default::default() };
        turn(&mut mouse, &[], gone, &mut sent);
        assert_eq!(sent.iter().map(|s| s.1).collect::<Vec<_>>(), [vr::event::MOUSE_BUTTON_DOWN, vr::event::MOUSE_BUTTON_UP, vr::event::FOCUS_LEAVE]);
        // Nothing more to say once it is gone; the lasers resting let go of nothing then.
        sent.clear();
        turn(&mut mouse, &[], Outcome::default(), &mut sent);
        mouse.let_go_all(&mut |h, e: vr::Event| sent.push((h, e.event_type, e.mouse())));
        assert!(sent.is_empty());
    }
}
