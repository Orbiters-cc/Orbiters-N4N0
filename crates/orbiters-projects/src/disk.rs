//! The one way this crate writes on a disk. Nothing is written, made, moved or removed in Creator Companion's or ALCOM's folders,
//! whatever path leads there: what a path names is compared by the identity of the folders themselves, through links, other
//! spellings and other cases. `no_write_tests` reads the crate for a writer anywhere else; `tests/no_write.rs` tries them all.
//!
//! The library that changes a project's packages (vrc-get) writes by itself: it is only given a project ([`project_io`]) and an
//! environment ([`environment_io`]) that were asked about here, and what a change touches is asked about before it ([`tree`]).
use orbiters_core::locks::*;
use crate::owned::{id_of, is_link, FileId};
use orbiters_i18n::tr;
use std::cell::RefCell;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long an answer about a folder stands: a copy asks it thousands of times.
const STANDS: Duration = Duration::from_millis(200);

/// The folders asked about besides this computer's own (a test's, a stand-in's), and the answers about all of them.
#[derive(Default)]
struct Theirs {
    more: Vec<PathBuf>,
    known: Option<(Instant, Ids)>,
    changes: u64,
}

#[derive(Clone, Default)]
struct Ids {
    /// Their folders, as they are spelled.
    named: Vec<PathBuf>,
    /// Their folders.
    theirs: HashSet<(u64, u64)>,
    /// Every folder one of theirs is in.
    above: HashSet<(u64, u64)>,
}

static THEIRS: Mutex<Theirs> = Mutex::new(Theirs { more: Vec::new(), known: None, changes: 0 });

/// Also nothing in `root` from now on: the Creator Companion folder an operation is for.
pub(crate) fn companion(root: &Path) {
    let mut theirs = THEIRS.locked();
    if !theirs.more.iter().any(|p| p == root) {
        theirs.more.push(root.to_path_buf());
        theirs.known = None;
        theirs.changes += 1;
    }
}

/// How long what is installed is known: the registry is asked that often at most.
const INSTALLED: Duration = Duration::from_secs(30);

/// This computer's own: Creator Companion's folder and program, ALCOM's own settings and program, a portable Creator Companion the
/// person pointed at ([`crate::programs`]). (A test, whose folder is a stand-in, does not look at them.)
fn this_computers(more: &[PathBuf]) -> Vec<PathBuf> {
    let own = crate::vcc_dir();
    if cfg!(test) || !more.is_empty() && !more.contains(&own) {
        return vec![];
    }
    let mut all = vec![own];
    if orbiters_core::paths::roots().is_none() {
        let alcom = [dirs::data_local_dir(), dirs::config_dir(), dirs::data_dir()];
        all.extend(alcom.into_iter().flatten().map(|d| d.join("com.anatawa12.vrc-get-gui")));
    }
    static PROGRAMS: Mutex<Option<(Instant, Vec<PathBuf>)>> = Mutex::new(None);
    let mut programs = PROGRAMS.locked();
    if programs.as_ref().is_none_or(|(at, _)| at.elapsed() >= INSTALLED) {
        *programs = Some((Instant::now(), crate::programs::folders()));
    }
    all.extend(programs.iter().flat_map(|(_, p)| p.iter().cloned()));
    all
}

fn number(id: FileId) -> (u64, u64) {
    (id.volume, id.index)
}

fn ids() -> (Ids, u64) {
    let mut theirs = THEIRS.locked();
    if let Some((at, ids)) = &theirs.known {
        if at.elapsed() < STANDS {
            return (ids.clone(), theirs.changes);
        }
    }
    let mut ids = Ids::default();
    for p in this_computers(&theirs.more).into_iter().chain(theirs.more.iter().cloned()) {
        if let Ok(real) = p.canonicalize() {
            if let Ok(id) = id_of(&real) {
                ids.theirs.insert(number(id));
            }
            ids.above.extend(real.ancestors().skip(1).filter_map(|a| id_of(a).ok()).map(number));
        }
        ids.named.push(p);
    }
    theirs.known = Some((Instant::now(), ids.clone()));
    (ids, theirs.changes)
}

fn refused(path: &Path) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, tr!("{path} is in Creator Companion's or ALCOM's folder: Orbiters changes nothing there.", path = path.display()))
}

