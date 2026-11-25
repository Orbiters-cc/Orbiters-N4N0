use super::*;

#[test]
fn what_the_hub_s_programs_say_is_read() {
    // Unity's own command line (as 1.0.1 writes it).
    assert_eq!(line(r#"{"type":"progress","pct":43,"msg":"Downloading 2022.3.22f1-arm64...","phase":"download"}"#), Some(Line::Progress { percent: 43, installing: false }));
    assert_eq!(line(r#"{"type":"progress","pct":80,"msg":"Installing...","phase":"install"}"#), Some(Line::Progress { percent: 80, installing: true }));
    assert_eq!(line(r#"{"type":"result","success":true,"command":"install","data":{},"errors":[],"warnings":[]}"#), Some(Line::Result { ok: true, why: None }));
    assert_eq!(
        line(r#"{"type":"result","success":false,"command":"install","data":null,"errors":[{"code":"X","message":"No space left"}]}"#),
        Some(Line::Result { ok: false, why: Some("No space left".into()) })
    );
    assert_eq!(line(r#"{"type":"notice"}"#), None);
    // A Hub 3's text.
    assert_eq!(line("[Unity (2022.3.22f1)] downloading 26.64%"), Some(Line::Progress { percent: 26, installing: false }));
    assert_eq!(line("[Unity (2022.3.22f1)] installing"), Some(Line::Progress { percent: 100, installing: true }));
    assert_eq!(line("All Tasks Completed Successfully."), Some(Line::Result { ok: true, why: None }));
    assert_eq!(line("Error while installing an editor or a module"), Some(Line::Result { ok: false, why: Some("Error while installing an editor or a module".into()) }));
    assert_eq!(line("Unity CLI sends usage analytics"), None);
}

fn wanted(version: &str, changeset: Option<&str>) -> Wanted {
    Wanted { version: version.into(), changeset: changeset.map(str::to_owned), project: PathBuf::from("/Projects/Avatar"), name: "Avatar".into() }
}

/// The version is the project's and the build its own or the release list's: nothing is written here, and only words that are a version and a build reach a command.
#[test]
fn the_version_and_its_build_come_from_the_project_and_the_release_list() {
    let list = matrix::Matrix::built_in();
    let row = list.stable_2022().expect("VRChat's 2022").clone();
    let w = wanted(&row.unity, None).checked(&list).unwrap();
    assert_eq!(w.changeset.as_deref(), Some(row.changeset.as_str()), "the list's");
    assert_eq!(w.hub_link(), Some(row.hub_link()));
    assert_eq!(wanted(&row.unity, Some("0123456789ab")).checked(&list).unwrap().changeset.as_deref(), Some("0123456789ab"), "the project's own first");
    for (version, changeset) in [("--version", None), ("2022.3.22f1; rm", None), ("", None), ("2022.3.22f1", Some("zz")), ("2022.3.22f1", Some("--headless"))] {
        assert_eq!(wanted(version, changeset).checked(&list), None, "{version} {changeset:?}");
    }
    let cli = HubCli::Unity("/unity".into());
    let args = cli.install(&w.version, w.changeset.as_deref());
    assert!(args.starts_with(&["install".to_string(), row.unity.clone(), "--changeset".into(), row.changeset.clone()]), "{args:?}");
    assert!(args.windows(2).any(|a| a == ["--architecture", "arm64"]) && args.windows(2).any(|a| a == ["--module", MODULE]));
    assert!(!args.iter().any(|a| a.contains("eula")), "no licence is accepted for the person: {args:?}");
    let old = HubCli::Headless("/hub".into()).install("2022.3.22f1", Some("887be4894c44"));
    assert_eq!(old[..3], ["--", "--headless", "install"]);
}

#[test]
fn what_it_takes_is_unity_s_word_for_apple_silicon() {
    let size = |n: u64| serde_json::json!({ "value": n, "unit": "BYTE" });
    let json = serde_json::json!({ "results": [{ "version": "2022.3.22f1", "shortRevision": "887be4894c44", "downloads": [
        { "platform": "MAC_OS", "architecture": "X86_64", "downloadSize": size(1), "installedSize": size(1), "modules": [] },
        { "platform": "MAC_OS", "architecture": "ARM64", "downloadSize": size(3_632_871_537), "installedSize": size(7_234_018_000), "modules": [
            { "id": "android", "downloadSize": size(9), "installedSize": size(9) },
            { "id": "windows-mono", "downloadSize": size(180_619_269), "installedSize": size(457_949_000) } ] } ] }] });
    let r = Release::from_json(&json.to_string(), "2022.3.22f1").unwrap();
    assert_eq!(r, Release { changeset: Some("887be4894c44".into()), download: 3_813_490_806, installed: 7_691_967_000 });
    assert_eq!(Release::from_json(&json.to_string(), "2022.3.6f1"), None);
    assert_eq!(Release::from_json("<html>", "2022.3.22f1"), None);
    let list = matrix::Matrix::built_in();
    let row = list.stable_2022().unwrap();
    assert!(Release::from_list(&list, &row.unity).unwrap().download > 3_000_000_000, "with no answer, the release list's");
}

#[cfg(target_os = "macos")]
mod on_a_mac {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    const VERSION: &str = "2022.3.22f1";
    const BUILD: &str = "887be4894c44";

    /// Disks with this much room, everywhere.
    struct Free(Arc<Mutex<u64>>);
    impl budget::Room for Free {
        fn of(&self, _: &Path) -> Option<(String, u64)> {
            Some(("disk".into(), *self.0.lock().unwrap()))
        }
    }

    /// A Mac of a test's own: its folders, its Hub (a script for a program), its licence, its disk.
    struct Rig {
        root: PathBuf,
        world: World,
        free: Arc<Mutex<u64>>,
        licence: Arc<Mutex<Licence>>,
        opened: Arc<Mutex<Vec<String>>>,
        steps: Mutex<Vec<Step>>,
        stop: AtomicBool,
        anyway: AtomicBool,
    }

    static RIGS: AtomicU32 = AtomicU32::new(0);

    /// What the editor installed in `root` says it is: a file with its version and its processor.
    fn editor_in(root: &Path, version: &str, build: Option<&str>) -> editors::Found {
        let program = root.join("editor");
        let Ok(said) = std::fs::read_to_string(&program) else { return editors::Found::Missing { wrong: vec![] } };
        let mut words = said.split_whitespace();
        let identity = editors::Identity {
            version: words.next().unwrap_or_default().into(),
            changeset: words.next().map(str::to_owned),
            arch: vec![if words.next() == Some("x86_64") { editors::Arch::X86_64 } else { editors::Arch::Arm64 }],
        };
        let check = editors::judge_build(Some(&identity), version, build);
        let c = editors::Candidate { program, hint: editors::Hint::HubFolder, check: check.clone(), identity: Some(identity), modules: Default::default() };
        match check {
            editors::Check::Verified => editors::Found::Exact(c),
            editors::Check::Unverified => editors::Found::Unverified(c),
            editors::Check::Wrong(_) => editors::Found::Missing { wrong: vec![c] },
        }
    }

    impl Rig {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("orbiters-unity-install-{}-{}", std::process::id(), RIGS.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let free = Arc::new(Mutex::new(100_000_000_000u64));
            let licence = Arc::new(Mutex::new(Licence::Here));
            let opened: Arc<Mutex<Vec<String>>> = Arc::default();
            let (r1, r2, l, o) = (root.clone(), root.clone(), licence.clone(), opened.clone());
            let world = World {
                hub: Box::new(move || Some(r1.join("Applications/Unity Hub.app/Contents/MacOS/Unity Hub")).filter(|p| p.is_file())),
                editor: Box::new(move |v, b| editor_in(&r2, v, b)),
                licence: Box::new(move |_| *l.lock().unwrap()),
                room: Box::new(Free(free.clone())),
                genuine: Box::new(|app| app.join("Contents/MacOS/Unity Hub").is_file()),
                open: Box::new(move |what| {
                    o.lock().unwrap().push(what.to_owned());
                    true
                }),
                // Nothing answers there: what is needed comes from the release list, and there is no Hub to download.
                hub_feed: "http://127.0.0.1:9/hub/latest-mac.yml".into(),
                releases: "http://127.0.0.1:9/releases".into(),
                applications: vec![root.join("Applications")],
                install_at: root.join("Editors"),
                download_at: root.join("Downloads"),
                folder: root.join("data"),
                every: Duration::from_millis(20),
                licence_every: Duration::from_millis(60),
                agent: orbiters_core::http_agent(Some(Duration::from_secs(5)), Some(Duration::from_secs(10))),
            };
            Self { root, world, free, licence, opened, steps: Mutex::default(), stop: AtomicBool::new(false), anyway: AtomicBool::new(false) }
        }

        /// A Hub whose command line is `script` (sh; `$ROOT` the rig's folder).
        fn hub(&self, script: &str) {
            let contents = self.root.join("Applications/Unity Hub.app/Contents");
            for (path, text) in [("MacOS/Unity Hub", "#!/bin/sh\nexit 1\n".to_string()), ("Resources/unity", format!("#!/bin/sh\nROOT='{}'\necho \"$@\" >> \"$ROOT/asked\"\n{script}\n", self.root.display()))] {
                let file = contents.join(path);
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(&file, text).unwrap();
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }

        fn run(&self) -> Result<(), End> {
            self.run_in(false)
        }

        fn run_in(&self, in_hub: bool) -> Result<(), End> {
            let say = |s: Step| self.steps.lock().unwrap().push(s);
            run(&self.world, &wanted(VERSION, Some(BUILD)), in_hub, &say, &self.stop, &self.anyway)
        }

        fn said(&self) -> Vec<Step> {
            self.steps.lock().unwrap().clone()
        }

        fn asked(&self) -> String {
            std::fs::read_to_string(self.root.join("asked")).unwrap_or_default()
        }

        /// Waits until a step said is as `good` says.
        fn until(&self, good: impl Fn(&Step) -> bool) {
            let began = Instant::now();
            while !self.said().iter().any(&good) {
                assert!(began.elapsed() < Duration::from_secs(20), "never came: {:?}", self.said());
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    const PROGRESS: &str = r#"echo '{"type":"progress","pct":0,"msg":"Starting install...","phase":"download"}'
echo '{"type":"progress","pct":60,"msg":"Downloading 2022.3.22f1-arm64...","phase":"download"}'
echo '{"type":"progress","pct":20,"msg":"Installing","phase":"install"}'"#;
    const DONE: &str = r#"echo '{"type":"result","success":true,"command":"install","data":{},"errors":[],"warnings":[]}'"#;

    #[test]
    fn with_the_hub_here_the_editor_is_installed_and_checked() {
        let rig = Rig::new();
        rig.hub(&format!("{PROGRESS}\necho '{VERSION} {BUILD} arm64' > \"$ROOT/editor\"\n{DONE}"));
        assert_eq!(rig.run(), Ok(()));
        let said = rig.said();
        assert!(said.iter().any(|s| matches!(s, Step::Editor { percent: 60, installing: false, bytes } if *bytes > 3_000_000_000)), "{said:?}");
        assert!(said.iter().any(|s| matches!(s, Step::Editor { installing: true, .. })) && said.contains(&Step::Verifying), "{said:?}");
        assert!(rig.asked().contains(&format!("install {VERSION} --changeset {BUILD} --architecture arm64 --module windows-mono")), "{}", rig.asked());
        assert_eq!(left(&rig.world.folder), None, "done: nothing is left to go on with");
    }

    #[test]
    fn with_the_editor_here_nothing_is_installed() {
        let rig = Rig::new();
        rig.hub("exit 1");
        std::fs::write(rig.root.join("editor"), format!("{VERSION} {BUILD} arm64")).unwrap();
        assert_eq!(rig.run(), Ok(()));
        assert_eq!(rig.asked(), "");
    }

    /// A command that ends well proves nothing: another version, another build or an editor for Intel is not it.
    #[test]
    fn nothing_is_ready_that_is_not_that_editor() {
        for found in ["", "2022.3.6f1 b9e6e7e9fa2d arm64", "2022.3.22f1 0000000000ff arm64", "2022.3.22f1 887be4894c44 x86_64"] {
            let rig = Rig::new();
            rig.hub(&format!("{}\n{DONE}", if found.is_empty() { String::new() } else { format!("echo '{found}' > \"$ROOT/editor\"") }));
            let Err(End::Failed(why)) = rig.run() else { panic!("{found:?} was taken for it") };
            assert!(why.contains(VERSION), "{why}");
            assert!(left(&rig.world.folder).is_some(), "it can be tried again");
        }
    }

    #[test]
    fn the_hub_s_own_words_for_a_failure_are_said() {
        let rig = Rig::new();
        rig.hub(r#"echo '{"type":"result","success":false,"command":"install","data":null,"errors":[{"code":"INSUFFICIENT_PERMISSIONS","message":"The install location is not writable"}]}'; exit 1"#);
        assert!(matches!(rig.run(), Err(End::Failed(why)) if why.contains("not writable")));
        // The Hub itself can then be asked, opened on that very editor; the flow goes on once it is there.
        let editor = rig.root.join("editor");
        std::thread::scope(|s| {
            let run = s.spawn(|| rig.run_in(true));
            rig.until(|s| *s == Step::InHub);
            assert_eq!(*rig.opened.lock().unwrap(), [format!("unityhub://{VERSION}/{BUILD}")]);
            std::fs::write(&editor, format!("{VERSION} {BUILD} arm64")).unwrap();
            assert_eq!(run.join().unwrap(), Ok(()));
        });
    }

    #[test]
    fn not_enough_room_is_said_before_anything_comes() {
        let rig = Rig::new();
        rig.hub("exit 1");
        *rig.free.lock().unwrap() = 9_000_000_000;
        let Err(End::Room { need, free, .. }) = rig.run() else { panic!("{:?}", rig.said()) };
        assert!(need > 14_000_000_000 && free == Some(9_000_000_000), "{need} {free:?}");
        assert_eq!((rig.asked(), left(&rig.world.folder)), (String::new(), None));
        // Space freed: it goes on.
        *rig.free.lock().unwrap() = 40_000_000_000;
        rig.hub(&format!("echo '{VERSION} {BUILD} arm64' > \"$ROOT/editor\"\n{DONE}"));
        assert_eq!(rig.run(), Ok(()));
    }

    /// Cancel interrupts the command as Ctrl+C does; asked again, the command is run again, which goes on from its own download.
    #[test]
    fn a_cancelled_install_ends_its_command_and_can_go_on() {
        let rig = Rig::new();
        rig.hub(&format!("trap 'echo interrupted > \"$ROOT/interrupted\"; exit 130' INT\n{PROGRESS}\nwhile :; do sleep 0.05; done"));
        std::thread::scope(|s| {
            let run = s.spawn(|| rig.run());
            rig.until(|s| matches!(s, Step::Editor { percent: 60, .. }));
            let record = left(&rig.world.folder).unwrap();
            assert!(running(&record));
            rig.stop.store(true, Ordering::Relaxed);
            assert_eq!(run.join().unwrap(), Err(End::Stopped));
            assert!(rig.root.join("interrupted").is_file() && !running(&record));
        });
        assert_eq!(left(&rig.world.folder).unwrap().wanted.version, VERSION, "kept: Resume");
        rig.stop.store(false, Ordering::Relaxed);
        rig.hub(&format!("echo '{VERSION} {BUILD} arm64' > \"$ROOT/editor\"\n{DONE}"));
        assert_eq!(rig.run(), Ok(()));
        assert_eq!(rig.asked().lines().count(), 2);
    }

    /// The window that began it is gone: the command goes on, and the next one follows it to its end without starting another.
    #[test]
    fn an_install_outlives_the_window_and_is_followed_by_the_next() {
        let rig = Rig::new();
        rig.hub(&format!("{PROGRESS}\nwhile [ ! -e \"$ROOT/go\" ]; do sleep 0.05; done\necho '{VERSION} {BUILD} arm64' > \"$ROOT/editor\"\n{DONE}"));
        // As an earlier window did: started, recorded, and nobody's any more.
        let cli = HubCli::of(&(rig.world.hub)().unwrap());
        std::fs::create_dir_all(&rig.world.folder).unwrap();
        let (record, child) = start_for_tests(&rig.world, &cli, &wanted(VERSION, Some(BUILD)), 5);
        // (reaped as the system does once its parent is gone)
        std::thread::spawn(move || {
            let mut child = child;
            let _ = child.wait();
        });
        assert!(running(&record) && left(&rig.world.folder) == Some(record.clone()));
        std::thread::scope(|s| {
            let run = s.spawn(|| rig.run());
            rig.until(|s| matches!(s, Step::Editor { percent: 60, .. }));
            std::fs::write(rig.root.join("go"), "").unwrap();
            assert_eq!(run.join().unwrap(), Ok(()));
        });
        assert_eq!(rig.asked().lines().count(), 1, "one install, followed: {}", rig.asked());
    }

    #[test]
    fn a_licence_is_waited_for_and_the_flow_goes_on_once_there_is_one() {
        let rig = Rig::new();
        rig.hub("exit 1");
        std::fs::write(rig.root.join("editor"), format!("{VERSION} {BUILD} arm64")).unwrap();
        *rig.licence.lock().unwrap() = Licence::None { signed_in: false };
        std::thread::scope(|s| {
            let run = s.spawn(|| rig.run());
            rig.until(|s| *s == Step::Licence { signed_in: false });
            *rig.licence.lock().unwrap() = Licence::None { signed_in: true };
            rig.until(|s| *s == Step::Licence { signed_in: true });
            *rig.licence.lock().unwrap() = Licence::Here;
            assert_eq!(run.join().unwrap(), Ok(()));
        });
        // A licence this cannot see (a company's): the person goes on all the same.
        *rig.licence.lock().unwrap() = Licence::None { signed_in: false };
        rig.steps.lock().unwrap().clear();
        std::thread::scope(|s| {
            let run = s.spawn(|| rig.run());
            rig.until(|s| matches!(s, Step::Licence { .. }));
            rig.anyway.store(true, Ordering::Relaxed);
            assert_eq!(run.join().unwrap(), Ok(()));
        });
        // And Cancel ends the wait.
        rig.anyway.store(false, Ordering::Relaxed);
        rig.steps.lock().unwrap().clear();
        std::thread::scope(|s| {
            let run = s.spawn(|| rig.run());
            rig.until(|s| matches!(s, Step::Licence { .. }));
            rig.stop.store(true, Ordering::Relaxed);
            assert_eq!(run.join().unwrap(), Err(End::Stopped));
        });
    }

    /// A server of files for one test: `cut` bytes of the archive at first, then the rest when it is asked for from there.
    fn serve(archive: Vec<u8>, feed: String, cut: usize) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let at = format!("http://{}", listener.local_addr().unwrap());
        let asked: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = asked.clone();
        std::thread::spawn(move || {
            let mut first = true;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).is_ok_and(|n| n == 1) {
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).to_lowercase();
                let path = head.split_whitespace().nth(1).unwrap_or_default().to_owned();
                let range = head.lines().find_map(|l| l.strip_prefix("range: bytes=")?.trim().strip_suffix('-')?.parse::<usize>().ok());
                log.lock().unwrap().push(format!("{path} {range:?}"));
                if path.ends_with(".yml") {
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{feed}", feed.len());
                } else if path.ends_with(".zip") {
                    let from = range.unwrap_or(0).min(archive.len());
                    let rest = &archive[from..];
                    let status = if range.is_some() { "206 Partial Content" } else { "200 OK" };
                    let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", rest.len());
                    let some = if std::mem::take(&mut first) && cut > 0 { &rest[..cut.min(rest.len())] } else { rest };
                    let _ = stream.write_all(some);
                } else {
                    let _ = write!(stream, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                }
            }
        });
        (at, asked)
    }

    /// An archive of a Hub as Unity's: `Unity Hub.app`, its command line inside.
    fn hub_archive(rig: &Rig, script: &str) -> Vec<u8> {
        rig.hub(script);
        let zip = rig.root.join("hub.zip");
        let app = rig.root.join("Applications/Unity Hub.app");
        assert!(std::process::Command::new("/usr/bin/ditto").args(["-ck", "--keepParent"]).arg(&app).arg(&zip).status().unwrap().success());
        std::fs::remove_dir_all(rig.root.join("Applications")).unwrap();
        std::fs::read(zip).unwrap()
    }

    fn feed(archive: &[u8]) -> String {
        use base64::Engine as _;
        let sha = base64::engine::general_purpose::STANDARD.encode(ring::digest::digest(&ring::digest::SHA512, archive));
        let other = base64::engine::general_purpose::STANDARD.encode([7u8; 64]);
        format!("version: 4.0.2\nfiles:\n  - url: 4.0.2/Hub-x64.zip\n    sha512: {other}\n    size: 12\n  - url: 4.0.2/Hub-arm64.zip\n    sha512: {sha}\n    size: {}\n  - url: 4.0.2/Hub-arm64.dmg\n    sha512: {other}\n    size: 40\npath: 4.0.2/Hub-x64.zip\n", archive.len())
    }

    #[test]
    fn with_no_hub_it_comes_first_and_a_download_cut_short_goes_on() {
        let mut rig = Rig::new();
        let archive = hub_archive(&rig, &format!("echo '{VERSION} {BUILD} arm64' > \"$ROOT/editor\"\n{DONE}"));
        let (at, asked) = serve(archive.clone(), feed(&archive), archive.len() / 2);
        rig.world.hub_feed = format!("{at}/hub/latest-mac.yml");
        assert!(matches!(rig.run(), Err(End::Failed(_))), "cut: {:?}", rig.said());
        assert!(!rig.root.join("Applications/Unity Hub.app").exists(), "nothing half is in place");
        assert_eq!(rig.run(), Ok(()), "{:?}", asked.lock().unwrap());
        assert!(asked.lock().unwrap().iter().any(|a| a.contains(".zip") && a.contains(&format!("Some({})", archive.len() / 2))), "from where it was: {:?}", asked.lock().unwrap());
        assert!(rig.root.join("Applications/Unity Hub.app/Contents/Resources/unity").is_file());
        assert!(rig.said().contains(&Step::PlacingHub) && rig.said().contains(&Step::Verifying));
        let left: Vec<_> = std::fs::read_dir(rig.root.join("Applications")).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left, ["Unity Hub.app"], "nothing else stays there");
    }

    #[test]
    fn a_hub_that_is_not_unity_s_or_not_what_its_list_says_is_never_put_in_place() {
        let mut rig = Rig::new();
        let archive = hub_archive(&rig, "exit 1");
        // Not what the list says: its hash is another's.
        let mut other = archive.clone();
        *other.last_mut().unwrap() ^= 1;
        let (at, _) = serve(other, feed(&archive), 0);
        rig.world.hub_feed = format!("{at}/hub/latest-mac.yml");
        assert!(matches!(rig.run(), Err(End::Failed(_))));
        assert!(!rig.root.join("Applications/Unity Hub.app").exists());
        // Not Unity's signature.
        let (at, _) = serve(archive.clone(), feed(&archive), 0);
        rig.world.hub_feed = format!("{at}/hub/latest-mac.yml");
        rig.world.genuine = Box::new(|_| false);
        assert!(matches!(rig.run(), Err(End::Failed(why)) if why.contains("signed")));
        assert!(!rig.root.join("Applications/Unity Hub.app").exists());
    }

    #[test]
    fn the_hub_of_unity_s_list_is_the_one_for_apple_silicon() {
        let archive = b"abc".to_vec();
        let f = HubFile::of(&feed(&archive)).unwrap();
        assert_eq!((f.url.as_str(), f.size), ("4.0.2/Hub-arm64.zip", 3));
        assert_eq!(HubFile::of("version: 1\nfiles:\n  - url: ../x-arm64.zip\n    sha512: AAAA\n    size: 1\n"), None);
        assert_eq!(HubFile::of(""), None);
    }
}
