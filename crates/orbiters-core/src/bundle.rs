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
use std::time::{Duration, Instant, SystemTime};

/// How often a recording's frame records and manifest reach the disk ([`BundleWriter::checkpoint`]).
pub const CHECKPOINT_EVERY: Duration = Duration::from_secs(5);

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
/// the paths this writer hands out; records are appended in frame order here.
pub struct BundleWriter {
    dir: PathBuf,
    manifest: SessionManifest,
    frames: BufWriter<File>,
    next_keyframe: i32,
    next_depth: i32,
    started: Instant,
    last_manifest_write: Instant,
    /// Duration measured in game time instead of wall time (sources replayed faster than real time).
    game_time_duration: bool,
}

impl BundleWriter {
    pub fn create(parent: &Path, folder_name: &str, manifest: SessionManifest) -> anyhow::Result<Self> {
        let dir = unique_dir(parent, &crate::safe_file_name(folder_name))?;
        fs::create_dir_all(dir.join(manifest::KEYFRAMES_DIR))?;
        fs::create_dir_all(dir.join(manifest::DEPTH_DIR))?;
        let frames = BufWriter::with_capacity(1 << 16, File::create(dir.join(manifest::FRAMES_FILE))?);
        let writer = Self {
            dir,
            manifest,
            frames,
            next_keyframe: 0,
            next_depth: 0,
            started: Instant::now(),
            last_manifest_write: Instant::now(),
            game_time_duration: false,
        };
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
        if self.last_manifest_write.elapsed() >= CHECKPOINT_EVERY {
            self.checkpoint()?;
        }
        Ok(())
    }

    /// Writes the frame records and the manifest as they stand (duration so far), so a session that never
    /// finishes (the worker killed or crashed, Windows shut down) reads up to here. Tried again after
    /// [`CHECKPOINT_EVERY`] when it fails.
    pub fn checkpoint(&mut self) -> anyhow::Result<()> {
        self.last_manifest_write = Instant::now();
        self.frames.flush()?;
        self.measure_duration();
        self.write_manifest()
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
        let (tw, th, small) = downscale(rgba, width, height, 480);
        fs::write(self.dir.join(manifest::THUMBNAIL_FILE), qoi::encode(&small, tw, th, 3))?;
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

    pub fn finish(mut self) -> anyhow::Result<(PathBuf, SessionManifest)> {
        self.frames.flush()?;
        self.measure_duration();
        self.write_manifest()?;
        Ok((self.dir, self.manifest))
    }
}

/// Box-filter downscale of RGBA to at most `max_width` wide; returns RGB.
pub fn downscale(rgba: &[u8], width: u32, height: u32, max_width: u32) -> (u32, u32, Vec<u8>) {
    let factor = width.div_ceil(max_width).max(1);
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
    /// Seconds since the Unix epoch of the manifest's last change.
    pub modified: u64,
    /// The latest finished reconstruction.
    pub reconstruction: Option<crate::reconstruction::Report>,
}

pub fn read_manifest(dir: &Path) -> anyhow::Result<SessionManifest> {
    let text = fs::read_to_string(dir.join(manifest::MANIFEST_FILE))?;
    Ok(serde_json::from_str(&text)?)
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

/// Sessions under `root`, newest first.
pub fn list_sessions(root: &Path) -> Vec<SessionSummary> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(root) else { return out };
    for e in entries.flatten() {
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(manifest) = read_manifest(&dir) else { continue };
        if manifest.format != manifest::MANIFEST_FORMAT {
            continue;
        }
        let modified = fs::metadata(dir.join(manifest::MANIFEST_FILE))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push(SessionSummary {
            has_thumbnail: dir.join(manifest::THUMBNAIL_FILE).is_file(),
            size_bytes: folder_size(&dir),
            reconstruction: crate::reconstruction::read_report(&dir),
            dir,
            manifest,
            modified,
        });
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.modified));
    out
}

/// All frame records of a bundle.
pub fn read_records(dir: &Path) -> anyhow::Result<Vec<FrameRecord>> {
    let bytes = fs::read(dir.join(manifest::FRAMES_FILE))?;
    bytes.as_chunks::<RECORD_SIZE>().0.iter().map(|c| FrameRecord::from_bytes(c).map_err(anyhow::Error::from)).collect()
}

/// The session thumbnail as top-down RGBA: (width, height, pixels).
pub fn read_thumbnail(dir: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let data = fs::read(dir.join(manifest::THUMBNAIL_FILE)).ok()?;
    let (h, rgba) = qoi::decode_rgba(&data).ok()?;
    Some((h.width, h.height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::manifest::SessionManifest;

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
        // A second session with the same name gets a numbered folder.
        let w2 = BundleWriter::create(&root, "World: test", SessionManifest::default()).unwrap();
        assert!(w2.dir().ends_with("World_ test (2)"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn records_and_manifest_reach_the_disk_every_few_seconds_not_every_frame() {
        let root = std::env::temp_dir().join(format!("orbiters-bundle-checkpoint-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut w = BundleWriter::create(&root, "Checkpoints", SessionManifest::new("id-2".into(), crate::now_utc_iso(), "0.1.0")).unwrap();
        let r = FrameRecord { flags: record_flags::STRIP_OK, ..Default::default() };
        w.append_record(&r).unwrap();
        // Not due yet: the manifest on disk is the first one.
        assert_eq!(read_manifest(w.dir()).unwrap().stats.frames, 0);
        // Due: records and manifest are written once, and not again on the next frame.
        w.last_manifest_write = Instant::now().checked_sub(CHECKPOINT_EVERY).unwrap();
        w.append_record(&r).unwrap();
        let saved = read_manifest(w.dir()).unwrap();
        assert_eq!((saved.stats.frames, read_records(w.dir()).unwrap().len()), (2, 2));
        assert!(saved.stats.duration_s > 0.0);
        w.append_record(&r).unwrap();
        assert_eq!(read_manifest(w.dir()).unwrap().stats.frames, 2);
        let (dir, manifest) = w.finish().unwrap();
        assert_eq!((manifest.stats.frames, read_manifest(&dir).unwrap().stats.frames), (3, 3));
        assert!(!dir.join("manifest.json.tmp").exists());
        fs::remove_dir_all(&root).ok();
    }
}
