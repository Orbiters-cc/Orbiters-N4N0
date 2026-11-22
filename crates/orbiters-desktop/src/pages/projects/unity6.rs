//! The Unity projects page's part of VRChat's move to Unity 6 (`orbiters_projects::unity6`): what is known of each
//! project and trial, read on workers beside the scan and applied by project and by generation (a late answer never
//! replaces a newer one, never moves the selection or the gallery); the editors, Creator Companions, compatibility view
//! and attempts' logs of the project shown; the release list asked of Orbiters now and then, apart from all of it.
//! What it shows: `unity6_card` (the panel's card), `unity6_sheet` (Try on Unity 6), the gallery's pills and rows.

use orbiters_i18n::{mark, tr};
use super::state::State;
use crate::pages::Job;
use orbiters_projects::templates::SharedProgress;
use orbiters_projects::unity6::{self, compat, editors, matrix, outcome, standing, trial, vcc};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the logs of a trial open in Unity 6 are read again while it shows.
const OUTCOME_EVERY: Duration = Duration::from_secs(3);

/// A trial being made by this window.
pub(crate) struct Run {
    pub recorded: Arc<Mutex<Option<String>>>,
    pub job: Job<Result<trial::Job, String>>,
    pub cancel: Arc<AtomicBool>,
    pub progress: SharedProgress,
}

/// The steps of Try on Unity 6, one decision each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Step {
    /// What cannot be undone, before anything exists: the content IDs the trial shares.
    #[default]
    Ids,
    /// The project builds in Unity 2022 today.
    Baseline,
    /// Where the trial goes, and the room it needs on each disk.
    Place,
    /// The copy running.
    Copying,
    /// Made (or stopped): what comes next, in Creator Companion.
    Done,
}

/// What the room check found for a place: the budget per disk, and what is in the way of a trial.
pub(crate) type Checked = Result<(unity6::budget::Budget, Vec<trial::Blocker>), String>;

#[derive(Default)]
pub(crate) struct Sheet {
    pub step: Step,
    pub name: String,
    pub location: String,
    /// The (location, name) checked, and what was found.
    pub checked: Option<((String, String), Checked)>,
    pub check: Job<((String, String), Checked)>,
    pub pick: Job<Option<PathBuf>>,
    pub done: Option<Result<trial::Job, String>>,
}

/// Screenshots: what the page shows without reading any project or asking anyone.
#[derive(Clone, Default)]
pub(crate) struct Fixture {
    pub mac: bool,
}

#[derive(Default)]
pub(crate) struct U6 {
    pub look: Option<unity6::Look>,
    look_job: Job<(u64, unity6::Look)>,
    asked: u64,
    applied: u64,
    /// The projects last asked about; another list asks again (once the running look ends).
    looked: Vec<PathBuf>,
    again: bool,
    refresh_job: Job<Result<matrix::Refresh, String>>,
    refreshed: Option<Instant>,
    pub refresh_error: Option<String>,
    pub editors: HashMap<String, editors::Found>,
    editor_job: Job<Vec<(String, editors::Found)>>,
    editors_asked: HashSet<String>,
    pub compat: Option<(PathBuf, Vec<compat::Row>, compat::Checklist)>,
    compat_job: Job<(PathBuf, Vec<compat::Row>, compat::Checklist)>,
    compat_cancel: Arc<AtomicBool>,
    pub vccs: Option<Vec<vcc::Vcc>>,
    vcc_job: Job<Vec<vcc::Vcc>>,
    pub outcomes: HashMap<(String, u32), outcome::Outcome>,
    outcome_job: Job<Vec<((String, u32), outcome::Outcome)>>,
    outcomes_at: Option<Instant>,
    pub sheet: Sheet,
    pub run: Option<Run>,
    /// A trial's smaller actions (list again, forget, take VCC's copy, unregister, locate, open in Unity 6), one at a time.
    pub action: Job<Result<String, String>>,
    pub action_label: String,
    pub error: Option<String>,
    /// The compatibility view's packages shown.
    pub show_packages: bool,
    /// Screenshots: the panel scrolls to the Unity 6 card on its next frame.
    pub scroll_to: bool,
    pub fixture: Option<Fixture>,
}

