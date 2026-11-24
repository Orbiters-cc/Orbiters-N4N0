use super::*;
use crate::copying::BETWEEN;
use crate::unity6::editors::{Arch, Candidate, Check, Found, Hint, Identity};
use crate::unity6::journal::{JOURNAL, LOGS};
use crate::unity6::standing::tests::{project, U2022, U6};
use crate::unity6::standing::{SDK_AVATARS, SDK_BASE};

pub(crate) const CONTENT: &str = "avtr_11111111-2222-4333-8444-555555555555";

/// A 2022 avatar project with its SDK installed and one uploaded avatar.
pub(crate) fn source(root: &Path) -> PathBuf {
    let p = root.join("Projects/My avatar");
    project(&p, Some(U2022), &[(SDK_BASE, "3.10.6"), (SDK_AVATARS, "3.10.6")], &[(SDK_BASE, "3.10.6", "2022.3"), (SDK_AVATARS, "3.10.6", "2022.3")]);
    std::fs::write(p.join("Packages/manifest.json"), r#"{"dependencies":{"com.unity.timeline":"1.7.5"}}"#).unwrap();
    std::fs::write(p.join("Packages/packages-lock.json"), r#"{"dependencies":{"com.unity.timeline":{"version":"1.7.5","source":"registry","url":"https://packages.unity.com"}}}"#).unwrap();
    std::fs::create_dir_all(p.join("Assets/Empty")).unwrap();
    std::fs::write(p.join("Assets/Avatar.unity"), scene(CONTENT)).unwrap();
    std::fs::write(p.join("Assets/Avatar.unity.meta"), "fileFormatVersion: 2\nguid: 0123456789abcdef0123456789abcdef\n").unwrap();
    std::fs::create_dir_all(p.join("Library")).unwrap();
    std::fs::write(p.join("Library/ArtifactDB"), "cache").unwrap();
    std::fs::write(p.join("ProjectSettings/ProjectSettings.asset"), "PlayerSettings:\n  productName: My avatar\n  productGUID: 0123\n").unwrap();
    p
}

pub(crate) fn target() -> Target {
    Target { sdk: "3.11.0-beta.1".into(), unity: "6000.0.67f1".into(), changeset: "78a1c2bbeb6a".into() }
}

pub(crate) struct Env {
    pub _temp: tempfile::TempDir,
    pub root: PathBuf,
    pub app: PathBuf,
    pub vcc: PathBuf,
    pub source: PathBuf,
}

/// A link to a folder, as this system's are.
pub(crate) fn link(target: &Path, at: &Path) {
    #[cfg(windows)]
    junction::create(target, at).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, at).unwrap();
}

/// Takes the link itself away.
pub(crate) fn unlink(at: &Path) {
    #[cfg(windows)]
    {
        junction::delete(at).unwrap();
        let _ = std::fs::remove_dir(at);
    }
    #[cfg(unix)]
    std::fs::remove_file(at).unwrap();
}

/// A file nobody else can read while this lives.
pub(crate) struct Kept {
    #[cfg(windows)]
    _file: std::fs::File,
    #[cfg(unix)]
    path: PathBuf,
}

pub(crate) fn kept_from_all(path: &Path) -> Kept {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        Kept { _file: std::fs::OpenOptions::new().read(true).share_mode(0).open(path).unwrap() }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0)).unwrap();
        Kept { path: path.to_path_buf() }
    }
}

#[cfg(unix)]
impl Drop for Kept {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o644));
    }
}

/// [`super::launch`] with no editor a crash left.
pub(crate) fn launch(dir: &Path, id: &str, find: Find, open: &dyn Fn(&Path) -> bool, alive: &dyn Fn(&Process) -> bool, start: Start) -> anyhow::Result<Attempt> {
    super::launch(dir, id, find, open, &Processes { alive, of_log: &|_| None }, start)
}

/// [`super::look`] with the app data alone.
pub(crate) fn look(dir: &Path, source: &Path, parent: &Path, name: &str, open: bool, cancel: &AtomicBool) -> Look {
    super::look(dir, &[dir], source, parent, name, open, cancel)
}

