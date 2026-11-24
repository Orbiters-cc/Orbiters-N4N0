//! The account's avatar favourites, for the avatar gun's picker (contracts `Docs/avatar-gun.md`): its avatar favourite
//! groups (`getFavoriteGroups` with `type=avatar`) and the public avatars of one of them (`getFavoritedAvatars` with
//! `tag` and `releaseStatus=public`), every page. Paths from the community OpenAPI spec (vrchatapi/specification). Every
//! request goes through the client's pacing (one a second per family) and its 429 back-off.
use super::*;

/// One page of `/avatars/favorites` (the spec's largest).
const PAGE: usize = 100;
/// Pages read of one group at most (a VRC+ group holds far fewer).
const PAGES: usize = 10;
/// How long a group's avatars are kept (as the favourite world lists are).
const KEEP: Duration = Duration::from_secs(60);

/// `/avatars/favorites` of the group `tag`, its page at `offset`.
pub fn favorite_avatars_path(tag: &str, offset: usize) -> String {
    format!("/avatars/favorites?tag={}&releaseStatus=public&n={PAGE}&offset={offset}", encode(tag))
}

/// A page of VRChat's avatars: the public ones with an avatar's id (a shape it does not read is left out, never the
/// page), and how many the page held.
fn parse_avatars(body: &[u8]) -> Result<(Vec<Avatar>, usize)> {
    let list: Vec<serde_json::Value> = serde_json::from_slice(body).map_err(|_| Error::InvalidResponse)?;
    let held = list.len();
    let avatars = list
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Avatar>(v).ok())
        .filter(|a| crate::contracts::avatar_gun::is_avatar_id(&a.id) && a.public())
        .map(|mut a| {
            a.name = a.name.trim().to_string();
            a
        })
        .collect();
    Ok((avatars, held))
}

impl Client {
    /// The account's avatar favourite groups (Favorites 1 to 4, more with VRC+): each one's tag (`avatars1`…, what
    /// `/avatars/favorites?tag=` takes) and the name the account gave it, in VRChat's order.
    pub fn favorite_avatar_groups(&mut self) -> Result<Vec<(String, String)>> {
        self.authenticated()?;
        let r = self.get("favorites", "/favorite/groups?type=avatar&n=50")?;
        parse_favorite_groups(&r.body, "avatar")
    }

    /// The public avatars of the avatar favourite group `tag`, in VRChat's order, every page; kept a minute. Each is kept
    /// in the day-long avatar cache too (what [`Client::avatar`] answers).
    pub fn favorite_avatars(&mut self, tag: &str) -> Result<Vec<Avatar>> {
        self.authenticated()?;
        if !(!tag.is_empty() && tag.len() <= 64 && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
            return Err(Error::InvalidResponse);
        }
        if let Some((_, avatars)) = self.avatar_lists.get(tag).filter(|(at, _)| at.elapsed() < KEEP) {
            return Ok(avatars.clone());
        }
        let mut all: Vec<Avatar> = Vec::new();
        for page in 0..PAGES {
            let r = self.get("avatars", &favorite_avatars_path(tag, page * PAGE))?;
            let (avatars, held) = parse_avatars(&r.body)?;
            for a in avatars {
                if !all.iter().any(|b| b.id == a.id) {
                    all.push(a);
                }
            }
            if held < PAGE {
                break;
            }
        }
        if self.avatars.len() + all.len() >= 512 {
            self.avatars.clear();
        }
        for a in &all {
            self.avatars.insert(a.id.clone(), (Instant::now(), a.clone()));
        }
        if self.avatar_lists.len() >= 32 {
            self.avatar_lists.clear();
        }
        self.avatar_lists.insert(tag.to_string(), (Instant::now(), all.clone()));
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct Recorded {
        answers: Mutex<VecDeque<(u16, String)>>,
        seen: Arc<Mutex<Vec<String>>>,
    }
    impl Transport for Recorded {
        fn send(&self, r: Request) -> Result<Response> {
            self.seen.lock().unwrap().push(format!("{} {}", r.method, r.url));
            let (status, body) = self.answers.lock().unwrap().pop_front().expect("unexpected HTTP request");
            Ok(Response { status, body: body.into_bytes(), cookies: vec![], retry_after: Some("7".into()), location: None })
        }
    }
    fn client(answers: Vec<(u16, String)>) -> (Client, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(vec![]));
        let mut c = Client::new(Arc::new(Gate::for_duration(Duration::from_secs(3600))));
        c.transport = Arc::new(Recorded { answers: Mutex::new(answers.into()), seen: seen.clone() });
        c.limiter = Arc::new(Mutex::new(Limiter { unpaced: true, ..Default::default() }));
        c.store = None;
        c.user = Some(User { id: "usr_test".into(), ..Default::default() });
        c.cookies.auth = "recorded".into();
        (c, seen)
    }
    fn id(n: usize) -> String {
        format!("avtr_00000000-0000-4000-8000-{n:012}")
    }
    fn avatar(n: usize, status: &str) -> serde_json::Value {
        serde_json::json!({"id": id(n), "name": format!(" Avatar {n} "), "thumbnailImageUrl": "https://api.vrchat.cloud/api/1/image/file_x/1/256", "imageUrl": "", "authorId": "usr_a", "releaseStatus": status})
    }

