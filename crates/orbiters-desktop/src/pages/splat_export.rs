//! Export…: a capture's splats as a file for other tools ([`crate::splat_export`]), in a dialog growing out of the button
//! or card that asked (the 3D view's top bar, the capture's window and card menu in Photos & captures, its detail in
//! Capture files). It says what the file holds (what the 3D view shows: hidden and deleted splats left out, no sky),
//! offers the three formats each with where it goes in one line and about how large, asks Windows' save dialog where
//! (the capture's name offered), shows how far the export got (closing stops it), then where it was saved with Show in
//! folder, or why not and the way on: Discard them for edits that cannot be applied (as the 3D view offers), Choose
//! another place for a full disk or a place N4N0 may not write in, Try again for anything else.

use super::Job;
use crate::splat_export::{self as export, Edits, Exported, Failure, Format, Progress, Step};
use egui::{Align2, CursorIcon, Id, Order, Rect, Sense, Ui, pos2, vec2};
use orbiters_i18n::tr;
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::dialog::{self, Dialog, Part};
use orbiters_ui::theme::{self, Txt, weight};
use orbiters_ui::widgets::{self, Button, Size, Tone, Variant};
use orbiters_ui::{feedback, format, icons};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// What Windows' save dialog is asked.
pub struct Ask {
    pub title: String,
    pub format: Format,
    /// The name offered, with its extension.
    pub name: String,
    /// Where it opens (Windows' own choice without one: the folder last used).
    pub folder: Option<PathBuf>,
}

/// What the dialog does outside the app: Windows' save dialog (run on a worker: it blocks until answered), Show in
/// folder, and the room left on a disk. Tests stand in for them.
#[derive(Clone, Copy)]
pub struct Hooks {
    pub pick: fn(Ask) -> Option<PathBuf>,
    pub reveal: fn(&Path) -> anyhow::Result<()>,
    pub free_space: fn(&Path) -> Option<u64>,
}

impl Default for Hooks {
    fn default() -> Self {
        Self { pick: save_dialog, reveal: super::editor::files::reveal, free_space: orbiters_projects::size::free_space }
    }
}

fn save_dialog(ask: Ask) -> Option<PathBuf> {
    let mut d = rfd::FileDialog::new().set_title(&ask.title).add_filter(ask.format.filter(), &[ask.format.extension()]).set_file_name(&ask.name);
    if let Some(folder) = ask.folder.filter(|f| f.is_dir()) {
        d = d.set_directory(folder);
    }
    d.save_file()
}

/// Stand-ins for [`Hooks`] every dialog opened from now on takes (screenshots: no Windows dialog in an offscreen run).
static STAGED: Mutex<Option<Hooks>> = Mutex::new(None);

/// Every dialog opened from now on uses `hooks` (screenshots).
pub fn stage(hooks: Hooks) {
    *STAGED.lock().unwrap_or_else(|e| e.into_inner()) = Some(hooks);
}

/// Where and as what the last export of this run of the app went: the next one opens there, on that format.
static LAST: Mutex<Option<(PathBuf, Format)>> = Mutex::new(None);

/// Every opening gets springs of its own.
static OPENED: AtomicU64 = AtomicU64::new(0);

