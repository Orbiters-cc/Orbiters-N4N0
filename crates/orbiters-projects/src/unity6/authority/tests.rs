use super::*;
use crate::unity6::trial::tests::{again, at_step, copy_tree, env, last, ok, run, run_at, vcc_migrates, Env, CONTENT};
use crate::unity6::trial::{abandon, adopt_vcc_copy, Step};

const EVERY: [Action; 5] = [Action::Open(Caller::Window), Action::Open(Caller::Bridge), Action::Packages, Action::Storage, Action::Duplicate];

fn may(e: &Env, path: &Path) -> Vec<bool> {
    EVERY.iter().map(|a| allow_on(&e.app, path, *a, false).is_ok()).collect()
}

/// Every state, and what each lets be done: open in the window, open from the bridge, packages, storage, duplicate; and
/// whether an upload may be prepared.
#[test]
fn each_state_lets_exactly_what_it_should() {
    let e = env();
    let no = [false; 5];
    // Not a trial, on 2022.
    assert_eq!(state(&e.app, &e.source), TrialState::NotATrial);
    assert_eq!(may(&e, &e.source), [true; 5]);
    assert!(upload(&e.app, &e.source).unwrap().warning.is_none());
    // Being made: asked from inside the copy.
    {
        let (app, trial) = (e.app.clone(), e.root.join("Projects/Copying"));
        let seen = std::rc::Rc::new(std::cell::Cell::new(0));
        let s = seen.clone();
        let _s = at_step(move |step| {
            if matches!(step, Step::Made | Step::Copied | Step::Compared) {
                assert!(matches!(state(&app, &trial), TrialState::Copying(_)), "{step:?}");
                assert!(EVERY.iter().all(|a| allow_on(&app, &trial, *a, false).is_err()));
                assert!(upload(&app, &trial).is_err());
                s.set(s.get() + 1);
            }
            anyhow::ensure!(step != Step::Compared, "stop here");
            Ok(())
        });
        assert!(run_at(&e, &e.root.join("Projects"), "Copying", &|_| false, &ok).is_err());
        assert_eq!(seen.get(), 3);
    }
    // Failed.
    let failed = e.root.join("Projects/Copying");
    assert!(matches!(state(&e.app, &failed), TrialState::CopyFailed(_)));
    assert_eq!(may(&e, &failed), no);
    assert!(upload(&e.app, &failed).is_err());
    // Verified, not listed.
    {
        let _s = at_step(|step| if step == Step::MarkerGone { Err(anyhow::anyhow!("no list")) } else { Ok(()) });
        assert!(run_at(&e, &e.root.join("Projects"), "V", &|_| false, &ok).is_err());
    }
    let verified = e.root.join("Projects/V");
    assert!(matches!(state(&e.app, &verified), TrialState::CopiedVerified(_)));
    assert_eq!(may(&e, &verified), no);
    assert!(upload(&e.app, &verified).is_err());
    // Waiting.
    again(&e, &last(&e).0.id).unwrap();
    assert!(matches!(state(&e.app, &verified), TrialState::WaitingForMigration(_)));
    assert_eq!(may(&e, &verified), [false, false, false, true, true]);
    assert!(upload(&e.app, &verified).is_err());
    // Migrated.
    vcc_migrates(&verified);
    assert!(matches!(state(&e.app, &verified), TrialState::Migrated(_)));
    assert_eq!(may(&e, &verified), [true, false, true, true, true], "never from the bridge");
    assert!(matches!(allow_on(&e.app, &verified, Action::Open(Caller::Window), false), Ok(Way::Trial(_))));
    assert!(allow_on(&e.app, &verified, Action::Open(Caller::Window), true).is_err(), "on the Mac, from Unity Hub");
    assert!(upload(&e.app, &verified).unwrap_err().contains("Unity 2022"));
    // Superseded.
    let job = run_at(&e, &e.root.join("Projects"), "S", &|_| false, &ok).unwrap();
    let beside = e.root.join("Projects/S-Migrated");
    copy_tree(&job.trial.path, &beside);
    vcc_migrates(&beside);
    adopt_vcc_copy(&e.app, &e.vcc, &job.id, &beside).unwrap();
    assert!(matches!(state(&e.app, &job.trial.path), TrialState::Superseded(_)));
    assert_eq!(may(&e, &job.trial.path), [false, false, false, false, true]);
    assert!(upload(&e.app, &job.trial.path).is_err());
    assert!(matches!(state(&e.app, &beside), TrialState::Migrated(_)));
    // Abandoned, whole and not.
    let failed_job = crate::unity6::journal::jobs(&e.app).into_iter().find(|j| j.trial.path == failed).unwrap();
    abandon(&e.app, &e.vcc, &failed_job.id).unwrap();
    assert_eq!(may(&e, &failed), no);
    let whole = run_at(&e, &e.root.join("Projects"), "W", &|_| false, &ok).unwrap();
    abandon(&e.app, &e.vcc, &whole.id).unwrap();
    assert_eq!(may(&e, &whole.trial.path), [true; 5]);
}

