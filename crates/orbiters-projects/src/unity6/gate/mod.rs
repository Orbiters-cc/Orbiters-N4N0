//! The one gate every write goes through, of the Unity 6 transition and of every copy of a project (Duplicate, a new
//! project from a template): second and third verifications, the write audits.
//!
//! - [`ReadRoot`] is a folder that is only read (the original, a folder it links to, a local package): held open by
//!   handle, listed and read relative to it. It has no function that writes.
//! - [`Destination`] is a place checked against everything the original can see ([`Protected`]), its parent held by handle;
//!   [`NewFolder`] is the new folder made in it, held with it for the whole of the operation that made it.
//! - [`WriteRoot`] is the only thing files are written through. It comes from a [`NewFolder`] just made, or from a private
//!   look in the journal ([`WriteRoot::of_job`]: the job, the folder's part in it, its phase and its identity), never from a
//!   path. Everything written is made **relative to its handle**, one name at a time: a name that is a link, a junction, a
//!   mounted folder or any reparse point is refused, and so is one whose opened object is not (by its final path) below the
//!   root, on the root's own volume. Nothing is ever written over: files are only made anew, and only what the same run made
//!   is ever removed.
//! - Nothing here, and nothing of the Unity 6 transition, writes a file of Creator Companion's or ALCOM's: their list of projects
//!   is theirs, and the person's in them.
//! - [`AppRoot`] is Orbiters' app data: the journal, the leases, the logs, the kept release list, a restore's staging folder.
//!
//! A test (`no_other_module_writes`) keeps every other module of the feature, and the copier of projects, to a list of calls
//! that only read.

#[cfg(unix)]
mod sys_unix;
#[cfg(windows)]
mod sys_windows;
#[cfg(unix)]
use sys_unix as sys;
#[cfg(windows)]
use sys_windows as sys;

use crate::owned::{id_of_file, FileId};
use orbiters_i18n::tr;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use sys::Want;

/// One name of a path, or why it is not one the gate takes: nothing that Windows reads as another name, a stream, a device
/// or another folder.
fn name_ok(name: &OsStr) -> io::Result<()> {
    let refused = || Err(io::Error::other(tr!("{name} is not a name Orbiters copies.", name = name.to_string_lossy())));
    let Some(text) = name.to_str() else { return if cfg!(windows) { refused() } else { Ok(()) } };
    if text.is_empty() || text == "." || text == ".." || text.contains(['/', '\0']) {
        return refused();
    }
    if cfg!(windows) && (text.contains([':', '\\']) || text.ends_with(['.', ' '])) {
        return refused();
    }
    Ok(())
}

/// The names of a relative path, each one taken by [`name_ok`].
fn names(rel: &Path) -> io::Result<Vec<&OsStr>> {
    rel.components()
        .map(|c| match c {
            Component::Normal(n) => name_ok(n).map(|()| n),
            _ => Err(io::Error::other(format!("{}: not below the folder", rel.display()))), // i18n: not shown (an error chained under a message)
        })
        .collect()
}

fn split(rel: &Path) -> io::Result<(&Path, &OsStr)> {
    let name = rel.file_name().ok_or_else(|| io::Error::other("no name"))?; // i18n: not shown (an error chained under a message)
    name_ok(name)?;
    Ok((rel.parent().unwrap_or(Path::new("")), name))
}

/// What an entry of a folder is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dir,
    /// `online`: read like any other, through the system that keeps it (OneDrive's placeholders, deduplicated files).
    File { online: bool },
    /// A link, a junction or a mounted folder.
    Link { dir: bool },
    /// Something else (a socket, a device).
    Other,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: OsString,
    pub kind: Kind,
    pub len: u64,
}

