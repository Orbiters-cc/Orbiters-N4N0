//! Shared logic of Orbiters - N4N0, used by the app and its workers.

pub mod account;
pub mod appearance;
pub mod assets;
pub mod bundle;
pub mod call;
pub mod cleanup;
pub mod clock;
pub mod community;
pub mod disk;
mod dpapi;
pub mod explicit;
pub mod explorer;
pub mod folder_watch;
pub mod picture;
pub mod features;
pub mod gallery_talk;
pub mod group_events;
pub mod groups;
pub mod here;
pub mod keyframes;
pub mod lan;
pub mod launcher;
pub mod launcher_feed;
pub mod launcher_local;
#[cfg(test)]
mod launcher_live;
pub mod maps;
pub mod measured;
pub mod notice_log;
pub mod obelisk;
pub mod origin;
pub mod relay;
pub mod osc;
pub mod picture_sources;
pub mod pipeline;
pub mod panic_log;
pub mod paths;
pub mod png_meta;
pub mod pose;
pub mod processes;
pub mod protocol;
pub mod proton;
pub mod reconstruction;
pub mod search;
pub mod sealed;
pub mod settings;
pub mod shared_view;
pub mod shares;
pub mod slice;
/// The app's own sources, for the tests that read them.
#[cfg(any(test, feature = "test-support"))]
pub mod source;
pub mod video_history;
pub mod visited;
pub mod voice;
pub mod vrchat_accounts;
pub mod vrchat_api;
pub mod vrchat_launch;
pub mod vrchat_log;
pub mod vrchat_source;
pub mod vrchat_store;
pub mod world_feedback;

pub use orbiters_contracts as contracts;

/// Ends this worker when the service that started it goes, even killed or crashed. On Windows the service's job object
/// does this; the Mac has no such thing, so a worker watches for it: its
/// parent changes when the service is gone. A worker left behind would keep the GPU or the microphone. (Linux has `PR_SET_PDEATHSIG`,
/// which also ends a worker when the *thread* that started it ends, as the service's short ones do: it watches too.)
pub fn leave_with_parent() {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
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

/// Version of the app and its workers (they must match: the shell refuses a worker of another version). Given by each
/// program at start (`orbiters_version::set(env!("ORBITERS_VERSION"))`), not compiled in here: see `orbiters-version`.
pub use orbiters_version::get as app_version;

/// The TLS the HTTP clients use: on Windows and the Mac, ureq's native TLS (Windows' SChannel, certificates checked against
/// Windows' store); on Linux and Android, rustls with ring (native TLS would mean OpenSSL there, another one on each
/// distribution), checked against Mozilla's roots (webpki-roots: Android's verifier would need the app's Java context). Each build carries its own platform's
/// alone: an agent left on another provider panics at its first `https` request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsStack {
    Native,
    Rustls,
}

/// This platform's TLS.
pub const TLS_STACK: TlsStack = if cfg!(any(target_os = "android", target_os = "linux")) { TlsStack::Rustls } else { TlsStack::Native };

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
    ureq::Agent::config_builder().middleware(origin::guard).tls_config(http_tls()).timeout_connect(connect).timeout_global(whole).build().new_agent()
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

/// [`write_atomic`] for a file only its owner may read (Unix permissions 0600; on Windows the folder's, which is the
/// user's own): a credential.
pub fn write_atomic_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_as(path, bytes, true)
}

/// The files [`read_text`] last failed to read (locked by another program for longer than it waits, access denied):
/// what this process holds for them is a default, not their content.
static UNREADABLE: std::sync::Mutex<std::collections::BTreeSet<std::path::PathBuf>> = std::sync::Mutex::new(std::collections::BTreeSet::new());

fn mark_unreadable(path: &std::path::Path, unreadable: bool) {
    let mut set = UNREADABLE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if unreadable {
        set.insert(path.to_path_buf());
    } else if !set.is_empty() {
        set.remove(path);
    }
}

/// Whether the last [`read_text`] of `path` failed (not "there is none": it could not be read).
pub fn was_unreadable(path: &std::path::Path) -> bool {
    UNREADABLE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains(path)
}

