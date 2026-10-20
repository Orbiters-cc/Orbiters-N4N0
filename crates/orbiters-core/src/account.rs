//! The Orbiters account, shared with the Unity tools (MCB, My Avatar, ReFit, Logger and Orbiters Toolkit): they keep
//! one sign-in per server in Unity's preferences folder (`Preferences/MCB/account.dat`, `account_dev.dat` for the
//! development server), written by Toolkit's `AuthenticationService`. On Windows the JSON is protected with DPAPI for
//! the current user (with Toolkit's entropy), elsewhere it is plain JSON readable by the user only. Signing in here
//! writes the same file, so every tool shares it; signing out removes it for every tool.
//!
//! A sign-in is the same browser pairing the tools use (`/editor-links`): Orbiters desktop asks for a link, opens it,
//! the person signs in on the website and types the code shown here, and polling with the link's secret returns an
//! editor credential (`orbit-…`, sent as a bearer token). It has no expiry of its own: the server drops it after 90
//! idle days or when the account signs out everywhere, and `unity-wizard/token` tells whether it still works.

use anyhow::{bail, Context as _};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The Orbiters server the Unity tools talk to (their **Production** / **Development** switch).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Server {
    Production,
    Development,
}

/// Unity's EditorPrefs key for the switch (Toolkit's `OrbitersEnvironment`).
const DEV_SWITCH: &str = "MCB_DevEnvironment";
/// Toolkit's DPAPI entropy for the account file.
const ENTROPY: &[u8] = b"Orbiters.Toolkit account";
/// The client name Orbiters desktop signs in as (the server's editor-link clients).
pub const CLIENT: &str = "desktop";

impl Server {
    /// The server the Unity tools are set to (production when Unity never set it, or on other systems).
    pub fn current() -> Self {
        if dev_switch().unwrap_or(false) {
            Server::Development
        } else {
            Server::Production
        }
    }

    pub fn api(self) -> &'static str {
        match self {
            Server::Production => "https://api.orbiters.cc/",
            Server::Development => "http://127.0.0.1:4100/",
        }
    }

    pub fn website(self) -> &'static str {
        match self {
            Server::Production => "https://orbiters.cc/",
            Server::Development => "https://dev.orbiters.cc/",
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Server::Production => "account.dat",
            Server::Development => "account_dev.dat",
        }
    }

    /// The older Toolkit's file (XOR "protected"), deleted on every sign-in and sign-out as Toolkit does.
    fn legacy_file_name(self) -> &'static str {
        match self {
            Server::Production => "auth.dat",
            Server::Development => "auth_dev.dat",
        }
    }
}

/// A signed-in account, as the tools store it (field names are Toolkit's `AuthData`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub token: String,
    /// The account's numeric id, as a string.
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default, rename = "avatarUrl")]
    pub avatar_url: Option<String>,
}

/// Unity's preferences folder (`InternalEditorUtility.unityPreferencesFolder`), shared by every project and version.
fn unity_preferences() -> Option<PathBuf> {
    if cfg!(windows) {
        dirs::config_dir().map(|d| d.join("Unity").join("Editor-5.x").join("Preferences"))
    } else if cfg!(target_os = "macos") {
        dirs::home_dir().map(|d| d.join("Library").join("Preferences").join("Unity").join("Editor-5.x"))
    } else {
        dirs::data_dir().map(|d| d.join("unity3d").join("Unity").join("Editor-5.x").join("Preferences"))
    }
}

/// Where the tools keep the account for `server`.
pub fn account_path(server: Server) -> Option<PathBuf> {
    unity_preferences().map(|p| p.join("MCB").join(server.file_name()))
}

/// The account signed in for `server`, if any (an unreadable or tampered file reads as signed out, as in Unity).
pub fn read(server: Server) -> Option<Account> {
    read_file(&account_path(server)?)
}

/// When the account file of `server` last changed (another tool signed in or out), or `None` without one.
pub fn modified(server: Server) -> Option<std::time::SystemTime> {
    std::fs::metadata(account_path(server)?).ok()?.modified().ok()
}

