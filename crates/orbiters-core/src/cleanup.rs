//! Freeing space in the session library. A session folder holds its capture (the lossless keyframes, the depth frames,
//! a video, the performance streams: sound, avatar parameters, tracking, the skeleton band), which only reconstructing
//! it again reads, and what its 3D view reads: the manifest, the frame records (the recorded cameras a view starts
//! from), the thumbnail and the reconstruction (`reconstruction/`: splats, sky map, report, preview, held-out renders).
//! "Keep only the 3D view" ([`keep_only_view`]) deletes the first and keeps the second; a session without a
//! reconstruction has no 3D view to keep and goes whole when asked ([`delete_session`]). Both delete for good: moved to
//! the recycle bin, the files would free nothing.
//!
//! A session whose capture files went has no `keyframes/` folder any more ([`trimmed`]): it is viewed, sent to Mixer and
//! shared on the Map explorer like any reconstructed session, and is not reconstructed again. Files the capture did not
//! write (something the person put in the folder) are left where they are.

use crate::contracts::manifest::{DEPTH_DIR, KEYFRAMES_DIR, MANIFEST_FILE, RAW_DEPTH_DIR};
use crate::contracts::performance as perf;
use crate::reconstruction;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// What a capture file is, as a confirmation lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    /// `keyframes/`: the lossless keyframes training reads.
    Keyframes,
    /// `depth/`, and the few depth frames kept as received (`depth-raw/`).
    Depth,
    /// The video of the other frames (the manifest's `capture.video`).
    Video,
    /// Sound, avatar parameters, tracking, the T-pose calibration and the skeleton band (contracts `Docs/performance.md`).
    Performance,
    /// Reconstructions that are not the result: a run cut short (`reconstruction.partial/`), and the previous result
    /// while the latest is in place (`reconstruction.old/`).
    Leftovers,
}

/// The performance streams of a session (contracts `Docs/performance.md`).
const PERFORMANCE: [&str; 8] = [
    perf::PARAMETERS_FILE,
    perf::PARAMETERS_TABLE,
    perf::TRACKING_FILE,
    perf::TRACKING_DEVICES,
    perf::CALIBRATION_FILE,
    perf::AUDIO_FILE,
    perf::AUDIO_DIR,
    perf::SKELETON_FILE,
];

/// The part of the capture `name` (an entry of a session folder) is, or `None` for what the 3D view needs and for what
/// the capture did not write. `video` is the manifest's `capture.video`, taken only as a file of the folder itself;
/// `result_in_place`: the latest result is in `reconstruction/` (else `reconstruction.old/` holds it, and stays).
pub fn capture_part(name: &str, video: Option<&str>, result_in_place: bool) -> Option<Part> {
    match name {
        KEYFRAMES_DIR => Some(Part::Keyframes),
        DEPTH_DIR | RAW_DEPTH_DIR => Some(Part::Depth),
        reconstruction::PARTIAL_DIR => Some(Part::Leftovers),
        reconstruction::OLD_DIR if result_in_place => Some(Part::Leftovers),
        _ if PERFORMANCE.contains(&name) => Some(Part::Performance),
        _ if video.is_some_and(|v| plain_name(v) == Some(name)) => Some(Part::Video),
        _ => None,
    }
}

/// `path` when it names a file right inside the folder (no separator, no `..`, not absolute).
fn plain_name(path: &str) -> Option<&str> {
    let mut parts = Path::new(path).components();
    match (parts.next(), parts.next()) {
        (Some(Component::Normal(name)), None) => name.to_str(),
        _ => None,
    }
}

/// What a session folder holds: everything in it, and the part [`capture_part`] names (what [`keep_only_view`] frees).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sizes {
    pub total: u64,
    pub capture: u64,
}

/// Measures a session folder, every file of it, sorting its entries by [`capture_part`]. Links are counted as links,
/// never followed.
pub fn measure(dir: &Path, video: Option<&str>) -> Sizes {
    let in_place = reconstruction::dir(dir).is_dir();
    let mut sizes = Sizes::default();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let bytes = entry_size(&e);
        sizes.total += bytes;
        if capture_part(&e.file_name().to_string_lossy(), video, in_place).is_some() {
            sizes.capture += bytes;
        }
    }
    sizes
}