/// A file whose last read failed is never replaced unseen: it is first copied aside ([`keep_aside`]) or, when it still
/// cannot be read, moved aside; when neither works the write is refused and the file stays as it is.
fn preserve_unreadable(path: &std::path::Path) -> std::io::Result<()> {
    if !was_unreadable(path) {
        return Ok(());
    }
    if !path.exists() || keep_aside(path).is_some() {
        mark_unreadable(path, false);
        return Ok(());
    }
    match move_aside(path) {
        Ok(()) => {
            mark_unreadable(path, false);
            Ok(())
        }
        Err(e) => Err(std::io::Error::new(e.kind(), orbiters_i18n::tr!("{path} could not be read, and is not replaced unseen: {error}", path = path.display(), error = e))),
    }
}

fn write_atomic_as(path: &std::path::Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    preserve_unreadable(path)?;
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

/// Reads a text file, `None` when there is none. Bytes that are not UTF-8 are read as well as they can be rather than refused.
pub fn read_text(path: &std::path::Path) -> std::io::Result<Option<String>> {
    match read_retrying(path) {
        // A read that works now does not end "could not be read": what this process made without the file is still
        // not the file, and the next write keeps the file beside first ([`preserve_unreadable`]).
        Ok(bytes) => Ok(bytes.map(|b| String::from_utf8(b).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))),
        Err(e) => {
            // Until it is read again, a write must not replace it with what was made without it.
            mark_unreadable(path, true);
            Err(e)
        }
    }
}

