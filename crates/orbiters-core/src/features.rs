//! Member-scoped feature checks use only the Orbiters client and credential.
use crate::account::Api;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const VRCHAT_LOGIN: &str = "n4n0-vrchat-login";
pub const REFRESH: Duration = Duration::from_secs(3 * 60 * 60);
#[derive(Deserialize)]
struct Reply {
    features: HashMap<String, bool>,
}
impl Api {
    pub fn vrchat_login_feature(&self, token: &str) -> anyhow::Result<bool> {
        let response = self.agent().get(&self.url("features/mine?names=n4n0-vrchat-login")).header("Authorization", &format!("Bearer {token}")).call()?;
        let reply: Reply = Self::json(response)?;
        Ok(reply.features.get(VRCHAT_LOGIN).copied().unwrap_or(false))
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
