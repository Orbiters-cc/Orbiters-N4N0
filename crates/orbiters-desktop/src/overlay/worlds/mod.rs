//! 3D worlds in the play space: search the worlds members captured and shared on Orbiters (the listing the VRChat
//! world "N4N0 - 3D picture viewer" shows) from N4N0's menu overlay, and see one around you in SteamVR, placed as that
//! world places it (its ground on your floor, its start view where you stand, facing your way) and drawn from the same
//! files (docs/world-view-overlay.md).
//!
//! A pill under N4N0's menu ("3D worlds", or the capture shown) opens the panel ([`panel`]) in the menu's place; the
//! menu's Back comes back to it. The capture stays around you once the menu closes; its page (Hide, Recenter, See
//! VRChat, sharpness) is a press on the pill away.

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
use crate::state::Snapshot;
use catalog::{Capture, Catalog, Entry, Progress, Source};
use egui::TextureId;
use orbiters_openvr::{self as vr, overlay_flag, Matrix34};
use orbiters_orb::menu::KeyboardLayout;
use panel::{Action, Panel, Status, CHIP_PX};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use view::View;

/// The pill's width in the play space, and its gap under the menu.
const CHIP_WIDTH_M: f32 = 0.2;
const CHIP_GAP_M: f32 = 0.012;

/// A capture on its way: its listing id, the download's progress, the answer.
struct Download {
    id: u64,
    progress: Arc<Progress>,
    answer: mpsc::Receiver<anyhow::Result<Capture>>,
}

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
    chip: vr::OverlayHandle,
    chip_canvas: Canvas,
    chip_shown: bool,
    chip_hover: bool,
    chip_pressed: bool,
    chip_drawn: Option<(bool, bool, Option<u64>, bool, bool)>,
    chip_placed: Option<[f32; 3]>,
    infos: crate::worlds::Infos,
    pictures: crate::worlds::Pictures,
    ctx: egui::Context,
    epoch: Instant,
    keys_seen: Option<u32>,
    last_paint: Option<Instant>,
}

