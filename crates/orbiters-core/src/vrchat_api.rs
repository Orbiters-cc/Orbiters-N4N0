//! An independent VRChat transport. Cookies never enter the Orbiters HTTP agent or API.
//! Blocking methods belong on background threads. No credentials, codes, cookies or response bodies are logged.
//! Reads, and one change: the picture of an avatar or world the signed-in account uploaded ([`replace_image`]).
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant};

const BASE: &str = "https://api.vrchat.cloud/api/1";
const UA: &str = concat!("Orbiters-N4N0/", env!("CARGO_PKG_VERSION"), " blackorbit.eclaire@gmail.com");
const ENTROPY: &[u8] = b"Orbiters.N4N0 VRChat session";

/// A short-lived grant for one Orbiters identity. Revocation is permanent: a new grant needs a new client.
/// The deadline is checked even while the controller is waiting for a network response.
pub struct Gate {
    revoked: AtomicBool,
    expires: Instant,
}
impl Default for Gate {
    fn default() -> Self {
        Self::for_duration(Duration::ZERO)
    }
}
impl Gate {
    pub fn for_duration(lifetime: Duration) -> Self {
        Self { revoked: AtomicBool::new(false), expires: Instant::now() + lifetime }
    }
    pub fn allowed(&self) -> bool {
        !self.revoked.load(Ordering::Acquire) && Instant::now() < self.expires
    }
    pub fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Disabled,
    Unauthorized,
    WrongCode,
    RateLimited(Duration),
    Unavailable,
    InvalidResponse,
    Storage,
    NotFound,
    UnsafeUrl,
    /// The avatar or world was uploaded by another account.
    NotYours,
    /// VRChat's storage refused an upload.
    Upload,
    /// VRChat did not finish checking an upload in time.
    Processing,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => f.write_str("VRChat sign-in is not available for this Orbiters account."),
            Self::Unauthorized => f.write_str("Your VRChat session expired. Sign in again."),
            Self::WrongCode => f.write_str("That code was not accepted. Check it and try again."),
            Self::RateLimited(d) => write!(f, "VRChat is rate limiting requests. Try again in {} seconds.", d.as_secs().max(1)),
            Self::Unavailable => f.write_str("VRChat could not be reached. Try again shortly."),
            Self::InvalidResponse => f.write_str("VRChat returned an unexpected answer. Try again shortly."),
            Self::Storage => f.write_str("The VRChat session could not be saved securely on this computer."),
            Self::NotFound => f.write_str("VRChat does not show this item to your account."),
            Self::UnsafeUrl => f.write_str("VRChat returned an unsupported image address."),
            Self::NotYours => f.write_str("Only the VRChat account that uploaded it can change its picture."),
            Self::Upload => f.write_str("VRChat's storage did not accept the picture. Try again shortly."),
            Self::Processing => f.write_str("VRChat is still processing the picture. Try again in a minute."),
        }
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

/// Password/code carried over the authenticated local IPC link. Debug output never prints its contents.
#[derive(Clone, Serialize, Deserialize)]
pub struct Secret(pub String);
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("(hidden)")
    }
}

