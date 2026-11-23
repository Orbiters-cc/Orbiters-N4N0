use super::super::journal::{self, Phase, Place};
use super::*;
use crate::unity6::trial::tests::{env, link, run, unlink};

fn real(dir: &Path) -> PathBuf {
    PathBuf::from(std::fs::canonicalize(dir).unwrap().to_string_lossy().trim_start_matches(r"\\?\").to_string())
}

fn trial(root: &Path) -> (PathBuf, NewFolder, WriteRoot) {
    let made = Destination::check(root, "T", &[], Parents::Exist).unwrap().make().unwrap();
    let written = made.root().unwrap();
    (made.path.clone(), made, written)
}

/// Third verification R9: what is written through comes from a folder just made, or from the journal, where the job, the part
/// the folder has in it, its phase and its identity say so. No folder chosen by a caller can stand for it, the original least.
#[test]
fn only_a_folder_made_now_or_recorded_is_written_in() {
    let e = env();
    let job = run(&e).unwrap();
    let of = |role| WriteRoot::of_job(&e.app, &job.id, role);
    assert!(of(Role::Trial).is_ok());
    assert!(of(Role::Migrated).is_err(), "no such folder in it");
    assert!(WriteRoot::of_job(&e.app, "nobody", Role::Trial).is_err() && WriteRoot::of_job(&e.root, &job.id, Role::Trial).is_err());
    // Another folder at its path, a link to it, no identity recorded.
    let away = e.root.join("away");
    std::fs::rename(&job.trial.path, &away).unwrap();
    assert!(of(Role::Trial).is_err(), "nothing there");
    std::fs::create_dir(&job.trial.path).unwrap();
    assert!(of(Role::Trial).is_err(), "another folder");
    std::fs::remove_dir(&job.trial.path).unwrap();
    link(&away, &job.trial.path);
    assert!(of(Role::Trial).is_err(), "a link to it");
    unlink(&job.trial.path);
    std::fs::rename(&away, &job.trial.path).unwrap();
    assert!(of(Role::Trial).is_ok());
    let kept = std::fs::read(e.app.join(journal::JOURNAL)).unwrap();
    let with = |f: &dyn Fn(&mut journal::Job)| {
        let changed = journal::set(&e.app, &job.id, f).is_ok();
        let r = (of(Role::Trial).is_ok(), of(Role::Migrated).is_ok());
        std::fs::write(e.app.join(journal::JOURNAL), &kept).unwrap();
        (changed, r)
    };
    assert_eq!(with(&|j| j.trial.id = None), (true, (false, false)));
    assert_eq!(with(&|j| j.phase = Phase::Planned), (true, (false, false)));
    // A record that names the original for the trial, by its path or by its identity, is not taken (or not even written).
    let source = Place::of(&e.source).unwrap();
    for role in 0..2 {
        let s = source.clone();
        let (_, r) = with(&move |j| match role {
            0 => j.trial = s.clone(),
            _ => j.migrated = Some(s.clone()),
        });
        assert!(!r.1 && (role != 0 || WriteRoot::of_job(&e.app, &job.id, Role::Trial).unwrap().id != source.id.unwrap()));
    }
    // The phase each part is written in.
    let place = job.trial.clone();
    assert!(with(&|j| j.migrated = Some(place.clone())).1 .1);
    assert!(!with(&|j| {
        j.migrated = Some(place.clone());
        j.phase = Phase::Verified;
    })
    .1
     .1);
    // A line that cannot be read: nothing is written.
    std::fs::write(e.app.join(journal::JOURNAL), [&kept[..], b"{torn"].concat()).unwrap();
    assert!(of(Role::Trial).is_err());
    std::fs::write(e.app.join(journal::JOURNAL), &kept).unwrap();
    // The original moved onto the trial's place, the record of it lost its source: still never.
    assert!(!with(&|j| j.source.id = None).1 .0);
}

/// Folders made on the way to a new one (a duplicate's) are each made below the one held, and go with it, empty.
#[test]
fn folders_made_on_the_way_are_held_and_go_with_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = real(temp.path());
    let at = root.join("a/b");
    assert!(matches!(Destination::check(&at, "T", &[], Parents::Exist), Err(Refused::NoFolder(_))));
    let made = Destination::check(&at, "T", &[], Parents::Make).unwrap().make().unwrap();
    assert!(made.path == at.join("T") && made.path.is_dir());
    #[cfg(windows)]
    assert!(std::fs::rename(root.join("a"), root.join("z")).is_err(), "held");
    assert!(made.remove().unwrap());
    assert!(!root.join("a").exists());
    // What somebody put there stays, and so do the folders above it.
    let made = Destination::check(&at, "T", &[], Parents::Make).unwrap().make().unwrap();
    std::fs::write(root.join("a/theirs"), "x").unwrap();
    assert!(made.remove().unwrap());
    assert!(!root.join("a/b").exists() && root.join("a/theirs").is_file());
    // Names on the way that are not names, a place put there meanwhile.
    assert!(Destination::check(&root.join("a./b "), "T", &[], Parents::Make).is_err());
    let late = Destination::check(&root.join("c"), "T", &[], Parents::Make).unwrap();
    std::fs::create_dir(root.join("d")).unwrap();
    link(&root.join("d"), &root.join("c"));
    assert!(late.make().is_err());
    assert_eq!(std::fs::read_dir(root.join("d")).unwrap().count(), 0);
    unlink(&root.join("c"));
}

/// Every write goes below the root: a link at any place of the path is refused, whatever it leads to, and nothing is made
/// through it; a file that is there is never written over.
#[test]
fn nothing_is_written_through_a_link_or_over_a_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = real(temp.path());
    let outside = root.join("outside");
    std::fs::create_dir_all(outside.join("in")).unwrap();
    std::fs::write(outside.join("file"), "theirs").unwrap();
    let (path, _new, t) = trial(&root);
    drop(t.make_dir(Path::new("a")).unwrap());
    std::fs::write(path.join("a/there"), "there").unwrap();
    link(&outside, &path.join("a/link"));
    // Another place of the trial itself is a link all the same.
    link(&path.join("a"), &path.join("self"));
    let mut refused = vec!["a/link/new", "a/link/in/new", "a/link/file", "a/link", "self/new", "self", "a/there"];
    if cfg!(windows) {
        refused.push("a/THERE");
    }
    for rel in refused {
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
    unlink(&path.join("a/link"));
    unlink(&path.join("self"));
}

/// Names that Windows reads as something else are refused.
#[test]
fn only_plain_names_are_taken() {
    let temp = tempfile::tempdir().unwrap();
    let (path, _new, t) = trial(&real(temp.path()));
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
    let (path, new, t) = trial(&root.join("up"));
    drop(t.make_dir(Path::new("a")).unwrap());
    assert!(std::fs::rename(&path, root.join("up/else")).is_err());
    assert!(std::fs::rename(root.join("up"), root.join("else")).is_err());
    // And the original, while it is read.
    let read = ReadRoot::open(&root.join("up")).unwrap();
    drop((t, new));
    assert!(std::fs::rename(root.join("up"), root.join("else")).is_err());
    drop(read);
    std::fs::rename(root.join("up"), root.join("else")).unwrap();
}

/// What replaced the file or folder made is never removed in its place.
#[test]
fn only_what_was_made_is_removed() {
    let temp = tempfile::tempdir().unwrap();
    let (path, _new, t) = trial(&real(temp.path()));
    let id = t.make_file(Path::new("f")).unwrap().id;
    std::fs::remove_file(path.join("f")).unwrap();
    std::fs::write(path.join("f"), "theirs").unwrap();
    assert!(!t.remove(Path::new("f"), id, false).unwrap());
    assert_eq!(std::fs::read_to_string(path.join("f")).unwrap(), "theirs");
    let (d, held) = t.make_dir(Path::new("d")).unwrap();
    #[cfg(windows)]
    assert!(std::fs::rename(path.join("d"), path.join("e")).is_err(), "held");
    drop(held);
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
    std::fs::create_dir(root.join("outside")).unwrap();
    link(&root.join("outside"), &root.join("app/deep/link"));
    assert!(app.write_new(Path::new("link/x"), b"x").is_err());
    assert!(app.append(Path::new("link/x"), b"x").is_err());
    assert_eq!(std::fs::read_dir(root.join("outside")).unwrap().count(), 0);
    unlink(&root.join("app/deep/link"));
    // A folder of its own goes with what is in it, and a link in it goes as a link.
    std::fs::write(root.join("outside/theirs"), "x").unwrap();
    app.write_new(Path::new("stage/one/a/b.txt"), b"x").unwrap();
    link(&root.join("outside"), &root.join("app/deep/stage/one/link"));
    assert_eq!(app.folders(Path::new("stage")), ["one"]);
    app.remove_tree(Path::new("stage/one")).unwrap();
    assert!(!root.join("app/deep/stage/one").exists() && root.join("outside/theirs").is_file());
    assert!(app.write_new(Path::new("../x"), b"x").is_err());
}

/// The source test. Nothing outside the gate writes: every other file of the feature, of the copier of projects and of VCC's
/// list is read as Rust (comments, texts and `#[cfg(test)]` items left out), each path in it is resolved through its imports,
/// and only what is known to read is let through. Anything else is named, whatever it is called by: a wrapper of the
/// filesystem, an archive's extraction, VPM's own project loader, a method that writes.
mod source {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    #[derive(Clone, Debug, PartialEq)]
    pub enum Token {
        Word(String),
        Sign(char),
    }
    use Token::*;

    /// The tokens of `text`, without comments; a text or a character is one `"`.
    pub fn tokens(text: &str) -> Vec<Token> {
        let c: Vec<char> = text.chars().collect();
        let (mut out, mut i) = (vec![], 0);
        let word = |c: char| c.is_alphanumeric() || c == '_';
        while i < c.len() {
            let next = c.get(i + 1).copied().unwrap_or(' ');
            match c[i] {
                '/' if next == '/' => {
                    while i < c.len() && c[i] != '\n' {
                        i += 1;
                    }
                }
                '/' if next == '*' => {
                    let mut depth = 0;
                    loop {
                        if i + 1 >= c.len() {
                            return out;
                        }
                        match (c[i], c[i + 1]) {
                            ('/', '*') => depth += 1,
                            ('*', '/') => depth -= 1,
                            _ => {
                                i += 1;
                                continue;
                            }
                        }
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    }
                }
                'r' | 'b' | 'c' if raw(&c, i).is_some() => {
                    i = raw(&c, i).unwrap();
                    out.push(Sign('"'));
                }
                'b' | 'c' if next == '"' => i += 1,
                '"' => {
                    i += 1;
                    while i < c.len() && c[i] != '"' {
                        i += if c[i] == '\\' { 2 } else { 1 };
                    }
                    i += 1;
                    out.push(Sign('"'));
                }
                '\'' => {
                    // A character, or a lifetime.
                    if next == '\\' {
                        i += 3;
                        while i < c.len() && c[i] != '\'' {
                            i += 1;
                        }
                        i += 1;
                        out.push(Sign('"'));
                    } else if c.get(i + 2) == Some(&'\'') {
                        i += 3;
                        out.push(Sign('"'));
                    } else {
                        i += 1;
                        while i < c.len() && word(c[i]) {
                            i += 1;
                        }
                    }
                }
                x if word(x) => {
                    let from = i;
                    while i < c.len() && word(c[i]) {
                        i += 1;
                    }
                    let text: String = c[from..i].iter().collect();
                    // A raw name is that name.
                    if text == "r" && c.get(i) == Some(&'#') {
                        i += 1;
                        continue;
                    }
                    out.push(Word(text));
                }
                x if x.is_whitespace() => i += 1,
                x => {
                    out.push(Sign(x));
                    i += 1;
                }
            }
        }
        out
    }

    /// Where the raw text that begins at `i` ends.
    fn raw(c: &[char], mut i: usize) -> Option<usize> {
        if matches!(c[i], 'b' | 'c') {
            i += 1;
        }
        if c.get(i) != Some(&'r') {
            return None;
        }
        i += 1;
        let mut hashes = 0;
        while c.get(i) == Some(&'#') {
            hashes += 1;
            i += 1;
        }
        if c.get(i) != Some(&'"') {
            return None;
        }
        i += 1;
        while i < c.len() {
            if c[i] == '"' && (1..=hashes).all(|h| c.get(i + h) == Some(&'#')) {
                return Some(i + hashes + 1);
            }
            i += 1;
        }
        Some(i)
    }

    /// Where the bracket that opens at `i` closes.
    fn closing(t: &[Token], i: usize) -> usize {
        let mut depth = 0;
        for (n, x) in t.iter().enumerate().skip(i) {
            match x {
                Sign('(' | '[' | '{') => depth += 1,
                Sign(')' | ']' | '}') => {
                    depth -= 1;
                    if depth == 0 {
                        return n;
                    }
                }
                _ => {}
            }
        }
        t.len() - 1
    }

    /// `t` without the items and statements marked `#[cfg(test)]`.
    pub fn production(t: &[Token]) -> Vec<Token> {
        let mark = [Sign('#'), Sign('['), Word("cfg".into()), Sign('('), Word("test".into()), Sign(')'), Sign(']')];
        let (mut out, mut i) = (vec![], 0);
        while i < t.len() {
            if t[i..].starts_with(&mark) {
                i += mark.len();
                // Other attributes, then the item: to its `;`, or its block.
                while i < t.len() {
                    match &t[i] {
                        Sign('#') if t.get(i + 1) == Some(&Sign('[')) => i = closing(t, i + 1) + 1,
                        Sign(';') => {
                            i += 1;
                            break;
                        }
                        Sign('{') => {
                            i = closing(t, i) + 1;
                            break;
                        }
                        Sign('(' | '[') => i = closing(t, i) + 1,
                        _ => i += 1,
                    }
                }
                continue;
            }
            out.push(t[i].clone());
            i += 1;
        }
        out
    }

    /// Every path of `t` in full (its first name resolved through the imports), the names of its methods, and whether it has
    /// anything unchecked of its own (`unsafe`, `extern`).
    #[derive(Default)]
    pub struct Read {
        pub paths: BTreeSet<String>,
        pub methods: BTreeSet<String>,
        pub unsafe_code: bool,
        /// The modules it declares (each is a file of its own, looked at too).
        pub modules: BTreeSet<String>,
    }

    fn imports(t: &[Token], i: &mut usize, prefix: &str, out: &mut BTreeMap<String, String>, all: &mut BTreeSet<String>) {
        let mut at = prefix.to_string();
        let mut last = String::new();
        while *i < t.len() {
            match &t[*i] {
                Word(w) if w == "as" => {
                    let Some(Word(name)) = t.get(*i + 1) else { return };
                    out.insert(name.clone(), at.clone());
                    all.insert(at.clone());
                    *i += 2;
                    last.clear();
                }
                Word(w) => {
                    if w == "self" && !at.is_empty() {
                        last = at.rsplit("::").next().unwrap_or_default().to_string();
                    } else {
                        at = if at.is_empty() { w.clone() } else { format!("{at}::{w}") };
                        last = w.clone();
                    }
                    *i += 1;
                }
                Sign(':') => *i += 1,
                Sign('*') => {
                    all.insert(format!("{at}::*"));
                    last.clear();
                    *i += 1;
                }
                Sign('{') => {
                    let end = closing(t, *i);
                    *i += 1;
                    while *i < end {
                        imports(t, i, &at, out, all);
                        if t.get(*i) == Some(&Sign(',')) {
                            *i += 1;
                        }
                    }
                    *i = end + 1;
                    return;
                }
                Sign(',' | ';' | '}') => break,
                _ => *i += 1,
            }
        }
        if !last.is_empty() {
            out.insert(last, at.clone());
            all.insert(at);
        }
    }

    pub fn read(t: &[Token]) -> Read {
        let mut out = Read::default();
        let mut names = BTreeMap::new();
        let mut body = vec![];
        let mut i = 0;
        while i < t.len() {
            if t[i] == Word("use".into()) && !matches!(t.get(i.wrapping_sub(1)), Some(Sign('.'))) {
                i += 1;
                imports(t, &mut i, "", &mut names, &mut out.paths);
                continue;
            }
            body.push(t[i].clone());
            i += 1;
        }
        let mut i = 0;
        while i < body.len() {
            let Word(first) = &body[i] else {
                i += 1;
                continue;
            };
            if first == "mod" {
                if let Some(Word(name)) = body.get(i + 1) {
                    out.modules.insert(name.clone());
                }
            }
            if first == "unsafe" || first == "extern" || first == "asm" {
                out.unsafe_code = true;
            }
            let before = i.checked_sub(1).map(|b| &body[b]);
            // `a..b` is a range, `a.b(` a method.
            let method = before == Some(&Sign('.')) && i.checked_sub(2).map(|b| &body[b]) != Some(&Sign('.'));
            if method {
                out.methods.insert(first.clone());
                i += 1;
                continue;
            }
            let rooted = i >= 2 && body[i - 1] == Sign(':') && body[i - 2] == Sign(':');
            let (mut path, mut n, mut single) = (names.get(first).filter(|_| !rooted).cloned().unwrap_or_else(|| first.clone()), i + 1, true);
            loop {
                let colons = body.get(n) == Some(&Sign(':')) && body.get(n + 1) == Some(&Sign(':'));
                match body.get(n + 2) {
                    Some(Word(w)) if colons => {
                        path = format!("{path}::{w}");
                        n += 3;
                        single = false;
                    }
                    // `Vec::<T>::new`
                    Some(Sign('<')) if colons => break,
                    _ => break,
                }
            }
            if rooted && i >= 3 && body[i - 3] == Sign('>') {
                // `<T as X>::f`: by the name of its function.
                out.methods.insert(path.clone());
            } else if !single || names.contains_key(first) {
                out.paths.insert(path);
            }
            i = n;
        }
        out
    }

    /// Every Rust file below `folder`, tests left out.
    pub fn files(folder: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(folder).unwrap().flatten() {
            let path = e.path();
            if path.is_dir() {
                files(&path, out);
            } else if path.extension().is_some_and(|x| x == "rs") && path.file_stem().is_some_and(|s| s != "tests") {
                out.push(path);
            }
        }
    }
}

/// What only reads. A path is let through when one of these is it, or begins it (ending in `::`).
const READS: &[&str] = &[
    // Reading files, their names and what they are.
    "std::fs", "std::fs::read", "std::fs::read_to_string", "std::fs::read_dir", "std::fs::metadata", "std::fs::symlink_metadata", "std::fs::canonicalize", "std::fs::Metadata", "std::fs::File::open",
    // Not files at all.
    "std::io::Read", "std::io::Write", "std::io::Error", "std::io::ErrorKind::", "std::io::Result", "std::io::copy", "std::io::sink", "std::io::BufRead", "std::io::BufReader", "std::io::BufReader::", "std::io",
    "std::io::Error::", "std::path::", "std::collections::", "std::sync::", "std::ffi::", "std::time::", "std::ops::", "std::fmt::", "std::cell::", "std::mem::", "std::ptr::", "std::env::temp_dir", "std::env::var_os", "std::env::consts::",
    "std::str::", "std::thread::", "std::os::unix::fs::MetadataExt", "std::os::windows::ffi::", "std::iter::", "std::cmp::", "std::process::id", "std::borrow::", "std::slice::", "std::io::Seek", "std::io::SeekFrom", "std::io::SeekFrom::", "clippy::",
    "anyhow::", "serde::", "serde_json::", "orbiters_i18n::", "ring::digest::", "ring::signature::", "uuid::", "time::", "url::", "base64::", "walkdir::", "sysinfo::", "flate2::read::", "regex::",
    "vrc_get_vpm::version::",
    // This app's own, which read.
    "crate::owned::FileId", "crate::owned::id_of", "crate::owned::id_of_file", "crate::owned::is_same", "crate::owned::is_link", "crate::owned",
    "crate::discovery::key", "crate::discovery::is_project", "crate::discovery::now", "crate::discovery::parse_version", "crate::discovery::is_link", "crate::discovery::Project", "crate::discovery::database_editors", "crate::discovery::editor_hints",
    "crate::editor::of", "crate::editor::running", "crate::size::free_space", "crate::data_dir", "crate::vcc_dir", "crate::vcc_app_dir", "crate::hub_data_dir", "crate::runtime", "crate::QuitAtOnce", "crate::mac::", "crate::unity_editors",
    "crate::templates::Keep", "crate::templates::Keep::", "crate::templates::left_out", "crate::templates::retarget_with", "crate::templates::packages_only_in_library", "crate::templates::installed_dependencies_complete",
    "crate::templates::dependencies_complete", "crate::templates::Marker", "crate::templates::Marker::", "crate::templates::INCOMPLETE", "crate::templates::Progress", "crate::templates::SharedProgress", "crate::templates::validate_name",
    "crate::builtin_templates::", "crate::copying", "crate::copying::", "crate::copies::", "crate::unity6::", "crate::find_editors", "crate::network::", "orbiters_core::app_version", "orbiters_core::account::", "orbiters_update::",
];

/// What one file alone may use besides, and why.
const ALONE: &[(&str, &[&str])] = &[
    // Not the Unity 6 feature's: a duplicate's or a new project's own. A restore's staging folder, in app data, is filled by VPM;
    // it is put on the list VCC and ALCOM share; the bundled templates' own cache.
    ("templates.rs", &["crate::vpm::change", "crate::vpm::Change::Resolve", "crate::vpm::register", "crate::bundled_templates::builtins"]),
    ("copies.rs", &["crate::vpm::register"]),
    // Asking a disk and an editor's program what they are.
    ("budget.rs", &["windows::core::PCWSTR", "windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW", "windows::Win32::Storage::FileSystem::GetVolumeNameForVolumeMountPointW", "windows::Win32::Storage::FileSystem::GetVolumePathNameW"]),
    ("editors.rs", &["windows::", "winreg::", "plist::"]),
];

/// Methods that write something on a disk, start something, or have VPM do it, whatever they are called on.
const WRITING: &[&str] = &[
    "set_len", "set_modified", "set_times", "set_permissions", "set_readonly", "persist", "persist_noclobber", "unpack", "unpack_in", "extract", "extract_unwrapped_root_dir", "create_new", "create_dir", "create_dir_all", "truncate", "spawn",
    "output", "status_now", "save", "apply_pending_changes", "add_package_request", "resolve_request", "migrate_unity_2022", "save_as", "into_temp_path", "write_to", "commit", "sync_data", "copy_to", "rename", "remove_dir_all",
    "remove_dir", "hard_link",
];

/// Files whose own unchecked code asks Windows a question and nothing else (each listed above by name).
const ASKS_WINDOWS: &[&str] = &["budget.rs", "editors.rs"];

fn refused(name: &str, read: &source::Read) -> Vec<String> {
    let alone = ALONE.iter().find(|(n, _)| *n == name).map_or(&[][..], |(_, a)| a);
    let lets = |path: &str| READS.iter().chain(alone).any(|a| if a.ends_with("::") { path.starts_with(a) } else { path == *a });
    let types = |path: &str| path.split("::").next().is_some_and(|f| f.chars().next().is_some_and(|c| c.is_uppercase() || c.is_numeric()));
    let local = |path: &str| ["self", "super", "Self"].contains(&path.split("::").next().unwrap_or_default());
    let core = |path: &str| ["u8", "u16", "u32", "u64", "i32", "i64", "usize", "f64", "str", "char", "bool"].contains(&path.split("::").next().unwrap_or_default());
    let mut out: Vec<String> = read.paths.iter().filter(|p| p.contains("::") && !(lets(p) || types(p) || local(p) || core(p) || read.modules.contains(p.split("::").next().unwrap_or_default()))).map(|p| format!("{name}: {p}")).collect();
    out.extend(read.methods.iter().filter(|m| WRITING.contains(&m.as_str())).map(|m| format!("{name}: .{m}(")));
    if read.unsafe_code && !ASKS_WINDOWS.contains(&name) {
        out.push(format!("{name}: unsafe"));
    }
    out
}

/// The paths of `read` that can write Creator Companion's or ALCOM's files.
fn theirs(read: &source::Read) -> Vec<String> {
    let reaches = |p: &str| {
        p.starts_with("vrc_get_vpm::") && !p.starts_with("vrc_get_vpm::version::") || p == "vrc_get_vpm" || p.starts_with("vrc_get_litedb") || p.starts_with("crate::vpm") || p.starts_with("super::super::vpm") || p.starts_with("crate::copies::list")
            || p.starts_with("crate::templates::make") || p.starts_with("crate::templates::duplicate") || p.starts_with("crate::templates::create")
    };
    read.paths.iter().filter(|p| reaches(p)).cloned().collect()
}

/// The test of it: each call that reaches VPM's environment is seen, however it is named.
#[test]
fn no_call_into_creator_companions_files_is_missed() {
    let reaches = |text: &str| !theirs(&source::read(&source::production(&source::tokens(text)))).is_empty();
    for writes in [
        "async fn f() { crate::vpm::register(r, p, true).await; }",
        "use crate::vpm; async fn f() { vpm::register(r, p, false).await; }",
        "use crate::vpm::register as keep; async fn f() { keep(r, p, true).await; }",
        "use crate::{owned, vpm::{self as v}}; async fn f() { v::change(r, p, c).await; }",
        "use vrc_get_vpm::environment::{Settings as S, VccDatabaseConnection}; async fn f() { S::load(io).await; }",
        "use vrc_get_vpm as v; async fn f() { v::environment::Settings::load(io).await; }",
        "async fn f() { vrc_get_vpm::environment::VccDatabaseConnection::connect(io).await; }",
        "use vrc_get_litedb::file_io::LiteDBFile; fn f() { LiteDBFile::parse(b); }",
        "fn f() { crate::copies::list(a, b, c); }",
        "fn f() { crate::templates::duplicate(a); }",
        "fn f() { anyhow::ensure!(crate::vpm::register(r, p, true).is_ok()); }",
    ] {
        assert!(reaches(writes), "{writes}");
    }
    for fine in ["fn f() { v.parse::<vrc_get_vpm::version::Version>(); }", "use vrc_get_vpm::version::UnityVersion; fn f() { UnityVersion::parse(t); }", "fn f() { crate::templates::validate_name(n); } // crate::vpm::register"] {
        assert!(!reaches(fine), "{fine}");
    }
}

#[test]
fn no_other_module_writes() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    if !src.is_dir() {
        return;
    }
    let mut all = vec![];
    source::files(&src.join("unity6"), &mut all);
    all.retain(|p| !p.starts_with(src.join("unity6/gate")));
    all.extend(["templates.rs", "copying.rs", "copies.rs"].map(|f| src.join(f)));
    // The modules of the feature, whatever folder they are in, are all looked at.
    for needed in ["authority.rs", "journal.rs", "payload.rs", "content.rs", "trial.rs", "matrix.rs", "budget.rs"] {
        assert!(all.iter().any(|p| p.ends_with(needed)), "{needed}");
    }
    let mut out = vec![];
    for path in all {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let tokens = source::production(&source::tokens(&std::fs::read_to_string(&path).unwrap()));
        assert!(tokens.len() > 50, "{name} is not read");
        let read = source::read(&tokens);
        out.extend(refused(&name, &read));
        // Decision A: nothing of the Unity 6 feature reaches what writes Creator Companion's or ALCOM's files, by any name: VPM's
        // environment (its settings, its database), this crate's own module that uses it, or the database's crate.
        if path.starts_with(src.join("unity6")) || name == "copying.rs" {
            out.extend(theirs(&read).into_iter().map(|p| format!("{name}: {p} writes Creator Companion's files")));
        }
    }
    assert!(out.is_empty(), "outside the gate:\n{}", out.join("\n"));
}

