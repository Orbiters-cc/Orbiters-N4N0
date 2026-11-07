//! SteamVR's settings file, written only while SteamVR is closed: the one way a change still owed can come back when there
//! is no SteamVR to ask. One key changes or goes, and every other byte of the file stays as SteamVR wrote it: the text is
//! cut and joined, never parsed and printed again, and the result is parsed to check that nothing else moved.
//!
//! Tests never come near the real file: the path is always given, and the one thing that finds the real one refuses inside
//! a test.

use crate::journal::{replace_durably, Access, Read, Target, Value};
use std::path::{Path, PathBuf};

/// Where `openvrpaths.vrpath` says SteamVR keeps a folder.
fn from_vrpath(text: &str, field: &str) -> Option<PathBuf> {
    let paths: serde_json::Value = serde_json::from_str(text).ok()?;
    Some(PathBuf::from(paths.get(field)?.as_array()?.first()?.as_str()?))
}

/// SteamVR's own list of its folders, `%LOCALAPPDATA%\openvr\openvrpaths.vrpath`.
fn vrpath_for_user() -> Option<PathBuf> {
    #[cfg(test)]
    {
        panic!("tests never look for the real SteamVR's files");
    }
    #[cfg(not(test))]
    Some(dirs::data_local_dir()?.join("openvr").join("openvrpaths.vrpath"))
}

/// The real `steamvr.vrsettings`.
pub fn settings_path_for_user() -> Option<PathBuf> {
    let text = std::fs::read_to_string(vrpath_for_user()?).ok()?;
    Some(from_vrpath(&text, "config")?.join("steamvr.vrsettings"))
}

/// The real `vrcompositor.txt`.
pub fn compositor_log_for_user() -> Option<PathBuf> {
    let text = std::fs::read_to_string(vrpath_for_user()?).ok()?;
    Some(from_vrpath(&text, "log")?.join("vrcompositor.txt"))
}

/// The value of a key in the file's text. `Absent` is a key not there, which a read through SteamVR cannot tell from a
/// default.
pub fn read_value(text: &str, section: &str, key: &str) -> Value {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) else { return Value::Absent };
    match root.get(section).and_then(|s| s.get(key)) {
        Some(serde_json::Value::Bool(b)) => Value::Bool(*b),
        Some(serde_json::Value::Number(n)) if n.is_i64() => Value::Int(n.as_i64().unwrap_or(0)),
        Some(serde_json::Value::Number(n)) => Value::Float(n.as_f64().unwrap_or(0.0) as f32),
        _ => Value::Absent,
    }
}

/// How a value is written in the file.
fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::Absent => None,
        Value::Bool(b) => Some(b.to_string()),
        Value::Int(i) => Some(i.to_string()),
        // The shortest text that reads back as this float.
        Value::Float(f) => Some(format!("{f:?}")),
    }
}

/// What walking the text found of a member of an object.
#[derive(Clone, Copy, Debug)]
struct Member {
    /// From its key's opening quote.
    start: usize,
    value_start: usize,
    value_end: usize,
}

/// A little reader of JSON's shape: just enough to find where things are, as the bytes between them must not change.
struct Walk<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Walk<'_> {
    fn spaces(&mut self) {
        while self.bytes.get(self.at).is_some_and(|b| b.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    fn string(&mut self) -> Option<(usize, usize)> {
        if self.bytes.get(self.at) != Some(&b'"') {
            return None;
        }
        let start = self.at + 1;
        let mut i = start;
        loop {
            match *self.bytes.get(i)? {
                b'\\' => i += 2,
                b'"' => {
                    self.at = i + 1;
                    return Some((start, i));
                }
                _ => i += 1,
            }
        }
    }

    fn value(&mut self) -> Option<()> {
        self.spaces();
        match *self.bytes.get(self.at)? {
            b'"' => {
                self.string()?;
            }
            b'{' | b'[' => {
                let mut depth = 0usize;
                loop {
                    match *self.bytes.get(self.at)? {
                        b'"' => {
                            self.string()?;
                            continue;
                        }
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                self.at += 1;
                                return Some(());
                            }
                        }
                        _ => {}
                    }
                    self.at += 1;
                }
            }
            _ => {
                while self.bytes.get(self.at).is_some_and(|b| !matches!(b, b',' | b'}' | b']') && !b.is_ascii_whitespace()) {
                    self.at += 1;
                }
            }
        }
        Some(())
    }

    /// The members of the object starting here, and where it closes.
    fn object(&mut self) -> Option<(Vec<(String, Member)>, usize)> {
        self.spaces();
        if self.bytes.get(self.at) != Some(&b'{') {
            return None;
        }
        self.at += 1;
        let mut members = Vec::new();
        loop {
            self.spaces();
            match *self.bytes.get(self.at)? {
                b'}' => {
                    let close = self.at;
                    self.at += 1;
                    return Some((members, close));
                }
                b',' => {
                    self.at += 1;
                    continue;
                }
                _ => {}
            }
            let start = self.at;
            let (from, to) = self.string()?;
            let key = String::from_utf8_lossy(&self.bytes[from..to]).into_owned();
            self.spaces();
            if self.bytes.get(self.at) != Some(&b':') {
                return None;
            }
            self.at += 1;
            self.spaces();
            let value_start = self.at;
            self.value()?;
            members.push((key, Member { start, value_start, value_end: self.at }));
        }
    }
}

