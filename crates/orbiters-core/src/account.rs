//! The Orbiters account, shared with the Unity tools (MCB, My Avatar, ReFit, Logger and Orbiters Toolkit): they keep
//! one sign-in per server in Unity's preferences folder (`Preferences/MCB/account.dat`, `account_dev.dat` for the
//! development server), written by Toolkit's `AuthenticationService`. On Windows the JSON is protected with DPAPI for
//! the current user (with Toolkit's entropy), elsewhere it is plain JSON readable by the user only. Signing in here
//! writes the same file, so every tool shares it; signing out removes it for every tool. An app without Unity beside it
//! keeps its own ([`set_store`]: on Android, in its files folder, sealed with a key of the Android Keystore).
//!
//! A sign-in is the same browser pairing the tools use (`/editor-links`): Orbiters - N4N0 listens on this computer's
//! loopback ([`Callback`]), asks Orbiters for a link naming where it listens, and opens it; the person signs in on the
//! website if needed and clicks Connect, and the website sends the browser to the listener with a one-time code, which
//! the link's secret trades for an editor credential (`orbit-…`, sent as a bearer token). Nothing is typed. A link
//! someone else sends connects nothing: their browser hands the code to their own computer, where no tool listens. The
//! credential has no expiry of its own: the server drops it after 90 idle days or when the account signs out
//! everywhere, and `unity-wizard/token` tells whether it still works.

use anyhow::{bail, Context as _};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

/// The Orbiters server the Unity tools talk to (their **Production** / **Development** switch).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Server {
    Production,
    Development,
}

/// Unity's EditorPrefs key for the switch (Toolkit's `OrbitersEnvironment`).
const DEV_SWITCH: &str = "MCB_DevEnvironment";
/// Fill this once the N4N0 gallery item is published on production. IDs are scoped to their server.
pub const N4N0_PRODUCTION_GALLERY_ASSET_ID: Option<u64> = None;
/// Toolkit's DPAPI entropy for the account file.
const ENTROPY: &[u8] = b"Orbiters.Toolkit account";
/// The client name Orbiters - N4N0 signs in as (the server's editor-link clients).
pub const CLIENT: &str = "desktop";

/// The server an app chose for itself ([`choose_server`]): 0 none, 1 production, 2 development.
static CHOSEN: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Talks to `server` from now on, whatever the Unity tools are set to (Orbiters - N4N0's own Development environment
/// switch); `None` follows the Unity tools again.
pub fn choose_server(server: Option<Server>) {
    let value = match server {
        None => 0,
        Some(Server::Production) => 1,
        Some(Server::Development) => 2,
    };
    CHOSEN.store(value, Ordering::Relaxed);
}

impl Server {
    pub fn n4n0_gallery_asset_id(self) -> Option<u64> {
        match self {
            Server::Development => Some(30),
            Server::Production => N4N0_PRODUCTION_GALLERY_ASSET_ID,
        }
    }

    /// The server the app chose ([`choose_server`]), else the one the Unity tools are set to (production when Unity
    /// never set it, or on other systems).
    pub fn current() -> Self {
        match CHOSEN.load(Ordering::Relaxed) {
            1 => return Server::Production,
            2 => return Server::Development,
            _ => {}
        }
        if dev_switch().unwrap_or(false) {
            Server::Development
        } else {
            Server::Production
        }
    }

    /// The API's address. The development server is the one a developer runs on this machine, over plain HTTP: its
    /// credential goes to whatever listens on that port (only while Unity's switch says Development).
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

    /// Whether `url` is a page of this server's website: https on its host, or, against the development API (a
    /// server on this machine), also a website on this machine. A sign-in link comes from the API: anything else
    /// (another scheme, a file, a share, another host) is never opened.
    pub fn is_website_page(self, url: &str) -> bool {
        // What the browser gets is what is checked here: no user info, spaces, quotes or backslashes to read twice.
        if url.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '\\' | '"' | '<' | '>' | '^' | '`' | '{' | '|' | '}')) {
            return false;
        }
        let Some((scheme, rest)) = url.split_once("://") else { return false };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default().to_ascii_lowercase();
        if authority.contains('@') {
            return false;
        }
        let website = self.website().trim_start_matches("https://").trim_end_matches('/');
        if scheme.eq_ignore_ascii_case("https") && authority == website {
            return true;
        }
        let host = match authority.rsplit_once(':') {
            Some((host, port)) if port.parse::<u16>().is_ok() => host,
            _ => authority.as_str(),
        };
        self == Server::Development
            && (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
            && matches!(host, "127.0.0.1" | "localhost")
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
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Without the credential: an account printed in a log or a test's failure shows who, never the token.
impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("token", &"(hidden)")
            .field("user", &self.user)
            .field("username", &self.username)
            .field("avatar_url", &self.avatar_url)
            .finish()
    }
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

/// Where and how the account is kept: the file of each server's account, and its bytes protected for this user.
pub trait CredentialStore: Send + Sync {
    /// The file of `server`'s account.
    fn path(&self, server: Server) -> Option<PathBuf>;
    /// The file's bytes for the account's JSON, protected for this user.
    fn seal(&self, plain: &[u8]) -> anyhow::Result<Vec<u8>>;
    /// The account's JSON from the file's bytes (an error when they were not sealed for this user, or were tampered with).
    fn open(&self, sealed: &[u8]) -> anyhow::Result<Vec<u8>>;
}

/// The Unity tools' own: `MCB/account.dat` in Unity's preferences folder, protected with DPAPI on Windows (with
/// Toolkit's entropy), plain JSON readable by the user only elsewhere.
pub struct UnityStore;

impl CredentialStore for UnityStore {
    fn path(&self, server: Server) -> Option<PathBuf> {
        unity_preferences().map(|p| p.join("MCB").join(server.file_name()))
    }

    fn seal(&self, plain: &[u8]) -> anyhow::Result<Vec<u8>> {
        if cfg!(windows) {
            protect::protect(plain)
        } else {
            Ok(plain.to_vec())
        }
    }

    fn open(&self, sealed: &[u8]) -> anyhow::Result<Vec<u8>> {
        if cfg!(windows) {
            protect::unprotect(sealed)
        } else {
            Ok(sealed.to_vec())
        }
    }
}

static STORE: OnceLock<Box<dyn CredentialStore>> = OnceLock::new();

/// Keeps the account in `store` from now on, instead of the Unity tools' file: once, at start, before the account is
/// read.
pub fn set_store(store: Box<dyn CredentialStore>) -> anyhow::Result<()> {
    STORE.set(store).map_err(|_| anyhow::anyhow!("the account's store was given already"))
}

/// Where the account is kept: the app's store, else the Unity tools'.
fn store() -> &'static dyn CredentialStore {
    match STORE.get() {
        Some(store) => store.as_ref(),
        None => &UnityStore,
    }
}

