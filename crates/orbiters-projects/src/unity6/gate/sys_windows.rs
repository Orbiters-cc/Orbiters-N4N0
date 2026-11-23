//! The gate's operations on Windows: every name is opened **relative to a handle** (`NtCreateFile` with a root directory),
//! one component at a time and without following a reparse point unless it is asked to.

use crate::owned::FileId;
use std::ffi::{c_void, OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::*;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *const u16,
}
#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: *mut c_void,
    name: *const UnicodeString,
    attributes: u32,
    descriptor: *const c_void,
    quality: *const c_void,
}
#[repr(C)]
struct IoStatus {
    status: usize,
    information: usize,
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtCreateFile(
        handle: *mut *mut c_void,
        access: u32,
        object: *const ObjectAttributes,
        io: *mut IoStatus,
        size: *const i64,
        attributes: u32,
        share: u32,
        disposition: u32,
        options: u32,
        ea: *const c_void,
        ea_length: u32,
    ) -> i32;
    fn NtSetInformationFile(handle: *mut c_void, io: *mut IoStatus, information: *const c_void, length: u32, class: u32) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

const READ_DATA: u32 = 0x1;
const WRITE_DATA: u32 = 0x2;
const APPEND_DATA: u32 = 0x4;
const TRAVERSE: u32 = 0x20;
const READ_ATTRIBUTES: u32 = 0x80;
const WRITE_ATTRIBUTES: u32 = 0x100;
const DELETE: u32 = 0x0001_0000;
const SYNCHRONIZE: u32 = 0x0010_0000;
const SHARE_READ: u32 = 1;
const SHARE_WRITE: u32 = 2;
const SHARE_DELETE: u32 = 4;
const OPEN: u32 = 1;
const CREATE: u32 = 2;
const OPEN_IF: u32 = 3;
const DIRECTORY: u32 = 0x1;
const SYNCHRONOUS: u32 = 0x20;
const NOT_DIRECTORY: u32 = 0x40;
const BACKUP_INTENT: u32 = 0x4000;
const REPARSE_POINT: u32 = 0x0020_0000;

/// What is wanted of a name under a held folder.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Want {
    /// A folder that is there, to list and go through; `hold`: while open, it cannot be renamed, moved or deleted.
    Dir { hold: bool },
    /// A new folder, held.
    NewDir,
    /// A file that is there, to read.
    File,
    /// A new file, to write.
    NewFile,
    /// A file to append to, made when it is not there.
    Append,
    /// A file to lock, made when it is not there.
    Lock,
    /// A file that is there, which nobody else has open.
    Alone,
    /// What is there, to delete.
    Delete { dir: bool },
}

fn check(status: i32) -> io::Result<()> {
    if status >= 0 {
        return Ok(());
    }
    // SAFETY: a plain conversion.
    Err(io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32))
}

