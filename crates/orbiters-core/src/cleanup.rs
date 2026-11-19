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

use crate::contracts::manifest::{DEPTH_DIR, FRAMES_FILE, KEYFRAMES_DIR, MANIFEST_FILE, RAW_DEPTH_DIR, THUMBNAIL_FILE};
use crate::contracts::performance as perf;
use crate::reconstruction;
use orbiters_i18n::tr;
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
    /// Reconstructions that are not the result: a run cut short (`reconstruction.partial/`), a paused run's checkpoint
    /// (`reconstruction.checkpoint`, no session waits on it once it can be cleaned up), and the previous result while the
    /// latest is in place (`reconstruction.old/`).
    Leftovers,
}

/// Where a session's keyframes wait while they are deleted. They leave `keyframes/` before anything goes: from then on
/// the session is one reduced to its 3D view ([`trimmed`]) whatever stops the rest (the app quitting halfway), so a
/// reconstruction never trains on half of its keyframes.
const KEYFRAMES_GOING: &str = "keyframes.deleting";

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

/// What a 3D view reads in its session folder: never a capture file, whatever the manifest names as its video.
const VIEW_FILES: [&str; 6] =
    [MANIFEST_FILE, FRAMES_FILE, THUMBNAIL_FILE, crate::bundle::PICTURE_THUMBNAIL_FILE, reconstruction::DIR, reconstruction::OLD_DIR];

/// The part of the capture `name` (an entry of a session folder) is, or `None` for what the 3D view needs and for what
/// the capture did not write. `video` is the manifest's `capture.video`, taken only as a file of the folder itself and
/// never for one of the view's own entries;
/// `result_in_place`: the latest result is in `reconstruction/` (else `reconstruction.old/` holds it, and stays).
pub fn capture_part(name: &str, video: Option<&str>, result_in_place: bool) -> Option<Part> {
    if VIEW_FILES.iter().any(|v| v.eq_ignore_ascii_case(name)) && !(name == reconstruction::OLD_DIR && result_in_place) {
        return None;
    }
    match name {
        KEYFRAMES_DIR | KEYFRAMES_GOING => Some(Part::Keyframes),
        DEPTH_DIR | RAW_DEPTH_DIR => Some(Part::Depth),
        reconstruction::PARTIAL_DIR => Some(Part::Leftovers),
        _ if name.starts_with(reconstruction::CHECKPOINT_FILE) => Some(Part::Leftovers),
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
        if entry_part(&e, video, in_place).is_some() {
            sizes.capture += bytes;
        }
    }
    sizes
}

/// [`capture_part`] of a folder entry, where the video counts only as a file (or a link, removed as one): a folder the
/// manifest names as its video is not taken for it.
fn entry_part(e: &std::fs::DirEntry, video: Option<&str>, in_place: bool) -> Option<Part> {
    let part = capture_part(&e.file_name().to_string_lossy(), video, in_place)?;
    if part == Part::Video && !std::fs::symlink_metadata(e.path()).is_ok_and(|m| !m.is_dir()) {
        return None;
    }
    Some(part)
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
            let part = entry_part(&e, video.as_deref(), in_place)?;
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
    trim(dir, &|_| true, freed)
}

/// Deletes for good only a reconstructed session's recording that training read and nothing else does: its keyframes, its
/// depth frames and leftover reconstructions ([`Part::Keyframes`], [`Part::Depth`], [`Part::Leftovers`]), nearly all of a 3D
/// picture's size (187 of 230 MB in an 8 s one). Unlike [`keep_only_view`] it keeps the avatar's streams: the skeleton band
/// gives a share its start view's ground and eye height (`views::eye_level`), so a capture shared after this is the capture
/// shared before it. It can no longer be reconstructed again. Refused without a finished reconstruction.
pub fn drop_recording(dir: &Path, freed: &mut dyn FnMut(u64)) -> anyhow::Result<u64> {
    trim(dir, &|part| matches!(part, Part::Keyframes | Part::Depth | Part::Leftovers), freed)
}