fn entry_size(e: &std::fs::DirEntry) -> u64 {
    match e.metadata() {
        Ok(m) if m.is_dir() => crate::bundle::folder_size(&e.path()),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

/// A capture file (or folder) of a session, with what it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub part: Part,
    pub bytes: u64,
    /// Files in it (1 for a file).
    pub files: u64,
}

/// The capture files of a session folder, what [`keep_only_view`] deletes (the manifest names the video).
pub fn capture_files(dir: &Path) -> Vec<Entry> {
    let video = crate::bundle::read_manifest(dir).ok().and_then(|m| m.capture.video);
    let in_place = reconstruction::dir(dir).is_dir();
    let mut entries: Vec<Entry> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let part = capture_part(&e.file_name().to_string_lossy(), video.as_deref(), in_place)?;
            let (bytes, files) = match e.metadata() {
                Ok(m) if m.is_dir() => count(&e.path()),
                Ok(m) => (m.len(), 1),
                Err(_) => (0, 0),
            };
            Some(Entry { path: e.path(), part, bytes, files })
        })
        .collect();
    entries.sort_by(|a, b| a.part.cmp(&b.part).then_with(|| a.path.cmp(&b.path)));
    entries
}

/// Bytes and files under a folder (links not followed).
fn count(dir: &Path) -> (u64, u64) {
    let (mut bytes, mut files) = (0, 0);
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        match e.metadata() {
            Ok(m) if m.is_dir() => {
                let (b, f) = count(&e.path());
                bytes += b;
                files += f;
            }
            Ok(m) => {
                bytes += m.len();
                files += 1;
            }
            Err(_) => {}
        }
    }
    (bytes, files)
}

/// Whether a session has a 3D view to keep: a finished reconstruction's splats and report.
pub fn has_view(dir: &Path) -> bool {
    let result = reconstruction::result_dir(dir);
    result.join(reconstruction::PLY_FILE).is_file() && result.join(reconstruction::REPORT_FILE).is_file()
}

/// Whether a session's capture files were deleted to keep only its 3D view: its keyframes are gone (every capture makes
/// the folder when it starts).
pub fn trimmed(dir: &Path) -> bool {
    !dir.join(KEYFRAMES_DIR).is_dir()
}

/// Deletes a session's capture files for good ([`capture_files`]), keeping its 3D view; `freed` hears the bytes of what
/// goes as it goes. Refused for a session without a finished reconstruction: nothing would be left to see. Returns
/// the bytes freed; on an error, what went before it stays gone.
pub fn keep_only_view(dir: &Path, freed: &mut dyn FnMut(u64)) -> anyhow::Result<u64> {
    anyhow::ensure!(dir.join(MANIFEST_FILE).is_file(), "{} is not a session folder.", dir.display());
    anyhow::ensure!(has_view(dir), "This session has no 3D view to keep: reconstruct it first.");
    let mut total = 0;
    let result = (|| {
        for e in capture_files(dir) {
            total += remove(&e.path, freed)?;
        }
        Ok(())
    })();
    crate::bundle::forget_sizes(dir);
    result.map(|()| total)
}

/// Deletes a whole session folder for good, the biggest parts first; `freed` as for [`keep_only_view`]. Refused for a
/// folder without a session manifest.
pub fn delete_session(dir: &Path, freed: &mut dyn FnMut(u64)) -> anyhow::Result<u64> {
    anyhow::ensure!(dir.join(MANIFEST_FILE).is_file(), "{} is not a session folder.", dir.display());
    let mut total = 0;
    let result = (|| -> std::io::Result<()> {
        for e in capture_files(dir) {
            total += remove(&e.path, freed)?;
        }
        total += remove(dir, freed)?;
        Ok(())
    })();
    crate::bundle::forget_sizes(dir);
    result?;
    Ok(total)
}