impl U6 {
    pub fn standing(&self, path: &Path) -> Option<&standing::Standing> {
        let key = orbiters_projects::discovery::key(path);
        self.look.as_ref()?.standings.iter().find(|(p, _)| orbiters_projects::discovery::key(p) == key).map(|(_, s)| s)
    }

    pub fn matrix(&self) -> matrix::Matrix {
        self.look.as_ref().map(|l| l.matrix.clone()).unwrap_or_else(matrix::Matrix::built_in)
    }

    /// The trial whose Unity 6 folder is `path`, and where it stands.
    pub fn trial_at(&self, path: &Path) -> Option<&(trial::Job, trial::Status)> {
        let key = orbiters_projects::discovery::key(path);
        self.look.as_ref()?.trials.iter().find(|(j, s)| {
            !matches!(s, trial::Status::Replaced) && (orbiters_projects::discovery::key(&j.unity6().path) == key || orbiters_projects::discovery::key(&j.trial.path) == key)
        })
    }

    /// Whether the project at `path` is a trial still waiting for Creator Companion's migration: nothing opens it or
    /// changes its packages from here then (the revision of DESIGN22, 4e).
    pub fn waiting(&self, path: &Path) -> bool {
        self.trial_at(path).is_some_and(|(_, s)| matches!(s, trial::Status::Waiting { .. }))
    }

    /// The trials of the original at `path` (both folders still the ones recorded).
    pub fn trials_of(&self, path: &Path) -> Vec<&unity6::Pair> {
        let key = orbiters_projects::discovery::key(path);
        self.look.as_ref().map(|l| l.pairs.iter().filter(|p| orbiters_projects::discovery::key(&p.original) == key).collect()).unwrap_or_default()
    }

    /// Trials the gallery shows as jobs (not projects yet, or not where they were).
    pub fn jobs(&self) -> Vec<&(trial::Job, trial::Status)> {
        self.look.as_ref().map(|l| l.trials.iter().filter(|(_, s)| s.is_job()).collect()).unwrap_or_default()
    }

    /// The macOS variant: no Creator Companion window to hand over to.
    pub fn mac(&self) -> bool {
        cfg!(target_os = "macos") || self.fixture.as_ref().is_some_and(|f| f.mac)
    }

    /// Takes a look's answer when it is newer than the one shown (generations count up); an older one is dropped.
    /// Nothing else changes: the selection and the gallery's order are not this look's.
    pub fn apply(&mut self, generation: u64, look: unity6::Look) -> bool {
        if generation <= self.applied {
            return false;
        }
        self.applied = generation;
        self.look = Some(look);
        true
    }

    fn running_ids(&self) -> Vec<String> {
        self.run.iter().filter_map(|r| r.recorded.lock().unwrap().clone()).collect()
    }

    /// Asks again what is known of `projects` (on a worker); a look already running is let finish and followed by one.
    pub fn ask(&mut self, ctx: &egui::Context, projects: Vec<PathBuf>) {
        if self.fixture.is_some() {
            return;
        }
        if self.look_job.running() {
            self.again = true;
            self.looked = projects;
            return;
        }
        self.asked += 1;
        let generation = self.asked;
        let running = self.running_ids();
        self.looked = projects.clone();
        self.again = false;
        let dir = orbiters_projects::data_dir();
        self.look_job = Job::spawn(ctx, move || (generation, unity6::look(&dir, &projects, &running)));
    }

