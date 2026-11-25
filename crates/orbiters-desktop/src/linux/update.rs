//! Putting an update in place: not yet (L12: the AppImage replaced beside itself, as `mac_update`). Nothing is downloaded until
//! then (`orbiters_core::here::Here::updates`), and Windows' setup is never run.
use std::path::Path;

pub fn start(_setup: &Path, _dir: &Path, _hidden: bool) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}
