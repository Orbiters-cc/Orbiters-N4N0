//! Freeing space in the library (the Sessions page): what the captures and 3D pictures take, and the clean-up that
//! deletes at once the capture files of the reconstructed ones (their 3D views stay) and, as a choice of its own, the
//! captures without a 3D view; the confirmation of a session's "Keep only the 3D view". Each says exactly what goes
//! before anything does. The service deletes (controller/cleanup.rs); what was asked shows as done at once
//! ([`Pending`]), and the library read afterwards says how it went.

use super::Job;
use crate::state::{CaptureState, CleanupJob, Snapshot};
use egui::{pos2, vec2, Align2, Id, Order, Painter, Rect, Sense, Ui, UiBuilder};
use orbiters_core::bundle::SessionSummary;
use orbiters_core::cleanup::{self, Part};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Variant};
use orbiters_ui::{format, icons};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What the library takes, as the clean-up sorts it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overview {
    pub total: u64,
    pub sessions: usize,
    /// Reconstructed sessions whose capture files can go, and what those files take.
    pub trim: Vec<PathBuf>,
    pub trim_bytes: u64,
    /// What the reconstructed sessions keep to be viewed (those already reduced to their 3D view too).
    pub views: usize,
    pub view_bytes: u64,
    /// Sessions without a 3D view (title and folder), and what they take.
    pub unviewed: Vec<(PathBuf, String)>,
    pub unviewed_bytes: u64,
    /// What sessions being recorded, reconstructed, waiting to be or being cleaned up hold beyond their views: in the
    /// total, in no choice.
    pub busy_bytes: u64,
}

/// Whether a session is in use: recorded, reconstructed, waiting to be, or being cleaned up.
pub fn busy(s: &SessionSummary, snap: &Snapshot) -> bool {
    let c = &snap.capture;
    (c.state != CaptureState::Idle && c.session_dir.as_ref() == Some(&s.dir))
        || snap.reconstruction.as_ref().is_some_and(|j| j.session_dir == s.dir)
        || snap.reconstruct_queue.contains(&s.dir)
        || snap.cleanup.as_ref().is_some_and(|j| j.of(&s.dir).is_some())
}

pub fn overview(sessions: &[SessionSummary], snap: &Snapshot) -> Overview {
    let mut o = Overview { sessions: sessions.len(), ..Default::default() };
    for s in sessions {
        o.total += s.size_bytes;
        let busy = busy(s, snap);
        if s.reconstruction.is_some() {
            let capture = s.capture_bytes.min(s.size_bytes);
            o.views += 1;
            o.view_bytes += s.size_bytes - capture;
            if busy {
                o.busy_bytes += capture;
            } else if capture > 0 {
                o.trim.push(s.dir.clone());
                o.trim_bytes += capture;
            }
        } else if busy {
            o.busy_bytes += s.size_bytes;
        } else {
            o.unviewed.push((s.dir.clone(), super::sessions::title(s)));
            o.unviewed_bytes += s.size_bytes;
        }
    }
    o
}

/// What the window asked of the clean-up, shown as done already: a session keeping only its 3D view is listed with it
/// alone, one going whole is gone. Once the service no longer has it to do, the library is read again and says how it
/// went (a session it could not clean shows as it was).
#[derive(Default)]
pub struct Pending {
    asked: HashMap<PathBuf, Asked>,
}

struct Asked {
    whole: bool,
    /// When it was asked (egui's clock).
    at: f64,
}

/// Before the service's answer arrives, a session out of its job is not taken for done.
const ANSWER_GRACE: f64 = 1.5;