/// Why a file was left as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    NotJson,
    /// The result would have changed more than the one key.
    WouldChangeMore,
}

/// The text with one key set, or gone (`Absent`), all else byte for byte as it was.
pub fn edit_text(text: &str, section: &str, key: &str, value: &Value) -> Result<String, EditError> {
    let bom = if text.starts_with('\u{feff}') { '\u{feff}'.len_utf8() } else { 0 };
    let bytes = text.as_bytes();
    let mut walk = Walk { bytes, at: bom };
    let (sections, top_close) = walk.object().ok_or(EditError::NotJson)?;
    let mut out = String::with_capacity(text.len() + 64);
    let new_value = text_of(value);
    match (sections.iter().find(|(k, _)| k == section), new_value) {
        (Some((_, member)), new_value) => {
            let mut inner = Walk { bytes, at: member.value_start };
            let (keys, close) = inner.object().ok_or(EditError::NotJson)?;
            match (keys.iter().position(|(k, _)| k == key), new_value) {
                (Some(i), Some(new_value)) => {
                    out.push_str(&text[..keys[i].1.value_start]);
                    out.push_str(&new_value);
                    out.push_str(&text[keys[i].1.value_end..]);
                }
                (Some(i), None) => {
                    // The member, and what ties it to its neighbours: a comma after it, or before it when it is the last.
                    let this = keys[i].1;
                    let (from, to) = match keys.get(i + 1) {
                        Some((_, next)) => (this.start, next.start),
                        None if i > 0 => (keys[i - 1].1.value_end, this.value_end),
                        None => (this.start, this.value_end),
                    };
                    out.push_str(&text[..from]);
                    out.push_str(&text[to..]);
                }
                (None, Some(new_value)) => {
                    let entry = format!("\"{key}\" : {new_value}");
                    match keys.first() {
                        // At the start, in the same indentation as the one it comes before.
                        Some((_, first)) => {
                            let open = member.value_start + 1;
                            let gap = &text[open..first.start];
                            out.push_str(&text[..first.start]);
                            out.push_str(&entry);
                            out.push(',');
                            out.push_str(gap);
                            out.push_str(&text[first.start..]);
                        }
                        None => {
                            out.push_str(&text[..close]);
                            out.push_str(&entry);
                            out.push_str(&text[close..]);
                        }
                    }
                }
                (None, None) => return Ok(text.to_string()),
            }
        }
        (None, None) => return Ok(text.to_string()),
        (None, Some(new_value)) => {
            let entry = format!("\"{section}\" : {{ \"{key}\" : {new_value} }}");
            match sections.first() {
                Some((_, first)) => {
                    out.push_str(&text[..first.start]);
                    out.push_str(&entry);
                    out.push_str(",\n   ");
                    out.push_str(&text[first.start..]);
                }
                None => {
                    out.push_str(&text[..top_close]);
                    out.push_str(&entry);
                    out.push_str(&text[top_close..]);
                }
            }
        }
    }
    // Nothing else may have moved: the old file with this one change must equal the new file, whatever the text looks like.
    let parse = |t: &str| serde_json::from_str::<serde_json::Value>(t.trim_start_matches('\u{feff}')).map_err(|_| EditError::NotJson);
    let mut expected = parse(text)?;
    let slot = expected.as_object_mut().ok_or(EditError::NotJson)?;
    match text_of(value) {
        Some(new_value) => {
            let parsed: serde_json::Value = serde_json::from_str(&new_value).map_err(|_| EditError::NotJson)?;
            slot.entry(section)
                .or_insert_with(|| serde_json::Value::Object(Default::default()))
                .as_object_mut()
                .ok_or(EditError::NotJson)?
                .insert(key.to_string(), parsed);
        }
        None => {
            if let Some(s) = slot.get_mut(section).and_then(serde_json::Value::as_object_mut) {
                s.remove(key);
            }
        }
    }
    if parse(&out)? != expected {
        return Err(EditError::WouldChangeMore);
    }
    Ok(out)
}