/// Where the tools keep the account for `server`.
pub fn account_path(server: Server) -> Option<PathBuf> {
    store().path(server)
}

/// The account signed in for `server`, if any (an unreadable or tampered file reads as signed out, as in Unity).
pub fn read(server: Server) -> Option<Account> {
    read_file(store(), &account_path(server)?)
}

/// When the account file of `server` last changed (another tool signed in or out), or `None` without one.
pub fn modified(server: Server) -> Option<std::time::SystemTime> {
    std::fs::metadata(account_path(server)?).ok()?.modified().ok()
}

/// Signs `account` in for every tool on `server`.
pub fn save(server: Server, account: &Account) -> anyhow::Result<()> {
    let path = account_path(server).context("no Unity preferences folder")?;
    write_file(store(), &path, account)?;
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

fn read_file(store: &dyn CredentialStore, path: &Path) -> Option<Account> {
    let data = std::fs::read(path).ok()?;
    let json = store.open(&data).ok()?;
    let account: Account = serde_json::from_slice(&json).ok()?;
    (!account.token.trim().is_empty()).then_some(account)
}

/// Writes the account whole, through a temporary file renamed over the old one ([`crate::write_atomic_private`]): a
/// Unity tool reading it meanwhile, a crash or a power cut never finds half a blob, which every tool reads as signed
/// out.
fn write_file(store: &dyn CredentialStore, path: &Path, account: &Account) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Toolkit writes nulls for empty fields; serde does the same for `None`.
    let json = serde_json::to_vec(account)?;
    let data = store.seal(&json)?;
    crate::write_atomic_private(path, &data)?;
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
        if out.pbData.is_null() {
            return Vec::new();
        }
        // SAFETY: DPAPI allocated `cbData` bytes at `pbData` (not null, checked above) with LocalAlloc; they are copied,
        // then freed once.
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

/// A browser sign-in waiting for the website's Connect: the page to open, where the browser goes once this tool has its
/// credential, and the secret only this tool holds, which trades the code the browser brings for the credential.
#[derive(Clone, Deserialize)]
pub struct Link {
    #[serde(rename = "linkUrl")]
    pub link_url: String,
    #[serde(rename = "connectedUrl")]
    pub connected_url: String,
    #[serde(rename = "exchangeToken")]
    pub exchange_token: String,
}

impl Link {
    /// Where the browser goes when the sign-in could not finish: the connected page, told so.
    pub fn failed_url(&self) -> String {
        let (page, fragment) = self.connected_url.split_once('#').map_or((self.connected_url.as_str(), None), |(p, f)| (p, Some(f)));
        let joined = format!("{page}{}failed=1", if page.contains('?') { '&' } else { '?' });
        match fragment {
            Some(f) => format!("{joined}#{f}"),
            None => joined,
        }
    }

    /// Where the browser goes after trading its code gave `exchange`, and whether the sign-in is over.
    pub fn answer(&self, exchange: &Exchange) -> Answer {
        match exchange {
            Exchange::Done(_) => Answer { location: self.connected_url.clone(), done: true },
            Exchange::Retry(_) => Answer { location: self.failed_url(), done: false },
            Exchange::Ended(_) => Answer { location: self.failed_url(), done: true },
        }
    }
}

/// Without its secrets: the exchange token, and the link's query (which holds the link's own token).
impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let page = self.link_url.split(['?', '#']).next().unwrap_or_default();
        f.debug_struct("Link")
            .field("link_url", &format_args!("{page}?(hidden)"))
            .field("connected_url", &self.connected_url)
            .field("exchange_token", &"(hidden)")
            .finish()
    }
}