/// Deletes the capture files of `dir` whose part `takes`, keeping its 3D view. The whole list is made and checked
/// before anything moves ([`check_plan`]): nothing goes when one entry is, holds or lies inside what the view reads.
fn trim(dir: &Path, takes: &dyn Fn(Part) -> bool, freed: &mut dyn FnMut(u64)) -> anyhow::Result<u64> {
    anyhow::ensure!(dir.join(MANIFEST_FILE).is_file(), "{}", tr!("{path} is not a session folder.", path = dir.display()));
    anyhow::ensure!(has_view(dir), "{}", tr("This session has no 3D view to keep: reconstruct it first."));
    let plan: Vec<PathBuf> = capture_files(dir).into_iter().filter(|e| takes(e.part)).map(|e| e.path).collect();
    check_plan(dir, &plan)?;
    // Only what the plan holds moves: the keyframes folder as the listing spelt it (a `KEYFRAMES` folder that is not the
    // capture's is never renamed through a case-insensitive lookup; night2, the verifier).
    let named = |name: &str| plan.iter().find(|p| p.file_name().is_some_and(|n| n == name)).cloned();
    let (keyframes, going) = (named(KEYFRAMES_DIR), dir.join(KEYFRAMES_GOING));
    let mut total = 0;
    let result = (|| {
        if let Some(keyframes) = keyframes.as_ref().filter(|k| std::fs::symlink_metadata(k).is_ok_and(|m| m.is_dir())) {
            // What a deletion cut short left there goes first, to make room (the plan's own entry only).
            if let Some(left) = named(KEYFRAMES_GOING) {
                if std::fs::symlink_metadata(&left).is_ok() {
                    total += remove(&left, freed)?;
                }
            }
            crate::rename_retrying(keyframes, &going)?;
        }
        for path in &plan {
            // The keyframes are deleted from where they wait; a leftover `keyframes.deleting` went above.
            let path = if Some(path) == keyframes.as_ref() { &going } else { path };
            if std::fs::symlink_metadata(path).is_ok() {
                total += remove(path, freed)?;
            }
        }
        Ok(())
    })();
    crate::bundle::forget_sizes(dir);
    result.map(|()| total)
}

/// Refuses a deletion `plan` (entries of the session folder `dir`) when an entry is, holds or lies inside one of the
/// paths the 3D view depends on, compared as resolved paths. What the view depends on is found, not listed: the session's
/// manifest, frame records and thumbnails, and every entry of its result folder, each resolved (a sky map or a held-out
/// folder that is a link protects what it leads to; night2, the verifier). An entry of the plan that is a link is
/// removed as a link, never what it leads to (see [`remove`]): only where it is counts. A dependency that exists but
/// cannot be resolved refuses the trim; one that is absent (an optional thumbnail, a dangling link) is nothing to keep.
fn check_plan(dir: &Path, plan: &[PathBuf]) -> anyhow::Result<()> {
    let view = reconstruction::result_dir(dir);
    // The splats themselves, whatever name they resolve to (a link to `final.ply`, a stored `SPLATS.PLY`).
    anyhow::ensure!(std::fs::metadata(view.join(reconstruction::PLY_FILE)).is_ok_and(|m| m.is_file()), "{}", tr("This session has no 3D view to keep: reconstruct it first."));
    let mut kept = Vec::new();
    for f in [MANIFEST_FILE, FRAMES_FILE, THUMBNAIL_FILE, crate::bundle::PICTURE_THUMBNAIL_FILE] {
        keep_resolved(&dir.join(f), &mut kept)?;
    }
    keep_resolved(&view, &mut kept)?;
    keep_view(&view, &mut kept)?;
    kept.sort();
    kept.dedup();
    let here = std::fs::canonicalize(dir)?;
    for path in plan {
        let at = here.join(path.file_name().unwrap_or_default());
        let link = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
        let leads = if link { None } else { std::fs::canonicalize(path).ok() };
        for p in std::iter::once(&at).chain(leads.as_ref()) {
            if let Some(k) = kept.iter().find(|k| k.starts_with(p) || p.starts_with(k)) {
                anyhow::bail!("{}", tr!("Nothing was deleted: {path} is part of the 3D view.", path = k.display()));
            }
        }
    }
    Ok(())
}

/// `path` resolved, into `kept`; nothing when it is absent (or a link to nothing), an error when it exists and cannot be
/// resolved (access denied, held by another program).
fn keep_resolved(path: &Path, kept: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if std::fs::symlink_metadata(path).is_err() {
        return Ok(());
    }
    match std::fs::canonicalize(path) {
        Ok(p) => kept.push(p),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => anyhow::bail!("{}", tr!("Nothing was deleted: {path} is part of the 3D view.", path = path.display())),
    }
    Ok(())
}

/// Every entry under the result folder `dir`, resolved (a link's target protected whole, never walked into).
fn keep_view(dir: &Path, kept: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let path = e.path();
        keep_resolved(&path, kept)?;
        if e.file_type()?.is_dir() {
            keep_view(&path, kept)?;
        }
    }
    Ok(())
}

