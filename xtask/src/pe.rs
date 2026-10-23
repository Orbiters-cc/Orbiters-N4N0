//! What a Windows program or library loads when it starts: the DLLs its import and delay-import tables name. `dist`
//! checks that each shipped file needs only Windows' own DLLs and the ones shipped beside it, so that a clean
//! Windows (without Visual C++'s runtime installed) runs the app and its setup.

use anyhow::Context as _;
use std::path::{Path, PathBuf};

/// The DLLs `path` names in its import tables.
pub fn imports(path: &Path) -> anyhow::Result<Vec<String>> {
    let d = std::fs::read(path).with_context(|| format!("{}", path.display()))?;
    let u16_at = |o: usize| d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).context("not a whole PE file");
    let u32_at = |o: usize| d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).context("not a whole PE file");
    let pe = u32_at(0x3c)? as usize;
    anyhow::ensure!(d.get(pe..pe + 4) == Some(b"PE\0\0".as_slice()), "{} is not a Windows program or library", path.display());
    let sections = u16_at(pe + 6)? as usize;
    let optional = pe + 24;
    let table = optional + u16_at(pe + 20)? as usize;
    let directories = optional + if u16_at(optional)? == 0x20b { 112 } else { 96 };
    let offset = |rva: u32| -> Option<usize> {
        if rva == 0 {
            return None;
        }
        (0..sections).find_map(|i| {
            let s = table + i * 40;
            let (size, va, raw_size, raw) = (u32_at(s + 8).ok()?, u32_at(s + 12).ok()?, u32_at(s + 16).ok()?, u32_at(s + 20).ok()?);
            (rva >= va && rva < va + size.max(raw_size)).then(|| (rva - va + raw) as usize)
        })
    };
    let name_at = |o: usize| -> String {
        let end = d[o..].iter().position(|&b| b == 0).map_or(d.len(), |p| o + p);
        String::from_utf8_lossy(&d[o..end]).into_owned()
    };
    let mut names = Vec::new();
    // Imports: 20-byte descriptors, the name's address at 12, ending with an empty one.
    if let Some(mut o) = offset(u32_at(directories + 8)?) {
        while let Ok(name) = u32_at(o + 12) {
            let Some(n) = offset(name) else { break };
            names.push(name_at(n));
            o += 20;
        }
    }
    // Delay imports: 32-byte descriptors, the name's address at 4.
    if let Some(mut o) = offset(u32_at(directories + 13 * 8)?) {
        while let Ok(name) = u32_at(o + 4) {
            let Some(n) = offset(name) else { break };
            names.push(name_at(n));
            o += 32;
        }
    }
    Ok(names)
}

/// Whether a DLL comes with Windows (10 and later) itself. Visual C++'s runtime does not, though a developer's
/// computer has it in System32.
pub fn from_windows(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("api-ms-win-") || lower.starts_with("ext-ms-") {
        return true;
    }
    if ["vcruntime", "msvcp", "concrt", "vccorlib", "vcomp", "ucrtbased", "mfc", "vcamp"].iter().any(|r| lower.starts_with(r)) {
        return false;
    }
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("System32");
    system.join(name).is_file()
}

/// What `files` need that is neither Windows' nor among `shipped` (DLL names): `(file, DLL)`.
pub fn missing(files: &[PathBuf], shipped: &[String]) -> anyhow::Result<Vec<(String, String)>> {
    let mut missing = Vec::new();
    for file in files {
        for dll in imports(file)? {
            if !from_windows(&dll) && !shipped.iter().any(|s| s.eq_ignore_ascii_case(&dll)) {
                missing.push((file.file_name().unwrap_or_default().to_string_lossy().into_owned(), dll));
            }
        }
    }
    Ok(missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_windows_program_names_its_dlls() {
        let notepad = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("System32").join("notepad.exe");
        if !notepad.is_file() {
            return;
        }
        let names = imports(&notepad).unwrap();
        assert!(names.iter().any(|n| n.eq_ignore_ascii_case("kernel32.dll") || n.to_ascii_lowercase().starts_with("api-ms-win-")), "{names:?}");
    }

    #[test]
    fn the_visual_cpp_runtime_is_not_windows() {
        assert!(!from_windows("VCRUNTIME140.dll"));
        assert!(!from_windows("MSVCP140.dll"));
        assert!(from_windows("api-ms-win-crt-runtime-l1-1-0.dll"));
    }
}