/// `name` (one component) under `dir`. `follow`: through a link, to what it leads to (reading the original's linked folders).
pub fn open_at(dir: &File, name: &OsStr, want: Want, follow: bool) -> io::Result<File> {
    let wide: Vec<u16> = name.encode_wide().collect();
    let bytes = u16::try_from(wide.len() * 2).map_err(io::Error::other)?;
    let text = UnicodeString { length: bytes, maximum: bytes, buffer: wide.as_ptr() };
    let object = ObjectAttributes {
        length: std::mem::size_of::<ObjectAttributes>() as u32,
        root: dir.as_raw_handle(),
        name: &text,
        // Names are matched as Windows matches them, whatever their case.
        attributes: 0x40,
        descriptor: std::ptr::null(),
        quality: std::ptr::null(),
    };
    let all = SHARE_READ | SHARE_WRITE | SHARE_DELETE;
    let (access, share, disposition, options) = match want {
        Want::Dir { hold } => (READ_DATA | TRAVERSE | READ_ATTRIBUTES, if hold { SHARE_READ | SHARE_WRITE } else { all }, OPEN, DIRECTORY | BACKUP_INTENT),
        Want::NewDir => (READ_DATA | TRAVERSE | READ_ATTRIBUTES, SHARE_READ | SHARE_WRITE, CREATE, DIRECTORY | BACKUP_INTENT),
        Want::File => (READ_DATA | READ_ATTRIBUTES, all, OPEN, NOT_DIRECTORY),
        // Nobody else writes, renames or deletes it while it is being written.
        Want::NewFile => (READ_DATA | WRITE_DATA | READ_ATTRIBUTES | WRITE_ATTRIBUTES | DELETE, SHARE_READ, CREATE, NOT_DIRECTORY),
        Want::Append => (READ_DATA | APPEND_DATA | READ_ATTRIBUTES, SHARE_READ | SHARE_WRITE, OPEN_IF, NOT_DIRECTORY),
        Want::Lock => (READ_DATA | WRITE_DATA | READ_ATTRIBUTES, SHARE_READ | SHARE_WRITE, OPEN_IF, NOT_DIRECTORY),
        Want::Alone => (READ_DATA | WRITE_DATA | READ_ATTRIBUTES, SHARE_DELETE, OPEN, NOT_DIRECTORY),
        Want::Delete { dir } => (DELETE | READ_ATTRIBUTES, all, OPEN, if dir { DIRECTORY | BACKUP_INTENT } else { NOT_DIRECTORY }),
    };
    let options = options | SYNCHRONOUS | if follow { 0 } else { REPARSE_POINT };
    let mut handle = std::ptr::null_mut();
    let mut io = IoStatus { status: 0, information: 0 };
    // SAFETY: every pointer is to a value alive for the call; the handle is owned by the `File` on success.
    let status = unsafe { NtCreateFile(&mut handle, access | SYNCHRONIZE, &object, &mut io, std::ptr::null(), 0x80, share, disposition, options, std::ptr::null(), 0) };
    check(status)?;
    Ok(unsafe { File::from_raw_handle(handle) })
}

/// The folder at `path` itself (a link is not followed at its end), held.
pub fn open_root(path: &Path, follow: bool) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .access_mode(READ_DATA | TRAVERSE | READ_ATTRIBUTES)
        .share_mode(SHARE_READ | SHARE_WRITE)
        .custom_flags(0x0200_0000 | if follow { 0 } else { REPARSE_POINT })
        .open(path)
}

pub fn is_link(file: &File) -> io::Result<bool> {
    Ok(file.metadata()?.file_attributes() & 0x400 != 0)
}

/// Whether the open object stands for another place (a link, a junction, a mounted folder), not one merely kept online.
pub fn is_redirect(file: &File) -> io::Result<bool> {
    if !is_link(file)? {
        return Ok(false);
    }
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: a live handle and a value of the size given.
    unsafe { GetFileInformationByHandleEx(HANDLE(file.as_raw_handle()), FileAttributeTagInfo, (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(), std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32) }
        .map_err(|e| io::Error::from_raw_os_error((e.code().0 as u32 & 0xffff) as i32))?;
    Ok(info.ReparseTag & 0x2000_0000 != 0)
}

/// Where the open object really is: its path on its own volume, named by the volume's GUID.
pub fn final_path(file: &File) -> io::Result<PathBuf> {
    path_of(file, GETFINALPATHNAMEBYHANDLE_FLAGS(1))
}

/// Where it really is, as a person writes it (`C:\...`).
pub fn plain_path(file: &File) -> io::Result<PathBuf> {
    let path = path_of(file, GETFINALPATHNAMEBYHANDLE_FLAGS(0))?;
    let text = path.to_string_lossy().into_owned();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => Ok(PathBuf::from(rest)),
        _ => Ok(path),
    }
}

