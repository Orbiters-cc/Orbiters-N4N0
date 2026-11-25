//! Putting a version in place, so that a power cut or a kill at any moment leaves a version that can be put back whole.
//!
//! Before the first file moves, a journal in the install folder says what is about to: which files are there and go aside,
//! which are new, and the install record that will say it is done. Each file is then moved aside and its new one in at
//! once, so that one file at most is missing at any moment, the app's own program last. Writing the record is what ends it:
//! a journal found later ([`recover`]: at the next start of a setup, or at the next sign-in, which Windows is told of) is
//! either of a swap that ended, and only tidied up, or of one that did not, and all of it goes back. The journal goes
//! first in both cases, then the files it named.

use crate::install::{contained, make_parent, remove_empty_dirs, remove_file, remove_tree, rename, BACKUP, STAGING};
use anyhow::Context as _;
use orbiters_i18n::tr;
use orbiters_update::{payload, InstallRecord, APP_EXE, PRODUCT, RECORD_FILE};
use std::io::{Read as _, Write as _};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// The journal's file, in the install folder.
pub(crate) const JOURNAL: &str = ".setup-journal";

/// What a swap does.
#[derive(Clone, Debug, PartialEq)]
struct Plan {
    /// Files that are there: each goes to the backup folder, and comes back.
    aside: Vec<String>,
    /// Files that were not: each comes from the staging folder, and goes back there.
    fresh: Vec<String>,
    /// The record that says the swap ended.
    to: InstallRecord,
}

impl Plan {
    fn paths(&self) -> impl DoubleEndedIterator<Item = &str> {
        self.aside.iter().chain(&self.fresh).map(String::as_str)
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({ "product": PRODUCT, "aside": self.aside, "fresh": self.fresh, "to": self.to })
    }

    fn read(bytes: &[u8]) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        let list = |name: &str| v[name].as_array()?.iter().map(|p| p.as_str().map(str::to_string)).collect::<Option<Vec<_>>>();
        (v["product"] == PRODUCT).then_some(())?;
        Some(Self { aside: list("aside")?, fresh: list("fresh")?, to: serde_json::from_value(v["to"].clone()).ok()? })
    }
}

/// The journal, held open and shared with nobody while a swap or a recovery is at work: two setups never work on one folder.
struct Journal {
    file: Option<std::fs::File>,
    path: PathBuf,
}

impl Journal {
    fn begin(dir: &Path, plan: &Plan) -> anyhow::Result<Self> {
        let path = dir.join(JOURNAL);
        let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).share_mode(0).open(&path).with_context(|| tr!("{path} cannot be written", path = path.display()))?;
        // Whole on the disk before the first move.
        file.write_all(&serde_json::to_vec(&plan.json())?)?;
        file.sync_all()?;
        Ok(Self { file: Some(file), path })
    }

    /// The journal a swap left in `dir`. One that cannot be read was cut before anything moved.
    fn left(dir: &Path) -> anyhow::Result<Option<(Self, Plan)>> {
        const SHARING_VIOLATION: i32 = 32;
        let path = dir.join(JOURNAL);
        match std::fs::symlink_metadata(&path) {
            Ok(m) if m.is_file() && m.file_attributes() & 0x400 == 0 => {}
            _ => return Ok(None),
        }
        let mut file = match std::fs::OpenOptions::new().read(true).share_mode(0).open(&path) {
            Err(e) if e.raw_os_error() == Some(SHARING_VIOLATION) => anyhow::bail!("{}", tr!("Another setup is at work in {folder}: nothing was changed.", folder = dir.display())),
            opened => opened?,
        };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(Plan::read(&bytes).map(|plan| (Self { file: Some(file), path }, plan)))
    }

    /// What it was for is over. An error when it could not be removed: it then still says what it said.
    fn over(mut self) -> std::io::Result<()> {
        drop(self.file.take());
        remove_file(&self.path)
    }
}