thread_local! {
    /// Folders found not to be in theirs, since when, and for which list of theirs.
    static FINE: RefCell<(Option<Instant>, u64, HashSet<PathBuf>)> = RefCell::default();
}

/// Whether the folder `dir` is one of theirs: one that is known, or a portable Creator Companion's by its look, wherever it is.
fn is_theirs(dir: &Path, ids: &Ids) -> bool {
    id_of(dir).is_ok_and(|id| ids.theirs.contains(&number(id))) || crate::programs::is_portable_data(dir) || crate::programs::is_portable(dir)
}

/// Whether `dir` (a folder there, or to be made) is in one of theirs.
fn outside(dir: &Path, ids: &Ids, changes: u64, asked: &Path, depth: usize) -> io::Result<()> {
    if depth > 64 {
        return Err(io::Error::other("too many links")); // i18n: not shown (chained under a sentence)
    }
    let stands = |fine: &(Option<Instant>, u64, HashSet<PathBuf>)| fine.1 == changes && fine.0.is_some_and(|at| at.elapsed() < STANDS);
    if FINE.with(|f| stands(&f.borrow()) && f.borrow().2.contains(dir)) {
        return Ok(());
    }
    let mut seen = vec![];
    let mut at = Some(dir);
    while let Some(here) = at {
        if FINE.with(|f| stands(&f.borrow()) && f.borrow().2.contains(here)) {
            break;
        }
        match std::fs::symlink_metadata(here) {
            // Not there yet: where it would be.
            Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => {}
            Err(_) => {}
            Ok(m) if is_link(&m) => {
                // What a link leads to is where what is below it is. One that leads nowhere has nothing below it.
                if let Ok(real) = here.canonicalize() {
                    outside(&real, ids, changes, asked, depth + 1)?;
                }
                // (And the link itself is where it is.)
            }
            Ok(_) => {
                if is_theirs(here, ids) {
                    return Err(refused(asked));
                }
            }
        }
        seen.push(here.to_path_buf());
        at = here.parent();
    }
    FINE.with(|f| {
        let mut f = f.borrow_mut();
        if !stands(&f) {
            *f = (Some(Instant::now()), changes, HashSet::new());
        }
        f.2.extend(seen);
    });
    Ok(())
}

fn whole(path: &Path) -> io::Result<PathBuf> {
    // `..` after a link is not what it looks like: what is there is asked for as it is.
    let path = std::path::absolute(path)?;
    if !path.components().any(|c| c == Component::ParentDir) {
        return Ok(path);
    }
    let mut there = path.as_path();
    let mut rest = vec![];
    loop {
        match there.canonicalize() {
            Ok(real) => return Ok(rest.iter().rev().fold(real, |p, part| p.join(part))),
            Err(e) => match (there.file_name(), there.parent()) {
                (Some(name), Some(up)) => {
                    rest.push(name.to_os_string());
                    there = up;
                }
                _ => return Err(e),
            },
        }
    }
}

fn check(path: &Path, followed: bool) -> io::Result<()> {
    let at = whole(path)?;
    let (ids, changes) = ids();
    // Theirs is not there: nothing makes it.
    let key = crate::discovery::key(&at);
    for theirs in &ids.named {
        let k = crate::discovery::key(theirs);
        if key == k || key.strip_prefix(&k).is_some_and(|rest| rest.starts_with(['\\', '/'])) {
            return Err(refused(path));
        }
    }
    match at.parent() {
        Some(parent) => outside(parent, &ids, changes, path, 0)?,
        None => return Ok(()),
    }
    // The folder itself, or what a link there leads to.
    match std::fs::symlink_metadata(&at) {
        Ok(m) if is_link(&m) => {
            if followed {
                if let Ok(real) = at.canonicalize() {
                    outside(&real, &ids, changes, path, 0)?;
                }
            }
        }
        Ok(m) if m.is_dir() && is_theirs(&at, &ids) => return Err(refused(path)),
        _ => {}
    }
    Ok(())
}

/// Whether what is at `path` itself may be made, replaced or removed.
pub(crate) fn allowed(path: &Path) -> io::Result<()> {
    check(path, false)
}

/// Whether what `path` leads to may be written.
pub(crate) fn content(path: &Path) -> io::Result<()> {
    check(path, true)
}

