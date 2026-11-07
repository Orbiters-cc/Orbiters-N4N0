//! Reads Rust sources for the text a person sees. Shared by `tests/coverage.rs` and `examples/i18n_scan.rs`.
//!
//! A text is found in two steps. First where it is: inside [`NOT_SHOWN`] calls (logs, errors for developers, paths, ids), in attributes, comments
//! and tests it is not read at all; inside `tr`, `tr_ctx`, `tr_n`, `tr!` or `mark` it is a key. Then what it looks like ([`looks_like_text`]): file names, paths,
//! identifiers and constants are not text. What is left is what a person would read, not yet translated.
//!
//! A line that has text no person sees, which this cannot tell, ends with `// i18n: not shown`. Whole names that are never translated are in [`NAMES`].

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Calls whose text no person using the app reads. A macro by its `!`, as widgets may be called `error` or `info`; a call by its name, or the path it ends in.
pub const NOT_SHOWN: &[&str] = &[
    // Logs and panics, for developers.
    "println!", "eprintln!", "print!", "eprint!", "dbg!", "panic!", "unreachable!", "todo!", "unimplemented!", "assert!", "assert_eq!", "assert_ne!", "debug_assert!", "debug_assert_eq!", "debug_assert_ne!", "trace!", "debug!", "info!", "warn!", "error!", "log!", "write!", "writeln!", "expect", "expect_err",
    // The program and its surroundings.
    "env!", "option_env!", "include_str!", "include_bytes!", "include!", "concat!", "stringify!", "cfg!", "var", "var_os", "set_var", "remove_var", "Command::new", "arg", "args", "current_dir", "Builder::new",
    // Paths and files.
    "join", "with_extension", "with_file_name", "PathBuf::from", "Path::new", "File::open", "File::create", "create_dir_all", "read_to_string", "ends_with", "starts_with", "strip_prefix", "strip_suffix", "split", "rsplit", "splitn", "rsplitn", "split_once", "rsplit_once", "contains", "find", "rfind", "replace", "trim_start_matches", "trim_end_matches", "eq_ignore_ascii_case",
    // Identifiers of egui and of the network.
    "Id::new", "with", "id_salt", "id", "push_id", "scope_id", "load_texture", "header", "get", "post", "put", "delete", "request", "url", "query", "json!", "get_mut", "contains_key", "Name", "from_blob", "lookup", "pointer",
];

/// Errors. Enzo: translated, especially those that can reach a toast. Reading the source cannot tell which do, so all are held to the test, and one that truly never
/// reaches a person (a log, what a server or an AI reads) is marked `// i18n: not shown`. Counted apart in reports, as they are wrapped differently.
pub const ERRORS: &[&str] = &["anyhow!", "bail!", "ensure!", "context", "with_context", "Err"];

/// Names that are the same in every language, as the glossary has them: a text of nothing but these needs no translation.
pub const NAMES: &[&str] = &[
    "Orbiters", "N4N0", "VRChat", "SteamVR", "Steam", "Unity", "YouTube", "Discord", "Windows", "macOS", "Blender", "GitHub", "Spout", "OSC", "My Avatar", "Orbiters - N4N0", "Orbiters N4N0", "Orbiters Toolkit", "Mixer", "ReFit", "Creator Companion", "VPM", "VCC", "Quest", "Android", "iOS", "PC", "OK", "Fable", "Claude", "Opus", "Sonnet", "Haiku", "OpenAI", "Gemini",
];

pub struct Literal {
    pub line: usize,
    pub text: String,
    /// `tr`, `tr_ctx`, `tr_n`, `tr!` or `mark` holds it.
    pub key: bool,
    /// In a `tr_ctx`, whose text is looked up under its context: not a key on its own.
    pub context: bool,
    /// A `tr_ctx`'s text, with the context written before it: `tr_ctx("the call", "Mute")`.
    pub in_context: Option<String>,
    /// An error's text.
    pub error: bool,
}