fn entries(dir: &File) -> io::Result<Vec<Entry>> {
    let mut out: Vec<Entry> = sys::list(dir)?
        .into_iter()
        .map(|l| Entry {
            kind: match (l.link, l.dir) {
                (true, dir) => Kind::Link { dir },
                (false, true) => Kind::Dir,
                (false, false) if l.special && cfg!(unix) => Kind::Other,
                (false, false) => Kind::File { online: l.special },
            },
            name: l.name,
            len: l.len,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// The last folder opened below a root: files come folder by folder, and each folder is opened name by name.
type Last = Mutex<Option<(PathBuf, File)>>;

/// A folder only ever read: held while this lives (Windows: it cannot be renamed, moved or deleted meanwhile).
pub struct ReadRoot {
    dir: File,
    pub id: FileId,
    /// Where it really is.
    real: PathBuf,
    /// The same, as a person writes it.
    pub shown: PathBuf,
    last: Last,
}

/// A file only ever read.
pub struct ReadFile(File);

impl Read for ReadFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl ReadFile {
    pub fn id(&self) -> io::Result<FileId> {
        id_of_file(&self.0)
    }

    pub fn len(&self) -> io::Result<u64> {
        Ok(self.0.metadata()?.len())
    }
}

impl ReadRoot {
    /// The folder at `path`, through any link there: what it really is.
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::from(sys::open_root(path, true)?)
    }

    fn from(dir: File) -> io::Result<Self> {
        if !dir.metadata()?.is_dir() {
            return Err(io::Error::from(io::ErrorKind::NotADirectory));
        }
        // A folder on a network disk has no volume of this computer to be named by: it is named as the system does.
        let shown = sys::plain_path(&dir)?;
        let real = sys::final_path(&dir).unwrap_or_else(|_| shown.clone());
        Ok(Self { id: id_of_file(&dir)?, real, shown, dir, last: Mutex::default() })
    }

    /// Whether it is on a network disk.
    pub fn remote(&self) -> bool {
        sys::remote(&self.dir)
    }

    /// The folder `rel` below, none of whose names is a link.
    fn dir(&self, rel: &Path) -> io::Result<File> {
        if let Some((at, dir)) = &*self.last.lock().unwrap() {
            if at == rel {
                return dir.try_clone();
            }
        }
        let mut at = self.dir.try_clone()?;
        for name in names(rel)? {
            at = sys::open_at(&at, name, Want::Dir { hold: false }, false)?;
            if sys::is_redirect(&at)? {
                return Err(io::Error::other(format!("{}: a link", rel.display()))); // i18n: not shown (an error chained under a message)
            }
        }
        *self.last.lock().unwrap() = Some((rel.to_path_buf(), at.try_clone()?));
        Ok(at)
    }

    pub fn entries(&self, rel: &Path) -> io::Result<Vec<Entry>> {
        entries(&self.dir(rel)?)
    }

    /// The folder the link at `rel` leads to.
    pub fn follow_dir(&self, rel: &Path) -> io::Result<ReadRoot> {
        let (parent, name) = split(rel)?;
        ReadRoot::from(sys::open_at(&self.dir(parent)?, name, Want::Dir { hold: true }, true)?)
    }

    /// The file at `rel` (`follow`: through the link it is).
    pub fn file(&self, rel: &Path, follow: bool) -> io::Result<ReadFile> {
        let (parent, name) = split(rel)?;
        Ok(ReadFile(sys::open_at(&self.dir(parent)?, name, Want::File, follow)?))
    }

    /// The whole file at `rel`.
    pub fn read(&self, rel: &Path) -> io::Result<Vec<u8>> {
        let mut out = vec![];
        self.file(rel, true)?.read_to_end(&mut out)?;
        Ok(out)
    }

    /// What the link or file at `rel` really is, for a trial never to go into it.
    pub fn protected_at(&self, rel: &Path) -> io::Result<Protected> {
        let (parent, name) = split(rel)?;
        let parent = self.dir(parent)?;
        let open = sys::open_at(&parent, name, Want::Dir { hold: false }, true).or_else(|_| sys::open_at(&parent, name, Want::File, true))?;
        Ok(Protected { id: Some(id_of_file(&open)?), real: sys::final_path(&open)?, shown: sys::plain_path(&open)? })
    }

    pub fn protected(&self) -> Protected {
        Protected { id: Some(self.id), real: self.real.clone(), shown: self.shown.clone() }
    }

    /// Whether `other` is this very folder or is inside it.
    pub fn holds(&self, other: &Protected) -> bool {
        other.id == Some(self.id) || sys::within(&other.real, &self.real)
    }
}

/// A folder (or file) the original can see: a trial never goes into it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Protected {
    /// `None`: what a link of it says and that is not there.
    pub id: Option<FileId>,
    pub real: PathBuf,
    /// As a person writes it.
    pub shown: PathBuf,
}

/// Where a new folder may be made: a place on a disk of this computer, held, checked against everything the original can see.
pub struct Destination {
    /// The nearest folder of the place that exists.
    parent: ReadRoot,
    /// The folders of the place that do not exist yet, outermost first (none for a trial).
    missing: Vec<OsString>,
    name: OsString,
}

/// Whether folders on the way that do not exist are made with the new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parents {
    /// They must exist (a trial).
    Exist,
    /// Made, each relative to the one above (a project's copy, a new project).
    Make,
}

