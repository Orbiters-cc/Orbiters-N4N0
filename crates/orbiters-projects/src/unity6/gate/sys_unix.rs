//! The gate's operations elsewhere than Windows: every name is opened relative to a folder's descriptor (`openat`), one
//! component at a time, without following a link unless it is asked to.

use crate::owned::FileId;
use std::ffi::{CString, OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// What is wanted of a name under a held folder (nothing holds a name here: it is compared by identity instead).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Want {
    Dir { hold: bool },
    NewDir,
    File,
    NewFile,
    Append,
    Lock,
    Delete { dir: bool },
}

fn c(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes()).map_err(io::Error::other)
}

fn fd(n: libc::c_int) -> io::Result<File> {
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a descriptor just opened, owned here.
    Ok(unsafe { File::from_raw_fd(n) })
}

/// `name` (one component) under `dir`. `follow`: through a link, to what it leads to.
pub fn open_at(dir: &File, name: &OsStr, want: Want, follow: bool) -> io::Result<File> {
    let name = c(name)?;
    let stay = if follow { 0 } else { libc::O_NOFOLLOW };
    let flags = match want {
        Want::Dir { .. } => libc::O_RDONLY | libc::O_DIRECTORY,
        Want::NewDir => {
            // SAFETY: a live descriptor and a null-terminated name.
            if unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o777) } != 0 {
                return Err(io::Error::last_os_error());
            }
            libc::O_RDONLY | libc::O_DIRECTORY
        }
        // Never waits on a pipe put at that name.
        Want::File => libc::O_RDONLY | libc::O_NONBLOCK,
        Want::NewFile => libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
        Want::Append => libc::O_RDWR | libc::O_APPEND | libc::O_CREAT,
        Want::Lock => libc::O_RDWR | libc::O_CREAT,
        Want::Delete { dir } => libc::O_RDONLY | libc::O_NONBLOCK | if dir { libc::O_DIRECTORY } else { 0 },
    };
    // SAFETY: a live descriptor and a null-terminated name; the mode is used only when a file is made.
    let file = fd(unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags | stay | libc::O_CLOEXEC, 0o666 as libc::c_uint) })?;
    let is_dir = file.metadata()?.is_dir();
    if matches!(want, Want::File | Want::NewFile | Want::Append | Want::Lock) && is_dir {
        return Err(io::Error::from(io::ErrorKind::IsADirectory));
    }
    Ok(file)
}

/// The folder at `path` itself (a link is not followed at its end).
pub fn open_root(path: &Path, follow: bool) -> io::Result<File> {
    let name = c(path.as_os_str())?;
    let stay = if follow { 0 } else { libc::O_NOFOLLOW };
    // SAFETY: a null-terminated path.
    fd(unsafe { libc::open(name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | stay | libc::O_CLOEXEC) })
}

/// An open descriptor is never a link's own: `O_NOFOLLOW` refused it.
pub fn is_link(file: &File) -> io::Result<bool> {
    Ok(file.metadata()?.file_type().is_symlink())
}

/// An open descriptor is never a link's own.
pub fn is_redirect(file: &File) -> io::Result<bool> {
    is_link(file)
}

/// Where the open object really is.
pub fn final_path(file: &File) -> io::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut buffer = vec![0u8; libc::PATH_MAX as usize + 1];
        // SAFETY: F_GETPATH fills at most PATH_MAX bytes.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buffer.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        buffer.truncate(buffer.iter().position(|&b| b == 0).unwrap_or(0));
        Ok(PathBuf::from(OsString::from_vec(buffer)))
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
    }
}

pub fn plain_path(file: &File) -> io::Result<PathBuf> {
    final_path(file)
}

/// Whether `path` is `root` or below it. APFS usually ignores case: compared without it, so that nothing is let through by
/// its spelling.
pub fn within(path: &Path, root: &Path) -> bool {
    let (p, r) = (path.to_string_lossy().to_lowercase(), root.to_string_lossy().to_lowercase());
    let r = r.trim_end_matches('/');
    p.trim_end_matches('/') == r || p.starts_with(&format!("{r}/"))
}