/// No Debug or public cookie access: the only serialization is inside encrypted local storage.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq)]
struct Cookies {
    auth: String,
    two_factor_auth: String,
}
impl Cookies {
    fn header(&self) -> String {
        [("auth", &self.auth), ("twoFactorAuth", &self.two_factor_auth)]
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
    fn absorb(&mut self, headers: &[String]) {
        for header in headers {
            let Some((name, value)) = header.split(';').next().unwrap_or_default().split_once('=') else { continue };
            // Cookie values are opaque RFC 6265 cookie-octets, never additional headers or cookies.
            if !value
                .bytes()
                .all(|b| b == 0x21 || (0x23..=0x2b).contains(&b) || (0x2d..=0x3a).contains(&b) || (0x3c..=0x5b).contains(&b) || (0x5d..=0x7e).contains(&b))
            {
                continue;
            }
            let expired = header.split(';').filter_map(|a| a.trim().split_once('=')).any(|(name, value)| {
                if name.eq_ignore_ascii_case("max-age") {
                    return value.trim().parse::<i64>().is_ok_and(|n| n <= 0);
                }
                name.eq_ignore_ascii_case("expires")
                    && time::OffsetDateTime::parse(value.trim(), &time::format_description::well_known::Rfc2822)
                        .is_ok_and(|date| date <= time::OffsetDateTime::now_utc())
            });
            let value = if expired { "" } else { value };
            match name.trim() {
                "auth" => self.auth = value.into(),
                "twoFactorAuth" => self.two_factor_auth = value.into(),
                _ => {}
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TwoFactor {
    Totp,
    Email,
    Recovery,
}
impl TwoFactor {
    pub fn label(self) -> &'static str {
        match self {
            Self::Totp => "Authenticator code",
            Self::Email => "Email code",
            Self::Recovery => "Recovery code",
        }
    }
    fn path(self) -> &'static str {
        match self {
            Self::Totp => "totp",
            Self::Email => "emailotp",
            Self::Recovery => "otp",
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub user_icon: String,
    #[serde(default)]
    pub icon_url: String,
    #[serde(default)]
    pub current_avatar_thumbnail_image_url: String,
    #[serde(default)]
    pub current_avatar_image_url: String,
}
impl User {
    pub fn picture(&self) -> &str {
        [&self.user_icon, &self.icon_url, &self.current_avatar_thumbnail_image_url, &self.current_avatar_image_url]
            .into_iter()
            .find(|url| !url.is_empty())
            .map_or("", String::as_str)
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Avatar {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub thumbnail_image_url: String,
    #[serde(default)]
    pub image_url: String,
    /// The account that uploaded it (`usr_…`); empty when the answer did not say.
    #[serde(default)]
    pub author_id: String,
}
/// A world as VRChat describes it to the signed-in account (its own private uploads too).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct World {
    pub id: String,
    pub name: String,
    pub thumbnail_image_url: String,
    pub image_url: String,
    pub author_id: String,
}
/// What a picture belongs to, told by its ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Avatar,
    World,
}
impl Kind {
    pub fn of(id: &str) -> Option<Self> {
        if id.starts_with("avtr_") {
            Some(Self::Avatar)
        } else if id.starts_with("wrld_") {
            Some(Self::World)
        } else {
            None
        }
    }
    /// Its API collection, which is also the request family it is paced in.
    fn collection(self) -> &'static str {
        match self {
            Self::Avatar => "avatars",
            Self::World => "worlds",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Avatar => "Avatar",
            Self::World => "World",
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum Login {
    SignedIn(User),
    TwoFactor(Vec<TwoFactor>),
}
fn parse_login(body: &[u8]) -> Result<Login> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| Error::InvalidResponse)?;
    if let Some(methods) = v.get("requiresTwoFactorAuth").and_then(|v| v.as_array()) {
        let methods: Vec<_> = methods
            .iter()
            .filter_map(|m| match m.as_str()? {
                "totp" => Some(TwoFactor::Totp),
                "emailOtp" => Some(TwoFactor::Email),
                "otp" => Some(TwoFactor::Recovery),
                _ => None,
            })
            .collect();
        if methods.is_empty() {
            return Err(Error::InvalidResponse);
        }
        return Ok(Login::TwoFactor(methods));
    }
    let user: User = serde_json::from_value(v).map_err(|_| Error::InvalidResponse)?;
    if !user.id.starts_with("usr_") {
        return Err(Error::InvalidResponse);
    }
    Ok(Login::SignedIn(user))
}

/// Individually URL-encode the two fields before joining them (VRChat's Basic auth contract).
pub fn basic_header(username: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{}:{}", encode(username), encode(password))))
}
pub(crate) fn encode(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char)
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

struct Request {
    method: &'static str,
    url: String,
    cookie: String,
    basic: Option<String>,
    body: Option<String>,
    /// Bytes for a storage address VRChat handed out: sent alone, with their type and MD5 and no cookie.
    upload: Option<Upload>,
}
struct Upload {
    mime: &'static str,
    /// Base64, as `Content-MD5` carries it.
    md5: String,
    bytes: Vec<u8>,
}
struct Response {
    status: u16,
    cookies: Vec<String>,
    retry_after: Option<String>,
    location: Option<String>,
    body: Vec<u8>,
}
trait Transport: Send + Sync {
    fn send(&self, request: Request) -> Result<Response>;
}
struct Http(ureq::Agent);
impl Transport for Http {
    fn send(&self, r: Request) -> Result<Response> {
        // Redirects are handled explicitly below; neither Basic auth nor cookies follow an untrusted redirect.
        let response = match (r.method, r.upload) {
            ("GET", _) => {
                let mut q = self.0.get(&r.url);
                if !r.cookie.is_empty() {
                    q = q.header("Cookie", &r.cookie);
                }
                if let Some(basic) = r.basic {
                    q = q.header("Authorization", &basic);
                }
                q.call()
            }
            ("POST", _) => self.0.post(&r.url).header("Cookie", &r.cookie).header("Content-Type", "application/json").send(r.body.unwrap_or_default()),
            // A picture can take longer than an API answer on a slow connection.
            ("PUT", Some(u)) => self
                .0
                .put(&r.url)
                .config()
                .timeout_global(Some(Duration::from_secs(300)))
                .build()
                .header("Content-Type", u.mime)
                .header("Content-MD5", &u.md5)
                .send(&u.bytes[..]),
            ("PUT", None) => match r.body {
                Some(body) => self.0.put(&r.url).header("Cookie", &r.cookie).header("Content-Type", "application/json").send(body),
                None => self.0.put(&r.url).header("Cookie", &r.cookie).send_empty(),
            },
            _ => return Err(Error::InvalidResponse),
        }
        .map_err(|_| Error::Unavailable)?;
        let status = response.status().as_u16();
        let cookies = response.headers().get_all("set-cookie").iter().filter_map(|h| h.to_str().ok().map(str::to_owned)).collect();
        let retry_after = response.headers().get("retry-after").and_then(|h| h.to_str().ok()).map(str::to_owned);
        let location = response.headers().get("location").and_then(|h| h.to_str().ok()).map(str::to_owned);
        let body = response.into_body().with_config().limit(16 * 1024 * 1024).read_to_vec().map_err(|_| Error::InvalidResponse)?;
        Ok(Response { status, cookies, retry_after, location, body })
    }
}

#[derive(Default)]
struct Limiter {
    next: HashMap<&'static str, Instant>,
    blocked: Option<Instant>,
    /// Tests of long request sequences go without the one-second spacing.
    #[cfg(test)]
    unpaced: bool,
}
impl Limiter {
    fn wait(&self, family: &'static str, now: Instant) -> Duration {
        self.next.get(family).copied().into_iter().chain(self.blocked).max().map_or(Duration::ZERO, |t| t.saturating_duration_since(now))
    }
    fn used(&mut self, family: &'static str, now: Instant) {
        #[cfg(test)]
        if self.unpaced {
            return;
        }
        self.next.insert(family, now + Duration::from_secs(1));
    }
}
fn retry_after(value: Option<&str>) -> Duration {
    if let Some(s) = value.and_then(|s| s.trim().parse::<u64>().ok()) {
        return Duration::from_secs(s.max(1).min(86400 * 365));
    }
    if let Some(date) = value.and_then(|s| time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc2822).ok()) {
        return Duration::from_secs((date - time::OffsetDateTime::now_utc()).whole_seconds().max(1) as u64);
    }
    Duration::from_secs(60)
}

/// Reusable session, deliberately unrelated to the Orbiters client. The gate is checked on every request and cache read.
pub struct Client {
    /// Shared with [`replace_image`]'s storage uploads, which run without the client locked.
    transport: Arc<dyn Transport>,
    cookies: Cookies,
    allowed: Arc<Gate>,
    limiter: Arc<Mutex<Limiter>>,
    user: Option<User>,
    pending_methods: Vec<TwoFactor>,
    avatars: HashMap<String, (Instant, Avatar)>,
    worlds: HashMap<String, (Instant, World)>,
    friends: Option<(Instant, Vec<User>)>,
    instances: HashMap<(String, String), (Instant, Option<bool>)>,
    store: Option<PathBuf>,
}
impl Client {
    pub fn new(allowed: Arc<Gate>) -> Self {
        // Changing Orbiters accounts or refreshing the feature cannot reset the rate limit or a 429 backoff.
        static LIMITER: OnceLock<Arc<Mutex<Limiter>>> = OnceLock::new();
        let agent = ureq::Agent::config_builder()
            .tls_config(crate::http_tls())
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(UA)
            .build()
            .new_agent();
        Self {
            transport: Arc::new(Http(agent)),
            cookies: Cookies::default(),
            allowed,
            limiter: LIMITER.get_or_init(Default::default).clone(),
            user: None,
            pending_methods: Vec::new(),
            avatars: HashMap::new(),
            worlds: HashMap::new(),
            friends: None,
            instances: HashMap::new(),
            store: dirs::data_local_dir().map(|d| d.join("Orbiters/vrchat-session.bin")),
        }
    }
    fn permitted(&self) -> Result<()> {
        if self.allowed.allowed() {
            Ok(())
        } else {
            Err(Error::Disabled)
        }
    }
    pub fn signed_in(&self) -> bool {
        self.allowed.allowed() && self.user.is_some()
    }
    fn authenticated(&self) -> Result<()> {
        self.permitted()?;
        if self.user.is_some() {
            Ok(())
        } else {
            Err(Error::Unauthorized)
        }
    }
    /// Called only after the feature check. A disabled feature never reads or decrypts this file.
    pub fn restore(&mut self) -> Result<bool> {
        self.permitted()?;
        let path = self.store.as_ref().ok_or(Error::Storage)?;
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err(Error::Storage),
        };
        if bytes.len() > 64 * 1024 {
            return Err(Error::Storage);
        }
        let plain = crate::dpapi::crypt(&bytes, ENTROPY, false).map_err(|_| Error::Storage)?;
        self.permitted()?;
        self.cookies = serde_json::from_slice(&plain).map_err(|_| Error::Storage)?;
        Ok(!self.cookies.auth.is_empty())
    }
    fn save(&self) -> Result<()> {
        self.permitted()?;
        // Unit tests have no store and never touch the person's session.
        let Some(path) = self.store.as_ref() else { return Ok(()) };
        let plain = serde_json::to_vec(&self.cookies).map_err(|_| Error::Storage)?;
        let sealed = crate::dpapi::crypt(&plain, ENTROPY, true).map_err(|_| Error::Storage)?;
        std::fs::create_dir_all(path.parent().ok_or(Error::Storage)?).map_err(|_| Error::Storage)?;
        crate::write_atomic_private(path, &sealed).map_err(|_| Error::Storage)
    }
    fn forget(&mut self) -> Result<()> {
        self.cookies = Cookies::default();
        self.user = None;
        self.pending_methods.clear();
        self.avatars.clear();
        self.worlds.clear();
        self.friends = None;
        self.instances.clear();
        if let Some(path) = &self.store {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(Error::Storage),
            }
        }
        Ok(())
    }
    fn pace(&self, family: &'static str) -> Result<()> {
        loop {
            self.permitted()?;
            let wait = {
                let mut limiter = self.limiter.lock().unwrap();
                let now = Instant::now();
                if let Some(blocked) = limiter.blocked {
                    let wait = blocked.saturating_duration_since(now);
                    if !wait.is_zero() {
                        return Err(Error::RateLimited(wait));
                    }
                }
                let wait = limiter.wait(family, now);
                if wait.is_zero() {
                    limiter.used(family, now);
                    return self.permitted();
                }
                wait
            };
            std::thread::sleep(wait);
        }
    }
    fn back_off(&self, response: &Response) -> Error {
        let delay = retry_after(response.retry_after.as_deref());
        let until = Instant::now() + delay;
        let mut limiter = self.limiter.lock().unwrap();
        limiter.blocked = Some(limiter.blocked.map_or(until, |old| old.max(until)));
        Error::RateLimited(delay)
    }
    fn request(&mut self, family: &'static str, method: &'static str, url: String, basic: Option<String>, body: Option<String>) -> Result<Response> {
        self.pace(family)?;
        let verifying = family == "auth" && url.contains("/twofactorauth/") && url.ends_with("/verify");
        let session_host = url.starts_with("https://api.vrchat.cloud/") || url.starts_with("https://files.vrchat.cloud/");
        let cookie = if session_host { self.cookies.header() } else { String::new() };
        let response = self.transport.send(Request { method, url, cookie, basic, body, upload: None })?;
        // Backoff survives a revoked grant. No response data or cookies do.
        if response.status == 429 {
            return Err(self.back_off(&response));
        }
        self.permitted()?;
        let before = self.cookies.clone();
        if session_host {
            self.cookies.absorb(&response.cookies);
        }
        if !before.auth.is_empty() && self.cookies.auth.is_empty() {
            self.forget()?;
            return Err(Error::Unauthorized);
        }
        match response.status {
            400 if verifying => Err(Error::WrongCode),
            401 if !session_host => Err(Error::NotFound),
            401 => {
                self.forget()?;
                Err(Error::Unauthorized)
            }
            404 | 403 => Err(Error::NotFound),
            500..=599 => Err(Error::Unavailable),
            200..=399 => {
                if self.cookies != before && self.user.is_some() {
                    self.save()?;
                }
                Ok(response)
            }
            _ => Err(Error::InvalidResponse),
        }
    }
    fn get(&mut self, family: &'static str, path: &str) -> Result<Response> {
        self.request(family, "GET", format!("{BASE}{path}"), None, None)
    }
    fn accept_login(&mut self, body: &[u8]) -> Result<Login> {
        let login = parse_login(body)?;
        if self.cookies.auth.is_empty() {
            return Err(Error::InvalidResponse);
        }
        match &login {
            Login::SignedIn(user) => {
                self.save()?;
                self.user = Some(user.clone());
                self.pending_methods.clear();
            }
            Login::TwoFactor(methods) => {
                self.user = None;
                self.avatars.clear();
                self.worlds.clear();
                self.friends = None;
                self.instances.clear();
                self.pending_methods = methods.clone();
            }
        }
        Ok(login)
    }
    pub fn sign_in(&mut self, username: &str, password: &str) -> Result<Login> {
        self.permitted()?;
        self.forget()?;
        let response = self.request("auth", "GET", format!("{BASE}/auth/user"), Some(basic_header(username, password)), None)?;
        self.accept_login(&response.body)
    }
    pub fn verify(&mut self, method: TwoFactor, code: &str) -> Result<Login> {
        self.permitted()?;
        if !self.pending_methods.contains(&method) {
            return Err(Error::InvalidResponse);
        }
        // A successful verification followed by a failed user lookup must not consume a recovery code twice.
        if !self.cookies.two_factor_auth.is_empty() {
            return self.current_login();
        }
        let body = serde_json::json!({"code":code}).to_string();
        let response = self.request("auth", "POST", format!("{BASE}/auth/twofactorauth/{}/verify", method.path()), None, Some(body));
        let response = response?;
        let value: serde_json::Value = serde_json::from_slice(&response.body).map_err(|_| Error::InvalidResponse)?;
        if value["verified"] != true {
            return Err(Error::WrongCode);
        }
        self.current_login()
    }
    pub fn current_login(&mut self) -> Result<Login> {
        let r = self.get("auth", "/auth/user")?;
        self.accept_login(&r.body)
    }
    pub fn cancel_login(&mut self) -> Result<()> {
        self.permitted()?;
        self.forget()
    }
    /// Retry a saved or partly completed session without sending a password or repeating verification.
    pub fn resume(&mut self) -> Result<Option<Login>> {
        self.permitted()?;
        if self.cookies.auth.is_empty() && !self.restore()? {
            return Ok(None);
        }
        self.current_login().map(Some)
    }
    pub fn current_user(&mut self) -> Result<User> {
        match self.current_login()? {
            Login::SignedIn(u) => Ok(u),
            _ => Err(Error::Unauthorized),
        }
    }
    /// Forget locally even if VRChat is offline. Remote logout is best effort, with any failure reported.
    pub fn sign_out(&mut self) -> Result<()> {
        self.permitted()?;
        let cookie = self.cookies.header();
        self.forget()?;
        if cookie.is_empty() {
            return Ok(());
        }
        self.pace("auth")?;
        let r = self.transport.send(Request { method: "PUT", url: format!("{BASE}/logout"), cookie, basic: None, body: None, upload: None })?;
        if r.status == 429 {
            return Err(self.back_off(&r));
        }
        if (200..300).contains(&r.status) || r.status == 401 {
            Ok(())
        } else {
            Err(Error::Unavailable)
        }
    }
    pub fn avatar(&mut self, id: &str) -> Result<Avatar> {
        self.authenticated()?;
        if let Some((at, a)) = self.avatars.get(id).filter(|(at, _)| at.elapsed() < Duration::from_secs(86400)) {
            let _ = at;
            return Ok(a.clone());
        }
        let r = self.get("avatars", &format!("/avatars/{}", encode(id)))?;
        self.keep_avatar(id, &r.body)
    }
    fn keep_avatar(&mut self, id: &str, body: &[u8]) -> Result<Avatar> {
        let a: Avatar = serde_json::from_slice(body).map_err(|_| Error::InvalidResponse)?;
        if self.avatars.len() >= 512 {
            self.avatars.clear();
        }
        self.avatars.insert(id.into(), (Instant::now(), a.clone()));
        Ok(a)
    }
    /// A world, cached for a day like avatars.
    pub fn world(&mut self, id: &str) -> Result<World> {
        self.authenticated()?;
        if let Some((_, w)) = self.worlds.get(id).filter(|(at, _)| at.elapsed() < Duration::from_secs(86400)) {
            return Ok(w.clone());
        }
        self.fetch_world(id)
    }
    /// Always asks (a world's thumbnail changes with its uploads), and keeps the answer for [`Client::world`].
    fn fetch_world(&mut self, id: &str) -> Result<World> {
        let r = self.get("worlds", &format!("/worlds/{}", encode(id)))?;
        self.keep_world(id, &r.body)
    }
    fn keep_world(&mut self, id: &str, body: &[u8]) -> Result<World> {
        let w: World = serde_json::from_slice(body).map_err(|_| Error::InvalidResponse)?;
        if self.worlds.len() >= 512 {
            self.worlds.clear();
        }
        self.worlds.insert(id.into(), (Instant::now(), w.clone()));
        Ok(w)
    }
    /// A world's thumbnail address (its own uploads included, public or not).
    pub fn world_thumbnail(&mut self, id: &str) -> Result<String> {
        self.authenticated()?;
        let w = self.fetch_world(id)?;
        Ok(if w.thumbnail_image_url.is_empty() { w.image_url } else { w.thumbnail_image_url })
    }
    /// Whether the signed-in account uploaded `id` (an avatar or a world), from the day-long caches. An ID VRChat does not
    /// show to the account, or of another kind, is not its own.
    pub fn owns(&mut self, id: &str) -> Result<bool> {
        self.authenticated()?;
        let me = self.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
        let author = match Kind::of(id) {
            Some(Kind::Avatar) => self.avatar(id).map(|a| a.author_id),
            Some(Kind::World) => self.world(id).map(|w| w.author_id),
            None => return Ok(false),
        };
        match author {
            Ok(author) => Ok(!author.is_empty() && author == me),
            Err(Error::NotFound) => Ok(false),
            Err(e) => Err(e),
        }
    }
    pub fn friends(&mut self) -> Result<Vec<User>> {
        self.authenticated()?;
        if let Some((_, friends)) = self.friends.as_ref().filter(|(at, _)| at.elapsed() < Duration::from_secs(300)) {
            return Ok(friends.clone());
        }
        let mut all = BTreeMap::new();
        for offline in [false, true] {
            let mut offset = 0;
            loop {
                let r = self.get("friends", &format!("/auth/user/friends?offline={offline}&n=100&offset={offset}"))?;
                let page: Vec<User> = serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)?;
                let n = page.len();
                for u in page {
                    all.insert(u.id.clone(), u);
                }
                if n < 100 {
                    break;
                }
                offset += 100;
                if offset > 10000 {
                    return Err(Error::InvalidResponse);
                }
            }
        }
        let all: Vec<_> = all.into_values().collect();
        self.friends = Some((Instant::now(), all.clone()));
        Ok(all)
    }
    pub fn user(&mut self, id: &str) -> Result<User> {
        self.authenticated()?;
        let r = self.get("users", &format!("/users/{}", encode(id)))?;
        serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)
    }
    /// None when access is denied: a hidden instance cannot be declared dead.
    pub fn instance_alive(&mut self, world: &str, instance: &str) -> Result<Option<bool>> {
        self.authenticated()?;
        let key = (world.to_owned(), instance.to_owned());
        if let Some((_, alive)) = self.instances.get(&key).filter(|(at, _)| at.elapsed() < Duration::from_secs(60)) {
            return Ok(*alive);
        }
        let alive = match self.get("instances", &format!("/instances/{}:{}", encode(world), encode(instance))) {
            Ok(r) => {
                let v: serde_json::Value = serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)?;
                Ok(v.get("active").and_then(|v| v.as_bool()))
            }
            Err(Error::NotFound) => Ok(None),
            Err(e) => Err(e),
        }?;
        if self.instances.len() >= 256 {
            self.instances.clear();
        }
        self.instances.insert(key, (Instant::now(), alive));
        Ok(alive)
    }
    /// Only VRChat-owned image hosts. Redirects to any other host are refused before credentials leave the process.
    pub fn image(&mut self, url: &str) -> Result<Vec<u8>> {
        self.authenticated()?;
        let mut url = url.to_string();
        for _ in 0..5 {
            if !image_url_allowed(&url) {
                return Err(Error::UnsafeUrl);
            }
            let r = self.request("images", "GET", url, None, None)?;
            if (200..300).contains(&r.status) {
                return Ok(r.body);
            }
            url = r.location.ok_or(Error::InvalidResponse)?;
        }
        Err(Error::InvalidResponse)
    }

    // The file API, as VRChat's SDK and VRCX use it for a picture. Each call checks the session again: a sign-out during
    // an upload stops it at its next step.
    /// The item asked again (not from the caches): who uploaded it, its name and picture.
    fn fresh(&mut self, kind: Kind, id: &str) -> Result<World> {
        self.authenticated()?;
        let r = self.get(kind.collection(), &format!("/{}/{}", kind.collection(), encode(id)))?;
        if kind == Kind::Avatar {
            self.keep_avatar(id, &r.body)?;
        }
        self.keep_world_like(kind, id, &r.body)
    }
    /// An avatar's answer read for the fields it shares with a world's; a world's kept for [`Client::world`].
    fn keep_world_like(&mut self, kind: Kind, id: &str, body: &[u8]) -> Result<World> {
        match kind {
            Kind::World => self.keep_world(id, body),
            Kind::Avatar => serde_json::from_slice(body).map_err(|_| Error::InvalidResponse),
        }
    }
    fn file(&mut self, id: &str) -> Result<RemoteFile> {
        self.authenticated()?;
        let r = self.get("files", &format!("/file/{}", encode(id)))?;
        serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)
    }
    fn create_file(&mut self, name: &str) -> Result<RemoteFile> {
        self.authenticated()?;
        let body = serde_json::json!({"name": name, "mimeType": "image/png", "extension": ".png"}).to_string();
        let r = self.request("files", "POST", format!("{BASE}/file"), None, Some(body))?;
        serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)
    }
    /// A new version of file `id` for a picture of `png` bytes with signature `signature`: its number.
    fn add_version(&mut self, id: &str, file: &Part, signature: &Part) -> Result<u64> {
        self.authenticated()?;
        let body = version_body(file, signature);
        let r = self.request("files", "POST", format!("{BASE}/file/{}", encode(id)), None, Some(body))?;
        let f: RemoteFile = serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)?;
        f.latest().filter(|v| v.version > 0).map(|v| v.version).ok_or(Error::InvalidResponse)
    }
    /// The storage address the `part` (`file` or `signature`) of a version goes to.
    fn start_part(&mut self, id: &str, version: u64, part: &str) -> Result<String> {
        self.authenticated()?;
        let r = self.request("files", "PUT", format!("{BASE}/file/{}/{version}/{part}/start", encode(id)), None, None)?;
        let v: serde_json::Value = serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)?;
        v["url"].as_str().filter(|u| !u.is_empty()).map(str::to_owned).ok_or(Error::InvalidResponse)
    }
    fn finish_part(&mut self, id: &str, version: u64, part: &str) -> Result<()> {
        self.authenticated()?;
        // One part, so no ETags (the signature never has any); the two counters are always zero, in strings.
        let body = serde_json::json!({"maxParts": "0", "nextPartNumber": "0"}).to_string();
        self.request("files", "PUT", format!("{BASE}/file/{}/{version}/{part}/finish", encode(id)), None, Some(body)).map(drop)
    }
    /// Points the item at its new picture; VRChat's answer must name it.
    fn set_image(&mut self, kind: Kind, id: &str, image_url: &str) -> Result<Replaced> {
        self.authenticated()?;
        let body = serde_json::json!({"id": id, "imageUrl": image_url}).to_string();
        let r = self.request(kind.collection(), "PUT", format!("{BASE}/{}/{}", kind.collection(), encode(id)), None, Some(body))?;
        if kind == Kind::Avatar {
            self.keep_avatar(id, &r.body)?;
        }
        let item = self.keep_world_like(kind, id, &r.body)?;
        if item.image_url != image_url {
            return Err(Error::InvalidResponse);
        }
        Ok(Replaced { image_url: item.image_url, thumbnail_image_url: item.thumbnail_image_url })
    }
}

