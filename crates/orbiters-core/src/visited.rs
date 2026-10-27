//! The worlds this player visited and the pictures they took there.
//!
//! VRChat deletes its old logs, so the tray service keeps the visits it reads in them ([`History`], in the config
//! folder). VRChat writes the world into each screenshot (XMP in a PNG `iTXt` chunk: `vrc:WorldID`,
//! `vrc:WorldDisplayName`, `xmp:CreateDate`); [`ShotIndex`] remembers what it read per file, so a library of
//! thousands of pictures is scanned once. Nothing here needs a VRChat account.

use crate::vrchat_log::LogEvent;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Visits kept per world (the newest).
const MAX_VISITS: usize = 200;
/// Worlds kept (those visited longest ago go first): a log that names worlds without end cannot grow the file
/// without end.
const MAX_WORLDS: usize = 5000;
/// Instances kept (the newest): N4N0's Previous instances lists them.
pub const MAX_INSTANCES: usize = 100;
/// Log bytes one [`History::read_logs_following`] reads at most: a first look at days of logs is spread over several
/// calls rather than holding up the thread that makes them.
const READ_BUDGET: u64 = 64 << 20;

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

/// An instance joined (VRChat's location: `wrld_…:<instance>`), and when it was joined last.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceVisit {
    pub world_id: String,
    /// What follows the world's id: its number and its tags (`12345~private(usr_…)~region(eu)`).
    pub instance: String,
    /// Unix seconds.
    pub at: u64,
}

impl InstanceVisit {
    /// VRChat's location, as an invite names it.
    pub fn location(&self) -> String {
        format!("{}:{}", self.world_id, self.instance)
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
    /// The instances joined, each once, the one joined last at the end ([`MAX_INSTANCES`] of them).
    #[serde(default)]
    pub instances: Vec<InstanceVisit>,
    /// The instances were read from every log there was (a history from before them is read again once, which adds
    /// no visit twice).
    #[serde(default)]
    instances_read: bool,
    /// The name of the world being joined ("Entering Room" comes right before the join).
    #[serde(skip)]
    entering: Option<String>,
}

pub fn history_file() -> PathBuf {
    crate::paths::config_dir().join("visited-worlds.json")
}

impl History {
    /// Reads the visits kept. What does not fit (a world entry from a newer version, a file cut short) is left out and
    /// the rest kept ([`crate::json_lenient`]); such a file is first kept beside as `visited-worlds.json.bad-<time>`,
    /// since the next save replaces it and VRChat has deleted the logs it came from.
    pub fn load(path: &Path) -> Self {
        let text = match crate::read_text(path) {
            Ok(Some(text)) => text,
            Ok(None) => return Self::default(),
            Err(e) => {
                eprintln!("visited worlds: {}: {e}", path.display());
                crate::keep_aside(path);
                return Self::default();
            }
        };
        let (history, dropped) = crate::json_lenient::<Self>(&text);
        if !dropped.is_empty() {
            let aside = crate::keep_aside(path);
            eprintln!("visited worlds: {} left out of {}, the file kept as {aside:?}", dropped.join(", "), path.display());
        }
        history
    }

    /// Written whole ([`crate::write_atomic`]): a reader, a crash or a power cut finds the old file or the new one.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::write_atomic(path, &serde_json::to_vec(self)?)?;
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
        if self.worlds.len() > MAX_WORLDS {
            let oldest = self.worlds.iter().filter(|(id, _)| id.as_str() != world_id).min_by_key(|(_, w)| w.last()).map(|(id, _)| id.clone());
            if let Some(id) = oldest {
                self.worlds.remove(&id);
            }
        }
        true
    }

    /// Adds a join of `world_id:instance` at `at` (one entry per instance: joining it again moves it to the end);
    /// returns whether it changed anything.
    pub fn record_instance(&mut self, world_id: &str, instance: &str, at: u64) -> bool {
        if instance.is_empty() {
            return false;
        }
        if let Some(i) = self.instances.iter().position(|v| v.world_id == world_id && v.instance == instance) {
            if self.instances[i].at >= at {
                return false;
            }
            self.instances.remove(i);
        }
        let visit = InstanceVisit { world_id: world_id.to_string(), instance: instance.to_string(), at };
        let i = self.instances.partition_point(|v| v.at <= at);
        self.instances.insert(i, visit);
        if self.instances.len() > MAX_INSTANCES {
            self.instances.remove(0);
        }
        true
    }

