//! Shared logic of Orbiters desktop, used by the app and its workers.

pub mod account;
pub mod bundle;
pub mod clock;
pub mod keyframes;
pub mod maps;
pub mod osc;
pub mod paths;
pub mod pose;
pub mod protocol;
pub mod reconstruction;
pub mod settings;
pub mod visited;
pub mod voice;
pub mod vrchat_accounts;
pub mod vrchat_log;

pub use orbiters_contracts as contracts;

/// Version of the app and its workers (they must match: the shell refuses a worker of another version).
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// TLS for HTTP clients: the build carries ureq's native TLS only (Windows' SChannel, certificates checked against
/// Windows' store). An agent left on ureq's default provider, rustls, panics at its first `https` request.
pub fn http_tls() -> ureq::tls::TlsConfig {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build()
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

/// Writes a file through a temporary one beside it and a rename: a reader (or a crash) finds the old content or the
/// new one, never part of it.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Makes a string safe as a single folder name on Windows and Linux.
pub fn safe_file_name(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    let limited: String = trimmed.chars().take(80).collect();
    if limited.is_empty() {
        "Untitled".into()
    } else {
        limited
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(safe_file_name("My: World?/ <3 ."), "My_ World__ _3");
        assert_eq!(safe_file_name("   "), "Untitled");
        assert_eq!(now_utc_iso().len(), 20);
    }
}
