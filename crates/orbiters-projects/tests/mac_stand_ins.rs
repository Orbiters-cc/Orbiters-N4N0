//! The Mac's whole way through a project with stand-ins for Unity, Unity Hub and Blender: real application bundles started
//! by the system, a real second disk (a disk image), in a home folder of their own. Needs a logged-in session, `swiftc` and
//! `hdiutil`, so it does not run by itself:
//! `cargo test -p orbiters-projects --test mac_stand_ins -- --ignored --nocapture --test-threads=1`
#![cfg(target_os = "macos")]

use orbiters_projects::discovery::{self, Project};
use orbiters_projects::editor::{self, Event, Session};
use orbiters_projects::{blender, size, storage};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

const VERSION: &str = "2022.3.22f1";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// An application bundle from one of the Swift sources beside these tests, signed as the system wants any program to be.
fn bundle(source: &str, app: &Path, program: &str, id: &str) {
    let macos = app.join("Contents/MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mac").join(source);
    let built = Command::new("swiftc").arg("-O").arg(&source).arg("-o").arg(macos.join(program)).status().expect("swiftc");
    assert!(built.success());
    write(
        &app.join("Contents/Info.plist"),
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleExecutable</key><string>{program}</string><key>CFBundleIdentifier</key><string>{id}</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleVersion</key><string>{VERSION}</string></dict></plist>"#
        ),
    );
    assert!(Command::new("codesign").args(["-s", "-", "--force"]).arg(app).stderr(std::process::Stdio::null()).status().unwrap().success());
}

