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
}
