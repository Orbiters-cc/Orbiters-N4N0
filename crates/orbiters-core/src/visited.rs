//! The worlds this player visited and the pictures they took there.
//!
//! VRChat deletes its old logs, so the tray service keeps the visits it reads in them ([`History`], in the config
//! folder). VRChat writes the world into each screenshot (XMP in a PNG `iTXt` chunk: `vrc:WorldID`,
//! `vrc:WorldDisplayName`, `xmp:CreateDate`); [`ShotIndex`] remembers what it read per file, so a library of
//! thousands of pictures is scanned once. Nothing here needs a VRChat account.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Visits kept per world (the newest).
const MAX_VISITS: usize = 200;

/// Unix time in seconds.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorldVisits {
    pub name: Option<String>,
    /// Unix times in seconds, oldest first.
    pub visits: Vec<u64>,
}

impl WorldVisits {
    pub fn last(&self) -> Option<u64> {
        self.visits.last().copied()
    }
}

/// `visited-worlds.json`: every world seen in VRChat's logs, with when.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct History {
    pub worlds: BTreeMap<String, WorldVisits>,
    /// How far each log was read (file name → bytes up to its last complete line).
    #[serde(default)]
    logs: BTreeMap<String, u64>,
    /// The world joined last ("Joining or Creating Room" after the join names it).
    #[serde(default)]
    last: Option<String>,
    /// The name of the world being joined ("Entering Room" comes right before the join).
    #[serde(skip)]
    entering: Option<String>,
}

pub fn history_file() -> PathBuf {
    crate::paths::config_dir().join("visited-worlds.json")
}

impl History {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    /// Written whole to a temporary file, then moved over the old one (a reader never sees half a file).
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Adds a visit (the same world at the same second counts once); returns whether it was new.
    pub fn record(&mut self, world_id: &str, at: u64) -> bool {
        self.last = Some(world_id.to_string());
        let w = self.worlds.entry(world_id.to_string()).or_default();
        if w.visits.contains(&at) {
            return false;
        }
        let i = w.visits.partition_point(|v| *v < at);
        w.visits.insert(i, at);
        if w.visits.len() > MAX_VISITS {
            w.visits.remove(0);
        }
        true
    }

    fn name_last(&mut self, name: &str) -> bool {
        let Some(w) = self.last.as_ref().and_then(|id| self.worlds.get_mut(id)) else { return false };
        if w.name.as_deref() == Some(name) {
            return false;
        }
        w.name = Some(name.to_string());
        true
    }

    /// Reads what every `output_log_*.txt` in `dir` gained since the last call; returns whether anything changed.
    pub fn read_logs(&mut self, dir: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else { return false };
        let mut logs: Vec<(String, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                (name.starts_with("output_log_") && name.ends_with(".txt")).then(|| (name, e.path()))
            })
            .collect();
        // Their names hold the start time: oldest first, so the last world joined is the newest.
        logs.sort();
        let mut changed = false;
        for (name, path) in logs {
            let from = self.logs.get(&name).copied().unwrap_or(0);
            let Some((read, lines)) = read_new_lines(&path, from) else { continue };
            for line in lines.lines() {
                changed |= self.read_line(line);
            }
            if read != from {
                self.logs.insert(name, read);
                changed = true;
            }
        }
        // Forget the logs VRChat deleted.
        let before = self.logs.len();
        self.logs.retain(|name, _| dir.join(name).is_file());
        changed | (self.logs.len() != before)
    }

    // A join reads "Entering Room: <name>", "Joining wrld_…:<instance>", then "Joining or Creating Room: <name>".
    fn read_line(&mut self, line: &str) -> bool {
        match crate::vrchat_log::parse_line(line) {
            Some(crate::vrchat_log::LogEvent::Joining { world_id, .. }) => {
                let Some(at) = log_time(line) else { return false };
                let mut changed = self.record(&world_id, at);
                if let Some(name) = self.entering.take() {
                    changed |= self.name_last(&name);
                }
                changed
            }
            Some(crate::vrchat_log::LogEvent::EnteringRoom { name }) if line.contains("[Behaviour] Joining or Creating Room: ") => self.name_last(&name),
            Some(crate::vrchat_log::LogEvent::EnteringRoom { name }) => {
                self.entering = Some(name);
                false
            }
            _ => false,
        }
    }
}

