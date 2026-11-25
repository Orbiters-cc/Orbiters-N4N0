//! Small private files sealed for this user of this computer (what was said to N4N0, the bridge's keys): Windows' DPAPI, on a Mac and
//! on Linux AES-256-GCM under the key of `dpapi` (a Mac's login keychain; on Linux a file only the person can read). On a phone, whose
//! apps' folders are closed to each other, they are written as they are.
//!
//! What was read, what there is none of and what could not be read are told apart: an unreadable file may be whole (the keychain locked,
//! another program holding it), so it is never "none", and the next write keeps it aside first ([`crate::keep_aside`]).

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// A way to seal: `crypt(bytes, entropy, seal or open)`, and what a sealed file starts with. Off Windows, versions before this header
/// wrote the file as it was.
struct Sealing {
    header: &'static [u8],
    crypt: fn(&[u8], &[u8], bool) -> anyhow::Result<Vec<u8>>,
}

#[cfg(windows)]
const HERE: Sealing = Sealing { header: b"", crypt: crate::dpapi::crypt };
#[cfg(any(target_os = "macos", target_os = "linux"))]
const HERE: Sealing = Sealing { header: HEADER, crypt: crate::dpapi::crypt };
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
const HERE: Sealing = Sealing { header: b"", crypt: |bytes, _, _| Ok(bytes.to_vec()) };

/// No JSON starts so: what these files were before it.
#[cfg(any(test, target_os = "macos", target_os = "linux"))]
const HEADER: &[u8] = b"Orbiters sealed 1\n"; // i18n: not shown (a file's first bytes)

/// What the sealed file `path` holds, `entropy` telling its use from others'. `Ok(None)`: there is none. `Err`: one is there and could
/// not be read, opened or understood. A file an earlier version wrote unsealed is read, and sealed in its place.
pub fn read<T: DeserializeOwned>(path: &Path, entropy: &[u8]) -> anyhow::Result<Option<T>> {
    HERE.read(path, entropy)
}

/// Writes `value` sealed, whole or not at all.
pub fn write<T: Serialize>(path: &Path, value: &T, entropy: &[u8]) -> anyhow::Result<()> {
    HERE.write(path, value, entropy)
}

/// Moves `path`, which could not be read, beside itself, out of the way of what is written next.
pub fn set_aside(path: &Path) -> std::io::Result<()> {
    crate::move_aside(path)?;
    crate::mark_unreadable(path, false);
    Ok(())
}

/// Removes `path` and what was kept aside of it when it could not be read.
pub fn remove(path: &Path) -> std::io::Result<()> {
    crate::mark_unreadable(path, false);
    std::iter::once(path.to_path_buf()).chain(asides(path)).fold(Ok(()), |done, file| match std::fs::remove_file(file) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => done,
    })
}

/// What was kept aside of `path`.
pub fn asides(path: &Path) -> Vec<PathBuf> {
    let Some((dir, name)) = path.parent().zip(path.file_name()) else { return Vec::new() };
    let name = format!("{}.bad-", name.to_string_lossy());
    std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&name))).collect()
}

impl Sealing {
    fn read<T: DeserializeOwned>(&self, path: &Path, entropy: &[u8]) -> anyhow::Result<Option<T>> {
        let opened = crate::read_retrying(path).map_err(anyhow::Error::from).and_then(|bytes| match bytes {
            Some(bytes) => Ok(Some(serde_json::from_slice(&self.open(path, bytes, entropy)?)?)),
            None => Ok(None),
        });
        crate::mark_unreadable(path, opened.is_err());
        opened
    }

    fn open(&self, path: &Path, bytes: Vec<u8>, entropy: &[u8]) -> anyhow::Result<Vec<u8>> {
        let Some(sealed) = bytes.strip_prefix(self.header) else {
            // As an earlier version wrote it: JSON. Anything else is damage, left as it is.
            serde_json::from_slice::<serde::de::IgnoredAny>(&bytes)?;
            if let Err(e) = self.seal_in_place(path, &bytes, entropy) {
                // Read all the same, and sealed at a next read.
                eprintln!("{} could not be sealed: {e:#}", path.display()); // i18n: not shown
            }
            return Ok(bytes);
        };
        (self.crypt)(sealed, entropy, false)
    }

