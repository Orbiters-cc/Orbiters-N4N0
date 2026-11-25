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
    /// Whether the job's record was already shown (a look asked for as soon as it exists, verification's major 6).
    pub shown: bool,
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

/// What looking at a place found: the budget per disk, what is in the way of a trial, and what of the original is outside it
/// (its links, its local packages), a sentence each.
#[derive(Clone)]
pub(crate) struct CheckedPlace {
    pub plan: unity6::budget::Budget,
    pub blockers: Vec<trial::Blocker>,
    pub outside: Vec<String>,
}
pub(crate) type Checked = Result<CheckedPlace, String>;

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
    look_job: Job<(u64, u64, unity6::Look)>,
    /// Which list of projects this is: one more each time the scan gives it anew, the same paths or not (second
    /// verification's M6: a project changed in place is looked at again).
    serial: u64,
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
    /// The release list the rows were made by: another one makes them again.
    compat_by: Option<matrix::Data>,
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
    /// A verified trial being compared again and listed: which, its Stop and its progress.
    pub finishing: Option<(String, Arc<AtomicBool>, SharedProgress)>,
    pub error: Option<String>,
    /// The compatibility view's packages shown.
    pub show_packages: bool,
    /// The scan's serial the gallery list was last taken at.
    pub scan_seen: Option<u64>,
    /// Screenshots: the panel scrolls to the Unity 6 card on its next frame.
    pub scroll_to: bool,
    pub fixture: Option<Fixture>,
}

