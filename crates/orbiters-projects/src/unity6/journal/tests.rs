use super::*;
use crate::unity6::trial::tests::{env, run, Env};

fn text(e: &Env) -> Vec<u8> {
    std::fs::read(e.app.join(JOURNAL)).unwrap()
}

fn put(e: &Env, bytes: &[u8]) {
    std::fs::write(e.app.join(JOURNAL), bytes).unwrap();
}

/// A line that cannot be read hides only itself: the other jobs are there, the one it was about is uncertain, and nothing is
/// ever written over it.
#[test]
fn an_unreadable_line_hides_only_itself_and_is_never_written_over() {
    let e = env();
    let one = run(&e).unwrap();
    let mut two = one.clone();
    two.id = "b".repeat(32);
    two.trial.path = e.root.join("Projects/Two");
    two.trial.id = None;
    insert(&e.app, &two).unwrap();
    let good = text(&e);
    let lines: Vec<&[u8]> = good.split(|b| *b == b'\n').filter(|l| !l.is_empty()).collect();
    let joined = |l: &[Vec<u8>]| l.iter().flat_map(|x| x.iter().copied().chain([b'\n'])).collect::<Vec<u8>>();
    // Every line of the first job altered or cut, at many places: the second job is always whole.
    for (i, _) in lines.iter().enumerate().take(lines.len() - 1) {
        for cut in [1, 10, 40, lines[i].len() / 2, lines[i].len() - 1] {
            let mut all: Vec<Vec<u8>> = lines.iter().map(|l| l.to_vec()).collect();
            all[i].truncate(cut);
            put(&e, &joined(&all));
            let r = read(&e.app);
            assert!(r.job(&two.id).is_some() && r.uncertain(&two.id).is_none(), "line {i} cut at {cut}");
            assert!(matches!(r.health(), Health::Damaged { lines: 1, .. }));
        }
    }
    // The first job's last line altered in one character: it is not what it was before, and not taken for it either.
    let last_of_one = lines.len() - 2;
    let mut all: Vec<Vec<u8>> = lines.iter().map(|l| l.to_vec()).collect();
    let at = String::from_utf8_lossy(&all[last_of_one]).find("Ready").unwrap();
    all[last_of_one][at] = b'X';
    let damaged = joined(&all);
    put(&e, &damaged);
    let r = read(&e.app);
    assert!(r.uncertain(&one.id).is_some());
    assert!(update(&e.app, &one.id, |_| Ok(())).is_err(), "nothing is done with it");
    assert!(matches!(crate::unity6::authority::state(&e.app, &one.trial.path), crate::unity6::authority::TrialState::Unknown(_)));
    // Work on the other goes on, after it.
    set(&e.app, &two.id, |j| j.error = Some("x".into())).unwrap();
    assert!(text(&e).starts_with(&damaged), "append only");
    // One cut before it names any folder still makes the job it is about uncertain.
    {
        let mut all: Vec<Vec<u8>> = lines.iter().map(|l| l.to_vec()).collect();
        let cut = String::from_utf8_lossy(&all[last_of_one]).find("created").unwrap();
        all[last_of_one].truncate(cut);
        put(&e, &joined(&all));
        let r = read(&e.app);
        assert!(r.named(&one.trial.path).is_none() && r.uncertain(&one.id).is_some());
        assert!(matches!(crate::unity6::authority::state(&e.app, &one.trial.path), crate::unity6::authority::TrialState::Unknown(_)));
        put(&e, &damaged);
        set(&e.app, &two.id, |j| j.error = Some("x".into())).unwrap();
    }
    // Set aside: kept beside and in the journal, not asked about again; the last good line counts.
    let aside = set_aside(&e.app).unwrap();
    assert!(std::fs::read_to_string(aside).unwrap().contains("Xeady"));
    assert!(text(&e).starts_with(&damaged));
    let r = read(&e.app);
    assert_eq!(r.health(), Health::Fine);
    assert!(r.uncertain(&one.id).is_none() && r.job(&one.id).is_some());
    assert!(set_aside(&e.app).is_err(), "nothing left to set aside");
}

