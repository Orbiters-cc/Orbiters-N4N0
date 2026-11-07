//! 3D worlds in the play space: search VRChat's worlds from N4N0's menu overlay (every world: the member's own VRChat
//! session when signed in, else Orbiters' Map explorer; VRChat's lists, the worlds visited: [`finder`]), those members
//! captured and shared on Orbiters (the listing the VRChat world "N4N0 - 3D picture viewer" shows) badged 3D, and see
//! one of those around you in SteamVR, placed as that world places it (its ground on your floor, its start view where
//! you stand, facing your way) and drawn from the same files (docs/world-view-overlay.md). A world without a capture
//! has its page: Invite me (an invite to oneself in VRChat) or Launch VRChat, and Open on VRChat.
//!
//! A pill under N4N0's menu ("3D worlds") opens the panel ([`panel`]) in the menu's place; the menu's Back comes back
//! to it. Once a capture shows or loads, a bar takes the pill's place, under the menu or the panel whatever page shows:
//! the capture (its page a press away), See VRChat, Recenter, how sharp, Hide. The capture stays around you once the
//! menu closes. Captures shown lately are kept ([`cache`]): showing one again needs no download.
//!
//! A capture's page gives its world's feedback as the VRChat world and the website do: what the panel asks (a world's
//! feedback read, the member's like, labels, comments) goes to the tray service as N4N0's menus' asks do
//! (controller/world_feedback.rs), and comes back in the snapshot.

pub mod cache;
pub mod catalog;
pub mod small_hint;
pub mod stick3d;

/// With this few captures, the list of them goes on with popular worlds.
const FEW_CAPTURES: usize = 3;

/// How much of a hole's width a capture lying in it fills: its top and foot are cut by the hole, as captures are wider than tall.
pub const SMALL_FILLS_HOLE: f32 = 0.62;
mod diagnostics;
pub mod finder;
#[cfg(windows)]
pub mod interop;
pub mod panel;
pub mod pacing;
pub mod render;
pub mod shots;
pub mod stereo;
pub mod timing_lab;
pub mod view;

use super::{Canvas, Presenter};
use orbiters_i18n::tr;
use crate::backend::Ctl;
use crate::controller::vrchat_data::{Request as DataRequest, Response as DataResponse, Scope};
use crate::ipc::N4n0Command;
use crate::state::Snapshot;
use catalog::{Capture, Catalog, Entry, Progress, Source};
use egui::TextureId;
use orbiters_core::maps::WorldList;
use orbiters_openvr::{self as vr, overlay_flag, Matrix34};
use orbiters_n4n0_core::feedback::{CommentRow, FeedbackAsk, FeedbackView};
use orbiters_n4n0_core::menu::KeyboardLayout;
use panel::{Action, Data, Facts, ListStatus, Panel, Section, Status, Target, BAR_PX, CHIP_PX};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use view::{Quality, View};

/// The pill's width in the play space, and its gap under the menu.
const CHIP_WIDTH_M: f32 = 0.2;
const CHIP_GAP_M: f32 = 0.012;
/// The bar is as wide as the menu.
const BAR_WIDTH_M: f32 = super::menu::WIDTH_M;
/// A world's feedback is asked for again after this long while one of its capture's pages shows.
const ASK_AGAIN: Duration = Duration::from_secs(120);
/// Worlds' pages still on their way are looked for this often (a picture of what loaded).
const FACTS_EVERY: Duration = Duration::from_millis(250);
/// The bar's loading ring turns at this rate.
const BAR_EVERY: Duration = Duration::from_millis(33);
/// A search is asked once the typing paused this long (Done asks at once).
const TYPING_PAUSE: Duration = Duration::from_millis(400);

/// What a panel on a sheet asks of the overlay's own 3D worlds, which has the 3D view.
#[derive(Clone, Debug)]
pub enum Relayed {
    Show(Entry),
    /// A capture of this computer's gallery: its session, what it is called, and what tells its results apart.
    ShowLocal { dir: std::path::PathBuf, name: String, changed: u64 },
    View(Action),
}

/// The world list the panel shows: the 3D captures, the worlds visited, or a list VRChat (or Orbiters) answers.
#[derive(Clone, Debug, PartialEq)]
enum Shown {
    Captures,
    Visited,
    List(WorldList),
}

/// A capture on its way: its listing id and files, the download's progress, the answer.
struct Download {
    id: u64,
    key: cache::Key,
    progress: Arc<Progress>,
    answer: mpsc::Receiver<anyhow::Result<Capture>>,
}

/// What the bar shows: when it changes, it is painted again.
type BarLook = (Option<Target>, Option<Target>, Option<u64>, bool, bool, Quality, Option<u16>, Option<String>, Option<String>);

pub struct Worlds {
    /// The panel shows in the menu's place.
    pub open: bool,
    panel: Panel,
    catalog: Catalog,
    source: Source,
    pub view: View,
    download: Option<Download>,
    /// The capture shown, or on its way (its entry).
    current: Option<Entry>,
    error: Option<String>,
    /// Captures shown lately, ready to show again.
    captures: cache::Memory,
    /// On a screen: the capture turning on the side panel, its picture, the pointer on it, and whether it turns by itself.
    live: Option<(u64, super::orbit::sheet_view::SheetView)>,
    live_frame: Option<super::orbit::sheet_view::Frame>,
    live_texture: Option<egui::TextureHandle>,
    live_pointer: Option<(f32, f32)>,
    live_turns: bool,
    chip: vr::OverlayHandle,
    chip_canvas: Canvas,
    chip_shown: bool,
    chip_hover: bool,
    chip_pressed: bool,
    chip_drawn: Option<(bool, bool)>,
    /// Where the pill was put (under the menu as it was then).
    chip_placed: Option<Matrix34>,
    bar: vr::OverlayHandle,
    bar_canvas: Canvas,
    bar_shown: bool,
    bar_hover: Option<Target>,
    bar_pressed: Option<Target>,
    bar_drawn: Option<BarLook>,
    bar_painted: Option<Instant>,
    bar_placed: Option<Matrix34>,
    /// The bar asked for the capture's page while the panel was closed: the host opens it.
    open_asked: bool,
    /// What the computer's keys asked, done on the next turn.
    deferred: Vec<Action>,
    /// The worlds' feedback the panel shows: the app's, with the panel's own presses applied until the app has them.
    feedback: FeedbackView,
    feedback_seen: FeedbackView,
    /// What the panel asks of the app, sent on the next turn.
    outbox: Vec<FeedbackAsk>,
    /// The capture whose world's feedback was last asked for, and when.
    asked: Option<(u64, Instant)>,
    /// The worlds' pages on the Map explorer, as far as fetched (performance, tags, pictures).
    facts: HashMap<String, Facts>,
    facts_pending: bool,
    /// On a sheet in VR: what the overlay's own 3D worlds say of their view, and what was asked of them.
    relay: Option<Status>,
    relayed: Vec<Relayed>,
    /// The player's VRChat name (a capture shared with some players only is listed for them).
    me: Option<String>,
    infos: crate::worlds::Infos,
    pictures: crate::worlds::Pictures,
    /// VRChat's worlds in general: lists' pages read lately, the worlds visited.
    finder: finder::Finder,
    visited: finder::Visited,
    /// What the finder and the panel showed last (a change paints again).
    found_seen: (u64, u64),
    /// The tray service (VRChat's requests go through it), the account and session they are made for, signed in to
    /// VRChat, and VRChat's region of the instance the player is in (new instances go there).
    backend: Option<Ctl>,
    scope: Option<Scope>,
    vrchat: bool,
    region: &'static str,
    /// What was typed and when it last changed, what is searched (the typing paused, or Done), Done pressed.
    typed: (String, Instant),
    searched: String,
    search_now: bool,
    /// The world whose page shows, kept while the lists change under it.
    picked: Option<Entry>,
    /// What a Join or an Open on VRChat said, from its thread.
    said: (mpsc::Sender<String>, mpsc::Receiver<String>),
    ctx: egui::Context,
    epoch: Instant,
    keys_seen: Option<u32>,
    last_paint: Option<Instant>,
    /// With no SteamVR: nothing can show around or before the person.
    alone: bool,
    key: String,
    /// What says how to handle what shows small; it has been used; a new opening of the menu asks it back for the next capture.
    hint: Option<small_hint::SmallHint>,
    backdrop: Option<small_hint::Backdrop>,
    small_logged: super::vr_log::Every,
    hide_chip: Option<small_hint::HideChip>,
    small_place: Option<(vr::Matrix34, f32)>,
    /// Where each hand's tool is and which way it points, and its trigger, now and a turn ago.
    small_rays: [Option<([f32; 3], [f32; 3])>; 2],
    small_triggers: [bool; 2],
    small_triggers_were: [bool; 2],
    hint_used: bool,
    /// The menus up now.
    menu_boxes: Vec<stereo::MenuBox>,
}

