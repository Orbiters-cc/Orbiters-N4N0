//! What the panel says about a project's disk, and the Library move it offers (`orbiters_projects::storage`).
//!
//! Open in Unity never waits on any of this: a project on a low disk opens, and this card stays and says so. The move
//! itself can take hours between a hard disk and a USB disk, so it is not an action of the page's one-at-a-time
//! [`State::task`], which greys out every other button while it runs: it has its own worker, the gallery and other
//! projects stay usable, and the card (or, from elsewhere in the page, the capsule over the cards) shows how far it got
//! and offers Cancel.
use orbiters_core::locks::*;
use orbiters_i18n::{tr, tr_n};
use super::state::{Poke, State};
use crate::pages::{Cx, Job};
use egui::{pos2, vec2, Align2, Id, Rect, Sense, Shape, Ui, UiBuilder};
use orbiters_projects::discovery::{self, Project};
use orbiters_projects::storage::{self, Health, Notice, Phase, Plan, Progress, RepairEnd, SharedProgress, Verdict};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, Size, Variant};
use orbiters_ui::{anim::{self, Spring}, feedback, format, icons, toast};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A Library move running.
pub(crate) struct Run {
    project: PathBuf,
    name: String,
    progress: SharedProgress,
    cancel: Arc<AtomicBool>,
    job: Job<Result<RepairEnd, String>>,
    /// When copying began, for the time left.
    copying_since: Option<Instant>,
    /// Screenshots: a state to draw, no worker behind it.
    still: bool,
}

/// Screenshots: what the card shows without looking at the disk.
#[derive(Clone, Default)]
pub(crate) struct Fixture {
    health: Health,
    plan: Option<Result<Plan, String>>,
    /// Measuring, this far.
    measuring: Option<Progress>,
}

#[derive(Default)]
pub(crate) struct Storage {
    run: Option<Run>,
    /// What a repair of this project would do.
    plan: Option<(PathBuf, Result<Plan, String>)>,
    plan_job: Job<(PathBuf, Result<Option<Plan>, String>)>,
    plan_for: Option<PathBuf>,
    plan_progress: SharedProgress,
    plan_cancel: Arc<AtomicBool>,
    /// Unity had the project open at the last look: its Library changes under a measurement.
    was_open: bool,
    fixture: Option<Fixture>,
}

#[cfg(not(target_os = "macos"))]
fn drive_of(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text).to_string();
    Path::new(&text).components().next().map_or(text.clone(), |c| c.as_os_str().to_string_lossy().into_owned())
}

/// A disk as the Mac's Finder names it: an external one by its name, the one the system starts from by what it is.
#[cfg(target_os = "macos")]
fn drive_of(path: &Path) -> String {
    match path.strip_prefix("/Volumes").ok().and_then(|p| p.components().next()) {
        Some(name) => name.as_os_str().to_string_lossy().into_owned(),
        None => tr("the startup disk").to_string(),
    }
}

