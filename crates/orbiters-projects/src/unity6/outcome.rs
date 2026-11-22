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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// The editor could not be started.
    DidNotStart(String),
    /// It ended right after starting (its exit status, when the system gave one).
    ExitedAtOnce(Option<String>),
    /// Still running, the import not finished according to the log.
    Importing,
    /// Unity entered Safe Mode: it holds back the project's scripts until their errors are fixed.
    SafeMode { errors: Vec<Diagnostic>, running: bool },
    /// The log lists compiler errors.
    CompilerErrors { errors: Vec<Diagnostic>, running: bool },
    /// The log says the first import finished, and lists no compiler error. `running`: Unity is still open (the SDK's
    /// Upgrade Wizard may be waiting).
    Imported { running: bool },
    /// The log does not tell.
    Undetermined(Unknown),
}

impl Outcome {
    /// Whether nothing more will come of this attempt (Unity is closed, or never started).
    pub fn ended(&self) -> bool {
        match self {
            Outcome::DidNotStart(_) | Outcome::ExitedAtOnce(_) => true,
            Outcome::SafeMode { running, .. } | Outcome::CompilerErrors { running, .. } | Outcome::Imported { running } => !running,
            Outcome::Importing => false,
            Outcome::Undetermined(_) => true,
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

/// What `log` (the attempt's file, `None` when there is none) says, Unity running or not.
pub fn judge(log: Option<&str>, running: bool) -> Outcome {
    let Some(text) = log else { return if running { Outcome::Importing } else { Outcome::Undetermined(Unknown::NoLog) } };
    if text.trim().is_empty() {
        return if running { Outcome::Importing } else { Outcome::Undetermined(Unknown::Empty) };
    }
    let mut errors: Vec<Diagnostic> = vec![];
    let (mut safe_mode, mut imported) = (false, false);
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
        if line.contains("InitialRefreshV2") || line.contains("Initial Refresh End") {
            imported = true;
        }
    }
    if safe_mode {
        Outcome::SafeMode { errors, running }
    } else if !errors.is_empty() {
        Outcome::CompilerErrors { errors, running }
    } else if imported {
        Outcome::Imported { running }
    } else if running {
        Outcome::Importing
    } else {
        Outcome::Undetermined(Unknown::EndedEarly)
    }
}

/// [`judge`] on the file at `path`.
pub fn read(path: &std::path::Path, running: bool) -> Outcome {
    match std::fs::read(path) {
        Ok(bytes) => judge(Some(&String::from_utf8_lossy(&bytes)), running),
        Err(_) => judge(None, running),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: &str = "Built from '6000.0/staging' branch; Version is '6000.0.67f1 (78a1c2bbeb6a)'\n[Package Manager] Done resolving packages\n";
    const IMPORT: &str = "Asset Pipeline Refresh (id=1a2b): Total: 812.403 seconds - Initiated by InitialRefreshV2(ForceSynchronousImport)\n";
    const ERROR: &str = "Assets/Scripts/Door.cs(12,5): error CS0246: The type or namespace name 'Foo' could not be found\n";

    #[test]
    fn each_outcome_is_what_the_log_and_the_process_show() {
        assert_eq!(judge(None, false), Outcome::Undetermined(Unknown::NoLog));
        assert_eq!(judge(Some(""), false), Outcome::Undetermined(Unknown::Empty));
        assert_eq!(judge(Some(START), true), Outcome::Importing);
        // Ended with a log cut short: nothing is claimed.
        assert_eq!(judge(Some(START), false), Outcome::Undetermined(Unknown::EndedEarly));
        assert_eq!(judge(Some(&format!("{START}{IMPORT}")), true), Outcome::Imported { running: true });
        assert_eq!(judge(Some(&format!("{START}{IMPORT}")), false), Outcome::Imported { running: false });
        let errors = judge(Some(&format!("{START}{ERROR}{ERROR}{IMPORT}")), true);
        let Outcome::CompilerErrors { errors, running: true } = errors else { panic!("{errors:?}") };
        assert_eq!(errors, vec![Diagnostic { file: "Assets/Scripts/Door.cs".into(), line: 12, code: "CS0246".into(), message: "The type or namespace name 'Foo' could not be found".into() }]);
        let safe = judge(Some(&format!("{START}{ERROR}Entering Safe Mode\n")), true);
        assert!(matches!(safe, Outcome::SafeMode { ref errors, running: true } if errors.len() == 1), "{safe:?}");
        // A warning or a line that only looks like an error is not one.
        assert_eq!(judge(Some(&format!("{START}Assets/A.cs(1,1): warning CS0618: obsolete\nerror: something (not a compiler line)\n{IMPORT}")), false), Outcome::Imported { running: false });
        assert!(Outcome::Imported { running: false }.ended() && !Outcome::Importing.ended());
    }

    #[test]
    fn a_log_file_missing_or_truncated_cannot_tell() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("attempt-1.log");
        assert_eq!(read(&log, false), Outcome::Undetermined(Unknown::NoLog));
        std::fs::write(&log, &START[..20]).unwrap();
        assert_eq!(read(&log, false), Outcome::Undetermined(Unknown::EndedEarly));
        std::fs::write(&log, [0xffu8, 0xfe, 0x00]).unwrap();
        assert_eq!(read(&log, false), Outcome::Undetermined(Unknown::EndedEarly));
    }
}
