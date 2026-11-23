//! Adding a project to the list Creator Companion and ALCOM share costs no other project its entry, and no other program its
//! save. Every store here is a generated one in a temporary folder; what reconciles it is what ALCOM's released versions do each
//! time they show their list (gui-v1.1.10: migrate, dedup, normalize, load_from_db, save both).

use orbiters_projects::vpm;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use vrc_get_vpm::environment::{Settings, VccDatabaseConnection};
use vrc_get_vpm::io::{DefaultEnvironmentIo, DefaultProjectIo};
use vrc_get_vpm::UnityProject;

struct Store {
    _temp: tempfile::TempDir,
    root: PathBuf,
    projects: PathBuf,
}

impl Store {
    /// A store whose database has the projects `A` (a favourite) and `B`, and whose settings are `settings`.
    fn new(settings: Option<Value>) -> Store {
        let temp = tempfile::tempdir().unwrap();
        let s = Store { root: temp.path().join("VRChatCreatorCompanion"), projects: temp.path().join("Projects"), _temp: temp };
        std::fs::create_dir_all(&s.root).unwrap();
        block(async {
            let io = s.io();
            let mut db = VccDatabaseConnection::connect(&io).await.unwrap();
            for name in ["A", "B"] {
                db.add_project(&UnityProject::load(DefaultProjectIo::new(s.project(name).into())).await.unwrap()).await.unwrap();
            }
            let mut a = db.find_project(s.path("A").to_str().unwrap()).unwrap().unwrap();
            a.set_favorite(true);
            db.update_project(&a);
            db.save(&io).await.unwrap();
        });
        if let Some(v) = settings {
            s.write(&v.to_string());
        }
        s
    }
    fn io(&self) -> DefaultEnvironmentIo {
        DefaultEnvironmentIo::new(self.root.clone().into())
    }
    fn path(&self, name: &str) -> PathBuf {
        self.projects.join(name)
    }
    fn project(&self, name: &str) -> PathBuf {
        let p = self.path(name);
        std::fs::create_dir_all(p.join("ProjectSettings")).unwrap();
        std::fs::create_dir_all(p.join("Packages")).unwrap();
        std::fs::write(p.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
        std::fs::write(p.join("Packages/vpm-manifest.json"), r#"{"dependencies":{},"locked":{}}"#).unwrap();
        std::fs::write(p.join("Packages/manifest.json"), r#"{"dependencies":{}}"#).unwrap();
        p
    }
    fn write(&self, text: &str) {
        std::fs::write(self.root.join("settings.json"), text).unwrap();
    }
    fn settings(&self) -> Option<Value> {
        std::fs::read(self.root.join("settings.json")).ok().map(|b| serde_json::from_slice(&b).unwrap())
    }
    fn files(&self) -> [Option<Vec<u8>>; 2] {
        ["settings.json", "vcc.liteDb"].map(|f| std::fs::read(self.root.join(f)).ok())
    }
    /// The names in the database, and which is a favourite.
    fn database(&self) -> Vec<(String, bool)> {
        block(async {
            let db = VccDatabaseConnection::connect(&self.io()).await.unwrap();
            let mut all: Vec<_> = db.get_projects().iter().map(|p| (Path::new(p.path().unwrap()).file_name().unwrap().to_string_lossy().into_owned(), p.favorite())).collect();
            all.sort();
            all
        })
    }
    /// What ALCOM does when it shows its list.
    fn alcom_shows_its_list(&self) {
        block(async {
            let io = self.io();
            let mut settings = Settings::load(&io).await.unwrap();
            let mut db = VccDatabaseConnection::connect(&io).await.unwrap();
            db.migrate(&settings, &io).await.unwrap();
            db.dedup_projects();
            db.normalize_path();
            settings.load_from_db(&db).unwrap();
            db.save(&io).await.unwrap();
            settings.save(&io).await.unwrap();
        });
    }
    fn register(&self, name: &str, add: bool) -> anyhow::Result<()> {
        let path = if add { self.project(name) } else { self.path(name) };
        block(vpm::register(&self.root, &path, add))
    }
}

fn block<T>(f: impl std::future::Future<Output = T>) -> T {
    orbiters_projects::runtime().unwrap().block_on(f)
}

fn named(list: &[(&str, bool)]) -> Vec<(String, bool)> {
    list.iter().map(|(n, f)| (n.to_string(), *f)).collect()
}

/// The list kept in the database alone (settings without `userProjects`, with null for it, empty, missing): adding a project
/// keeps the others, their favourites and dates, whoever reconciles the two afterwards.
#[test]
fn projects_only_the_database_has_stay() {
    for (what, text) in [
        ("no list", Some(r#"{"showPrereleasePackages":true}"#)),
        ("null", Some(r#"{"userProjects":null,"showPrereleasePackages":true}"#)),
        ("a blank file", Some("")),
        ("a file of zeros", Some("\0\0\0\0")),
        ("no file", None),
    ] {
        let s = Store::new(None);
        if let Some(text) = text {
            s.write(text);
        }
        assert_eq!(s.database(), named(&[("A", true), ("B", false)]), "{what}");
        s.register("C", true).unwrap();
        assert_eq!(s.database(), named(&[("A", true), ("B", false), ("C", false)]), "{what}: before anything reconciles");
        s.alcom_shows_its_list();
        assert_eq!(s.database(), named(&[("A", true), ("B", false), ("C", false)]), "{what}: after ALCOM showed its list");
    }
}

/// The list in both: as before, once each.
#[test]
fn projects_in_both_stay_and_none_is_listed_twice() {
    let s = Store::new(None);
    let list = json!([s.path("A"), s.path("B")]);
    s.write(&json!({ "userProjects": list, "unknown": { "kept": [1] } }).to_string());
    for _ in 0..2 {
        s.register("C", true).unwrap();
    }
    let now = s.settings().unwrap();
    assert_eq!(now["userProjects"].as_array().unwrap().len(), 3, "{now}");
    assert_eq!(now["unknown"], json!({ "kept": [1] }));
    s.alcom_shows_its_list();
    assert_eq!(s.database(), named(&[("A", true), ("B", false), ("C", false)]));
    s.register("C", false).unwrap();
    s.alcom_shows_its_list();
    assert_eq!(s.database(), named(&[("A", true), ("B", false)]));
}

/// Taking one off a list kept in the database alone leaves the others, and makes no list.
#[test]
fn taking_one_off_leaves_the_others() {
    let s = Store::new(Some(json!({})));
    s.register("B", false).unwrap();
    assert_eq!(s.settings().unwrap().get("userProjects"), None);
    s.alcom_shows_its_list();
    assert_eq!(s.database(), named(&[("A", true)]));
}

/// Another program saves while this one is adding: here ALCOM, which holds the database's lock during its own change, so that
/// adding waits for it. What it saved stays.
#[test]
fn a_save_made_meanwhile_stays() {
    let s = Store::new(None);
    s.write(&json!({ "userProjects": [s.path("A"), s.path("B")], "showPrereleasePackages": false }).to_string());
    s.project("C");
    s.project("D");
    let (locked, held) = std::sync::mpsc::channel();
    let io = s.io();
    let (settings, d, list) = (s.root.join("settings.json"), s.path("D"), json!([s.path("D"), s.path("A"), s.path("B")]));
    let other = std::thread::spawn(move || {
        block(async {
            let mut db = VccDatabaseConnection::connect(&io).await.unwrap();
            locked.send(()).unwrap();
            // Time for the other one to begin and wait.
            std::thread::sleep(std::time::Duration::from_millis(1500));
            db.add_project(&UnityProject::load(DefaultProjectIo::new(d.into())).await.unwrap()).await.unwrap();
            db.save(&io).await.unwrap();
            std::fs::write(&settings, json!({ "userProjects": list, "showPrereleasePackages": true, "theirs": 1 }).to_string()).unwrap();
        })
    });
    held.recv().unwrap();
    s.register("C", true).unwrap();
    other.join().unwrap();
    let now = s.settings().unwrap();
    assert_eq!((&now["showPrereleasePackages"], &now["theirs"]), (&json!(true), &json!(1)), "their settings: {now}");
    s.alcom_shows_its_list();
    assert_eq!(s.database(), named(&[("A", true), ("B", false), ("C", false), ("D", false)]));
}

/// Another program replaces a file between it being read and being written (a save that takes no lock): that save stays, or this
/// one is refused, and nothing is half done.
#[cfg(windows)]
#[test]
fn a_file_in_use_refuses_and_changes_neither() {
    use std::os::windows::fs::OpenOptionsExt;
    let s = Store::new(None);
    s.write(&json!({ "userProjects": [s.path("A"), s.path("B")] }).to_string());
    s.project("C");
    let before = s.files();
    for name in ["settings.json", "vcc.liteDb"] {
        // Open as a program in the middle of writing it has it.
        let held = std::fs::OpenOptions::new().read(true).write(true).share_mode(1).open(s.root.join(name)).unwrap();
        assert!(s.register("C", true).is_err(), "{name}");
        drop(held);
        assert_eq!(s.files(), before, "{name}");
    }
    s.register("C", true).unwrap();
}

/// A file that is not in a form known here is never written over.
#[test]
fn what_cannot_be_read_is_never_written_over() {
    let s = Store::new(None);
    s.write(&json!({ "userProjects": [s.path("A"), s.path("B")] }).to_string());
    s.project("C");
    let good = s.files();
    for bad in ["{", "[]", r#"{"userProjects":{}}"#, r#"{"userProjects":[1]}"#, r#"{"userProjects":[],"userProjects":[]}"#] {
        s.write(bad);
        let before = s.files();
        assert!(s.register("C", true).is_err(), "{bad}");
        assert!(s.register("A", false).is_err(), "{bad}");
        assert_eq!(s.files(), before, "{bad}");
    }
    std::fs::write(s.root.join("settings.json"), good[0].as_ref().unwrap()).unwrap();
    let whole = good[1].clone().unwrap();
    for bad in [whole[..whole.len() - 100].to_vec(), b"short".to_vec(), vec![]] {
        std::fs::write(s.root.join("vcc.liteDb"), &bad).unwrap();
        assert!(s.register("C", true).is_err(), "{} bytes", bad.len());
        assert_eq!(std::fs::read(s.root.join("vcc.liteDb")).unwrap(), bad);
        assert_eq!(std::fs::read(s.root.join("settings.json")).unwrap(), *good[0].as_ref().unwrap());
    }
    // Changes the database has not taken in yet.
    std::fs::write(s.root.join("vcc.liteDb"), &whole).unwrap();
    std::fs::write(s.root.join("vcc-log.liteDb"), "x").unwrap();
    assert!(s.register("C", true).is_err());
    assert_eq!(s.files(), good);
}

/// Every other byte of the settings stays as it was written.
#[test]
fn every_other_byte_of_the_settings_stays() {
    let s = Store::new(None);
    let q = |n: &str| serde_json::to_string(s.path(n).to_str().unwrap()).unwrap();
    let before = format!("{{\r\n\t\"z\" : 1.50,\r\n  \"userProjects\": [{}, {}],\r\n  \"a\": \"}}\\\"{{\"\r\n}}\r\n", q("A"), q("B"));
    s.write(&before);
    s.register("C", true).unwrap();
    let after = std::fs::read_to_string(s.root.join("settings.json")).unwrap();
    let (head, rest) = after.split_once("\"userProjects\": [").unwrap();
    assert_eq!(head, "{\r\n\t\"z\" : 1.50,\r\n  ");
    assert!(rest.ends_with("],\r\n  \"a\": \"}\\\"{\"\r\n}\r\n"), "{after}");
    s.register("C", false).unwrap();
    let back: Value = serde_json::from_slice(&std::fs::read(s.root.join("settings.json")).unwrap()).unwrap();
    assert_eq!(back, serde_json::from_str::<Value>(&before).unwrap());
}
