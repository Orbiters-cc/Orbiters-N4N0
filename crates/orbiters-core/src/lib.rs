//! Shared logic of Orbiters - N4N0, used by the app and its workers.

pub mod account;
pub mod assets;
pub mod bundle;
pub mod call;
pub mod cleanup;
pub mod clock;
pub mod community;
mod dpapi;
pub mod explicit;
pub mod picture;
pub mod features;
pub mod gallery_talk;
pub mod keyframes;
pub mod lan;
pub mod maps;
pub mod osc;
pub mod panic_log;
pub mod paths;
pub mod pose;
pub mod protocol;
pub mod reconstruction;
pub mod settings;
pub mod shares;
pub mod video_history;
pub mod visited;
pub mod voice;
pub mod vrchat_accounts;
pub mod vrchat_api;
pub mod vrchat_log;
pub mod vrchat_source;
pub mod world_feedback;

pub use orbiters_contracts as contracts;

/// Ends this worker when the service that started it goes, even killed or crashed. On Windows the service's job object
/// does this; the Mac has no such thing, so a worker watches for it: its
/// parent changes when the service is gone. A worker left behind would keep the GPU or the microphone.
pub fn leave_with_parent() {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn getppid() -> i32;
        }
        // SAFETY: getppid takes nothing and cannot fail.
        let parent = unsafe { getppid() };
        let _ = std::thread::Builder::new().name("parent".into()).spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            // SAFETY: as above.
            if unsafe { getppid() } != parent {
                std::process::exit(0);
            }
        });
    }
}

/// Version of the app and its workers (they must match: the shell refuses a worker of another version).
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The TLS the HTTP clients use: on the desktop, ureq's native TLS (Windows' SChannel, certificates checked against
/// Windows' store); on Android, rustls with ring (native TLS would mean OpenSSL there), checked against Mozilla's roots
/// (webpki-roots: the platform's verifier would need the app's Java context). Each build carries its own platform's
/// alone: an agent left on another provider panics at its first `https` request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsStack {
    Native,
    Rustls,
}

/// This platform's TLS.
pub const TLS_STACK: TlsStack = if cfg!(target_os = "android") { TlsStack::Rustls } else { TlsStack::Native };

/// The TLS configuration of `stack`.
pub fn tls_config(stack: TlsStack) -> ureq::tls::TlsConfig {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    match stack {
        TlsStack::Native => TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build(),
        TlsStack::Rustls => TlsConfig::builder().provider(TlsProvider::Rustls).root_certs(RootCerts::WebPki).build(),
    }
}

/// TLS for HTTP clients, this platform's ([`TLS_STACK`]).
pub fn http_tls() -> ureq::tls::TlsConfig {
    tls_config(TLS_STACK)
}

/// An HTTP client that can reach `https` addresses ([`http_tls`]), with these timeouts.
pub fn http_agent(connect: Option<std::time::Duration>, whole: Option<std::time::Duration>) -> ureq::Agent {
    ureq::Agent::config_builder().tls_config(http_tls()).timeout_connect(connect).timeout_global(whole).build().new_agent()
}

/// Current time as ISO 8601 UTC with second precision, e.g. `2026-10-20T01:23:45Z`.
pub fn now_utc_iso() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", now.year(), now.month() as u8, now.day(), now.hour(), now.minute(), now.second())
}

/// Local date and time for folder names, e.g. `2026-10-20 01-23-45`.
pub fn now_local_for_names() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    format!("{:04}-{:02}-{:02} {:02}-{:02}-{:02}", now.year(), now.month() as u8, now.day(), now.hour(), now.minute(), now.second())
}

/// Writes a file through a temporary one beside it, flushed to the disk, then renamed over it: a reader, a crash or
/// a power cut finds the old content or the new one, never part of it (nor the zeros NTFS can leave in a renamed file
/// whose data never reached the disk). Each write has a temporary file of its own, so two writers of one file at once
/// never mix their bytes: the last rename wins whole.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_as(path, bytes, false)
}