/// Why a place is refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    NotAbsolute,
    /// Not an ordinary path (`\\?\`, a device) or not on a disk of this computer.
    NotLocal,
    /// The folder is not there (Orbiters makes none on the way).
    NoFolder(PathBuf),
    /// Inside a folder the original is, links to or takes a package from.
    Shared(PathBuf),
    InsideProject(PathBuf),
    Exists(PathBuf),
    Name(String),
    Unreadable(String),
    /// The original links to a folder Orbiters keeps its own files in (or a duplicate's, Creator Companion).
    Own(PathBuf),
    /// In Creator Companion's or ALCOM's folder.
    Theirs(String),
}

impl Refused {
    pub fn text(&self) -> String {
        match self {
            Refused::NotAbsolute => tr("Choose an absolute destination folder.").into(),
            Refused::NotLocal => tr("Choose an ordinary folder on a disk of this computer: a copy is not made on a network share.").into(),
            Refused::NoFolder(p) => tr!("{path} is not a folder: choose one that exists.", path = p.display()),
            Refused::Shared(p) => tr!("The original uses {path}: a copy there would change it. Choose a folder the original does not use.", path = p.display()),
            Refused::InsideProject(p) => tr!("{project} is a Unity project: choose a folder outside any Unity project.", project = p.display()),
            Refused::Exists(p) => tr!("{path} exists already: choose another name.", path = p.display()),
            Refused::Name(why) | Refused::Theirs(why) => why.clone(),
            Refused::Own(p) => tr!("The original links to {path}, where Orbiters or Creator Companion keeps its own files: working on a copy of it would write in what it links to.", path = p.display()),
            Refused::Unreadable(why) => tr!("This place cannot be checked: {why}", why = why),
        }
    }
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text())
    }
}

impl std::error::Error for Refused {}