pub(crate) fn env() -> Env {
    // The trial's own rules are the same everywhere: these tests run on every system.
    AVAILABLE.with(|a| a.set(Some(true)));
    let temp = tempfile::Builder::new().prefix("unity6-trial").tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    // As a person writes it.
    let root = PathBuf::from(root.to_string_lossy().trim_start_matches(r"\\?\").to_string());
    let source = source(&root);
    Env { app: root.join("app"), vcc: root.join("vcc"), source, root, _temp: temp }
}

pub(crate) fn ok(_: &Path, _: u64) -> anyhow::Result<u64> {
    Ok(10)
}

pub(crate) fn run_at(e: &Env, parent: &Path, name: &str, open: &dyn Fn(&Path) -> bool, budget: &dyn Fn(&Path, u64) -> anyhow::Result<u64>) -> anyhow::Result<Job> {
    let recorded = |_: &Job| {};
    prepare(Prepare { dir: &e.app, source: &e.source, parent, name, target: target(), open, budget, recorded: &recorded }, &AtomicBool::new(false), &Default::default())
}

pub(crate) fn run(e: &Env) -> anyhow::Result<Job> {
    run_at(e, &e.root.join("Projects"), &trial_name("My avatar"), &|_| false, &ok)
}

pub(crate) fn again(e: &Env, id: &str) -> anyhow::Result<Job> {
    finish(&e.app, id, &AtomicBool::new(false), &Default::default())
}

/// The job recorded last, and where it stands.
pub(crate) fn last(e: &Env) -> (Job, Status) {
    let record = journal::read(&e.app);
    let job = record.jobs.last().expect("a job").clone();
    let s = status(&e.app, &record, &job);
    (job, s)
}

pub(crate) fn listed(e: &Env, path: &Path) -> bool {
    let all = crate::gallery::listed(&e.vcc).projects;
    all.iter().any(|p| crate::discovery::key(p) == crate::discovery::key(path))
}

/// Every file of VCC's folder, by its bytes.
pub(crate) fn store(e: &Env) -> Vec<(PathBuf, Vec<u8>)> {
    let walk = walkdir::WalkDir::new(&e.vcc).sort_by_file_name().into_iter().flatten().filter(|f| f.file_type().is_file());
    walk.map(|f| (f.path().to_path_buf(), std::fs::read(f.path()).unwrap())).collect()
}

/// A store of Creator Companion's with two projects of the person's, in both files.
pub(crate) fn their_store(e: &Env) {
    std::fs::create_dir_all(&e.vcc).unwrap();
    for name in ["Theirs A", "Theirs B"] {
        let at = e.root.join("Theirs").join(name);
        copy_tree(&e.source, &at);
        crate::gallery::add(&e.vcc, &at).unwrap();
    }
}

/// A whole scene of Unity's with one object uploaded as `id`.
pub(crate) fn scene(id: &str) -> String {
    format!("%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!114 &1\nMonoBehaviour:\n  blueprintId: {id}\n  completedSDKPipeline: 1\n")
}

/// Does `f` between two steps.
pub(crate) fn at_step(f: impl Fn(Step) -> anyhow::Result<()> + 'static) -> Stepping {
    BETWEEN.with(|b| *b.borrow_mut() = Some(Box::new(f)));
    Stepping
}
pub(crate) struct Stepping;
impl Drop for Stepping {
    fn drop(&mut self) {
        BETWEEN.with(|b| *b.borrow_mut() = None);
    }
}

/// The files Orbiters never writes on a trial once it made them.
fn guarded(project: &Path) -> Vec<(String, Option<Vec<u8>>)> {
    ["Packages/vpm-manifest.json", "Packages/packages-lock.json", "Packages/manifest.json", "ProjectSettings/ProjectVersion.txt"]
        .iter()
        .map(|f| (f.to_string(), std::fs::read(project.join(f)).ok()))
        .collect()
}

pub(crate) fn exact(root: &Path, version: &str, changeset: Option<&str>) -> Candidate {
    let program = root.join("Editors").join(version).join("Editor/Unity.exe");
    std::fs::create_dir_all(program.parent().unwrap()).unwrap();
    std::fs::write(&program, b"stand-in").unwrap();
    Candidate {
        program,
        hint: Hint::HubFolder,
        check: Check::Verified,
        identity: Some(Identity { version: version.into(), changeset: changeset.map(Into::into), arch: vec![Arch::here()] }),
        modules: Default::default(),
    }
}

/// What Creator Companion's migration leaves in the trial (the test stands in for it: Orbiters never does this).
pub(crate) fn vcc_migrates(trial: &Path) {
    project(trial, Some(U6), &[(SDK_BASE, "3.11.0-beta.1"), (SDK_AVATARS, "3.11.0-beta.1")], &[(SDK_BASE, "3.11.0-beta.1", "6000.0"), (SDK_AVATARS, "3.11.0-beta.1", "6000.0")]);
}

pub(crate) fn copy_tree(from: &Path, to: &Path) {
    for f in walkdir::WalkDir::new(from).into_iter().flatten() {
        let at = to.join(f.path().strip_prefix(from).unwrap());
        if f.file_type().is_dir() {
            std::fs::create_dir_all(at).unwrap();
        } else {
            std::fs::copy(f.path(), at).unwrap();
        }
    }
}

pub(crate) fn slash(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// An external local package of the original.
pub(crate) fn external(e: &Env, payload: &str) -> PathBuf {
    let ext = e.root.join("External/com.example.tool");
    std::fs::create_dir_all(&ext).unwrap();
    std::fs::write(ext.join("package.json"), r#"{"name":"com.example.tool","version":"1.0.0"}"#).unwrap();
    std::fs::write(ext.join("data.txt"), payload).unwrap();
    std::fs::write(e.source.join("Packages/manifest.json"), format!(r#"{{"dependencies":{{"com.example.tool":"file:{}"}}}}"#, slash(&ext))).unwrap();
    ext
}

#[test]
fn a_trial_is_recorded_copied_compared_and_waits_for_creator_companion() {
    let e = env();
    let before = guarded(&e.source);
    let job = run(&e).unwrap();
    assert_eq!(job.phase, Phase::Ready);
    assert!(job.trial.id.is_some() && job.source.id.is_some(), "both bound to their identity");
    assert_eq!(job.content, [CONTENT]);
    assert!(e.app.join(JOURNAL).is_file() && e.app.join(LOGS).join(&job.id).join(VERIFIED).is_file());
    assert!(store(&e).is_empty() && !e.vcc.exists(), "nothing of Creator Companion's");
    assert_eq!(folders(&e.app), [job.trial.path.clone()], "in the gallery by Orbiters' own record");
    assert_eq!(guarded(&job.trial.path), before);
    assert_eq!(guarded(&e.source), before, "the original untouched");
    assert!(job.trial.path.join("Assets/Empty").is_dir() && !job.trial.path.join("Library").exists() && !job.trial.path.join(INCOMPLETE).exists());
    let settings = std::fs::read_to_string(job.trial.path.join("ProjectSettings/ProjectSettings.asset")).unwrap();
    assert!(settings.contains(&job.guid) && settings.contains("My avatar (Unity 6)"), "{settings}");
    assert_eq!(last(&e).1, Status::Waiting { vcc_copy: None });
    let editor = exact(&e.root, "6000.0.67f1", Some("78a1c2bbeb6a"));
    assert!(launch(&e.app, &job.id, &|_, _| Found::Exact(editor.clone()), &|_| false, &|_| false, &|_, _, _| panic!("started")).is_err());
}

/// Second verification B2: a copy that is not what the original holds, byte for byte, is never ready: a file altered (at the
/// same size), missing or added during the copy, and the list of what differs is kept.
#[test]
fn a_copy_that_differs_by_a_byte_is_a_failed_copy() {
    type Change = fn(&Path);
    let changes: [(&str, Change); 4] = [
        ("Assets/Avatar.unity", |t| {
            let p = t.join("Assets/Avatar.unity");
            let mut b = std::fs::read(&p).unwrap();
            b[0] ^= 1;
            std::fs::write(p, b).unwrap();
        }),
        ("ProjectSettings/ProjectSettings.asset", |t| std::fs::remove_file(t.join("ProjectSettings/ProjectSettings.asset")).unwrap()),
        ("Packages/manifest.json", |t| std::fs::remove_file(t.join("Packages/manifest.json")).unwrap()),
        ("Assets/Extra.txt", |t| std::fs::write(t.join("Assets/Extra.txt"), "x").unwrap()),
    ];
    // A folder of it is held while it is made.
    #[cfg(windows)]
    {
        let e = env();
        let trial = e.root.join("Projects/T");
        let _s = at_step(move |s| {
            if s == Step::Copied {
                assert!(std::fs::remove_dir(trial.join("Assets/Empty")).is_err());
            }
            Ok(())
        });
        run_at(&e, &e.root.join("Projects"), "T", &|_| false, &ok).unwrap();
    }
    for (name, change) in changes {
        let e = env();
        let trial = e.root.join("Projects/T");
        let _s = at_step(move |s| {
            if s == Step::Copied {
                change(&trial);
            }
            Ok(())
        });
        let err = run_at(&e, &e.root.join("Projects"), "T", &|_| false, &ok).unwrap_err();
        let (job, s) = last(&e);
        assert_eq!((job.phase, s), (Phase::Failed, Status::Failed { kept: true }), "{name}: {err:#}");
        let named = name.replace('/', std::path::MAIN_SEPARATOR_STR);
        assert!(job.differences.iter().any(|d| d.starts_with(&named)), "{name}: {:?}", job.differences);
        assert!(folders(&e.app).is_empty() && !crate::discovery::is_project(&job.trial.path), "never a project");
        assert!(again(&e, &job.id).is_err(), "never finished");
    }
}

/// Second verification B2: Finish takes nothing on its name: every byte is compared again with what was verified.
#[test]
fn finish_compares_every_byte_again() {
    for (name, change) in [
        ("nothing", (|_: &Path| {}) as fn(&Path)),
        ("altered", |t| std::fs::write(t.join("Packages/com.example.tool/data.txt"), "BBBB").unwrap()),
        ("added", |t| std::fs::write(t.join("Assets/new.txt"), "x").unwrap()),
        ("missing", |t| std::fs::remove_file(t.join("Packages/com.example.tool/data.txt")).unwrap()),
    ] {
        let e = env();
        external(&e, "AAAA");
        // Verified, and cut short before its last step.
        {
            let _s = at_step(|s| if s == Step::Verified { Err(anyhow::anyhow!("cut short")) } else { Ok(()) });
            assert!(run_at(&e, &e.root.join("Projects"), "T", &|_| false, &ok).is_err());
        }
        let (job, s) = last(&e);
        assert_eq!((job.phase, &s), (Phase::Verified, &Status::Unfinished));
        assert!(!crate::discovery::is_project(&job.trial.path), "its marker is there");
        change(&job.trial.path);
        let done = again(&e, &job.id);
        let (after, s) = last(&e);
        if name == "nothing" {
            assert!(done.is_ok() && s == Status::Waiting { vcc_copy: None } && crate::discovery::is_project(&job.trial.path));
            // Done once: not again.
            assert!(again(&e, &job.id).is_err());
        } else {
            assert!(done.is_err() && matches!(s, Status::Failed { kept: true }), "{name}: {s:?}");
            assert!(!after.differences.is_empty() && !crate::discovery::is_project(&job.trial.path), "{name}");
        }
    }
}

/// Second verification N10: a local package out of a Git checkout is compared without what the copy leaves out.
#[test]
fn a_package_from_a_git_checkout_is_copied_and_compared_without_its_history() {
    let e = env();
    let ext = external(&e, "AAAA");
    std::fs::create_dir_all(ext.join(".git")).unwrap();
    std::fs::write(ext.join(".git/HEAD"), "ref").unwrap();
    let job = run(&e).unwrap();
    assert_eq!(job.localized, ["com.example.tool"]);
    assert_eq!(std::fs::read_to_string(job.trial.path.join("Packages/com.example.tool/data.txt")).unwrap(), "AAAA");
    assert!(!job.trial.path.join("Packages/com.example.tool/.git").exists());
    let m = std::fs::read_to_string(job.trial.path.join("Packages/manifest.json")).unwrap();
    assert!(m.contains("file:com.example.tool") && !m.contains("External"), "{m}");
}

/// Second verification N1: the original's links are copied as ordinary files and reported; the trial holds no link.
#[test]
fn linked_folders_become_ordinary_files_of_the_trial() {
    let e = env();
    let shared = e.root.join("Shared");
    std::fs::create_dir_all(shared.join("Deep")).unwrap();
    std::fs::write(shared.join("Deep/shared.mat"), "m").unwrap();
    link(&shared, &e.source.join("Assets/Shared"));
    let job = run(&e).unwrap();
    let at = job.trial.path.join("Assets/Shared");
    assert!(!crate::owned::is_link(&std::fs::symlink_metadata(&at).unwrap()));
    assert_eq!(std::fs::read_to_string(at.join("Deep/shared.mat")).unwrap(), "m");
    assert_eq!(job.linked.len(), 1);
    assert!(job.linked[0].contains("Shared"), "{:?}", job.linked);
    assert!(walkdir::WalkDir::new(&job.trial.path).into_iter().flatten().all(|e| !crate::owned::is_link(&std::fs::symlink_metadata(e.path()).unwrap())));
    // A link back to a folder it is in has no end: refused, nothing made.
    link(&e.source.join("Assets"), &e.source.join("Assets/Loop"));
    assert!(run_at(&e, &e.root.join("Projects"), "Loop", &|_| false, &ok).is_err());
    assert!(!e.root.join("Projects/Loop").exists());
}

#[test]
fn a_project_not_on_2022_or_with_an_unsupported_layout_is_blocked() {
    let e = env();
    let m = crate::unity6::matrix::Matrix::built_in();
    let of = |p: &Path| {
        let l = look(&e.app, p, &e.root.join("Projects"), "X", false, &AtomicBool::new(false));
        l.blockers
    };
    assert!(of(&e.source).is_empty());
    let write = |rel: &str, body: &str| std::fs::write(e.source.join(rel), body).unwrap();
    let manifest = std::fs::read_to_string(e.source.join("Packages/manifest.json")).unwrap();
    // Required files.
    for needed in payload::NEEDED {
        let kept = std::fs::read(e.source.join(needed)).unwrap();
        std::fs::remove_file(e.source.join(needed)).unwrap();
        assert!(!of(&e.source).is_empty(), "{needed} missing");
        assert!(run(&e).is_err());
        std::fs::write(e.source.join(needed), kept).unwrap();
    }
    // Settings that are not text, manifests and locks of the wrong shape.
    let settings = std::fs::read(e.source.join("ProjectSettings/ProjectSettings.asset")).unwrap();
    std::fs::write(e.source.join("ProjectSettings/ProjectSettings.asset"), [0xff, 0xfe, 0x80]).unwrap();
    assert!(matches!(&of(&e.source)[..], [Blocker::Unsupported(_)]));
    std::fs::write(e.source.join("ProjectSettings/ProjectSettings.asset"), settings).unwrap();
    for bad in ["[]", "{", r#"{"dependencies":[]}"#, r#"{"dependencies":{"a":1}}"#] {
        write("Packages/manifest.json", bad);
        assert!(of(&e.source).iter().any(|b| matches!(b, Blocker::Unsupported(_))), "{bad}");
    }
    write("Packages/manifest.json", &manifest);
    write("Packages/packages-lock.json", "[]");
    assert!(of(&e.source).iter().any(|b| matches!(b, Blocker::Unsupported(_))));
    write("Packages/packages-lock.json", r#"{"dependencies":{"com.x":{"version":"git","source":"git"}}}"#);
    assert!(of(&e.source).iter().any(|b| matches!(b, Blocker::OnlyInLibrary(n) if n == &["com.x"])));
    write("Packages/packages-lock.json", "{}");
    // A local package that is missing, one with local references of its own, one whose place is taken.
    write("Packages/manifest.json", r#"{"dependencies":{"com.gone":"file:../../../gone"}}"#);
    assert!(of(&e.source).iter().any(|b| matches!(b, Blocker::LocalMissing(n) if n == &["com.gone"])));
    let ext = external(&e, "A");
    std::fs::write(ext.join("package.json"), r#"{"name":"com.example.tool","version":"1.0.0","dependencies":{"b":"file:../b"}}"#).unwrap();
    assert!(of(&e.source).iter().any(|b| matches!(b, Blocker::Unsupported(n) if n == &["com.example.tool"])));
    // Second and third verifications: anything but an object with that name and a version.
    for bad in ["[]", "1", "{", r#"{"name":"com.example.tool"}"#, r#"{"name":"com.other","version":"1.0.0"}"#, r#"{"name":"com.example.tool","version":"one"}"#, r#"{"name":"com.example.tool","version":"1.0.0","dependencies":[]}"#] {
        std::fs::write(ext.join("package.json"), bad).unwrap();
        assert!(of(&e.source).iter().any(|b| matches!(b, Blocker::Unsupported(n) if n == &["com.example.tool"])), "{bad}");
        assert!(run(&e).is_err());
    }
    std::fs::write(ext.join("package.json"), r#"{"name":"com.example.tool","version":"1.0.0"}"#).unwrap();
    assert!(of(&e.source).is_empty());
    std::fs::create_dir_all(e.source.join("Packages/com.example.tool")).unwrap();
    std::fs::write(e.source.join("Packages/com.example.tool/package.json"), r#"{"name":"com.example.tool"}"#).unwrap();
    assert!(of(&e.source).iter().any(|b| matches!(b, Blocker::Unsupported(_))));
    std::fs::remove_dir_all(e.source.join("Packages/com.example.tool")).unwrap();
    // Not on 2022, open, itself a trial.
    let s = standing::read(&e.source, &m);
    assert!(blockers(&e.app, &e.source, &s, true, None).contains(&Blocker::Open));
    let job = run(&e).unwrap();
    assert!(of(&job.trial.path).iter().any(|b| matches!(b, Blocker::Trial(_))));
    vcc_migrates(&e.source);
    assert!(of(&e.source).iter().any(|b| matches!(b, Blocker::NotOn2022(_))));
}

#[test]
fn the_source_open_check_sees_unitys_lock() {
    let e = env();
    assert!(!is_open(&e.source));
    std::fs::create_dir_all(e.source.join("Temp")).unwrap();
    std::fs::write(e.source.join("Temp/UnityLockfile"), "").unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let _held = std::fs::OpenOptions::new().write(true).share_mode(0).open(e.source.join("Temp/UnityLockfile")).unwrap();
        assert!(is_open(&e.source));
        drop(_held);
        // Held for writing, shared with readers: still held.
        let _held = std::fs::OpenOptions::new().write(true).share_mode(7).open(e.source.join("Temp/UnityLockfile")).unwrap();
        assert!(is_open(&e.source));
    }
    #[cfg(windows)]
    assert!(!is_open(&e.source), "left behind by a crash");
}

#[test]
fn the_record_is_bound_to_identity_and_a_copied_marker_is_inert() {
    let e = env();
    let job = run(&e).unwrap();
    let moved = e.root.join("Projects/Moved");
    std::fs::rename(&job.trial.path, &moved).unwrap();
    assert_eq!(last(&e).1, Status::Missing);
    // Another folder at its path is not the trial, whatever it holds.
    copy_tree(&moved, &job.trial.path);
    assert_eq!(last(&e).1, Status::Replaced);
    assert!(adopt_vcc_copy(&e.app, &job.id, &moved).is_err());
    assert_eq!(authority::state(&e.app, &job.trial.path), TrialState::NotATrial, "the folder put there is not it");
    assert!(matches!(authority::state(&e.app, &moved), TrialState::WaitingForMigration(_)), "the trial is, wherever it is");
    // Not in the gallery as the trial, and taking it off does not touch what is there now.
    assert!(folders(&e.app).is_empty());
    abandon(&e.app, &job.id).unwrap();
    assert!(job.trial.path.join("Assets/Avatar.unity").is_file());
}

/// Second verification N2, fourth's decision A: taking a trial off changes Orbiters' record alone, whatever state it is in: nothing
/// of Creator Companion's, none of its folder, and its guard stays.
#[test]
fn taking_a_trial_off_changes_orbiters_record_alone() {
    // Waiting, and added in Creator Companion by the person.
    let e = env();
    their_store(&e);
    let job = run(&e).unwrap();
    crate::gallery::listed(&e.vcc);
    let before = store(&e);
    let left = abandon(&e.app, &job.id).unwrap();
    assert_eq!(left, Forgotten { at: job.trial.path.clone(), there: true });
    assert!(store(&e) == before && job.trial.path.join("Assets/Avatar.unity").is_file());
    assert!(matches!(authority::state(&e.app, &job.trial.path), TrialState::Abandoned { whole: true, .. }));
    assert!(authority::allow(&e.app, &job.trial.path, authority::Action::Packages).is_ok(), "a verified copy is an ordinary project now");
    assert!(crate::unity6::look(&e.app, &[]).trials.is_empty() && folders(&e.app).is_empty(), "not shown any more");
    // Failed.
    let trial = e.root.join("Projects/F");
    {
        let t = trial.clone();
        let _s = at_step(move |s| {
            if s == Step::Copied {
                std::fs::write(t.join("Assets/x"), "x").unwrap();
            }
            Ok(())
        });
        assert!(run_at(&e, &e.root.join("Projects"), "F", &|_| false, &ok).is_err());
    }
    let (job, _) = last(&e);
    abandon(&e.app, &job.id).unwrap();
    assert!(store(&e) == before && trial.join("Assets/Avatar.unity").is_file());
    let state = authority::state(&e.app, &trial);
    assert!(matches!(state, TrialState::Abandoned { whole: false, .. }));
    use authority::{Action, Caller};
    for action in [Action::Open(Caller::Window), Action::Packages, Action::Duplicate, Action::Storage, Action::List, Action::Unlist] {
        assert!(authority::allow(&e.app, &trial, action).is_err(), "{action:?} on a copy that was never verified");
    }
    // Not there (a disk taken out), another folder at its place: the same.
    let job = run_at(&e, &e.root.join("Projects"), "G", &|_| false, &ok).unwrap();
    std::fs::rename(&job.trial.path, e.root.join("away")).unwrap();
    assert_eq!(abandon(&e.app, &job.id).unwrap(), Forgotten { at: job.trial.path.clone(), there: false });
    assert!(store(&e) == before);
}

/// A stop removes exactly what the copy made; a file somebody else put there stays, and is named.
#[test]
fn a_stop_removes_only_what_the_copy_made() {
    for stranger in [false, true] {
        let e = env();
        let trial = e.root.join("Projects/T");
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let (c, t) = (cancel.clone(), trial.clone());
        let _s = at_step(move |s| {
            if s == Step::Copied {
                if stranger {
                    std::fs::write(t.join("Assets/theirs.txt"), "theirs").unwrap();
                }
                c.store(true, Ordering::Relaxed);
            }
            Ok(())
        });
        let r = prepare(
            Prepare { dir: &e.app, source: &e.source, parent: &e.root.join("Projects"), name: "T", target: target(), open: &|_| false, budget: &ok, recorded: &|_| {} },
            &cancel,
            &Default::default(),
        );
        assert!(r.is_err());
        let (job, s) = last(&e);
        assert_eq!(job.phase, Phase::Failed);
        if stranger {
            assert_eq!(s, Status::Failed { kept: true });
            assert_eq!(std::fs::read_to_string(trial.join("Assets/theirs.txt")).unwrap(), "theirs");
            assert!(!trial.join("ProjectSettings").exists(), "what the copy made is gone");
        } else {
            assert_eq!(s, Status::Failed { kept: false });
            assert!(!trial.exists());
        }
    }
}

#[test]
fn each_launch_is_reserved_verified_and_logged() {
    let e = env();
    let job = run(&e).unwrap();
    vcc_migrates(&job.trial.path);
    let after_vcc = guarded(&job.trial.path);
    let good = exact(&e.root, "6000.0.67f1", Some("78a1c2bbeb6a"));
    let launch_with = |found: Found, start: Start| launch(&e.app, &job.id, &move |_, _| found.clone(), &|_| false, &|_| false, start);
    let me = std::process::id();
    let started = |_: &Path, _: &Path, _: &Path| -> anyhow::Result<Option<u32>> { Ok(Some(me)) };
    // Not verified now, another build, none said, or none at all: refused before anything is reserved.
    let unverified = Candidate { check: Check::Unverified, identity: None, hint: Hint::Located, ..good.clone() };
    assert!(launch_with(Found::Unverified(unverified), &started).is_err());
    assert!(launch_with(Found::Exact(exact(&e.root, "6000.0.67f1", Some("61dfb374e36f"))), &started).is_err());
    assert!(launch_with(Found::Exact(exact(&e.root, "6000.0.67f1", None)), &started).is_err());
    assert!(launch_with(Found::Missing { wrong: vec![] }, &started).is_err());
    assert!(last(&e).0.attempts.is_empty());
    // The first one is reserved and bound to its process; a second while it runs is refused.
    let a1 = launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|_| true, &started).unwrap();
    assert_eq!(a1.process.as_ref().map(|p| p.pid), Some(me));
    assert!(alive(a1.process.as_ref().unwrap()) && !a1.reserved);
    assert!(a1.log.starts_with(e.app.join(LOGS).join(&job.id)));
    assert!(launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|p| p.pid == me, &started).is_err());
    // A reservation a crash left (no process recorded) blocks for a while, then not.
    journal::set(&e.app, &job.id, |j| {
        j.attempts[0].process = None;
        j.attempts[0].reserved = true;
    })
    .unwrap();
    assert!(launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|_| false, &started).is_err());
    journal::set(&e.app, &job.id, |j| j.attempts[0].started = 0).unwrap();
    let a2 = launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|_| false, &started).unwrap();
    assert_ne!(a1.log, a2.log);
    // Typed failures.
    let quit = |_: &Path, _: &Path, _: &Path| -> anyhow::Result<Option<u32>> { Err(anyhow::Error::new(crate::QuitAtOnce(Some("exit code: 3".into()), "quit".into()))) };
    assert!(launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|_| false, &quit).is_err());
    let spawn = |_: &Path, _: &Path, _: &Path| -> anyhow::Result<Option<u32>> { anyhow::bail!("no such program") };
    assert!(launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|_| false, &spawn).is_err());
    let j = last(&e).0;
    assert!(matches!(attempt_outcome(&j.attempts[2], false), Outcome::ExitedAtOnce(_)));
    assert!(matches!(attempt_outcome(&j.attempts[3], false), Outcome::DidNotStart(_)));
    // Kept: the last few, older logs removed with them.
    for _ in 0..KEEP_ATTEMPTS {
        launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|_| false, &|_, _, _| Ok(None)).unwrap();
    }
    assert_eq!(last(&e).0.attempts.len(), KEEP_ATTEMPTS);
    assert!(!a1.log.exists());
    assert_eq!(guarded(&job.trial.path), after_vcc, "no write by Orbiters on the trial's files");
}

/// Second verification M5: a number the system gave again is not the process that was started.
#[test]
fn an_attempt_runs_while_its_very_process_does() {
    let attempt = |process| Attempt { n: 1, started: 1, log: PathBuf::from("x"), editor: PathBuf::from("x"), unity: "6000.0.67f1".into(), start: None, process, reserved: false };
    let pid = std::process::id();
    let mine = started_at(pid).unwrap();
    assert!(attempt_running(&attempt(Some(Process { pid, started: mine })), false));
    assert!(!attempt_running(&attempt(Some(Process { pid, started: mine - 60 })), true), "another process with that number");
    assert!(attempt_running(&attempt(None), true) && !attempt_running(&attempt(None), false));
}

/// Second verification B4: what changed between the checks and the start stops the opening, the program included.
#[test]
fn what_changes_right_before_the_start_stops_it() {
    let e = env();
    let job = run(&e).unwrap();
    vcc_migrates(&job.trial.path);
    let good = exact(&e.root, "6000.0.67f1", Some("78a1c2bbeb6a"));
    let changed = |find: Find| {
        assert!(launch(&e.app, &job.id, find, &|_| false, &|_| false, &|_, _, _| panic!("started")).is_err());
        let j = last(&e).0;
        assert!(matches!(j.attempts.last().unwrap().start, Some(Outcome::DidNotStart(_))));
        assert!(!j.attempts.last().unwrap().reserved, "the reservation is given back");
    };
    // Another program by the time of the second look.
    let calls = std::cell::Cell::new(0);
    changed(&|_, _| {
        calls.set(calls.get() + 1);
        let mut c = good.clone();
        if calls.get() > 1 {
            c.identity = Some(Identity { version: "6000.0.67f1".into(), changeset: Some("78a1c2bbeb6a".into()), arch: vec![] });
        }
        Found::Exact(c)
    });
    // The program gone.
    let calls = std::cell::Cell::new(0);
    changed(&|_, _| {
        calls.set(calls.get() + 1);
        if calls.get() > 1 { Found::Missing { wrong: vec![] } } else { Found::Exact(good.clone()) }
    });
    // The project put back on 2022.
    changed(&|_, _| {
        std::fs::write(job.trial.path.join("ProjectSettings/ProjectVersion.txt"), U2022).unwrap();
        Found::Exact(good.clone())
    });
}

/// Second verification M7: every SDK package, not the base alone, is at the reviewed version, and the build is named.
#[test]
fn only_the_whole_reviewed_tuple_is_launched() {
    let e = env();
    let job = run(&e).unwrap();
    let ask = |editor: &str, locked: &[(&str, &str)], installed: &[(&str, &str, &str)]| {
        let _ = std::fs::remove_dir_all(job.trial.path.join("Packages/com.vrchat.base"));
        let _ = std::fs::remove_dir_all(job.trial.path.join("Packages/com.vrchat.avatars"));
        project(&job.trial.path, Some(editor), locked, installed);
        let asked = std::cell::Cell::new(false);
        let r = launch(&e.app, &job.id, &|_, _| { asked.set(true); Found::Missing { wrong: vec![] } }, &|_| false, &|_| false, &|_, _, _| panic!("started"));
        assert!(r.is_err());
        asked.get()
    };
    let beta = "3.11.0-beta.1";
    assert!(ask(U6, &[(SDK_BASE, beta), (SDK_AVATARS, beta)], &[(SDK_BASE, beta, "6000.0"), (SDK_AVATARS, beta, "6000.0")]), "the reviewed one is looked for");
    assert!(!ask(U6, &[(SDK_BASE, beta), (SDK_AVATARS, "3.10.6")], &[(SDK_BASE, beta, "6000.0"), (SDK_AVATARS, "3.10.6", "2022.3")]), "an old Avatars SDK");
    assert!(!ask(U6, &[(SDK_BASE, beta)], &[(SDK_BASE, beta, "6000.0"), (SDK_AVATARS, beta, "6000.0")]), "Avatars installed and not locked");
    assert!(!ask(U6, &[(SDK_BASE, "4.0.0"), (SDK_AVATARS, "4.0.0")], &[(SDK_BASE, "4.0.0", "6000.0"), (SDK_AVATARS, "4.0.0", "6000.0")]), "an unknown SDK");
    assert!(!ask("m_EditorVersion: 6000.0.67f1\n", &[(SDK_BASE, beta), (SDK_AVATARS, beta)], &[(SDK_BASE, beta, "6000.0"), (SDK_AVATARS, beta, "6000.0")]), "no build named");
    assert!(!ask("m_EditorVersion: 6000.0.99f1\nm_EditorVersionWithRevision: 6000.0.99f1 (78a1c2bbeb6a)\n", &[(SDK_BASE, beta), (SDK_AVATARS, beta)], &[(SDK_BASE, beta, "6000.0"), (SDK_AVATARS, beta, "6000.0")]), "another editor");
}

/// Second verification N6: an opening needs room for what the first import may still take, however many were tried.
#[test]
fn every_opening_before_a_finished_import_needs_its_room() {
    let e = env();
    let job = run_at(&e, &e.root.join("Projects"), "T", &|_| false, &|_, _| Ok(1000)).unwrap();
    vcc_migrates(&job.trial.path);
    let good = exact(&e.root, "6000.0.67f1", Some("78a1c2bbeb6a"));
    let go = |room: u64| {
        FREE.with(|f| f.set(Some(room)));
        let r = launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|_| false, &|_, _, _| Ok(None));
        FREE.with(|f| f.set(None));
        r
    };
    assert!(go(999).is_err());
    let a = go(1000).unwrap();
    assert!(go(999).is_err(), "a second opening too");
    // What its Library holds already is not needed again.
    std::fs::create_dir_all(job.trial.path.join("Library")).unwrap();
    std::fs::write(job.trial.path.join("Library/ArtifactDB"), vec![0; 400]).unwrap();
    assert!(go(599).is_err() && go(600).is_ok());
    // Nothing once an opening's own log says it finished, while its Library has what it imported.
    std::fs::write(&a.log, "Asset Pipeline Refresh (id=ab12): Total: 3.5 seconds - Initiated by InitialRefreshV2(ForceSynchronousImport)\n").unwrap();
    assert!(go(599).is_err(), "half a Library");
    std::fs::write(job.trial.path.join("Library/SourceAssetDB"), "x").unwrap();
    assert!(go(0).is_ok());
    // Third verification: the Library cleared since is imported again.
    std::fs::remove_dir_all(job.trial.path.join("Library")).unwrap();
    assert!(go(999).is_err() && go(1000).is_ok());
}

/// Second verification M1: only the same project, by its files, made since, and named by the person, is taken as Creator
/// Companion's migrated copy.
#[test]
fn only_the_same_project_is_taken_as_the_migrated_copy() {
    let e = env();
    let job = run_at(&e, &e.root.join("Projects"), "T", &|_| false, &ok).unwrap();
    let beside = e.root.join("Projects/T-Migrated");
    // An unrelated project of that name.
    std::fs::create_dir_all(&beside).unwrap();
    vcc_migrates(&beside);
    assert_eq!(last(&e).1, Status::Waiting { vcc_copy: None });
    assert!(adopt_vcc_copy(&e.app, &job.id, &beside).is_err());
    // The same file names with other GUIDs: another project's.
    copy_tree(&job.trial.path, &beside);
    vcc_migrates(&beside);
    std::fs::write(beside.join("Assets/Avatar.unity.meta"), "guid: ffffffffffffffffffffffffffffffff\n").unwrap();
    assert_eq!(last(&e).1, Status::Waiting { vcc_copy: None });
    // Third verification: one contradicting GUID among many is another project, however much is alike.
    for i in 0..30 {
        std::fs::write(job.trial.path.join(format!("Assets/{i}.txt.meta")), format!("guid: {i:032x}\n")).unwrap();
        std::fs::write(beside.join(format!("Assets/{i}.txt.meta")), format!("guid: {i:032x}\n")).unwrap();
    }
    assert!(adopt_vcc_copy(&e.app, &job.id, &beside).is_err());
    for i in 0..30 {
        std::fs::remove_file(job.trial.path.join(format!("Assets/{i}.txt.meta"))).unwrap();
    }
    // The same files, and another product: a project made from the same template.
    std::fs::remove_dir_all(&beside).unwrap();
    copy_tree(&job.trial.path, &beside);
    vcc_migrates(&beside);
    let settings = std::fs::read_to_string(beside.join("ProjectSettings/ProjectSettings.asset")).unwrap();
    std::fs::write(beside.join("ProjectSettings/ProjectSettings.asset"), settings.replace(&job.guid, "ffffffffffffffffffffffffffffffff")).unwrap();
    assert_eq!(last(&e).1, Status::Waiting { vcc_copy: None });
    assert!(format!("{:#}", adopt_vcc_copy(&e.app, &job.id, &beside).unwrap_err()).contains("GUID"));
    // The very copy: offered, and taken.
    std::fs::remove_dir_all(&beside).unwrap();
    copy_tree(&job.trial.path, &beside);
    assert!(adopt_vcc_copy(&e.app, &job.id, &beside).is_err(), "not migrated yet");
    vcc_migrates(&beside);
    assert_eq!(last(&e).1, Status::Waiting { vcc_copy: Some(beside.clone()) });
    // The trial itself or its original is not it.
    assert!(adopt_vcc_copy(&e.app, &job.id, &job.trial.path).is_err());
    let job = adopt_vcc_copy(&e.app, &job.id, &beside).unwrap();
    assert_eq!(job.unity6().path, beside);
    assert_eq!(folders(&e.app), [job.trial.path.clone(), beside.clone()]);
    assert_eq!(last(&e).1, Status::OnUnity6);
    assert!(matches!(authority::state(&e.app, &job.trial.path), TrialState::Superseded(_)), "the first copy keeps its guard");
    assert!(matches!(authority::state(&e.app, &beside), TrialState::Migrated(_)));
    assert!(adopt_vcc_copy(&e.app, &job.id, &beside).is_err(), "once");
}

/// A folder that was beside the trial when it was handed over is never offered, and none is when they could not all be read.
#[test]
fn a_folder_that_was_there_before_is_never_offered() {
    let e = env();
    let beside = e.root.join("Projects/T-Migrated");
    copy_tree(&e.source, &beside);
    let job = run_at(&e, &e.root.join("Projects"), "T", &|_| false, &ok).unwrap();
    vcc_migrates(&beside);
    assert_eq!(last(&e).1, Status::Waiting { vcc_copy: None });
    std::fs::remove_dir_all(&beside).unwrap();
    copy_tree(&job.trial.path, &beside);
    vcc_migrates(&beside);
    assert!(matches!(last(&e).1, Status::Waiting { vcc_copy: Some(_) }));
    journal::set(&e.app, &job.id, |j| j.beside = None).unwrap();
    assert_eq!(last(&e).1, Status::Waiting { vcc_copy: None });
}

#[test]
fn a_trial_is_paired_only_with_its_very_original_wherever_it_is() {
    let e = env();
    run(&e).unwrap();
    let pairs = |projects: &[PathBuf]| crate::unity6::look(&e.app, projects).pairs;
    assert_eq!(pairs(std::slice::from_ref(&e.source)).len(), 1);
    let moved = e.root.join("Projects/Old");
    std::fs::rename(&e.source, &moved).unwrap();
    source(&e.root);
    assert!(pairs(std::slice::from_ref(&e.source)).is_empty());
    assert_eq!(pairs(&[e.source.clone(), moved.clone()]).iter().map(|p| p.original.clone()).collect::<Vec<_>>(), [moved]);
}

/// The sheet's check refuses the places and layouts the copy refuses, before Prepare (second verification N8).
#[test]
fn the_sheets_look_refuses_what_preparing_refuses() {
    let e = env();
    let look_at = |parent: &Path, name: &str| look(&e.app, &e.source, parent, name, false, &AtomicBool::new(false));
    assert_eq!(look_at(&e.root.join("Projects"), "T").place.unwrap(), e.root.join("Projects").join("T"));
    for (parent, name) in [
        (e.source.join("Assets"), "T"),
        (e.source.join("Assets/New/Deeper"), "T"),
        (e.root.join("Projects"), "My avatar"),
        (e.root.join("Nowhere"), "T"),
        (e.root.join("Projects"), "bad:name"),
    ] {
        assert!(look_at(&parent, name).place.is_err(), "{}/{name}", parent.display());
    }
    assert!(look_at(&e.root.join("Projects"), "T").bytes > 0);
}

/// Two windows never work on one trial at once, nor does one take up what another is making.
#[test]
fn one_lease_at_a_time_on_a_trial() {
    let e = env();
    let job = run(&e).unwrap();
    let lease = Lease::take(&e.app, &job.id).unwrap();
    assert_eq!(last(&e).1, Status::Running);
    assert!(again(&e, &job.id).is_err() && abandon(&e.app, &job.id).is_err());
    assert!(adopt_vcc_copy(&e.app, &job.id, &e.root).is_err());
    drop(lease);
    abandon(&e.app, &job.id).unwrap();
}

/// Fourth verification, decision A: a trial's whole life writes no byte of Creator Companion's or ALCOM's, whatever is there (nothing,
/// their store with projects, one that cannot be read, one in use), whatever fails and is cut short.
#[test]
fn a_trials_whole_life_writes_nothing_of_creator_companion() {
    for store_is in ["none", "theirs", "a database alone", "unreadable", "in use"] {
        let e = env();
        match store_is {
            "theirs" | "in use" => their_store(&e),
            "a database alone" => {
                their_store(&e);
                std::fs::write(e.vcc.join("settings.json"), r#"{"showPrereleasePackages":true}"#).unwrap();
            }
            "unreadable" => {
                std::fs::create_dir_all(&e.vcc).unwrap();
                std::fs::write(e.vcc.join("settings.json"), "{").unwrap();
                std::fs::write(e.vcc.join("vcc.liteDb"), "x").unwrap();
            }
            _ => {}
        }
        let before = store(&e);
        let _held = (store_is == "in use").then(|| kept_from_all(&e.vcc.join("settings.json")));
        let same = |what: &str| {
            let names: Vec<_> = walkdir::WalkDir::new(&e.vcc).into_iter().flatten().filter(|f| f.file_type().is_file()).map(|f| f.path().to_path_buf()).collect();
            assert_eq!(names.len(), before.len(), "{store_is}: {what}: {names:?}");
            if store_is != "in use" {
                assert!(store(&e) == before, "{store_is}: {what}");
            }
        };
        for step in [Step::Recorded, Step::Made, Step::Folders, Step::Copied, Step::Compared, Step::Verified, Step::MarkerGone, Step::Checked] {
            let _s = at_step(move |s| if s == step { Err(anyhow::anyhow!("made to fail")) } else { Ok(()) });
            assert!(run_at(&e, &e.root.join("Projects"), &format!("{step:?}"), &|_| false, &ok).is_err());
            same(&format!("{step:?}"));
            let (job, s) = last(&e);
            if s == Status::Unfinished {
                drop(_s);
                again(&e, &job.id).unwrap();
                same("finished");
            }
            abandon(&e.app, &job.id).unwrap();
            same("taken off");
        }
        let job = run_at(&e, &e.root.join("Projects"), "T", &|_| false, &ok).unwrap();
        let beside = e.root.join("Projects/T-Migrated");
        copy_tree(&job.trial.path, &beside);
        vcc_migrates(&beside);
        adopt_vcc_copy(&e.app, &job.id, &beside).unwrap();
        same("taken");
        let good = exact(&e.root, "6000.0.67f1", Some("78a1c2bbeb6a"));
        launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|_| false, &|_, _, _| Ok(None)).unwrap();
        let _ = crate::unity6::look(&e.app, &[e.source.clone(), beside.clone()]);
        same("opened, looked at");
        // The gallery's, the bridge's and the menus' own Add and Remove, on any folder of it.
        for at in [&job.trial.path, &beside] {
            for add in [true, false] {
                assert!(authority::allow(&e.app, at, if add { authority::Action::List } else { authority::Action::Unlist }).is_err());
            }
        }
        same("asked");
        abandon(&e.app, &job.id).unwrap();
        same("taken off");
        assert!(!e.vcc.exists() == (store_is == "none"));
    }
}

/// Third verification R2: nothing of a trial's own is written by anything but its copy: the manifest vrc-get's own loader rewrites
/// included; and a plan of changes is only looked at.
#[test]
fn no_byte_of_the_trial_changes() {
    let e = env();
    let p = e.source.join("Packages/vpm-manifest.json");
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    v["note"] = "3.2-3.8".into();
    std::fs::write(&p, v.to_string()).unwrap();
    let before = std::fs::read(&p).unwrap();
    let job = run(&e).unwrap();
    assert_eq!(std::fs::read(job.trial.path.join("Packages/vpm-manifest.json")).unwrap(), before);
    let rt = crate::runtime().unwrap();
    assert!(rt.block_on(crate::coherent::solve(&e.vcc, &e.source, &[], &[], false)).is_err(), "VPM's loader would write");
    assert!(rt.block_on(crate::coherent::solve(&e.vcc, &job.trial.path, &[], &[], false)).is_err(), "a waiting trial");
    assert_eq!(std::fs::read(&p).unwrap(), before);
}

/// What changed between verifying and the end is found, told, and the copy is a failed one.
#[test]
fn a_trial_changed_before_it_is_handed_over_is_a_failed_copy() {
    for step in [Step::Verified, Step::MarkerGone] {
        let e = env();
        let trial = e.root.join("Projects/T");
        let t = trial.clone();
        let _s = at_step(move |s| {
            if s == step {
                std::fs::write(t.join("Packages/vpm-manifest.json"), "{}").unwrap();
            }
            Ok(())
        });
        let err = format!("{:#}", run_at(&e, &e.root.join("Projects"), "T", &|_| false, &ok).unwrap_err());
        assert!(err.contains("vpm-manifest.json"), "{err}");
        let (job, s) = last(&e);
        assert_eq!((job.phase, s), (Phase::Failed, Status::Failed { kept: true }), "{step:?}");
        assert!(job.differences.iter().any(|d| d.contains("vpm-manifest.json")));
        assert!(authority::allow(&e.app, &trial, authority::Action::Packages).is_err(), "{step:?}");
    }
}

/// Third verification R6: taking the migrated copy is one line of the record: done, or not done and done again.
#[test]
fn taking_the_migrated_copy_is_done_or_done_again() {
    let e = env();
    let job = run_at(&e, &e.root.join("Projects"), "T", &|_| false, &ok).unwrap();
    let beside = e.root.join("Projects/T-Migrated");
    copy_tree(&job.trial.path, &beside);
    vcc_migrates(&beside);
    // The record cannot be written: nothing is taken, and it is still offered.
    {
        let lease = Lease::take(&e.app, &job.id).unwrap();
        assert!(adopt_vcc_copy(&e.app, &job.id, &beside).is_err());
        drop(lease);
        assert!(last(&e).0.migrated.is_none());
        assert_eq!(last(&e).1, Status::Waiting { vcc_copy: Some(beside.clone()) });
    }
    let done = adopt_vcc_copy(&e.app, &job.id, &beside).unwrap();
    assert!(done.unity6().path == beside);
    assert!(matches!(authority::state(&e.app, &job.trial.path), TrialState::Superseded(_)));
    assert!(adopt_vcc_copy(&e.app, &job.id, &beside).is_err(), "once");
}

/// Third verification R5: Forget asks the record and the lease first, and takes no protection off: an altered copy never becomes an
/// ordinary project, a superseded one stays superseded, one on Unity 6 stays a trial.
#[test]
fn forgetting_checks_first_and_keeps_every_protection() {
    use authority::{allow_on, Action, Caller};
    let e = env();
    // Part of the record cannot be read: nothing is changed.
    let job = run_at(&e, &e.root.join("Projects"), "T", &|_| false, &ok).unwrap();
    let journal_file = e.app.join(JOURNAL);
    let whole = std::fs::read(&journal_file).unwrap();
    let torn = [&whole[..], b"{torn"].concat();
    std::fs::write(&journal_file, &torn).unwrap();
    assert!(abandon(&e.app, &job.id).is_err());
    assert_eq!(std::fs::read(&journal_file).unwrap(), torn);
    std::fs::write(&journal_file, &whole).unwrap();
    // Verified, not finished, changed since: never an ordinary project.
    for changed in [false, true] {
        let name = if changed { "Changed" } else { "Same" };
        {
            let _s = at_step(|s| if s == Step::MarkerGone { Err(anyhow::anyhow!("cut short")) } else { Ok(()) });
            assert!(run_at(&e, &e.root.join("Projects"), name, &|_| false, &ok).is_err());
        }
        let (job, s) = last(&e);
        assert_eq!(s, Status::Unfinished);
        if changed {
            std::fs::write(job.trial.path.join("Assets/new.txt"), "x").unwrap();
        }
        abandon(&e.app, &job.id).unwrap();
        assert_eq!(last(&e).0.phase, Phase::Abandoned { whole: !changed });
        assert_eq!(authority::allow(&e.app, &job.trial.path, Action::Packages).is_ok(), !changed);
        assert_eq!(authority::allow(&e.app, &job.trial.path, Action::List).is_ok(), !changed, "the gallery's own Add, once it is an ordinary project");
    }
    // An adopted pair: the first copy stays superseded, the migrated one a trial on Unity 6.
    let job = run_at(&e, &e.root.join("Projects"), "P", &|_| false, &ok).unwrap();
    let beside = e.root.join("Projects/P-Migrated");
    copy_tree(&job.trial.path, &beside);
    vcc_migrates(&beside);
    adopt_vcc_copy(&e.app, &job.id, &beside).unwrap();
    abandon(&e.app, &job.id).unwrap();
    assert!(matches!(authority::state(&e.app, &job.trial.path), TrialState::Superseded(_)));
    assert!(matches!(authority::state(&e.app, &beside), TrialState::Migrated(_)));
    for action in [Action::Open(Caller::Window), Action::Packages, Action::List] {
        assert!(allow_on(&e.app, &job.trial.path, action, false).is_err(), "{action:?}");
    }
    let open = Action::Open(Caller::Window);
    assert!(matches!(allow_on(&e.app, &beside, open, false), Ok(authority::Way::Trial(_))), "its own launcher, still");
    assert!(allow_on(&e.app, &beside, open, true).is_err(), "and not on the Mac");
    assert!(authority::upload(&e.app, &beside).is_err());
    // Its launcher still opens it, reviewed and logged.
    let good = exact(&e.root, "6000.0.67f1", Some("78a1c2bbeb6a"));
    launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|_| false, &|_, _, _| Ok(None)).unwrap();
    // Forgotten on 2022, migrated afterwards by the person: a trial again by what it is.
    let job = run_at(&e, &e.root.join("Projects"), "L", &|_| false, &ok).unwrap();
    abandon(&e.app, &job.id).unwrap();
    assert!(matches!(authority::state(&e.app, &job.trial.path), TrialState::Abandoned { whole: true, .. }));
    vcc_migrates(&job.trial.path);
    assert!(matches!(authority::state(&e.app, &job.trial.path), TrialState::Migrated(_)));
    assert!(allow_on(&e.app, &job.trial.path, open, true).is_err());
}

/// Third verification R8: where a folder cannot be held, every step that makes or changes a trial refuses at its entry, whatever
/// the window shows.
#[test]
fn where_trials_are_not_available_every_step_refuses() {
    let e = env();
    let job = run(&e).unwrap();
    let record = std::fs::read(e.app.join(JOURNAL)).unwrap();
    AVAILABLE.with(|a| a.set(Some(false)));
    assert!(run_at(&e, &e.root.join("Projects"), "N", &|_| false, &ok).is_err());
    assert!(again(&e, &job.id).is_err());
    assert!(adopt_vcc_copy(&e.app, &job.id, &e.root).is_err());
    assert!(abandon(&e.app, &job.id).is_err());
    let none = |_: &str, _: &str| Found::Missing { wrong: vec![] };
    assert!(launch(&e.app, &job.id, &none, &|_| false, &|_| false, &|_, _, _| panic!("started")).is_err());
    AVAILABLE.with(|a| a.set(None));
    assert_eq!(available(), cfg!(windows));
    assert_eq!(std::fs::read(e.app.join(JOURNAL)).unwrap(), record);
    assert!(!e.root.join("Projects/N").exists());
}

/// Third verification M5: a run that ended between starting the editor and recording it leaves an attempt that is bound to
/// the editor once it is seen, however late.
#[test]
fn an_interrupted_start_is_reconciled_with_its_editor() {
    let e = env();
    let job = run(&e).unwrap();
    vcc_migrates(&job.trial.path);
    let good = exact(&e.root, "6000.0.67f1", Some("78a1c2bbeb6a"));
    let a = launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|_| false, &|_, _, _| Ok(None)).unwrap();
    journal::set(&e.app, &job.id, |j| {
        j.attempts[0].reserved = true;
        // Long past what a reservation alone blocks.
        j.attempts[0].started = 0;
    })
    .unwrap();
    let editor = Process { pid: 7, started: 9 };
    let (log, seen) = (a.log.clone(), editor.clone());
    let of_log = move |l: &Path| (l == log).then(|| seen.clone());
    let go = |alive: &dyn Fn(&Process) -> bool| {
        super::launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &Processes { alive, of_log: &of_log }, &|_, _, _| Ok(None))
    };
    assert!(go(&|p| p.pid == 7).is_err(), "it runs: no second one");
    let now = &last(&e).0.attempts[0];
    assert!(now.process.as_ref() == Some(&editor) && !now.reserved);
    assert!(go(&|_| false).is_ok(), "it ended");
    // The real question: a process whose command line names a log (here, this one's has none).
    assert!(of_log_none(&e.app));
}

