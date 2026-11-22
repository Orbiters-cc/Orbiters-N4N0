//! Every notice the app showed (a toast lasts seconds): kept in `notices.log` of the logs' folder, one JSON object per
//! line, so that an error nobody had the time to read is still there in Settings, Log. The service writes, the window
//! reads; the file is cut to its newer half when it grows past [`LARGEST`].
use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::{Path, PathBuf};

const FILE: &str = "notices.log";
/// Past this size the older half goes (some ten thousand notices).
const LARGEST: u64 = 2 << 20;

/// One notice, as it was shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Seconds since 1970.
    pub at: i64,
    /// `info`, `success`, `warning`, `error`, or `detail` (what a program said behind an error, never shown as a toast).
    pub kind: String,
    pub text: String,
}

pub fn file(logs: &Path) -> PathBuf {
    logs.join(FILE)
}

/// Adds one notice to the log in `logs`. A log that cannot be written is no reason to fail what was being done.
pub fn append(logs: &Path, kind: &str, text: &str, at: i64) {
    let _ = try_append(logs, kind, text, at);
}

fn try_append(logs: &Path, kind: &str, text: &str, at: i64) -> std::io::Result<()> {
    std::fs::create_dir_all(logs)?;
    let path = file(logs);
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > LARGEST {
        let kept = std::fs::read_to_string(&path)?;
        let half = kept.len() / 2;
        // From the first whole line of the newer half.
        let from = kept[half..].find('\n').map_or(kept.len(), |i| half + i + 1);
        std::fs::write(&path, &kept.as_bytes()[from..])?;
    }
    let line = serde_json::to_string(&Entry { at, kind: kind.into(), text: text.into() }).map_err(std::io::Error::other)?;
    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    out.write_all(format!("{line}\n").as_bytes())
}

/// The newest `most` notices of the log in `logs`, newest first. Lines that are not notices are passed.
pub fn newest(logs: &Path, most: usize) -> Vec<Entry> {
    let text = std::fs::read_to_string(file(logs)).unwrap_or_default();
    text.lines().rev().filter_map(|line| serde_json::from_str(line).ok()).take(most).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_are_kept_newest_first_and_the_log_does_not_grow_for_ever() {
        let logs = std::env::temp_dir().join(format!("orbiters-notice-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&logs);
        assert!(newest(&logs, 10).is_empty(), "no log yet");
        append(&logs, "error", "VRChat could not be started: \"no\"\nsecond line", 100);
        append(&logs, "info", "Ready", 200);
        std::fs::OpenOptions::new().append(true).open(file(&logs)).unwrap().write_all(b"not a notice\n").unwrap();
        append(&logs, "warning", "Low disk", 300);
        let all = newest(&logs, 10);
        assert_eq!(all.iter().map(|e| e.at).collect::<Vec<_>>(), [300, 200, 100]);
        assert_eq!(all[2], Entry { at: 100, kind: "error".into(), text: "VRChat could not be started: \"no\"\nsecond line".into() }, "whatever the text holds, one line each");
        assert_eq!(newest(&logs, 2).len(), 2);
        // Past its largest size, the older half goes and what is left is whole lines.
        let long = "x".repeat(4096);
        for i in 0..700 {
            append(&logs, "info", &long, 1000 + i);
        }
        let size = std::fs::metadata(file(&logs)).unwrap().len();
        assert!(size <= LARGEST + 8192, "{size}");
        let kept = newest(&logs, usize::MAX);
        assert_eq!(kept[0].at, 1699, "the newest is always there");
        assert!(kept.len() > 200 && kept.len() < 700);
        assert!(kept.windows(2).all(|w| w[0].at == w[1].at + 1), "no line was cut in two");
        let _ = std::fs::remove_dir_all(&logs);
    }
}
