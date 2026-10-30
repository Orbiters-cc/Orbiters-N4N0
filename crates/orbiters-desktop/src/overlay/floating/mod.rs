//! Orbiters beside VRChat in the play space (docs/floating-overlay.md): the floating panel ([`panel`], the SteamVR tab's
//! interface on an overlay that is no dashboard overlay), the watch on a wrist that opens and closes it ([`watch`]),
//! and the controllers' own lasers that use them ([`laser`], [`input`]).
//!
//! SteamVR's dashboard, and any overlay that turns SteamVR's laser mouse on, take the controllers from VRChat while
//! they show. These take nothing: SteamVR gives the overlay's actions (each hand's tip, trigger and grip,
//! [`crate::overlay::actions`]) to VRChat as well, and only a laser on the panel, its bar or the watch does anything
//! here. A trigger pulled there reaches VRChat too, as it always does.
//!
//! Every turn costs one read of the poses (predicted to when the turn's changes light the displays) while the watch is
//! on or the panel open, the actions only while there is something to point at, and SteamVR calls only for what moved:
//! the panel paints as the tab does, the watch and the bar when their hover changes, the lasers' pictures once.

pub mod input;
pub mod laser;
pub mod panel;
pub mod space;
pub mod watch;

use super::actions::Actions;
use super::ui::{OverlayUi, Poses};
use super::Presenter;
use crate::backend::Ctl;
use crate::state::Snapshot;
use input::{HandIn, Interaction, Outcome, Target, Targets};
use orbiters_core::settings::Hand;
use orbiters_openvr::{self as vr, ControllerRole, Pose, Universe, MAX_DEVICES};
use panel::{BarButton, Panel};
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
}

pub struct Floating {
    panel: Panel,
    watch: Watch,
    lasers: laser::Lasers,
    input: Interaction,
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
            lasers: laser::Lasers::new(overlay, key)?,
            input: Interaction::default(),
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

