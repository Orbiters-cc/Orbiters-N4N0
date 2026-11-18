//! The launcher's calls (REQUEST10): friends with everything VRChat says about them, favourite friend groups, profiles,
//! notifications, invite requests, boops, the pipeline's token. Paths and bodies from the community OpenAPI spec
//! (vrchatapi/specification, 2026-11-10): `getFriends`, `getFavoriteGroups`, `getFavorites`, `getPublicProfile`, `getUser`,
//! `getMutuals`, `getNotifications`, `getNotificationV2s`, `deleteNotification` (hide), `requestInvite`, `boop`,
//! `verifyAuthToken`, `getInventoryTemplate`. Every request goes through the client's pacing (one a second per family) and
//! its 429 back-off; the answers are read by [`crate::launcher_feed`].
use super::*;
use serde_json::Value;

fn json(body: &[u8]) -> Result<Value> {
    serde_json::from_slice(body).map_err(|_| Error::InvalidResponse)
}

fn list(body: &[u8]) -> Result<Vec<Value>> {
    match json(body)? {
        Value::Array(items) => Ok(items),
        _ => Err(Error::InvalidResponse),
    }
}

impl Client {
    /// One page (up to 100) of the friends, as VRChat's `LimitedUserFriend`s: `offline=false` gives those in the game
    /// and on the website, `offline=true` the others. The caller pages (`offset` += 100 while a page is full); the
    /// client's pacing spaces the pages a second apart.
    pub fn friends_page(&mut self, offline: bool, offset: usize) -> Result<Vec<Value>> {
        self.authenticated()?;
        let r = self.get("friends", &format!("/auth/user/friends?offline={offline}&n=100&offset={offset}"))?;
        list(&r.body)
    }

    /// The account itself (`/auth/user`, the `CurrentUser`), raw.
    pub fn current_user_value(&mut self) -> Result<Value> {
        self.authenticated()?;
        let r = self.get("auth", "/auth/user")?;
        json(&r.body)
    }

    /// The account's favourite groups of every kind (`/favorite/groups`); [`crate::launcher_feed::favourite_groups`] keeps
    /// the friend ones.
    pub fn favourite_groups_value(&mut self) -> Result<Value> {
        self.authenticated()?;
        let r = self.get("favorites", "/favorite/groups?type=friend&n=50")?;
        json(&r.body)
    }

    /// The account's favourite friends with their groups (`/favorites?type=friend`), every page.
    pub fn favourite_friends_value(&mut self) -> Result<Value> {
        self.authenticated()?;
        let mut all = Vec::new();
        for page in 0..10 {
            let r = self.get("favorites", &format!("/favorites?type=friend&n=100&offset={}", page * 100))?;
            let items = list(&r.body)?;
            let n = items.len();
            all.extend(items);
            if n < 100 {
                break;
            }
        }
        Ok(Value::Array(all))
    }

    /// `/profile/{id}` (`getPublicProfile`): bio, links, languages, badges, banner, decorations.
    pub fn public_profile(&mut self, id: &str) -> Result<Value> {
        self.authenticated()?;
        let r = self.get("users", &format!("/profile/{}", encode(id)))?;
        json(&r.body)
    }

    /// `/users/{id}` (`getUser`): note, date joined, last seen.
    pub fn user_value(&mut self, id: &str) -> Result<Value> {
        self.authenticated()?;
        let r = self.get("users", &format!("/users/{}", encode(id)))?;
        json(&r.body)
    }

    /// `/users/{id}/mutuals` (`getMutuals`): friends and groups in common.
    pub fn mutuals(&mut self, id: &str) -> Result<(u32, u32)> {
        self.authenticated()?;
        let r = self.get("users", &format!("/users/{}/mutuals", encode(id)))?;
        let v = json(&r.body)?;
        let n = |k: &str| v.get(k).and_then(Value::as_u64).map(|n| n.min(u32::MAX as u64) as u32).ok_or(Error::InvalidResponse);
        Ok((n("friends")?, n("groups")?))
    }

    /// The (v1) notifications not hidden: invites, invite requests, friend requests (`getNotifications`). Up to three
    /// pages: on an account with many friend requests waiting, the first hundred can all be those (seen 2026-11-18).
    pub fn notifications(&mut self) -> Result<Vec<Value>> {
        self.authenticated()?;
        let mut all = Vec::new();
        for page in 0..3 {
            let r = self.get("notifications", &format!("/auth/user/notifications?hidden=false&n=100&offset={}", page * 100))?;
            let items = list(&r.body)?;
            let n = items.len();
            all.extend(items);
            if n < 100 {
                break;
            }
        }
        Ok(all)
    }