/// What trading a code the browser brought gave.
#[derive(Debug)]
pub enum Exchange {
    /// The credential.
    Done(Account),
    /// Not with this code (a wrong one, an older one after Connect was clicked again, Orbiters out of reach): the
    /// sign-in waits for the browser to bring another.
    Retry(String),
    /// The sign-in is over (expired, ended after wrong codes, revoked): it starts again from Sign in.
    Ended(String),
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

    /// An API at another address (a disposable server in tests). Every `Api` of one address shares one HTTP client
    /// for the life of the process, so connections are kept and reused from call to call: no TCP and TLS handshake
    /// for each account check, each sign-in poll every two seconds, each upload.
    pub fn at(base: &str) -> Self {
        static AGENTS: std::sync::Mutex<Vec<(String, ureq::Agent)>> = std::sync::Mutex::new(Vec::new());
        let base = format!("{}/", base.trim_end_matches('/'));
        let mut agents = AGENTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, agent)) = agents.iter().find(|(b, _)| *b == base) {
            return Self { agent: agent.clone(), base };
        }
        let tls = crate::http_tls();
        let agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(90)))
            .user_agent(concat!("Orbiters-N4N0/", env!("CARGO_PKG_VERSION")))
            .build()
            .new_agent();
        agents.push((base.clone(), agent.clone()));
        Self { agent, base }
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

    /// Starts a browser sign-in whose Connect brings its code to this tool's [`Callback`] (its `port` and `path`).
    pub fn begin_link(&self, port: u16, path: &str) -> anyhow::Result<Link> {
        let device = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "Orbiters - N4N0".into());
        let body = serde_json::json!({ "client": CLIENT, "device": device, "port": port, "path": path });
        let link: Link = Self::json(self.agent.post(&self.url("editor-links")).send_json(body)?)?;
        if link.link_url.is_empty() || link.connected_url.is_empty() || link.exchange_token.is_empty() {
            bail!("Orbiters did not return a sign-in link.");
        }
        Ok(link)
    }

    /// Trades the code the browser brought (with the link's secret) for the credential: once, as Orbiters hands it out
    /// only once. 410 (expired, ended, revoked) ends the sign-in; a wrong code, Connect not clicked yet (425), a busy or
    /// unreachable server leave it waiting for another code.
    pub fn exchange_link(&self, exchange_token: &str, code: &str) -> Exchange {
        let body = serde_json::json!({ "exchangeToken": exchange_token, "code": code });
        let response = match self.agent.post(&self.url("editor-links/exchange")).send_json(body) {
            Ok(response) => response,
            Err(e) => return Exchange::Retry(format!("Orbiters could not be reached: {e}")),
        };
        let status = response.status().as_u16();
        match Self::json::<Account>(response) {
            Ok(account) if !account.token.trim().is_empty() => Exchange::Done(account),
            Ok(_) => Exchange::Ended("Orbiters did not return a credential. Sign in again.".into()),
            Err(e) => {
                let said = e.downcast_ref::<ApiError>().map(|api| api.message.clone()).filter(|m| !m.is_empty());
                match status {
                    404 | 410 => Exchange::Ended(said.unwrap_or_else(|| "The sign-in expired. Sign in again.".into())),
                    _ => Exchange::Retry(said.unwrap_or_else(|| format!("Orbiters could not finish the sign-in ({status})."))),
                }
            }
        }
    }
}

/// The browser's way back to this tool once Connect is clicked on the website: an HTTP listener on this computer's
/// loopback (127.0.0.1), at a port the system chose and a path nobody can guess. The website sends the browser to
/// `http://127.0.0.1:<port>/<path>?code=…`; anything else (another path or method, another host name, no code) gets a
/// 404 and changes nothing.
pub struct Callback {
    listener: TcpListener,
    port: u16,
    path: String,
}

/// What a code the browser brought came to: where the browser goes next, and whether the sign-in is over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub location: String,
    pub done: bool,
}

/// How a [`Callback`] stopped listening.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Served {
    /// A code ended the sign-in ([`Answer::done`]).
    Done,
    /// Asked to stop (cancelled, another sign-in, signed out).
    Stopped,
    /// The link's time ran out with no code that ended it.
    TimedOut,
}