/// Deletes a whole session folder for good, the biggest parts first; `freed` as for [`keep_only_view`]. Refused for a
/// folder without a session manifest.
pub fn delete_session(dir: &Path, freed: &mut dyn FnMut(u64)) -> anyhow::Result<u64> {
    anyhow::ensure!(dir.join(MANIFEST_FILE).is_file(), "{}", tr!("{path} is not a session folder.", path = dir.display()));
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

    /// An error that can reach a toast is written in the language of the moment it happens (the typed part of the sentence, here the
    /// folder, stays as it is), and the same error in English is untouched.
    #[test]
    fn errors_are_written_in_the_language_of_the_moment() {
        use orbiters_i18n::{in_language, Language};
        let nothing = std::env::temp_dir().join(format!("orbiters-no-session-{}", std::process::id()));
        let shown = nothing.display().to_string();
        let message = || delete_session(&nothing, &mut |_| {}).unwrap_err().to_string();
        assert_eq!(message(), format!("{shown} is not a session folder."));
        in_language(Language::Fr, || assert_eq!(message(), format!("{shown} n’est pas un dossier de session.")));
        in_language(Language::Es, || assert_eq!(message(), format!("{shown} no es una carpeta de sesión.")));
        in_language(Language::Ja, || assert_eq!(message(), format!("{shown}はセッションフォルダーではありません。")));
        in_language(Language::PtBr, || assert_eq!(message(), format!("{shown} não é uma pasta de sessão.")));
    }

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

    /// A tour's picture drops only what training read: its keyframes, depth and leftover runs; the streams a share reads
    /// (the skeleton band's ground and eye height), the 3D view and the person's own files stay; not without a 3D view.
    #[test]
    fn dropping_the_recording_keeps_what_a_share_and_the_view_read() {
        let (root, dir) = session("drop", false);
        assert!(drop_recording(&dir, &mut |_| {}).is_err(), "nothing to keep without a reconstruction");
        assert!(dir.join("keyframes").is_dir());
        let _ = fs::remove_dir_all(&root);
        let (root, dir) = session("drop-done", true);
        let mut told = 0;
        let freed = drop_recording(&dir, &mut |n| told += n).unwrap();
        assert_eq!((freed, told), (5000 + 1700 + 900 + 1500, 5000 + 1700 + 900 + 1500));
        for gone in ["keyframes", "depth", "depth-raw", "reconstruction.partial", "reconstruction.old", "keyframes.deleting"] {
            assert!(!dir.join(gone).exists(), "{gone}");
        }
        for kept in ["skeleton.bin", "parameters.bin", "audio/mic.wav", "colour.mkv", "notes.txt", "reconstruction/splats.ply", "reconstruction/report.json", "manifest.json"] {
            assert!(dir.join(kept).exists(), "{kept}");
        }
        assert!(trimmed(&dir) && has_view(&dir), "a session reduced to its 3D view, as the library knows them");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn keeping_only_the_3d_view_deletes_the_capture_files_and_keeps_what_the_view_reads() {
        let (root, dir) = session("view", true);
        let kept_before = |p: &str| fs::metadata(dir.join(p)).map(|m| m.len()).ok();
        let manifest = kept_before("manifest.json").unwrap();
        let frames = kept_before(FRAMES_FILE).unwrap_or(0);
        let thumbnail = kept_before(THUMBNAIL_FILE).unwrap();
        let picture_thumbnail = kept_before(bundle::PICTURE_THUMBNAIL_FILE).unwrap();
        let view = manifest + frames + thumbnail + picture_thumbnail + 10 + 2000 + 400 + 20 + 300 + 300;
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
        assert_eq!(left, ["frames.bin", "manifest.json", "notes.txt", "reconstruction", "thumbnail-picture-v2.qoi", "thumbnail.qoi"]);
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

    /// A deletion cut short (the app quit halfway through the keyframes) left a session already reduced to its 3D
    /// view: its keyframes had left `keyframes/` first. The next one finishes it.
    #[test]
    fn a_deletion_cut_short_leaves_no_half_keyframes_to_reconstruct_from() {
        let (root, dir) = session("cut", true);
        fs::rename(dir.join("keyframes"), dir.join(KEYFRAMES_GOING)).unwrap();
        fs::remove_file(dir.join(KEYFRAMES_GOING).join("000000.qoi")).unwrap();
        assert!(trimmed(&dir) && has_view(&dir));
        let going = capture_files(&dir).into_iter().find(|e| e.path.ends_with(KEYFRAMES_GOING)).expect("listed");
        assert_eq!((going.part, going.bytes, going.files), (Part::Keyframes, 4000, 4));
        assert_eq!(measure(&dir, Some("colour.mkv")).capture, CAPTURE - 1000 + 900 + 1500);
        keep_only_view(&dir, &mut |_| {}).unwrap();
        assert!(!dir.join(KEYFRAMES_GOING).exists() && !dir.join("depth").exists());
        // Both there (a session that got keyframes again): the leftover goes first, then the keyframes.
        let (root2, dir2) = session("both", true);
        fs::create_dir_all(dir2.join(KEYFRAMES_GOING)).unwrap();
        fs::write(dir2.join(KEYFRAMES_GOING).join("old.qoi"), b"x").unwrap();
        keep_only_view(&dir2, &mut |_| {}).unwrap();
        assert!(!dir2.join(KEYFRAMES_GOING).exists() && !dir2.join("keyframes").exists());
        fs::remove_dir_all(&root).ok();
        fs::remove_dir_all(&root2).ok();
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

    /// The manifest of `dir` names `video` as its capture's video.
    fn name_video(dir: &Path, video: &str) {
        let mut manifest = bundle::read_manifest(dir).unwrap();
        manifest.capture.video = Some(video.into());
        fs::write(dir.join(MANIFEST_FILE), serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    }

    /// AS-15: a manifest naming the view's own folder (or file) as its video never puts it on the list: the trim deletes
    /// the capture and the reconstruction, its splats and report stay whole.
    #[test]
    fn a_video_named_as_the_view_is_never_deleted() {
        for video in ["reconstruction", "Reconstruction", "manifest.json", "frames.bin", "thumbnail.qoi", "thumbnail-picture-v2.qoi"] {
            let (root, dir) = session("as15", true);
            name_video(&dir, video);
            let before = bundle::folder_size(&dir.join("reconstruction"));
            assert!(capture_files(&dir).iter().all(|e| e.part != Part::Video), "{video}");
            keep_only_view(&dir, &mut |_| {}).unwrap();
            assert_eq!(bundle::folder_size(&dir.join("reconstruction")), before, "{video}");
            assert!(has_view(&dir) && dir.join(MANIFEST_FILE).is_file() && dir.join(THUMBNAIL_FILE).is_file(), "{video}");
            assert!(!dir.join("keyframes").exists() && !dir.join("depth").exists(), "{video}: the capture still goes");
            fs::remove_dir_all(&root).ok();
        }
        // The previous result is the view while the latest is not in place: never the video either.
        let (root, dir) = session("as15-old", true);
        fs::remove_dir_all(dir.join("reconstruction")).unwrap();
        fs::write(dir.join("reconstruction.old/report.json"), "{}").unwrap();
        name_video(&dir, "reconstruction.old");
        drop_recording(&dir, &mut |_| {}).unwrap();
        assert!(dir.join("reconstruction.old/splats.ply").is_file() && has_view(&dir));
        fs::remove_dir_all(&root).ok();
    }

    /// A video is a file: a folder the manifest names (the person's own) is not taken for it, nor counted as capture.
    #[test]
    fn a_folder_named_as_the_video_is_not_the_video() {
        let (root, dir) = session("as15-folder", true);
        fs::create_dir_all(dir.join("mine")).unwrap();
        fs::write(dir.join("mine/photo.png"), b"mine").unwrap();
        name_video(&dir, "mine");
        let capture = measure(&dir, Some("mine")).capture;
        assert_eq!(capture, CAPTURE - 5000 + 900 + 1500, "the folder is not counted, nor colour.mkv any more");
        keep_only_view(&dir, &mut |_| {}).unwrap();
        assert!(dir.join("mine/photo.png").is_file());
        fs::remove_dir_all(&root).ok();
    }

    /// A folder link at `link` leading to `target` (a junction on Windows, which needs no rights); made, or the test fails.
    fn folder_link(link: &Path, target: &Path) {
        #[cfg(windows)]
        let made = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(link).arg(target).output().is_ok_and(|o| o.status.success());
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, link).is_ok();
        assert!(made, "could not link {} to {}", link.display(), target.display());
    }

    /// Removes the link at `link` alone, never what it leads to.
    fn unlink(link: &Path) {
        fs::remove_dir(link).or_else(|_| fs::remove_file(link)).ok();
    }

    /// night2 (the verifier): a capture entry that is a link into the view goes as a link (what it leads to was never
    /// deleted); the trim no longer refuses for good a session that only had such a link.
    #[test]
    fn a_link_into_the_view_goes_as_a_link() {
        let (root, dir) = session("as15-link", true);
        fs::remove_dir_all(dir.join("audio")).unwrap();
        let target = dir.join("reconstruction");
        folder_link(&dir.join("audio"), &target);
        let before = bundle::folder_size(&target);
        keep_only_view(&dir, &mut |_| {}).unwrap();
        assert!(fs::symlink_metadata(dir.join("audio")).is_err(), "the link went");
        assert_eq!(bundle::folder_size(&target), before);
        assert!(has_view(&dir) && !dir.join("keyframes").exists());
        fs::remove_dir_all(&root).ok();
    }

    /// night2 (the verifier): a part of the view that is a link into the capture (here the held-out renders, as a sky map
    /// linked to a file under depth/ would be) protects what it leads to: nothing is deleted, nothing moves.
    #[test]
    fn a_view_part_linked_into_the_capture_deletes_nothing() {
        let (root, dir) = session("as15-viewlink", true);
        let held_out = dir.join("reconstruction").join("held-out");
        fs::remove_dir_all(&held_out).unwrap();
        folder_link(&held_out, &dir.join("depth-raw"));
        let err = keep_only_view(&dir, &mut |_| {}).unwrap_err().to_string();
        assert!(err.starts_with("Nothing was deleted"), "{err}");
        assert!(dir.join("keyframes").is_dir() && !dir.join(KEYFRAMES_GOING).exists(), "nothing moved");
        assert!(held_out.join("000002.qoi").is_file(), "the view's renders still resolve");
        assert!(drop_recording(&dir, &mut |_| {}).is_err(), "drop_recording takes depth-raw too");
        assert!(held_out.join("000002.qoi").is_file());
        // A sky map that is a file link into depth/ (where this system lets a test make one).
        let sky = dir.join("reconstruction/sky.png");
        fs::remove_file(&sky).unwrap();
        #[cfg(windows)]
        let file_linked = std::os::windows::fs::symlink_file(dir.join("depth/000001.d16"), &sky).is_ok();
        #[cfg(unix)]
        let file_linked = std::os::unix::fs::symlink(dir.join("depth/000001.d16"), &sky).is_ok();
        unlink(&held_out);
        fs::create_dir_all(&held_out).unwrap();
        if file_linked {
            assert!(keep_only_view(&dir, &mut |_| {}).is_err());
            assert!(fs::read(&sky).is_ok_and(|b| b.len() == 300), "the sky still resolves");
        } else {
            eprintln!("no file link could be made here (Windows without the right): the sky case is covered by the folder link above");
        }
        fs::remove_dir_all(&root).ok();
    }

    /// night2 (the verifier): splats stored as `SPLATS.PLY` on a volume that ignores case are the view (it was refused
    /// for good: their resolved name did not end in `splats.ply`).
    #[cfg(windows)]
    #[test]
    fn splats_stored_under_another_spelling_keep_their_view() {
        let (root, dir) = session("as15-spelling", true);
        fs::rename(dir.join("reconstruction/splats.ply"), dir.join("reconstruction/SPLATS.PLY")).unwrap();
        assert!(has_view(&dir));
        keep_only_view(&dir, &mut |_| {}).unwrap();
        assert!(dir.join("reconstruction/SPLATS.PLY").is_file() && !dir.join("keyframes").exists());
        fs::remove_dir_all(&root).ok();
    }

    /// night2 (the verifier): only what the plan lists moves: a `KEYFRAMES` folder that is not the capture's (its name
    /// does not match) is not renamed through a lookup that ignores case.
    #[cfg(windows)]
    #[test]
    fn only_the_listed_keyframes_move() {
        let (root, dir) = session("as15-case", true);
        fs::rename(dir.join("keyframes"), dir.join("KEYFRAMES")).unwrap();
        keep_only_view(&dir, &mut |_| {}).unwrap();
        let names: Vec<String> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(names.contains(&"KEYFRAMES".to_string()) && !names.iter().any(|n| n.eq_ignore_ascii_case(KEYFRAMES_GOING)), "{names:?}");
        assert!(!dir.join("depth").exists(), "the rest of the capture still goes");
        fs::remove_dir_all(&root).ok();
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