    /// The newer notifications: boops among them (`getNotificationV2s`, `/notifications`).
    pub fn notifications_v2(&mut self) -> Result<Vec<Value>> {
        self.authenticated()?;
        let r = self.get("notifications", "/notifications?limit=100")?;
        list(&r.body)
    }

    /// Hides a (v1) notification at VRChat: what declining an invite or an invite request here does. Nothing is sent to
    /// the sender.
    pub fn hide_notification(&mut self, id: &str) -> Result<()> {
        self.authenticated()?;
        self.request("notifications", "PUT", format!("{BASE}/auth/user/notifications/{}/hide", encode(id)), None, None).map(drop)
    }

    /// Asks `user` for an invite (`requestInvite`, `POST /requestInvite/{userId}`, body `RequestInviteRequest` with no
    /// message slot).
    pub fn request_invite(&mut self, user: &str) -> Result<()> {
        self.authenticated()?;
        if !crate::vrchat_accounts::is_user_id(user) {
            return Err(Error::NotFound);
        }
        self.request("invites", "POST", format!("{BASE}/requestInvite/{}", encode(user)), None, Some("{}".into())).map(drop)
    }

    /// Boops `user` (`boop`, `POST /users/{userId}/boop`, body `BoopRequest`): with one of VRChat's default emojis
    /// (`emojiId`), or a plain boop (an empty body).
    pub fn boop(&mut self, user: &str, emoji: Option<&str>) -> Result<()> {
        self.authenticated()?;
        if !crate::vrchat_accounts::is_user_id(user) {
            return Err(Error::NotFound);
        }
        let body = boop_body(emoji)?;
        self.request("invites", "POST", format!("{BASE}/users/{}/boop", encode(user)), None, Some(body)).map(drop)
    }

    /// An inventory template's picture (`getInventoryTemplate`): what an `iconFrame` / `nameplateEffect` id shows.
    pub fn inventory_template_picture(&mut self, id: &str) -> Result<String> {
        self.authenticated()?;
        let r = self.get("inventory", &format!("/inventory/template/{}", encode(id)))?;
        let v = json(&r.body)?;
        v.get("imageUrl").and_then(Value::as_str).filter(|u| u.starts_with("https://")).map(str::to_string).ok_or(Error::InvalidResponse)
    }

    /// The token the pipeline takes (`verifyAuthToken`, `GET /auth`): `wss://pipeline.vrchat.cloud/?authToken=<token>`.
    /// Never logged or shown.
    pub fn pipeline_token(&mut self) -> Result<Secret> {
        self.authenticated()?;
        let r = self.get("auth", "/auth")?;
        let v = json(&r.body)?;
        let token = v.get("token").and_then(Value::as_str).filter(|t| !t.is_empty() && v.get("ok").and_then(Value::as_bool) != Some(false));
        token.map(|t| Secret(t.to_string())).ok_or(Error::Unauthorized)
    }
}