fn path_of(file: &File, flags: GETFINALPATHNAMEBYHANDLE_FLAGS) -> io::Result<PathBuf> {
    let mut buffer = vec![0u16; 1024];
    loop {
        // SAFETY: a live handle and a buffer of the length given.
        let n = unsafe { GetFinalPathNameByHandleW(HANDLE(file.as_raw_handle()), &mut buffer, flags) } as usize;
        match n {
            0 => return Err(io::Error::last_os_error()),
            n if n < buffer.len() => return Ok(PathBuf::from(OsString::from_wide(&buffer[..n]))),
            n => buffer.resize(n + 1, 0),
        }
    }
}

/// Whether `path` (a final path) is `root` or below it.
pub fn within(path: &Path, root: &Path) -> bool {
    let (p, r) = (path.to_string_lossy().to_lowercase(), root.to_string_lossy().to_lowercase());
    let r = r.trim_end_matches('\\');
    p.trim_end_matches('\\') == r || p.starts_with(&format!("{r}\\"))
}

/// The room this person may use on the disk the open folder is on.
pub fn free_space(dir: &File) -> Option<u64> {
    let path = final_path(dir).ok()?;
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.extend([b'\\' as u16, 0]);
    let mut free = 0u64;
    // SAFETY: a null-terminated path and a live out-value.
    unsafe { GetDiskFreeSpaceExW(windows::core::PCWSTR(wide.as_ptr()), Some(&mut free), None, None) }.ok()?;
    Some(free)
}

/// Whether the open folder is on a network disk, whatever its spelling (a mapped drive, a loopback share).
pub fn remote(dir: &File) -> bool {
    let mut info = FILE_REMOTE_PROTOCOL_INFO::default();
    // SAFETY: a live handle and a value of the size given; the call fails for a disk of this computer.
    unsafe {
        GetFileInformationByHandleEx(
            HANDLE(dir.as_raw_handle()),
            FileRemoteProtocolInfo,
            (&mut info as *mut FILE_REMOTE_PROTOCOL_INFO).cast(),
            std::mem::size_of::<FILE_REMOTE_PROTOCOL_INFO>() as u32,
        )
    }
    .is_ok()
}

/// One entry of a folder, as its listing says.
pub struct Listed {
    pub name: OsString,
    pub dir: bool,
    /// A link, junction or mounted folder (what a name there leads to is elsewhere).
    pub link: bool,
    /// Another reparse point (a file kept online, deduplicated): read like any other, never written through.
    pub special: bool,
    pub len: u64,
}

pub fn list(dir: &File) -> io::Result<Vec<Listed>> {
    let mut out = vec![];
    // Aligned for the records' 64-bit fields.
    let mut buffer = vec![0u64; 8192];
    let mut first = true;
    loop {
        let class = if first { FileIdBothDirectoryRestartInfo } else { FileIdBothDirectoryInfo };
        first = false;
        // SAFETY: a live directory handle and a buffer of the size given.
        let got = unsafe { GetFileInformationByHandleEx(HANDLE(dir.as_raw_handle()), class, buffer.as_mut_ptr().cast(), (buffer.len() * 8) as u32) };
        if let Err(e) = got {
            // ERROR_NO_MORE_FILES (or, on an empty root, ERROR_FILE_NOT_FOUND): the end.
            return match e.code().0 as u32 & 0xffff {
                18 | 2 => Ok(out),
                code => Err(io::Error::from_raw_os_error(code as i32)),
            };
        }
        let mut at = 0usize;
        loop {
            // SAFETY: the system filled the buffer with records linked by their offsets, each inside it.
            let entry = unsafe { &*(buffer.as_ptr().cast::<u8>().add(at).cast::<FILE_ID_BOTH_DIR_INFO>()) };
            let name = unsafe { std::slice::from_raw_parts(entry.FileName.as_ptr(), entry.FileNameLength as usize / 2) };
            let name = OsString::from_wide(name);
            if name != "." && name != ".." {
                let reparse = entry.FileAttributes & 0x400 != 0;
                // The reparse tag is where a reparse point's listing keeps it; a name surrogate stands for another place.
                let surrogate = reparse && entry.EaSize & 0x2000_0000 != 0;
                out.push(Listed {
                    name,
                    dir: entry.FileAttributes & 0x10 != 0,
                    link: surrogate,
                    special: reparse && !surrogate,
                    len: entry.EndOfFile as u64,
                });
            }
            if entry.NextEntryOffset == 0 {
                break;
            }
            at += entry.NextEntryOffset as usize;
        }
    }
}