    /// `plain`, as `path` holds it now, sealed there: only once the seal opens again, and while the file is still that.
    fn seal_in_place(&self, path: &Path, plain: &[u8], entropy: &[u8]) -> anyhow::Result<()> {
        let sealed = (self.crypt)(plain, entropy, true)?;
        anyhow::ensure!((self.crypt)(&sealed, entropy, false)? == plain && std::fs::read(path)? == plain, "not the same"); // i18n: not shown
        Ok(crate::write_atomic_private(path, &[self.header, &sealed].concat())?)
    }

    fn write<T: Serialize>(&self, path: &Path, value: &T, entropy: &[u8]) -> anyhow::Result<()> {
        let sealed = (self.crypt)(&serde_json::to_vec(value)?, entropy, true)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Ok(crate::write_atomic_private(path, &[self.header, &sealed].concat())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicBool, Ordering};

    static LOCKED: AtomicBool = AtomicBool::new(false);

    /// A seal as any platform's: its entropy opens it, and a locked keychain neither seals nor opens.
    fn toy(bytes: &[u8], entropy: &[u8], seal: bool) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(!LOCKED.load(Ordering::SeqCst), "locked");
        let mask: Vec<u8> = bytes.iter().map(|b| b ^ 0x5a).collect();
        if seal {
            return Ok([entropy, &mask].concat());
        }
        let sealed = bytes.strip_prefix(entropy).ok_or_else(|| anyhow::anyhow!("another use's"))?;
        Ok(sealed.iter().map(|b| b ^ 0x5a).collect())
    }

    /// XCT-20: off Windows these files were written as they were. They are read once so and sealed, and nothing of them is lost while
    /// sealing cannot be done, nor for a file that cannot be opened.
    #[test]
    fn a_file_written_before_it_was_sealed_is_read_then_sealed() {
        let s = Sealing { header: HEADER, crypt: toy };
        let dir = std::env::temp_dir().join(format!("orbiters-sealed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("kept.dat");
        let read = |entropy: &[u8]| s.read::<Value>(&file, entropy);
        assert!(read(b"e").unwrap().is_none());
        let plain = br#"{"said":"hello"}"#;
        std::fs::write(&file, plain).unwrap();
        // Not while it cannot be sealed: read, and left as it was.
        LOCKED.store(true, Ordering::SeqCst);
        assert_eq!(read(b"e").unwrap().unwrap(), json!({ "said": "hello" }));
        assert_eq!(std::fs::read(&file).unwrap(), plain);
        LOCKED.store(false, Ordering::SeqCst);
        for _ in 0..2 {
            assert_eq!(read(b"e").unwrap().unwrap(), json!({ "said": "hello" }));
            let on_disk = std::fs::read(&file).unwrap();
            assert!(on_disk.starts_with(HEADER) && !on_disk.windows(5).any(|w| w == b"hello"), "sealed");
        }
        assert!(read(b"other").is_err(), "another use's");
        // It cannot be opened now: not "none", nothing of it goes, and a write keeps it aside.
        let sealed = std::fs::read(&file).unwrap();
        LOCKED.store(true, Ordering::SeqCst);
        assert!(read(b"e").is_err() && s.write(&file, &json!({}), b"e").is_err());
        assert_eq!(std::fs::read(&file).unwrap(), sealed);
        LOCKED.store(false, Ordering::SeqCst);
        assert!(crate::was_unreadable(&file));
        s.write(&file, &json!({}), b"e").unwrap();
        assert_eq!(asides(&file).iter().map(|f| std::fs::read(f).unwrap()).collect::<Vec<_>>(), [sealed]);
        // Damage, neither sealed nor JSON, is left alone.
        std::fs::write(&file, b"\0\0\0").unwrap();
        assert!(read(b"e").is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"\0\0\0");
        remove(&file).unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "nothing of it is left");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// This computer's own way.
    #[test]
    fn what_is_written_here_opens_here() {
        let dir = std::env::temp_dir().join(format!("orbiters-sealed-here-{}", std::process::id()));
        let file = dir.join("kept.dat");
        write(&file, &json!({ "said": "hello" }), b"e").unwrap();
        assert!(!std::fs::read(&file).unwrap().windows(5).any(|w| w == b"hello"));
        assert_eq!(read::<Value>(&file, b"e").unwrap().unwrap(), json!({ "said": "hello" }));
        assert!(read::<Value>(&file, b"other").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
