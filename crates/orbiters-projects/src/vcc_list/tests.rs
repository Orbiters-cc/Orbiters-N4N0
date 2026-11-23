use super::*;
use crate::unity6::trial::tests::{env, link, Env};

fn run(e: &Env, path: &Path, on: bool) -> anyhow::Result<()> {
    crate::runtime().unwrap().block_on(crate::vpm::list_entry(&e.vcc, path, &Which::of(path), on))
}

fn files(e: &Env) -> [Option<Vec<u8>>; 2] {
    [SETTINGS, DATABASE].map(|f| std::fs::read(e.vcc.join(f)).ok())
}

fn text(e: &Env) -> String {
    std::fs::read_to_string(e.vcc.join(SETTINGS)).unwrap()
}

fn put(e: &Env, text: &str) {
    std::fs::create_dir_all(&e.vcc).unwrap();
    std::fs::write(e.vcc.join(SETTINGS), text).unwrap();
}

fn database(e: &Env) -> LiteDBFile {
    LiteDBFile::parse(&std::fs::read(e.vcc.join(DATABASE)).unwrap()).unwrap()
}

fn paths(e: &Env) -> Vec<String> {
    database(e).get_all(PROJECTS).map(|d| d["Path"].as_str().unwrap().to_string()).collect()
}

/// Nothing there yet: both files are made, as both programs read them; doing it again changes nothing; taken off again.
#[test]
fn a_project_is_listed_once_and_taken_off() {
    let e = env();
    run(&e, &e.source, true).unwrap();
    let first = files(&e);
    run(&e, &e.source, true).unwrap();
    assert_eq!(files(&e), first);
    let shown = e.source.display().to_string();
    let v: serde_json::Value = serde_json::from_str(&text(&e)).unwrap();
    assert_eq!(v[LIST], serde_json::json!([shown]));
    let db = database(&e);
    let doc = db.get_all(PROJECTS).next().unwrap().clone();
    assert_eq!(doc["Path"].as_str(), plain(&e.source).to_str());
    assert_eq!(doc["UnityVersion"].as_str(), Some("2022.3.22f1"));
    assert_eq!(doc["Type"], Value::from(ProjectType::Avatars as i32));
    assert!(doc["Favorite"] == Value::from(false) && doc["CreatedAt"].as_date_time().is_some());
    // Read by the same code VCC's partner reads it with.
    let rt = crate::runtime().unwrap();
    assert_eq!(rt.block_on(crate::vpm::registered(&e.vcc)).unwrap(), [e.source.clone()]);
    // By another spelling: the same folder, once.
    let alias = e.root.join("alias/My avatar");
    std::fs::create_dir_all(alias.parent().unwrap()).unwrap();
    link(&e.source, &alias);
    run(&e, &alias, true).unwrap();
    assert_eq!(files(&e), first);
    run(&e, &alias, false).unwrap();
    assert!(paths(&e).is_empty());
    assert_eq!(serde_json::from_str::<serde_json::Value>(&text(&e)).unwrap()[LIST], serde_json::json!([]));
    crate::unity6::trial::tests::unlink(&alias);
}

/// Every other byte of the settings stays: unknown fields, order, spacing, line ends, a mark.
#[test]
fn every_other_byte_of_the_settings_stays() {
    let e = env();
    let quoted = serde_json::to_string(&e.source.display().to_string()).unwrap();
    let cases = [
        ("{}", format!("{{\"userProjects\":[{quoted}]}}")),
        ("{\"a\":1}", format!("{{\"userProjects\":[{quoted}],\"a\":1}}")),
        ("{\r\n  \"unknown\" : { \"userProjects\": [\"x\"], \"b\": \"}\\\"{\" },\r\n  \"userProjects\" :  [ ] ,\r\n\t\"z\": 1.50\r\n}\r\n", format!("{{\r\n  \"unknown\" : {{ \"userProjects\": [\"x\"], \"b\": \"}}\\\"{{\" }},\r\n  \"userProjects\" :  [\n    {quoted}\n  ] ,\r\n\t\"z\": 1.50\r\n}}\r\n")),
        ("\u{feff}{\"userProjects\":[\"Z:\\\\é\"],\"userProjects2\":null}", format!("\u{feff}{{\"userProjects\":[{quoted},\"Z:\\\\é\"],\"userProjects2\":null}}")),
    ];
    for (before, after) in cases {
        put(&e, before);
        let _ = std::fs::remove_file(e.vcc.join(DATABASE));
        run(&e, &e.source, true).unwrap();
        assert_eq!(text(&e), after);
        // Off again: only the list differs from before.
        run(&e, &e.source, false).unwrap();
        let strip = |t: &str| t.trim_start_matches('\u{feff}').to_string();
        let (mut a, mut b): (serde_json::Value, serde_json::Value) = (serde_json::from_str(&strip(before)).unwrap(), serde_json::from_str(&strip(&text(&e))).unwrap());
        let now = b.as_object_mut().unwrap().remove(LIST).unwrap();
        assert_eq!(now, a.as_object_mut().unwrap().remove(LIST).unwrap_or(serde_json::json!([])));
        assert_eq!(a, b);
    }
}