fn make_project(path: &Path, version: &str, sdk: &str) -> Project {
    write(&path.join("ProjectSettings/ProjectVersion.txt"), &format!("m_EditorVersion: {version}\n"));
    write(&path.join("Packages/vpm-manifest.json"), &format!(r#"{{"dependencies":{{"{sdk}":{{"version":"3.7.0"}}}},"locked":{{"{sdk}":{{"version":"3.7.0"}}}}}}"#));
    write(&path.join("Packages/manifest.json"), r#"{"dependencies":{}}"#);
    write(&path.join("Assets/Scenes/Main.unity"), "%YAML 1.1\n");
    for i in 0..8 {
        let bytes: Vec<u8> = (0..300_000u32).map(|b| (b.wrapping_mul(i + 3) >> 2) as u8).collect();
        std::fs::create_dir_all(path.join("Library/Artifacts/ab")).unwrap();
        std::fs::write(path.join(format!("Library/Artifacts/ab/{i}.bin")), bytes).unwrap();
    }
    discovery::describe(path.to_path_buf(), false, &mut Default::default())
}

fn wait_for(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < limit, "{what}: not in {limit:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The project's editor as the panel sees it: (process, asking).
fn seen(project: &Path) -> Option<(u32, bool)> {
    editor::of(&editor::running(), project, false).map(|e| (e.pid, e.asking))
}

/// Whatever is still running from this test's folder, ended however the test ends.
struct StandIns(PathBuf);
impl Drop for StandIns {
    fn drop(&mut self) {
        let _ = Command::new("pkill").arg("-f").arg(&self.0).status();
    }
}

struct Disk(PathBuf);
impl Drop for Disk {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil").args(["detach", "-force"]).arg(&self.0).stdout(std::process::Stdio::null()).status();
    }
}

#[test]
#[ignore]
fn a_project_goes_through_everything_the_panel_does() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let _stand_ins = StandIns(root.clone());
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: this test is the only one of this program that runs (see the command above), before any thread of its own.
    unsafe { std::env::set_var("HOME", &home) };
    assert_eq!(dirs::home_dir().as_deref(), Some(home.as_path()));

    // ---- Unity Hub's files, two editors, its folder of projects.
    let hub = home.join("Library/Application Support/UnityHub");
    let editors = root.join("External editors");
    let unity = editors.join(VERSION).join("Unity.app");
    bundle("StandInUnity.swift", &unity, "Unity", "cc.orbiters.test.unity");
    let projects = home.join("Unity Projects");
    let avatar = make_project(&projects.join("Mac Avatar"), VERSION, "com.vrchat.avatars");
    let world = make_project(&projects.join("Mac World"), VERSION, "com.vrchat.worlds");
    let newer = make_project(&projects.join("Newer"), "6000.0.1f1", "com.vrchat.avatars");
    write(&hub.join("editors-v2.json"), &serde_json::json!({"schema_version": "v2", "data": [{"version": VERSION, "location": [unity], "architecture": "arm64"}]}).to_string());
    write(&hub.join("projects-v1.json"), &serde_json::json!({"schema_version": "v1", "data": {avatar.path.to_str().unwrap(): {"path": avatar.path, "title": "Mac Avatar", "version": VERSION}}}).to_string());
    write(&hub.join("projectDir.json"), &serde_json::json!({"directoryPath": projects}).to_string());

    // ---- Discovery from Unity Hub's files, the disks not walked past this home folder's guarded folders.
    let cancel = AtomicBool::new(false);
    let scan = discovery::discover(discovery::Scan { last_full: discovery::now(), ..Default::default() }, false, &cancel, |_| {}).unwrap();
    let found = |p: &Path| scan.projects.iter().find(|x| discovery::key(&x.path) == discovery::key(p)).cloned();
    assert_eq!(found(&avatar.path).expect("named by Unity Hub").unity, VERSION);
    assert_eq!(found(&world.path).expect("in Unity Hub's folder of projects").kind, discovery::Kind::Worlds);
    assert!(found(&newer.path).is_some());
    assert!(home.join("Library/Application Support/Orbiters/unity-projects.json").is_file(), "the scan is kept in the app's data");

    // ---- The editors: the version recorded, and only that.
    assert_eq!(discovery::find_editor(VERSION), Some(unity.join("Contents/MacOS/Unity")));
    let missing = orbiters_projects::open_project(&newer).unwrap_err();
    assert!(format!("{missing:#}").contains("Unity 6000.0.1f1 is not installed"), "{missing:#}");

    // ---- Opening: through the system, with the project's path; a second press finds it open.
    let opened = orbiters_projects::open_project(&avatar).unwrap();
    assert!(!opened.already_open);
    wait_for("the editor in the process list", Duration::from_secs(10), || seen(&avatar.path).is_some());
    let (pid, _) = seen(&avatar.path).unwrap();
    wait_for("the stand-in up", Duration::from_secs(10), || avatar.path.join("Temp/started-with").is_file());
    let started_with = std::fs::read_to_string(avatar.path.join("Temp/started-with")).unwrap();
    assert!(started_with.lines().any(|a| a == avatar.path.to_str().unwrap()), "{started_with}");
    assert!(orbiters_projects::unity_open(&avatar.path));
    assert_eq!(orbiters_projects::open_project(&avatar).unwrap().already_open, true);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(seen(&avatar.path).unwrap().0, pid, "no second editor");
    // Another project of the same Unity gets an editor of its own.
    orbiters_projects::open_project(&world).unwrap();
    wait_for("the second editor", Duration::from_secs(10), || seen(&world.path).is_some());
    assert_ne!(seen(&world.path).unwrap().0, pid);
    let world_pid = seen(&world.path).unwrap().0;

    // ---- What may not happen to a project that is open.
    let lock_refused = storage::repair(&avatar.path, &Default::default(), &AtomicBool::new(false)).unwrap_err();
    assert!(format!("{lock_refused:#}").contains("Unity has this project open"), "{lock_refused:#}");

    // ---- Show. Only the app in front may hand the front over (Orbiters is when Show is pressed, this test program never),
    // so here it may be declined: two editors of one bundle leave no other way, and the panel says so.
    let shown = editor::show(pid, &avatar.path);
    println!("Show, from the background: {shown:?}");
    if let Err(e) = shown {
        assert!(format!("{e:#}").contains("did not bring Unity to the front"), "{e:#}");
    }

    // ---- Save & Exit with a modified scene, answered Cancel: it asks, then stays.
    write(&avatar.path.join("Temp/modified"), "");
    write(&avatar.path.join("Temp/answer-cancel"), "");
    let mut session = Session::Open { pid }.next(Event::Close);
    editor::close(pid, &avatar.path).unwrap();
    wait_for("Unity's question", Duration::from_secs(5), || {
        session = session.next(Event::Seen(seen(&avatar.path)));
        matches!(session, Session::Closing { asking: true, .. })
    });
    wait_for("the question answered", Duration::from_secs(8), || {
        session = session.next(Event::Seen(seen(&avatar.path)));
        matches!(session, Session::Closing { answered: true, .. })
    });
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(seen(&avatar.path).map(|s| s.0), Some(pid), "Cancel: it stays open");

    // ---- Save & Exit again, answered Don't Save: it quits and the panel says so.
    std::fs::remove_file(avatar.path.join("Temp/answer-cancel")).unwrap();
    let mut session = Session::Open { pid }.next(Event::Close);
    editor::close(pid, &avatar.path).unwrap();
    wait_for("Unity's question again", Duration::from_secs(5), || {
        session = session.next(Event::Seen(seen(&avatar.path)));
        matches!(session, Session::Closing { asking: true, .. })
    });
    wait_for("Unity gone", Duration::from_secs(10), || seen(&avatar.path).is_none());
    assert!(session.ended(session.next(Event::Seen(None))));
    assert!(!orbiters_projects::unity_open(&avatar.path), "the lock file stays behind, unheld");
    assert!(avatar.path.join("Temp/UnityLockfile").is_file());
    assert_eq!(seen(&world.path).map(|s| s.0), Some(world_pid), "the other project's editor is left alone");

    // ---- Kill: that editor, nothing else, nothing deleted.
    editor::kill(world_pid, &world.path).unwrap();
    wait_for("the editor ended", Duration::from_secs(5), || seen(&world.path).is_none());
    assert!(world.path.join("Assets/Scenes/Main.unity").is_file());
    assert!(!orbiters_projects::unity_open(&world.path));

    // ---- An editor that quits at once is no editor started.
    write(&world.path.join("Temp/fail-at-once"), "");
    let quick = format!("{:#}", orbiters_projects::open_project(&world).unwrap_err());
    assert!(quick.contains("quit right after starting") && quick.contains("~/Library/Logs/Unity/Editor.log"), "{quick}");
    assert_eq!(seen(&world.path), None);
    std::fs::remove_file(world.path.join("Temp/fail-at-once")).unwrap();

    // ---- Unity's log says the disk is full, for this project only.
    write(&home.join("Library/Logs/Unity/Editor.log"), &format!("ENOSPC: no space left on device, write '{}/Library/x'\n", avatar.path.display()));
    assert!(storage::inspect(&avatar.path).disk_error);
    assert!(!storage::inspect(&world.path).disk_error);
    assert!(storage::inspect(&avatar.path).free.is_some());
    assert_eq!(storage::inspect(&avatar.path).disk.as_deref(), Some(Path::new("/")));

    // ---- Sizes.
    let measured = size::measure(&avatar.path, &AtomicBool::new(false)).unwrap();
    assert!(measured.library >= 8 * 300_000 && measured.total > measured.library, "{measured:?}");
    assert_eq!(size::lookup(&avatar.path).size, Some(measured));

    // ---- The repair, to a real second disk: a symbolic link in the Library's place.
    let image = root.join("second-disk.sparseimage");
    let made = Command::new("hdiutil").args(["create", "-size", "12g", "-type", "SPARSE", "-fs", "APFS", "-volname", "Orbiters second disk"]).arg(&image).stdout(std::process::Stdio::null()).status().unwrap();
    assert!(made.success());
    let attached = Command::new("hdiutil").args(["attach", "-nobrowse"]).arg(&image).output().unwrap();
    assert!(attached.status.success());
    let second = PathBuf::from("/Volumes/Orbiters second disk");
    let _detach = Disk(second.clone());
    assert_ne!(storage::inspect(&second).disk.as_deref(), Some(Path::new("/")), "a volume of its own");
    let cache = second.join("Orbiters/UnityCache");
    // SAFETY: as above.
    unsafe { std::env::set_var(storage::CACHE_DIR_VAR, &cache) };
    let library = avatar.path.join("Library");
    let before = std::fs::read(library.join("Artifacts/ab/3.bin")).unwrap();
    let tool = library.join("Artifacts/ab/tool");
    write(&tool, "#!/bin/sh\n");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let progress: storage::SharedProgress = Default::default();
    let plan = storage::plan(&avatar.path, &progress, &AtomicBool::new(false)).unwrap().unwrap();
    assert!(plan.files == 9 && plan.bytes >= 8 * 300_000, "{plan:?}");
    assert!(matches!(&plan.verdict, storage::Verdict::Ready { cache_root, .. } if *cache_root == cache), "{:?}", plan.verdict);
    // Cancelled before anything was copied: nothing changed.
    let stopped = storage::repair(&avatar.path, &Default::default(), &AtomicBool::new(true)).unwrap();
    assert!(matches!(stopped, storage::RepairEnd::Cancelled { .. }), "{stopped:?}");
    assert!(!discovery::is_link(&std::fs::symlink_metadata(&library).unwrap()));
    let storage::RepairEnd::Moved(moved) = storage::repair(&avatar.path, &progress, &AtomicBool::new(false)).unwrap() else { panic!("not moved") };
    assert!(moved.target.starts_with(&cache) && moved.old_left.is_none(), "{moved:?}");
    assert!(discovery::is_link(&std::fs::symlink_metadata(&library).unwrap()));
    assert_eq!(std::fs::read_link(&library).unwrap(), moved.target);
    assert_eq!(std::fs::read(library.join("Artifacts/ab/3.bin")).unwrap(), before, "read through the link");
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(library.join("Artifacts/ab/tool")).unwrap().permissions().mode() & 0o777, 0o755, "a program stays one");
    }
    assert_eq!(storage::inspect(&avatar.path).disk.as_deref(), Some(second.as_path()), "the disk the Library really is on");
    assert!(matches!(storage::plan(&avatar.path, &progress, &AtomicBool::new(false)).unwrap().unwrap().verdict, storage::Verdict::Redirected { to: Some(_) }));
    let refused = storage::repair(&avatar.path, &Default::default(), &AtomicBool::new(false)).unwrap_err();
    assert!(format!("{refused:#}").contains("already redirected"), "{refused:#}");
    // Assets, Packages and settings stay; the walk does not go through the link into the other disk.
    assert!(avatar.path.join("Assets/Scenes/Main.unity").is_file() && avatar.path.join("Packages/vpm-manifest.json").is_file());
    assert_eq!(size::measure(&avatar.path, &AtomicBool::new(false)).unwrap().library, 0);
    // Unity opens the project through the link, and the cache stays while the link points at it.
    orbiters_projects::open_project(&avatar).unwrap();
    wait_for("the editor, on the moved Library", Duration::from_secs(10), || seen(&avatar.path).is_some());
    let in_use = storage::remove_orphans(&AtomicBool::new(false));
    assert!(in_use.removed.is_empty(), "{in_use:?}");
    let (pid, _) = seen(&avatar.path).unwrap();
    editor::kill(pid, &avatar.path).unwrap();
    wait_for("the editor ended", Duration::from_secs(5), || seen(&avatar.path).is_none());
    assert!(moved.target.join("Artifacts/ab/3.bin").is_file());
    // A kill between the rename and the link leaves the old Library renamed: the next sweep puts it back.
    std::fs::remove_file(&library).unwrap();
    let old = avatar.path.join("Library.orbiters-relocation-abc");
    std::fs::create_dir_all(old.join("Artifacts")).unwrap();
    let swept = storage::remove_orphans(&AtomicBool::new(false));
    assert_eq!(swept.removed, vec![moved.target.clone()], "{swept:?}");
    assert_eq!(swept.restored, vec![avatar.path.canonicalize().unwrap()], "{swept:?}");
    assert!(library.join("Artifacts").is_dir() && !old.exists() && !moved.target.exists());

    // ---- Unity Hub, where VCC or ALCOM were told it is.
    let hub_app = root.join("Elsewhere/Unity Hub.app");
    bundle("StandInBlender.swift", &hub_app, "Unity Hub", "cc.orbiters.test.hub");
    write(&orbiters_projects::vcc_dir().join("settings.json"), &serde_json::json!({"pathToUnityHub": hub_app}).to_string());
    assert_eq!(orbiters_projects::unity_hub(), Some(hub_app.join("Contents/MacOS/Unity Hub")));
    orbiters_projects::open_hub().unwrap();
    let hub_running = || Command::new("pgrep").args(["-f", hub_app.join("Contents/MacOS/Unity Hub").to_str().unwrap()]).output().unwrap().stdout;
    wait_for("Unity Hub started", Duration::from_secs(10), || !hub_running().is_empty());
    let _ = Command::new("pkill").args(["-f", hub_app.join("Contents/MacOS/Unity Hub").to_str().unwrap()]).status();

    // ---- Blender: the newest with Orbiters' add-on, started by the system with the file.
    let current = home.join("Applications/Blender.app");
    let older = home.join("Applications/Blender 3.6.app");
    bundle("StandInBlender.swift", &current, "Blender", "cc.orbiters.test.blender");
    bundle("StandInBlender.swift", &older, "Blender", "cc.orbiters.test.blender36");
    std::fs::create_dir_all(current.join("Contents/Resources/4.2")).unwrap();
    std::fs::create_dir_all(older.join("Contents/Resources/3.6")).unwrap();
    std::fs::create_dir_all(home.join("Library/Application Support/Blender/3.6/scripts/addons/orbiters_mixer")).unwrap();
    let note = root.join("opened-with");
    write(&older.join("Contents/Resources/note-path"), note.to_str().unwrap());
    write(&current.join("Contents/Resources/note-path"), note.to_str().unwrap());
    let installs = blender::installs();
    println!("Blenders: {installs:?}");
    assert!(installs.iter().any(|b| b.exe == current.join("Contents/MacOS/Blender") && b.version == Some((4, 2))));
    assert_eq!(blender::find().map(|b| b.exe), Some(older.join("Contents/MacOS/Blender")), "the one with the add-on");
    let blend = projects.join("Mac Avatar/Assets/My Avatar.blend");
    write(&blend, "BLENDER");
    assert_eq!(blender::open(&blend).unwrap(), "Blender 3.6");
    wait_for("the file reaching Blender", Duration::from_secs(15), || note.is_file());
    assert_eq!(std::fs::read_to_string(&note).unwrap().trim(), blend.to_str().unwrap());
    assert!(format!("{:#}", blender::open(&root.join("gone.blend")).unwrap_err()).contains("not there anymore"));

    // The stand-ins leave no trace in the system's list of applications.
    let lsregister = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
    for app in [&unity, &hub_app, &current, &older] {
        let _ = Command::new(lsregister).arg("-u").arg(app).status();
    }
}
