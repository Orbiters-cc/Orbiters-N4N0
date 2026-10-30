//! 3D worlds in the play space: search the worlds members captured and shared on Orbiters (the listing the VRChat world
//! "N4N0 - 3D picture viewer" shows) from N4N0's menu overlay, and see one around you in SteamVR, placed as that
//! world places it (its ground on your floor, its start view where you stand, facing your way) and drawn from the same
//! files (docs/world-view-overlay.md).
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
mod diagnostics;
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
use crate::backend::Ctl;
use crate::ipc::OrbCommand;
use crate::state::Snapshot;
use catalog::{Capture, Catalog, Entry, Progress, Source};
use egui::TextureId;
use orbiters_openvr::{self as vr, overlay_flag, Matrix34};
use orbiters_orb::feedback::{CommentRow, FeedbackAsk, FeedbackView};
use orbiters_orb::menu::KeyboardLayout;
use panel::{Action, Data, Facts, Panel, Status, Target, BAR_PX, CHIP_PX};
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
    chip: vr::OverlayHandle,
    chip_canvas: Canvas,
    chip_shown: bool,
    chip_hover: bool,
    chip_pressed: bool,
    chip_drawn: Option<(bool, bool)>,
    chip_placed: Option<[f32; 3]>,
    bar: vr::OverlayHandle,
    bar_canvas: Canvas,
    bar_shown: bool,
    bar_hover: Option<Target>,
    bar_pressed: Option<Target>,
    bar_drawn: Option<BarLook>,
    bar_painted: Option<Instant>,
    bar_placed: Option<[f32; 3]>,
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
    /// The player's VRChat name (a capture shared with some players only is listed for them).
    me: Option<String>,
    infos: crate::worlds::Infos,
    pictures: crate::worlds::Pictures,
    ctx: egui::Context,
    epoch: Instant,
    keys_seen: Option<u32>,
    last_paint: Option<Instant>,
}

/// Now, in Unix seconds.
fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn interactive(overlay: &vr::Overlay, handle: vr::OverlayHandle, width_m: f32, px: [u32; 2]) {
    let _ = overlay.set_width_in_meters(handle, width_m);
    let _ = overlay.set_mouse_input(handle);
    let _ = overlay.set_mouse_scale(handle, px[0] as f32, px[1] as f32);
    let _ = overlay.set_flag(handle, overlay_flag::MAKE_OVERLAYS_INTERACTIVE_IF_VISIBLE, true);
    let _ = overlay.set_flag(handle, overlay_flag::ENABLE_CLICK_STABILIZATION, true);
    let _ = overlay.set_flag(handle, overlay_flag::IS_PREMULTIPLIED, true);
    let _ = overlay.set_sort_order(handle, 10);
}