/// Removes the file or (empty) folder `name` under `dir` when it is the very object `id` names and not a link: opened once,
/// looked at and deleted through that handle.
pub fn remove_at(dir: &File, name: &OsStr, id: FileId, is_dir: bool) -> io::Result<bool> {
    let file = open_at(dir, name, Want::Delete { dir: is_dir }, false)?;
    if is_link(&file)? || crate::owned::id_of_file(&file)? != id {
        return Ok(false);
    }
    delete(&file)?;
    Ok(true)
}

fn delete(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: a live handle and a value of the size given.
    unsafe {
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileDispositionInfo,
            (&info as *const FILE_DISPOSITION_INFO).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    }
    .map_err(|e| io::Error::from_raw_os_error((e.code().0 as u32 & 0xffff) as i32))
}

/// Takes away the file or link `name` itself under `dir` (never what a link leads to): for Orbiters' own staging folder.
pub fn unlink_at(dir: &File, name: &OsStr, is_dir: bool) -> io::Result<()> {
    delete(&open_at(dir, name, Want::Delete { dir: is_dir }, false)?)
}

/// Puts the open file `file` (made under `dir` by this process) at `name` under `dir`, in place of what is there.
pub fn publish_at(dir: &File, file: &File, _from: &OsStr, name: &OsStr) -> io::Result<()> {
    rename(dir, file, name, false)
}

/// [`publish_at`] in the place of the file `held` (which nobody else has open), still held while it is replaced, where the
/// disk can do that.
pub fn publish_held(dir: &File, file: &File, _from: &OsStr, name: &OsStr, held: Option<File>) -> io::Result<()> {
    match &held {
        Some(_) if rename(dir, file, name, true).is_ok() => Ok(()),
        _ => {
            drop(held);
            rename(dir, file, name, false)
        }
    }
}

/// `posix`: what is replaced may be open (by whoever shares it so).
fn rename(dir: &File, file: &File, name: &OsStr, posix: bool) -> io::Result<()> {
    let wide: Vec<u16> = name.encode_wide().collect();
    // FILE_RENAME_INFORMATION: replace (aligned), the root directory, the name's length in bytes, the name.
    let mut buffer = vec![0u64; 3 + wide.len() / 4 + 1];
    let bytes = buffer.as_mut_ptr().cast::<u8>();
    // SAFETY: the buffer is larger than the header and the name written into it.
    unsafe {
        // FILE_RENAME_INFORMATION_EX has flags where the other one has a byte: replace (1), POSIX (2).
        *bytes = if posix { 3 } else { 1 };
        *bytes.add(8).cast::<*mut c_void>() = dir.as_raw_handle();
        *bytes.add(16).cast::<u32>() = (wide.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(wide.as_ptr(), bytes.add(20).cast::<u16>(), wide.len());
    }
    let length = (20 + wide.len() * 2).max(24) as u32;
    let mut tries = 0;
    loop {
        let mut io = IoStatus { status: 0, information: 0 };
        // SAFETY: live handles; the buffer holds `length` bytes of the structure classes 10 and 65 expect.
        let status = unsafe { NtSetInformationFile(file.as_raw_handle(), &mut io, buffer.as_ptr().cast(), length, if posix { 65 } else { 10 }) };
        match check(status) {
            // Someone reads or replaces the same file at this instant.
            Err(_) if !posix && tries < 20 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(5 * tries));
            }
            other => return other,
        }
    }
}

pub fn set_executable_like(_to: &File, _from: &File) -> io::Result<()> {
    Ok(())
}