impl Destination {
    /// Checks `parent`/`name` by reading only. `protected`: what the original can see.
    pub fn check(parent: &Path, name: &str, protected: &[Protected], parents: Parents) -> Result<Self, Refused> {
        crate::templates::validate_name(name).map_err(|e| Refused::Name(format!("{e:#}")))?;
        if !parent.is_absolute() {
            return Err(Refused::NotAbsolute);
        }
        #[cfg(windows)]
        let plain = matches!(parent.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), std::path::Prefix::Disk(_)));
        #[cfg(not(windows))]
        let plain = true;
        if !plain || parent.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(Refused::NotLocal);
        }
        // What is made here is written through its held folder: that it is none of theirs is asked once, of the place.
        crate::disk::tree(&parent.join(name)).map_err(|e| Refused::Theirs(e.to_string()))?;
        // The nearest folder of it that is there; what is missing below it are plain names.
        let (mut existing, mut missing) = (parent, vec![]);
        while parents == Parents::Make && !existing.exists() {
            let (Some(name), Some(above)) = (existing.file_name(), existing.parent()) else { return Err(Refused::NoFolder(parent.to_path_buf())) };
            crate::templates::validate_name(&name.to_string_lossy()).map_err(|e| Refused::Name(format!("{e:#}")))?;
            missing.insert(0, name.to_os_string());
            existing = above;
        }
        let parent = existing;
        let held = ReadRoot::open(parent).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => Refused::NoFolder(parent.to_path_buf()),
            _ if parent.is_file() => Refused::NoFolder(parent.to_path_buf()),
            // No volume of this computer: a share behind a drive letter, a disk the system cannot name.
            _ if parent.is_dir() => Refused::NotLocal,
            _ => Refused::Unreadable(e.to_string()),
        })?;
        if held.remote() || sys::final_path(&held.dir).is_err() {
            return Err(Refused::NotLocal);
        }
        // The folder to be made.
        let new = missing.iter().fold(held.real.clone(), |at, m| at.join(m)).join(name);
        for p in protected {
            // (Where a link leads that is not there: a folder made there, or around it, would be what it leads to.)
            let nowhere = p.id.is_none() && (sys::within(&new, &p.real) || sys::within(&p.real, &new));
            if p.id == Some(held.id) || sys::within(&held.real, &p.real) || nowhere {
                return Err(Refused::Shared(p.shown.clone()));
            }
        }
        // Every folder above, by what it is: compared by identity as well, and none may be a Unity project.
        let mut above: Option<&Path> = Some(&held.real);
        while let Some(folder) = above {
            if let Ok(dir) = sys::open_root(folder, false) {
                let id = id_of_file(&dir).map_err(|e| Refused::Unreadable(e.to_string()))?;
                if let Some(p) = protected.iter().find(|p| p.id == Some(id)) {
                    return Err(Refused::Shared(p.shown.clone()));
                }
                let settings = sys::open_at(&dir, OsStr::new("ProjectSettings"), Want::Dir { hold: false }, true);
                if settings.is_ok_and(|s| sys::open_at(&s, OsStr::new("ProjectVersion.txt"), Want::File, true).is_ok()) {
                    return Err(Refused::InsideProject(sys::plain_path(&dir).unwrap_or_else(|_| folder.to_path_buf())));
                }
            }
            above = folder.parent();
        }
        if !missing.is_empty() {
            return Ok(Self { parent: held, missing, name: name.into() });
        }
        // Anything at all under that name, whatever it is and however it is spelled (its case, its short name).
        let free = [Want::Dir { hold: false }, Want::File].into_iter().all(|w| sys::open_at(&held.dir, OsStr::new(name), w, false).is_err_and(|e| e.kind() == io::ErrorKind::NotFound));
        if !free {
            return Err(Refused::Exists(held.shown.join(name)));
        }
        Ok(Self { parent: held, missing, name: name.into() })
    }

    /// The new folder's own path, in its real form (no link, no substituted drive).
    pub fn path(&self) -> PathBuf {
        let mut out = self.parent.shown.clone();
        out.extend(&self.missing);
        out.join(&self.name)
    }

    /// Room for this person on its disk.
    pub fn free(&self) -> Option<u64> {
        sys::free_space(&self.parent.dir)
    }

    /// Makes the folder: new (never one that is there), in the very parent checked; what is missing above it first, each
    /// relative to the handle of the one above.
    pub fn make(self) -> io::Result<NewFolder> {
        let path = self.path();
        let mut made = NewFolder { id: FileId { volume: 0, index: 0, born: 0 }, path, parent: self.parent, chain: vec![] };
        for name in self.missing.iter().chain([&self.name]) {
            let above = made.chain.last().map_or(&made.parent.dir, |c| &c.dir);
            let step = (|| -> io::Result<Link> {
                let dir = sys::open_at(above, name, Want::NewDir, false)?;
                let (id, real, over) = (id_of_file(&dir)?, sys::final_path(&dir)?, sys::final_path(above)?);
                if sys::is_link(&dir)? || !sys::within(&real, &over) || real == over || id.volume != made.parent.id.volume {
                    return Err(io::Error::other("the folder made is not in the place checked")); // i18n: not shown (an error chained under a message)
                }
                Ok(Link { dir, name: name.clone(), id })
            })();
            match step {
                Ok(link) => made.chain.push(link),
                Err(e) => {
                    let _ = made.remove();
                    return Err(e);
                }
            }
        }
        made.id = made.chain.last().map(|l| l.id).unwrap_or(made.id);
        Ok(made)
    }
}