/// A `.tgz` of `members`.
pub(crate) fn archive(at: &Path, members: &[(&str, String)]) {
    use std::io::Write;
    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
    let mut tar = vec![];
    let mut put = |name: &str, kind: u8, body: &[u8]| {
        let mut head = [0u8; 512];
        head[..name.len().min(100)].copy_from_slice(&name.as_bytes()[..name.len().min(100)]);
        head[124..135].copy_from_slice(format!("{:011o}", body.len()).as_bytes());
        head[156] = kind;
        head[257..262].copy_from_slice(b"ustar");
        head[148..156].copy_from_slice(b"        ");
        let sum: u32 = head.iter().map(|b| u32::from(*b)).sum();
        head[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
        tar.extend(head);
        tar.extend(body);
        tar.resize(tar.len().div_ceil(512) * 512, 0);
    };
    for (i, (name, body)) in members.iter().enumerate() {
        if name.len() > 100 {
            // Both ways of a long name.
            if i % 2 == 0 {
                put("././@LongLink", b'L', format!("{name}\0").as_bytes());
            } else {
                let record = format!(" path={name}\n");
                let n = record.len() + 3;
                put("pax", b'x', format!("{n}{record}").as_bytes());
            }
        }
        put(name, b'0', body.as_bytes());
    }
    tar.extend([0u8; 1024]);
    let mut gz = flate2::write::GzEncoder::new(std::fs::File::create(at).unwrap(), flate2::Compression::fast());
    gz.write_all(&tar).unwrap();
    gz.finish().unwrap();
}

fn of_log_none(app: &Path) -> bool {
    super::of_log(&app.join("no such log")).is_none()
}

/// Third verification, ruling (a): app data or VCC's folder that the original can see is refused before anything is written.
#[test]
fn nothing_is_kept_where_the_original_can_see_it() {
    for (inside, at) in [("app", "Assets/Orbiters"), ("vcc", "Assets/Vcc"), ("app/unity6-logs", "Assets/Logs")] {
        // (VCC's folder: a duplicate is added to its list; a trial writes nothing there.)
        let e = env();
        let target = e.root.join(inside);
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("theirs.txt"), "theirs").unwrap();
        link(&target, &e.source.join(at));
        let before: Vec<_> = walkdir::WalkDir::new(&e.root).into_iter().flatten().map(|f| f.path().to_path_buf()).collect();
        if inside != "vcc" {
            let err = format!("{:#}", run(&e).unwrap_err());
            assert!(err.contains("keeps its own files"), "{inside}: {err}");
            assert!(super::look(&e.app, &[&e.app], &e.source, &e.root.join("Projects"), "T", false, &AtomicBool::new(false)).place.is_err());
            let after: Vec<_> = walkdir::WalkDir::new(&e.root).into_iter().flatten().map(|f| f.path().to_path_buf()).collect();
            assert_eq!(before, after, "{inside}");
        }
        // Nor a duplicate of it.
        if inside == "vcc" {
            let rt = crate::runtime().unwrap();
            assert!(rt.block_on(crate::templates::duplicate(&e.vcc, &e.source, &e.root.join("Projects"), "D", Default::default(), &AtomicBool::new(false), &Default::default())).is_err());
            assert_eq!(std::fs::read_dir(&e.vcc).unwrap().count(), 1);
        }
        unlink(&e.source.join(at));
    }
}

