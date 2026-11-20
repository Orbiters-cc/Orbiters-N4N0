//! 3D worlds in the play space: search VRChat's worlds from N4N0's menu overlay (every world: the member's own VRChat
//! session when signed in, else Orbiters' Map explorer; VRChat's lists, the worlds visited: [`finder`]), those members
//! captured and shared on Orbiters (the listing the VRChat world "N4N0 - 3D picture viewer" shows) badged 3D, and see
//! one of those around you in SteamVR, placed as that world places it (its ground on your floor, its start view where
//! you stand, facing your way) and drawn from the same files (docs/world-view-overlay.md). A world without a capture
//! has its page: Invite me when signed in to VRChat (an invite to oneself), and Open on VRChat.
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
pub mod floaters;
pub mod small_hint;
pub mod stick3d;

/// With this few captures, the list of them goes on with popular worlds.
const FEW_CAPTURES: usize = 3;

/// How much of a hole's width a capture lying in it fills: its top and foot are cut by the hole, as captures are wider than tall.
/// Nearer filling it (round 15's judges asked for most of the hole), still whole top to foot (request 60).
// Wider than the hole now that it lies a whole radius behind it and is seen from low: from the eyes it fills the hole side to side (round 18's judges: half of it).
pub const SMALL_FILLS_HOLE: f32 = 1.25;
mod diagnostics;
pub mod finder;
#[cfg(windows)]
pub mod interop;
pub mod panel;
pub mod opening;
pub mod pacing;
pub mod render;
pub mod shots;
pub mod stereo;
pub mod timing_lab;
pub mod view;

mod paint_snapshot;
pub(crate) use paint_snapshot::PaintSnapshot;

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
    /// The captures a flip of the carousel may show next, nearest first: downloaded ahead, so a flip shows the 3D view as soon as the GPU has it (K-carousel).
    Prepare(Vec<Entry>),
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

/// What the overlay's 3D worlds do with the shared 3D view (`Worlds::update_shared`).
#[derive(Default)]
struct SharedSide {
    /// The friend's view opened (its number), and whether it showed since.
    opened: Option<u64>,
    shown: bool,
    /// The friend's zoom and pan last applied.
    applied: Option<(f32, [f32; 2])>,
    /// The small mode before a friend's view made it small, and this player's own capture that showed then.
    small_was: Option<bool>,
    before: Option<Entry>,
    /// This player's view last sent: when, its zoom and pan; a stop sent.
    sent: Option<(Instant, f32, [f32; 2])>,
    stop_sent: bool,
    /// The share chip, the hand whose trigger was pulled on it, and the triggers last turn.
    chip: Option<small_hint::HideChip>,
    down: Option<usize>,
    triggers_were: [bool; 2],
}

/// What the bar shows: when it changes, it is painted again.
type BarLook = (Option<Target>, Option<Target>, Option<u64>, bool, bool, Quality, Option<u16>, Option<String>, Option<String>);

pub struct Worlds {
    paint_fixture: Option<paint_snapshot::PaintFixture>,
    snapshot_mode: bool,
    presented_paint: Option<PaintSnapshot>,
    pressed_paint: Option<PaintSnapshot>,
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
    /// The entry turning on the side panel, as it was when it started: checked again each turn ([`Worlds::revalidate`]).
    live_entry: Option<Entry>,
    /// What the side panel's capture is read with: told to stop when it is not wanted any more (verify-r15 B2).
    live_progress: Option<Arc<catalog::Progress>>,
    /// The Worlds page shows now in the desktop's menus: nothing turns on its side panel otherwise (verify-r15 B2).
    on_sheet: bool,
    live_frame: Option<super::orbit::sheet_view::Frame>,
    live_texture: Option<egui::TextureHandle>,
    live_pointer: Option<(f32, f32)>,
    live_turns: bool,
    chip: vr::OverlayHandle,
    chip_canvas: Canvas,
    chip_presented: crate::overlay::presented::Presented,
    chip_shown: bool,
    chip_hover: bool,
    chip_pressed: bool,
    chip_drawn: Option<(bool, bool)>,
    /// Where the pill was put (under the menu as it was then).
    chip_placed: Option<Matrix34>,
    bar: vr::OverlayHandle,
    bar_canvas: Canvas,
    bar_presented: crate::overlay::presented::Presented,
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
    /// The hole a capture shows through opened, closed or moved since the page was last painted: the page is painted at once, with the sheet's back that is
    /// painted in the same turn, never a throttle's turn later (dev.21's test caught the page still solid there while its back was clear).
    hole_changed: bool,
    /// On a sheet in VR: what the overlay's own 3D worlds say of their view, and what was asked of them.
    relay: Option<Status>,
    relayed: Vec<Relayed>,
    /// The player's VRChat name (a capture shared with some players only is listed for them).
    me: Option<String>,
    /// How many times the account changed: part of the page's paint key ([`Worlds::paint_key`]).
    accounts: u64,
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
    textures: crate::overlay::PaintTextures,
    epoch: Instant,
    keys_seen: Option<u32>,
    last_paint: Option<Instant>,
    /// With no SteamVR: nothing can show around or before the person.
    alone: bool,
    key: String,
    /// What says how to handle what shows small; it has been used; a new opening of the menu asks it back for the next capture.
    hint: Option<small_hint::SmallHint>,
    small_logged: super::vr_log::Every,
    eye_frames_kept: u32,
    hide_chip: Option<small_hint::HideChip>,
    /// The shared 3D view (`controller/shared_view.rs`): what this side does with it.
    shared: SharedSide,
    small_place: Option<(vr::Matrix34, f32)>,
    /// Where each hand's tool is and which way it points, and its trigger, now and a turn ago.
    small_rays: [Option<([f32; 3], [f32; 3])>; 2],
    small_triggers: [bool; 2],
    small_triggers_were: [bool; 2],
    hint_used: bool,
    /// The menus up now.
    menu_boxes: Vec<stereo::MenuBox>,
    /// A world's page opened from elsewhere (a previous instance's row, F10): what the panel showed before, which its Back
    /// brings back as it leaves for where the page was opened from.
    opened_from: Option<(panel::Page, Option<u64>)>,
    /// Go there (F5): the instances of the world whose page shows, and what its threads answered.
    go: panel::go::GoView,
    go_said: (mpsc::Sender<GoSaid>, mpsc::Receiver<GoSaid>),
    /// The carousel (K-carousel): the stick's flip held (which way, when it last flipped, how many times), the neighbours last asked to be prepared (for which world
    /// and list), when the last flip was and to what (for `vr-test.log`'s times).
    flip_latch: Option<(bool, Instant, u32)>,
    prepared_for: Option<(u64, usize, Vec<u64>)>,
    flipped_at: Option<(u64, Instant)>,
    /// What the carousel's next flips may show, downloaded one at a time while nothing else is, and the one downloading.
    prepare: Vec<Entry>,
    preparing: Option<Download>,
    /// A capture kept in memory, asked for while another lay in the panel's hole: shown once the panel has closed its hole over the old one ([`SWITCH_AFTER`]).
    switch: Option<(Arc<Capture>, Instant)>,
    /// When the capture now current was asked for, and whether it was ready in memory then (`vr-test.log`).
    asked_at: Option<(u64, Instant, bool)>,
}

/// How long a capture switched to while another lay in the panel's hole waits before the old one's faces go: long enough for the panel, painted at once without a
/// hole, to reach the headset first (never a frame with neither, the rule of dev.50's page switch).
const SWITCH_AFTER: Duration = Duration::from_millis(45);

/// The stick on the carousel: how far to one side flips, how far back lets it flip again, and how soon it flips again while held there.
const FLIP_AT: f32 = 0.6;
const FLIP_REARM: f32 = 0.3;
const FLIP_REPEAT_FIRST: Duration = Duration::from_millis(550);
const FLIP_REPEAT: Duration = Duration::from_millis(350);
/// How many of the carousel's next captures are downloaded ahead: the next two and the one before.
const PREPARED_AHEAD: usize = 3;

/// What Go there's threads answer, with the world and the account they were asked for.
enum GoSaid {
    Listed { world: String, scope: Scope, read: Result<orbiters_core::vrchat_api::WorldInstances, String> },
    /// The next batch of the list.
    More { world: String, scope: Scope, read: Result<orbiters_core::vrchat_api::WorldInstances, String> },
    /// On the way to this instance, and how; or why not (and whether the list changed under it).
    Went { world: String, scope: Scope, sent: Result<(orbiters_core::vrchat_api::WorldInstance, crate::controller::travel::Went), (String, bool)> },
}

/// Where Go there sends the person.
enum GoTarget {
    /// An instance of the list, by its exact id.
    Listed(String),
    New(orbiters_core::vrchat_api::CreateAccess, orbiters_core::vrchat_api::InstanceRegion),
    /// An invite to the instance just gone to, asked for instead (VRChat only showed its page).
    Invite(orbiters_core::vrchat_api::WorldInstance),
}

/// Why Go there could not, in words.
fn go_words(e: &crate::controller::vrchat_data::WorldError) -> String {
    use crate::controller::vrchat_data::WorldError;
    match e {
        WorldError::AccountChanged => tr("Your VRChat account changed: try again."),
        WorldError::SignInRequired => tr("Sign in to VRChat in Orbiters to go there."),
        WorldError::InvalidWorld => tr("VRChat does not know this world."),
        WorldError::NotListed => tr("That instance is no longer on the list: here it is again."),
        WorldError::Full => tr("That instance is full now."),
        WorldError::Closed => tr("That instance has closed."),
        WorldError::NotJoinable => tr("You cannot join that instance."),
        WorldError::BridgeBusy => tr("Orbiters' AI bridge is using VRChat with your account on this computer: go there once it is done."),
        WorldError::NotStarted(e) => return tr!("VRChat could not be started there: {error}", error = e),
        WorldError::Api(e) => return e.to_string(),
    }
    .to_string()
}

/// Whether VRChat's log's instance (`46657~hidden(usr_…)~region(eu)~nonce(…)`) is the one the API named (it may leave
/// out the `~nonce(…)` VRChat's client adds).
fn same_instance(logged: &str, named: &str) -> bool {
    logged == named || logged.strip_prefix(named).is_some_and(|rest| rest.starts_with("~nonce("))
}

/// Whether the list Go there shows is out of date after this answer.
fn go_list_changed(e: &crate::controller::vrchat_data::WorldError) -> bool {
    use crate::controller::vrchat_data::WorldError;
    matches!(e, WorldError::NotListed | WorldError::Full | WorldError::Closed | WorldError::NotJoinable)
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
        forget_kept_eye_frames();
        let chip = crate::overlay::slots::create(overlay, &format!("{key}.worlds-chip"), "N4N0's 3D worlds").map_err(|c| anyhow::anyhow!("3D worlds pill: SteamVR overlay error {c}"))?; // i18n: not shown (the overlay's name in SteamVR)
        interactive(overlay, chip, CHIP_WIDTH_M);
        let bar = crate::overlay::slots::create(overlay, &format!("{key}.worlds-bar"), "N4N0's 3D world controls").map_err(|c| { // i18n: not shown (a name for SteamVR, or the overlay process's own stderr)
            let _ = overlay.destroy(chip);
            anyhow::anyhow!("3D worlds bar: SteamVR overlay error {c}") // i18n: not shown (a name for SteamVR, or the overlay process's own stderr)
        })?; // i18n: not shown (the overlay's name in SteamVR)
        interactive(overlay, bar, BAR_WIDTH_M);
        Ok(Self {
            paint_fixture: None, snapshot_mode: false, presented_paint: None, pressed_paint: None,
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
            live_progress: None,
            live_entry: None,
            on_sheet: false,
            live_frame: None,
            live_texture: None,
            live_pointer: None,
            live_turns: true,
            chip,
            chip_canvas: Canvas::new(CHIP_PX[0], CHIP_PX[1]),
            chip_presented: Default::default(),
            chip_shown: false,
            chip_hover: false,
            chip_pressed: false,
            chip_drawn: None,
            chip_placed: None,
            bar,
            bar_canvas: Canvas::new(BAR_PX[0], BAR_PX[1]),
            bar_presented: Default::default(),
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
            hole_changed: false,
            relay: None,
            relayed: Vec::new(),
            me: None,
            accounts: 0,
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
            textures: crate::overlay::PaintTextures::of(&ctx),
            ctx,
            epoch: Instant::now(),
            keys_seen: None,
            last_paint: None,
            alone: false,
            key: key.to_string(),
            hint: None,
            hide_chip: None,
            shared: SharedSide::default(),
            small_rays: [None; 2],
            small_triggers: [false; 2],
            small_triggers_were: [false; 2],
            small_logged: Default::default(),
            eye_frames_kept: 0,
            small_place: None,
            hint_used: false,
            menu_boxes: Vec::new(),
            opened_from: None,
            go: Default::default(),
            go_said: mpsc::channel(),
            flip_latch: None,
            prepared_for: None,
            flipped_at: None,
            prepare: Vec::new(),
            preparing: None,
            switch: None,
            asked_at: None,
        })
    }

    /// What the left eye shows now, half size, into `vr-test-frames` in the logs' folder: what the compositor really shows at a panel's hole.
    /// Only when [`eye_frames_wanted`].
