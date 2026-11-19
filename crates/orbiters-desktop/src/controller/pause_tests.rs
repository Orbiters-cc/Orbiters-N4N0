//! What the controller does around a paused reconstruction, with a stand-in worker: `cmd` waiting for a line, then saying
//! what it was told to and leaving. The real worker's side is orbiters_reconstruct::checkpoint's tests, and the walk with
//! the real worker `--only pause-resume` (crate::screenshot::pause_resume).

use super::*;
use crate::pages::reconstruct_again::{self as again, Offer};
use crate::pages::sessions::overall_progress;
use orbiters_core::reconstruction::Stage;

/// A session's folder with a checkpoint-shaped file in it.
fn session(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("orbiters-pause-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(orbiters_core::reconstruction::CHECKPOINT_FILE), b"kept").unwrap();
    dir
}

/// A controller with a run of `dir` under way, half trained, whose worker says `lines` once it is asked anything.
fn running(ctl: &Shared, dir: &Path, integrated: bool, lines: &str) {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("cmd");
    cmd.raw_arg(format!(r#"/S /C "set /p asked= {lines}""#));
    let events = ctl.clone();
    let exit = ctl.clone();
    let exits = stand_in_exits(ctl);
    exits.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let handle = ReconstructHandle::spawn_command(cmd, move |e| events.on_reconstruct_event(e), |_| {}, move || {
        exit.on_reconstruct_exit();
        exits.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }).unwrap();
    *ctl.reconstruction.lock().unwrap() = Some(Reconstruction { handle, dir: dir.to_path_buf(), told: false, integrated, pausing: None, seen: None, freed: false, stopped: false });
    ctl.update(|s| {
        let mut job = ReconstructionJob::new(dir.to_path_buf(), integrated);
        job.stage = Stage::Training;
        job.at(500, 1000, 42_000, Some(30.0), 10.0);
        s.reconstruction = Some(job);
    });
}

const PAUSED: &str = r#"& echo {"event":"pausing"} & echo {"event":"paused","kept":true,"message":null}"#; // i18n: not shown

/// How many stand-ins each controller started, and how many finished leaving. `on_reconstruct_exit` takes the run off
/// `ctl.reconstruction` first and only then deletes what it leaves and clears what shows, so a run being gone from there
/// does not mean its exit is over: on a busy computer a test looking at that moment saw the old state.
fn stand_in_exits(ctl: &Shared) -> Arc<(std::sync::atomic::AtomicUsize, std::sync::atomic::AtomicUsize)> {
    static EXITS: Mutex<Vec<(usize, Arc<(std::sync::atomic::AtomicUsize, std::sync::atomic::AtomicUsize)>)>> = Mutex::new(Vec::new());
    let id = Arc::as_ptr(ctl) as usize;
    let mut all = EXITS.lock().unwrap();
    if let Some((_, e)) = all.iter().find(|(i, _)| *i == id) {
        return e.clone();
    }
    let e = Arc::new(Default::default());
    all.push((id, Arc::clone(&e)));
    e
}

/// Waits until every stand-in of `ctl` has left and its exit is over.
fn gone(ctl: &Shared) {
    use std::sync::atomic::Ordering::SeqCst;
    let exits = stand_in_exits(ctl);
    let until = Instant::now() + Duration::from_secs(30);
    while ctl.reconstruction.lock().unwrap().is_some() || exits.1.load(SeqCst) < exits.0.load(SeqCst) {
        assert!(Instant::now() < until, "the stand-in never left");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn toast(ctl: &Shared, words: &str) -> bool {
    ctl.take_notices().iter().any(|n| matches!(n, Notice::Toast { message, .. } if message.contains(words)))
}

fn capturing(ctl: &Shared, on: bool) {
    ctl.update(|s| s.capture.state = if on { CaptureState::Recording } else { CaptureState::Idle });
}

/// night2 (the verifier): a worker that said it saw the pause is not stopped at the 15 s of one that never answers (it
/// is reading back where it stands and giving the GPU back), and is still stopped once its longer limit passes.
#[test]
fn a_worker_that_saw_the_pause_has_longer_but_not_for_ever() {
    let s = Duration::from_secs;
    assert!(!pause_overdue(s(10), None));
    assert!(pause_overdue(s(16), None), "silent: stuck after 15 s, as before");
    assert!(!pause_overdue(s(40), Some(s(30))), "it saw the pause: it may go on reading back");
    assert!(pause_overdue(s(90), Some(s(61))), "bounded");
}

#[test]
fn a_capture_pauses_the_run_and_its_progress_stays_on_show() {
    let ctl = Controller::new(Settings::default());
    let dir = session("capture");
    capturing(&ctl, true);
    running(&ctl, &dir, false, PAUSED);
    let before = ctl.view().reconstruction.as_ref().map(overall_progress).unwrap();
    assert!((before - 0.52).abs() < 1e-5);
    let asked = Instant::now();
    ctl.capture_takes_the_gpu();
    assert!(asked.elapsed() < Duration::from_millis(100), "a capture does not wait for it: {:?}", asked.elapsed());
    assert!(toast(&ctl, "continues from where it was"), "the notice");
    let j = ctl.view().reconstruction.clone().unwrap();
    assert!(j.paused && overall_progress(&j) == before);
    assert_eq!(ctl.view().reconstruct_queue, vec![dir.clone()], "first in line");
    assert!(!ctl.pause_reconstruction(), "asked once");
    gone(&ctl);
    let j = ctl.view().reconstruction.clone().expect("on show, paused");
    assert!(j.paused && overall_progress(&j) == before && j.iteration == 500);
    assert!(orbiters_reconstruct::checkpoint::exists(&dir));
    // What it shows wherever it shows anything, and why.
    let snap = ctl.view();
    let facts = again::Facts { dir: &dir, reconstructed: false, no_keyframes: false, no_depth: false, trimmed: false };
    match again::offer(&facts, &snap) {
        Offer::Paused { fraction, why } => {
            assert_eq!(fraction, before);
            assert!(why.contains("when the capture ends"), "{why}");
        }
        other => panic!("{other:?}"),
    }
    assert!(again::offer(&facts, &snap).running() && again::offer(&facts, &snap).label() == "Stop reconstruction");
    let (badge, _) = crate::pages::sessions::job_badge(&j, &snap);
    assert_eq!(badge, "Paused 52% · continues after the capture");
    // Not while the capture runs.
    ctl.next_reconstruction();
    assert!(ctl.running_reconstruction().is_none());
    assert_eq!(ctl.view().reconstruct_queue, vec![dir.clone()]);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A run going on shows what it kept while it reads its session again, then its steps; one that cannot go on says so.
#[test]
fn progress_never_falls_while_a_run_goes_on() {
    let ctl = Controller::new(Settings::default());
    let dir = session("floor");
    capturing(&ctl, true);
    running(&ctl, &dir, false, PAUSED);
    ctl.capture_takes_the_gpu();
    gone(&ctl);
    let kept = overall_progress(&ctl.view().reconstruction.clone().unwrap());
    // What start_reconstruction does, then what the worker says.
    ctl.update(|s| {
        let j = s.reconstruction.as_mut().unwrap();
        j.paused = false;
        j.resuming = true;
    });
    ctl.on_reconstruct_event(ReconstructEvent::Stage { stage: Stage::Loading, progress: 0.0 });
    assert_eq!(overall_progress(&ctl.view().reconstruction.clone().unwrap()), kept, "reading the session again does not go back");
    ctl.on_reconstruct_event(ReconstructEvent::Resumed { iteration: 500, iterations: 1000, gaussians: 42_000, psnr: Some(30.0), seconds: 10.0 });
    ctl.on_reconstruct_event(ReconstructEvent::Stage { stage: Stage::Loading, progress: 0.9 });
    let j = ctl.view().reconstruction.clone().unwrap();
    assert!(j.resuming && j.iteration == 500 && overall_progress(&j) == kept);
    ctl.on_reconstruct_event(ReconstructEvent::Stage { stage: Stage::Training, progress: 0.5 });
    let j = ctl.view().reconstruction.clone().unwrap();
    assert!(!j.resuming && overall_progress(&j) == kept);
    // Its checkpoint turned out useless: from the start, and said.
    ctl.on_reconstruct_event(ReconstructEvent::Stage { stage: Stage::Seeding, progress: 0.0 });
    let j = ctl.view().reconstruction.clone().unwrap();
    assert!(overall_progress(&j) <= 0.06 && j.iteration == 0 && j.psnr.is_none());
    assert!(toast(&ctl, "it starts over"));
    // A pause that could not be kept says why.
    ctl.on_reconstruct_event(ReconstructEvent::Paused { kept: false, message: Some("only 900 MB is free on the disk".into()) });
    assert!(toast(&ctl, "only 900 MB"));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Stop means stop, paused or running: no job, nothing waiting, nothing to go on from.
#[test]
fn stopping_a_paused_or_running_reconstruction_leaves_nothing() {
    let ctl = Controller::new(Settings::default());
    let dir = session("stop");
    capturing(&ctl, true);
    running(&ctl, &dir, false, PAUSED);
    ctl.capture_takes_the_gpu();
    gone(&ctl);
    assert!(ctl.stop_reconstruction());
    assert!(ctl.view().reconstruction.is_none() && ctl.view().reconstruct_queue.is_empty());
    assert!(!orbiters_reconstruct::checkpoint::exists(&dir), "paused and stopped");
    assert!(!ctl.stop_reconstruction(), "nothing left to stop");

    std::fs::write(dir.join(orbiters_core::reconstruction::CHECKPOINT_FILE), b"kept").unwrap();
    running(&ctl, &dir, false, "");
    assert!(ctl.stop_reconstruction());
    gone(&ctl);
    assert!(ctl.view().reconstruction.is_none() && ctl.view().reconstruct_queue.is_empty());
    assert!(!orbiters_reconstruct::checkpoint::exists(&dir), "stopped while it went on from it");

    // Stopped while it pauses.
    std::fs::write(dir.join(orbiters_core::reconstruction::CHECKPOINT_FILE), b"kept").unwrap();
    running(&ctl, &dir, false, PAUSED);
    ctl.capture_takes_the_gpu();
    assert!(ctl.stop_reconstruction());
    gone(&ctl);
    assert!(ctl.view().reconstruction.is_none() && ctl.view().reconstruct_queue.is_empty());
    assert!(!orbiters_reconstruct::checkpoint::exists(&dir));

    // Taken off the list after the app opened on it.
    std::fs::write(dir.join(orbiters_core::reconstruction::CHECKPOINT_FILE), b"kept").unwrap();
    ctl.change_queue(|q| q.push_back(dir.clone()));
    ctl.unqueue_reconstruction(&dir);
    assert!(!orbiters_reconstruct::checkpoint::exists(&dir));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A pause that came too late: it finished, or failed. Nothing goes on, and what shows is over.
#[test]
fn a_run_that_ends_as_it_is_asked_to_pause_is_over() {
    let ctl = Controller::new(Settings::default());
    let dir = session("late");
    capturing(&ctl, true);
    running(&ctl, &dir, false, r#"& echo {"event":"error","message":"late"}"#);
    ctl.capture_takes_the_gpu();
    gone(&ctl);
    assert!(toast(&ctl, "failed: late"));
    assert!(ctl.view().reconstruction.is_none() && ctl.view().reconstruct_queue.is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A worker that leaves without a word while pausing goes on from its last checkpoint: the file is what counts. One that
/// crashes unasked starts over: what it went on from may be what crashed it.
#[test]
fn a_worker_that_dies_keeps_or_loses_what_it_should() {
    let ctl = Controller::new(Settings::default());
    let dir = session("dies");
    capturing(&ctl, true);
    running(&ctl, &dir, false, "");
    ctl.capture_takes_the_gpu();
    gone(&ctl);
    assert!(ctl.view().reconstruction.as_ref().is_some_and(|j| j.paused) && ctl.view().reconstruct_queue == vec![dir.clone()]);
    assert!(orbiters_reconstruct::checkpoint::exists(&dir));
    assert!(!toast(&ctl, "stopped unexpectedly"));
    ctl.change_queue(VecDeque::clear);
    ctl.update(|s| s.reconstruction = None);

    // Unasked: the stand-in leaves as soon as it is told anything, so tell it nothing a pause would.
    running(&ctl, &dir, false, "");
    ctl.reconstruction.lock().unwrap().as_mut().unwrap().handle.cancel();
    gone(&ctl);
    assert!(toast(&ctl, "stopped unexpectedly"));
    assert!(!orbiters_reconstruct::checkpoint::exists(&dir));
    assert!(ctl.view().reconstruction.is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// All pauses behave alike: held for VRChat whenever the run started, moved between GPUs as the setting says.
#[test]
fn a_run_follows_where_the_setting_has_it_beside_vrchat() {
    let settings_with = |w: WhilePlaying| Settings { reconstruct_while_playing: w, ..Settings::default() };
    let dir = session("playing");

    // Beside VRChat on the card when the setting says so: VRChat starting changes nothing.
    let ctl = Controller::new(settings_with(WhilePlaying::GraphicsCard));
    capturing(&ctl, true);
    running(&ctl, &dir, false, PAUSED);
    ctl.update(|s| s.vrchat.running = true);
    ctl.vrchat_changed(true);
    assert!(ctl.running_reconstruction().is_some() && !ctl.view().reconstruction.clone().unwrap().paused);
    ctl.stop_reconstruction();
    gone(&ctl);

    // After VRChat: it pauses, says so, and says when it goes on.
    std::fs::write(dir.join(orbiters_core::reconstruction::CHECKPOINT_FILE), b"kept").unwrap();
    let ctl = Controller::new(settings_with(WhilePlaying::AfterVrchat));
    running(&ctl, &dir, false, PAUSED);
    ctl.update(|s| s.vrchat.running = true);
    ctl.vrchat_changed(true);
    assert!(ctl.view().reconstruction.clone().unwrap().paused);
    assert!(toast(&ctl, "once VRChat closes"));
    gone(&ctl);
    assert!(crate::pages::sessions::pause_why(&ctl.view()).contains("once VRChat closes"));
    assert!(ctl.running_reconstruction().is_none(), "held");

    // On a break it may run; the headset back on pauses it.
    ctl.change_queue(VecDeque::clear);
    *ctl.headset_off_since.lock().unwrap() = Some(Instant::now() - HEADSET_BREAK * 2);
    running(&ctl, &dir, false, PAUSED);
    ctl.follow_while_playing();
    assert!(!ctl.view().reconstruction.clone().unwrap().paused, "on a break");
    ctl.set_headset(Some(true));
    assert!(ctl.view().reconstruction.clone().unwrap().paused, "the break is over");
    gone(&ctl);

    // On the integrated GPU beside VRChat: to the card once it closes, keeping what it did.
    let ctl = Controller::new(settings_with(WhilePlaying::IntegratedGpu));
    capturing(&ctl, true);
    ctl.update(|s| s.vrchat.running = true);
    running(&ctl, &dir, true, PAUSED);
    ctl.follow_while_playing();
    assert!(!ctl.view().reconstruction.clone().unwrap().paused, "where the setting has it");
    ctl.update(|s| s.vrchat.running = false);
    ctl.vrchat_changed(false);
    assert!(ctl.view().reconstruction.clone().unwrap().paused);
    assert!(toast(&ctl, "on the graphics card"));
    gone(&ctl);

    // And to the integrated GPU when VRChat starts beside a run on the card.
    ctl.change_queue(VecDeque::clear);
    running(&ctl, &dir, false, PAUSED);
    ctl.update(|s| s.vrchat.running = true);
    ctl.vrchat_changed(true);
    assert!(ctl.view().reconstruction.clone().unwrap().paused);
    assert!(toast(&ctl, "on the integrated GPU"));
    gone(&ctl);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Closing the app asks it to keep what it has, and waits for it within reason; the next start finds it.
#[test]
fn quitting_keeps_a_run_for_the_next_time() {
    let dir = session("quit");
    let root = dir.parent().unwrap().to_path_buf();
    let settings = Settings { sessions_dir: root, ..Settings::default() };
    let ctl = Controller::new(settings);
    running(&ctl, &dir, false, PAUSED);
    let asked = Instant::now();
    ctl.shutdown();
    assert!(asked.elapsed() < QUIT_PAUSE_LIMIT, "it left by itself");
    assert!(ctl.reconstruction.lock().unwrap().is_none());
    assert!(orbiters_reconstruct::checkpoint::exists(&dir));
    std::fs::remove_dir_all(&dir).unwrap();
}
