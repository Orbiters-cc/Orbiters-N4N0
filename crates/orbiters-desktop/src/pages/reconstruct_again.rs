//! Reconstruct again, wherever a 3D capture of this computer shows: a session's detail, a capture's window and its card's
//! menu in Photos & captures, its row on a world's page in Visited worlds. One answer to "what can be asked of this
//! capture now" ([`offer`]: start, why not, how far it got, how to stop it), so that the four places never disagree, and
//! one question before a new result takes what the old one had ([`Ask`]).
//!
//! What reconstructing again replaces, and when: nothing while it runs. The worker trains into a folder of its own and
//! moves the new result into place only once it finished, so a run stopped or failed leaves the capture as it was. A
//! finished run replaces the splats, and with them:
//! - the splats hidden or deleted in its 3D view (Select splats): those marks name splats of the old result, so the
//!   worker deletes them with it. Before this question they were lost with a toast that came after the press.
//! - nothing of what is shared at once: the link stays and goes on showing the version uploaded, which from then on is
//!   out of date. Its window offers Update the shared capture ([`crate::controller::shares::reconstructed`]).
//!
//! A capture with neither starts at once, as the session's detail always did: nothing is lost that a person could miss.

use super::Cx;
use crate::controller::shares::{self, Entry, Local};
use crate::state::Snapshot;
use egui::{pos2, Id, Rect, Ui};
use orbiters_core::bundle::SessionSummary;
use orbiters_core::shares::Item;
use orbiters_i18n::{mark, tr};
use orbiters_ui::dialog::{self, Dialog, Part};
use orbiters_ui::theme::{self, Txt};
use orbiters_ui::widgets::{self, Button, Variant};
use orbiters_ui::icons;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// What a capture says about itself, whichever list it came from.
#[derive(Clone, Copy, Debug)]
pub struct Facts<'a> {
    pub dir: &'a Path,
    /// It has a 3D view: the new one replaces it ("Reconstruct again", not "Reconstruct").
    pub reconstructed: bool,
    /// Frames came without the capture strip: nothing to make a 3D view from.
    pub no_keyframes: bool,
    pub no_depth: bool,
    /// Its capture files are gone ([`orbiters_core::cleanup::trimmed`]).
    pub trimmed: bool,
}

impl<'a> Facts<'a> {
    /// A capture of this computer in Photos & captures. Never what is merely shared with you (no session here to train
    /// from) and never a picture.
    pub fn of_entry(e: &'a Entry) -> Option<Self> {
        match &e.local {
            Some(Local::Session(dir)) if e.mine() => {
                Some(Self { dir, reconstructed: e.ready, no_keyframes: e.no_keyframes, no_depth: e.no_depth, trimmed: e.trimmed })
            }
            _ => None,
        }
    }

    pub fn of_session(s: &'a SessionSummary) -> Self {
        Self {
            dir: &s.dir,
            reconstructed: s.reconstruction.is_some(),
            no_keyframes: s.has_no_keyframes(),
            no_depth: s.manifest.stats.depth_frames == 0,
            trimmed: s.trimmed,
        }
    }
}

