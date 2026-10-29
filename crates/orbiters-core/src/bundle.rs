//! Session bundles (see `contracts/Docs/session-bundle.md`): writing during capture, listing and reading
//! for the library.

use crate::contracts::frame_record::{record_flags, FrameRecord, RECORD_SIZE};
use crate::contracts::manifest::{self, SessionManifest};
use crate::contracts::qoi;
use anyhow::Context;
use serde::Serialize;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

pub fn keyframe_file(index: i32) -> String {
    format!("{index:06}.qoi")
}

pub fn depth_file(index: i32) -> String {
    format!("{index:06}.d16")
}

/// Creates `parent/name`, or `parent/name (2)`, `(3)`, … when it exists.
pub fn unique_dir(parent: &Path, name: &str) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    for i in 1..10_000 {
        let candidate = if i == 1 { parent.join(name) } else { parent.join(format!("{name} ({i})")) };
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("creating {}", candidate.display())),
        }
    }
    anyhow::bail!("no free folder name for {name}")
}

/// Writes one session bundle. Keyframe and depth files are written by the caller (often on other threads) at
/// the paths this writer hands out; records are appended in frame order here. The caller checkpoints now and then
/// ([`BundleWriter::checkpoint`]: the recording session does every few seconds).
pub struct BundleWriter {
    dir: PathBuf,
    manifest: SessionManifest,
    frames: BufWriter<File>,
    next_keyframe: i32,
    next_depth: i32,
    started: Instant,
    /// Duration measured in game time instead of wall time (sources replayed faster than real time).
    game_time_duration: bool,
}

