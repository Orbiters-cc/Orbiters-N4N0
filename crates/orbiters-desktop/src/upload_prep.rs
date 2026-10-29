//! Reusable upload preparation for public previews and private captures. One atomic, checksummed cache beside the
//! reconstruction holds all four files and their metadata. Inputs are content-hashed, including cameras and the
//! report that chooses the start view. A retry still gets a new tree key so VRChat can detect a replaced slot.

use orbiters_core::{contracts::manifest, maps::PreviewFiles, reconstruction, visited::sha256_file};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

// Change the format/policy marker whenever preparation changes its outputs or tuning policy.
const MAGIC: &[u8; 8] = b"N4PREP01";
const FILE: &str = "upload-prep-v1.bin";
const MAX_BYTES: u64 = 96 << 20;
pub static PREPARING: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fingerprint(Vec<Option<String>>);

impl Fingerprint {
    pub fn read(session: &Path, result: &Path) -> anyhow::Result<Self> {
        let paths = [
            result.join(reconstruction::PLY_FILE),
            session.join(manifest::FRAMES_FILE),
            result.join(reconstruction::REPORT_FILE),
            session.join(manifest::MANIFEST_FILE),
        ];
        let hashes = paths
            .iter()
            .enumerate()
            .map(|(i, path)| match sha256_file(path) {
                Ok(hash) => Ok(Some(hash)),
                Err(e) if i >= 2 && e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            })
            .collect::<Result<_, _>>()?;
        Ok(Self(hashes))
    }
}

#[derive(Serialize, Deserialize)]
struct Header {
    fingerprint: Fingerprint,
    lengths: [usize; 4],
    metadata: serde_json::Value,
}

pub fn tree_key() -> u32 {
    // Random per upload rather than per cached reconstruction; all three files receive this same key.
    u32::from_le_bytes(uuid::Uuid::new_v4().as_bytes()[..4].try_into().unwrap())
}

fn rekey(files: &mut PreviewFiles, key: u32) -> Option<()> {
    for bytes in [&mut files.vrchat_pc, &mut files.vrchat_quest, &mut files.vrchat_coarse] {
        if bytes.get(..8)? != orbiters_reconstruct::tree::MAGIC {
            return None;
        }
        bytes.get_mut(20..24)?.copy_from_slice(&key.to_le_bytes());
    }
    Some(())
}

fn decode(bytes: &[u8], fingerprint: &Fingerprint, key: u32) -> Option<(PreviewFiles, serde_json::Value)> {
    if bytes.get(..8)? != MAGIC || bytes.len() as u64 > MAX_BYTES {
        return None;
    }
    let payload = bytes.get(40..)?;
    if &Sha256::digest(payload)[..] != bytes.get(8..40)? {
        return None;
    }
    let length = u32::from_le_bytes(payload.get(..4)?.try_into().ok()?) as usize;
    if length > 16_384 {
        return None;
    }
    let header: Header = serde_json::from_slice(payload.get(4..4 + length)?).ok()?;
    if &header.fingerprint != fingerprint {
        return None;
    }
    let mut rest = payload.get(4 + length..)?;
    let mut next = |n: usize| {
        let file = rest.get(..n)?.to_vec();
        rest = rest.get(n..)?;
        Some(file)
    };
    let [web, pc, quest, coarse] = header.lengths;
    let mut files = PreviewFiles { splat: next(web)?, vrchat_pc: next(pc)?, vrchat_quest: next(quest)?, vrchat_coarse: next(coarse)? };
    if !rest.is_empty() {
        return None;
    }
    rekey(&mut files, key)?;
    Some((files, header.metadata))
}

pub fn load(result: &Path, fingerprint: &Fingerprint, key: u32) -> Option<(PreviewFiles, serde_json::Value)> {
    use std::io::Read;
    let file = std::fs::File::open(result.join(FILE)).ok()?;
    if file.metadata().ok()?.len() > MAX_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).ok()?;
    decode(&bytes, fingerprint, key)
}