/// `plain` protected for this user of this computer (Windows' DPAPI, with `entropy` telling this use from others; as it is
/// elsewhere, where a private file is all there is).
pub fn seal_for_user(plain: &[u8], entropy: &[u8]) -> anyhow::Result<Vec<u8>> {
    #[cfg(windows)]
    return dpapi::crypt(plain, entropy, true);
    #[cfg(not(windows))]
    {
        let _ = entropy;
        Ok(plain.to_vec())
    }
}

/// What [`seal_for_user`] protected: an error when it was not sealed for this user, or was tampered with.
pub fn open_for_user(sealed: &[u8], entropy: &[u8]) -> anyhow::Result<Vec<u8>> {
    #[cfg(windows)]
    return dpapi::crypt(sealed, entropy, false);
    #[cfg(not(windows))]
    {
        let _ = entropy;
        Ok(sealed.to_vec())
    }
}

/// [`write_atomic`] for a file only its owner may read (Unix permissions 0600; on Windows the folder's, which is the
/// user's own): a credential.
pub fn write_atomic_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_as(path, bytes, true)
}

fn write_atomic_as(path: &std::path::Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}-{}.tmp", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
    let tmp = std::path::PathBuf::from(tmp);
    let written = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(not(unix))]
        let _ = private;
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        rename_retrying(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// `rename`, tried again for a moment while another program holds the target without delete sharing (an antivirus
/// scan, a backup, a reader): Windows refuses with "access denied" or a sharing violation meanwhile.
pub fn rename_retrying(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    let mut wait = std::time::Duration::from_millis(10);
    for _ in 0..6 {
        match std::fs::rename(from, to) {
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(32) => {
                std::thread::sleep(wait);
                wait *= 2;
            }
            other => return other,
        }
    }
    std::fs::rename(from, to)
}

/// Reads a text file, `None` when there is none. A moment's lock by another program (an antivirus scan, a backup) is
/// waited out, and bytes that are not UTF-8 are read as well as they can be rather than refused.
pub fn read_text(path: &std::path::Path) -> std::io::Result<Option<String>> {
    let mut wait = std::time::Duration::from_millis(20);
    for attempt in 0..5 {
        match std::fs::read(path) {
            Ok(bytes) => return Ok(Some(String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) if attempt < 4 && (e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(32)) => {
                std::thread::sleep(wait);
                wait *= 2;
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("the last attempt returns")
}

/// Reads JSON into `T` keeping every field that fits: one that does not (out of range, a value from a newer version,
/// the wrong type) gets its default, the others keep theirs, one level of nested objects down (`n4n0.menu` alone
/// rather than all of `n4n0`). A UTF-8 byte order mark (some editors add one) is fine. Returns the value and the fields
/// left out (`"n4n0.keyboard"`; `"(not JSON)"` when nothing could be read), empty when the text fit as it was.
pub fn json_lenient<T: serde::de::DeserializeOwned + serde::Serialize + Default>(text: &str) -> (T, Vec<String>) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Ok(input) = serde_json::from_str::<serde_json::Value>(text) else { return (T::default(), vec!["(not JSON)".into()]) };
    if let Ok(value) = serde_json::from_value::<T>(input.clone()) {
        return (value, Vec::new());
    }
    let (Ok(mut base), Some(fields)) = (serde_json::to_value(T::default()), input.as_object()) else { return (T::default(), vec!["(not JSON)".into()]) };
    let fits = |candidate: &serde_json::Value| serde_json::from_value::<T>(candidate.clone()).is_ok();
    let mut dropped = Vec::new();
    for (key, value) in fields {
        let mut candidate = base.clone();
        candidate[key] = value.clone();
        if fits(&candidate) {
            base = candidate;
            continue;
        }
        match (value.as_object(), base.get(key).is_some_and(|b| b.is_object())) {
            (Some(inner), true) => {
                for (inner_key, inner_value) in inner {
                    let mut candidate = base.clone();
                    candidate[key][inner_key] = inner_value.clone();
                    if fits(&candidate) {
                        base = candidate;
                    } else {
                        dropped.push(format!("{key}.{inner_key}"));
                    }
                }
            }
            _ => dropped.push(key.clone()),
        }
    }
    (serde_json::from_value(base).unwrap_or_default(), dropped)
}

/// Keeps a copy of `path` beside it as `<name>.bad-<unix seconds>`: a file that could be read only in part, before the
/// next save replaces it.
pub fn keep_aside(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut aside = path.as_os_str().to_owned();
    aside.push(format!(".bad-{seconds}"));
    let aside = std::path::PathBuf::from(aside);
    std::fs::copy(path, &aside).ok().map(|_| aside)
}

/// Makes a string safe as a single folder name on Windows and Linux: no reserved character, at most 80 characters,
/// no trailing dot or space (Windows drops them when it creates the folder, and the name then no longer leads to it),
/// and no device name (`CON`, `NUL`, `COM1`… stay devices whatever their extension).
pub fn safe_file_name(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let limited: String = cleaned.trim().chars().take(80).collect();
    let trimmed = limited.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        return "Untitled".into(); // i18n: not shown (a file name made from nothing)
    }
    // The device is named by what comes before the first dot ("NUL.txt" and "NUL .tar.gz" are NUL too): the mark goes
    // there, not at the end.
    let first = trimmed.split('.').next().unwrap_or("");
    let stem = first.trim_end().to_ascii_uppercase();
    let numbered = (stem.starts_with("COM") || stem.starts_with("LPT")) && {
        let mut tail = stem[3..].chars();
        matches!((tail.next(), tail.next()), (Some(c), None) if c.is_ascii_digit() || "¹²³".contains(c))
    };
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered {
        let stem_end = first.trim_end().len();
        format!("{}_{}", &trimmed[..stem_end], &trimmed[stem_end..])
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_platform_has_its_own_tls() {
        use ureq::tls::{RootCerts, TlsProvider};
        assert_eq!(TLS_STACK, if cfg!(target_os = "android") { TlsStack::Rustls } else { TlsStack::Native });
        let native = tls_config(TlsStack::Native);
        assert_eq!(native.provider(), TlsProvider::NativeTls);
        assert!(matches!(native.root_certs(), RootCerts::PlatformVerifier));
        // Android's: rustls, with the roots it carries.
        let android = tls_config(TlsStack::Rustls);
        assert_eq!(android.provider(), TlsProvider::Rustls);
        assert!(matches!(android.root_certs(), RootCerts::WebPki));
        assert_eq!(http_tls().provider(), tls_config(TLS_STACK).provider());
    }

    #[test]
    fn names() {
        assert_eq!(safe_file_name("My: World?/ <3 ."), "My_ World__ _3");
        assert_eq!(safe_file_name("   "), "Untitled");
        assert_eq!(now_utc_iso().len(), 20);
        // Cut at 80 characters, then no trailing space: the date after a 79-character world name is cut away.
        let long = format!("{} 2026-10-21 12-00-00", "w".repeat(79));
        let name = safe_file_name(&long);
        assert_eq!(name.chars().count(), 79);
        assert!(!name.ends_with(' '));
        assert_eq!(safe_file_name("CON"), "CON_");
        assert_eq!(safe_file_name("nul.txt"), "nul_.txt");
        assert_eq!(safe_file_name("AUX .tar.gz"), "AUX_ .tar.gz");
        assert_eq!(safe_file_name("COM1"), "COM1_");
        assert_eq!(safe_file_name("COMPUTER"), "COMPUTER");
        assert_eq!(safe_file_name("LPT10"), "LPT10");
    }

    #[test]
    fn writes_whole_files_even_when_writers_race() {
        let dir = std::env::temp_dir().join(format!("orbiters-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        // Writers of different lengths at once: every result is one of them, whole.
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        let _ = write_atomic(&path, "x".repeat(10 + i * 7).as_bytes());
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.chars().all(|c| c == 'x') && (10..=59).contains(&text.len()) && (text.len() - 10).is_multiple_of(7), "{} bytes", text.len());
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x == "tmp")).collect();
        assert!(left.is_empty(), "no temporary file is left behind");
        std::fs::remove_dir_all(dir).ok();
    }
}