    /// Takes in what workers answered; asks again what changed.
    pub fn poll(&mut self, ctx: &egui::Context, projects: Vec<PathBuf>, server: orbiters_core::account::Server) {
        if let Some((generation, look)) = self.look_job.take() {
            self.apply(generation, look);
        }
        if self.fixture.is_some() {
            return;
        }
        if (self.again || projects != self.looked) && !self.look_job.running() {
            self.ask(ctx, projects);
        }
        if let Some(found) = self.editor_job.take() {
            self.editors.extend(found);
        }
        if let Some(c) = self.compat_job.take() {
            self.compat = Some(c);
        }
        if let Some(v) = self.vcc_job.take() {
            self.vccs = Some(v);
        }
        if let Some(found) = self.outcome_job.take() {
            self.outcomes.extend(found);
        }
        // The release list: asked of Orbiters at most every twelve hours, on its own worker; a scan never waits for it.
        if let Some(r) = self.refresh_job.take() {
            self.refresh_error = r.err();
            let list = self.looked.clone();
            self.again = true;
            self.ask(ctx, list);
        }
        if !self.refresh_job.running() && self.refreshed.is_none_or(|t| t.elapsed() > Duration::from_secs(matrix::REFRESH_SECS)) && !cfg!(test) {
            self.refreshed = Some(Instant::now());
            let dir = orbiters_projects::data_dir();
            self.refresh_job = Job::spawn(ctx, move || {
                let Some(key) = unity6::key() else { return Err(tr("This build has no release key: the built-in release list is used.").to_string()) };
                matrix::refresh(&dir, &key, &matrix::Api(server), &matrix::served_url(server), orbiters_projects::discovery::now()).map_err(|e| format!("{e:#}"))
            });
        }
    }

    /// The editors of these versions, looked for once per visit (on a worker: VCC's database can take seconds).
    pub fn want_editors(&mut self, ctx: &egui::Context, versions: &[String]) {
        if self.fixture.is_some() || self.editor_job.running() {
            return;
        }
        let new: Vec<String> = versions.iter().filter(|v| !v.is_empty() && self.editors_asked.insert((*v).clone())).cloned().collect();
        if !new.is_empty() {
            self.editor_job = Job::spawn(ctx, move || new.into_iter().map(|v| (v.clone(), editors::find(&v))).collect());
        }
    }

    /// Forgets the editors found (a new visit, an editor located or installed meanwhile).
    pub fn forget_editors(&mut self) {
        if self.fixture.is_none() {
            self.editors_asked.clear();
            self.editors.clear();
        }
    }

    /// The compatibility view of `path` (once per project shown; a new project stops the look at the last one).
    pub fn want_compat(&mut self, ctx: &egui::Context, path: &Path, content: standing::Content) {
        if self.fixture.is_some() || self.compat.as_ref().is_some_and(|(p, _, _)| p == path) || self.compat_job.running() {
            return;
        }
        self.compat_cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.compat_cancel = cancel.clone();
        let path = path.to_path_buf();
        let target = self.matrix().unity6_target().map(|r| r.sdk.clone()).unwrap_or_default();
        let m = self.matrix();
        self.compat_job = Job::spawn(ctx, move || {
            let rows = compat::packages(&path, &m, &target);
            let list = compat::checklist(&path, content, &cancel);
            (path, rows, list)
        });
    }

    pub fn want_vccs(&mut self, ctx: &egui::Context) {
        if self.fixture.is_none() && self.vccs.is_none() && !self.vcc_job.running() {
            let dir = orbiters_projects::data_dir();
            self.vcc_job = Job::spawn(ctx, move || vcc::found(&dir));
        }
    }

    /// Reads the attempts' logs of `job` again (every few seconds while one may still change).
    pub fn want_outcomes(&mut self, ctx: &egui::Context, job: &trial::Job) {
        if self.fixture.is_some() || self.outcome_job.running() || self.outcomes_at.is_some_and(|t| t.elapsed() < OUTCOME_EVERY) {
            return;
        }
        let pending: Vec<trial::Attempt> =
            job.attempts.iter().filter(|a| self.outcomes.get(&(job.id.clone(), a.n)).is_none_or(|o| !o.ended())).cloned().collect();
        if pending.is_empty() {
            return;
        }
        self.outcomes_at = Some(Instant::now());
        let (id, at) = (job.id.clone(), job.unity6().path.clone());
        self.outcome_job = Job::spawn(ctx, move || {
            let running = trial::is_open(&at);
            let last = pending.iter().map(|a| a.n).max();
            pending.iter().map(|a| ((id.clone(), a.n), trial::attempt_outcome(a, running && Some(a.n) == last))).collect()
        });
        crate::pages::frame_in(ctx, OUTCOME_EVERY.as_secs_f64());
    }