/// Signs `account` in for every tool on `server`.
pub fn save(server: Server, account: &Account) -> anyhow::Result<()> {
    let path = account_path(server).context("no Unity preferences folder")?;
    write_file(&path, account)?;
    let _ = std::fs::remove_file(path.with_file_name(server.legacy_file_name()));
    Ok(())
}

/// Signs every tool out of `server` (nothing is revoked on the server, as in Unity).
pub fn remove(server: Server) -> anyhow::Result<()> {
    let path = account_path(server).context("no Unity preferences folder")?;
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("could not remove {}", path.display())),
    }
    let _ = std::fs::remove_file(path.with_file_name(server.legacy_file_name()));
    Ok(())
}

fn read_file(path: &Path) -> Option<Account> {
    let data = std::fs::read(path).ok()?;
    let json = if cfg!(windows) { protect::unprotect(&data).ok()? } else { data };
    let account: Account = serde_json::from_slice(&json).ok()?;
    (!account.token.trim().is_empty()).then_some(account)
}

fn write_file(path: &Path, account: &Account) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Toolkit writes nulls for empty fields; serde does the same for `None`.
    let json = serde_json::to_vec(account)?;
    if cfg!(windows) {
        std::fs::write(path, protect::protect(&json)?)?;
    } else {
        write_private(path, &json)?;
    }
    Ok(())
}

#[cfg(unix)]
fn write_private(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(data)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    std::fs::write(path, data)?;
    Ok(())
}

/// Unity stores an EditorPrefs value `key` in the registry as `key_h<hash>`, the hash being djb2 with xor.
fn editor_prefs_name(key: &str) -> String {
    let hash = key.bytes().fold(5381u32, |h, c| h.wrapping_mul(33) ^ c as u32);
    format!("{key}_h{hash}")
}

#[cfg(windows)]
fn dev_switch() -> Option<bool> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: the output buffer is a u32 and its size is passed.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(r"Software\Unity Technologies\Unity Editor 5.x"),
            &HSTRING::from(editor_prefs_name(DEV_SWITCH)),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    status.is_ok().then_some(value != 0)
}

#[cfg(not(windows))]
fn dev_switch() -> Option<bool> {
    let _ = DEV_SWITCH;
    None
}

#[cfg(windows)]
mod protect {
    use super::ENTROPY;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        // SAFETY: DPAPI allocated `cbData` bytes at `pbData` with LocalAlloc; they are copied, then freed once.
        unsafe {
            let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            let _ = LocalFree(Some(HLOCAL(out.pbData.cast())));
            bytes
        }
    }

    pub fn protect(data: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: input blobs point at live slices for the duration of the call.
        unsafe { CryptProtectData(&blob(data), None, Some(&blob(ENTROPY)), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)? };
        Ok(take(out))
    }

    pub fn unprotect(data: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: as above.
        unsafe { CryptUnprotectData(&blob(data), None, Some(&blob(ENTROPY)), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)? };
        Ok(take(out))
    }
}

#[cfg(not(windows))]
mod protect {
    pub fn protect(data: &[u8]) -> anyhow::Result<Vec<u8>> {
        Ok(data.to_vec())
    }

    pub fn unprotect(data: &[u8]) -> anyhow::Result<Vec<u8>> {
        Ok(data.to_vec())
    }
}

/// A browser sign-in in progress: open `link_url`, show `code`, poll with `poll_token`.
#[derive(Clone, Debug, Deserialize)]
pub struct Link {
    #[serde(rename = "linkUrl")]
    pub link_url: String,
    #[serde(rename = "pollToken")]
    pub poll_token: String,
    pub code: String,
}

/// Where a browser sign-in stands.
#[derive(Clone, Debug)]
pub enum Poll {
    /// Not confirmed in the browser yet (or the server is busy): ask again in two seconds.
    Pending,
    Done(Account),
}

