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
/// the paths this writer hands out; records are appended in frame order here.
pub struct BundleWriter {
    dir: PathBuf,
    manifest: SessionManifest,
    frames: BufWriter<File>,
    next_keyframe: i32,
    next_depth: i32,
    started: Instant,
    last_manifest_write: Instant,
}

impl BundleWriter {
    pub fn create(parent: &Path, folder_name: &str, manifest: SessionManifest) -> anyhow::Result<Self> {
        let dir = unique_dir(parent, &crate::safe_file_name(folder_name))?;
        fs::create_dir_all(dir.join(manifest::KEYFRAMES_DIR))?;
        fs::create_dir_all(dir.join(manifest::DEPTH_DIR))?;
        let frames = BufWriter::with_capacity(1 << 16, File::create(dir.join(manifest::FRAMES_FILE))?);
        let writer = Self { dir, manifest, frames, next_keyframe: 0, next_depth: 0, started: Instant::now(), last_manifest_write: Instant::now() };
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
        if self.last_manifest_write.elapsed().as_secs() >= 5 {
            self.frames.flush()?;
            self.write_manifest()?;
        }
        Ok(())
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
        let path = self.dir.join(manifest::MANIFEST_FILE);
        let tmp = self.dir.join("manifest.json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(&self.manifest)?)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn finish(mut self) -> anyhow::Result<(PathBuf, SessionManifest)> {
        self.frames.flush()?;
        self.manifest.stats.duration_s = self.started.elapsed().as_secs_f64();
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
            dir,
            manifest,
            modified,
        });
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

/// All frame records of a bundle.
pub fn read_records(dir: &Path) -> anyhow::Result<Vec<FrameRecord>> {
    let bytes = fs::read(dir.join(manifest::FRAMES_FILE))?;
    bytes.chunks_exact(RECORD_SIZE).map(|c| FrameRecord::from_bytes(c).map_err(anyhow::Error::from)).collect()
}

/// The thumbnail as a BMP file (what a web view can show directly), base64-encoded.
pub fn thumbnail_bmp_base64(dir: &Path) -> Option<String> {
    let data = fs::read(dir.join(manifest::THUMBNAIL_FILE)).ok()?;
    let (h, rgba) = qoi::decode_rgba(&data).ok()?;
    Some(base64(&bmp(&rgba, h.width, h.height)))
}

/// 24-bit BMP of a top-down RGBA image.
pub fn bmp(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let row = (width * 3).div_ceil(4) * 4;
    let size = 54 + row * height;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]);
    for y in (0..height).rev() {
        let start = out.len();
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            out.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i]]);
        }
        out.resize(start + row as usize, 0);
    }
    out
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
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
        assert!(thumbnail_bmp_base64(&dir).unwrap().starts_with("Qk")); // "BM"
        // A second session with the same name gets a numbered folder.
        let w2 = BundleWriter::create(&root, "World: test", SessionManifest::default()).unwrap();
        assert!(w2.dir().ends_with("World_ test (2)"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn base64_known() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }
}