/// Every string in `source` outside tests, comments and attributes, and outside calls that are not shown.
pub fn literals(source: &str) -> Vec<Literal> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line) = (0, 1);
    // The calls we are inside, by bracket: `None` for a bracket that is no call, and how many arguments of it have gone by.
    let mut stack: Vec<(char, Option<String>, usize)> = Vec::new();
    let mut path = String::new();
    let lines: Vec<&str> = source.lines().collect();
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '\n' => {
                line += 1;
                i += 1;
            }
            '/' if next == Some('/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if next == Some('*') => {
                let mut depth = 1;
                i += 2;
                while i < chars.len() && depth > 0 {
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                        depth += 1;
                        i += 2;
                    } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            // An attribute: tests are left out whole, the rest only as far as its bracket.
            '#' if next == Some('[') || (next == Some('!') && chars.get(i + 2) == Some(&'[')) => {
                let start = if next == Some('[') { i + 1 } else { i + 2 };
                let mut depth = 0;
                let mut j = start;
                let mut text = String::new();
                while j < chars.len() {
                    match chars[j] {
                        '[' => depth += 1,
                        ']' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        '\n' => line += 1,
                        _ => {}
                    }
                    text.push(chars[j]);
                    j += 1;
                }
                i = j + 1;
                if text.contains("cfg(test)") || text == "[test" {
                    // The item it is on: to its closing brace (or its semicolon).
                    let mut depth = 0;
                    while i < chars.len() {
                        match chars[i] {
                            '{' => depth += 1,
                            '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    i += 1;
                                    break;
                                }
                            }
                            ';' if depth == 0 => {
                                i += 1;
                                break;
                            }
                            '\n' => line += 1,
                            // Braces in strings and characters would miscount.
                            '"' => {
                                i += 1;
                                while i < chars.len() && chars[i] != '"' {
                                    if chars[i] == '\\' {
                                        i += 1;
                                    }
                                    if chars.get(i) == Some(&'\n') {
                                        line += 1;
                                    }
                                    i += 1;
                                }
                            }
                            '\'' if chars.get(i + 2) == Some(&'\'') => i += 2,
                            '\'' if chars.get(i + 1) == Some(&'\\') && chars.get(i + 3) == Some(&'\'') => i += 3,
                            _ => {}
                        }
                        i += 1;
                    }
                }
            }
            'r' | 'b' if matches!(next, Some('"') | Some('#') | Some('r')) && !path.chars().last().is_some_and(|p| p.is_alphanumeric() || p == '_') => {
                // Raw and byte strings.
                let mut j = i;
                let byte = chars[j] == 'b';
                if byte {
                    j += 1;
                }
                let raw = chars.get(j) == Some(&'r');
                if raw {
                    j += 1;
                }
                let mut hashes = 0;
                while chars.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if chars.get(j) != Some(&'"') || (hashes > 0 && !raw) {
                    path.push(c);
                    i += 1;
                    continue;
                }
                j += 1;
                let start_line = line;
                let mut text = String::new();
                while j < chars.len() {
                    if chars[j] == '"' && (0..hashes).all(|h| chars.get(j + 1 + h) == Some(&'#')) {
                        break;
                    }
                    if !raw && chars[j] == '\\' {
                        j += 1;
                    }
                    if chars.get(j) == Some(&'\n') {
                        line += 1;
                    }
                    if let Some(&ch) = chars.get(j) {
                        text.push(ch);
                    }
                    j += 1;
                }
                i = j + 1 + hashes;
                path.clear();
                if !byte {
                    emit(&mut out, &stack, start_line, text, &lines);
                }
            }
            '"' => {
                let start_line = line;
                let mut text = String::new();
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        i += 1;
                        match chars.get(i) {
                            Some('n') => text.push('\n'),
                            Some('t') => text.push('\t'),
                            Some('\r') | Some('\n') => {
                                // A continued line: its indentation is not text. `\r` first in a file checked out with CRLF, which rustc reads as `\n`.
                                if chars[i] == '\r' {
                                    i += 1;
                                }
                                line += 1;
                                i += 1;
                                while chars.get(i).is_some_and(|c| c.is_whitespace()) {
                                    if chars[i] == '\n' {
                                        line += 1;
                                    }
                                    i += 1;
                                }
                                continue;
                            }
                            Some('u') => {
                                let mut hex = String::new();
                                i += 2;
                                while chars.get(i).is_some_and(|c| *c != '}') {
                                    hex.push(chars[i]);
                                    i += 1;
                                }
                                if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                                    text.push(ch);
                                }
                            }
                            Some(&ch) => text.push(ch),
                            None => {}
                        }
                        i += 1;
                        continue;
                    }
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    if chars[i] != '\r' || chars.get(i + 1) != Some(&'\n') {
                        text.push(chars[i]);
                    }
                    i += 1;
                }
                i += 1;
                path.clear();
                emit(&mut out, &stack, start_line, text, &lines);
            }
            // A character, not a lifetime.
            '\'' => {
                if next == Some('\\') {
                    i += 2;
                    while chars.get(i).is_some_and(|c| *c != '\'') {
                        i += 1;
                    }
                    i += 1;
                } else if chars.get(i + 2) == Some(&'\'') {
                    i += 3;
                } else {
                    i += 1;
                }
                path.clear();
            }
            '(' | '[' | '{' => {
                let name = if c == '(' && !path.is_empty() { Some(path.clone()) } else { None };
                stack.push((c, name, 0));
                path.clear();
                i += 1;
            }
            ')' | ']' | '}' => {
                stack.pop();
                path.clear();
                i += 1;
            }
            ',' => {
                if let Some(top) = stack.last_mut() {
                    top.2 += 1;
                }
                path.clear();
                i += 1;
            }
            c if c.is_alphanumeric() || c == '_' || c == ':' || c == '!' => {
                path.push(c);
                i += 1;
            }
            c if c.is_whitespace() => {
                // `foo (` is not written; a space ends a name, except inside a path (`a::\n b`). After one colon the name was a field's, and the call is what comes next.
                if !path.ends_with("::") {
                    path.clear();
                }
                i += 1;
            }
            _ => {
                path.clear();
                i += 1;
            }
        }
    }
    out
}

