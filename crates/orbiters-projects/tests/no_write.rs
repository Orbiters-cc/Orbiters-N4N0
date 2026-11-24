//! Orbiters writes nothing of Creator Companion's or ALCOM's: every action of the Unity projects page, on every state their files
//! can be in, leaves their folder as it was, byte for byte and to the time of its last change, down to the folders themselves.
//! Every store here is a generated one in a temporary folder.

#[path = "support/repository.rs"]
mod repository;

use orbiters_projects::{missing, templates, vpm};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use vrc_get_vpm::environment::{Settings, VccDatabaseConnection};
use vrc_get_vpm::io::{DefaultEnvironmentIo, DefaultProjectIo};
use vrc_get_vpm::UnityProject;

/// What the page does, by the names it has on this branch.
mod page {
    use std::path::{Path, PathBuf};
    pub fn add(root: &Path, project: &Path) -> anyhow::Result<()> {
        orbiters_projects::gallery::add(root, project)
    }
    pub fn remove(root: &Path, project: &Path) -> anyhow::Result<()> {
        orbiters_projects::gallery::remove(root, project)
    }
    pub fn listed(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
        Ok(orbiters_projects::gallery::listed(root).projects)
    }
    pub fn own(root: &Path) -> PathBuf {
        orbiters_projects::own::dir(root)
    }
}

fn block<T>(f: impl std::future::Future<Output = T>) -> T {
    orbiters_projects::runtime().unwrap().block_on(f)
}

/// No test reaches VRChat's repositories: they are asked through a proxy of this test's that refuses. (The loopback ones need
/// none.) Answers that proxy's address: a server that has nothing.
fn offline() -> &'static str {
    static REFUSES: std::sync::OnceLock<(String, repository::Repository)> = std::sync::OnceLock::new();
    let (at, _) = REFUSES.get_or_init(|| {
        let server = repository::serve_raw(|_| b"HTTP/1.0 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec());
        // SAFETY: before anything reads it, once.
        unsafe { std::env::set_var("HTTPS_PROXY", &server.0) };
        server
    });
    at
}

struct Store {
    root: PathBuf,
    projects: PathBuf,
    held: Mutex<Vec<std::fs::File>>,
    _temp: tempfile::TempDir,
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(page::own(&self.root));
    }
}