/// Connections read at once at most: a page cannot make the listener hold more.
const CALLBACK_READERS: usize = 8;
/// What a connection has to send its request in, and its size at most.
const CALLBACK_READ: Duration = Duration::from_secs(5);
const CALLBACK_HEAD: usize = 8 * 1024;

impl Callback {
    /// Listens on 127.0.0.1 at a port the system picks, under a path of 32 random characters.
    pub fn bind() -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        Ok(Self { listener, port, path: uuid::Uuid::new_v4().simple().to_string() })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// Serves until `handle` says a code ended the sign-in, `stop` is set or `until` passes; the port closes then.
    /// `handle(code)` runs for each code brought, one at a time, and says where the browser goes (a redirect it gets
    /// only once `handle` returned: the credential is saved by then).
    pub fn serve(self, stop: &AtomicBool, until: Instant, mut handle: impl FnMut(&str) -> Answer) -> Served {
        let (codes, brought) = mpsc::channel::<(TcpStream, String)>();
        let reading = Arc::new(AtomicUsize::new(0));
        loop {
            while let Ok((mut stream, code)) = brought.try_recv() {
                let answer = handle(&code);
                respond(&mut stream, Some(&answer.location));
                if answer.done {
                    return Served::Done;
                }
            }
            if stop.load(Ordering::Relaxed) {
                return Served::Stopped;
            }
            if Instant::now() >= until {
                return Served::TimedOut;
            }
            match self.listener.accept() {
                Ok((stream, _)) if reading.fetch_add(1, Ordering::Relaxed) < CALLBACK_READERS => {
                    let (codes, done, path, port) = (codes.clone(), reading.clone(), self.path.clone(), self.port);
                    let spawned = std::thread::Builder::new().name("sign-in-reader".into()).spawn(move || {
                        let mut stream = stream;
                        match read_code(&mut stream, &path, port) {
                            Some(code) => {
                                let _ = codes.send((stream, code));
                            }
                            None => respond(&mut stream, None),
                        }
                        done.fetch_sub(1, Ordering::Relaxed);
                    });
                    if spawned.is_err() {
                        reading.fetch_sub(1, Ordering::Relaxed);
                    }
                }
                // Too many at once: this one is closed unread.
                Ok(_) => {
                    reading.fetch_sub(1, Ordering::Relaxed);
                }
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }
}

/// The code of a request that is the website's hand-back: `GET /<path>?…code=…` to `127.0.0.1:<port>`. Anything else
/// (or a request not sent whole within [`CALLBACK_READ`]) is `None`.
fn read_code(stream: &mut TcpStream, path: &str, port: u16) -> Option<String> {
    stream.set_nonblocking(false).ok()?;
    let deadline = Instant::now() + CALLBACK_READ;
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || head.len() > CALLBACK_HEAD {
            return None;
        }
        stream.set_read_timeout(Some(left)).ok()?;
        match stream.read(&mut buf) {
            Ok(0) => return None,
            Ok(n) => head.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    let head = String::from_utf8_lossy(&head);
    let mut lines = head.split("\r\n");
    let mut request = lines.next()?.split(' ');
    let (method, target, version) = (request.next()?, request.next()?, request.next()?);
    if method != "GET" || !version.starts_with("HTTP/1.") {
        return None;
    }
    // A page of another site pointed at this port (a DNS rebinding) names its own host.
    let host = lines.find_map(|l| l.split_once(':').filter(|(name, _)| name.trim().eq_ignore_ascii_case("host")).map(|(_, v)| v.trim()))?;
    if !host.eq_ignore_ascii_case(&format!("127.0.0.1:{port}")) {
        return None;
    }
    let query = target.strip_prefix('/')?.strip_prefix(path)?.strip_prefix('?')?;
    let code = query.split('&').find_map(|pair| pair.strip_prefix("code="))?;
    let code = percent_decode(code)?;
    (!code.is_empty() && code.len() <= 512).then_some(code)
}

/// `%XX` escapes decoded (a code is URL-safe base64, sent through `encodeURIComponent`).
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A redirect to `location`, or a 404 without one. Closed after it.
fn respond(stream: &mut TcpStream, location: Option<&str>) {
    let answer = match location.filter(|l| !l.chars().any(char::is_control)) {
        Some(location) => format!(
            "HTTP/1.1 302 Found\r\nLocation: {location}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ),
        None => "HTTP/1.1 404 Not Found\r\nCache-Control: no-store\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
    };
    let _ = stream.set_write_timeout(Some(CALLBACK_READ));
    let _ = stream.write_all(answer.as_bytes());
    let _ = stream.flush();
}

fn error(status: u16, body: &str) -> ApiError {
    let message =
        serde_json::from_str::<serde_json::Value>(body).ok().and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string)).unwrap_or_default();
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
    fn sign_in_links_open_only_on_the_website() {
        let prod = Server::Production;
        assert!(prod.is_website_page("https://orbiters.cc/link-editor?token=abc&provider=discord&app=desktop"));
        assert!(prod.is_website_page("https://orbiters.cc/fr/link-editor?token=abc"));
        assert!(prod.is_website_page("HTTPS://Orbiters.cc/link-editor"));
        for bad in [
            "http://orbiters.cc/link-editor",
            "https://dev.orbiters.cc/link-editor",
            "https://orbiters.cc.example.com/link-editor",
            "https://orbiters.cc@example.com/link-editor",
            "https://example.com/?https://orbiters.cc/",
            "file:///C:/Windows/System32/calc.exe",
            r"\\host\share\x.exe",
            r"https://orbiters.cc\..\x",
            "search-ms:query=x",
            "ms-msdt:/id PCWDiagnostic",
            "https://orbiters.cc/ x.exe",
            "https://127.0.0.1:3000/link-editor",
            "",
        ] {
            assert!(!prod.is_website_page(bad), "{bad}");
        }
        // The development website, and a website on this machine when the API is the local one.
        let dev = Server::Development;
        assert!(dev.is_website_page("https://dev.orbiters.cc/link-editor?token=abc"));
        assert!(dev.is_website_page("http://127.0.0.1:3000/link-editor?token=abc"));
        assert!(dev.is_website_page("http://localhost:4200/link-editor"));
        assert!(!dev.is_website_page("https://orbiters.cc/link-editor"));
        assert!(!dev.is_website_page("http://127.0.0.1.example.com/link-editor"));
        assert!(!dev.is_website_page("file://127.0.0.1/C$/x.exe"));
    }

    #[test]
    fn account_files_round_trip_in_toolkit_format() {
        let dir = std::env::temp_dir().join(format!("orbiters-account-{}", std::process::id()));
        let path = dir.join("MCB").join("account.dat");
        let account = Account {
            token: "orbit-00000000-00000000-00000000-01-01-2026".into(),
            user: Some("42".into()),
            username: Some("Example".into()),
            avatar_url: None,
        };
        write_file(&UnityStore, &path, &account).unwrap();
        assert_eq!(read_file(&UnityStore, &path), Some(account.clone()));
        assert!(!format!("{account:?} {:?}", Exchange::Done(account.clone())).contains("orbit-"), "no credential in what is printed");
        let link = Link {
            link_url: "https://orbiters.cc/link-editor?token=secret".into(),
            connected_url: "https://orbiters.cc/link-editor/connected?client=desktop".into(),
            exchange_token: "exchange-secret".into(),
        };
        assert!(!format!("{link:?}").contains("secret") && format!("{link:?}").contains("connected?client=desktop"));
        // Written again over itself: no temporary file is left beside.
        write_file(&UnityStore, &path, &account).unwrap();
        assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        let raw = std::fs::read(&path).unwrap();
        if cfg!(windows) {
            // A DPAPI blob (version 1, the DPAPI provider's GUID), not readable JSON.
            assert_eq!(&raw[..4], &[1, 0, 0, 0]);
            assert!(serde_json::from_slice::<Account>(&raw).is_err());
            let json = protect::unprotect(&raw).unwrap();
            assert_eq!(
                String::from_utf8(json).unwrap(),
                r#"{"token":"orbit-00000000-00000000-00000000-01-01-2026","user":"42","username":"Example","avatarUrl":null}"#
            );
        }
        // Tampered: signed out.
        std::fs::write(&path, b"nonsense").unwrap();
        assert_eq!(read_file(&UnityStore, &path), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A stand-in for a phone's store: a folder of its own, its bytes sealed with a key only it holds (as the Android
    /// Keystore's would be).
    struct Sealed {
        dir: PathBuf,
        key: u8,
    }

    impl CredentialStore for Sealed {
        fn path(&self, server: Server) -> Option<PathBuf> {
            Some(self.dir.join(server.file_name()))
        }

        fn seal(&self, plain: &[u8]) -> anyhow::Result<Vec<u8>> {
            Ok([b"SEALED".as_slice(), &plain.iter().map(|b| b ^ self.key).collect::<Vec<u8>>()].concat())
        }

        fn open(&self, sealed: &[u8]) -> anyhow::Result<Vec<u8>> {
            let body = sealed.strip_prefix(b"SEALED").ok_or_else(|| anyhow::anyhow!("not sealed here"))?;
            Ok(body.iter().map(|b| b ^ self.key).collect())
        }
    }

    #[test]
    fn an_app_s_own_store_keeps_the_account_sealed() {
        let dir = std::env::temp_dir().join(format!("orbiters-account-store-{}", std::process::id()));
        let store = Sealed { dir: dir.clone(), key: 0x5a };
        let path = store.path(Server::Production).unwrap();
        assert_eq!(path, dir.join("account.dat"));
        let account = Account { token: "orbit-phone".into(), user: Some("7".into()), username: Some("Quest".into()), avatar_url: None };
        write_file(&store, &path, &account).unwrap();
        let raw = std::fs::read(&path).unwrap();
        assert!(raw.starts_with(b"SEALED") && !String::from_utf8_lossy(&raw).contains("orbit-phone"), "only sealed bytes on disk");
        assert_eq!(read_file(&store, &path), Some(account));
        // Sealed with another key (another install), or not by this store at all: signed out.
        assert_eq!(read_file(&Sealed { dir: dir.clone(), key: 0x33 }, &path), None);
        std::fs::write(&path, br#"{"token":"orbit-plain"}"#).unwrap();
        assert_eq!(read_file(&store, &path), None);
        std::fs::remove_dir_all(&dir).unwrap();
        // The desktop sets none: the Unity tools' file.
        assert_eq!(account_path(Server::Production), UnityStore.path(Server::Production));
    }

    /// What a browser gets from `url` (`http://127.0.0.1:…`, `method`, `host` as the Host header when not the URL's
    /// own): the status, and where it is sent, without following it.
    fn browse(url: &str, method: &str, host: Option<&str>) -> (u16, Option<String>) {
        let rest = url.strip_prefix("http://").unwrap();
        let (authority, target) = rest.split_at(rest.find('/').unwrap());
        let mut s = TcpStream::connect(authority).unwrap();
        write!(s, "{method} {target} HTTP/1.1\r\nHost: {}\r\nAccept: text/html\r\nContent-Length: 0\r\n\r\n", host.unwrap_or(authority)).unwrap();
        let mut answer = String::new();
        s.read_to_string(&mut answer).unwrap();
        let status = answer.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        let location = answer.lines().find_map(|l| l.strip_prefix("Location: ")).map(str::to_string);
        (status, location)
    }

    /// A stand-in for Orbiters' editor links (the link it hands out, trades of the codes "good", "gone" and anything
    /// else), and what it was asked: the links' bodies and the codes traded.
    struct FakeOrbiters {
        base: String,
        links: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
        codes: Arc<std::sync::Mutex<Vec<String>>>,
    }

    const CONNECTED: &str = "https://orbiters.cc/link-editor/connected?client=desktop";

    fn fake_orbiters() -> FakeOrbiters {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let (links, codes) = (Arc::new(std::sync::Mutex::new(Vec::new())), Arc::new(std::sync::Mutex::new(Vec::new())));
        let (l, c) = (links.clone(), codes.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut data = Vec::new();
                let mut buf = [0u8; 4096];
                // The head, then as much body as it says.
                let body = loop {
                    let n = s.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break None;
                    }
                    data.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&data).to_string();
                    let Some((head, body)) = text.split_once("\r\n\r\n") else { continue };
                    let length =
                        head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)));
                    if body.len() >= length.unwrap_or(0) {
                        break Some((head.lines().next().unwrap_or_default().to_string(), body.to_string()));
                    }
                };
                let Some((request, body)) = body else { continue };
                let json: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let (status, answer) = if request.starts_with("POST /editor-links/exchange ") {
                    let code = json["code"].as_str().unwrap_or_default().to_string();
                    c.lock().unwrap().push(code.clone());
                    match (json["exchangeToken"].as_str(), code.as_str()) {
                        (Some("exchange-1"), "good") => {
                            (200, serde_json::json!({ "token": "orbit-test", "user": "42", "username": "Tester", "avatarUrl": null }))
                        }
                        (Some("exchange-1"), "gone") => {
                            (410, serde_json::json!({ "error": "This sign-in ended. Click Sign in again.", "code": "EDITOR_LINK_CODE_LOCKED" }))
                        }
                        (Some("exchange-1"), _) => {
                            (400, serde_json::json!({ "error": "This sign-in code does not match.", "code": "EDITOR_LINK_CODE_MISMATCH" }))
                        }
                        _ => (410, serde_json::json!({ "error": "This sign-in expired. Click Sign in again.", "code": "EDITOR_LINK_EXPIRED" })),
                    }
                } else if request.starts_with("POST /editor-links ") {
                    l.lock().unwrap().push(json.clone());
                    let fits = json["port"].as_u64().is_some_and(|p| (1024..=65535).contains(&p))
                        && json["path"]
                            .as_str()
                            .is_some_and(|p| (16..=64).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)));
                    if fits && json["client"] == "desktop" {
                        (
                            200,
                            serde_json::json!({ "linkUrl": "https://orbiters.cc/link-editor?token=link-1&app=desktop", "connectedUrl": CONNECTED,
                                "exchangeToken": "exchange-1", "expiresAt": "2026-10-25T12:10:00.000Z" }),
                        )
                    } else {
                        (400, serde_json::json!({ "error": "Update Orbiters - N4N0 to sign in.", "code": "EDITOR_LINK_UPDATE_TOOL" }))
                    }
                } else {
                    (404, serde_json::json!({ "error": "Not found" }))
                };
                let body = answer.to_string();
                let _ = write!(s, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        FakeOrbiters { base, links, codes }
    }