/// A picture's new addresses on VRChat, once [`replace_image`] pointed its item at them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Replaced {
    pub image_url: String,
    pub thumbnail_image_url: String,
}
/// Where [`replace_image`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Step {
    /// Making sure the signed-in account uploaded the item.
    Checking,
    /// Adding a version to the item's picture file.
    Preparing,
    Uploading,
    /// Uploading the picture's librsync signature, which VRChat's files keep beside each version.
    Signing,
    /// Waiting for VRChat to call the version complete.
    Processing,
    /// Pointing the item at its new picture.
    Saving,
}
impl Step {
    pub fn label(self) -> &'static str {
        match self {
            Self::Checking => "Checking it is yours…",
            Self::Preparing => "Preparing the upload…",
            Self::Uploading => "Uploading the picture…",
            Self::Signing => "Uploading its signature…",
            Self::Processing => "VRChat is checking it…",
            Self::Saving => "Saving…",
        }
    }
    /// How far the replacement is (0 to 1) once this step starts.
    pub fn progress(self) -> f32 {
        match self {
            Self::Checking => 0.04,
            Self::Preparing => 0.14,
            Self::Uploading => 0.24,
            Self::Signing => 0.66,
            Self::Processing => 0.76,
            Self::Saving => 0.9,
        }
    }
}

