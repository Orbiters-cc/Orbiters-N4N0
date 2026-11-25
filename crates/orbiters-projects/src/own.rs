//! What Orbiters keeps of its own about projects and package repositories, in its data folder. Creator Companion's and ALCOM's
//! files are only ever read (`theirs`): never written, locked or repaired.
use serde::{de::DeserializeOwned, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where Orbiters keeps its own for the Creator Companion folder `root`: its data folder. For any other folder than this
/// computer's (a test's, a stand-in's) a temporary one, never the person's.
pub fn dir(root: &Path) -> PathBuf {
    crate::disk::companion(root);
    if !cfg!(test) && root == crate::vcc_dir() {
        return crate::data_dir();
    }
    stand_ins().join(format!("{}-{}", std::process::id(), short(&crate::discovery::key(root))))
}

/// Where those temporary folders are; one that is a day old is removed when the first is asked for.
fn stand_ins() -> PathBuf {
    static TIDIED: std::sync::Once = std::sync::Once::new();
    let all = std::env::temp_dir().join("orbiters-projects-tests");
    TIDIED.call_once(|| {
        for e in std::fs::read_dir(&all).into_iter().flatten().flatten() {
            let age = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok());
            if age.is_some_and(|age| age > Duration::from_secs(24 * 3600)) {
                let _ = crate::disk::remove_dir_all(e.path());
            }
        }
    });
    all
}

/// For tests: Orbiters' list for `root` cannot be saved (a disk with no room) until what this answers is dropped.
#[cfg(test)]
pub(crate) fn full_for(root: &Path) -> impl Drop {
    struct Full(PathBuf);
    impl Drop for Full {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir(&self.0);
        }
    }
    let lock = dir(root).join("unity-gallery.lock");
    let _ = std::fs::remove_file(&lock);
    std::fs::create_dir_all(&lock).unwrap();
    Full(lock)
}

/// A short name for a text, the same on every run (FNV-1a).
pub(crate) fn short(text: &str) -> String {
    format!("{:016x}", text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)))
}

/// Where the copy of `file` before its last change is.
fn previous(file: &Path) -> PathBuf {
    file.with_extension("previous.json")
}

fn parse<T: DeserializeOwned>(file: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(file).ok()?).ok()
}

/// What `file` holds; where it is damaged or gone, what its copy before the last change holds.
pub(crate) fn read<T: DeserializeOwned>(file: &Path) -> Option<T> {
    parse(file).or_else(|| parse(&previous(file)))
}

/// Held until dropped: one change of `file` at a time, across this app's processes.
pub(crate) fn lock(file: &Path) -> std::io::Result<std::fs::File> {
    if let Some(dir) = file.parent() {
        crate::disk::create_dir_all(dir)?;
    }
    let held = crate::disk::open(std::fs::File::options().create(true).write(true).truncate(false), file.with_extension("lock"))?;
    held.lock()?;
    Ok(held)
}

/// Changes what `file` holds (`first` when there is nothing): read and written under its lock, each write whole or not at all,
/// and what was there before kept beside it. A file that cannot be read is put aside, never written over.
pub(crate) fn change<T, R>(file: &Path, first: impl FnOnce() -> T, edit: impl FnOnce(&mut T) -> R) -> anyhow::Result<R>
where
    T: Serialize + DeserializeOwned + Clone + PartialEq,
{
    let _held = lock(file)?;
    let whole: Option<T> = parse(file);
    if whole.is_none() && file.exists() {
        crate::disk::rename(file, file.with_extension(format!("damaged-{}.json", uuid::Uuid::new_v4().simple())))?;
    }
    let kept = whole.clone().or_else(|| parse(&previous(file)));
    let mut now = kept.clone().unwrap_or_else(first);
    let out = edit(&mut now);
    if whole.as_ref() == Some(&now) {
        return Ok(out);
    }
    if let Some(before) = &whole {
        crate::write_json(&previous(file), before)?;
    }
    crate::write_json(file, &now)?;
    Ok(out)
}

/// The bytes of a file another program may be writing: the same before and after, or an error. Opened for reading alone, shared
/// with every other use.
pub(crate) fn steady(file: &Path) -> std::io::Result<Option<Vec<u8>>> {
    let stamp = || std::fs::metadata(file).map(|m| (m.len(), m.modified().ok()));
    let mut last = None;
    for attempt in 0..4 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(20));
        }
        let read = stamp().and_then(|before| Ok((before, std::fs::read(file)?, stamp()?)));
        match read {
            Ok((before, bytes, after)) if before == after && bytes.len() as u64 == before.0 => return Ok(Some(bytes)),
            Ok(_) => last = Some(std::io::Error::other("it is being written")), // i18n: not shown (chained under a sentence)
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && attempt > 0 => return Ok(None),
            // (Replaced by a rename, it is away for an instant.)
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => last = None,
            Err(e) => last = Some(e),
        }
    }
    last.map_or(Ok(None), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A damaged list is what it was before its last change; it is kept aside, and never written over.
    #[test]
    fn a_damaged_file_falls_back_to_the_one_before() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("list.json");
        let add = |n: u32| change(&file, Vec::new, |l: &mut Vec<u32>| l.push(n)).unwrap();
        assert_eq!(read::<Vec<u32>>(&file), None);
        add(1);
        add(2);
        assert_eq!((read(&file), parse(&previous(&file))), (Some(vec![1u32, 2]), Some(vec![1u32])));
        for bad in ["", "[1, 2", "{}", "\0\0\0\0"] {
            std::fs::write(&file, bad).unwrap();
            assert_eq!(read(&file), Some(vec![1u32]), "{bad:?}");
        }
        add(3);
        assert_eq!((read(&file), parse(&previous(&file))), (Some(vec![1u32, 3]), Some(vec![1u32])), "the good copy is not replaced by the damaged one");
        let aside = std::fs::read_dir(temp.path()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains("damaged")).count();
        assert_eq!(aside, 1);
        // Gone altogether.
        std::fs::remove_file(&file).unwrap();
        assert_eq!(read(&file), Some(vec![1u32]));
        // Nothing to change: nothing written.
        add(4);
        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        change(&file, Vec::new, |_: &mut Vec<u32>| ()).unwrap();
        assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), before);
    }

    /// Two windows, the service: no change is lost.
    #[test]
    fn changes_at_once_are_all_kept() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("list.json");
        std::thread::scope(|s| {
            for n in 0..8u32 {
                let file = &file;
                s.spawn(move || (0..10).for_each(|i| change(file, Vec::new, |l: &mut Vec<u32>| l.push(n * 100 + i)).unwrap()));
            }
        });
        assert_eq!(read::<Vec<u32>>(&file).unwrap().len(), 80);
    }

    #[test]
    fn a_file_is_read_whole_or_not_at_all() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("f");
        assert_eq!(steady(&file).unwrap(), None);
        std::fs::write(&file, "abc").unwrap();
        assert_eq!(steady(&file).unwrap().as_deref(), Some(&b"abc"[..]));
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let held = std::fs::File::options().read(true).write(true).share_mode(0).open(&file).unwrap();
            assert!(steady(&file).is_err(), "in use by another program alone");
            drop(held);
            // Open here, it is no less usable there.
            assert!(std::fs::File::options().write(true).open(&file).is_ok());
        }
    }
}
