//! What no code of the app does, by reading all of it: Creator Companion's and ALCOM's files have no writer here. The library both
//! come with (vrc-get) is used for its resolver and installer alone, in Orbiters' own environment.
use source::Token;
use std::path::{Path, PathBuf};

/// The tokens of Rust files, and the paths and methods they name.
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

/// Methods of vrc-get's (and its database's) that change what those programs keep, whatever they are called on.
const CHANGES: &[&str] = &[
    "add_user_project", "remove_user_project", "retain_user_projects", "load_from_db", "set_show_prerelease_packages", "set_default_project_path", "set_project_backup_path", "set_unity_hub_path", "remove_repo", "remove_repo_at_index",
    "remove_id_duplication", "update_id", "add_remote_repo", "add_local_repo", "add_user_package", "remove_user_package", "reorder_user_repos_by_indices", "add_project", "remove_project", "update_project", "dedup_projects", "normalize_path",
    "update_unity_from_unity_hub_and_fs", "add_unity_installation", "remove_unity_installation",
];
/// Of the database's reader alone.
const DATABASE: &[&str] = &["serialize", "insert", "update", "delete", "upsert", "ensure_index", "drop_index", "drop_collection", "create_index", "rename_collection", "write", "write_all", "set_len", "truncate", "append", "open", "create", "lock"];

/// What of `text`, the file `name` of the crate `krate`, could write those programs' files.
fn writes(krate: &str, name: &str, text: &str) -> Vec<String> {
    let tokens = source::production(&source::tokens(text));
    let read = source::read(&tokens);
    let mut out = vec![];
    for p in &read.paths {
        let parts: Vec<&str> = p.split("::").filter(|part| !matches!(*part, "crate" | "super" | "self")).collect();
        let fine = match parts[..] {
            ["vrc_get_vpm"] => false,
            ["vrc_get_vpm", "environment"] => krate == "orbiters-projects" && name == "vpm.rs",
            ["vrc_get_vpm", "environment", "Settings", "save"] => false,
            ["vrc_get_vpm", "environment", "Settings", ..] => krate == "orbiters-projects" && matches!(name, "vpm.rs" | "vpm_http.rs"),
            ["vrc_get_vpm", "environment", "PackageCollection" | "PackageInstaller" | "add_remote_repo", ..] => krate == "orbiters-projects" && name == "vpm.rs",
            ["vrc_get_vpm", "environment", ..] => false,
            ["vrc_get_vpm", "io"] => false,
            ["vrc_get_vpm", "io", "DefaultProjectIo", ..] => krate == "orbiters-projects",
            ["vrc_get_vpm", "io", "DefaultEnvironmentIo"] | ["vrc_get_vpm", "io", "DefaultEnvironmentIo", "new"] => krate == "orbiters-projects" && name == "vpm.rs",
            ["vrc_get_vpm", "io", ..] | ["vrc_get_vpm", "*"] => false,
            ["vrc_get_vpm", ..] => krate == "orbiters-projects",
            ["vrc_get_litedb", "file_io", "LiteDBFile"] | ["vrc_get_litedb", "file_io", "LiteDBFile", "parse"] | ["vrc_get_litedb", "bson", "Document"] => krate == "orbiters-projects" && name == "theirs.rs",
            ["vrc_get_litedb", ..] => false,
            _ => true,
        };
        if !fine {
            out.push(p.clone());
        }
    }
    let library = read.paths.iter().any(|p| p.contains("vrc_get_vpm::environment") || p.contains("vrc_get_litedb"));
    for m in &read.methods {
        if CHANGES.contains(&m.as_str()) && (library || krate == "orbiters-projects" && m != "normalize_path") || name == "theirs.rs" && DATABASE.contains(&m.as_str()) {
            out.push(format!(".{m}("));
        }
    }
    let word = |i: usize, w: &str| tokens.get(i) == Some(&Token::Word(w.into()));
    let sign = |i: usize, c: char| tokens.get(i) == Some(&Token::Sign(c));
    for i in 0..tokens.len() {
        // Saved: a project's own manifest, and nothing else of the library's.
        if library && i >= 2 && word(i, "save") && sign(i - 1, '.') && !word(i - 2, "project") {
            out.push("save of what is not a project".into());
        }
        // The environment it works in is Orbiters' own, made at one place.
        let path = ["(", "crate", "repos", "prepare"];
        if word(i, "DefaultEnvironmentIo") && sign(i + 1, ':') && word(i + 3, "new") && !(sign(i + 4, '(') && word(i + 5, path[1]) && word(i + 8, path[2]) && word(i + 11, path[3])) {
            out.push("an environment that is not Orbiters'".into());
        }
        // By its own name, so that the one place is seen.
        if word(i, "DefaultEnvironmentIo") && word(i + 1, "as") {
            out.push("an environment by another name".into());
        }
    }
    // Their reader has no other way to a file than the one that reads it whole.
    if krate == "orbiters-projects" && name == "theirs.rs" {
        out.extend(read.paths.iter().filter(|p| ["std::fs", "std::os", "std::process", "tokio", "windows", "std::io"].iter().any(|w| p.starts_with(w))).cloned());
    }
    out
}