/// What the server says about a stored credential.
#[derive(Clone, Debug)]
pub enum Check {
    /// It works; the account as the server knows it now.
    Valid(Account),
    /// Refused: expired, or signed out everywhere.
    Refused,
}

/// Calls to the Orbiters API of one server.
#[derive(Clone)]
pub struct Api {
    agent: ureq::Agent,
    base: String,
}

/// An error answer: the server's own message when it gave one.
#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

impl Api {
    pub fn new(server: Server) -> Self {
        Self::at(server.api())
    }

    /// An API at another address (a disposable server in tests).
    pub fn at(base: &str) -> Self {
        use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
        let tls = TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build();
        let agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(90)))
            .user_agent(concat!("OrbitersDesktop/", env!("CARGO_PKG_VERSION")))
            .build()
            .new_agent();
        Self { agent, base: format!("{}/", base.trim_end_matches('/')) }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path.trim_start_matches('/'))
    }

    pub fn agent(&self) -> &ureq::Agent {
        &self.agent
    }

    /// The answer as JSON, or the server's error.
    pub fn json<T: serde::de::DeserializeOwned>(response: ureq::http::Response<ureq::Body>) -> anyhow::Result<T> {
        let status = response.status().as_u16();
        let mut body = response.into_body();
        let text = body.with_config().limit(32 * 1024 * 1024).read_to_string().unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(error(status, &text).into());
        }
        serde_json::from_str(&text).with_context(|| format!("unexpected answer from Orbiters ({status})"))
    }

    /// Whether `token` still works, with the account it belongs to.
    pub fn check(&self, token: &str) -> anyhow::Result<Check> {
        let response = self.agent.get(&self.url("unity-wizard/token")).header("Authorization", &format!("Bearer {token}")).call()?;
        // The server answers an unknown or inactive credential with 425 ("Token not found or inactive") on this route.
        if matches!(response.status().as_u16(), 401 | 403 | 404 | 425) {
            return Ok(Check::Refused);
        }
        let mut account: Account = Self::json(response)?;
        if account.token.trim().is_empty() {
            account.token = token.to_string();
        }
        Ok(Check::Valid(account))
    }

    /// Starts a browser sign-in with `provider` (`discord` or `telegram`).
    pub fn begin_link(&self, provider: &str) -> anyhow::Result<Link> {
        let device = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "Orbiters desktop".into());
        let response = self.agent.post(&self.url("editor-links")).send_json(serde_json::json!({ "provider": provider, "client": CLIENT, "device": device }))?;
        let link: Link = Self::json(response)?;
        if link.link_url.is_empty() || link.poll_token.is_empty() {
            bail!("Orbiters did not return a sign-in link.");
        }
        Ok(link)
    }

    /// Asks once whether the browser sign-in finished. Network errors, 425, 429 and server errors are `Pending`.
    pub fn poll_link(&self, poll_token: &str) -> anyhow::Result<Poll> {
        let Ok(response) = self.agent.post(&self.url("editor-links/poll")).send_json(serde_json::json!({ "pollToken": poll_token })) else {
            return Ok(Poll::Pending);
        };
        let status = response.status().as_u16();
        if status == 425 || status == 429 || status >= 500 {
            return Ok(Poll::Pending);
        }
        let account: Account = Self::json(response).map_err(|e| match e.downcast::<ApiError>() {
            Ok(api) if api.message.is_empty() => anyhow::anyhow!("The sign-in expired. Sign in again."),
            Ok(api) => api.into(),
            Err(e) => e,
        })?;
        if account.token.trim().is_empty() {
            bail!("Orbiters did not return a credential.");
        }
        Ok(Poll::Done(account))
    }
}

