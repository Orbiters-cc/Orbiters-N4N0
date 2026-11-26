//! What became of Open in Unity, said where it was pressed, and the editor installed from there when it is missing.
//!
//! **Where.** One card ([`card`]) beside the button, on every place that has one: under the panel's hero (and under its compact bar once scrolled), in the preview's window over
//! its button, in the set-up dialog and on the Unity 6 sheet. A card's own menu opens the panel, where the card is. (It was a banner far down the panel: a press looked like
//! nothing, the owner's first try on a Mac, 2026-11-25.) The button itself turns meanwhile ([`State::opening_label`]).
//!
//! **Installing** ([`orbiters_projects::unity_install`]) is a Mac's, as Home sets VRChat's engine up: one button, what it is at, Cancel, and what only the person can do as a
//! step of its own. Elsewhere the card has what there was: Unity Hub, on that very editor.

use orbiters_core::locks::*;
use super::state::{Outcome, State};
use crate::pages::{Cx, Job};
use egui::{pos2, vec2, Id, Rect, Sense, Ui, UiBuilder};
use orbiters_i18n::{mark, tr};
use orbiters_projects::discovery::{self, Project};
use orbiters_projects::unity_install::{End, NotInstalled, Step, Wanted};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, Size, Variant};
use orbiters_ui::{feedback, format, icons, toast};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Why a project did not open.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Why {
    /// The editor it needs is not here (or could not be verified).
    Missing(Wanted),
    /// Unity quit at once, and no licence of its is seen here.
    Licence(Wanted),
    /// Unity Hub itself did not open.
    Hub,
    /// Anything else, as its words say it.
    Said,
}

/// What a press of Open in Unity came to, until it is pressed again or closed.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Note {
    pub project: PathBuf,
    pub words: String,
    pub why: Why,
    /// Unity Hub is here: looked for as the note is made, never as it is drawn.
    pub hub: bool,
}

impl Note {
    pub(super) fn said(project: &Path, words: impl Into<String>) -> Self {
        Self { project: project.to_path_buf(), words: words.into(), why: Why::Said, hub: true }
    }

    /// Of an error of Open in Unity's.
    pub(super) fn of(project: &Project, error: &anyhow::Error) -> Self {
        let words = format!("{error:#}");
        let wanted = |version: &str, changeset: Option<String>| Wanted { version: version.to_owned(), changeset, project: project.path.clone(), name: project.name.clone() };
        let why = match error.downcast_ref::<NotInstalled>() {
            Some(n) => Why::Missing(wanted(&n.version, n.revision.clone())),
            None if error.downcast_ref::<orbiters_projects::QuitAtOnce>().is_some() && unlicensed() => Why::Licence(wanted(&project.unity, None)),
            None => Why::Said,
        };
        Self { project: project.path.clone(), words, hub: orbiters_projects::unity_hub().is_some(), why }
    }
}

/// No licence of Unity's is seen on this computer (asked on a Mac alone).
fn unlicensed() -> bool {
    #[cfg(target_os = "macos")]
    {
        use orbiters_projects::unity_install as install;
        let cli = orbiters_projects::unity_hub().map(|p| install::HubCli::of(&p));
        !matches!(install::licence(cli.as_ref()), install::Licence::Here)
    }
    #[cfg(not(target_os = "macos"))]
    false
}

/// The install running, or how the last one ended.
#[derive(Default)]
pub(super) struct Install {
    run: Option<Run>,
    ended: Option<(Wanted, End)>,
    /// An earlier window's was looked for.
    looked: bool,
}

struct Run {
    wanted: Wanted,
    step: Arc<Mutex<Step>>,
    stop: Arc<AtomicBool>,
    anyway: Arc<AtomicBool>,
    job: Job<Result<(), End>>,
}

/// How an install of the editor is done here: the computer's own way (a test has its own).
type Work = Arc<dyn Fn(&Wanted, bool, &dyn Fn(Step), &AtomicBool, &AtomicBool) -> Result<(), End> + Send + Sync>;