/// The test of the test: each way of writing the third verification found it missed, and others, is seen; what reads is not.
#[test]
fn the_source_test_sees_every_way_of_writing() {
    let refuses = |text: &str| !refused("x.rs", &source::read(&source::production(&source::tokens(text)))).is_empty();
    for writes in [
        // By any name.
        "fn f() { std::fs::write(p, b); }",
        "use std::fs; fn f() { fs::write(p, b); }",
        "use std::fs::write; fn f() { write(p, b); }",
        "use std::fs::{self as disk, File as F}; fn f() { disk::remove_file(p); }",
        "use std::fs::File as F; fn f() { F::create(p); }",
        "use std::{fs, io}; fn f() { fs::rename(a, b); }",
        "use std::fs::*; fn f() { write(p, b); }",
        "fn f() { ::std::fs::copy(a, b); }",
        "fn f() { std :: fs\n:: // a comment\n create_dir_all(p); }",
        "fn f() { r#std::r#fs::write(p, b); }",
        "fn f() { let w = std::fs::write::<&Path, &[u8]>; }",
        "fn f() { std::fs::OpenOptions::new().write(true).open(p); }",
        "fn f() { let o: std::fs::OpenOptions = x; }",
        "fn f() { std::fs::File::options(); }",
        // In a macro, after the tests, on a line with a text that looks like a comment.
        "fn f() { anyhow::ensure!(std::fs::write(p, b).is_ok()); }",
        "#[cfg(test)] mod tests { fn g() {} } fn f() { std::fs::write(p, b); }",
        "#[cfg(test)]\nmod tests;\nfn f() { std::fs::write(p, b); }",
        "fn f() { let a = \"http://x\"; std::fs::write(p, a); }",
        "fn f() { let a = '\"'; std::fs::write(p, a); }",
        "fn f() { let a = r#\"\"\"#; std::fs::write(p, a); }",
        "/* /* */ */ fn f() { std::fs::write(p, a); }",
        "fn f<'a>(x: &'a str) { std::fs::write(p, x); }",
        // Wrappers, other crates, other ways in.
        "fn f() { crate::write_json(p, v); }",
        "use crate::write_json as keep; fn f() { keep(p, v); }",
        "fn f() { crate::storage::repair(p); }",
        "fn f() { crate::owned::delete_file_if(p, id); }",
        "fn f() { crate::owned::Pinned::open(p); }",
        "fn f() { crate::templates::copy(a, b, c, d); }",
        "fn f() { crate::vpm::register(r, p, true); }",
        "fn f() { tokio::fs::write(p, b); }",
        "fn f() { tar::Archive::new(r).unpack(p); }",
        "fn f(a: A) { a.unpack(p); }",
        "fn f(z: Z) { z.extract(p); }",
        "fn f() { flate2::write::GzEncoder::new(f, l); }",
        "async fn f() { vrc_get_vpm::UnityProject::load(io).await; }",
        "use vrc_get_vpm::UnityProject; async fn f() { UnityProject::load(io).await; }",
        "use vrc_get_vpm::io::DefaultProjectIo; fn f() {}",
        "fn f() { tempfile::tempdir(); }",
        "fn f(t: T) { t.persist(p); }",
        "fn f(x: F) { x.set_len(0); }",
        "fn f() { std::process::Command::new(p); }",
        "fn f(c: C) { c.spawn(); }",
        "fn f() { junction::create(a, b); }",
        "fn f() { std::os::unix::fs::symlink(a, b); }",
        "fn f() { unsafe { x() } }",
        "extern \"C\" { fn unlink(p: *const u8); }",
    ] {
        assert!(refuses(writes), "{writes}");
    }
    for reads in [
        "use std::path::Path; fn f() { std::fs::read(p); }",
        "use std::fs; fn f() { fs::read_to_string(p); }",
        "// std::fs::write\n/* std::fs::write */ fn f() { let s = \"std::fs::write\"; let r = r#\"std::fs::write\"#; }",
        "#[cfg(test)] mod tests { fn f() { std::fs::write(p, b); } }",
        "#[cfg(test)]\nthread_local! { static A: X = const { Y }; }\nfn f() { a.b(); }",
        "fn f() { #[cfg(test)] if x(std::fs::write) { y() } z(); }",
        "fn f() { Vec::<u8>::new(); 1..2; a.iter().map(String::from); }",
    ] {
        assert!(!refuses(reads), "{reads}");
    }
}