impl U6 {
    pub fn standing(&self, path: &Path) -> Option<&standing::Standing> {
        self.look.as_ref()?.standing(path)
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

    /// Whether the project at `path` is a trial that nothing opens or changes from here now, as the last look saw it. Shown
    /// only: every action asks the library's `authority` again on its worker, and what it cannot tell is refused there.
    pub fn waiting(&self, path: &Path) -> bool {
        let key = orbiters_projects::discovery::key(path);
        self.trial_at(path).is_some_and(|(j, s)| {
            let first_after_adoption = j.migrated.is_some() && orbiters_projects::discovery::key(&j.trial.path) == key;
            first_after_adoption || !matches!(s, trial::Status::OnUnity6)
        })
    }

    /// Whether the trial folder at `path` was there at the last look (the page never asks the disk while drawing).
    pub fn present(&self, path: &Path) -> bool {
        self.look.as_ref().is_some_and(|l| l.present.contains(&orbiters_projects::discovery::key(path)))
    }

    /// The trials record's health at the last look.
    pub fn health(&self) -> trial::Health {
        self.look.as_ref().map(|l| l.health.clone()).unwrap_or(trial::Health::Fine)
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
    pub fn apply(&mut self, generation: u64, serial: u64, look: unity6::Look) -> bool {
        if generation <= self.applied {
            return false;
        }
        // An answer for another list of projects than the one the gallery shows now is not taken (verification's major 6).
        if self.fixture.is_none() && (serial != self.serial || look.projects != self.looked) {
            self.again = true;
            return false;
        }
        self.applied = generation;
        self.look = Some(look);
        true
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
        ctx.request_repaint();
        self.asked += 1;
        let (generation, serial) = (self.asked, self.serial);
        self.looked = projects.clone();
        self.again = false;
        let dir = orbiters_projects::data_dir();
        self.look_job = Job::spawn(ctx, move || (generation, serial, unity6::look(&dir, &projects)));
    }

    /// Takes in what workers answered; asks again what changed. `projects`: the gallery's list, `None` when it did
    /// not change since the last frame (nothing is rebuilt then).
    pub fn poll(&mut self, ctx: &egui::Context, projects: Option<Vec<PathBuf>>, server: orbiters_core::account::Server) {
        if let Some(projects) = projects {
            self.looked = projects;
            self.serial += 1;
            self.again = true;
        }
        if let Some((generation, serial, look)) = self.look_job.take() {
            self.apply(generation, serial, look);
        }
        if self.fixture.is_some() {
            return;
        }
        // A trial being made shows in the gallery as soon as it is recorded.
        if let Some(run) = self.run.as_mut() {
            if !run.shown && run.recorded.lock().unwrap().is_some() {
                run.shown = true;
                self.again = true;
            }
        }
        // The first one waits for nothing, not even a project: the trials recorded show at once (second verification's N9).
        if (self.again || self.asked == 0) && !self.look_job.running() {
            let list = self.looked.clone();
            self.ask(ctx, list);
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
        let by = self.look.as_ref().map(|l| &l.matrix.data);
        let same = self.compat.as_ref().is_some_and(|(p, _, _)| p == path) && self.compat_by.as_ref() == by;
        if self.fixture.is_some() || same || self.compat_job.running() {
            return;
        }
        self.compat_by = by.cloned();
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
            // What a log does not tell is read again: Unity may write it later than its process is first looked for.
            job.attempts.iter().filter(|a| self.outcomes.get(&(job.id.clone(), a.n)).is_none_or(|o| !o.ended() || matches!(o, outcome::Outcome::Undetermined { .. }))).cloned().collect();
        if pending.is_empty() {
            return;
        }
        self.outcomes_at = Some(Instant::now());
        let (id, at) = (job.id.clone(), job.unity6().path.clone());
        self.outcome_job = Job::spawn(ctx, move || {
            // Each attempt's own process; Unity on the trial only stands in for the last one where the system gave none.
            let open = trial::is_open(&at);
            let last = pending.iter().map(|a| a.n).max();
            pending.iter().map(|a| ((id.clone(), a.n), trial::attempt_outcome(a, trial::attempt_running(a, open && Some(a.n) == last)))).collect()
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

/// The room a trial of `source` at `place` needs on each disk, its editors found and probed now; and the high end of its
/// first import.
pub(crate) fn plan_for(source: &Path, place: &Path, copy: u64, library: Option<u64>, target: &matrix::Row) -> (unity6::budget::Budget, u64) {
    use unity6::budget;
    let now = standing::EditorStamp::read(source).text;
    let editor = target.size_here().and_then(|size| budget::EditorNeed::here(size, &editors::find_build(&target.unity, Some(&target.changeset)), &editors::find(&now)));
    let (demands, guessed) = budget::demands(place, copy, library, editor);
    let high = demands.iter().find(|d| d.need == budget::Need::Import).map_or(0, |d| d.bytes);
    (budget::plan(demands, &budget::Disks, guessed), high)
}

impl State {
    /// What the projects listed need of the transition, each frame: answers taken in, new looks asked.
    pub(super) fn poll_unity6(&mut self, ctx: &egui::Context, cx: &mut crate::pages::Cx) {
        // The list is rebuilt only when the scan changed it (verification's minor 3: not every frame).
        let projects = (self.u6.scan_seen != Some(self.scan_serial)).then(|| {
            self.u6.scan_seen = Some(self.scan_serial);
            self.scan.projects.iter().filter(|p| !p.missing).map(|p| p.path.clone()).collect()
        });
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
        let working = super::super::Working::begin();
        let wake = ctx.clone();
        let job = Job::spawn(ctx, move || {
            let _working = working;
            let dir = orbiters_projects::data_dir();
            let library = orbiters_projects::size::lookup(&from).size.map(|s| s.library);
            let budget = |place: &Path, copy: u64| -> anyhow::Result<u64> {
                let (plan, high) = plan_for(&from, place, copy, library, &target);
                anyhow::ensure!(plan.fits(), "{}", tr("Not enough room on a disk for the trial, checked again just now: choose another place or free some space."));
                Ok(high)
            };
            let recorded = |j: &trial::Job| {
                *rec.lock().unwrap() = Some(j.id.clone());
                wake.request_repaint();
            };
            let open = |p: &Path| trial::is_open(p);
            let t = trial::Target { sdk: target.sdk.clone(), unity: target.unity.clone(), changeset: target.changeset.clone() };
            trial::prepare(trial::Prepare { dir: &dir, source: &from, parent: &parent, name: &name, target: t, open: &open, budget: &budget, recorded: &recorded }, &stop, &prog)
                .map_err(|e| format!("{e:#}"))
        });
        self.u6.run = Some(Run { shown: false, recorded, job, cancel, progress });
        self.u6.sheet.step = Step::Copying;
    }
}

/// The phase of a copy as the person reads it.
pub(crate) fn phase_label(phase: &str) -> &'static str {
    match phase {
        p if p == mark("Registering project") => tr("Registering project"),
        p if p == mark("Comparing every file with the original") => tr("Comparing every file with the original"),
        _ => tr("Copying project"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(states: &[(&str, standing::State)]) -> unity6::Look {
        let standings: Vec<(PathBuf, standing::Standing)> = states.iter().map(|(p, s)| (PathBuf::from(p), standing::Standing { state: *s, ..Default::default() })).collect();
        let index = standings.iter().enumerate().map(|(i, (p, _))| (orbiters_projects::discovery::key(p), i)).collect();
        unity6::Look {
            matrix: matrix::Matrix::built_in(),
            projects: vec![],
            standings,
            index,
            trials: vec![],
            pairs: vec![],
            health: trial::Health::Fine,
            copies: vec![],
            present: Default::default(),
        }
    }

    /// Verification major 6: an answer for another list of projects than the gallery's now is not taken, and asks again.
    #[test]
    fn a_look_for_an_older_list_of_projects_is_not_taken() {
        let mut state = State::default();
        state.u6.looked = vec![PathBuf::from("H:/Unity projects/A"), PathBuf::from("H:/Unity projects/B")];
        let mut stale = look(&[("H:/Unity projects/A", standing::State::On2022)]);
        stale.projects = vec![PathBuf::from("H:/Unity projects/A")];
        assert!(!state.u6.apply(5, 0, stale));
        assert!(state.u6.look.is_none() && state.u6.again, "dropped, and a new look is due");
        let mut fresh = look(&[("H:/Unity projects/A", standing::State::On2022)]);
        fresh.projects = state.u6.looked.clone();
        assert!(state.u6.apply(6, 0, fresh));
    }

    /// M10: a late answer (asked before the one shown) is dropped; answers never touch the selection.
    #[test]
    fn a_late_or_superseded_look_is_dropped_and_the_selection_stays() {
        let mut state = State::default();
        state.selected = Some(PathBuf::from("H:/Unity projects/A"));
        assert!(state.u6.apply(2, 0, look(&[("H:/Unity projects/A", standing::State::OnUnity6)])));
        assert!(!state.u6.apply(1, 0, look(&[("H:/Unity projects/A", standing::State::On2022)])), "older than the one shown");
        assert!(!state.u6.apply(2, 0, look(&[("H:/Unity projects/A", standing::State::Disagree)])), "the same generation twice");
        assert_eq!(state.u6.standing(Path::new("H:/Unity projects/A")).map(|s| s.state), Some(standing::State::OnUnity6));
        assert!(state.u6.apply(3, 0, look(&[("h:\\unity projects\\a", standing::State::On2022)])));
        assert_eq!(state.u6.standing(Path::new("H:/Unity projects/A")).map(|s| s.state), Some(standing::State::On2022), "by project, whatever the path's spelling");
        assert_eq!(state.selected.as_deref(), Some(Path::new("H:/Unity projects/A")));
    }

    /// The revision's 4e: a trial waiting for Creator Companion is neither opened nor offered Open from here.
    #[test]
    fn a_trial_waiting_for_creator_companion_is_not_opened() {
        let mut state = State::default();
        let path = PathBuf::from("H:/Unity projects/My avatars (Unity 6)");
        let job = trial_job(&path, None);
        let mut l = look(&[]);
        l.trials = vec![(job, trial::Status::Waiting { vcc_copy: None })];
        state.u6.apply(1, 0, l);
        assert!(state.u6.waiting(&path));
        let project = orbiters_projects::discovery::Project { path: path.clone(), name: "My avatars (Unity 6)".into(), registered: true, ..Default::default() };
        let acts = super::super::menus::project_items(&state, &project, true);
        assert!(!acts.iter().any(|(_, a)| *a == super::super::menus::ProjectAct::Open), "no Open in its menu");
        let ctx = egui::Context::default();
        state.open_unity(&ctx, &project, false);
        assert!(!state.action.running() && state.opening.note.as_ref().is_some_and(|n| n.project == path && n.words.contains("Creator Companion")), "said beside its button");
        // Not finished, or the first copy once Creator Companion's own copy was taken: the same.
        let mut l = look(&[]);
        l.trials = vec![(trial_job(&path, None), trial::Status::Unfinished)];
        state.u6.apply(2, 0, l);
        assert!(state.u6.waiting(&path));
        let mut l = look(&[]);
        l.trials = vec![(trial_job(&path, Some(PathBuf::from("H:/Unity projects/My avatars (Unity 6)-Migrated"))), trial::Status::OnUnity6)];
        state.u6.apply(3, 0, l);
        assert!(state.u6.waiting(&path), "the first copy keeps its guard");
        assert!(!state.u6.waiting(Path::new("H:/Unity projects/My avatars (Unity 6)-Migrated")), "the migrated copy is a Unity 6 trial");
    }

    fn trial_job(path: &Path, migrated: Option<PathBuf>) -> trial::Job {
        trial::Job {
            id: "1".repeat(32),
            created: 0,
            source: trial::Place { path: PathBuf::from("H:/Unity projects/My avatars"), id: None, name: "My avatars".into() },
            trial: trial::Place { path: path.to_path_buf(), id: None, name: "My avatars (Unity 6)".into() },
            phase: trial::Phase::Ready,
            target: trial::Target { sdk: "3.11.0-beta.1".into(), unity: "6000.0.67f1".into(), changeset: "78a1c2bbeb6a".into() },
            error: None,
            differences: vec![],
            attempts: vec![],
            migrated: migrated.map(|p| trial::Place { name: "x".into(), path: p, id: None }),
            beside: None,
            localized: vec![],
            linked: vec![],
            content: vec![],
            content_unsure: vec![],
            guid: String::new(),
            manifest: None,
            import_high: 0,
        }
    }
    /// Verification minor 3: what the page draws comes from the look's snapshot, never from the disk while drawing.
    #[test]
    fn the_page_draws_from_the_snapshot() {
        let mut state = State::default();
        let mut l = look(&[("Q:/Not/Here", standing::State::On2022)]);
        l.present.insert(orbiters_projects::discovery::key(Path::new("Q:/Not/Here/Trial")));
        state.u6.apply(1, 0, l);
        assert!(state.u6.present(Path::new("Q:/Not/Here/Trial")), "said present as the worker saw it, though no such disk exists here");
        assert!(state.u6.standing(Path::new("q:\\not\\here")).is_some(), "found by its key");
    }

    /// Verification major 6 and minor 3: the gallery drawn for real, scrolled to its found projects. A trial's row that
    /// arrives (a copy that starts, one that failed) moves no card and no row in view, and neither does a damaged record.
    /// Nothing drawn is on this computer.
    #[test]
    fn a_trials_row_that_arrives_moves_nothing_in_view() {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let ctl: crate::backend::Ctl = Arc::new(crate::backend::Local::new(crate::controller::Controller::new(orbiters_core::settings::Settings::default())));
        let snap = (*ctl.view()).clone();
        let mut state = State::default();
        state.fixture = true;
        state.u6.fixture = Some(Fixture::default());
        state.scan.projects = (0..24u64)
            .map(|i| orbiters_projects::discovery::Project {
                path: PathBuf::from(format!("Q:/Not/Here/P{i}")),
                name: format!("Project {i}"),
                unity: "2022.3.22f1".into(),
                registered: i < 6,
                modified: 1000 - i,
                ..Default::default()
            })
            .collect();
        state.u6.apply(1, 0, look(&[("Q:/Not/Here/P0", standing::State::On2022)]));
        let mut time = 0.0;
        let mut draw = |state: &mut State, frames: usize| {
            for _ in 0..frames {
                time += 1.0 / 60.0;
                let input = egui::RawInput { time: Some(time), screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0))), ..Default::default() };
                let mut toasts = orbiters_ui::toast::Toasts::default();
                let mut output = ctx.run_ui(input, |ui| {
                    let mut cx = crate::pages::Cx { ctl: &ctl, snap: &snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
                    super::super::ui::show(ui, &mut cx, state);
                });
                output.textures_delta.clear();
            }
        };
        draw(&mut state, 60);
        state.scroll_fix = Some(500.0);
        draw(&mut state, 60);
        let before = state.row_places.clone();
        let shown = before.iter().filter(|(_, r)| r.bottom() > 120.0 && r.top() < 700.0).count();
        assert!(shown > 0 && before[0].1.top() < 0.0, "scrolled into the gallery: {before:?}");
        let arrivals = [
            (trial::Status::Running, trial::Health::Fine),
            (trial::Status::Failed { kept: true }, trial::Health::Fine),
            (trial::Status::Unfinished, trial::Health::Damaged { lines: 1, why: "x".into(), newer: false }),
        ];
        for (i, (status, health)) in arrivals.into_iter().enumerate() {
            let mut l = look(&[("Q:/Not/Here/P0", standing::State::On2022)]);
            let missing = PathBuf::from("Q:/Not/Here/Trial");
            l.trials = vec![(trial_job(&missing, None), status)];
            l.health = health;
            assert!(state.u6.apply(2 + i as u64, 0, l));
            assert_eq!(state.u6.jobs().len(), 1, "a row of its own");
            draw(&mut state, 60);
            assert_eq!(state.row_places, before, "arrival {i}");
        }
    }

    /// Verification minor 3: what paints the card, the job rows and the sheet asks nothing of the disk (a network share
    /// that does not answer would stall the whole window). Reads happen in workers; the one look at a folder here is a
    /// click's.
    #[test]
    fn nothing_painted_reads_the_disk() {
        for (name, source) in [("unity6_card.rs", include_str!("unity6_card.rs")), ("unity6_sheet.rs", include_str!("unity6_sheet.rs"))] {
            for (n, line) in source.lines().enumerate() {
                let asks = [".exists()", "std::fs::", "canonicalize(", ".verified()", "source_verified()", "trial::guard(", "trial::record(", "trial::jobs("].iter().any(|w| line.contains(w));
                let worker = line.contains("destination.exists()"); // in the sheet's check, a worker's
                assert!(!asks || worker, "{name}:{}: {line}", n + 1);
            }
        }
    }

    /// Second verification N9: the first look is asked at once, with no project listed and no answer from anywhere.
    #[test]
    fn the_first_look_waits_for_nothing() {
        let ctx = egui::Context::default();
        let mut u6 = U6::default();
        u6.poll(&ctx, Some(vec![]), orbiters_core::account::Server::Production);
        assert!(u6.look_job.running() || u6.look.is_some(), "asked with an empty list");
        let mut u6 = U6::default();
        u6.poll(&ctx, None, orbiters_core::account::Server::Production);
        assert!(u6.look_job.running() || u6.look.is_some(), "asked before any scan said anything");
    }

    /// Second verification M6: a scan that gives the same paths again asks again, and an answer asked before it is not taken.
    #[test]
    fn the_same_list_scanned_again_is_looked_at_again() {
        let ctx = egui::Context::default();
        let mut u6 = U6::default();
        let list = vec![PathBuf::from("Q:/Not/Here/A")];
        u6.poll(&ctx, Some(list.clone()), orbiters_core::account::Server::Production);
        let first = u6.asked;
        let (generation, serial) = (u6.asked + 10, u6.serial);
        u6.poll(&ctx, Some(list.clone()), orbiters_core::account::Server::Production);
        let mut early = look(&[("Q:/Not/Here/A", standing::State::On2022)]);
        early.projects = list.clone();
        assert!(!u6.apply(generation, serial, early), "asked before the list was scanned again");
        let until = Instant::now() + Duration::from_secs(20);
        while u6.asked == first {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
            u6.poll(&ctx, None, orbiters_core::account::Server::Production);
        }
    }

    /// Second verification N3: without the trials looked at, nothing is offered for an upload.
    #[test]
    fn without_the_trials_looked_at_no_project_is_offered_for_an_upload() {
        use super::super::toolkit_update::ToolkitOffer;
        let mut state = State::default();
        state.started = true;
        state.scan.projects = vec![orbiters_projects::discovery::Project {
            path: PathBuf::from("Q:/Not/Here/Six"),
            name: "Avatar (Unity 6)".into(),
            registered: true,
            unity: "6000.0.67f1".into(),
            avatars: vec![orbiters_projects::avatars::Avatar { id: "avtr_a".into(), ..Default::default() }],
            ..Default::default()
        }];
        assert_eq!(state.toolkit_offer("avtr_a"), ToolkitOffer::Looking);
        state.u6.apply(1, 0, look(&[]));
        assert!(matches!(state.toolkit_offer("avtr_a"), ToolkitOffer::Project(_)), "its own worker asks again before anything");
    }
}