/// The complete lines a file gained after byte `from`, and where they end.
fn read_new_lines(path: &Path, from: u64) -> Option<(u64, String)> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let from = if len < from { 0 } else { from };
    if len == from {
        return Some((from, String::new()));
    }
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::with_capacity((len - from).min(256 << 20) as usize);
    f.take(256 << 20).read_to_end(&mut buf).ok()?;
    let end = buf.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    buf.truncate(end);
    Some((from + end as u64, String::from_utf8_lossy(&buf).into_owned()))
}

/// The time at the start of a log line (`2026.10.18 20:23:20 …`, this computer's local time) as Unix seconds.
pub fn log_time(line: &str) -> Option<u64> {
    let b = line.as_bytes();
    if b.len() < 19 || b[4] != b'.' || b[7] != b'.' || b[10] != b' ' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| line.get(r)?.parse::<u32>().ok();
    local_unix(n(0..4)? as i32, n(5..7)? as u8, n(8..10)? as u8, n(11..13)? as u8, n(14..16)? as u8, n(17..19)? as u8)
}

fn local_unix(year: i32, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Option<u64> {
    let date = time::Date::from_calendar_date(year, time::Month::try_from(month).ok()?, day).ok()?;
    let t = time::PrimitiveDateTime::new(date, time::Time::from_hms(hour, minute, second).ok()?);
    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    u64::try_from(t.assume_offset(offset).unix_timestamp()).ok()
}

/// What a VRChat screenshot says about itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShotInfo {
    pub world_id: Option<String>,
    pub world_name: Option<String>,
    /// Unix seconds: the XMP creation date, else the time in the file name, else the file's.
    pub taken: u64,
    pub width: u32,
    pub height: u32,
}

/// Reads a PNG's size and VRChat XMP, seeking past the image data (a 4K screenshot costs a few small reads).
pub fn read_shot(path: &Path) -> Option<ShotInfo> {
    let mut f = File::open(path).ok()?;
    let mut sig = [0u8; 8];
    f.read_exact(&mut sig).ok()?;
    if &sig != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let mut info = ShotInfo::default();
    let mut xmp = None;
    loop {
        let mut head = [0u8; 8];
        if f.read_exact(&mut head).is_err() {
            break;
        }
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64;
        let kind = &head[4..8];
        if kind == b"IHDR" && len >= 8 {
            let mut d = [0u8; 8];
            f.read_exact(&mut d).ok()?;
            info.width = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
            info.height = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
            f.seek(SeekFrom::Current(len as i64 - 8 + 4)).ok()?;
        } else if kind == b"iTXt" && len < (4 << 20) {
            let mut data = vec![0u8; len as usize];
            f.read_exact(&mut data).ok()?;
            f.seek(SeekFrom::Current(4)).ok()?;
            if let Some(text) = itxt(&data, "XML:com.adobe.xmp") {
                xmp = Some(text);
            }
        } else if kind == b"IEND" {
            break;
        } else {
            f.seek(SeekFrom::Current(len as i64 + 4)).ok()?;
        }
    }
    if let Some(x) = &xmp {
        info.world_id = xml_value(x, "vrc:WorldID").filter(|id| id.starts_with("wrld_"));
        info.world_name = xml_value(x, "vrc:WorldDisplayName").filter(|n| !n.is_empty());
        info.taken = xml_value(x, "xmp:CreateDate").and_then(|d| iso_unix(&d)).unwrap_or(0);
    }
    if info.taken == 0 {
        info.taken = path.file_name().and_then(|n| name_time(&n.to_string_lossy())).unwrap_or_else(|| modified(path));
    }
    Some(info)
}

