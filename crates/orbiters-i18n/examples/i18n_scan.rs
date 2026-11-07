//! `cargo run -p orbiters-i18n --example i18n_scan -- report`: every source file's text not yet wrapped, in numbers (tab separated: file, strings, words, error strings, words of those).
//! `... -- list`: every string of them, tab separated (file, line, whether an error, its text as JSON), for the batch list and the glossary.
//! `... -- file <path>`: that file's, line by line, to wrap them. `... -- todo <area>`: what the area's files still have to do.

#[path = "../tests/scan/mod.rs"]
mod scan;

use scan::*;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("file") => {
            for path in &args[1..] {
                for literal in scan_file(&root().join(path)).bare {
                    println!("{path}:{}: {:?}", literal.line, literal.text);
                }
            }
        }
        Some("list") => {
            let mut files = Vec::new();
            rust_files(&root().join("crates"), &mut files);
            for path in files {
                let scanned = scan_file(&path);
                if not_shown(&scanned.file) {
                    continue;
                }
                for (literal, error) in scanned.bare.iter().map(|l| (l, 0)).chain(scanned.errors.iter().map(|l| (l, 1))) {
                    println!("{}\t{}\t{error}\t{}", scanned.file, literal.line, serde_json::Value::String(literal.text.clone()));
                }
            }
        }
        Some("todo") => {
            let area = Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog").join(&args[1]);
            for file in lines_of(&area.join("files.txt")) {
                let scanned = scan_file(&root().join(&file));
                println!("{file}: {} to wrap, {} wrapped", scanned.bare.len(), scanned.keys.len());
            }
        }
        _ => {
            let mut files = Vec::new();
            rust_files(&root().join("crates"), &mut files);
            for path in files {
                let scanned = scan_file(&path);
                if !not_shown(&scanned.file) && !(scanned.bare.is_empty() && scanned.errors.is_empty()) {
                    let words = |list: &[scan::Literal]| list.iter().map(|l| l.text.split_whitespace().count()).sum::<usize>();
                    println!("{}\t{}\t{}\t{}\t{}", scanned.file, scanned.bare.len(), words(&scanned.bare), scanned.errors.len(), words(&scanned.errors));
                }
            }
        }
    }
}