/// Moves the unpacked files of `index` from `staging` into `dir`, what they replace (and what the `previous` install had that
/// this one has not) into `backup`, and writes `record`. A move that fails puts back the ones made; so does [`recover`]
/// after a cut.
pub(crate) fn swap(dir: &Path, staging: &Path, backup: &Path, index: &payload::Index, previous: Option<&InstallRecord>, record: &InstallRecord) -> anyhow::Result<()> {
    let new: Vec<&str> = index.files.iter().map(|f| f.path.as_str()).collect();
    let gone: Vec<&str> = previous.map(|r| r.files.iter().map(String::as_str).filter(|f| !new.contains(f)).collect()).unwrap_or_default();
    // The app's own program last: until then the one that starts is the old one.
    let is_app = |p: &&str| p.eq_ignore_ascii_case(APP_EXE);
    let order: Vec<&str> = new.iter().chain(&gone).copied().filter(|p| !is_app(p)).chain(new.iter().chain(&gone).copied().filter(is_app)).collect();
    let there = |p: &&str| contained(dir, p).is_some_and(|dest| dest.is_file());
    let plan = Plan {
        aside: order.iter().copied().filter(there).map(str::to_string).collect(),
        fresh: order.iter().copied().filter(|p| new.contains(p) && !there(p)).map(str::to_string).collect(),
        to: record.clone(),
    };
    let journal = Journal::begin(dir, &plan)?;
    let result = (|| -> anyhow::Result<()> {
        for path in order {
            let is_new = new.contains(&path);
            let (Some(dest), Some(to), Some(from)) = (contained(dir, path), contained(backup, path), contained(staging, path)) else {
                anyhow::ensure!(!is_new, "{}", tr!("{path} is reached through a link to another folder", path = path));
                continue;
            };
            if dest.is_file() {
                make_parent(&to)?;
                rename(&dest, &to).with_context(|| tr!("{path} is in use by another program", path = dest.display()))?;
            }
            if is_new {
                make_parent(&dest)?;
                rename(&from, &dest).with_context(|| tr!("{path} cannot be written", path = dest.display()))?;
            }
        }
        record.write(dir).with_context(|| tr!("{path} cannot be written", path = dir.join(RECORD_FILE).display()))
    })();
    if let Err(e) = result {
        // As it was: the new files out, the old ones back. Each move checked (F16): what could not be put back is named, and
        // the backup folder then stays as it is, with the journal that says what is in it.
        let stuck = put_back(dir, staging, backup, &plan);
        if !stuck.is_empty() {
            return Err(e.context(not_put_back(&stuck, backup)));
        }
        if journal.over().is_ok() {
            remove_tree(staging);
        }
        return Err(e.context(tr("The previous version was put back")));
    }
    // (One that stays is of a swap whose record is written: the next setup only tidies up after it.)
    let _ = journal.over();
    Ok(())
}

fn not_put_back(stuck: &[String], backup: &Path) -> String {
    tr!("The previous version could not be fully put back: {files}. Its files are kept in {folder}.", files = stuck.join(", "), folder = backup.display())
}

/// Every file of `plan` where it was before the swap, whatever it had reached: what could not be. Cut itself, it goes on from
/// where it stopped.
fn put_back(dir: &Path, staging: &Path, backup: &Path, plan: &Plan) -> Vec<String> {
    let mut stuck = Vec::new();
    for path in plan.paths().rev() {
        let (Some(dest), Some(old), Some(new)) = (contained(dir, path), contained(backup, path), contained(staging, path)) else {
            stuck.push(path.to_string());
            continue;
        };
        let moved = |from: &Path, to: &Path| make_parent(to).is_ok() && rename(from, to).is_ok();
        let was_there = plan.aside.iter().any(|p| p == path);
        let back = if was_there {
            // (Not in the backup folder: not moved yet, or back already.)
            !old.is_file() || ((!dest.exists() || (!new.exists() && moved(&dest, &new))) && moved(&old, &dest))
        } else {
            // (In the staging folder: not moved yet, or back already.)
            new.exists() || !dest.exists() || moved(&dest, &new)
        };
        if !back {
            stuck.push(path.to_string());
        }
    }
    stuck
}