/// A line cut by a crash stays as it is; what comes after is on a line of its own.
#[test]
fn a_torn_end_is_kept_and_followed() {
    let e = env();
    let job = run(&e).unwrap();
    let mut torn = text(&e);
    let keep = torn.len() - 30;
    torn.truncate(keep);
    put(&e, &torn);
    assert!(read(&e.app).uncertain(&job.id).is_some());
    let mut other = job.clone();
    other.id = "c".repeat(32);
    other.trial.path = e.root.join("Projects/Other");
    insert(&e.app, &other).unwrap();
    assert!(text(&e).starts_with(&torn));
    let r = read(&e.app);
    assert!(r.job(&other.id).is_some() && r.uncertain(&other.id).is_none());
}

/// A line of a newer version: shown as such, never set aside, its job never acted on, its folder never taken for an ordinary
/// project.
#[test]
fn a_newer_line_is_kept_and_its_job_not_touched() {
    let e = env();
    let job = run(&e).unwrap();
    let mut all = text(&e);
    let newer = serde_json::json!({ "v": 2, "what": { "id": job.id, "trial": job.trial.path } }).to_string();
    all.extend(newer.bytes());
    all.push(b'\n');
    put(&e, &all);
    let r = read(&e.app);
    assert!(matches!(r.health(), Health::Damaged { newer: true, .. }));
    assert!(matches!(crate::unity6::authority::state(&e.app, &job.trial.path), crate::unity6::authority::TrialState::Unknown(_)));
    assert!(set_aside(&e.app).is_err());
    assert_eq!(text(&e), all);
    // One about a trial this version knows nothing of still guards the folder it names.
    let elsewhere = e.root.join("Projects/Elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let mut only = serde_json::json!({ "v": 3, "at": elsewhere }).to_string().into_bytes();
    only.push(b'\n');
    put(&e, &only);
    assert!(matches!(crate::unity6::authority::state(&e.app, &elsewhere), crate::unity6::authority::TrialState::Unknown(_)));
    assert_eq!(crate::unity6::authority::state(&e.app, &e.source), crate::unity6::authority::TrialState::NotATrial);
}

/// A journal that cannot be opened at all refuses everything; an absent one is an empty one.
#[test]
fn a_journal_that_cannot_be_read_refuses() {
    let e = env();
    assert_eq!(read(&e.app).health(), Health::Fine);
    std::fs::create_dir_all(e.app.join(JOURNAL)).unwrap();
    assert!(matches!(read(&e.app).health(), Health::Unreadable(_)));
    assert!(matches!(crate::unity6::authority::state(&e.app, &e.source), crate::unity6::authority::TrialState::Unknown(_)));
    assert!(run(&e).is_err());
}

/// A record edited by hand leads nowhere else: ids and paths are checked, and logs are at their own place whatever it says.
#[test]
fn a_record_edited_by_hand_leads_nowhere_else() {
    let e = env();
    let job = run(&e).unwrap();
    for bad in [
        { let mut j = job.clone(); j.id = "../../x".into(); j },
        { let mut j = job.clone(); j.trial.path = PathBuf::from("relative"); j },
        { let mut j = job.clone(); j.trial.path = e.source.join("Assets/T"); j },
        { let mut j = job.clone(); j.trial.path = e.root.join("a/../b"); j },
    ] {
        let body = serde_json::to_string(&Body::Put(Box::new(bad.clone()))).unwrap();
        let line = serde_json::to_string(&Line { v: 1, crc: format!("{:08x}", crc32(body.as_bytes())), body }).unwrap();
        put(&e, format!("{line}\n").as_bytes());
        let r = read(&e.app);
        assert!(r.jobs.is_empty() && r.unread.len() == 1);
        assert!(insert(&e.app, &bad).is_err());
    }
    assert!(Lease::take(&e.app, "../../x").is_err());
    assert!(!e.root.join("x.lock").exists());
    // A log's path is not even in the journal.
    put(&e, b"");
    let mut j = job.clone();
    j.attempts.push(Attempt { n: 3, started: 1, log: e.root.join("outside.log"), editor: "x".into(), unity: "x".into(), start: None, process: None, reserved: false });
    insert(&e.app, &j).unwrap();
    assert!(!String::from_utf8_lossy(&text(&e)).contains("outside.log"));
    assert_eq!(jobs(&e.app)[0].attempts[0].log, e.app.join(LOGS).join(&j.id).join("attempt-3.log"));
}

/// The child of the tests below: another real process of this very program.
#[test]
#[ignore = "started by the tests of two processes"]
fn child() {
    let Ok(dir) = std::env::var("ORBITERS_JOURNAL_DIR") else { return };
    let (dir, what) = (PathBuf::from(dir), std::env::var("ORBITERS_JOURNAL_DO").unwrap());
    let id = std::env::var("ORBITERS_JOURNAL_ID").unwrap_or_default();
    match what.as_str() {
        "add" => {
            let n: usize = std::env::var("ORBITERS_JOURNAL_N").unwrap().parse().unwrap();
            for _ in 0..n {
                set(&dir, &id, |j| j.import_high += 1).unwrap();
            }
        }
        // Holds the lease and the journal's lock until it is killed, saying when it has them.
        "hold" => {
            let app = AppRoot::open(&dir).unwrap();
            let _lease = Lease::take(&dir, &id).unwrap();
            let _lock = lock(&app).unwrap();
            app.write_new(Path::new("holding"), b"x").unwrap();
            std::thread::sleep(std::time::Duration::from_secs(120));
        }
        other => panic!("{other}"),
    }
}

fn start(e: &Env, what: &str, id: &str, n: usize) -> std::process::Child {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "unity6::journal::tests::child", "--ignored", "--nocapture"])
        .env("ORBITERS_JOURNAL_DIR", &e.app)
        .env("ORBITERS_JOURNAL_DO", what)
        .env("ORBITERS_JOURNAL_ID", id)
        .env("ORBITERS_JOURNAL_N", n.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap()
}