struct Link {
    dir: File,
    name: OsString,
    id: FileId,
}

/// The folder just made, held with every folder made above it and the parent it is in: the proof for the whole operation.
pub struct NewFolder {
    pub id: FileId,
    pub path: PathBuf,
    parent: ReadRoot,
    chain: Vec<Link>,
}

impl NewFolder {
    /// What writes in it.
    pub fn root(&self) -> io::Result<WriteRoot> {
        let dir = match self.chain.last() {
            Some(l) => l.dir.try_clone()?,
            None => return Err(io::Error::other("nothing was made")), // i18n: not shown (an error chained under a message)
        };
        Ok(WriteRoot { id: self.id, real: sys::final_path(&dir)?, path: self.path.clone(), dir, last: Mutex::default() })
    }

    /// Lets the folder go and removes it, empty, with the folders made above it, each when it is still the very one made.
    /// Whether the folder itself went.
    pub fn remove(self) -> io::Result<bool> {
        let NewFolder { parent, mut chain, .. } = self;
        let (mut itself, mut first) = (Ok(false), true);
        while let Some(link) = chain.pop() {
            drop(link.dir);
            let above = chain.last().map_or(&parent.dir, |c| &c.dir);
            let gone = sys::remove_at(above, &link.name, link.id, true);
            let went = matches!(gone, Ok(true));
            if first {
                itself = gone;
                first = false;
            }
            if !went {
                break;
            }
        }
        itself
    }
}

/// What files are written through: see the module.
pub struct WriteRoot {
    dir: File,
    pub id: FileId,
    real: PathBuf,
    pub path: PathBuf,
    last: Last,
}

/// Which folder of a job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The trial Orbiters copied.
    Trial,
    /// The copy Creator Companion made of it, taken in its place.
    Migrated,
}

/// A folder made during a copy, held while this lives (Windows: nobody can rename, move or delete it meanwhile). Elsewhere holding
/// it open keeps nobody from anything, and a process may only open so many files (256 for a Mac app, and a copy makes thousands of
/// folders: PRJ-2): nothing is held, each use asks again ([`WriteRoot::inside`]).
pub struct HeldDir {
    #[cfg(windows)]
    #[allow(dead_code)]
    held: File,
}

/// A file being made.
pub struct NewFile {
    file: File,
    pub id: FileId,
}

impl Write for NewFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl NewFile {
    /// Its modification time and executable bits, those of the file it copies (a disk that cannot keep them still has the
    /// whole file).
    pub fn like(&self, from: &ReadFile) -> io::Result<()> {
        if let Ok(modified) = from.0.metadata().and_then(|m| m.modified()) {
            let _ = self.file.set_modified(modified);
        }
        sys::set_executable_like(&self.file, &from.0)
    }
}

impl WriteRoot {
    /// The folder a job of the journal in `dir` has in `role`, when it may be written in now and is the very one recorded:
    /// looked up here, so that no other place (the job's original, a folder chosen somewhere) can ever be passed for it.
    pub fn of_job(dir: &Path, id: &str, role: Role) -> anyhow::Result<Self> {
        use crate::unity6::journal::{self, Phase};
        let record = journal::read(dir);
        if let Some(why) = record.error.clone().or_else(|| record.uncertain(id)) {
            anyhow::bail!("{}", journal::uncertain_text(&why));
        }
        let job = record.job(id).ok_or_else(|| anyhow::anyhow!("{}", journal::gone_text()))?;
        let place = match role {
            Role::Trial => Some(&job.trial),
            Role::Migrated => job.migrated.as_ref(),
        };
        let Some(place) = place else { anyhow::bail!("{}", journal::gone_text()) };
        let not_it = || anyhow::anyhow!("{}", tr!("The folder at {path} is not the trial Orbiters made.", path = place.path.display()));
        let allowed = match role {
            Role::Trial => !matches!(job.phase, Phase::Planned),
            Role::Migrated => matches!(job.phase, Phase::Ready | Phase::Abandoned { .. }),
        };
        let recorded = place.id.ok_or_else(not_it)?;
        // Never the original, whatever the record says.
        let apart = job.source.id.is_some_and(|source| source != recorded) && crate::owned::id_of(&job.source.path).ok() != Some(recorded);
        if !allowed || !apart {
            return Err(not_it());
        }
        crate::disk::content(&place.path)?;
        let held = sys::open_root(&place.path, false).map_err(|_| not_it())?;
        let now = id_of_file(&held)?;
        if sys::is_link(&held)? || now != recorded || !held.metadata()?.is_dir() {
            return Err(not_it());
        }
        Ok(Self { id: now, real: sys::final_path(&held)?, path: place.path.clone(), dir: held, last: Mutex::default() })
    }