pub fn free_space(dir: &File) -> Option<u64> {
    crate::size::free_space(&final_path(dir).ok()?)
}

/// Whether the open folder is on a network disk.
pub fn remote(dir: &File) -> bool {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: a zeroed statfs is a valid out-value for the call.
        let mut s: libc::statfs = unsafe { std::mem::zeroed() };
        unsafe { libc::fstatfs(dir.as_raw_fd(), &mut s) != 0 || s.f_flags & libc::MNT_LOCAL as u32 == 0 }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = dir;
        false
    }
}

/// One entry of a folder.
pub struct Listed {
    pub name: OsString,
    pub dir: bool,
    pub link: bool,
    pub special: bool,
    pub len: u64,
}

pub fn list(dir: &File) -> io::Result<Vec<Listed>> {
    // A descriptor of its own: closing the stream closes it, and its place in the folder is its own.
    let dot = c(OsStr::new("."))?;
    // SAFETY: a live descriptor and a null-terminated name.
    let copy = unsafe { libc::openat(dir.as_raw_fd(), dot.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if copy < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `copy` is a directory's descriptor; the stream owns it from here.
    let stream = unsafe { libc::fdopendir(copy) };
    if stream.is_null() {
        let e = io::Error::last_os_error();
        // SAFETY: not taken by a stream.
        unsafe { libc::close(copy) };
        return Err(e);
    }
    let mut names = vec![];
    // `readdir_r` tells an error from the end of the folder (a listing cut short is never taken for a whole one).
    let failed = loop {
        // SAFETY: a zeroed entry is a valid out-value.
        let mut entry: libc::dirent = unsafe { std::mem::zeroed() };
        let mut got: *mut libc::dirent = std::ptr::null_mut();
        // SAFETY: a live stream and live out-values.
        #[allow(deprecated)]
        let code = unsafe { libc::readdir_r(stream, &mut entry, &mut got) };
        if code != 0 {
            break Some(io::Error::from_raw_os_error(code));
        }
        if got.is_null() {
            break None;
        }
        // SAFETY: the system ends the name with a zero.
        let name = unsafe { std::ffi::CStr::from_ptr(entry.d_name.as_ptr()) }.to_bytes().to_vec();
        if name != b"." && name != b".." {
            names.push(OsString::from_vec(name));
        }
    };
    // SAFETY: the stream opened above.
    unsafe { libc::closedir(stream) };
    if let Some(e) = failed {
        return Err(e);
    }
    let mut out = vec![];
    for name in names {
        let at = c(&name)?;
        // SAFETY: a zeroed stat is a valid out-value for the call.
        let mut s: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: a live descriptor, a null-terminated name and a live out-value.
        if unsafe { libc::fstatat(dir.as_raw_fd(), at.as_ptr(), &mut s, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let kind = s.st_mode as u32 & libc::S_IFMT as u32;
        let (folder, link, regular) = (kind == libc::S_IFDIR as u32, kind == libc::S_IFLNK as u32, kind == libc::S_IFREG as u32);
        out.push(Listed { name, dir: folder, link, special: !folder && !link && !regular, len: s.st_size as u64 });
    }
    Ok(out)
}

/// Removes the file or (empty) folder `name` under `dir` when it is the very object `id` names. Here nothing holds a name, so
/// it is first moved to a folder of this call's own, where nobody else puts anything, and looked at there: what is not the
/// object meant goes back to where it was (or, when its name was taken again meanwhile, stays there, whole).
pub fn remove_at(dir: &File, name: &OsStr, id: FileId, is_dir: bool) -> io::Result<bool> {
    let before = open_at(dir, name, Want::Delete { dir: is_dir }, false)?.metadata()?;
    if (before.dev(), before.ino()) != (id.volume, id.index) {
        return Ok(false);
    }
    let own = OsString::from(format!(".orbiters-removing-{}", uuid::Uuid::new_v4().simple()));
    let (aside, at, inner) = (c(&own)?, c(name)?, c(OsStr::new("x"))?);
    // SAFETY: a live descriptor and null-terminated names throughout.
    unsafe {
        if libc::mkdirat(dir.as_raw_fd(), aside.as_ptr(), 0o700) != 0 {
            return Err(io::Error::last_os_error());
        }
        let room = open_at(dir, &own, Want::Dir { hold: false }, false)?;
        if libc::renameat(dir.as_raw_fd(), at.as_ptr(), room.as_raw_fd(), inner.as_ptr()) != 0 {
            let e = io::Error::last_os_error();
            libc::unlinkat(dir.as_raw_fd(), aside.as_ptr(), libc::AT_REMOVEDIR);
            return Err(e);
        }
        let moved = open_at(&room, OsStr::new("x"), Want::Delete { dir: is_dir }, false).and_then(|f| f.metadata());
        let mine = moved.is_ok_and(|m| (m.dev(), m.ino()) == (id.volume, id.index));
        let gone = mine && libc::unlinkat(room.as_raw_fd(), inner.as_ptr(), if is_dir { libc::AT_REMOVEDIR } else { 0 }) == 0;
        let failed = (mine && !gone).then(io::Error::last_os_error);
        if !gone {
            // Back where it was, never over what is there now (decided by the system, in one step); else it stays there, whole.
            put_back(&room, &inner, dir, &at);
        }
        libc::unlinkat(dir.as_raw_fd(), aside.as_ptr(), libc::AT_REMOVEDIR);
        match failed {
            Some(e) => Err(e),
            None => Ok(gone),
        }
    }
}

/// Puts `from` of `room` at `to` of `dir` when nothing is there.
fn put_back(room: &File, from: &std::ffi::CStr, dir: &File, to: &std::ffi::CStr) {
    // SAFETY: live descriptors and null-terminated names.
    #[cfg(target_vendor = "apple")]
    unsafe {
        libc::renameatx_np(room.as_raw_fd(), from.as_ptr(), dir.as_raw_fd(), to.as_ptr(), libc::RENAME_EXCL);
    }
    // SAFETY: as above; 1 is RENAME_NOREPLACE.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe {
        libc::syscall(libc::SYS_renameat2, room.as_raw_fd(), from.as_ptr(), dir.as_raw_fd(), to.as_ptr(), 1u32);
    }
    #[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
    let _ = (room, from, dir, to);
}

/// Takes away the file or link `name` itself under `dir` (never what a link leads to): for Orbiters' own staging folder.
pub fn unlink_at(dir: &File, name: &OsStr, _is_dir: bool) -> io::Result<()> {
    let at = c(name)?;
    // SAFETY: a live descriptor and a null-terminated name.
    if unsafe { libc::unlinkat(dir.as_raw_fd(), at.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Puts the file `from` under `dir` at `name` under `dir`, in place of what is there.
pub fn publish_at(dir: &File, _file: &File, from: &OsStr, name: &OsStr) -> io::Result<()> {
    let (from, to) = (c(from)?, c(name)?);
    // SAFETY: live descriptors and null-terminated names.
    if unsafe { libc::renameat(dir.as_raw_fd(), from.as_ptr(), dir.as_raw_fd(), to.as_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// A new file is not executable here, and a copy must be usable: a package's tools, a plugin's programs.
pub fn set_executable_like(to: &File, from: &File) -> io::Result<()> {
    let executable = from.metadata()?.permissions().mode() & 0o111;
    if executable != 0 {
        let mut permissions = to.metadata()?.permissions();
        permissions.set_mode(permissions.mode() | executable);
        to.set_permissions(permissions)?;
    }
    Ok(())
}