    #[test]
    fn the_groups_of_avatars_are_read_from_the_groups_of_every_kind() {
        let body = r#"[{"name":"worlds1","displayName":"Chill","type":"world"},{"name":"avatars1","displayName":"Party","type":"avatar"},{"name":"avatars2","displayName":" ","type":"avatar"},{"name":"bad tag","type":"avatar"}]"#;
        let (mut c, seen) = client(vec![(200, body.into())]);
        assert_eq!(c.favorite_avatar_groups().unwrap(), [("avatars1".to_string(), "Party".to_string()), ("avatars2".to_string(), "avatars2".to_string())]);
        assert_eq!(seen.lock().unwrap().as_slice(), ["GET https://api.vrchat.cloud/api/1/favorite/groups?type=avatar&n=50"]);
    }

    #[test]
    fn a_groups_public_avatars_are_read_page_after_page_and_kept_a_minute() {
        let first: Vec<serde_json::Value> = (0..PAGE).map(|n| avatar(n, if n == 3 { "private" } else { "public" })).collect();
        let mut second = vec![avatar(PAGE, "public"), serde_json::json!({"id": "wrld_x", "name": "a world"}), serde_json::json!({"oops": 1})];
        second.push(avatar(0, "public"));
        let (mut c, seen) = client(vec![(200, serde_json::Value::from(first).to_string()), (200, serde_json::Value::from(second).to_string())]);
        let avatars = c.favorite_avatars("avatars1").unwrap();
        assert_eq!(avatars.len(), PAGE, "private, misshapen and repeated ones left out: {}", avatars.len());
        assert_eq!(avatars[0].name, "Avatar 0", "names trimmed");
        assert!(avatars.iter().all(Avatar::public));
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            [
                "GET https://api.vrchat.cloud/api/1/avatars/favorites?tag=avatars1&releaseStatus=public&n=100&offset=0",
                "GET https://api.vrchat.cloud/api/1/avatars/favorites?tag=avatars1&releaseStatus=public&n=100&offset=100"
            ]
        );
        // Asked again: from what was kept, nothing asked; and each avatar is known (the hit player's check reads it).
        assert_eq!(c.favorite_avatars("avatars1").unwrap().len(), PAGE);
        assert_eq!(c.avatar(&id(5)).unwrap().release_status, "public");
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert!(c.favorite_avatars("../x").is_err(), "a tag that is not a plain word is never sent");
    }

    #[test]
    fn a_rate_limit_is_said_and_holds_every_family() {
        let (mut c, _) = client(vec![(429, "{}".into())]);
        assert_eq!(c.favorite_avatars("avatars1"), Err(Error::RateLimited(Duration::from_secs(7))));
        assert!(matches!(c.favorite_avatar_groups(), Err(Error::RateLimited(_))), "the back-off holds the groups too, with nothing asked");
    }

    #[test]
    fn signed_out_nothing_is_asked() {
        let (mut c, seen) = client(vec![]);
        c.user = None;
        assert!(c.favorite_avatar_groups().is_err() && c.favorite_avatars("avatars1").is_err());
        assert!(seen.lock().unwrap().is_empty());
    }
}
