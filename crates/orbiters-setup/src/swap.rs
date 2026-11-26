//! Putting a version in place, so that a power cut or a kill at any moment leaves a version that can be put back whole.
//!
//! Before the first file moves, a journal in the install folder says what is about to: which files are there and go aside,
//! which are new, and the install record that will say it is done. Each file is then moved aside and its new one in at
//! once, so that one file at most is missing at any moment, the app's own program last. Writing the record is what ends it:
//! a journal found later ([`recover`]: at the next start of a setup, or at the next sign-in, which Windows is told of) is
//! either of a swap that ended, and only tidied up, or of one that did not, and all of it goes back. The journal goes
//! first in both cases, then the files it named.

use crate::install::{contained, make_parent, remove_empty_dirs, remove_file, remove_tree, rename, rename_marker, BACKUP, STAGING};
use anyhow::Context as _;
use orbiters_i18n::tr;
use orbiters_update::{payload, InstallRecord, APP_EXE, PRODUCT, RECORD_FILE};
use std::io::Read as _;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// The journal's file, in the install folder.
pub(crate) const JOURNAL: &str = ".setup-journal";
pub(crate) const RECORD_DRAFT: &str = "install.json.tmp";

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
    /// Written whole in the staging folder (this setup's own), then published under its name in `dir` (W1-3): a cut before
    /// that leaves no journal at all, never an empty or half one that a first install would take for a file of the person's.
    /// Held from the start, shared only to be renamed: another setup finds it in use.
    fn begin(dir: &Path, plan: &Plan) -> anyhow::Result<Self> {
        let path = dir.join(JOURNAL);
        let draft = dir.join(STAGING).join(JOURNAL);
        make_parent(&draft)?;
        let bytes = serde_json::to_vec(&plan.json())?;
        let file = orbiters_update::publish_file(&draft, &path, &bytes, |from, to| rename_marker(from, to, false))
            .with_context(|| tr!("{path} cannot be written", path = path.display()))?;
        Ok(Self { file: Some(file), path })
    }

    /// The journal a swap left in `dir`: none when it is confirmed not there (or is a folder, or a link: not ours). One that is
    /// there but cannot be looked at now is an error, never "none" (W1-1). Its plan is `None` when it holds no whole one: an
    /// older setup wrote it in place and was cut while writing it (V2-2).
    fn left(dir: &Path) -> anyhow::Result<Option<(Self, Option<Plan>)>> {
        const SHARING_VIOLATION: i32 = 32;
        let path = dir.join(JOURNAL);
        match std::fs::symlink_metadata(&path) {
            Ok(m) if m.is_file() && m.file_attributes() & 0x400 == 0 => {}
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(unreadable(&path, e)),
            _ => return Ok(None),
        }
        let mut file = match std::fs::OpenOptions::new().read(true).share_mode(0).open(&path) {
            Err(e) if e.raw_os_error() == Some(SHARING_VIOLATION) => anyhow::bail!("{}", tr!("Another setup is at work in {folder}: nothing was changed.", folder = dir.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            opened => opened.map_err(|e| unreadable(&path, e))?,
        };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|e| unreadable(&path, e))?;
        Ok(Some((Self { file: Some(file), path }, Plan::read(&bytes))))
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
        record.publish(dir, &staging.join(RECORD_DRAFT), |from, to| rename_marker(from, to, true)).with_context(|| tr!("{path} cannot be written", path = dir.join(RECORD_FILE).display()))
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

/// A file of the install that is there and could not be read now: the setup is to be run again (at the next sign-in, it is).
pub(crate) fn unreadable(path: &Path, e: std::io::Error) -> anyhow::Error {
    anyhow::anyhow!("{}", tr!("{path} cannot be read right now ({why}). Try again in a moment.", path = path.display(), why = e))
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
        let back = (|| -> std::io::Result<bool> {
            if plan.aside.iter().any(|p| p == path) {
                // An absent backup means that the old file never moved or has already been restored.
                if !old.try_exists()? {
                    return Ok(true);
                }
                return Ok(old.is_file() && (!dest.try_exists()? || (!new.try_exists()? && moved(&dest, &new))) && moved(&old, &dest));
            }
            Ok(new.try_exists()? || !dest.try_exists()? || moved(&dest, &new))
        })().unwrap_or(false);
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
    let Some(plan) = plan else {
        // V2-2: this app's journal, at its own name in the install folder, that says no whole plan: an older setup wrote it in
        // place and was cut while it did, before its first move. It is kept beside under a name of its own (whatever it holds
        // stays), never left at its name, where every later setup would refuse to write its own and stop there for good.
        let journal_path = journal.path.clone();
        drop(journal);
        crate::install::keep_aside(dir, &journal_path).with_context(|| tr!("{path} cannot be written", path = journal_path.display()))?;
        return Ok(true);
    };
    let (staging, backup) = (dir.join(STAGING), dir.join(BACKUP));
    // Whether the swap ended is what its record says: one that cannot be read now decides nothing, before any file moves (W1-1).
    let record = dir.join(RECORD_FILE);
    let ended = InstallRecord::load(dir).map_err(|e| unreadable(&record, e))?.as_ref() == Some(&plan.to);
    if !ended {
        let stuck = put_back(dir, &staging, &backup, &plan);
        anyhow::ensure!(stuck.is_empty(), "{}", not_put_back(&stuck, &backup));
    }
    // A cut can leave an unpublished record in staging. Root-level files of the same name are left alone.
    if let Some(draft) = contained(&staging, RECORD_DRAFT) {
        let _ = remove_file(&draft);
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

/// Whether `dir` holds one version whole, the only kind that may be started (W1-4): no swap left to put back (a put-back that
/// could not end leaves the old program beside new files), or one whose record was written; and the record read.
pub(crate) fn whole(dir: &Path) -> bool {
    let Ok(journal) = Journal::left(dir) else { return false };
    let Ok(Some(record)) = InstallRecord::load(dir) else { return false };
    if journal.as_ref().and_then(|(_, plan)| plan.as_ref()).is_some_and(|plan| record != plan.to) {
        return false;
    }
    let available = |path: &str| contained(dir, path).is_some_and(|file| file.is_file());
    available(APP_EXE) && record.files.iter().all(|path| available(path))
}

/// The programs a cut swap in `dir` was about, which must not run while it is put back.
pub(crate) fn left_programs(dir: &Path) -> anyhow::Result<Vec<String>> {
    Ok(Journal::left(dir)?.and_then(|(_, plan)| plan).map(|plan| plan.paths().map(str::to_string).collect()).unwrap_or_default())
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
            // Cut at the journal's publication (its first move), the swap had not begun: what was unpacked waits in the staging
            // folder, which the next setup sets aside.
            let left: Vec<_> = tree(&dir).into_iter().filter(|(p, _)| cut > 0 || !p.starts_with(&format!("{STAGING}/"))).collect();
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

    /// A journal another setup holds is a swap under way. One at its name that holds no whole plan (V2-2: what an older setup,
    /// which wrote it in place, left when cut while writing it) moves nothing: it is kept beside as it is, and its name is free.
    /// A folder at its name is none.
    #[test]
    fn only_a_swap_that_was_cut_is_recovered() {
        let dir = scratch("held");
        let (_, old, to) = before(&dir);
        let held = Journal::begin(&dir, &Plan { aside: old.files.clone(), fresh: vec![], to }).unwrap();
        let said = format!("{:#}", recover(&dir).unwrap_err());
        assert!(said.contains("Another setup is at work"), "{said}");
        drop(held);
        let unplanned = tree(&dir).into_iter().filter(|(p, _)| p != JOURNAL).collect::<Vec<_>>();
        for text in [&b"{\"product\":\"orbiters-desktop\",\"aside\":[\"mine.txt\"]"[..], b"{}", b"a person's", b""] {
            std::fs::write(dir.join(JOURNAL), text).unwrap();
            assert!(recover(&dir).unwrap());
            assert!(!dir.join(JOURNAL).exists() && !recover(&dir).unwrap());
            let (kept, rest): (Vec<_>, Vec<_>) = tree(&dir).into_iter().partition(|(p, _)| p.starts_with(&format!("{JOURNAL}-")));
            assert_eq!(rest, unplanned, "nothing of the install moved");
            assert_eq!(kept.len(), 1);
            assert_eq!(kept[0].1.as_bytes(), text);
            std::fs::remove_file(dir.join(&kept[0].0)).unwrap();
        }
        std::fs::create_dir(dir.join(JOURNAL)).unwrap();
        assert!(!recover(&dir).unwrap() && dir.join(JOURNAL).is_dir());
        std::fs::remove_dir(dir.join(JOURNAL)).unwrap();
        std::fs::write(dir.join(JOURNAL), b"a person's").unwrap();
        let (_, old, to) = before(&dir);
        assert!(Journal::begin(&dir, &Plan { aside: old.files, fresh: vec![], to }).is_err());
        assert_eq!(std::fs::read(dir.join(JOURNAL)).unwrap(), b"a person's");
        remove_tree(&dir);
    }

    /// W1-1: a swap whose record is written, with that record held a moment by another program (an antivirus), is not undone:
    /// nothing moves, the journal stays, and the next recovery keeps the new version.
    #[test]
    fn a_committed_swap_waits_while_its_record_cannot_be_read() {
        let dir = scratch("committed-record-held");
        let (index, old, to) = before(&dir);
        let (staging, backup) = (dir.join(STAGING), dir.join(BACKUP));
        swap(&dir, &staging, &backup, &index, Some(&old), &to).unwrap();
        let plan = Plan { aside: old.files.clone(), fresh: vec!["fresh/c.dll".into()], to: to.clone() };
        drop(Journal::begin(&dir, &plan).unwrap());
        let before_recovery = tree(&dir);
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(dir.join(RECORD_FILE)).unwrap();
        let said = format!("{:#}", recover(&dir).unwrap_err());
        assert!(!whole(&dir), "not to be started while it cannot be told which version it is");
        drop(held);
        assert!(said.contains(RECORD_FILE), "{said}");
        assert_eq!(tree(&dir), before_recovery, "nothing moved");
        assert!(recover(&dir).unwrap());
        assert_eq!(InstallRecord::read(&dir).unwrap(), to);
        assert_eq!(std::fs::read_to_string(dir.join(APP_EXE)).unwrap(), format!("new {APP_EXE}"));
        assert!(whole(&dir) && !dir.join(JOURNAL).exists());
        remove_tree(&dir);
    }

    /// W1-4: a put-back that could not end (a new helper held open) leaves a mixed folder: never one to start, until a later
    /// recovery puts the whole old version back.
    #[test]
    fn a_partial_put_back_is_not_a_version_to_start() {
        let dir = scratch("partial-put-back");
        let (index, old, to) = before(&dir);
        let (staging, backup) = (dir.join(STAGING), dir.join(BACKUP));
        let helper = "orbiters-capture.exe";
        assert!(index.files.iter().any(|f| f.path == helper));
        assert!(whole(&dir));
        drop(Journal::begin(&dir, &Plan { aside: old.files.clone(), fresh: vec!["fresh/c.dll".into()], to }).unwrap());
        make_parent(&backup.join(helper)).unwrap();
        std::fs::rename(dir.join(helper), backup.join(helper)).unwrap();
        std::fs::rename(staging.join(helper), dir.join(helper)).unwrap();
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(dir.join(helper)).unwrap();
        assert!(recover(&dir).is_err());
        assert!(dir.join(JOURNAL).exists());
        assert_eq!(std::fs::read_to_string(dir.join(APP_EXE)).unwrap(), "old app");
        assert!(!whole(&dir), "the old program beside a new helper");
        // The app's start after an update (and the failure page's Open) goes through the same door, which starts nothing here.
        let started = std::cell::RefCell::new(vec![]);
        let launch = |program: &Path, _: &[&str]| {
            started.borrow_mut().push(program.to_path_buf());
            Ok(())
        };
        crate::relaunch_with(&dir, true, launch);
        assert!(started.borrow().is_empty(), "the old program beside a new helper is never started");
        drop(held);
        recover(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(helper)).unwrap(), "old capture");
        assert!(whole(&dir));
        crate::relaunch_with(&dir, true, launch);
        assert_eq!(*started.borrow(), vec![dir.join(APP_EXE)], "a whole version is");
        remove_tree(&dir);
    }

    /// W1-1: a journal that cannot be looked at now is not "no journal".
    #[test]
    fn a_journal_that_cannot_be_read_now_is_an_error() {
        let dir = scratch("journal-held");
        let (_, old, to) = before(&dir);
        drop(Journal::begin(&dir, &Plan { aside: old.files.clone(), fresh: vec![], to }).unwrap());
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(dir.join(JOURNAL)).unwrap();
        assert!(recover(&dir).is_err() && !whole(&dir));
        drop(held);
        assert!(recover(&dir).unwrap());
        remove_tree(&dir);
    }

    /// W1-1, its first step on its own: where the journal's name cannot even be looked at (here a name Windows refuses, the one
    /// failure of that step a test can bring about), that is an error, never "no journal".
    #[test]
    fn a_journal_name_that_cannot_be_looked_at_is_an_error() {
        let dir = scratch("journal-name").join("a?b");
        let looked = std::fs::symlink_metadata(dir.join(JOURNAL)).unwrap_err();
        assert_ne!(looked.kind(), std::io::ErrorKind::NotFound, "{looked}");
        assert!(Journal::left(&dir).is_err() && recover(&dir).is_err() && left_programs(&dir).is_err() && !whole(&dir));
        remove_tree(dir.parent().unwrap());
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