/// The same at the start of a sentence.
fn starting(disk: &str) -> String {
    let mut chars = disk.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn bytes(n: u64) -> String {
    format::bytes(n as f64)
}

fn files(n: u64) -> String {
    // The number is written with the language's separators, so it goes in after the plural was chosen.
    let amount = format::integer(n as f64);
    orbiters_i18n::fill(&tr_n("{amount} file", "{amount} files", n), &[("amount", &amount)])
}

/// "1 h 12 min", "4 min", "under a minute": how long is left, roughly.
fn time_left(seconds: f64) -> String {
    let minutes = (seconds / 60.0).ceil() as u64;
    match minutes {
        0..=1 => tr("under a minute left").to_string(),
        2..=59 => tr!("about {minutes} min left", minutes = minutes),
        _ => tr!("about {hours} h {minutes} min left", hours = minutes / 60, minutes = minutes % 60),
    }
}

impl State {
    /// A Library move of this project is running.
    pub(super) fn repairing(&self, path: &Path) -> bool {
        self.storage.run.as_ref().is_some_and(|r| discovery::same(&r.project, path))
    }

    /// Open in Unity, from wherever it is offered. `quiet`: no word when it worked (the setup dialog goes on by itself).
    pub(super) fn open_unity(&mut self, ctx: &egui::Context, project: &Project, quiet: bool) {
        self.open_unity_with(ctx, project, quiet, orbiters_projects::open_project);
    }

    /// Opens an original about to be tried on Unity 6: Orbiters moves nothing of it first.
    pub(super) fn open_unity_as_is(&mut self, ctx: &egui::Context, project: &Project) {
        self.open_unity_with(ctx, project, false, orbiters_projects::open_project_as_is);
    }

    fn open_unity_with(&mut self, ctx: &egui::Context, project: &Project, quiet: bool, open: fn(&Project) -> anyhow::Result<orbiters_projects::Opened>) {
        let refused = if self.u6.waiting(&project.path) {
            Some(tr("This trial waits for Creator Companion's migration: Orbiters does not open it or change its packages before.").to_string())
        } else if self.repairing(&project.path) {
            Some(tr!("Orbiters is moving the Library of {name}. Unity can open it once that ends, or after Cancel.", name = project.name))
        } else if self.action.running() {
            // A menu's Open is not greyed out by another action, and saying nothing would look like a click that did nothing.
            Some(tr!("Unity was not opened: wait for this to end first ({action}).", action = self.action_label.trim_end_matches('…')))
        } else {
            None
        };
        if let Some(words) = refused {
            return self.not_opened(ctx, super::opening::Note::said(&project.path, words));
        }
        self.opening.note = None;
        self.opening.now = Some(project.path.clone());
        let p = project.clone();
        self.task(ctx, tr("Opening Unity…"), move || {
            let opened = match open(&p) {
                Ok(opened) => opened,
                Err(e) => return Ok(super::opening::outcome(&p, &e)),
            };
            if quiet {
                return Ok(super::state::Outcome::Picked(None));
            }
            Ok(if opened.already_open {
                super::state::Outcome::Said(tr!("Unity already has {name} open", name = p.name))
            } else if let Some(moved) = opened.moved {
                super::state::Outcome::Changed(tr!("Project sent to Unity. Its new Library is at {path}, on a disk with room", path = moved.display()))
            } else {
                super::state::Outcome::Changed(tr("Project sent to Unity").into())
            })
        });
    }

    /// Starts moving `project`'s Library. One move at a time.
    pub(super) fn start_repair(&mut self, ctx: &egui::Context, project: &Project) {
        if self.storage.run.is_some() {
            return;
        }
        self.error = None;
        let progress = SharedProgress::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = {
            let (path, progress, cancel) = (project.path.clone(), progress.clone(), cancel.clone());
            Job::spawn(ctx, move || storage::repair(&path, &progress, &cancel).map_err(|e| format!("{e:#}")))
        };
        self.storage.run = Some(Run { project: project.path.clone(), name: project.name.clone(), progress, cancel, job, copying_since: None, still: false });
    }

    pub(super) fn cancel_repair(&mut self) {
        if let Some(run) = &self.storage.run {
            run.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Takes in a finished move: whatever page section is showing, since it can end long after it was asked for.
    pub(super) fn poll_repair(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        let Some(run) = self.storage.run.as_mut() else { return };
        if run.still {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(200));
        let Some(end) = run.job.take() else { return };
        let run = self.storage.run.take().expect("just looked at");
        // Everything known about the disk and the Library has changed.
        self.storage.plan = None;
        self.storage.plan_for = None;
        self.storage_health = None;
        self.storage_watch.stop();
        self.sizes_asked.remove(&run.project);
        let name = run.name;
        match end {
            Ok(RepairEnd::Moved(moved)) => {
                cx.toasts.push(ctx, toast::Kind::Success, tr!("{name}: its Library is now at {path}. Open it in Unity when you're ready.", name = name, path = moved.target.display()));
                if let Some(old) = moved.old_left {
                    cx.toasts.push(ctx, toast::Kind::Warning, tr!("The old Library could not be fully deleted: {path}. Orbiters tries again the next time its window opens.", path = old.display()));
                }
            }
            Ok(RepairEnd::Cancelled { left: None }) => {
                cx.toasts.push(ctx, toast::Kind::Info, tr!("Repair of {name} cancelled. Its Library stays where it was, and the partial copy is deleted.", name = name));
            }
            Ok(RepairEnd::Cancelled { left: Some(left) }) => {
                cx.toasts.push(ctx, toast::Kind::Warning, tr!("Repair of {name} cancelled. Its Library stays where it was. The partial copy at {path} could not be deleted; Orbiters tries again the next time its window opens.", name = name, path = left.display()));
            }
            Err(e) => self.error = Some(tr!("The storage of {name} was not repaired: {error}", name = name, error = e)),
        }
        // The project is still there, as it was or with its new Library: the gallery says so.
        self.load_view(ctx, false);
        self.refresh(ctx, false);
    }

    /// The capsule over the cards while a move runs and the card with its progress is not on screen: (label, detail,
    /// fraction, whether Cancel does something).
    pub(super) fn repair_capsule(&self) -> Option<(String, String, Option<f32>, bool)> {
        let run = self.storage.run.as_ref()?;
        if self.selected.as_ref().is_some_and(|p| discovery::same(p, &run.project)) {
            return None;
        }
        let p = run.progress.locked().clone();
        let cancelling = run.cancel.load(Ordering::Relaxed);
        let label = format!("{}: {}", run.name, heading(&p, cancelling));
        Some((label, detail(&p), p.fraction(), p.cancellable() && !cancelling))
    }

    /// Measures what a repair would copy, once for each project looked at and again when Unity closed it.
    fn want_plan(&mut self, ctx: &egui::Context, project: &Path, open: bool) {
        if let Some((path, plan)) = self.storage.plan_job.take() {
            match plan {
                Ok(Some(plan)) => self.storage.plan = Some((path, Ok(plan))),
                Ok(None) => {}
                Err(e) => self.storage.plan = Some((path, Err(e))),
            }
        }
        if self.storage.was_open && !open {
            self.storage.plan_for = None;
        }
        self.storage.was_open = open;
        if self.storage.plan_for.as_deref().is_some_and(|p| discovery::same(p, project)) {
            return;
        }
        // Another project's measurement has nothing more to say.
        self.storage.plan_cancel.store(true, Ordering::Relaxed);
        if self.storage.plan_job.running() {
            return;
        }
        self.storage.plan = None;
        self.storage.plan_for = Some(project.to_path_buf());
        self.storage.plan_progress = SharedProgress::default();
        self.storage.plan_cancel = Arc::new(AtomicBool::new(false));
        let (path, progress, cancel) = (project.to_path_buf(), self.storage.plan_progress.clone(), self.storage.plan_cancel.clone());
        self.storage.plan_job = Job::spawn(ctx, move || {
            discovery::low_priority();
            let plan = storage::plan(&path, &progress, &cancel).map_err(|e| format!("{e:#}"));
            (path, plan)
        });
    }

    /// Look at the logs and the disk on a thread, every few seconds while this project's panel shows, for editors opened
    /// outside Orbiters too. The panel is drawn again only when what the card says of them changed.
    fn poll_health(&mut self, ctx: &egui::Context, project: &Project) {
        let path = &project.path;
        let new = self.storage_watch.keep(ctx, path, Duration::from_secs(3), || {
            let path = path.clone();
            move || Seen::of(storage::inspect(&path))
        });
        if let Some(seen) = new {
            self.storage_health = Some((path.clone(), seen.health));
        }
    }
}

/// A look at a project's disk, the same as another when the card would say the same of both: the free space is never
/// the same to the byte from one look to the next.
#[derive(Clone)]
pub(in crate::pages) struct Seen {
    health: storage::Health,
    /// What the card shows of it: its notice, the free space as written, the disk, whether Unity has it open.
    shown: (Option<Notice>, Option<String>, Option<PathBuf>, bool),
}

impl Seen {
    fn of(health: storage::Health) -> Self {
        let notice = health.notice();
        // Without a notice there is no card: the numbers do not show.
        let free = notice.and(health.free).map(bytes);
        Self { shown: (notice, free, health.disk.clone(), health.open), health }
    }
}

impl PartialEq for Seen {
    fn eq(&self, other: &Self) -> bool {
        self.shown == other.shown
    }
}

/// What a phase is doing, in the person's terms.
fn heading(p: &Progress, cancelling: bool) -> String {
    let to = p.destination.as_deref().map(drive_of).unwrap_or_default();
    match p.phase {
        Phase::Starting => tr("Starting the repair…").into(),
        _ if cancelling && p.cancellable() => tr("Cancelling…").into(),
        Phase::Leftovers => tr("Clearing what an interrupted repair left…").into(),
        Phase::Measuring => tr("Measuring the Library…").into(),
        Phase::Copying => tr!("Copying the Library to {to}", to = to),
        Phase::Switching => tr("Switching to the new copy…").into(),
        Phase::RemovingOld => tr("Library moved. Deleting the old copy…").into(),
        Phase::RemovingPartial => tr("Deleting the partial copy…").into(),
    }
}

/// How far it got.
fn detail(p: &Progress) -> String {
    match p.phase {
        Phase::Copying => tr!("{done} of {total} checked · {files_done} of {files_total}", done = bytes(p.bytes_checked), total = bytes(p.bytes_total), files_done = format::integer(p.files_done as f64), files_total = files(p.files_total)),
        Phase::Measuring => tr!("{size} so far · {files}", size = bytes(p.bytes_total), files = files(p.files_total)),
        Phase::RemovingOld | Phase::RemovingPartial => tr!("{done} of {total} deleted", done = format::integer(p.files_removed as f64), total = files(p.files_to_remove)),
        _ => String::new(),
    }
}

/// A paragraph wrapped to the width left.
fn para(ui: &mut Ui, txt: Txt) {
    let w = ui.available_width();
    let galley = ui.painter().layout_job(txt.job(w));
    let (r, _) = ui.allocate_exact_size(vec2(w, galley.size().y), Sense::hover());
    ui.painter().galley(r.min, galley, theme::fg());
}

fn body(text: impl Into<String>) -> Txt {
    Txt::new(text.into(), 12.5).color(theme::fg_2()).line_height(18.0)
}

fn note(text: impl Into<String>) -> Txt {
    Txt::new(text.into(), 11.5).color(theme::fg_3()).line_height(16.0)
}

/// The card's frame: a tinted panel with its icon, `inside` below `top_left` at full width.
fn frame(ui: &mut Ui, salt: &str, icon: &str, tint: egui::Color32, inside: impl FnOnce(&mut Ui)) {
    let w = ui.available_width();
    let top_left = ui.cursor().min;
    let bg = ui.painter().add(Shape::Noop);
    let inner = Rect::from_min_size(top_left + vec2(44.0, 12.0), vec2(w - 44.0 - 16.0, 100_000.0));
    let mut cu = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)).id_salt(("storage-card", salt)));
    cu.spacing_mut().item_spacing = vec2(8.0, 6.0);
    inside(&mut cu);
    let h = cu.min_rect().height() + 12.0 + 14.0;
    let rect = Rect::from_min_size(top_left, vec2(w, h));
    let p = ui.painter();
    p.set(bg, Shape::mesh(orbiters_ui::paint::squircle_mesh(rect, theme::squircle(14.0), theme::alpha(tint, 0.1), 1.0 / ui.ctx().pixels_per_point())));
    widgets::icon(p, pos2(rect.left() + 24.0, rect.top() + 21.5), icon, 16.0, tint);
    ui.allocate_exact_size(vec2(w, h), Sense::hover());
    ui.add_space(2.0);
}

