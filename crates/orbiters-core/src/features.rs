//! Member-scoped feature checks use only the Orbiters client and credential.
use crate::account::Api;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const VRCHAT_LOGIN: &str = "n4n0-vrchat-login";
/// The MCP bridge to VRChat (`docs/vrchat-bridge.md`). No rule names it on the server: owners pass every feature there,
/// everybody else has no rule and is refused, which is what "owners only, for now" means, on production and on
/// development alike. A rule on the server would widen it without a release of the app.
pub const VRCHAT_BRIDGE: &str = "n4n0-vrchat-bridge";
/// N4N0 calls (contracts `Docs/n4n0-calls.md`, "Who has calls"): off for everyone until an administrator switches an
/// account on. Without it Call shows as "Coming soon" and the calls API refuses the account (`not_enabled`).
pub const N4N0_CALLS: &str = "n4n0-calls";
pub const REFRESH: Duration = Duration::from_secs(3 * 60 * 60);
#[derive(Deserialize)]
struct Reply {
    features: HashMap<String, bool>,
}
impl Api {
    pub fn vrchat_login_feature(&self, token: &str) -> anyhow::Result<bool> {
        self.member_feature(token, VRCHAT_LOGIN)
    }

    /// Whether the member `token` belongs to has the feature `name` (a plain name: letters, digits, `-`), as this server
    /// says now. A name the server does not mention is refused.
    pub fn member_feature(&self, token: &str, name: &str) -> anyhow::Result<bool> {
        let response = self.agent().get(&self.url(&format!("features/mine?names={name}"))).header("Authorization", &format!("Bearer {token}")).call()?;
        let reply: Reply = Self::json(response)?;
        Ok(reply.features.get(name).copied().unwrap_or(false))
    }
}
/// Fails closed at expiry. An offline refresh cannot keep an old grant alive indefinitely.
#[derive(Default)]
pub struct FeatureCache {
    checked: Option<Instant>,
    allowed: bool,
}
impl FeatureCache {
    pub fn set(&mut self, allowed: bool) {
        self.checked = Some(Instant::now());
        self.allowed = allowed;
    }
    pub fn due(&self) -> bool {
        self.checked.is_none_or(|t| t.elapsed() >= REFRESH)
    }
    pub fn allowed(&self) -> bool {
        self.allowed && !self.due()
    }
}
/// A feature that keeps being asked for, for something that acts while it is granted: unlike [`FeatureCache`], asked again
/// every `refresh` and good for `lifetime` from the last answer that said yes, so a refusal ends it at the next look, a
/// server out of reach only once the proof is too old, and nothing here survives the process.
#[derive(Debug)]
pub struct Grant {
    refresh: Duration,
    lifetime: Duration,
    /// When the server last said yes.
    proof: Option<Instant>,
    /// When it was last asked, whatever came of it.
    asked: Option<Instant>,
}

impl Grant {
    /// How long to wait after asking and getting no answer.
    const RETRY: Duration = Duration::from_secs(30);

    pub fn new(refresh: Duration, lifetime: Duration) -> Self {
        Self { refresh, lifetime, proof: None, asked: None }
    }

    /// What the server answered, now: yes, no, or (an error) no answer.
    pub fn answered(&mut self, answer: &anyhow::Result<bool>, now: Instant) {
        self.asked = Some(now);
        match answer {
            Ok(true) => self.proof = Some(now),
            Ok(false) => self.proof = None,
            Err(e) => {
                // A credential the server refuses (signed out everywhere, expired) is a no, not "out of reach".
                if e.downcast_ref::<crate::account::ApiError>().is_some_and(|a| matches!(a.status, 401 | 403 | 404 | 425)) {
                    self.proof = None;
                }
            }
        }
    }

    /// Whether to ask the server now.
    pub fn due(&self, now: Instant) -> bool {
        let wait = if self.proof.is_some() { self.refresh } else { Self::RETRY.min(self.refresh) };
        self.asked.is_none_or(|t| now.duration_since(t) >= wait)
    }

    /// Whether the feature is granted now: the server said yes, recently enough.
    pub fn allowed(&self, now: Instant) -> bool {
        self.proof.is_some_and(|t| now.duration_since(t) < self.lifetime)
    }

    /// Back to knowing nothing (another account, another server).
    pub fn forget(&mut self) {
        self.proof = None;
        self.asked = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn feature_is_opt_in_and_expires() {
        let mut c = FeatureCache::default();
        assert!(!c.allowed());
        c.set(true);
        assert!(c.allowed());
        c.checked = Some(Instant::now() - REFRESH);
        assert!(!c.allowed());
        c.set(false);
        assert!(!c.allowed());
    }
    #[test]
    fn a_grant_ends_with_a_refusal_at_once_and_without_an_answer_when_too_old() {
        let (refresh, lifetime) = (Duration::from_secs(300), Duration::from_secs(1800));
        let start = Instant::now();
        let mut grant = Grant::new(refresh, lifetime);
        assert!(!grant.allowed(start), "nothing without an answer");
        assert!(grant.due(start));
        grant.answered(&Ok(true), start);
        assert!(grant.allowed(start));
        assert!(!grant.due(start + Duration::from_secs(10)));
        assert!(grant.due(start + refresh));
        // Out of reach: it stands while the proof is recent, is asked again soon, and goes when the proof is old.
        grant.answered(&Err(anyhow::anyhow!("no route")), start + refresh);
        assert!(grant.allowed(start + refresh));
        assert!(grant.allowed(start + lifetime - Duration::from_secs(1)));
        assert!(!grant.allowed(start + lifetime), "fails closed once the proof is too old");
        // No more: gone at once, whatever the last proof was.
        let mut grant = Grant::new(refresh, lifetime);
        grant.answered(&Ok(true), start);
        grant.answered(&Ok(false), start + Duration::from_secs(1));
        assert!(!grant.allowed(start + Duration::from_secs(1)));
        // The server refusing the credential is a no, not a server out of reach.
        let mut grant = Grant::new(refresh, lifetime);
        grant.answered(&Ok(true), start);
        grant.answered(&Err(crate::account::ApiError { status: 401, message: "expired".into(), code: None }.into()), start + Duration::from_secs(1));
        assert!(!grant.allowed(start + Duration::from_secs(1)));
        // Another account starts from nothing.
        let mut grant = Grant::new(refresh, lifetime);
        grant.answered(&Ok(true), start);
        grant.forget();
        assert!(!grant.allowed(start) && grant.due(start));
    }
    #[test]
    fn checks_the_member_feature_with_only_the_orbiters_credential() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        for (status, body, expected) in [
            (200, r#"{"features":{"n4n0-vrchat-login":true}}"#, Some(true)),
            (200, r#"{"features":{"n4n0-vrchat-login":false}}"#, Some(false)),
            (200, r#"{"features":{}}"#, Some(false)),
            (200, r#"{"features":{"n4n0-vrchat-login":"true"}}"#, None),
            (401, r#"{"message":"expired"}"#, None),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let api = Api::at(&format!("http://{}", listener.local_addr().unwrap()));
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.ends_with(b"\r\n\r\n") {
                    let n = stream.read(&mut buffer).unwrap();
                    assert_ne!(n, 0);
                    request.extend_from_slice(&buffer[..n]);
                }
                let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
                assert!(request.starts_with("get /features/mine?names=n4n0-vrchat-login "));
                assert!(request.contains("authorization: bearer orbiters-test"));
                assert!(!request.contains("cookie:"));
                write!(stream, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            assert_eq!(api.vrchat_login_feature("orbiters-test").ok(), expected);
            server.join().unwrap();
        }
    }
}
