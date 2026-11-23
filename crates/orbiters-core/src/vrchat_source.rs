//! The single routing boundary for VRChat data. Call on background threads.
//! A direct session's cookies are private to `vrchat_api`; the fallback sees only the Orbiters bearer credential.
use crate::account::Api;
use orbiters_i18n::tr;
use crate::vrchat_api::{Avatar, Client, Error, Replaced, Step, User};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Source {
    direct: Option<Arc<Mutex<Client>>>,
    orbiters: Option<(Api, String)>,
    log_account: Option<String>,
    operation: Option<crate::vrchat_api::Operation>,
}
impl Source {
    /// Personal-only operations preserve 401/expiry and rate-limit errors; never fall back to Orbiters.
    pub fn personal<T>(&self, user: &str, current: impl Fn() -> bool, f: impl FnOnce(&mut Client) -> crate::vrchat_api::Result<T>) -> crate::vrchat_api::Result<T> {
        self.personal_until(user, current, std::time::Instant::now() + std::time::Duration::from_secs(60), f)
    }
    pub fn personal_until<T>(&self, user: &str, current: impl Fn() -> bool, deadline: std::time::Instant, f: impl FnOnce(&mut Client) -> crate::vrchat_api::Result<T>) -> crate::vrchat_api::Result<T> {
        // Capture cancellation before contending for the mutex.
        let direct = self.direct.as_ref().ok_or(Error::Unauthorized)?;
        let operation = self.operation.as_ref().copied().ok_or(Error::Disabled)?;
        let mut client = direct.lock().unwrap();
        if !client.signed_in() { return Err(Error::Unauthorized); }
        // Cached authenticated identity, no additional HTTP request.
        if client.signed_in_user_id() != Some(user) { return Err(Error::Unauthorized); }
        if !current() { return Err(Error::Disabled); }
        let result = client.with_operation(operation.with_deadline(deadline), f);
        if !current() { return Err(Error::Disabled); }
        result
    }
    pub fn direct_available(&self) -> bool {
        self.direct.as_ref().is_some_and(|c| c.lock().unwrap().signed_in())
    }
    /// Selection changes the playing account, so a matching log identity is mandatory.
    pub fn select_avatar(&self, id: &str) -> anyhow::Result<()> {
        let playing = self.log_account.as_deref().ok_or(Error::Unauthorized)?;
        self.direct(|c| {
            if c.current_user()?.id != playing { return Err(Error::Unauthorized); }
            c.select_avatar(id)
        }).ok_or(Error::Unauthorized)?
    }
    pub fn new(direct: Option<Arc<Mutex<Client>>>, orbiters: Option<(Api, String)>, log_account: Option<String>) -> Self {
        let operation = direct.as_ref().map(|c| c.lock().unwrap().operation(std::time::Instant::now() + std::time::Duration::from_secs(60)));
        Self { direct, orbiters, log_account, operation }
    }
    pub fn scoped(direct: Option<Arc<Mutex<Client>>>, orbiters: Option<(Api, String)>, log_account: Option<String>, operation: crate::vrchat_api::Operation) -> Self {
        Self { direct, orbiters, log_account, operation: Some(operation) }
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
            ..Default::default()
        }))
    }
    /// The avatars and worlds among `ids` the signed-in VRChat account uploaded (VRChat's answers kept a day). None
    /// without a personal session: Orbiters' fallback cannot tell.
    pub fn owned(&self, ids: &[String]) -> anyhow::Result<Vec<String>> {
        let mut owned = Vec::new();
        for id in ids {
            match self.direct(|c| c.owns(id)) {
                None => return Ok(Vec::new()),
                Some(Ok(true)) => owned.push(id.clone()),
                Some(Ok(false)) => {}
                Some(Err(e)) => return Err(e),
            }
        }
        Ok(owned)
    }
    /// Replaces the picture of an avatar or world the signed-in VRChat account uploaded
    /// ([`crate::vrchat_api::replace_image`]). A change has no fallback: it needs the personal session.
    pub fn replace_image(&self, id: &str, png: &[u8], progress: impl FnMut(Step)) -> anyhow::Result<Replaced> {
        let Some(client) = self.direct.as_ref().filter(|c| c.lock().unwrap().signed_in()) else {
            anyhow::bail!("{}", tr("Sign in to VRChat in Settings to change pictures."))
        };
        Ok(crate::vrchat_api::replace_image(client, id, png, progress)?)
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
    /// A page of a world list: VRChat's own answer through the personal session when signed in, else Orbiters' public
    /// Map explorer (a search, the featured worlds; the lists only VRChat has say to sign in).
    pub fn worlds(&self, list: &crate::maps::WorldList, page: usize) -> anyhow::Result<crate::maps::FoundWorlds> {
        use crate::maps::{FoundBy, FoundWorlds};
        if let Some(r) = self.direct(|c| c.find_worlds(list, page)) {
            return r.map(|(items, more)| FoundWorlds { items, more, source: FoundBy::Vrchat });
        }
        let api = match &self.orbiters {
            Some((api, _)) => api.clone(),
            None => Api::new(crate::account::Server::current()),
        };
        api.find_worlds(list, page)
    }
    /// The signed-in VRChat account's world playlists (favourite groups): tag and name. Needs the personal session.
    pub fn favorite_world_groups(&self) -> anyhow::Result<Vec<(String, String)>> {
        match self.direct(|c| c.favorite_world_groups()) {
            Some(r) => r,
            None => Err(Error::Unauthorized.into()),
        }
    }
    /// A new private instance of `world`, owned by the signed-in account. Needs the personal session.
    pub fn create_private_instance(&self, world: &str, region: &str) -> anyhow::Result<String> {
        match self.direct(|c| c.create_private_instance(world, region)) {
            Some(r) => r,
            None => Err(Error::Unauthorized.into()),
        }
    }
    /// An invite to oneself to `world`'s instance `instance`, shown in VRChat. Needs the personal session.
    pub fn invite_self(&self, world: &str, instance: &str) -> anyhow::Result<()> {
        match self.direct(|c| c.invite_self(world, instance)) {
            Some(r) => r,
            None => anyhow::bail!("{}", tr("Sign in to VRChat in Orbiters - N4N0's Settings to be invited.")),
        }
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
    /// [`Self::instance_alive`] as VRChat said it `at_most` ago, and how long ago that was.
    pub fn instance_seen(&self, world: &str, instance: &str, at_most: std::time::Duration) -> anyhow::Result<(Option<bool>, std::time::Duration)> {
        self.direct(|c| c.instance_seen(world, instance, at_most)).unwrap_or(Ok((None, std::time::Duration::ZERO)))
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

/// The signed-in account's friends as VRChat listed them, a picture VRCX or VRCNext kept filling one VRChat left
/// empty. Nobody is added from them: what they hold may be another account's friends on this computer (another VRChat
/// account, one VRCX last used, a profile VRCNext only looked at), or people unfriended since.
fn merge_friends(direct: Vec<User>, local: Vec<User>) -> Vec<User> {
    let pictures: std::collections::HashMap<String, String> = local.into_iter().filter(|u| !u.user_icon.is_empty()).map(|u| (u.id, u.user_icon)).collect();
    let mut seen = std::collections::HashSet::new();
    let mut users: Vec<_> = direct.into_iter().filter(|u| u.id.starts_with("usr_") && seen.insert(u.id.clone())).collect();
    for u in &mut users {
        if u.user_icon.is_empty() {
            if let Some(picture) = pictures.get(&u.id) {
                u.user_icon = picture.clone();
            }
        }
    }
    users.sort_by_cached_key(|u| u.display_name.to_lowercase());
    users
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn personal_friends_win_over_local_duplicates() {
        let user = |id: &str, name: &str| User { id: id.into(), display_name: name.into(), ..Default::default() };
        let pictured = User { user_icon: "https://api.vrchat.cloud/a".into(), ..user("usr_a", "Stale") };
        let merged = merge_friends(vec![user("usr_a", "Fresh")], vec![pictured, user("usr_b", "Other"), user("invalid", "Skip")]);
        // CORE2-6: with a VRChat sign-in the list is that account's friends; the local apps only fill a picture.
        assert_eq!(merged.iter().map(|u| u.display_name.as_str()).collect::<Vec<_>>(), ["Fresh"], "usr_b may be another account's friend");
        assert_eq!(merged[0].user_icon, "https://api.vrchat.cloud/a");
    }
    #[test]
    fn world_search_without_a_session_asks_orbiters_map_explorer() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api = Api::at(&format!("http://{}", listener.local_addr().unwrap()));
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut data = [0; 4096];
            let n = s.read(&mut data).unwrap();
            let request = String::from_utf8_lossy(&data[..n]).to_string();
            assert!(request.starts_with("GET /maps/search?q=black%20cat&page=1 "), "{request}");
            assert!(!request.to_ascii_lowercase().contains("cookie:"));
            let body = r#"{"items":[{"worldId":"wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd","name":"The Black Cat","authorName":"spooky",
                "thumbnailUrl":"https://api.vrchat.cloud/x/256","capacity":40,"tags":["chill"],"preview":{"id":3},"pictures":4,
                "performance":{"fps":71.6,"fps_low":40,"ratio":0.8}},{"name":"no id"}],"hasMore":true,"source":"vrchat"}"#;
            write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        // An expired personal session falls back too.
        let direct = crate::vrchat_api::tests::expired_session();
        let source = Source::new(Some(Arc::new(Mutex::new(direct))), Some((api, "orbit-test".into())), None);
        let found = source.worlds(&crate::maps::WorldList::Search("black cat".into()), 1).unwrap();
        server.join().unwrap();
        assert_eq!(found.source, crate::maps::FoundBy::Orbiters);
        assert!(found.more);
        assert_eq!(found.items.len(), 1, "an item without its id is left out");
        let w = &found.items[0];
        assert!(w.preview);
        assert_eq!((w.pictures, w.world.capacity, w.world.performance.as_ref().map(|p| p.fps)), (4, Some(40), Some(72)));
        // The lists only VRChat has need its session; no request is sent.
        let offline = Source::new(None, Some((Api::at("http://127.0.0.1:9"), "t".into())), None);
        assert!(offline.worlds(&crate::maps::WorldList::Favorites, 0).unwrap_err().to_string().contains("Sign in to VRChat"));
        assert!(offline.invite_self("wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd", "1").is_err());
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