pub fn save(result: &Path, fingerprint: &Fingerprint, files: &PreviewFiles, metadata: &serde_json::Value) -> anyhow::Result<()> {
    let parts = [&files.splat, &files.vrchat_pc, &files.vrchat_quest, &files.vrchat_coarse];
    let header = serde_json::to_vec(&Header { fingerprint: fingerprint.clone(), lengths: parts.map(|p| p.len()), metadata: metadata.clone() })?;
    let mut bytes = Vec::with_capacity(44 + header.len() + files.len() as usize);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&[0; 32]);
    bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&header);
    for part in parts {
        bytes.extend_from_slice(part);
    }
    anyhow::ensure!(bytes.len() as u64 <= MAX_BYTES, "upload preparation exceeds the cache limit");
    let digest = Sha256::digest(&bytes[40..]);
    bytes[8..40].copy_from_slice(&digest);
    orbiters_core::write_atomic(&result.join(FILE), &bytes)?;
    Ok(())
}

pub fn sky(result: &Path) -> Option<PathBuf> {
    Some(result.join(reconstruction::SKY_FILE)).filter(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_files_are_exact_and_all_trees_get_the_new_upload_key() {
        let dir = std::env::temp_dir().join(format!("orbiters-prep-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut g = orbiters_reconstruct::gaussians::Gaussians::default();
        for i in 0..1024 {
            g.push([i as f32 * 0.01, 0.0, 1.0], [-3.0; 3], [1.0, 0.0, 0.0, 0.0], 1.0, [0.5; 3]);
        }
        let trees = orbiters_reconstruct::tree_tune::vrchat_files(&g, &[], None, 7, |_| {}).unwrap();
        let files = PreviewFiles { splat: vec![1, 2, 3], vrchat_pc: trees.pc, vrchat_quest: trees.quest, vrchat_coarse: trees.coarse };
        let fingerprint = Fingerprint(vec![Some("fixture".into())]);
        let metadata = serde_json::json!({"quality":33.0,"view":{"position":[1,2,3]}});
        save(&dir, &fingerprint, &files, &metadata).unwrap();
        let (loaded, found) = load(&dir, &fingerprint, 9).unwrap();
        assert_eq!(found, metadata);
        assert_eq!(loaded.splat, files.splat);
        for (a, b) in [(&loaded.vrchat_pc, &files.vrchat_pc), (&loaded.vrchat_quest, &files.vrchat_quest), (&loaded.vrchat_coarse, &files.vrchat_coarse)] {
            assert_eq!(&a[..20], &b[..20]);
            assert_eq!(&a[20..24], &9u32.to_le_bytes());
            assert_eq!(&a[24..], &b[24..]);
        }
        assert!(load(&dir, &Fingerprint(vec![]), 9).is_none());
        let mut bytes = std::fs::read(dir.join(FILE)).unwrap();
        bytes[50] ^= 1;
        assert!(decode(&bytes, &fingerprint, 9).is_none());
        assert!(decode(&bytes[..30], &fingerprint, 9).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_source_is_content_hashed_even_when_the_length_stays_the_same() {
        let dir = std::env::temp_dir().join(format!("orbiters-prep-{}", uuid::Uuid::new_v4()));
        let result = dir.join("reconstruction");
        std::fs::create_dir_all(&result).unwrap();
        let sources = [
            result.join(reconstruction::PLY_FILE),
            dir.join(manifest::FRAMES_FILE),
            result.join(reconstruction::REPORT_FILE),
            dir.join(manifest::MANIFEST_FILE),
        ];
        for path in &sources {
            std::fs::write(path, b"old").unwrap();
        }
        let original = Fingerprint::read(&dir, &result).unwrap();
        for path in &sources {
            std::fs::write(path, b"new").unwrap();
            assert_ne!(Fingerprint::read(&dir, &result).unwrap(), original);
            std::fs::write(path, b"old").unwrap();
        }
        std::fs::remove_file(&sources[2]).unwrap();
        assert_ne!(Fingerprint::read(&dir, &result).unwrap(), original);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
