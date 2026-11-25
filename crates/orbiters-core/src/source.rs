//! The app's own sources, for the tests that hold a rule by reading them (a credential goes to its own server only,
//! nobody resolves whoever is signed in by themselves, a layer knows none above it...).
//!
//! What such a test reads is all of the app's code and nothing of its tests. `#[cfg(test)]` marks fields, statements and
//! functions in the middle of the app's code as well as the test module at its foot: a test that cut a file at the first
//! one read 76 of `controller.rs`'s 3,600 lines (XCT-41, 2026-11-25). [`production`] leaves out each item so marked and
//! goes on after it; [`files`] leaves out the files only tests compile, by how the modules are declared.
//!
//! Compiled for tests alone (`test-support`).

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A name, a keyword, a number.
    Word,
    /// One character of punctuation.
    Sign,
    /// A string or a character.
    Text,
}

/// One token, by its place in the source. Comments are none.
#[derive(Clone, Debug)]
pub struct Token {
    pub kind: Kind,
    pub at: Range<usize>,
}

/// The tokens of `text`.
pub fn tokens(text: &str) -> Vec<Token> {
    let c = text.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
    let (mut out, mut i) = (Vec::new(), 0);
    while i < c.len() {
        let next = c.get(i + 1).copied().unwrap_or(0);
        let start = i;
        match c[i] {
            b'/' if next == b'/' => {
                while i < c.len() && c[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if next == b'*' => {
                let mut depth = 0;
                while i < c.len() {
                    match (c[i], c.get(i + 1)) {
                        (b'/', Some(b'*')) => {
                            depth += 1;
                            i += 2;
                        }
                        (b'*', Some(b'/')) => {
                            depth -= 1;
                            i += 2;
                        }
                        _ => i += 1,
                    }
                    if depth == 0 {
                        break;
                    }
                }
            }
            b'"' => {
                i = quoted(c, i + 1, b'"');
                out.push(Token { kind: Kind::Text, at: start..i });
            }
            // A character, or a lifetime.
            b'\'' => {
                // (One character may be several bytes.)
                let long = text[i + 1..].chars().next().map_or(1, char::len_utf8);
                if next == b'\\' || c.get(i + 1 + long) == Some(&b'\'') {
                    i = quoted(c, i + 1, b'\'');
                    out.push(Token { kind: Kind::Text, at: start..i });
                } else {
                    i += 1;
                    while i < c.len() && word(c[i]) {
                        i += 1;
                    }
                    out.push(Token { kind: Kind::Word, at: start..i });
                }
            }
            b if word(b) => {
                while i < c.len() && word(c[i]) {
                    i += 1;
                }
                let prefix = &text[start..i];
                let raw = matches!(prefix, "r" | "br" | "cr");
                let hashes = c[i..].iter().take_while(|b| **b == b'#').count();
                if raw && c.get(i + hashes) == Some(&b'"') {
                    let close = format!("\"{}", "#".repeat(hashes));
                    let from = i + hashes + 1;
                    i = text[from..].find(&close).map_or(c.len(), |n| from + n + close.len());
                    out.push(Token { kind: Kind::Text, at: start..i });
                } else if matches!(prefix, "b" | "c") && c.get(i) == Some(&b'"') {
                    i = quoted(c, i + 1, b'"');
                    out.push(Token { kind: Kind::Text, at: start..i });
                } else if prefix == "b" && c.get(i) == Some(&b'\'') {
                    i = quoted(c, i + 1, b'\'');
                    out.push(Token { kind: Kind::Text, at: start..i });
                } else if prefix == "r" && hashes == 1 {
                    // A raw name is that name.
                    i += 1;
                } else {
                    out.push(Token { kind: Kind::Word, at: start..i });
                }
            }
            b if b.is_ascii_whitespace() => i += 1,
            _ => {
                i += 1;
                out.push(Token { kind: Kind::Sign, at: start..i });
            }
        }
    }
    out
}

/// Where the text opened before `i` ends, past its closing mark.
fn quoted(c: &[u8], mut i: usize, close: u8) -> usize {
    while i < c.len() && c[i] != close {
        i += if c[i] == b'\\' { 2 } else { 1 };
    }
    (i + 1).min(c.len())
}

/// A source read: its tokens, by word or sign.
struct Reading<'a> {
    text: &'a str,
    t: Vec<Token>,
}

impl<'a> Reading<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, t: tokens(text) }
    }

    /// The word or sign at `i`; nothing for a text.
    fn at(&self, i: usize) -> &'a str {
        self.t.get(i).filter(|t| t.kind != Kind::Text).map_or("", |t| &self.text[t.at.clone()])
    }

    /// Where the bracket that opens at `i` closes (the last token when it does not).
    fn closing(&self, i: usize) -> usize {
        let mut depth = 0usize;
        for n in i..self.t.len() {
            match self.at(n) {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return n;
                    }
                }
                _ => {}
            }
        }
        self.t.len().saturating_sub(1)
    }

    /// The attributes from `i`: where what they are on begins, whether one of them keeps it out of the app, and the file
    /// `#[path]` names.
    fn attributes(&self, mut i: usize) -> (usize, bool, Option<String>) {
        let (mut test, mut path) = (false, None);
        while self.at(i) == "#" && self.at(i + 1) == "[" {
            let end = self.closing(i + 1);
            test |= match self.at(i + 2) {
                "cfg" if self.at(i + 3) == "(" => self.never(i + 4).0,
                "test" => end == i + 3,
                _ => false,
            };
            if self.at(i + 2) == "path" && self.at(i + 3) == "=" {
                path = self.t.get(i + 4).map(|t| self.text[t.at.clone()].trim_matches('"').to_string());
            }
            i = end + 1;
        }
        (i, test, path)
    }

    /// Whether the condition at `i` never holds in the app (not for a test, whatever the platform), whether it always
    /// does as far as tests go, and where it ends.
    fn never(&self, i: usize) -> (bool, bool, usize) {
        let name = self.at(i);
        if self.at(i + 1) == "(" {
            let end = self.closing(i + 1);
            let (mut n, mut parts) = (i + 2, Vec::new());
            while n < end {
                let (no, yes, after) = self.never(n);
                parts.push((no, yes));
                n = after.max(n) + 1; // (its comma)
            }
            let (no, yes) = match name {
                "not" => parts.first().map_or((false, false), |(no, yes)| (*yes, *no)),
                "all" => (parts.iter().any(|p| p.0), parts.iter().all(|p| p.1)),
                "any" => (parts.iter().all(|p| p.0), parts.iter().any(|p| p.1)),
                _ => (false, false),
            };
            return (no, yes, end + 1);
        }
        if self.at(i + 1) == "=" {
            // `feature = "test-support"`: switched on by tests' dependencies alone.
            let value = self.t.get(i + 2).map_or("", |t| &self.text[t.at.clone()]);
            return (name == "feature" && value == "\"test-support\"", false, i + 3);
        }
        (name == "test", false, i + 1)
    }

    /// Past the end of the item, field, statement, parameter or arm that begins at `i`. Where its end cannot be told, it
    /// ends too early rather than too late: what is left of it is read as the app's, never the app's code as its.
    fn end(&self, mut i: usize) -> usize {
        let first = loop {
            match self.at(i) {
                "pub" if self.at(i + 1) == "(" => i = self.closing(i + 1) + 1,
                "pub" => i += 1,
                other => break other,
            }
        };
        // To a `;` whatever is in between, to the first block (or `;`), or a field, parameter, arm or expression: to its comma.
        let value = matches!(first, "let" | "use" | "static" | "type") || (first == "const" && !matches!(self.at(i + 1), "fn" | "unsafe" | "async" | "extern"));
        let item = matches!(first, "fn" | "mod" | "impl" | "struct" | "enum" | "trait" | "union" | "extern" | "unsafe" | "async" | "const" | "macro_rules");
        let mut angles = 0usize;
        let mut arm = false;
        while i < self.t.len() {
            match self.at(i) {
                ";" => return i + 1,
                ")" | "]" | "}" => return i,
                "{" => {
                    let end = self.closing(i) + 1;
                    // The block of an item, of an arm, or a statement that is one (`if x { … } else { … }`).
                    let goes_on = matches!(self.at(end), "." | "?" | "else" | "=") || value || (!item && !arm && matches!(self.at(end), ";" | ","));
                    if !goes_on {
                        // (An arm's comma after its block.)
                        return if arm && self.at(end) == "," { end + 1 } else { end };
                    }
                    i = end;
                    continue;
                }
                "(" | "[" => {
                    i = self.closing(i) + 1;
                    continue;
                }
                "=" if self.at(i + 1) == ">" => {
                    arm = true;
                    angles = 0;
                    i += 2;
                    continue;
                }
                "-" if self.at(i + 1) == ">" => {
                    i += 2;
                    continue;
                }
                // `HashMap<A, B>`: written against its name, as a comparison is not.
                "<" if self.text[..self.t[i].at.start].ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == ':') => angles += 1,
                ">" => angles = angles.saturating_sub(1),
                "," if !item && !value && angles == 0 => return i + 1,
                _ => {}
            }
            i += 1;
        }
        self.t.len()
    }

    /// The parts of the text only tests compile.
    fn tests(&self) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.t.len() {
            if self.at(i) == "#" && self.at(i + 1) == "!" && self.at(i + 2) == "[" {
                // `#![cfg(test)]`: the whole file (an inline module's is not written here).
                if self.at(i + 3) == "cfg" && self.at(i + 4) == "(" && self.never(i + 5).0 {
                    out.push(0..self.text.len());
                    return out;
                }
                i = self.closing(i + 2) + 1;
                continue;
            }
            if self.at(i) == "#" && self.at(i + 1) == "[" {
                let (item, test, _) = self.attributes(i);
                if test {
                    let end = self.end(item).max(item);
                    let last = end.checked_sub(1).map_or(0, |l| self.t[l].at.end);
                    out.push(self.t[i].at.start..last);
                    i = end;
                    continue;
                }
                i = item;
                continue;
            }
            i += 1;
        }
        out
    }

    /// The modules in files of their own it declares: each one's name with the inline modules around it, the file its
    /// `#[path]` names, and whether only tests compile it.
    fn modules(&self) -> Vec<(Vec<String>, Option<String>, bool)> {
        let tests = self.tests();
        let mut out = Vec::new();
        let mut inside: Vec<(String, usize)> = Vec::new();
        let mut i = 0;
        let mut path = None;
        while i < self.t.len() {
            inside.retain(|(_, end)| *end > i);
            if self.at(i) == "#" && self.at(i + 1) == "[" {
                let (item, _, p) = self.attributes(i);
                path = p;
                i = item;
                continue;
            }
            if self.at(i) == "mod" && self.t.get(i + 1).is_some_and(|t| t.kind == Kind::Word) {
                let name = self.at(i + 1).to_string();
                if self.at(i + 2) == ";" {
                    let at = self.t[i].at.start;
                    let mut full: Vec<String> = inside.iter().map(|(n, _)| n.clone()).collect();
                    full.push(name);
                    out.push((full, path.take(), tests.iter().any(|r| r.contains(&at))));
                } else if self.at(i + 2) == "{" {
                    inside.push((name, self.closing(i + 2)));
                }
                path = None;
                i += 2;
                continue;
            }
            match self.at(i) {
                "pub" => {}
                "(" if self.at(i.wrapping_sub(1)) == "pub" => i = self.closing(i),
                _ => path = None,
            }
            i += 1;
        }
        out
    }
}