    /// The instances joined, the newest first, with their world's name when known.
    pub fn recent_instances(&self) -> Vec<(InstanceVisit, Option<String>)> {
        self.instances.iter().rev().map(|v| (v.clone(), self.worlds.get(&v.world_id).and_then(|w| w.name.clone()))).collect()
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
        self.read_logs_following(dir, None)
    }

    /// [`History::read_logs`] next to the log follower, which is at byte `at` of the log named `following.0`: that log
    /// is read up to there only, after which the follower's reading is taken instead ([`History::follow`]) and the
    /// newest log is read once, not twice. At most [`READ_BUDGET`] bytes a call: the rest is read by the next ones.
    pub fn read_logs_following(&mut self, dir: &Path, following: Option<(&str, u64)>) -> bool {
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
        // A history from before instances were kept reads every log there is again, once, for them.
        if !self.instances_read {
            self.instances_read = true;
            self.logs.clear();
        }
        let mut changed = false;
        let mut budget = READ_BUDGET;
        for (name, path) in &logs {
            let Ok(mut file) = File::open(path) else { continue };
            let Ok(len) = file.metadata().map(|m| m.len()) else { continue };
            // A log shorter than what was read of it was replaced: read again.
            let from = self.logs.get(name).copied().filter(|&from| from <= len).unwrap_or(0);
            let end = match following {
                Some((followed, at)) if followed == name.as_str() => at.min(len),
                _ => len,
            };
            if from >= end || budget == 0 {
                continue;
            }
            let mut events = Vec::new();
            // Only joins and room names count here: other lines are let go after a look at their first bytes.
            let (read, _) = crate::vrchat_log::read_lines(&mut file, from, end, budget, |line| {
                let joins =
                    crate::vrchat_log::message(line).is_some_and(|m| m.starts_with(b"[Behaviour] Joining ") || m.starts_with(b"[Behaviour] Entering Room: "));
                if let Some(e) = joins.then(|| crate::vrchat_log::parse_bytes(line)).flatten() {
                    events.push(e);
                }
            });
            budget = budget.saturating_sub(read - from);
            for e in &events {
                changed |= self.observe(e);
            }
            if self.logs.get(name) != Some(&read) {
                self.logs.insert(name.clone(), read);
                changed = true;
            }
        }
        // Forget the logs VRChat deleted.
        let before = self.logs.len();
        self.logs.retain(|name, _| logs.iter().any(|(n, _)| n == name));
        changed | (self.logs.len() != before)
    }

    /// What the log follower read of the log named `name`, bytes `from` to `to`, with its events: taken in place of
    /// reading them again once the visits are known up to `from` (until then [`History::read_logs_following`] reads
    /// that log itself). Returns whether anything changed.
    pub fn follow(&mut self, name: &str, from: u64, to: u64, events: &[LogEvent]) -> bool {
        if to <= from || self.logs.get(name).copied().unwrap_or(0) != from {
            return false;
        }
        for e in events {
            self.observe(e);
        }
        self.logs.insert(name.to_string(), to);
        true
    }