/// Ends what a cut swap left in `dir`: everything back as it was, or, when its record was written, the old files removed.
/// Whether there was one. An error when the previous version could not be put back whole: its files and the journal stay.
pub fn recover(dir: &Path) -> anyhow::Result<bool> {
    let Some((journal, plan)) = Journal::left(dir)? else { return Ok(false) };
    let (staging, backup) = (dir.join(STAGING), dir.join(BACKUP));
    if InstallRecord::read(dir).as_ref() != Some(&plan.to) {
        let stuck = put_back(dir, &staging, &backup, &plan);
        anyhow::ensure!(stuck.is_empty(), "{}", not_put_back(&stuck, &backup));
    }
    // The journal first: without its files, what it says would be taken for a swap to undo.
    if journal.over().is_ok() {
        for path in plan.paths() {
            for folder in [&staging, &backup] {
                if let Some(file) = contained(folder, path).filter(|f| f.is_file()) {
                    let _ = remove_file(&file);
                }
            }
        }
        remove_empty_dirs(&staging);
        remove_empty_dirs(&backup);
    }
    Ok(true)
}

/// The programs a cut swap in `dir` was about, which must not run while it is put back.
pub(crate) fn left_programs(dir: &Path) -> Vec<String> {
    let left = Journal::left(dir).ok().flatten();
    left.map(|(_, plan)| plan.paths().map(str::to_string).collect()).unwrap_or_default()
}

