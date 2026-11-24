//! Opening folders, files and pages with what the person's desktop has for them.
use std::path::Path;
use std::process::{Command, Stdio};

/// `xdg-open`, waited for on a thread of its own (it ends at once, and is nobody's to wait for otherwise).
fn xdg_open(what: &std::ffi::OsStr) -> anyhow::Result<()> {
    let mut child = Command::new("xdg-open").arg(what).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    let _ = std::thread::Builder::new().name("xdg-open".into()).spawn(move || child.wait());
    Ok(())
}

/// Opens a folder in the file manager, or a file with its program.
pub fn path(path: &Path) -> anyhow::Result<()> {
    xdg_open(path.as_os_str())
}

/// Opens a web page (checked to be one by the caller) in the default browser.
pub fn url(url: &str) -> anyhow::Result<()> {
    xdg_open(url.as_ref())
}

/// Shows `path` in its folder: its folder, for now (L2: `org.freedesktop.FileManager1.ShowItems` selects it there).
pub fn reveal(path: &Path) -> anyhow::Result<()> {
    self::path(path.parent().unwrap_or(path))
}