    // A join reads "Entering Room: <name>", "Joining wrld_…:<instance>", then "Joining or Creating Room: <name>".
    fn observe(&mut self, event: &LogEvent) -> bool {
        match event {
            LogEvent::Joining { world_id, instance, at: Some(at) } => {
                let mut changed = self.record(world_id, *at);
                changed |= self.record_instance(world_id, instance, *at);
                if let Some(name) = self.entering.take() {
                    changed |= self.name_last(&name);
                }
                changed
            }
            LogEvent::JoinedRoom { name } => self.name_last(name),
            LogEvent::EnteringRoom { name } => {
                self.entering = Some(name.clone());
                false
            }
            _ => false,
        }
    }
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

/// A local date and time as Unix seconds, with the offset this computer's time zone had then: a log or a picture
/// from before a change to or from summer time is not an hour off (and a log read again counts no visit twice).
fn local_unix(year: i32, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Option<u64> {
    let date = time::Date::from_calendar_date(year, time::Month::try_from(month).ok()?, day).ok()?;
    let t = time::PrimitiveDateTime::new(date, time::Time::from_hms(hour, minute, second).ok()?);
    let now = || time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    let guess = time::UtcOffset::local_offset_at(t.assume_utc()).unwrap_or_else(|_| now());
    // The guess was the offset at that time read as UTC: next to a change, the offset at the moment it gives wins.
    let offset = time::UtcOffset::local_offset_at(t.assume_offset(guess)).unwrap_or(guess);
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
    crate::paths::cache_dir().join("screenshots.json")
}

/// Where the index was kept before caches moved out of the roaming config folder.
fn legacy_shot_index_file() -> PathBuf {
    crate::paths::config_dir().join("cache").join("screenshots.json")
}

impl ShotIndex {
    /// The index as saved; one that cannot be read is made again (it is a cache: the pictures are read again).
    pub fn load(path: &Path) -> Self {
        let legacy = (!path.exists() && path == shot_index_file()).then(legacy_shot_index_file);
        let Ok(Some(text)) = crate::read_text(legacy.as_deref().unwrap_or(path)) else { return Self::default() };
        serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text)).unwrap_or_else(|e| {
            eprintln!("screenshots index: {}: {e}; reading the pictures again", path.display());
            Self::default()
        })
    }

    /// Written whole, through a temporary file of its own ([`crate::write_atomic`]): two windows saving at once each
    /// leave a whole index.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::write_atomic(path, &serde_json::to_vec(self)?)?;
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
        assert!(index.hash(std::slice::from_ref(&path)));
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

    const A: &str = "wrld_99123966-6b37-4b5b-9f3c-0a1b2c3d4e5f";
    const B: &str = "wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b";

