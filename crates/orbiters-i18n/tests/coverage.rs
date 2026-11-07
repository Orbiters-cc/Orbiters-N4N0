//! Fails on text a person sees that is not translated: in the files a catalog area says it covers (`catalog/<area>/files.txt`), every string that
//! reads as words, errors included, must be wrapped (or on the area's `allow.txt`, or its line marked `// i18n: not shown`), and every wrapped text must have a translation in
//! all four languages, with the same `{names}` as the English.

#[path = "scan/mod.rs"]
mod scan;

use orbiters_i18n::{placeholders, Language, CATALOGS};
use scan::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn catalog_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog")
}

fn areas() -> Vec<std::path::PathBuf> {
    let mut areas: Vec<_> = std::fs::read_dir(catalog_root()).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    areas.sort();
    areas
}

/// Each language's texts as `(area, English, translation)`.
fn entries(language: Language) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (area, tag, json) in CATALOGS.iter().filter(|c| c.1 == language.tag()) {
        let value: serde_json::Value = serde_json::from_str(json).unwrap_or_else(|e| panic!("{area}/{tag}.json is not JSON: {e}"));
        let serde_json::Value::Object(map) = value else { panic!("{area}/{tag}.json is not an object") };
        for (english, translated) in map {
            let serde_json::Value::String(translated) = translated else { panic!("{area}/{tag}.json: {english:?} is not a string") };
            out.push((area.to_string(), english, translated));
        }
    }
    out
}

#[test]
fn every_area_has_all_four_languages_and_says_what_it_covers() {
    for area in areas() {
        for language in Language::TRANSLATED {
            assert!(area.join(format!("{}.json", language.tag())).is_file(), "{} has no {}", area.display(), language.tag());
        }
        assert!(area.join("files.txt").is_file(), "{} has no files.txt", area.display());
    }
}

#[test]
fn no_file_belongs_to_two_areas() {
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for area in areas() {
        let name = area.file_name().unwrap().to_string_lossy().to_string();
        for file in lines_of(&area.join("files.txt")) {
            assert!(root().join(&file).is_file(), "{name} covers {file}, which does not exist");
            if let Some(other) = seen.insert(file.clone(), name.clone()) {
                panic!("{file} is in both {other} and {name}");
            }
        }
    }
}