/// The transform `height_m` tall hanging under the menu at `at`.
fn under_menu(at: &Matrix34, height_m: f32) -> Matrix34 {
    let menu_height = super::menu::WIDTH_M * orbiters_orb::menu::layout::HEIGHT as f32 / orbiters_orb::menu::layout::WIDTH as f32;
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
        let chip = overlay.create(&format!("{key}.worlds-chip"), "N4N0's 3D worlds").map_err(|c| anyhow::anyhow!("3D worlds pill: SteamVR overlay error {c}"))?;
        interactive(overlay, chip, CHIP_WIDTH_M, CHIP_PX);
        let bar = overlay.create(&format!("{key}.worlds-bar"), "N4N0's 3D world controls").map_err(|c| anyhow::anyhow!("3D worlds bar: SteamVR overlay error {c}"))?;
        interactive(overlay, bar, BAR_WIDTH_M, BAR_PX);
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
            me: None,
            infos: crate::worlds::Infos::new(&ctx),
            pictures: crate::worlds::Pictures::new(&ctx),
            ctx,
            epoch: Instant::now(),
            keys_seen: None,
            last_paint: None,
        })
    }

    pub fn destroy(&mut self, overlay: &vr::Overlay) {
        self.view.destroy(overlay);
        let _ = overlay.destroy(self.chip);
        let _ = overlay.destroy(self.bar);
    }

    fn time(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    /// The listing's captures this player may see.
    fn entries(&self) -> Vec<Entry> {
        let (entries, ..) = self.catalog.snapshot();
        entries.into_iter().filter(|e| e.for_player(self.me.as_deref())).collect()
    }

    /// What the panel shows of the capture view.
    fn status(&self) -> Status {
        let (fetching, listing_error) = self.catalog.state();
        let mut s = Status {
            current: self.current.as_ref().map(|e| e.id),
            quality: self.view.quality(),
            see_through: self.view.see_through,
            error: self.error.clone().or_else(|| self.view.error.clone()),
            fetching,
            listing_error,
            ..Default::default()
        };
        if let Some(d) = &self.download {
            let (read, total) = (d.progress.read.load(Ordering::Relaxed), d.progress.total.load(Ordering::Relaxed));
            if d.progress.stage.load(Ordering::Relaxed) == catalog::STAGE_PREPARING {
                s.stage = Some("Preparing the splats".into());
            } else {
                s.download = Some(d.progress.fraction());
                s.stage = Some(if total > 0 { format!("Downloading {:.1} of {:.1} MB", read as f64 / 1e6, total as f64 / 1e6) } else { "Downloading".into() });
            }
        } else if self.current.is_some() && self.view.active() {
            if self.view.preparing() {
                s.stage = Some("Getting your graphics card ready".into());
            } else {
                s.shown = true;
            }
        }
        if s.shown {
            let splats = self.current.as_ref().map_or(0, |e| e.splats as u64);
            let face = if self.view.batch_ms > 0.0 { format!(" · {:.1} ms a batch", self.view.batch_ms) } else { String::new() };
            s.stats = Some(format!("{} splats{face} · {} px", panel::short_count(splats), self.view.quality().pixels()));
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
        if snap.orb.feedback != self.feedback_seen {
            self.feedback_seen = snap.orb.feedback.clone();
            self.feedback = snap.orb.feedback.clone();
            self.panel.dirty = true;
        }
        let me = snap.vrchat_login.user.as_ref().map(|u| u.display_name.clone()).filter(|n| !n.trim().is_empty());
        if me != self.me {
            self.me = me;
            self.panel.dirty = true;
        }
        let deferred = std::mem::take(&mut self.deferred);
        if !deferred.is_empty() {
            let entries = self.entries();
            self.act(deferred, &entries, overlay, steamvr);
        }
        self.finish_download(overlay, steamvr);
        self.view.update(overlay, steamvr);
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
            backend.orb(OrbCommand::Feedback { ask });
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
        let at = menu_at.expect("checked above");
        let origin = stereo::position(at);
        if self.chip_placed != Some(origin) {
            let _ = overlay.set_transform_absolute(self.chip, &under_menu(at, CHIP_WIDTH_M * CHIP_PX[1] as f32 / CHIP_PX[0] as f32));
            self.chip_placed = Some(origin);
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
        let at = menu_at.expect("checked above");
        let origin = stereo::position(at);
        if self.bar_placed != Some(origin) {
            let _ = overlay.set_transform_absolute(self.bar, &under_menu(at, BAR_WIDTH_M * BAR_PX[1] as f32 / BAR_PX[0] as f32));
            self.bar_placed = Some(origin);
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
        let world = self.catalog.world_of(id);
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
            Err(mpsc::TryRecvError::Disconnected) => Err(anyhow::anyhow!("the download stopped")),
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
        let data = |feedback, facts| Data { entries: &entries, status: &status, feedback, facts, thumb: &no_thumb, picture: &no_picture, now: unix_now() };
        match e.event_type {
            vr::event::MOUSE_MOVE => {
                let (x, y, _) = e.mouse();
                let hover = self.panel.hit(&data(&self.feedback, &self.facts), x, height - y);
                if hover != self.panel.hover {
                    self.panel.hover = hover;
                    self.panel.dirty = true;
                }
                false
            }
            vr::event::MOUSE_BUTTON_DOWN => {
                let (x, y, _) = e.mouse();
                let d = data(&self.feedback, &self.facts);
                let Some(target) = self.panel.hit(&d, x, height - y) else { return false };
                // The press shows at once; what it does follows.
                self.panel.pressed = Some(target);
                let actions = self.panel.press(target, &d, time);
                self.act(actions, &entries, overlay, steamvr);
                true
            }
            vr::event::MOUSE_BUTTON_UP | vr::event::FOCUS_LEAVE => {
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
    pub fn keys(&mut self, keys: &[(u32, orbiters_orb::menu::KeyInput)]) {
        let newest = keys.last().map_or(0, |(n, _)| *n);
        let Some(seen) = self.keys_seen.replace(newest) else { return };
        if !self.open || !keys.iter().any(|(n, _)| n.wrapping_sub(seen) as i32 > 0) {
            return;
        }
        let entries = self.entries();
        let status = self.status();
        let time = self.time();
        let (no_thumb, no_picture) = (|_: &Entry| -> Option<TextureId> { None }, |_: u64, _: bool| -> Option<TextureId> { None });
        for (n, key) in keys {
            if n.wrapping_sub(seen) as i32 > 0 {
                let d = Data { entries: &entries, status: &status, feedback: &self.feedback, facts: &self.facts, thumb: &no_thumb, picture: &no_picture, now: unix_now() };
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
                    let Some(entry) = entries.iter().find(|e| e.id == id).cloned() else { continue };
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
                        let _ = tx.send(source.download(&e, &p));
                    });
                    self.download = Some(Download { id, key, progress, answer: rx });
                    self.current = Some(entry);
                }
                Action::Hide => {
                    self.download = None;
                    self.view.hide();
                    if !self.view.active() {
                        self.current = None;
                    }
                }
                Action::Recenter => self.view.recenter(steamvr),
                Action::SeeThrough(on) => self.view.see_through = on,
                Action::Quality(q) => self.view.set_quality(q),
                Action::Leave => self.open = false,
                Action::Refresh => self.catalog.refresh(&self.source),
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
                w.comments.insert(0, CommentRow { id: 0, by: "You".into(), text: text.clone(), at: unix_now(), mine: true });
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
        self.panel.dirty = false;
        self.last_paint = Some(Instant::now());
        let entries = self.entries();
        let status = self.status();
        // The worlds' pages the panel shows (all of them to order by frame rate).
        self.facts_pending = false;
        if self.panel.sort == panel::Sort::Smooth {
            for e in &entries {
                self.facts_for(&e.world_id);
            }
        }
        let visible = self.visible(&entries, &status);
        for (_, world) in &visible {
            self.facts_for(world);
        }
        let thumbs = self.thumbnails(&visible);
        let pictures = self.world_pictures(&entries);
        let time = self.time();
        let thumb = |e: &Entry| -> Option<TextureId> { thumbs.get(&e.id).copied() };
        let picture = |id: u64, large: bool| -> Option<TextureId> { pictures.get(&(id, large)).copied() };
        let d = Data { entries: &entries, status: &status, feedback: &self.feedback, facts: &self.facts, thumb: &thumb, picture: &picture, now: unix_now() };
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

    /// The captures whose pictures the panel shows now: (listing id, world id).
    fn visible(&self, entries: &[Entry], status: &Status) -> Vec<(u64, String)> {
        let (no_thumb, no_picture) = (|_: &Entry| -> Option<TextureId> { None }, |_: u64, _: bool| -> Option<TextureId> { None });
        let d = Data { entries, status, feedback: &self.feedback, facts: &self.facts, thumb: &no_thumb, picture: &no_picture, now: 0 };
        let shown: Vec<&Entry> = match self.panel.page {
            panel::Page::Typing => self.panel.results(&d).into_iter().take(panel::PER_ROW).collect(),
            panel::Page::Browse => self.panel.results(&d).into_iter().skip(self.panel.results_page * panel::PER_PAGE).take(panel::PER_PAGE).collect(),
            panel::Page::Filter => Vec::new(),
            _ => self.panel.entry(&d).into_iter().collect(),
        };
        shown.into_iter().map(|e| (e.id, e.world_id.clone())).collect()
    }

    /// The thumbnails of the captures the panel shows now (their world's picture on VRChat, through the window's
    /// cache of world pages and pictures).
    fn thumbnails(&mut self, visible: &[(u64, String)]) -> HashMap<u64, TextureId> {
        let mut out = HashMap::new();
        for (id, world) in visible {
            let Some(url) = self.facts.get(world).and_then(|f| f.thumbnail.clone()) else { continue };
            let picture = crate::worlds::Picture::Remote { world_id: world.clone(), url, large: false };
            if let Some(t) = self.pictures.get(&self.ctx, &picture) {
                out.insert(*id, t.id());
            }
        }
        out
    }

    /// The selected capture's world's pictures on the pictures page: the page's thumbnails, the one shown whole.
    fn world_pictures(&mut self, entries: &[Entry]) -> HashMap<(u64, bool), TextureId> {
        let mut out = HashMap::new();
        if self.panel.page != panel::Page::Pictures {
            return out;
        }
        let Some(e) = self.panel.selected.and_then(|id| entries.iter().find(|e| e.id == id)) else { return out };
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

/// A download's error in a line the panel can show.
fn short_error(e: &anyhow::Error) -> String {
    let text = format!("{e:#}");
    let text = if text.contains("replaced") {
        "This capture was replaced on Orbiters: go back and pick it again".to_string()
    } else if text.contains("answered 404") {
        "This capture is no longer on Orbiters".to_string()
    } else {
        format!("Could not download it: {text}")
    };
    text.chars().take(90).collect()
}