    /// A sign-in served on a thread as the app serves it: each code traded, the browser sent on by [`Link::answer`].
    fn serving(api: &Api, link: &Link, callback: Callback, stop: Arc<AtomicBool>, for_: Duration) -> std::thread::JoinHandle<(Served, Option<Account>)> {
        let (api, link) = (api.clone(), link.clone());
        std::thread::spawn(move || {
            let mut signed_in = None;
            let served = callback.serve(&stop, Instant::now() + for_, |code| {
                let exchange = api.exchange_link(&link.exchange_token, code);
                let answer = link.answer(&exchange);
                if let Exchange::Done(account) = exchange {
                    signed_in = Some(account);
                }
                answer
            });
            (served, signed_in)
        })
    }

    fn wait_for(server: &std::thread::JoinHandle<(Served, Option<Account>)>, within: Duration) -> bool {
        let until = Instant::now() + within;
        while !server.is_finished() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        server.is_finished()
    }

    #[test]
    fn signs_in_when_the_browser_brings_the_code() {
        let orbiters = fake_orbiters();
        let api = Api::at(&orbiters.base);
        let callback = Callback::bind().unwrap();
        let (port, path) = (callback.port(), callback.path().to_string());
        assert!(path.len() >= 16 && path.chars().all(|c| c.is_ascii_alphanumeric()));
        let link = api.begin_link(port, &path).unwrap();
        let asked = orbiters.links.lock().unwrap()[0].clone();
        assert_eq!((asked["client"].as_str(), asked["port"].as_u64(), asked["path"].as_str()), (Some(CLIENT), Some(port as u64), Some(path.as_str())));
        assert!(asked.get("provider").is_none(), "the website lets the person choose");
        let server = serving(&api, &link, callback, Arc::default(), Duration::from_secs(20));
        let at = format!("http://127.0.0.1:{port}/{path}");
        // A code that does not match (an older one, after Connect was clicked again): the browser hears it failed, and
        // the sign-in waits for the next.
        assert_eq!(browse(&format!("{at}?code=wrong"), "GET", None), (302, Some(format!("{CONNECTED}&failed=1"))));
        assert!(!wait_for(&server, Duration::from_millis(200)), "still listening");
        // The website's hand-back: traded, then the browser goes to the connected page.
        assert_eq!(browse(&format!("{at}?code=go%6Fd&from=site"), "GET", None), (302, Some(CONNECTED.to_string())));
        let (served, account) = server.join().unwrap();
        assert_eq!(served, Served::Done);
        let account = account.unwrap();
        assert_eq!((account.token.as_str(), account.username.as_deref(), account.user.as_deref()), ("orbit-test", Some("Tester"), Some("42")));
        assert_eq!(*orbiters.codes.lock().unwrap(), ["wrong", "good"]);
        // Nothing listens there any more.
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    }