/// A form not known here: refused, and neither file is written.
#[test]
fn a_form_not_known_changes_neither_file() {
    let e = env();
    run(&e, &e.source, true).unwrap();
    let other = e.root.join("Projects/Other");
    crate::unity6::trial::tests::copy_tree(&e.source, &other);
    let good = files(&e);
    for bad in ["", "[]", "null", "{", "{\"userProjects\":{}}", "{\"userProjects\":[1]}", "{\"userProjects\":[],\"userProjects\":[]}", "{\"userProjects\":[],\"user\\u0050rojects\":[]}", "{\"a\":1,}"] {
        put(&e, bad);
        let before = files(&e);
        assert!(run(&e, &other, true).is_err() || bad.is_empty(), "{bad}");
        assert!(run(&e, &e.source, false).is_err() || bad.is_empty(), "{bad}");
        assert_eq!(files(&e), before, "{bad}");
    }
    std::fs::write(e.vcc.join(SETTINGS), [0xff, 0xfe]).unwrap();
    assert!(run(&e, &other, true).is_err());
    std::fs::write(e.vcc.join(SETTINGS), good[0].as_ref().unwrap()).unwrap();
    let whole = good[1].clone().unwrap();
    let cut = whole[..whole.len() - 100].to_vec();
    for bad in [b"not a database".to_vec(), vec![], vec![0; PAGE], cut, [&whole[..], b"more"].concat()] {
        std::fs::write(e.vcc.join(DATABASE), &bad).unwrap();
        assert!(run(&e, &other, true).is_err() && run(&e, &e.source, false).is_err());
        assert_eq!(std::fs::read(e.vcc.join(DATABASE)).unwrap(), bad);
        assert_eq!(std::fs::read(e.vcc.join(SETTINGS)).unwrap(), *good[0].as_ref().unwrap());
    }
    // Changes it has not taken in yet.
    std::fs::write(e.vcc.join(DATABASE), &whole).unwrap();
    std::fs::write(e.vcc.join(LOG), "x").unwrap();
    assert!(run(&e, &other, true).is_err() && run(&e, &e.source, false).is_err());
    assert_eq!(files(&e), good);
    std::fs::write(e.vcc.join(LOG), "").unwrap();
    run(&e, &other, true).unwrap();
}

/// Another program has a file open, or changed it since it was read: not written over.
#[test]
fn a_file_in_use_or_changed_is_not_written_over() {
    let e = env();
    run(&e, &e.source, true).unwrap();
    #[cfg(windows)]
    let before = files(&e);
    #[cfg(windows)]
    for name in [SETTINGS, DATABASE] {
        use std::os::windows::fs::OpenOptionsExt;
        // Alone, or sharing it with everybody as most programs do.
        for share in [0, 7] {
            let held = std::fs::OpenOptions::new().read(true).share_mode(share).open(e.vcc.join(name)).unwrap();
            assert!(run(&e, &e.source, false).is_err(), "{name} {share}");
            drop(held);
            assert_eq!(files(&e), before, "neither is written, {name} {share}");
        }
    }
    let root = ListRoot::open(&e.vcc).unwrap();
    let now = std::fs::read(e.vcc.join(SETTINGS)).unwrap();
    let replace = |before: Option<&[u8]>, after: &[u8]| root.replace(&[(SETTINGS, before.map(<[u8]>::to_vec), after.to_vec())]);
    assert!(replace(Some(b"another"), b"x").is_err());
    assert!(replace(None, b"x").is_err(), "one that is there where none was");
    assert_eq!(std::fs::read(e.vcc.join(SETTINGS)).unwrap(), now);
    replace(Some(&now), b"{}").unwrap();
    assert_eq!(std::fs::read(e.vcc.join(SETTINGS)).unwrap(), b"{}");
    // Nothing of a run is left beside them.
    let mut names: Vec<_> = std::fs::read_dir(&e.vcc).unwrap().flatten().map(|f| f.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".tmp")).collect();
    names.sort();
    assert!(names.is_empty(), "{names:?}");
    // Through a link there: never.
    let elsewhere = e.root.join("elsewhere.json");
    std::fs::rename(e.vcc.join(SETTINGS), &elsewhere).unwrap();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&elsewhere, e.vcc.join(SETTINGS)).is_ok();
    // Needs a right Windows gives developers only: where it is not given there is nothing to test.
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&elsewhere, e.vcc.join(SETTINGS)).is_ok();
    if linked {
        assert!(run(&e, &e.source, true).is_err());
        assert_eq!(std::fs::read(&elsewhere).unwrap(), b"{}");
    }
}