/// The boop's body: only VRChat's default emoji ids (`default_…`, `DefaultEmojiID`) are sent; no emoji, `{}`.
pub fn boop_body(emoji: Option<&str>) -> Result<String> {
    match emoji {
        None => Ok("{}".into()),
        Some(e) if e.starts_with("default_") && e.len() <= 64 && e.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') => Ok(serde_json::json!({ "emojiId": e }).to_string()),
        Some(_) => Err(Error::NotFound),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct Recorded {
        answers: Mutex<VecDeque<(u16, String)>>,
        seen: Arc<Mutex<Vec<(String, String, Option<String>)>>>,
    }
    impl Transport for Recorded {
        fn send(&self, r: Request) -> Result<Response> {
            self.seen.lock().unwrap().push((r.method.to_string(), r.url, r.body));
            let (status, body) = self.answers.lock().unwrap().pop_front().expect("unexpected HTTP request");
            Ok(Response { status, body: body.into_bytes(), cookies: vec![], retry_after: Some("7".into()), location: None })
        }
    }
    fn client(answers: &[(u16, &str)]) -> (Client, Arc<Mutex<Vec<(String, String, Option<String>)>>>) {
        let seen = Arc::new(Mutex::new(vec![]));
        let mut c = Client::new(Arc::new(Gate::for_duration(Duration::from_secs(3600))));
        c.transport = Arc::new(Recorded { answers: Mutex::new(answers.iter().map(|(s, b)| (*s, b.to_string())).collect()), seen: seen.clone() });
        c.limiter = Arc::new(Mutex::new(Limiter { unpaced: true, ..Default::default() }));
        c.store = None;
        c.user = Some(User { id: "usr_test".into(), ..Default::default() });
        c.cookies.auth = "recorded".into();
        (c, seen)
    }
    const A: &str = "usr_00000000-0000-4000-8000-000000000001";

    #[test]
    fn reads_go_to_the_specs_paths() {
        let (mut c, seen) = client(&[
            (200, r#"[{"id":"usr_00000000-0000-4000-8000-000000000001","displayName":"N"}]"#),
            (200, r#"{"id":"usr_test","displayName":"Me"}"#),
            (200, r#"[{"name":"group_0","displayName":"best","type":"friend"}]"#),
            (200, r#"[{"type":"friend","favoriteId":"usr_00000000-0000-4000-8000-000000000001","tags":["group_0"]}]"#),
            (200, r#"{"bio":"hi"}"#),
            (200, r#"{"note":"n"}"#),
            (200, r#"{"friends":3,"groups":1}"#),
            (200, r#"[]"#),
            (200, r#"[]"#),
            (200, r#"{"id":"invt_1","imageUrl":"https://api.vrchat.cloud/plate"}"#),
            (200, r#"{"ok":true,"token":"authcookie_x"}"#),
        ]);
        assert_eq!(c.friends_page(false, 0).unwrap().len(), 1);
        assert_eq!(c.current_user_value().unwrap()["displayName"], "Me");
        assert!(c.favourite_groups_value().unwrap().is_array());
        assert_eq!(c.favourite_friends_value().unwrap().as_array().unwrap().len(), 1);
        assert_eq!(c.public_profile(A).unwrap()["bio"], "hi");
        assert_eq!(c.user_value(A).unwrap()["note"], "n");
        assert_eq!(c.mutuals(A).unwrap(), (3, 1));
        assert!(c.notifications().unwrap().is_empty());
        assert!(c.notifications_v2().unwrap().is_empty());
        assert_eq!(c.inventory_template_picture("invt_1").unwrap(), "https://api.vrchat.cloud/plate");
        assert_eq!(c.pipeline_token().unwrap().0, "authcookie_x");
        let paths: Vec<String> = seen.lock().unwrap().iter().map(|(m, u, _)| format!("{m} {}", u.trim_start_matches(BASE))).collect();
        assert_eq!(
            paths,
            [
                "GET /auth/user/friends?offline=false&n=100&offset=0",
                "GET /auth/user",
                "GET /favorite/groups?type=friend&n=50",
                "GET /favorites?type=friend&n=100&offset=0",
                &format!("GET /profile/{A}"),
                &format!("GET /users/{A}"),
                &format!("GET /users/{A}/mutuals"),
                "GET /auth/user/notifications?hidden=false&n=100&offset=0",
                "GET /notifications?limit=100",
                "GET /inventory/template/invt_1",
                "GET /auth",
            ]
        );
    }

    #[test]
    fn sends_go_to_the_specs_paths_with_the_specs_bodies() {
        let (mut c, seen) = client(&[(200, r#"{"id":"not_1","type":"requestInvite"}"#), (200, r#"{"success":{"message":"ok"}}"#), (200, r#"{}"#), (200, r#"{"success":{"message":"ok"}}"#)]);
        c.request_invite(A).unwrap();
        c.boop(A, None).unwrap();
        c.boop(A, Some("default_heart")).unwrap();
        c.hide_notification("not_1").unwrap();
        let seen = seen.lock().unwrap();
        let got: Vec<(String, String, Option<String>)> = seen.iter().map(|(m, u, b)| (m.clone(), u.trim_start_matches(BASE).to_string(), b.clone())).collect();
        assert_eq!(
            got,
            [
                ("POST".into(), format!("/requestInvite/{A}"), Some("{}".into())),
                ("POST".into(), format!("/users/{A}/boop"), Some("{}".into())),
                ("POST".into(), format!("/users/{A}/boop"), Some(r#"{"emojiId":"default_heart"}"#.into())),
                ("PUT".into(), "/auth/user/notifications/not_1/hide".into(), None),
            ]
        );
    }

    #[test]
    fn nothing_odd_is_sent() {
        let (mut c, seen) = client(&[]);
        assert_eq!(c.boop(A, Some("file_123")), Err(Error::NotFound), "uploaded emojis are not offered");
        assert_eq!(c.boop("usr_bad/../x", None), Err(Error::NotFound));
        assert_eq!(c.request_invite("grp_x"), Err(Error::NotFound));
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn errors_come_back_as_the_clients() {
        let (mut c, _) = client(&[(429, "{}"), (400, r#"{"error":{"message":"You are not friends","status_code":400}}"#)]);
        assert!(matches!(c.boop(A, None), Err(Error::RateLimited(_))));
        c.limiter.lock().unwrap().blocked = None;
        assert!(matches!(c.request_invite(A), Err(Error::Refused(400, _))));
        let (mut c, _) = client(&[(200, r#"{"ok":false}"#)]);
        assert_eq!(c.pipeline_token().map(|_| ()), Err(Error::Unauthorized));
    }
}