impl BundleWriter {
    pub fn create(parent: &Path, folder_name: &str, manifest: SessionManifest) -> anyhow::Result<Self> {
        let dir = unique_dir(parent, &crate::safe_file_name(folder_name))?;
        fs::create_dir_all(dir.join(manifest::KEYFRAMES_DIR))?;
        fs::create_dir_all(dir.join(manifest::DEPTH_DIR))?;
        let frames = BufWriter::with_capacity(1 << 16, File::create(dir.join(manifest::FRAMES_FILE))?);
        let writer = Self { dir, manifest, frames, next_keyframe: 0, next_depth: 0, started: Instant::now(), game_time_duration: false };
        writer.write_manifest()?;
        Ok(writer)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn manifest(&self) -> &SessionManifest {
        &self.manifest
    }

    pub fn manifest_mut(&mut self) -> &mut SessionManifest {
        &mut self.manifest
    }

    pub fn elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    /// Next keyframe index and its file path.
    pub fn reserve_keyframe(&mut self) -> (i32, PathBuf) {
        let i = self.next_keyframe;
        self.next_keyframe += 1;
        (i, self.dir.join(manifest::KEYFRAMES_DIR).join(keyframe_file(i)))
    }

    /// Where depth frame `index` goes as received (`depth-raw/`, created on first use).
    pub fn raw_depth_path(&self, index: i32) -> anyhow::Result<PathBuf> {
        let dir = self.dir.join(manifest::RAW_DEPTH_DIR);
        fs::create_dir_all(&dir)?;
        Ok(dir.join(keyframe_file(index)))
    }

    /// Next depth index and its file path.
    pub fn reserve_depth(&mut self) -> (i32, PathBuf) {
        let i = self.next_depth;
        self.next_depth += 1;
        (i, self.dir.join(manifest::DEPTH_DIR).join(depth_file(i)))
    }

    pub fn append_record(&mut self, record: &FrameRecord) -> anyhow::Result<()> {
        self.frames.write_all(&record.to_bytes())?;
        let s = &mut self.manifest.stats;
        s.frames += 1;
        s.bytes += RECORD_SIZE as u64;
        if record.has(record_flags::STRIP_OK) {
            s.strip_ok += 1;
            if s.first_game_time.is_none() {
                s.first_game_time = Some(record.game_time);
            }
            s.last_game_time = Some(record.game_time);
        }
        if record.has(record_flags::KEYFRAME) {
            s.keyframes += 1;
        }
        if record.has(record_flags::DEPTH) {
            s.depth_frames += 1;
        }
        if record.has(record_flags::GATED) {
            s.gated += 1;
        }
        Ok(())
    }

    /// Writes the frame records and the manifest as they stand (duration so far), so a session that never
    /// finishes (the worker killed or crashed, Windows shut down) reads up to here. The caller calls it every few
    /// seconds, and again later when it fails.
    pub fn checkpoint(&mut self) -> anyhow::Result<()> {
        self.frames.flush()?;
        self.measure_duration();
        self.write_manifest()
    }

    /// [`BundleWriter::checkpoint`] for a caller that writes the manifest on another thread (a recording's thread
    /// does not wait for the disk to flush it): the frame records go to their file, and the manifest as it stands
    /// (duration so far) comes back as the path and bytes of `manifest.json`, to write with [`crate::write_atomic`].
    pub fn checkpoint_deferred(&mut self) -> anyhow::Result<(PathBuf, Vec<u8>)> {
        self.frames.flush()?;
        self.measure_duration();
        Ok((self.dir.join(manifest::MANIFEST_FILE), serde_json::to_vec_pretty(&self.manifest)?))
    }

    fn measure_duration(&mut self) {
        let s = &mut self.manifest.stats;
        s.duration_s = match (self.game_time_duration, s.first_game_time, s.last_game_time) {
            (true, Some(first), Some(last)) => f64::from((last - first).max(0.0)),
            _ => self.started.elapsed().as_secs_f64(),
        };
    }

    /// Bytes written by other threads (keyframes, depth files).
    pub fn add_bytes(&mut self, n: u64) {
        self.manifest.stats.bytes += n;
    }

    /// Writes the session thumbnail (a downscaled copy of the first keyframe).
    pub fn write_thumbnail(&self, rgba: &[u8], width: u32, height: u32) -> anyhow::Result<()> {
        anyhow::ensure!(width > 0 && height > 0 && u64::from(width) * u64::from(height) * 4 <= rgba.len() as u64, "Invalid thumbnail dimensions.");
        let crop = capture_picture_rect(width, height);
        let row = width as usize * 4;
        let (tw, th, small) = downscale(&rgba[crop.y0 as usize * row..crop.y1 as usize * row], width, crop.y1 - crop.y0, 480);
        let bytes = qoi::encode(&small, tw, th, 3);
        fs::write(self.dir.join(manifest::THUMBNAIL_FILE), &bytes)?;
        crate::write_atomic(&self.dir.join(PICTURE_THUMBNAIL_FILE), &bytes)?;
        Ok(())
    }

    pub fn write_manifest(&self) -> anyhow::Result<()> {
        crate::write_atomic(&self.dir.join(manifest::MANIFEST_FILE), serde_json::to_string_pretty(&self.manifest)?.as_bytes())?;
        Ok(())
    }

    /// Measures the session's duration in game time (for sources replayed faster than real time).
    pub fn use_game_time_duration(&mut self) {
        self.game_time_duration = true;
    }

    pub fn finish(self) -> anyhow::Result<(PathBuf, SessionManifest)> {
        let (dir, manifest, error) = self.finish_keeping();
        match error {
            Some(e) => Err(e),
            None => Ok((dir, manifest)),
        }
    }

    /// Flushes the frame records and writes the final manifest, both tried whatever the other did; the manifest
    /// comes back even when it could not be written (a full disk), with why.
    pub fn finish_keeping(mut self) -> (PathBuf, SessionManifest, Option<anyhow::Error>) {
        let flushed = self.frames.flush().map_err(anyhow::Error::from);
        self.measure_duration();
        let written = self.write_manifest();
        (self.dir, self.manifest, flushed.err().or(written.err()))
    }
}

/// Box-filter downscale of RGBA to at most `max_width` wide; returns RGB. An empty image (or fewer pixels than its
/// size says) gives an empty one.
pub fn downscale(rgba: &[u8], width: u32, height: u32, max_width: u32) -> (u32, u32, Vec<u8>) {
    if width == 0 || height == 0 || (rgba.len() as u64) < width as u64 * height as u64 * 4 {
        return (0, 0, Vec::new());
    }
    let factor = width.div_ceil(max_width.max(1)).max(1);
    let (tw, th) = ((width / factor).max(1), (height / factor).max(1));
    let mut out = vec![0u8; (tw * th * 3) as usize];
    for ty in 0..th {
        for tx in 0..tw {
            let mut acc = [0u32; 3];
            for dy in 0..factor {
                for dx in 0..factor {
                    let (x, y) = ((tx * factor + dx).min(width - 1), (ty * factor + dy).min(height - 1));
                    let i = ((y * width + x) * 4) as usize;
                    acc[0] += rgba[i] as u32;
                    acc[1] += rgba[i + 1] as u32;
                    acc[2] += rgba[i + 2] as u32;
                }
            }
            let n = factor * factor;
            let o = ((ty * tw + tx) * 3) as usize;
            out[o] = (acc[0] / n) as u8;
            out[o + 1] = (acc[1] / n) as u8;
            out[o + 2] = (acc[2] / n) as u8;
        }
    }
    (tw, th, out)
}

/// A session as the library shows it.
#[derive(Clone, Debug, Serialize)]
pub struct SessionSummary {
    pub dir: PathBuf,
    pub manifest: SessionManifest,
    pub has_thumbnail: bool,
    pub size_bytes: u64,
    /// Of `size_bytes`, the capture files only reconstructing it again needs (frames, depth, sound and the other
    /// streams): what keeping only its 3D view frees ([`crate::cleanup`]).
    pub capture_bytes: u64,
    /// Its keyframes are gone ([`crate::cleanup::trimmed`]): with a reconstruction, its capture files were deleted to
    /// keep only its 3D view, and it is not reconstructed again.
    pub trimmed: bool,
    /// Seconds since the Unix epoch of the manifest's last change.
    pub modified: u64,
    /// The latest finished reconstruction.
    pub reconstruction: Option<crate::reconstruction::Report>,
    /// Why the session's manifest could not be read (cut short by a crash or a power cut, or not JSON): the session is
    /// listed all the same, with what its folder shows and an empty manifest, so it can be seen, opened and removed
    /// rather than hide its gigabytes. `None` for a session read as it should be.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub damaged: Option<String>,
}

/// A session's manifest. One written by another version (a field it does not know the shape of) keeps every field
/// that fits rather than leaving the session out of the library ([`crate::json_lenient`]); a file that is not JSON is
/// an error.
pub fn read_manifest(dir: &Path) -> anyhow::Result<SessionManifest> {
    let path = dir.join(manifest::MANIFEST_FILE);
    let text = crate::read_text(&path)?.ok_or_else(|| anyhow::anyhow!("{} not found", path.display()))?;
    let (manifest, dropped) = crate::json_lenient::<SessionManifest>(&text);
    anyhow::ensure!(dropped.iter().all(|d| d != "(not JSON)"), "{} is not a manifest", path.display());
    Ok(manifest)
}

/// What a session folder's size was measured with: when its manifest, the folder itself (files added or removed
/// there), its latest reconstruction's report and its keyframes and depth folders last changed. A recording rewrites
/// the manifest every few seconds, a finished reconstruction is renamed into the folder with a report of its own, and
/// freeing space ([`crate::cleanup`]) empties or removes the keyframes and depth folders.
pub(crate) type Stamp = [Option<SystemTime>; 5];

pub(crate) fn stamp(dir: &Path) -> Stamp {
    let modified = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    let report = crate::reconstruction::result_dir(dir).join(crate::reconstruction::REPORT_FILE);
    [
        modified(&dir.join(manifest::MANIFEST_FILE)),
        modified(dir),
        modified(&report),
        modified(&dir.join(manifest::KEYFRAMES_DIR)),
        modified(&dir.join(manifest::DEPTH_DIR)),
    ]
}

/// Session sizes measured, by folder, with what they were measured with.
type Measured = std::collections::HashMap<PathBuf, (Stamp, crate::cleanup::Sizes)>;

fn measured() -> std::sync::MutexGuard<'static, Option<Measured>> {
    static SIZES: std::sync::Mutex<Option<Measured>> = std::sync::Mutex::new(None);
    SIZES.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// [`crate::cleanup::measure`] of a session (`video`: its manifest's `capture.video`), measured again only once
/// something in it changed ([`Stamp`]): the library's size is known again without stating every file of every session
/// (thousands of keyframes each).
pub fn session_sizes(dir: &Path, video: Option<&str>) -> crate::cleanup::Sizes {
    let now = stamp(dir);
    let known = measured().as_ref().and_then(|sizes| sizes.get(dir).copied());
    if let Some((then, sizes)) = known {
        if then == now && now[0].is_some() {
            return sizes;
        }
    }
    let sizes = crate::cleanup::measure(dir, video);
    let mut all = measured();
    let all = all.get_or_insert_with(Measured::new);
    // Folders gone (trashed, another library) are forgotten now and then.
    if all.len() > 4096 {
        all.retain(|d, _| d.is_dir());
    }
    all.insert(dir.to_path_buf(), (now, sizes));
    sizes
}

/// Forgets the size kept for a session: files were deleted from it, it is measured again when next listed.
pub fn forget_sizes(dir: &Path) {
    if let Some(all) = measured().as_mut() {
        all.remove(dir);
    }
}

/// Total size of a folder's files (recursive).
pub fn folder_size(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => total += folder_size(&e.path()),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total
}

/// Sessions under `root`, newest first. A folder with a manifest of another program's format is not a session; one
/// whose manifest cannot be read at all is listed as [`SessionSummary::damaged`].
pub fn list_sessions(root: &Path) -> Vec<SessionSummary> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(root) else { return out };
    for e in entries.flatten() {
        let dir = e.path();
        let manifest_file = dir.join(manifest::MANIFEST_FILE);
        if !dir.is_dir() || !manifest_file.is_file() {
            continue;
        }
        let (manifest, damaged) = match read_manifest(&dir) {
            Ok(manifest) if manifest.format == manifest::MANIFEST_FORMAT => (manifest, None),
            Ok(_) => continue,
            Err(e) => (SessionManifest { format: manifest::MANIFEST_FORMAT.into(), ..Default::default() }, Some(format!("{e:#}"))),
        };
        let changed = fs::metadata(&manifest_file).and_then(|m| m.modified()).or_else(|_| fs::metadata(&dir).and_then(|m| m.modified()));
        let modified = changed.ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        let sizes = session_sizes(&dir, manifest.capture.video.as_deref());
        out.push(SessionSummary {
            has_thumbnail: dir.join(manifest::THUMBNAIL_FILE).is_file(),
            size_bytes: sizes.total,
            capture_bytes: sizes.capture,
            trimmed: crate::cleanup::trimmed(&dir),
            reconstruction: crate::reconstruction::read_report(&dir),
            dir,
            manifest,
            modified,
            damaged,
        });
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.modified));
    out
}