impl Pending {
    pub fn ask(&mut self, trim: &[PathBuf], delete: &[PathBuf], now: f64) {
        for (dirs, whole) in [(trim, false), (delete, true)] {
            for dir in dirs {
                self.asked.insert(dir.clone(), Asked { whole, at: now });
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.asked.is_empty()
    }

    /// Whether the service is done with one asked (or turned it down): the library should be read again.
    pub fn settled(&self, job: Option<&CleanupJob>, now: f64) -> bool {
        self.asked.iter().any(|(dir, a)| !in_job(job, dir) && now - a.at > ANSWER_GRACE)
    }

    /// A list was read: what the service is done with is as the list says, what it still has to do shows as done.
    pub fn on_list(&mut self, list: &mut Vec<SessionSummary>, job: Option<&CleanupJob>, now: f64) {
        self.asked.retain(|dir, a| in_job(job, dir) || now - a.at <= ANSWER_GRACE);
        self.apply(list);
    }

    pub fn apply(&self, list: &mut Vec<SessionSummary>) {
        list.retain(|s| !self.asked.get(&s.dir).is_some_and(|a| a.whole));
        for s in list.iter_mut() {
            if self.asked.contains_key(&s.dir) && s.reconstruction.is_some() {
                s.size_bytes -= s.capture_bytes.min(s.size_bytes);
                s.capture_bytes = 0;
                s.trimmed = true;
            }
        }
    }
}

fn in_job(job: Option<&CleanupJob>, dir: &Path) -> bool {
    job.is_some_and(|j| j.of(dir).is_some())
}

/// The storage popover's state.
pub struct Popover {
    serial: u64,
    /// Shrinking back into its button (dropped once [`PopoverAction::Gone`]).
    pub closing: bool,
    /// The captures without a 3D view go too: a choice of its own, off when the popover opens.
    whole: bool,
    confirming: bool,
}

impl Popover {
    pub fn new(serial: u64) -> Self {
        Self { serial, closing: false, whole: false, confirming: false }
    }

    /// Opens on its confirmation, with the captures without a 3D view chosen too (screenshots).
    pub fn confirming(serial: u64) -> Self {
        Self { serial, closing: false, whole: true, confirming: true }
    }
}

pub enum PopoverAction {
    Close,
    /// Closed and back in its button: drop it.
    Gone,
    CleanUp {
        trim: Vec<PathBuf>,
        delete: Vec<PathBuf>,
    },
}

const POPOVER_W: f32 = 440.0;

/// Under the Sessions page's "Free up space" (`anchor`): what the captures and 3D pictures take, sorted as the clean-up
/// sorts them, the choice of the captures without a 3D view, then the confirmation that names what goes; while the
/// service deletes, how far it got. Closes on a click elsewhere.
pub fn popover(ctx: &egui::Context, snap: &Snapshot, p: &mut Popover, ov: &Overview, anchor: Rect) -> Option<PopoverAction> {
    let id = Id::unique(("storage-popover", p.serial));
    let job = snap.cleanup.as_ref();
    let whole = p.whole && !ov.unviewed.is_empty();
    let freeing = ov.trim_bytes + if whole { ov.unviewed_bytes } else { 0 };
    // Rows: the bar's legend (three), then the choice, then the button; or what goes and the buttons; or the job.
    let body_h = match (job, p.confirming) {
        (Some(_), _) => 90.0,
        (None, true) => 72.0 * (usize::from(ov.trim_bytes > 0) + usize::from(whole)) as f32 + 8.0 + note_height(ctx) + 20.0 + 40.0,
        (None, false) => 3.0 * 46.0 + 8.0 + if ov.unviewed.is_empty() { 0.0 } else { 64.0 } + 12.0 + 40.0,
    };
    let h = 20.0 + 60.0 + 26.0 + body_h + 20.0;
    let base = Rect::from_min_size(pos2(anchor.right() - POPOVER_W, anchor.bottom() + 8.0), vec2(POPOVER_W, h));
    // Grows from its top-right corner, and back into it.
    let pop = orbiters_ui::popup::popover(ctx, id, anchor, base, !p.closing);
    if pop.gone {
        return Some(PopoverAction::Gone);
    }
    let rect = pop.rect;
    let mut action = None;
    let area = egui::Area::new(id).order(Order::Foreground).fixed_pos(rect.min).interactable(!p.closing).show(ctx, |ui| {
        ui.set_min_size(rect.size());
        ui.multiply_opacity(pop.opacity);
        if p.closing {
            ui.disable();
        }
        let painter = ui.painter().clone();
        widgets::panel(&painter, rect, 16.0);
        let x = rect.left() + 20.0;
        let w = rect.width() - 40.0;
        widgets::text(&painter, pos2(x, rect.top() + 30.0), Align2::LEFT_CENTER, &Txt::new("Storage", 13.5).weight(weight::SEMIBOLD));
        widgets::text(
            &painter,
            pos2(rect.right() - 20.0, rect.top() + 30.0),
            Align2::RIGHT_CENTER,
            &Txt::new(format::bytes(ov.total as f64), 13.5).weight(weight::SEMIBOLD),
        );
        let free = snap.storage.free_bytes.map(|f| format!(" · {} free on the drive", format::bytes(f as f64))).unwrap_or_default();
        let sub = format!("{} of captures and 3D pictures{free}", plural(ov.sessions, "session", "sessions"));
        widgets::text_truncated(&painter, pos2(x, rect.top() + 50.0), Align2::LEFT_CENTER, &Txt::new(sub, 12.0).color(theme::FG_3), w);
        let bar = Rect::from_min_size(pos2(x, rect.top() + 72.0), vec2(w, 10.0));
        stacked_bar(ui, id.with("bar"), bar, ov, whole);
        let top = bar.bottom() + 24.0;
        if let Some(job) = job {
            running(ui, id, Rect::from_min_size(pos2(x, top), vec2(w, body_h)), job);
            return;
        }
        if p.confirming {
            let mut y = top;
            let unviewed: Vec<&str> = ov.unviewed.iter().map(|(_, t)| t.as_str()).collect();
            let trim_detail =
                format!("{}: frames, depth and sound. Their 3D views stay.", plural(ov.trim.len(), "reconstructed capture", "reconstructed captures"));
            if ov.trim_bytes > 0 {
                goes(&painter, Rect::from_min_size(pos2(x, y), vec2(w, 64.0)), icons::BROOM, "Capture files", &trim_detail, ov.trim_bytes);
                y += 72.0;
            }
            if whole {
                let names = names(&unviewed);
                let label = format!("{} without a 3D view, whole", plural(ov.unviewed.len(), "capture", "captures"));
                goes(&painter, Rect::from_min_size(pos2(x, y), vec2(w, 64.0)), icons::TRASH, &label, &names, ov.unviewed_bytes);
                y += 72.0;
            }
            y += 8.0;
            y += note(&painter, pos2(x, y), w) + 20.0;
            let cancel = Rect::from_min_size(pos2(x, y), vec2((w - 8.0) * 0.4, 40.0));
            let confirm = Rect::from_min_max(pos2(cancel.right() + 8.0, y), pos2(x + w, y + 40.0));
            if Button::new("Cancel").variant(Variant::Ghost).width(cancel.width()).show_at(ui, cancel).clicked() {
                p.confirming = false;
            }
            let label = format!("Delete {}", format::bytes(freeing as f64));
            if Button::new(&label).icon(icons::TRASH).variant(Variant::Danger).width(confirm.width()).id_salt("storage-delete").show_at(ui, confirm).clicked() {
                let delete = if whole { ov.unviewed.iter().map(|(d, _)| d.clone()).collect() } else { Vec::new() };
                action = Some(PopoverAction::CleanUp { trim: ov.trim.clone(), delete });
            }
            return;
        }
        // What each part is, counted; a part with nothing in it says so.
        let rows = [
            match ov.trim.len() {
                0 => (theme::ACCENT, "Capture files that can go".to_string(), "None now", 0),
                n => (
                    theme::ACCENT,
                    format!("Capture files of {}", plural(n, "reconstructed capture", "reconstructed captures")),
                    "Frames, depth and sound: only reconstructing again needs them",
                    ov.trim_bytes,
                ),
            },
            match ov.unviewed.len() {
                0 => (theme::WARN, "Captures without a 3D view".to_string(), "None", 0),
                n => (
                    theme::WARN,
                    format!("{} without a 3D view", plural(n, "capture", "captures")),
                    "Not reconstructed: they go whole, only if you choose",
                    ov.unviewed_bytes,
                ),
            },
            // Sessions in use (recorded, reconstructed) stay with the views for now.
            match (ov.views, ov.busy_bytes) {
                (0, 0) => (theme::FG_4, "3D views".to_string(), "None yet", 0),
                (0, busy) => (theme::FG_4, "In use".to_string(), "Being recorded or reconstructed: they stay", busy),
                (n, _) => (
                    theme::FG_4,
                    format!("3D views of {}", plural(n, "capture", "captures")),
                    "Splats, previews and camera paths: they stay",
                    ov.view_bytes + ov.busy_bytes,
                ),
            },
        ];
        for (i, (color, label, detail, bytes)) in rows.iter().enumerate() {
            let r = Rect::from_min_size(pos2(x, top + i as f32 * 46.0), vec2(w, 40.0));
            painter.circle_filled(pos2(r.left() + 5.0, r.top() + 11.0), 4.5, *color);
            widgets::text_truncated(
                &painter,
                pos2(r.left() + 18.0, r.top() + 11.0),
                Align2::LEFT_CENTER,
                &Txt::new(label, 13.0).weight(weight::MEDIUM),
                r.width() - 110.0,
            );
            widgets::text_truncated(
                &painter,
                pos2(r.left() + 18.0, r.top() + 30.0),
                Align2::LEFT_CENTER,
                &Txt::new(*detail, 11.5).color(theme::FG_3),
                r.width() - 18.0,
            );
            widgets::text(
                &painter,
                pos2(r.right(), r.top() + 11.0),
                Align2::RIGHT_CENTER,
                &Txt::new(format::bytes(*bytes as f64), 13.0).weight(weight::MEDIUM),
            );
        }
        let mut y = top + 3.0 * 46.0 + 8.0;
        if !ov.unviewed.is_empty() {
            widgets::separator(&mut ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(pos2(x, y - 6.0), vec2(w, 1.0)))));
            let row = Rect::from_min_size(pos2(x, y + 6.0), vec2(w, 48.0));
            let label = format!("Also delete the {} without a 3D view", plural(ov.unviewed.len(), "capture", "captures"));
            widgets::text_truncated(&painter, pos2(row.left(), row.top() + 14.0), Align2::LEFT_CENTER, &Txt::new(label, 13.0).weight(weight::MEDIUM), w - 64.0);
            widgets::text_truncated(
                &painter,
                pos2(row.left(), row.top() + 33.0),
                Align2::LEFT_CENTER,
                &Txt::new(format!("{}: nothing of them stays", format::bytes(ov.unviewed_bytes as f64)), 11.5).color(theme::FG_3),
                w - 64.0,
            );
            let mut sw = ui.new_child(
                UiBuilder::new().max_rect(Rect::from_min_size(pos2(row.right() - 48.0, row.center().y - 14.0), vec2(48.0, 28.0))).id_salt("storage-whole"),
            );
            widgets::toggle(&mut sw, "storage-whole", &mut p.whole);
            y += 64.0;
        }
        let button = Rect::from_min_size(pos2(x, y + 12.0), vec2(w, 40.0));
        let label = if freeing > 0 { format!("Free {}", format::bytes(freeing as f64)) } else { "Nothing to free".to_string() };
        let mut free_button = Button::new(&label).icon(icons::BROOM).variant(Variant::Primary).enabled(freeing > 0).width(w).id_salt("storage-free");
        if freeing == 0 {
            free_button = free_button.hint(if ov.unviewed.is_empty() {
                "No reconstructed capture has capture files left"
            } else {
                "No reconstructed capture has capture files left: the captures without a 3D view can go, if you choose them above"
            });
        }
        if free_button.show_at(ui, button).clicked() {
            p.confirming = true;
        }
    });
    if area.response.clicked_elsewhere() && !ctx.input(|i| i.pointer.interact_pos().is_some_and(|p| anchor.contains(p))) {
        action = action.or(Some(PopoverAction::Close));
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        if p.confirming && job.is_none() {
            p.confirming = false;
        } else {
            action = action.or(Some(PopoverAction::Close));
        }
    }
    if matches!(action, Some(PopoverAction::CleanUp { .. })) {
        p.confirming = false;
        p.whole = false;
    }
    action
}

