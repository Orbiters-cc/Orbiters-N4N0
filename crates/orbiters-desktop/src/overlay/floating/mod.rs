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

/// The top bit of the band's id for the window's offers ([`crate::window_rest`]): they and "Talk with N4N0?" both count
/// from 1.
const REST_BIT: u64 = 1 << 63;
/// The top bits of a friend's offer to look at a 3D picture together, and of the share chip's friend picker
/// (`crate::shared_view_ui`).
const SHARE_BIT: u64 = 1 << 62;
const PICK_BIT: u64 = 1 << 61;
/// Steps of a band's line of time left that are painted.
const STEPS: f32 = 60.0;
/// And the world notices' ([`crate::world_notice`], REQUEST13), which count from 1 too.
const NOTICE_BIT: u64 = 1 << 60;

/// What has the band, one at a time, in this order when several are up: "Talk with N4N0?", a friend's offer to look at a
/// 3D picture together, the share chip's friend picker, Keep open (the window's countdown), then the world notice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BandOffer {
    Talk(u64),
    Share(u64),
    Pick(u64),
    Rest(u64),
    Notice(u64),
}

impl BandOffer {
    /// The first of those up.
    fn first(talk: Option<u64>, share: Option<u64>, pick: Option<u64>, rest: Option<u64>, notice: Option<u64>) -> Option<Self> {
        talk.map(Self::Talk).or(share.map(Self::Share)).or(pick.map(Self::Pick)).or(rest.map(Self::Rest)).or(notice.map(Self::Notice))
    }

    /// Its id on the band: each kind counts from 1 under a top bit of its own, so two kinds never share an id.
    fn id(self) -> u64 {
        match self {
            Self::Talk(id) => id,
            Self::Share(id) => id | SHARE_BIT,
            Self::Pick(id) => id | PICK_BIT,
            Self::Rest(id) => id | REST_BIT,
            Self::Notice(id) => id | NOTICE_BIT,
        }
    }
}

/// What the band has to tell the service of the window's offer it shows (`showing`: now), given what it last said:
/// `Some(what)` when that changed, then remembered as said.
fn band_says(said: &mut Option<u64>, showing: Option<u64>) -> Option<Option<u64>> {
    (*said != showing).then(|| {
        *said = showing;
        showing
    })
}

