//! Telling a file or folder apart from whatever later takes its name: its identity on its disk (the volume and file
//! index on Windows, the device and inode elsewhere), read without following a link or junction. What deletes data
//! (a stopped copy, the startup sweep) acts only on what it can prove it made, by this identity, never by path alone.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Where a file or folder is on its disk: equal for the same object under any name, different for a substitute.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileId {
    pub volume: u64,
    pub index: u64,
}

/// Whether `meta` (from `symlink_metadata`) is a link, a junction or another reparse point.
pub fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_type().is_symlink() || meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

/// The identity of what is at `path` itself (a link's own, never its target's).
pub fn id_of(path: &Path) -> std::io::Result<FileId> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_READ_ATTRIBUTES: u32 = 0x80;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(7)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        id_of_file(&file)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::symlink_metadata(path)?;
        Ok(FileId { volume: m.dev(), index: m.ino() })
    }
}

/// The identity of an open file.
pub fn id_of_file(file: &std::fs::File) -> std::io::Result<FileId> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }.map_err(std::io::Error::other)?;
        Ok(FileId { volume: info.dwVolumeSerialNumber as u64, index: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64 })
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = file.metadata()?;
        Ok(FileId { volume: m.dev(), index: m.ino() })
    }
}

/// Whether `path` is still the very object `id` names, and not a link.
pub fn is_same(path: &Path, id: FileId) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| !is_link(&m)) && id_of(path).is_ok_and(|now| now == id)
}

/// A folder held open so that, while held, nobody can rename, move or delete it, nor any folder above it (Windows: the
/// handle does not share deletion; a folder holding an open entry cannot be renamed). What is made in it is made in
/// this very folder, and it is removed through the handle itself, never by its name again. Elsewhere (macOS) the name
/// is checked against the identity at each use instead.
pub struct Pinned {
    file: std::fs::File,
    pub id: FileId,
    #[cfg(not(windows))]
    path: std::path::PathBuf,
}
impl Pinned {
    /// `path`, when it is a real folder (not a link), held.
    pub fn open(path: &Path) -> std::io::Result<Pinned> {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const DELETE: u32 = 0x0001_0000;
            const FILE_READ_ATTRIBUTES: u32 = 0x80;
            const FILE_SHARE_READ_WRITE: u32 = 0x1 | 0x2;
            const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            let file = std::fs::OpenOptions::new()
                .access_mode(DELETE | FILE_READ_ATTRIBUTES)
                .share_mode(FILE_SHARE_READ_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path)?;
            let meta = file.metadata()?;
            if !meta.is_dir() || is_link(&meta) {
                return Err(std::io::Error::other("not a real folder")); // i18n: not shown (an error chained under a message)
            }
            let id = id_of_file(&file)?;
            Ok(Pinned { file, id })
        }
        #[cfg(not(windows))]
        {
            let meta = std::fs::symlink_metadata(path)?;
            if !meta.is_dir() || is_link(&meta) {
                return Err(std::io::Error::other("not a real folder")); // i18n: not shown (an error chained under a message)
            }
            let file = std::fs::File::open(path)?;
            let id = id_of_file(&file)?;
            Ok(Pinned { file, id, path: path.to_path_buf() })
        }
    }
    /// Removes the folder held (only when empty), through the handle.
    pub fn delete(self) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            delete_by_handle(&self.file)
        }
        #[cfg(not(windows))]
        {
            if !is_same(&self.path, self.id) {
                return Err(std::io::Error::other("the folder was replaced")); // i18n: not shown (an error chained under a message)
            }
            let _ = &self.file;
            std::fs::remove_dir(&self.path)
        }
    }
}

#[cfg(windows)]
fn delete_by_handle(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO};
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    unsafe {
        SetFileInformationByHandle(HANDLE(file.as_raw_handle()), FileDispositionInfo, (&info as *const FILE_DISPOSITION_INFO).cast(), std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32)
    }
    .map_err(std::io::Error::other)
}

/// Removes the file at `path` when it is the very file `id` names (not a link, not a file put in its place): opened
/// once, its identity read from that handle, deleted through it. False when it is another (left as it is).
pub fn delete_file_if(path: &Path, id: FileId) -> std::io::Result<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const DELETE: u32 = 0x0001_0000;
        const FILE_READ_ATTRIBUTES: u32 = 0x80;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        let file = std::fs::OpenOptions::new().access_mode(DELETE | FILE_READ_ATTRIBUTES).share_mode(7).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(path)?;
        let meta = file.metadata()?;
        if meta.is_dir() || is_link(&meta) || id_of_file(&file)? != id {
            return Ok(false);
        }
        delete_by_handle(&file)?;
        Ok(true)
    }
    #[cfg(not(windows))]
    {
        if !is_same(path, id) || std::fs::symlink_metadata(path)?.is_dir() {
            return Ok(false);
        }
        std::fs::remove_file(path)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_follows_the_object_not_its_name() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        std::fs::create_dir(&a).unwrap();
        let id = id_of(&a).unwrap();
        assert!(is_same(&a, id));
        // Renamed: the same object. Replaced by another folder of the same name: not.
        let b = temp.path().join("b");
        std::fs::rename(&a, &b).unwrap();
        assert!(is_same(&b, id));
        std::fs::create_dir(&a).unwrap();
        assert!(!is_same(&a, id));
        let file = std::fs::File::create(temp.path().join("f")).unwrap();
        assert_eq!(id_of_file(&file).unwrap(), id_of(&temp.path().join("f")).unwrap());
    }

    /// A pinned folder can be neither renamed nor replaced while held (Windows), and is removed through its handle; a file
    /// put in place of another is not deleted for it.
    #[test]
    fn a_pinned_folder_stays_itself_and_only_the_very_file_goes() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        std::fs::create_dir(&a).unwrap();
        let pinned = Pinned::open(&a).unwrap();
        #[cfg(windows)]
        assert!(std::fs::rename(&a, temp.path().join("b")).is_err(), "a held folder cannot be renamed");
        std::fs::write(a.join("f"), "mine").unwrap();
        let id = id_of(&a.join("f")).unwrap();
        std::fs::remove_file(a.join("f")).unwrap();
        std::fs::write(a.join("f"), "theirs").unwrap();
        assert!(!delete_file_if(&a.join("f"), id).unwrap(), "another file under that name");
        assert_eq!(std::fs::read_to_string(a.join("f")).unwrap(), "theirs");
        let id = id_of(&a.join("f")).unwrap();
        assert!(delete_file_if(&a.join("f"), id).unwrap());
        pinned.delete().unwrap();
        assert!(!a.exists());
    }
}