/// Whether `path` and everything in it may be moved or removed: theirs is not in it either.
fn all_of(path: &Path) -> io::Result<()> {
    allowed(path)?;
    let (ids, _) = ids();
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() && !is_link(&m) && (id_of(path).is_ok_and(|id| ids.above.contains(&number(id))) || crate::programs::holds_portable_data(path)) => Err(refused(path)),
        _ => Ok(()),
    }
}

/// Whether what another library puts at `path`, or takes from there, may be.
pub(crate) fn tree(path: &Path) -> io::Result<()> {
    content(path)?;
    all_of(path)
}

pub(crate) fn create_dir(path: impl AsRef<Path>) -> io::Result<()> {
    allowed(path.as_ref())?;
    std::fs::create_dir(path)
}

pub(crate) fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    content(path.as_ref())?;
    std::fs::create_dir_all(path)
}

pub(crate) fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    content(path.as_ref())?;
    std::fs::write(path, bytes)
}

/// Opened as `options` say, to be written.
pub(crate) fn open(options: &OpenOptions, path: impl AsRef<Path>) -> io::Result<File> {
    content(path.as_ref())?;
    options.open(path)
}

pub(crate) fn create_file(path: impl AsRef<Path>) -> io::Result<File> {
    content(path.as_ref())?;
    File::create(path)
}

/// A copy of a file, which is only read.
pub(crate) fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
    content(to.as_ref())?;
    std::fs::copy(from, to)
}

pub(crate) fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    all_of(from.as_ref())?;
    allowed(to.as_ref())?;
    std::fs::rename(from, to)
}

/// `from` in the place of `to`, with no moment without either.
pub(crate) fn replace(from: &Path, to: &Path) -> io::Result<()> {
    all_of(from)?;
    allowed(to)?;
    #[cfg(windows)]
    {
        // Windows rename does not replace existing files. MoveFileEx provides replacement without a gap.
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};
        let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain(Some(0)).collect() };
        let (src, dst) = (wide(from), wide(to));
        // SAFETY: two texts that end.
        unsafe { MoveFileExW(PCWSTR(src.as_ptr()), PCWSTR(dst.as_ptr()), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) }.map_err(io::Error::other)
    }
    #[cfg(not(windows))]
    std::fs::rename(from, to)
}

pub(crate) fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    allowed(path.as_ref())?;
    std::fs::remove_file(path)
}

pub(crate) fn remove_dir(path: impl AsRef<Path>) -> io::Result<()> {
    allowed(path.as_ref())?;
    std::fs::remove_dir(path)
}

pub(crate) fn remove_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    all_of(path.as_ref())?;
    std::fs::remove_dir_all(path)
}

pub(crate) fn set_permissions(path: impl AsRef<Path>, permissions: std::fs::Permissions) -> io::Result<()> {
    content(path.as_ref())?;
    std::fs::set_permissions(path, permissions)
}

/// Makes `link` lead to the folder `target`: an NTFS junction, a symbolic link elsewhere.
pub(crate) fn link(target: impl AsRef<Path>, link: impl AsRef<Path>) -> io::Result<()> {
    allowed(link.as_ref())?;
    #[cfg(windows)]
    {
        junction::create(target, link)
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link)
    }
}

/// Takes such a link away, never what it leads to.
pub(crate) fn unlink(link: impl AsRef<Path>) -> io::Result<()> {
    allowed(link.as_ref())?;
    #[cfg(windows)]
    {
        junction::delete(link)
    }
    #[cfg(not(windows))]
    {
        std::fs::remove_file(link)
    }
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    allowed(path)?;
    orbiters_core::write_atomic(path, bytes)
}

/// An icon kept as a picture.
pub(crate) fn save_png(path: &Path, rgba: &[u8], size: [u32; 2]) -> bool {
    content(path).is_ok() && image::save_buffer_with_format(path, rgba, size[0], size[1], image::ExtendedColorType::Rgba8, image::ImageFormat::Png).is_ok()
}

/// The project at `path`, for the library that changes its packages.
pub(crate) fn project_io(path: &Path) -> io::Result<vrc_get_vpm::io::DefaultProjectIo> {
    for at in [path.to_path_buf(), path.join("Packages"), path.join("Packages/vpm-manifest.json"), path.join("Packages/manifest.json"), path.join("Assets")] {
        content(&at)?;
    }
    Ok(vrc_get_vpm::io::DefaultProjectIo::new(path.into()))
}

