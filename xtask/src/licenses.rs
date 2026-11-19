//! The licence file shipped with the programs (`THIRD_PARTY_LICENSES.md`, written by `scripts/third_party_licenses.py`)
//! names every crate built into them, at the version built: a release whose Cargo.lock links a crate the file does not
//! name stops before anything is compiled. On 2026-11-19 the file was two weeks behind (BUILD-11): the WebView2 crates,
//! `tempfile` and `unicode-normalization` were linked and their MIT and Unicode notices shipped nowhere.

use anyhow::Context as _;
use std::collections::{BTreeSet, HashMap, HashSet};

/// The file, from the repository's root.
pub const FILE: &str = "THIRD_PARTY_LICENSES.md";
/// The platform the file is written for (the script's PLATFORM).
const PLATFORM: &str = "x86_64-pc-windows-msvc";

/// Refuses a release whose programs link a crate (`name version`) that the licence file does not name.
pub fn check(programs: &[&str]) -> anyhow::Result<()> {
    let root = crate::root();
    let meta = crate::output(crate::cargo().args(["metadata", "--format-version", "1", "--filter-platform", PLATFORM, "--locked", "--offline"]))?;
    let meta: serde_json::Value = serde_json::from_str(&meta).context("cargo metadata's answer")?;
    let shipped = shipped(&meta, programs)?;
    let text = std::fs::read_to_string(root.join(FILE)).with_context(|| format!("{FILE} cannot be read"))?;
    let missing = missing(&shipped, &text);
    anyhow::ensure!(
        missing.is_empty(),
        "{FILE} does not name {} crate(s) the programs link: {}. Run `python scripts/third_party_licenses.py`, look at what it adds (a native library \
         a crate links needs its own notice in the script's NATIVE) and commit it",
        missing.len(),
        missing.join(", ")
    );
    println!("{FILE} names all {} crates the programs link", shipped.len());
    Ok(())
}

/// `name version` of every registry or git package `programs` reach through normal dependencies (not build or dev
/// ones), procedural macros left out: what is built into them, as the script reckons it.
fn shipped(meta: &serde_json::Value, programs: &[&str]) -> anyhow::Result<BTreeSet<String>> {
    let packages: HashMap<&str, &serde_json::Value> =
        meta["packages"].as_array().context("no packages")?.iter().filter_map(|p| Some((p["id"].as_str()?, p))).collect();
    let nodes: HashMap<&str, &serde_json::Value> =
        meta["resolve"]["nodes"].as_array().context("no resolve")?.iter().filter_map(|n| Some((n["id"].as_str()?, n))).collect();
    let mut stack: Vec<&str> = meta["workspace_members"]
        .as_array()
        .context("no workspace members")?
        .iter()
        .filter_map(|m| m.as_str())
        .filter(|id| packages.get(id).and_then(|p| p["name"].as_str()).is_some_and(|n| programs.contains(&n)))
        .collect();
    anyhow::ensure!(stack.len() == programs.len(), "cargo metadata names {} of the {} programs", stack.len(), programs.len());
    let mut seen = HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let node = nodes.get(id).with_context(|| format!("{id} is not resolved"))?;
        for dep in node["deps"].as_array().into_iter().flatten() {
            let normal = dep["dep_kinds"].as_array().into_iter().flatten().any(|k| k["kind"].is_null());
            let Some(pkg) = dep["pkg"].as_str() else { continue };
            let macro_crate = packages
                .get(pkg)
                .and_then(|p| p["targets"].as_array())
                .is_some_and(|t| t.iter().any(|t| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "proc-macro"))));
            if normal && !macro_crate {
                stack.push(pkg);
            }
        }
    }
    Ok(seen
        .into_iter()
        .filter_map(|id| packages.get(id))
        .filter(|p| !p["source"].is_null())
        .filter_map(|p| Some(format!("{} {}", p["name"].as_str()?, p["version"].as_str()?)))
        .collect())
}