pub struct Floating {
    panel: Panel,
    watch: Watch,
    /// The menu mismatch strip (none when SteamVR would not make its overlay).
    strip: Option<strip::Strip>,
    /// "Talk with N4N0?", or the window's "will minimize to save memory" (none when SteamVR would not make its overlay).
    prompt: Option<prompt::Prompt>,
    /// The window's offer to close the band last told the service it shows (`None`: it said it shows none).
    rest_said: Option<u64>,
    /// When the world notice was last said to be held back behind a menu (once a second at most).
    notice_held: Option<Instant>,
    /// The world notice whose "Take a 3D picture" a trigger pulled: it acts when let go on it ([`notice_release`]).
    notice_down: Option<u64>,
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
            rest_said: None,
            notice_held: None,
            notice_down: None,
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
        // Or, the band free, a friend's offer to look at a 3D picture together, then the share chip's friend picker
        // (`crate::shared_view_ui`).
        let shared = cx.snap.shared_view.arc();
        let share_offer = shared.offer.clone().filter(|o| offer.is_none() && self.prompt.is_some() && o.until_ms > now_ms);
        let pick = crate::shared_view_ui::picking(now_ms).filter(|_| offer.is_none() && share_offer.is_none() && self.prompt.is_some() && shared.outgoing.is_none() && !shared.targets.is_empty());
        // Or, the band free, "Orbiters - N4N0 will minimize to save memory" with Keep open (`crate::window_rest`).
        let rest = cx.snap.window_rest.offer.filter(|o| offer.is_none() && share_offer.is_none() && pick.is_none() && self.prompt.is_some() && cx.snap.window_rest.kept != Some(o.id));
        // The band's ids: a wake offer's, a share offer's, a picker's, or a rest offer's, each kind with its top bit (all count from 1).
        let band = BandOffer::first(offer.map(|o| o.id), share_offer.as_ref().map(|o| o.id), pick.map(|p| p.id), rest.map(|o| o.id), None).map(BandOffer::id);
        // Or, the band still free, "No 3D picture of this world yet" (REQUEST13), never over a menu, our panel or SteamVR's
        // dashboard: held back while one is up (its time waits, a little).
        let notice = cx.snap.explorer.notice.clone().filter(|n| n.up(crate::controller::explorer::now_ms()) && band.is_none() && self.prompt.is_some());
        let in_the_way = cx.menu_open || !cx.menu.is_empty() || cx.dashboard || self.panel.is_shown();
        if let Some(n) = notice.as_ref().filter(|_| in_the_way) {
            if self.notice_held.is_none_or(|at| now.duration_since(at).as_secs_f32() >= 1.0) {
                self.notice_held = Some(now);
                cx.backend.explorer(crate::controller::explorer::Command::NoticeHeld { id: n.id });
            }
        }
        let notice = notice.filter(|_| !in_the_way);
        let band = band.or(notice.as_ref().map(|n| BandOffer::Notice(n.id).id()));
        // The band at the size of what it shows: the world notice is taller; every other face is "Talk with N4N0?"'s size.
        if let (Some(p), Some(n)) = (self.prompt.as_mut(), notice.as_ref()) {
            p.fit(cx.overlay, prompt::Face::World { earns_one: n.earns == Some(1.0), on: None });
        } else if let Some(p) = self.prompt.as_mut().filter(|_| band.is_some()) {
            p.fit(cx.overlay, prompt::Face::Talk);
        }
        if !settings.watch && !strip_on && band.is_none() && !self.panel.is_shown() && !asked && cx.menu.is_empty() {
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
        let prompt = self.prompt.as_mut().and_then(|p| p.place(cx.overlay, band.filter(|_| !cx.dashboard), head.as_ref()));
        // The window counts down only while the band is before the eyes showing its offer: said each time that changes,
        // both ways (hidden by SteamVR's dashboard, replaced by "Talk with N4N0?", gone), never once for good (SWEEP-7).
        let showing = rest.filter(|o| self.prompt.as_ref().is_some_and(|p| p.showing(BandOffer::Rest(o.id).id()))).map(|o| o.id);
        if let Some(said) = band_says(&mut self.rest_said, showing) {
            cx.backend.window_rest_shown(said);
        }

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
                let mut outcome = self.input.step(&head, hands, &targets);
                withhold(&mut outcome, Current {
                    bar: self.panel.bar_current(),
                    watch: self.watch.current(),
                    strip: self.strip.as_ref().is_none_or(|s| s.current()),
                    prompt: self.prompt.as_ref().is_none_or(|p| p.current()),
                });
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
            cx.backend.n4n0(crate::ipc::N4n0Command::WakeOffer { accept: true, id: Some(offer.id) });
            if let Some(p) = self.prompt.as_mut() {
                p.pressed(cx.overlay, offer.id);
            }
        }
        // A friend's offer: Show or Not now, on the release, on its pill.
        if let (Some(o), Some(uv)) = (share_offer.as_ref(), outcome.prompt_release) {
            if let Some(accept) = crate::shared_view_ui::answer_at(uv) {
                clicked = true;
                cx.backend.shared_view(crate::ipc::SharedViewCommand::Answer { id: o.id, accept });
                if let Some(p) = self.prompt.as_mut() {
                    p.pressed(cx.overlay, BandOffer::Share(o.id).id());
                }
            }
        }
        // The picker: shown to the friend whose pill it is.
        if let (Some(pk), Some(uv)) = (pick, outcome.prompt_release) {
            if let Some(friend) = crate::shared_view_ui::pick_at(uv, shared.targets.len()).and_then(|i| shared.targets.get(i)) {
                clicked = true;
                cx.backend.shared_view(crate::ipc::SharedViewCommand::Share { contact: friend.id.clone(), capture: pk.capture, revision: pk.revision, reach: pk.reach, moved: pk.moved });
                crate::shared_view_ui::clear_pick();
                if let Some(p) = self.prompt.as_mut() {
                    p.pressed(cx.overlay, BandOffer::Pick(pk.id).id());
                }
            }
        }
        // "Take a 3D picture" (the picture waits for VRChat's camera), or its cross. Unlike the two above it acts when let go
        // on the band, and a band gone or changed under the held trigger does nothing (verify-vr: presses on release).
        let held = cx.actions.triggers().iter().any(|t| *t);
        let shown = notice.as_ref().map(|n| n.id);
        if let Some(id) = notice_release(&mut self.notice_down, shown, outcome.prompt_press, held, outcome.prompt_hover, cx.input_read) {
            clicked = true;
            cx.backend.explorer(notice_command(id, outcome.prompt_at));
            if let Some(p) = self.prompt.as_mut() {
                p.pressed(cx.overlay, BandOffer::Notice(id).id());
            }
        }
        clicked |= outcome.prompt_press && shown.is_some();
        // Keep open: the window stays.
        if let Some(o) = rest.filter(|_| outcome.prompt_press) {
            clicked = true;
            cx.backend.keep_window_open(o.id);
            if let Some(p) = self.prompt.as_mut() {
                p.pressed(cx.overlay, BandOffer::Rest(o.id).id());
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
            p.paint(cx.overlay, cx.presenter, prompt::Face::Talk, outcome.prompt_hover, offer.left(now_ms));
        } else if let (Some(p), Some(o)) = (self.prompt.as_mut(), share_offer.as_ref()) {
            let hover = outcome.prompt_at.and_then(crate::shared_view_ui::answer_at);
            let left = o.left(now_ms);
            let look = prompt::look_of((o.id, &o.from, &o.title, hover, orbiters_i18n::language(), (left * STEPS).ceil() as u32));
            p.paint_with(cx.overlay, cx.presenter, look, |painter| crate::shared_view_ui::paint_offer(painter, &o.from, &o.title, hover, left));
        } else if let (Some(p), Some(pk)) = (self.prompt.as_mut(), pick) {
            let names: Vec<String> = shared.targets.iter().map(|t| t.name.clone()).collect();
            let hover = outcome.prompt_at.and_then(|uv| crate::shared_view_ui::pick_at(uv, names.len()));
            let left = pk.left(now_ms);
            let look = prompt::look_of((pk.id, &names, hover, orbiters_i18n::language(), (left * STEPS).ceil() as u32));
            p.paint_with(cx.overlay, cx.presenter, look, |painter| crate::shared_view_ui::paint_pick(painter, &names, hover, left));
        } else if let (Some(p), Some(o)) = (self.prompt.as_mut(), rest) {
            p.paint(cx.overlay, cx.presenter, prompt::Face::KeepOpen { seconds: o.seconds(now_ms) }, outcome.prompt_hover, o.left(now_ms));
        } else if let (Some(p), Some(n)) = (self.prompt.as_mut(), notice.as_ref()) {
            // No line of time on it: its picture changes only with the laser; it fades out at SteamVR's alpha.
            let on = outcome.prompt_at.and_then(crate::world_notice::part_at_uv);
            p.paint(cx.overlay, cx.presenter, prompt::Face::World { earns_one: n.earns == Some(1.0), on }, outcome.prompt_hover, 0.0);
            p.fade(cx.overlay, crate::world_notice::opacity(n, crate::controller::explorer::now_ms()));
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
        if let Some(said) = band_says(&mut self.rest_said, None) {
            backend.window_rest_shown(said);
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
        self.notice_down = None;
        self.resting = true;
    }
}

/// The world notice's press, acted on when let go: `down` the notice a pull began on; `shown` the notice the band shows
/// now; `pulled` a trigger was pulled on the band this turn; `held` a trigger is still down; `on` a laser is on the band;
/// `read` this turn's input was read. Returns the notice to take now. A band showing another notice, or none, cancels.
pub fn notice_release(down: &mut Option<u64>, shown: Option<u64>, pulled: bool, held: bool, on: bool, read: bool) -> Option<u64> {
    if shown.is_none() || (down.is_some() && *down != shown) {
        *down = None;
        return None;
    }
    if pulled {
        *down = shown;
        return None;
    }
    if !read || held {
        return None;
    }
    let id = down.take()?;
    on.then_some(id)
}

/// What a let-go on the world notice (`id`) asks, by where the laser was on it (`at`, from its top left): its cross closes
/// it (that world is not told again), anywhere else takes a 3D picture (Request20).
fn notice_command(id: u64, at: Option<[f32; 2]>) -> crate::controller::explorer::Command {
    use crate::controller::explorer::Command;
    match at.and_then(crate::world_notice::part_at_uv) {
        Some(crate::world_notice::Part::Close) => Command::NoticeDismiss { id },
        _ => Command::NoticeTake { id },
    }
}

/// Which of the floating pieces show a picture of their current layout ([`crate::overlay::presented::Presented::current`]).
#[derive(Clone, Copy, Debug)]
struct Current {
    bar: bool,
    watch: bool,
    strip: bool,
    prompt: bool,
}

/// Takes out of a turn's outcome the pointing and pressing on a piece whose picture SteamVR has not taken yet (verify2: its
/// controls are the new picture's, what shows is the old one). The panel itself takes its egui events through the same rule.
fn withhold(outcome: &mut Outcome, current: Current) {
    if !current.bar {
        outcome.bar_hover = None;
        outcome.bar_press = None;
    }
    if !current.watch {
        outcome.watch_hover = false;
        outcome.watch_press = false;
    }
    if !current.strip {
        outcome.strip_hover = None;
        outcome.strip_press = None;
    }
    if !current.prompt {
        outcome.prompt_hover = false;
        outcome.prompt_press = false;
        outcome.prompt_at = None;
        outcome.prompt_release = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// verify2: the bar, the watch, the strip and the band are neither pointed at nor pressed while their picture is not the
    /// current one; each piece on its own, the others untouched.
    #[test]
    fn a_floating_piece_behind_its_picture_is_not_pressed() {
        let all = Outcome {
            bar_hover: Some([0.5, 0.5]),
            bar_press: Some([0.5, 0.5]),
            watch_hover: true,
            watch_press: true,
            strip_hover: Some([0.5, 0.5]),
            strip_press: Some([0.5, 0.5]),
            prompt_hover: true,
            prompt_press: true,
            prompt_at: Some([0.5, 0.5]),
            prompt_release: Some([0.5, 0.5]),
            ..Default::default()
        };
        let every = Current { bar: true, watch: true, strip: true, prompt: true };
        let mut kept = all;
        withhold(&mut kept, every);
        assert_eq!(kept, all, "all current: nothing withheld");
        let mut o = all;
        withhold(&mut o, Current { bar: false, ..every });
        assert!(o.bar_hover.is_none() && o.bar_press.is_none() && o.watch_press && o.prompt_press);
        let mut o = all;
        withhold(&mut o, Current { watch: false, ..every });
        assert!(!o.watch_hover && !o.watch_press && o.bar_press.is_some());
        let mut o = all;
        withhold(&mut o, Current { strip: false, ..every });
        assert!(o.strip_hover.is_none() && o.strip_press.is_none() && o.watch_press);
        let mut o = all;
        withhold(&mut o, Current { prompt: false, ..every });
        assert!(!o.prompt_hover && !o.prompt_press && o.prompt_at.is_none() && o.prompt_release.is_none() && o.strip_press.is_some());
    }

    /// Every kind of offer on the band in turn (night4/window and explorer-app together): one at a time in the band's
    /// order, each under an id of its own, Keep open counted only while it shows, the world notice only when the band is
    /// free of the four before it and acting on the let-go on it, and each one withdrawn leaving the band to the next.
    /// (The pulls and releases of Talk, Keep open, the friend's offer and the picker, and SteamVR's own confirmation of
    /// the picture, are in `Floating::update` and `prompt.rs`'s own test.)
    #[test]
    fn every_offer_on_the_band_keeps_its_own_rules() {
        use BandOffer::*;
        // One number for all five kinds: their ids never meet.
        let all = [Talk(7), Share(7), Pick(7), Rest(7), Notice(7)];
        let ids: std::collections::BTreeSet<u64> = all.iter().map(|o| o.id()).collect();
        assert_eq!(ids.len(), all.len(), "{ids:?}");
        // In turn: with a kind and those after it up, that kind has the band; all withdrawn, the band is free.
        for (i, kind) in all.iter().enumerate() {
            let up = |k: usize| (k >= i).then_some(7u64);
            assert_eq!(BandOffer::first(up(0), up(1), up(2), up(3), up(4)), Some(*kind), "with {kind:?} and those after it up");
        }
        assert_eq!(BandOffer::first(None, None, None, None, None), None, "all withdrawn: the band is free");
        // Keep open: the window counts down only while the band shows it; any kind before it replaces it, and it says so.
        let rest_showing = |band: Option<BandOffer>| band.and_then(|b| matches!(b, Rest(_)).then_some(7));
        let mut said = None;
        assert_eq!(band_says(&mut said, rest_showing(BandOffer::first(None, None, None, Some(7), Some(3)))), Some(Some(7)), "Keep open shows, before the notice");
        for (talk, share, pick) in [(Some(1), None, None), (None, Some(2), None), (None, None, Some(4))] {
            assert_eq!(band_says(&mut said, rest_showing(BandOffer::first(talk, share, pick, Some(7), None))), Some(None), "replaced: not counting");
            assert_eq!(band_says(&mut said, rest_showing(BandOffer::first(None, None, None, Some(7), None))), Some(Some(7)), "back: counting again");
        }
        assert_eq!(band_says(&mut said, rest_showing(BandOffer::first(None, None, None, None, Some(3)))), Some(None), "withdrawn: not counting");
        // The world notice: not on the band under another kind; on it, nothing on the pull, the picture on the let-go on it,
        // and nothing when another kind took the band under the held trigger.
        let notice_shown = |band: Option<BandOffer>| band.and_then(|b| if let Notice(n) = b { Some(n) } else { None });
        let mut down = None;
        assert_eq!(notice_release(&mut down, notice_shown(BandOffer::first(Some(1), None, None, None, Some(3))), true, true, true, true), None, "under Talk it is not on the band");
        let shown = notice_shown(BandOffer::first(None, None, None, None, Some(3)));
        assert_eq!(notice_release(&mut down, shown, true, true, true, true), None, "the pull does nothing");
        assert_eq!(notice_release(&mut down, shown, false, false, true, true), Some(3), "the let-go on it takes the picture");
        assert_eq!(notice_release(&mut down, shown, true, true, true, true), None);
        assert_eq!(notice_release(&mut down, notice_shown(BandOffer::first(None, None, None, Some(7), Some(3))), false, false, true, true), None, "Keep open came under the held trigger: nothing");
    }

    /// The notice's "Take a 3D picture": nothing on the pull, done on the let-go on it; let go off it, or the band gone or
    /// showing another notice under the held trigger, nothing.
    #[test]
    fn the_world_notice_acts_when_let_go_on_it_and_never_after_it_went() {
        let mut down = None;
        assert_eq!(notice_release(&mut down, Some(4), true, true, true, true), None, "the pull does nothing yet");
        assert_eq!(notice_release(&mut down, Some(4), false, true, true, true), None, "held");
        assert_eq!(notice_release(&mut down, Some(4), false, false, true, false), None, "input not read: nothing decided");
        assert_eq!(notice_release(&mut down, Some(4), false, false, true, true), Some(4), "let go on it");
        assert_eq!(notice_release(&mut down, Some(4), false, false, true, true), None, "once");
        notice_release(&mut down, Some(4), true, true, true, true);
        assert_eq!(notice_release(&mut down, Some(4), false, false, false, true), None, "let go off it");
        assert_eq!(down, None);
        notice_release(&mut down, Some(4), true, true, true, true);
        assert_eq!(notice_release(&mut down, None, false, true, false, true), None, "the band went under the trigger");
        assert_eq!(notice_release(&mut down, Some(4), false, false, true, true), None, "and came back: nothing");
        notice_release(&mut down, Some(4), true, true, true, true);
        assert_eq!(notice_release(&mut down, Some(5), false, false, true, true), None, "another notice");
    }

    /// Request20: let go on the notice's cross, it closes; anywhere else on it, it takes a 3D picture.
    #[test]
    fn the_notice_s_cross_closes_it_and_the_rest_takes_a_picture() {
        use crate::controller::explorer::Command;
        assert_eq!(notice_command(3, Some([0.97, 0.5])), Command::NoticeDismiss { id: 3 });
        assert_eq!(notice_command(3, Some([0.86, 0.05])), Command::NoticeDismiss { id: 3 }, "the cross's part is the card's whole height");
        assert_eq!(notice_command(3, Some([0.4, 0.7])), Command::NoticeTake { id: 3 });
        assert_eq!(notice_command(3, Some([0.05, 0.1])), Command::NoticeTake { id: 3 });
        assert_eq!(notice_command(3, None), Command::NoticeTake { id: 3 }, "on it with no place (its edge): the card's own action");
    }

    /// Night3, SWEEP-7 and -8: what the band says of the window's offer follows what it shows, both ways, every time:
    /// shown, hidden (the dashboard, "Talk with N4N0?"), shown again; and an offer shown, gone, then one of the same
    /// number (a later window) is said again (it was said once for good and never again).
    #[test]
    fn the_band_says_what_it_shows_each_time_it_changes() {
        let mut said = None;
        assert_eq!(band_says(&mut said, Some(1)), Some(Some(1)), "shown");
        assert_eq!(band_says(&mut said, Some(1)), None, "nothing new");
        assert_eq!(band_says(&mut said, None), Some(None), "hidden");
        assert_eq!(band_says(&mut said, Some(1)), Some(Some(1)), "shown again");
        assert_eq!(band_says(&mut said, None), Some(None), "gone");
        assert_eq!(band_says(&mut said, Some(1)), Some(Some(1)), "the same number again: said again");
    }

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