/// Whether that library may change the packages of the project at `path`: it makes what it installs in the project's `Temp/vrc-get`,
/// and then removes that folder whole.
pub(crate) fn staging(path: &Path) -> io::Result<()> {
    content(&path.join("Temp"))?; // i18n: not shown (a folder's name)
    tree(&path.join("Temp/vrc-get"))
}

/// The environment in `path` (Orbiters' own), for the same library.
pub(crate) fn environment_io(path: PathBuf) -> io::Result<vrc_get_vpm::io::DefaultEnvironmentIo> {
    for at in [path.clone(), path.join("Repos"), path.join("settings.json")] {
        content(&at)?;
    }
    Ok(vrc_get_vpm::io::DefaultEnvironmentIo::new(path.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// However a path leads there, it is refused; beside it, and what only looks like it, is not.
    #[test]
    fn their_folder_is_told_however_it_is_said() {
        let temp = tempfile::tempdir().unwrap();
        let (root, beside) = (temp.path().join("VCC"), temp.path().join("VCC2"));
        std::fs::create_dir_all(root.join("Deep")).unwrap();
        std::fs::create_dir_all(&beside).unwrap();
        companion(&root);
        let (alias, above) = (temp.path().join("Alias"), temp.path().join("Above"));
        for (link, to) in [(&alias, &root), (&above, &temp.path().to_path_buf())] {
            #[cfg(windows)]
            junction::create(to, link).unwrap();
            #[cfg(not(windows))]
            std::os::unix::fs::symlink(to, link).unwrap();
        }
        let mut ways = vec![root.clone(), root.join("Deep"), root.join("Not/There"), beside.join("../VCC"), alias.clone(), alias.join("Deep"), above.join("VCC"), above.join("VCC/Deep/New")];
        if cfg!(windows) {
            ways.extend([temp.path().join("vcc"), PathBuf::from(format!(r"\\?\{}", root.display())), PathBuf::from(root.to_string_lossy().replace('\\', "/"))]);
        }
        for way in &ways {
            let inside = way.join("f");
            assert!(write(&inside, "x").is_err() && create_dir(&inside).is_err() && create_dir_all(inside.join("g")).is_err(), "{}", way.display());
            assert!(create_file(&inside).is_err() && copy(file!(), &inside).is_err() && rename(&beside, &inside).is_err(), "{}", way.display());
            assert!(open(File::options().write(true).create(true), &inside).is_err() && link(&beside, &inside).is_err(), "{}", way.display());
            assert!(crate::write_json(&inside, &1).is_err() && project_io(way).is_err() && tree(&inside).is_err(), "{}", way.display());
        }
        std::fs::write(root.join("Deep/kept"), "x").unwrap();
        for way in [root.join("Deep/kept"), alias.join("Deep/kept"), above.join("VCC/Deep/kept")] {
            assert!(remove_file(&way).is_err() && rename(&way, temp.path().join("gone")).is_err() && set_permissions(&way, std::fs::metadata(&way).unwrap().permissions()).is_err());
        }
        // What holds it is not moved or removed either; a link to it is a link.
        assert!(remove_dir_all(temp.path()).is_err() && remove_dir_all(&root).is_err() && rename(&root, temp.path().join("moved")).is_err() && remove_dir(&root).is_err());
        assert!(root.join("Deep/kept").is_file());
        unlink(&alias).unwrap();
        // A link made later, over a folder already asked about.
        let later = temp.path().join("Later");
        write(temp.path().join("plain"), "x").unwrap();
        link(&root, &later).unwrap();
        assert!(write(later.join("f"), "x").is_err());
        unlink(&later).unwrap();
        // Beside.
        write(beside.join("f"), "x").unwrap();
        create_dir_all(beside.join("VCC/Deep")).unwrap();
        write(beside.join("VCC/Deep/f"), "x").unwrap();
        remove_dir_all(&beside).unwrap();
        // Not there: nothing makes it.
        let gone = temp.path().join("Gone");
        companion(&gone);
        assert!(create_dir_all(&gone).is_err() && create_dir_all(gone.join("a/b")).is_err() && write(gone.join("f"), "x").is_err());
        if cfg!(windows) {
            assert!(create_dir(temp.path().join("GONE")).is_err());
        }
        assert!(!gone.exists());
        unlink(&above).unwrap();
    }
}