const KEY_CALLS: &[&str] = &["tr", "tr!", "tr_ctx", "tr_n", "tr_latin", "mark"];

fn is_call(name: &str, list: &[&str]) -> bool {
    let last = name.rsplit("::").next().unwrap_or(name);
    list.contains(&last) || list.contains(&name) || list.iter().any(|n| n.contains("::") && name.ends_with(n))
}

fn emit(out: &mut Vec<Literal>, stack: &[(char, Option<String>, usize)], line: usize, text: String, lines: &[&str]) {
    let names = || stack.iter().filter_map(|s| s.1.as_deref());
    let innermost = stack.iter().rev().find_map(|s| s.1.as_deref());
    if innermost.is_some_and(|n| is_call(n, KEY_CALLS)) {
        let context = innermost.is_some_and(|n| n.ends_with("tr_ctx"));
        // Its second argument, when the first was written out too.
        let second = stack.iter().rev().find(|s| s.1.is_some()).is_some_and(|s| s.2 >= 1);
        let in_context = if context && second { out.last().filter(|l| l.context && l.in_context.is_none()).map(|l| l.text.clone()) } else { None };
        out.push(Literal { line, text, key: true, context, in_context, error: false });
        return;
    }
    if lines.get(line - 1).is_some_and(|l| l.contains("i18n: not shown")) {
        return;
    }
    // `write!(f, ...)` in a `Display` is what an error says, which is shown: only the writing to a file or a log is not.
    let says = lines.get(line - 1).is_some_and(|l| l.contains("write!(f,") || l.contains("writeln!(f,") || l.contains("f.write_str("));
    if !says && names().any(|name| is_call(name, NOT_SHOWN)) {
        return;
    }
    out.push(Literal { line, text, key: false, context: false, in_context: None, error: says || names().any(|name| is_call(name, ERRORS)) });
}