fn error(status: u16, body: &str) -> ApiError {
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_default();
    ApiError { status, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_prefs_names_match_unity() {
        // Read from the registry of a machine where Toolkit set the switch.
        assert_eq!(editor_prefs_name("MCB_DevEnvironment"), "MCB_DevEnvironment_h3740056980");
    }

    #[test]
    fn account_files_round_trip_in_toolkit_format() {
        let dir = std::env::temp_dir().join(format!("orbiters-account-{}", std::process::id()));
        let path = dir.join("MCB").join("account.dat");
        let account = Account { token: "orbit-00000000-00000000-00000000-01-01-2026".into(), user: Some("42".into()), username: Some("Example".into()), avatar_url: None };
        write_file(&path, &account).unwrap();
        assert_eq!(read_file(&path), Some(account));
        let raw = std::fs::read(&path).unwrap();
        if cfg!(windows) {
            // A DPAPI blob (version 1, the DPAPI provider's GUID), not readable JSON.
            assert_eq!(&raw[..4], &[1, 0, 0, 0]);
            assert!(serde_json::from_slice::<Account>(&raw).is_err());
            let json = protect::unprotect(&raw).unwrap();
            assert_eq!(String::from_utf8(json).unwrap(), r#"{"token":"orbit-00000000-00000000-00000000-01-01-2026","user":"42","username":"Example","avatarUrl":null}"#);
        }
        // Tampered: signed out.
        std::fs::write(&path, b"nonsense").unwrap();
        assert_eq!(read_file(&path), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The browser pairing as Orbiters desktop runs it, against a disposable server: ORBITERS_TEST_API (its address) and
    /// ORBITERS_TEST_JWT (a website token of a member there, who "confirms" in place of the browser). Run by hand:
    /// cargo test -p orbiters-core account::tests::signs_in_through_the_browser_pairing -- --ignored --nocapture
    #[test]
    #[ignore]
    fn signs_in_through_the_browser_pairing() {
        let base = std::env::var("ORBITERS_TEST_API").expect("ORBITERS_TEST_API");
        let jwt = std::env::var("ORBITERS_TEST_JWT").expect("ORBITERS_TEST_JWT");
        let api = Api::at(&base);
        let link = api.begin_link("discord").unwrap();
        assert!(link.link_url.contains("app=desktop"), "{}", link.link_url);
        assert!(matches!(api.poll_link(&link.poll_token).unwrap(), Poll::Pending));
        let token = link.link_url.split("token=").nth(1).unwrap().split('&').next().unwrap().to_string();
        // A wrong code is refused in Orbiters desktop's words, the right one confirms.
        let confirm = |code: &str| {
            api.agent()
                .post(&api.url(&format!("editor-links/{token}/confirm")))
                .header("Authorization", &format!("Bearer {jwt}"))
                .send_json(serde_json::json!({ "code": code, "app": "desktop" }))
                .unwrap()
        };
        let wrong = confirm(if link.code == "AAAA" { "BBBB" } else { "AAAA" });
        assert_eq!(wrong.status().as_u16(), 400);
        assert!(wrong.into_body().read_to_string().unwrap().contains("Orbiters desktop shows"));
        assert_eq!(confirm(&link.code).status().as_u16(), 200);
        let Poll::Done(account) = api.poll_link(&link.poll_token).unwrap() else { panic!("not signed in") };
        assert!(account.token.starts_with("orbit-"));
        eprintln!("signed in as {:?}", account.username);
        // The credential works, and only once is it handed out.
        assert!(matches!(api.check(&account.token).unwrap(), Check::Valid(_)));
        assert!(api.poll_link(&link.poll_token).is_err());
        assert!(matches!(api.check("orbit-00000000-00000000-00000000-01-01-2026").unwrap(), Check::Refused));
    }

    /// Reads the account the Unity tools saved on this machine (run by hand; prints the name only):
    /// cargo test -p orbiters-core account::tests::reads_the_unity_tools_account -- --ignored --nocapture
    #[test]
    #[ignore]
    fn reads_the_unity_tools_account() {
        let server = Server::current();
        let account = read(server).expect("no account saved by the Unity tools");
        eprintln!("{server:?}: signed in as {:?} (user {:?})", account.username, account.user);
        match Api::new(server).check(&account.token).unwrap() {
            Check::Valid(a) => eprintln!("the server accepts it: {:?}", a.username),
            Check::Refused => eprintln!("the server refuses it"),
        }
    }
}