    #[test]
    fn keeps_visits_across_logs_read_in_pieces() {
        let dir = std::env::temp_dir().join(format!("orbiters-visits-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("output_log_2026-10-18_20-17-45.txt");
        std::fs::write(
            &log,
            format!(
                "2026.10.18 20:23:20 Debug      -  [Behaviour] Entering Room: A\n2026.10.18 20:23:20 Debug      -  [Behaviour] Joining {A}:1~region(eu)\n\
                 2026.10.18 20:23:20 Debug      -  [Behaviour] Joining or Creating Room: A\n2026.10.18 20:59:59 Debug      -  [Behaviour] OnLeftRoom\n\
                 2026.10.18 21:00:00 Debug      -  [Script] [Behaviour] Joining {B}:66\n\
                 2026.10.18 21:00:00 Debug      -  [Behaviour] Entering Room: B\n2026.10.18 21:00:00 Debug      -  [Behaviour] Joining {B}:2\n\
                 2026.10.18 21:00:01 Debug      -  [Behaviour] Joining or"
            ),
        )
        .unwrap();
        let mut h = History::default();
        assert!(h.read_logs(&dir));
        assert_eq!(h.worlds[A].name.as_deref(), Some("A"));
        assert_eq!(h.worlds[A].visits.len(), 1);
        assert_eq!(h.worlds[B].name.as_deref(), Some("B"));
        assert_eq!(h.worlds[B].visits.len(), 1, "a script's line is no visit");
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        writeln!(f, " Creating Room: B (renamed)\n2026.10.18 22:00:00 Debug      -  [Behaviour] Entering Room: A\n2026.10.18 22:00:00 Debug      -  [Behaviour] Joining {A}:3").unwrap();
        drop(f);
        assert!(h.read_logs(&dir));
        assert_eq!(h.worlds[B].name.as_deref(), Some("B (renamed)"));
        assert_eq!(h.worlds[A].name.as_deref(), Some("A"));
        assert_eq!(h.worlds[A].visits.len(), 2);
        // Each instance once, the newest first; a script's line is none.
        let instances: Vec<String> = h.recent_instances().iter().map(|(v, _)| v.location()).collect();
        assert_eq!(instances, [format!("{A}:3"), format!("{B}:2"), format!("{A}:1~region(eu)")]);
        assert_eq!(h.recent_instances()[1].1.as_deref(), Some("B (renamed)"));
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

    #[test]
    fn instances_are_kept_once_each_newest_last_and_bounded() {
        let mut h = History::default();
        assert!(h.record_instance(A, "1~region(eu)", 100));
        assert!(h.record_instance(B, "2", 200));
        assert!(!h.record_instance(A, "1~region(eu)", 100), "the same join twice");
        assert!(!h.record_instance(A, "", 300), "no instance");
        assert!(h.record_instance(A, "1~region(eu)", 300), "joined again: it moves to the end");
        assert_eq!(h.instances.iter().map(|v| v.at).collect::<Vec<_>>(), [200, 300]);
        for i in 0..MAX_INSTANCES as u64 + 5 {
            h.record_instance(B, &format!("{}", 1000 + i), 1000 + i);
        }
        assert_eq!(h.instances.len(), MAX_INSTANCES);
        assert_eq!(h.instances.last().unwrap().instance, format!("{}", 1000 + MAX_INSTANCES as u64 + 4));
    }

    #[test]
    fn a_history_from_before_instances_reads_its_logs_again_once() {
        let dir = std::env::temp_dir().join(format!("orbiters-visits-instances-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("output_log_2026-10-18_20-17-45.txt");
        let text = format!("2026.10.18 20:23:20 Debug      -  [Behaviour] Joining {A}:7~friends(usr_x)
");
        std::fs::write(&log, &text).unwrap();
        // Read up to its end by a version that kept no instances.
        let old = format!(r#"{{"worlds":{{"{A}":{{"name":"A","visits":[1]}}}},"logs":{{"output_log_2026-10-18_20-17-45.txt":{}}}}}"#, text.len());
        let mut h: History = serde_json::from_str(&old).unwrap();
        assert!(h.read_logs(&dir));
        assert_eq!(h.instances.len(), 1);
        assert_eq!(h.instances[0].location(), format!("{A}:7~friends(usr_x)"));
        assert_eq!(h.worlds[A].visits.len(), 2, "the earlier visit and the log's, each once");
        assert!(!h.read_logs(&dir), "read once");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_newest_log_is_read_once_beside_the_follower() {
        let dir = std::env::temp_dir().join(format!("orbiters-visits-follow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let name = "output_log_2026-10-18_20-17-45.txt";
        let first =
            format!("2026.10.18 20:23:20 Debug      -  [Behaviour] Joining {A}:1\n2026.10.18 20:23:20 Debug      -  [Behaviour] Joining or Creating Room: A\n");
        std::fs::write(dir.join(name), &first).unwrap();
        let mut h = History::default();
        // The follower is at the end of what is there: the visits are read up to it.
        assert!(h.read_logs_following(&dir, Some((name, first.len() as u64))));
        assert_eq!(h.worlds[A].name.as_deref(), Some("A"));
        // What it reads next is taken from it; the history does not read those bytes itself.
        let more = format!("2026.10.18 21:00:00 Debug      -  [Behaviour] Joining {B}:2\n");
        std::fs::write(dir.join(name), format!("{first}{more}")).unwrap();
        let to = (first.len() + more.len()) as u64;
        let events = vec![crate::vrchat_log::parse_line(more.trim_end()).unwrap()];
        assert!(!h.follow(name, 3, to, &events), "not where the history is");
        assert!(h.follow(name, first.len() as u64, to, &events));
        assert_eq!(h.worlds[B].visits.len(), 1);
        assert!(!h.read_logs_following(&dir, Some((name, to))), "nothing left to read");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_history_that_does_not_fit_keeps_what_does_and_the_file() {
        let dir = std::env::temp_dir().join(format!("orbiters-visits-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("visited-worlds.json");
        std::fs::write(&path, format!("\u{feff}{{\"worlds\":{{\"{A}\":{{\"name\":\"A\",\"visits\":[5]}},\"{B}\":{{\"name\":7,\"visits\":[\"x\"]}}}}}}"))
            .unwrap();
        let h = History::load(&path);
        assert_eq!(h.worlds[A].visits, vec![5]);
        assert!(!h.worlds.contains_key(B));
        let kept = std::fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("visited-worlds.json.bad-"));
        assert!(kept);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn worlds_kept_are_bounded() {
        let mut h = History::default();
        for i in 0..MAX_WORLDS + 10 {
            assert!(h.record(&format!("wrld_{i:08x}-0000-4000-8000-000000000000"), 1000 + i as u64));
        }
        assert_eq!(h.worlds.len(), MAX_WORLDS);
        assert!(!h.worlds.contains_key("wrld_00000000-0000-4000-8000-000000000000"), "the world visited longest ago went first");
    }
}