/// Whether a string reads as words for a person, rather than an identifier, a path or a format.
pub fn looks_like_text(text: &str) -> bool {
    // What it says without its values.
    let mut plain = String::new();
    let mut depth = 0;
    for c in text.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth = (depth as i32 - 1).max(0) as usize,
            c if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    let spaced = text != text.trim() || (text.contains('{') && plain != plain.trim());
    let plain = plain.trim();
    if !plain.chars().any(char::is_alphabetic) {
        return false;
    }
    if plain.chars().any(|c| ('\u{E000}'..='\u{F8FF}').contains(&c)) {
        return false;
    }
    if NAMES.contains(&plain) {
        return false;
    }
    // Not words yet: a run of two letters at least.
    let letters = plain.chars().filter(|c| c.is_alphabetic()).count();
    if letters < 2 {
        return false;
    }
    // Addresses and paths, code and markup.
    if ["://", "::", "\\", "=>", "->", "</", "/>", "&&", "||", ".exe", ".dll", "www.", "orbiters.cc", "%s", "%d", "HKEY_", "$("].iter().any(|t| plain.contains(t)) {
        return false;
    }
    if plain.starts_with(['/', '.', '-', '#', '<', '[', '(', '$', '%', '@', '_', '~']) {
        return false;
    }
    if !plain.contains(' ') {
        // One word: only a capitalized word with no other capitals or marks is a word to a person ("Settings"; not "maxWidth", "VRCX", "abc_def", "a.b", "file.png", "a/b").
        let mut chars = plain.chars();
        let first = chars.next().unwrap_or(' ');
        let rest: String = chars.collect();
        let non_latin = plain.chars().any(|c| c.is_alphabetic() && !c.is_ascii() && !c.is_lowercase() && !c.is_uppercase());
        if non_latin {
            return false; // Sample names and already foreign text.
        }
        let capitalized = first.is_uppercase() && rest.chars().all(|c| c.is_lowercase() || matches!(c, '\'' | '\u{2019}' | '-' | '.' | '!' | '?' | ':' | ',' | '…'));
        // A word set apart from a value (`{} avatars`, `{n} of`) or with a space left for what is glued to it (` friends`): a person reads those, whatever the case. A unit (`{} ms`) has its two letters.
        let between = text.trim().starts_with('{') && text.trim().ends_with('}');
        let spoken = spaced && (between || plain.chars().count() >= 3) && plain.chars().all(|c| c.is_lowercase() || c == '\'' || c == '\u{2019}' || c == '-');
        return capitalized || spoken;
    }
    // Several words: at least one real word in lower case, or a capitalized one.
    let words: Vec<&str> = plain.split_whitespace().collect();
    let wordy = words.iter().filter(|w| {
        let w = w.trim_matches(|c: char| !c.is_alphanumeric());
        w.chars().count() >= 2 && w.chars().all(char::is_alphabetic)
    });
    if wordy.count() == 0 {
        return false;
    }
    // Command lines and code that happens to hold spaces.
    if words.iter().any(|w| w.starts_with("--") || w.contains("=") && !w.ends_with('=') && w.chars().all(|c| c.is_ascii())) && !plain.contains(". ") {
        return false;
    }
    // Names of nothing but capitals and digits, as a key would be.
    if plain.chars().all(|c| !c.is_lowercase()) && !words.iter().any(|w| w.len() > 5) {
        return false;
    }
    true
}

/// Rust files under `dir`, left to right.
pub fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if !matches!(path.file_name().and_then(|n| n.to_str()), Some("target" | "tests" | "benches" | "examples")) {
                rust_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// This repository's root, seen from this crate.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("the repository")
}

/// A file's path from the repository's root, with forward slashes.
pub fn relative(path: &Path) -> String {
    path.strip_prefix(root()).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// What a catalog area says about itself: the files it covers, and the exact texts it leaves alone.
pub fn lines_of(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path).map_or_else(|_| Vec::new(), |s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty() && !l.starts_with('#')).collect())
}

/// The text of a file with what it does not show set aside, in order.
pub struct Scanned {
    pub file: String,
    /// Text a person would read, not wrapped.
    pub bare: Vec<Literal>,
    /// Keys: wrapped text.
    pub keys: Vec<Literal>,
    /// Errors, unwrapped: see [`ERRORS`].
    pub errors: Vec<Literal>,
}

pub fn scan_file(path: &Path) -> Scanned {
    let source = std::fs::read_to_string(path).unwrap_or_default();
    let mut bare = Vec::new();
    let mut keys = Vec::new();
    let mut errors = Vec::new();
    for literal in literals(&source) {
        if literal.key {
            keys.push(literal);
        } else if looks_like_text(&literal.text) {
            if literal.error {
                errors.push(literal);
            } else {
                bare.push(literal);
            }
        }
    }
    let file = relative(path);
    if errors_only(&file) {
        bare.clear();
    }
    Scanned { file, bare, keys, errors }
}

/// The files nobody reads the text of (`not-shown.txt`): those paths, and all under those folders.
pub fn not_shown(file: &str) -> bool {
    lines_of(&Path::new(env!("CARGO_MANIFEST_DIR")).join("not-shown.txt")).iter().any(|p| file == p || file.starts_with(&format!("{p}/")))
}

/// The files whose other strings are internal names but whose errors can reach a person (`not-shown.txt`'s `errors-only` lines).
pub fn errors_only(file: &str) -> bool {
    lines_of(&Path::new(env!("CARGO_MANIFEST_DIR")).join("not-shown.txt")).iter().filter_map(|l| l.strip_prefix("errors-only ")).any(|p| file == p || file.starts_with(&format!("{p}/")))
}