    #[test]
    fn only_the_websites_hand_back_reaches_the_sign_in() {
        let orbiters = fake_orbiters();
        let api = Api::at(&orbiters.base);
        let callback = Callback::bind().unwrap();
        let (port, path) = (callback.port(), callback.path().to_string());
        let link = api.begin_link(port, &path).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let server = serving(&api, &link, callback, stop.clone(), Duration::from_secs(20));
        let root = format!("http://127.0.0.1:{port}");
        for (url, method, host) in [
            (format!("{root}/favicon.ico"), "GET", None),
            (format!("{root}/?code=good"), "GET", None),
            (format!("{root}/{}?code=good", &path[..path.len() - 1]), "GET", None),
            (format!("{root}/{path}x?code=good"), "GET", None),
            (format!("{root}/{path}/?code=good"), "GET", None),
            (format!("{root}/{path}"), "GET", None),
            (format!("{root}/{path}?code="), "GET", None),
            (format!("{root}/{path}?state=good"), "GET", None),
            (format!("{root}/{path}?code=good"), "POST", None),
            (format!("{root}/{path}?code=good"), "PUT", None),
            // A page of another site reaching this port through its own name (DNS rebinding).
            (format!("{root}/{path}?code=good"), "GET", Some(format!("evil.example:{port}"))),
            (format!("{root}/{path}?code=good"), "GET", Some(format!("localhost:{port}"))),
        ] {
            assert_eq!(browse(&url, method, host.as_deref()), (404, None), "{method} {url} as {host:?}");
        }
        // A connection that never says anything holds no one up.
        let silent = TcpStream::connect(("127.0.0.1", port)).unwrap();
        assert!(orbiters.codes.lock().unwrap().is_empty(), "nothing was traded");
        assert!(!server.is_finished());
        // Cancelled: the listener goes at once.
        stop.store(true, Ordering::Relaxed);
        assert!(wait_for(&server, Duration::from_secs(2)));
        assert_eq!(server.join().unwrap().0, Served::Stopped);
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
        drop(silent);
    }