/// The text of an `iTXt` chunk with `keyword` (keyword, flag, method, language, translated keyword, text).
fn itxt(data: &[u8], keyword: &str) -> Option<String> {
    let k = data.iter().position(|b| *b == 0)?;
    if &data[..k] != keyword.as_bytes() || data.len() < k + 3 {
        return None;
    }
    let compressed = data[k + 1] != 0;
    let rest = &data[k + 3..];
    let lang = rest.iter().position(|b| *b == 0)?;
    let rest = &rest[lang + 1..];
    let translated = rest.iter().position(|b| *b == 0)?;
    let text = &rest[translated + 1..];
    if compressed {
        let mut out = String::new();
        flate2::read::ZlibDecoder::new(text).take(4 << 20).read_to_string(&mut out).ok()?;
        Some(out)
    } else {
        Some(String::from_utf8_lossy(text).into_owned())
    }
}

/// `<tag>value</tag>` in an XMP packet, unescaped.
fn xml_value(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = start + xml[start..].find("</")?;
    let v = xml[start..end].trim().replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&");
    Some(v)
}

/// An ISO 8601 date with an offset (`2026-10-18T20:37:41.2922427+02:00`) as Unix seconds.
fn iso_unix(s: &str) -> Option<u64> {
    // At most nine fractional digits are accepted: drop the fraction, seconds are enough.
    let s = match (s.find('.'), s.rfind(['+', '-', 'Z'])) {
        (Some(dot), Some(zone)) if zone > dot => format!("{}{}", &s[..dot], &s[zone..]),
        _ => s.to_string(),
    };
    let t = time::OffsetDateTime::parse(&s, &time::format_description::well_known::Rfc3339).ok()?;
    u64::try_from(t.unix_timestamp()).ok()
}

/// The local time in a screenshot's name (`VRChat_2026-10-18_20-37-40.794_3840x2160.png`).
fn name_time(name: &str) -> Option<u64> {
    let rest = name.strip_prefix("VRChat_")?;
    let d = rest.get(0..19)?;
    let b = d.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'_' || b[13] != b'-' || b[16] != b'-' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| d.get(r)?.parse::<u32>().ok();
    local_unix(n(0..4)? as i32, n(5..7)? as u8, n(8..10)? as u8, n(11..13)? as u8, n(14..16)? as u8, n(17..19)? as u8)
}