fn keep_an_eye_frame(steamvr: &vr::Client, n: u32) {
    let Ok(compositor) = steamvr.compositor() else { return };
    let Ok(reader) = orbiters_openvr::mirror::EyeReader::new(steamvr.dxgi_adapter_index()) else { return };
    match reader.read(&compositor, vr::compositor::Eye::Left) {
        Ok(picture) => {
            let dir = orbiters_core::paths::logs_dir().join("vr-test-frames");
            let _ = std::fs::create_dir_all(&dir);
            if let Some(image) = image::RgbaImage::from_raw(picture.width, picture.height, picture.rgba) {
                let half = image::imageops::resize(&image, picture.width / 2, picture.height / 2, image::imageops::FilterType::Triangle);
                let _ = half.save(dir.join(format!("small-in-its-hole-{n}.png")));
            }
        }
        Err(e) => super::vr_log::line(&format!("eye frame: {e}")), // i18n: not shown (a log's line)
    }
}

/// The panel alone, for the desktop's panels around VRChat's menus (`overlay::orbit`): no SteamVR, no pill, no bar, nothing shown around the person. Its lists, a world's page
    /// and its feedback work as ever; a capture opens in the Orbiters window instead ([`Worlds::mouse_alone`]).
    pub fn alone(ctx: egui::Context) -> Self {
        let textures = crate::overlay::PaintTextures::of(&ctx);
        Self::alone_with_textures(ctx, textures)
    }

    pub(crate) fn alone_with_textures(ctx: egui::Context, textures: crate::overlay::PaintTextures) -> Self {
        Self {
            paint_fixture: None, snapshot_mode: false, presented_paint: None, pressed_paint: None,
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
            live_progress: None,
            live_entry: None,
            on_sheet: false,
            live_frame: None,
            live_texture: None,
            live_pointer: None,
            live_turns: true,
            chip: 0,
            chip_canvas: Canvas::new(1, 1),
            chip_presented: Default::default(),
            chip_shown: false,
            chip_hover: false,
            chip_pressed: false,
            chip_drawn: None,
            chip_placed: None,
            bar: 0,
            bar_canvas: Canvas::new(1, 1),
            bar_presented: Default::default(),
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
            hole_changed: false,
            relay: None,
            relayed: Vec::new(),
            me: None,
            accounts: 0,
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
            textures,
            ctx,
            epoch: Instant::now(),
            keys_seen: None,
            last_paint: None,
            alone: true,
            key: String::new(),
            hint: None,
            hide_chip: None,
            shared: SharedSide::default(),
            small_rays: [None; 2],
            small_triggers: [false; 2],
            small_triggers_were: [false; 2],
            small_logged: Default::default(),
            eye_frames_kept: 0,
            small_place: None,
            hint_used: false,
            menu_boxes: Vec::new(),
            opened_from: None,
            go: Default::default(),
            go_said: mpsc::channel(),
            flip_latch: None,
            prepared_for: None,
            flipped_at: None,
            prepare: Vec::new(),
            preparing: None,
            switch: None,
            asked_at: None,
        }
    }

    /// The environment was switched (Settings, Development): the 3D worlds are listed and downloaded from the server
    /// chosen now (CORE2-13).
    fn follow_environment(&mut self) {
        if !self.source.is_current() {
            self.source = Source::current();
            self.catalog.refresh(&self.source);
            self.panel.dirty = true;
        }
    }

    /// A turn of [`Worlds::alone`]: what the panel asked of the app, the app's feedback, the lists.
    pub fn update_alone(&mut self, snap: &Snapshot, backend: &Ctl) {
        self.follow_environment();
        if snap.n4n0.feedback != self.feedback_seen {
            self.feedback_seen = snap.n4n0.feedback.clone();
            self.feedback = snap.n4n0.feedback.clone();
            self.panel.dirty = true;
        }
        if self.backend.is_none() {
            self.backend = Some(backend.clone());
        }
        self.follow_login(snap);
        self.revalidate();
        self.region = finder::region(snap.vrchat.world.instance.as_deref());
        self.go_arrived(&snap.vrchat.world);
        self.live_turns = !snap.settings.reduce_motion;
        // What turns on the side panel: its newest picture, or that it cannot.
        self.follow_screen_live();
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
        self.prepare_neighbours();
        self.find();
        let seen = (self.finder.version(), self.visited.state().1);
        if seen != self.found_seen {
            self.found_seen = seen;
            self.panel.dirty = true;
        }
        self.ask_feedback();
        self.follow_go();
        for ask in self.outbox.drain(..) {
            backend.n4n0(N4n0Command::Feedback { ask });
        }
    }

    /// On a sheet in VR, with a capture's side panel open: the overlay's own 3D worlds are asked to download the carousel's next captures ahead (K-carousel), the
    /// nearest first, each time the world shown or the list changes.
    fn prepare_neighbours(&mut self) {
        if self.relay.is_none() || self.panel.page != panel::Page::Detail {
            return;
        }
        let entries = self.entries();
        let status = self.status();
        let picked = self.picked().cloned();
        let (no_thumb, no_picture) = (|_: &Entry| -> Option<TextureId> { None }, |_: u64, _: bool| -> Option<TextureId> { None });
        let d = Data { entries: &entries, picked: picked.as_ref(), status: &status, feedback: &self.feedback, facts: &self.facts, thumb: &no_thumb, picture: &no_picture, now: 0 };
        let set = self.panel.carousel(&d);
        let Some(at) = self.panel.selected.and_then(|id| set.iter().position(|e| e.id == id)) else { return };
        // The way it was last flipped first: who flips forward goes on forward.
        let back = self.panel.flipped_back();
        let mut order: Vec<Option<&Entry>> = vec![set.get(at + 1).copied(), at.checked_sub(1).map(|i| set[i]), set.get(at + 2).copied(), at.checked_sub(2).map(|i| set[i])];
        if back {
            order.swap(0, 1);
            order.swap(2, 3);
        }
        let near: Vec<Entry> = order.into_iter().flatten().filter(|e| e.capture).take(PREPARED_AHEAD).cloned().collect();
        let key = (set[at].id, set.len(), near.iter().map(|e| e.id).collect::<Vec<_>>());
        if self.prepared_for.as_ref() != Some(&key) {
            self.prepared_for = Some(key);
            self.relayed.push(Relayed::Prepare(near));
        }
    }

    /// What turns on the side panel follows the page: nothing once it is not a world's page or the panel closed. On a screen a world with a 3D capture turns there as
    /// soon as it is chosen, where its picture is, with no button to press first (REQUEST15 item 6); one at a time, the one before going as another is chosen.
    fn follow_screen_live(&mut self) {
        if self.panel.page != panel::Page::Detail || !self.open || !self.on_sheet {
            self.stop_live();
        } else if self.on_screen() {
            self.live_chosen();
        }
    }

    /// This panel is on a screen (the desktop's overlay), not on a sheet in a headset: nothing shows around anyone, captures turn on the panel.
    fn on_screen(&self) -> bool {
        match &self.relay {
            Some(view) => !view.headset,
            None => self.alone,
        }
    }

    /// The world chosen turning on the side panel when it has a 3D capture; else nothing turns there, and its picture stays. Started without waiting: its capture
    /// is read on a thread of its own ([`super::orbit::sheet_view::SheetView::open_with`]), and its picture shows meanwhile.
    fn live_chosen(&mut self) {
        let Some(id) = self.panel.selected else { return };
        let entries = self.entries();
        let chosen = entries.iter().find(|e| e.id == id).or(self.picked()).filter(|e| e.capture && self.allowed(e)).cloned();
        match chosen {
            // The same one, the same upload, still allowed: it goes on turning. Another upload of it, or one no longer allowed, is not reused (blocker 1).
            Some(entry) if self.live_entry.as_ref().is_some_and(|l| l.id == entry.id && cache::Key::of(l) == cache::Key::of(&entry)) && self.live.is_some() => {}
            Some(entry) => self.start_live(&entry),
            None => self.stop_live(),
        }
    }

    /// Whether the Worlds page shows in the desktop's menus now (its sheet open, the menu up). Not shown: what turns on its side panel stops, its reading too.
    pub fn set_shown(&mut self, shown: bool) {
        self.on_sheet = shown;
        if !shown {
            self.stop_live();
        }
    }

    /// Starts `entry` turning on the side panel. It reads what is on this computer, or what was downloaded before, as any showing of it does. The one before
    /// is told to stop first: one reading at a time, the latest chosen.
    fn start_live(&mut self, entry: &Entry) {
        self.stop_live();
        self.live_entry = Some(entry.clone());
        let (source, from) = (self.source.clone(), entry.clone());
        let progress = Arc::new(catalog::Progress::default());
        self.live_progress = Some(progress.clone());
        let load = move || {
            one_load_at_a_time(&progress, || match &from.local {
                Some(dir) => catalog::read_kept(&from, dir, &progress),
                None => source.download(&from, &progress),
            })
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
        self.live_entry = None;
        if let Some(progress) = self.live_progress.take() {
            progress.stop.store(true, Ordering::Relaxed);
        }
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
                    if let Some(entry) = entries.iter().find(|e| e.id == id).or(self.picked.as_ref().filter(|e| e.id == id)).filter(|e| self.allowed(e)).cloned() {
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
                Action::Join(world) => self.go_load(world),
                Action::GoTo { world, instance } => self.go_there(world, GoTarget::Listed(instance)),
                Action::NewInstance { world, access, region } => self.go_there(world, GoTarget::New(access, region)),
                Action::InviteInstead { world, instance } => {
                    if let Some(gone) = self.go.went.as_ref().filter(|g| g.instance.id == instance) {
                        let target = GoTarget::Invite(gone.instance.clone());
                        self.go_there(world, target);
                    }
                }
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
                Action::SignIn => {
                    if let Some(backend) = &self.backend {
                        backend.sign_in();
                    }
                }
                // Asked again on the next turn.
                Action::RetryFeedback => self.asked = None,
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
        let hover = if self.snapshot_mode { self.presented_paint.as_ref().and_then(|shown| shown.hit(at)) }
            else { at.and_then(|(x, y)| self.panel.hit(&d, x, y)) };
        // A hand on what turns on the panel turns it.
        if let (Some(panel::Target::Hero), Some((_, view)), Some(from), Some(now)) = (self.panel.pressed, &self.live, self.live_pointer, at) {
            view.turn(now.0 - from.0, now.1 - from.1);
        }
        self.live_pointer = at;
        // Where the laser is says what the stick does there, in words under the picture: painted again when that changes.
        let spot = |w: &Self| w.panel.pointer.and_then(|p| w.stick_spot(p.x, p.y));
        let before = spot(self);
        self.panel.pointer = at.map(|(x, y)| egui::pos2(x, y));
        if spot(self) != before {
            self.panel.dirty = true;
        }
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
        if self.snapshot_mode {
            let Some(shown) = self.presented_paint.as_ref() else { return false };
            let Some(target) = self.panel.hover else { return false };
            if !shown.compatible(self, &entries, &status, picked.as_ref(), target) { return false; }
            self.pressed_paint = Some(shown.clone());
        }
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
        if self.snapshot_mode {
            let shown = self.pressed_paint.as_ref().or(self.presented_paint.as_ref());
            if !shown.is_some_and(|s| s.compatible(self, entries, status, picked, target)) { return; }
        }
        let (no_thumb, no_picture) = (|_: &Entry| -> Option<TextureId> { None }, |_: u64, _: bool| -> Option<TextureId> { None });
        self.panel.pressed = Some(target);
        let hole = self.small_hole();
        let selected = self.panel.selected;
        let actions = {
            let d = Data { entries, picked, status, feedback: &self.feedback, facts: &self.facts, thumb: &no_thumb, picture: &no_picture, now: unix_now() };
            self.panel.press(target, &d, time)
        };
        if self.panel.selected != selected {
            self.flipped(hole);
        }
        self.keep_picked(entries);
        if let (panel::Target::Hero, Some((_, view))) = (target, &self.live) {
            view.hold(true);
        }
        // On the whole page it is worth drawing larger.
        if let (Some((_, view)), Some(live)) = (&self.live, &self.panel.live) {
            view.set_largest(if live.full { 0 } else { 2 });
        }
        self.act_alone(actions, entries);
        self.follow_live(entries);
    }

    /// What a stick does with a laser at (x, y) on this panel: moves in the capture in its hole, flips the carousel, or nothing of ours ([`panel::StickSpot`]).
    pub fn stick_spot(&self, x: f32, y: f32) -> Option<panel::StickSpot> {
        if !self.open && self.relay.is_none() {
            return None;
        }
        let entries = self.entries();
        let status = self.status();
        let (no_thumb, no_picture) = (|_: &Entry| -> Option<TextureId> { None }, |_: u64, _: bool| -> Option<TextureId> { None });
        let picked = self.picked().cloned();
        let d = Data { entries: &entries, picked: picked.as_ref(), status: &status, feedback: &self.feedback, facts: &self.facts, thumb: &no_thumb, picture: &no_picture, now: 0 };
        self.panel.stick_spot(&d, x, y)
    }

    /// A stick taken on the carousel, side to side (-1 to 1): past [`FLIP_AT`] it flips once; held there, again after [`FLIP_REPEAT_FIRST`] and then every
    /// [`FLIP_REPEAT`], to go through many quickly; it flips again only once let back under [`FLIP_REARM`]. Whether it flipped.
    pub fn flip_stick(&mut self, x: f32) -> bool {
        let now = Instant::now();
        let way = if x >= FLIP_AT { Some(true) } else if x <= -FLIP_AT { Some(false) } else { None };
        match (way, self.flip_latch) {
            (None, _) => {
                if x.abs() < FLIP_REARM {
                    self.flip_latch = None;
                }
                return false;
            }
            (Some(forward), Some((held, at, times))) if forward == held => {
                let wait = if times == 1 { FLIP_REPEAT_FIRST } else { FLIP_REPEAT };
                if now.duration_since(at) < wait {
                    return false;
                }
                self.flip_latch = Some((forward, now, times + 1));
            }
            // A latch the other way holds until the stick comes back to its middle.
            (Some(_), Some(_)) => return false,
            (Some(forward), None) => self.flip_latch = Some((forward, now, 1)),
        }
        let forward = way == Some(true);
        let entries = self.entries();
        let status = self.status();
        let time = self.time();
        let picked = self.picked().cloned();
        let hole = self.small_hole();
        let actions = {
            let (no_thumb, no_picture) = (|_: &Entry| -> Option<TextureId> { None }, |_: u64, _: bool| -> Option<TextureId> { None });
            let d = Data { entries: &entries, picked: picked.as_ref(), status: &status, feedback: &self.feedback, facts: &self.facts, thumb: &no_thumb, picture: &no_picture, now: unix_now() };
            let before = self.panel.selected;
            let actions = self.panel.flip_by_stick(&d, forward, time);
            if self.panel.selected == before {
                return false;
            }
            actions
        };
        self.flipped(hole);
        self.keep_picked(&entries);
        self.act_alone(actions, &entries);
        true
    }

    /// The carousel flipped: its time noted, and the page painted at once if its hole closed over the capture leaving.
    fn flipped(&mut self, hole: Option<egui::Rect>) {
        if let Some(id) = self.panel.selected {
            self.flipped_at = Some((id, Instant::now()));
        }
        self.hole_changed |= self.small_hole() != hole;
    }

    #[cfg(test)]
    pub fn show_world_for_test(&mut self, id: u64) {
        self.panel.page = panel::Page::Detail;
        self.panel.selected = Some(id);
        // Its side panel already out.
        self.panel.moves = false;
        self.panel.dirty = true;
    }

    /// As [`Worlds::show_world_for_test`], its side panel on its way out now, as a press on a world brings it.
    #[cfg(test)]
    pub fn slide_world_in_for_test(&mut self, id: u64) {
        self.show_world_for_test(id);
        self.panel.moves = true;
        self.panel.side_at = self.time();
    }

    /// `entry`'s page, as a press on it in a list opens it.
    #[cfg(test)]
    pub fn pick_for_test(&mut self, entry: Entry) {
        self.show_world_for_test(entry.id);
        self.picked = Some(entry);
    }

    /// Where `target` is on the panel now, in its units: where a hand points to press it.
    #[cfg(test)]
    pub fn target_for_test(&self, target: panel::Target) -> Option<egui::Pos2> {
        if self.snapshot_mode { return self.presented_paint.as_ref().and_then(|p| p.target_for_test(target)); }
        let entries = self.entries();
        let status = self.status();
        let (no_thumb, no_picture) = (|_: &Entry| -> Option<TextureId> { None }, |_: u64, _: bool| -> Option<TextureId> { None });
        let picked = self.picked().cloned();
        let d = Data { entries: &entries, picked: picked.as_ref(), status: &status, feedback: &self.feedback, facts: &self.facts, thumb: &no_thumb, picture: &no_picture, now: unix_now() };
        self.panel.targets(&d).into_iter().find(|(t, _)| *t == target).map(|(_, r)| r.center())
    }

    /// The 3D captures Orbiters lists, as if it had just answered.
    #[cfg(test)]
    pub fn listing_for_test(&self, entries: Vec<Entry>) {
        self.catalog.put_for_test(entries);
    }

    #[cfg(test)]
    pub fn draft_for_test(&self) -> &str { &self.panel.draft }

    /// A test's world chosen: its page open on `id`, from `entries` as the listing.
    #[cfg(test)]
    pub(crate) fn chosen_for_test(&mut self, entries: Vec<Entry>, id: u64) {
        self.listing_for_test(entries);
        self.open = true;
        self.panel.page = panel::Page::Detail;
        self.panel.selected = Some(id);
    }

    /// The world whose capture turns on the side panel now, and whether something reads one.
    #[cfg(test)]
    pub(crate) fn turning_for_test(&self) -> (Option<u64>, bool) {
        (self.live.as_ref().map(|(id, _)| *id), self.live_progress.is_some())
    }

    /// Whether a read of the listing is under way, for the tests: what it brings replaces a listing put in meanwhile.
    #[cfg(test)]
    pub fn listing_read_for_test(&self) -> bool {
        self.catalog.snapshot().1
    }

    #[cfg(test)]
    pub fn list_scroll_for_test(&self) -> f32 {
        self.panel.list_scroll_for_test()
    }

    #[cfg(test)]
    pub fn page_for_test(&self) -> panel::Page {
        self.panel.page
    }

    #[cfg(test)]
    pub fn status_for_test(&self) -> Status {
        self.status()
    }

    /// Whether the pointer is on something of the panel.
    pub fn points_at_something(&self) -> bool {
        self.panel.hover.is_some() || (self.panel.vr_dragging())
    }

    /// The press of [`Worlds::press_alone`] ended without being let go (its menu went away under it): nothing is done, the
    /// pressed card is not opened (OVL2-17).
    pub fn cancel_alone(&mut self) {
        if let (Some(panel::Target::Hero), Some((_, view))) = (self.panel.pressed, &self.live) {
            view.hold(false);
        }
        self.panel.vr_cancel();
        self.panel.pressed = None;
        self.panel.dirty = true;
        self.pressed_paint = None;
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
        self.pressed_paint = None;
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
        if let Some(chip) = self.hide_chip.take() {
            chip.destroy(overlay);
        }
        if let Some(chip) = self.shared.chip.take() {
            chip.destroy(overlay);
        }
        let _ = overlay.destroy(self.chip);
        let _ = overlay.destroy(self.bar);
    }

    fn time(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    /// Whether `e` may show now, as a picture, a page or in 3D: shared with the player signed in, and, being one of the listing's captures, still listed as
    /// this same upload (its revision: [`cache::Key`]) and still shared with them. A session of this computer, or a file kept here, needs only the first.
    /// Asked again before anything is shown or reused, not only when it was chosen (verify2-r15 blocker 1).
    fn allowed(&self, e: &Entry) -> bool {
        self.permitted(e) && (e.local.is_some() || !e.capture || self.paint_fixture.is_some() || self.catalog.entry(e.id).is_some_and(|now| cache::Key::of(&now) == cache::Key::of(e)))
    }

    /// Whether the world `e` is still the player's to see at all, whichever upload of it: shared with them, and listed (for one of the listing's captures).
    fn permitted(&self, e: &Entry) -> bool {
        let me = self.me.as_deref();
        if !e.for_player(me) {
            return false;
        }
        // A painted test or snapshot page: its own made-up list.
        if let Some(fixture) = &self.paint_fixture {
            return fixture.entries.iter().any(|x| x.id == e.id);
        }
        if e.local.is_some() || !e.capture {
            return true;
        }
        self.catalog.entry(e.id).is_some_and(|now| now.for_player(me))
    }

    /// The 3D view around the person goes at once, with the capture it shows and every download or switch on its way, all told to stop: shown to nobody once
    /// the account changed or the capture is no longer the person's to see (verify2-r15 blocker 1).
    fn drop_view(&mut self) {
        for d in [self.download.take(), self.preparing.take()].into_iter().flatten() {
            d.progress.stop.store(true, Ordering::Relaxed);
        }
        self.switch = None;
        self.prepare.clear();
        self.prepared_for = None;
        if self.current.take().is_some() || self.view.active() {
            self.view.hide_now();
        }
        self.panel.dirty = true;
    }

    /// Everything shown or on its way checked again, each turn: the listing read again may have taken a capture from this player, or replaced it with another
    /// upload. The 3D view around the person, its download and switch, what turns on the side panel, the page open.
    fn revalidate(&mut self) {
        if let Some(current) = self.current.clone() {
            if !self.allowed(&current) {
                self.drop_view();
            }
        }
        if let Some(id) = self.preparing.as_ref().map(|d| d.id) {
            if !self.catalog.entry(id).is_some_and(|e| self.allowed(&e)) {
                if let Some(d) = self.preparing.take() {
                    d.progress.stop.store(true, Ordering::Relaxed);
                }
            }
        }
        if let Some(entry) = self.live_entry.clone() {
            if !self.allowed(&entry) {
                self.stop_live();
            }
        }
        // The page open: gone once the world is not theirs to see; another upload of it, and the page shows that one.
        let open = self.panel.selected.and_then(|id| self.picked.as_ref().filter(|e| e.id == id)).cloned();
        if let Some(open) = open.filter(|e| e.capture) {
            if !self.permitted(&open) {
                self.forget_shown_world();
            } else if !self.allowed(&open) {
                self.picked = self.catalog.entry(open.id);
                self.panel.dirty = true;
            }
        }
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

    /// The Orbiters member and VRChat account this panel shows for. Another one (a switch, a sign-out, another session): nothing of the one before stays, its
    /// lists, the world whose page was open, what turned on its side panel. A capture shared with the one before only must not stay in view (verify-r15 B1).
    fn follow_login(&mut self, snap: &Snapshot) {
        let me = snap.vrchat_login.user.as_ref().map(|u| u.display_name.clone()).filter(|n| !n.trim().is_empty());
        if me != self.me {
            self.me = me;
            self.panel.dirty = true;
        }
        let scope = Scope::of(snap);
        if self.scope.as_ref() == Some(&scope) {
            return;
        }
        if self.scope.is_some() {
            self.finder.clear();
            self.forget_shown_world();
            self.drop_view();
            self.accounts += 1;
        }
        self.scope = Some(scope);
        self.vrchat = snap.vrchat_login.allowed && snap.vrchat_login.user.is_some();
        self.panel.dirty = true;
    }

    /// The world whose page is open goes, with what turns on its side panel and its kept copy: back to the list.
    fn forget_shown_world(&mut self) {
        self.stop_live();
        self.picked = None;
        self.presented_paint = None;
        self.pressed_paint = None;
        self.panel.selected = None;
        if self.panel.page == panel::Page::Detail {
            self.panel.page = panel::Page::Browse;
        }
        if let Some(live) = self.panel.live.as_mut() {
            live.full = false;
        }
        self.panel.dirty = true;
    }

    /// What the panel lists: the list shown, its worlds that have captures as their captures ([`finder::merge`]).
    fn entries(&self) -> Vec<Entry> {
        if let Some(fixture) = &self.paint_fixture { return fixture.entries.clone(); }
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
                    WorldList::Search(q) => {
                        let folded = orbiters_core::search::Query::new(q);
                        (Some(q.as_str()), captures.iter().filter(|e| e.matches_query(&folded, q)).collect())
                    }
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
        // Only one this player may see now (a capture shared with some people only), whatever was kept before.
        self.picked.as_ref().filter(|e| e.id == selected && self.allowed(e)).or(self.current.as_ref().filter(|e| e.id == selected && self.allowed(e)))
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
            status = Status { fetching: status.fetching, listing_error: status.listing_error, list: status.list, go: status.go, error: view.error.clone().or(status.error), ..view.clone() };
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
        if self.relay.as_ref().is_some_and(|was| same_but_stats(was, &view)) {
            // Only the 3D view's figures moved (its mean batch time, at every batch): nothing draws them, nothing to paint (OVL2-21).
            self.relay = Some(view);
        } else if self.relay.as_ref() != Some(&view) {
            let hole = self.small_hole();
            self.relay = Some(view);
            self.panel.dirty = true;
            self.hole_changed |= self.small_hole() != hole;
            // A capture lying in the side panel's hole now: the panel and the list beside it are where they end, at once. On their way, the list's cards still
            // reached over the hole and the panel round it was drawn moved, over the capture.
            if hole.is_none() && self.small_hole().is_some() {
                self.panel.side_at = self.time() - 10.0;
            }
        }
        std::mem::take(&mut self.relayed)
    }

    /// The 3D view goes: from a button that is not on this panel.
    /// What says how to handle what shows small, until it has been used.
    fn update_hint(&mut self, overlay: &vr::Overlay, steamvr: &vr::Client, presenter: &mut Presenter, snap: &Snapshot) {
        let small = self.view.small_now().copied();
        self.hint_used |= small.is_some_and(|s| s.zoomed);
        // Across a menu it stays: the menu is drawn over it (Enzo's diagram: menu, 3D view, game). It never fades for that, which made it vanish.
        if self.small_logged.due(std::time::Duration::from_secs(1)) && self.view.small() {
            let hint = self.hint.as_ref().map_or("no hint".to_string(), |h| h.report()); // i18n: not shown (a line of the VR test log)
            let chip = self.hide_chip.as_ref().map_or("no X".to_string(), |c| c.report()); // i18n: not shown (a line of the VR test log)
            super::vr_log::line(&format!("{} | {hint} | {chip} | menus up {} | hint used {}", self.view.report(overlay), self.menu_boxes.len(), self.hint_used)); // i18n: not shown (a log's line)
            // And three times a session, what the eye itself shows while it lies in a hole: what no render can say. Kept in the logs' folder of this computer only.
            // Only in a development build or when asked (`ORBITERS_VR_PICTURES`), as the other eye pictures: it was on for everyone,
            // three pictures of what the person sees in every session (PRIV-5). Decided before anything is read.
            if eye_frames_wanted(cfg!(debug_assertions), std::env::var_os("ORBITERS_VR_PICTURES").is_some()) && small.is_some_and(|s| s.docked) && self.eye_frames_kept < 3 {
                self.eye_frames_kept += 1;
                Self::keep_an_eye_frame(steamvr, self.eye_frames_kept);
            }
        }
        // Nothing lies behind it in a panel's hole: the hole is a window onto it (Enzo, 2026-11-11, IMG2). The opaque pane that was there, of order 0, was drawn over the
        // sheet's head and page and over the capture, which only an order by distance does: what SteamVR did there, whatever the orders said.
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
        // Not pressed while SteamVR shows an older picture of it.
        if wanted.is_some_and(|(_, _, pressed)| self.hide_chip.as_ref().is_some_and(|c| c.press(pressed))) {
            self.deferred.push(Action::Hide);
        }
    }

    /// The shared 3D view, each turn: a friend's view opens (small, before this player) once its file came and follows
    /// their zoom and pan; this player's X on it leaves it. The share chip on this player's own small view of an Orbiters
    /// capture, while friends here can be shown it: one friend, shown at once; several, the band's picker; sharing, it
    /// stops. Pressed on the trigger's release, still on it; gone under a held trigger, nothing.
    fn update_shared(&mut self, overlay: &vr::Overlay, steamvr: &vr::Client, presenter: &mut Presenter, snap: &Snapshot, backend: &Ctl) {
        let sv = snap.shared_view.arc();
        let now = Instant::now();
        // A friend's view.
        match sv.watching.as_ref() {
            Some(w) => {
                if self.shared.opened != Some(w.id) {
                    if let (Some(file), Some(start)) = (&w.file, w.start) {
                        let view = stereo::StartView { position: start.position, target: start.target, floor: start.floor, eye_height: start.eye_height };
                        let entry = Entry::shared(file.clone(), w.capture, w.revision, w.title.clone(), view);
                        // Small, before this player, whatever the 3D worlds show otherwise (as it was once it ends).
                        if !self.view.small_on() {
                            self.shared.small_was = Some(false);
                            self.view.set_small(true, overlay, steamvr);
                        }
                        let id = entry.id;
                        // This player's own capture, if one showed: back once the friend's goes.
                        if self.shared.before.is_none() {
                            self.shared.before = self.current.clone().filter(|e| !e.is_shared());
                        }
                        self.picked = Some(entry);
                        self.deferred.push(Action::Show(id));
                        self.shared.opened = Some(w.id);
                        self.shared.shown = false;
                        self.shared.applied = None;
                    }
                } else {
                    let showing = self.current.as_ref().is_some_and(|e| e.is_shared());
                    if showing {
                        self.shared.shown = true;
                    } else if self.shared.shown {
                        // Its X (or another capture opened): this player stops looking; their own capture comes back
                        // when its X closed the friend's.
                        self.shared.shown = false;
                        backend.shared_view(crate::ipc::SharedViewCommand::Leave);
                        if self.current.is_none() {
                            self.restore_own();
                        }
                    }
                    if showing && self.view.small_now().is_some() && w.view.is_some() && self.shared.applied != w.view {
                        let (reach, moved) = w.view.expect("just looked");
                        self.view.follow_shared(reach, moved);
                        self.shared.applied = w.view;
                    }
                }
            }
            None => {
                if self.shared.opened.take().is_some() {
                    let showing = self.current.as_ref().is_some_and(|e| e.is_shared());
                    if showing && self.view.small_now().is_some_and(|s| s.held()) {
                        // Held in a hand: it stays (Enzo's rule for a held view); it only stops following. Its X closes it.
                        self.shared.before = None;
                        self.shared.small_was = None;
                    } else if showing {
                        // The friend stopped, or this player left: the view goes, and the 3D worlds show as before.
                        self.restore_own();
                    }
                }
            }
        }
        // The small mode as it was, once nothing shows any more (set while a capture shows, it would show it again).
        if self.shared.opened.is_none() && !self.view.active() {
            if let Some(was) = self.shared.small_was.take() {
                self.deferred.push(Action::Small(was));
            }
        }
        // This player's own small view of an Orbiters capture (not a session of this computer, not a friend's).
        let own = self.current.as_ref().filter(|e| e.capture && e.local.is_none()).map(|e| (e.id, e.key as u64));
        let small = self.view.small_now().copied().filter(|s| !s.docked && self.menu_boxes.is_empty());
        let sharing = sv.outgoing.as_ref();
        if let Some(out) = sharing {
            match (own, self.view.small_now()) {
                (Some((id, _)), Some(s)) if id == out.capture => {
                    let (reach, moved) = (s.reach(), s.moved());
                    let due = self.shared.sent.is_none_or(|(at, r, m)| (r != reach || m != moved) && now.duration_since(at) >= Duration::from_millis(orbiters_core::shared_view::SEND_EVERY_MS));
                    if due {
                        self.shared.sent = Some((now, reach, moved));
                        backend.shared_view(crate::ipc::SharedViewCommand::View { reach, moved });
                    }
                }
                // Hidden, or another capture: it stops (once the download of the same one is not still on its way).
                _ if self.download.as_ref().is_none_or(|d| d.id != out.capture) && own.is_none_or(|(id, _)| id != out.capture) => {
                    if !self.shared.stop_sent {
                        self.shared.stop_sent = true;
                        backend.shared_view(crate::ipc::SharedViewCommand::Stop);
                    }
                }
                _ => {}
            }
        } else {
            self.shared.sent = None;
            self.shared.stop_sent = false;
        }
        let lit = sharing.is_some_and(|o| own.is_some_and(|(id, _)| id == o.capture));
        let eye = stereo::position(&view::head_pose(steamvr, 0.0));
        let wanted = small.zip(own).filter(|_| lit || (!sv.targets.is_empty() && sharing.is_none())).map(|(s, _)| {
            let place = small_hint::share_chip_place(s.anchor, s.radius(), eye);
            let holding = s.held_by();
            let on = |h: usize| Some(h) != holding && self.small_rays[h].is_some_and(|(tip, along)| small_hint::on_chip(&place, tip, along));
            (place, on(0) || on(1), [on(0), on(1)])
        });
        let mut pressed = false;
        match (self.shared.down, wanted) {
            (_, None) => self.shared.down = None,
            (None, Some((_, _, on))) => {
                self.shared.down = (0..2).find(|&h| on[h] && self.small_triggers[h] && !self.shared.triggers_were[h]);
            }
            (Some(h), Some((_, _, on))) if !self.small_triggers[h] => {
                self.shared.down = None;
                pressed = on[h];
            }
            _ => {}
        }
        self.shared.triggers_were = self.small_triggers;
        if wanted.is_some() && self.shared.chip.is_none() {
            self.shared.chip = small_hint::HideChip::share(overlay, &self.key).map_err(|e| eprintln!("{e:#}")).ok();
        }
        if let Some(chip) = self.shared.chip.as_mut() {
            chip.set_lit(lit);
            chip.update(overlay, presenter, wanted.map(|(place, pointed, _)| (place, pointed)));
        }
        let pressed = self.shared.chip.as_ref().is_some_and(|c| c.press(pressed));
        if let (true, Some((_, key)), Some(s)) = (pressed, own, small) {
            let capture = self.current.as_ref().map_or(0, |e| e.id);
            let ms = crate::wake_offer::now_ms();
            if lit {
                backend.shared_view(crate::ipc::SharedViewCommand::Stop);
            } else if let [friend] = sv.targets.as_slice() {
                backend.shared_view(crate::ipc::SharedViewCommand::Share { contact: friend.id.clone(), capture, revision: key, reach: s.reach(), moved: s.moved() });
            } else {
                crate::shared_view_ui::ask_pick(capture, key, s.reach(), s.moved(), ms);
            }
        }
    }

    /// The friend's view goes: this player's own capture, when one showed before it, shows again in its place, in the mode it
    /// had; otherwise nothing shows.
    fn restore_own(&mut self) {
        match self.shared.before.take() {
            Some(own) => {
                let id = own.id;
                self.picked = Some(own);
                self.deferred.push(Action::Show(id));
                if let Some(was) = self.shared.small_was.take() {
                    self.deferred.push(Action::Small(was));
                }
            }
            None => self.deferred.push(Action::Hide),
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

    /// The hole of that place, for the backdrop behind what shows small in it (opening.rs).
    pub fn small_opening(&mut self, opening: Option<opening::Opening>) {
        self.view.small_opening(opening);
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
    pub fn small_input(&mut self, hands: [Option<vr::Matrix34>; 2], grips: [bool; 2], sticks: [f32; 2], pads: [[f32; 2]; 2], pointed: Option<(vr::Matrix34, [f32; 2])>) {
        self.view.small_input(hands, grips, sticks, pads, pointed);
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
                Relayed::Prepare(entries) => self.prepare = entries,
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
            go: self.go.clone(),
            ..Default::default()
        };
        // Signed out of VRChat meanwhile: nothing of Go there can happen now, and it says so.
        if !self.vrchat && !s.go.world.is_empty() {
            s.go = panel::go::GoView {
                world: s.go.world.clone(),
                error: Some(go_words(&crate::controller::vrchat_data::WorldError::SignInRequired)),
                region: s.go.region,
                ..Default::default()
            };
        }
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
        } else if self.switch.is_some() {
            s.stage = Some(tr("Opening it").into());
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
        self.opened_from = None;
        self.panel.layout = layout;
        let status = self.status();
        let time = self.time();
        self.panel.open(&status, time);
    }

    /// Whether what moves on it moves, or is simply there.
    pub fn set_moves(&mut self, moves: bool) {
        self.panel.moves = moves;
    }

    /// The page of the world `world_id` (named `name`), opened from elsewhere on the sheet: a previous instance's row
    /// (Enzo, VR feedback F10). A world Orbiters has a 3D capture of shows its capture's page, the newest; any other its
    /// world's page, its author, description and pictures read as the Visited list reads them. Nothing shows around the
    /// person and nothing is sent to VRChat: the page has its own buttons for that. Its Back comes back here.
    pub fn open_world(&mut self, world_id: &str, name: &str) {
        let was = (self.panel.page, self.panel.selected);
        let captures = self.captures();
        let capture = captures.iter().filter(|e| e.world_id.eq_ignore_ascii_case(world_id)).max_by(|a, b| a.captured_at.cmp(&b.captured_at)).cloned();
        let entry = capture.unwrap_or_else(|| {
            let found = orbiters_core::maps::FoundWorld { world: orbiters_core::maps::World { world_id: world_id.to_string(), name: name.to_string(), ..Default::default() }, ..Default::default() };
            Entry::world(&found, None, self.catalog.feedback(world_id))
        });
        self.facts_for(&entry.world_id);
        let time = self.time();
        self.panel.show_detail(entry.id, time);
        self.picked = Some(entry);
        self.opened_from = Some(was);
        self.panel.dirty = true;
    }

    /// The world whose page shows, for the tests.
    #[cfg(test)]
    pub fn shown_world_for_test(&self) -> Option<(panel::Page, String, bool)> {
        let picked = self.picked()?;
        Some((self.panel.page, picked.world_id.clone(), picked.capture))
    }

    /// Back to it on a sheet: as it was left. Only its list is read again.
    pub fn come_back(&mut self, layout: KeyboardLayout) {
        self.catalog.refresh(&self.source);
        self.open = true;
        self.opened_from = None;
        self.panel.layout = layout;
        self.panel.dirty = true;
    }

    /// The head's Back, if there is somewhere to go within this panel. Its own does not show on a sheet.
    pub fn back_within(&mut self) -> bool {
        // A world's page opened from elsewhere: Back goes back there, and the panel is left as it was before.
        if self.panel.page == panel::Page::Detail {
            if let Some((page, selected)) = self.opened_from.take() {
                let time = self.time();
                self.panel.restore(page, selected, time);
                return false;
            }
        }
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
        self.follow_environment();
        // The app's feedback replaces the panel's own (its presses show at once, before the app has them).
        if snap.n4n0.feedback != self.feedback_seen {
            self.feedback_seen = snap.n4n0.feedback.clone();
            self.feedback = snap.n4n0.feedback.clone();
            self.panel.dirty = true;
        }
        // VRChat's requests go through the service, for this account and session: another one, other answers.
        if self.backend.is_none() {
            self.backend = Some(backend.clone());
        }
        self.follow_login(snap);
        self.revalidate();
        self.region = finder::region(snap.vrchat.world.instance.as_deref());
        self.go_arrived(&snap.vrchat.world);
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
        if let Some((capture, _)) = self.switch.take_if(|(_, at)| Instant::now() >= *at) {
            if self.current.as_ref().is_some_and(|e| self.allowed(e)) {
                self.view.show(overlay, steamvr, capture);
            }
        }
        self.prepare_ahead();
        self.view.update(overlay, steamvr);
        self.log_flip_times();
        self.update_hint(overlay, steamvr, presenter, snap);
        self.update_hide_chip(overlay, steamvr, presenter);
        self.update_shared(overlay, steamvr, presenter, snap, backend);
        if self.current.is_some() && self.download.is_none() && !self.view.active() {
            // Hidden (or failed): nothing is current any more.
            self.current = None;
            self.panel.dirty = true;
        }
        self.ask_feedback();
        self.follow_go();
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
        self.chip_presented.hand_over(None, std::time::Instant::now(), |r| presenter.show_part(overlay, self.chip, self.chip_canvas.pixels(), CHIP_PX[0], CHIP_PX[1], r));
        while let Some(e) = overlay.poll_event(self.chip) {
            if !show || !self.chip_presented.admits(e.event_type) {
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
            let damage = self.chip_canvas.damage();
            self.chip_presented.hand_over(damage, std::time::Instant::now(), |r| presenter.show_part(overlay, self.chip, self.chip_canvas.pixels(), CHIP_PX[0], CHIP_PX[1], r));
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
        self.bar_presented.hand_over(None, std::time::Instant::now(), |r| presenter.show_part(overlay, self.bar, self.bar_canvas.pixels(), BAR_PX[0], BAR_PX[1], r));
        while let Some(e) = overlay.poll_event(self.bar) {
            if !show || !self.bar_presented.admits(e.event_type) {
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
            let damage = self.bar_canvas.damage();
            self.bar_presented.hand_over(damage, std::time::Instant::now(), |r| presenter.show_part(overlay, self.bar, self.bar_canvas.pixels(), BAR_PX[0], BAR_PX[1], r));
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
    /// The carousel's next captures, downloaded one at a time while nothing else downloads, into the captures kept in memory: a flip to one shows it at once.
    fn prepare_ahead(&mut self) {
        if let Some(p) = &self.preparing {
            match p.answer.try_recv() {
                Ok(Ok(capture)) => {
                    let on_screen = self.current.as_ref().map(cache::Key::of);
                    self.captures.put(p.key.clone(), Arc::new(capture), on_screen.as_ref());
                    super::vr_log::line(&format!("carousel: {} downloaded ahead", p.id)); // i18n: not shown (a log's line)
                    self.preparing = None;
                }
                Ok(Err(e)) => {
                    super::vr_log::line(&format!("carousel: {} could not be downloaded ahead: {e:#}", p.id)); // i18n: not shown (a log's line)
                    self.preparing = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.preparing = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        // Nothing ahead of what the person waits for: only once the capture shown is downloaded.
        if self.preparing.is_some() || self.download.is_some() || self.current.is_none() {
            return;
        }
        let current = self.current.as_ref().map(|e| e.id);
        let Some(i) = self.prepare.iter().position(|e| Some(e.id) != current && !self.captures.holds(&cache::Key::of(e))) else { return };
        let entry = self.prepare.remove(i);
        let progress = Arc::new(Progress::default());
        let (tx, rx) = mpsc::channel();
        let (source, p, e) = (self.source.clone(), progress.clone(), entry.clone());
        let _ = std::thread::Builder::new().name("3d-worlds-ahead".into()).spawn(move || {
            let _ = tx.send(match &e.local {
                Some(dir) => catalog::read_kept(&e, dir, &p),
                None => source.download(&e, &p),
            });
        });
        self.preparing = Some(Download { id: entry.id, key: cache::Key::of(&entry), progress, answer: rx });
    }

    /// For `vr-test.log`: how long after a flip the 3D view showed.
    fn log_flip_times(&mut self) {
        let Some((id, at, ready)) = self.asked_at else { return };
        if self.current.as_ref().map(|e| e.id) != Some(id) {
            self.asked_at = None;
            return;
        }
        if self.switch.is_none() && self.download.is_none() && self.view.active() && !self.view.preparing() {
            super::vr_log::line(&format!("carousel: {id} in 3D {} ms after it was asked for ({})", at.elapsed().as_millis(), if ready { "downloaded before" } else { "downloaded then" })); // i18n: not shown (a log's line)
            self.asked_at = None;
        }
    }

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
        // Shown only while it is still the person's to see, as this upload (blocker 1).
        let current = self.current.as_ref().is_some_and(|e| e.id == id && cache::Key::of(e) == key && self.allowed(e));
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
            // Counted by its name alone: a world's id, an instance, a comment are texts, and `stable_name` drops them.
            crate::usage::press(&crate::usage::stable_name(&format!("WorldPanel({action:?})")));
            match action {
                Action::Show(id) => {
                    let Some(entry) = entries.iter().find(|e| e.id == id).or(self.picked.as_ref().filter(|e| e.id == id)).filter(|e| e.capture && self.allowed(e)).cloned() else {
                        continue;
                    };
                    // Enzo: nothing showing, and one opened from the start: its hint again, even if an earlier one was zoomed by accident.
                    if !self.view.active() {
                        self.hint_used = false;
                    }
                    self.error = None;
                    self.view.error = None;
                    self.switch = None;
                    let key = cache::Key::of(&entry);
                    if let Some(capture) = self.captures.get(&key) {
                        // Shown lately, or downloaded ahead by the carousel: at once, no download.
                        self.download = None;
                        self.asked_at = Some((id, Instant::now(), true));
                        // One lying in the panel's hole goes only once the panel has closed its hole over it: never a frame with neither.
                        if self.view.small_now().is_some_and(|s| s.docked) && self.current.as_ref().is_some_and(|c| c.id != id) {
                            self.current = Some(entry);
                            self.switch = Some((capture, Instant::now() + SWITCH_AFTER));
                            continue;
                        }
                        self.current = Some(entry);
                        self.view.show(overlay, steamvr, capture);
                        continue;
                    }
                    self.asked_at = Some((id, Instant::now(), false));
                    // Being downloaded ahead already: that download goes on as this one.
                    if let Some(ahead) = self.preparing.take_if(|p| p.id == id) {
                        self.view.warm_up(steamvr);
                        self.download = Some(ahead);
                        self.current = Some(entry);
                        continue;
                    }
                    // The render device and its shaders get ready while the capture downloads.
                    self.view.warm_up(steamvr);
                    let progress = Arc::new(Progress::default());
                    let (tx, rx) = mpsc::channel();
                    let (source, p, e) = (self.source.clone(), progress.clone(), entry.clone());
                    let _ = std::thread::Builder::new().name("3d-worlds-download".into()).spawn(move || {
                        let _ = tx.send(match &e.local {
                            Some(dir) => catalog::read_kept(&e, dir, &p),
                            None => source.download(&e, &p),
                        });
                    });
                    self.download = Some(Download { id, key, progress, answer: rx });
                    self.current = Some(entry);
                }
                Action::Hide => {
                    self.download = None;
                    self.switch = None;
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
                Action::Join(world) => self.go_load(world),
                Action::GoTo { world, instance } => self.go_there(world, GoTarget::Listed(instance)),
                Action::NewInstance { world, access, region } => self.go_there(world, GoTarget::New(access, region)),
                Action::InviteInstead { world, instance } => {
                    if let Some(gone) = self.go.went.as_ref().filter(|g| g.instance.id == instance) {
                        let target = GoTarget::Invite(gone.instance.clone());
                        self.go_there(world, target);
                    }
                }
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
                Action::SignIn => {
                    if let Some(backend) = &self.backend {
                        backend.sign_in();
                    }
                }
                // Asked again on the next turn.
                Action::RetryFeedback => self.asked = None,
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

    /// Go there's list of `world`'s instances, read on a thread (F5). Nothing is sent to VRChat but the read.
    fn go_load(&mut self, world: String) {
        let region = panel::go::region_of(self.region);
        let kept = self.go.list.take().filter(|_| self.go.world == world);
        self.go = panel::go::GoView { world: world.clone(), loading: true, list: kept, region, ..Default::default() };
        let (Some(backend), Some(scope)) = (self.backend.clone(), self.scope.clone()) else {
            self.go.loading = false;
            self.go.error = Some(go_words(&crate::controller::vrchat_data::WorldError::SignInRequired));
            return;
        };
        let tx = self.go_said.0.clone();
        let _ = std::thread::Builder::new().name("3d-worlds-instances".into()).spawn(move || {
            let read = match backend.vrchat_data(scope.clone(), DataRequest::WorldInstances { world: world.clone() }) {
                Ok(DataResponse::WorldInstances(list)) => Ok(list),
                Ok(DataResponse::WorldError(e)) => Err(go_words(&e)),
                Ok(_) => Err(tr("VRChat answered something unexpected.").to_string()),
                Err(e) => Err(e),
            };
            let _ = tx.send(GoSaid::Listed { world, scope, read });
        });
    }

    /// The next batch of Go there's list, by the exact ids VRChat left for it, when the page shown nears the end of what was read.
    fn go_more(&mut self) {
        let wanted = (self.panel.go_page + 2) * panel::go::GO_ROWS;
        let Some(list) = self.go.list.as_ref().filter(|l| !l.remaining.is_empty() && l.items.len() < wanted) else { return };
        if self.go.loading || self.go.more_loading {
            return;
        }
        let (Some(backend), Some(scope)) = (self.backend.clone(), self.scope.clone()) else { return };
        let (world, instances) = (self.go.world.clone(), list.remaining.clone());
        self.go.more_loading = true;
        let tx = self.go_said.0.clone();
        let _ = std::thread::Builder::new().name("3d-worlds-instances".into()).spawn(move || {
            let read = match backend.vrchat_data(scope.clone(), DataRequest::WorldInstancesNext { world: world.clone(), instances }) {
                Ok(DataResponse::WorldInstances(list)) => Ok(list),
                Ok(DataResponse::WorldError(e)) => Err(go_words(&e)),
                Ok(_) => Err(tr("VRChat answered something unexpected.").to_string()),
                Err(e) => Err(e),
            };
            let _ = tx.send(GoSaid::More { world, scope, read });
        });
    }

    /// To an instance of `world` listed, or to a new one made for the press, directly where it can (L-go-direct), on a
    /// thread; or the invite to the one just gone to, asked for instead.
    fn go_there(&mut self, world: String, target: GoTarget) {
        let (Some(backend), Some(scope)) = (self.backend.clone(), self.scope.clone()) else { return };
        if self.go.world != world || self.go.busy.is_some() {
            return;
        }
        self.go.error = None;
        self.go.busy = Some(match target {
            GoTarget::Listed(_) => panel::go::GoBusy::Going,
            GoTarget::New(..) => panel::go::GoBusy::Creating,
            GoTarget::Invite(_) => panel::go::GoBusy::Inviting,
        });
        let listed = match &target {
            GoTarget::Listed(id) => self.go.list.as_ref().and_then(|l| l.items.iter().find(|i| &i.id == id)).cloned(),
            GoTarget::New(..) | GoTarget::Invite(_) => None,
        };
        let tx = self.go_said.0.clone();
        let _ = std::thread::Builder::new().name("3d-worlds-invite".into()).spawn(move || {
            let ask = |request| match backend.vrchat_data(scope.clone(), request) {
                Ok(DataResponse::WorldError(e)) => Err((go_words(&e), go_list_changed(&e))),
                Ok(answer) => Ok(answer),
                Err(e) => Err((e, false)),
            };
            let unexpected = || (tr("VRChat answered something unexpected.").to_string(), false);
            let sent = match (target, listed) {
                (GoTarget::Invite(instance), _) => match ask(DataRequest::InviteSelf { world: world.clone(), instance: instance.id.clone() }) {
                    // Only an explicit Invited is an invite sent.
                    Ok(DataResponse::Invited) => Ok((instance, crate::controller::travel::Went::Invited(crate::controller::travel::Detour::Asked))),
                    Ok(_) => Err(unexpected()),
                    Err((e, changed)) => Err((tr!("The invite could not be sent: {e}", e = e), changed)),
                },
                (GoTarget::Listed(_), None) => Err((go_words(&crate::controller::vrchat_data::WorldError::NotListed), true)),
                (target, listed) => {
                    let instance = match (target, listed) {
                        (GoTarget::Listed(_), Some(instance)) => Ok(instance),
                        // Made for this one press, then gone to: the person asked to go to a new one.
                        (GoTarget::New(access, region), _) => match ask(DataRequest::CreateInstance { world: world.clone(), access, region }) {
                            Ok(DataResponse::CreatedInstance(made)) => Ok(made),
                            Ok(_) => Err(unexpected()),
                            // Said as what failed: the making, not a list.
                            Err((e, _)) => Err((tr!("The instance could not be made: {e}", e = e), false)),
                        },
                        _ => unreachable!(),
                    };
                    instance.and_then(|instance| match ask(DataRequest::GoThere { world: world.clone(), instance: instance.id.clone() }) {
                        Ok(DataResponse::Went(how)) => Ok((instance, how)),
                        Ok(_) => Err(unexpected()),
                        Err(e) => Err(e),
                    })
                }
            };
            let _ = tx.send(GoSaid::Went { world, scope, sent });
        });
    }

    /// VRChat's log says the person is in the instance Go there took them to: its card says so.
    fn go_arrived(&mut self, now: &orbiters_core::vrchat_log::WorldState) {
        let Some(gone) = self.go.went.as_mut().filter(|g| !g.arrived) else { return };
        if now.world_id.as_deref() == Some(gone.instance.world.as_str()) && now.instance.as_deref().is_some_and(|i| same_instance(i, &gone.instance.id)) {
            gone.arrived = true;
            self.panel.dirty = true;
        }
    }

    /// What Go there's threads answered: kept only for the world and the account it was asked for.
    fn follow_go(&mut self) {
        while let Ok(said) = self.go_said.1.try_recv() {
            let (world, scope) = match &said {
                GoSaid::Listed { world, scope, .. } | GoSaid::More { world, scope, .. } | GoSaid::Went { world, scope, .. } => (world.clone(), scope.clone()),
            };
            if world != self.go.world || self.scope.as_ref() != Some(&scope) {
                continue;
            }
            match said {
                GoSaid::Listed { read, .. } => {
                    self.go.loading = false;
                    match read {
                        Ok(list) => self.go.list = Some(list),
                        Err(e) => {
                            self.go.list = None;
                            self.go.error = Some(e);
                        }
                    }
                }
                // Added to what was read, where the person is in it untouched.
                GoSaid::More { read, .. } => {
                    self.go.more_loading = false;
                    match (read, self.go.list.as_mut()) {
                        (Ok(more), Some(list)) => {
                            list.items.extend(more.items);
                            list.unavailable += more.unavailable;
                            list.remaining = more.remaining;
                        }
                        (Ok(_), None) => {}
                        (Err(e), list) => {
                            // The rest cannot be read now: what was read stays, and says so.
                            if let Some(list) = list {
                                list.remaining.clear();
                            }
                            self.go.error = Some(e);
                        }
                    }
                }
                GoSaid::Went { sent, .. } => {
                    self.go.busy = None;
                    match sent {
                        Ok((instance, how)) => {
                            self.go.went = Some(panel::go::GoneTo { instance, how, arrived: false });
                            self.go.error = None;
                        }
                        // The list changed under it (full, closed, gone): read again, and said why.
                        Err((e, changed)) => {
                            if changed {
                                self.go_load(world);
                            }
                            self.go.error = Some(e);
                        }
                    }
                }
            }
            self.panel.dirty = true;
        }
        self.go_more();
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
        // The CPU replay draws at frozen time with acknowledged local pictures.
        // A real-time caret/fade tick must not keep that settled fixture busy.
        if self.paint_fixture.is_some() { return self.hole_changed || self.panel.dirty || self.facts_pending; }
        let since = self.last_paint.map(|t| t.elapsed());
        let due = since.is_none_or(|t| t >= Duration::from_millis(22));
        let facts = self.facts_pending && since.is_none_or(|t| t >= FACTS_EVERY);
        self.hole_changed || (due && (self.panel.dirty || self.panel.animating(&self.status(), self.time()) || self.pictures.loading() || facts))
    }

    /// Paints the panel on the menu's canvas.
    pub fn paint(&mut self, canvas: &mut Canvas) {
        let snapshot = self.prepare_paint(canvas.ctx());
        canvas.paint(snapshot.time, |p| snapshot.draw(p));
        self.panel.adopt_metrics(&snapshot.panel);
        if self.snapshot_mode { self.presented_paint = Some(snapshot); }
    }

    /// What a picture of the page is kept under: the page, and the account it was painted for (a page cached for the one before is never shown again, B1).
    pub(crate) fn paint_key(&self) -> String { paint_snapshot::page_key(&self.panel, self.accounts) }

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
                // The carousel's neighbours, two each way: their pictures are here before a flip shows one (K-carousel).
                let set = self.panel.carousel(&d);
                if let Some(at) = self.panel.selected.and_then(|id| set.iter().position(|e| e.id == id)) {
                    for e in set[at.saturating_sub(2)..(at + 3).min(set.len())].iter().copied() {
                        if !list.iter().any(|l| l.id == e.id) {
                            list.push(e);
                        }
                    }
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
        let now = self.time();
        let mut out = HashMap::new();
        for v in visible {
            let Some(url) = v.thumbnail.clone().or_else(|| self.facts.get(&v.world).and_then(|f| f.thumbnail.clone())) else { continue };
            let picture = crate::worlds::Picture::Remote { world_id: v.world.clone(), url, large: false };
            if let Some(t) = self.pictures.get_at(&self.textures, now, &picture) {
                out.insert(v.id, t.id());
            }
        }
        out
    }

    /// A world's thumbnail, from its id alone, once it is here: for what lists worlds outside this panel.
    /// A source can be loaded in another canvas without borrowing this canvas's texture id.
    pub fn world_thumbnail_source(&mut self, world_id: &str) -> Option<crate::worlds::Picture> {
        self.facts_for(world_id);
        let url = self.facts.get(world_id).and_then(|f| f.thumbnail.clone())?;
        Some(crate::worlds::Picture::Remote { world_id: world_id.to_string(), url, large: false })
    }

    pub fn world_thumbnail(&mut self, world_id: &str) -> Option<TextureId> {
        let now = self.time();
        self.facts_for(world_id);
        let url = self.facts.get(world_id).and_then(|f| f.thumbnail.clone())?;
        let picture = crate::worlds::Picture::Remote { world_id: world_id.to_string(), url, large: false };
        self.pictures.get_at(&self.textures, now, &picture).map(|t| t.id())
    }

    /// The selected capture's world's pictures on the pictures page: the page's thumbnails, the one shown whole.
    fn world_pictures(&mut self, entries: &[Entry]) -> HashMap<(u64, bool), TextureId> {
        let now = self.time();
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
            if let Some(t) = self.pictures.get_at(&self.textures, now, &picture) {
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


/// Whether two statuses differ in nothing but `stats`, the 3D view's figures, which no page draws.
fn same_but_stats(a: &Status, b: &Status) -> bool {
    a.stats != b.stats && Status { stats: None, ..a.clone() } == Status { stats: None, ..b.clone() }
}

/// Whether the pictures of the left eye at a docked small view are kept: in a development build, or when the eye pictures are
/// asked for (`ORBITERS_VR_PICTURES`); never otherwise.
fn eye_frames_wanted(debug_build: bool, pictures_asked: bool) -> bool {
    debug_build || pictures_asked
}

/// The pictures [`Worlds::keep_an_eye_frame`] writes, by name: only these are ever deleted (verify-vr: the whole folder went,
/// with whatever else someone kept there).
const EYE_FRAME_NAMES: [&str; 3] = ["small-in-its-hole-1.png", "small-in-its-hole-2.png", "small-in-its-hole-3.png"];

/// Deletes the eye pictures in `dir`, then `dir` itself only if nothing else is left in it. Says what could not be deleted.
fn forget_eye_frames_in(dir: &std::path::Path) -> Vec<String> {
    let mut problems = Vec::new();
    for name in EYE_FRAME_NAMES {
        match std::fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => problems.push(format!("{name} could not be deleted: {e}")), // i18n: not shown (a log's line)
        }
    }
    if std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_none()) {
        if let Err(e) = std::fs::remove_dir(dir) {
            problems.push(format!("the empty folder could not be deleted: {e}")); // i18n: not shown (a log's line)
        }
    }
    problems
}

/// The side panel's captures are read one at a time: chosen quickly one after another, each waits for the one before, which was told to stop and ends at
/// its next chunk, and gives up at once if it was told to stop meanwhile. However fast worlds are flipped, at most one download or decode runs, for the
/// latest chosen (verify2-r15 S4).
static SIDE_LOADS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn one_load_at_a_time<T>(progress: &Progress, load: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    progress.check()?;
    let _turn = SIDE_LOADS.lock().unwrap_or_else(|e| e.into_inner());
    progress.check()?;
    load()
}

#[cfg(test)]
mod load_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// verify2-r15 S4: worlds chosen quickly one after another: one reading runs at a time, and those told to stop while they waited never run.
    #[test]
    fn rapid_choices_read_one_at_a_time_and_only_the_last() {
        let (running, most, ran) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let first = Arc::new(Progress::default());
        let (held, hold) = (std::sync::mpsc::channel::<()>(), std::sync::mpsc::channel::<()>());
        let reader = |p: Arc<Progress>, wait: Option<std::sync::mpsc::Receiver<()>>, started: Option<std::sync::mpsc::Sender<()>>| {
            let (running, most, ran) = (running.clone(), most.clone(), ran.clone());
            std::thread::spawn(move || {
                one_load_at_a_time(&p, || {
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    most.fetch_max(now, Ordering::SeqCst);
                    ran.fetch_add(1, Ordering::SeqCst);
                    if let Some(s) = started {
                        let _ = s.send(());
                    }
                    if let Some(w) = wait {
                        let _ = w.recv();
                    }
                    running.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                })
            })
        };
        let a = reader(first.clone(), Some(hold.1), Some(held.0));
        held.1.recv().unwrap();
        // Four more chosen while the first reads; each told to stop as the next is chosen, the last one wanted.
        let later: Vec<Arc<Progress>> = (0..4).map(|_| Arc::new(Progress::default())).collect();
        let threads: Vec<_> = later.iter().map(|p| reader(p.clone(), None, None)).collect();
        first.stop.store(true, Ordering::SeqCst);
        for p in &later[..3] {
            p.stop.store(true, Ordering::SeqCst);
        }
        hold.0.send(()).unwrap();
        let _ = a.join().unwrap();
        let results: Vec<bool> = threads.into_iter().map(|t| t.join().unwrap().is_ok()).collect();
        assert_eq!(most.load(Ordering::SeqCst), 1, "never two at once");
        assert_eq!(results, [false, false, false, true], "those told to stop never read; the last one does");
        assert_eq!(ran.load(Ordering::SeqCst), 2, "the first (already reading) and the last");
    }
}

/// Those a release build kept before it stopped (PRIV-5) go, once, when they are not wanted.
fn forget_kept_eye_frames() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if !eye_frames_wanted(cfg!(debug_assertions), std::env::var_os("ORBITERS_VR_PICTURES").is_some()) {
            for problem in forget_eye_frames_in(&orbiters_core::paths::logs_dir().join("vr-test-frames")) {
                super::vr_log::line(&format!("eye frames: {problem}")); // i18n: not shown (a log's line)
            }
        }
    });
}

#[cfg(test)]
mod go_tests {
    /// verify-vr on PRIV-5: only the three eye pictures go; anything else in the folder stays, and so does the folder then.
    #[test]
    fn only_the_eye_pictures_are_deleted() {
        let dir = std::env::temp_dir().join(format!("orbiters-eye-frames-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("notes")).unwrap();
        for name in super::EYE_FRAME_NAMES {
            std::fs::write(dir.join(name), b"png").unwrap();
        }
        std::fs::write(dir.join("comparison.png"), b"mine").unwrap();
        std::fs::write(dir.join("notes").join("kept.txt"), b"mine").unwrap();
        assert!(super::forget_eye_frames_in(&dir).is_empty());
        for name in super::EYE_FRAME_NAMES {
            assert!(!dir.join(name).exists(), "{name}");
        }
        assert!(dir.join("comparison.png").exists() && dir.join("notes").join("kept.txt").exists(), "nothing else is touched");
        std::fs::remove_file(dir.join("comparison.png")).unwrap();
        std::fs::remove_dir_all(dir.join("notes")).unwrap();
        std::fs::write(dir.join(super::EYE_FRAME_NAMES[0]), b"png").unwrap();
        assert!(super::forget_eye_frames_in(&dir).is_empty());
        assert!(!dir.exists(), "a folder left empty goes");
        assert!(super::forget_eye_frames_in(&dir).is_empty(), "nothing there: nothing to say");
    }

    /// PRIV-5: a release build keeps no picture of the person's view unless they are asked for.
    #[test]
    fn eye_pictures_are_kept_only_in_development_or_when_asked() {
        assert!(!super::eye_frames_wanted(false, false), "a release build, not asked: none");
        assert!(super::eye_frames_wanted(false, true));
        assert!(super::eye_frames_wanted(true, false));
    }

    /// OVL2-21: a relayed status whose only change is the undrawn figures does not repaint the page; any other change does.
    #[test]
    fn undrawn_figures_do_not_repaint_the_page() {
        use super::panel::Status;
        let a = Status { current: Some(10), shown: true, stats: Some("1 ms".into()), ..Default::default() };
        let b = Status { stats: Some("1.1 ms".into()), ..a.clone() };
        assert!(super::same_but_stats(&a, &b));
        assert!(!super::same_but_stats(&a, &a), "the same is not 'only the figures'");
        assert!(!super::same_but_stats(&a, &Status { shown: false, ..b.clone() }));
    }

    use super::*;
    use orbiters_core::vrchat_api::{InstanceAccess, WorldInstance, WorldInstances};

    const WORLD: &str = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd";

    fn instance(id: &str) -> WorldInstance {
        WorldInstance { world: WORLD.into(), id: id.into(), access: InstanceAccess::Public, region: "eu".into(), occupants: 3, capacity: Some(16), friends: None, full: false, closed: false, can_join: true }
    }

    /// F5: Go there's threads answer late; what they say is kept only for the world and the VRChat account it was asked for, and an invite refused because
    /// the list changed says why.
    #[test]
    fn go_there_keeps_only_answers_for_its_world_and_account() {
        let mut snap = crate::state::Snapshot::new(orbiters_core::settings::Settings::default());
        let ours = Scope::of(&snap);
        snap.vrchat_login.revision += 1;
        let other = Scope::of(&snap);
        assert_ne!(ours, other);
        let mut worlds = Worlds::alone(egui::Context::default());
        worlds.scope = Some(ours.clone());
        worlds.vrchat = true;
        worlds.go = panel::go::GoView { world: WORLD.into(), loading: true, ..Default::default() };
        let list = || Ok(WorldInstances { items: vec![instance("1~region(eu)")], unavailable: 0, remaining: Vec::new() });
        let tx = worlds.go_said.0.clone();
        tx.send(GoSaid::Listed { world: "wrld_00000000-0000-4000-8000-000000000002".into(), scope: ours.clone(), read: list() }).unwrap();
        tx.send(GoSaid::Listed { world: WORLD.into(), scope: other.clone(), read: list() }).unwrap();
        worlds.follow_go();
        assert!(worlds.go.loading && worlds.go.list.is_none(), "another world's, another account's: not this list");
        tx.send(GoSaid::Listed { world: WORLD.into(), scope: ours.clone(), read: list() }).unwrap();
        worlds.follow_go();
        assert!(!worlds.go.loading && worlds.go.list.as_ref().is_some_and(|l| l.items.len() == 1));
        assert!(worlds.status().go.list.is_some(), "and the panel sees it");
        // The invite refused, the list having changed: said, and the list read again.
        worlds.go.busy = Some(panel::go::GoBusy::Inviting);
        tx.send(GoSaid::Went { world: WORLD.into(), scope: ours.clone(), sent: Err(("That instance is full now.".into(), true)) }).unwrap();
        worlds.follow_go();
        assert_eq!(worlds.go.busy, None);
        assert_eq!(worlds.go.error.as_deref(), Some("That instance is full now."));
        worlds.go.loading = false;
        worlds.go.busy = Some(panel::go::GoBusy::Inviting);
        worlds.go.busy = Some(panel::go::GoBusy::Going);
        tx.send(GoSaid::Went { world: WORLD.into(), scope: ours.clone(), sent: Ok((instance("1~region(eu)"), crate::controller::travel::Went::Direct)) }).unwrap();
        worlds.follow_go();
        assert_eq!(worlds.go.went.as_ref().map(|g| (g.instance.id.as_str(), g.how, g.arrived)), Some(("1~region(eu)", crate::controller::travel::Went::Direct, false)));
        // VRChat's log: still elsewhere, then in it (its id with the client's nonce): the card says the person is there.
        let mut now = orbiters_core::vrchat_log::WorldState { world_id: Some(WORLD.into()), instance: Some("2~region(eu)".into()), ..Default::default() };
        worlds.go_arrived(&now);
        assert!(!worlds.go.went.as_ref().unwrap().arrived);
        now.instance = Some("1~region(eu)~nonce(abc)".into());
        worlds.go_arrived(&now);
        assert!(worlds.go.went.as_ref().unwrap().arrived);
        // VRChat's list comes a batch at a time: the next batch is added to what was read; one that cannot be read leaves what was, and says so.
        worlds.go.list.as_mut().unwrap().remaining = vec!["2~region(eu)".into(), "3~region(eu)".into()];
        assert_eq!(panel::go::go_pages(&worlds.go), 1, "five a page: one, the batch still to come with it");
        tx.send(GoSaid::More { world: WORLD.into(), scope: other, read: Ok(WorldInstances { items: vec![instance("9")], unavailable: 0, remaining: Vec::new() }) }).unwrap();
        tx.send(GoSaid::More { world: WORLD.into(), scope: ours.clone(), read: Ok(WorldInstances { items: vec![instance("2~region(eu)")], unavailable: 1, remaining: vec!["3~region(eu)".into()] }) }).unwrap();
        worlds.follow_go();
        let list = worlds.go.list.clone().unwrap();
        assert_eq!(list.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["1~region(eu)", "2~region(eu)"]);
        assert_eq!((list.unavailable, list.remaining.len()), (1, 1));
        tx.send(GoSaid::More { world: WORLD.into(), scope: ours.clone(), read: Err("VRChat asks to wait".into()) }).unwrap();
        worlds.follow_go();
        assert!(worlds.go.list.as_ref().is_some_and(|l| l.items.len() == 2 && l.remaining.is_empty()), "what was read stays");
        assert!(worlds.go.error.is_some());
        // Signed out of VRChat: the page says so, whatever it read.
        worlds.vrchat = false;
        let status = worlds.status();
        assert!(status.go.list.is_none() && status.go.went.is_none() && status.go.error.is_some());
    }
}

#[cfg(test)]
mod carousel_tests {
    use super::*;

    fn many(n: u64) -> Vec<Entry> {
        let previews: Vec<String> = (1..=n)
            .map(|i| format!(r#"{{"slot":{i},"key":{i},"id":{i},"worldId":"wrld_00000000-0000-4000-8000-{i:012}","name":"World {i}","author":"a","uploader":"b","view":{{"position":[0,1,0],"target":[0,1,1]}}}}"#))
            .collect();
        catalog::parse(&format!(r#"{{"previews":[{}],"feedback":{{}}}}"#, previews.join(","))).unwrap()
    }

    /// A sheet's panel in a headset, a capture's side panel open on `id`, its view small in the hole.
    fn on_a_sheet(id: u64) -> Worlds {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let mut worlds = Worlds::alone(ctx);
        worlds.listing_for_test(many(8));
        worlds.relay_to(Status { headset: true, current: Some(id), shown: true, small: true, small_docked: true, ..Default::default() });
        worlds.panel.page = panel::Page::Detail;
        worlds.panel.selected = Some(id);
        worlds
    }

    fn shows(relayed: &[Relayed]) -> Vec<u64> {
        relayed.iter().filter_map(|r| if let Relayed::Show(e) = r { Some(e.id) } else { None }).collect()
    }

    /// K-carousel: the stick on the carousel flips once a push, again only once let back, and on by itself while held to one side, to go through many quickly.
    #[test]
    fn the_stick_flips_once_a_push_and_on_while_held() {
        let mut worlds = on_a_sheet(3);
        let view = worlds.relay.clone().unwrap();
        assert!(!worlds.flip_stick(0.4), "a thumb's nudge");
        assert!(worlds.flip_stick(0.9));
        assert_eq!(worlds.panel.selected, Some(4));
        assert_eq!(shows(&worlds.relay_to(view.clone())), vec![4], "shown as it comes, by the overlay's own view");
        assert!(!worlds.flip_stick(0.95), "held: not again at once");
        assert!(!worlds.flip_stick(0.4), "half back is not let go");
        assert!(!worlds.flip_stick(-0.9), "the other way waits for the middle");
        assert!(!worlds.flip_stick(0.1));
        assert!(worlds.flip_stick(-0.9));
        assert_eq!(worlds.panel.selected, Some(3));
        std::thread::sleep(FLIP_REPEAT_FIRST + Duration::from_millis(30));
        assert!(worlds.flip_stick(-0.9), "held: on by itself");
        assert_eq!(worlds.panel.selected, Some(2));
        assert!(!worlds.flip_stick(-0.9));
        worlds.flip_stick(0.0);
        assert!(worlds.flip_stick(-0.9));
        assert_eq!(worlds.panel.selected, Some(1));
        worlds.flip_stick(0.0);
        assert!(!worlds.flip_stick(-0.9), "nothing before the first");
    }

    /// K-carousel: the overlay's own view is asked to download the next captures ahead, nearest first and the way the person goes, once for each world shown.
    #[test]
    fn the_next_captures_are_asked_ahead_the_way_the_person_goes() {
        let mut worlds = on_a_sheet(3);
        let view = worlds.relay.clone().unwrap();
        worlds.prepare_neighbours();
        let prepared = |relayed: Vec<Relayed>| relayed.into_iter().find_map(|r| if let Relayed::Prepare(e) = r { Some(e.iter().map(|e| e.id).collect::<Vec<_>>()) } else { None });
        assert_eq!(prepared(worlds.relay_to(view.clone())), Some(vec![4, 2, 5]));
        worlds.prepare_neighbours();
        assert_eq!(prepared(worlds.relay_to(view.clone())), None, "asked once");
        worlds.flip_stick(-0.9);
        worlds.prepare_neighbours();
        assert_eq!(prepared(worlds.relay_to(view.clone())), Some(vec![1, 3, 4]), "going back: the one before first");
        // The overlay's own 3D worlds keep what was asked.
        let mut own = Worlds::alone(egui::Context::default());
        own.from_elsewhere(vec![Relayed::Prepare(many(3))]);
        assert_eq!(own.prepare.iter().map(|e| e.id).collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    /// REQUEST15 item 6: on a screen, choosing a world with a 3D capture starts its view turning in the side panel at once, without a See it in 3D; one at a time,
    /// the one before let go as another is chosen, all of it gone with the page; a world without one keeps its picture; in a headset nothing starts by itself.
    #[test]
    fn on_a_screen_the_chosen_world_turns_in_its_panel_one_at_a_time() {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let mut worlds = Worlds::alone(ctx);
        // Kept here, in a folder that is not there: no download, and the view says it cannot show rather than wait for the network.
        let gone = std::env::temp_dir().join(format!("orbiters-no-capture-{}", std::process::id()));
        let mut list = many(3);
        for e in &mut list {
            e.local = Some(gone.join(e.id.to_string()));
        }
        list[2].capture = false;
        worlds.listing_for_test(list);
        worlds.open = true;
        worlds.set_shown(true);
        worlds.panel.page = panel::Page::Detail;
        worlds.panel.selected = Some(1);
        worlds.follow_screen_live();
        let live = |w: &Worlds| (w.live.as_ref().map(|(id, _)| *id), w.panel.live.as_ref().map(|l| l.id));
        assert_eq!(live(&worlds), (Some(1), Some(1)), "turning at once, no button pressed");
        worlds.follow_screen_live();
        assert_eq!(live(&worlds), (Some(1), Some(1)), "not started again every turn");
        worlds.panel.selected = Some(2);
        worlds.follow_screen_live();
        assert_eq!(live(&worlds), (Some(2), Some(2)), "the next one, the one before let go");
        worlds.panel.selected = Some(3);
        worlds.follow_screen_live();
        assert_eq!(live(&worlds), (None, None), "no capture: its picture stays, nothing turns");
        worlds.panel.selected = Some(1);
        worlds.follow_screen_live();
        worlds.panel.page = panel::Page::Browse;
        worlds.follow_screen_live();
        assert_eq!(live(&worlds), (None, None), "gone with the page");
        // In a headset the view is the overlay's own, around the person: nothing turns on the panel by itself.
        let mut sheet = on_a_sheet(1);
        sheet.open = true;
        sheet.set_shown(true);
        sheet.follow_screen_live();
        assert_eq!(live(&sheet), (None, None));
    }

    /// verify-r15 B2: the Worlds page left for another tab, or the menu closed, its capture stops turning and its reading is told to stop, and nothing starts it
    /// again turn after turn behind the other page; back on the page it turns again. Choosing another world stops the reading before.
    #[test]
    fn a_capture_stops_with_its_page_and_is_not_started_again_behind_another() {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let mut worlds = Worlds::alone(ctx);
        let gone = std::env::temp_dir().join(format!("orbiters-no-capture-b2-{}", std::process::id()));
        let mut list = many(2);
        for e in &mut list {
            e.local = Some(gone.join(e.id.to_string()));
        }
        worlds.listing_for_test(list);
        worlds.open = true;
        worlds.panel.page = panel::Page::Detail;
        worlds.panel.selected = Some(1);
        worlds.set_shown(true);
        worlds.follow_screen_live();
        let first = worlds.live_progress.clone().expect("reading");
        assert!(worlds.live.is_some());
        worlds.panel.selected = Some(2);
        worlds.follow_screen_live();
        assert!(first.stop.load(Ordering::Relaxed), "the one before told to stop");
        let second = worlds.live_progress.clone().expect("reading the next");
        // Another tab: the page stays the Worlds' own detail behind it, as the orbit keeps it.
        worlds.set_shown(false);
        for _ in 0..5 {
            worlds.follow_screen_live();
            assert!(worlds.live.is_none() && worlds.live_progress.is_none(), "not started again behind another page");
        }
        assert!(second.stop.load(Ordering::Relaxed), "its reading told to stop");
        worlds.set_shown(true);
        worlds.follow_screen_live();
        assert!(worlds.live.is_some(), "back on the page: it turns again");
    }

    /// A capture of the listing shared with Alice only, and a signed-in snapshot for each account.
    fn alice_only() -> (Worlds, Vec<Entry>) {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let worlds = Worlds::alone(ctx);
        let mut list = many(2);
        list[0].only_for = Some(vec!["Alice".into()]);
        worlds.listing_for_test(list.clone());
        (worlds, list)
    }

    fn signed_in_as(id: &str, name: &str, revision: u64) -> crate::state::Snapshot {
        let mut snap = crate::state::Snapshot::new(orbiters_core::settings::Settings::default());
        snap.vrchat_login.allowed = true;
        snap.vrchat_login.revision = revision;
        snap.vrchat_login.user = Some(orbiters_core::vrchat_api::User { id: id.into(), display_name: name.into(), ..Default::default() });
        snap
    }

    /// verify2-r15 blocker 1: Alice's capture shown around her in the headset, its download still on its way, its switch pending: Bob signs in. Nothing of it
    /// stays current, the download is told to stop and what it brings is never shown, the switch goes, and the view is told to go at once.
    #[test]
    fn the_headset_s_view_and_its_pending_work_go_with_the_account() {
        let (mut worlds, list) = alice_only();
        worlds.follow_login(&signed_in_as("usr_a", "Alice", 1));
        let private = list[0].clone();
        assert!(worlds.allowed(&private));
        // Shown (as `act` leaves it), a download for it on its way, a switch to it pending, the next one being read ahead.
        let (tx, rx) = std::sync::mpsc::channel();
        let progress = Arc::new(Progress::default());
        worlds.current = Some(private.clone());
        worlds.download = Some(Download { id: private.id, key: cache::Key::of(&private), progress: progress.clone(), answer: rx });
        let ahead = Arc::new(Progress::default());
        let (_ahead_tx, ahead_rx) = std::sync::mpsc::channel();
        worlds.preparing = Some(Download { id: list[1].id, key: cache::Key::of(&list[1]), progress: ahead.clone(), answer: ahead_rx });
        worlds.switch = Some((Arc::new(Capture::new(Default::default(), None, private.view)), Instant::now()));
        worlds.follow_login(&signed_in_as("usr_b", "Bob", 2));
        assert!(worlds.current.is_none() && worlds.download.is_none() && worlds.switch.is_none() && worlds.preparing.is_none());
        assert!(progress.stop.load(Ordering::Relaxed) && ahead.stop.load(Ordering::Relaxed), "both readings told to stop");
        assert!(tx.send(Ok(Capture::new(Default::default(), None, private.view))).is_err(), "what the download brings has nowhere to go");
        // Shown again by any way (a relayed Show): refused for Bob.
        worlds.picked = Some(private.clone());
        assert!(!worlds.allowed(&private) && worlds.picked().is_none());
    }

    /// verify2-r15 blocker 1: the same account, the listing read again: Alice's capture now shared with someone else only, or replaced by another upload.
    /// What turned on the side panel with the same id goes (or turns as the new upload), its page goes, and the headset's view goes.
    #[test]
    fn a_listing_that_takes_a_capture_away_takes_it_from_view() {
        let (mut worlds, list) = alice_only();
        let mut list = list;
        for e in &mut list {
            e.local = None;
        }
        worlds.listing_for_test(list.clone());
        worlds.follow_login(&signed_in_as("usr_a", "Alice", 1));
        worlds.open = true;
        worlds.set_shown(true);
        worlds.panel.page = panel::Page::Detail;
        worlds.panel.selected = Some(1);
        worlds.keep_picked(&worlds.entries());
        worlds.follow_screen_live();
        assert_eq!(worlds.live.as_ref().map(|(id, _)| *id), Some(1), "Alice's turns");
        // Another upload of it: not the one turning; it turns as the new one.
        let mut newer = list.clone();
        newer[0].key += 1;
        worlds.listing_for_test(newer.clone());
        worlds.revalidate();
        worlds.keep_picked(&worlds.entries());
        worlds.follow_screen_live();
        assert_eq!(worlds.live_entry.as_ref().map(|e| e.key), Some(newer[0].key), "the new upload, not the old one reused");
        // Shared with Carol only now: same id, same account: nothing of it shows.
        worlds.current = Some(newer[0].clone());
        let mut taken = newer.clone();
        taken[0].only_for = Some(vec!["Carol".into()]);
        worlds.listing_for_test(taken);
        worlds.revalidate();
        worlds.follow_screen_live();
        assert!(worlds.live.is_none() && worlds.live_entry.is_none(), "not turning");
        assert!(worlds.panel.selected.is_none() && worlds.picked().is_none(), "its page gone");
        assert!(worlds.current.is_none(), "the headset's view gone");
    }

    /// verify-r15 B1: a capture shared with the player signed in only, open with its 3D turning, goes the moment another account signs in (or the player signs
    /// out): no page, no 3D, no kept copy, and a page painted before is not the one the new account's page is kept under.
    #[test]
    fn a_private_capture_goes_with_the_account_that_could_see_it() {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let mut worlds = Worlds::alone(ctx);
        let gone = std::env::temp_dir().join(format!("orbiters-no-capture-b1-{}", std::process::id()));
        let mut list = many(2);
        for e in &mut list {
            e.local = Some(gone.join(e.id.to_string()));
        }
        list[0].only_for = Some(vec!["Alice".into()]);
        worlds.listing_for_test(list);
        let signed_in = |id: &str, name: &str, revision: u64| {
            let mut snap = crate::state::Snapshot::new(orbiters_core::settings::Settings::default());
            snap.vrchat_login.allowed = true;
            snap.vrchat_login.revision = revision;
            snap.vrchat_login.user = Some(orbiters_core::vrchat_api::User { id: id.into(), display_name: name.into(), ..Default::default() });
            snap
        };
        worlds.follow_login(&signed_in("usr_a", "Alice", 1));
        worlds.open = true;
        worlds.set_shown(true);
        worlds.panel.page = panel::Page::Detail;
        worlds.panel.selected = Some(1);
        worlds.keep_picked(&worlds.entries());
        worlds.follow_screen_live();
        assert!(worlds.live.is_some() && worlds.picked().is_some(), "Alice sees her capture turning");
        let before = worlds.paint_key();
        for (id, name, revision) in [("usr_b", "Bob", 2), ("", "", 3)] {
            let snap = if id.is_empty() { let mut s = signed_in("x", "x", revision); s.vrchat_login.user = None; s } else { signed_in(id, name, revision) };
            worlds.panel.page = panel::Page::Detail;
            worlds.panel.selected = Some(1);
            worlds.picked = worlds.catalog.snapshot().0.into_iter().find(|e| e.id == 1);
            worlds.follow_login(&snap);
            worlds.follow_screen_live();
            assert!(worlds.live.is_none() && worlds.panel.live.is_none(), "{name:?}: nothing of hers turns");
            assert!(worlds.picked().is_none() && worlds.panel.selected.is_none() && worlds.panel.page == panel::Page::Browse, "{name:?}: her page is gone");
            assert!(!worlds.entries().iter().any(|e| e.id == 1), "{name:?}: not listed");
            assert_ne!(worlds.paint_key(), before, "{name:?}: never the picture painted for her");
        }
        // Kept from before by any other way, it is still not shown to someone it is not shared with.
        worlds.picked = worlds.catalog.snapshot().0.into_iter().find(|e| e.id == 1);
        worlds.panel.selected = Some(1);
        assert!(worlds.picked().is_none());
    }

    /// K-small-stick: a laser on the capture lying in the hole is where the stick moves in it; the sheet's panel says so for the loop to take the stick.
    #[test]
    fn the_sheet_says_where_the_stick_moves_in_the_capture() {
        let worlds = on_a_sheet(3);
        let hole = worlds.small_hole().expect("its hole");
        assert_eq!(worlds.stick_spot(hole.center().x, hole.center().y), Some(panel::StickSpot::InCapture));
        assert_eq!(worlds.stick_spot(200.0, 300.0), None);
    }
}