/// Reads a file, `None` when there is none. A moment's lock by another program (an antivirus scan, a backup) is waited out.
fn read_retrying(path: &std::path::Path) -> std::io::Result<Option<Vec<u8>>> {
    let mut wait = std::time::Duration::from_millis(20);
    for attempt in 0..5 {
        match std::fs::read(path) {
            Ok(bytes) => return Ok(Some(bytes)),
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
    json_lenient_value(input)
}

/// Decodes a parsed JSON document with the same field recovery as json_lenient.
fn json_lenient_value<T: serde::de::DeserializeOwned + serde::Serialize + Default>(input: serde_json::Value) -> (T, Vec<String>) {
    if let Ok(value) = serde_json::from_value::<T>(input.clone()) {
        return (value, Vec::new());
    }
    let (Ok(mut base), Some(fields)) = (serde_json::to_value(T::default()), input.as_object()) else { return (T::default(), vec!["(not JSON)".into()]) };
    let fits = |candidate: &serde_json::Value| {
        #[cfg(test)]
        RECOVERY_PROBES.set(RECOVERY_PROBES.get() + 1);
        serde_json::from_value::<T>(candidate.clone()).is_ok()
    };
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

#[cfg(test)]
thread_local! {
    static RECOVERY_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// When a file that keeps how far VRChat's logs were read beside what was found in them (the visited worlds, the
/// videos) is saved: at once when what was found changed, and for the read positions alone every
/// [`Checkpoint::OFFSETS_EVERY`] (a crash in between reads a few minutes of log again, which finds nothing new). A
/// failed save is tried again, later each time.
#[derive(Clone, Debug, Default)]
pub struct Checkpoint {
    /// What was found changed since the last save.
    pub(crate) unsaved: bool,
    saved_at: Option<std::time::Instant>,
    retry_at: Option<std::time::Instant>,
    failures: u32,
}

impl Checkpoint {
    pub const OFFSETS_EVERY: std::time::Duration = std::time::Duration::from_secs(300);
    const RETRY_FIRST: std::time::Duration = std::time::Duration::from_secs(10);

    /// Whether to save now, the read positions having moved since the last save (`moved`) or not.
    pub fn due(&self, moved: bool, now: std::time::Instant) -> bool {
        if self.retry_at.is_some_and(|t| now < t) {
            return false;
        }
        self.unsaved || (moved && self.saved_at.is_none_or(|t| now.duration_since(t) >= Self::OFFSETS_EVERY))
    }

    /// A save ended (`ok` or not) at `now`.
    pub fn saved(&mut self, ok: bool, now: std::time::Instant) {
        if ok {
            *self = Self { saved_at: Some(now), ..Self::default() };
        } else {
            self.failures = self.failures.saturating_add(1);
            let wait = Self::RETRY_FIRST.saturating_mul(1 << self.failures.min(5).saturating_sub(1)).min(Self::OFFSETS_EVERY);
            self.retry_at = Some(now + wait);
        }
    }
}

/// Keeps a copy of `path` beside it as `<name>.bad-<unix seconds>`: a file that could be read only in part, before the
/// next save replaces it.
pub fn keep_aside(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let aside = aside_of(path);
    std::fs::copy(path, &aside).ok().map(|_| aside)
}

/// `<name>.bad-<unix seconds>` beside `path`.
fn aside_of(path: &std::path::Path) -> std::path::PathBuf {
    let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut aside = path.as_os_str().to_owned();
    aside.push(format!(".bad-{seconds}"));
    aside.into()
}

fn move_aside(path: &std::path::Path) -> std::io::Result<()> {
    rename_retrying(path, &aside_of(path))
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

    /// CORE-7 / CORE-C4: what was found is saved at once, read positions alone every few minutes, and a failed save
    /// stays to do, tried again later each time.
    #[test]
    fn history_saves_finds_at_once_positions_now_and_then_and_retries_failures() {
        let t0 = std::time::Instant::now();
        let mut c = Checkpoint::default();
        assert!(c.due(true, t0), "never saved: the first move is saved");
        c.saved(true, t0);
        assert!(!c.due(true, t0 + std::time::Duration::from_secs(10)), "positions alone: not every beat");
        assert!(c.due(true, t0 + Checkpoint::OFFSETS_EVERY), "but every few minutes");
        assert!(!c.due(false, t0 + Checkpoint::OFFSETS_EVERY), "nothing moved: nothing to save");
        c.unsaved = true;
        assert!(c.due(false, t0 + std::time::Duration::from_secs(10)), "a find: at once");
        let t1 = t0 + std::time::Duration::from_secs(20);
        c.saved(false, t1);
        assert!(c.unsaved, "a failed save keeps it to do");
        assert!(!c.due(false, t1 + std::time::Duration::from_secs(5)));
        assert!(c.due(false, t1 + std::time::Duration::from_secs(10)), "tried again 10 s later");
        c.saved(false, t1 + std::time::Duration::from_secs(10));
        assert!(!c.due(false, t1 + std::time::Duration::from_secs(25)) && c.due(false, t1 + std::time::Duration::from_secs(30)), "then 20 s");
        for _ in 0..10 {
            c.saved(false, t1);
        }
        assert!(c.due(false, t1 + Checkpoint::OFFSETS_EVERY), "never more than five minutes apart");
        c.saved(true, t1);
        assert!(!c.unsaved && !c.due(false, t1));
    }

    /// A file another program held when it was read is kept aside before a write replaces it.
    #[cfg(windows)]
    #[test]
    fn a_file_that_could_not_be_read_is_kept_before_it_is_replaced() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = std::env::temp_dir().join(format!("orbiters-unreadable-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");
        std::fs::write(&path, b"precious").unwrap();
        let lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(&path).unwrap();
        assert!(read_text(&path).is_err());
        assert!(was_unreadable(&path));
        drop(lock);
        // Another reader of the same process reads it now: still "could not be read" for the writer (verifier, CORE-4).
        assert_eq!(read_text(&path).unwrap().as_deref(), Some("precious"));
        assert!(was_unreadable(&path));
        write_atomic(&path, b"defaults").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"defaults");
        let kept: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("history.json.bad-")).collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read(kept[0].path()).unwrap(), b"precious");
        assert!(!was_unreadable(&path));
        // Read again, written again: nothing more kept.
        assert_eq!(read_text(&path).unwrap().as_deref(), Some("defaults"));
        write_atomic(&path, b"next").unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn each_platform_has_its_own_tls() {
        use ureq::tls::{RootCerts, TlsProvider};
        assert_eq!(TLS_STACK, if cfg!(any(target_os = "android", target_os = "linux")) { TlsStack::Rustls } else { TlsStack::Native });
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
