//! What one opening of a trial in Unity 6 showed, read from that attempt's own log (`-logFile`, a new file per attempt
//! in app data, so another editor's start never replaces it). Each outcome is what the log and the process show; a
//! quiet log proves nothing, so "opened clean" is never said: a finished import with no compiler error in this log is
//! "imported", and Unity still open means the SDK's Upgrade Wizard may be waiting for the person. A log missing, empty
//! or ended before the import finished is "cannot tell".

use serde::{Deserialize, Serialize};

/// A compiler error, by where it is (a location, not a cause: the package that holds the file did not necessarily
/// break it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub file: String,
    pub line: u32,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unknown {
    /// No log was written.
    NoLog,
    /// The log is empty.
    Empty,
    /// Unity ended before the log said the import finished.
    EndedEarly,
    /// Unity runs, and its log says nothing this reads yet.
    NotYet,
    /// The log is there and could not be read.
    Unreadable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// The editor could not be started.
    DidNotStart(String),
    /// It ended right after starting (its exit status, when the system gave one).
    ExitedAtOnce(Option<String>),
    /// Still running, the log showing the first import begun and not finished.
    Importing,
    /// Unity entered Safe Mode: it holds back the project's scripts until their errors are fixed.
    SafeMode { errors: Vec<Diagnostic>, running: bool },
    /// The log lists compiler errors.
    CompilerErrors { errors: Vec<Diagnostic>, running: bool },
    /// The log says the first import finished, and lists no compiler error. `running`: Unity is still open (the SDK's
    /// Upgrade Wizard may be waiting). `cut`: only the log's end was read, so an error before it would not be seen.
    Imported {
        running: bool,
        #[serde(default)]
        cut: bool,
    },
    /// The log does not tell; `running`: Unity's process still runs (a separate fact).
    Undetermined { why: Unknown, running: bool },
}

impl Outcome {
    /// Whether nothing more will come of this attempt (Unity is closed, or never started).
    pub fn ended(&self) -> bool {
        match self {
            Outcome::DidNotStart(_) | Outcome::ExitedAtOnce(_) => true,
            Outcome::SafeMode { running, .. } | Outcome::CompilerErrors { running, .. } | Outcome::Imported { running, .. } => !running,
            Outcome::Importing => false,
            Outcome::Undetermined { running, .. } => !running,
        }
    }
}

/// A compiler error line: `Assets/Scripts/Door.cs(12,5): error CS0246: The type ...`.
fn diagnostic(line: &str) -> Option<Diagnostic> {
    let line = line.trim();
    let (head, rest) = line.split_once("): error ")?;
    let (file, at) = head.rsplit_once('(')?;
    let line_no = at.split(',').next()?.trim().parse().ok()?;
    let (code, message) = rest.split_once(':')?;
    let code = code.trim();
    (code.starts_with("CS") && file.to_ascii_lowercase().ends_with(".cs")).then(|| Diagnostic { file: file.trim().to_string(), line: line_no, code: code.to_string(), message: message.trim().to_string() })
}

/// The most compiler errors kept from one log.
const KEEP: usize = 50;

/// The line Unity writes when its first import has finished, the whole of it and nothing else:
/// `Asset Pipeline Refresh (id=<hex>): Total: <seconds> seconds - Initiated by InitialRefreshV2(<options>)`.
fn import_finished(line: &str) -> bool {
    let parse = || -> Option<()> {
        let rest = line.trim().strip_prefix("Asset Pipeline Refresh (id=")?;
        let (id, rest) = rest.split_once("): Total: ")?;
        let (seconds, rest) = rest.split_once(" seconds - Initiated by InitialRefreshV2(")?;
        let options = rest.strip_suffix(')')?;
        let hex = !id.is_empty() && id.bytes().all(|b| b.is_ascii_hexdigit());
        let named = options.bytes().all(|b| b.is_ascii_alphanumeric() || b" |,_".contains(&b));
        (hex && named && seconds.parse::<f64>().is_ok_and(|s| s.is_finite() && s >= 0.0)).then_some(())
    };
    parse().is_some()
}

/// What `log` (the attempt's file, `None` when there is none) says, Unity running or not.
pub fn judge(log: Option<&str>, running: bool) -> Outcome {
    judge_part(log, running, false)
}

/// [`judge`] of a log's end (`cut`: what came before it was not read).
fn judge_part(log: Option<&str>, running: bool, cut: bool) -> Outcome {
    let Some(text) = log else { return Outcome::Undetermined { why: Unknown::NoLog, running } };
    if text.trim().is_empty() {
        return Outcome::Undetermined { why: Unknown::Empty, running };
    }
    let mut errors: Vec<Diagnostic> = vec![];
    let (mut safe_mode, mut imported, mut began) = (false, false, false);
    for line in text.lines() {
        if let Some(d) = diagnostic(line) {
            if errors.len() < KEEP && !errors.contains(&d) {
                errors.push(d);
            }
        }
        let lower = line.to_ascii_lowercase();
        if lower.contains("entering safe mode") || lower.contains("[safe mode]") || lower.contains("safe mode is active") {
            safe_mode = true;
        }
        if import_finished(line) {
            imported = true;
        } else if line.contains("InitialRefreshV2") {
            began = true;
        }
    }
    if safe_mode {
        Outcome::SafeMode { errors, running }
    } else if !errors.is_empty() {
        Outcome::CompilerErrors { errors, running }
    } else if imported {
        Outcome::Imported { running, cut }
    } else if running && began {
        Outcome::Importing
    } else if running {
        Outcome::Undetermined { why: Unknown::NotYet, running }
    } else {
        Outcome::Undetermined { why: Unknown::EndedEarly, running }
    }
}

