//! Shared logic of Orbiters desktop, used by the app and its workers.

pub mod bundle;
pub mod clock;
pub mod keyframes;
pub mod osc;
pub mod paths;
pub mod pose;
pub mod protocol;
pub mod reconstruction;
pub mod settings;
pub mod vrchat_log;

pub use orbiters_contracts as contracts;

/// Version of the app and its workers (they must match: the shell refuses a worker of another version).
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

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