    #[test]
    fn a_sign_in_ends_with_its_link() {
        let orbiters = fake_orbiters();
        let api = Api::at(&orbiters.base);
        // Ended by Orbiters (too many wrong codes, expired, revoked): the browser hears it failed, the listener closes.
        let callback = Callback::bind().unwrap();
        let (port, path) = (callback.port(), callback.path().to_string());
        let link = api.begin_link(port, &path).unwrap();
        let server = serving(&api, &link, callback, Arc::default(), Duration::from_secs(20));
        assert_eq!(browse(&format!("http://127.0.0.1:{port}/{path}?code=gone"), "GET", None), (302, Some(format!("{CONNECTED}&failed=1"))));
        assert_eq!(server.join().unwrap(), (Served::Done, None));
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
        // Or by its time, with no code at all.
        let callback = Callback::bind().unwrap();
        let port = callback.port();
        let started = Instant::now();
        let server = serving(&api, &link, callback, Arc::default(), Duration::from_millis(300));
        assert_eq!(server.join().unwrap(), (Served::TimedOut, None));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
        // An exchange refused for good or for now, in the words Orbiters used.
        assert!(matches!(api.exchange_link("exchange-1", "gone"), Exchange::Ended(m) if m.contains("ended")));
        assert!(matches!(api.exchange_link("exchange-1", "wrong"), Exchange::Retry(m) if m.contains("does not match")));
        assert!(matches!(api.exchange_link("other", "good"), Exchange::Ended(m) if m.contains("expired")));
        assert!(matches!(Api::at("http://127.0.0.1:9/").exchange_link("exchange-1", "good"), Exchange::Retry(m) if m.contains("could not be reached")));
        let fragment = Link { link_url: String::new(), connected_url: "https://orbiters.cc/done#top".into(), exchange_token: String::new() };
        assert_eq!(fragment.failed_url(), "https://orbiters.cc/done?failed=1#top");
    }

