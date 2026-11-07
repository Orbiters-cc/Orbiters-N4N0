//! Nothing listens beyond loopback unless the run allows it. On Windows, the first time a program binds a socket on a
//! network interface, its firewall asks the person in a window that takes the foreground, over a game in VR; a test
//! program is named by a hash of its build, so every new build asks again. Agents' test runs set
//! `ORBITERS_NO_NETWORK_LISTENING` (`orbiters_core::osc::no_network_listening`), and every place that would bind every
//! interface must check it first.
//!
//! This reads the workspace's sources: an address that means "every interface" (`0.0.0.0`, `[::]`, `UNSPECIFIED`, or a
//! constant holding one) must appear only in a function that asks `no_network_listening()` before it, or on a line that
//! says why it is no socket (`// network: not a socket (...)`).

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != "target") {
                rust_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Where a function starts: `fn`, with what may come before it.
fn starts_a_function(line: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    let at = words.iter().position(|w| *w == "fn");
    at.is_some_and(|i| words[..i].iter().all(|w| w.starts_with("pub") || matches!(*w, "async" | "unsafe" | "const" | "extern" | "\"C\"")))
}

#[test]
fn nothing_listens_beyond_loopback_unless_the_run_allows_it() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    rust_files(&crates, &mut files);
    assert!(files.len() > 100, "the workspace's sources are read: {}", files.len());
    // An address, not any four zeros (a pixel, a buffer): `SocketAddr::from(([0, 0, 0, 0], port))` is how one is written.
    let every = ["\"0.0.0.0", "(([0, 0, 0, 0]", "Ipv4Addr::new(0, 0, 0, 0)", "UNSPECIFIED", "\"[::]", "\"[::]:"];
    let mut wrong = Vec::new();
    for path in &files {
        if path.ends_with("listening.rs") {
            continue;
        }
        let source = std::fs::read_to_string(path).unwrap_or_default();
        let lines: Vec<&str> = source.lines().collect();
        // Constants that hold such an address count as it.
        let names: Vec<String> = lines
            .iter()
            .filter_map(|l| {
                let l = l.trim_start();
                let rest = l.strip_prefix("pub ").unwrap_or(l).strip_prefix("const ")?;
                let (name, value) = rest.split_once(':')?;
                every.iter().any(|a| value.contains(a)).then(|| name.trim().to_string())
            })
            .collect();
        for (i, line) in lines.iter().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            let is_const = names.iter().any(|n| code.contains(&format!("const {n}:")));
            let hit = every.iter().any(|a| code.contains(a)) || names.iter().any(|n| code.split(|c: char| !c.is_alphanumeric() && c != '_').any(|w| w == n));
            if !hit || is_const || line.contains("network: not a socket") {
                continue;
            }
            let start = (0..=i).rev().find(|&j| starts_a_function(lines[j])).unwrap_or(0);
            if !lines[start..i].iter().any(|l| l.split("//").next().unwrap_or_default().contains("no_network_listening()")) {
                wrong.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
            }
        }
    }
    assert!(wrong.is_empty(), "every interface, with no check of ORBITERS_NO_NETWORK_LISTENING before it:\n{}", wrong.join("\n"));
}
