//! An independent VRChat transport. Cookies never enter the Orbiters HTTP agent or API.
//! Blocking methods belong on background threads. No credentials, codes, cookies or response bodies are logged.
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
}
struct Response {
    status: u16,
    cookies: Vec<String>,
    retry_after: Option<String>,
    location: Option<String>,
    body: Vec<u8>,
}
trait Transport: Send {
    fn send(&self, request: Request) -> Result<Response>;
}
struct Http(ureq::Agent);
impl Transport for Http {
    fn send(&self, r: Request) -> Result<Response> {
        // Redirects are handled explicitly below; neither Basic auth nor cookies follow an untrusted redirect.
        let response = match r.method {
            "GET" => {
                let mut q = self.0.get(&r.url);
                if !r.cookie.is_empty() {
                    q = q.header("Cookie", &r.cookie);
                }
                if let Some(basic) = r.basic {
                    q = q.header("Authorization", &basic);
                }
                q.call()
            }
            "POST" => self.0.post(&r.url).header("Cookie", &r.cookie).header("Content-Type", "application/json").send(r.body.unwrap_or_default()),
            "PUT" => self.0.put(&r.url).header("Cookie", &r.cookie).send_empty(),
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
}
impl Limiter {
    fn wait(&self, family: &'static str, now: Instant) -> Duration {
        self.next.get(family).copied().into_iter().chain(self.blocked).max().map_or(Duration::ZERO, |t| t.saturating_duration_since(now))
    }
    fn used(&mut self, family: &'static str, now: Instant) {
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
    transport: Box<dyn Transport>,
    cookies: Cookies,
    allowed: Arc<Gate>,
    limiter: Arc<Mutex<Limiter>>,
    user: Option<User>,
    pending_methods: Vec<TwoFactor>,
    avatars: HashMap<String, (Instant, Avatar)>,
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
            transport: Box::new(Http(agent)),
            cookies: Cookies::default(),
            allowed,
            limiter: LIMITER.get_or_init(Default::default).clone(),
            user: None,
            pending_methods: Vec::new(),
            avatars: HashMap::new(),
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
        let response = self.transport.send(Request { method, url, cookie, basic, body })?;
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
        let r = self.transport.send(Request { method: "PUT", url: format!("{BASE}/logout"), cookie, basic: None, body: None })?;
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
        let a: Avatar = serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)?;
        if self.avatars.len() >= 512 {
            self.avatars.clear();
        }
        self.avatars.insert(id.into(), (Instant::now(), a.clone()));
        Ok(a)
    }
    /// A world's thumbnail address (its own uploads included, public or not).
    pub fn world_thumbnail(&mut self, id: &str) -> Result<String> {
        self.authenticated()?;
        let r = self.get("worlds", &format!("/worlds/{}", encode(id)))?;
        let v: serde_json::Value = serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)?;
        let url = v["thumbnailImageUrl"].as_str().filter(|u| !u.is_empty()).or_else(|| v["imageUrl"].as_str()).unwrap_or_default();
        Ok(url.to_string())
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
        c.transport = Box::new(Mock { answers: Mutex::new(answers.into()), seen: seen.clone() });
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
        c.transport = Box::new(Revoke(c.allowed.clone()));
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
}
