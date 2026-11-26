//! Panics written down. Release builds have no console (`windows_subsystem = "windows"`), so a thread that panics in
//! the tray service, the window or the overlay would die without a trace while the rest runs on half working. With
//! [`install`] called first thing in `main`, every process of the app notes each panic, on any thread, in
//! `%LOCALAPPDATA%/Orbiters/logs` ([`crate::paths::logs_dir`]): when, which process and thread, the message, where,
//! and the backtrace. The standard hook still runs after it (stderr, which a worker's parent reads as its log).

use std::io::Write;
use std::path::{Path, PathBuf};

/// A log is set aside as `<name>.old.log` once it grows past this, so a process's notes take at most twice it.
const MAX_LOG: u64 = 1 << 20;

/// Notes every panic of this process in [`crate::paths::logs_dir`], then lets the standard hook report it as usual.
pub fn install() {
    install_in(crate::paths::logs_dir());
}

/// [`install`], the notes going to `dir` (tests).
pub fn install_in(dir: PathBuf) {
    let process = process_name();
    let role = std::env::args().nth(1).filter(|a| a.starts_with("--") || !a.contains(['/', '\\']));
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let message = info.payload_as_str().unwrap_or("(a value that is not text)");
        let location = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_else(|| "an unknown place".into()); // i18n: not shown (a log line)
        let backtrace = std::backtrace::Backtrace::force_capture();
        crate::app_log::panicked(info.location());
        let who = match &role {
            Some(role) => format!("{process} {role}"),
            None => process.clone(),
        };
        let note = note(&crate::now_utc_iso(), &who, thread.name().unwrap_or("unnamed"), message, &location, &backtrace.to_string());
        if let Err(e) = append(&dir, &process, &note) {
            eprintln!("the panic below could not be noted in {}: {e}", dir.display());
        }
        previous(info);
    }));
}

/// The executable's name without its extension (`orbiters-desktop`): one log per program.
fn process_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .map(|name| crate::safe_file_name(&name))
        .unwrap_or_else(|| "orbiters".into())
}

/// One panic as it is noted.
fn note(at: &str, who: &str, thread: &str, message: &str, location: &str, backtrace: &str) -> String {
    format!("{at} {who} (process {}), thread '{thread}' panicked at {location}:\n{message}\n{backtrace}\n\n", std::process::id())
}

/// Adds `note` to `dir/<process>.log`, set aside first as `<process>.old.log` (replacing the one before) when it grew
/// past [`MAX_LOG`]. One write per note, so the notes of the window, the overlay and the service, which share the
/// program and its log, never mix.
fn append(dir: &Path, process: &str, note: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{process}.log"));
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG) {
        let _ = std::fs::rename(&path, dir.join(format!("{process}.old.log")));
    }
    std::fs::OpenOptions::new().create(true).append(true).open(&path)?.write_all(note.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-panics-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn notes_go_to_a_log_set_aside_when_it_grows() {
        let dir = temp("rotate");
        let first = note("2026-10-21T12:00:00Z", "orbiters-desktop --window", "osc-receive", "boom", "src/x.rs:1:2", "0: main");
        assert!(first.starts_with("2026-10-21T12:00:00Z orbiters-desktop --window (process "), "{first}");
        assert!(first.contains("thread 'osc-receive' panicked at src/x.rs:1:2:\nboom\n0: main"), "{first}");
        append(&dir, "orbiters-desktop", &first).unwrap();
        append(&dir, "orbiters-desktop", &first).unwrap();
        let log = dir.join("orbiters-desktop.log");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), format!("{first}{first}"));
        // Past the limit, the log starts again and the full one is kept beside, once.
        std::fs::write(&log, vec![b'x'; MAX_LOG as usize + 1]).unwrap();
        append(&dir, "orbiters-desktop", &first).unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), first);
        assert_eq!(std::fs::metadata(dir.join("orbiters-desktop.old.log")).unwrap().len(), MAX_LOG + 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_thread_that_panics_leaves_a_note() {
        let dir = temp("hook");
        install_in(dir.clone());
        let thread = std::thread::Builder::new().name("noted".into()).spawn(|| panic!("the test panics on purpose")).unwrap();
        assert!(thread.join().is_err());
        // Back to the standard hook for the other tests.
        let _ = std::panic::take_hook();
        let log = std::fs::read_dir(&dir).unwrap().flatten().find(|e| e.file_name().to_string_lossy().ends_with(".log")).expect("a log").path();
        let text = std::fs::read_to_string(log).unwrap();
        assert!(text.contains("thread 'noted' panicked at ") && text.contains("panic_log.rs"), "{text}");
        assert!(text.contains("the test panics on purpose"), "{text}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
