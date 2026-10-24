//! The programs inside the setup: a zstd-compressed tar (with zstd's own checksum) whose first entry, [`INDEX`], lists
//! every other file with its size and SHA-256. The setup unpacks it into a folder of its own and checks each file
//! against the index as it goes: a payload that does not match its index installs nothing.

use crate::{hex, inside, PRODUCT};
use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// The index's name, first in the archive.
pub const INDEX: &str = "payload.json";
/// zstd's level for releases: slow to pack once, as fast as any to unpack.
const LEVEL: i32 = 19;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    /// Always [`PRODUCT`].
    pub product: String,
    pub version: String,
    pub files: Vec<Entry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// In the install folder, `/` between folders.
    pub path: String,
    pub size: u64,
    /// Lowercase hexadecimal.
    pub sha256: String,
}

impl Index {
    /// Bytes of all the files.
    pub fn total(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// Packs `files` (each a path in the install folder and the file on this computer) into `out` as `version`.
pub fn pack(version: &str, files: &[(String, PathBuf)], out: &Path) -> anyhow::Result<Index> {
    let mut entries = Vec::with_capacity(files.len());
    let mut seen = HashSet::new();
    for (path, source) in files {
        anyhow::ensure!(inside(Path::new("."), path).is_some() && path != INDEX && !path.contains('\\'), "{path}: not a path for the install folder");
        anyhow::ensure!(seen.insert(path.to_ascii_lowercase()), "{path} is in the payload twice");
        let size = std::fs::metadata(source).with_context(|| format!("{}", source.display()))?.len();
        entries.push(Entry { path: path.clone(), size, sha256: crate::sha256_file(source)? });
    }
    let index = Index { product: PRODUCT.into(), version: version.into(), files: entries };

    let file = std::fs::File::create(out).with_context(|| format!("{}", out.display()))?;
    let mut encoder = zstd::Encoder::new(BufWriter::with_capacity(1 << 20, file), LEVEL)?;
    encoder.include_checksum(true)?;
    let mut tar = tar::Builder::new(encoder);
    let json = serde_json::to_vec_pretty(&index)?;
    tar.append_data(&mut header(json.len() as u64), INDEX, json.as_slice())?;
    for ((path, source), entry) in files.iter().zip(&index.files) {
        let file = std::fs::File::open(source)?;
        anyhow::ensure!(file.metadata()?.len() == entry.size, "{} changed while it was packed", source.display());
        tar.append_data(&mut header(entry.size), path, file)?;
    }
    let writer = tar.into_inner()?.finish()?;
    writer.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    Ok(index)
}

/// A tar header of a plain file without the packing computer's details (owner, time).
fn header(size: u64) -> tar::Header {
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Regular);
    h.set_size(size);
    h.set_mode(0o755);
    h.set_mtime(0);
    h
}

/// The index of `payload`, without unpacking the rest.
pub fn read_index(payload: &[u8]) -> anyhow::Result<Index> {
    let mut archive = tar::Archive::new(zstd::Decoder::with_buffer(payload)?);
    let mut entries = archive.entries()?;
    let first = entries.next().context("the installer's payload is empty")??;
    index_of(first)
}

fn index_of(entry: tar::Entry<'_, impl Read>) -> anyhow::Result<Index> {
    anyhow::ensure!(entry.path()?.as_ref() == Path::new(INDEX), "the installer's payload does not start with its index");
    let index: Index = serde_json::from_reader(entry.take(4 << 20)).context("the installer's payload index cannot be read")?;
    anyhow::ensure!(index.product == PRODUCT, "the installer holds {}, not Orbiters - N4N0", index.product);
    Ok(index)
}

/// Unpacks `payload` into the folder `into` (made if missing), checking every file against the index. `progress`
/// hears the bytes written so far, of the total, and the file being written.
pub fn unpack(payload: &[u8], into: &Path, mut progress: impl FnMut(u64, u64, &str)) -> anyhow::Result<Index> {
    std::fs::create_dir_all(into)?;
    let mut archive = tar::Archive::new(zstd::Decoder::with_buffer(payload)?);
    let mut entries = archive.entries()?;
    let index = index_of(entries.next().context("the installer's payload is empty")??)?;
    let total = index.total();
    let mut done = 0u64;
    let mut seen = HashSet::new();
    let mut buf = vec![0u8; 256 * 1024];
    for entry in entries {
        let mut entry = entry?;
        anyhow::ensure!(entry.header().entry_type().is_file(), "the installer's payload holds something that is not a file");
        let path = entry.path()?.to_string_lossy().replace('\\', "/");
        let expected = index.files.iter().find(|f| f.path == path).with_context(|| format!("{path} is in the installer but not in its index"))?;
        anyhow::ensure!(seen.insert(path.clone()), "{path} is in the installer twice");
        let dest = inside(into, &path).with_context(|| format!("{path} would land outside the install folder"))?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = BufWriter::with_capacity(1 << 20, std::fs::File::create(&dest).with_context(|| format!("{}", dest.display()))?);
        let mut hasher = Sha256::new();
        let mut written = 0u64;
        loop {
            let n = entry.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            written += n as u64;
            done += n as u64;
            progress(done, total, &path);
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        anyhow::ensure!(written == expected.size && hex(&hasher.finalize()) == expected.sha256, "{path} is damaged in the installer");
    }
    anyhow::ensure!(seen.len() == index.files.len(), "the installer is missing files of its index");
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-payload-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample(dir: &Path) -> Vec<(String, PathBuf)> {
        let a = dir.join("a.exe");
        let b = dir.join("notes.md");
        std::fs::write(&a, (0..300_000u32).flat_map(|i| i.to_le_bytes()).collect::<Vec<_>>()).unwrap();
        std::fs::write(&b, b"hello").unwrap();
        vec![("orbiters-desktop.exe".into(), a), ("licenses/notes.md".into(), b)]
    }

    #[test]
    fn a_payload_unpacks_to_what_was_packed() {
        let dir = scratch("roundtrip");
        let files = sample(&dir);
        let out = dir.join("payload.tar.zst");
        let index = pack("1.2.3", &files, &out).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(read_index(&bytes).unwrap(), index);

        let into = dir.join("unpacked");
        let mut last = (0, 0);
        let unpacked = unpack(&bytes, &into, |done, total, _| last = (done, total)).unwrap();
        assert_eq!(unpacked, index);
        assert_eq!(last, (index.total(), index.total()));
        assert_eq!(std::fs::read(into.join("orbiters-desktop.exe")).unwrap(), std::fs::read(&files[0].1).unwrap());
        assert_eq!(std::fs::read(into.join("licenses/notes.md")).unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_payload_is_refused() {
        let dir = scratch("damaged");
        let files = sample(&dir);
        let out = dir.join("payload.tar.zst");
        pack("1.2.3", &files, &out).unwrap();
        let mut bytes = std::fs::read(&out).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0x40;
        assert!(unpack(&bytes, &dir.join("unpacked"), |_, _, _| {}).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paths_outside_the_folder_are_not_packed() {
        let dir = scratch("outside");
        let source = dir.join("x");
        std::fs::write(&source, b"x").unwrap();
        for bad in ["../x", "/x", "a\\b", INDEX] {
            assert!(pack("1.0.0", &[(bad.into(), source.clone())], &dir.join("p")).is_err(), "{bad}");
        }
        assert!(pack("1.0.0", &[("x".into(), source.clone()), ("X".into(), source)], &dir.join("p")).is_err(), "twice");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