/// The card, under the panel's hero: how the disk is, or how far a move of this project's Library got.
pub(super) fn card(ui: &mut Ui, state: &mut State, project: &Project) {
    let ctx = ui.ctx().clone();
    if state.repairing(&project.path) {
        progress_card(ui, state);
        return;
    }
    let fixture = state.storage.fixture.clone();
    let health = match &fixture {
        Some(f) => f.health.clone(),
        None if state.fixture => return,
        None => {
            state.poll_health(&ctx, project);
            let Some((_, health)) = state.storage_health.as_ref().filter(|(p, _)| p == &project.path) else { return };
            health.clone()
        }
    };
    let Some(notice) = health.notice() else { return };
    let disk = health.disk.as_deref().map(drive_of).unwrap_or_else(|| tr("this disk").to_string());
    let free = health.free.map(bytes).unwrap_or_else(|| tr("an unknown amount").to_string());
    let (title, tint, icon, says) = match notice {
        Notice::RunningLow => (
            tr("This disk is running low"),
            theme::WARN,
            icons::WARNING,
            tr!("{disk} has {free} free. Unity opens as usual, but a large import can run out of room: Orbiters looks for at least {gigabytes} GB free.", disk = starting(&disk), free = free, gigabytes = storage::IMPORT_RESERVE >> 30),
        ),
        Notice::OutOfSpace => (
            tr("Unity ran out of disk space"),
            theme::DANGER,
            icons::X_CIRCLE,
            tr!("Unity's log says it could not write this project's files, and {disk} has {free} free. Opening it again can stop at the same point until there is more room.", disk = disk, free = free),
        ),
        Notice::FailedEarlier => (
            tr("Unity ran out of disk space earlier"),
            theme::BLUE,
            icons::INFO,
            {
                let again = if health.open { tr("Quit Unity and open the project again") } else { tr("Open the project again") };
                match health.free {
                    Some(free) => tr!("Unity's log still shows that it could not write this project's files, but {disk} has {free} free now, so there is nothing to move. {again} to let it finish.", disk = disk, free = bytes(free), again = again),
                    None => tr!("Unity's log still shows that it could not write this project's files. Free some space on its disk if it stops there again. {again} to let it finish.", again = again),
                }
            },
        ),
    };
    let mut repair = false;
    let mut measure_again = false;
    if notice != Notice::FailedEarlier {
        if fixture.as_ref().is_none_or(|f| f.plan.is_none() && f.measuring.is_none()) {
            state.want_plan(&ctx, &project.path, health.open);
        }
    }
    let plan = match &fixture {
        Some(f) if f.plan.is_some() => f.plan.clone(),
        _ => state.storage.plan.as_ref().filter(|(p, _)| discovery::same(p, &project.path)).map(|(_, plan)| plan.clone()),
    };
    let measuring = fixture.as_ref().and_then(|f| f.measuring.clone()).or_else(|| plan.is_none().then(|| state.storage.plan_progress.locked().clone()));
    let other_running = state.storage.run.as_ref().map(|r| r.name.clone());
    let action_running = state.running();
    let mut anchor = None;
    frame(ui, "notice", icon, tint, |cu| {
        para(cu, Txt::new(title, 13.5).weight(weight::SEMIBOLD).line_height(19.0));
        para(cu, body(says));
        if notice == Notice::FailedEarlier {
            return;
        }
        let ready = match &plan {
            None => {
                let p = measuring.clone().unwrap_or_default();
                let (r, _) = cu.allocate_exact_size(vec2(cu.available_width(), 22.0), Sense::hover());
                feedback::spinner(cu.painter(), pos2(r.left() + 8.0, r.center().y), 6.0, 1.8, theme::accent());
                let text = tr!("Measuring the Library, to say what a repair would move: {detail}", detail = detail(&Progress { phase: Phase::Measuring, ..p }));
                widgets::text_truncated(cu.painter(), pos2(r.left() + 22.0, r.center().y), Align2::LEFT_CENTER, &note(text), r.width() - 22.0);
                false
            }
            Some(Err(e)) => {
                para(cu, body(tr!("The Library could not be measured: {error}", error = e)));
                if widgets::activated(&Button::new(tr("Measure again")).variant(Variant::Secondary).size(Size::Sm).id_salt("storage-measure").show(cu)) {
                    measure_again = true;
                }
                false
            }
            Some(Ok(plan)) => match &plan.verdict {
                Verdict::Ready { cache_root, free } => {
                    para(cu, body(tr!(
                        "Repair storage moves this project's generated Library to a disk with room: {size} in {files} to {folder} ({free} free there). Assets, Packages and settings stay where they are.",
                        size = bytes(plan.bytes),
                        files = files(plan.files),
                        folder = cache_root.display(),
                        free = bytes(*free)
                    )));
                    para(cu, note(tr!(
                        "Every file is checked before the old Library goes, and you can cancel until then. {disk} has to be connected whenever you use this project.",
                        disk = starting(&drive_of(cache_root))
                    )));
                    true
                }
                Verdict::Redirected { to } => {
                    let text = match to {
                        Some(path) => tr!("This project's Library is already on another disk ({path}), and that is the one running low. Free some space there.", path = path.display()),
                        None => tr("This project's Library is already on another disk, and that is the one running low. Free some space there.").to_string(),
                    };
                    para(cu, body(text));
                    false
                }
                Verdict::HasLink { at } => {
                    para(cu, body(tr!("Its Library holds a linked folder ({path}), so Orbiters does not move it. Free some space on {disk}.", path = at.display(), disk = disk)));
                    false
                }
                Verdict::NoRoom { needed, best } => {
                    if cfg!(target_os = "macos") && best.is_none() {
                        // Most Macs have the one disk.
                        para(cu, body(tr!("Moving it needs {size} on another disk (the Library and room for Unity's imports), and none is connected. Free some space on {disk}, or connect a disk with room.", size = bytes(*needed), disk = disk)));
                    } else if let Some((root, free)) = best {
                        para(cu, body(tr!("Moving it needs {size} on another disk (the Library and room for Unity's imports). The most on another disk is {free} on {disk}. Free some space on one of them.", size = bytes(*needed), free = bytes(*free), disk = drive_of(root))));
                    } else {
                        para(cu, body(tr!("Moving it needs {size} on another disk (the Library and room for Unity's imports). Free some space on one of them.", size = bytes(*needed))));
                    }
                    false
                }
                Verdict::Unsupported => {
                    para(cu, body(tr!("Free some space on {disk}.", disk = disk)));
                    false
                }
            },
        };
        if plan.as_ref().is_some_and(|p| p.is_err()) || (plan.is_some() && !ready) {
            return;
        }
        if health.open {
            para(cu, note(tr("Quit this project's Unity window first.")));
        } else if let Some(name) = &other_running {
            para(cu, note(tr!("Orbiters is moving the Library of {name}: one at a time.", name = name)));
        }
        cu.add_space(2.0);
        let enabled = ready && !health.open && other_running.is_none() && !action_running;
        let variant = if notice == Notice::OutOfSpace { Variant::Primary } else { Variant::Secondary };
        let resp = Button::new(tr("Repair storage")).icon(icons::HARD_DRIVE).variant(variant).size(Size::Sm).enabled(enabled).id_salt("storage-repair").show(cu);
        anchor = Some(resp.rect);
        if widgets::activated(&resp) {
            repair = true;
        }
    });
    if let Some(r) = anchor {
        state.anchors.insert("storage-repair".into(), r);
    }
    if measure_again {
        state.storage.plan = None;
        state.storage.plan_for = None;
    }
    if repair {
        state.start_repair(&ctx, project);
    }
}