/// Every spelling of a trial's folder is the trial: through a junction, another case, `\\?\`, renamed.
#[test]
fn a_trial_is_recognised_however_it_is_named() {
    let e = env();
    let job = run(&e).unwrap();
    let is = |p: &Path| matches!(state(&e.app, p), TrialState::WaitingForMigration(_));
    assert!(is(&job.trial.path));
    #[cfg(windows)]
    {
        assert!(is(Path::new(&job.trial.path.to_string_lossy().to_uppercase())));
        assert!(is(&std::fs::canonicalize(&job.trial.path).unwrap()));
        assert!(is(&job.trial.path.join("..").join(job.trial.path.file_name().unwrap())));
        let link = e.root.join("alias");
        junction::create(&job.trial.path, &link).unwrap();
        assert!(is(&link));
        junction::delete(&link).unwrap();
    }
    let moved = e.root.join("Projects/Renamed");
    std::fs::rename(&job.trial.path, &moved).unwrap();
    assert!(is(&moved));
}

/// Second verification N3: the warning follows the content IDs. A duplicate of a trial, or a restored copy, on Unity 6 is
/// refused; on 2022 it is warned; the original keeps its 2022 uploads without a word; and what cannot be read is refused.
#[test]
fn the_upload_warning_follows_the_content_ids() {
    let e = env();
    let job = run(&e).unwrap();
    // A duplicate made while it waits, on 2022.
    let copy = e.root.join("Projects/Copy");
    copy_tree(&job.trial.path, &copy);
    let h = upload(&e.app, &copy).unwrap();
    assert!(h.warning.as_deref().unwrap().contains(CONTENT) && h.is_for(&copy) && !h.is_for(&e.source));
    // Migrated: refused, whatever made it.
    vcc_migrates(&copy);
    assert!(upload(&e.app, &copy).unwrap_err().contains(CONTENT));
    // The trial taken off, its folder gone: the copy is still recognised.
    abandon(&e.app, &e.vcc, &job.id).unwrap();
    std::fs::remove_dir_all(&job.trial.path).unwrap();
    assert!(upload(&e.app, &copy).is_err());
    // The original, on 2022.
    assert!(upload(&e.app, &e.source).unwrap().warning.is_none());
    // The original itself moved to Unity 6: its own uploads from 2022 end.
    let own = e.root.join("Projects/Restored");
    copy_tree(&e.source, &own);
    vcc_migrates(&own);
    assert!(upload(&e.app, &own).is_err());
    // Other content on Unity 6: allowed, and warned.
    std::fs::write(own.join("Assets/Avatar.unity"), "%YAML 1.1\n  blueprintId: avtr_99999999-2222-4333-8444-555555555555\n").unwrap();
    assert!(upload(&e.app, &own).unwrap().warning.unwrap().contains("Unity 2022"));
    // A line that cannot be told, scenes that cannot be read.
    std::fs::write(own.join("ProjectSettings/ProjectVersion.txt"), "nothing\n").unwrap();
    assert!(upload(&e.app, &own).is_err());
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let _held = std::fs::OpenOptions::new().read(true).share_mode(0).open(copy.join("Assets/Avatar.unity")).unwrap();
        vcc_migrates(&copy);
        assert!(upload(&e.app, &copy).is_err());
    }
}

