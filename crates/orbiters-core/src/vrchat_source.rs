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
    pub fn direct_available(&self) -> bool {
        self.direct.as_ref().is_some_and(|c| c.lock().unwrap().signed_in())
    }
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
    /// A world's thumbnail address: VRChat's own answer when signed in (private worlds too), else Orbiters' Map explorer
    /// (public worlds only). `None` when neither knows it.
    pub fn world_thumbnail(&self, id: &str) -> anyhow::Result<Option<String>> {
        if let Some(r) = self.direct(|c| c.world_thumbnail(id)) {
            match r {
                Ok(url) if !url.is_empty() => return Ok(Some(url)),
                Err(e) if e.downcast_ref::<Error>() != Some(&Error::NotFound) => return Err(e),
                _ => {}
            }
        }
        let api = match &self.orbiters {
            Some((api, _)) => api.clone(),
            None => Api::new(crate::account::Server::current()),
        };
        let Some(page) = api.world(id)? else { return Ok(None) };
        Ok(page.world.thumbnail_url.or(page.world.image_url).filter(|u| !u.is_empty()))
    }
    pub fn friends(&self) -> anyhow::Result<Vec<User>> {
        let local = || {
            crate::vrchat_accounts::friends(self.log_account.as_deref())
                .into_iter()
                .map(|f| User { id: f.id, display_name: f.name, user_icon: f.image.unwrap_or_default(), ..Default::default() })
                .collect::<Vec<_>>()
        };
        if let Some(r) = self.direct(Client::friends) {
            return r.map(|direct| merge_friends(direct, local()));
        }
        Ok(local())
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
        match &self.orbiters {
            Some((api, _)) => api.image(url),
            None => Api::new(crate::account::Server::current()).image(url),
        }
    }
}

fn merge_friends(direct: Vec<User>, local: Vec<User>) -> Vec<User> {
    let mut seen = std::collections::HashSet::new();
    let mut users: Vec<_> = direct.into_iter().chain(local).filter(|u| u.id.starts_with("usr_") && seen.insert(u.id.clone())).collect();
    users.sort_by_cached_key(|u| u.display_name.to_lowercase());
    users
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn personal_friends_win_over_local_duplicates() {
        let user = |id: &str, name: &str| User { id: id.into(), display_name: name.into(), ..Default::default() };
        let merged = merge_friends(vec![user("usr_a", "Fresh")], vec![user("usr_a", "Stale"), user("usr_b", "Other"), user("invalid", "Skip")]);
        assert_eq!(merged.iter().map(|u| u.display_name.as_str()).collect::<Vec<_>>(), ["Fresh", "Other"]);
    }
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
