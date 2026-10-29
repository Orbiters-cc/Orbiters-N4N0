//! The single routing boundary for VRChat data. Call on background threads.
//! A direct session's cookies are private to `vrchat_api`; the fallback sees only the Orbiters bearer credential.
use crate::account::Api;
use crate::vrchat_api::{Avatar, Client, Error, User};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Source {
    direct: Option<Arc<Mutex<Client>>>,
    orbiters: Option<(Api, String)>,
    log_account: Option<String>,
}
impl Source {
    pub fn new(direct: Option<Arc<Mutex<Client>>>, orbiters: Option<(Api, String)>, log_account: Option<String>) -> Self {
        Self { direct, orbiters, log_account }
    }
    fn direct<T>(&self, f: impl FnOnce(&mut Client) -> crate::vrchat_api::Result<T>) -> Option<anyhow::Result<T>> {
        let mut c = self.direct.as_ref()?.lock().unwrap();
        if !c.signed_in() {
            return None;
        }
        match f(&mut c) {
            Err(Error::Disabled | Error::Unauthorized) => None,
            r => Some(r.map_err(Into::into)),
        }
    }
    pub fn avatar(&self, id: &str) -> anyhow::Result<Option<Avatar>> {
        if let Some(r) = self.direct(|c| c.avatar(id)) {
            return match r {
                Err(e) if e.downcast_ref::<Error>() == Some(&Error::NotFound) => Ok(None),
                r => r.map(Some),
            };
        }
        let Some((api, token)) = &self.orbiters else { return Ok(None) };
        let response = api
            .agent()
            .get(&api.url(&format!("vrchat/avatars/thumbnails?ids={}", crate::vrchat_api::encode(id))))
            .header("Authorization", &format!("Bearer {token}"))
            .call()?;
        let v: serde_json::Value = Api::json(response)?;
        let Some(a) = v.get("avatars").and_then(|v| v.get(id)).filter(|v| !v.is_null()) else { return Ok(None) };
        Ok(Some(Avatar {
            id: id.into(),
            name: a["name"].as_str().unwrap_or_default().into(),
            thumbnail_image_url: a["thumbnailUrl"].as_str().unwrap_or_default().into(),
            image_url: a["imageUrl"].as_str().unwrap_or_default().into(),
        }))
    }
    pub fn friends(&self) -> anyhow::Result<Vec<User>> {
        if let Some(r) = self.direct(Client::friends) {
            return r;
        }
        Ok(crate::vrchat_accounts::friends(self.log_account.as_deref())
            .into_iter()
            .map(|f| User { id: f.id, display_name: f.name, user_icon: f.image.unwrap_or_default(), ..Default::default() })
            .collect())
    }
    pub fn user(&self, id: &str) -> anyhow::Result<Option<User>> {
        self.direct(|c| c.user(id)).transpose()
    }
    /// None preserves the existing unverified link: missing/private access is not proof that an instance closed.
    pub fn instance_alive(&self, world: &str, instance: &str) -> anyhow::Result<Option<bool>> {
        self.direct(|c| c.instance_alive(world, instance)).unwrap_or(Ok(None))
    }
    pub fn current_user(&self) -> anyhow::Result<Option<User>> {
        self.direct(Client::current_user).transpose()
    }
    pub fn image(&self, url: &str) -> anyhow::Result<Vec<u8>> {
        if let Some(r) = self.direct(|c| c.image(url)) {
            return r;
        }
        let (api, _) = self.orbiters.as_ref().ok_or_else(|| anyhow::anyhow!("Sign in to Orbiters to load this image."))?;
        api.image(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn orbiters_fallback_never_receives_vrchat_cookies() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api = Api::at(&format!("http://{}", listener.local_addr().unwrap()));
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut data = [0; 4096];
            let n = s.read(&mut data).unwrap();
            let request = String::from_utf8_lossy(&data[..n]).to_ascii_lowercase();
            assert!(request.contains("authorization: bearer orbit-test"));
            assert!(!request.contains("cookie:"));
            let body = r#"{"avatars":{}}"#;
            write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        // The direct request really starts with a cookie, expires, and then falls back to Orbiters.
        let direct = crate::vrchat_api::tests::expired_session();
        assert!(direct.signed_in());
        assert!(Source::new(Some(Arc::new(Mutex::new(direct))), Some((api, "orbit-test".into())), None).avatar("avtr_test").unwrap().is_none());
        server.join().unwrap();
    }
}