    /// Runs one of a trial's actions on a worker; the look is asked again when it ends.
    pub fn act(&mut self, ctx: &egui::Context, label: &str, f: impl FnOnce() -> anyhow::Result<String> + Send + 'static) {
        if self.action.running() || self.fixture.is_some() {
            return;
        }
        self.error = None;
        self.action_label = label.to_string();
        self.action = Job::spawn(ctx, move || f().map_err(|e| format!("{e:#}")));
    }

    pub fn busy(&self) -> Option<&str> {
        self.action.running().then_some(self.action_label.as_str())
    }
}

impl State {
    /// What the projects listed need of the transition, each frame: answers taken in, new looks asked.
    pub(super) fn poll_unity6(&mut self, ctx: &egui::Context, cx: &mut crate::pages::Cx) {
        let projects: Vec<PathBuf> = self.scan.projects.iter().filter(|p| !p.missing).map(|p| p.path.clone()).collect();
        let server = super::state::server(cx);
        self.u6.poll(ctx, projects, server);
        if let Some(r) = self.u6.action.take() {
            match r {
                Ok(said) => {
                    if !said.is_empty() {
                        cx.toasts.push(ctx, orbiters_ui::toast::Kind::Success, said);
                    }
                }
                Err(e) => self.u6.error = Some(e),
            }
            self.u6.forget_editors();
            let list = self.u6.looked.clone();
            self.u6.again = true;
            self.u6.ask(ctx, list);
            self.refresh(ctx, false);
        }
        let done = self.u6.run.as_mut().and_then(|r| r.job.take());
        if let Some(result) = done {
            let run = self.u6.run.take();
            self.u6.sheet.done = Some(result.clone());
            self.u6.sheet.step = Step::Done;
            if let Ok(job) = &result {
                self.reveal = Some(job.trial.path.clone());
            }
            if let (Err(e), Some(run)) = (&result, run) {
                // Stopped before anything was recorded: back to the place, said why.
                if run.recorded.lock().unwrap().is_none() {
                    self.u6.sheet.step = Step::Place;
                    self.u6.sheet.checked = None;
                    self.u6.error = Some(e.clone());
                }
            }
            let list = self.u6.looked.clone();
            self.u6.again = true;
            self.u6.ask(ctx, list);
            self.refresh(ctx, false);
        }
    }

    /// Starts making a trial of `source` at `parent`/`name` (Try on Unity 6's last step).
    pub(super) fn start_trial(&mut self, ctx: &egui::Context, source: PathBuf, parent: PathBuf, name: String) {
        if self.u6.run.is_some() || self.fixture {
            return;
        }
        let Some(target) = self.u6.matrix().unity6_target().cloned() else { return };
        let recorded = Arc::new(Mutex::new(None));
        let cancel = Arc::new(AtomicBool::new(false));
        let progress: SharedProgress = Arc::new(Mutex::new(Default::default()));
        let (rec, stop, prog, from) = (recorded.clone(), cancel.clone(), progress.clone(), source.clone());
        let editor_missing = self.u6.editors.get(&target.unity).is_none_or(|f| f.usable().is_none());
        let size = target.size_here().map(|s| s.installed + s.download).filter(|_| editor_missing);
        let working = super::super::Working::begin();
        let job = Job::spawn(ctx, move || {
            let _working = working;
            let dir = orbiters_projects::data_dir();
            let vcc = orbiters_projects::vcc_dir();
            let library = orbiters_projects::size::lookup(&from).size.map(|s| s.library);
            let budget = |copy: u64| -> anyhow::Result<()> {
                let editor = size.map(|b| (unity6::budget::editor_install_folder(), b));
                let (demands, guessed) = unity6::budget::demands(&parent.join(&name), copy, library, editor);
                let plan = unity6::budget::plan(demands, &unity6::budget::Disks, guessed);
                anyhow::ensure!(plan.fits(), "{}", tr("Not enough room on a disk for the trial, checked again just now: choose another place or free some space."));
                Ok(())
            };
            let recorded = |j: &trial::Job| *rec.lock().unwrap() = Some(j.id.clone());
            let open = |p: &Path| trial::is_open(p);
            let t = trial::Target { sdk: target.sdk.clone(), unity: target.unity.clone(), changeset: target.changeset.clone() };
            trial::prepare(trial::Prepare { dir: &dir, vcc: &vcc, source: &from, parent: &parent, name: &name, target: t, open: &open, budget: &budget, recorded: &recorded }, &stop, &prog)
                .map_err(|e| format!("{e:#}"))
        });
        self.u6.run = Some(Run { recorded, job, cancel, progress });
        self.u6.sheet.step = Step::Copying;
    }
}