fn production_files(krate: &str) -> Vec<PathBuf> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join(krate).join("src");
    let mut all = vec![];
    if src.is_dir() {
        source::files(&src, &mut all);
    }
    all
}

#[test]
fn nothing_in_the_app_writes_creator_companions_or_alcoms_files() {
    let mut out = vec![];
    let mut looked = 0;
    for krate in ["orbiters-projects", "orbiters-desktop", "orbiters-core", "orbiters-n4n0-core", "orbiters-n4n0"] {
        for path in production_files(krate) {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            // A test module in a file of its own.
            if name.ends_with("_tests.rs") {
                continue;
            }
            looked += 1;
            let text = std::fs::read_to_string(&path).unwrap();
            out.extend(writes(krate, &name, &text).into_iter().map(|w| format!("{krate}: {name}: {w}")));
            // Their files are named where they are read, and nowhere else.
            let named = ["vcc.liteDb", "vcc-log.liteDb", "gui-config.json", "vcc-settings-backup.json"].iter().filter(|f| text.lines().any(|l| !l.trim_start().starts_with("//") && l.contains(&format!("{f}\""))));
            if !(krate == "orbiters-projects" && matches!(name.as_str(), "theirs.rs" | "no_write_tests.rs")) {
                out.extend(named.map(|f| format!("{krate}: {name}: names {f}")));
            }
        }
    }
    assert!(out.is_empty(), "these could write Creator Companion's or ALCOM's files:\n{}", out.join("\n"));
    if Path::new(env!("CARGO_MANIFEST_DIR")).join("src/vpm.rs").is_file() {
        assert!(looked > 100, "{looked} files");
    }
}

/// The test of the test.
#[test]
fn no_way_of_writing_them_is_missed() {
    for bad in [
        // As main did it.
        "use vrc_get_vpm::environment::{Settings, VccDatabaseConnection}; async fn f() { let mut db = VccDatabaseConnection::connect(&io).await?; db.save(&io).await?; }",
        "use vrc_get_vpm::environment::Settings; async fn f() { let mut settings = Settings::load(&io).await?; settings.save(&io).await?; }",
        "fn f() { settings.add_user_project(p); }",
        "fn f() { settings.retain_user_projects(|p| p != x); }",
        "async fn f() { db.add_project(&project).await?; }",
        "fn f() { db.remove_project(&p); }",
        "use vrc_get_vpm::environment as e; async fn f() { e::VccDatabaseConnection::connect(&io).await; }",
        "use vrc_get_vpm::*;",
        "use vrc_get_vpm::environment::*;",
        "use vrc_get_vpm::environment::{self, Settings}; async fn f() { environment::cleanup_repos_folder(&s, &io).await; }",
        "use vrc_get_vpm::io::DefaultEnvironmentIo; fn f() { DefaultEnvironmentIo::new(root.into()); }",
        "use vrc_get_vpm::io::DefaultEnvironmentIo as Io; fn f() { Io::new_default(); }",
        "use vrc_get_vpm::io::DefaultEnvironmentIo as Io; fn f() { Io::new(root.into()); }",
        "fn f() { vrc_get_vpm::io::DefaultEnvironmentIo::new(root.into()); }",
        "use vrc_get_vpm::io::IoTrait;",
        "use vrc_get_litedb::file_io::LiteDBFile; fn f() { LiteDBFile::new(); }",
        "fn f() { vrc_get_litedb::file_io::LiteDBFile::parse(b)?.serialize(); }",
        "fn f() { settings.remove_repo(|r| true); }",
    ] {
        for name in ["vpm.rs", "gallery.rs", "theirs.rs"] {
            // (What vpm.rs alone may name, it may not do this with.)
            assert!(!writes("orbiters-projects", name, bad).is_empty(), "{name}: {bad}");
        }
        assert!(!writes("orbiters-desktop", "menus.rs", bad).is_empty() || !bad.contains("vrc_get"), "{bad}");
    }
    for bad in ["fn f() { std::fs::write(p, b); }", "fn f() { std::fs::File::options().write(true); }", "fn f() { db.serialize(); }", "use std::fs; fn f() { fs::remove_file(p); }", "fn f() { file.lock(); }"] {
        assert!(!writes("orbiters-projects", "theirs.rs", bad).is_empty(), "{bad}");
    }
    for fine in [
        "use vrc_get_vpm::environment::{self, PackageCollection, PackageInstaller, Settings}; async fn f() { let io = DefaultEnvironmentIo::new(crate::repos::prepare(root)?.into()); Settings::load(&io).await; project.save().await; }",
        "use vrc_get_vpm::io::{DefaultEnvironmentIo, DefaultProjectIo}; fn f() { UnityProject::load(DefaultProjectIo::new(p)); } // db.save(&io)",
    ] {
        assert_eq!(writes("orbiters-projects", "vpm.rs", fine), Vec::<String>::new());
    }
}
