//! Starting the app, in the tray, when the person signs in to Windows: a value of their own part of the registry
//! (`HKEY_CURRENT_USER\<RUN_KEY>`, no administrator rights). The setup sets it from its option, the app from its
//! settings; both read it there.

use crate::{RUN_KEY, RUN_VALUE};
use std::path::{Path, PathBuf};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegGetValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
    RRF_RT_REG_SZ,
};

/// What Windows runs: the app in the tray, without its window.
pub fn command_line(exe: &Path) -> String {
    format!("\"{}\" --hidden", exe.display())
}

/// Starts `exe` when the person signs in (`on`), or stops doing so.
pub fn set(exe: &Path, on: bool) -> anyhow::Result<()> {
    if !on {
        let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), &HSTRING::from(RUN_VALUE)) };
        anyhow::ensure!(status.is_ok() || status == ERROR_FILE_NOT_FOUND, "Windows did not let the start with Windows go ({status:?})");
        return Ok(());
    }
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut key, None).ok()?;
    }
    let wide: Vec<u16> = command_line(exe).encode_utf16().chain(std::iter::once(0)).collect();
    let bytes: Vec<u8> = wide.iter().flat_map(|c| c.to_le_bytes()).collect();
    let written = unsafe { RegSetValueExW(key, &HSTRING::from(RUN_VALUE), None, REG_SZ, Some(&bytes)) };
    unsafe {
        let _ = RegCloseKey(key);
    }
    written.ok()?;
    Ok(())
}

/// The program Windows starts when the person signs in, if it starts the app at all.
pub fn program() -> Option<PathBuf> {
    let mut buf = vec![0u16; 4096];
    let mut size = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), &HSTRING::from(RUN_VALUE), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut size))
    };
    if status.is_err() {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1).min(buf.len());
    program_of(&String::from_utf16_lossy(&buf[..len]))
}

/// The program of a command line: `"<program>" --hidden`, or a program without quotes.
fn program_of(line: &str) -> Option<PathBuf> {
    let line = line.trim();
    let program = match line.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or_default(),
        None => line.split(' ').next().unwrap_or_default(),
    };
    (!program.is_empty()).then(|| PathBuf::from(program))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_program_is_read_back_from_the_command_line() {
        let exe = Path::new(r"C:\Users\Someone\AppData\Local\Programs\Orbiters - N4N0\orbiters-desktop.exe");
        assert_eq!(program_of(&command_line(exe)).as_deref(), Some(exe));
        assert_eq!(program_of(r"C:\Tools\orbiters-desktop.exe --hidden").as_deref(), Some(Path::new(r"C:\Tools\orbiters-desktop.exe")));
        assert_eq!(program_of("  "), None);
    }
}