fn modified(path: &Path) -> u64 {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

/// Where VRChat saves pictures: its `picture_output_folder` setting, else `Pictures/VRChat`.
pub fn screenshot_dirs() -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Some(log) = crate::paths::vrchat_log_dir() {
        let custom = std::fs::read_to_string(log.join("config.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v.get("picture_output_folder")?.as_str().map(PathBuf::from));
        if let Some(c) = custom.filter(|c| c.is_dir()) {
            found.push(c);
        }
    }
    if let Some(p) = dirs::picture_dir().map(|p| p.join("VRChat")).filter(|p| p.is_dir()) {
        if !found.contains(&p) {
            found.push(p);
        }
    }
    found
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShotEntry {
    pub size: u64,
    pub modified: u64,
    pub info: ShotInfo,
    /// SHA-256 of the file (hex), once something needed it: Orbiters knows uploaded pictures by it.
    #[serde(default)]
    pub sha256: Option<String>,
}

/// A screenshot on disk with what it says.
#[derive(Clone, Debug, PartialEq)]
pub struct Shot {
    pub path: PathBuf,
    pub size: u64,
    pub info: ShotInfo,
    pub sha256: Option<String>,
}

/// `screenshots.json` in the cache folder: what each screenshot said, by path, re-read when its size or date changes.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ShotIndex {
    files: BTreeMap<PathBuf, ShotEntry>,
}

pub fn shot_index_file() -> PathBuf {
    crate::paths::config_dir().join("cache").join("screenshots.json")
}

impl ShotIndex {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Walks `dirs` (VRChat sorts pictures in month folders) and reads the new or changed PNGs; returns whether the
    /// index changed.
    pub fn refresh(&mut self, dirs: &[PathBuf]) -> bool {
        let mut seen = std::collections::BTreeSet::new();
        let mut changed = false;
        let mut stack: Vec<(PathBuf, usize)> = dirs.iter().map(|d| (d.clone(), 0)).collect();
        while let Some((dir, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let Ok(meta) = e.metadata() else { continue };
                let path = e.path();
                if meta.is_dir() {
                    if depth < 3 {
                        stack.push((path, depth + 1));
                    }
                    continue;
                }
                if !path.extension().is_some_and(|x| x.eq_ignore_ascii_case("png")) {
                    continue;
                }
                let size = meta.len();
                let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
                seen.insert(path.clone());
                if self.files.get(&path).is_some_and(|f| f.size == size && f.modified == modified) {
                    continue;
                }
                if let Some(info) = read_shot(&path) {
                    self.files.insert(path, ShotEntry { size, modified, info, sha256: None });
                    changed = true;
                }
            }
        }
        let before = self.files.len();
        self.files.retain(|p, _| seen.contains(p));
        changed | (self.files.len() != before)
    }

    /// Every screenshot with a world, newest first.
    pub fn shots(&self) -> Vec<Shot> {
        let mut out: Vec<Shot> = self
            .files
            .iter()
            .filter(|(_, e)| e.info.world_id.is_some())
            .map(|(p, e)| Shot { path: p.clone(), size: e.size, info: e.info.clone(), sha256: e.sha256.clone() })
            .collect();
        out.sort_by(|a, b| b.info.taken.cmp(&a.info.taken).then_with(|| b.path.cmp(&a.path)));
        out
    }

    /// Hashes the given screenshots that have no hash yet; returns whether any was added.
    pub fn hash(&mut self, paths: &[PathBuf]) -> bool {
        let mut changed = false;
        for p in paths {
            let Some(e) = self.files.get_mut(p) else { continue };
            if e.sha256.is_some() {
                continue;
            }
            if let Ok(h) = sha256_file(p) {
                e.sha256 = Some(h);
                changed = true;
            }
        }
        changed
    }
}

/// SHA-256 of a file as lowercase hex.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_with_xmp(xmp: &str) -> Vec<u8> {
        fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            out.extend_from_slice(&[0, 0, 0, 0]);
        }
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&3840u32.to_be_bytes());
        ihdr.extend_from_slice(&2160u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &[0u8; 64]);
        let mut itxt = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
        itxt.extend_from_slice(xmp.as_bytes());
        chunk(&mut out, b"iTXt", &itxt);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn reads_the_world_from_a_screenshot() {
        let dir = std::env::temp_dir().join(format!("orbiters-shots-{}", std::process::id()));
        let month = dir.join("2026-10");
        std::fs::create_dir_all(&month).unwrap();
        let xmp = "<x:xmpmeta><rdf:Description><xmp:CreateDate>2026-10-18T20:37:41.2922427+02:00</xmp:CreateDate>\
                   <vrc:WorldID>wrld_99123966-6b37</vrc:WorldID><vrc:WorldDisplayName>Tom &amp; Jerry</vrc:WorldDisplayName></rdf:Description></x:xmpmeta>";
        let path = month.join("VRChat_2026-10-18_20-37-40.794_3840x2160.png");
        std::fs::write(&path, png_with_xmp(xmp)).unwrap();
        std::fs::write(month.join("notes.txt"), "x").unwrap();
        let info = read_shot(&path).unwrap();
        assert_eq!((info.width, info.height), (3840, 2160));
        assert_eq!(info.world_id.as_deref(), Some("wrld_99123966-6b37"));
        assert_eq!(info.world_name.as_deref(), Some("Tom & Jerry"));
        assert_eq!(info.taken, 1_792_348_661);

        let mut index = ShotIndex::default();
        assert!(index.refresh(std::slice::from_ref(&dir)));
        assert!(!index.refresh(std::slice::from_ref(&dir)));
        assert_eq!(index.shots().len(), 1);
        assert!(index.hash(&[path.clone()]));
        assert_eq!(index.shots()[0].sha256.as_deref(), Some(sha256_file(&path).unwrap().as_str()));
        std::fs::remove_file(&path).unwrap();
        assert!(index.refresh(std::slice::from_ref(&dir)));
        assert!(index.shots().is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    /// `cargo test -p orbiters-core reads_this_computers_vrchat_history -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn reads_this_computers_vrchat_history() {
        let mut h = History::default();
        let t = std::time::Instant::now();
        h.read_logs(&crate::paths::vrchat_log_dir().expect("VRChat's log folder"));
        println!("{} worlds from the logs in {:?}", h.worlds.len(), t.elapsed());
        for (id, w) in &h.worlds {
            println!("  {id} {:?} ×{}", w.name, w.visits.len());
        }
        let t = std::time::Instant::now();
        let mut index = ShotIndex::default();
        index.refresh(&screenshot_dirs());
        let shots = index.shots();
        println!("{} screenshots with a world in {:?} ({:?})", shots.len(), t.elapsed(), screenshot_dirs());
    }

    #[test]
    fn keeps_visits_across_logs_read_in_pieces() {
        let dir = std::env::temp_dir().join(format!("orbiters-visits-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("output_log_2026-10-18_20-17-45.txt");
        std::fs::write(
            &log,
            "2026.10.18 20:23:20 Debug      -  [Behaviour] Entering Room: A\n2026.10.18 20:23:20 Debug      -  [Behaviour] Joining wrld_a:1~region(eu)\n\
             2026.10.18 20:23:20 Debug      -  [Behaviour] Joining or Creating Room: A\n2026.10.18 20:59:59 Debug      -  [Behaviour] OnLeftRoom\n\
             2026.10.18 21:00:00 Debug      -  [Behaviour] Entering Room: B\n2026.10.18 21:00:00 Debug      -  [Behaviour] Joining wrld_b:2\n\
             2026.10.18 21:00:01 Debug      -  [Behaviour] Joining or",
        )
        .unwrap();
        let mut h = History::default();
        assert!(h.read_logs(&dir));
        assert_eq!(h.worlds["wrld_a"].name.as_deref(), Some("A"));
        assert_eq!(h.worlds["wrld_a"].visits.len(), 1);
        assert_eq!(h.worlds["wrld_b"].name.as_deref(), Some("B"));
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        writeln!(f, " Creating Room: B (renamed)\n2026.10.18 22:00:00 Debug      -  [Behaviour] Entering Room: A\n2026.10.18 22:00:00 Debug      -  [Behaviour] Joining wrld_a:3").unwrap();
        drop(f);
        assert!(h.read_logs(&dir));
        assert_eq!(h.worlds["wrld_b"].name.as_deref(), Some("B (renamed)"));
        assert_eq!(h.worlds["wrld_a"].name.as_deref(), Some("A"));
        assert_eq!(h.worlds["wrld_a"].visits.len(), 2);
        assert!(!h.read_logs(&dir));
        let saved = dir.join("visited-worlds.json");
        h.save(&saved).unwrap();
        let again = History::load(&saved);
        assert_eq!(again.worlds, h.worlds);
        std::fs::remove_file(&log).unwrap();
        let mut again = again;
        assert!(again.read_logs(&dir));
        assert_eq!(again.worlds.len(), 2);
        std::fs::remove_dir_all(dir).ok();
    }
}