/// A part of a file version: its bytes, type and base64 MD5 (the form both VRChat's file API and `Content-MD5` take).
struct Part<'a> {
    bytes: &'a [u8],
    mime: &'static str,
    md5: String,
}
impl<'a> Part<'a> {
    fn new(bytes: &'a [u8], mime: &'static str) -> Self {
        Self { bytes, mime, md5: md5_base64(bytes) }
    }
}
fn version_body(file: &Part, signature: &Part) -> String {
    serde_json::json!({
        "signatureMd5": signature.md5,
        "signatureSizeInBytes": signature.bytes.len(),
        "fileMd5": file.md5,
        "fileSizeInBytes": file.bytes.len(),
    })
    .to_string()
}
fn md5_base64(bytes: &[u8]) -> String {
    STANDARD.encode(md5(bytes))
}
/// MD5 (RFC 1321), which VRChat's file API checks uploads with.
fn md5(data: &[u8]) -> [u8; 16] {
    const SHIFT: [u32; 16] = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be,
        0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
        0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c,
        0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
        0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1, 0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1,
        0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
    ];
    let mut state: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_le_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (w, c) in m.iter_mut().zip(chunk.chunks_exact(4)) {
            *w = u32::from_le_bytes(c.try_into().unwrap());
        }
        let [mut a, mut b, mut c, mut d] = state;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(SHIFT[(i / 16) * 4 + i % 4]));
        }
        for (s, v) in state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }
    let mut out = [0u8; 16];
    for (o, s) in out.chunks_exact_mut(4).zip(state) {
        o.copy_from_slice(&s.to_le_bytes());
    }
    out
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteFile {
    id: String,
    #[serde(default)]
    owner_id: String,
    #[serde(default)]
    mime_type: String,
    #[serde(default)]
    versions: Vec<RemoteVersion>,
}
#[derive(Deserialize)]
struct RemoteVersion {
    version: u64,
    #[serde(default)]
    status: String,
}
impl RemoteFile {
    fn latest(&self) -> Option<&RemoteVersion> {
        self.versions.iter().max_by_key(|v| v.version)
    }
    /// A PNG file of `me` whose last version is complete takes the next version, as the SDK's uploads do. One with an
    /// upload left unfinished (here or by the SDK) is left alone: a new file is made instead, nothing deleted.
    fn takes_a_version(&self, me: &str) -> bool {
        self.owner_id == me && self.mime_type == "image/png" && self.latest().is_some_and(|v| v.status == "complete")
    }
}
/// The file a VRChat picture address names (`https://api.vrchat.cloud/api/1/file/file_<uuid>/<version>/file`).
fn file_id(url: &str) -> Option<&str> {
    url.split(['/', '?', '#']).find(|s| s.len() > 5 && s.starts_with("file_") && s[5..].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// Replaces the picture of `id`, an avatar or a world the signed-in account uploaded, with `png` (a PNG), the way
/// VRChat's SDK and VRCX do: a new version of the item's own picture file (or of a new file when it has none that
/// can take one) gets the picture and its librsync signature, each put to the storage address VRChat hands out for
/// it, and once VRChat calls the version complete the item is pointed at
/// `https://api.vrchat.cloud/api/1/file/<file>/<version>/file`. `progress` hears each step as it starts.
///
/// The owner is asked again first: anything another account uploaded is refused before a byte is sent. The client
/// is locked for each VRChat request only, so the uploads and the waits leave it to other requests. Nothing is
/// retried: a failure leaves at most an unfinished version of the account's own file, which the next attempt does
/// not reuse.
pub fn replace_image(client: &Mutex<Client>, id: &str, png: &[u8], mut progress: impl FnMut(Step)) -> Result<Replaced> {
    let kind = Kind::of(id).ok_or(Error::NotFound)?;
    progress(Step::Checking);
    let (me, item) = {
        let mut c = client.lock().unwrap();
        c.authenticated()?;
        let me = c.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
        (me, c.fresh(kind, id)?)
    };
    if item.author_id.is_empty() || item.author_id != me {
        return Err(Error::NotYours);
    }
    progress(Step::Preparing);
    let signature = signature(png);
    let parts = [("file", Part::new(png, "image/png")), ("signature", Part::new(&signature, "application/x-rsync-signature"))];
    let (file, version) = {
        let mut c = client.lock().unwrap();
        let existing = match file_id(&item.image_url) {
            Some(f) => match c.file(f) {
                Ok(file) if file.takes_a_version(&me) => Some(file.id),
                Ok(_) | Err(Error::NotFound) => None,
                Err(e) => return Err(e),
            },
            None => None,
        };
        let file = match existing {
            Some(f) => f,
            None => c.create_file(&format!("{} - {} - Image", kind.label(), item.name))?.id,
        };
        let version = c.add_version(&file, &parts[0].1, &parts[1].1)?;
        (file, version)
    };
    for ((name, part), step) in parts.iter().zip([Step::Uploading, Step::Signing]) {
        progress(step);
        let url = client.lock().unwrap().start_part(&file, version, name)?;
        put_part(client, &url, part)?;
        client.lock().unwrap().finish_part(&file, version, name)?;
    }
    progress(Step::Processing);
    let mut attempts = 0;
    loop {
        let status = {
            let mut c = client.lock().unwrap();
            let f = c.file(&file)?;
            f.versions.into_iter().find(|v| v.version == version).map(|v| v.status).unwrap_or_default()
        };
        match status.as_str() {
            "complete" => break,
            "waiting" | "queued" if attempts < 30 => {}
            "waiting" | "queued" => return Err(Error::Processing),
            _ => return Err(Error::InvalidResponse),
        }
        attempts += 1;
        // The client stays free meanwhile; a revoked grant ends the wait.
        for _ in 0..4 {
            std::thread::sleep(Duration::from_millis(500));
            client.lock().unwrap().permitted()?;
        }
    }
    progress(Step::Saving);
    client.lock().unwrap().set_image(kind, id, &format!("{BASE}/file/{file}/{version}/file"))
}
/// Puts a part to the storage address VRChat just gave for it, without the client locked: no session cookie or
/// credential goes with it and none is taken from the answer, and VRChat's back-off is not touched by the storage's.
fn put_part(client: &Mutex<Client>, url: &str, part: &Part) -> Result<()> {
    if !upload_url_allowed(url) {
        return Err(Error::UnsafeUrl);
    }
    let (transport, gate) = {
        let c = client.lock().unwrap();
        c.authenticated()?;
        c.pace("uploads")?;
        (c.transport.clone(), c.allowed.clone())
    };
    let upload = Upload { mime: part.mime, md5: part.md5.clone(), bytes: part.bytes.to_vec() };
    let r = transport.send(Request { method: "PUT", url: url.into(), cookie: String::new(), basic: None, body: None, upload: Some(upload) })?;
    if !gate.allowed() {
        return Err(Error::Disabled);
    }
    match r.status {
        200..=299 => Ok(()),
        500..=599 => Err(Error::Unavailable),
        _ => Err(Error::Upload),
    }
}
/// VRChat hands out Amazon S3 addresses for uploads (presigned for one object, valid minutes): HTTPS on the default
/// port, without credentials in the address. Anything else is refused before the picture leaves.
fn upload_url_allowed(url: &str) -> bool {
    let Ok(uri) = url.parse::<ureq::http::Uri>() else { return false };
    let host = uri.host().unwrap_or_default().to_ascii_lowercase();
    uri.scheme_str() == Some("https")
        && (host.ends_with(".amazonaws.com") || host == "files.vrchat.cloud")
        && uri.port_u16().is_none()
        && !uri.authority().is_some_and(|a| a.as_str().contains('@'))
}

/// The librsync signature VRChat's file versions keep beside each upload, as its SDK and VRCX make it (librsync.net's
/// defaults): a header of the BLAKE2 signature magic (`rs\x017`), the block length (2048) and the strong-sum length
/// (32), then for each 2 KiB block its rolling checksum and its BLAKE2b-256, all big-endian.
pub fn signature(data: &[u8]) -> Vec<u8> {
    const MAGIC: u32 = 0x7273_0137;
    const BLOCK: usize = 2048;
    const STRONG: usize = 32;
    let mut out = Vec::with_capacity(12 + data.len().div_ceil(BLOCK) * (4 + STRONG));
    for word in [MAGIC, BLOCK as u32, STRONG as u32] {
        out.extend_from_slice(&word.to_be_bytes());
    }
    for block in data.chunks(BLOCK) {
        out.extend_from_slice(&rollsum(block).to_be_bytes());
        out.extend_from_slice(&blake2b_256(block)[..STRONG]);
    }
    out
}
/// librsync's weak checksum (`rollsum.c`, with its character offset of 31): the low halves of its two sums.
fn rollsum(block: &[u8]) -> u32 {
    const OFFSET: u64 = 31;
    let (mut s1, mut s2) = (0u64, 0u64);
    for &b in block {
        s1 = s1.wrapping_add(b as u64 + OFFSET);
        s2 = s2.wrapping_add(s1);
    }
    ((s2 << 16) | (s1 & 0xffff)) as u32
}
/// BLAKE2b (RFC 7693) with a 32-byte digest and no key: librsync's strong sum.
fn blake2b_256(data: &[u8]) -> [u8; 32] {
    const IV: [u64; 8] = [
        0x6a09e667f3bcc908,
        0xbb67ae8584caa73b,
        0x3c6ef372fe94f82b,
        0xa54ff53a5f1d36f1,
        0x510e527fade682d1,
        0x9b05688c2b3e6c1f,
        0x1f83d9abfb41bd6b,
        0x5be0cd19137e2179,
    ];
    const SIGMA: [[usize; 16]; 10] = [
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
        [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
        [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
        [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
        [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
        [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
        [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
        [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
        [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
    ];
    fn compress(h: &mut [u64; 8], block: &[u8; 128], bytes: u128, last: bool) {
        let mut m = [0u64; 16];
        for (w, c) in m.iter_mut().zip(block.chunks_exact(8)) {
            *w = u64::from_le_bytes(c.try_into().unwrap());
        }
        let mut v = [0u64; 16];
        v[..8].copy_from_slice(h);
        v[8..].copy_from_slice(&IV);
        v[12] ^= bytes as u64;
        v[13] ^= (bytes >> 64) as u64;
        if last {
            v[14] = !v[14];
        }
        let mut g = |a: usize, b: usize, c: usize, d: usize, x: u64, y: u64| {
            v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
            v[d] = (v[d] ^ v[a]).rotate_right(32);
            v[c] = v[c].wrapping_add(v[d]);
            v[b] = (v[b] ^ v[c]).rotate_right(24);
            v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
            v[d] = (v[d] ^ v[a]).rotate_right(16);
            v[c] = v[c].wrapping_add(v[d]);
            v[b] = (v[b] ^ v[c]).rotate_right(63);
        };
        for round in 0..12 {
            let s = &SIGMA[round % 10];
            g(0, 4, 8, 12, m[s[0]], m[s[1]]);
            g(1, 5, 9, 13, m[s[2]], m[s[3]]);
            g(2, 6, 10, 14, m[s[4]], m[s[5]]);
            g(3, 7, 11, 15, m[s[6]], m[s[7]]);
            g(0, 5, 10, 15, m[s[8]], m[s[9]]);
            g(1, 6, 11, 12, m[s[10]], m[s[11]]);
            g(2, 7, 8, 13, m[s[12]], m[s[13]]);
            g(3, 4, 9, 14, m[s[14]], m[s[15]]);
        }
        for i in 0..8 {
            h[i] ^= v[i] ^ v[i + 8];
        }
    }
    let mut h = IV;
    // The parameter block: a 32-byte digest, no key, fanout and depth 1.
    h[0] ^= 0x0101_0000 ^ 32;
    let mut done = 0u128;
    let mut rest = data;
    // Every block but the last is compressed as it comes; the last (an empty one for empty data) is padded and final.
    while rest.len() > 128 {
        done += 128;
        compress(&mut h, rest[..128].try_into().unwrap(), done, false);
        rest = &rest[128..];
    }
    let mut last = [0u8; 128];
    last[..rest.len()].copy_from_slice(rest);
    compress(&mut h, &last, done + rest.len() as u128, true);
    let mut out = [0u8; 32];
    for (c, w) in out.chunks_exact_mut(8).zip(h) {
        c.copy_from_slice(&w.to_le_bytes());
    }
    out
}
fn image_url_allowed(url: &str) -> bool {
    let Ok(uri) = url.parse::<ureq::http::Uri>() else { return false };
    uri.scheme_str() == Some("https")
        && matches!(uri.host(), Some("api.vrchat.cloud" | "files.vrchat.cloud" | "assets.vrchat.com"))
        && uri.port_u16().is_none()
        && !uri.authority().is_some_and(|a| a.as_str().contains('@'))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    struct Mock {
        answers: Mutex<VecDeque<Response>>,
        seen: Arc<Mutex<Vec<Request>>>,
    }
    impl Transport for Mock {
        fn send(&self, r: Request) -> Result<Response> {
            self.seen.lock().unwrap().push(r);
            Ok(self.answers.lock().unwrap().pop_front().expect("unexpected HTTP request"))
        }
    }
    fn response(status: u16, body: &str, cookies: &[&str]) -> Response {
        Response { status, body: body.as_bytes().to_vec(), cookies: cookies.iter().map(|s| s.to_string()).collect(), retry_after: None, location: None }
    }
    fn mock(answers: Vec<Response>) -> (Client, Arc<Mutex<Vec<Request>>>) {
        let seen = Arc::new(Mutex::new(vec![]));
        let mut c = Client::new(Arc::new(Gate::for_duration(Duration::from_secs(3600))));
        c.transport = Arc::new(Mock { answers: Mutex::new(answers.into()), seen: seen.clone() });
        c.limiter = Default::default();
        c.store = None;
        c.user = Some(User { id: "usr_test".into(), display_name: "Tester".into(), ..Default::default() });
        c.cookies.auth = "mock-session".into();
        (c, seen)
    }
    pub(crate) fn expired_session() -> Client {
        mock(vec![response(401, "", &[])]).0
    }
    #[test]
    fn source_routes_avatar_image_and_instance_through_the_personal_session() {
        let (c, seen) = mock(vec![
            response(200, r#"{"id":"avtr_a","name":"Avatar","thumbnailImageUrl":"https://api.vrchat.cloud/image"}"#, &[]),
            response(200, "pixels", &[]),
            response(200, r#"{"active":true}"#, &[]),
        ]);
        let source = crate::vrchat_source::Source::new(Some(Arc::new(Mutex::new(c))), None, None);
        assert!(source.direct_available());
        let avatar = source.avatar("avtr_a").unwrap().unwrap();
        assert_eq!(source.image(&avatar.thumbnail_image_url).unwrap(), b"pixels");
        assert_eq!(source.instance_alive("wrld_a", "123").unwrap(), Some(true));
        assert_eq!(source.instance_alive("wrld_a", "123").unwrap(), Some(true));
        let calls = seen.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!(calls.iter().all(|r| r.cookie.contains("mock-session") && r.basic.is_none()));
    }
    #[test]
    fn basic_encodes_special_characters_before_base64() {
        assert_eq!(
            String::from_utf8(STANDARD.decode(basic_header("a:b +é", "p:@/%").strip_prefix("Basic ").unwrap()).unwrap()).unwrap(),
            "a%3Ab%20%2B%C3%A9:p%3A%40%2F%25"
        );
    }
    #[test]
    fn parses_each_two_factor_method() {
        for (wire, method) in [("totp", TwoFactor::Totp), ("otp", TwoFactor::Recovery), ("emailOtp", TwoFactor::Email)] {
            assert_eq!(parse_login(format!(r#"{{"requiresTwoFactorAuth":["{wire}"]}}"#).as_bytes()).unwrap(), Login::TwoFactor(vec![method]));
        }
        assert!(parse_login(br#"{"requiresTwoFactorAuth":["unknown"]}"#).is_err());
    }
    #[test]
    fn extracts_only_session_cookies_and_serializes() {
        let mut c = Cookies::default();
        c.absorb(&["auth=a=b; Path=/; HttpOnly".into(), "twoFactorAuth=second; Secure".into(), "other=ignored".into(), "auth=bad\r\nCookie: stolen".into()]);
        let c: Cookies = serde_json::from_slice(&serde_json::to_vec(&c).unwrap()).unwrap();
        assert_eq!(c.header(), "auth=a=b; twoFactorAuth=second");
    }
    #[test]
    fn limiter_separates_families_and_honors_backoff() {
        let now = Instant::now();
        let mut l = Limiter::default();
        l.used("auth", now);
        assert_eq!(l.wait("auth", now), Duration::from_secs(1));
        assert_eq!(l.wait("avatars", now), Duration::ZERO);
        l.blocked = Some(now + Duration::from_secs(120));
        assert_eq!(l.wait("avatars", now), Duration::from_secs(120));
        assert_eq!(retry_after(Some("73")), Duration::from_secs(73));
        assert_eq!(retry_after(Some("Wed, 21 Oct 2015 07:28:00 GMT")), Duration::from_secs(1));
    }
    #[test]
    fn gate_prevents_network_and_cached_data() {
        let (mut c, seen) = mock(vec![]);
        c.avatars.insert("avtr_a".into(), (Instant::now(), Avatar::default()));
        c.friends = Some((Instant::now(), vec![User::default()]));
        c.allowed.revoke();
        assert_eq!(c.sign_in("u", "p"), Err(Error::Disabled));
        assert_eq!(c.avatar("avtr_a"), Err(Error::Disabled));
        assert_eq!(c.friends(), Err(Error::Disabled));
        assert!(c.restore() == Err(Error::Disabled));
        assert!(seen.lock().unwrap().is_empty());
    }
    #[test]
    fn full_two_factor_flow_reuses_cookies_without_repeating_password() {
        for method in [TwoFactor::Totp, TwoFactor::Email, TwoFactor::Recovery] {
            let (mut c, seen) = mock(vec![
                response(
                    200,
                    &format!(
                        r#"{{"requiresTwoFactorAuth":["{}"]}}"#,
                        match method {
                            TwoFactor::Totp => "totp",
                            TwoFactor::Email => "emailOtp",
                            TwoFactor::Recovery => "otp",
                        }
                    ),
                    &["auth=session; HttpOnly"],
                ),
                response(200, r#"{"verified":true}"#, &["twoFactorAuth=factor; HttpOnly"]),
                response(200, r#"{"id":"usr_test","displayName":"Tester"}"#, &[]),
            ]);
            assert!(matches!(c.sign_in("user", "pass"), Ok(Login::TwoFactor(_))));
            assert!(matches!(c.verify(method, "123456"), Ok(Login::SignedIn(_))));
            let seen = seen.lock().unwrap();
            assert!(seen[0].basic.is_some());
            assert!(seen[1].basic.is_none() && seen[2].basic.is_none());
            assert_eq!(seen[1].cookie, "auth=session");
            assert_eq!(seen[2].cookie, "auth=session; twoFactorAuth=factor");
            assert!(seen[1].url.ends_with(&format!("/{}/verify", method.path())));
        }
    }
    #[test]
    fn expired_session_and_rate_limit_are_actionable() {
        let (mut c, _) = mock(vec![response(401, "", &[])]);
        assert_eq!(c.current_user(), Err(Error::Unauthorized));
        let mut r = response(429, "", &[]);
        r.retry_after = Some("20".into());
        let (mut c, seen) = mock(vec![r]);
        assert!(matches!(c.current_user(), Err(Error::RateLimited(_))));
        assert!(matches!(c.current_user(), Err(Error::RateLimited(_))));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
    #[test]
    fn avatars_are_cached_and_untrusted_images_never_get_cookies() {
        let (mut c, seen) = mock(vec![response(200, r#"{"id":"avtr_a","name":"A"}"#, &[])]);
        c.avatar("avtr_a").unwrap();
        c.avatar("avtr_a").unwrap();
        assert_eq!(seen.lock().unwrap().len(), 1);
        for u in ["https://api.orbiters.cc/image", "https://api.vrchat.cloud.evil/a", "https://evil@api.vrchat.cloud/a", "http://api.vrchat.cloud/a"] {
            assert_eq!(c.image(u), Err(Error::UnsafeUrl));
        }
    }
    #[test]
    fn transport_has_no_orbiters_api_or_credential_path() {
        let source = include_str!("vrchat_api.rs").split("#[cfg(test)]").next().unwrap();
        assert!(!source.contains("account::") && !source.contains("crate::account") && !source.contains("Bearer "));
    }
    #[test]
    fn expired_grants_and_revoked_in_flight_requests_stay_disabled() {
        let (mut c, seen) = mock(vec![]);
        c.allowed = Arc::new(Gate::for_duration(Duration::ZERO));
        assert_eq!(c.current_user(), Err(Error::Disabled));
        assert!(seen.lock().unwrap().is_empty());
        struct Revoke(Arc<Gate>);
        impl Transport for Revoke {
            fn send(&self, _: Request) -> Result<Response> {
                self.0.revoke();
                Ok(response(200, r#"{"id":"usr_test","displayName":"Tester"}"#, &["auth=must-not-be-kept"]))
            }
        }
        c.allowed = Arc::new(Gate::for_duration(Duration::from_secs(3600)));
        c.transport = Arc::new(Revoke(c.allowed.clone()));
        assert_eq!(c.sign_in("user", "password"), Err(Error::Disabled));
        assert!(c.cookies.auth.is_empty());
        assert!(!c.signed_in());
    }
    #[test]
    fn wrong_code_keeps_the_challenge_and_unrequested_methods_send_nothing() {
        let (mut c, seen) = mock(vec![
            response(200, r#"{"requiresTwoFactorAuth":["emailOtp"]}"#, &["auth=session"]),
            response(400, "", &[]),
            response(200, r#"{"verified":false}"#, &[]),
        ]);
        c.sign_in("user", "password").unwrap();
        assert_eq!(c.verify(TwoFactor::Totp, "123456"), Err(Error::InvalidResponse));
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert_eq!(c.verify(TwoFactor::Email, "bad"), Err(Error::WrongCode));
        assert_eq!(c.verify(TwoFactor::Email, "bad"), Err(Error::WrongCode));
        assert_eq!(c.cookies.auth, "session");
        assert_eq!(c.pending_methods, [TwoFactor::Email]);
    }
    #[test]
    fn an_expired_second_factor_session_returns_to_signed_out() {
        let (mut c, _) = mock(vec![response(200, r#"{"requiresTwoFactorAuth":["totp"]}"#, &["auth=session"]), response(401, "", &[])]);
        c.sign_in("test", "test").unwrap();
        assert_eq!(c.verify(TwoFactor::Totp, "123456"), Err(Error::Unauthorized));
        assert!(c.cookies.auth.is_empty() && c.pending_methods.is_empty() && !c.signed_in());
    }
    #[test]
    fn a_deleted_auth_cookie_invalidates_the_session_and_cached_data() {
        let (mut c, _) = mock(vec![response(200, r#"{"id":"avtr_a","name":"A"}"#, &["auth=deleted; Max-Age=-1"])]);
        assert_eq!(c.avatar("avtr_a"), Err(Error::Unauthorized));
        assert!(c.cookies.auth.is_empty() && c.avatars.is_empty() && !c.signed_in());
    }
    #[test]
    fn verification_is_not_repeated_after_a_failed_user_lookup() {
        let (mut c, seen) = mock(vec![
            response(200, r#"{"requiresTwoFactorAuth":["otp"]}"#, &["auth=session"]),
            response(200, r#"{"verified":true}"#, &["twoFactorAuth=verified"]),
            response(503, "", &[]),
            response(200, r#"{"id":"usr_test","displayName":"Tester"}"#, &[]),
        ]);
        c.sign_in("user", "password").unwrap();
        assert_eq!(c.verify(TwoFactor::Recovery, "one-use-code"), Err(Error::Unavailable));
        assert!(matches!(c.resume(), Ok(Some(Login::SignedIn(_)))));
        assert_eq!(seen.lock().unwrap().iter().filter(|r| r.method == "POST").count(), 1);
    }
    #[test]
    fn friends_page_both_states_deduplicate_and_cache_for_five_minutes() {
        let first: Vec<_> = (0..100).map(|i| serde_json::json!({"id":format!("usr_{i}"),"displayName":format!("Friend {i}")})).collect();
        let (mut c, seen) = mock(vec![
            response(200, &serde_json::to_string(&first).unwrap(), &[]),
            response(200, "[]", &[]),
            response(200, r#"[{"id":"usr_0","displayName":"Friend 0"},{"id":"usr_offline","displayName":"Offline"}]"#, &[]),
            response(200, "[]", &[]),
            response(200, "[]", &[]),
        ]);
        assert_eq!(c.friends().unwrap().len(), 101);
        assert_eq!(c.friends().unwrap().len(), 101);
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 3);
            assert!(seen[0].url.ends_with("offline=false&n=100&offset=0"));
            assert!(seen[1].url.ends_with("offline=false&n=100&offset=100"));
            assert!(seen[2].url.ends_with("offline=true&n=100&offset=0"));
        }
        c.friends.as_mut().unwrap().0 = Instant::now() - Duration::from_secs(301);
        assert!(c.friends().unwrap().is_empty());
    }
    #[test]
    fn avatar_cache_expires_after_twenty_four_hours() {
        let (mut c, seen) = mock(vec![response(200, r#"{"id":"avtr_a","name":"Before"}"#, &[]), response(200, r#"{"id":"avtr_a","name":"After"}"#, &[])]);
        assert_eq!(c.avatar("avtr_a").unwrap().name, "Before");
        c.avatars.get_mut("avtr_a").unwrap().0 = Instant::now() - Duration::from_secs(86401);
        assert_eq!(c.avatar("avtr_a").unwrap().name, "After");
        assert_eq!(seen.lock().unwrap().len(), 2);
    }
    #[test]
    fn redirects_never_send_cookies_to_another_service_or_absorb_public_cdn_cookies() {
        let mut redirect = response(302, "", &[]);
        redirect.location = Some("https://api.orbiters.cc/image".into());
        let (mut c, seen) = mock(vec![redirect]);
        c.cookies.auth = "private-cookie".into();
        assert_eq!(c.image("https://api.vrchat.cloud/api/1/file/file_a/1/file"), Err(Error::UnsafeUrl));
        assert_eq!(seen.lock().unwrap().len(), 1);
        let mut redirect = response(302, "", &[]);
        redirect.location = Some("https://assets.vrchat.com/image.png".into());
        let (mut c, seen) = mock(vec![redirect, response(200, "image", &["auth=untrusted"])]);
        c.cookies.auth = "private-cookie".into();
        assert_eq!(c.image("https://api.vrchat.cloud/api/1/image/file_a/1/256").unwrap(), b"image");
        assert!(seen.lock().unwrap()[1].cookie.is_empty());
        assert_eq!(c.cookies.auth, "private-cookie");
    }
    #[test]
    fn instances_do_not_confuse_hidden_or_unknown_with_closed() {
        let (mut c, seen) =
            mock(vec![response(200, r#"{"active":true}"#, &[]), response(200, r#"{"active":false}"#, &[]), response(403, "", &[]), response(200, "null", &[])]);
        assert_eq!(c.instance_alive("wrld_a", "123"), Ok(Some(true)));
        assert_eq!(c.instance_alive("wrld_a", "123"), Ok(Some(true)));
        assert_eq!(seen.lock().unwrap().len(), 1);
        c.instances.get_mut(&("wrld_a".into(), "123".into())).unwrap().0 -= Duration::from_secs(61);
        assert_eq!(c.instance_alive("wrld_a", "123"), Ok(Some(false)));
        assert_eq!(c.instance_alive("wrld_a", "hidden"), Ok(None));
        assert_eq!(c.instance_alive("wrld_a", "unknown"), Ok(None));
        c.allowed.revoke();
        assert_eq!(c.instance_alive("wrld_a", "123"), Err(Error::Disabled));
    }
    #[cfg(windows)]
    #[test]
    fn encrypted_store_rotates_restores_and_is_deleted_even_when_logout_fails() {
        let path = std::env::temp_dir().join(format!("orbiters-vrchat-test-{}.bin", uuid::Uuid::new_v4()));
        let (mut c, _) = mock(vec![
            response(200, r#"{"id":"usr_test","displayName":"Tester"}"#, &["auth=initial"]),
            response(200, r#"{"id":"avtr_a","name":"A"}"#, &["auth=rotated"]),
            response(503, "", &[]),
        ]);
        c.store = Some(path.clone());
        c.sign_in("test-user", "never-persist-this-password").unwrap();
        c.avatar("avtr_a").unwrap();
        let encrypted = std::fs::read(&path).unwrap();
        let plain = crate::dpapi::crypt(&encrypted, ENTROPY, false).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&plain).unwrap();
        assert_eq!(v.as_object().unwrap().len(), 2);
        assert_eq!(v["auth"], "rotated");
        assert!(!String::from_utf8_lossy(&plain).contains("never-persist"));
        let (mut restored, seen) = mock(vec![response(200, r#"{"id":"usr_test","displayName":"Tester"}"#, &[])]);
        restored.cookies = Cookies::default();
        restored.user = None;
        restored.store = Some(path.clone());
        restored.resume().unwrap();
        assert_eq!(seen.lock().unwrap()[0].cookie, "auth=rotated");
        assert!(seen.lock().unwrap()[0].basic.is_none());
        assert_eq!(c.sign_out(), Err(Error::Unavailable));
        assert!(!path.exists());
        assert!(!c.signed_in() && c.cookies.header().is_empty());
    }
    #[cfg(windows)]
    #[test]
    fn dpapi_round_trip_and_wrong_entropy() {
        let plain = br#"{"auth":"test-cookie","two_factor_auth":"test-factor"}"#;
        let sealed = crate::dpapi::crypt(plain, ENTROPY, true).unwrap();
        assert_ne!(sealed, plain);
        assert_eq!(crate::dpapi::crypt(&sealed, ENTROPY, false).unwrap(), plain);
        assert!(crate::dpapi::crypt(&sealed, b"other", false).is_err());
    }

    fn unpaced(answers: Vec<Response>) -> (Client, Arc<Mutex<Vec<Request>>>) {
        let (c, seen) = mock(answers);
        c.limiter.lock().unwrap().unpaced = true;
        (c, seen)
    }
    fn json(value: serde_json::Value) -> Response {
        response(200, &value.to_string(), &[])
    }
    const FILE: &str = "https://api.vrchat.cloud/api/1/file/file_0a1b-2c3d";
    const STORAGE: &str = "https://s3.us-east-1.amazonaws.com/files.vrchat.cloud/file_0a1b-2c3d.4.png?X-Amz-Signature=abc";
    const STORAGE_SIGNATURE: &str = "https://s3.us-east-1.amazonaws.com/files.vrchat.cloud/file_0a1b-2c3d.4.sig?X-Amz-Signature=def";
    /// What VRChat answers to replacing avatar `avtr_a`'s picture (version 3 of its file) with a fourth version.
    fn avatar_upload_answers() -> Vec<Response> {
        vec![
            json(serde_json::json!({"id":"avtr_a","name":"Mine","authorId":"usr_test","imageUrl":format!("{FILE}/3/file")})),
            json(
                serde_json::json!({"id":"file_0a1b-2c3d","ownerId":"usr_test","mimeType":"image/png","versions":[{"version":0,"status":"complete"},{"version":3,"status":"complete"}]}),
            ),
            json(
                serde_json::json!({"id":"file_0a1b-2c3d","ownerId":"usr_test","mimeType":"image/png","versions":[{"version":3,"status":"complete"},{"version":4,"status":"waiting"}]}),
            ),
            json(serde_json::json!({ "url": STORAGE })),
            // The storage's own cookie is never taken.
            response(200, "", &["auth=from-storage"]),
            response(200, "{}", &[]),
            json(serde_json::json!({ "url": STORAGE_SIGNATURE })),
            response(200, "", &[]),
            response(200, "{}", &[]),
            json(serde_json::json!({"id":"file_0a1b-2c3d","versions":[{"version":3,"status":"complete"},{"version":4,"status":"complete"}]})),
            json(
                serde_json::json!({"id":"avtr_a","name":"Mine","authorId":"usr_test","imageUrl":format!("{FILE}/4/file"),"thumbnailImageUrl":"https://api.vrchat.cloud/api/1/image/file_0a1b-2c3d/4/256"}),
            ),
        ]
    }
    #[test]
    fn replacing_a_picture_adds_a_version_uploads_both_parts_and_points_the_avatar_at_it() {
        let png = b"not a real PNG, any bytes do: ".repeat(150);
        let (c, seen) = unpaced(avatar_upload_answers());
        let client = Mutex::new(c);
        let mut steps = vec![];
        let replaced = replace_image(&client, "avtr_a", &png, |s| steps.push(s)).unwrap();
        assert_eq!(replaced.image_url, format!("{FILE}/4/file"));
        assert_eq!(replaced.thumbnail_image_url, "https://api.vrchat.cloud/api/1/image/file_0a1b-2c3d/4/256");
        assert_eq!(steps, [Step::Checking, Step::Preparing, Step::Uploading, Step::Signing, Step::Processing, Step::Saving]);
        let seen = seen.lock().unwrap();
        let calls: Vec<String> = seen.iter().map(|r| format!("{} {}", r.method, r.url)).collect();
        assert_eq!(
            calls,
            [
                format!("GET {BASE}/avatars/avtr_a"),
                format!("GET {FILE}"),
                format!("POST {FILE}"),
                format!("PUT {FILE}/4/file/start"),
                format!("PUT {STORAGE}"),
                format!("PUT {FILE}/4/file/finish"),
                format!("PUT {FILE}/4/signature/start"),
                format!("PUT {STORAGE_SIGNATURE}"),
                format!("PUT {FILE}/4/signature/finish"),
                format!("GET {FILE}"),
                format!("PUT {BASE}/avatars/avtr_a"),
            ]
        );
        let body = |i: usize| serde_json::from_str::<serde_json::Value>(seen[i].body.as_deref().unwrap()).unwrap();
        let sig = signature(&png);
        assert_eq!(
            body(2),
            serde_json::json!({"fileMd5": md5_base64(&png), "fileSizeInBytes": png.len(), "signatureMd5": md5_base64(&sig), "signatureSizeInBytes": sig.len()})
        );
        assert_eq!(body(5), serde_json::json!({"maxParts": "0", "nextPartNumber": "0"}));
        assert_eq!(body(8), body(5));
        assert_eq!(body(10), serde_json::json!({"id": "avtr_a", "imageUrl": format!("{FILE}/4/file")}));
        // The storage gets the bytes alone, typed and with their MD5; never a cookie or a credential.
        for (r, bytes, mime) in [(&seen[4], &png[..], "image/png"), (&seen[7], &sig[..], "application/x-rsync-signature")] {
            let u = r.upload.as_ref().unwrap();
            assert!(r.cookie.is_empty() && r.basic.is_none() && r.body.is_none());
            assert_eq!((u.mime, u.md5.as_str(), &u.bytes[..]), (mime, md5_base64(bytes).as_str(), bytes));
        }
        assert!(seen.iter().filter(|r| r.upload.is_none()).all(|r| r.cookie == "auth=mock-session" && r.basic.is_none()));
        let c = client.lock().unwrap();
        assert_eq!(c.cookies.auth, "mock-session");
        assert_eq!(c.avatars["avtr_a"].1.image_url, format!("{FILE}/4/file"));
    }
    #[test]
    fn someone_elses_avatar_is_refused_before_anything_is_sent() {
        let (c, seen) = unpaced(vec![json(serde_json::json!({"id":"avtr_a","name":"Theirs","authorId":"usr_other","imageUrl":format!("{FILE}/1/file")}))]);
        let client = Mutex::new(c);
        assert_eq!(replace_image(&client, "avtr_a", b"picture", |_| {}), Err(Error::NotYours));
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert_eq!(replace_image(&client, "usr_test", b"picture", |_| {}), Err(Error::NotFound));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
    #[test]
    fn a_file_left_unfinished_gets_a_new_file_and_a_foreign_storage_address_gets_nothing() {
        let (c, seen) = unpaced(vec![
            json(serde_json::json!({"id":"wrld_a","name":"Moon","authorId":"usr_test","imageUrl":format!("{FILE}/2/file")})),
            json(serde_json::json!({"id":"file_0a1b-2c3d","ownerId":"usr_test","mimeType":"image/png","versions":[{"version":2,"status":"waiting"}]})),
            json(serde_json::json!({"id":"file_new","ownerId":"usr_test","mimeType":"image/png","versions":[{"version":0,"status":"complete"}]})),
            json(
                serde_json::json!({"id":"file_new","ownerId":"usr_test","mimeType":"image/png","versions":[{"version":0,"status":"complete"},{"version":1,"status":"waiting"}]}),
            ),
            json(serde_json::json!({"url": "https://uploads.example.com/file_new.png"})),
        ]);
        let client = Mutex::new(c);
        assert_eq!(replace_image(&client, "wrld_a", b"picture", |_| {}), Err(Error::UnsafeUrl));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 5);
        assert_eq!(seen[0].url, format!("{BASE}/worlds/wrld_a"));
        assert_eq!((seen[2].method, seen[2].url.as_str()), ("POST", format!("{BASE}/file").as_str()));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(seen[2].body.as_deref().unwrap()).unwrap(),
            serde_json::json!({"name": "World - Moon - Image", "mimeType": "image/png", "extension": ".png"})
        );
        assert_eq!(seen[3].url, format!("{BASE}/file/file_new"));
        assert_eq!(seen[4].url, format!("{BASE}/file/file_new/1/file/start"));
    }
    #[test]
    fn a_revoked_grant_stops_an_upload_at_its_next_step() {
        let (c, seen) = unpaced(avatar_upload_answers());
        let gate = c.allowed.clone();
        let client = Mutex::new(c);
        let result = replace_image(&client, "avtr_a", b"picture", |s| {
            if s == Step::Signing {
                gate.revoke();
            }
        });
        assert_eq!(result, Err(Error::Disabled));
        // The avatar, its file, the version, then the picture's start, upload and finish; the signature never starts.
        assert_eq!(seen.lock().unwrap().len(), 6);
    }
    #[test]
    fn ownership_compares_the_author_with_the_session_and_is_cached() {
        let (mut c, seen) = unpaced(vec![
            json(serde_json::json!({"id":"avtr_a","name":"A","authorId":"usr_test"})),
            json(serde_json::json!({"id":"wrld_b","name":"B","authorId":"usr_other"})),
            response(404, "", &[]),
            json(serde_json::json!({"id":"avtr_c","name":"C"})),
        ]);
        assert_eq!(c.owns("avtr_a"), Ok(true));
        assert_eq!(c.owns("avtr_a"), Ok(true));
        assert_eq!(c.owns("wrld_b"), Ok(false));
        assert_eq!(c.owns("wrld_gone"), Ok(false));
        assert_eq!(c.owns("avtr_c"), Ok(false));
        assert_eq!(c.owns("usr_test"), Ok(false));
        assert_eq!(seen.lock().unwrap().len(), 4);
        c.allowed.revoke();
        assert_eq!(c.owns("avtr_a"), Err(Error::Disabled));
    }
    #[test]
    fn storage_addresses_and_file_ids_are_read_strictly() {
        for ok in [STORAGE, "https://files.vrchat.cloud.s3.amazonaws.com/a.png?sig=1", "https://files.vrchat.cloud/upload/a"] {
            assert!(upload_url_allowed(ok), "{ok}");
        }
        for bad in [
            "http://s3.amazonaws.com/a",
            "https://amazonaws.com.evil.com/a",
            "https://s3.amazonaws.com:8443/a",
            "https://user@s3.amazonaws.com/a",
            "https://api.orbiters.cc/a",
            "not a url",
        ] {
            assert!(!upload_url_allowed(bad), "{bad}");
        }
        assert_eq!(file_id(&format!("{FILE}/3/file")), Some("file_0a1b-2c3d"));
        assert_eq!(file_id("https://api.vrchat.cloud/api/1/image/file_ab-12/1/256?x=1"), Some("file_ab-12"));
        assert_eq!(file_id("https://files.vrchat.cloud/thumbnails/x.png"), None);
        assert_eq!(file_id("https://api.vrchat.cloud/api/1/file/file_../1/file"), None);
    }
    /// A deterministic input, also given to the Python reference that produced the expected values below
    /// (`hashlib.blake2b(digest_size=32)`, `hashlib.md5`, and librsync.net's `Rollsum.Update` and signature writer
    /// transcribed).
    fn pattern(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 + i / 13) as u8).collect()
    }
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
    #[test]
    fn blake2b_256_and_md5_match_the_reference() {
        assert_eq!(hex(&blake2b_256(b"")), "0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8");
        assert_eq!(hex(&blake2b_256(b"abc")), "bddd813c634239723171ef3fee98579b94964e3bb1cb3e427262c8c068d52319");
        assert_eq!(hex(&blake2b_256(&pattern(128))), "e72160789c9e7f92097993129753cf19d63dd5e7e7a752a5900a45d7077a25b9");
        assert_eq!(hex(&blake2b_256(&pattern(129))), "170c01ceb5d6879b5f1ac36b5639dc80b276b79ba74b1db650c3c2b380163eba");
        assert_eq!(hex(&blake2b_256(&pattern(1000))), "a2475af68dc9c424af64e8703b88e465ead7f959ecd11fff50871d8a41f2a981");
        assert_eq!(md5_base64(b""), "1B2M2Y8AsgTpgAmY7PhCfg==");
        assert_eq!(md5_base64(b"abc"), "kAFQmDzST7DWlj99KOF/cg==");
        assert_eq!(md5_base64(b"The quick brown fox jumps over the lazy dog"), "nhB9nTcrtoJr2B01QqQZ1g==");
        // Around the padding's boundaries.
        for (n, md5) in
            [(55, "7jz+ubOTNhC0cycSxUFNyw=="), (56, "29Hc00Nb4aGlImKkspIqPw=="), (64, "rwPnqjfSUQYYOq1oMXGTvw=="), (119, "q1UULBvJk301vBXwUsy6fQ==")]
        {
            assert_eq!(md5_base64(&pattern(n)), md5, "{n} bytes");
        }
        assert_eq!(md5_base64(&pattern(5000)), "ECX7OHPPxsGEXJLtiZPZtQ==");
    }
    #[test]
    fn signatures_have_librsync_nets_layout_and_sums() {
        assert_eq!(rollsum(&pattern(5)), 0x025d00e1);
        assert_eq!(rollsum(&pattern(2048)), 0x6fc6ef29);
        assert_eq!(hex(&signature(&pattern(5))), "727301370000080000000020025d00e19cb9287b8139d67f72ec9d9486d820743163e5f4128f394b8368666aee839b9f");
        // Length and MD5 of whole signatures, for inputs around the block boundaries.
        for (n, len, md5) in [
            (0, 12, "wFTc7oMFDcoZnqdLNiwHBw=="),
            (1, 48, "C/T4GyLdmTdktnl/gji4tQ=="),
            (3, 48, "URpkolLJ8oy/1hyLsJ7OnA=="),
            (4, 48, "O4EYS7ZbPQRXwTPsCL8YTA=="),
            (2048, 48, "kG3LqRw5Sb9Ne853/PwN/w=="),
            (2049, 84, "TtBd/yvRGSZVXHrJ1JCirw=="),
            (5000, 120, "WT6uuGuLWfJdEjj8NSTgHA=="),
        ] {
            let s = signature(&pattern(n));
            assert_eq!((s.len(), md5_base64(&s).as_str()), (len, md5), "{n} bytes");
        }
    }
}