/// [`Access`] to the settings file itself, only while `closed` says SteamVR is not running: SteamVR keeps them in memory,
/// and would write over any change at its exit.
pub struct SettingsFile<'a> {
    pub path: PathBuf,
    pub closed: &'a dyn Fn() -> bool,
}

impl SettingsFile<'_> {
    fn text(path: &Path) -> Result<String, ()> {
        match std::fs::read(path) {
            Ok(bytes) => String::from_utf8(bytes).map_err(|_| ()),
            // SteamVR makes it when it has something to keep: no file is every key absent.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("{}\n".to_string()),
            Err(_) => Err(()),
        }
    }
}

impl Access for SettingsFile<'_> {
    fn read(&mut self, target: &Target) -> Read {
        let Target::SteamVr { section, key } = target else { return Read::Unreachable };
        if !(self.closed)() {
            return Read::Unreachable;
        }
        match Self::text(&self.path) {
            Ok(text) => Read::Value(read_value(&text, section, key)),
            Err(()) => Read::Unreachable,
        }
    }

    fn write(&mut self, target: &Target, value: &Value) -> bool {
        let Target::SteamVr { section, key } = target else { return false };
        if !(self.closed)() {
            return false;
        }
        let Ok(text) = Self::text(&self.path) else { return false };
        match edit_text(&text, section, key, value) {
            Ok(edited) if edited == text => true,
            Ok(edited) => replace_durably(&self.path, edited.as_bytes(), false, None).is_ok(),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::tests::{setting, Scratch};
    use crate::journal::{NotArmed, Store};

    /// As SteamVR writes it: three spaces, a space before each colon, numbers with many digits.
    const FILE: &str = "{\n   \"GpuSpeed\" : {\n      \"gpuSpeedRenderTargetScale\" : 1.3899999856948853,\n      \"gpuSpeedVendor\" : \"NVIDIA \\\"x\\\" {odd}\"\n   },\n   \"steamvr\" : {\n      \"disableAsync\" : true,\n      \"supersampleManualOverride\" : true,\n      \"supersampleScale\" : 1.399999976158142,\n      \"values\" : [ 1, { \"a\" : 2 } ]\n   },\n   \"last\" : {}\n}\n";

    #[test]
    fn a_value_changes_and_not_one_other_byte() {
        let got = edit_text(FILE, "steamvr", "supersampleScale", &Value::Float(1.19)).unwrap();
        assert_eq!(got, FILE.replace("1.399999976158142", "1.19"));
    }

    #[test]
    fn a_key_goes_and_not_one_other_byte() {
        let got = edit_text(FILE, "steamvr", "supersampleManualOverride", &Value::Absent).unwrap();
        assert_eq!(got, FILE.replace("      \"supersampleManualOverride\" : true,\n", ""));
        // The last one in its section takes the comma before it.
        let got = edit_text(FILE, "GpuSpeed", "gpuSpeedVendor", &Value::Absent).unwrap();
        assert_eq!(got, FILE.replace(",\n      \"gpuSpeedVendor\" : \"NVIDIA \\\"x\\\" {odd}\"", ""));
        // A key not there: the file as it is.
        assert_eq!(edit_text(FILE, "steamvr", "nothing", &Value::Absent).unwrap(), FILE);
        assert_eq!(edit_text(FILE, "nothing", "nothing", &Value::Absent).unwrap(), FILE);
    }

    #[test]
    fn a_new_key_goes_in_with_every_other_byte_kept() {
        let got = edit_text(FILE, "steamvr", "motionSmoothing", &Value::Bool(false)).unwrap();
        assert_eq!(got, FILE.replace("\"disableAsync\" : true", "\"motionSmoothing\" : false,\n      \"disableAsync\" : true"));
        assert_eq!(read_value(&got, "steamvr", "motionSmoothing"), Value::Bool(false));
        // In a section with nothing in it, and in one not there.
        let got = edit_text(FILE, "last", "k", &Value::Int(3)).unwrap();
        assert_eq!(read_value(&got, "last", "k"), Value::Int(3));
        let got = edit_text(FILE, "new", "k", &Value::Int(3)).unwrap();
        assert_eq!(read_value(&got, "new", "k"), Value::Int(3));
        assert_eq!(read_value(&got, "steamvr", "disableAsync"), Value::Bool(true));
        // And there and back is the same file.
        let there = edit_text(FILE, "steamvr", "motionSmoothing", &Value::Bool(false)).unwrap();
        assert_eq!(edit_text(&there, "steamvr", "motionSmoothing", &Value::Absent).unwrap(), FILE);
    }

    /// What ordinary parsing would lose: order, many digits, strange text, a byte order mark.
    #[test]
    fn digits_order_and_a_byte_order_mark_stay() {
        let with_bom = format!("\u{feff}{FILE}");
        let got = edit_text(&with_bom, "steamvr", "supersampleScale", &Value::Float(1.0)).unwrap();
        assert!(got.starts_with('\u{feff}'));
        assert!(got.contains("1.3899999856948853"));
        assert!(got.find("GpuSpeed").unwrap() < got.find("steamvr").unwrap());
        assert_eq!(read_value(&got, "steamvr", "supersampleScale"), Value::Float(1.0));
    }

    #[test]
    fn what_is_not_json_is_left_alone() {
        assert_eq!(edit_text("not json at all", "s", "k", &Value::Bool(true)), Err(EditError::NotJson));
        assert_eq!(edit_text("{ \"s\" : { \"k\" : tru", "s", "k", &Value::Bool(true)), Err(EditError::NotJson));
    }

    #[test]
    fn absent_is_told_from_a_value() {
        assert_eq!(read_value(FILE, "steamvr", "supersampleManualOverride"), Value::Bool(true));
        assert_eq!(read_value(FILE, "steamvr", "nope"), Value::Absent);
        assert_eq!(read_value(FILE, "nope", "nope"), Value::Absent);
    }

    #[test]
    fn the_real_file_is_found_from_steamvrs_own_list() {
        let list = r#"{ "config": ["C:\\Steam\\config"], "log": ["C:\\Steam\\logs"], "version": 1 }"#;
        assert_eq!(from_vrpath(list, "config"), Some(PathBuf::from("C:\\Steam\\config")));
        assert_eq!(from_vrpath(list, "log"), Some(PathBuf::from("C:\\Steam\\logs")));
        assert_eq!(from_vrpath("{}", "config"), None);
    }

    /// The journal against a file in the test's own folder: while SteamVR "runs" the entry waits and the file does not change;
    /// closed, the key comes back, or goes if it was absent, and every other byte is as it was.
    #[test]
    fn a_value_owed_comes_back_through_the_file_only_once_steamvr_is_closed() {
        let dir = Scratch::new("offline");
        let path = dir.0.join("steamvr.vrsettings");
        std::fs::write(&path, FILE).unwrap();
        let store = Store::new(&dir.0, Box::new(NotArmed));
        let open = || false;
        let closed = || true;
        store
            .apply(
                &mut SettingsFile { path: path.clone(), closed: &closed },
                &[
                    crate::journal::Change {
                        target: Target::SteamVr { section: "steamvr".into(), key: "supersampleScale".into() },
                        value: Value::Float(1.19),
                        leave_with: None,
                    },
                    crate::journal::Change { target: setting("motionSmoothing"), value: Value::Bool(true), leave_with: None },
                ],
                &mut |_| {},
            )
            .unwrap();
        let changed = std::fs::read_to_string(&path).unwrap();
        assert!(changed.contains("1.19") && changed.contains("motionSmoothing"));
        let waited = store.restore(&mut SettingsFile { path: path.clone(), closed: &open }, &|_| true, &mut |_| {}).unwrap();
        assert_eq!(waited.waiting.len(), 2);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), changed, "not touched while SteamVR runs");
        let back = store.restore(&mut SettingsFile { path: path.clone(), closed: &closed }, &|_| true, &mut |_| {}).unwrap();
        assert_eq!(back.back.len(), 2);
        let now = std::fs::read_to_string(&path).unwrap();
        assert!(!now.contains("motionSmoothing"), "absent again");
        assert_eq!(read_value(&now, "steamvr", "supersampleScale"), Value::Float(1.4));
        assert_eq!(now, FILE.replace("1.399999976158142", "1.4"), "every other byte as it was");
    }

    #[test]
    #[should_panic(expected = "tests never look")]
    fn a_test_cannot_find_the_real_file() {
        let _ = settings_path_for_user();
    }
}
