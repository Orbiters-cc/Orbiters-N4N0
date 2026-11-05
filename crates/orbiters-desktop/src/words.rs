//! What the interface calls things that have another name on the Mac.

/// Where deleted files wait.
pub const BIN: &str = if cfg!(target_os = "macos") { "Trash" } else { "recycle bin" };
/// Where the app waits with its window closed.
pub const TRAY: &str = if cfg!(target_os = "macos") { "menu bar" } else { "tray" };
/// The system's default microphone, as a choice among microphones.
pub const DEFAULT_MIC: &str = if cfg!(target_os = "macos") { "System default" } else { "Windows default" };
/// The operating system.
pub const SYSTEM: &str = if cfg!(target_os = "macos") { "macOS" } else { "Windows" };
