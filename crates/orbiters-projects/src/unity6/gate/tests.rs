use super::super::journal::Place;
use super::*;

fn real(dir: &Path) -> PathBuf {
    PathBuf::from(std::fs::canonicalize(dir).unwrap().to_string_lossy().trim_start_matches(r"\\?\").to_string())
}

fn trial(root: &Path) -> (PathBuf, TrialRoot) {
    let made = Destination::check(root, "T", &[]).unwrap().make().unwrap();
    let place = Place { path: made.path.clone(), id: Some(made.id), name: "T".into() };
    let path = made.path.clone();
    drop(made);
    (path, TrialRoot::open(&place).unwrap())
}

/// The proof is the folder's identity: another folder, a link, or no identity is no proof.
#[test]
fn only_the_recorded_folder_is_a_trial_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = real(temp.path());
    let (path, t) = trial(&root);
    let place = Place { path: path.clone(), id: Some(t.id), name: "T".into() };
    drop(t);
    assert!(TrialRoot::open(&Place { id: None, ..place.clone() }).is_err());
    std::fs::rename(&path, root.join("Moved")).unwrap();
    assert!(TrialRoot::open(&place).is_err(), "nothing there");
    std::fs::create_dir(&path).unwrap();
    assert!(TrialRoot::open(&place).is_err(), "another folder");
    std::fs::remove_dir(&path).unwrap();
    #[cfg(windows)]
    {
        junction::create(root.join("Moved"), &path).unwrap();
        assert!(TrialRoot::open(&place).is_err(), "a link to it");
        junction::delete(&path).unwrap();
        let _ = std::fs::remove_dir(&path);
    }
    std::fs::rename(root.join("Moved"), &path).unwrap();
    assert!(TrialRoot::open(&place).is_ok());
}

/// Every write goes below the root: a link at any place of the path is refused, whatever it leads to, and nothing is made
/// through it; a file that is there is never written over.
#[cfg(windows)]
#[test]
fn nothing_is_written_through_a_link_or_over_a_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = real(temp.path());
    let outside = root.join("outside");
    std::fs::create_dir_all(outside.join("in")).unwrap();
    std::fs::write(outside.join("file"), "theirs").unwrap();
    let (path, t) = trial(&root);
    t.make_dir(Path::new("a")).unwrap();
    std::fs::write(path.join("a/there"), "there").unwrap();
    junction::create(&outside, path.join("a/link")).unwrap();
    // Another place of the trial itself is a link all the same.
    junction::create(path.join("a"), path.join("self")).unwrap();
    for rel in ["a/link/new", "a/link/in/new", "a/link/file", "a/link", "self/new", "self", "a/there", "a/THERE"] {
        assert!(t.make_file(Path::new(rel)).is_err(), "{rel}");
        assert!(t.make_dir(Path::new(rel)).is_err(), "{rel}");
    }
    for rel in ["a/link/file", "self/there", "a/link/in"] {
        assert!(t.file(Path::new(rel)).is_err() || rel == "x", "{rel}");
        assert!(t.entries(Path::new(rel)).is_err(), "{rel}");
    }
    let id = crate::owned::id_of(&outside.join("file")).unwrap();
    assert!(t.remove(Path::new("a/link/file"), id, false).is_err());
    // A link is never removed as a folder of the trial.
    let link = crate::owned::id_of(&path.join("a/link")).unwrap();
    assert!(!t.remove(Path::new("a/link"), link, true).unwrap_or(false));
    assert_eq!(std::fs::read_to_string(outside.join("file")).unwrap(), "theirs");
    assert_eq!(std::fs::read_dir(outside.join("in")).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 2);
    assert_eq!(std::fs::read_to_string(path.join("a/there")).unwrap(), "there");
    junction::delete(path.join("a/link")).unwrap();
    junction::delete(path.join("self")).unwrap();
}

/// Names that Windows reads as something else are refused.
#[test]
fn only_plain_names_are_taken() {
    let temp = tempfile::tempdir().unwrap();
    let (path, t) = trial(&real(temp.path()));
    for rel in ["..", "../x", "a/../../x", "/x", ".", ""] {
        assert!(t.make_file(Path::new(rel)).is_err(), "{rel:?}");
    }
    #[cfg(windows)]
    for rel in ["x:stream", "a:b/c", "x.", "x ", r"C:\x", r"\x"] {
        assert!(t.make_file(Path::new(rel)).is_err(), "{rel:?}");
    }
    assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
}

/// While its root is held, the trial (and what is above it) cannot be put elsewhere (Windows).
#[cfg(windows)]
#[test]
fn a_held_root_stays_where_it_is() {
    let temp = tempfile::tempdir().unwrap();
    let root = real(temp.path());
    std::fs::create_dir(root.join("up")).unwrap();
    let (path, t) = trial(&root.join("up"));
    t.make_dir(Path::new("a")).unwrap();
    assert!(std::fs::rename(&path, root.join("up/else")).is_err());
    assert!(std::fs::rename(root.join("up"), root.join("else")).is_err());
    // And the original, while it is read.
    let read = ReadRoot::open(&root.join("up")).unwrap();
    drop(t);
    assert!(std::fs::rename(root.join("up"), root.join("else")).is_err());
    drop(read);
    std::fs::rename(root.join("up"), root.join("else")).unwrap();
}