fn work_here() -> Option<Work> {
    #[cfg(target_os = "macos")]
    {
        Some(Arc::new(|wanted, in_hub, say, stop, anyway| orbiters_projects::unity_install::run(&orbiters_projects::unity_install::World::here(), wanted, in_hub, say, stop, anyway)))
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// Open in Unity's own state.
#[derive(Default)]
pub(super) struct Opening {
    /// The project being opened now.
    pub now: Option<PathBuf>,
    pub note: Option<Note>,
    pub install: Install,
    /// Unity Hub being opened, for a project's card.
    pub hub: Job<(PathBuf, Result<(), String>)>,
    /// How an editor is installed, when not this computer's way.
    pub work: Option<Work>,
    /// Screenshots and tests: what a Mac's card offers is offered.
    pub offers: Option<bool>,
}

impl Opening {
    fn offers(&self) -> bool {
        self.offers.unwrap_or(cfg!(target_os = "macos"))
    }

    /// A state for a screenshot: the install of `wanted` at `step`, or ended so.
    pub(super) fn fixture(wanted: Wanted, step: Option<Step>, ended: Option<End>) -> Self {
        let run = step.map(|s| Run { wanted: wanted.clone(), step: Arc::new(Mutex::new(s)), stop: Arc::default(), anyway: Arc::default(), job: Job::default() });
        Self { install: Install { run, ended: ended.map(|e| (wanted, e)), looked: true }, offers: Some(true), ..Self::default() }
    }

    /// The project whose install ended before its editor was there.
    pub(super) fn ended_for(&self) -> Option<PathBuf> {
        self.install.ended.as_ref().map(|(w, _)| w.project.clone())
    }

    /// The project an install is for.
    pub(super) fn installing(&self) -> Option<&Wanted> {
        self.install.run.as_ref().map(|r| &r.wanted)
    }
}

impl State {
    /// What Open in Unity's button says while `path` opens: the button itself shows it was pressed.
    pub(super) fn opening_label(&self, path: &Path) -> Option<&'static str> {
        if self.opening.now.as_deref().is_some_and(|p| discovery::key(p) == discovery::key(path)) && self.running() {
            return Some(tr("Opening Unity…"));
        }
        let installing = self.opening.installing().is_some_and(|w| discovery::key(&w.project) == discovery::key(path));
        installing.then(|| tr("Installing Unity…"))
    }

    /// A press of Open in Unity came to this.
    pub(super) fn not_opened(&mut self, ctx: &egui::Context, note: Note) {
        let path = note.project.clone();
        self.opening.note = Some(note);
        // Pressed from a card's menu, with nothing of the project on screen: its panel has the card at its top.
        if self.selected.as_deref().map(discovery::key) != Some(discovery::key(&path)) && self.modal.is_none() {
            self.open(ctx, path);
        }
    }

    /// Whether something of Open in Unity is to be said about `path`.
    pub(super) fn says_of_opening(&self, path: &Path) -> bool {
        let key = discovery::key(path);
        let of = |p: &Path| discovery::key(p) == key;
        let o = &self.opening;
        o.note.as_ref().is_some_and(|n| of(&n.project)) || o.install.run.as_ref().is_some_and(|r| of(&r.wanted.project)) || o.install.ended.as_ref().is_some_and(|(w, _)| of(&w.project))
    }

    /// Starts installing the editor of `wanted`, or goes on with it. `in_hub`: in Unity Hub's own window.
    pub(super) fn install_unity(&mut self, ctx: &egui::Context, wanted: Wanted, in_hub: bool) {
        if self.opening.install.run.is_some() || self.fixture && self.opening.work.is_none() {
            return;
        }
        let Some(work) = self.opening.work.clone().or_else(work_here) else { return };
        self.opening.note = None;
        self.opening.install.ended = None;
        let (step, stop, anyway): (_, Arc<AtomicBool>, Arc<AtomicBool>) = (Arc::new(Mutex::new(Step::Checking)), Arc::default(), Arc::default());
        let job = {
            let (wanted, step, stop, anyway, ctx) = (wanted.clone(), step.clone(), stop.clone(), anyway.clone(), ctx.clone());
            Job::spawn(&ctx.clone(), move || {
                let say = |s: Step| {
                    let mut now = step.locked();
                    if *now != s {
                        *now = s;
                        ctx.request_repaint();
                    }
                };
                work(&wanted, in_hub, &say, &stop, &anyway)
            })
        };
        self.opening.install.run = Some(Run { wanted, step, stop, anyway, job });
    }

    /// Unity Hub, from beside Open in Unity of the project in `path`: what keeps it from opening is said there.
    pub(super) fn open_hub_for(&mut self, ctx: &egui::Context, path: &Path) {
        if self.fixture || self.opening.hub.running() {
            return;
        }
        let path = path.to_path_buf();
        self.opening.hub = Job::spawn(ctx, move || (path, orbiters_projects::open_hub().map_err(|e| format!("{e:#}"))));
    }

    /// The capsule of an install whose project's panel is not on screen: its words, how far it is.
    pub(super) fn install_capsule(&self) -> Option<(String, Option<f32>)> {
        let run = self.opening.install.run.as_ref()?;
        if self.selected.as_deref().is_some_and(|p| discovery::key(p) == discovery::key(&run.wanted.project)) {
            return None;
        }
        let says = of_step(&run.step.locked(), &run.wanted, run.stop.load(Ordering::Relaxed));
        Some((format!("{}: {}", run.wanted.name, says.title), says.bar.flatten()))
    }

    pub(super) fn cancel_install(&mut self) {
        if let Some(run) = &self.opening.install.run {
            run.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Takes in an install that ended, from whatever page shows; and, once, one an earlier window left.
    pub fn poll_opening(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        self.look_for_left(ctx);
        if let Some((project, Err(why))) = self.opening.hub.take() {
            self.opening.note = Some(Note { project, words: why, why: Why::Hub, hub: false });
        }
        let Some(run) = self.opening.install.run.as_mut() else { return };
        let Some(end) = run.job.take().or_else(|| run.job.died().then(|| Err(End::Failed(tr("The installation stopped.").into())))) else { return };
        let run = self.opening.install.run.take().expect("just looked at");
        match end {
            Ok(()) => {
                cx.toasts.push(ctx, toast::Kind::Success, tr!("Unity {version} is installed.", version = run.wanted.version));
                // As asked in the first place.
                let project = self.scan.projects.iter().find(|p| discovery::key(&p.path) == discovery::key(&run.wanted.project)).cloned();
                let project = project.unwrap_or_else(|| Project { path: run.wanted.project.clone(), name: run.wanted.name.clone(), unity: run.wanted.version.clone(), registered: true, ..Project::default() });
                self.open_unity(ctx, &project, false);
            }
            Err(end) => {
                // Said wherever the person is; the card has the rest.
                if end != End::Stopped {
                    cx.toasts.push(ctx, toast::Kind::Error, of_end(&end, &run.wanted).title);
                }
                self.opening.install.ended = Some((run.wanted, end));
            }
        }
    }

    /// An install an earlier window began: followed while it runs, offered to go on with once it does not.
    fn look_for_left(&mut self, ctx: &egui::Context) {
        if std::mem::replace(&mut self.opening.install.looked, true) || self.fixture {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            use orbiters_projects::unity_install as install;
            if cfg!(test) {
                return;
            }
            if let Some(record) = install::left(&install::folder()) {
                if install::running(&record) {
                    self.install_unity(ctx, record.wanted, false);
                } else {
                    self.opening.install.ended = Some((record.wanted, End::Stopped));
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = ctx;
    }
}

fn para(ui: &mut Ui, txt: Txt) {
    let w = ui.available_width();
    let galley = ui.painter().layout_job(txt.job(w));
    let (r, _) = ui.allocate_exact_size(vec2(w, galley.size().y), Sense::hover());
    ui.painter().galley(r.min, galley, theme::fg());
}

fn body(text: impl Into<String>) -> Txt {
    Txt::new(text.into(), 12.5).color(theme::fg_2()).line_height(18.0)
}

/// What a button of the card does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Act {
    Install,
    InHub,
    Cancel,
    Hub,
    GetHub,
    HubLink,
    Anyway,
}

/// What the card shows: its title, its words, the bar (none: no bar; `Some(None)`: no end in sight), its buttons (the first stands out).
struct Says {
    tint: egui::Color32,
    icon: &'static str,
    title: String,
    words: String,
    bar: Option<Option<f32>>,
    buttons: Vec<(&'static str, &'static str, Act)>,
    closes: bool,
    wanted: Option<Wanted>,
}

fn of_step(step: &Step, wanted: &Wanted, stopping: bool) -> Says {
    let version = wanted.version.as_str();
    let cancel = vec![(tr("Cancel"), icons::X, Act::Cancel)];
    let working = |title: String, words: String, bar: Option<Option<f32>>| Says { tint: theme::accent(), icon: icons::DOWNLOAD_SIMPLE, title, words, bar, buttons: cancel.clone(), closes: false, wanted: Some(wanted.clone()) };
    let then = tr!("{name} opens by itself once it is there. You can go on using Orbiters, or close it: the installation goes on.", name = wanted.name);
    let mut says = match step {
        Step::Checking => working(tr!("Installing Unity {version}", version = version), tr("Looking at what it takes…").into(), Some(None)),
        Step::Hub { have, total } => working(
            tr("Getting Unity Hub, which installs Unity").into(),
            if *total > 0 { tr!("{have} of {total}", have = format::bytes(*have as f64), total = format::bytes(*total as f64)) } else { tr("From Unity's website…").into() },
            Some((*total > 0).then(|| *have as f32 / *total as f32)),
        ),
        Step::PlacingHub => working(tr("Getting Unity Hub, which installs Unity").into(), tr("Checking Unity's signature and putting it in Applications…").into(), Some(None)),
        Step::Editor { percent, installing: false, bytes } => working(tr!("Downloading Unity {version}: {percent} %", version = version, percent = percent), tr!("About {size} in all, with what builds for a PC. {then}", size = format::bytes(*bytes as f64), then = then), Some(Some(f32::from(*percent) / 100.0))),
        Step::Editor { installing: true, .. } => working(tr!("Putting Unity {version} in place", version = version), then, Some(None)),
        Step::Waiting => working(tr!("Installing Unity {version}", version = version), tr("Unity Hub is installing something else first.").into(), Some(None)),
        Step::InHub => Says {
            icon: icons::ARROW_SQUARE_OUT,
            buttons: vec![(tr("Open Unity Hub"), icons::CUBE, Act::HubLink), (tr("Cancel"), icons::X, Act::Cancel)],
            ..working(
                tr!("Install Unity {version} in Unity Hub", version = version),
                tr!("Unity Hub is open on it: press Install there, for Apple silicon, with Windows Build Support (Mono). macOS may ask for your password. {name} opens by itself once Unity is there.", name = wanted.name),
                Some(None),
            )
        },
        Step::Verifying => working(tr!("Checking Unity {version}", version = version), String::new(), Some(None)),
        Step::Licence { signed_in } => Says {
            tint: theme::WARN,
            icon: icons::USER,
            buttons: vec![(tr("Open Unity Hub"), icons::CUBE, Act::Hub), (tr("Open the project anyway"), icons::PLAY, Act::Anyway), (tr("Cancel"), icons::X, Act::Cancel)],
            ..working(
                if *signed_in { tr("One step is yours: a Unity licence").into() } else { tr("One step is yours: sign in to Unity").into() },
                if *signed_in {
                    tr!("Unity {version} is installed. In Unity Hub, get the free Personal licence (Licenses, Add): its terms are yours to accept. {name} opens by itself once it is active.", version = version, name = wanted.name)
                } else {
                    tr!("Unity {version} is installed. In Unity Hub, sign in to your Unity account (or make one) and get the free Personal licence: the account and its terms are yours. {name} opens by itself once it is active.", version = version, name = wanted.name)
                },
                None,
            )
        },
    };
    if stopping {
        says.title = tr("Stopping…").into();
        says.buttons.clear();
        says.bar = Some(None);
    }
    says
}

fn of_end(end: &End, wanted: &Wanted) -> Says {
    let version = wanted.version.as_str();
    let says = |tint, icon, title: String, words: String, buttons| Says { tint, icon, title, words, bar: None, buttons, closes: true, wanted: Some(wanted.clone()) };
    match end {
        End::Stopped => says(
            theme::fg_3(),
            icons::DOWNLOAD_SIMPLE,
            tr!("Unity {version} is not installed yet", version = version),
            tr!("Its installation, for {name}, was stopped. What was downloaded is kept: it goes on from there.", name = wanted.name),
            vec![(tr("Go on installing"), icons::DOWNLOAD_SIMPLE, Act::Install)],
        ),
        End::Room { at, need, free } => says(
            theme::WARN,
            icons::HARD_DRIVE,
            tr!("Not enough room for Unity {version}", version = version),
            match free {
                Some(free) => tr!("It takes {need} on this disk ({place}), the download and the room Unity needs to work included, and {free} is free. Nothing was downloaded. Free some space, then check again.", need = format::bytes(*need as f64), place = at.display(), free = format::bytes(*free as f64)),
                None => tr!("It takes {need} on this disk ({place}), and what is free there could not be read. Nothing was downloaded.", need = format::bytes(*need as f64), place = at.display()),
            },
            vec![(tr("Check again"), icons::ARROW_COUNTER_CLOCKWISE, Act::Install)],
        ),
        End::Failed(why) => says(
            theme::DANGER,
            icons::WARNING,
            tr!("Unity {version} was not installed", version = version),
            why.clone(),
            vec![(tr("Try again"), icons::ARROW_COUNTER_CLOCKWISE, Act::Install), (tr("Install it in Unity Hub"), icons::ARROW_SQUARE_OUT, Act::InHub)],
        ),
    }
}

fn of_note(note: &Note, offers: bool) -> Says {
    let says = |title: &str, words: String, buttons, wanted: Option<&Wanted>| Says { tint: theme::DANGER, icon: icons::WARNING, title: title.into(), words, bar: None, buttons, closes: true, wanted: wanted.cloned() };
    match &note.why {
        Why::Missing(w) if offers => Says {
            tint: theme::WARN,
            icon: icons::DOWNLOAD_SIMPLE,
            ..says(
                &tr!("{name} needs Unity {version}", name = w.name, version = w.version),
                tr("Orbiters installs it: it gets Unity Hub when it is missing, then that Unity, and opens the project. You only sign in to Unity if you are not yet.").into(),
                vec![(INSTALL, icons::DOWNLOAD_SIMPLE, Act::Install)],
                Some(w),
            )
        },
        // Elsewhere: Unity Hub, on that editor when its build is known.
        Why::Missing(w) => {
            let mut buttons = Vec::new();
            match (note.hub, w.hub_link().is_some()) {
                (true, true) => buttons.push((tr("Install with Unity Hub"), icons::ARROW_SQUARE_OUT, Act::HubLink)),
                (true, false) => buttons.push((tr("Open Unity Hub"), icons::CUBE, Act::Hub)),
                (false, _) => buttons.push((tr("Get Unity Hub"), icons::ARROW_SQUARE_OUT, Act::GetHub)),
            }
            says(tr("Unity did not open"), note.words.clone(), buttons, Some(w))
        }
        Why::Licence(w) => says(
            tr("Unity quit at once"),
            format!("{} {}", note.words, tr("No Unity licence is seen on this Mac: sign in and get the free Personal licence in Unity Hub, then open the project again.")),
            vec![(tr("Open Unity Hub"), icons::CUBE, Act::Hub)],
            Some(w),
        ),
        Why::Hub => says(tr("Unity Hub did not open"), note.words.clone(), vec![(tr("Get Unity Hub"), icons::ARROW_SQUARE_OUT, Act::GetHub)], None),
        Why::Said => says(tr("Unity did not open"), note.words.clone(), vec![], None),
    }
}

/// The install's button, whose words have the version in them ([`label`]).
const INSTALL: &str = mark("Install Unity {version}");

fn label(text: &'static str, wanted: Option<&Wanted>) -> String {
    match (text, wanted) {
        (INSTALL, Some(w)) => tr!("Install Unity {version}", version = w.version),
        (t, _) => t.to_string(),
    }
}

/// The card beside Open in Unity of `project`, when there is something to say of it. Whether it was drawn.
pub(super) fn card(ui: &mut Ui, cx: &mut Cx, state: &mut State, project: &Path, salt: &str) -> bool {
    let ctx = ui.ctx().clone();
    let of = |p: &Path| discovery::key(p) == discovery::key(project);
    let o = &state.opening;
    let says = if let Some(run) = o.install.run.as_ref().filter(|r| of(&r.wanted.project)) {
        let step = run.step.locked().clone();
        of_step(&step, &run.wanted, run.stop.load(Ordering::Relaxed))
    } else if let Some(note) = o.note.as_ref().filter(|n| of(&n.project)) {
        let mut says = of_note(note, o.offers());
        // One at a time, as the Hub itself.
        if let (Some(other), true) = (o.installing(), says.buttons.iter().any(|b| b.2 == Act::Install)) {
            says.words = tr!("Unity {version} is being installed for {name}: this one can follow it.", version = other.version, name = other.name);
            says.buttons.clear();
        }
        says
    } else if let Some((wanted, end)) = o.install.ended.as_ref().filter(|(w, _)| of(&w.project)) {
        of_end(end, wanted)
    } else {
        return false;
    };
    let id = Id::unique(("opening-card", salt));
    let w = ui.available_width();
    let top_left = ui.cursor().min;
    let bg = ui.painter().add(egui::Shape::Noop);
    let right = if says.closes { 48.0 } else { 16.0 };
    let inner = Rect::from_min_size(top_left + vec2(44.0, 12.0), vec2(w - 44.0 - right, 100_000.0));
    let mut cu = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)).id_salt(id));
    cu.spacing_mut().item_spacing = vec2(8.0, 6.0);
    para(&mut cu, Txt::new(says.title.clone(), 13.5).weight(weight::SEMIBOLD).line_height(19.0));
    if !says.words.is_empty() {
        para(&mut cu, body(says.words.clone()));
    }
    if let Some(bar) = says.bar {
        let (r, _) = cu.allocate_exact_size(vec2(cu.available_width(), 6.0), Sense::hover());
        let pt = cu.painter();
        pt.rect_filled(r, theme::radius(3.0), theme::white(0.08));
        let filled = match bar {
            Some(f) => {
                let f = anim::value(&ctx, id.with("bar"), f, Spring::SOFT).clamp(0.0, 1.0);
                Rect::from_min_size(r.min, vec2((r.width() * f).max(6.0), r.height()))
            }
            // No end in sight: a piece that goes along it.
            None => {
                let t = (anim::time(&ctx) * 0.6).rem_euclid(1.0) as f32;
                anim::request_frame(&ctx);
                let piece = r.width() * 0.25;
                let x = r.left() + (r.width() + piece) * t - piece;
                Rect::from_min_max(pos2(x.max(r.left()), r.top()), pos2((x + piece).min(r.right()), r.bottom()))
            }
        };
        pt.rect_filled(filled, theme::radius(3.0), theme::accent());
        cu.add_space(2.0);
    }
    // What Unity says it takes, when VRChat's release list has it.
    if let Some(size) = says.buttons.iter().any(|b| b.2 == Act::Install && b.0 == INSTALL).then(|| state.u6.matrix().row_of_editor(&says.wanted.as_ref()?.version)?.size_here().cloned()).flatten() {
        para(&mut cu, body(tr!("About {download} to download, {installed} on disk.", download = format::bytes(size.download as f64), installed = format::bytes(size.installed as f64))).color(theme::fg_3()));
    }
    let mut pressed = None;
    if !says.buttons.is_empty() {
        let labels: Vec<String> = says.buttons.iter().map(|(l, _, _)| label(l, says.wanted.as_ref())).collect();
        let widths: Vec<f32> = labels.iter().zip(&says.buttons).map(|(l, (_, g, _))| Button::new(l).icon(g).size(Size::Sm).desired_width(&cu)).collect();
        let (spots, h) = widgets::flow(&widths, cu.available_width(), 32.0, 8.0);
        let (r, _) = cu.allocate_exact_size(vec2(cu.available_width(), h), Sense::hover());
        for (i, (l, at)) in labels.iter().zip(spots).enumerate() {
            let (_, glyph, act) = says.buttons[i];
            let variant = if i == 0 && act != Act::Cancel { Variant::Primary } else { Variant::Secondary };
            let rect = Rect::from_min_size(r.min + at, vec2(widths[i], 32.0));
            let b = Button::new(l).icon(glyph).size(Size::Sm).variant(variant).id_salt((id, act as u8));
            state.anchors.insert(format!("opening-{salt}-{act:?}"), rect);
            if widgets::activated(&b.show_at(&mut cu, rect)) {
                pressed = Some(act);
            }
        }
    }
    let h = cu.min_rect().height() + 12.0 + 14.0;
    let rect = Rect::from_min_size(top_left, vec2(w, h));
    state.anchors.insert(format!("opening-{salt}"), rect);
    let p = ui.painter();
    let fill = theme::mix(theme::ink_900(), says.tint, 0.12);
    p.set(bg, egui::Shape::mesh(orbiters_ui::paint::squircle_mesh(rect, theme::squircle(14.0), fill, 1.0 / ctx.pixels_per_point())));
    let icon_at = pos2(rect.left() + 24.0, rect.top() + 21.5);
    if says.bar.is_some() {
        feedback::spinner(p, icon_at, 7.0, 2.0, theme::accent());
    } else {
        widgets::icon(p, icon_at, says.icon, 16.0, says.tint);
    }
    let mut close = false;
    if says.closes {
        let at = Rect::from_center_size(pos2(rect.right() - 24.0, rect.top() + 22.0), vec2(28.0, 28.0));
        close = widgets::activated(&IconButton::new(icons::X, mark("Close")).size(28.0, 13.0).id_salt((id, "close")).show_at(ui, at));
    }
    ui.allocate_exact_size(vec2(w, h), Sense::hover());
    ui.add_space(2.0);
    if close {
        state.opening.note = None;
        state.opening.install.ended = None;
    }
    if let Some(act) = pressed {
        do_it(&ctx, cx, state, act, says.wanted);
    }
    true
}

fn do_it(ctx: &egui::Context, cx: &mut Cx, state: &mut State, act: Act, wanted: Option<Wanted>) {
    match (act, wanted) {
        (Act::Install, Some(w)) => state.install_unity(ctx, w, false),
        (Act::InHub, Some(w)) => state.install_unity(ctx, w, true),
        (Act::Cancel, _) => state.cancel_install(),
        (Act::Anyway, _) => {
            if let Some(run) = &state.opening.install.run {
                run.anyway.store(true, Ordering::Relaxed);
            }
        }
        (Act::HubLink, Some(w)) => {
            if let Some(link) = w.hub_link() {
                super::unity6_card::open_url(ctx, cx, &link);
            }
        }
        (Act::GetHub, _) => super::unity6_card::open_url(ctx, cx, "https://unity.com/download"),
        (Act::Hub, w) => {
            if let Some(project) = w.map(|w| w.project) {
                state.open_hub_for(ctx, &project);
            }
        }
        _ => {}
    }
}

/// [`Outcome`] of a press whose project did not open.
pub(super) fn outcome(project: &Project, error: &anyhow::Error) -> Outcome {
    Outcome::NotOpened(Box::new(Note::of(project, error)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, RawInput};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn wanted() -> Wanted {
        Wanted { version: "2022.3.22f1".into(), changeset: Some("887be4894c44".into()), project: PathBuf::from("/Projects/Avatar"), name: "Avatar".into() }
    }

    fn missing() -> Note {
        let project = Project { path: wanted().project, name: wanted().name, unity: wanted().version, ..Project::default() };
        let error = anyhow::Error::new(NotInstalled { version: wanted().version, revision: wanted().changeset, found: None, words: "Unity 2022.3.22f1 is not installed.".into() });
        Note::of(&project, &error)
    }

    /// What a test's install is told to do, and what it was asked.
    struct Told {
        step: mpsc::Sender<Result<Step, Result<(), End>>>,
        asked: Arc<Mutex<Vec<(Wanted, bool)>>>,
    }

    struct Harness {
        ctx: egui::Context,
        state: State,
        toasts: orbiters_ui::toast::Toasts,
        ctl: crate::backend::Ctl,
        snap: crate::state::Snapshot,
        frame: u32,
        told: Told,
        /// The whole page instead of the card alone.
        page: bool,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            orbiters_ui::install(&ctx);
            let mut state = State::default();
            state.fixture = true;
            state.opening.offers = Some(true);
            state.opening.install.looked = true;
            let (step, steps) = mpsc::channel();
            let steps = Mutex::new(steps);
            let asked: Arc<Mutex<Vec<(Wanted, bool)>>> = Arc::default();
            let log = asked.clone();
            state.opening.work = Some(Arc::new(move |w, in_hub, say, stop, anyway| {
                log.lock().unwrap().push((w.clone(), in_hub));
                let steps = steps.lock().unwrap();
                loop {
                    if stop.load(Ordering::Relaxed) {
                        return Err(End::Stopped);
                    }
                    if anyway.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    match steps.recv_timeout(Duration::from_millis(10)) {
                        Ok(Ok(step)) => say(step),
                        Ok(Err(end)) => return end,
                        Err(_) => {}
                    }
                }
            }));
            let snap = crate::state::Snapshot::new(orbiters_core::settings::Settings::default());
            Self { ctx, state, toasts: Default::default(), ctl: Arc::new(crate::screenshot::Still), snap, frame: 0, told: Told { step, asked }, page: false }
        }

        fn run(&mut self, events: Vec<Event>) {
            self.frame += 1;
            let view = Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 820.0));
            let input = RawInput { screen_rect: Some(view), time: Some(self.frame as f64 / 60.0), events, ..Default::default() };
            let (state, toasts, ctl, snap, page) = (&mut self.state, &mut self.toasts, &self.ctl, &self.snap, self.page);
            state.anchors.retain(|k, _| !k.starts_with("opening-"));
            let mut output = self.ctx.run_ui(input, |ui| {
                let mut cx = Cx { ctl, snap, toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 40.0, go: None };
                if page {
                    let mut child = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_max(pos2(230.0, 60.0), view.max - vec2(20.0, 20.0))));
                    super::super::show(&mut child, &mut cx, state);
                } else {
                    let ctx = ui.ctx().clone();
                    state.poll_opening(&ctx, &mut cx);
                    let mut child = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(pos2(20.0, 20.0), vec2(380.0, 700.0))).layout(egui::Layout::top_down(egui::Align::Min)));
                    card(&mut child, &mut cx, state, &wanted().project, "t");
                }
            });
            output.textures_delta.clear();
        }

        fn frames(&mut self, n: u32) {
            for _ in 0..n {
                self.run(vec![]);
            }
        }

        fn at(&self, act: &str) -> Option<Rect> {
            self.state.anchors.get(&format!("opening-t-{act}")).copied()
        }

        fn press(&mut self, act: &str) {
            let at = self.at(act).unwrap_or_else(|| panic!("no {act}: {:?}", self.state.anchors.keys().collect::<Vec<_>>())).center();
            self.run(vec![Event::PointerMoved(at)]);
            for pressed in [true, false] {
                self.run(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() }]);
            }
            self.frames(2);
        }

        fn until(&mut self, what: &str, good: impl Fn(&Self) -> bool) {
            let began = Instant::now();
            while !good(self) {
                assert!(began.elapsed() < Duration::from_secs(10), "{what} never came");
                self.run(vec![]);
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn asked(&self) -> Vec<(Wanted, bool)> {
            self.told.asked.lock().unwrap().clone()
        }
    }

    #[test]
    fn an_editor_that_is_missing_is_installed_from_the_card_and_its_project_opens() {
        let mut h = Harness::new();
        h.state.opening.note = Some(missing());
        assert!(matches!(&h.state.opening.note.as_ref().unwrap().why, Why::Missing(w) if *w == wanted()), "the version and the build are the project's");
        h.frames(2);
        h.press("Install");
        h.until("the install", |h| h.asked() == [(wanted(), false)]);
        assert_eq!(h.state.opening.note, None);
        assert_eq!(h.state.opening_label(&wanted().project), Some(tr("Installing Unity…")), "the button says it");
        h.told.step.send(Ok(Step::Editor { percent: 43, installing: false, bytes: 3_800_000_000 })).unwrap();
        h.until("the progress", |h| h.state.install_capsule().is_some_and(|(words, f)| words.contains("43") && f == Some(0.43)));
        assert!(h.at("Cancel").is_some() && h.at("Install").is_none());
        h.told.step.send(Ok(Step::Licence { signed_in: false })).unwrap();
        h.until("the person's step", |h| h.at("Hub").is_some());
        assert!(h.at("Anyway").is_some());
        h.told.step.send(Err(Ok(()))).unwrap();
        h.until("the project opening", |h| h.state.opening.now.as_deref() == Some(Path::new("/Projects/Avatar")));
        assert!(h.state.opening.installing().is_none());
    }

    #[test]
    fn a_cancelled_install_stays_and_goes_on() {
        let mut h = Harness::new();
        h.state.opening.note = Some(missing());
        h.frames(2);
        h.press("Install");
        h.until("the install", |h| !h.asked().is_empty());
        h.frames(2);
        h.press("Cancel");
        h.until("its end", |h| h.state.opening.ended_for().is_some());
        assert_eq!(h.state.opening.now, None, "nothing opens");
        h.frames(2);
        h.press("Install");
        h.until("going on", |h| h.asked().len() == 2);
        assert!(h.state.opening.ended_for().is_none());
        // With a licence this cannot see, the person goes on.
        h.told.step.send(Ok(Step::Licence { signed_in: true })).unwrap();
        h.until("the person's step", |h| h.at("Anyway").is_some());
        h.press("Anyway");
        h.until("the project opening", |h| h.state.opening.now.is_some());
    }

    #[test]
    fn what_failed_says_so_and_offers_the_next_way() {
        let mut h = Harness::new();
        h.state.install_unity(&h.ctx.clone(), wanted(), false);
        h.told.step.send(Err(Err(End::Room { at: "/Applications".into(), need: 16_000_000_000, free: Some(9_000_000_000) }))).unwrap();
        h.until("no room", |h| h.state.opening.ended_for().is_some() && h.at("Install").is_some());
        assert!(h.at("InHub").is_none());
        h.press("Install");
        h.until("again", |h| h.asked().len() == 2);
        h.told.step.send(Err(Err(End::Failed("no".into())))).unwrap();
        h.until("the failure", |h| h.at("InHub").is_some());
        h.press("InHub");
        h.until("Unity Hub's own", |h| h.asked().last().is_some_and(|(_, in_hub)| *in_hub));
    }

    /// Off a Mac nothing is installed from here: the same card, with Unity Hub on that editor (as before), or where to get it.
    #[test]
    fn elsewhere_the_card_has_unity_hub() {
        let mut h = Harness::new();
        h.state.opening.offers = Some(false);
        h.state.opening.note = Some(missing());
        h.frames(2);
        assert!(h.at("Install").is_none());
        assert!(h.at("HubLink").is_some() || h.at("GetHub").is_some());
        assert_eq!(wanted().hub_link().as_deref(), Some("unityhub://2022.3.22f1/887be4894c44"));
        // Any other failure is said there too, and closed.
        h.state.opening.note = Some(Note::said(&wanted().project, "Avatar is not at /Projects/Avatar any more."));
        h.frames(2);
        assert!(h.state.anchors.contains_key("opening-t"));
        assert!(!h.state.says_of_opening(Path::new("/Projects/Other")));
    }

    /// The whole page, as a person has it: what Open in Unity came to is in sight beside its button, at the panel's top and once scrolled.
    #[test]
    fn it_is_said_in_sight_beside_the_button() {
        for (name, anchor, scroll) in [("projects-opening-missing", "opening-panel", 0.0), ("projects-opening-missing", "opening-bar", 900.0), ("projects-opening-installing", "opening-panel", 0.0)] {
            let mut h = Harness::new();
            h.page = true;
            let ctx = h.ctx.clone();
            h.state.stage(&ctx, name);
            h.frames(40);
            if scroll > 0.0 {
                let over = Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -scroll), phase: egui::TouchPhase::Move, modifiers: Default::default(), source: egui::MouseWheelSource::Wheel };
                h.run(vec![Event::PointerMoved(pos2(1050.0, 500.0)), over]);
                h.frames(60);
            }
            let card = h.state.anchors.get(anchor).copied().unwrap_or_else(|| panic!("{name}: no {anchor}"));
            let view = Rect::from_min_max(pos2(230.0, 60.0), pos2(1260.0, 800.0));
            assert!(view.contains_rect(card), "{name} {anchor}: {card:?}");
        }
    }
}