/// The library as one bar: what the clean-up frees (the captures without a 3D view only when chosen, otherwise dimmed),
/// then what stays.
fn stacked_bar(ui: &Ui, id: Id, rect: Rect, ov: &Overview, whole: bool) {
    let p = ui.painter();
    p.rect_filled(rect, theme::radius(rect.height() * 0.5), theme::white(0.07));
    let total = ov.total.max(1) as f32;
    let k = anim::spring_from(ui.ctx(), id, [0.0], [1.0], Spring::SOFT)[0].clamp(0.0, 1.0);
    let unviewed = theme::alpha(theme::WARN, if whole { 1.0 } else { 0.45 });
    let mut x = rect.left();
    let clip = p.with_clip_rect(rect.intersect(p.clip_rect()));
    for (bytes, color) in [(ov.trim_bytes, theme::ACCENT), (ov.unviewed_bytes, unviewed), (ov.view_bytes + ov.busy_bytes, theme::INK_600)] {
        let w = rect.width() * bytes as f32 / total * k;
        if w < 0.5 {
            continue;
        }
        // Each part rounded, a hairline of track between them.
        let part = Rect::from_min_size(pos2(x, rect.top()), vec2(w, rect.height()));
        clip.rect_filled(part.shrink2(vec2(0.75, 0.0)), theme::radius(rect.height() * 0.5), color);
        x += w;
    }
}