/// What can be asked of a capture now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Offer {
    /// It can start. `again`: it replaces the 3D view it has.
    Start { again: bool },
    /// It cannot: why in a sentence (a button's hint), and in a few words (a row of a card's menu).
    Blocked { again: bool, reason: &'static str, short: &'static str },
    /// It runs: how far it got as a whole, and what it is doing.
    Running { fraction: f32, stage: &'static str },
    /// It paused with this much kept, and goes on by itself: why and when, as a sentence.
    Paused { fraction: f32, why: &'static str },
    /// It waits its turn, and for what.
    Waiting { hint: &'static str },
}

impl Offer {
    /// What its button says while it starts or says why not.
    pub fn label(self) -> &'static str {
        match self {
            Offer::Start { again: true } | Offer::Blocked { again: true, .. } => tr("Reconstruct again"),
            Offer::Start { again: false } | Offer::Blocked { again: false, .. } => tr("Reconstruct"),
            Offer::Running { .. } | Offer::Paused { .. } => tr("Stop reconstruction"),
            Offer::Waiting { .. } => tr("Waiting · cancel"),
        }
    }

    /// The new result is on its way: what is shared or exported now would be the one going away.
    pub fn running(self) -> bool {
        matches!(self, Offer::Running { .. } | Offer::Paused { .. })
    }
}

pub const VIEW_ONLY: &str = mark("3D view only: its capture files were deleted, so there is nothing left to reconstruct from");
pub const CLEANING: &str = mark("Its capture files are being deleted, so there is nothing left to reconstruct from");
pub const NO_DEPTH: &str = mark("This session has no depth frames: record with the capture kit's depth frames on");
pub const NO_KEYFRAMES: &str = mark("Recorded without the capture strip: nothing to make 3D from");
pub const RECORDING: &str = mark("It is still being recorded: stop the capture first");

/// [`Offer`] of the capture `f` as things stand in `snap`. A run under way or waiting comes first: it can be stopped
/// whatever else became of the capture.
pub fn offer(f: &Facts<'_>, snap: &Snapshot) -> Offer {
    if let Some(j) = snap.reconstruction.as_ref().filter(|j| j.session_dir == f.dir) {
        if j.paused {
            return Offer::Paused { fraction: super::sessions::overall_progress(j), why: super::sessions::pause_why(snap) };
        }
        return Offer::Running { fraction: super::sessions::overall_progress(j), stage: super::sessions::stage_label(j.stage) };
    }
    if snap.reconstruct_queue.iter().any(|d| d == f.dir) {
        let hint = if snap.is_capturing() {
            tr("Starts when the capture ends")
        } else if snap.reconstruction.is_some() {
            tr("Starts after the reconstructions before it")
        } else if snap.vrchat.running {
            tr("Starts once VRChat closes or the headset has been off a minute")
        } else {
            tr("Starts in a moment")
        };
        return Offer::Waiting { hint };
    }
    let again = f.reconstructed;
    let blocked = |reason, short| Offer::Blocked { again, reason, short };
    if snap.capture.state != crate::state::CaptureState::Idle && snap.capture.session_dir.as_deref() == Some(f.dir) {
        return blocked(tr(RECORDING), tr("being recorded"));
    }
    if snap.cleanup.as_ref().is_some_and(|j| j.of(f.dir).is_some()) {
        return blocked(tr(CLEANING), tr("capture files going"));
    }
    if f.trimmed && f.reconstructed {
        return blocked(tr(VIEW_ONLY), tr("capture files deleted"));
    }
    if f.no_keyframes {
        return blocked(tr(NO_KEYFRAMES), tr("no keyframes"));
    }
    if f.no_depth {
        return blocked(tr(NO_DEPTH), tr("no depth frames"));
    }
    Offer::Start { again }
}

/// [`offer`] of a card or window's entry; none on what cannot be reconstructed from this computer at all.
pub fn offer_of(e: &Entry, snap: &Snapshot) -> Option<Offer> {
    Facts::of_entry(e).map(|f| offer(&f, snap))
}

/// A press on Reconstruct again (or Reconstruct) of the capture `dir`, drawn in `origin`. With nothing to lose it starts;
/// otherwise the question, for the caller to keep and show until it is gone. `remote`: what the page knows of its share.
pub fn press(ctx: &egui::Context, cx: &mut Cx<'_>, dir: &Path, title: &str, remote: Option<&Item>, origin: Rect) -> Option<Ask> {
    let server = if cx.snap.settings.development { orbiters_core::account::Server::Development } else { orbiters_core::account::Server::Production };
    let lost = shares::reconstruct_replaces(dir, server, remote);
    if lost.is_empty() {
        started(ctx, cx, dir);
        return None;
    }
    Some(Ask::new(dir.to_path_buf(), title, lost, origin))
}

/// Starts it (in its turn), and says so when it starts now: a capture waiting has the service say what for.
fn started(ctx: &egui::Context, cx: &mut Cx<'_>, dir: &Path) {
    if cx.snap.reconstruction.is_none() && !cx.snap.is_capturing() {
        cx.toasts.push(ctx, orbiters_ui::toast::Kind::Info, tr("Reconstruction started: its card shows how far it got, and its 3D view stays until the new one is ready."));
    }
    cx.ctl.reconstruct(dir.to_path_buf());
}

/// Every question gets springs of its own.
static ASKED: AtomicU64 = AtomicU64::new(0);

/// The question before a finished run takes what the current result has: what would go, in the person's words, and
/// Reconstruct again or Cancel.
pub struct Ask {
    id: Id,
    dir: PathBuf,
    title: String,
    lost: Vec<String>,
    origin: Rect,
    /// Its footer's answer, acted on once the frame's closure is over.
    go: bool,
}

impl Ask {
    pub fn new(dir: PathBuf, title: &str, lost: Vec<String>, origin: Rect) -> Self {
        let id = Id::unique(("reconstruct-again", ASKED.fetch_add(1, Ordering::Relaxed)));
        Self { id, dir, title: title.to_string(), lost, origin, go: false }
    }

    /// Draws it; false once it is gone. Reconstruct again starts the run here.
    pub fn show(&mut self, ctx: &egui::Context, cx: &mut Cx<'_>) -> bool {
        let title = tr!("Reconstruct {title} again?", title = self.title);
        let (lost, id) = (std::mem::take(&mut self.lost), self.id);
        let mut go = false;
        let response = Dialog::new(id, self.origin, &title)
            .glyph(icons::MAGIC_WAND)
            .subtitle(tr("A new 3D view replaces the one it has"))
            .width(500.0)
            .footer(true)
            .show(ctx, |ui, part| match part {
                Part::Body => body(ui, &lost),
                Part::Footer => go |= footer(ui, ctx, id),
            });
        self.lost = lost;
        if go && !self.go {
            self.go = true;
            started(ctx, cx, &self.dir);
            dialog::close(ctx, id);
        }
        !response.closed
    }
}

fn body(ui: &mut Ui, lost: &[String]) {
    let w = ui.available_width();
    for line in lost {
        let job = ui.painter().layout_job(Txt::new(line, 13.0).color(theme::FG).line_height(19.0).job(w - 28.0));
        let h = job.size().y;
        let (r, _) = ui.allocate_exact_size(egui::vec2(w, h + 10.0), egui::Sense::hover());
        widgets::icon(ui.painter(), pos2(r.left() + 8.0, r.top() + 9.5), icons::WARNING, 15.0, theme::WARN);
        ui.painter().galley(pos2(r.left() + 28.0, r.top()), job, theme::FG);
    }
    let note = ui.painter().layout_job(Txt::new(tr("It stays as it is until the new one is ready: stopped or failed, nothing changes."), 12.0).color(theme::FG_3).line_height(18.0).job(w));
    let (r, _) = ui.allocate_exact_size(egui::vec2(w, note.size().y + 6.0), egui::Sense::hover());
    ui.painter().galley(pos2(r.left(), r.top() + 6.0), note, theme::FG_3);
}

/// Right to left: true when Reconstruct again was pressed.
fn footer(ui: &mut Ui, ctx: &egui::Context, id: Id) -> bool {
    let go = widgets::activated(&Button::new(tr("Reconstruct again")).icon(icons::MAGIC_WAND).variant(Variant::Warn).id_salt("again-go").show(ui));
    if widgets::activated(&Button::new(tr("Cancel")).variant(Variant::Ghost).id_salt("again-cancel").show(ui)) {
        dialog::close(ctx, id);
    }
    go
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{CaptureState, CleanupJob, ReconstructionJob};
    use orbiters_core::reconstruction::Stage;
    use orbiters_core::shares::Kind;

    const DIR: &str = "C:/Sessions/Home 2026-11-04 12-14-00";

    fn entry() -> Entry {
        let mut e = crate::controller::shares::remote_entry(Item::default());
        e.remote = None;
        e.local = Some(Local::Session(PathBuf::from(DIR)));
        e.ready = true;
        e
    }

    fn snap() -> Snapshot {
        Snapshot::new(Default::default())
    }

    /// A reconstructed capture of this computer offers it; one not reconstructed yet says Reconstruct.
    #[test]
    fn a_capture_of_this_computer_can_be_reconstructed_again() {
        assert_eq!(offer_of(&entry(), &snap()), Some(Offer::Start { again: true }));
        assert_eq!(Offer::Start { again: true }.label(), "Reconstruct again");
        let mut first = entry();
        first.ready = false;
        assert_eq!(offer_of(&first, &snap()), Some(Offer::Start { again: false }));
        assert_eq!(Offer::Start { again: false }.label(), "Reconstruct");
    }

    /// What is merely shared with you, a capture of yours known only to Orbiters, and pictures: never offered.
    #[test]
    fn only_a_capture_on_this_computer_is_offered() {
        let shared_with_me = Item { kind: Kind::Capture, id: 7, mine: false, ..Default::default() };
        assert_eq!(offer_of(&crate::controller::shares::remote_entry(shared_with_me), &snap()), None);
        let mine_on_orbiters = Item { kind: Kind::Capture, id: 8, mine: true, ..Default::default() };
        assert_eq!(offer_of(&crate::controller::shares::remote_entry(mine_on_orbiters), &snap()), None);
        let mut picture = entry();
        picture.local = Some(Local::Picture(PathBuf::from("C:/Pictures/a.png")));
        assert_eq!(offer_of(&picture, &snap()), None);
        let mut matched_to_someone_elses = entry();
        matched_to_someone_elses.remote = Some(Item { kind: Kind::Capture, id: 9, mine: false, ..Default::default() });
        assert_eq!(offer_of(&matched_to_someone_elses, &snap()), None);
    }

    /// Disabled with why: no depth frames, no keyframes, capture files deleted or going, still being recorded.
    #[test]
    fn it_says_why_it_cannot_be_reconstructed() {
        let reason = |o: Option<Offer>| match o {
            Some(Offer::Blocked { reason, .. }) => reason,
            other => panic!("not blocked: {other:?}"),
        };
        let mut e = entry();
        e.no_depth = true;
        assert_eq!(reason(offer_of(&e, &snap())), NO_DEPTH);
        let mut e = entry();
        e.no_keyframes = true;
        assert_eq!(reason(offer_of(&e, &snap())), NO_KEYFRAMES);
        let mut e = entry();
        e.trimmed = true;
        assert_eq!(reason(offer_of(&e, &snap())), VIEW_ONLY, "a capture reduced to its 3D view");
        assert_eq!(offer_of(&e, &snap()).unwrap().label(), "Reconstruct again");
        let mut cleaning = snap();
        cleaning.cleanup = Some(CleanupJob { current: Some((PathBuf::from(DIR), false)), ..Default::default() });
        assert_eq!(reason(offer_of(&entry(), &cleaning)), CLEANING);
        let mut recording = snap();
        recording.capture.state = CaptureState::Recording;
        recording.capture.session_dir = Some(PathBuf::from(DIR));
        assert_eq!(reason(offer_of(&entry(), &recording)), RECORDING);
        recording.capture.session_dir = Some(PathBuf::from("C:/Sessions/another"));
        assert!(matches!(offer_of(&entry(), &recording), Some(Offer::Start { .. })), "another session recording only makes it wait");
    }

    /// Running: how far as a whole, and the stop; only on the capture it runs on.
    #[test]
    fn it_shows_its_run_and_stops_it() {
        let mut running = snap();
        let mut job = ReconstructionJob::new(PathBuf::from(DIR), false);
        job.stage = Stage::Training;
        job.progress = 0.5;
        running.reconstruction = Some(job);
        let offered = offer_of(&entry(), &running).unwrap();
        assert!(matches!(offered, Offer::Running { fraction, stage: "Training splats" } if (fraction - 0.52).abs() < 1e-6), "{offered:?}");
        assert!(offered.running());
        assert_eq!(offered.label(), "Stop reconstruction");
        let mut other = entry();
        other.local = Some(Local::Session(PathBuf::from("C:/Sessions/another")));
        assert_eq!(offer_of(&other, &running), Some(Offer::Start { again: true }), "another capture's run does not stop this one");
    }

    /// A run on a capture whose files then went can still be stopped, a run outranking every reason not to start one.
    #[test]
    fn a_run_comes_before_any_reason_not_to_start() {
        let mut e = entry();
        e.trimmed = true;
        let mut running = snap();
        running.reconstruction = Some(ReconstructionJob::new(PathBuf::from(DIR), false));
        assert!(offer_of(&e, &running).unwrap().running());
    }

    /// Waiting: for what, and the cancel.
    #[test]
    fn it_says_what_it_waits_for() {
        let mut waiting = snap();
        *waiting.reconstruct_queue = vec![PathBuf::from(DIR)];
        waiting.vrchat.running = true;
        let hint = |s: &Snapshot| match offer_of(&entry(), s) {
            Some(Offer::Waiting { hint }) => hint,
            other => panic!("not waiting: {other:?}"),
        };
        assert_eq!(hint(&waiting), "Starts once VRChat closes or the headset has been off a minute");
        assert_eq!(offer_of(&entry(), &waiting).unwrap().label(), "Waiting · cancel");
        waiting.reconstruction = Some(ReconstructionJob::new(PathBuf::from("C:/Sessions/another"), false));
        assert_eq!(hint(&waiting), "Starts after the reconstructions before it");
        waiting.capture.state = CaptureState::Recording;
        assert_eq!(hint(&waiting), "Starts when the capture ends");
    }

    /// A session of the list says what its entry says.
    #[test]
    fn a_session_row_answers_as_its_card_does() {
        let mut manifest = orbiters_contracts::manifest::SessionManifest::default();
        manifest.stats.depth_frames = 0;
        let s = SessionSummary {
            dir: PathBuf::from(DIR),
            manifest,
            has_thumbnail: false,
            size_bytes: 0,
            capture_bytes: 0,
            trimmed: false,
            picture: None,
            modified: 0,
            reconstruction: None,
            damaged: None,
        };
        assert!(matches!(offer(&Facts::of_session(&s), &snap()), Offer::Blocked { again: false, reason: NO_DEPTH, .. }));
    }

    /// What a button says follows the language where it is asked, and the English a test or a log reads stays the key.
    #[test]
    fn the_words_follow_the_language_when_asked() {
        use orbiters_i18n::{in_language, Language};
        assert_eq!(Offer::Start { again: true }.label(), "Reconstruct again");
        assert_eq!(in_language(Language::Fr, || Offer::Start { again: true }.label()), "Reconstruire à nouveau");
        assert_eq!(in_language(Language::Ja, || Offer::Waiting { hint: "" }.label()), "待機中 · キャンセル");
        assert_eq!(in_language(Language::PtBr, || tr(RECORDING)), "Ainda está sendo gravado: pare a captura primeiro");
    }
}