/// `text` with `parts` as blanks: the same lines, at the same numbers.
fn without(text: &str, parts: &[Range<usize>]) -> String {
    text.char_indices().map(|(at, c)| if c != '\n' && parts.iter().any(|p| p.contains(&at)) { ' ' } else { c }).collect()
}

/// `text` without what only tests compile: as blanks, so that every line keeps its number.
pub fn production(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let tests = Reading::new(&text).tests();
    without(&text, &tests)
}

/// One of the app's source files: its path, the module it is (`["pages", "launcher"]`, none for the crate's root), and what
/// of it the app compiles.
#[derive(Clone, Debug)]
pub struct File {
    pub path: PathBuf,
    pub module: Vec<String>,
    pub text: String,
}

/// Every Rust file below `src`, a crate's source folder, that the app compiles, in order.
pub fn files(src: &Path) -> Vec<File> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = e.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                out.push(path);
            }
        }
    }
    let mut paths = Vec::new();
    walk(src, &mut paths);
    paths.sort();
    let texts: BTreeMap<PathBuf, String> = paths.iter().map(|p| (p.clone(), std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())).replace("\r\n", "\n"))).collect();
    // Who declares whom.
    let mut declared: Vec<(PathBuf, PathBuf, bool)> = Vec::new();
    for (path, text) in &texts {
        let here = path.parent().expect("a file is in a folder");
        let stem = path.file_stem().expect("a Rust file has a name").to_string_lossy().into_owned();
        let own = if matches!(stem.as_str(), "mod" | "lib" | "main") { here.to_path_buf() } else { here.join(&stem) };
        for (names, named, test) in Reading::new(text).modules() {
            let (name, around) = names.split_last().expect("its own name");
            let dir = around.iter().fold(own.clone(), |d, n| d.join(n));
            let targets = match named {
                Some(file) if around.is_empty() => vec![here.join(file)],
                Some(file) => vec![dir.join(file)],
                None => vec![dir.join(format!("{name}.rs")), dir.join(name).join("mod.rs")],
            };
            declared.extend(targets.into_iter().filter(|t| texts.contains_key(t)).map(|t| (path.clone(), t, test)));
        }
    }
    // What the app compiles: every file nobody declares (the crate's root), and what those declare outside their tests.
    let children: BTreeSet<&PathBuf> = declared.iter().map(|(_, child, _)| child).collect();
    let mut compiled: BTreeSet<PathBuf> = texts.keys().filter(|p| !children.contains(p)).cloned().collect();
    loop {
        let more: Vec<PathBuf> = declared.iter().filter(|(parent, child, test)| !test && compiled.contains(parent) && !compiled.contains(child)).map(|(_, child, _)| child.clone()).collect();
        if more.is_empty() {
            break;
        }
        compiled.extend(more);
    }
    let module_of = |path: &Path| -> Vec<String> {
        let mut names: Vec<String> = path.strip_prefix(src).unwrap_or(path).with_extension("").components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        if matches!(names.last().map(String::as_str), Some("mod" | "lib" | "main")) {
            names.pop();
        }
        names
    };
    compiled.into_iter().map(|path| File { module: module_of(&path), text: production(&texts[&path]), path }).collect()
}