/// While the service deletes: how much it freed of what it frees, and what is left.
fn running(ui: &Ui, id: Id, rect: Rect, job: &CleanupJob) {
    let p = ui.painter();
    let left = job.trim.len() + job.delete.len() + usize::from(job.current.is_some());
    widgets::text(p, pos2(rect.left(), rect.top() + 10.0), Align2::LEFT_CENTER, &Txt::new("Freeing space…", 13.0).weight(weight::MEDIUM));
    let of = if job.total > 0 { format!(" of {}", format::bytes(job.total as f64)) } else { String::new() };
    widgets::text(
        p,
        pos2(rect.right(), rect.top() + 10.0),
        Align2::RIGHT_CENTER,
        &Txt::new(format!("{}{of}", format::bytes(job.freed as f64)), 13.0).color(theme::FG_2),
    );
    let bar = Rect::from_min_size(pos2(rect.left(), rect.top() + 32.0), vec2(rect.width(), 6.0));
    let f = anim::spring_from(ui.ctx(), id.with("job"), [0.0], [job.fraction()], Spring::SOFT)[0].clamp(0.0, 1.0);
    p.rect_filled(bar, theme::radius(3.0), theme::white(0.08));
    p.rect_filled(Rect::from_min_size(bar.min, vec2((bar.width() * f).max(6.0), 6.0)), theme::radius(3.0), theme::ACCENT);
    let doing = match &job.current {
        Some((dir, true)) => format!("Deleting {}", format::folder_name(dir)),
        Some((dir, false)) => format!("Keeping only the 3D view of {}", format::folder_name(dir)),
        None => "Starting".to_string(),
    };
    widgets::text_truncated(p, pos2(rect.left(), rect.top() + 58.0), Align2::LEFT_CENTER, &Txt::new(doing, 12.0).color(theme::FG_3), rect.width());
    widgets::text_truncated(
        p,
        pos2(rect.left(), rect.top() + 80.0),
        Align2::LEFT_CENTER,
        &Txt::new(format!("{} · it goes on with the window closed", plural(left, "session left", "sessions left")), 12.0).color(theme::FG_4),
        rect.width(),
    );
}