    /// `file` (just opened below this root) as the gate takes it: not a link, on the root's volume, below the root.
    fn inside(&self, file: File, rel: &Path) -> io::Result<File> {
        let (id, real) = (id_of_file(&file)?, sys::final_path(&file)?);
        // Held on Windows; elsewhere it may have been renamed, and its place now counts.
        let root = if cfg!(windows) { self.real.clone() } else { sys::final_path(&self.dir)? };
        if sys::is_link(&file)? || id.volume != self.id.volume || !sys::within(&real, &root) || real == root {
            return Err(io::Error::other(tr!("{path} is a link or leads outside the trial: Orbiters does not write through it.", path = rel.display())));
        }
        Ok(file)
    }

    fn dir(&self, rel: &Path) -> io::Result<File> {
        if let Some((at, dir)) = &*self.last.lock().unwrap() {
            if at == rel {
                let dir = dir.try_clone()?;
                // Not held elsewhere than on Windows: still below the root before anything is made in it.
                return if cfg!(windows) || rel.as_os_str().is_empty() { Ok(dir) } else { self.inside(dir, rel) };
            }
        }
        let mut at = self.dir.try_clone()?;
        let mut so_far = PathBuf::new();
        for name in names(rel)? {
            so_far.push(name);
            at = self.inside(sys::open_at(&at, name, Want::Dir { hold: true }, false)?, &so_far)?;
        }
        *self.last.lock().unwrap() = Some((rel.to_path_buf(), at.try_clone()?));
        Ok(at)
    }

    /// A new folder at `rel` (its parent made before), with its identity, held.
    pub fn make_dir(&self, rel: &Path) -> io::Result<(FileId, HeldDir)> {
        let (parent, name) = split(rel)?;
        let made = self.inside(sys::open_at(&self.dir(parent)?, name, Want::NewDir, false)?, rel)?;
        let id = id_of_file(&made)?;
        Ok((
            id,
            HeldDir {
                #[cfg(windows)]
                held: made,
            },
        ))
    }

    /// A new file at `rel`: never one that is there.
    pub fn make_file(&self, rel: &Path) -> io::Result<NewFile> {
        let (parent, name) = split(rel)?;
        let file = self.inside(sys::open_at(&self.dir(parent)?, name, Want::NewFile, false)?, rel)?;
        Ok(NewFile { id: id_of_file(&file)?, file })
    }

    pub fn entries(&self, rel: &Path) -> io::Result<Vec<Entry>> {
        entries(&self.dir(rel)?)
    }

    /// The file at `rel`, to read.
    pub fn file(&self, rel: &Path) -> io::Result<ReadFile> {
        let (parent, name) = split(rel)?;
        Ok(ReadFile(self.inside(sys::open_at(&self.dir(parent)?, name, Want::File, false)?, rel)?))
    }

    /// Removes the file or (empty) folder at `rel` when it is the very one `id` names.
    pub fn remove(&self, rel: &Path, id: FileId, is_dir: bool) -> io::Result<bool> {
        let (parent, name) = split(rel)?;
        *self.last.lock().unwrap() = None;
        let dir = self.dir(parent)?;
        *self.last.lock().unwrap() = None;
        sys::remove_at(&dir, name, id, is_dir)
    }

    /// Room on its disk for this person.
    pub fn free(&self) -> Option<u64> {
        sys::free_space(&self.dir)
    }
}