/// The same files, named as they are below `src` (`pages/launcher/mod.rs`).
pub fn named(src: &Path) -> Vec<(String, String)> {
    files(src).into_iter().map(|f| (f.path.strip_prefix(src).unwrap_or(&f.path).to_string_lossy().replace('\\', "/"), f.text)).collect()
}

/// The paths of `file` that begin at the crate's root, written `crate::…`, `self::…` or `super::…`, in a `use` or in
/// the code: the line of each and its names from the root down.
pub fn references(file: &File) -> Vec<(usize, Vec<String>)> {
    let r = Reading::new(&file.text);
    let line = |i: usize| r.text[..r.t[i].at.start].matches('\n').count() + 1;
    let colons = |i: usize| r.at(i) == ":" && r.at(i + 1) == ":";
    let mut out = Vec::new();
    let mut inside: Vec<(String, usize)> = Vec::new();
    let mut i = 0;
    while i < r.t.len() {
        inside.retain(|(_, end)| *end > i);
        if r.at(i) == "mod" && r.at(i + 2) == "{" {
            inside.push((r.at(i + 1).to_string(), r.closing(i + 2)));
            i += 3;
            continue;
        }
        let begins = matches!(r.at(i), "crate" | "super" | "self") && colons(i + 1) && !(i >= 2 && colons(i - 2));
        if !begins {
            i += 1;
            continue;
        }
        let mut at: Vec<String> = match r.at(i) {
            "crate" => Vec::new(),
            _ => file.module.iter().cloned().chain(inside.iter().map(|(n, _)| n.clone())).collect(),
        };
        if r.at(i) == "super" {
            at.pop();
        }
        i += 1;
        while colons(i) && r.at(i + 2) == "super" {
            at.pop();
            i += 3;
        }
        // The rest of the path; a group of a `use` is read to its ends.
        fn rest(r: &Reading, i: &mut usize, at: Vec<String>, line: &dyn Fn(usize) -> usize, out: &mut Vec<(usize, Vec<String>)>) {
            let mut at = at;
            let from = *i;
            let colons = |i: usize| r.at(i) == ":" && r.at(i + 1) == ":";
            while colons(*i) {
                if r.at(*i + 2) == "{" {
                    let end = r.closing(*i + 2);
                    *i += 3;
                    while *i < end {
                        if r.at(*i) == "," {
                            *i += 1;
                            continue;
                        }
                        let mut inner = at.clone();
                        if r.at(*i) != "self" && r.at(*i) != "*" {
                            inner.push(r.at(*i).to_string());
                        }
                        let name = *i;
                        *i += 1;
                        if colons(*i) {
                            rest(r, i, inner, line, out);
                        } else {
                            out.push((line(name), inner));
                            // `name as other`
                            while *i < end && r.at(*i) != "," {
                                *i += 1;
                            }
                        }
                    }
                    *i = end + 1;
                    return;
                }
                if r.t.get(*i + 2).is_none_or(|t| t.kind != Kind::Word) {
                    break;
                }
                at.push(r.at(*i + 2).to_string());
                *i += 3;
            }
            out.push((line(from), at));
        }
        rest(&r, &mut i, at, &line, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"
//! Says #[cfg(test)] in a comment.
pub struct Held {
    #[cfg(test)]
    for_tests: std::collections::HashMap<String, Vec<(u8, u8)>>,
    #[cfg(test)] last: Option<u8>
}
fn after_a_field() { send("Bearer one") }
#[cfg(test)]
pub(crate) const COUNTED: &[&str] = &["Bearer test a"];
#[cfg(test)]
mod elsewhere;
#[cfg(all(test, windows))]
#[path = "other_tests.rs"]
pub(crate) mod other;
fn arms(x: u8) -> Held {
    #[cfg(test)]
    if x == 1 {
        send("Bearer test b");
    } else {
        send("Bearer test c");
    }
    #[cfg(test)]
    let y = if x > 2 { send("Bearer test d") } else { 4 };
    match x {
        #[cfg(test)]
        1 => send("Bearer test e"),
        #[cfg(test)]
        2 => {
            send("Bearer test f");
        }
        3 => send("Bearer two"),
        _ => {}
    }
    #[cfg(test)]
    { send("Bearer test g") }
    Held { #[cfg(test)] for_tests: make("Bearer test h", 1), last: send("Bearer three"), #[cfg(test)] end: Vec::new() }
}
#[cfg(not(test))]
fn real() { let quote = '"'; send(r#"Bearer " four"#) }
#[cfg(any(test, feature = "test-support"))]
impl Held { fn mock<A, B>(a: A) -> [u8; 2] where A: Copy, B: Copy { send("Bearer test i") } }
#[cfg(any(windows, test))]
fn on_windows<'a>(x: &'a str) { send("Bearer five") }
#[test]
fn alone() { send("Bearer test j") }
#[cfg(test)]
mod tests { fn t() { send("Bearer test k") } }
fn after_the_tests() { send("Bearer six"); }
"##;

    /// XCT-41: a field, a statement, an arm or a module only tests have ends where it ends; all that follows is read.
    #[test]
    fn what_is_only_for_tests_is_left_out_and_nothing_else() {
        for text in [SAMPLE.to_string(), SAMPLE.replace('\n', "\r\n")] {
            let left = production(&text);
            assert!(!left.contains("Bearer test"), "a test's code is read:\n{left}");
            for kept in ["one", "two", "three", "\" four", "five", "six"] {
                assert!(left.contains(&format!("Bearer {kept}")), "{kept} is not read:\n{left}");
            }
            assert!(!left.contains("for_tests") && left.contains("//! Says #[cfg(test)]"), "{left}");
            assert_eq!(left.lines().count(), SAMPLE.lines().count(), "every line keeps its number");
        }
        assert_eq!(production("#![cfg(test)]\nfn a() {}\n").trim(), "");
        // The cut this replaces read no further than the first field.
        assert!(!SAMPLE.split("#[cfg(test)]").nth(1).is_some_and(|read| read.contains("Bearer six")));
    }

    /// The files only tests compile are told by how they are declared, whatever their names, and theirs with them.
    #[test]
    fn a_tests_own_files_are_left_out() {
        let src = std::env::temp_dir().join(format!("orbiters-source-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&src);
        std::fs::create_dir_all(src.join("pages/other")).unwrap();
        let write = |name: &str, text: &str| std::fs::write(src.join(name), text).unwrap();
        write("main.rs", "mod pages;\n#[cfg(test)]\nmod checks;\nfn main() {}\n");
        write("checks.rs", "#[path = \"checks_more.rs\"]\nmod more;\n");
        write("checks_more.rs", "fn more() {}\n");
        write("pages/mod.rs", "pub(crate) mod tests_of_pages;\nmod other;\n#[cfg(test)]\n#[path = \"held.rs\"]\nmod tests;\n");
        write("pages/tests_of_pages.rs", "fn a_page_about_tests() {}\n");
        write("pages/held.rs", "fn held() {}\n");
        write("pages/other/mod.rs", "use crate::{overlay::{self, Ui as U}, worlds};\nfn f() { super::super::controller::shares::api(); self::g(); }\nmod inner { use super::g; }\n");
        let read: Vec<String> = named(&src).into_iter().map(|(name, _)| name).collect();
        assert_eq!(read, ["main.rs", "pages/mod.rs", "pages/other/mod.rs", "pages/tests_of_pages.rs"]);
        let other = files(&src).into_iter().find(|f| f.module == ["pages", "other"]).unwrap();
        let said: Vec<(usize, String)> = references(&other).into_iter().map(|(line, path)| (line, path.join("::"))).collect();
        let said: Vec<(usize, &str)> = said.iter().map(|(n, p)| (*n, p.as_str())).collect();
        assert_eq!(said, [(1, "overlay"), (1, "overlay::Ui"), (1, "worlds"), (2, "controller::shares::api"), (2, "pages::other::g"), (3, "pages::other::g")]);
        let _ = std::fs::remove_dir_all(&src);
    }
}