/// One thing a confirmation deletes: a tinted tile, what it is, its detail under it (two lines at most), its size.
fn goes(p: &Painter, rect: Rect, glyph: &str, label: &str, detail: &str, bytes: u64) {
    widgets::frame(p, rect, theme::TILE_RADIUS, theme::surface_soft());
    let tile = Rect::from_min_size(pos2(rect.left() + 12.0, rect.center().y - 18.0), vec2(36.0, 36.0));
    p.rect_filled(tile, theme::radius(11.0), theme::alpha(theme::DANGER, 0.12));
    widgets::icon(p, tile.center(), glyph, 16.0, theme::DANGER);
    let x = tile.right() + 12.0;
    let mut job = Txt::new(detail, 11.5).color(theme::FG_3).line_height(16.0).job(rect.right() - 14.0 - x);
    job.wrap.max_rows = 2;
    let g = p.layout_job(job);
    // The label over its detail, the two centred in the row; the size on the label's line.
    let top = rect.center().y - (18.0 + g.size().y) * 0.5;
    let size = Txt::new(format::bytes(bytes as f64), 13.0).weight(weight::SEMIBOLD);
    let size_r = widgets::text(p, pos2(rect.right() - 14.0, top + 9.0), Align2::RIGHT_CENTER, &size);
    widgets::text_truncated(p, pos2(x, top + 9.0), Align2::LEFT_CENTER, &Txt::new(label, 13.0).weight(weight::MEDIUM), size_r.left() - x - 12.0);
    p.galley(pos2(x, top + 18.0), g, theme::FG_3);
}

const NOTE: &str = "Deleted for good: not moved to the recycle bin, which would free nothing. This cannot be undone.";

fn note_height(ctx: &egui::Context) -> f32 {
    ctx.fonts_mut(|f| f.layout_job(Txt::new(NOTE, 12.0).color(theme::FG_3).line_height(18.0).job(POPOVER_W - 40.0)).size().y)
}