/// Folders written besides a copy (`own`: Orbiters' app data; VCC's, for a duplicate, which it is added to) are nothing the original can see: else what is written there would
/// be written in the original. A folder that is not there yet is asked by the nearest one above it.
pub fn apart(own: &[&Path], protected: &[Protected]) -> Result<(), Refused> {
    for folder in own {
        let mut at: &Path = folder;
        while !at.exists() {
            let Some(above) = at.parent() else { break };
            at = above;
        }
        let Ok(held) = ReadRoot::open(at) else { continue };
        let mut above = Some(held.real.as_path());
        // Either way round: what is written here is seen in the original, or the original sees what is kept here. (The original
        // itself, first among them, may be somewhere inside: a template of VCC's.)
        let mut shared = protected.iter().any(|p| p.id == Some(held.id) || sys::within(&held.real, &p.real)) || (at == *folder && protected.iter().skip(1).any(|p| sys::within(&p.real, &held.real)));
        while let (false, Some(up)) = (shared, above) {
            let id = sys::open_root(up, false).and_then(|d| id_of_file(&d)).ok();
            shared = id.is_some() && protected.iter().any(|p| p.id == id);
            above = up.parent();
        }
        if shared {
            return Err(Refused::Own(folder.to_path_buf()));
        }
    }
    Ok(())
}

/// Whether Unity holds the project's lock file, asked by reading only (the original is never opened for writing).
pub fn unity_holds(project: &Path) -> bool {
    crate::unity_open(project)
}

/// The program at `path`, held so that nothing replaces it while it is looked at and started (Windows; elsewhere a program
/// that runs keeps its file whatever is put at its name).
pub fn hold_program(path: &Path) -> io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().read(true).share_mode(1).open(path)
    }
    #[cfg(not(windows))]
    {
        File::open(path)
    }
}

/// Orbiters' app data: the only other place this feature writes. Its names are opened relative to it, never through a link.
pub struct AppRoot {
    dir: File,
}

/// A lock on a file of app data, across every process of this computer; released when dropped, or by the system when the
/// process holding it ends (so no lock is ever stale).
pub struct Lock(File);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl AppRoot {
    /// `dir`, made when it is not there.
    pub fn open(dir: &Path) -> io::Result<Self> {
        crate::disk::create_dir_all(dir)?;
        Self::existing(dir)
    }

    /// `dir`, when it is there.
    pub fn existing(dir: &Path) -> io::Result<Self> {
        crate::disk::content(dir)?;
        Ok(Self { dir: sys::open_root(dir, true)? })
    }

    fn sub(&self, folder: &Path, make: bool) -> io::Result<File> {
        let mut at = self.dir.try_clone()?;
        for name in names(folder)? {
            let open = |at: &File| sys::open_at(at, name, Want::Dir { hold: false }, false);
            let next = match open(&at) {
                Err(e) if make && e.kind() == io::ErrorKind::NotFound => match sys::open_at(&at, name, Want::NewDir, false) {
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => open(&at)?,
                    other => other?,
                },
                other => other?,
            };
            if sys::is_link(&next)? {
                return Err(io::Error::other(format!("{}: a link", folder.display()))); // i18n: not shown (an error chained under a message)
            }
            at = next;
        }
        Ok(at)
    }

    fn at(&self, rel: &Path, want: Want) -> io::Result<File> {
        let (parent, name) = split(rel)?;
        let make = matches!(want, Want::NewFile | Want::Append | Want::Lock);
        let file = sys::open_at(&self.sub(parent, make)?, name, want, false)?;
        if sys::is_link(&file)? {
            return Err(io::Error::other(format!("{}: a link", rel.display()))); // i18n: not shown (an error chained under a message)
        }
        Ok(file)
    }

    pub fn read(&self, rel: &Path) -> io::Result<Vec<u8>> {
        let mut out = vec![];
        self.at(rel, Want::File)?.read_to_end(&mut out)?;
        Ok(out)
    }