/// The frame records of a bundle that read as records ([`read_records_counted`]).
pub fn read_records(dir: &Path) -> anyhow::Result<Vec<FrameRecord>> {
    read_records_counted(dir).map(|(records, _)| records)
}

/// The frame records of a bundle, and how many did not read as one. Records have a fixed size, so one that does not
/// read (zeros a power cut left, a version this one does not know) is passed over and the next ones still read; a
/// record cut short at the end (the worker stopped mid-write) is left out. Only a file that cannot be read is an error.
pub fn read_records_counted(dir: &Path) -> anyhow::Result<(Vec<FrameRecord>, usize)> {
    let bytes = fs::read(dir.join(manifest::FRAMES_FILE))?;
    let chunks = bytes.as_chunks::<RECORD_SIZE>().0;
    let records: Vec<FrameRecord> = chunks.iter().filter_map(|c| FrameRecord::from_bytes(c).ok()).collect();
    let bad = chunks.len() - records.len();
    Ok((records, bad))
}

/// The largest session thumbnail read: [`BundleWriter::write_thumbnail`] makes them at most 480 wide (a picture
/// standing up, four times as tall). A file saying it is larger is not one, and gets no memory.
const THUMBNAIL_PIXELS: u64 = 480 * 480 * 4;

/// Versioned display cache: old session thumbnails include the bottom skeleton band.
pub const PICTURE_THUMBNAIL_FILE: &str = "thumbnail-picture-v2.qoi";