/// The command that puts back what a cut swap in `dir` leaves: `setup`, with its folder named where Windows' 260 characters
/// for such a command allow it (it is found otherwise). `None` when not even that fits.
pub(crate) fn recover_command(setup: &Path, dir: &Path) -> Option<String> {
    const MOST: usize = 260;
    let short = format!("\"{}\" --recover", setup.display());
    let long = format!("{short} --dir \"{}\"", dir.display());
    [long, short].into_iter().find(|c| c.encode_utf16().count() <= MOST)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::{cut_after, tests::*};

    /// A folder of the install as it is between two starts of anything: each file and what it holds, the scratch folders and
    /// the journal included.
    fn tree(dir: &Path) -> Vec<(String, String)> {
        fn walk(top: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(top, &p, out);
                } else {
                    out.push((p.strip_prefix(top).unwrap().to_string_lossy().replace('\\', "/"), String::from_utf8_lossy(&std::fs::read(&p).unwrap()).into_owned()));
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    const NEW: &[&str] = &[APP_EXE, "orbiters-capture.exe", "sub/b.dll", "fresh/c.dll"];

    /// An install of 0.9.0 with 1.0.0 unpacked beside it.
    fn before(dir: &Path) -> (payload::Index, InstallRecord, InstallRecord) {
        let old = record(&[APP_EXE, "orbiters-capture.exe", "sub/b.dll", "gone.dll"]);
        for (name, text) in [(APP_EXE, "old app"), ("orbiters-capture.exe", "old capture"), ("sub/b.dll", "old b"), ("gone.dll", "old gone"), ("mine.txt", "the person's"), ("sub/mine.txt", "theirs")] {
            make_parent(&dir.join(name)).unwrap();
            std::fs::write(dir.join(name), text).unwrap();
        }
        old.write(dir).unwrap();
        for name in NEW {
            let staged = dir.join(STAGING).join(name);
            make_parent(&staged).unwrap();
            std::fs::write(staged, format!("new {name}")).unwrap();
        }
        let to = InstallRecord { version: "1.0.0".into(), installed: "2026-11-25T00:00:00Z".into(), files: NEW.iter().map(|f| f.to_string()).collect(), ..old.clone() };
        (index(NEW), old, to)
    }

    /// PRJ-40: cut after any move (a power cut, a kill), the next start of a setup finds one version whole: the old one, or the
    /// new one once its record is written. And a cut of the recovery itself is recovered from.
    #[test]
    fn a_swap_cut_at_any_moment_leaves_a_whole_version() {
        let whole_old: Vec<(String, String)> = {
            let dir = scratch("cut-before");
            before(&dir);
            remove_tree(&dir.join(STAGING));
            let tree = tree(&dir);
            remove_tree(&dir);
            tree
        };
        let mut ended = 0;
        for cut in 0..40 {
            let dir = scratch("cut");
            let (index, old, to) = before(&dir);
            let (staging, backup) = (dir.join(STAGING), dir.join(BACKUP));
            let outcome = cut_after(Some(cut), || swap(&dir, &staging, &backup, &index, Some(&old), &to));
            // At no moment is more than one of its files missing, and the app's program is a whole one or on its way.
            let missing = old.files.iter().filter(|f| **f != "gone.dll").filter(|f| !dir.join(f).is_file()).count();
            assert!(missing <= 1, "cut after {cut} moves: {:?}", tree(&dir));
            if let Some(done) = outcome {
                done.unwrap();
                ended += 1;
            }
            // Recovering, cut too after a move, then whole.
            for again in 0..3 {
                let _ = cut_after(Some(again), || recover(&dir));
            }
            cut_after(None, || recover(&dir)).unwrap().unwrap();
            let left = tree(&dir);
            if InstallRecord::read(&dir).unwrap() == to {
                for name in NEW {
                    assert!(left.contains(&(name.to_string(), format!("new {name}"))), "cut after {cut} moves: {left:?}");
                }
                assert!(!dir.join("gone.dll").exists() && dir.join("mine.txt").is_file() && dir.join("sub/mine.txt").is_file());
            } else {
                assert_eq!(left, whole_old, "cut after {cut} moves");
            }
            assert!(!dir.join(JOURNAL).exists(), "nothing more to recover");
            assert!(!recover(&dir).unwrap());
            remove_tree(&dir);
            if ended == 3 {
                break;
            }
        }
        assert_eq!(ended, 3, "the swap was cut after each of its moves");
    }

    /// Ended, with its journal still there (it could not be removed, or the cut came between): only tidied up, never undone.
    #[test]
    fn a_swap_that_ended_is_never_undone() {
        let dir = scratch("ended");
        let (index, old, to) = before(&dir);
        let (staging, backup) = (dir.join(STAGING), dir.join(BACKUP));
        swap(&dir, &staging, &backup, &index, Some(&old), &to).unwrap();
        assert!(!dir.join(JOURNAL).exists());
        let plan = Plan { aside: old.files.clone(), fresh: vec!["fresh/c.dll".into()], to: to.clone() };
        drop(Journal::begin(&dir, &plan).unwrap());
        assert!(recover(&dir).unwrap());
        assert_eq!(std::fs::read_to_string(dir.join(APP_EXE)).unwrap(), format!("new {APP_EXE}"));
        assert!(dir.join("fresh/c.dll").is_file() && !backup.exists() && !dir.join(JOURNAL).exists());
        // Its files gone with no record of it (whatever removed them), a journal takes nothing out of the install but what
        // was not there before.
        drop(Journal::begin(&dir, &Plan { aside: NEW.iter().map(|f| f.to_string()).collect(), fresh: vec![], to: old }).unwrap());
        recover(&dir).unwrap();
        assert!(NEW.iter().all(|f| dir.join(f).is_file()), "{:?}", tree(&dir));
        remove_tree(&dir);
    }

    /// A journal another setup holds is a swap under way; one that cannot be read, or is a person's file, is none.
    #[test]
    fn only_a_swap_that_was_cut_is_recovered() {
        let dir = scratch("held");
        let (_, old, to) = before(&dir);
        let held = Journal::begin(&dir, &Plan { aside: old.files.clone(), fresh: vec![], to }).unwrap();
        let said = format!("{:#}", recover(&dir).unwrap_err());
        assert!(said.contains("Another setup is at work"), "{said}");
        drop(held);
        for text in [&b"{\"product\":\"orbiters-desktop\",\"aside\":[\"mine.txt\"]"[..], b"{}", b"a person's", b""] {
            std::fs::write(dir.join(JOURNAL), text).unwrap();
            assert!(!recover(&dir).unwrap());
            assert!(dir.join("mine.txt").is_file());
        }
        remove_tree(&dir);
    }

    /// What Windows runs at the next sign-in is at most 260 characters, or it would not run it.
    #[test]
    fn what_is_run_at_the_next_sign_in_fits() {
        let (setup, dir) = (Path::new(r"C:\U\OrbitersSetup.exe"), Path::new(r"C:\P\Orbiters - N4N0"));
        assert_eq!(recover_command(setup, dir).unwrap(), r#""C:\U\OrbitersSetup.exe" --recover --dir "C:\P\Orbiters - N4N0""#);
        let long = PathBuf::from(format!(r"C:\{}", "d".repeat(240)));
        assert_eq!(recover_command(setup, &long).unwrap(), r#""C:\U\OrbitersSetup.exe" --recover"#);
        assert_eq!(recover_command(&long.join("setup.exe"), dir), None);
    }
}
