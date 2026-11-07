//! What the interface calls things that have another name on the Mac.

use orbiters_i18n::mark;

/// Where deleted files wait. These are English keys: the interface says `tr(BIN)` where it draws them (each starts a phrase with its article
/// in the languages that have one).
pub const BIN: &str = if cfg!(target_os = "macos") { mark("Trash") } else { mark("recycle bin") };
/// The system's default microphone, as a choice among microphones.
pub const DEFAULT_MIC: &str = if cfg!(target_os = "macos") { mark("System default") } else { mark("Windows default") };
/// The operating system.
pub const SYSTEM: &str = if cfg!(target_os = "macos") { "macOS" } else { "Windows" };