/// Now, in Unix seconds.
fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The pill's or the bar's overlay, `width_m` wide. No SteamVR input method: its laser mouse would take VRChat's input
/// while they show. The controllers' own lasers send them the same mouse events instead, in `px` (`Overlay::inject_event`,
/// from `overlay::floating`; [`Worlds::pointables`] says where they are).
fn interactive(overlay: &vr::Overlay, handle: vr::OverlayHandle, width_m: f32) {
    let _ = overlay.set_width_in_meters(handle, width_m);
    let _ = overlay.set_flag(handle, overlay_flag::IS_PREMULTIPLIED, true);
    let _ = overlay.set_sort_order(handle, 10);
}

/// The transform `height_m` tall hanging under the menu at `at`.
fn under_menu(at: &Matrix34, height_m: f32) -> Matrix34 {
    let menu_height = super::menu::WIDTH_M * orbiters_n4n0_core::menu::layout::HEIGHT as f32 / orbiters_n4n0_core::menu::layout::WIDTH as f32;
    let down = menu_height / 2.0 + CHIP_GAP_M + height_m / 2.0;
    let mut t = *at;
    for i in 0..3 {
        t.m[i][3] -= at.m[i][1] * down;
    }
    t
}

impl Worlds {
    /// The pill's and the bar's overlays; the panel draws on the menu's canvas, whose context `ctx` is.
    pub fn new(overlay: &vr::Overlay, key: &str, ctx: egui::Context) -> anyhow::Result<Self> {
        let chip = overlay.create(&format!("{key}.worlds-chip"), "N4N0's 3D worlds").map_err(|c| anyhow::anyhow!("3D worlds pill: SteamVR overlay error {c}"))?; // i18n: not shown (the overlay's name in SteamVR)
        interactive(overlay, chip, CHIP_WIDTH_M);
        let bar = overlay.create(&format!("{key}.worlds-bar"), "N4N0's 3D world controls").map_err(|c| anyhow::anyhow!("3D worlds bar: SteamVR overlay error {c}"))?; // i18n: not shown (the overlay's name in SteamVR)
        interactive(overlay, bar, BAR_WIDTH_M);
        Ok(Self {
            open: false,
            panel: Panel::default(),
            catalog: Catalog::default(),
            source: Source::current(),
            view: View::new(key),
            download: None,
            current: None,
            error: None,
            captures: cache::Memory::with_default_budget(),
            live: None,
            live_frame: None,
            live_texture: None,
            live_pointer: None,
            live_turns: true,
            chip,
            chip_canvas: Canvas::new(CHIP_PX[0], CHIP_PX[1]),
            chip_shown: false,
            chip_hover: false,
            chip_pressed: false,
            chip_drawn: None,
            chip_placed: None,
            bar,
            bar_canvas: Canvas::new(BAR_PX[0], BAR_PX[1]),
            bar_shown: false,
            bar_hover: None,
            bar_pressed: None,
            bar_drawn: None,
            bar_painted: None,
            bar_placed: None,
            open_asked: false,
            deferred: Vec::new(),
            feedback: FeedbackView::default(),
            feedback_seen: FeedbackView::default(),
            outbox: Vec::new(),
            asked: None,
            facts: HashMap::new(),
            facts_pending: false,
            relay: None,
            relayed: Vec::new(),
            me: None,
            infos: crate::worlds::Infos::new(&ctx),
            pictures: crate::worlds::Pictures::new(&ctx),
            finder: finder::Finder::default(),
            visited: finder::Visited::default(),
            found_seen: (0, 0),
            backend: None,
            scope: None,
            vrchat: false,
            region: "us",
            typed: (String::new(), Instant::now()),
            searched: String::new(),
            search_now: false,
            picked: None,
            said: mpsc::channel(),
            ctx,
            epoch: Instant::now(),
            keys_seen: None,
            last_paint: None,
            alone: false,
            key: key.to_string(),
            hint: None,
            hide_chip: None,
            small_rays: [None; 2],
            small_triggers: [false; 2],
            small_triggers_were: [false; 2],
            backdrop: None,
            small_logged: Default::default(),
            small_place: None,
            hint_used: false,
            menu_boxes: Vec::new(),
        })
    }

    /// The panel alone, for the desktop's panels around VRChat's menus (`overlay::orbit`): no SteamVR, no pill, no bar, nothing shown around the person. Its lists, a world's page
    /// and its feedback work as ever; a capture opens in the Orbiters window instead ([`Worlds::mouse_alone`]).
    pub fn alone(ctx: egui::Context) -> Self {
        Self {
            open: false,
            panel: { let mut panel = Panel::default(); panel.on_sheet = true; panel },
            catalog: Catalog::default(),
            source: Source::current(),
            view: View::new("cc.orbiters.desktop.orbit"),
            download: None,
            current: None,
            error: None,
            captures: cache::Memory::with_default_budget(),
            live: None,
            live_frame: None,
            live_texture: None,
            live_pointer: None,
            live_turns: true,
            chip: 0,
            chip_canvas: Canvas::new(1, 1),
            chip_shown: false,
            chip_hover: false,
            chip_pressed: false,
            chip_drawn: None,
            chip_placed: None,
            bar: 0,
            bar_canvas: Canvas::new(1, 1),
            bar_shown: false,
            bar_hover: None,
            bar_pressed: None,
            bar_drawn: None,
            bar_painted: None,
            bar_placed: None,
            open_asked: false,
            deferred: Vec::new(),
            feedback: FeedbackView::default(),
            feedback_seen: FeedbackView::default(),
            outbox: Vec::new(),
            asked: None,
            facts: HashMap::new(),
            facts_pending: false,
            relay: None,
            relayed: Vec::new(),
            me: None,
            infos: crate::worlds::Infos::new(&ctx),
            pictures: crate::worlds::Pictures::new(&ctx),
            finder: finder::Finder::default(),
            visited: finder::Visited::default(),
            found_seen: (0, 0),
            backend: None,
            scope: None,
            vrchat: false,
            region: "us",
            typed: (String::new(), Instant::now()),
            searched: String::new(),
            search_now: false,
            picked: None,
            said: mpsc::channel(),
            ctx,
            epoch: Instant::now(),
            keys_seen: None,
            last_paint: None,
            alone: true,
            key: String::new(),
            hint: None,
            hide_chip: None,
            small_rays: [None; 2],
            small_triggers: [false; 2],
            small_triggers_were: [false; 2],
            backdrop: None,
            small_logged: Default::default(),
            small_place: None,
            hint_used: false,
            menu_boxes: Vec::new(),
        }
    }

    /// A turn of [`Worlds::alone`]: what the panel asked of the app, the app's feedback, the lists.
    pub fn update_alone(&mut self, snap: &Snapshot, backend: &Ctl) {
        if snap.n4n0.feedback != self.feedback_seen {
            self.feedback_seen = snap.n4n0.feedback.clone();
            self.feedback = snap.n4n0.feedback.clone();
            self.panel.dirty = true;
        }
        let me = snap.vrchat_login.user.as_ref().map(|u| u.display_name.clone()).filter(|n| !n.trim().is_empty());
        if me != self.me {
            self.me = me;
            self.panel.dirty = true;
        }
        if self.backend.is_none() {
            self.backend = Some(backend.clone());
        }
        let scope = Scope::of(snap);
        if self.scope.as_ref() != Some(&scope) {
            if self.scope.is_some() {
                self.finder.clear();
            }
            self.scope = Some(scope);
            self.vrchat = snap.vrchat_login.allowed && snap.vrchat_login.user.is_some();
            self.panel.dirty = true;
        }
        self.region = finder::region(snap.vrchat.world.instance.as_deref());
        self.live_turns = !snap.settings.reduce_motion;
        // What turns on the side panel: its newest picture, or that it cannot.
        if self.panel.page != panel::Page::Detail || !self.open {
            self.stop_live();
        }
        if let Some((_, view)) = &self.live {
            let failed = matches!(view.state(), super::orbit::sheet_view::ViewState::Failed(_));
            let frame = view.take_frame();
            let was_failed = self.panel.live.as_ref().is_some_and(|l| l.failed);
            if frame.is_some() || failed != was_failed {
                self.live_frame = frame.or(self.live_frame.take());
                if let Some(live) = self.panel.live.as_mut() {
                    live.failed = failed;
                }
                self.panel.dirty = true;
            }
        }
        while let Ok(said) = self.said.1.try_recv() {
            let time = self.time();
            self.panel.say(said, time);
            self.panel.dirty = true;
        }
        let deferred = std::mem::take(&mut self.deferred);
        if !deferred.is_empty() {
            let entries = self.entries();
            self.act_alone(deferred, &entries);
        }
        self.find();
        let seen = (self.finder.version(), self.visited.state().1);
        if seen != self.found_seen {
            self.found_seen = seen;
            self.panel.dirty = true;
        }
        self.ask_feedback();
        for ask in self.outbox.drain(..) {
            backend.n4n0(N4n0Command::Feedback { ask });
        }
    }

    /// Starts `entry` turning on the side panel. It reads what is on this computer, or what was downloaded before, as any showing of it does.
    fn start_live(&mut self, entry: &Entry) {
        let (source, from) = (self.source.clone(), entry.clone());
        let load = move || match &from.local {
            Some(dir) => catalog::read_local(dir, &Default::default()),
            None => source.download(&from, &Default::default()),
        };
        self.live = Some((entry.id, super::orbit::sheet_view::SheetView::open_with(load, self.live_turns, 2)));
        self.live_frame = None;
        self.live_texture = None;
        self.panel.live = Some(panel::Live { id: entry.id, ..Default::default() });
        self.panel.dirty = true;
    }