/// What replaced the file or folder made is never removed in its place.
#[test]
fn only_what_was_made_is_removed() {
    let temp = tempfile::tempdir().unwrap();
    let (path, t) = trial(&real(temp.path()));
    let id = t.make_file(Path::new("f")).unwrap().id;
    std::fs::remove_file(path.join("f")).unwrap();
    std::fs::write(path.join("f"), "theirs").unwrap();
    assert!(!t.remove(Path::new("f"), id, false).unwrap());
    assert_eq!(std::fs::read_to_string(path.join("f")).unwrap(), "theirs");
    let d = t.make_dir(Path::new("d")).unwrap();
    std::fs::write(path.join("d/x"), "x").unwrap();
    assert!(t.remove(Path::new("d"), d, true).is_err(), "not empty");
    std::fs::remove_file(path.join("d/x")).unwrap();
    assert!(t.remove(Path::new("d"), d, true).unwrap());
}

/// App data: published whole, appended, locked, and never through a link.
#[test]
fn app_data_is_written_in_place_and_never_through_a_link() {
    let temp = tempfile::tempdir().unwrap();
    let root = real(temp.path());
    let app = AppRoot::open(&root.join("app/deep")).unwrap();
    app.replace_json(Path::new("x.json"), &[1, 2]).unwrap();
    app.replace_json(Path::new("x.json"), &[3]).unwrap();
    assert_eq!(app.read(Path::new("x.json")).unwrap(), b"[\n  3\n]");
    app.append(Path::new("sub/log"), b"a").unwrap();
    app.append(Path::new("sub/log"), b"b").unwrap();
    assert_eq!(app.read(Path::new("sub/log")).unwrap(), b"ab");
    assert!(app.write_new(Path::new("sub/log"), b"c").is_err());
    let held = app.lock(Path::new("l")).unwrap();
    assert!(app.try_lock(Path::new("l")).unwrap().is_none());
    drop(held);
    assert!(app.try_lock(Path::new("l")).unwrap().is_some());
    assert_eq!(app.files(Path::new("sub")), ["log"]);
    app.remove_file(Path::new("sub/log")).unwrap();
    assert!(app.files(Path::new("sub")).is_empty());
    #[cfg(windows)]
    {
        std::fs::create_dir(root.join("outside")).unwrap();
        junction::create(root.join("outside"), root.join("app/deep/link")).unwrap();
        assert!(app.write_new(Path::new("link/x"), b"x").is_err());
        assert!(app.append(Path::new("link/x"), b"x").is_err());
        assert_eq!(std::fs::read_dir(root.join("outside")).unwrap().count(), 0);
        junction::delete(root.join("app/deep/link")).unwrap();
    }
    assert!(app.write_new(Path::new("../x"), b"x").is_err());
}

/// Every other module of the feature is free of the filesystem's writing calls: what writes goes through the gate. Their
/// tests (after the marker) stand in for other programs and may.
#[test]
fn no_other_module_writes() {
    let here: &[(&str, &str)] = &[
        ("authority.rs", include_str!("../authority.rs")),
        ("budget.rs", include_str!("../budget.rs")),
        ("compat.rs", include_str!("../compat.rs")),
        ("editors.rs", include_str!("../editors.rs")),
        ("journal.rs", include_str!("../journal.rs")),
        ("matrix.rs", include_str!("../matrix.rs")),
        ("mod.rs", include_str!("../mod.rs")),
        ("outcome.rs", include_str!("../outcome.rs")),
        ("payload.rs", include_str!("../payload.rs")),
        ("standing.rs", include_str!("../standing.rs")),
        ("trial.rs", include_str!("../trial.rs")),
        ("vcc.rs", include_str!("../vcc.rs")),
    ];
    // Every file of the feature is in the list.
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/unity6");
    if folder.is_dir() {
        for f in std::fs::read_dir(folder).unwrap().flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            if name.ends_with(".rs") && !name.ends_with("_tests.rs") {
                assert!(here.iter().any(|(n, _)| *n == name), "{name} is not looked at");
            }
        }
    }
    const WRITES: &[&str] = &[
        "fs::write", "fs::create_dir", "fs::remove_", "fs::rename", "fs::copy", "fs::set_permissions", "fs::hard_link", "fs::soft_link", "symlink_dir", "symlink_file", "os::unix::fs::symlink", "File::create",
        "OpenOptions", "File::options", ".set_len(", ".set_modified(", ".set_times(", ".set_permissions(", "write_json", "junction::create", "junction::delete", "tokio::fs", "fs_extra", "DirBuilder",
        "MoveFile", "DeleteFile", "CreateFile", "copy_keeping", "copy_local_packages", "templates::copy", "storage::", "vpm::register", "vpm::change", ".persist(", "libc::", "NtCreateFile",
    ];
    for (name, text) in here {
        let text = text.replace("
", "
");
        let production = text.split("#[cfg(test)]\npub(crate) mod tests").next().unwrap().split("#[cfg(test)]\nmod tests").next().unwrap();
        for (n, line) in production.lines().enumerate() {
            let code = line.split("//").next().unwrap();
            for w in WRITES {
                assert!(!code.contains(w), "{name}:{}: {w} outside the gate: {line}", n + 1);
            }
        }
    }
}