/// How far a move of the shown project got. It is drawn in the place of the warning, in the same frame.
fn progress_card(ui: &mut Ui, state: &mut State) {
    let ctx = ui.ctx().clone();
    let Some(run) = state.storage.run.as_mut() else { return };
    let p = run.progress.locked().clone();
    if p.phase == Phase::Copying && run.copying_since.is_none() {
        run.copying_since = Some(Instant::now());
    }
    let cancelling = run.cancel.load(Ordering::Relaxed);
    let since = run.copying_since;
    let mut cancel = false;
    let mut anchor = None;
    frame(ui, "progress", icons::HARD_DRIVE, theme::accent(), |cu| {
        let title = heading(&p, cancelling);
        let (r, _) = cu.allocate_exact_size(vec2(cu.available_width(), 20.0), Sense::hover());
        feedback::spinner(cu.painter(), pos2(r.right() - 9.0, r.center().y), 6.0, 1.8, theme::accent());
        widgets::text_truncated(cu.painter(), pos2(r.left(), r.center().y), Align2::LEFT_CENTER, &Txt::new(title, 13.5).weight(weight::SEMIBOLD), r.width() - 24.0);
        if let Some(f) = p.fraction() {
            let (bar, _) = cu.allocate_exact_size(vec2(cu.available_width(), 6.0), Sense::hover());
            let pt = cu.painter();
            pt.rect_filled(bar, theme::radius(3.0), theme::white(0.08));
            let f = anim::value(&ctx, Id::unique(("storage-progress", p.phase as u8)), f, Spring::SOFT).clamp(0.0, 1.0);
            pt.rect_filled(Rect::from_min_size(bar.min, vec2((bar.width() * f).max(6.0), 6.0)), theme::radius(3.0), theme::accent());
        }
        let mut says = detail(&p);
        if p.phase == Phase::Copying {
            if let (Some(since), Some(f)) = (since, p.fraction()) {
                let elapsed = since.elapsed().as_secs_f64();
                if f > 0.01 && elapsed > 5.0 {
                    says.push_str(" · ");
                    says.push_str(&time_left(elapsed * (1.0 - f as f64) / f as f64));
                }
            }
        }
        if !says.is_empty() {
            para(cu, body(says));
        }
        if p.phase == Phase::Copying && !p.file.is_empty() {
            let (r, _) = cu.allocate_exact_size(vec2(cu.available_width(), 16.0), Sense::hover());
            widgets::text_truncated(cu.painter(), pos2(r.left(), r.center().y), Align2::LEFT_CENTER, &note(p.file.clone()), r.width());
        }
        para(cu, note(match p.phase {
            Phase::RemovingOld => tr("The project already uses its new Library. Unity can open it once the old copy is gone."),
            Phase::RemovingPartial => tr("The project's Library was not touched."),
            Phase::Switching => tr("This takes a moment and can no longer be cancelled."),
            _ => tr("The project's Library stays as it is until the copy is checked. Unity cannot open the project until this ends; everything else stays usable."),
        }));
        if p.cancellable() && !cancelling {
            cu.add_space(2.0);
            let resp = Button::new(tr("Cancel")).variant(Variant::Secondary).size(Size::Sm).id_salt("storage-cancel").show(cu);
            anchor = Some(resp.rect);
            if widgets::activated(&resp) {
                cancel = true;
            }
        }
    });
    if let Some(r) = anchor {
        state.anchors.insert("storage-cancel".into(), r);
    }
    if cancel {
        state.cancel_repair();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_unity_says_why_it_waits_during_a_move_of_that_project() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        let project = Project { name: "Avatorbit".into(), path: PathBuf::from(r"H:\metaverse\unity projects\Avatorbit"), ..Default::default() };
        state.storage.run = Some(Run {
            // (Where names differ by their case, it is the same name.)
            project: if cfg!(any(windows, target_os = "macos")) { PathBuf::from("h:/metaverse/unity projects/avatorbit/") } else { project.path.clone() },
            name: project.name.clone(),
            progress: SharedProgress::default(),
            cancel: Arc::default(),
            job: Job::default(),
            copying_since: None,
            still: true,
        });
        assert!(state.repairing(&project.path), "the same folder, however it is written");
        state.open_unity(&ctx, &project, false);
        let note = state.opening.note.clone().expect("said beside the button");
        assert!(note.words.contains("moving the Library of Avatorbit") && note.project == project.path);
        assert_eq!(state.selected.as_ref(), Some(&project.path), "pressed from a card's menu: its panel opens, where that is");
        assert!(!state.action.running(), "no attempt to open it was started");
        // Another project opens meanwhile: the move holds nothing of the page's (one action at a time is for short ones).
        let other = Project { name: "Other".into(), path: PathBuf::from("Z:/no/such/project"), ..Default::default() };
        assert!(!state.repairing(&other.path));
        assert!(!state.running());
    }

    #[test]
    fn the_capsule_shows_a_move_only_where_the_card_does_not() {
        let mut state = State::default();
        let path = PathBuf::from("H:/Avatorbit");
        state.storage.run = Some(Run {
            project: path.clone(),
            name: "Avatorbit".into(),
            progress: Arc::new(std::sync::Mutex::new(Progress { phase: Phase::Copying, bytes_total: 100, bytes_copied: 50, bytes_checked: 50, destination: Some(PathBuf::from(r"I:\Orbiters\UnityCache\abc")), ..Progress::default() })),
            cancel: Arc::default(),
            job: Job::default(),
            copying_since: None,
            still: true,
        });
        let (label, _, fraction, cancel) = state.repair_capsule().expect("no panel: the capsule");
        assert_eq!(label, if cfg!(target_os = "macos") { "Avatorbit: Copying the Library to the startup disk" } else { "Avatorbit: Copying the Library to I:" });
        assert_eq!(fraction, Some(0.5));
        assert!(cancel);
        state.selected = Some(PathBuf::from("H:/Other"));
        assert!(state.repair_capsule().is_some());
        state.selected = Some(path);
        assert!(state.repair_capsule().is_none(), "its panel shows the card");
    }

    #[test]
    fn past_the_switch_the_move_can_no_longer_be_cancelled() {
        for (phase, can) in [(Phase::Leftovers, true), (Phase::Measuring, true), (Phase::Copying, true), (Phase::Switching, false), (Phase::RemovingOld, false), (Phase::RemovingPartial, false)] {
            assert_eq!(Progress { phase, ..Progress::default() }.cancellable(), can, "{phase:?}");
        }
    }

    #[test]
    fn time_left_is_said_roughly() {
        assert_eq!(time_left(20.0), "under a minute left");
        assert_eq!(time_left(600.0), "about 10 min left");
        assert_eq!(time_left(4320.0), "about 1 h 12 min left");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Screenshots.
// ---------------------------------------------------------------------------------------------------------------------

/// How much a real run moves: enough to be seen in the middle of (`ORBITERS_SCREENSHOT_REPAIR_MB`).
fn real_size_mb() -> usize {
    std::env::var("ORBITERS_SCREENSHOT_REPAIR_MB").ok().and_then(|s| s.parse().ok()).unwrap_or(64)
}

impl State {
    /// Screenshots: the card's states, and (`projects-repair-real*`) a real Library move of a stand-in project in the
    /// run's sandbox, started and cancelled by presses on the card's own buttons.
    pub(super) fn stage_storage(&mut self, name: &str) {
        if !name.starts_with("projects-storage") && !name.starts_with("projects-repair") {
            return;
        }
        let first = self.scan.projects[0].path.clone();
        self.selected = Some(first.clone());
        const GB: u64 = 1_000_000_000;
        let (project_disk, cache_root) = if cfg!(target_os = "macos") { ("/", "/Volumes/Archive/Orbiters/UnityCache") } else { (r"H:\", r"I:\Orbiters\UnityCache") };
        let project_disk = PathBuf::from(project_disk);
        let low = Health { free: Some(7_200_000_000), disk: Some(project_disk.clone()), ..Health::default() };
        let cache_root = PathBuf::from(cache_root);
        let ready = |bytes: u64| Plan { bytes, files: 212_431, verdict: Verdict::Ready { cache_root: cache_root.clone(), free: 1_240 * GB } };
        let mut fixture = Fixture { health: low.clone(), plan: Some(Ok(ready(38_200_000_000))), measuring: None };
        match name {
            "projects-storage-failed" => fixture.health.disk_error = true,
            "projects-storage-earlier" => fixture.health = Health { free: Some(41 * GB), disk_error: true, disk: Some(project_disk), open: false },
            "projects-storage-unity-open" => fixture.health.open = true,
            "projects-storage-measuring" => {
                fixture.plan = None;
                fixture.measuring = Some(Progress { phase: Phase::Measuring, bytes_total: 12_300_000_000, files_total: 41_233, ..Progress::default() });
            }
            "projects-storage-no-room" => {
                fixture.plan = Some(Ok(Plan {
                    bytes: 38_200_000_000,
                    files: 212_431,
                    verdict: Verdict::NoRoom { needed: 46_800_000_000, best: Some((PathBuf::from(if cfg!(target_os = "macos") { "/Volumes/Archive/Orbiters/UnityCache" } else { r"D:\Orbiters\UnityCache" }), 20 * GB)) },
                }));
            }
            "projects-storage-redirected" => {
                fixture.plan = Some(Ok(Plan { bytes: 0, files: 0, verdict: Verdict::Redirected { to: Some(PathBuf::from(if cfg!(target_os = "macos") { "/Volumes/Archive/Orbiters/UnityCache/4b8c601f16724fd9aa5a31e1b3af4335" } else { r"I:\Orbiters\UnityCache\4b8c601f16724fd9aa5a31e1b3af4335" })) } }));
            }
            "projects-repair-copying" | "projects-repair-cancelling" | "projects-repair-removing" | "projects-repair-other" => {
                let mut p = Progress {
                    phase: Phase::Copying,
                    bytes_total: 38_200_000_000,
                    files_total: 212_431,
                    bytes_copied: 14_900_000_000,
                    bytes_checked: 14_700_000_000,
                    files_done: 81_204,
                    file: r"Artifacts\4f\4f1a9e02b6c7d3e8f5a4c0b1d2e3f4a5".into(),
                    destination: Some(cache_root.join("4b8c601f16724fd9aa5a31e1b3af4335")),
                    ..Progress::default()
                };
                if name == "projects-repair-cancelling" {
                    p.phase = Phase::RemovingPartial;
                    p.files_removed = 30_112;
                    p.files_to_remove = 81_205;
                } else if name == "projects-repair-removing" {
                    p.phase = Phase::RemovingOld;
                    p.files_done = 212_431;
                    p.files_removed = 90_500;
                    p.files_to_remove = 212_431;
                }
                let run_name = self.scan.projects[0].name.clone();
                self.storage.run = Some(Run {
                    project: first.clone(),
                    name: run_name,
                    progress: Arc::new(std::sync::Mutex::new(p)),
                    cancel: Arc::new(AtomicBool::new(false)),
                    job: Job::default(),
                    copying_since: Some(Instant::now() - Duration::from_secs(1500)),
                    still: true,
                });
                if name == "projects-repair-other" {
                    // Another project's panel: it can be opened, and its own repair waits.
                    self.selected = Some(self.scan.projects[1].path.clone());
                }
            }
            n if n.starts_with("projects-repair-real") => {
                // A real project on a real disk: the sandbox's, never the person's (`storage::CACHE_DIR_VAR`).
                let root = orbiters_core::paths::cache_dir().join("repair-stand-in");
                let project = root.join("Stand-in project");
                let library = project.join("Library/Artifacts");
                std::fs::create_dir_all(&library).expect("stand-in Library");
                std::fs::create_dir_all(project.join("ProjectSettings")).expect("stand-in project");
                std::fs::write(project.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").expect("stand-in project");
                let chunk: Vec<u8> = (0..4usize * 1024 * 1024).map(|i| (i.wrapping_mul(31) >> 5) as u8).collect();
                for i in 0..real_size_mb() / 4 {
                    std::fs::write(library.join(format!("{i:05}.bin")), &chunk).expect("stand-in Library");
                }
                std::env::set_var(storage::CACHE_DIR_VAR, root.join("UnityCache"));
                if let Some(p) = self.scan.projects.first_mut() {
                    p.path = project.clone();
                }
                self.selected = Some(project);
                // The disk reads as low and the plan is the real one, measured by the card.
                fixture.plan = None;
            }
            _ => {}
        }
        self.storage.fixture = Some(fixture);
    }

    /// Screenshots: presses on the card's own buttons in a real run. Started as soon as the plan allows it; cancelled once
    /// `projects-repair-real-cancel` has copied a quarter of the bytes.
    pub(super) fn stage_storage_frame(&mut self, name: &str, frame: u32) -> Option<Poke> {
        if !name.starts_with("projects-repair-real") || frame < 5 {
            return None;
        }
        let Some(run) = &self.storage.run else {
            if self.stage_repair_started {
                // Ended: the moved project's disk is no longer the low one, a cancelled one's still is.
                if name == "projects-repair-real-done" {
                    self.storage.fixture = None;
                }
                return None;
            }
            let ready = self.storage.plan.as_ref().is_some_and(|(_, p)| matches!(p, Ok(Plan { verdict: Verdict::Ready { .. }, .. })));
            if ready {
                if let Some(r) = self.anchors.get("storage-repair") {
                    self.stage_repair_started = true;
                    return Some(Poke::Press(r.center()));
                }
            }
            return None;
        };
        if name == "projects-repair-real-cancel" {
            let p = run.progress.locked().clone();
            if p.phase == Phase::Copying && p.bytes_copied * 4 > p.bytes_total && !run.cancel.load(Ordering::Relaxed) {
                return self.anchors.get("storage-cancel").map(|r| Poke::Press(r.center()));
            }
        }
        None
    }
}