/// Two real processes change the same job, forty times each, while this one does too: no change is lost.
#[test]
fn processes_writing_at_once_lose_nothing() {
    let e = env();
    let job = run(&e).unwrap();
    let before = jobs(&e.app)[0].import_high;
    let mut children = [start(&e, "add", &job.id, 40), start(&e, "add", &job.id, 40)];
    for _ in 0..40 {
        set(&e.app, &job.id, |j| j.import_high += 1).unwrap();
    }
    for c in &mut children {
        assert!(c.wait().unwrap().success());
    }
    let r = read(&e.app);
    assert_eq!(r.jobs[0].import_high, before + 120);
    assert_eq!(r.health(), Health::Fine);
}

/// A process that holds the locks and is killed leaves nothing held: the system lets them go.
#[test]
fn a_killed_process_leaves_no_lock_behind() {
    let e = env();
    let job = run(&e).unwrap();
    let mut child = start(&e, "hold", &job.id, 0);
    let limit = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !e.app.join("holding").exists() {
        assert!(std::time::Instant::now() < limit, "the child did not start");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(Lease::held(&e.app, &job.id), "held by another process");
    assert!(Lease::take(&e.app, &job.id).is_err());
    let app = AppRoot::open(&e.app).unwrap();
    assert!(app.try_lock(Path::new(LOCK)).unwrap().is_none());
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(!Lease::held(&e.app, &job.id));
    drop(Lease::take(&e.app, &job.id).unwrap());
    set(&e.app, &job.id, |j| j.error = None).unwrap();
}

/// Third verification B5: the only line of a trial destroyed past recognition, its folder is not an ordinary project: where a
/// job's folders are is kept beside its logs too. Set aside by the person, it is.
#[test]
fn a_trial_whose_only_line_is_lost_is_not_an_ordinary_project() {
    use crate::unity6::authority::{allow, state, Action, TrialState};
    let e = env();
    let job = run(&e).unwrap();
    // Every line of it, each damaged so that nothing of it can be read.
    let lines = text(&e).iter().filter(|b| **b == b'\n').count();
    put(&e, &b"################\n".repeat(lines));
    let r = read(&e.app);
    assert!(r.jobs.is_empty() && r.named(&job.trial.path).is_none());
    assert!(matches!(state(&e.app, &job.trial.path), TrialState::Unknown(_)));
    for action in [Action::Packages, Action::List, Action::Unlist, Action::Duplicate, Action::Storage] {
        assert!(allow(&e.app, &job.trial.path, action).is_err(), "{action:?}");
    }
    // Moved, it still is what it is; other projects are what they are.
    let moved = e.root.join("Projects/Moved");
    std::fs::rename(&job.trial.path, &moved).unwrap();
    assert!(matches!(state(&e.app, &moved), TrialState::Unknown(_)));
    assert_eq!(state(&e.app, &e.source), TrialState::NotATrial);
    set_aside(&e.app).unwrap();
    assert_eq!(state(&e.app, &moved), TrialState::NotATrial);
}