impl Worlds {
    /// The pill's overlay; the panel draws on the menu's canvas, whose context `ctx` is.
    pub fn new(overlay: &vr::Overlay, key: &str, ctx: egui::Context) -> anyhow::Result<Self> {
        let chip = overlay.create(&format!("{key}.worlds-chip"), "N4N0's 3D worlds").map_err(|c| anyhow::anyhow!("3D worlds pill: SteamVR overlay error {c}"))?;
        let _ = overlay.set_width_in_meters(chip, CHIP_WIDTH_M);
        let _ = overlay.set_mouse_input(chip);
        let _ = overlay.set_mouse_scale(chip, CHIP_PX[0] as f32, CHIP_PX[1] as f32);
        let _ = overlay.set_flag(chip, overlay_flag::MAKE_OVERLAYS_INTERACTIVE_IF_VISIBLE, true);
        let _ = overlay.set_flag(chip, overlay_flag::ENABLE_CLICK_STABILIZATION, true);
        let _ = overlay.set_flag(chip, overlay_flag::IS_PREMULTIPLIED, true);
        let _ = overlay.set_sort_order(chip, 10);
        Ok(Self {
            open: false,
            panel: Panel::default(),
            catalog: Catalog::default(),
            source: Source::current(),
            view: View::new(key),
            download: None,
            current: None,
            error: None,
            chip,
            chip_canvas: Canvas::new(CHIP_PX[0], CHIP_PX[1]),
            chip_shown: false,
            chip_hover: false,
            chip_pressed: false,
            chip_drawn: None,
            chip_placed: None,
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
    }

    fn time(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    /// What the panel shows of the capture view.
    fn status(&self) -> Status {
        let (_, fetching, listing_error, _) = self.catalog.snapshot();
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

    /// One turn: the capture view, its download, the pill under the menu (`menu_at`: the menu's place while it shows
    /// its front, the panel closed or not). Returns whether the pill was pressed (the host opens the panel then) and
    /// whether a press happened (for the click sound).
    pub(super) fn update(&mut self, overlay: &vr::Overlay, steamvr: &vr::Client, presenter: &mut Presenter, snap: &Snapshot, menu_at: Option<&Matrix34>) -> (bool, bool) {
        let _ = snap;
        self.finish_download(overlay, steamvr);
        self.view.update(overlay, steamvr);
        if self.current.is_some() && self.download.is_none() && !self.view.active() {
            // Hidden (or failed): nothing is current any more.
            self.current = None;
            self.panel.dirty = true;
        }
        // The pill: under the menu while the menu shows (and the panel does not).
        let show_chip = menu_at.is_some() && !self.open;
        let mut opened = false;
        let mut clicked = false;
        while let Some(e) = overlay.poll_event(self.chip) {
            if !show_chip {
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
        if !show_chip {
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
            let menu_height = super::menu::WIDTH_M * orbiters_orb::menu::layout::HEIGHT as f32 / orbiters_orb::menu::layout::WIDTH as f32;
            let down = menu_height / 2.0 + CHIP_GAP_M + CHIP_WIDTH_M * CHIP_PX[1] as f32 / CHIP_PX[0] as f32 / 2.0;
            let mut t = *at;
            for i in 0..3 {
                t.m[i][3] -= at.m[i][1] * down;
            }
            let _ = overlay.set_transform_absolute(self.chip, &t);
            self.chip_placed = Some(origin);
        }
        let status = self.status();
        let loading = status.current.is_some() && !status.shown;
        let look = (self.chip_hover, self.chip_pressed, status.current, status.shown, loading);
        if self.chip_drawn != Some(look) || loading || !self.chip_shown {
            if !self.chip_shown {
                self.chip_canvas.invalidate();
            }
            self.chip_drawn = Some(look);
            let time = self.time();
            let name = self.current.as_ref().map(|e| e.name.clone());
            let (hover, pressed) = (self.chip_hover, self.chip_pressed);
            self.chip_canvas.paint(time, |p| panel::draw_chip(p, &status, name.as_deref(), hover, pressed, time));
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

    /// The download's answer: the capture goes to the view.
    fn finish_download(&mut self, overlay: &vr::Overlay, steamvr: &vr::Client) {
        let Some(d) = &self.download else { return };
        let answer = match d.answer.try_recv() {
            Ok(a) => a,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(anyhow::anyhow!("the download stopped")),
        };
        let id = d.id;
        self.download = None;
        self.panel.dirty = true;
        if self.current.as_ref().map(|e| e.id) != Some(id) {
            return;
        }
        match answer {
            Ok(capture) => self.view.show(overlay, steamvr, Arc::new(capture)),
            Err(e) => {
                eprintln!("3D worlds: {e:#}");
                self.error = Some(short_error(&e));
            }
        }
    }

    /// A laser event on the menu's overlay while the panel shows. Returns whether it was a press.
    pub fn mouse(&mut self, e: &vr::Event, overlay: &vr::Overlay, steamvr: &vr::Client) -> bool {
        let (entries, ..) = self.catalog.snapshot();
        let status = self.status();
        let time = self.time();
        let height = panel::HEIGHT;
        match e.event_type {
            vr::event::MOUSE_MOVE => {
                let (x, y, _) = e.mouse();
                let hover = self.panel.hit(&entries, &status, x, height - y);
                if hover != self.panel.hover {
                    self.panel.hover = hover;
                    self.panel.dirty = true;
                }
                false
            }
            vr::event::MOUSE_BUTTON_DOWN => {
                let (x, y, _) = e.mouse();
                let Some(target) = self.panel.hit(&entries, &status, x, height - y) else { return false };
                // The press shows at once; what it does follows.
                self.panel.pressed = Some(target);
                let actions = self.panel.press(target, &entries, &status, time);
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

    /// The computer's keys typed while the panel types (numbered: only the new ones count).
    pub fn keys(&mut self, keys: &[(u32, orbiters_orb::menu::KeyInput)]) {
        let newest = keys.last().map_or(0, |(n, _)| *n);
        let Some(seen) = self.keys_seen.replace(newest) else { return };
        if !self.open {
            return;
        }
        let time = self.time();
        for (n, key) in keys {
            if n.wrapping_sub(seen) as i32 > 0 {
                let _ = self.panel.key(key, time);
            }
        }
    }

    fn act(&mut self, actions: Vec<Action>, entries: &[Entry], overlay: &vr::Overlay, steamvr: &vr::Client) {
        for action in actions {
            match action {
                Action::Show(id) => {
                    let Some(entry) = entries.iter().find(|e| e.id == id).cloned() else { continue };
                    self.error = None;
                    self.view.error = None;
                    // The render device and its shaders get ready while the capture downloads.
                    self.view.warm_up(steamvr);
                    let progress = Arc::new(Progress::default());
                    let (tx, rx) = mpsc::channel();
                    let (source, p, e) = (self.source.clone(), progress.clone(), entry.clone());
                    let _ = std::thread::Builder::new().name("3d-worlds-download".into()).spawn(move || {
                        let _ = tx.send(source.download(&e, &p));
                    });
                    self.download = Some(Download { id, progress, answer: rx });
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
            }
        }
        let _ = overlay;
        self.panel.dirty = true;
    }

    /// Whether the panel should be painted now (something changed or moves), at most 45 times a second.
    pub fn wants_paint(&self) -> bool {
        let due = self.last_paint.is_none_or(|t| t.elapsed() >= Duration::from_millis(22));
        due && (self.panel.dirty || self.panel.animating(&self.status(), self.time()) || self.pictures.loading())
    }

    /// Paints the panel on the menu's canvas.
    pub fn paint(&mut self, canvas: &mut Canvas) {
        self.panel.dirty = false;
        self.last_paint = Some(Instant::now());
        let (entries, ..) = self.catalog.snapshot();
        let status = self.status();
        let thumbs = self.thumbnails(&entries);
        let time = self.time();
        let panel = &self.panel;
        let thumb = |e: &Entry| -> Option<TextureId> { thumbs.get(&e.id).copied() };
        canvas.paint(time, |p| panel.draw(p, &entries, &status, &thumb, time));
    }

    /// The thumbnails of the captures the panel shows now (their world's picture on VRChat, through the window's
    /// cache of world pages and pictures).
    fn thumbnails(&mut self, entries: &[Entry]) -> HashMap<u64, TextureId> {
        let shown: Vec<&Entry> = match self.panel.page {
            panel::Page::Detail => entries.iter().filter(|e| Some(e.id) == self.panel.selected).collect(),
            panel::Page::Typing => self.panel.results(entries).into_iter().take(panel::PER_ROW).collect(),
            panel::Page::Browse => self.panel.results(entries).into_iter().skip(self.panel.results_page * panel::PER_PAGE).take(panel::PER_PAGE).collect(),
        };
        let mut out = HashMap::new();
        for e in shown {
            let Some(crate::worlds::Info::Listed(page)) = self.infos.get(&e.world_id) else { continue };
            let Some(url) = page.world.thumbnail_url.clone().or_else(|| page.world.image_url.clone()) else { continue };
            let picture = crate::worlds::Picture::Remote { world_id: e.world_id.clone(), url, large: false };
            if let Some(t) = self.pictures.get(&self.ctx, &picture) {
                out.insert(e.id, t.id());
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