    /// One turn: the watch, the lasers, the panel's pointer, a grab, the bar's buttons, then the panel's picture when
    /// something asked. `state` is the tab's interface, shared with the tab. Returns whether a press happened (for the
    /// click sound).
    pub fn update(&mut self, cx: Cx<'_, '_>, state: &mut OverlayUi, poses: &mut dyn Poses) -> bool {
        let settings = &cx.snap.settings.floating;
        let now = Instant::now();
        let dt = self.last_turn.map_or(0.0, |t| now.duration_since(t).as_secs_f32());
        self.last_turn = Some(now);
        let asked = state.take_float();
        let was_open = self.panel.is_open();
        if !settings.watch && !self.panel.is_shown() && !asked {
            if !self.resting {
                self.rest(cx.overlay, cx.presenter, state, cx.backend);
            }
            return false;
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

        // The tab's Float button, or the watch: before the eyes (or where it was left, when that is before them).
        let open = |panel: &mut Panel, head: &vr::Matrix34| panel.open(cx.overlay, panel::opening_place(head, settings), settings.width_m);
        if let (true, Some(head)) = (asked, head.as_ref()) {
            open(&mut self.panel, head);
        }

        // The watch, unless it is off, its hand holds the panel or SteamVR's dashboard has the controllers.
        let watch_right = settings.watch_hand == Hand::Right;
        let watch_hand = watch_right as usize;
        let watch_at = (settings.watch && !cx.dashboard && self.input.holding() != Some(watch_hand))
            .then(|| devices[watch_hand].zip(grip(watch_hand)).map(|(d, g)| (d, watch_right, g)))
            .flatten();
        let live = !cx.snap.settings.reduce_motion;
        let watch = self.watch.place(cx.overlay, watch_at, head.as_ref(), dt, live);

        // Who points at what.
        let panel_targets = self.panel.targets();
        if let Some(head) = head.as_ref().filter(|_| self.panel.is_open()) {
            self.panel.face(cx.overlay, head);
        }
        let pointing = !cx.dashboard && cx.input_read && head.is_some() && (panel_targets.is_some() || watch.is_some());
        let outcome = match head.filter(|_| pointing) {
            Some(head) => {
                let lasers = cx.actions.lasers(cx.steamvr, seconds);
                let grabs = cx.actions.grabs(cx.steamvr);
                let triggers = cx.actions.triggers();
                let hands: [Option<HandIn>; 2] = std::array::from_fn(|i| {
                    grip(i).map(|g| HandIn { grip: g, tip: lasers[i].unwrap_or(g), trigger: triggers[i], grab: grabs[i] })
                });
                let targets = Targets {
                    panel: panel_targets.map(|t| t.0),
                    bar: panel_targets.map(|t| t.1),
                    handle: panel_targets.map(|t| (BarButton::handle_uv(), t.2)),
                    watch: watch.map(|q| (watch_hand, q)),
                };
                let outcome = self.input.step(&head, hands, &targets);
                self.lasers_for(cx.overlay, cx.presenter, &outcome, &hands, &devices, &head, &targets);
                outcome
            }
            None => {
                if self.input.reset() {
                    self.panel.pointer(None, Some(false), true);
                }
                for hand in 0..2 {
                    self.lasers.update(cx.overlay, cx.presenter, hand, None);
                }
                Outcome::default()
            }
        };

        // A held panel follows its hand; let go, its place is kept (in the play space, for the next time).
        if let Some(moved) = outcome.moved {
            self.panel.place(cx.overlay, moved);
        }
        if outcome.released {
            let mut changed = cx.snap.settings.clone();
            changed.floating.placement = Some(self.panel.placed().m);
            cx.backend.save_changes(&cx.snap.settings, &changed);
        }
        // The watch: the panel before the eyes, or closed.
        if outcome.watch_press {
            clicked = true;
            if self.panel.is_open() {
                self.panel.close();
            } else if let Some(head) = head.as_ref() {
                open(&mut self.panel, head);
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
            self.panel.pointer(outcome.pointer, outcome.button, outcome.gone);
        } else if was_open {
            // Closed under a held Talk: its end (the last picture lets go of the rest).
            state.release(cx.backend);
        }

        // Pictures: the panel when something asked, the bar and the watch when their look changed.
        clicked |= self.panel.paint(cx.overlay, cx.presenter, cx.snap, cx.backend, state, poses, dt);
        let lit = self.bar_pressed.filter(|(_, at)| now.duration_since(*at).as_millis() < 150).map(|(b, _)| b);
        let hover = outcome.bar_hover.map(|uv| self.panel.picture_uv(uv)).and_then(BarButton::at);
        self.panel.paint_bar(cx.overlay, cx.presenter, hover, lit);
        self.watch.paint(cx.overlay, cx.presenter, outcome.watch_hover, self.panel.is_open());
        clicked
    }

    /// Each hand's laser as this turn's outcome says: from its tip to what it is on.
    #[allow(clippy::too_many_arguments)]
    fn lasers_for(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, outcome: &Outcome, hands: &[Option<HandIn>; 2], devices: &[Option<u32>; 2], head: &vr::Matrix34, targets: &Targets) {
        for hand in 0..2 {
            let laser = outcome.lasers[hand].zip(hands[hand]).zip(devices[hand]).and_then(|(((target, hit), h), device)| {
                let surface = match target {
                    Target::Panel => targets.panel?.transform,
                    Target::Bar => targets.bar?.transform,
                    Target::Watch => targets.watch?.1.transform,
                };
                Some(laser::Laser { device, grip: h.grip, tip: h.tip, eyes: space::position(head), end: hit.point, surface, pressed: h.trigger })
            });
            self.lasers.update(overlay, presenter, hand, laser);
        }
    }

    /// Everything put away (the watch turned off with the panel closed): nothing more is read or drawn until it is on.
    fn rest(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, state: &mut OverlayUi, backend: &Ctl) {
        if self.input.reset() {
            state.release(backend);
        }
        self.watch.place(overlay, None, None, 0.0, false);
        for hand in 0..2 {
            self.lasers.update(overlay, presenter, hand, None);
        }
        self.resting = true;
    }
}