/// `{name}` must survive translation: a translation that loses or adds one would show a hole, or a brace, at run time.
#[test]
fn every_translation_keeps_the_placeholders_of_its_english() {
    let mut wrong = Vec::new();
    for language in Language::TRANSLATED {
        for (area, english, translated) in entries(language) {
            let key = english.rsplit('\u{4}').next().unwrap_or(&english);
            if placeholders(key) != placeholders(&translated) {
                wrong.push(format!("{area}/{}: {english:?} -> {translated:?}", language.tag()));
            }
            if translated.trim().is_empty() {
                wrong.push(format!("{area}/{}: {english:?} is empty", language.tag()));
            }
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

/// Two areas giving one English text two translations: the glossary decides, or `tr_ctx` tells them apart. Which one showed would depend on the order of folders.
#[test]
fn one_english_text_has_one_translation() {
    let mut wrong = Vec::new();
    for language in Language::TRANSLATED {
        let mut seen: BTreeMap<String, (String, String)> = BTreeMap::new();
        for (area, english, translated) in entries(language) {
            match seen.get(&english) {
                Some((other, was)) if *was != translated => wrong.push(format!("{}: {english:?} is {was:?} in {other} and {translated:?} in {area}", language.tag())),
                _ => {
                    seen.insert(english, (area, translated));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

/// The test the batches are held to.
#[test]
fn what_an_area_covers_is_wrapped_and_translated() {
    let mut known: BTreeMap<Language, BTreeSet<String>> = BTreeMap::new();
    for language in Language::TRANSLATED {
        known.insert(language, entries(language).into_iter().map(|e| e.1).collect());
    }
    let mut problems = Vec::new();
    for area in areas() {
        let name = area.file_name().unwrap().to_string_lossy().to_string();
        let allowed: BTreeSet<String> = lines_of(&area.join("allow.txt")).into_iter().collect();
        for file in lines_of(&area.join("files.txt")) {
            let scanned = scan_file(&root().join(&file));
            for literal in scanned.bare.iter().chain(&scanned.errors) {
                if !allowed.contains(literal.text.as_str()) {
                    problems.push(format!("{file}:{}: not wrapped: {:?}", literal.line, literal.text));
                }
            }
            for literal in scanned.keys.iter().filter(|l| !l.context) {
                for language in Language::TRANSLATED {
                    if !known[&language].contains(&literal.text) {
                        problems.push(format!("{file}:{}: no {} for {:?} (add it to catalog/{name}/{}.json)", literal.line, language.tag(), literal.text, language.tag()));
                    }
                }
            }
        }
    }
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
}

/// The end of the work: every file of the app, with text a person reads, is covered by an area. Ignored until then; `cargo test -p orbiters-i18n -- --ignored` says what is left.
#[test]
#[ignore = "until every batch is in"]
fn every_file_with_text_is_covered() {
    let covered: BTreeSet<String> = areas().iter().flat_map(|a| lines_of(&a.join("files.txt"))).collect();
    let mut files = Vec::new();
    rust_files(&root().join("crates"), &mut files);
    let left: Vec<String> = files
        .iter()
        .filter_map(|path| {
            let scanned = scan_file(path);
            let count = scanned.bare.len() + scanned.errors.len();
            (count > 0 && !covered.contains(&scanned.file) && !not_shown(&scanned.file)).then(|| format!("{}: {count}", scanned.file))
        })
        .collect();
    assert!(left.is_empty(), "{} files left:\n{}", left.len(), left.join("\n"));
}

/// `mark` and `tr` take literals only: a key the scan cannot read could not be checked.
#[test]
fn the_reader_tells_text_from_code() {
    for text in ["Settings", "Worlds in 3D", "Press {key} to start", "Hide tiles", "Can't reach Orbiters.", "Loading…", "Sound in VR", "3D picture of people", "Try again", "THE WEEKND FROM SOMEWHERE ELSE", "Saving..."] {
        assert!(looks_like_text(text), "{text:?} is text");
    }
    for text in ["", "{}", "{} {}", "foo_bar", "maxWidth", "file.png", "a/b/c", "https://orbiters.cc/x", "n4n0/calls", "wrld_123", "VRCX", "orbit-wheel", "--hidden", "VRChat", "N4N0", "\u{E000}", "-1", "x", "en-US", "application/json", "{a}/{b}.json", "C:\\Users"] {
        assert!(!looks_like_text(text), "{text:?} is not text");
    }
    let source = r##"
        // "A comment"
        /// "A doc comment"
        #[serde(rename = "An attribute")]
        fn f() {
            println!("A log line");
            let a = "Shown text";
            let b = tr("Wrapped text");
            let c = tr!("Wrapped {x} text", x = 1);
            let d = mark("Marked text");
            let e = std::env::var("AN_ENV_VAR");
            let f = r#"Raw text shown"#;
            let g = tr_ctx("A context", "With a context");
            let h = "Line marked"; // i18n: not shown
            let i = Id::new("An id");
            let j = 'x';
            let k: &'static str = "Static text";
            let l = format!("Formatted {} text", 1);
            let m = foo("Argument text", bar("Nested text"));
            widgets::field(ui, "A label", Some("About it"), 1.0, |ui| {});
            feedback::error(ui, "A widget called error");
            error!("A log called error");
            bail!("An error");
            let n = thing().context("Another error")?;
        }
        #[cfg(test)]
        mod tests { fn t() { let x = "Test text"; } }
        fn after() { let z = "After tests"; }
    "##;
    let found = literals(source);
    let shown: Vec<&str> = found.iter().filter(|l| !l.key).map(|l| l.text.as_str()).collect();
    assert_eq!(shown, ["Shown text", "Raw text shown", "Static text", "Formatted {} text", "Argument text", "Nested text", "A label", "About it", "A widget called error", "An error", "Another error", "After tests"]);
    let errors: Vec<&str> = found.iter().filter(|l| l.error).map(|l| l.text.as_str()).collect();
    assert_eq!(errors, ["An error", "Another error"], "counted apart");
    let keys: Vec<&str> = found.iter().filter(|l| l.key && !l.context).map(|l| l.text.as_str()).collect();
    assert_eq!(keys, ["Wrapped text", "Wrapped {x} text", "Marked text"], "a context is not a key on its own");
}