/// The library's own entry points ask, whoever calls them: packages (before the repository is added), the SDK hand-off,
/// Open, Duplicate, the Library's move.
#[test]
fn every_entry_point_of_the_library_asks() {
    let mut e = env();
    e.app = crate::data_dir();
    let job = run(&e).unwrap();
    let at = job.trial.path.clone();
    let rt = crate::runtime().unwrap();
    let said = |r: anyhow::Result<()>| format!("{:#}", r.unwrap_err());
    assert!(said(rt.block_on(crate::vpm::change(&e.vcc, &at, crate::vpm::Change::Resolve))).contains("Creator Companion"));
    let repos = std::fs::read(e.vcc.join("settings.json")).unwrap_or_default();
    assert!(said(rt.block_on(crate::vpm::install_orbiters(&e.vcc, &at, &["orbiters.toolkit"]))).contains("Creator Companion"));
    assert_eq!(std::fs::read(e.vcc.join("settings.json")).unwrap_or_default(), repos, "no repository added for it");
    let project = crate::discovery::Project { path: at.clone(), name: "T".into(), unity: "2022.3.22f1".into(), ..Default::default() };
    assert!(said(crate::open_project(&project).map(|_| ())).contains("Creator Companion"));
    assert!(said(crate::open_project_as_is(&project).map(|_| ())).contains("Creator Companion"));
    // The SDK hand-off needs leave for that very project, and a waiting or migrated trial has none.
    let avatar = crate::avatars::Avatar { id: CONTENT.into(), ..Default::default() };
    let request = crate::setup::request(avatar, 1, "https://orbiters.cc/api/", "0.0.0").unwrap();
    let leave = upload(&e.app, &e.source).unwrap();
    assert!(crate::setup::write(&at, &request, &leave).is_err());
    assert!(!at.join("UserSettings").exists());
    // On Unity 6, the bridge opens neither it nor any project of that line.
    vcc_migrates(&at);
    assert!(said(crate::open_project_from_bridge(&project).map(|_| ())).contains("bridge"));
    let plain = e.root.join("Projects/Six");
    copy_tree(&e.source, &plain);
    std::fs::write(plain.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 6000.0.99f9\n").unwrap();
    let six = crate::discovery::Project { path: plain, name: "Six".into(), unity: "6000.0.99f9".into(), ..Default::default() };
    assert!(said(crate::open_project_from_bridge(&six).map(|_| ())).contains("bridge"));
    // A failed copy is neither moved nor duplicated.
    {
        let _s = at_step(|step| if step == Step::Copied { Err(anyhow::anyhow!("x")) } else { Ok(()) });
        assert!(run_at(&e, &e.root.join("Projects"), "F", &|_| false, &ok).is_err());
    }
    let f = e.root.join("Projects/F");
    let cancel = AtomicBool::new(false);
    assert!(crate::storage::repair(&f, &Default::default(), &cancel).is_err());
    let before = std::fs::read_dir(e.root.join("Projects")).unwrap().count();
    assert!(rt.block_on(crate::templates::duplicate(&e.vcc, &f, &e.root.join("Projects"), "F2", Default::default(), &cancel, &Default::default())).is_err());
    assert_eq!(std::fs::read_dir(e.root.join("Projects")).unwrap().count(), before);
    for j in crate::unity6::journal::jobs(&e.app).into_iter().filter(|j| j.trial.path.starts_with(&e.root)) {
        let _ = abandon(&e.app, &e.vcc, &j.id);
    }
}