/// Removes `path`: a file or a link (a link to a folder too, never what it points at), or a folder with all it holds.
/// The plain files right inside a folder go one by one, each told to `freed` (a keyframes folder holds thousands), then
/// whatever is left goes with the folder (`remove_dir_all`, which never follows links). Returns the bytes freed.
fn remove(path: &Path, freed: &mut dyn FnMut(u64)) -> std::io::Result<u64> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        if let Err(e) = std::fs::remove_file(path) {
            // A link to a folder (or a junction) is a folder entry to Windows: removed as one, its target untouched.
            if !meta.file_type().is_symlink() {
                return Err(e);
            }
            std::fs::remove_dir(path)?;
        }
        freed(meta.len());
        return Ok(meta.len());
    }
    let mut total = 0;
    for e in std::fs::read_dir(path)?.flatten() {
        // A plain file (the entry's own type: a link is not one).
        if !e.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
        // One that will not go alone (read-only, held a moment) goes with the folder below.
        if std::fs::remove_file(e.path()).is_ok() {
            freed(bytes);
            total += bytes;
        }
    }
    let rest = crate::bundle::folder_size(path);
    std::fs::remove_dir_all(path)?;
    freed(rest);
    Ok(total + rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{self, BundleWriter};
    use crate::contracts::manifest::{SessionManifest, FRAMES_FILE, THUMBNAIL_FILE};
    use std::fs;

    /// A session as a capture with every stream and a reconstruction leaves it, plus a note the person put there; its
    /// folder is under a temporary root of its own (`name`).
    fn session(name: &str, reconstructed: bool) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("orbiters-cleanup-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut manifest = SessionManifest::new("id-c".into(), crate::now_utc_iso(), "0.2.1");
        manifest.capture.video = Some("colour.mkv".into());
        let writer = BundleWriter::create(&root, "World", manifest).unwrap();
        let dir = writer.dir().to_path_buf();
        writer.write_thumbnail(&[90u8; 16 * 9 * 4], 16, 9).unwrap();
        writer.finish().unwrap();
        let file = |rel: &str, bytes: usize| {
            let path = dir.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, vec![7u8; bytes]).unwrap();
        };
        for i in 0..5 {
            file(&format!("keyframes/{i:06}.qoi"), 1000);
            file(&format!("depth/{i:06}.d16"), 300);
        }
        file("depth-raw/000002.qoi", 200);
        file("colour.mkv", 5000);
        file("parameters.bin", 64);
        file("parameters.json", 32);
        file("tracking.bin", 128);
        file("tracking.json", 16);
        file("calibration.json", 8);
        file("audio.json", 8);
        file("audio/mic.wav", 4000);
        file("skeleton.bin", 100);
        file("notes.txt", 10);
        file("reconstruction.partial/splats.ply", 900);
        if reconstructed {
            file("reconstruction/splats.ply", 2000);
            file("reconstruction/sky.png", 400);
            file("reconstruction/report.json", 20);
            file("reconstruction/preview.qoi", 300);
            file("reconstruction/held-out/000004.qoi", 300);
            file("reconstruction.old/splats.ply", 1500);
        }
        (root, dir)
    }

    /// What the capture wrote: 5 × 1000 keyframes, 5 × 300 + 200 depth, a 5000 video, 4356 of performance streams.
    const CAPTURE: u64 = 5000 + 1700 + 5000 + 4356;

    #[test]
    fn keeping_only_the_3d_view_deletes_the_capture_files_and_keeps_what_the_view_reads() {
        let (root, dir) = session("view", true);
        let kept_before = |p: &str| fs::metadata(dir.join(p)).map(|m| m.len()).ok();
        let manifest = kept_before("manifest.json").unwrap();
        let frames = kept_before(FRAMES_FILE).unwrap_or(0);
        let thumbnail = kept_before(THUMBNAIL_FILE).unwrap();
        let view = manifest + frames + thumbnail + 10 + 2000 + 400 + 20 + 300 + 300;
        // Measured: the capture, plus the run cut short and the previous result (the latest is in place).
        let leftovers = 900 + 1500;
        assert_eq!(measure(&dir, Some("colour.mkv")), Sizes { total: view + CAPTURE + leftovers, capture: CAPTURE + leftovers });
        assert_eq!(bundle::folder_size(&dir), view + CAPTURE + leftovers);
        let files = capture_files(&dir);
        let by = |part| files.iter().filter(|e| e.part == part).map(|e| (e.bytes, e.files)).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        assert_eq!(by(Part::Keyframes), (5000, 5));
        assert_eq!(by(Part::Depth), (1700, 6));
        assert_eq!(by(Part::Video), (5000, 1));
        assert_eq!(by(Part::Performance), (4356, 8));
        assert_eq!(by(Part::Leftovers), (2400, 2));
        assert!(!trimmed(&dir) && has_view(&dir));
        // Listed (its size kept for the next listing), and as another process's library would have kept it.
        let listed = bundle::list_sessions(&root);
        assert_eq!((listed[0].size_bytes, listed[0].capture_bytes, listed[0].trimmed), (view + CAPTURE + leftovers, CAPTURE + leftovers, false));
        let stamp = bundle::stamp(&dir);

        let mut told = 0;
        let freed = keep_only_view(&dir, &mut |n| told += n).unwrap();
        assert_eq!((freed, told), (CAPTURE + leftovers, CAPTURE + leftovers));
        let mut left: Vec<String> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["frames.bin", "manifest.json", "notes.txt", "reconstruction", "thumbnail.qoi"]);
        for kept in [
            "reconstruction/splats.ply",
            "reconstruction/sky.png",
            "reconstruction/report.json",
            "reconstruction/preview.qoi",
            "reconstruction/held-out/000004.qoi",
        ] {
            assert!(dir.join(kept).is_file(), "{kept}");
        }
        assert!(trimmed(&dir) && has_view(&dir));
        assert_eq!(measure(&dir, Some("colour.mkv")), Sizes { total: view, capture: 0 });
        // The library knows at once, size and state: this process forgot the size it kept, and another one (the
        // window) sees the folder changed, however soon after it measured it.
        assert_ne!(bundle::stamp(&dir), stamp);
        let listed = bundle::list_sessions(&root);
        assert_eq!((listed[0].size_bytes, listed[0].capture_bytes, listed[0].trimmed), (view, 0, true));
        // Once more: nothing left to delete.
        assert_eq!(keep_only_view(&dir, &mut |_| {}).unwrap(), 0);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_session_without_a_3d_view_is_not_trimmed_and_can_go_whole() {
        let (root, dir) = session("whole", false);
        assert!(!has_view(&dir));
        let size = bundle::folder_size(&dir);
        // Its run cut short counts as capture files; there is no previous result to tell apart.
        assert_eq!(measure(&dir, None).capture, CAPTURE - 5000 + 900, "without the manifest's name, a video is not the capture's");
        assert!(keep_only_view(&dir, &mut |_| {}).is_err());
        assert!(dir.join("keyframes").is_dir(), "nothing went");
        let mut told = 0;
        assert_eq!(delete_session(&dir, &mut |n| told += n).unwrap(), size);
        assert_eq!(told, size);
        assert!(!dir.exists() && root.is_dir());
        assert!(bundle::list_sessions(&root).is_empty());
        // A folder that is not a session is never deleted.
        let other = root.join("Other");
        fs::create_dir_all(other.join("keyframes")).unwrap();
        assert!(delete_session(&other, &mut |_| {}).is_err() && keep_only_view(&other, &mut |_| {}).is_err());
        assert!(other.join("keyframes").is_dir());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_result_left_aside_by_a_swap_cut_short_is_the_view() {
        let (root, dir) = session("old", true);
        // The swap stopped halfway: the latest result waits in reconstruction.old/.
        fs::remove_dir_all(dir.join("reconstruction")).unwrap();
        fs::write(dir.join("reconstruction.old/report.json"), "{}").unwrap();
        assert!(has_view(&dir));
        assert_eq!(capture_part("reconstruction.old", None, false), None);
        keep_only_view(&dir, &mut |_| {}).unwrap();
        assert!(dir.join("reconstruction.old/splats.ply").is_file() && !dir.join("reconstruction.partial").exists());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn only_the_captures_own_files_count() {
        assert_eq!(capture_part("keyframes", None, true), Some(Part::Keyframes));
        assert_eq!(capture_part("audio", None, true), Some(Part::Performance));
        assert_eq!(capture_part("colour.mkv", Some("colour.mkv"), true), Some(Part::Video));
        for kept in ["manifest.json", "frames.bin", "thumbnail.qoi", "reconstruction", "Keyframes", "notes.txt", "colour.mkv"] {
            assert_eq!(capture_part(kept, None, true), None, "{kept}");
        }
        // A video named outside the folder is never taken for one of its files.
        for video in ["../colour.mkv", "sub/colour.mkv", "C:/colour.mkv", "/colour.mkv", ".."] {
            assert_eq!(capture_part("colour.mkv", Some(video), true), None, "{video}");
        }
    }

    /// A link in the capture's folders goes as a link: what it points at stays.
    #[test]
    fn links_are_removed_not_followed() {
        let (root, dir) = session("links", true);
        let outside = root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("precious.txt"), b"keep me").unwrap();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&outside, dir.join("keyframes").join("link")).is_ok();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&outside, dir.join("keyframes").join("link")).is_ok();
        // A whole capture folder that is a junction to somewhere else (Windows): the junction goes, not what is there.
        #[cfg(windows)]
        let junction = {
            fs::remove_dir_all(dir.join("audio")).unwrap();
            let made = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(dir.join("audio")).arg(&outside).output();
            made.is_ok_and(|o| o.status.success())
        };
        #[cfg(not(windows))]
        let junction = false;
        keep_only_view(&dir, &mut |_| {}).unwrap();
        assert!(outside.join("precious.txt").is_file(), "followed a link (link made: {linked}, junction made: {junction})");
        assert!(!dir.join("keyframes").exists() && fs::symlink_metadata(dir.join("audio")).is_err());
        fs::remove_dir_all(&root).ok();
    }
}