fn last() -> Option<(PathBuf, Format)> {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// An export running: as what, how far, and its outcome to come.
struct Running {
    format: Format,
    progress: Arc<Progress>,
    job: Job<Result<Exported, Failure>>,
}

pub struct Export {
    id: Id,
    origin: Rect,
    order: Order,
    session: PathBuf,
    title: String,
    format: Format,
    edits: Job<Edits>,
    picking: Job<Option<PathBuf>>,
    running: Option<Running>,
    /// The last export's format and outcome, and where it was written (Try again writes there again).
    outcome: Option<(Format, Result<Exported, Failure>)>,
    dest: Option<PathBuf>,
    discarding: Job<Result<(), String>>,
    /// Discard them or Show in folder failed: why.
    note: Option<String>,
    hooks: Hooks,
}

/// Why the capture's splat edits cannot be applied, as read when the dialog opened or as the export found them since.
fn problem(edits: Option<&Edits>, outcome: Option<&(Format, Result<Exported, Failure>)>) -> Option<String> {
    edits.and_then(|e| e.problem.clone()).or_else(|| match outcome {
        Some((_, Err(Failure::Edits(why)))) => Some(why.clone()),
        _ => None,
    })
}

impl Export {
    /// The dialog for the capture in `session` (`title` its world), growing out of `origin`; its edits are read as it opens.
    pub fn open(ctx: &egui::Context, session: &Path, title: &str, origin: Rect) -> Self {
        let hooks = STAGED.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or_default();
        Self::with_hooks(ctx, session, title, origin, hooks)
    }

    pub fn with_hooks(ctx: &egui::Context, session: &Path, title: &str, origin: Rect, hooks: Hooks) -> Self {
        let dir = session.to_path_buf();
        Self {
            id: Id::unique(("splat-export", OPENED.fetch_add(1, Ordering::Relaxed))),
            origin,
            order: Order::Foreground,
            session: session.to_path_buf(),
            title: title.to_string(),
            format: last().map(|(_, f)| f).unwrap_or_default(),
            edits: Job::spawn(ctx, move || export::edits_now(&dir)),
            picking: Job::default(),
            running: None,
            outcome: None,
            dest: None,
            discarding: Job::default(),
            note: None,
            hooks,
        }
    }

    /// Drawn in `order` (over a 3D view in the tooltip layer).
    pub fn order(mut self, order: Order) -> Self {
        self.order = order;
        self
    }

    /// Takes in what its jobs came back with.
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(Some(path)) = self.picking.take() {
            self.start(ctx, path);
        }
        if let Some(r) = self.running.as_mut().and_then(|r| r.job.take()) {
            let format = self.running.take().map(|r| r.format).unwrap_or(self.format);
            if let Ok(done) = &r {
                *LAST.lock().unwrap_or_else(|e| e.into_inner()) = done.path.parent().map(|p| (p.to_path_buf(), format));
            }
            self.outcome = Some((format, r));
        }
        if let Some(r) = self.discarding.take() {
            match r {
                Ok(()) => {
                    let dir = self.session.clone();
                    self.edits = Job::spawn(ctx, move || export::edits_now(&dir));
                    if matches!(self.outcome, Some((_, Err(Failure::Edits(_))))) {
                        self.outcome = None;
                    }
                    self.note = None;
                }
                Err(e) => self.note = Some(tr!("They could not be discarded: {e}", e = e)),
            }
        }
    }

    /// Windows' save dialog, on the capture's name (or the name last chosen, for another place).
    fn ask(&mut self, ctx: &egui::Context, elsewhere: bool) {
        let name = match (&self.dest, elsewhere) {
            (Some(d), true) => d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            _ => export::file_name(&self.session, &self.title, self.format),
        };
        let folder = if elsewhere { None } else { last().map(|(f, _)| f) };
        let title = tr!("Export {title} as {format}", title = self.title, format = self.format.label());
        let ask = Ask { title, format: self.format, name, folder };
        let pick = self.hooks.pick;
        self.note = None;
        self.picking = Job::spawn(ctx, move || pick(ask));
    }

    /// Exports to `dest` (with the format's extension) on a worker.
    fn start(&mut self, ctx: &egui::Context, dest: PathBuf) {
        let dest = export::with_extension(dest, self.format);
        let progress = Arc::new(Progress::default());
        let (session, format, p, free_space, to) = (self.session.clone(), self.format, progress.clone(), self.hooks.free_space, dest.clone());
        let job = Job::spawn(ctx, move || export::export_with(&session, format, &to, &p, &free_space));
        self.running = Some(Running { format, progress, job });
        self.outcome = None;
        self.note = None;
        self.dest = Some(dest);
    }

    fn discard(&mut self, ctx: &egui::Context) {
        let dir = self.session.clone();
        self.discarding = Job::spawn(ctx, move || crate::pages::splats::discard_edits(&dir).map_err(|e| format!("{e:#}")));
    }

    /// The dialog this frame; false once it closed (closing stops an export under way: nothing is left of it).
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        self.poll(ctx);
        let busy = self.running.is_some() || self.picking.running() || self.discarding.running();
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        let title = tr!("Export {title}", title = self.title);
        let dialog = Dialog::new(self.id, self.origin, &title)
            .glyph(icons::UPLOAD_SIMPLE)
            .subtitle(tr("Its splats as a file for other tools"))
            .width(560.0)
            .footer(true)
            .busy(busy)
            .order(self.order);
        let response = dialog.show(ctx, |ui, part| match part {
            Part::Body => self.body(ui),
            Part::Footer => self.footer(ui),
        });
        if response.closing {
            if let Some(r) = &self.running {
                r.progress.stop();
            }
        }
        !response.closed
    }

    fn body(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let w = ui.available_width();
        let edits = self.edits.poll().cloned();
        let busy = self.running.is_some() || self.picking.running();
        let removed = edits.as_ref().map_or(0, |e| e.removed);
        let lead = match removed {
            0 => tr("The file holds what its 3D view shows: every splat as trained. The sky stays in N4N0: other tools draw their own.").to_string(),
            1 => tr("The file holds what its 3D view shows: the splat hidden or deleted there is left out, as from every share. The sky stays in N4N0: other tools draw their own.").to_string(),
            n => tr!(
                "The file holds what its 3D view shows: the {count} splats hidden or deleted there are left out, as from every share. The sky stays in N4N0: other tools draw their own.",
                count = format::integer(n as f64)
            ),
        };
        paragraph(ui, &lead, theme::fg_2());

        // Edits that cannot be applied: why, and the way on.
        if let Some(problem) = problem(edits.as_ref(), self.outcome.as_ref()) {
            let text = tr!("{problem} Discard them to export every splat as trained: Select splats in its 3D view can remove them again.", problem = problem);
            feedback::banner(ui, Id::unique(("export-problem", &self.session)), Tone::Warn, &text, false);
            let b = Button::new(if self.discarding.running() { tr("Discarding…") } else { tr("Discard them") })
                .icon(icons::TRASH)
                .variant(Variant::Danger)
                .size(Size::Sm)
                .enabled(!self.discarding.running() && !busy)
                .id_salt("export-discard");
            let bw = b.desired_width(ui);
            let (r, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
            if widgets::pressed_fresh(&b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
                self.discard(&ctx);
            }
        }

        caps(ui, tr("Format"));
        let splats = edits.as_ref().and_then(|e| e.splats).map(|n| n.saturating_sub(removed));
        for f in Format::ALL {
            self.row(ui, f, splats, !busy);
        }

        // Under way: how far.
        if let Some(r) = &self.running {
            let (step, done) = r.progress.get().unwrap_or((Step::Reading, 0.0));
            let percent = format!("{:.0}", done * 100.0);
            let heading = match step {
                Step::Reading => tr!("Reading the splats… {percent}%", percent = percent),
                Step::Making => tr!("Making the {format}… {percent}%", format = r.format.label(), percent = percent),
                Step::Writing => tr!("Writing the file… {percent}%", percent = percent),
            };
            progress_card(ui, Id::unique(("export-progress", self.id)), &heading, done, self.dest.as_deref());
        }

        // Done, or why not.
        match &self.outcome {
            Some((_, Ok(done))) => {
                let name = done.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let folder = done.path.parent().map(|p| p.display().to_string()).unwrap_or_default();
                let text = tr!("Saved {name} in {folder}: {size}, {count} splats.", name = name, folder = folder, size = format::bytes(done.bytes as f64), count = format::integer(done.splats as f64));
                feedback::banner(ui, Id::unique(("export-saved", &done.path)), Tone::Ok, &text, false);
                if let Some(s) = done.saturated.filter(|s| s.colors * 1000 > s.components) {
                    let share = s.colors as f64 / s.components.max(1) as f64 * 100.0;
                    note(
                        ui,
                        &tr!(
                            "SPZ keeps colours in 8 bits: {share}% of the colour values N4N0 learned were brighter or darker than it holds, and were clamped. GLB and PLY keep every value.",
                            share = format!("{share:.1}")
                        ),
                    );
                }
            }
            Some((f, Err(Failure::Stopped))) => note(ui, &Failure::Stopped.message(*f)),
            // Said with Discard them, above.
            Some((_, Err(Failure::Edits(_)))) => {}
            Some((f, Err(e))) => {
                feedback::banner(ui, Id::unique(("export-failed", self.id, format!("{e:?}"))), Tone::Danger, &e.message(*f), false);
            }
            None => {}
        }
        if let Some(n) = self.note.clone() {
            feedback::banner(ui, Id::unique(("export-note", self.id, &n)), Tone::Danger, &n, false);
        }
    }

    /// One format: its name and what it is, about how large, where it goes; a radio on the right (an automated run
    /// finds it by the format's name, widgets::control).
    fn row(&mut self, ui: &mut Ui, f: Format, splats: Option<usize>, enabled: bool) {
        let w = ui.available_width();
        let hint = ui.painter().layout_job(Txt::new(f.hint(), 12.0).color(theme::fg_3()).line_height(17.0).job(w - 16.0 - 56.0));
        let (r, _) = ui.allocate_exact_size(vec2(w, 14.0 + 20.0 + 4.0 + hint.size().y + 14.0), Sense::hover());
        let id = Id::unique(("export-format", f.extension()));
        let selected = self.format == f;
        let resp = ui.interact(r, id, if enabled { Sense::click() } else { Sense::hover() });
        let resp = if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
        let hover = anim::toggle(ui.ctx(), id.with("hover"), resp.hovered() && enabled, Spring::SOFT);
        let press = anim::toggle(ui.ctx(), id.with("press"), resp.is_pointer_button_down_on(), Spring::SNAPPY);
        let shown = Rect::from_center_size(r.center(), r.size() * (1.0 - 0.015 * press));
        let p = ui.painter();
        let sel = super::projects::art::selection(p, id.with("ring"), shown, 16.0, selected);
        widgets::frame(p, shown, 16.0, theme::mix(theme::surface_soft(), theme::ink_750(), hover.max(sel * 0.7)));
        let x = r.left() + 16.0;
        let y = r.top() + 14.0 + 10.0;
        let name = widgets::text(p, pos2(x, y), Align2::LEFT_CENTER, &Txt::new(f.label(), 14.0).weight(weight::SEMIBOLD));
        let kind = widgets::text(p, pos2(name.right() + 10.0, y), Align2::LEFT_CENTER, &Txt::new(f.kind(), 12.0).weight(weight::MEDIUM).color(theme::fg_2()));
        if let Some(n) = splats {
            let size = tr!("about {size}", size = format::bytes(f.estimate(n) as f64));
            widgets::text(p, pos2(kind.right() + 10.0, y), Align2::LEFT_CENTER, &Txt::new(size, 12.0).color(theme::fg_4()));
        }
        p.galley(pos2(x, r.top() + 14.0 + 20.0 + 4.0), hint, theme::fg_3());
        // A radio: a ring, filled with a check once chosen.
        let c = pos2(r.right() - 28.0, r.center().y);
        p.circle_stroke(c, 10.0, egui::Stroke::new(1.5, theme::mix(theme::fg_4(), theme::accent(), sel.clamp(0.0, 1.0))));
        super::projects::art::check(p, c, sel);
        widgets::note_control(ui.ctx(), f.label(), r, enabled);
        if enabled && widgets::activated(&resp) {
            self.format = f;
        }
    }

    fn footer(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let close = |ctx: &egui::Context, id: Id| dialog::close(ctx, id);
        if let Some(r) = &self.running {
            let done = r.progress.get().map_or(0.0, |(_, d)| d);
            Button::new(&tr!("Exporting… {percent}%", percent = format!("{:.0}", done * 100.0)))
                .icon(icons::UPLOAD_SIMPLE)
                .variant(Variant::Primary)
                .enabled(false)
                .id_salt("export-go")
                .show(ui);
            if widgets::activated(&Button::new(tr("Stop")).variant(Variant::Ghost).id_salt("export-stop").show(ui)) {
                r.progress.stop();
            }
            return;
        }
        match &self.outcome {
            Some((f, Ok(done))) if *f == self.format => {
                let path = done.path.clone();
                if widgets::activated(&Button::new(tr("Show in folder")).icon(icons::FOLDER_OPEN).variant(Variant::Primary).id_salt("export-reveal").show(ui)) {
                    if let Err(e) = (self.hooks.reveal)(&path) {
                        self.note = Some(tr!("Its folder could not be shown: {e}", e = format!("{e:#}")));
                    }
                }
                if widgets::activated(&Button::new(tr("Done")).variant(Variant::Ghost).id_salt("export-done").show(ui)) {
                    close(&ctx, self.id);
                }
                return;
            }
            Some((f, Err(e))) if *f == self.format && e.elsewhere() => {
                let picking = self.picking.running();
                let b = Button::new(if picking { tr("Choosing where…") } else { tr("Choose another place…") })
                    .icon(icons::FOLDER_OPEN)
                    .variant(Variant::Primary)
                    .enabled(!picking);
                if widgets::activated(&b.id_salt("export-elsewhere").show(ui)) {
                    self.ask(&ctx, true);
                }
                if widgets::activated(&Button::new(tr("Close")).variant(Variant::Ghost).id_salt("export-close").show(ui)) {
                    close(&ctx, self.id);
                }
                return;
            }
            Some((f, Err(Failure::Failed(_) | Failure::Changed))) if *f == self.format => {
                if widgets::activated(&Button::new(tr("Try again")).icon(icons::ARROW_CLOCKWISE).variant(Variant::Primary).id_salt("export-retry").show(ui)) {
                    if let Some(dest) = self.dest.clone() {
                        self.start(&ctx, dest);
                    }
                }
                if widgets::activated(&Button::new(tr("Close")).variant(Variant::Ghost).id_salt("export-close").show(ui)) {
                    close(&ctx, self.id);
                }
                return;
            }
            _ => {}
        }
        // Ready once its edits are known to apply.
        let known = self.edits.poll().cloned();
        let ready = known.is_some() && problem(known.as_ref(), self.outcome.as_ref()).is_none() && !self.discarding.running();
        let picking = self.picking.running();
        let label = if picking { tr("Choosing where…").to_string() } else { tr!("Export {format}…", format = self.format.label()) };
        let mut b = Button::new(&label).icon(icons::UPLOAD_SIMPLE).variant(Variant::Primary).enabled(ready && !picking).id_salt("export-go");
        if !ready && !self.edits.running() {
            b = b.hint(tr("Discard the splat edits that could not be applied first"));
        }
        let go = b.show(ui);
        // Its name with a value in it has no English of its own to be found by, in a run in another language.
        if !picking {
            widgets::note_control(ui.ctx(), &format!("Export {}…", self.format.label()), go.rect, ready); // i18n: not shown
        }
        if widgets::activated(&go) && ready && !picking {
            self.ask(&ctx, false);
        }
        if widgets::activated(&Button::new(tr("Cancel")).variant(Variant::Ghost).id_salt("export-cancel").show(ui)) {
            close(&ctx, self.id);
        }
    }
}