/// The phase of a copy as the person reads it.
pub(crate) fn phase_label(phase: &str) -> &'static str {
    match phase {
        p if p == mark("Registering project") => tr("Registering project"),
        _ => tr("Copying project"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(states: &[(&str, standing::State)]) -> unity6::Look {
        unity6::Look {
            matrix: matrix::Matrix::built_in(),
            standings: states.iter().map(|(p, s)| (PathBuf::from(p), standing::Standing { state: *s, ..Default::default() })).collect(),
            trials: vec![],
            pairs: vec![],
        }
    }

    /// M10: a late answer (asked before the one shown) is dropped; answers never touch the selection.
    #[test]
    fn a_late_or_superseded_look_is_dropped_and_the_selection_stays() {
        let mut state = State::default();
        state.selected = Some(PathBuf::from("H:/Unity projects/A"));
        assert!(state.u6.apply(2, look(&[("H:/Unity projects/A", standing::State::OnUnity6)])));
        assert!(!state.u6.apply(1, look(&[("H:/Unity projects/A", standing::State::On2022)])), "older than the one shown");
        assert!(!state.u6.apply(2, look(&[("H:/Unity projects/A", standing::State::Disagree)])), "the same generation twice");
        assert_eq!(state.u6.standing(Path::new("H:/Unity projects/A")).map(|s| s.state), Some(standing::State::OnUnity6));
        assert!(state.u6.apply(3, look(&[("h:\\unity projects\\a", standing::State::On2022)])));
        assert_eq!(state.u6.standing(Path::new("H:/Unity projects/A")).map(|s| s.state), Some(standing::State::On2022), "by project, whatever the path's spelling");
        assert_eq!(state.selected.as_deref(), Some(Path::new("H:/Unity projects/A")));
    }

    /// The revision's 4e: a trial waiting for Creator Companion is neither opened nor offered Open from here.
    #[test]
    fn a_trial_waiting_for_creator_companion_is_not_opened() {
        let mut state = State::default();
        let path = PathBuf::from("H:/Unity projects/My avatars (Unity 6)");
        let job = trial::Job {
            id: "t".into(),
            created: 0,
            source: trial::Place { path: PathBuf::from("H:/Unity projects/My avatars"), id: None, name: "My avatars".into() },
            trial: trial::Place { path: path.clone(), id: None, name: "My avatars (Unity 6)".into() },
            phase: trial::Phase::Prepared,
            target: trial::Target { sdk: "3.11.0-beta.1".into(), unity: "6000.0.67f1".into(), changeset: "78a1c2bbeb6a".into() },
            error: None,
            attempts: vec![],
            migrated: None,
        };
        let mut l = look(&[]);
        l.trials = vec![(job, trial::Status::Waiting { vcc_copy: None })];
        state.u6.apply(1, l);
        assert!(state.u6.waiting(&path));
        let project = orbiters_projects::discovery::Project { path: path.clone(), name: "My avatars (Unity 6)".into(), registered: true, ..Default::default() };
        let acts = super::super::menus::project_items(&state, &project, true);
        assert!(!acts.iter().any(|(_, a)| *a == super::super::menus::ProjectAct::Open), "no Open in its menu");
        let ctx = egui::Context::default();
        state.open_unity(&ctx, &project, false);
        assert!(!state.action.running() && state.error.as_deref().is_some_and(|e| e.contains("Creator Companion")));
    }
}
