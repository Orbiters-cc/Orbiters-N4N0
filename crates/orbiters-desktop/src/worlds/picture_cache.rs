//! Small decoded pictures kept on the system disk. Originals are never changed.
use super::*;

const LIMIT: u64 = 512 << 20;
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub(super) struct Stamp { size: u64, modified: u128 }
fn indexed() -> &'static Mutex<HashMap<PathBuf, Stamp>> {
    static INDEX: std::sync::OnceLock<Mutex<HashMap<PathBuf, Stamp>>> = std::sync::OnceLock::new();
    INDEX.get_or_init(Mutex::default)
}
pub(super) fn set_index(index: &visited::ShotIndex) {
    *indexed().lock().unwrap_or_else(|e| e.into_inner()) = index.fingerprints()
        .map(|(p, size, modified)| (p.to_path_buf(), Stamp { size, modified: u128::from(modified) })).collect();
}

pub(super) fn stamp(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    Some(Stamp { size: m.len(), modified: m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos() })
}
fn files(p: &Picture) -> (PathBuf, PathBuf) {
    let file = cache_dir().join("thumbs-v2").join(format!("{:016x}.qoi", key(p)));
    let meta = file.with_extension("json");
    (file, meta)
}
pub(super) fn read(p: &Picture, path: &Path, width: u32) -> Option<(qoi::QoiHeader, Vec<u8>)> {
    if width > THUMB_WIDTH { return None; }
    let (file, meta) = files(p);
    read_cached(&file, &meta, path)
}
fn read_cached(file: &Path, meta: &Path, path: &Path) -> Option<(qoi::QoiHeader, Vec<u8>)> {
    let saved: Stamp = serde_json::from_slice(&std::fs::read(meta).ok()?).ok()?;
    // An unplugged drive keeps its last good preview; an online changed file is decoded again.
    let known = indexed().lock().unwrap_or_else(|e| e.into_inner()).get(path).cloned();
    let stale = match known {
        Some(now) => now != saved,
        None => stamp(path).is_some_and(|now| now != saved),
    };
    if stale { return None; }
    qoi::decode_rgba_max(&std::fs::read(file).ok()?, u64::from(THUMB_WIDTH).pow(2) * 4).ok()
}
pub(super) fn write(p: &Picture, path: &Path, width: u32, before: Option<Stamp>, bytes: &[u8]) {
    if width > THUMB_WIDTH { return; }
    let Some(stamp) = stamp(path) else { return };
    if before.as_ref() != Some(&stamp) { return; }
    let (file, meta) = files(p);
    let Some(dir) = file.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() { return; }
    // Cache writes are serialized, leaving at least 600 MiB for the rest of the application.
    static LOCK: Mutex<()> = Mutex::new(());
    let _one = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if orbiters_projects::size::free_space(dir).is_none_or(|n| n < (600 << 20) + bytes.len() as u64) { return; }
    static TOTAL: std::sync::OnceLock<Mutex<u64>> = std::sync::OnceLock::new();
    let total = TOTAL.get_or_init(|| Mutex::new(std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()));
    let mut total = total.lock().unwrap_or_else(|e| e.into_inner());
    let cost = bytes.len() as u64 + 128;
    if *total + cost > LIMIT {
        let mut files: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten()
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path(), e.metadata().ok()?.len()))).collect();
        files.sort_by_key(|e| e.0);
        for (_, old, len) in files {
            if *total + cost <= LIMIT * 3 / 4 { break; }
            if std::fs::remove_file(old).is_ok() { *total = total.saturating_sub(len); }
        }
    }
    if *total + cost > LIMIT { return; }
    if orbiters_core::write_atomic(&file, bytes).is_ok() {
        let _ = orbiters_core::write_atomic(&meta, &serde_json::to_vec(&stamp).unwrap_or_default());
        *total += cost;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_preview_invalidates_changes_and_survives_offline_source() {
        let root = std::env::temp_dir().join(format!("orbiters-thumb-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.png");
        let file = root.join("preview.qoi");
        let meta = root.join("preview.json");
        std::fs::write(&source, b"original").unwrap();
        std::fs::write(&file, qoi::encode(&[20, 30, 40], 1, 1, 3)).unwrap();
        std::fs::write(&meta, serde_json::to_vec(&stamp(&source).unwrap()).unwrap()).unwrap();
        assert!(read_cached(&file, &meta, &source).is_some());
        let modified = std::fs::metadata(&source).unwrap().modified().unwrap();
        std::fs::write(&source, b"modified").unwrap();
        std::fs::File::options().write(true).open(&source).unwrap().set_times(
            std::fs::FileTimes::new().set_modified(modified + std::time::Duration::from_secs(1))).unwrap();
        assert!(read_cached(&file, &meta, &source).is_none());
        std::fs::remove_file(&source).unwrap();
        assert!(read_cached(&file, &meta, &source).is_some());
        std::fs::write(&file, b"broken").unwrap();
        assert!(read_cached(&file, &meta, &source).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_identity_includes_path_size_and_width() {
        let a = Picture::Shot { path: "a.png".into(), size: 10, width: 560 };
        for b in [Picture::Shot { path: "b.png".into(), size: 10, width: 560 },
            Picture::Shot { path: "a.png".into(), size: 11, width: 560 },
            Picture::Shot { path: "a.png".into(), size: 10, width: 480 }] { assert_ne!(files(&a), files(&b)); }
        assert_ne!(Stamp { size: 10, modified: 1 }, Stamp { size: 10, modified: 2 });
    }
}