impl Drop for Export {
    /// A dialog its page let go of without closing it (its 3D view or window gone, another one opened while it was
    /// closing) stops its export too: closing stops it, and so does going.
    fn drop(&mut self) {
        if let Some(r) = &self.running {
            r.progress.stop();
        }
    }
}

fn paragraph(ui: &mut Ui, text: &str, color: egui::Color32) {
    let w = ui.available_width();
    let g = ui.painter().layout_job(Txt::new(text, 12.5).color(color).line_height(19.0).job(w));
    let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
    ui.painter().galley(r.min, g, color);
}

/// A small label in caps over a group of rows.
fn caps(ui: &mut Ui, text: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    widgets::text(ui.painter(), r.left_center(), Align2::LEFT_CENTER, &Txt::caps(text));
}

/// A line under an info mark.
fn note(ui: &mut Ui, text: &str) {
    let w = ui.available_width();
    let g = ui.painter().layout_job(Txt::new(text, 12.0).color(theme::fg_3()).line_height(18.0).job(w - 22.0));
    let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
    widgets::icon(ui.painter(), pos2(r.left() + 7.0, r.top() + 9.0), icons::INFO, 13.0, theme::fg_4());
    ui.painter().galley(pos2(r.left() + 22.0, r.top()), g, theme::fg_3());
}