/// The crates of `shipped` that the licence file `text` does not name, as the script names them: in its "Used by …:"
/// lines and its "Under the standard … text (…), published without a copy of their own: ….", a comma between each.
fn missing(shipped: &BTreeSet<String>, text: &str) -> Vec<String> {
    let mut named = HashSet::new();
    for line in text.lines() {
        let list = if let Some(rest) = line.strip_prefix("Used by ") {
            rest.strip_suffix(':')
        } else if line.starts_with("Under the standard ") {
            line.split_once("published without a copy of their own: ").and_then(|(_, rest)| rest.strip_suffix('.'))
        } else {
            None
        };
        named.extend(list.into_iter().flat_map(|l| l.split(", ")).map(str::trim));
    }
    shipped.iter().filter(|c| !named.contains(c.as_str())).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// A crate missing from the file, or named at another version, is found; one named in either kind of list is not.
    #[test]
    fn a_crate_the_file_does_not_name_at_its_version_is_missing() {
        let text = "# Third-party licenses\n\n## MIT\n\nUnder the standard MIT text (the first text below), published without a copy of their own: \
                    a 1.0.0, b 0.2.1.\n\nUsed by c 3.27.0, d 0.1.26:\n\n```text\nUsed by z 9.9.9:\n```\n\nUsed by e 1.0.0:\n";
        assert_eq!(missing(&set(&["a 1.0.0", "b 0.2.1", "c 3.27.0", "d 0.1.26", "e 1.0.0"]), text), Vec::<String>::new());
        assert_eq!(missing(&set(&["c 3.27.0", "tempfile 3.27.0"]), text), ["tempfile 3.27.0"]);
        assert_eq!(missing(&set(&["d 0.1.27"]), text), ["d 0.1.27"], "another version than the one named");
        assert_eq!(missing(&set(&["e 1.0"]), text), ["e 1.0"], "a prefix of a named version is not named");
    }

    /// The walk the script makes: normal dependencies of the programs only, procedural macros and path crates left out.
    #[test]
    fn what_the_programs_link_is_their_normal_dependencies() {
        let meta = serde_json::json!({
            "workspace_members": ["app", "tool"],
            "packages": [
                {"id": "app", "name": "orbiters-desktop", "version": "0.3.6", "source": null, "targets": [{"kind": ["bin"]}]},
                {"id": "tool", "name": "xtask", "version": "0.3.6", "source": null, "targets": [{"kind": ["bin"]}]},
                {"id": "lib", "name": "tempfile", "version": "3.27.0", "source": "registry", "targets": [{"kind": ["lib"]}]},
                {"id": "deep", "name": "fastrand", "version": "2.3.0", "source": "registry", "targets": [{"kind": ["lib"]}]},
                {"id": "mac", "name": "serde_derive", "version": "1.0.0", "source": "registry", "targets": [{"kind": ["proc-macro"]}]},
                {"id": "build", "name": "cc", "version": "1.2.0", "source": "registry", "targets": [{"kind": ["lib"]}]},
                {"id": "dev", "name": "insta", "version": "1.0.0", "source": "registry", "targets": [{"kind": ["lib"]}]},
                {"id": "toolonly", "name": "semver", "version": "1.0.0", "source": "registry", "targets": [{"kind": ["lib"]}]}
            ],
            "resolve": {"nodes": [
                {"id": "app", "deps": [
                    {"pkg": "lib", "dep_kinds": [{"kind": null}]},
                    {"pkg": "mac", "dep_kinds": [{"kind": null}]},
                    {"pkg": "build", "dep_kinds": [{"kind": "build"}]},
                    {"pkg": "dev", "dep_kinds": [{"kind": "dev"}]}
                ]},
                {"id": "lib", "deps": [{"pkg": "deep", "dep_kinds": [{"kind": null}]}]},
                {"id": "deep", "deps": []},
                {"id": "mac", "deps": []},
                {"id": "build", "deps": []},
                {"id": "dev", "deps": []},
                {"id": "tool", "deps": [{"pkg": "toolonly", "dep_kinds": [{"kind": null}]}]},
                {"id": "toolonly", "deps": []}
            ]}
        });
        assert_eq!(shipped(&meta, &["orbiters-desktop"]).unwrap(), set(&["fastrand 2.3.0", "tempfile 3.27.0"]));
        assert!(shipped(&meta, &["orbiters-desktop", "orbiters-call"]).is_err(), "a program cargo does not know");
    }

    /// The repository's own file names every crate its programs link (the check a release makes, without cargo's
    /// network: `--offline`).
    #[test]
    fn the_repository_s_licence_file_is_complete() {
        check(&crate::dist::PROGRAMS).expect("run python scripts/third_party_licenses.py");
    }
}