    /// All of it painted again: how it looks changed, or the language did (the pill and the bar are drawn only when what they show changes).
    pub fn repaint(&mut self) {
        self.panel.dirty = true;
        self.chip_drawn = None;
        self.bar_drawn = None;
    }

    /// What turns on the side panel goes, with all it holds.
    pub fn stop_live(&mut self) {
        if self.live.take().is_some() {
            self.live_frame = None;
            self.live_texture = None;
            self.panel.live = None;
            self.panel.dirty = true;
        }
    }

    /// Goes on from one world to the next as the person does (to see quickly what they look like), and stops with the panel.
    fn follow_live(&mut self, entries: &[Entry]) {
        let Some((id, _)) = &self.live else { return };
        let id = *id;
        if self.panel.page != panel::Page::Detail {
            self.stop_live();
        } else if self.panel.selected != Some(id) {
            let next = self.panel.selected.and_then(|s| entries.iter().find(|e| e.id == s).or(self.picked.as_ref().filter(|e| e.id == s))).filter(|e| e.capture).cloned();
            match next {
                Some(entry) => self.start_live(&entry),
                None => self.stop_live(),
            }
        }
    }

    /// The mouse's wheel over what turns on the side panel.
    pub fn wheel_alone(&mut self, notches: f32) {
        if let (Some((_, view)), Some(panel::Target::Hero)) = (&self.live, self.panel.hover) {
            view.zoom(notches);
        } else {
            self.panel.scroll_by(-notches * 120.0);
        }
    }

    /// The same as [`Worlds::act`] with no SteamVR: nothing can be shown around the person, so a capture opens in the Orbiters window, and what only the view has does nothing.
    fn act_alone(&mut self, actions: Vec<Action>, entries: &[Entry]) {
        let headset = self.status().headset;
        for action in actions {
            match action {
                // On a screen: on this panel, as a capture in the gallery does.
                Action::Show(id) if !headset => {
                    if let Some(entry) = entries.iter().find(|e| e.id == id).or(self.picked.as_ref().filter(|e| e.id == id)).cloned() {
                        self.start_live(&entry);
                    }
                }
                // On a sheet in VR: for the overlay's own 3D worlds, which has the view.
                Action::Show(id) if self.relay.is_some() => {
                    if let Some(entry) = entries.iter().find(|e| e.id == id).or(self.picked.as_ref().filter(|e| e.id == id)) {
                        self.relayed.push(Relayed::Show(entry.clone()));
                    }
                }
                action @ (Action::Hide | Action::Recenter | Action::SeeThrough(_) | Action::Small(_) | Action::SmallBack | Action::Quality(_)) if self.relay.is_some() => self.relayed.push(Relayed::View(action)),
                Action::Show(_) => {
                    if let Some(backend) = &self.backend {
                        backend.show_window();
                    }
                    let time = self.time();
                    self.panel.say(tr("Opened in the Orbiters window, where it shows in 3D"), time);
                }
                Action::Hide | Action::Recenter | Action::SeeThrough(_) | Action::Small(_) | Action::SmallBack | Action::Quality(_) | Action::Open => {}
                Action::Leave => self.open = false,
                Action::Refresh => {
                    self.catalog.refresh(&self.source);
                    self.finder.retry();
                }
                Action::Search => self.search_now = true,
                Action::Join(world) => self.join(world),
                Action::OpenWeb(world) => {
                    let Some(link) = finder::web_link(&world) else { continue };
                    let tx = self.said.0.clone();
                    let _ = std::thread::Builder::new().name("3d-worlds-web".into()).spawn(move || {
                        let said = match crate::controller::open_url(&link) {
                            Ok(()) => tr("Its page opened in the computer's browser").to_string(),
                            Err(e) => tr!("The browser did not open: {e}", e = e),
                        };
                        let _ = tx.send(said);
                    });
                }
                Action::Feedback(ask) => {
                    self.apply(&ask);
                    self.outbox.push(ask);
                }
            }
        }
        let _ = entries;
        self.panel.dirty = true;
    }

    /// The pointer on the panel of [`Worlds::alone`] at (x, y) in its own units, from its top left.
    pub fn hover_alone(&mut self, at: Option<(f32, f32)>) {
        let entries = self.entries();
        let status = self.status();
        let (no_thumb, no_picture) = (|_: &Entry| -> Option<TextureId> { None }, |_: u64, _: bool| -> Option<TextureId> { None });
        let picked = self.picked().cloned();
        let d = Data { entries: &entries, picked: picked.as_ref(), status: &status, feedback: &self.feedback, facts: &self.facts, thumb: &no_thumb, picture: &no_picture, now: unix_now() };
        let hover = at.and_then(|(x, y)| self.panel.hit(&d, x, y));
        // A hand on what turns on the panel turns it.
        if let (Some(panel::Target::Hero), Some((_, view)), Some(from), Some(now)) = (self.panel.pressed, &self.live, self.live_pointer, at) {
            view.turn(now.0 - from.0, now.1 - from.1);
        }
        self.live_pointer = at;
        if let Some((_, y)) = at {
            self.panel.vr_drag(y, self.time());
        }
        // A list being dragged is no press of what passes under the hand.
        let hover = if self.panel.vr_dragging() { None } else { hover };
        if hover != self.panel.hover {
            self.panel.hover = hover;
            self.panel.dirty = true;
        }
    }

    /// A press where the pointer is on the panel of [`Worlds::alone`]. Returns whether something was hit.
    pub fn press_alone(&mut self) -> bool {
        let entries = self.entries();
        let status = self.status();
        let time = self.time();
        let picked = self.picked().cloned();
        // On a headset's endless list a press waits: it may be a drag of the list.
        if let Some((_, y)) = self.live_pointer.filter(|at| self.panel.vr_list_on(&status) && self.panel.vr_list_holds(&status, at.0, at.1)) {
            if matches!(self.panel.hover, None | Some(panel::Target::Card(_))) {
                self.panel.pressed = self.panel.hover;
                self.panel.vr_press(y, self.panel.hover, time);
                return true;
            }
        }
        let Some(target) = self.panel.hover else { return false };
        self.press_target(target, &entries, &status, picked.as_ref(), time);
        true
    }

    fn press_target(&mut self, target: panel::Target, entries: &[Entry], status: &panel::Status, picked: Option<&Entry>, time: f64) {
        let (no_thumb, no_picture) = (|_: &Entry| -> Option<TextureId> { None }, |_: u64, _: bool| -> Option<TextureId> { None });
        self.panel.pressed = Some(target);
        let actions = {
            let d = Data { entries, picked, status, feedback: &self.feedback, facts: &self.facts, thumb: &no_thumb, picture: &no_picture, now: unix_now() };
            self.panel.press(target, &d, time)
        };
        self.keep_picked(entries);
        if let (panel::Target::Hero, Some((_, view))) = (target, &self.live) {
            view.hold(true);
        }
        self.act_alone(actions, entries);
        self.follow_live(entries);
    }

    #[cfg(test)]
    pub fn show_world_for_test(&mut self, id: u64) {
        self.panel.page = panel::Page::Detail;
        self.panel.selected = Some(id);
        self.panel.dirty = true;
    }

    #[cfg(test)]
    pub fn status_for_test(&self) -> Status {
        self.status()
    }

    /// Whether the pointer is on something of the panel.
    pub fn points_at_something(&self) -> bool {
        self.panel.hover.is_some() || (self.panel.vr_dragging())
    }

    /// The press of [`Worlds::press_alone`] let go.
    pub fn release_alone(&mut self) {
        if let (Some(panel::Target::Hero), Some((_, view))) = (self.panel.pressed, &self.live) {
            view.hold(false);
        }
        // A press on the endless list that did not become a drag: it is a press after all.
        let time = self.time();
        if let Some(target) = self.panel.vr_release(time) {
            let entries = self.entries();
            let status = self.status();
            let picked = self.picked().cloned();
            self.press_target(target, &entries, &status, picked.as_ref(), time);
        }
        self.panel.pressed = None;
        self.panel.dirty = true;
    }

    /// The pill and the bar while they show, for the controllers' lasers: each overlay, where it is and its pixels
    /// (their mouse events count in those, from the bottom left).
    pub fn pointables(&self) -> Vec<super::floating::Pointable> {
        use super::floating::{space::Quad, Pointable};
        let mut out = Vec::new();
        for (shown, handle, placed, width, px) in
            [(self.chip_shown, self.chip, self.chip_placed, CHIP_WIDTH_M, CHIP_PX), (self.bar_shown, self.bar, self.bar_placed, BAR_WIDTH_M, BAR_PX)]
        {
            if let Some(transform) = placed.filter(|_| shown) {
                out.push(Pointable { handle, quad: Quad { transform, width, height: width * px[1] as f32 / px[0] as f32 }, px });
            }
        }
        out
    }

    pub fn destroy(&mut self, overlay: &vr::Overlay) {
        self.view.destroy(overlay);
        if let Some(hint) = self.hint.take() {
            hint.destroy(overlay);
        }
        if let Some(backdrop) = self.backdrop.take() {
            backdrop.destroy(overlay);
        }
        if let Some(chip) = self.hide_chip.take() {
            chip.destroy(overlay);
        }
        let _ = overlay.destroy(self.chip);
        let _ = overlay.destroy(self.bar);
    }