impl Store {
    fn new() -> Store {
        offline();
        let temp = tempfile::tempdir().unwrap();
        Store { root: temp.path().join("VRChatCreatorCompanion"), projects: temp.path().join("Projects"), held: Mutex::default(), _temp: temp }
    }
    fn io(&self) -> DefaultEnvironmentIo {
        DefaultEnvironmentIo::new(self.root.clone().into())
    }
    fn path(&self, name: &str) -> PathBuf {
        self.projects.join(name)
    }
    fn project(&self, name: &str) -> PathBuf {
        let p = self.path(name);
        for dir in ["ProjectSettings", "Packages", "Assets"] {
            std::fs::create_dir_all(p.join(dir)).unwrap();
        }
        std::fs::write(p.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
        std::fs::write(p.join("Packages/vpm-manifest.json"), r#"{"dependencies":{},"locked":{}}"#).unwrap();
        std::fs::write(p.join("Packages/manifest.json"), r#"{"dependencies":{}}"#).unwrap();
        std::fs::write(p.join("Assets/a.txt"), name).unwrap();
        p
    }
    /// Their database, with the projects `names` (the first a favourite).
    fn database(&self, names: &[&str]) {
        std::fs::create_dir_all(&self.root).unwrap();
        block(async {
            let io = self.io();
            let mut db = VccDatabaseConnection::connect(&io).await.unwrap();
            for (i, name) in names.iter().enumerate() {
                let path = self.project(name);
                db.add_project(&UnityProject::load(DefaultProjectIo::new(path.clone().into())).await.unwrap()).await.unwrap();
                let mut p = db.find_project(path.to_str().unwrap()).unwrap().unwrap();
                p.set_favorite(i == 0);
                db.update_project(&p);
            }
            db.save(&io).await.unwrap();
        });
    }
    /// Takes `name` off their list, as either program does.
    fn they_remove(&self, name: &str) {
        block(async {
            let io = self.io();
            let mut db = VccDatabaseConnection::connect(&io).await.unwrap();
            let p = db.find_project(self.path(name).to_str().unwrap()).unwrap().unwrap();
            db.remove_project(&p);
            db.save(&io).await.unwrap();
            let mut settings = Settings::load(&io).await.unwrap();
            settings.retain_user_projects(|p| p != self.path(name).to_str().unwrap());
            settings.save(&io).await.unwrap();
        });
    }
    fn settings(&self, text: &str) {
        std::fs::create_dir_all(&self.root).unwrap();
        std::fs::write(self.root.join("settings.json"), text).unwrap();
    }
    /// Settings with the whole list, and with a key no program here knows.
    fn whole(&self) -> serde_json::Value {
        json!({ "userProjects": [self.path("A"), self.path("B")], "showPrereleasePackages": true, "someoneElses": { "kept": [1, 2.50] }, "userRepos": [] })
    }
    fn damage(&self, edit: impl FnOnce(&mut Vec<u8>)) {
        let at = self.root.join("vcc.liteDb");
        let mut bytes = std::fs::read(&at).unwrap();
        edit(&mut bytes);
        std::fs::write(at, bytes).unwrap();
    }
    /// `file`, held by another program alone.
    fn hold(&self, file: &str) {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            self.held.lock().unwrap().push(std::fs::File::options().read(true).write(true).share_mode(0).open(self.root.join(file)).unwrap());
        }
        #[cfg(not(windows))]
        let _ = file;
    }
}

/// Every state their files are tried in.
const STATES: &[&str] = &[
    "as VCC and ALCOM keep it",
    "VCC alone",
    "ALCOM alone",
    "no list in the settings",
    "null for the list",
    "settings of spaces",
    "settings of zeros",
    "no settings",
    "no settings, ALCOM's copy of them",
    "no database",
    "nothing in it",
    "not there",
    "settings cut short",
    "a list of another type",
    "a database whose directory is damaged",
    "a database cut to a page",
    "a database of five bytes",
    "changes left in its log",
    "settings in use",
    "a database in use",
];

/// A store in that state, with the projects A and B, which it lists where it lists any.
fn make(state: &str) -> Store {
    let s = Store::new();
    let (a, b) = (s.project("A"), s.project("B"));
    let alcom = |s: &Store| {
        orbiters_projects::write_json(&s.root.join("vrc-get/settings.json"), &json!({ "ignoreOfficialRepository": false })).unwrap();
        orbiters_projects::write_json(&s.root.join("vrc-get/gui-config.json"), &json!({ "guiHiddenRepositories": ["some.repo"], "language": "en" })).unwrap();
        orbiters_projects::write_json(&s.root.join("vrc-get/vcc-settings-backup.json"), &s.whole()).unwrap();
    };
    if !matches!(state, "no database" | "nothing in it" | "not there" | "a database of five bytes") {
        s.database(&["A", "B"]);
    }
    match state {
        "as VCC and ALCOM keep it" => {
            s.settings(&serde_json::to_string_pretty(&s.whole()).unwrap());
            alcom(&s);
            std::fs::create_dir_all(s.root.join("Repos/test.base")).unwrap();
            std::fs::write(s.root.join("Repos/package-cache.json"), "{}").unwrap();
        }
        "ALCOM alone" => {
            s.settings(&s.whole().to_string());
            alcom(&s);
        }
        "VCC alone" | "no database" => s.settings(&s.whole().to_string()),
        "no list in the settings" => s.settings(r#"{"showPrereleasePackages":true}"#),
        "null for the list" => s.settings(r#"{"userProjects":null}"#),
        "settings of spaces" => s.settings("  \n\t "),
        "settings of zeros" => s.settings("\0\0\0\0\0\0"),
        "no settings, ALCOM's copy of them" => alcom(&s),
        "nothing in it" => std::fs::create_dir_all(&s.root).unwrap(),
        "settings cut short" => s.settings(r#"{"userProjects":["#),
        "a list of another type" => s.settings(r#"{"userProjects":{"a":1}}"#),
        // The fourth verification's fixture: the length of the directory in the header.
        "a database whose directory is damaged" => s.damage(|b| {
            b.resize(49_152.max(b.len()), 0);
            b[192..196].fill(0);
        }),
        "a database cut to a page" => s.damage(|b| b.truncate(8192)),
        "a database of five bytes" => {
            std::fs::create_dir_all(&s.root).unwrap();
            std::fs::write(s.root.join("vcc.liteDb"), "five!").unwrap();
        }
        "changes left in its log" => {
            s.settings(&s.whole().to_string());
            std::fs::write(s.root.join("vcc-log.liteDb"), vec![7u8; 8192]).unwrap();
        }
        "settings in use" | "a database in use" => s.settings(&s.whole().to_string()),
        _ => {}
    }
    let _ = (a, b);
    s
}

type Entry = (bool, Vec<u8>, SystemTime, Option<SystemTime>);

/// Everything in their folder: which it is, its bytes, when it was changed and made, the folders too.
fn everything(root: &Path) -> BTreeMap<PathBuf, Entry> {
    let mut out = BTreeMap::new();
    for e in walkdir::WalkDir::new(root).into_iter().flatten() {
        let m = std::fs::symlink_metadata(e.path()).unwrap();
        let bytes = if m.is_file() { std::fs::read(e.path()).unwrap() } else { vec![] };
        out.insert(e.path().strip_prefix(root).unwrap().to_path_buf(), (m.is_dir(), bytes, m.modified().unwrap(), m.created().ok()));
    }
    out
}

fn duplicate(s: &Store, source: &str, name: &str, stop: bool) -> anyhow::Result<PathBuf> {
    block(templates::duplicate(&s.root, &s.path(source), &s.projects, name, templates::Keep::default(), &AtomicBool::new(stop), &Arc::new(Mutex::default())))
}

fn lists(s: &Store, name: &str) -> bool {
    page::listed(&s.root).is_ok_and(|all| all.iter().any(|p| *p == s.path(name)))
}

/// Every action: its name, what it needs first (not looked at), what it does, and whether it went as it should.
type Action = (&'static str, fn(&Store, &str), fn(&Store, &str) -> anyhow::Result<bool>);

fn nothing(_: &Store, _: &str) {}

fn repos(s: &Store, url: &str) -> anyhow::Result<()> {
    block(vpm::add_repo(&s.root, url, BTreeMap::from([("X-Fixture".into(), "enabled".into())])))
}

fn with_repository(s: &Store, url: &str) {
    s.project("P");
    repos(s, url).unwrap();
}

fn needing(s: &Store, package: &str) {
    let p = s.project("Needs");
    std::fs::write(p.join("Packages/vpm-manifest.json"), json!({ "dependencies": { package: { "version": "1.0.0" } }, "locked": { package: { "version": "1.0.0" } } }).to_string()).unwrap();
}

const ACTIONS: &[Action] = &[
    ("add an existing project", nothing, |s, _| {
        let c = s.project("C");
        page::add(&s.root, &c)?;
        Ok(lists(s, "C"))
    }),
    ("add a listed one again", nothing, |s, _| {
        page::add(&s.root, &s.path("A"))?;
        page::add(&s.root, &s.path("A"))?;
        Ok(lists(s, "A"))
    }),
    ("add what is no project", nothing, |s, _| {
        std::fs::create_dir_all(s.path("Not")).unwrap();
        Ok(page::add(&s.root, &s.path("Not")).is_err() && !lists(s, "Not"))
    }),
    ("add one an old manifest spelling is in", nothing, |s, _| {
        let c = s.project("Old");
        let text = r#"{"dependencies":{},"locked":{},"note":"3.2-3.8","mine":1}"#;
        std::fs::write(c.join("Packages/vpm-manifest.json"), text).unwrap();
        page::add(&s.root, &c)?;
        Ok(std::fs::read_to_string(c.join("Packages/vpm-manifest.json"))? == text)
    }),
    ("remove a listed one", nothing, |s, _| {
        page::remove(&s.root, &s.path("A"))?;
        Ok(!lists(s, "A"))
    }),
    ("remove one whose folder is gone", nothing, |s, _| {
        std::fs::remove_dir_all(s.path("B")).unwrap();
        page::remove(&s.root, &s.path("B"))?;
        Ok(!lists(s, "B"))
    }),
    ("add, remove, add", nothing, |s, _| {
        let c = s.project("C");
        page::add(&s.root, &c)?;
        page::remove(&s.root, &c)?;
        let gone = !lists(s, "C");
        page::add(&s.root, &c)?;
        Ok(gone && lists(s, "C"))
    }),
    ("duplicate", nothing, |s, _| {
        let copy = duplicate(s, "A", "A copy", false)?;
        Ok(lists(s, "A copy") && copy.join("Assets/a.txt").is_file())
    }),
    ("duplicate, stopped", nothing, |s, _| Ok(duplicate(s, "A", "Stopped", true).is_err() && !lists(s, "Stopped"))),
    ("duplicate, its packages nowhere to be had", |s, _| needing(s, "gone.package"), |s, _| {
        let said = format!("{:#}", duplicate(s, "Needs", "Needs copy", false).unwrap_err());
        Ok(lists(s, "Needs copy") && said.contains(&s.path("Needs copy").display().to_string()))
    }),
    ("duplicate, its packages restored", |s, url| {
        with_repository(s, url);
        needing(s, "test.base");
    }, |s, _| {
        let copy = duplicate(s, "Needs", "Needs copy", false)?;
        Ok(lists(s, "Needs copy") && copy.join("Packages/test.base/package.json").is_file())
    }),
    ("restore again after an error", |s, url| {
        with_repository(s, url);
        needing(s, "gone.package");
        assert!(duplicate(s, "Needs", "Needs copy", false).is_err());
    }, |s, _| Ok(block(vpm::change(&s.root, &s.path("Needs copy"), vpm::Change::Resolve)).is_err() && lists(s, "Needs copy"))),
    ("a new project from a template", |s, url| {
        with_repository(s, url);
        needing(s, "test.base");
    }, |s, _| {
        let new = block(templates::create(&s.root, &s.path("Needs"), &s.projects, "New", "2022.3.22f1", false, &AtomicBool::new(false), &Arc::new(Mutex::default())))?;
        Ok(lists(s, "New") && new.join("Packages/test.base/package.json").is_file())
    }),
    ("a new project from a project", nothing, |s, _| {
        block(templates::create(&s.root, &s.path("B"), &s.projects, "From B", "2022.3.22f1", true, &AtomicBool::new(false), &Arc::new(Mutex::default())))?;
        Ok(lists(s, "From B"))
    }),
    ("look at the packages", nothing, |s, _| Ok(block(vpm::view(&s.root, Some(&s.path("A")), false)).is_ok())),
    ("download the repositories again", with_repository, |s, _| Ok(block(vpm::view(&s.root, None, true))?.repos.iter().any(|r| r.id == "test.repo"))),
    ("add a repository", nothing, |s, url| {
        repos(s, url)?;
        repos(s, url)?;
        Ok(block(vpm::view(&s.root, None, false))?.repos.iter().filter(|r| r.id == "test.repo").count() == 1)
    }),
    ("turn one off and on", with_repository, |s, _| {
        let on = |s: &Store| block(vpm::view(&s.root, None, false)).unwrap().repos.iter().find(|r| r.id == "test.repo").unwrap().enabled;
        block(vpm::set_enabled(&s.root, "test.repo", false))?;
        let off = !on(s);
        block(vpm::set_enabled(&s.root, "test.repo", true))?;
        Ok(off && on(s))
    }),
    ("remove a repository", with_repository, |s, url| {
        block(vpm::remove_repo(&s.root, "test.repo"))?;
        let gone = block(vpm::view(&s.root, None, false))?.repos.iter().all(|r| r.id != "test.repo");
        repos(s, url)?;
        Ok(gone && block(vpm::view(&s.root, None, false))?.repos.iter().any(|r| r.id == "test.repo"))
    }),
    ("the repositories of missing packages", nothing, |s, url| {
        // (Its listing needs a header: it is asked for, and refused.)
        let added = block(missing::add(&s.root, &[missing::Addition { name: "Extra".into(), url: format!("{url}-none"), turn_on: None, packages: vec![] }]));
        Ok(added.is_err())
    }),
    ("change packages", with_repository, |s, _| {
        let p = s.path("P");
        let install = |v: &str| vpm::Change::Install { names: vec![("test.avatar".into(), Some(v.into()))], prerelease: false };
        block(vpm::change(&s.root, &p, install("1.0.0")))?;
        block(vpm::change(&s.root, &p, vpm::Change::Plan { install: vec![("test.avatar".into(), "2.0.0".into())], remove: vec![], prerelease: false }))?;
        std::fs::remove_dir_all(p.join("Packages/test.avatar")).unwrap();
        block(vpm::change(&s.root, &p, vpm::Change::Resolve))?;
        let there = p.join("Packages/test.avatar/package.json").is_file() && p.join("Packages/test.base/package.json").is_file();
        block(vpm::change(&s.root, &p, vpm::Change::Remove("test.avatar".into())))?;
        Ok(there && !p.join("Packages/test.avatar").exists())
    }),
];

/// The whole of it: every action on every state. Their folder is what it was, and the action did what it says.
#[test]
fn no_action_changes_a_file_of_theirs_whatever_its_state() {
    offline();
    let server = repository::repository();
    let (mut wrong, mut ran, mut changed) = (vec![], 0, 0);
    for state in STATES {
        for (action, first, go) in ACTIONS {
            let s = make(state);
            let before_first = everything(&s.root);
            let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| first(&s, &server.url)));
            if prepared.is_err() {
                wrong.push(format!("{state}: {action}: its preparation failed"));
                continue;
            }
            // What the preparation does is an action too.
            let before = everything(&s.root);
            let prepared_changed = before != before_first;
            let held = match *state {
                "settings in use" => Some("settings.json"),
                "a database in use" => Some("vcc.liteDb"),
                _ => None,
            };
            held.iter().for_each(|f| s.hold(f));
            let went = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| go(&s, &server.url)));
            s.held.lock().unwrap().clear();
            ran += 1;
            assert!(!page::own(&s.root).join("vpm/Repos/vrc-official.json").exists(), "{state}: {action}: a test reached the network");
            let after = everything(&s.root);
            if after != before || prepared_changed {
                changed += 1;
                let names: std::collections::BTreeSet<_> =
                    before.keys().chain(after.keys()).filter(|k| before.get(*k) != after.get(*k)).map(|k| if k.as_os_str().is_empty() { ".".into() } else { k.display().to_string() }).collect();
                wrong.push(format!("{state}: {action}: CHANGED {names:?}"));
            }
            match went {
                Ok(Ok(true)) => {}
                Ok(Ok(false)) => wrong.push(format!("{state}: {action}: did not do it")),
                Ok(Err(e)) => wrong.push(format!("{state}: {action}: {e:#}")),
                Err(_) => wrong.push(format!("{state}: {action}: panicked")),
            }
        }
    }
    println!("{} states, {} actions: {ran} runs; their files changed in {changed}", STATES.len(), ACTIONS.len());
    assert!(wrong.is_empty(), "{} of {ran} runs (their files changed in {changed}):\n{}", wrong.len(), wrong.join("\n"));
    assert_eq!(ran, STATES.len() * ACTIONS.len());
}

/// What the page showed before the update is what it shows after it, whatever state their files are in.
#[test]
fn the_page_shows_the_same_projects_as_before_the_update() {
    for state in STATES {
        let s = make(state);
        let projects: Vec<_> = ["A", "B", "Found"]
            .iter()
            .map(|n| json!({ "path": s.path(n), "name": n, "unity": "2022.3.22f1", "kind": "Avatars", "modified": 1, "registered": *n != "Found", "missing": false, "avatars": [] }))
            .collect();
        let scan = json!({ "projects": projects, "last_full": 0, "full_scan_ms": 0, "directories": 0, "warnings": [] });
        orbiters_projects::write_json(&page::own(&s.root).join("unity-projects.json"), &scan).unwrap();
        let before = everything(&s.root);
        let held = match *state {
            "settings in use" => Some("settings.json"),
            "a database in use" => Some("vcc.liteDb"),
            _ => None,
        };
        held.iter().for_each(|f| s.hold(f));
        for look in ["first", "again"] {
            let mut now = page::listed(&s.root).unwrap();
            now.sort();
            assert_eq!(now, [s.path("A"), s.path("B")], "{state}: {look}");
        }
        s.held.lock().unwrap().clear();
        assert!(everything(&s.root) == before, "{state}");
    }
}

/// What they list is followed, each way; what was taken off in Orbiters stays off until it is added in Orbiters; what was added in
/// Orbiters stays whatever they do.
#[test]
fn their_list_is_followed_and_ours_is_ours() {
    let s = make("as VCC and ALCOM keep it");
    let c = s.project("C");
    page::add(&s.root, &c).unwrap();
    assert!(lists(&s, "A") && lists(&s, "B") && lists(&s, "C"));
    s.they_remove("B");
    assert!(lists(&s, "A") && !lists(&s, "B") && lists(&s, "C"));
    s.database(&["D"]);
    assert!(lists(&s, "D"));
    page::remove(&s.root, &s.path("A")).unwrap();
    assert!(!lists(&s, "A"));
    // Their folder goes (Creator Companion uninstalled): what it listed last stands, and ours stays.
    std::fs::remove_dir_all(&s.root).unwrap();
    assert!(!lists(&s, "A") && lists(&s, "C") && lists(&s, "D"));
    page::add(&s.root, &s.path("A")).unwrap();
    assert!(lists(&s, "A"));
    assert!(!s.root.exists(), "nothing makes it again");
}

/// Orbiters' list outlives the app, and a damaged one is what it was before its last change.
#[test]
fn our_list_is_kept_and_falls_back() {
    let s = make("not there");
    for n in ["C", "D", "E"] {
        let p = s.project(n);
        page::add(&s.root, &p).unwrap();
    }
    let file = page::own(&s.root).join("unity-gallery.json");
    let kept: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(kept["projects"].as_array().unwrap().len(), 3, "in its file, not in memory");
    for bad in ["", "{", "\0\0\0"] {
        std::fs::write(&file, bad).unwrap();
        assert!(lists(&s, "C") && lists(&s, "D") && !lists(&s, "E"), "{bad:?}: as before the last change");
    }
    let f = s.project("F");
    page::add(&s.root, &f).unwrap();
    assert!(lists(&s, "C") && lists(&s, "D") && lists(&s, "F"));
    // Both copies damaged: what they and the last scan have is still there, and a new list begins; the damaged one is kept.
    std::fs::write(&file, "{").unwrap();
    std::fs::write(file.with_extension("previous.json"), "{").unwrap();
    let scan = json!({ "projects": [{ "path": s.path("D"), "name": "D", "unity": "", "kind": "Unity", "modified": 1, "registered": true, "missing": false, "avatars": [] }], "last_full": 0, "full_scan_ms": 0, "directories": 0, "warnings": [] });
    orbiters_projects::write_json(&page::own(&s.root).join("unity-projects.json"), &scan).unwrap();
    assert!(lists(&s, "D"));
    page::add(&s.root, &s.path("E")).unwrap();
    assert!(lists(&s, "D") && lists(&s, "E") && std::fs::read_dir(page::own(&s.root)).unwrap().flatten().any(|e| e.file_name().to_string_lossy().contains("damaged")));
}

/// CLAUDE.md: a whole copy is on the page also when its list could not be saved (a full disk), and is said.
#[test]
fn a_copy_that_could_not_be_saved_on_the_list_is_on_the_page() {
    let s = make("as VCC and ALCOM keep it");
    let lock = page::own(&s.root).join("unity-gallery.lock");
    std::fs::create_dir_all(&lock).unwrap();
    let said = format!("{:#}", duplicate(&s, "A", "A copy", false).unwrap_err());
    assert!(said.contains(&s.path("A copy").display().to_string()), "{said}");
    assert!(lists(&s, "A copy") && s.path("A copy/Assets/a.txt").is_file());
    std::fs::remove_dir(&lock).unwrap();
    // Saved at the next chance.
    let c = s.project("C");
    page::add(&s.root, &c).unwrap();
    let kept = std::fs::read_to_string(page::own(&s.root).join("unity-gallery.json")).unwrap();
    assert!(kept.contains("A copy"), "{kept}");
}

/// A repository of theirs: its listing and its packages, downloaded already, are used here without asking for them again; taking
/// it out or turning it off in Orbiters leaves it to them, and what they do to it later is followed.
#[test]
fn their_repositories_are_used_and_stay_theirs() {
    let s = make("as VCC and ALCOM keep it");
    let gone = format!("{}/index.json", offline());
    let gone = gone.as_str();
    let listing = json!({ "repo": { "id": "their.repo", "name": "Theirs", "url": gone, "packages": { "their.package": { "versions": { "1.0.0": { "name": "their.package", "version": "1.0.0", "displayName": "Theirs", "url": format!("{}/their.zip", offline()) } } } } } });
    let at = s.root.join("Repos/their.repo.json");
    orbiters_projects::write_json(&at, &listing).unwrap();
    let local = s.root.join("Repos/local.json");
    orbiters_projects::write_json(&local, &json!({ "repo": { "id": "local.repo", "name": "A file", "packages": {} } })).unwrap();
    let zip = repository::archive("their.package", "1.0.0");
    let folder = s.root.join("Repos/their.package");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("vrc-get-their.package-1.0.0.zip"), &zip).unwrap();
    std::fs::write(folder.join("vrc-get-their.package-1.0.0.zip.sha256"), format!("{} x\n", repository::sha256(&zip))).unwrap();
    let mut settings = s.whole();
    settings["userRepos"] = json!([{ "localPath": at, "name": "Theirs", "url": gone, "id": "their.repo" }, { "localPath": local, "name": "A file", "id": "local.repo" }]);
    s.settings(&settings.to_string());
    let before = everything(&s.root);
    let view = block(vpm::view(&s.root, None, false)).unwrap();
    assert!(view.repos.iter().any(|r| r.id == "their.repo" && r.enabled), "{:?}", view.repos);
    assert!(view.packages.iter().any(|p| p.name == "their.package"));
    assert!(view.repos.iter().any(|r| r.id == "local.repo"), "{:?}", view.repos);
    let p = s.project("P");
    block(vpm::change(&s.root, &p, vpm::Change::Install { names: vec![("their.package".into(), None)], prerelease: false })).unwrap();
    assert!(p.join("Packages/their.package/package.json").is_file(), "from what they downloaded: its server is gone");
    block(vpm::set_enabled(&s.root, "their.repo", false)).unwrap();
    assert!(!block(vpm::view(&s.root, None, false)).unwrap().repos.iter().find(|r| r.id == "their.repo").unwrap().enabled);
    block(vpm::set_enabled(&s.root, "their.repo", true)).unwrap();
    block(vpm::remove_repo(&s.root, "their.repo")).unwrap();
    assert!(block(vpm::view(&s.root, None, false)).unwrap().repos.iter().all(|r| r.id != "their.repo"));
    // Added again here: theirs again, nothing asked of its server.
    block(vpm::add_repo(&s.root, gone, BTreeMap::new())).unwrap();
    assert!(block(vpm::view(&s.root, None, false)).unwrap().repos.iter().any(|r| r.id == "their.repo"));
    assert!(everything(&s.root) == before, "theirs is as it was");
    // They take it out: it goes here too. They cannot be read: what they last had stands.
    s.settings("{");
    assert!(block(vpm::view(&s.root, None, false)).unwrap().repos.iter().any(|r| r.id == "their.repo"));
    s.settings(&s.whole().to_string());
    assert!(block(vpm::view(&s.root, None, false)).unwrap().repos.iter().all(|r| r.id != "their.repo"));
}
