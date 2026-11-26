//! What must not go unseen, held by reading the app's sources: no lock gives up after a panic (`orbiters_core::locks`,
//! XCT-31) and no message goes to a stderr nobody has (`orbiters_core::app_log`, XCT-27).
//! `scripts/visible_failures.py` moves a branch made before them.

use orbiters_core::source::{self, Kind};
use std::path::{Path, PathBuf};

/// What runs in the service, the window and the overlay: each crate, and whether it knows orbiters-core (one that does not
/// recovers by itself: `unwrap_or_else(PoisonError::into_inner)`).
const CRATES: &[(&str, bool)] = &[
    ("orbiters-desktop", true),
    ("orbiters-core", true),
    ("orbiters-n4n0", true),
    ("orbiters-n4n0-core", true),
    ("orbiters-projects", true),
    ("orbiters-ui", false),
    ("orbiters-web", false),
    ("orbiters-spout", false),
    ("orbiters-sound", false),
];

/// The files that hold the rule itself: what they do is said there.
const RULE: &[&str] = &["locks.rs", "app_log.rs", "panic_log.rs"];

/// A developer's command line: what it writes on stderr is its output.
fn command(file: &str) -> bool {
    const COMMANDS: &[&str] = &[
        "background_bench.rs",
        "bench.rs",
        "harness.rs",
        "idle_frames.rs",
        "link_between.rs",
        "link_smoke.rs",
        "bridge/lab.rs",
        "controller/publish_cost.rs",
        "controller/picture_repair.rs",
        "overlay/eye_picture.rs",
        "overlay/orbit/desktop_mac/lab.rs",
        "overlay/orbit/paint_bench.rs",
        "overlay/worlds/sequence.rs",
        "overlay/worlds/shots.rs",
        "overlay/worlds/timing_lab.rs",
        "screenshot.rs",
    ];
    COMMANDS.contains(&file) || file.starts_with("screenshot/")
}

fn src(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("the crates' folder").join(name).join("src")
}

/// The words and signs of `text`, each with where it is: no comment, and a text's content is none of them. Read once, from
/// the start to the end: whatever the source holds, this ends.
fn words(text: &str) -> Vec<(&str, usize)> {
    source::tokens(text).into_iter().map(|t| (if t.kind == Kind::Text { "\"\"" } else { &text[t.at.clone()] }, t.at.start)).collect()
}

/// Where `text` takes a lock and gives up when a thread panicked with it; `own`: or recovers by itself where
/// `orbiters_core::locks` would say so in the log.
fn gives_up(text: &str, own: bool) -> Vec<usize> {
    let t = words(text);
    let is = |i: usize, what: &[&str]| what.iter().enumerate().all(|(n, w)| t.get(i + n).is_some_and(|(x, _)| x == w));
    let mut out = Vec::new();
    for i in 0..t.len() {
        let lock = ["lock", "read", "write", "try_lock"].iter().any(|name| is(i, &[".", name, "(", ")"]));
        if !lock {
            continue;
        }
        let bare = is(i + 4, &[".", "unwrap", "(", ")"]) || is(i + 4, &[".", "expect", "("]);
        let recovers = own && is(i + 4, &[".", "unwrap_or_else", "("]) && {
            let mut depth = 1;
            t[i + 7..].iter().take_while(|(word, _)| {
                match *word {
                    "(" => depth += 1,
                    ")" => depth -= 1,
                    _ => {}
                }
                depth > 0
            }).any(|(word, _)| *word == "into_inner")
        };
        if bare || recovers {
            out.push(text[..t[i].1].matches('\n').count() + 1);
        }
    }
    out
}

/// Where `text` writes to stderr by itself.
fn to_stderr(text: &str) -> Vec<usize> {
    let t = words(text);
    let found = |i: &usize| matches!(t[*i].0, "eprintln" | "eprint" | "dbg") && t.get(i + 1).is_some_and(|(w, _)| *w == "!");
    (0..t.len()).filter(found).map(|i| text[..t[i].1].matches('\n').count() + 1).collect()
}

#[test]
fn no_lock_gives_up_after_a_panic() {
    let mut wrong = Vec::new();
    for (name, core) in CRATES {
        let files = source::named(&src(name));
        assert!(!files.is_empty(), "{name} is read");
        for (file, text) in files {
            if *name == "orbiters-core" && RULE.contains(&file.as_str()) {
                continue;
            }
            wrong.extend(gives_up(&text, *core).into_iter().map(|line| format!("crates/{name}/src/{file}:{line}")));
        }
    }
    assert!(
        wrong.is_empty(),
        "a lock that a panic elsewhere breaks for good (or recovers without a word): `locked()` of orbiters_core::locks, or `unwrap_or_else(PoisonError::into_inner)` below orbiters-core \
         (python scripts/visible_failures.py moves them):\n{}",
        wrong.join("\n")
    );
}

#[test]
fn no_message_goes_to_a_stderr_nobody_has() {
    let files = source::named(&src("orbiters-desktop"));
    let wrong: Vec<String> = files
        .iter()
        .filter(|(file, _)| !command(file))
        .flat_map(|(file, text)| to_stderr(text).into_iter().map(move |line| format!("crates/orbiters-desktop/src/{file}:{line}")))
        .collect();
    assert!(wrong.is_empty(), "an installed build has no stderr: log_error!, log_warn! or log_info! (python scripts/visible_failures.py moves them):\n{}", wrong.join("\n"));
    // The list of commands has no file that is gone.
    for file in ["bench.rs", "bridge/lab.rs", "overlay/worlds/shots.rs", "screenshot.rs"] {
        assert!(command(file) && files.iter().any(|(f, _)| f == file), "{file}");
    }
}

/// The test of the tests: what they look for is found however it is written, what only looks like it is not, and any text
/// is read to its end.
#[test]
fn a_lock_that_gives_up_is_found_however_it_is_written() {
    let text = "fn a() {\n  x.lock().unwrap();\n  x\n    .lock()\n    .unwrap()\n    .push(1);\n  x.read().expect(\"no\");\n  x.write().unwrap_or_else(|e| e.into_inner());\n  x.lock().unwrap_or_else(std::sync::PoisonError::into_inner);\n}\n";
    assert_eq!(gives_up(text, true), [2, 4, 7, 8, 9]);
    assert_eq!(gives_up(text, false), [2, 4, 7]);
    let not = "fn a() {\n  x.locked(); // x.lock().unwrap()\n  let s = \"x.lock().unwrap()\";\n  std::io::stdin().lock().lines();\n  file.lock()?;\n  x.read(&mut b).unwrap();\n}\n/* x.lock().unwrap() */";
    assert_eq!(gives_up(not, true), [0usize; 0]);
    assert_eq!(to_stderr("fn a() { eprintln!(\"x\"); log_warn!(\"eprintln!\"); }\n// eprintln!()\nfn b() { dbg!(1); }"), [1, 3]);
    let production = source::production("fn a() { x.lock().unwrap() }\n#[cfg(test)]\nmod tests { fn b() { x.lock().unwrap() } }\nfn c() -> impl Fn() + use<> { || x.lock().unwrap() }\n");
    assert_eq!(gives_up(&production, true), [1, 4]);
    for odd in ["", "x.lock()", "x.lock().unwrap_or_else(", "'", "\"", "r#\"", "/*", "impl X + use<> {", "é.lock().unwrap()", "b'", "x.lock().", "#[cfg(test)]"] {
        gives_up(odd, true);
        to_stderr(odd);
        source::production(odd);
    }
}