    fn time(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    /// The listing's captures this player may see.
    fn captures(&self) -> Vec<Entry> {
        let (entries, ..) = self.catalog.snapshot();
        entries.into_iter().filter(|e| e.for_player(self.me.as_deref())).collect()
    }

    /// The list shown: a search once two characters are typed and the typing paused, else what is browsed.
    fn shown(&self) -> Shown {
        let q = self.searched.trim();
        if q.chars().count() >= 2 {
            return Shown::List(WorldList::Search(q.to_lowercase()));
        }
        match self.panel.section {
            Section::Captures => Shown::Captures,
            Section::Visited => Shown::Visited,
            s => s.list().map_or(Shown::Captures, Shown::List),
        }
    }

    /// What the panel lists: the list shown, its worlds that have captures as their captures ([`finder::merge`]).
    fn entries(&self) -> Vec<Entry> {
        let captures = self.captures();
        let feedback = |w: &str| self.catalog.feedback(w);
        match self.shown() {
            // Few so far: worlds to look at after them, rather than free space.
            Shown::Captures if captures.len() <= FEW_CAPTURES => {
                let listed = self.finder.list(&WorldList::Popular);
                let first: Vec<&Entry> = captures.iter().collect();
                let mut out = finder::merge(&captures, &first, &listed.items, None, &feedback);
                // Under their heading, a few: Popular has the rest.
                out.truncate(captures.len() + panel::FEW_MORE);
                out
            }
            Shown::Captures => captures,
            Shown::Visited => {
                let (worlds, ..) = self.visited.worlds();
                let found = finder::visited_found(&worlds);
                let items: Vec<_> = found.iter().map(|(f, ..)| f.clone()).collect();
                let when: HashMap<String, (u64, usize)> = found.iter().map(|(f, last, n)| (f.world.world_id.to_ascii_lowercase(), (*last, *n))).collect();
                let mut out = finder::merge(&captures, &[], &items, None, &feedback);
                for e in &mut out {
                    e.visited = when.get(&e.world_id.to_ascii_lowercase()).copied();
                }
                out
            }
            Shown::List(list) => {
                let listed = self.finder.list(&list);
                let (found_by, first): (Option<&str>, Vec<&Entry>) = match &list {
                    WorldList::Search(q) => (Some(q.as_str()), captures.iter().filter(|e| e.matches(q)).collect()),
                    _ => (None, Vec::new()),
                };
                finder::merge(&captures, &first, &listed.items, found_by, &feedback)
            }
        }
    }

    /// The world whose page shows when the list shown does not have it: the one kept, or the capture around the person
    /// (its page opened from the bar).
    fn picked(&self) -> Option<&Entry> {
        let selected = self.panel.selected?;
        self.picked.as_ref().filter(|e| e.id == selected).or(self.current.as_ref().filter(|e| e.id == selected))
    }

    /// Keeps the world whose page shows, whatever list shows next.
    fn keep_picked(&mut self, entries: &[Entry]) {
        match self.panel.selected {
            Some(id) => {
                if let Some(e) = entries.iter().find(|e| e.id == id) {
                    self.picked = Some(e.clone());
                }
            }
            None => self.picked = None,
        }
    }

    /// The list shown, as the panel tells it.
    fn list_status(&self) -> ListStatus {
        let shown = self.shown();
        let mut s = match &shown {
            Shown::Captures => ListStatus::default(),
            Shown::Visited => ListStatus { loading: self.visited.state().0, ..Default::default() },
            Shown::List(list) => {
                let read = self.finder.status(list);
                let waiting = read.pages == 0 && read.error.is_none() && self.askable(list);
                ListStatus { loading: read.loading || waiting, more: read.more, error: read.error, source: read.source, ..Default::default() }
            }
        };
        s.vrchat = self.vrchat;
        s.searching = matches!(shown, Shown::List(WorldList::Search(_)));
        s
    }

    /// Whether `list` can be asked for now: the lists only VRChat has need its session.
    fn askable(&self, list: &WorldList) -> bool {
        self.vrchat || !matches!(list, WorldList::Active | WorldList::Favorites)
    }

    /// The typing paused (or Done): what is searched; the list shown asked for, its next page once the results
    /// near its end. Done on the turn, its requests on a thread of their own ([`finder::Finder`]).
    fn find(&mut self) {
        let typed = self.panel.query.trim().to_string();
        if typed != self.typed.0 {
            self.typed = (typed.clone(), Instant::now());
        }
        if typed.is_empty() {
            self.searched.clear();
        } else if self.searched != typed && (self.search_now || self.typed.1.elapsed() >= TYPING_PAUSE) {
            self.searched = typed;
            self.panel.dirty = true;
        }
        self.search_now = false;
        if !self.open {
            return;
        }
        let shown = match self.shown() {
            Shown::Captures if self.captures().len() <= FEW_CAPTURES => Shown::List(WorldList::Popular),
            shown => shown,
        };
        match shown {
            Shown::Captures => {}
            Shown::Visited => self.visited.refresh(),
            Shown::List(list) => {
                if !self.askable(&list) {
                    return;
                }
                let read = self.finder.status(&list);
                let shown_to = (self.panel.results_page + 2) * panel::PER_PAGE;
                let page = if read.pages == 0 {
                    0
                } else if read.more && shown_to >= self.finder.count(&list) {
                    read.pages
                } else {
                    return;
                };
                let (backend, scope, ask) = (self.backend.clone(), self.scope.clone(), list.clone());
                self.finder.ask(&list, page, move || {
                    let (Some(backend), Some(scope)) = (backend, scope) else { return Err(tr("The app's service is not there.").into()) };
                    match backend.vrchat_data(scope, DataRequest::WorldList { list: ask, page })? {
                        DataResponse::Worlds(found) => Ok(found),
                        _ => Err(tr("The app's service answered something else.").into()),
                    }
                });
            }
        }
    }

    /// What the panel shows of the capture view.
    fn status(&self) -> Status {
        let mut status = self.own_status();
        // On a sheet in VR the 3D view is the overlay's own: what shows is what it says.
        // All of it: this panel has no view of its own there. It said it was on a screen, and so a capture pressed in a headset turned flat on the page, with no way to be around you.
        if let Some(view) = &self.relay {
            status = Status { fetching: status.fetching, listing_error: status.listing_error, list: status.list, error: view.error.clone().or(status.error), ..view.clone() };
        }
        status
    }

    /// What this one's 3D view is doing, for a panel elsewhere ([`Worlds::relay_to`]).
    pub fn view_status(&self) -> Status {
        self.own_status()
    }

    /// Makes this panel, on a sheet in VR, one that asks for what only the overlay's own 3D worlds can do, `view` being what those say. Returns what it asked since last time.
    pub fn relay_to(&mut self, view: Status) -> Vec<Relayed> {
        // Only when it says something new. Called every turn, this painted the whole page 45 times a second for nothing: 41 ms turns in a headset, with hover waiting behind them.
        if self.relay.as_ref() != Some(&view) {
            self.relay = Some(view);
            self.panel.dirty = true;
        }
        std::mem::take(&mut self.relayed)
    }

    /// The 3D view goes: from a button that is not on this panel.
    /// What says how to handle what shows small, until it has been used.
    fn update_hint(&mut self, overlay: &vr::Overlay, steamvr: &vr::Client, presenter: &mut Presenter, snap: &Snapshot) {
        let small = self.view.small_now().copied();
        self.hint_used |= small.is_some_and(|s| s.zoomed);
        // Across a menu it stays: the menu is drawn over it by sort order (Enzo's diagram: menu, 3D view, backdrop, game). It never fades for that, which made it vanish.
        if self.small_logged.due(std::time::Duration::from_secs(1)) && self.view.small() {
            super::vr_log::line(&self.view.report(overlay));
        }
        // The dark pane behind it in a panel's hole.
        let pane = small.filter(|s| s.docked).and(self.small_place).map(|(at, radius)| (at, radius * 2.0 / SMALL_FILLS_HOLE + 0.012));
        if pane.is_some() && self.backdrop.is_none() {
            self.backdrop = small_hint::Backdrop::new(overlay, &self.key).map_err(|e| eprintln!("{e:#}")).ok();
        }
        if let Some(backdrop) = self.backdrop.as_mut() {
            backdrop.update(overlay, presenter, pane);
        }
        let wanted = small.filter(|s| !self.hint_used && !s.docked).map(|s| {
            let stage = match s.held_by() {
                Some(hand) => small_hint::Stage::Stick { hand },
                None => small_hint::Stage::Grip { near: s.near },
            };
            (stage, s.anchor, s.radius())
        });
        if self.hint.is_none() {
            if wanted.is_none() {
                return;
            }
            match small_hint::SmallHint::new(overlay, &self.key) {
                Ok(hint) => self.hint = Some(hint),
                Err(e) => {
                    eprintln!("{e:#}");
                    return;
                }
            }
        }
        if let Some(hint) = self.hint.as_mut() {
            let head = view::head_pose(steamvr, 0.0);
            let eyes = [vr::compositor::Eye::Left, vr::compositor::Eye::Right].map(|eye| stereo::position(&stereo::compose(&head, &steamvr.eye_to_head(eye))));
            hint.update(overlay, presenter, &head, eyes, wanted, !snap.settings.reduce_motion);
        }
    }

    /// Enzo: with the menu off, an X floating above it, for a laser or a hand, with no menu to reopen. With the menu up, the menu has its own Hide.
    fn update_hide_chip(&mut self, overlay: &vr::Overlay, steamvr: &vr::Client, presenter: &mut Presenter) {
        let small = self.view.small_now().copied().filter(|s| !s.docked && self.menu_boxes.is_empty());
        let eye = stereo::position(&view::head_pose(steamvr, 0.0));
        let wanted = small.map(|s| {
            let place = small_hint::chip_place(s.anchor, s.radius(), eye, !self.hint_used);
            // The hand holding it does not press what lies under it.
            let holding = s.held_by();
            let pointed = (0..2).any(|h| Some(h) != holding && self.small_rays[h].is_some_and(|(tip, along)| small_hint::on_chip(&place, tip, along)));
            let pressed = (0..2).any(|h| Some(h) != holding && self.small_triggers[h] && !self.small_triggers_were[h] && self.small_rays[h].is_some_and(|(tip, along)| small_hint::on_chip(&place, tip, along)));
            (place, pointed, pressed)
        });
        self.small_triggers_were = self.small_triggers;
        if wanted.is_some() && self.hide_chip.is_none() {
            self.hide_chip = small_hint::HideChip::new(overlay, &self.key).map_err(|e| eprintln!("{e:#}")).ok();
        }
        if let Some(chip) = self.hide_chip.as_mut() {
            chip.update(overlay, presenter, wanted.map(|(place, pointed, _)| (place, pointed)));
        }
        if wanted.is_some_and(|(_, _, pressed)| pressed) {
            self.deferred.push(Action::Hide);
        }
    }

    /// The menus up now, for what must not show over them: none with no menu.
    pub fn menus_up(&mut self, boxes: Vec<stereo::MenuBox>) {
        self.menu_boxes = boxes;
    }

    /// The tools of the hands for a view's Hide: where each is, which way it points, and its trigger.
    pub fn small_tools(&mut self, hands: [Option<vr::Matrix34>; 2], triggers: [bool; 2]) {
        self.small_rays = hands.map(|h| h.map(|m| (stereo::position(&m), stereo::scale(stereo::axis(&m, 2), -1.0))));
        self.small_triggers = triggers;
    }

    /// Where in the panel what shows small lies, when it may.
    pub fn small_place_on_panel(&self) -> Option<egui::Rect> {
        self.panel.small_place(&self.status())
    }

    /// The hole in the panel it shows through, when it lies there now.
    pub fn small_hole(&self) -> Option<egui::Rect> {
        self.panel.hole(&self.status())
    }

    /// Where a panel has a place for what shows small, and how large it is.
    pub fn small_place(&mut self, place: Option<(vr::Matrix34, f32)>) {
        self.small_place = place;
        self.view.small_place(place);
    }

    pub fn view_small(&mut self, on: bool) {
        if self.relay.is_some() {
            self.relayed.push(Relayed::View(Action::Small(on)));
        }
    }

    /// Whether what shows can be taken in a hand.
    pub fn small_showing(&self) -> bool {
        self.view.small()
    }

    /// VRChat's turning stick, for what shows around the person.
    pub fn turn_stick(&mut self, stick: f32, steamvr: &vr::Client) -> f32 {
        self.view.turn_stick(stick, steamvr)
    }

    /// The hands, for what shows small.
    pub fn small_input(&mut self, hands: [Option<vr::Matrix34>; 2], grips: [bool; 2], sticks: [f32; 2], pads: [[f32; 2]; 2]) {
        self.view.small_input(hands, grips, sticks, pads);
    }

    pub fn hide_view(&mut self) {
        if self.relay.is_some() {
            self.relayed.push(Relayed::View(Action::Hide));
        }
    }

    /// What a panel on a sheet asked: as if pressed on this one.
    pub fn from_elsewhere(&mut self, asked: Vec<Relayed>) {
        for ask in asked {
            match ask {
                Relayed::Show(entry) => {
                    let id = entry.id;
                    self.picked = Some(entry);
                    self.deferred.push(Action::Show(id));
                }
                Relayed::ShowLocal { dir, name, changed } => {
                    let entry = Entry::local(dir, name, changed);
                    let id = entry.id;
                    self.picked = Some(entry);
                    self.deferred.push(Action::Show(id));
                }
                Relayed::View(action) => self.deferred.push(action),
            }
        }
    }

    fn own_status(&self) -> Status {
        let (fetching, listing_error) = self.catalog.state();
        let mut s = Status {
            current: self.current.as_ref().map(|e| e.id),
            quality: self.view.quality(),
            see_through: self.view.see_through,
            small: self.view.small_on(),
            small_taken: self.view.small_now().is_some_and(|s| s.taken),
            small_held: self.view.small_now().is_some_and(|s| s.held_by().is_some()),
            small_docked: self.view.small_now().is_some_and(|s| s.docked),
            small_near: self.view.small_now().is_some_and(|s| s.near),
            headset: !self.alone,
            error: self.error.clone().or_else(|| self.view.error.clone()),
            fetching,
            listing_error,
            list: self.list_status(),
            ..Default::default()
        };
        if let Some(d) = &self.download {
            let (read, total) = (d.progress.read.load(Ordering::Relaxed), d.progress.total.load(Ordering::Relaxed));
            if d.progress.stage.load(Ordering::Relaxed) == catalog::STAGE_PREPARING {
                s.stage = Some(tr("Preparing the splats").into());
            } else {
                s.download = Some(d.progress.fraction());
                s.stage = Some(if total > 0 {
                    tr!("Downloading {read} of {total} MB", read = format!("{:.1}", read as f64 / 1e6), total = format!("{:.1}", total as f64 / 1e6))
                } else {
                    tr("Downloading").into()
                });
            }
        } else if self.current.is_some() && self.view.active() {
            if self.view.preparing() {
                // Its device and shaders stay for the next: after the first, a capture only has to get there.
                s.stage = Some(if self.view.first_time() { tr("Getting your graphics card ready") } else { tr("Opening it") }.into());
            } else {
                s.shown = true;
            }
        }
        if s.shown {
            let splats = self.current.as_ref().map_or(0, |e| e.splats as u64);
            let (count, pixels) = (panel::short_count(splats), self.view.quality().pixels());
            s.stats = Some(if self.view.batch_ms > 0.0 {
                tr!("{count} splats · {ms} ms a batch · {pixels} px", count = count, ms = format!("{:.1}", self.view.batch_ms), pixels = pixels)
            } else {
                tr!("{count} splats · {pixels} px", count = count, pixels = pixels)
            });
        }
        s
    }

    /// Whether the computer's keyboard should type in the panel.
    pub fn typing(&self) -> bool {
        self.open && self.panel.typing()
    }

    /// Opens the panel in the menu's place.
    pub fn open_panel(&mut self, layout: KeyboardLayout) {
        self.catalog.refresh(&self.source);
        self.open = true;
        self.panel.layout = layout;
        let status = self.status();
        let time = self.time();
        self.panel.open(&status, time);
    }

    /// Whether what moves on it moves, or is simply there.
    pub fn set_moves(&mut self, moves: bool) {
        self.panel.moves = moves;
    }

    /// Back to it on a sheet: as it was left. Only its list is read again.
    pub fn come_back(&mut self, layout: KeyboardLayout) {
        self.catalog.refresh(&self.source);
        self.open = true;
        self.panel.layout = layout;
        self.panel.dirty = true;
    }

    /// The head's Back, if there is somewhere to go within this panel. Its own does not show on a sheet.
    pub fn back_within(&mut self) -> bool {
        if !self.panel.goes_back_within() {
            return false;
        }
        let entries = self.entries();
        let status = self.status();
        let picked = self.picked().cloned();
        let time = self.time();
        self.press_target(panel::Target::Back, &entries, &status, picked.as_ref(), time);
        true
    }

    /// The stick on its list.
    pub fn scroll_by(&mut self, by: f32) {
        self.panel.scroll_by(by);
    }

    /// One turn: what the panel asked of the app, the app's feedback, the capture view and its download, the pill or
    /// the bar under the menu (`menu_at`: the menu's place while it shows its front, the panel open or not). Returns
    /// whether the pill or the bar asked for the panel (the host opens it then) and whether a press happened (for the
    /// click sound).
    pub(super) fn update(
        &mut self,
        overlay: &vr::Overlay,
        steamvr: &vr::Client,
        presenter: &mut Presenter,
        snap: &Snapshot,
        backend: &Ctl,
        menu_at: Option<&Matrix34>,
    ) -> (bool, bool) {
        // The app's feedback replaces the panel's own (its presses show at once, before the app has them).
        if snap.n4n0.feedback != self.feedback_seen {
            self.feedback_seen = snap.n4n0.feedback.clone();
            self.feedback = snap.n4n0.feedback.clone();
            self.panel.dirty = true;
        }
        let me = snap.vrchat_login.user.as_ref().map(|u| u.display_name.clone()).filter(|n| !n.trim().is_empty());
        if me != self.me {
            self.me = me;
            self.panel.dirty = true;
        }
        // VRChat's requests go through the service, for this account and session: another one, other answers.
        if self.backend.is_none() {
            self.backend = Some(backend.clone());
        }
        let scope = Scope::of(snap);
        if self.scope.as_ref() != Some(&scope) {
            if self.scope.is_some() {
                self.finder.clear();
            }
            self.scope = Some(scope);
            self.vrchat = snap.vrchat_login.allowed && snap.vrchat_login.user.is_some();
            self.panel.dirty = true;
        }
        self.region = finder::region(snap.vrchat.world.instance.as_deref());
        while let Ok(said) = self.said.1.try_recv() {
            let time = self.time();
            self.panel.say(said, time);
            self.panel.dirty = true;
        }
        let deferred = std::mem::take(&mut self.deferred);
        if !deferred.is_empty() {
            let entries = self.entries();
            self.act(deferred, &entries, overlay, steamvr);
        }
        self.find();
        let seen = (self.finder.version(), self.visited.state().1);
        if seen != self.found_seen {
            self.found_seen = seen;
            self.panel.dirty = true;
        }
        self.finish_download(overlay, steamvr);
        self.view.update(overlay, steamvr);
        self.update_hint(overlay, steamvr, presenter, snap);
        self.update_hide_chip(overlay, steamvr, presenter);
        if self.current.is_some() && self.download.is_none() && !self.view.active() {
            // Hidden (or failed): nothing is current any more.
            self.current = None;
            self.panel.dirty = true;
        }
        self.ask_feedback();
        let (mut opened, mut clicked) = self.update_chip(overlay, presenter, menu_at);
        let (bar_opened, bar_clicked) = self.update_bar(overlay, steamvr, presenter, menu_at);
        opened |= bar_opened;
        clicked |= bar_clicked;
        for ask in self.outbox.drain(..) {
            backend.n4n0(N4n0Command::Feedback { ask });
        }
        (opened, clicked)
    }

    /// The pill under the menu, while the menu shows (and neither the panel nor a capture does).
    fn update_chip(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, menu_at: Option<&Matrix34>) -> (bool, bool) {
        let show = menu_at.is_some() && !self.open && self.current.is_none();
        let (mut opened, mut clicked) = (false, false);
        while let Some(e) = overlay.poll_event(self.chip) {
            if !show {
                continue;
            }
            match e.event_type {
                vr::event::MOUSE_MOVE if !self.chip_hover => self.chip_hover = true,
                vr::event::MOUSE_BUTTON_DOWN => {
                    self.chip_pressed = true;
                    clicked = true;
                }
                vr::event::MOUSE_BUTTON_UP => {
                    if self.chip_pressed {
                        opened = true;
                    }
                    self.chip_pressed = false;
                }
                vr::event::FOCUS_LEAVE => {
                    self.chip_hover = false;
                    self.chip_pressed = false;
                }
                _ => {}
            }
        }
        if !show {
            if self.chip_shown {
                let _ = overlay.hide(self.chip);
                self.chip_shown = false;
                self.chip_hover = false;
                self.chip_pressed = false;
            }
            return (false, clicked);
        }
        let at = under_menu(menu_at.expect("checked above"), CHIP_WIDTH_M * CHIP_PX[1] as f32 / CHIP_PX[0] as f32);
        if self.chip_placed != Some(at) {
            let _ = overlay.set_transform_absolute(self.chip, &at);
            self.chip_placed = Some(at);
        }
        let look = (self.chip_hover, self.chip_pressed);
        if self.chip_drawn != Some(look) || !self.chip_shown {
            if !self.chip_shown {
                self.chip_canvas.invalidate();
            }
            self.chip_drawn = Some(look);
            let time = self.time();
            let (hover, pressed) = look;
            self.chip_canvas.paint(time, |p| panel::draw_chip(p, &Status::default(), None, hover, pressed, time));
            if let Some(changed) = self.chip_canvas.damage() {
                presenter.show_part(overlay, self.chip, self.chip_canvas.pixels(), CHIP_PX[0], CHIP_PX[1], changed);
            }
        }
        if !self.chip_shown {
            let _ = overlay.show(self.chip);
            self.chip_shown = true;
        }
        (opened, clicked)
    }

    /// The capture's bar under the menu or the panel, while the menu shows and a capture shows or loads.
    fn update_bar(&mut self, overlay: &vr::Overlay, steamvr: &vr::Client, presenter: &mut Presenter, menu_at: Option<&Matrix34>) -> (bool, bool) {
        let show = menu_at.is_some() && self.current.is_some();
        let mut clicked = false;
        while let Some(e) = overlay.poll_event(self.bar) {
            if !show {
                continue;
            }
            match e.event_type {
                vr::event::MOUSE_MOVE => {
                    let (x, y, _) = e.mouse();
                    self.bar_hover = panel::bar_hit(x, BAR_PX[1] as f32 - y);
                }
                vr::event::MOUSE_BUTTON_DOWN => {
                    let (x, y, _) = e.mouse();
                    if let Some(target) = panel::bar_hit(x, BAR_PX[1] as f32 - y) {
                        // The press shows at once; what it does follows.
                        self.bar_pressed = Some(target);
                        clicked = true;
                        let actions = panel::bar_press(target, &self.status());
                        let entries = self.entries();
                        self.act(actions, &entries, overlay, steamvr);
                    }
                }
                vr::event::MOUSE_BUTTON_UP => self.bar_pressed = None,
                vr::event::FOCUS_LEAVE => {
                    self.bar_hover = None;
                    self.bar_pressed = None;
                }
                _ => {}
            }
        }
        let opened = std::mem::take(&mut self.open_asked);
        if !show || self.current.is_none() {
            if self.bar_shown {
                let _ = overlay.hide(self.bar);
                self.bar_shown = false;
                self.bar_hover = None;
                self.bar_pressed = None;
            }
            return (opened, clicked);
        }
        let at = under_menu(menu_at.expect("checked above"), BAR_WIDTH_M * BAR_PX[1] as f32 / BAR_PX[0] as f32);
        if self.bar_placed != Some(at) {
            let _ = overlay.set_transform_absolute(self.bar, &at);
            self.bar_placed = Some(at);
        }
        let status = self.status();
        let loading = !status.shown && status.error.is_none();
        let look: BarLook = (
            self.bar_hover,
            self.bar_pressed,
            status.current,
            status.shown,
            status.see_through,
            status.quality,
            status.download.map(|f| (f * 1000.0) as u16),
            status.stage.clone(),
            status.error.clone(),
        );
        let turning = loading && self.bar_painted.is_none_or(|t| t.elapsed() >= BAR_EVERY);
        if self.bar_drawn.as_ref() != Some(&look) || turning || !self.bar_shown {
            if !self.bar_shown {
                self.bar_canvas.invalidate();
            }
            self.bar_drawn = Some(look);
            self.bar_painted = Some(Instant::now());
            let time = self.time();
            let name = self.current.as_ref().map(|e| e.name.clone());
            let (hover, pressed) = (self.bar_hover, self.bar_pressed);
            self.bar_canvas.paint(time, |p| panel::draw_bar(p, &status, name.as_deref(), hover, pressed, time));
            if let Some(changed) = self.bar_canvas.damage() {
                presenter.show_part(overlay, self.bar, self.bar_canvas.pixels(), BAR_PX[0], BAR_PX[1], changed);
            }
        }
        if !self.bar_shown {
            let _ = overlay.show(self.bar);
            self.bar_shown = true;
        }
        (opened, clicked)
    }

    /// The selected capture's world feedback, asked for when one of its pages opens and again now and then while it
    /// shows (the app reads it from Orbiters and keeps it fresh for a while).
    fn ask_feedback(&mut self) {
        let showing = matches!(self.panel.page, panel::Page::Detail | panel::Page::Labels | panel::Page::Comments | panel::Page::Compose(_));
        let Some(id) = self.panel.selected.filter(|_| self.open && showing) else { return };
        if self.asked.is_some_and(|(asked, at)| asked == id && at.elapsed() < ASK_AGAIN) {
            return;
        }
        self.asked = Some((id, Instant::now()));
        let world = self.entries().into_iter().find(|e| e.id == id).or_else(|| self.picked().cloned()).map(|e| e.world_id);
        if let Some(world) = world.filter(|w| orbiters_contracts::world_feedback::is_world_id(w)) {
            self.outbox.push(FeedbackAsk::Show { world: panel::world_key(&world) });
        }
    }

    /// The download's answer: the capture goes to the view, and is kept for showing again.
    fn finish_download(&mut self, overlay: &vr::Overlay, steamvr: &vr::Client) {
        let Some(d) = &self.download else { return };
        let answer = match d.answer.try_recv() {
            Ok(a) => a,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(anyhow::anyhow!("{}", tr("the download stopped"))),
        };
        let (id, key) = (d.id, d.key.clone());
        self.download = None;
        self.panel.dirty = true;
        let current = self.current.as_ref().map(|e| e.id) == Some(id);
        match answer {
            Ok(capture) => {
                let capture = Arc::new(capture);
                self.captures.put(key.clone(), capture.clone(), current.then_some(&key));
                if current {
                    self.view.show(overlay, steamvr, capture);
                }
            }
            Err(e) if current => {
                eprintln!("3D worlds: {e:#}");
                self.error = Some(short_error(&e));
            }
            Err(_) => {}
        }
    }

    /// A laser event on the menu's overlay while the panel shows. Returns whether it was a press.
    pub fn mouse(&mut self, e: &vr::Event, overlay: &vr::Overlay, steamvr: &vr::Client) -> bool {
        let entries = self.entries();
        let status = self.status();
        let time = self.time();
        let height = panel::HEIGHT;
        let (no_thumb, no_picture) = (|_: &Entry| -> Option<TextureId> { None }, |_: u64, _: bool| -> Option<TextureId> { None });
        let picked = self.picked().cloned();
        let data = |feedback, facts| Data { entries: &entries, picked: picked.as_ref(), status: &status, feedback, facts, thumb: &no_thumb, picture: &no_picture, now: unix_now() };
        match e.event_type {
            vr::event::MOUSE_MOVE => {
                let (x, y, _) = e.mouse();
                let hover = self.panel.hit(&data(&self.feedback, &self.facts), x, height - y);
                self.panel.vr_drag(height - y, time);
                let hover = if self.panel.vr_dragging() { None } else { hover };
                if hover != self.panel.hover {
                    self.panel.hover = hover;
                    self.panel.dirty = true;
                }
                false
            }
            vr::event::MOUSE_BUTTON_DOWN => {
                let (x, y, _) = e.mouse();
                let d = data(&self.feedback, &self.facts);
                let hit = self.panel.hit(&d, x, height - y);
                if self.panel.vr_list_on(&status) && self.panel.vr_list_holds(&status, x, height - y) && matches!(hit, None | Some(panel::Target::Card(_))) {
                    self.panel.pressed = hit;
                    self.panel.vr_press(height - y, hit, time);
                    return true;
                }
                let Some(target) = hit else { return false };
                // The press shows at once; what it does follows.
                self.panel.pressed = Some(target);
                let actions = self.panel.press(target, &d, time);
                self.keep_picked(&entries);
                self.act(actions, &entries, overlay, steamvr);
                true
            }
            vr::event::MOUSE_BUTTON_UP | vr::event::FOCUS_LEAVE => {
                if let Some(target) = self.panel.vr_release(time).filter(|_| e.event_type == vr::event::MOUSE_BUTTON_UP) {
                    let d = data(&self.feedback, &self.facts);
                    let actions = self.panel.press(target, &d, time);
                    self.keep_picked(&entries);
                    self.act(actions, &entries, overlay, steamvr);
                }
                self.panel.pressed = None;
                if e.event_type == vr::event::FOCUS_LEAVE {
                    self.panel.hover = None;
                }
                self.panel.dirty = true;
                false
            }
            _ => false,
        }
    }

    /// The computer's keys typed while the panel types (numbered: only the new ones count). What they ask is done on
    /// the next turn.
    pub fn keys(&mut self, keys: &[(u32, orbiters_n4n0_core::menu::KeyInput)]) {
        let newest = keys.last().map_or(0, |(n, _)| *n);
        let Some(seen) = self.keys_seen.replace(newest) else { return };
        if !self.open || !keys.iter().any(|(n, _)| n.wrapping_sub(seen) as i32 > 0) {
            return;
        }
        let entries = self.entries();
        let status = self.status();
        let time = self.time();
        let (no_thumb, no_picture) = (|_: &Entry| -> Option<TextureId> { None }, |_: u64, _: bool| -> Option<TextureId> { None });
        let picked = self.picked().cloned();
        for (n, key) in keys {
            if n.wrapping_sub(seen) as i32 > 0 {
                let d = Data {
                    entries: &entries,
                    picked: picked.as_ref(),
                    status: &status,
                    feedback: &self.feedback,
                    facts: &self.facts,
                    thumb: &no_thumb,
                    picture: &no_picture,
                    now: unix_now(),
                };
                let actions = self.panel.key(key, &d, time);
                self.deferred.extend(actions);
            }
        }
        if self.deferred.iter().any(|a| matches!(a, Action::Leave)) {
            self.open = false;
        }
    }

    fn act(&mut self, actions: Vec<Action>, entries: &[Entry], overlay: &vr::Overlay, steamvr: &vr::Client) {
        for action in actions {
            match action {
                Action::Show(id) => {
                    let Some(entry) = entries.iter().find(|e| e.id == id).or(self.picked.as_ref().filter(|e| e.id == id)).filter(|e| e.capture).cloned() else {
                        continue;
                    };
                    // Enzo: nothing showing, and one opened from the start: its hint again, even if an earlier one was zoomed by accident.
                    if !self.view.active() {
                        self.hint_used = false;
                    }
                    self.error = None;
                    self.view.error = None;
                    let key = cache::Key::of(&entry);
                    if let Some(capture) = self.captures.get(&key) {
                        // Shown lately: at once, no download.
                        self.download = None;
                        self.current = Some(entry);
                        self.view.show(overlay, steamvr, capture);
                        continue;
                    }
                    // The render device and its shaders get ready while the capture downloads.
                    self.view.warm_up(steamvr);
                    let progress = Arc::new(Progress::default());
                    let (tx, rx) = mpsc::channel();
                    let (source, p, e) = (self.source.clone(), progress.clone(), entry.clone());
                    let _ = std::thread::Builder::new().name("3d-worlds-download".into()).spawn(move || {
                        let _ = tx.send(match &e.local {
                            Some(dir) => catalog::read_local(dir, &p),
                            None => source.download(&e, &p),
                        });
                    });
                    self.download = Some(Download { id, key, progress, answer: rx });
                    self.current = Some(entry);
                }
                Action::Hide => {
                    self.download = None;
                    self.error = None;
                    self.view.hide();
                    if !self.view.active() {
                        self.current = None;
                    }
                }
                Action::Recenter => self.view.recenter(steamvr),
                Action::SeeThrough(on) => self.view.see_through = on,
                Action::Small(on) => self.view.set_small(on, overlay, steamvr),
                Action::SmallBack => self.view.small_back(),
                Action::Quality(q) => self.view.set_quality(q),
                Action::Leave => self.open = false,
                Action::Refresh => {
                    self.catalog.refresh(&self.source);
                    self.finder.retry();
                }
                Action::Search => self.search_now = true,
                Action::Join(world) => self.join(world),
                Action::OpenWeb(world) => {
                    let Some(link) = finder::web_link(&world) else { continue };
                    let tx = self.said.0.clone();
                    let _ = std::thread::Builder::new().name("3d-worlds-web".into()).spawn(move || {
                        let said = match crate::controller::open_url(&link) {
                            Ok(()) => tr("Its page opened in the computer's browser").to_string(),
                            Err(e) => tr!("The browser did not open: {e}", e = e),
                        };
                        let _ = tx.send(said);
                    });
                }
                Action::Feedback(ask) => {
                    self.apply(&ask);
                    self.outbox.push(ask);
                }
                Action::Open => {
                    if self.open {
                        let (status, time) = (self.status(), self.time());
                        self.panel.open(&status, time);
                    } else {
                        self.open_asked = true;
                    }
                }
            }
        }
        self.panel.dirty = true;
    }

    /// Joins `world` in a new public instance in the region the player is in: an invite to oneself, which VRChat shows
    /// in the game, when signed in to VRChat in the app; else VRChat's launcher with the world (VRChat running, it may
    /// ask to switch). What happened is said on the world's page.
    fn join(&mut self, world: String) {
        let instance = finder::new_instance(self.region, uuid::Uuid::new_v4().as_u128() as u64);
        let tx = self.said.0.clone();
        let time = self.time();
        if self.vrchat {
            let (Some(backend), Some(scope)) = (self.backend.clone(), self.scope.clone()) else { return };
            self.panel.say(tr("Sending you an invite…"), time);
            let _ = std::thread::Builder::new().name("3d-worlds-invite".into()).spawn(move || {
                let said = match backend.vrchat_data(scope, DataRequest::InviteSelf { world, instance }) {
                    Ok(_) => tr("Invite sent: accept it in VRChat's notifications").to_string(),
                    Err(e) => tr!("The invite could not be sent: {e}", e = e),
                };
                let _ = tx.send(said);
            });
        } else {
            let Some(link) = finder::launch_link(&world, &instance) else { return };
            self.panel.say(tr("Opening VRChat's launcher…"), time);
            let _ = std::thread::Builder::new().name("3d-worlds-launch".into()).spawn(move || {
                let said = match finder::open_launch(&link) {
                    Ok(()) => tr("VRChat's launcher has it: sign in to VRChat in the app to be invited instead").to_string(),
                    Err(e) => format!("{e}"),
                };
                let _ = tx.send(said);
            });
        }
    }

    /// The panel's own ask, shown at once (the app's answer replaces it).
    fn apply(&mut self, ask: &FeedbackAsk) {
        match ask {
            FeedbackAsk::Show { .. } => {}
            FeedbackAsk::Like { world, on } => self.feedback.world_mut(world).set_like(*on),
            FeedbackAsk::Label { world, label, on, text } => {
                let w = self.feedback.world_mut(world);
                w.set_label(label, *on);
                if text.is_some() {
                    w.justification = text.clone();
                }
            }
            FeedbackAsk::Comment { world, text } => {
                let w = self.feedback.world_mut(world);
                w.comments.insert(0, CommentRow { id: 0, by: "You".into(), text: text.clone(), at: unix_now(), mine: true }); // i18n: not shown (data: the page writes a mine comment's author with tr)
                w.comment_count += 1;
            }
            FeedbackAsk::Uncomment { world, comment } => {
                let w = self.feedback.world_mut(world);
                if let Some(i) = w.comments.iter().position(|c| c.id == *comment) {
                    w.comments.remove(i);
                    w.comment_count = w.comment_count.saturating_sub(1);
                }
            }
        }
    }

    /// Whether the panel should be painted now (something changed or moves), at most 45 times a second.
    pub fn wants_paint(&self) -> bool {
        let since = self.last_paint.map(|t| t.elapsed());
        let due = since.is_none_or(|t| t >= Duration::from_millis(22));
        let facts = self.facts_pending && since.is_none_or(|t| t >= FACTS_EVERY);
        due && (self.panel.dirty || self.panel.animating(&self.status(), self.time()) || self.pictures.loading() || facts)
    }

    /// Paints the panel on the menu's canvas.
    pub fn paint(&mut self, canvas: &mut Canvas) {
        self.panel.vr_coast(self.time());
        self.panel.dirty = false;
        if let Some(frame) = self.live_frame.take() {
            let image = egui::ColorImage::from_rgba_premultiplied(frame.size, &frame.rgba);
            match &mut self.live_texture {
                Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
                None => self.live_texture = Some(canvas.ctx().load_texture("side-panel-view", image, egui::TextureOptions::LINEAR)),
            }
            if let Some(live) = self.panel.live.as_mut() {
                live.picture = self.live_texture.as_ref().map(|t| t.id());
                live.plate = frame.plate;
            }
        }
        self.last_paint = Some(Instant::now());
        let entries = self.entries();
        let status = self.status();
        // The worlds' pages the panel shows (the captures' all, to order by frame rate; a list's worlds come with theirs).
        self.facts_pending = false;
        if self.panel.sort == panel::Sort::Smooth {
            for e in entries.iter().filter(|e| e.capture) {
                self.facts_for(&e.world_id);
            }
        }
        let visible = self.visible(&entries, &status);
        for v in &visible {
            if v.page {
                self.facts_for(&v.world);
            }
        }
        let thumbs = self.thumbnails(&visible);
        let pictures = self.world_pictures(&entries);
        let time = self.time();
        let thumb = |e: &Entry| -> Option<TextureId> { thumbs.get(&e.id).copied() };
        let picture = |id: u64, large: bool| -> Option<TextureId> { pictures.get(&(id, large)).copied() };
        let d = Data {
            entries: &entries,
            picked: self.picked(),
            status: &status,
            feedback: &self.feedback,
            facts: &self.facts,
            thumb: &thumb,
            picture: &picture,
            now: unix_now(),
        };
        let panel = &self.panel;
        canvas.paint(time, |p| panel.draw(p, &d, time));
    }

    /// A world's page on the Map explorer, once fetched (asked for the first time).
    fn facts_for(&mut self, world_id: &str) {
        if self.facts.contains_key(world_id) {
            return;
        }
        match self.infos.get(world_id) {
            Some(crate::worlds::Info::Listed(page)) => {
                self.facts.insert(world_id.to_string(), Facts::of(&page));
            }
            // Not on the Map explorer, or Orbiters could not be reached: nothing more to show of it.
            Some(_) => {
                self.facts.insert(world_id.to_string(), Facts::default());
            }
            None => self.facts_pending = true,
        }
    }

    /// The worlds whose pictures the panel shows now.
    fn visible(&self, entries: &[Entry], status: &Status) -> Vec<Visible> {
        let (no_thumb, no_picture) = (|_: &Entry| -> Option<TextureId> { None }, |_: u64, _: bool| -> Option<TextureId> { None });
        let d = Data { entries, picked: self.picked(), status, feedback: &self.feedback, facts: &self.facts, thumb: &no_thumb, picture: &no_picture, now: 0 };
        let (shown, cards): (Vec<&Entry>, bool) = match self.panel.page {
            panel::Page::Typing => (self.panel.results(&d).into_iter().take(panel::PER_ROW).collect(), true),
            panel::Page::Browse => {
                let results = self.panel.results(&d);
                let window = self.panel.list_window(status.headset, results.len());
                (results[window].to_vec(), true)
            }
            panel::Page::Filter => (Vec::new(), true),
            // The list beside the side panel, and the world on it.
            panel::Page::Detail => {
                let results = self.panel.results(&d);
                let window = self.panel.list_window(status.headset, results.len());
                let mut list: Vec<&Entry> = results[window].to_vec();
                if let Some(e) = self.panel.entry(&d).filter(|e| !list.iter().any(|l| l.id == e.id)) {
                    list.push(e);
                }
                (list, true)
            }
            _ => (self.panel.entry(&d).into_iter().collect(), false),
        };
        // A card of a list's world has what its list said; its page, or a capture's card, reads the world's page on
        // the Map explorer (Orbiters reads VRChat for it: asked only as needed).
        shown
            .into_iter()
            .map(|e| Visible { id: e.id, world: e.world_id.clone(), thumbnail: e.thumbnail.clone(), page: !cards || e.capture || e.thumbnail.is_none() || (self.panel.page == panel::Page::Detail && self.panel.selected == Some(e.id)) })
            .collect()
    }

    /// The thumbnails of the worlds the panel shows now (their picture on VRChat, through the window's cache of world
    /// pages and pictures).
    fn thumbnails(&mut self, visible: &[Visible]) -> HashMap<u64, TextureId> {
        let mut out = HashMap::new();
        for v in visible {
            let Some(url) = v.thumbnail.clone().or_else(|| self.facts.get(&v.world).and_then(|f| f.thumbnail.clone())) else { continue };
            let picture = crate::worlds::Picture::Remote { world_id: v.world.clone(), url, large: false };
            if let Some(t) = self.pictures.get(&self.ctx, &picture) {
                out.insert(v.id, t.id());
            }
        }
        out
    }

    /// A world's thumbnail, from its id alone, once it is here: for what lists worlds outside this panel.
    pub fn world_thumbnail(&mut self, world_id: &str) -> Option<TextureId> {
        self.facts_for(world_id);
        let url = self.facts.get(world_id).and_then(|f| f.thumbnail.clone())?;
        let picture = crate::worlds::Picture::Remote { world_id: world_id.to_string(), url, large: false };
        self.pictures.get(&self.ctx, &picture).map(|t| t.id())
    }

    /// The selected capture's world's pictures on the pictures page: the page's thumbnails, the one shown whole.
    fn world_pictures(&mut self, entries: &[Entry]) -> HashMap<(u64, bool), TextureId> {
        let mut out = HashMap::new();
        if self.panel.page != panel::Page::Pictures {
            return out;
        }
        let Some(e) = self.panel.selected.and_then(|id| entries.iter().find(|e| e.id == id).or(self.picked())) else {
            return out;
        };
        let Some(f) = self.facts.get(&e.world_id) else { return out };
        let first = self.panel.pictures_page * panel::PICTURES_PER_PAGE;
        let mut wanted: Vec<(&orbiters_core::maps::WorldPicture, bool)> =
            f.pictures.iter().skip(first).take(panel::PICTURES_PER_PAGE).map(|w| (w, false)).collect();
        if let Some(w) = self.panel.picture.and_then(|i| f.pictures.get(i)) {
            wanted.push((w, false));
            wanted.push((w, true));
        }
        let asks: Vec<(u64, bool, crate::worlds::Picture)> = wanted
            .into_iter()
            .filter_map(|(w, large)| {
                let url = if large { w.url.clone() } else { w.thumbnail_url.clone().or_else(|| w.url.clone()) }?;
                // Kept on disk by its own name: a picture never changes.
                Some((w.id, large, crate::worlds::Picture::Remote { world_id: format!("{}-picture-{}", e.world_id, w.id), url: self.source.absolute(&url), large }))
            })
            .collect();
        for (id, large, picture) in asks {
            if let Some(t) = self.pictures.get(&self.ctx, &picture) {
                out.insert((id, large), t.id());
            }
        }
        out
    }
}

/// A world whose picture the panel shows: its list's thumbnail, and whether its page on the Map explorer is read.
struct Visible {
    id: u64,
    world: String,
    thumbnail: Option<String>,
    page: bool,
}

/// A download's error in a line the panel can show.
fn short_error(e: &anyhow::Error) -> String {
    let text = if e.chain().any(|cause| cause.is::<catalog::Replaced>()) {
        tr("This capture was replaced on Orbiters: go back and pick it again").to_string()
    } else if e.chain().filter_map(|cause| cause.downcast_ref::<catalog::Answered>()).any(|a| a.status == 404) {
        tr("This capture is no longer on Orbiters").to_string()
    } else {
        tr!("Could not download it: {text}", text = format!("{e:#}"))
    };
    text.chars().take(90).collect()
}