    /// The browser pairing as Orbiters - N4N0 runs it, against a disposable server: ORBITERS_TEST_API (its address) and
    /// ORBITERS_TEST_JWT (a website token of a member there, who clicks Connect in place of the browser). Run by hand:
    /// cargo test -p orbiters-core account::tests::signs_in_through_the_browser_pairing -- --ignored --nocapture
    #[test]
    #[ignore]
    fn signs_in_through_the_browser_pairing() {
        let base = std::env::var("ORBITERS_TEST_API").expect("ORBITERS_TEST_API");
        let jwt = std::env::var("ORBITERS_TEST_JWT").expect("ORBITERS_TEST_JWT");
        let api = Api::at(&base);
        let callback = Callback::bind().unwrap();
        let (port, path) = (callback.port(), callback.path().to_string());
        let link = api.begin_link(port, &path).unwrap();
        assert!(link.link_url.contains("app=desktop") && link.connected_url.contains("client=desktop"), "{link:?}");
        // Before Connect, the secret alone trades nothing.
        assert!(matches!(api.exchange_link(&link.exchange_token, "nothing-yet"), Exchange::Retry(_)));
        let token = link.link_url.split("token=").nth(1).unwrap().split('&').next().unwrap().to_string();
        // Connect as the website clicks it: the browser is sent to this tool's listener with the code.
        let confirmed = api
            .agent()
            .post(&api.url(&format!("editor-links/{token}/confirm")))
            .header("Authorization", &format!("Bearer {jwt}"))
            .send_json(serde_json::json!({ "app": "desktop" }))
            .unwrap();
        assert_eq!(confirmed.status().as_u16(), 200);
        let redirect: serde_json::Value = serde_json::from_str(&confirmed.into_body().read_to_string().unwrap()).unwrap();
        let redirect = redirect["redirectTo"].as_str().unwrap().to_string();
        assert!(redirect.starts_with(&format!("http://127.0.0.1:{port}/{path}?code=")), "{redirect}");
        let server = serving(&api, &link, callback, Arc::default(), Duration::from_secs(60));
        assert_eq!(browse(&redirect, "GET", None), (302, Some(link.connected_url.clone())));
        let (served, account) = server.join().unwrap();
        assert_eq!(served, Served::Done);
        let account = account.expect("signed in");
        assert!(account.token.starts_with("orbit-"));
        eprintln!("signed in as {:?}", account.username);
        // The credential works, and only once is it handed out.
        assert!(matches!(api.check(&account.token).unwrap(), Check::Valid(_)));
        assert!(matches!(api.exchange_link(&link.exchange_token, redirect.split("code=").nth(1).unwrap()), Exchange::Ended(_)));
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