/// Third verification R3: what a trial was uploaded as, from everything in it: a package from outside, an embedded one, nested
/// prefab variants, what an edit made before the copy began added; and what cannot be read as Unity's.
#[test]
fn the_content_of_everything_that_ends_up_in_the_trial_is_known() {
    let e = env();
    let id = |n: u32| format!("avtr_{n:08}-2222-4333-8444-555555555555");
    let prefab = |n: u32| scene(&id(n));
    let variant = |n: u32| format!("%YAML 1.1\n--- !u!1001 &1\nPrefabInstance:\n  m_Modification:\n    m_Modifications:\n    - target: {{fileID: 1, guid: 2, type: 3}}\n      propertyPath: blueprintId\n      value: {}\n      objectReference: {{fileID: 0}}\n", id(n));
    let ext = external(&e, "x");
    std::fs::create_dir_all(ext.join("Deep")).unwrap();
    std::fs::write(ext.join("Deep/Outside.prefab"), prefab(1)).unwrap();
    // A folder the package from outside links to.
    let shared = e.root.join("Shared");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(shared.join("Linked.asset"), prefab(2)).unwrap();
    link(&shared, &ext.join("Shared"));
    std::fs::create_dir_all(e.source.join("Packages/com.embedded")).unwrap();
    std::fs::write(e.source.join("Packages/com.embedded/package.json"), r#"{"name":"com.embedded","version":"1.0.0"}"#).unwrap();
    std::fs::write(e.source.join("Packages/com.embedded/Variant.prefab"), variant(3)).unwrap();
    std::fs::write(e.source.join("Assets/Variant of variant.prefab"), variant(4)).unwrap();
    std::fs::write(e.source.join("Assets/Broken.asset"), "version https://git-lfs.github.com/spec/v1\n").unwrap();
    let src = e.source.clone();
    // Fourth verification: a package archive, one as npm packs it and one with a long name; a package named like VRChat's.
    let tgz = e.root.join("External/com.arc.tgz");
    archive(&tgz, &[("package/package.json", r#"{"name":"com.arc","version":"1.0.0"}"#.into()), ("package/Deep/A.prefab", prefab(6)), (&format!("package/{}/B.unity", "long".repeat(40)), prefab(7))]);
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(e.source.join("Packages/manifest.json")).unwrap()).unwrap();
    let mut manifest = manifest;
    manifest["dependencies"]["com.arc"] = format!("file:{}", slash(&tgz)).into();
    std::fs::write(e.source.join("Packages/manifest.json"), manifest.to_string()).unwrap();
    std::fs::create_dir_all(e.source.join("Packages/com.vrchat.personal")).unwrap();
    std::fs::write(e.source.join("Packages/com.vrchat.personal/package.json"), r#"{"name":"com.vrchat.personal","version":"1.0.0"}"#).unwrap();
    std::fs::write(e.source.join("Packages/com.vrchat.personal/A.prefab"), prefab(8)).unwrap();
    let late = prefab(5);
    let s = at_step(move |s| {
        if s == Step::Recorded {
            std::fs::write(src.join("Assets/Avatar.unity"), &late).unwrap();
        }
        Ok(())
    });
    let job = run(&e).unwrap();
    drop(s);
    for n in 1..=8 {
        assert!(job.content.contains(&id(n)), "{n}: {:?}", job.content);
    }
    assert_eq!(job.content_unsure, [Path::new("Assets").join("Broken.asset").display().to_string()]);
    // Each is told of wherever it is: in a duplicate, a restored copy, a project that has the same package from outside.
    let cancel = AtomicBool::new(false);
    let rt = crate::runtime().unwrap();
    let copy = rt.block_on(crate::templates::duplicate(&e.vcc, &e.source, &e.root.join("Projects"), "Copy", Default::default(), &cancel, &Default::default())).unwrap();
    let all = payload::content_of(&copy, &cancel).unwrap();
    assert!((1..=8).all(|n| all.ids.contains(&id(n))), "{all:?}");
    assert_eq!(all.unsure.len(), 1);
    let same_package = e.root.join("Projects/Other");
    project(&same_package, Some(U6), &[], &[]);
    std::fs::create_dir_all(same_package.join("Packages")).unwrap();
    std::fs::write(same_package.join("Packages/manifest.json"), std::fs::read(e.source.join("Packages/manifest.json")).unwrap()).unwrap();
    assert!(authority::upload(&e.app, &same_package).unwrap_err().contains(&id(1)), "its package from outside has the content");
    // Each alone is enough: the archive, the package named like VRChat's.
    for n in [6, 7, 8] {
        let one = e.root.join(format!("Projects/One {n}"));
        project(&one, Some(U6), &[], &[]);
        if n == 8 {
            std::fs::create_dir_all(one.join("Packages/com.vrchat.personal")).unwrap();
            std::fs::write(one.join("Packages/com.vrchat.personal/A.prefab"), prefab(8)).unwrap();
        } else {
            std::fs::create_dir_all(one.join("Packages")).unwrap();
            std::fs::write(one.join("Packages/manifest.json"), format!(r#"{{"dependencies":{{"com.arc":"file:{}"}}}}"#, slash(&tgz))).unwrap();
        }
        assert!(authority::upload(&e.app, &one).unwrap_err().contains(&id(n)), "{n}");
    }
    // An archive that cannot be opened, cut short, or of something else.
    let broken = e.root.join("Projects/Broken");
    project(&broken, Some(U2022), &[], &[]);
    std::fs::create_dir_all(broken.join("Packages")).unwrap();
    std::fs::write(broken.join("Packages/manifest.json"), r#"{"dependencies":{"com.bad":"file:bad.tgz"}}"#).unwrap();
    let whole = std::fs::read(&tgz).unwrap();
    for bad in [b"not an archive".to_vec(), vec![], whole[..whole.len() / 2].to_vec(), whole[..whole.len() - 9].to_vec()] {
        std::fs::write(broken.join("Packages/bad.tgz"), &bad).unwrap();
        // (What could be read of it before its end counts as what it is.)
        let warned = authority::upload(&e.app, &broken).unwrap().warning.unwrap();
        assert!(warned.contains("bad.tgz") || warned.contains(&id(6)), "{} bytes: {warned}", bad.len());
    }
    // Nothing in common, and a file of it or of the trial that cannot be read: it may be.
    let apart = e.root.join("Projects/Apart");
    project(&apart, Some(U2022), &[], &[]);
    std::fs::create_dir_all(apart.join("Assets")).unwrap();
    std::fs::create_dir_all(apart.join("Packages")).unwrap();
    assert!(authority::upload(&e.app, &apart).unwrap().warning.unwrap().contains("Broken.asset"), "of the trial");
    journal::set(&e.app, &job.id, |j| j.content_unsure.clear()).unwrap();
    assert!(authority::upload(&e.app, &apart).unwrap().warning.is_none());
    std::fs::write(apart.join("Assets/Odd.prefab"), "not Unity's").unwrap();
    assert!(authority::upload(&e.app, &apart).unwrap().warning.unwrap().contains("Odd.prefab"), "of the project");
    // A package of it that is not there.
    std::fs::remove_file(apart.join("Assets/Odd.prefab")).unwrap();
    std::fs::write(apart.join("Packages/manifest.json"), r#"{"dependencies":{"com.gone":"file:../../../gone"}}"#).unwrap();
    assert!(authority::upload(&e.app, &apart).unwrap().warning.unwrap().contains("com.gone"));
    // One only its Library holds: read there, or not known.
    std::fs::write(apart.join("Packages/manifest.json"), "{}").unwrap();
    std::fs::write(apart.join("Packages/packages-lock.json"), r#"{"dependencies":{"com.git":{"version":"x","source":"git"}}}"#).unwrap();
    assert!(authority::upload(&e.app, &apart).unwrap().warning.unwrap().contains("com.git"));
    std::fs::create_dir_all(apart.join("Library/PackageCache/com.git@12ab")).unwrap();
    std::fs::write(apart.join("Library/PackageCache/com.git@12ab/A.prefab"), prefab(3)).unwrap();
    assert!(authority::upload(&e.app, &apart).unwrap().warning.unwrap().contains(&id(3)));
}