/// The most of a log read: its end (where the outcome is), so a huge log costs no more than this.
const TAIL: u64 = 8 * 1024 * 1024;

/// [`judge`] on the end of the file at `path`.
pub fn read(path: &std::path::Path, running: bool) -> Outcome {
    use std::io::{Read as _, Seek as _};
    let Ok(mut file) = std::fs::File::open(path) else { return judge(None, running) };
    let mut bytes = vec![];
    let got = (|| -> std::io::Result<bool> {
        let len = file.metadata()?.len();
        let from = len.saturating_sub(TAIL);
        file.seek(std::io::SeekFrom::Start(from))?;
        file.take(TAIL).read_to_end(&mut bytes)?;
        Ok(from > 0)
    })();
    match got {
        Ok(cut) => judge_part(Some(&String::from_utf8_lossy(&bytes)), running, cut),
        // Part of it could not be read: what it says is not known.
        Err(_) => Outcome::Undetermined { why: Unknown::Unreadable, running },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: &str = "Built from '6000.0/staging' branch; Version is '6000.0.67f1 (78a1c2bbeb6a)'\n[Package Manager] Done resolving packages\n";
    const IMPORT: &str = "Asset Pipeline Refresh (id=1a2bc3d4): Total: 812.403 seconds - Initiated by InitialRefreshV2(ForceSynchronousImport)\n";
    const ERROR: &str = "Assets/Scripts/Door.cs(12,5): error CS0246: The type or namespace name 'Foo' could not be found\n";

    #[test]
    fn each_outcome_is_what_the_log_and_the_process_show() {
        assert_eq!(judge(None, false), Outcome::Undetermined { why: Unknown::NoLog, running: false });
        assert_eq!(judge(Some(""), false), Outcome::Undetermined { why: Unknown::Empty, running: false });
        // Verification major 4: running with no log, or a log saying nothing read here, is not "importing".
        assert_eq!(judge(None, true), Outcome::Undetermined { why: Unknown::NoLog, running: true });
        assert_eq!(judge(Some(START), true), Outcome::Undetermined { why: Unknown::NotYet, running: true });
        // Second verification major 4: the line's words in another order, or a line that only holds them, are not its end.
        for not in [
            "InitialRefreshV2 cancelled: Asset Pipeline Refresh failed; Total: unknown",
            "Asset Pipeline Refresh (id=ab): Total: many seconds - Initiated by InitialRefreshV2(X)",
            "Asset Pipeline Refresh (id=ab): Total: 1.5 seconds - Initiated by StopAssetImportingV2(X)",
            "Asset Pipeline Refresh (id=): Total: 1.5 seconds - Initiated by InitialRefreshV2(X)",
            "Asset Pipeline Refresh (id=ab): Total: NaN seconds - Initiated by InitialRefreshV2(X)",
            "x Asset Pipeline Refresh (id=ab): Total: 1.5 seconds - Initiated by InitialRefreshV2(X)",
        ] {
            assert!(!matches!(judge(Some(not), false), Outcome::Imported { .. }), "{not}");
        }
        // A mention of the operation is not its end.
        assert_eq!(judge(Some(&format!("{START}InitialRefreshV2 started\n")), false), Outcome::Undetermined { why: Unknown::EndedEarly, running: false });
        assert_eq!(judge(Some(&format!("{START}InitialRefreshV2 started\n")), true), Outcome::Importing);
        // Ended with a log cut short: nothing is claimed.
        assert_eq!(judge(Some(START), false), Outcome::Undetermined { why: Unknown::EndedEarly, running: false });
        assert_eq!(judge(Some(&format!("{START}{IMPORT}")), true), Outcome::Imported { running: true, cut: false });
        assert_eq!(judge(Some(&format!("{START}{IMPORT}")), false), Outcome::Imported { running: false, cut: false });
        let errors = judge(Some(&format!("{START}{ERROR}{ERROR}{IMPORT}")), true);
        let Outcome::CompilerErrors { errors, running: true } = errors else { panic!("{errors:?}") };
        assert_eq!(errors, vec![Diagnostic { file: "Assets/Scripts/Door.cs".into(), line: 12, code: "CS0246".into(), message: "The type or namespace name 'Foo' could not be found".into() }]);
        let safe = judge(Some(&format!("{START}{ERROR}Entering Safe Mode\n")), true);
        assert!(matches!(safe, Outcome::SafeMode { ref errors, running: true } if errors.len() == 1), "{safe:?}");
        // A warning or a line that only looks like an error is not one.
        assert_eq!(judge(Some(&format!("{START}Assets/A.cs(1,1): warning CS0618: obsolete\nerror: something (not a compiler line)\n{IMPORT}")), false), Outcome::Imported { running: false, cut: false });
        assert!(Outcome::Imported { running: false, cut: false }.ended() && !Outcome::Importing.ended());
    }

    #[test]
    fn a_log_file_missing_or_truncated_cannot_tell() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("attempt-1.log");
        assert_eq!(read(&log, false), Outcome::Undetermined { why: Unknown::NoLog, running: false });
        std::fs::write(&log, &START[..20]).unwrap();
        assert_eq!(read(&log, false), Outcome::Undetermined { why: Unknown::EndedEarly, running: false });
        std::fs::write(&log, [0xffu8, 0xfe, 0x00]).unwrap();
        assert_eq!(read(&log, false), Outcome::Undetermined { why: Unknown::EndedEarly, running: false });
    }
}