/// Scene rows between the capture strip (including its reference patches) and skeleton band.
/// Layouts are computed at the ORIGINAL frame size, before any thumbnail downscaling.
pub fn capture_picture_rect(width: u32, height: u32) -> crate::contracts::image::Rect {
    let top = crate::contracts::strip::StripLayout::new(width, height).map_or(0, |l| l.reserved.y1);
    let bottom = crate::contracts::skeleton::BandLayout::new(width, height).map_or(height, |l| l.reserved().y0);
    let (top, bottom) = if top < bottom { (top, bottom) } else { (0, height) };
    crate::contracts::image::Rect { x0: 0, y0: top, x1: width, y1: bottom }
}

/// The session thumbnail as top-down RGBA: (width, height, pixels).
pub fn read_thumbnail(dir: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let cached = dir.join(PICTURE_THUMBNAIL_FILE);
    if let Some((h, rgba)) = fs::read(&cached).ok().and_then(|data| qoi::decode_rgba_max(&data, THUMBNAIL_PIXELS).ok()) {
        return Some((h.width, h.height, rgba));
    }
    let data = fs::read(dir.join(manifest::THUMBNAIL_FILE)).ok()?;
    let (h, rgba) = qoi::decode_rgba_max(&data, THUMBNAIL_PIXELS).ok()?;
    let manifest: Option<SessionManifest> = fs::read(dir.join(manifest::MANIFEST_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let (top, bottom) = manifest.filter(|m| m.capture.width > 0 && m.capture.height > 0).map_or((0, h.height), |m| {
        let (w, height) = (m.capture.width, m.capture.height);
        let crop = capture_picture_rect(w, height);
        // Capture's encoder already omitted the top strip; BundleWriter's old thumbnails held the full frame.
        // Compare aspect ratios to identify which rows the cached image represents, with rounding tolerance.
        let expected_height = |rows: u32| f64::from(rows) * f64::from(h.width) / f64::from(w);
        let full_error = (expected_height(height) - f64::from(h.height)).abs();
        let below_error = (expected_height(height - crop.y0) - f64::from(h.height)).abs();
        let original_top = if below_error < full_error { crop.y0 } else { 0 };
        let rows = height - original_top;
        let top = (u64::from(crop.y0 - original_top) * u64::from(h.height)).div_ceil(u64::from(rows)) as u32;
        let bottom = (u64::from(crop.y1 - original_top) * u64::from(h.height) / u64::from(rows)) as u32;
        if top < bottom {
            (top, bottom)
        } else {
            (0, h.height)
        }
    });
    let row = h.width as usize * 4;
    let rgba = rgba[top as usize * row..bottom as usize * row].to_vec();
    let height = bottom - top;
    let _ = crate::write_atomic(&cached, &qoi::encode(&rgba, h.width, height, 4));
    Some((h.width, height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn picture_crop_uses_original_capture_layouts() {
        use crate::contracts::image::Rect;
        assert_eq!(capture_picture_rect(1920, 1080), Rect { x0: 0, y0: 240, x1: 1920, y1: 1026 });
        assert_eq!(capture_picture_rect(3840, 2160), Rect { x0: 0, y0: 480, x1: 3840, y1: 2106 });
        assert_eq!(capture_picture_rect(1280, 720), Rect { x0: 0, y0: 150, x1: 1280, y1: 618 });
        assert_eq!(capture_picture_rect(64, 32), Rect { x0: 0, y0: 0, x1: 64, y1: 32 });
    }

    #[test]
    fn stale_picture_thumbnails_crop_both_full_and_already_top_cropped_frames() {
        let root = std::env::temp_dir().join(format!("orbiters-thumb-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let mut m = SessionManifest::default();
        m.capture.width = 1920;
        m.capture.height = 1080;
        fs::write(root.join(manifest::MANIFEST_FILE), serde_json::to_vec(&m).unwrap()).unwrap();
        for (height, top) in [(270, 60), (210, 0)] {
            let mut pixels = vec![255; 480 * height * 4];
            for row in top..(height - 14) {
                pixels[row * 480 * 4..(row + 1) * 480 * 4].fill(80);
            }
            fs::write(root.join(manifest::THUMBNAIL_FILE), qoi::encode(&pixels, 480, height as u32, 4)).unwrap();
            let (w, h, rgba) = read_thumbnail(&root).unwrap();
            assert_eq!((w, h), (480, 196));
            assert!(rgba.iter().all(|v| *v == 80));
            assert_eq!(read_thumbnail(&root).unwrap().2, rgba, "the versioned cache is not cropped again");
            fs::remove_file(root.join(PICTURE_THUMBNAIL_FILE)).unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }
    use crate::contracts::manifest::SessionManifest;
    use std::time::Duration;

    #[test]
    fn a_damaged_session_is_listed_and_its_good_records_read() {
        let root = std::env::temp_dir().join(format!("orbiters-bundle-damaged-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut w = BundleWriter::create(&root, "Damaged", SessionManifest::new("id-4".into(), crate::now_utc_iso(), "0.1.0")).unwrap();
        let good = |i: u64| FrameRecord { frame_index: i, flags: record_flags::STRIP_OK, ..Default::default() };
        w.append_record(&good(0)).unwrap();
        let (dir, _) = w.finish().unwrap();
        // Zeros where a record should be (a power cut), a good record after them, and one cut short at the end.
        let mut bytes = fs::read(dir.join(manifest::FRAMES_FILE)).unwrap();
        bytes.extend_from_slice(&[0u8; RECORD_SIZE]);
        bytes.extend_from_slice(&good(2).to_bytes());
        bytes.extend_from_slice(&good(3).to_bytes()[..RECORD_SIZE / 2]);
        fs::write(dir.join(manifest::FRAMES_FILE), bytes).unwrap();
        let (records, bad) = read_records_counted(&dir).unwrap();
        assert_eq!((records.iter().map(|r| r.frame_index).collect::<Vec<_>>(), bad), (vec![0, 2], 1));
        assert_eq!(read_records(&dir).unwrap().len(), 2);
        // A manifest that is no longer JSON: the session is still listed, flagged, with its size.
        fs::write(dir.join(manifest::MANIFEST_FILE), [0u8; 64]).unwrap();
        let list = list_sessions(&root);
        assert_eq!(list.len(), 1);
        assert!(list[0].damaged.is_some() && list[0].size_bytes > 0, "{:?}", list[0].damaged);
        // A folder without a manifest, or with another program's, is no session.
        fs::create_dir_all(root.join("notes")).unwrap();
        fs::create_dir_all(root.join("other")).unwrap();
        fs::write(root.join("other").join(manifest::MANIFEST_FILE), r#"{"format":"something.else"}"#).unwrap();
        assert_eq!(list_sessions(&root).len(), 1);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn write_list_read() {
        let root = std::env::temp_dir().join(format!("orbiters-bundle-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let m = SessionManifest::new("id-1".into(), crate::now_utc_iso(), "0.1.0");
        let mut w = BundleWriter::create(&root, "World: test", m).unwrap();
        let mut r = FrameRecord { flags: record_flags::STRIP_OK | record_flags::KEYFRAME, ..Default::default() };
        let (ki, kp) = w.reserve_keyframe();
        r.keyframe_index = ki;
        fs::write(&kp, b"x").unwrap();
        w.append_record(&r).unwrap();
        w.write_thumbnail(&[200u8; 64 * 32 * 4], 64, 32).unwrap();
        let (dir, manifest) = w.finish().unwrap();
        assert!(dir.ends_with("World_ test"));
        assert_eq!((manifest.stats.frames, manifest.stats.keyframes), (1, 1));
        let list = list_sessions(&root);
        assert_eq!(list.len(), 1);
        assert!(list[0].has_thumbnail);
        assert_eq!(read_records(&dir).unwrap()[0], r);
        let (tw, th, pixels) = read_thumbnail(&dir).unwrap();
        assert_eq!((tw, th, pixels.len()), (64, 32, 64 * 32 * 4));
        // One that says it is larger than any thumbnail made is not read (nor given the memory it asks for).
        let mut big = qoi::encode(&[0u8; 3], 1, 1, 3);
        big[4..8].copy_from_slice(&4000u32.to_be_bytes());
        big[8..12].copy_from_slice(&4000u32.to_be_bytes());
        fs::write(dir.join(manifest::THUMBNAIL_FILE), big).unwrap();
        fs::remove_file(dir.join(PICTURE_THUMBNAIL_FILE)).unwrap();
        assert!(read_thumbnail(&dir).is_none());
        // A second session with the same name gets a numbered folder.
        let w2 = BundleWriter::create(&root, "World: test", SessionManifest::default()).unwrap();
        assert!(w2.dir().ends_with("World_ test (2)"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_manifest_from_another_version_still_lists_and_sizes_follow_changes() {
        let root = std::env::temp_dir().join(format!("orbiters-bundle-lenient-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let w = BundleWriter::create(&root, "Later", SessionManifest::new("id-3".into(), crate::now_utc_iso(), "0.1.0")).unwrap();
        let (dir, _) = w.finish().unwrap();
        // A field of a shape this version does not know: the rest of the manifest is read.
        let mut value: serde_json::Value = serde_json::from_str(&fs::read_to_string(dir.join(manifest::MANIFEST_FILE)).unwrap()).unwrap();
        value["stats"]["frames"] = serde_json::json!("many");
        fs::write(dir.join(manifest::MANIFEST_FILE), value.to_string()).unwrap();
        let read = read_manifest(&dir).unwrap();
        assert_eq!((read.id.as_str(), read.stats.frames), ("id-3", 0));
        assert_eq!(list_sessions(&root).len(), 1);
        fs::write(dir.join(manifest::MANIFEST_FILE), "not json").unwrap();
        assert!(read_manifest(&dir).is_err());
        // The size is measured again once the folder changes.
        let before = session_sizes(&dir, None);
        assert_eq!(session_sizes(&dir, None), before);
        // Past the file system clock's tick (about 16 ms on Windows), so the change has a time of its own.
        std::thread::sleep(Duration::from_millis(40));
        fs::write(dir.join("extra.bin"), vec![0u8; 1000]).unwrap();
        assert_eq!(session_sizes(&dir, None).total, before.total + 1000);
        // Frames added to the keyframes folder alone (the session folder itself unchanged): measured again too.
        std::thread::sleep(Duration::from_millis(40));
        fs::write(dir.join(manifest::KEYFRAMES_DIR).join(keyframe_file(0)), vec![0u8; 500]).unwrap();
        assert_eq!(session_sizes(&dir, None), crate::cleanup::Sizes { total: before.total + 1500, capture: before.capture + 500 });
        fs::remove_dir_all(&root).ok();
        assert_eq!(downscale(&[], 0, 0, 0), (0, 0, Vec::new()));
        assert_eq!(downscale(&[1, 2, 3, 4], 1, 1, 0).0, 1, "no division by a width of 0");
    }

    #[test]
    fn records_and_manifest_reach_the_disk_at_checkpoints_not_every_frame() {
        let root = std::env::temp_dir().join(format!("orbiters-bundle-checkpoint-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut w = BundleWriter::create(&root, "Checkpoints", SessionManifest::new("id-2".into(), crate::now_utc_iso(), "0.1.0")).unwrap();
        let r = FrameRecord { flags: record_flags::STRIP_OK, ..Default::default() };
        w.append_record(&r).unwrap();
        // A frame writes neither: the manifest on disk is the first one.
        assert_eq!(read_manifest(w.dir()).unwrap().stats.frames, 0);
        // A checkpoint (the recording makes one every few seconds) writes both.
        w.checkpoint().unwrap();
        let saved = read_manifest(w.dir()).unwrap();
        assert_eq!((saved.stats.frames, read_records(w.dir()).unwrap().len()), (1, 1));
        assert!(saved.stats.duration_s > 0.0);
        w.append_record(&r).unwrap();
        assert_eq!(read_manifest(w.dir()).unwrap().stats.frames, 1);
        let (dir, manifest) = w.finish().unwrap();
        assert_eq!((manifest.stats.frames, read_manifest(&dir).unwrap().stats.frames), (2, 2));
        assert!(!dir.join("manifest.json.tmp").exists());
        fs::remove_dir_all(&root).ok();
    }

    /// A deferred checkpoint writes the records and hands the manifest over, for another thread to write.
    #[test]
    fn a_deferred_checkpoint_hands_the_manifest_over() {
        let root = std::env::temp_dir().join(format!("orbiters-bundle-deferred-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut w = BundleWriter::create(&root, "Deferred", SessionManifest::new("id-4".into(), crate::now_utc_iso(), "0.1.0")).unwrap();
        w.append_record(&FrameRecord { flags: record_flags::STRIP_OK, ..Default::default() }).unwrap();
        let (path, bytes) = w.checkpoint_deferred().unwrap();
        assert_eq!(path, w.dir().join(manifest::MANIFEST_FILE));
        assert_eq!(read_records(w.dir()).unwrap().len(), 1, "the records are on disk");
        assert_eq!(read_manifest(w.dir()).unwrap().stats.frames, 0, "the manifest is not yet");
        let handed: SessionManifest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!((handed.id.as_str(), handed.stats.frames), ("id-4", 1));
        crate::write_atomic(&path, &bytes).unwrap();
        assert_eq!(read_manifest(w.dir()).unwrap().stats.frames, 1);
        drop(w);
        fs::remove_dir_all(&root).ok();
    }
}