    /// A new file with `bytes`, never over one that is there.
    pub fn write_new(&self, rel: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut file = self.at(rel, Want::NewFile)?;
        file.write_all(bytes)?;
        file.sync_all()
    }

    /// `bytes` at the end of the file, on the disk when this returns.
    pub fn append(&self, rel: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut file = self.at(rel, Want::Append)?;
        file.write_all(bytes)?;
        file.sync_all()
    }

    /// `value` as the file's whole content, published at once.
    pub fn replace_json(&self, rel: &Path, value: &impl serde::Serialize) -> anyhow::Result<()> {
        let (parent, name) = split(rel)?;
        let dir = self.sub(parent, true)?;
        let temp = OsString::from(format!("{}.{}.tmp", name.to_string_lossy(), uuid::Uuid::new_v4().simple())); // i18n: not shown (a file's name)
        let mut file = sys::open_at(&dir, &temp, Want::NewFile, false)?;
        let done = (|| -> anyhow::Result<()> {
            file.write_all(&serde_json::to_vec_pretty(value)?)?;
            file.sync_all()?;
            Ok(sys::publish_at(&dir, &file, &temp, name)?)
        })();
        if done.is_err() {
            if let Ok(id) = id_of_file(&file) {
                drop(file);
                let _ = sys::remove_at(&dir, &temp, id, false);
            }
        }
        done
    }

    /// Waits for the lock file `rel`.
    pub fn lock(&self, rel: &Path) -> io::Result<Lock> {
        let file = self.at(rel, Want::Lock)?;
        file.lock()?;
        Ok(Lock(file))
    }

    /// The lock file `rel`, when nobody holds it.
    pub fn try_lock(&self, rel: &Path) -> io::Result<Option<Lock>> {
        let file = self.at(rel, Want::Lock)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Lock(file))),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => Err(e),
        }
    }

    /// The files in the folder `rel` (none when it is not there).
    pub fn files(&self, rel: &Path) -> Vec<OsString> {
        let listed = self.sub(rel, false).and_then(|d| entries(&d)).unwrap_or_default();
        listed.into_iter().filter(|e| matches!(e.kind, Kind::File { .. })).map(|e| e.name).collect()
    }

    /// The folders in the folder `rel` (none when it is not there).
    pub fn folders(&self, rel: &Path) -> Vec<OsString> {
        let listed = self.sub(rel, false).and_then(|d| entries(&d)).unwrap_or_default();
        listed.into_iter().filter(|e| e.kind == Kind::Dir).map(|e| e.name).collect()
    }

    /// Removes the folder `rel` and what is in it, following nothing: a link in it is taken away as one.
    pub fn remove_tree(&self, rel: &Path) -> io::Result<()> {
        let dir = self.sub(rel, false)?;
        empty(&dir)?;
        let (parent, name) = split(rel)?;
        let id = id_of_file(&dir)?;
        drop(dir);
        sys::remove_at(&self.sub(parent, false)?, name, id, true).map(|_| ())
    }

    /// The folder `rel`, made when it is not there.
    pub fn make_dir(&self, rel: &Path) -> io::Result<()> {
        self.sub(rel, true).map(|_| ())
    }

    /// Removes the file `rel` (never a link, never a folder).
    pub fn remove_file(&self, rel: &Path) -> io::Result<()> {
        let (parent, name) = split(rel)?;
        let dir = self.sub(parent, false)?;
        let id = id_of_file(&sys::open_at(&dir, name, Want::File, false)?)?;
        sys::remove_at(&dir, name, id, false).map(|_| ())
    }
}

fn empty(dir: &File) -> io::Result<()> {
    for e in entries(dir)? {
        match e.kind {
            Kind::Dir => {
                let sub = sys::open_at(dir, &e.name, Want::Dir { hold: false }, false)?;
                let id = id_of_file(&sub)?;
                empty(&sub)?;
                drop(sub);
                sys::remove_at(dir, &e.name, id, true)?;
            }
            Kind::Link { dir: folder } => sys::unlink_at(dir, &e.name, folder)?,
            Kind::File { .. } | Kind::Other => sys::unlink_at(dir, &e.name, false)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