/// Every other document, collection and field of the database stays.
#[test]
fn every_other_document_of_the_database_stays() {
    let e = env();
    run(&e, &e.source, true).unwrap();
    let mut db = database(&e);
    let mut doc = db.get_all(PROJECTS).next().unwrap().clone();
    doc.insert("Favorite", true);
    doc.insert("Unknown", "kept");
    db.update(PROJECTS, vec![doc.clone()]).unwrap();
    let mut other = Document::new();
    other.insert("x", 1);
    db.insert("unityVersions", vec![other], BsonAutoId::ObjectId).unwrap();
    std::fs::write(e.vcc.join(DATABASE), db.serialize()).unwrap();
    let before = documents(&database(&e));
    let more = e.root.join("Projects/More");
    crate::unity6::trial::tests::copy_tree(&e.source, &more);
    run(&e, &more, true).unwrap();
    assert_eq!(paths(&e).len(), 2);
    run(&e, &e.source, true).unwrap();
    run(&e, &more, false).unwrap();
    assert_eq!(documents(&database(&e)), before);
}

/// An entry is taken out by the folder it names: a like spelling of another folder stays, and a folder that is not there is
/// matched by its very text only.
#[test]
fn an_entry_goes_by_its_folder() {
    let e = env();
    let (a, b) = (e.root.join("Projects/One"), e.root.join("Other/ONE"));
    for p in [&a, &b] {
        crate::unity6::trial::tests::copy_tree(&e.source, p);
        run(&e, p, true).unwrap();
    }
    run(&e, &a, false).unwrap();
    let kept = |p: &Path| plain(p).display().to_string();
    assert_eq!(paths(&e), [kept(&b)]);
    // Not there: another case of its name is not it, and a folder that is there never goes as one that is not.
    let gone = e.root.join("Projects/Gone");
    std::fs::rename(&b, &gone).unwrap();
    run(&e, &gone, true).unwrap();
    std::fs::rename(&gone, e.root.join("taken out")).unwrap();
    run(&e, &PathBuf::from(gone.to_string_lossy().replace("Gone", "GONE")), false).unwrap();
    assert!(paths(&e).contains(&kept(&gone)));
    run(&e, &gone, false).unwrap();
    assert_eq!(paths(&e), [kept(&b)]);
    assert!(!Which::Gone(a.display().to_string()).is(&a.display().to_string()), "it is there");
    // Two folders whose names differ by case only, where the disk keeps them apart.
    let (x, y) = (e.root.join("Case/proj"), e.root.join("Case/PROJ"));
    std::fs::create_dir_all(&x).unwrap();
    if std::fs::create_dir(&y).is_ok() {
        for p in [&x, &y] {
            crate::unity6::trial::tests::copy_tree(&e.source, p);
            run(&e, p, true).unwrap();
        }
        run(&e, &x, false).unwrap();
        assert!(paths(&e).contains(&kept(&y)) && !paths(&e).contains(&kept(&x)));
    }
}

/// What the list says of a project is read from its files.
#[test]
fn the_kind_of_a_project_is_read_from_its_files() {
    let e = env();
    let kind = |vpm: &str, upm: &str| {
        std::fs::write(e.source.join("Packages/vpm-manifest.json"), vpm).unwrap();
        std::fs::write(e.source.join("Packages/manifest.json"), upm).unwrap();
        facts(&e.source).kind as i32
    };
    for (vpm, upm, wanted) in [
        (r#"{"locked":{"com.vrchat.avatars":{},"com.vrchat.base":{}}}"#, "{}", ProjectType::Avatars),
        (r#"{"locked":{"com.vrchat.worlds":{}}}"#, "{}", ProjectType::Worlds),
        (r#"{"dependencies":{"x":{}}}"#, "{}", ProjectType::VpmStarter),
        ("{}", r#"{"dependencies":{"com.vrchat.avatars":"1"}}"#, ProjectType::UpmAvatars),
        ("{}", r#"{"dependencies":{"com.vrchat.worlds":"1"}}"#, ProjectType::UpmWorlds),
        ("{}", r#"{"dependencies":{"com.vrchat.base":"1"}}"#, ProjectType::UpmStarter),
        ("[]", "nonsense", ProjectType::Unknown),
    ] {
        assert_eq!(kind(vpm, upm), wanted as i32, "{vpm} {upm}");
    }
}