/// The note every confirmation ends with; returns its height.
fn note(p: &Painter, at: egui::Pos2, width: f32) -> f32 {
    let g = p.layout_job(Txt::new(NOTE, 12.0).color(theme::FG_3).line_height(18.0).job(width));
    let h = g.size().y;
    p.galley(at, g, theme::FG_3);
    h
}

/// "Palm Point - Night, VRChat Home ×3, Test pattern": titles once each, counted.
fn names(titles: &[&str]) -> String {
    let mut counted: Vec<(&str, usize)> = Vec::new();
    for t in titles {
        match counted.iter_mut().find(|(n, _)| n == t) {
            Some((_, c)) => *c += 1,
            None => counted.push((t, 1)),
        }
    }
    counted.iter().map(|(n, c)| if *c > 1 { format!("{n} ×{c}") } else { n.to_string() }).collect::<Vec<_>>().join(", ")
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The confirmation of a session's "Keep only the 3D view", over its detail.
pub struct TrimDialog {
    files: Job<Vec<cleanup::Entry>>,
    serial: u64,
}

impl TrimDialog {
    /// Reads what would go (off the interface thread) as it opens.
    pub fn new(ctx: &egui::Context, dir: PathBuf, serial: u64) -> Self {
        Self { files: Job::spawn(ctx, move || cleanup::capture_files(&dir)), serial }
    }

    pub fn loading(&self) -> bool {
        self.files.running()
    }
}

/// The parts of a session's capture files as the dialog lists them: what, how many, how big.
fn parts(files: &[cleanup::Entry]) -> Vec<(Part, u64, u64)> {
    let mut parts: Vec<(Part, u64, u64)> = Vec::new();
    for e in files {
        match parts.iter_mut().find(|(p, ..)| *p == e.part) {
            Some((_, count, bytes)) => {
                *count += e.files;
                *bytes += e.bytes;
            }
            None => parts.push((e.part, e.files, e.bytes)),
        }
    }
    parts.sort_by_key(|(p, ..)| *p);
    parts
}

fn part_words(part: Part, count: u64) -> (&'static str, String, &'static str) {
    let n = format::integer(count as f64);
    match part {
        Part::Keyframes => ("Keyframes", format!("{n} lossless pictures"), icons::CUBE),
        Part::Depth => ("Depth frames", format!("{n} files"), icons::CPU),
        Part::Video => ("Video", "the frames between the keyframes".into(), icons::FILM_STRIP),
        Part::Performance => ("Sound, parameters and tracking", format!("{n} files"), icons::WAVEFORM),
        Part::Leftovers => ("Unfinished reconstructions", "runs cut short, the result before the last".into(), icons::STACK),
    }
}

/// Over the session's detail (`over`, its whole card, which it dims and keeps from clicks): what keeping only the 3D
/// view deletes, part by part, and Cancel / Delete. `Some(true)` once confirmed, `Some(false)` when cancelled.
pub fn trim_dialog(ui: &mut Ui, d: &mut TrimDialog, over: Rect, capture_bytes: u64) -> Option<bool> {
    let ctx = ui.ctx().clone();
    let id = Id::unique(("trim-dialog", d.serial));
    // Clicks on the detail under it stop here.
    let shield = ui.interact(over, id.with("shield"), Sense::click());
    let k = anim::spring_from(&ctx, id.with("in"), [0.0], [1.0], Spring::BOUNCY)[0];
    let parts = d.files.poll().map(|f| parts(f));
    let bytes = parts.as_ref().map_or(capture_bytes, |p| p.iter().map(|(_, _, b)| b).sum());
    let rows = parts.as_ref().map_or(2, |p| p.len().max(1));
    let w = 420.0f32.min(over.width() - 48.0);
    let note_h = ui.painter().layout_job(Txt::new(NOTE, 12.0).line_height(18.0).job(w - 48.0)).size().y;
    let lead = "The splats, their preview and the camera path stay: it can still be viewed, sent to Mixer and shared, not reconstructed again.";
    let lead_h = ui.painter().layout_job(Txt::new(lead, 12.5).line_height(19.0).job(w - 48.0)).size().y;
    let h = 24.0 + 36.0 + 14.0 + lead_h + 16.0 + rows as f32 * 52.0 + 4.0 + note_h + 20.0 + 40.0 + 24.0;
    let p = ui.painter().clone();
    p.with_clip_rect(over).rect_filled(over, theme::radius(28.0), theme::alpha(theme::INK_950, 0.55 * k.clamp(0.0, 1.0)));
    let base = Rect::from_center_size(over.center(), vec2(w, h));
    let dialog = Rect::from_center_size(base.center() + vec2(0.0, 16.0 * (1.0 - k)), base.size() * (0.94 + 0.06 * k));
    let mut dp = p.clone();
    dp.multiply_opacity(k.clamp(0.0, 1.0));
    widgets::panel(&dp, dialog, 24.0);
    let tile = Rect::from_min_size(dialog.min + vec2(24.0, 24.0), vec2(36.0, 36.0));
    dp.rect_filled(tile, theme::radius(12.0), theme::alpha(theme::ACCENT, 0.12));
    widgets::icon(&dp, tile.center(), icons::BROOM, 18.0, theme::ACCENT);
    widgets::text(&dp, pos2(tile.right() + 12.0, tile.center().y), Align2::LEFT_CENTER, &Txt::new("Keep only the 3D view?", 16.0).weight(weight::SEMIBOLD));
    let x = dialog.left() + 24.0;
    let iw = dialog.width() - 48.0;
    let mut y = tile.bottom() + 14.0;
    let g = dp.layout_job(Txt::new(lead, 12.5).color(theme::FG_2).line_height(19.0).job(iw));
    dp.galley(pos2(x, y), g, theme::FG_2);
    y += lead_h + 16.0;
    match &parts {
        Some(parts) if !parts.is_empty() => {
            for (part, count, bytes) in parts {
                let (label, detail, glyph) = part_words(*part, *count);
                goes(&dp, Rect::from_min_size(pos2(x, y), vec2(iw, 46.0)), glyph, label, &detail, *bytes);
                y += 52.0;
            }
        }
        Some(_) => {
            widgets::text(&dp, pos2(x, y + 20.0), Align2::LEFT_CENTER, &Txt::new("No capture files are left in this session.", 12.5).color(theme::FG_3));
            y += 52.0;
        }
        None => {
            for _ in 0..rows {
                widgets::frame(&dp, Rect::from_min_size(pos2(x, y), vec2(iw, 46.0)), theme::TILE_RADIUS, theme::surface_soft());
                y += 52.0;
            }
        }
    }
    y += 4.0;
    note(&dp, pos2(x, y), iw);
    y += note_h + 20.0;
    let mut bu = ui.new_child(UiBuilder::new().max_rect(dialog).id_salt(("trim-dialog-buttons", d.serial)));
    bu.multiply_opacity(k.clamp(0.0, 1.0));
    let cancel = Rect::from_min_size(pos2(x, y), vec2((iw - 8.0) * 0.4, 40.0));
    let confirm = Rect::from_min_max(pos2(cancel.right() + 8.0, y), pos2(x + iw, y + 40.0));
    let mut answer = None;
    if Button::new("Cancel").variant(Variant::Ghost).width(cancel.width()).id_salt("trim-cancel").show_at(&mut bu, cancel).clicked() {
        answer = Some(false);
    }
    let label = format!("Delete {}", format::bytes(bytes as f64));
    let ready = parts.as_ref().is_some_and(|p| !p.is_empty());
    if Button::new(&label)
        .icon(icons::TRASH)
        .variant(Variant::Danger)
        .enabled(ready)
        .width(confirm.width())
        .id_salt("trim-delete")
        .show_at(&mut bu, confirm)
        .clicked()
    {
        answer = Some(true);
    }
    // A click outside the dialog, or Escape, is a cancel.
    if shield.clicked() && !dialog.contains(shield.interact_pointer_pos().unwrap_or_default()) {
        answer = answer.or(Some(false));
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = answer.or(Some(false));
    }
    answer
}

/// The small "3D view only" line of a session reduced to its 3D view (its detail's first action, which reconstructs
/// a session that still has its frames).
pub fn view_only(p: &Painter, rect: Rect, cleaning: bool) {
    p.rect_filled(rect, theme::radius(rect.height() * 0.5), theme::surface_soft());
    let (glyph, text) = if cleaning { (icons::BROOM, "Deleting its capture files…") } else { (icons::CUBE_FOCUS, "3D view only: capture files deleted") };
    let t = Txt::new(text, 13.0).weight(weight::MEDIUM).color(theme::FG_3);
    let tw = p.layout_job(t.job(f32::INFINITY)).size().x;
    let x = rect.center().x - (tw + 15.0 + 8.0) * 0.5;
    widgets::icon(p, pos2(x + 7.5, rect.center().y), glyph, 15.0, theme::FG_3);
    widgets::text(p, pos2(x + 23.0, rect.center().y), Align2::LEFT_CENTER, &t);
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_contracts::manifest::SessionManifest;
    use orbiters_core::reconstruction::Report;
    use orbiters_core::settings::Settings;

    fn session(name: &str, size: u64, capture: u64, reconstructed: bool) -> SessionSummary {
        let mut manifest = SessionManifest::default();
        manifest.world = Some(orbiters_contracts::manifest::WorldInfo { name: Some(name.into()), ..Default::default() });
        SessionSummary {
            dir: PathBuf::from(format!("C:/Sessions/{name} {size}")),
            manifest,
            has_thumbnail: true,
            size_bytes: size,
            capture_bytes: capture,
            trimmed: reconstructed && capture == 0,
            modified: 0,
            reconstruction: reconstructed.then(Report::default),
            damaged: None,
        }
    }

    #[test]
    fn the_library_is_sorted_as_the_clean_up_frees_it() {
        let list = vec![
            session("Silent Dawn", 900, 760, true),
            session("Palm Point", 400, 300, true),
            session("Home", 100, 0, true),
            session("Home", 50, 40, false),
            session("Home", 70, 60, false),
            session("Canine", 300, 200, true),
            session("Recording", 500, 500, false),
        ];
        let mut snap = Snapshot::new(Settings::default());
        // Canine is being reconstructed, the last one recorded: in the total, in no choice.
        snap.reconstruction = Some(crate::state::ReconstructionJob::new(list[5].dir.clone(), false));
        snap.capture.state = CaptureState::Recording;
        snap.capture.session_dir = Some(list[6].dir.clone());
        let o = overview(&list, &snap);
        assert_eq!((o.total, o.sessions), (2320, 7));
        assert_eq!((o.trim.len(), o.trim_bytes), (2, 1060));
        assert_eq!((o.views, o.view_bytes), (4, 140 + 100 + 100 + 100));
        assert_eq!(o.unviewed.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(), ["Home", "Home"]);
        assert_eq!(o.unviewed_bytes, 120);
        assert_eq!(o.busy_bytes, 200 + 500);
        assert_eq!(o.trim_bytes + o.unviewed_bytes + o.view_bytes + o.busy_bytes, o.total, "every byte is somewhere");
        assert_eq!(names(&["Palm Point", "Home", "Home", "Test", "Home"]), "Palm Point, Home ×3, Test");
    }

    #[test]
    fn what_was_asked_shows_as_done_until_the_library_says_how_it_went() {
        let list = vec![session("Viewed", 900, 760, true), session("Raw", 50, 40, false), session("Other", 70, 60, false)];
        let (viewed, raw) = (list[0].dir.clone(), list[1].dir.clone());
        let mut pending = Pending::default();
        pending.ask(std::slice::from_ref(&viewed), std::slice::from_ref(&raw), 10.0);
        let mut shown = list.clone();
        pending.apply(&mut shown);
        assert_eq!(shown.len(), 2, "the one going whole is gone at once");
        assert!(shown[0].trimmed && shown[0].size_bytes == 140 && shown[0].capture_bytes == 0);
        // Under way: a list read meanwhile (another change) still shows them done; nothing settled yet.
        let mut job = CleanupJob::default();
        job.add(std::slice::from_ref(&viewed), std::slice::from_ref(&raw));
        let mut read = list.clone();
        pending.on_list(&mut read, Some(&job), 20.0);
        assert_eq!(read.len(), 2);
        assert!(!pending.settled(Some(&job), 20.0));
        // Before the service answers, a job without them is not taken for an end.
        assert!(!pending.settled(None, 10.5));
        // The service is done with them: the library is read again, and that list is how things are (here, the raw
        // one could not be deleted).
        assert!(pending.settled(None, 20.0));
        let mut read = list.clone();
        read[0].trimmed = true;
        read[0].size_bytes = 140;
        read[0].capture_bytes = 0;
        pending.on_list(&mut read, None, 21.0);
        assert!(pending.is_empty());
        assert_eq!(read.len(), 3);
        assert!(!read[1].trimmed && read[1].size_bytes == 50);
    }
}
