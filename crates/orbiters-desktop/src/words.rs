//! What the interface calls things that have another name on the Mac or on Linux.

use orbiters_i18n::mark;

/// Where deleted files wait. These are English keys: the interface says `tr(BIN)` where it draws them (each starts a phrase with its article
/// in the languages that have one).
pub const BIN: &str = if cfg!(windows) { mark("recycle bin") } else { mark("Trash") };
/// The system's default microphone, as a choice among microphones.
pub const DEFAULT_MIC: &str = if cfg!(windows) { mark("Windows default") } else { mark("System default") };
/// The operating system.
pub const SYSTEM: &str = if cfg!(windows) {
    "Windows"
} else if cfg!(target_os = "macos") {
    "macOS"
} else {
    "Linux"
};