/// What runs, how far, and where it goes.
fn progress_card(ui: &mut Ui, id: Id, heading: &str, done: f32, dest: Option<&Path>) {
    let ctx = ui.ctx().clone();
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 92.0), Sense::hover());
    let p = ui.painter();
    widgets::frame(p, r, 14.0, theme::alpha(theme::accent(), 0.07));
    feedback::spinner(p, pos2(r.left() + 22.0, r.top() + 26.0), 8.0, 2.2, theme::accent());
    widgets::text(p, pos2(r.left() + 42.0, r.top() + 26.0), Align2::LEFT_CENTER, &Txt::new(heading, 13.5).weight(weight::MEDIUM));
    let bar = Rect::from_min_size(pos2(r.left() + 16.0, r.top() + 50.0), vec2(r.width() - 32.0, 6.0));
    p.rect_filled(bar, theme::radius(3.0), theme::white(0.08));
    let f = anim::value(&ctx, id, done, Spring::SOFT).clamp(0.0, 1.0);
    p.rect_filled(Rect::from_min_size(bar.min, vec2((bar.width() * f).max(6.0), 6.0)), theme::radius(3.0), theme::accent());
    if let Some(d) = dest {
        let line = tr!("To {path} · closing stops it", path = d.display());
        widgets::text_truncated(p, pos2(r.left() + 16.0, r.top() + 74.0), Align2::LEFT_CENTER, &Txt::new(line, 11.5).color(theme::fg_3()), r.width() - 32.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::splat_export::tests::{positions, remove, session};
    use egui::{Event, Modifiers, PointerButton, RawInput};
    use orbiters_core::reconstruction as rec;

    /// Where the stand-in save dialog answers, what Show in folder was asked to show, and the room the disk has.
    static PICK: Mutex<Option<PathBuf>> = Mutex::new(None);
    static ASKED: Mutex<Option<String>> = Mutex::new(None);
    static REVEALED: Mutex<Option<PathBuf>> = Mutex::new(None);
    static FREE: Mutex<Option<u64>> = Mutex::new(None);
    /// A capture whose splat edits the next export hides one more splat of while it is made (as it asks for the room).
    static EDIT_DURING: Mutex<Option<PathBuf>> = Mutex::new(None);

    fn hooks() -> Hooks {
        Hooks {
            pick: |ask| {
                *ASKED.lock().unwrap() = Some(ask.name);
                PICK.lock().unwrap().clone()
            },
            reveal: |p| {
                *REVEALED.lock().unwrap() = Some(p.to_path_buf());
                Ok(())
            },
            free_space: |_| {
                if let Some(dir) = EDIT_DURING.lock().unwrap().take() {
                    let result = rec::result_dir(&dir);
                    let mut e = orbiters_reconstruct::edit::Edit::decode(&std::fs::read(result.join(rec::EDIT_FILE)).unwrap()).unwrap();
                    e.marks[0] = orbiters_reconstruct::edit::HIDDEN;
                    e.save(&result).unwrap();
                }
                *FREE.lock().unwrap()
            },
        }
    }

    /// The window's size in the harness: the app's default (screenshot.rs).
    const SCREEN: egui::Vec2 = vec2(1280.0, 820.0);

    struct Harness {
        ctx: egui::Context,
        export: Export,
        open: bool,
        time: f64,
        /// With `ORBITERS_TEST_OUT` set: the CPU painter (orbiters_ui::software) the dialog's states are shot with into
        /// that folder, no GPU needed, and the last frame's shapes.
        painter: Option<(PathBuf, orbiters_ui::software::Software)>,
        shapes: Option<(Vec<egui::epaint::ClippedShape>, f32)>,
    }

    impl Harness {
        fn new(dir: &Path) -> Self {
            let ctx = egui::Context::default();
            orbiters_ui::install(&ctx);
            widgets::watch_controls();
            let export = Export::with_hooks(&ctx, dir, "VRChat Home", Rect::from_min_size(pos2(900.0, 40.0), vec2(120.0, 40.0)), hooks());
            let painter =
                std::env::var_os("ORBITERS_TEST_OUT").map(|d| (PathBuf::from(d), orbiters_ui::software::Software::new(SCREEN.x as u32, SCREEN.y as u32)));
            let mut h = Self { ctx, export, open: true, time: 0.0, painter, shapes: None };
            h.settle();
            h
        }

        /// The last frame as `dialog-<name>.png` in `ORBITERS_TEST_OUT` (nothing without it), over the app's dark page.
        fn shoot(&mut self, name: &str) {
            self.frame(Vec::new());
            let (Some((dir, sw)), Some((shapes, ppp))) = (self.painter.as_mut(), self.shapes.take()) else { return };
            let primitives = self.ctx.tessellate(shapes, ppp);
            let pixels = sw.paint(&primitives, ppp, egui::Color32::from_rgb(9, 9, 11));
            let image = egui::ColorImage::from_rgba_premultiplied([SCREEN.x as usize, SCREEN.y as usize], pixels);
            std::fs::create_dir_all(&*dir).unwrap();
            orbiters_ui::offscreen::save_png(&dir.join(format!("dialog-{name}.png")), &image).unwrap();
        }

        fn frame(&mut self, events: Vec<Event>) {
            self.time += 1.0 / 60.0;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), SCREEN)), time: Some(self.time), events, ..Default::default() };
            let (export, open) = (&mut self.export, &mut self.open);
            let mut output = self.ctx.run_ui(input, |ui| {
                *open = export.show(ui.ctx());
            });
            if let Some((_, sw)) = self.painter.as_mut() {
                sw.set_textures(&mut output.textures_delta);
                sw.free_textures(&mut output.textures_delta);
                self.shapes = Some((output.shapes, output.pixels_per_point));
            }
            output.textures_delta.clear();
        }

        fn settle(&mut self) {
            for _ in 0..60 {
                self.frame(Vec::new());
            }
        }

        /// Frames until `done` (jobs run on workers), at most ten seconds.
        fn until(&mut self, what: &str, done: impl Fn(&Self) -> bool) {
            for _ in 0..2000 {
                self.frame(Vec::new());
                if done(self) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            panic!("waited for {what}");
        }

        fn control(&self, label: &str) -> Option<(Rect, bool)> {
            widgets::control(&self.ctx, label)
        }

        fn click_at(&mut self, at: egui::Pos2) {
            self.frame(vec![
                Event::PointerMoved(at),
                Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
            ]);
            self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
            self.frame(Vec::new());
        }

        fn click(&mut self, label: &str) {
            let (r, enabled) = self.control(label).unwrap_or_else(|| panic!("{label:?} is drawn"));
            assert!(enabled, "{label:?} takes clicks");
            self.click_at(r.center());
        }
    }

    /// From the dialog to the file: GLB picked, Windows' dialog offered the capture's name, the file holds the splats
    /// the 3D view keeps, Show in folder shows it, Done closes the dialog. Then a full disk (Choose another place, no
    /// file), and damaged edits (Export waits, Discard them, then every splat but the sky).
    #[test]
    fn the_dialog_exports_what_the_view_shows_and_leads_out_of_each_failure() {
        let (dir, kept) = session("dialog", true);
        let out = dir.parent().unwrap().join("Exports");
        std::fs::create_dir_all(&out).unwrap();
        *FREE.lock().unwrap() = None;

        let mut h = Harness::new(&dir);
        // Its edits are read on a worker as it opens.
        h.until("SPZ first, ready", |h| h.control("Export SPZ…").is_some_and(|(_, on)| on));
        h.shoot("open");
        h.click("GLB");
        assert_eq!(h.export.format, Format::Glb);
        h.settle();
        h.shoot("glb");
        *PICK.lock().unwrap() = Some(out.join("My capture"));
        h.click("Export GLB…");
        h.until("the export", |h| h.control("Show in folder").is_some());
        h.settle();
        h.shoot("saved");
        assert_eq!(ASKED.lock().unwrap().as_deref(), Some("Home 2026-11-02 21-30.glb"), "the capture's name offered");
        let file = out.join("My capture.glb");
        assert_eq!(positions(&file, Format::Glb).len(), kept.len());
        h.click("Show in folder");
        assert_eq!(REVEALED.lock().unwrap().as_deref(), Some(file.as_path()));
        h.click("Done");
        h.until("the dialog closed", |h| !h.open);

        // A full disk: why, Choose another place, nothing written.
        *FREE.lock().unwrap() = Some(1 << 20);
        let mut h = Harness::new(&dir);
        assert_eq!(h.export.format, Format::Glb, "on the format last exported");
        h.until("ready", |h| h.control("Export GLB…").is_some_and(|(_, on)| on));
        *PICK.lock().unwrap() = Some(out.join("Second.glb"));
        h.click("Export GLB…");
        h.until("the failure", |h| h.control("Choose another place…").is_some());
        h.settle();
        h.shoot("no-room");
        assert!(matches!(h.export.outcome, Some((Format::Glb, Err(Failure::NoRoom { .. })))));
        let parts = || std::fs::read_dir(&out).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".part")).count();
        assert!(!out.join("Second.glb").exists() && parts() == 0);
        *FREE.lock().unwrap() = None;
        h.click("Choose another place…");
        // Windows' dialog runs on a worker: what it was asked, once it was.
        h.until("the name chosen offered again", |_| ASKED.lock().unwrap().as_deref() == Some("Second.glb"));
        h.until("the export elsewhere", |h| h.control("Show in folder").is_some());
        assert!(out.join("Second.glb").is_file());
        h.click("Done");
        h.until("closed", |h| !h.open);

        // Damaged edits: Export waits; Discard them; then every splat but the sky.
        std::fs::write(rec::result_dir(&dir).join(rec::EDIT_FILE), b"N4EDIT01 damaged").unwrap();
        let mut h = Harness::new(&dir);
        h.until("the damaged edits found", |h| h.control("Discard them").is_some_and(|(_, on)| on));
        assert!(h.control("Export GLB…").is_some_and(|(_, on)| !on), "Export waits for the edits");
        h.settle();
        h.shoot("damaged");
        h.click("Discard them");
        h.until("the edits discarded", |h| h.control("Export GLB…").is_some_and(|(_, on)| on));
        assert!(!rec::result_dir(&dir).join(rec::EDIT_FILE).exists());
        *PICK.lock().unwrap() = Some(out.join("All.glb"));
        h.click("Export GLB…");
        h.until("the export", |h| h.control("Show in folder").is_some());
        assert_eq!(positions(&out.join("All.glb"), Format::Glb).len(), 7, "every splat but the sky");
        h.click("Done");
        h.until("closed", |h| !h.open);

        // Edits of another reconstruction written after the dialog opened: the export stops on them, and offers Discard them.
        let mut h = Harness::new(&dir);
        h.until("ready", |h| h.control("Export GLB…").is_some_and(|(_, on)| on));
        let mut stale = orbiters_reconstruct::edit::Edit::new(8, orbiters_reconstruct::edit::Identity { bytes: 1234, hash: 99 });
        stale.marks[0] = orbiters_reconstruct::edit::HIDDEN;
        std::fs::write(rec::result_dir(&dir).join(rec::EDIT_FILE), stale.encode()).unwrap();
        *PICK.lock().unwrap() = Some(out.join("Late.glb"));
        h.click("Export GLB…");
        h.until("the edits found by the export", |h| h.control("Discard them").is_some_and(|(_, on)| on));
        assert!(matches!(h.export.outcome, Some((_, Err(Failure::Edits(_))))));
        assert!(h.control("Export GLB…").is_some_and(|(_, on)| !on), "Export waits for them");
        assert!(!out.join("Late.glb").exists());
        h.click("Discard them");
        h.until("ready again", |h| h.control("Export GLB…").is_some_and(|(_, on)| on));
        h.click("Cancel");
        h.until("closed", |h| !h.open);

        // Edits saved while the file is made: nothing saved, why, and Try again exports them.
        let (dir2, kept2) = session("dialog-changed", true);
        let mut h = Harness::new(&dir2);
        h.until("ready", |h| h.control("Export GLB…").is_some_and(|(_, on)| on));
        *EDIT_DURING.lock().unwrap() = Some(dir2.clone());
        *PICK.lock().unwrap() = Some(out.join("Changed.glb"));
        h.click("Export GLB…");
        h.until("the change found", |h| h.control("Try again").is_some());
        assert!(matches!(h.export.outcome, Some((Format::Glb, Err(Failure::Changed)))));
        assert!(!out.join("Changed.glb").exists());
        h.click("Try again");
        h.until("the export again", |h| h.control("Show in folder").is_some());
        assert_eq!(positions(&out.join("Changed.glb"), Format::Glb).len(), kept2.len() - 1, "the splat hidden meanwhile left out");
        h.click("Done");
        h.until("closed", |h| !h.open);
        *LAST.lock().unwrap() = None;
        remove(&dir2);
        remove(&dir);
    }
}
